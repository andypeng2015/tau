//! Tool registry: dispatches a `ToolStarted` to the right handler.

use tau_proto::{CborValue, Event, ToolError, ToolProgress, ToolResult, ToolResultKind};

use crate::config::ShellConfig;
use crate::display::{ToolFailure, ToolOutput};

pub(crate) mod apply_patch;
pub(crate) mod edit;
pub(crate) mod find;
pub(crate) mod grep;
pub(crate) mod ls;
pub(crate) mod read;
pub(crate) mod shell;

#[cfg(any(test, feature = "echo-agent"))]
pub const ECHO_TOOL_NAME: &str = "echo";
pub const READ_TOOL_NAME: &str = "read";
pub const EDIT_TOOL_NAME: &str = "edit";
pub const APPLY_PATCH_TOOL_NAME: &str = "apply_patch";
pub const SHELL_TOOL_NAME: &str = "shell";
pub const GPT_SHELL_TOOL_NAME: &str = "gpt_shell";
pub const GREP_TOOL_NAME: &str = "grep";
pub const FIND_TOOL_NAME: &str = "find";
pub const LS_TOOL_NAME: &str = "ls";

/// Execute a tool and return the response event(s).
pub(crate) fn execute_tool(
    invoke: tau_proto::ToolStarted,
    shell_config: &ShellConfig,
) -> Vec<Event> {
    #[cfg(any(test, feature = "echo-agent"))]
    if invoke.tool_name == ECHO_TOOL_NAME {
        return vec![Event::ToolResult(ToolResult {
            call_id: invoke.call_id,
            tool_name: invoke.tool_name,
            tool_type: tau_proto::ToolType::Function,
            result: invoke.arguments,
            kind: ToolResultKind::Final,
            display: None,
            originator: invoke.originator.clone(),
        })];
    }

    if invoke.tool_name == READ_TOOL_NAME {
        return wrap_pure(invoke, read::read_file);
    }
    if invoke.tool_name == EDIT_TOOL_NAME {
        return wrap_pure(invoke, edit::edit_file);
    }
    if invoke.tool_name == APPLY_PATCH_TOOL_NAME {
        return wrap_pure(invoke, apply_patch::apply_patch);
    }
    if invoke.tool_name == GREP_TOOL_NAME {
        return wrap_pure(invoke, grep::run_grep);
    }
    if invoke.tool_name == FIND_TOOL_NAME {
        return wrap_pure(invoke, find::run_find);
    }
    if invoke.tool_name == LS_TOOL_NAME {
        return wrap_pure(invoke, ls::run_ls);
    }

    if invoke.tool_name == SHELL_TOOL_NAME || invoke.tool_name == GPT_SHELL_TOOL_NAME {
        let mut events = vec![Event::ToolProgress(ToolProgress {
            call_id: invoke.call_id.clone(),
            tool_name: invoke.tool_name.clone(),
            message: Some("running shell command".to_owned()),
            progress: None,
            display: None,
        })];
        match shell::run_command(&invoke.arguments, shell_config) {
            Ok(ToolOutput { result, display }) => events.push(Event::ToolResult(ToolResult {
                call_id: invoke.call_id,
                tool_name: invoke.tool_name,
                tool_type: tau_proto::ToolType::Function,
                result,
                kind: ToolResultKind::Final,
                display: Some(display),
                originator: invoke.originator.clone(),
            })),
            Err(ToolFailure {
                message,
                details,
                display,
            }) => events.push(Event::ToolError(ToolError {
                call_id: invoke.call_id,
                tool_name: invoke.tool_name,
                tool_type: tau_proto::ToolType::Function,
                message,
                details: details.map(|details| *details),
                display: Some(*display),
                originator: invoke.originator.clone(),
            })),
        }
        return events;
    }

    vec![Event::ToolError(ToolError {
        call_id: invoke.call_id,
        tool_name: invoke.tool_name,
        tool_type: tau_proto::ToolType::Function,
        message: "unknown tool".to_owned(),
        details: None,
        display: None,
        originator: invoke.originator.clone(),
    })]
}

/// Common Ok/Err → Result/Error wrapping for tools whose handler is a
/// pure `(arguments) -> Result<ToolOutput, ToolFailure>`. The handler's
/// display descriptor and purpose-built failure details are forwarded to
/// the event.
fn wrap_pure(
    invoke: tau_proto::ToolStarted,
    handler: fn(&CborValue) -> Result<ToolOutput, ToolFailure>,
) -> Vec<Event> {
    match handler(&invoke.arguments) {
        Ok(ToolOutput { result, display }) => vec![Event::ToolResult(ToolResult {
            call_id: invoke.call_id,
            tool_name: invoke.tool_name,
            tool_type: tau_proto::ToolType::Function,
            result,
            kind: ToolResultKind::Final,
            display: Some(display),
            originator: invoke.originator.clone(),
        })],
        Err(ToolFailure {
            message,
            details,
            display,
        }) => vec![Event::ToolError(ToolError {
            call_id: invoke.call_id,
            tool_name: invoke.tool_name,
            tool_type: tau_proto::ToolType::Function,
            message,
            details: details.map(|details| *details),
            display: Some(*display),
            originator: invoke.originator.clone(),
        })],
    }
}
