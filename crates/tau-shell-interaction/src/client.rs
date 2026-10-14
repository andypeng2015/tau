//! Transport-independent UI endpoint state and authenticated reply validation.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use tau_proto::{EventDelivery, EventPublisher};

use super::*;

/// One authenticated, currently advertised shell-like process.
#[derive(Clone, Debug)]
pub struct Endpoint {
    /// Explicit command/completion recipient.
    pub target: Target,
    /// Immutable generic publisher metadata expected on every reply.
    pub publisher: EventPublisher,
    /// This process accepts shell execution.
    pub execute: bool,
    /// This process performs path enumeration.
    pub complete: bool,
}

/// Validated observations for the UI's renderer and asynchronous completion
/// cache.
#[derive(Default)]
pub struct ClientUpdate {
    /// This live passive append is already represented by a validated terminal
    /// card. Historical generic append facts are never suppressed.
    pub suppress_event: bool,
    /// Presentation-only legacy DTO projections; never publish them to the
    /// harness.
    pub events: Vec<Event>,
    /// New discovery/running queries, never automatic execution retries.
    pub requests: Vec<Request>,
    /// Correlated token-completion responses; the caller still checks draft
    /// state.
    pub completions: Vec<(Context, Target, Vec<PathCandidate>)>,
    /// User-facing diagnostics, including distinct passive append failure.
    pub diagnostics: Vec<String>,
}

/// Immutable command ownership plus local terminal deduplication.
struct TrackedCommand {
    /// Exact process that accepted or was explicitly selected for this command.
    endpoint: Endpoint,
    /// Original command before instance-labelled display projection.
    command: UiShellCommand,
    /// Whether this UI has observed extension acceptance.
    accepted: bool,
    /// Whether a terminal or unconfirmed-outcome notice was already shown.
    terminal: bool,
    /// A real endpoint result, unlike a local disconnect/rejection, was
    /// rendered.
    finished_observed: bool,
    /// Whether silence was already reported while allowing a late real result.
    acceptance_notified: bool,
    /// Time after which silence must be shown as unconfirmed, never as
    /// nonexecution.
    acceptance_deadline: Instant,
}

/// UI-owned shell inventory, correlations, and terminal deduplication.
pub struct Client {
    /// One immutable harness session for this attachment.
    session_id: SessionId,
    /// Latest bounded discovery round and its response deadline.
    discovery: Option<(String, Instant)>,
    /// Latest lifecycle observation per configured name.
    lifecycle: HashMap<ExtensionName, (tau_proto::ExtensionInstanceId, bool)>,
    /// Advertisements authenticated by generic publisher metadata.
    endpoints: BTreeMap<ExtensionName, Endpoint>,
    /// Local pending and observed operations. Tombstones prevent duplicate
    /// display.
    commands: HashMap<String, TrackedCommand>,
    /// Outstanding running-state queries for newly discovered incarnations.
    running_queries: HashMap<String, Target>,
}

impl Client {
    /// Create an empty attachment-local inventory. Discovery is explicit.
    pub fn new(session_id: SessionId) -> Self {
        Self {
            session_id,
            discovery: None,
            lifecycle: HashMap::new(),
            endpoints: BTreeMap::new(),
            commands: HashMap::new(),
            running_queries: HashMap::new(),
        }
    }

    /// Query after subscribing, and again after lifecycle readiness changes.
    pub fn discover(&mut self) -> Request {
        let request = Request {
            context: self.context(None),
            target: None,
            operation: Operation::Discover,
        };
        self.discovery = Some((
            request.context.request_id.clone(),
            Instant::now() + Duration::from_secs(5),
        ));
        request
    }

    /// Return the current explicitly named capabilities.
    pub fn endpoints(&self) -> impl Iterator<Item = &Endpoint> {
        self.endpoints.values()
    }

    /// Create a fresh correlation without transmitting a prompt draft.
    pub fn context(&self, agent_id: Option<AgentId>) -> Context {
        Context {
            session_id: self.session_id.clone(),
            agent_id,
            request_id: nonce(),
        }
    }

    /// Resolve mandatory syntax once and register pending ownership before
    /// send.
    pub fn execute(&mut self, line: &str, agent_id: Option<AgentId>) -> Result<Request, String> {
        let (instance, command, include_in_context) = parse_command(line).map_err(str::to_owned)?;
        let endpoint = self
            .endpoints
            .get(&instance)
            .filter(|endpoint| endpoint.execute)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "shell instance `{instance}` is unavailable; no default executor is selected"
                )
            })?;
        if include_in_context && agent_id.is_none() {
            return Err(
                "! requires a selected loaded agent; use !!instance command for UI-only output"
                    .to_owned(),
            );
        }
        if self.commands.len() >= OPERATION_LIMIT {
            return Err("this UI's shell-operation capacity is exhausted; reattach to start a fresh observation window".to_owned());
        }
        let context = self.context(agent_id);
        let request = Request {
            context: context.clone(),
            target: Some(endpoint.target.clone()),
            operation: Operation::Execute {
                command: command.to_owned(),
                include_in_context,
            },
        };
        self.commands.insert(
            context.request_id.clone(),
            TrackedCommand {
                endpoint,
                command: UiShellCommand {
                    session_id: context.session_id,
                    command_id: context.request_id.parse().expect("nonce is a command ID"),
                    command: command.to_owned(),
                    include_in_context,
                    target_agent_id: context.agent_id,
                },
                accepted: false,
                terminal: false,
                finished_observed: false,
                acceptance_notified: false,
                acceptance_deadline: Instant::now() + Duration::from_secs(5),
            },
        );
        Ok(request)
    }

    /// Request cancellation of this UI's known active work for an agent.
    /// The executor independently checks the original connection owner.
    pub fn cancel(&self, agent_id: Option<&AgentId>) -> Vec<Request> {
        self.commands
            .iter()
            .filter(|(_, command)| {
                !command.terminal && command.command.target_agent_id.as_ref() == agent_id
            })
            .map(|(id, command)| Request {
                context: self.context(command.command.target_agent_id.clone()),
                target: Some(command.endpoint.target.clone()),
                operation: Operation::Cancel {
                    command_id: id.clone(),
                },
            })
            .collect()
    }

    /// Mark timed-out acceptance as unconfirmed without retrying execution.
    pub fn tick(&mut self) -> ClientUpdate {
        let mut update = ClientUpdate::default();
        for command in self.commands.values_mut() {
            if !command.accepted
                && !command.terminal
                && !command.acceptance_notified
                && Instant::now() >= command.acceptance_deadline
            {
                command.acceptance_notified = true;
                update.diagnostics.push(format!(
                    "{}: command outcome unconfirmed (no acceptance); execution was not retried",
                    command.endpoint.target.instance,
                ));
            }
        }
        update
    }

    /// Fold authenticated lifecycle and custom replies. Historical custom
    /// frames never restore execution or command ownership.
    pub fn observe(&mut self, delivery: &EventDelivery) -> ClientUpdate {
        self.observe_parts(
            &delivery.event,
            delivery.publisher.as_ref(),
            delivery.replay,
        )
    }

    /// Observe a borrowed delivery without copying its potentially large
    /// payload.
    pub fn observe_parts(
        &mut self,
        event: &Event,
        publisher: Option<&EventPublisher>,
        replay: bool,
    ) -> ClientUpdate {
        let mut update = ClientUpdate::default();
        match event {
            Event::AgentPassiveContextAppended(appended)
                if !replay
                    && publisher == Some(&EventPublisher::Harness)
                    && appended.session_id == self.session_id =>
            {
                update.suppress_event = self.commands.values().any(|command| {
                    command.finished_observed
                        && command.command.target_agent_id.as_ref() == Some(&appended.agent_id)
                        && command.endpoint.target.instance == appended.publisher
                        && format!(
                            "{}:{}",
                            command.endpoint.target.nonce, command.command.command_id
                        ) == appended.operation_id
                });
            }
            Event::ExtensionReady(ready) => {
                self.lifecycle
                    .insert(ready.extension_name.clone(), (ready.instance_id, true));
                update.requests.push(self.discover());
            }
            Event::ExtensionExited(exited) => {
                self.lifecycle
                    .insert(exited.extension_name.clone(), (exited.instance_id, false));
                self.retire(&exited.extension_name, &mut update);
            }
            Event::ExtPassiveContextAppendResult(result)
                if !replay
                    && publisher == Some(&EventPublisher::Harness)
                    && result.session_id == self.session_id =>
            {
                if let tau_proto::PassiveContextAppendOutcome::Failed { reason } = &result.outcome
                    && self.commands.values().any(|command| {
                        command.endpoint.target.instance == result.publisher
                            && format!(
                                "{}:{}",
                                command.endpoint.target.nonce, command.command.command_id
                            ) == result.operation_id
                            && command.command.target_agent_id.as_ref() == Some(&result.agent_id)
                    })
                {
                    update.diagnostics.push(format!(
                        "{}: command completed; context insertion failed: {reason}",
                        result.publisher,
                    ));
                }
            }
            _ => {}
        }
        if replay {
            return update;
        }
        let Some(reply) = decode_reply(event) else {
            return update;
        };
        if reply.context.session_id != self.session_id {
            return update;
        }
        let Some(
            publisher @ EventPublisher::Extension {
                name, instance_id, ..
            },
        ) = publisher
        else {
            return update;
        };
        if name != &reply.target.instance
            || reply.target.nonce.len() != 32
            || !reply
                .target
                .nonce
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || self
                .lifecycle
                .get(name)
                .is_some_and(|(id, live)| id != instance_id || !live)
        {
            return update;
        }
        if let Response::Available { execute, complete } = reply.response {
            if !self.discovery.as_ref().is_some_and(|(id, deadline)| {
                id == &reply.context.request_id && Instant::now() < *deadline
            }) {
                return update;
            }
            let unchanged = self
                .endpoints
                .get(name)
                .is_some_and(|known| known.target == reply.target && known.publisher == *publisher);
            if unchanged {
                return update;
            }
            let changed = self
                .endpoints
                .get(name)
                .is_some_and(|known| known.target != reply.target || known.publisher != *publisher);
            if changed {
                self.retire(name, &mut update);
            }
            self.endpoints.insert(
                name.clone(),
                Endpoint {
                    target: reply.target.clone(),
                    publisher: publisher.clone(),
                    execute,
                    complete,
                },
            );
            let query = Request {
                context: self.context(None),
                target: Some(reply.target),
                operation: Operation::Running,
            };
            self.running_queries.insert(
                query.context.request_id.clone(),
                query.target.clone().expect("running query target"),
            );
            update.requests.push(query);
            return update;
        }
        let Some(endpoint) = self
            .endpoints
            .get(name)
            .filter(|known| known.target == reply.target && known.publisher == *publisher)
            .cloned()
        else {
            return update;
        };
        match reply.response {
            Response::Started(started) => {
                if command_context_matches(&started.command, &reply.context) {
                    self.accept_started(&endpoint, started.command, &mut update);
                }
            }
            Response::Running { command }
                if self.running_queries.get(&reply.context.request_id) == Some(&reply.target)
                    && command.command.session_id == reply.context.session_id
                    && command.command.target_agent_id == reply.context.agent_id =>
            {
                self.accept_started(&endpoint, command.command, &mut update);
            }
            Response::RunningComplete if reply.context.agent_id.is_none() => {
                if self.running_queries.get(&reply.context.request_id) == Some(&reply.target) {
                    self.running_queries.remove(&reply.context.request_id);
                }
            }
            Response::Progress(progress) => {
                if let Some(command) = self.commands.get(progress.command_id.as_str())
                    && command.accepted
                    && !command.terminal
                    && command.endpoint.target == reply.target
                    && command_context_matches(&command.command, &reply.context)
                    && progress.target_agent_id == command.command.target_agent_id
                    && progress.chunk.len() <= 16 * 1024
                {
                    update.events.push(Event::ShellCommandProgress(progress));
                }
            }
            Response::Finished(mut finished) => {
                if let Some(command) = self.commands.get_mut(finished.command_id.as_str())
                    && !command.terminal
                    && command.endpoint.target == reply.target
                    && command_context_matches(&command.command, &reply.context)
                    && finished.session_id == command.command.session_id
                    && finished.target_agent_id == command.command.target_agent_id
                    && finished.command == command.command.command
                    && finished.include_in_context == command.command.include_in_context
                    && finished.output.len() <= 64 * 1024
                {
                    if !command.accepted {
                        update
                            .events
                            .push(display_start(&command.endpoint, &command.command));
                    }
                    command.terminal = true;
                    command.finished_observed = true;
                    finished.command = format!("{} {}", reply.target.instance, finished.command);
                    update.events.push(Event::ShellCommandFinished(finished));
                }
            }
            Response::Rejected { reason } => {
                if let Some(command) = self.commands.get_mut(&reply.context.request_id)
                    && !command.accepted
                    && !command.terminal
                    && command.endpoint.target == reply.target
                    && command_context_matches(&command.command, &reply.context)
                {
                    command.terminal = true;
                    update.diagnostics.push(format!(
                        "{}: {}",
                        reply.target.instance,
                        reason.chars().take(512).collect::<String>()
                    ));
                }
            }
            Response::Completed { candidates }
                if candidates.len() <= COMPLETION_LIMIT
                    && candidates.iter().all(|candidate| {
                        candidate.value.len() <= TOKEN_LIMIT
                            && !candidate.value.chars().any(char::is_control)
                    }) =>
            {
                update
                    .completions
                    .push((reply.context, reply.target, candidates));
            }
            _ => {}
        }
        update
    }

    fn accept_started(
        &mut self,
        endpoint: &Endpoint,
        command: UiShellCommand,
        update: &mut ClientUpdate,
    ) {
        if command.command.is_empty() || command.command.len() > COMMAND_LIMIT {
            return;
        }
        if !self.commands.contains_key(command.command_id.as_str()) {
            if self.commands.len() >= OPERATION_LIMIT {
                return;
            }
            self.commands.insert(
                command.command_id.to_string(),
                TrackedCommand {
                    endpoint: endpoint.clone(),
                    command: command.clone(),
                    accepted: false,
                    terminal: false,
                    finished_observed: false,
                    acceptance_notified: false,
                    acceptance_deadline: Instant::now() + Duration::from_secs(5),
                },
            );
        }
        let known = self
            .commands
            .get_mut(command.command_id.as_str())
            .expect("command inserted");
        if known.command == command
            && known.endpoint.target == endpoint.target
            && !known.accepted
            && !known.terminal
        {
            known.accepted = true;
            update.events.push(display_start(endpoint, &command));
        }
    }

    fn retire(&mut self, name: &ExtensionName, update: &mut ClientUpdate) {
        self.endpoints.remove(name);
        self.running_queries
            .retain(|_, target| &target.instance != name);
        for command in self.commands.values_mut() {
            if &command.endpoint.target.instance == name && !command.terminal {
                command.terminal = true;
                update.diagnostics.push(format!(
                    "{name}: command disconnected; outcome unknown, execution was not retried"
                ));
                if command.accepted {
                    update.events.push(Event::ShellCommandFinished(ShellCommandFinished {
                        session_id: command.command.session_id.clone(),
                        command_id: command.command.command_id.clone(),
                        target_agent_id: command.command.target_agent_id.clone(),
                        command: format!("{name} {}", command.command.command),
                        include_in_context: command.command.include_in_context,
                        output: "Executor disconnected. Execution outcome unknown; the command was not retried.".to_owned(),
                        exit_code: None,
                        cancelled: false,
                    }));
                }
            }
        }
    }
}

fn command_context_matches(command: &UiShellCommand, context: &Context) -> bool {
    command.command_id.as_str() == context.request_id
        && command.session_id == context.session_id
        && command.target_agent_id == context.agent_id
}

fn display_start(endpoint: &Endpoint, command: &UiShellCommand) -> Event {
    let mut command = command.clone();
    command.command = format!("{} {}", endpoint.target.instance, command.command);
    Event::UiShellCommand(command)
}
