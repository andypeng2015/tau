//! Persistent-WebSocket transport for the Codex Responses API.
//!
//! The agent owns a small pool of these connections, keyed by
//! `(base_url, account_id, session_id)`, so the connection-local
//! `previous_response_id` cache stays warm across turns of the same
//! conversation. The pool itself lives in [`pool`]; this module
//! handles a single connection's lifecycle and one-turn streaming.
//!
//! Wire shape:
//! - Upgrade `wss://{base_url}/codex/responses` (same path as the HTTP+SSE
//!   endpoint, just `wss://`) with `Authorization`, `chatgpt-account-id`, and
//!   the dated `OpenAI-Beta: responses_websockets=2026-02-06` header.
//! - Send one client text frame per turn: a `{ "type": "response.create", ...
//!   }` envelope produced by [`super::build_ws_envelope`].
//! - Read server text frames as one decoded `response.*` event each and hand
//!   them to [`super::apply_event`]. Same event shape as SSE (the WS guide is
//!   explicit on this).
//! - On `response.completed`/`response.done` the connection stays open and idle
//!   for the next turn.

// Dead-code-allow until `pool` + the agent's `run()` loop wire this
// in. Without it cargo errors on the unused helpers (the crate
// promotes `unused` to deny).
#![allow(dead_code)]

use std::net::{Shutdown, TcpStream};
use std::time::Instant;

use tungstenite::client::IntoClientRequest;
use tungstenite::handshake::client::Request;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use super::{ResponsesConfig, apply_event, build_ws_envelope};
use crate::common::{LlmError, PromptPayload, StreamState};

/// Beta-feature header value the OpenAI WebSocket endpoint expects.
/// Dated by the server; will need a bump when OpenAI rolls a new
/// release. Pinned here as a single `const` so that bump is a
/// one-line change.
pub(crate) const OPENAI_BETA_WS: &str = "responses_websockets=2026-02-06";

/// One live WS connection to a Responses endpoint.
///
/// Single-threaded: the agent's main loop owns the pool, takes a
/// `WsConn` out for the duration of one turn, then puts it back. No
/// internal locking.
pub(crate) struct WsConn {
    stream: WebSocket<MaybeTlsStream<TcpStream>>,
    /// Wall-clock time of the upgrade. Used by the pool to retire
    /// connections before the server's 60-minute hard cap fires
    /// mid-turn.
    pub opened_at: Instant,
    /// Bearer token the upgrade was authenticated with. The pool
    /// compares against the current resolved token on checkout — a
    /// mismatch means OAuth refreshed and this socket's auth is
    /// stale, so it gets dropped and reopened.
    pub bearer: String,
}

impl WsConn {
    /// Open a fresh connection and perform the WS upgrade.
    ///
    /// Errors:
    /// - `LlmError::HttpStatus(426, _)` — server rejected the upgrade (sticky
    ///   fallback to HTTP+SSE).
    /// - `LlmError::HttpStatus(0, "stream error: ...")` — transient transport
    ///   hiccup, retryable.
    /// - Other 4xx — surface as-is.
    pub fn connect(config: &ResponsesConfig) -> Result<Self, LlmError> {
        let url = build_ws_url(&config.base_url)?;
        let mut request: Request = url
            .as_str()
            .into_client_request()
            .map_err(|e| LlmError::HttpStatus(0, format!("ws request build: {e}")))?;

        set_header(
            request.headers_mut(),
            "Authorization",
            &format!("Bearer {}", config.api_key),
        )?;
        set_header(request.headers_mut(), "OpenAI-Beta", OPENAI_BETA_WS)?;
        if let Some(account_id) = config.account_id.as_deref() {
            set_header(request.headers_mut(), "chatgpt-account-id", account_id)?;
        }

        let (stream, _response) = tungstenite::connect(request).map_err(map_ws_connect_error)?;

        Ok(Self {
            stream,
            opened_at: Instant::now(),
            bearer: config.api_key.clone(),
        })
    }

    /// Send one `response.create` envelope and stream events back
    /// until `response.completed` / `response.done`. Returns the
    /// accumulated [`StreamState`]; leaves the socket open for the
    /// next turn.
    ///
    /// Mid-stream WS close or IO error surfaces as a retryable
    /// `LlmError` (code 0, body prefixed with `"stream error:"`) so
    /// the agent's outer retry loop reopens on the next attempt.
    pub fn run_turn(
        &mut self,
        config: &ResponsesConfig,
        request: &PromptPayload<'_>,
        on_update: &mut impl FnMut(&str, Option<&str>),
    ) -> Result<StreamState, LlmError> {
        let envelope = build_ws_envelope(config, request);
        let text = serde_json::to_string(&envelope).map_err(LlmError::Json)?;
        self.stream
            .send(Message::Text(text.into()))
            .map_err(map_ws_runtime_error)?;

        let mut state = StreamState::new();
        loop {
            let msg = self.stream.read().map_err(map_ws_runtime_error)?;
            match msg {
                Message::Text(payload) => {
                    let event: serde_json::Value = match serde_json::from_str(payload.as_str()) {
                        Ok(v) => v,
                        // Unparseable frames are ignored on the SSE
                        // path too (line-level resync). Mirror it.
                        Err(_) => continue,
                    };
                    if apply_event(&mut state, &event, on_update)? {
                        break;
                    }
                }
                Message::Binary(_) => {
                    // Codex backend never sends binary; if it ever
                    // does, ignore rather than fault the turn.
                }
                Message::Close(frame) => {
                    let reason = frame
                        .as_ref()
                        .map(|f| format!("code={} reason={}", f.code, f.reason))
                        .unwrap_or_else(|| "no close frame".to_owned());
                    return Err(LlmError::HttpStatus(
                        0,
                        format!("stream error: ws closed mid-stream ({reason})"),
                    ));
                }
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {
                    // Tungstenite auto-pongs pings during `read()`,
                    // but the message still surfaces; ignore.
                }
            }
        }
        Ok(state)
    }

    /// Best-effort: force the underlying TCP socket's read half
    /// closed so a thread blocked in `read()` wakes immediately.
    /// Used for prompt-cancel without polling. Caller should drop
    /// the connection after this — the socket is no longer usable.
    #[allow(dead_code)] // wired up by the cancel path in a later step
    pub fn shutdown_read(&mut self) {
        // tungstenite's `MaybeTlsStream` carries rustls/native-tls
        // variants behind features. The `Plain` arm is always
        // present; TLS arms aren't reachable here for a `wss://`
        // socket without naming the rustls variant explicitly.
        // Dropping the connection from the pool after this is the
        // safe behavior either way — the upper layer surfaces a
        // `stream error` on the next call and reopens.
        if let MaybeTlsStream::Plain(s) = self.stream.get_ref() {
            let _ = s.shutdown(Shutdown::Read);
        }
    }
}

/// Map the configured HTTP base URL to a `ws://` / `wss://` URL
/// pointing at the WebSocket endpoint. The Codex backend lives at
/// the same path as the HTTP+SSE endpoint (`/codex/responses`) — the
/// only delta is the scheme.
fn build_ws_url(base_url: &str) -> Result<String, LlmError> {
    let base = base_url.trim_end_matches('/');
    let rest = if let Some(rest) = base.strip_prefix("https://") {
        return Ok(format!("wss://{rest}/codex/responses"));
    } else if let Some(rest) = base.strip_prefix("http://") {
        rest
    } else {
        return Err(LlmError::HttpStatus(
            0,
            format!("ws scheme unsupported in base_url: {base_url}"),
        ));
    };
    Ok(format!("ws://{rest}/codex/responses"))
}

fn set_header(
    headers: &mut tungstenite::http::HeaderMap,
    name: &'static str,
    value: &str,
) -> Result<(), LlmError> {
    let header_value = value
        .parse()
        .map_err(|e| LlmError::HttpStatus(0, format!("ws header {name}: {e}")))?;
    headers.insert(name, header_value);
    Ok(())
}

fn map_ws_connect_error(e: tungstenite::Error) -> LlmError {
    if let tungstenite::Error::Http(response) = &e {
        let code = response.status().as_u16();
        let body = response
            .body()
            .as_ref()
            .and_then(|b| std::str::from_utf8(b).ok())
            .map(str::to_owned)
            .unwrap_or_default();
        return LlmError::HttpStatus(code, body);
    }
    // Network / TLS / protocol — treat as retryable transport.
    LlmError::HttpStatus(0, format!("stream error: ws connect: {e}"))
}

fn map_ws_runtime_error(e: tungstenite::Error) -> LlmError {
    match e {
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
            LlmError::HttpStatus(0, "stream error: ws closed".to_owned())
        }
        other => LlmError::HttpStatus(0, format!("stream error: {other}")),
    }
}
