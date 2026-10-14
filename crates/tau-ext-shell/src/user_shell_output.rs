//! Scoped endpoint output for one explicitly addressed UI command.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use tau_proto::{Event, HarnessInputMessage};
use tau_shell_interaction::{Context, Reply, Response, RunningCommand, Target};

/// Active execution state shared by the protocol loop and command workers.
pub(crate) struct ActiveUserCommand {
    /// Unaccepted request awaiting the generic lease acknowledgement.
    pub(crate) pending_lease: Option<tau_shell_interaction::Request>,
    /// Immutable request and original authenticated UI owner.
    pub(crate) snapshot: RunningCommand,
    /// Sticky cancellation, including while queued before a worker starts.
    pub(crate) cancelled: Arc<AtomicBool>,
}

/// Bounded process-local registry. Completed IDs remain tombstones until exit.
#[derive(Default)]
pub(crate) struct UserCommandRegistry {
    /// Original owner of every admitted ID; never evicted within a process.
    pub(crate) seen: HashMap<String, tau_proto::ConnectionId>,
    /// Commands still queued or executing.
    pub(crate) active: HashMap<String, ActiveUserCommand>,
    /// Completed commands whose attributed context has not settled yet.
    pub(crate) pending_context: HashMap<String, Context>,
}

/// Converts worker-local output into the extension-owned interaction protocol.
#[derive(Clone)]
pub(crate) struct UserShellOutput {
    /// Original session/agent/request correlation.
    pub(crate) context: Context,
    /// Exact configured instance/process selected by the UI.
    pub(crate) target: Target,
    /// Registry whose active command ends after successful result publication.
    pub(crate) registry: Arc<Mutex<UserCommandRegistry>>,
}

impl UserShellOutput {
    /// Wrap bounded progress without changing its captured correlation.
    pub(crate) fn progress(
        &self,
        progress: tau_proto::ShellCommandProgress,
    ) -> HarnessInputMessage {
        tau_shell_interaction::reply_message(&Reply {
            context: self.context.clone(),
            target: self.target.clone(),
            response: Response::Progress(progress),
        })
    }

    /// Send UI completion first, then independently request passive context.
    /// Output failure never reschedules the command.
    pub(crate) fn finish(
        &self,
        output: &crate::Output,
        finished: tau_proto::ShellCommandFinished,
    ) -> tau_client::ClientResult<()> {
        if self
            .registry
            .lock()
            .expect("user command registry")
            .active
            .remove(&self.context.request_id)
            .is_none()
        {
            return Ok(());
        }
        output.send_wire_checked(tau_shell_interaction::reply_message(&Reply {
            context: self.context.clone(),
            target: self.target.clone(),
            response: Response::Finished(finished.clone()),
        }))?;
        if finished.include_in_context
            && let Some(agent_id) = self.context.agent_id.clone()
        {
            self.registry
                .lock()
                .expect("user command registry")
                .pending_context
                .insert(self.operation_id(), self.context.clone());
            let exit = finished.exit_code.map_or_else(
                || {
                    if finished.cancelled {
                        "cancelled"
                    } else {
                        "unknown"
                    }
                    .to_owned()
                },
                |code| code.to_string(),
            );
            let body = format!(
                "Command: {}\nExit status: {exit}\n\n{}",
                finished.command, finished.output,
            );
            output.send_wire_checked(HarnessInputMessage::emit_with_persist(
                Event::ExtPassiveContextAppendRequest(tau_proto::ExtPassiveContextAppendRequest {
                    session_id: self.context.session_id.clone(),
                    agent_id,
                    operation_id: self.operation_id(),
                    body,
                }),
                false,
            ))?;
        } else {
            output.send_wire_checked(Self::release_message(&self.context, self.operation_id()))?;
        }
        Ok(())
    }

    /// Release generic activity only after command and context settlement.
    pub(crate) fn release_message(context: &Context, operation_id: String) -> HarnessInputMessage {
        HarnessInputMessage::emit_with_persist(
            Event::ExtActivityLeaseRequest(tau_proto::ExtActivityLeaseRequest {
                session_id: context.session_id.clone(),
                operation_id,
                action: tau_proto::ActivityLeaseAction::Release,
            }),
            false,
        )
    }

    /// Keep append deduplication independent of extension restarts.
    pub(crate) fn operation_id(&self) -> String {
        format!("{}:{}", self.target.nonce, self.context.request_id)
    }
}
