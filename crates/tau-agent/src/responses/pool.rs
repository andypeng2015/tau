//! WebSocket connection pool for the Codex Responses backend.
//!
//! See `TODO-codex-websocket.md` §2 for the design rationale. Recap:
//!
//! - The agent's `run()` loop is single-threaded and processes prompts
//!   serially, but it *alternates* between conversations (different sessions,
//!   sub-agent delegations interleaved with the parent). The OpenAI WS endpoint
//!   only caches the *most recent* `previous_response_id` per socket, so
//!   routing A → B → A on one shared socket would flush each chain's warmth on
//!   every switch. Keep one connection per `(account, session)` so warmth
//!   survives context-switches.
//! - Single owner = the agent loop. No `Mutex`/`Arc`/`DashMap`. Take the
//!   connection *out* of the map for the duration of one turn
//!   (`HashMap::remove`), put it back on success. Connection-in-flight
//!   exclusivity is enforced by ownership.
//! - Bounded by a soft cap (env-tunable `TAU_WS_POOL_MAX`,
//!   [`DEFAULT_POOL_MAX`]). LRU eviction when full.
//! - Connections age out near the server's 60-minute hard cap so a call doesn't
//!   fail mid-turn from the server slamming the door.
//! - Bearer-mismatch on checkout means OAuth refreshed; drop the stale socket
//!   and open a new one.

#![allow(dead_code)] // wired up by the agent's `run()` loop in a later step

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use super::ResponsesConfig;
use super::ws::WsConn;
use crate::common::LlmError;

/// Default soft cap on simultaneously-cached WS connections.
///
/// One per `(account, session)`. A typical interactive workload runs
/// 1–3 active sessions (the user's main + any in-flight sub-agent
/// delegation). The cap exists to bound pathological growth (a
/// long-lived agent process where the user reopens many old
/// sessions), not because the normal path needs many slots.
pub(crate) const DEFAULT_POOL_MAX: usize = 10;

/// Environment variable that overrides [`DEFAULT_POOL_MAX`] at
/// `WsPool::new()` time.
pub(crate) const POOL_MAX_ENV: &str = "TAU_WS_POOL_MAX";

/// Margin under the server's 60-minute hard cap before we
/// pre-emptively reopen a connection on checkout. Five minutes is
/// safer than cutting it close — a 59-minute-old connection that
/// dies *after* we send `response.create` surfaces as a mid-stream
/// `stream error` to the user, which a `<55min ? reuse : reopen`
/// check avoids entirely.
pub(crate) const MAX_CONNECTION_AGE: Duration = Duration::from_secs(55 * 60);

/// Pool key. A connection caches the previous_response of one
/// conversation chain; different chains get different sockets so
/// alternating between them preserves each chain's warm cache.
///
/// - `base_url` + `account_id` form a "socket realm" — same bearer, same
///   server-side state. Cross-realm reuse is impossible.
/// - `session_id` is the harness's per-conversation identifier. The harness
///   stamps it on every `SessionPromptCreated`; sub-agent delegations get their
///   own session_id and therefore their own slot.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PoolKey {
    pub base_url: String,
    pub account_id: Option<String>,
    pub session_id: String,
}

impl PoolKey {
    pub fn for_request(config: &ResponsesConfig, session_id: &str) -> Self {
        Self {
            base_url: config.base_url.clone(),
            account_id: config.account_id.clone(),
            session_id: session_id.to_owned(),
        }
    }
}

/// Single-threaded pool of WS connections.
///
/// Hot path (turn N+1 on a known session): `checkout` returns the
/// existing `WsConn` (removed from the map); the caller runs the
/// turn; on success it calls `release` to put the conn back at the
/// head of the LRU queue. On error (mid-stream close, IO break),
/// the caller drops the connection — the entry is already removed
/// from the map and the LRU list resyncs lazily.
pub(crate) struct WsPool {
    conns: HashMap<PoolKey, WsConn>,
    /// Front = most recent. Pruned of stale keys on `release` /
    /// `checkout` rather than eagerly — a key in the queue without
    /// a matching map entry just means that connection died and was
    /// dropped, so we skip it next time we walk the queue.
    lru: VecDeque<PoolKey>,
    max: usize,
}

impl WsPool {
    pub fn new() -> Self {
        let max = std::env::var(POOL_MAX_ENV)
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_POOL_MAX);
        Self {
            conns: HashMap::new(),
            lru: VecDeque::new(),
            max,
        }
    }

    /// Look up an existing connection for `key`, validating its
    /// bearer/age against the current request. Returns:
    ///
    /// - `Some(conn)` — caller owns it for the turn, must call
    ///   [`Self::release`] on success or drop on failure.
    /// - `None` — pool miss. Caller should `connect()` a fresh `WsConn` and
    ///   insert it via [`Self::release`] after the turn.
    ///
    /// Drops the entry if its bearer has rotated (OAuth refresh) or
    /// the connection is approaching the server-side age limit.
    pub fn checkout(&mut self, key: &PoolKey, current_bearer: &str) -> Option<WsConn> {
        let conn = self.conns.remove(key)?;
        // Bearer rotation: refreshed access token means upstream
        // would reject the existing socket on the next message
        // anyway. Drop and let caller reopen with the new token.
        if conn.bearer != current_bearer {
            self.purge_key(key);
            return None;
        }
        // Age-out: a 59-minute-old socket would die mid-stream.
        // Reopen here instead, before sending anything.
        if conn.opened_at.elapsed() >= MAX_CONNECTION_AGE {
            self.purge_key(key);
            return None;
        }
        // LRU bookkeeping: take the key out — caller will put it
        // back at the front on `release`.
        self.lru.retain(|k| k != key);
        Some(conn)
    }

    /// Put a connection (newly opened or just-used) back into the
    /// pool. Inserts at the LRU front. Evicts the LRU tail when the
    /// pool was already at capacity.
    pub fn release(&mut self, key: PoolKey, conn: WsConn) {
        if self.conns.len() >= self.max && !self.conns.contains_key(&key) {
            self.evict_lru();
        }
        // Lazy-prune: if a stale copy of this key is somewhere in
        // the queue (e.g. it was age-purged earlier), drop it so we
        // don't double-count.
        self.lru.retain(|k| k != &key);
        self.lru.push_front(key.clone());
        self.conns.insert(key, conn);
    }

    /// Drop every cached connection. Cheap full reset — used when
    /// the resolver issues a token refresh and we want to invalidate
    /// every socket in one shot (alternative: per-entry bearer check
    /// on checkout, which is what [`Self::checkout`] does today).
    pub fn flush(&mut self) {
        self.conns.clear();
        self.lru.clear();
    }

    pub fn len(&self) -> usize {
        self.conns.len()
    }

    fn purge_key(&mut self, key: &PoolKey) {
        self.conns.remove(key);
        self.lru.retain(|k| k != key);
    }

    fn evict_lru(&mut self) {
        // Walk the LRU tail forward until we find a key still
        // backed by the map. Stale keys (entry removed earlier
        // without queue update) are silently skipped.
        while let Some(stale) = self.lru.pop_back() {
            if self.conns.remove(&stale).is_some() {
                return;
            }
        }
    }
}

impl Default for WsPool {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience wrapper that wires `checkout` → `WsConn::run_turn` →
/// `release` together with reopen-on-miss semantics. The agent's
/// `run()` loop calls this; tests can call it directly with a fake
/// `WsConn::connect` impl by exercising the lower-level methods.
pub(crate) fn run_turn_through_pool(
    pool: &mut WsPool,
    config: &ResponsesConfig,
    session_id: &str,
    request: &crate::common::PromptPayload<'_>,
    on_update: &mut impl FnMut(&str, Option<&str>),
) -> Result<crate::common::StreamState, LlmError> {
    let key = PoolKey::for_request(config, session_id);
    let mut conn = match pool.checkout(&key, &config.api_key) {
        Some(c) => c,
        None => WsConn::connect(config)?,
    };
    match conn.run_turn(config, request, on_update) {
        Ok(state) => {
            pool.release(key, conn);
            Ok(state)
        }
        Err(err) => {
            // Connection state may be poisoned. Drop it. The next
            // turn on this session will reopen.
            drop(conn);
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::thread;

    use tungstenite::Message;

    use super::*;
    use crate::common::PromptPayload;

    #[test]
    fn keys_distinguish_sessions_under_same_account() {
        let cfg = make_config("https://chatgpt.com/backend-api", Some("acc"));
        let a = PoolKey::for_request(&cfg, "session-a");
        let b = PoolKey::for_request(&cfg, "session-b");
        assert_ne!(a, b);
    }

    #[test]
    fn keys_distinguish_accounts_under_same_session() {
        let a = PoolKey::for_request(
            &make_config("https://chatgpt.com/backend-api", Some("acc-1")),
            "session",
        );
        let b = PoolKey::for_request(
            &make_config("https://chatgpt.com/backend-api", Some("acc-2")),
            "session",
        );
        assert_ne!(a, b);
    }

    /// The headline pool invariant: alternating between two sessions
    /// must NOT cause the second session's turn to flush the first
    /// session's connection. Each `(account, session)` must hold its
    /// own socket so the OpenAI connection-local
    /// `previous_response_id` cache stays warm across context
    /// switches.
    #[test]
    fn pool_routes_each_session_to_its_own_socket_and_reuses_them() {
        let (addr, server) = spawn_fake_codex_server();
        let config = make_config(&format!("http://{addr}/backend-api"), Some("acc"));
        let mut pool = WsPool::new();
        let mut on_update = |_: &str, _: Option<&str>| {};

        // Two turns on session A, interleaved with one on session B.
        // Expected: 2 upgrades total (one per session), 3 turns.
        for session in ["session-a", "session-b", "session-a"] {
            let session_id = tau_proto::SessionId::new(session);
            let request = PromptPayload {
                system_prompt: "sys",
                messages: &[],
                tools: &[],
                params: tau_proto::ModelParams::default(),
                previous_response: None,
                originator: &tau_proto::PromptOriginator::User,
                session_id: &session_id,
            };
            run_turn_through_pool(&mut pool, &config, session, &request, &mut on_update)
                .expect("turn ok");
        }

        let state = server.lock().unwrap();
        assert_eq!(
            state.upgrade_count, 2,
            "expected one upgrade per distinct session_id (alternating A/B/A — reuses A's socket)"
        );
        assert_eq!(
            state.turns_per_connection,
            vec![2, 1],
            "session-a's socket should have served two turns; session-b's, one"
        );
    }

    /// Cap the pool at 2 and exercise three sessions. The
    /// least-recently-used session's socket must get evicted; a
    /// follow-up turn on that session triggers a fresh upgrade.
    #[test]
    fn pool_evicts_lru_when_capacity_exceeded() {
        let (addr, server) = spawn_fake_codex_server();
        let config = make_config(&format!("http://{addr}/backend-api"), Some("acc"));
        let mut pool = WsPool::new();
        pool.max = 2;
        let mut on_update = |_: &str, _: Option<&str>| {};

        // A → B → C: three different sessions, cap=2.
        // After C: A (LRU) is evicted, pool holds {B, C}.
        for session in ["a", "b", "c"] {
            run_turn(&mut pool, &config, session, &mut on_update);
        }
        assert_eq!(pool.len(), 2);
        assert_eq!(server.lock().unwrap().upgrade_count, 3);

        // Touching A again must re-upgrade (its old socket got
        // evicted on C's release).
        run_turn(&mut pool, &config, "a", &mut on_update);
        assert_eq!(server.lock().unwrap().upgrade_count, 4);
    }

    /// Connections older than `MAX_CONNECTION_AGE` must be
    /// pre-emptively reopened on checkout, so the server's 60-min
    /// hard cap never fires mid-turn.
    #[test]
    fn pool_reopens_aged_out_connections_on_checkout() {
        let (addr, server) = spawn_fake_codex_server();
        let config = make_config(&format!("http://{addr}/backend-api"), Some("acc"));
        let mut pool = WsPool::new();
        let mut on_update = |_: &str, _: Option<&str>| {};

        // First turn opens connection #1.
        run_turn(&mut pool, &config, "session-aged", &mut on_update);
        assert_eq!(server.lock().unwrap().upgrade_count, 1);

        // Forcibly age the cached connection past the threshold.
        let key = PoolKey::for_request(&config, "session-aged");
        if let Some(conn) = pool.conns.get_mut(&key) {
            conn.opened_at =
                std::time::Instant::now() - MAX_CONNECTION_AGE - Duration::from_secs(1);
        } else {
            panic!("expected connection in pool");
        }

        // Next turn must reopen rather than send on the stale socket.
        run_turn(&mut pool, &config, "session-aged", &mut on_update);
        assert_eq!(
            server.lock().unwrap().upgrade_count,
            2,
            "aged-out connection should have been replaced"
        );
    }

    /// HTTP+SSE base + plain TCP fake server doubles as the WS
    /// transport's smoke test: connect, send a turn, read all the
    /// expected events back, see `response_id` captured.
    #[test]
    fn ws_turn_captures_response_id_for_chain_continuation() {
        let (addr, _server) = spawn_fake_codex_server();
        let config = make_config(&format!("http://{addr}/backend-api"), Some("acc"));
        let mut pool = WsPool::new();
        let mut last_text = String::new();
        let mut on_update = |text: &str, _thinking: Option<&str>| {
            last_text = text.to_owned();
        };

        let session_id = tau_proto::SessionId::new("session-x");
        let request = PromptPayload {
            system_prompt: "sys",
            messages: &[],
            tools: &[],
            params: tau_proto::ModelParams::default(),
            previous_response: None,
            originator: &tau_proto::PromptOriginator::User,
            session_id: &session_id,
        };

        let state =
            run_turn_through_pool(&mut pool, &config, "session-x", &request, &mut on_update)
                .expect("turn ok");
        assert_eq!(last_text, "hello");
        assert!(
            state.response_id.is_some(),
            "response_id must be captured so the next turn can chain via previous_response_id"
        );
    }

    // -----------------------------------------------------------------
    // Fake Codex server: minimal blocking tungstenite acceptor.
    // -----------------------------------------------------------------

    #[derive(Default)]
    struct ServerState {
        /// How many TCP+upgrade pairs we've accepted. Each
        /// `(account, session)` pair the pool keys against should
        /// produce exactly one upgrade across its lifetime (modulo
        /// age-out / OAuth refresh).
        upgrade_count: usize,
        /// `turns_per_connection[i]` is the number of
        /// `response.create` envelopes connection `i` served before
        /// closing. Lets pool-reuse tests assert that A's two turns
        /// landed on one socket.
        turns_per_connection: Vec<usize>,
        /// Captured request bodies, in arrival order across all
        /// connections. Available for tests that want to inspect
        /// what the client actually sent (chain ids, model knobs).
        #[allow(dead_code)]
        requests: Vec<serde_json::Value>,
    }

    fn spawn_fake_codex_server() -> (SocketAddr, Arc<Mutex<ServerState>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let state = Arc::new(Mutex::new(ServerState::default()));
        let state_clone = state.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let conn_state = state_clone.clone();
                thread::spawn(move || handle_one_connection(stream, conn_state));
            }
        });
        (addr, state)
    }

    fn handle_one_connection(stream: TcpStream, state: Arc<Mutex<ServerState>>) {
        let mut ws = match tungstenite::accept(stream) {
            Ok(ws) => ws,
            Err(_) => return,
        };
        let conn_idx;
        {
            let mut s = state.lock().unwrap();
            s.upgrade_count += 1;
            conn_idx = s.turns_per_connection.len();
            s.turns_per_connection.push(0);
        }

        let mut turn_counter = 0_usize;
        loop {
            let msg = match ws.read() {
                Ok(m) => m,
                Err(_) => return,
            };
            match msg {
                Message::Text(text) => {
                    let parsed: serde_json::Value =
                        serde_json::from_str(text.as_str()).unwrap_or(serde_json::Value::Null);
                    {
                        let mut s = state.lock().unwrap();
                        s.requests.push(parsed.clone());
                        s.turns_per_connection[conn_idx] += 1;
                    }
                    turn_counter += 1;
                    // Stream a tiny canned event sequence: one
                    // visible-text delta, then completed.
                    let events = [
                        serde_json::json!({
                            "type": "response.output_text.delta",
                            "delta": "hello",
                        }),
                        serde_json::json!({
                            "type": "response.completed",
                            "response": {
                                "id": format!("resp_{conn_idx}_{turn_counter}"),
                                "usage": {
                                    "input_tokens": 1,
                                    "output_tokens": 1,
                                    "input_tokens_details": { "cached_tokens": 0 },
                                },
                            },
                        }),
                    ];
                    for ev in events {
                        let txt = serde_json::to_string(&ev).expect("serialize");
                        if ws.send(Message::Text(txt.into())).is_err() {
                            return;
                        }
                    }
                }
                Message::Close(_) => return,
                _ => continue,
            }
        }
    }

    fn run_turn(
        pool: &mut WsPool,
        config: &ResponsesConfig,
        session: &str,
        on_update: &mut impl FnMut(&str, Option<&str>),
    ) {
        let session_id = tau_proto::SessionId::new(session);
        let request = PromptPayload {
            system_prompt: "sys",
            messages: &[],
            tools: &[],
            params: tau_proto::ModelParams::default(),
            previous_response: None,
            originator: &tau_proto::PromptOriginator::User,
            session_id: &session_id,
        };
        run_turn_through_pool(pool, config, session, &request, on_update).expect("turn ok");
    }

    fn make_config(base_url: &str, account_id: Option<&str>) -> ResponsesConfig {
        ResponsesConfig {
            base_url: base_url.into(),
            api_key: "test".into(),
            model_id: "gpt-5-codex".into(),
            account_id: account_id.map(str::to_owned),
            supports_reasoning_effort: false,
            supports_reasoning_summary: false,
            supports_verbosity: false,
            supports_phase: false,
            supports_websocket: true,
            prompt_cache_key: None,
            prompt_cache_retention: None,
        }
    }
}
