//! Provider context and transcript item support types.

use serde::{Deserialize, Serialize};

use crate::events::{ProviderBackend, ToolFormat, ToolType};
use crate::{CborValue, ProviderTokenUsage, ToolCallId, ToolName};

// ---------------------------------------------------------------------------
// Item-based conversation types
// ---------------------------------------------------------------------------

/// Role of a participant in one message item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextRole {
    /// System-level instructions.
    System,
    /// Developer-level instructions.
    Developer,
    /// User-authored message content.
    User,
    /// Assistant-authored message content.
    Assistant,
}

/// One content part inside a message item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    /// Plain UTF-8 text content.
    Text {
        /// Text body for this content part.
        text: String,
    },
}

/// Opaque provider-owned payload preserved without interpretation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OpaqueProviderItem(
    /// Provider-owned CBOR payload preserved exactly enough for replay.
    pub CborValue,
);

/// One message item in the prompt or assistant output timeline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MessageItem {
    /// Role that authored the message.
    pub role: ContextRole,
    /// Ordered content parts for the message.
    pub content: Vec<ContentPart>,
    /// Optional assistant-message phase metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<MessagePhase>,
}

/// One tool call item in the prompt or assistant output timeline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCallItem {
    /// Stable tool-call identifier.
    pub call_id: ToolCallId,
    /// Tool name requested by the assistant.
    pub name: ToolName,
    /// Kind of tool call.
    pub tool_type: ToolType,
    /// Tool arguments in protocol CBOR form.
    pub arguments: CborValue,
}

/// Terminal status for one tool result item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolResultStatus {
    /// Tool completed successfully.
    Success,
    /// Tool failed with a diagnostic message.
    Error {
        /// Human-readable failure message.
        message: String,
    },
    /// Tool execution was cancelled.
    Cancelled {
        /// Human-readable cancellation reason.
        reason: String,
    },
}

/// One rendered header in the text sent to a provider for a tool response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolResponseHeader {
    /// Header key rendered before the `: ` separator.
    pub key: String,
    /// Header value rendered after the `: ` separator.
    pub value: String,
}

/// Provider-facing text form of a tool response.
///
/// The canonical rendering is header lines in `<key>: <value>` form, followed
/// by an empty line and then the tool-specific body. Tool result events still
/// carry raw CBOR so extensions do not need to coordinate a wire-format
/// migration; this type is the normalized boundary used before provider output.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolResponse {
    /// Original tool payload kept for non-provider consumers that need
    /// structured data rather than rendered text.
    pub raw: CborValue,
    /// Structured headers rendered before the response body.
    pub headers: Vec<ToolResponseHeader>,
    /// Tool-specific response text rendered after the blank separator.
    pub body: String,
}

impl ToolResponse {
    /// Builds a normalized provider-facing response from a raw CBOR tool
    /// result.
    #[must_use]
    pub fn from_cbor(value: &CborValue) -> Self {
        match value {
            CborValue::Map(entries) => Self::from_cbor_map(entries),
            other => Self {
                raw: other.clone(),
                headers: Vec::new(),
                body: cbor_tool_response_text(other),
            },
        }
    }

    /// Renders this response as header lines, a blank line, then body text.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        for header in &self.headers {
            out.push_str(&header.key);
            out.push_str(": ");
            out.push_str(&header.value);
            out.push('\n');
        }
        if !self.headers.is_empty() {
            out.push('\n');
        }
        out.push_str(&self.body);
        out
    }

    fn from_cbor_map(entries: &[(CborValue, CborValue)]) -> Self {
        let raw = CborValue::Map(entries.to_vec());
        let mut headers = Vec::new();
        let mut body_parts = Vec::new();
        for (key, value) in entries {
            let key = cbor_tool_response_text(key);
            let value = cbor_tool_response_text(value);
            if key == "output" || key == "line-numbered content" {
                body_parts.push(value);
            } else if value.contains('\n') {
                body_parts.push(format!("{key}:\n{value}"));
            } else {
                headers.push(ToolResponseHeader { key, value });
            }
        }
        Self {
            raw,
            headers,
            body: body_parts.join("\n"),
        }
    }
}

fn cbor_tool_response_text(value: &CborValue) -> String {
    match value {
        CborValue::Null => String::new(),
        CborValue::Bool(b) => b.to_string(),
        CborValue::Integer(i) => {
            let n: i128 = (*i).into();
            n.to_string()
        }
        CborValue::Float(f) => f.to_string(),
        CborValue::Text(s) => s.clone(),
        CborValue::Bytes(b) => format!("<{} bytes>", b.len()),
        CborValue::Array(arr) => arr
            .iter()
            .map(cbor_tool_response_text)
            .collect::<Vec<_>>()
            .join("\n"),
        CborValue::Map(entries) => ToolResponse::from_cbor_map(entries).render(),
        CborValue::Tag(_, inner) => cbor_tool_response_text(inner),
        _ => String::new(),
    }
}

/// One tool result item in the prompt timeline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolResultItem {
    /// Tool call this result answers.
    pub call_id: ToolCallId,
    /// Kind of tool that produced the result.
    pub tool_type: ToolType,
    /// Terminal status of the tool call.
    pub status: ToolResultStatus,
    /// Provider-facing rendered tool response plus raw payload.
    pub output: ToolResponse,
}

/// One item in Tau's prompt/response timeline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ContextItem {
    /// Message authored by a system, developer, user, or assistant role.
    Message(MessageItem),
    /// Assistant request to invoke a tool.
    ToolCall(ToolCallItem),
    /// Tool result returned to the model.
    ToolResult(ToolResultItem),
    /// Provider-specific reasoning item.
    Reasoning(OpaqueProviderItem),
    /// Provider-specific compaction item.
    Compaction(OpaqueProviderItem),
    /// Provider item that Tau does not yet understand.
    UnknownProviderItem(OpaqueProviderItem),
}

/// Transcript node projected from durable facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum TranscriptNode {
    /// User input node.
    UserInput(UserInputNode),
    /// Assistant response node.
    AssistantResponse(AssistantResponseNode),
    /// Tool results node.
    ToolResults(ToolResultsNode),
    /// Compaction replacement node.
    Compaction(CompactionNode),
}

/// Transcript node containing user input context items.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserInputNode {
    /// Context items that make up the user input.
    pub items: Vec<ContextItem>,
}

/// Transcript node containing one assistant response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssistantResponseNode {
    /// Provider response id, when the backend returned one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_response_id: Option<String>,
    /// Provider backend that produced the response, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<ProviderBackend>,
    /// Output items produced by the assistant.
    pub output_items: Vec<ContextItem>,
    /// Provider token usage for this response, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ProviderTokenUsage>,
}

/// Transcript node containing tool results.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolResultsNode {
    /// Tool result items in this node.
    pub items: Vec<ToolResultItem>,
}

/// Transcript node containing a compacted replacement window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompactionNode {
    /// Context items that replace earlier transcript history.
    pub replacement_window: Vec<ContextItem>,
}

/// Assistant-message phase label, mirroring the OpenAI Codex
/// `phase` field on assistant `message` items.
///
/// The Codex Responses API attaches one of these to each assistant
/// turn it produces (on models that support it, currently
/// `gpt-5.3-codex` and later). Resending the same value on later
/// turns lets the model distinguish intermediate progress from
/// completed work — the doc-recommended remedy for "early stopping"
/// in long, tool-heavy runs.
///
/// We capture the value off the SSE stream, persist it alongside the
/// assistant turn, and echo it back on every re-serialized history
/// replay. Older models that do not emit this field still receive
/// the `final_answer` default on assistant message items the harness
/// re-serializes, which is the explicit guidance in the deployment
/// checklist.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessagePhase {
    /// Intermediate progress / preliminary notes.
    Commentary,
    /// Final completed response.
    FinalAnswer,
}

impl MessagePhase {
    /// Wire string accepted by the OpenAI Codex Responses API on
    /// assistant `message` items.
    #[must_use]
    pub const fn as_openai_wire(self) -> &'static str {
        match self {
            Self::Commentary => "commentary",
            Self::FinalAnswer => "final_answer",
        }
    }
}

/// A tool definition available for the agent to use.
///
/// This is outbound (harness → LLM in the prompt), so the harness
/// controls the string and we enforce the `ToolName` invariant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Protocol tool name used for calls and results.
    pub name: ToolName,
    /// Optional provider-visible tool name when it differs from the protocol
    /// name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_visible_name: Option<ToolName>,
    /// Optional model-visible tool description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether this is a JSON-schema function tool or a freeform custom tool.
    pub tool_type: ToolType,
    /// JSON Schema describing the tool's input parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
    /// Optional freeform/custom input format. `None` means provider-default
    /// unconstrained text for custom tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<ToolFormat>,
}
