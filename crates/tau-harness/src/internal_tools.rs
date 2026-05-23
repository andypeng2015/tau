//! Injection point for harness-internal tools owned by higher crates.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use tau_proto::{ToolDisplay, ToolName, ToolSpec, ToolStarted};

use crate::discovery::DiscoveredSkillSource;
use crate::error::HarnessError;
use crate::harness::{HARNESS_CONNECTION_ID, Harness};
use crate::{AgentToolCall, ConversationId};

/// A handler for tools implemented inside the harness process.
pub trait InternalToolHandler: Send + Sync {
    /// Tool specifications this handler registers as internal tools.
    fn tool_specs(&self) -> Vec<ToolSpec>;

    /// Return true when this handler owns `internal_tool_name`.
    fn handles(&self, internal_tool_name: &ToolName) -> bool;

    /// Handle a routed `ToolStarted` invocation.
    fn handle_started(
        &self,
        host: &mut InternalToolHost<'_>,
        conversation_id: &ConversationId,
        call: &AgentToolCall,
        visible_tool_name: ToolName,
    ) -> Result<(), HarnessError>;
}

/// Shared reference-counted internal tool handler.
pub type InternalToolHandlers = Vec<Arc<dyn InternalToolHandler>>;

/// Public snapshot of one skill known to the harness.
#[derive(Clone)]
pub struct InternalSkill {
    /// Skill name used as the `skill` query exact match.
    pub name: String,
    /// Short human-facing description.
    pub description: String,
    /// Markdown source for loading or content search.
    pub source: InternalSkillSource,
}

/// Public snapshot of a skill Markdown source.
#[derive(Clone)]
pub enum InternalSkillSource {
    /// An extension-announced skill backed by an on-disk Markdown file.
    File(PathBuf),
    /// A Tau built-in skill embedded into the harness binary.
    BuiltIn { content: Cow<'static, str> },
}

impl InternalSkillSource {
    /// Human-readable source label for warnings.
    pub fn label(&self) -> String {
        match self {
            Self::File(path) => path.display().to_string(),
            Self::BuiltIn { .. } => "built-in skill".to_owned(),
        }
    }
}

/// Narrow facade exposed to internal tool handler crates.
pub struct InternalToolHost<'a> {
    harness: &'a mut Harness,
}

impl<'a> InternalToolHost<'a> {
    pub(crate) fn new(harness: &'a mut Harness) -> Self {
        Self { harness }
    }

    /// Register a harness-process internal tool.
    pub fn register_internal_tool(&mut self, spec: ToolSpec) {
        let _ = self
            .harness
            .registry
            .register_internal(HARNESS_CONNECTION_ID, spec);
    }

    /// Return a cloned snapshot of skills discovered by the harness.
    pub fn discovered_skills(&self) -> Vec<InternalSkill> {
        self.harness
            .discovered_skills
            .iter()
            .map(|(name, skill)| InternalSkill {
                name: name.as_str().to_owned(),
                description: skill.description.clone(),
                source: match &skill.source {
                    DiscoveredSkillSource::File(path) => InternalSkillSource::File(path.clone()),
                    DiscoveredSkillSource::BuiltIn { content } => InternalSkillSource::BuiltIn {
                        content: content.clone(),
                    },
                },
            })
            .collect()
    }

    /// Emit an important informational message to the user.
    pub fn emit_info_important(&mut self, message: &str) {
        self.harness.emit_info_important(message);
    }

    /// Handle the built-in `delegate` tool.
    pub fn handle_delegate_tool_call(
        &mut self,
        conversation_id: &ConversationId,
        call: &AgentToolCall,
        visible_tool_name: ToolName,
    ) -> Result<(), HarnessError> {
        self.harness
            .handle_delegate_tool_call(conversation_id, call, visible_tool_name)
    }

    /// Handle the built-in `wait` tool.
    pub fn handle_wait_tool_call(
        &mut self,
        conversation_id: &ConversationId,
        call: &AgentToolCall,
        visible_tool_name: ToolName,
    ) -> Result<(), HarnessError> {
        self.harness
            .handle_wait_tool_call(conversation_id, call, visible_tool_name)
    }

    #[cfg(test)]
    pub(crate) fn handle_message_tool_call(
        &mut self,
        conversation_id: &ConversationId,
        call: &AgentToolCall,
        visible_tool_name: ToolName,
    ) -> Result<(), HarnessError> {
        self.harness
            .handle_message_tool_call(conversation_id, call, visible_tool_name)
    }

    #[cfg(test)]
    pub(crate) fn handle_cancel_tool_call(
        &mut self,
        conversation_id: &ConversationId,
        call: &AgentToolCall,
        visible_tool_name: ToolName,
    ) -> Result<(), HarnessError> {
        self.harness
            .handle_cancel_tool_call(conversation_id, call, visible_tool_name)
    }

    /// Ensure the harness tracks an internal tool call before it completes.
    pub fn ensure_internal_tool_tracking(
        &mut self,
        conversation_id: &ConversationId,
        call: &AgentToolCall,
        visible_tool_name: &ToolName,
    ) {
        self.harness
            .ensure_harness_owned_tool_tracking(conversation_id, call, visible_tool_name);
    }

    /// Complete an internal tool call with a final text result.
    pub fn finish_tool_with_result(
        &mut self,
        conversation_id: &ConversationId,
        call_id: tau_proto::ToolCallId,
        tool_name: ToolName,
        tool_type: tau_proto::ToolType,
        result: String,
        details: Option<tau_proto::CborValue>,
    ) {
        self.harness.finish_harness_owned_tool_with_result(
            conversation_id,
            call_id,
            tool_name,
            tool_type,
            result,
            details,
        );
    }

    /// Complete an internal tool call with a final structured result.
    pub fn finish_tool_with_cbor_result(
        &mut self,
        conversation_id: &ConversationId,
        call_id: tau_proto::ToolCallId,
        tool_name: ToolName,
        tool_type: tau_proto::ToolType,
        result: tau_proto::CborValue,
        display: Option<ToolDisplay>,
    ) {
        self.harness.finish_harness_owned_tool_with_cbor_result(
            conversation_id,
            call_id,
            tool_name,
            tool_type,
            result,
            display,
        );
    }

    /// Complete an internal tool call with a final error.
    pub fn finish_tool_with_error(
        &mut self,
        conversation_id: &ConversationId,
        call_id: tau_proto::ToolCallId,
        tool_name: ToolName,
        tool_type: tau_proto::ToolType,
        message: String,
        details: Option<tau_proto::CborValue>,
    ) {
        self.harness.finish_harness_owned_tool_with_error(
            conversation_id,
            call_id,
            tool_name,
            tool_type,
            message,
            details,
        );
    }

    /// Complete an internal tool call with a final displayed error.
    pub fn finish_tool_with_display_error(
        &mut self,
        conversation_id: &ConversationId,
        call_id: tau_proto::ToolCallId,
        tool_name: ToolName,
        tool_type: tau_proto::ToolType,
        message: String,
        details: Option<tau_proto::CborValue>,
        display: Option<ToolDisplay>,
    ) {
        self.harness.finish_harness_owned_tool_with_display_error(
            conversation_id,
            call_id,
            tool_name,
            tool_type,
            message,
            details,
            display,
        );
    }

    /// Request cancellation of a running cancellable tool call.
    pub fn cancel_tool_call(
        &mut self,
        target_call_id: &tau_proto::ToolCallId,
    ) -> Result<(), String> {
        self.harness.cancel_tool_call(target_call_id)
    }

    /// Publish an agent-to-agent or agent-to-user message from a conversation.
    pub fn publish_agent_message(
        &mut self,
        conversation_id: &ConversationId,
        recipient_id: String,
        message: String,
    ) -> Result<(), String> {
        self.harness
            .publish_agent_message_from_conversation(conversation_id, recipient_id, message)
    }
}

impl Harness {
    /// Install handlers and register their internal tool specs.
    pub fn install_internal_tool_handlers(&mut self, handlers: InternalToolHandlers) {
        self.internal_tool_handlers = handlers;
        let handlers = self.internal_tool_handlers.clone();
        let mut host = InternalToolHost::new(self);
        for handler in handlers {
            for spec in handler.tool_specs() {
                host.register_internal_tool(spec);
            }
        }
    }

    pub(crate) fn dispatch_internal_tool_started(
        &mut self,
        started: ToolStarted,
    ) -> Result<(), HarnessError> {
        let Some(cid) = self.tool_conversations.get(&started.call_id).cloned() else {
            self.emit_info(&format!(
                "discarding internal tool.started for unknown call_id={}",
                started.call_id
            ));
            return Ok(());
        };
        let Some(pending) = self.pending_tools.get(&started.call_id).cloned() else {
            self.emit_info(&format!(
                "discarding internal tool.started without pending tool for call_id={}",
                started.call_id
            ));
            return Ok(());
        };
        let call = AgentToolCall {
            id: started.call_id,
            name: pending.internal_name.clone(),
            tool_type: pending.tool_type,
            arguments: started.arguments,
            display: None,
        };
        let Some(handler) = self
            .internal_tool_handlers
            .iter()
            .find(|handler| handler.handles(&pending.internal_name))
            .cloned()
        else {
            self.emit_info(&format!(
                "discarding internal tool.started for unhandled internal tool `{}`",
                pending.internal_name
            ));
            return Ok(());
        };
        let mut host = InternalToolHost::new(self);
        handler.handle_started(&mut host, &cid, &call, pending.name)
    }
}
