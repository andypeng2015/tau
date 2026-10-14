use tau_shell_interaction::Context;

use super::*;

/// Create a UI-attributed live request without bypassing endpoint validation.
fn delivered(request: &Request) -> tau_proto::EventDelivery {
    let HarnessInputMessage::Emit(emit) = tau_shell_interaction::request_message(request) else {
        unreachable!()
    };
    let mut delivery = tau_proto::EventDelivery::live(tau_proto::UnixMicros::new(1), *emit.event);
    delivery.publisher = Some(tau_proto::EventPublisher::Ui {
        connection_id: "ui".parse().expect("UI connection"),
    });
    delivery
}

/// Model the harness acknowledgement separately from execution submission.
fn lease_ack(runtime: &ShellRuntime, request: &Request) -> tau_proto::EventDelivery {
    let mut delivery = tau_proto::EventDelivery::live(
        tau_proto::UnixMicros::new(1),
        Event::ExtActivityLeaseResult(tau_proto::ExtActivityLeaseResult {
            session_id: request.context.session_id.clone(),
            publisher: runtime.interaction_target.instance.clone(),
            connection_id: "shell".parse().expect("connection"),
            operation_id: format!(
                "{}:{}",
                runtime.interaction_target.nonce, request.context.request_id
            ),
            action: tau_proto::ActivityLeaseAction::Acquire,
            outcome: tau_proto::ActivityLeaseOutcome::Acquired,
        }),
    );
    delivery.publisher = Some(tau_proto::EventPublisher::Harness);
    delivery
}

/// Deliver a successful current harness acknowledgement to the production path.
fn acknowledge_lease(runtime: &mut ShellRuntime, request: &Request) {
    let delivery = lease_ack(runtime, request);
    runtime
        .handle_activity_lease_result(&delivery)
        .expect("acquire ACK");
}

/// Recovery rows must retain their original agent scope for generic debug-log
/// suppression, even when an attaching UI queries all running commands.
#[test]
fn running_recovery_scopes_each_command_instead_of_mixing_agents() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (tx, rx) = mpsc::channel();
    let mut runtime = ShellRuntime::new_for_test_harness(
        Output::channel(tx),
        ExtConfig::default(),
        DiscoverySourcePolicy::Environment,
        temp.path().to_owned(),
    );
    runtime.bound_session_id = Some("session".parse().expect("session"));
    for agent in ["public-agent", "private-agent"] {
        let command = tau_proto::UiShellCommand {
            session_id: runtime.bound_session_id.clone().expect("session"),
            command_id: agent.parse().expect("command ID"),
            command: format!("secret-for-{agent}"),
            include_in_context: false,
            target_agent_id: Some(agent.parse().expect("agent")),
        };
        runtime
            .user_commands
            .lock()
            .expect("registry")
            .active
            .insert(
                agent.to_owned(),
                crate::user_shell_output::ActiveUserCommand {
                    pending_lease: None,
                    snapshot: RunningCommand {
                        owner: "ui".parse().expect("owner"),
                        command,
                    },
                    cancelled: Arc::new(AtomicBool::new(false)),
                },
            );
    }
    let request = Request {
        context: Context {
            session_id: runtime.bound_session_id.clone().expect("session"),
            agent_id: None,
            request_id: "recover".to_owned(),
        },
        target: Some(runtime.interaction_target.clone()),
        operation: Operation::Running,
    };
    runtime
        .handle_interaction(&delivered(&request))
        .expect("snapshot");
    let mut rows = 0;
    let mut done = false;
    for message in rx.try_iter() {
        let HarnessInputMessage::Emit(emit) = message else {
            continue;
        };
        let reply = tau_shell_interaction::decode_reply(&emit.event).expect("reply");
        let Event::ExtensionEvent(custom) = emit.event.as_ref() else {
            panic!("opaque event")
        };
        match reply.response {
            Response::Running { command } => {
                rows += 1;
                assert_eq!(custom.agent_id(), command.command.target_agent_id.as_ref());
                assert_eq!(reply.context.agent_id, command.command.target_agent_id);
                assert!(reply.context.agent_id.is_some());
                assert_eq!(reply.context.request_id, request.context.request_id);
            }
            Response::RunningComplete => {
                assert!(custom.agent_id().is_none());
                done = true;
            }
            response => panic!("unexpected {response:?}"),
        }
    }
    assert_eq!(rows, 2);
    assert!(done);
    runtime
        .user_commands
        .lock()
        .expect("registry")
        .active
        .clear();
    runtime.final_shutdown();
}

/// A stale target, historical replay, and duplicate delivery must never execute
/// the same command again. These are endpoint checks, not harness routing.
#[test]
fn user_shell_endpoint_rejects_stale_replayed_and_duplicate_execution() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (tx, rx) = mpsc::channel();
    let mut runtime = ShellRuntime::new_for_test_harness(
        Output::channel(tx),
        ExtConfig::default(),
        DiscoverySourcePolicy::Environment,
        temp.path().to_owned(),
    );
    runtime.bound_session_id = Some("session".parse().expect("session"));
    let mut request = Request {
        context: Context {
            session_id: runtime.bound_session_id.clone().expect("session"),
            agent_id: None,
            request_id: "execution-1".to_owned(),
        },
        target: Some(runtime.interaction_target.clone()),
        operation: Operation::Execute {
            command: "printf x >> executed".to_owned(),
            include_in_context: false,
        },
    };
    request.target.as_mut().expect("target").nonce = tau_shell_interaction::nonce();
    runtime
        .handle_interaction(&delivered(&request))
        .expect("stale ignored");
    assert!(
        runtime
            .user_commands
            .lock()
            .expect("registry")
            .seen
            .is_empty()
    );
    request.target = Some(runtime.interaction_target.clone());
    let mut historical = delivered(&request);
    historical.replay = true;
    runtime
        .handle_interaction(&historical)
        .expect("replay ignored");
    assert!(
        runtime
            .user_commands
            .lock()
            .expect("registry")
            .seen
            .is_empty()
    );
    runtime
        .handle_interaction(&delivered(&request))
        .expect("first execution");
    runtime
        .handle_interaction(&delivered(&request))
        .expect("duplicate ignored");
    assert!(
        !temp.path().join("executed").exists(),
        "no effect before acquire ACK"
    );
    let mut invalid = lease_ack(&runtime, &request);
    invalid.replay = true;
    runtime
        .handle_activity_lease_result(&invalid)
        .expect("ignore replay");
    invalid.replay = false;
    invalid.publisher = Some(tau_proto::EventPublisher::Ui {
        connection_id: "forger".parse().expect("UI"),
    });
    runtime
        .handle_activity_lease_result(&invalid)
        .expect("ignore forged ACK");
    assert!(
        runtime.user_commands.lock().expect("registry").active[&request.context.request_id]
            .pending_lease
            .is_some()
    );
    acknowledge_lease(&mut runtime, &request);
    acknowledge_lease(&mut runtime, &request);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut finished = false;
    let mut accepted = 0;
    while Instant::now() < deadline {
        let message = rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("response");
        let HarnessInputMessage::Emit(emit) = message else {
            continue;
        };
        if let Some(reply) = tau_shell_interaction::decode_reply(&emit.event) {
            match reply.response {
                Response::Rejected { reason } => panic!("duplicate settled original: {reason}"),
                Response::Started(_) => accepted += 1,
                Response::Finished(_) => {
                    finished = true;
                    break;
                }
                _ => {}
            }
        }
    }
    assert!(finished);
    assert_eq!(
        accepted, 1,
        "duplicate intent and ACK authorize one acceptance"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("executed")).expect("execution output"),
        "x"
    );
    assert!(
        runtime
            .user_commands
            .lock()
            .expect("registry")
            .active
            .is_empty()
    );
    runtime.final_shutdown();
}

/// Denied activity cannot produce acceptance or effects, and replaying a later
/// successful acknowledgement cannot revive the rejected execution intent.
#[test]
fn user_shell_endpoint_denied_activity_never_starts_effects() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (tx, rx) = mpsc::channel();
    let mut runtime = ShellRuntime::new_for_test_harness(
        Output::channel(tx),
        ExtConfig::default(),
        DiscoverySourcePolicy::Environment,
        temp.path().to_owned(),
    );
    runtime.bound_session_id = Some("session".parse().expect("session"));
    let request = Request {
        context: Context {
            session_id: runtime.bound_session_id.clone().expect("session"),
            agent_id: None,
            request_id: "denied".to_owned(),
        },
        target: Some(runtime.interaction_target.clone()),
        operation: Operation::Execute {
            command: "printf forbidden > effect".to_owned(),
            include_in_context: false,
        },
    };
    runtime
        .handle_interaction(&delivered(&request))
        .expect("intent");
    let mut denied = lease_ack(&runtime, &request);
    if let Event::ExtActivityLeaseResult(result) = denied.event.as_mut() {
        result.outcome = tau_proto::ActivityLeaseOutcome::Rejected {
            reason: "capacity".to_owned(),
        };
    }
    runtime
        .handle_activity_lease_result(&denied)
        .expect("denied");
    acknowledge_lease(&mut runtime, &request);
    assert!(!temp.path().join("effect").exists());
    assert!(
        runtime
            .user_commands
            .lock()
            .expect("registry")
            .active
            .is_empty()
    );
    let replies = rx
        .try_iter()
        .filter_map(|message| match message {
            HarnessInputMessage::Emit(emit) => tau_shell_interaction::decode_reply(&emit.event),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(replies.len(), 1);
    assert!(matches!(replies[0].response, Response::Rejected { .. }));
    runtime.final_shutdown();
}

/// A single-bang command publishes a UI result and a separate generic passive
/// append; double-bang never appends, even when a target agent exists.
#[test]
fn user_shell_endpoint_owns_passive_append_policy() {
    for include in [false, true] {
        let temp = tempfile::tempdir().expect("tempdir");
        let (tx, rx) = mpsc::channel();
        let mut runtime = ShellRuntime::new_for_test_harness(
            Output::channel(tx),
            ExtConfig::default(),
            DiscoverySourcePolicy::Environment,
            temp.path().to_owned(),
        );
        let session: tau_proto::SessionId = "session".parse().expect("session");
        let agent: tau_proto::AgentId = "agent".parse().expect("agent");
        runtime.bound_session_id = Some(session.clone());
        runtime.cwd_state.set_pending_ready(
            agent.clone(),
            session.clone(),
            "init".parse().expect("initialization"),
        );
        runtime.cwd_state.take_pending_ready(&agent);
        let request = Request {
            context: Context {
                session_id: session,
                agent_id: Some(agent),
                request_id: "execution-1".to_owned(),
            },
            target: Some(runtime.interaction_target.clone()),
            operation: Operation::Execute {
                command: "printf completed".to_owned(),
                include_in_context: include,
            },
        };
        runtime
            .handle_interaction(&delivered(&request))
            .expect("execute");
        acknowledge_lease(&mut runtime, &request);
        let mut saw_finish = false;
        let mut saw_append = false;
        let mut append_request = None;
        loop {
            let message = rx.recv_timeout(Duration::from_secs(5)).expect("response");
            let HarnessInputMessage::Emit(emit) = message else {
                continue;
            };
            if let Event::ExtPassiveContextAppendRequest(append) = emit.event.as_ref() {
                assert!(saw_finish, "UI result must be sent before passive append");
                assert!(append.body.contains("completed"));
                saw_append = true;
                append_request = Some(append.clone());
                break;
            }
            if let Some(reply) = tau_shell_interaction::decode_reply(&emit.event)
                && matches!(reply.response, Response::Finished(_))
            {
                saw_finish = true;
                if !include {
                    break;
                }
            }
        }
        if let Some(append) = append_request {
            assert!(!rx.try_iter().any(|message| matches!(message,
                HarnessInputMessage::Emit(emit) if matches!(*emit.event,
                    Event::ExtActivityLeaseRequest(tau_proto::ExtActivityLeaseRequest { action: tau_proto::ActivityLeaseAction::Release, .. })))));
            let mut result = tau_proto::EventDelivery::live(
                tau_proto::UnixMicros::new(2),
                Event::ExtPassiveContextAppendResult(tau_proto::ExtPassiveContextAppendResult {
                    session_id: append.session_id,
                    agent_id: append.agent_id,
                    publisher: runtime.interaction_target.instance.clone(),
                    operation_id: append.operation_id,
                    outcome: tau_proto::PassiveContextAppendOutcome::Failed {
                        reason: "injected append failure".to_owned(),
                    },
                }),
            );
            result.publisher = Some(tau_proto::EventPublisher::Harness);
            runtime
                .handle_passive_context_result(&result)
                .expect("settle failure");
        }
        let release = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("release after settlement");
        assert!(
            matches!(release, HarnessInputMessage::Emit(emit) if matches!(*emit.event,
            Event::ExtActivityLeaseRequest(tau_proto::ExtActivityLeaseRequest { action: tau_proto::ActivityLeaseAction::Release, .. })))
        );
        runtime.final_shutdown();
        if !include {
            assert!(!rx.try_iter().any(|message| matches!(message,
                HarnessInputMessage::Emit(emit) if matches!(*emit.event, Event::ExtPassiveContextAppendRequest(_)))));
        }
        assert_eq!(saw_append, include);
        assert!(saw_finish);
    }
}

/// Cancellation belongs to the initiating UI, survives the queue/start race,
/// and produces one terminal without permitting the delayed effect.
#[test]
fn user_shell_endpoint_cancel_is_owner_bound_and_sticky() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (tx, rx) = mpsc::channel();
    let mut runtime = ShellRuntime::new_for_test_harness(
        Output::channel(tx),
        ExtConfig::default(),
        DiscoverySourcePolicy::Environment,
        temp.path().to_owned(),
    );
    runtime.bound_session_id = Some("session".parse().expect("session"));
    let execute = Request {
        context: Context {
            session_id: runtime.bound_session_id.clone().expect("session"),
            agent_id: None,
            request_id: "cancelled-command".to_owned(),
        },
        target: Some(runtime.interaction_target.clone()),
        operation: Operation::Execute {
            command: "sleep 20; printf forbidden > effect".to_owned(),
            include_in_context: false,
        },
    };
    runtime
        .handle_interaction(&delivered(&execute))
        .expect("execute");
    let cancelled = runtime
        .user_commands
        .lock()
        .expect("registry")
        .active
        .get(&execute.context.request_id)
        .expect("active")
        .cancelled
        .clone();
    let cancel = Request {
        context: Context {
            request_id: "cancel-request".to_owned(),
            ..execute.context.clone()
        },
        target: execute.target.clone(),
        operation: Operation::Cancel {
            command_id: execute.context.request_id.clone(),
        },
    };
    let mut wrong_owner = delivered(&cancel);
    wrong_owner.publisher = Some(tau_proto::EventPublisher::Ui {
        connection_id: "another-ui".parse().expect("other UI"),
    });
    runtime
        .handle_interaction(&wrong_owner)
        .expect("ignore wrong owner");
    assert!(!cancelled.load(Ordering::Acquire));
    runtime
        .handle_interaction(&delivered(&cancel))
        .expect("owner cancellation");
    assert!(
        cancelled.load(Ordering::Acquire),
        "flag survives pre-worker cancellation"
    );
    acknowledge_lease(&mut runtime, &execute);
    loop {
        let HarnessInputMessage::Emit(emit) =
            rx.recv_timeout(Duration::from_secs(5)).expect("terminal")
        else {
            continue;
        };
        if let Some(reply) = tau_shell_interaction::decode_reply(&emit.event)
            && let Response::Finished(finished) = reply.response
        {
            assert!(finished.cancelled);
            break;
        }
    }
    runtime.final_shutdown();
    assert!(!temp.path().join("effect").exists());
    assert!(
        runtime
            .user_commands
            .lock()
            .expect("registry")
            .active
            .is_empty()
    );
    assert!(
        !rx.try_iter().any(|message| match message {
            HarnessInputMessage::Emit(emit) => tau_shell_interaction::decode_reply(&emit.event)
                .is_some_and(|reply| matches!(reply.response, Response::Finished(_))),
            _ => false,
        }),
        "one terminal only"
    );
}
