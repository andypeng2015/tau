//! Live-only shell interaction admission at the extension endpoint.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tau_shell_interaction::{Operation, Reply, Request, Response, RunningCommand};

use super::*;
use crate::scheduler::WorkPriority;
use crate::user_shell_output::UserShellOutput;

#[cfg(test)]
mod tests;

impl ShellRuntime {
    /// Retain terminal-producing queued jobs while cancelling their effects.
    /// Active workers observe the same flag plus their process cancellation
    /// channel; no unload path resubmits execution.
    pub(super) fn cancel_user_commands_for_agent(&self, agent: &tau_proto::AgentId) {
        let registry = self.user_commands.lock().expect("user command registry");
        let running = self
            .running_ui_commands
            .lock()
            .expect("running user commands");
        for active in registry.active.values() {
            if active.snapshot.command.target_agent_id.as_ref() == Some(agent) {
                active.cancelled.store(true, Ordering::Release);
                if let Some(cancel) = running.get(&active.snapshot.command.command_id) {
                    let _ = cancel.send(());
                }
            }
        }
    }

    /// Snapshot the extension's remembered workdir and enumerate off the
    /// protocol loop. This is the extension host namespace, not a configured
    /// shell wrapper/container namespace.
    fn complete_interaction(
        &self,
        request: &Request,
        token: &str,
        matching: tau_shell_interaction::CompletionKind,
    ) -> tau_client::ClientResult<()> {
        if token.len() > tau_shell_interaction::TOKEN_LIMIT {
            return Ok(());
        }
        let cwd = match request.context.agent_id.as_ref() {
            Some(agent)
                if self.cwd_state.initialization(agent).is_some()
                    && self.cwd_state.pending_ready(agent).is_none() =>
            {
                self.cwd_state.get_or_default(agent)
            }
            Some(_) => {
                return self.reply(
                    request,
                    Response::Completed {
                        candidates: Vec::new(),
                    },
                );
            }
            None => self.cwd_state.process_default(),
        };
        let Ok(cwd) = cwd else {
            return self.reply(
                request,
                Response::Completed {
                    candidates: Vec::new(),
                },
            );
        };
        let context = request.context.clone();
        let target = self.interaction_target.clone();
        let token = token.to_owned();
        let output = self.tx.clone();
        let deadline = Instant::now() + Duration::from_millis(200);
        let job = self.scheduler()?.enqueue(
            WorkPriority::Cheap,
            crate::scheduler::WorkMeta {
                call_id: None,
                agent_id: request.context.agent_id.clone(),
                queued_bytes: token.len(),
            },
            move || {
                let candidates = crate::path_completion::complete(&cwd, &token, matching, deadline);
                let _ = output.send(tau_shell_interaction::reply_message(&Reply {
                    context,
                    target,
                    response: Response::Completed { candidates },
                }));
            },
        );
        if job.is_err() {
            self.reply(
                request,
                Response::Completed {
                    candidates: Vec::new(),
                },
            )?;
        }
        Ok(())
    }

    /// Validate publisher and process targeting before interpreting a UI
    /// request.
    pub(crate) fn handle_interaction(
        &mut self,
        delivery: &tau_proto::EventDelivery,
    ) -> tau_client::ClientResult<()> {
        if delivery.replay {
            return Ok(());
        }
        let Some(tau_proto::EventPublisher::Ui { connection_id }) = &delivery.publisher else {
            return Ok(());
        };
        let Some(request) = tau_shell_interaction::decode_request(&delivery.event) else {
            return Ok(());
        };
        if self.bound_session_id.as_ref() != Some(&request.context.session_id) {
            return Ok(());
        }
        if matches!(request.operation, Operation::Discover) {
            if request.target.is_none() {
                self.reply(
                    &request,
                    Response::Available {
                        execute: true,
                        complete: true,
                    },
                )?;
            }
            return Ok(());
        }
        if request.target.as_ref() != Some(&self.interaction_target) {
            // In particular, do not execute intent delayed until a replacement
            // process subscribes. There is deliberately no default target.
            return Ok(());
        }
        match &request.operation {
            Operation::Execute {
                command,
                include_in_context,
            } => self.execute_interaction(&request, connection_id, command, *include_in_context),
            Operation::Cancel { command_id } => {
                let registry = self.user_commands.lock().expect("user command registry");
                if let Some(active) = registry.active.get(command_id)
                    && &active.snapshot.owner == connection_id
                    && active.snapshot.command.target_agent_id == request.context.agent_id
                {
                    active.cancelled.store(true, Ordering::Release);
                    if let Some(cancel) = self
                        .running_ui_commands
                        .lock()
                        .expect("running user commands")
                        .get(&active.snapshot.command.command_id)
                    {
                        let _ = cancel.send(());
                    }
                }
                Ok(())
            }
            Operation::Running => {
                let commands: Vec<_> = self
                    .user_commands
                    .lock()
                    .expect("user command registry")
                    .active
                    .values()
                    .filter(|active| active.pending_lease.is_none())
                    .map(|active| active.snapshot.clone())
                    .collect();
                for command in commands {
                    let mut scoped = request.clone();
                    scoped.context.agent_id = command.command.target_agent_id.clone();
                    self.reply(&scoped, Response::Running { command })?;
                }
                self.reply(&request, Response::RunningComplete)
            }
            Operation::Complete { token, matching } => {
                self.complete_interaction(&request, token, *matching)
            }
            Operation::Discover => Ok(()),
        }
    }

    /// Deduplicate before queueing or publishing acceptance. A command ID is
    /// never evicted and re-executed within the advertised process incarnation.
    fn execute_interaction(
        &self,
        request: &Request,
        owner: &tau_proto::ConnectionId,
        command: &str,
        include_in_context: bool,
    ) -> tau_client::ClientResult<()> {
        if self
            .user_commands
            .lock()
            .expect("user command registry")
            .seen
            .get(&request.context.request_id)
            == Some(owner)
        {
            // The original may still await its activity acknowledgement.
            // A rejection here would settle that operation before its real
            // acceptance; a duplicate must not publish a second outcome.
            return Ok(());
        }
        if command.is_empty()
            || command.len() > tau_shell_interaction::COMMAND_LIMIT
            || (include_in_context && request.context.agent_id.is_none())
            || request
                .context
                .agent_id
                .as_ref()
                .is_some_and(|agent| self.cwd_state.initialization(agent).is_none())
        {
            return self.reply(
                request,
                Response::Rejected {
                    reason:
                        "invalid command or unavailable target agent (! requires a loaded agent)"
                            .to_owned(),
                },
            );
        }
        let Ok(command_id) = request.context.request_id.parse() else {
            return Ok(());
        };
        let snapshot = RunningCommand {
            owner: owner.clone(),
            command: tau_proto::UiShellCommand {
                session_id: request.context.session_id.clone(),
                command_id,
                command: command.to_owned(),
                include_in_context,
                target_agent_id: request.context.agent_id.clone(),
            },
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        {
            let mut registry = self.user_commands.lock().expect("user command registry");
            if registry.seen.contains_key(&request.context.request_id)
                || registry.seen.len() >= tau_shell_interaction::OPERATION_LIMIT
                || registry.active.len() >= tau_shell_interaction::RUNNING_LIMIT
            {
                drop(registry);
                return self.reply(request, Response::Rejected {
                    reason: "command ID already admitted or executor capacity exhausted; command was not resubmitted".to_owned(),
                });
            }
            registry
                .seen
                .insert(request.context.request_id.clone(), owner.clone());
            registry.active.insert(
                request.context.request_id.clone(),
                crate::user_shell_output::ActiveUserCommand {
                    pending_lease: Some(request.clone()),
                    snapshot: snapshot.clone(),
                    cancelled: cancelled.clone(),
                },
            );
        }
        self.tx.send_checked(HarnessInputMessage::emit_with_persist(
            Event::ExtActivityLeaseRequest(tau_proto::ExtActivityLeaseRequest {
                session_id: request.context.session_id.clone(),
                operation_id: format!(
                    "{}:{}",
                    self.interaction_target.nonce, request.context.request_id
                ),
                action: tau_proto::ActivityLeaseAction::Acquire,
            }),
            false,
        ))
    }

    /// Only authenticated live acquire acknowledgements authorize execution.
    /// Result loss leaves effects unstarted; it never causes a submission
    /// retry.
    pub(crate) fn handle_activity_lease_result(
        &mut self,
        delivery: &tau_proto::EventDelivery,
    ) -> tau_client::ClientResult<()> {
        if delivery.replay || delivery.publisher != Some(tau_proto::EventPublisher::Harness) {
            return Ok(());
        }
        let Event::ExtActivityLeaseResult(result) = delivery.event.as_ref() else {
            return Ok(());
        };
        if result.publisher != self.interaction_target.instance
            || self.bound_session_id.as_ref() != Some(&result.session_id)
            || result.action != tau_proto::ActivityLeaseAction::Acquire
        {
            return Ok(());
        }
        let prefix = format!("{}:", self.interaction_target.nonce);
        let Some(id) = result.operation_id.strip_prefix(&prefix) else {
            return Ok(());
        };
        let (request, snapshot, cancelled) = {
            let mut registry = self.user_commands.lock().expect("user command registry");
            let Some(active) = registry.active.get_mut(id) else {
                return Ok(());
            };
            let Some(request) = active.pending_lease.take() else {
                return Ok(());
            };
            let snapshot = active.snapshot.clone();
            let cancelled = active.cancelled.clone();
            if !matches!(result.outcome, tau_proto::ActivityLeaseOutcome::Acquired) {
                registry.active.remove(id);
            }
            (request, snapshot, cancelled)
        };
        if !matches!(result.outcome, tau_proto::ActivityLeaseOutcome::Acquired) {
            return self.reply(
                &request,
                Response::Rejected {
                    reason: "activity lease was not acquired; command was not started".to_owned(),
                },
            );
        }
        self.reply(&request, Response::Started(snapshot.clone()))?;
        let mut output = self.tx.clone();
        output.user_shell = Some(crate::user_shell_output::UserShellOutput {
            context: request.context.clone(),
            target: self.interaction_target.clone(),
            registry: self.user_commands.clone(),
        });
        self.handle_scoped_ui_shell_command(snapshot.command, &output, cancelled)
    }

    /// Context success and failure both settle activity; neither reruns
    /// effects.
    pub(crate) fn handle_passive_context_result(
        &mut self,
        delivery: &tau_proto::EventDelivery,
    ) -> tau_client::ClientResult<()> {
        if delivery.replay || delivery.publisher != Some(tau_proto::EventPublisher::Harness) {
            return Ok(());
        }
        let Event::ExtPassiveContextAppendResult(result) = delivery.event.as_ref() else {
            return Ok(());
        };
        if result.publisher != self.interaction_target.instance
            || self.bound_session_id.as_ref() != Some(&result.session_id)
        {
            return Ok(());
        }
        let mut registry = self.user_commands.lock().expect("user command registry");
        if registry
            .pending_context
            .get(&result.operation_id)
            .is_some_and(|context| context.agent_id.as_ref() == Some(&result.agent_id))
        {
            let context = registry
                .pending_context
                .remove(&result.operation_id)
                .expect("checked pending context");
            drop(registry);
            self.tx.send_checked(UserShellOutput::release_message(
                &context,
                result.operation_id.clone(),
            ))?;
        }
        Ok(())
    }

    /// Publish one checked endpoint response in ordinary custom-event order.
    fn reply(&self, request: &Request, response: Response) -> tau_client::ClientResult<()> {
        self.tx
            .send_checked(tau_shell_interaction::reply_message(&Reply {
                context: request.context.clone(),
                target: self.interaction_target.clone(),
                response,
            }))
    }
}
