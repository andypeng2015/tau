//! Built-in internal tools for `tau-harness`.

use std::sync::Arc;

use tau_harness::{
    AgentToolCall, ConversationId, HarnessError, InternalToolHandler, InternalToolHost,
};
use tau_proto::{BackgroundSupport, ToolExecutionMode, ToolName, ToolSpec, ToolType};

const SKILL_TOOL_NAME: &str = "skill";
const DELEGATE_TOOL_NAME: &str = "delegate";
const WAIT_TOOL_NAME: &str = "wait";
const CANCEL_TOOL_NAME: &str = "cancel";
const MESSAGE_TOOL_NAME: &str = "message";

/// Return handlers for Tau's built-in harness-process tools.
pub fn builtin_handlers() -> Vec<Arc<dyn InternalToolHandler>> {
    vec![Arc::new(BuiltinTools)]
}

struct BuiltinTools;

impl InternalToolHandler for BuiltinTools {
    fn tool_specs(&self) -> Vec<ToolSpec> {
        vec![
            skill_tool_spec(),
            delegate_tool_spec(),
            wait_tool_spec(),
            cancel_tool_spec(),
            message_tool_spec(),
        ]
    }

    fn handles(&self, internal_tool_name: &ToolName) -> bool {
        matches!(
            internal_tool_name.as_str(),
            SKILL_TOOL_NAME
                | DELEGATE_TOOL_NAME
                | WAIT_TOOL_NAME
                | CANCEL_TOOL_NAME
                | MESSAGE_TOOL_NAME
        )
    }

    fn handle_started(
        &self,
        host: &mut InternalToolHost<'_>,
        conversation_id: &ConversationId,
        call: &AgentToolCall,
        visible_tool_name: ToolName,
    ) -> Result<(), HarnessError> {
        match call.name.as_str() {
            SKILL_TOOL_NAME => host.handle_skill_tool_call(conversation_id, call),
            DELEGATE_TOOL_NAME => {
                host.handle_delegate_tool_call(conversation_id, call, visible_tool_name)
            }
            WAIT_TOOL_NAME => host.handle_wait_tool_call(conversation_id, call, visible_tool_name),
            MESSAGE_TOOL_NAME => {
                host.handle_message_tool_call(conversation_id, call, visible_tool_name)
            }
            CANCEL_TOOL_NAME => {
                host.handle_cancel_tool_call(conversation_id, call, visible_tool_name)
            }
            _ => Ok(()),
        }
    }
}

fn skill_tool_spec() -> ToolSpec {
    ToolSpec {
        name: ToolName::new(SKILL_TOOL_NAME),
        model_visible_name: None,
        description: Some("Discover and load skills — short, focused playbooks for specific tasks. The user has likely curated skills for workflows they care about, so reach for this tool early: before tackling any request that touches a tool, command, framework, or domain you are not deeply familiar with — or anything the user might have an opinionated way of doing. Most skills are NOT pre-advertised in <available_skills>, so a missing entry there is no reason to skip this tool. Pass a query string; punctuation separates terms except hyphens inside skill names. If the search resolves to one skill, or a single-term query exactly matches a skill name, the full skill is loaded; otherwise matching skill names and descriptions are returned with guidance. Query terms are split on punctuation, lowercased, and deduplicated; hyphenated skill names are preserved. To load a specific ambiguous result, call this tool again with only the exact skill name.".to_owned()),
        tool_type: ToolType::Function,
        parameters: Some(serde_json::json!({"type":"object","properties":{"query":{"type":"string","description":"Keywords matched case-insensitively against skill names and descriptions. Punctuation separates terms except hyphens inside skill names; terms are lowercased and deduplicated. Use only an exact skill name to load a specific ambiguous result."},"search_content":{"type":"boolean","description":"When true, also search the first 64 KiB of the skill file after stripping frontmatter from that prefix. Default false."}},"required":["query"],"additionalProperties":false})),
        format: None,
        enabled_by_default: true,
        execution_mode: ToolExecutionMode::Shared,
        background_support: None,
    }
}

fn delegate_tool_spec() -> ToolSpec {
    ToolSpec { name: ToolName::new(DELEGATE_TOOL_NAME), model_visible_name: None, description: Some("Delegate a self-contained sub-task to a fresh sub-agent that runs with its own context and tools, and returns only its final text answer. The instant background placeholder and final result include `self_agent_id` and `sub_agent_id` headers/values. Pass `sub_agent_id` to `message`.".to_owned()), tool_type: ToolType::Function, parameters: Some(serde_json::json!({"type":"object","properties":{"task_name":{"type":"string","description":"Short human-readable label for the sub-task (a few words, lowercase). Surfaced live to the user as `delegate [task_name]` while the sub-agent runs."},"prompt":{"type":"string","description":"Self-contained task for the sub-agent."},"execution_mode":{"type":"string","enum":["shared","update","exclusive"],"description":"Default: `shared`."},"role":{"type":"string","description":"Optional sub-agent role to use."}},"required":["task_name","prompt"],"additionalProperties":false})), format: None, enabled_by_default: true, execution_mode: ToolExecutionMode::Shared, background_support: Some(BackgroundSupport::Instant) }
}

fn message_tool_spec() -> ToolSpec {
    ToolSpec { name: ToolName::new(MESSAGE_TOOL_NAME), model_visible_name: None, description: Some("Send an async message to another live or pending agent, or to the user. Use recipient_id `user`, or a `sub_agent_id` returned by `delegate`; UI display depends on `/set show-messages`. A non-user recipient also receives a hidden prompt. Requires `recipient_id` and `message`.".to_owned()), tool_type: ToolType::Function, parameters: Some(serde_json::json!({"type":"object","properties":{"recipient_id":{"type":"string","description":"Recipient agent_id, or the special value `user`."},"message":{"type":"string","description":"Message body."}},"required":["recipient_id","message"],"additionalProperties":false})), format: None, enabled_by_default: true, execution_mode: ToolExecutionMode::Shared, background_support: Some(BackgroundSupport::Never) }
}

fn cancel_tool_spec() -> ToolSpec {
    ToolSpec { name: ToolName::new(CANCEL_TOOL_NAME), model_visible_name: None, description: Some("Cancel a running supported background tool call. Requires `tool_call_id`; currently delegate and shell tool calls can be canceled. Duplicate cancellation requests for the same tool call fail when tracked.".to_owned()), tool_type: ToolType::Function, parameters: Some(serde_json::json!({"type":"object","properties":{"tool_call_id":{"type":"string","description":"Required id of the running supported background tool call to cancel."}},"required":["tool_call_id"],"additionalProperties":false})), format: None, enabled_by_default: true, execution_mode: ToolExecutionMode::Shared, background_support: Some(BackgroundSupport::Never) }
}

fn wait_tool_spec() -> ToolSpec {
    ToolSpec { name: ToolName::new(WAIT_TOOL_NAME), model_visible_name: None, description: Some("Wait for background tool calls. With `tool_call_id`, wait for that specific background call. Without `tool_call_id`, wait for the first background call in this conversation to finish and return its `original_tool_call_id`. Already-finished matching results return immediately. Tau will notify you via marked internal messages about background calls completing; `wait({})` consumes one completion and suppresses that completion notice.".to_owned()), tool_type: ToolType::Function, parameters: Some(serde_json::json!({"type":"object","properties":{"tool_call_id":{"type":"string","description":"Optional. When set, wait for this specific background tool call. When omitted, wait for the first background tool call in this conversation to finish."}},"additionalProperties":false})), format: None, enabled_by_default: true, execution_mode: ToolExecutionMode::Shared, background_support: Some(BackgroundSupport::Never) }
}
