//! Injection point for harness-internal tools owned by higher crates.

use std::sync::Arc;

use tau_proto::{ToolName, ToolSpec, ToolStarted};

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

    /// Handle the built-in `skill` tool.
    pub fn handle_skill_tool_call(
        &mut self,
        conversation_id: &ConversationId,
        call: &AgentToolCall,
    ) -> Result<(), HarnessError> {
        self.harness.handle_skill_tool_call(conversation_id, call)
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

    /// Handle the built-in `message` tool.
    pub fn handle_message_tool_call(
        &mut self,
        conversation_id: &ConversationId,
        call: &AgentToolCall,
        visible_tool_name: ToolName,
    ) -> Result<(), HarnessError> {
        self.harness
            .handle_message_tool_call(conversation_id, call, visible_tool_name)
    }

    /// Handle the built-in `cancel` tool.
    pub fn handle_cancel_tool_call(
        &mut self,
        conversation_id: &ConversationId,
        call: &AgentToolCall,
        visible_tool_name: ToolName,
    ) -> Result<(), HarnessError> {
        self.harness
            .handle_cancel_tool_call(conversation_id, call, visible_tool_name)
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
