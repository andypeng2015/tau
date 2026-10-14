//! UI/extension-owned shell protocol carried by opaque, transient custom
//! events.
//!
//! The harness must not decode these payloads or use them to route commands.

mod client;
#[cfg(test)]
mod tests;
pub use client::{Client, ClientUpdate, Endpoint};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tau_proto::{
    AgentId, CborValue, ConnectionId, CustomEvent, Event, EventName, ExtensionName,
    HarnessInputMessage, SessionId, ShellCommandFinished, ShellCommandProgress, UiShellCommand,
};

/// Exact extension-owned request name. Consumers subscribe live-only.
pub const REQUEST_EVENT: &str = "shellux.request";
/// Exact extension-owned response name. Consumers subscribe live-only.
pub const REPLY_EVENT: &str = "shellux.reply";
/// Maximum completed and executing operation IDs retained for one process.
/// Reject new operations at this bound rather than evicting deduplication keys.
pub const OPERATION_LIMIT: usize = 4096;
/// Maximum simultaneously admitted UI commands per extension process.
pub const RUNNING_LIMIT: usize = 32;
/// Maximum path candidates returned by one completer.
pub const COMPLETION_LIMIT: usize = 100;
/// Maximum UTF-8 bytes in a completion token or one suggestion.
pub const TOKEN_LIMIT: usize = 4096;
/// Maximum raw command text accepted by an endpoint.
pub const COMMAND_LIMIT: usize = 32 * 1024;

/// Generate a process/draft/request nonce without relying on clock or PID
/// reuse.
pub fn nonce() -> String {
    let mut bytes = [0_u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// One explicitly selected configured extension process.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Target {
    /// Stable configured instance name, never inferred from tool registration.
    pub instance: ExtensionName,
    /// Random per-process incarnation; stale queued intents must not match.
    pub nonce: String,
}

/// Owner and correlation captured before asynchronous work.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Context {
    /// Bound harness session.
    pub session_id: SessionId,
    /// Originating transcript; `None` is an explicit no-agent UI target.
    pub agent_id: Option<AgentId>,
    /// UI-generated unique request ID, including an attachment-local nonce.
    pub request_id: String,
}

/// Path matching requested without transmitting the rest of the prompt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionKind {
    /// Enumerate entries in the token's parent directory.
    Path,
    /// Search file paths beneath the extension's remembered workdir.
    PathFuzzy,
}

/// Request body interpreted only by a shell-like extension.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Operation {
    /// Query capabilities after subscription, including late attachment.
    Discover,
    /// Run once in the explicitly selected process.
    Execute {
        /// Raw shell command, excluding the UI's instance selector.
        command: String,
        /// Submit a separate generic passive append after completion.
        include_in_context: bool,
    },
    /// Best-effort cancellation; never implies rollback of OS effects.
    Cancel {
        /// Original execution request ID.
        command_id: String,
    },
    /// Query bounded active command state for UI catchup.
    Running,
    /// Token-only asynchronous environment completion.
    Complete {
        /// Decoded path token up to the cursor, not the complete draft.
        token: String,
        /// Prefix directory enumeration or fuzzy file search.
        matching: CompletionKind,
    },
}

/// UI-originated transient request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Original session, agent and correlation.
    pub context: Context,
    /// Required for every operation except discovery.
    pub target: Option<Target>,
    /// Requested endpoint action.
    pub operation: Operation,
}

/// Bounded active command state owned by the executor, not the harness.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RunningCommand {
    /// Immutable original command fields; legacy DTO reused only as data.
    pub command: UiShellCommand,
    /// Authenticated original UI connection.
    pub owner: ConnectionId,
}

/// One filesystem suggestion, before UI quoting and replacement construction.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PathCandidate {
    /// Decoded path token to insert; directories end in `/`.
    pub value: String,
    /// Whether accepting it should keep the completion menu active.
    pub directory: bool,
}

/// Extension-authored transient response body.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response {
    /// Protocol-v1 capability advertisement in response to discovery.
    Available {
        /// Whether this instance accepts execution requests.
        execute: bool,
        /// Whether it answers both path completion modes.
        complete: bool,
    },
    /// Command accepted into the bounded executor queue.
    Started(RunningCommand),
    /// Bounded incremental output, reusing the historical data-only DTO.
    Progress(ShellCommandProgress),
    /// Final bounded output, distinct from transcript append outcome.
    Finished(ShellCommandFinished),
    /// One active command, with reply agent metadata scoped to this row.
    /// A query emits at most [`RUNNING_LIMIT`] rows, then `RunningComplete`.
    Running {
        /// Original immutable command; never a mixed-agent payload.
        command: RunningCommand,
    },
    /// Content-free end of a running query, using its original query context.
    RunningComplete,
    /// Bounded path suggestions in the extension host's filesystem namespace.
    Completed {
        /// At most [`COMPLETION_LIMIT`] bounded suggestions.
        candidates: Vec<PathCandidate>,
    },
    /// The endpoint rejected a request before execution or enumeration.
    Rejected {
        /// Human-readable diagnostic, bounded by the producing endpoint.
        reason: String,
    },
}

/// Reply authenticated through delivery metadata rather than payload labels.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    /// Echoed request correlation.
    pub context: Context,
    /// Configured instance and process nonce that answered.
    pub target: Target,
    /// Endpoint-owned result.
    pub response: Response,
}

/// Encode a request with generic privacy scope and mandatory transient
/// delivery.
pub fn request_message(request: &Request) -> HarnessInputMessage {
    encode(REQUEST_EVENT, &request.context, request)
}

/// Encode a response with the same generic session/agent privacy scope.
pub fn reply_message(reply: &Reply) -> HarnessInputMessage {
    encode(REPLY_EVENT, &reply.context, reply)
}

/// Decode a request without inventing publisher authority.
pub fn decode_request(event: &Event) -> Option<Request> {
    decode(REQUEST_EVENT, event)
        .filter(|request: &Request| envelope_matches(event, &request.context))
}

/// Decode a response without inventing publisher authority.
pub fn decode_reply(event: &Event) -> Option<Reply> {
    decode(REPLY_EVENT, event).filter(|reply: &Reply| envelope_matches(event, &reply.context))
}

/// Parse the deliberately mandatory `!instance command` / `!!instance command`
/// spelling. An empty command is invalid; no default executor exists.
pub fn parse_command(line: &str) -> Result<(ExtensionName, &str, bool), &'static str> {
    let line = line.trim_start();
    let (rest, include) = if let Some(rest) = line.strip_prefix("!!") {
        (rest, false)
    } else if let Some(rest) = line.strip_prefix('!') {
        (rest, true)
    } else {
        return Err("expected !instance command or !!instance command");
    };
    let Some(end) = rest.find(char::is_whitespace) else {
        return Err("select an instance: !instance command or !!instance command");
    };
    let instance = rest[..end]
        .parse()
        .map_err(|_| "invalid or missing shell instance")?;
    let command = rest[end..].trim_start();
    if command.is_empty() {
        return Err("a command is required after the shell instance");
    }
    if command.len() > COMMAND_LIMIT {
        return Err("shell command is too long");
    }
    Ok((instance, command, include))
}

fn encode(name: &str, context: &Context, value: &impl Serialize) -> HarnessInputMessage {
    let mut event = CustomEvent::try_new(
        name.parse::<EventName>().expect("static custom event name"),
        Some(context.session_id.clone()),
        CborValue::serialized(value).expect("protocol payload is serializable"),
    )
    .expect("extension-owned event name");
    if let Some(agent_id) = &context.agent_id {
        event = event.with_agent_id(agent_id.clone());
    }
    HarnessInputMessage::emit_with_persist(Event::ExtensionEvent(event), false)
}

fn decode<T: serde::de::DeserializeOwned>(name: &str, event: &Event) -> Option<T> {
    let Event::ExtensionEvent(event) = event else {
        return None;
    };
    (event.name().to_string() == name)
        .then(|| event.payload().deserialized().ok())
        .flatten()
}

fn envelope_matches(event: &Event, context: &Context) -> bool {
    let Event::ExtensionEvent(event) = event else {
        return false;
    };
    event.session_id() == Some(&context.session_id)
        && event.agent_id() == context.agent_id.as_ref()
        && !context.request_id.is_empty()
        && context.request_id.len() <= 128
}
