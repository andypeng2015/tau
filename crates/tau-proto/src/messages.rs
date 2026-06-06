//! Control-plane point-to-point messages.
//!
//! `Message` is the sibling of [`crate::Event`]: where `Event` carries
//! bus facts (broadcast to subscribers, dotted `category.call` names),
//! `Message` carries directed control-plane traffic — handshake,
//! subscription registration, configuration, the at-least-once
//! `LogEvent`/`Ack` envelope, etc. Messages are not subscribable;
//! they're sent point-to-point between the harness and one specific
//! peer.
//!
//! Wire form: `{"message": "hello", "payload": {...}}` — flat, lower
//! snake_case names, distinct from `Event`'s `{"event": "tool.started",
//! ...}` shape so the [`crate::Frame`] envelope can disambiguate by
//! discriminator.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    CborValue, ClientKind, Event, EventSelector, ExtensionName, InterceptionPriority,
    ToolDefinition,
};

// ---------------------------------------------------------------------------
// Lifecycle messages
// ---------------------------------------------------------------------------

/// Announcement sent by a participant after connecting.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol_version: u32,
    pub client_name: ExtensionName,
    pub client_kind: ClientKind,
}

/// Subscription request describing which events a participant wants.
///
/// Selectors describe event interest, not replay intent. UI socket
/// clients currently receive selected late-join replay from the
/// harness, while extension subscriptions are live-only. This payload
/// has no past-event opt-in field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Subscribe {
    pub selectors: Vec<EventSelector>,
}

/// Interception request describing which event emissions a participant wants
/// to handle before they reach the event log and regular subscribers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Intercept {
    pub selectors: Vec<EventSelector>,
    pub priority: InterceptionPriority,
}

/// Readiness notification emitted after startup or handshake.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Ready {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Disconnect notification with an optional human-readable reason.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Disconnect {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Configuration handed to an extension at startup. Sent
/// point-to-point from the harness to the extension immediately
/// after the harness sees the extension's
/// [`Hello`](crate::Hello). Carries whatever the
/// `config: { … }` value was for that extension in `harness.yaml`,
/// or [`CborValue::Null`] / an empty map when no config was
/// provided. `state_dir` is the harness-assigned persistent state
/// directory for this extension instance, when the harness can provide
/// one.
///
/// `Eq` is not derivable because the underlying CBOR value can
/// contain floats; `PartialEq` is enough for tests.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Configure {
    /// Free-form extension configuration from harness settings.
    pub config: CborValue,
    /// Persistent directory reserved for this extension's runtime state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_dir: Option<PathBuf>,
    /// Secret values explicitly authorized for this extension.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub secrets: BTreeMap<String, SecretValue>,
}

/// Secret text passed from the harness to one authorized extension.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretValue(String);

impl SecretValue {
    /// Wrap a resolved secret value for protocol transport.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the underlying secret text. Avoid logging this value.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Reported by an extension when its
/// [`Configure`](Configure) value is malformed (or
/// otherwise unusable). The harness surfaces the message just like
/// a `harness.yaml` parse error so the user can see why their
/// per-extension config was rejected.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConfigError {
    pub message: String,
}

// ---------------------------------------------------------------------------
// Wire transport — sequenced delivery for runtime events
// ---------------------------------------------------------------------------

/// Monotonic sequence assigned by the harness runtime event stream.
///
/// This sequence is relative to the running harness as a whole. Every
/// `LogEvent` envelope emitted by the running harness gets the next value in
/// this single stream, regardless of whether the inner event is transient,
/// persisted in an agent log, persisted in a session log, or replayed from
/// history. It is not comparable to persisted agent/session event sequences.
/// Receivers acknowledge processing by returning the same sequence in
/// [`Ack::up_to`].
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(transparent)]
pub struct EventLogSeq(u64);

impl EventLogSeq {
    #[must_use]
    pub fn new(v: u64) -> Self {
        Self(v)
    }

    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }

    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

impl std::fmt::Display for EventLogSeq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Wall-clock timestamp as microseconds since the UNIX epoch.
///
/// Stamped onto persisted session events and the JSONL debug log so
/// offline inspection can compute inter-event gaps, RPM bursts, and
/// correlations with provider-side cache misses. `u64` µs covers
/// ~584,000 years past 1970, so saturation is not a concern in
/// practice — callers still saturate on bogus clocks rather than
/// panic, keeping the persistence path infallible. A zero value
/// marks records written before this field existed
/// (`#[serde(default)]` on the carrying struct).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(transparent)]
pub struct UnixMicros(u64);

impl UnixMicros {
    #[must_use]
    pub fn new(v: u64) -> Self {
        Self(v)
    }

    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }

    /// Reads the current wall clock and returns a `UnixMicros`.
    /// Saturates on bogus clocks (pre-1970 or post-2554) instead of
    /// panicking, so callers on the durable-write path can stay
    /// infallible.
    #[must_use]
    pub fn now() -> Self {
        let micros = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_micros()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        Self(micros)
    }
}

impl std::fmt::Display for UnixMicros {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A bus event delivered with harness-owned sequencing metadata. Receivers
/// must process the inner event and then send an [`Ack`] referencing
/// `seq` (or any later sequence, since acks are cumulative).
///
/// `event` is boxed because the inner value is the (potentially
/// large) bus fact. It is never another `LogEvent` or `Ack` — only
/// "real" payload events (e.g., `SessionStarted`, `ExtensionReady`).
///
/// `recorded_at` is stamped by the harness at the publish chokepoint.
/// Subscribers receive the same value the persisted record carries, so offline
/// timing analyses agree with what live consumers saw. Older peers send
/// records without the field; they deserialize as `UnixMicros(0)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogEvent {
    /// Sequence assigned by the harness runtime event log.
    pub seq: EventLogSeq,
    /// Runtime append timestamp shared with durable records when the event is
    /// persisted.
    #[serde(default)]
    pub recorded_at: UnixMicros,
    /// Inner bus fact carried by this event-log envelope.
    pub event: Box<Event>,
}

/// Extension/client request to emit one event with harness-owned
/// delivery metadata.
///
/// The inner `event` is the fact that subscribers see. `transient`
/// controls whether the harness writes eligible semantic facts to durable
/// session or agent event history; it is not part of the emitted fact itself.
///
/// `Emit` is strictly for emitting fresh events. Interceptor replies
/// — including the optionally-mutated event — go through
/// [`InterceptReply`], not `Emit`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Emit {
    pub event: Box<Event>,
    #[serde(default, skip_serializing_if = "core::ops::Not::not")]
    pub transient: bool,
}

/// Directed harness → interceptor message carrying an event emission that has
/// not reached the event log yet. The interceptor must reply with an
/// [`InterceptReply`]; until it does, the harness suspends draining of any
/// further publishes that would themselves be subject to interception.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InterceptRequest {
    pub event: Box<Event>,
    #[serde(default, skip_serializing_if = "core::ops::Not::not")]
    pub transient: bool,
}

/// What an interceptor wants the harness to do with the event it was given.
///
/// `Pass(None)` republishes the original event unchanged (the common
/// no-op case). `Pass(Some(event))` substitutes a possibly-mutated
/// version that flows on through any remaining interceptors and then to
/// subscribers. `Drop` discards the event entirely — but the harness
/// may override `Drop` for events the publisher marked `must_pass`,
/// `tracing::warn!`-ing and falling back to the original.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum InterceptAction {
    Pass(Option<Box<Event>>),
    Drop,
}

/// Interceptor → harness response to an [`InterceptRequest`]. Exactly
/// one reply per request; out-of-order or duplicate replies are a
/// programming error and the harness logs + falls back to the original
/// event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InterceptReply {
    pub action: InterceptAction,
}

/// Best-effort request for a materialized full `agent.prompt_created` payload
/// by id.
///
/// Prompt-created payloads are transient delivery objects; harnesses are not
/// required to retain them after live delivery. A missing prompt is reported as
/// `None` in [`AgentPromptCreatedResult::prompt`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetAgentPromptCreated {
    /// Request correlation id echoed by [`AgentPromptCreatedResult`].
    pub request_id: String,
    /// Session containing the requested prompt.
    pub session_id: crate::SessionId,
    /// Prompt to materialize.
    pub agent_prompt_id: crate::AgentPromptId,
}

/// Response to [`GetAgentPromptCreated`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentPromptCreatedResult {
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<crate::AgentPromptCreated>,
}

/// Request that the harness render the effective system prompt for one role.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetRenderedSystemPrompt {
    /// Request correlation id echoed by [`RenderedSystemPromptResult`].
    pub request_id: String,
    /// Role name whose resolved prompt should be rendered.
    pub role: String,
}

/// Response to [`GetRenderedSystemPrompt`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RenderedSystemPromptResult {
    /// Request correlation id copied from the request.
    pub request_id: String,
    /// Rendered prompt when the role exists and template rendering succeeds.
    /// Exactly one of `prompt` and `error` should be present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Human-readable failure when the role is unknown or rendering fails.
    /// Exactly one of `prompt` and `error` should be present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Request that the harness report the effective tools for one role.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetRenderedToolDefinitions {
    /// Request correlation id echoed by [`RenderedToolDefinitionsResult`].
    pub request_id: String,
    /// Role name whose resolved tool list should be reported.
    pub role: String,
}

/// Response to [`GetRenderedToolDefinitions`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RenderedToolDefinitionsResult {
    /// Request correlation id copied from the request.
    pub request_id: String,
    /// Effective provider-facing tool definitions for the requested role.
    /// Exactly one of `tools` and `error` should be present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    /// Human-readable failure when the role is unknown.
    /// Exactly one of `tools` and `error` should be present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Extension data RPC
// ---------------------------------------------------------------------------

/// Harness-owned storage scope for extension data RPC requests.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionDataScope {
    /// Session-local data under `<session_data_dir>/ext/data/<ext-name>`.
    Session,
    /// User-persistent data under `~/.local/state/tau/ext/<ext-name>`.
    User,
    /// User cache data under `~/.cache/tau/ext/<ext-name>`.
    Cache,
}

/// Extension request for harness-mediated file access inside its data roots.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtensionDataRequest {
    /// Request correlation id echoed by [`ExtensionDataResult`].
    pub request_id: String,
    /// Storage scope to access.
    pub scope: ExtensionDataScope,
    /// File operation to perform.
    pub op: ExtensionDataRequestOp,
}

/// File operation requested by an extension data RPC.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ExtensionDataRequestOp {
    /// Read one whole file at a sanitized relative path.
    ReadFile { path: String },
    /// Write one whole file at a sanitized relative path, replacing any old
    /// content.
    WriteFile { path: String, contents: Vec<u8> },
    /// List direct children of a sanitized relative directory path.
    ListFiles { path: String },
}

/// Harness response to an [`ExtensionDataRequest`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtensionDataResult {
    /// Request correlation id copied from the request.
    pub request_id: String,
    /// Operation result or human-readable error.
    pub result: ExtensionDataResultPayload,
}

/// Result payload for an extension data RPC.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ExtensionDataResultPayload {
    /// Operation succeeded.
    Ok { value: ExtensionDataValue },
    /// Operation failed.
    Error { message: String },
}

/// Successful value returned by an extension data RPC.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ExtensionDataValue {
    /// Whole file contents from a read request.
    ReadFile { contents: Vec<u8> },
    /// Empty success marker for a write request.
    WriteFile,
    /// Direct child entries from a list request.
    ListFiles { entries: Vec<ExtensionDataEntry> },
}

/// One direct child returned by an extension data list request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtensionDataEntry {
    /// Sanitized path relative to the requested scope root.
    pub path: String,
    /// True when this entry is a directory.
    pub is_dir: bool,
    /// File size in bytes for files. Directories use `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub len: Option<u64>,
}

/// Receiver → sender acknowledgement that all log events with sequence
/// `<= up_to` have been processed. Cumulative — newer acks supersede
/// older ones.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Ack {
    pub up_to: EventLogSeq,
}

// ---------------------------------------------------------------------------
// Top-level message envelope
// ---------------------------------------------------------------------------

/// Directional subsets of the point-to-point control-plane protocol.
///
/// The harness ↔ extension protocol is directional even though the shared wire
/// envelope below remains bidirectional for compatibility and UI/client use.
/// Control-plane messages sent by the harness to an extension.
///
/// These are point-to-point replies, lifecycle/configuration messages, or live
/// delivery envelopes. They are not extension-authored requests and do not by
/// themselves represent facts appended to the event log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "message", content = "payload", rename_all = "snake_case")]
pub enum HarnessMessage {
    Configure(Configure),
    Disconnect(Disconnect),
    InterceptRequest(InterceptRequest),
    AgentPromptCreatedResult(Box<AgentPromptCreatedResult>),
    ExtensionDataResult(Box<ExtensionDataResult>),
    LogEvent(LogEvent),
}

impl From<HarnessMessage> for Message {
    fn from(message: HarnessMessage) -> Self {
        match message {
            HarnessMessage::Configure(value) => Self::Configure(value),
            HarnessMessage::Disconnect(value) => Self::Disconnect(value),
            HarnessMessage::InterceptRequest(value) => Self::InterceptRequest(value),
            HarnessMessage::AgentPromptCreatedResult(value) => {
                Self::AgentPromptCreatedResult(value)
            }
            HarnessMessage::ExtensionDataResult(value) => Self::ExtensionDataResult(value),
            HarnessMessage::LogEvent(value) => Self::LogEvent(value),
        }
    }
}

/// Control-plane messages sent by an extension to the harness.
///
/// These are extension-authored lifecycle messages, subscriptions,
/// event-publication requests, interceptor replies, RPC requests, and delivery
/// acknowledgements. Some variants are also accepted from UI clients, but this
/// direction names the extension side of the harness ↔ extension protocol.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "message", content = "payload", rename_all = "snake_case")]
pub enum ExtensionMessage {
    Hello(Hello),
    Subscribe(Subscribe),
    Intercept(Intercept),
    Ready(Ready),
    ConfigError(ConfigError),
    Emit(Emit),
    InterceptReply(InterceptReply),
    GetAgentPromptCreated(GetAgentPromptCreated),
    ExtensionDataRequest(ExtensionDataRequest),
    Ack(Ack),
}

impl From<ExtensionMessage> for Message {
    fn from(message: ExtensionMessage) -> Self {
        match message {
            ExtensionMessage::Hello(value) => Self::Hello(value),
            ExtensionMessage::Subscribe(value) => Self::Subscribe(value),
            ExtensionMessage::Intercept(value) => Self::Intercept(value),
            ExtensionMessage::Ready(value) => Self::Ready(value),
            ExtensionMessage::ConfigError(value) => Self::ConfigError(value),
            ExtensionMessage::Emit(value) => Self::Emit(value),
            ExtensionMessage::InterceptReply(value) => Self::InterceptReply(value),
            ExtensionMessage::GetAgentPromptCreated(value) => Self::GetAgentPromptCreated(value),
            ExtensionMessage::ExtensionDataRequest(value) => Self::ExtensionDataRequest(value),
            ExtensionMessage::Ack(value) => Self::Ack(value),
        }
    }
}

/// Bidirectional point-to-point control-plane message envelope used on the
/// wire.
///
/// Wire form is `{"message": "<flat_name>", "payload": {...}}`. Names
/// are flat (no dot, snake_case) to make the discriminator trivially
/// distinguishable from [`Event`]'s dotted `category.call` form — the
/// outer [`crate::Frame`] envelope relies on this distinction.
///
/// Prefer [`HarnessMessage`] or [`ExtensionMessage`] in new extension-facing
/// code when the communication direction is known. This bidirectional enum is
/// retained as the shared wire envelope and for UI/client messages that are not
/// exclusively part of the harness ↔ extension flow.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "message", content = "payload", rename_all = "snake_case")]
pub enum Message {
    Hello(Hello),
    Subscribe(Subscribe),
    Intercept(Intercept),
    Ready(Ready),
    Disconnect(Disconnect),
    Configure(Configure),
    ConfigError(ConfigError),
    Emit(Emit),
    InterceptRequest(InterceptRequest),
    InterceptReply(InterceptReply),
    GetAgentPromptCreated(GetAgentPromptCreated),
    AgentPromptCreatedResult(Box<AgentPromptCreatedResult>),
    GetRenderedSystemPrompt(GetRenderedSystemPrompt),
    RenderedSystemPromptResult(Box<RenderedSystemPromptResult>),
    GetRenderedToolDefinitions(GetRenderedToolDefinitions),
    ExtensionDataRequest(ExtensionDataRequest),
    RenderedToolDefinitionsResult(Box<RenderedToolDefinitionsResult>),
    ExtensionDataResult(Box<ExtensionDataResult>),
    LogEvent(LogEvent),
    Ack(Ack),
}
