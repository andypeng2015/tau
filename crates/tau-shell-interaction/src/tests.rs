use tau_proto::{EventDelivery, EventPublisher, ExtensionInstanceId};

use super::*;

/// Construct one live response with authenticated configured attribution.
fn delivery(reply: &Reply) -> EventDelivery {
    let HarnessInputMessage::Emit(emit) = reply_message(reply) else {
        unreachable!()
    };
    let mut delivery = EventDelivery::live(tau_proto::UnixMicros::new(1), *emit.event);
    delivery.publisher = Some(EventPublisher::Extension {
        name: reply.target.instance.clone(),
        instance_id: ExtensionInstanceId::new(1),
        connection_id: "shell-connection".parse().expect("connection"),
    });
    delivery
}

/// Discover one fixture process through the same round-trip as an attached UI.
fn discovered_client() -> (Client, Target) {
    let mut client = Client::new("session".parse().expect("session"));
    let request = client.discover();
    let target = Target {
        instance: "remote".parse().expect("instance"),
        nonce: nonce(),
    };
    let reply = Reply {
        context: request.context,
        target: target.clone(),
        response: Response::Available {
            execute: true,
            complete: true,
        },
    };
    let update = client.observe(&delivery(&reply));
    assert!(matches!(update.requests[0].operation, Operation::Running));
    assert_eq!(client.endpoints().count(), 1);
    (client, target)
}

/// Recovery can span several agent-scoped rows but must reject a row whose
/// payload and generic privacy scope disagree, and stop at the query
/// terminator.
#[test]
fn running_recovery_accepts_only_correlated_agent_scoped_rows() {
    let mut client = Client::new("session".parse().expect("session"));
    let discovery = client.discover();
    let target = Target {
        instance: "remote".parse().expect("instance"),
        nonce: nonce(),
    };
    let update = client.observe(&delivery(&Reply {
        context: discovery.context,
        target: target.clone(),
        response: Response::Available {
            execute: true,
            complete: true,
        },
    }));
    let query = &update.requests[0];
    let mut last = None;
    for agent in ["private-agent", "public-agent"] {
        let command = RunningCommand {
            owner: "other-ui".parse().expect("owner"),
            command: tau_proto::UiShellCommand {
                session_id: query.context.session_id.clone(),
                target_agent_id: Some(agent.parse().expect("agent")),
                command_id: agent.parse().expect("command ID"),
                command: "sensitive".to_owned(),
                include_in_context: false,
            },
        };
        let mut reply = Reply {
            context: query.context.clone(),
            target: target.clone(),
            response: Response::Running { command },
        };
        assert!(
            client.observe(&delivery(&reply)).events.is_empty(),
            "wrong scope"
        );
        reply.context.agent_id = Some(agent.parse().expect("agent"));
        assert_eq!(client.observe(&delivery(&reply)).events.len(), 1);
        last = Some(reply);
    }
    client.observe(&delivery(&Reply {
        context: query.context.clone(),
        target,
        response: Response::RunningComplete,
    }));
    assert!(
        client
            .observe(&delivery(&last.expect("last row")))
            .events
            .is_empty()
    );
}

/// Missing and legacy unqualified selectors must never pick even a sole
/// available executor. Shell quoting remains untouched after the selector.
#[test]
fn selectors_are_mandatory_and_command_bytes_are_preserved() {
    for invalid in ["!pwd", "!!pwd", "! pwd", "!! pwd", "!remote", "!remote "] {
        assert!(parse_command(invalid).is_err(), "{invalid}");
    }
    let (name, command, include) = parse_command("!!remote printf '%s' 'hé llo'").expect("command");
    assert_eq!(name.as_str(), "remote");
    assert_eq!(command, "printf '%s' 'hé llo'");
    assert!(!include);
    let (mut client, _) = discovered_client();
    assert!(client.execute("!pwd", None).is_err());
    assert!(client.execute("!!unknown pwd", None).is_err());
}

/// Payload-local labels and a replay flag cannot authenticate discovery.
#[test]
fn discovery_requires_original_publisher_and_live_correlation() {
    let mut client = Client::new("session".parse().expect("session"));
    let request = client.discover();
    let reply = Reply {
        context: request.context,
        target: Target {
            instance: "remote".parse().expect("instance"),
            nonce: nonce(),
        },
        response: Response::Available {
            execute: true,
            complete: true,
        },
    };
    let mut frame = delivery(&reply);
    frame.publisher = Some(EventPublisher::Ui {
        connection_id: "ui".parse().expect("UI"),
    });
    client.observe(&frame);
    assert_eq!(client.endpoints().count(), 0);
    frame = delivery(&reply);
    frame.replay = true;
    client.observe(&frame);
    assert_eq!(client.endpoints().count(), 0);
    frame.replay = false;
    client.observe(&frame);
    assert_eq!(client.endpoints().count(), 1);
}

/// Only the selected process and immutable request can settle a pending
/// command; duplicate terminals and stale incarnation replies must not render
/// twice.
#[test]
fn command_results_validate_target_fields_and_deduplicate() {
    let (mut client, target) = discovered_client();
    let request = client.execute("!!remote printf ok", None).expect("request");
    let finished = ShellCommandFinished {
        session_id: request.context.session_id.clone(),
        command_id: request.context.request_id.parse().expect("command"),
        command: "printf ok".to_owned(),
        include_in_context: false,
        target_agent_id: None,
        output: "ok".to_owned(),
        exit_code: Some(0),
        cancelled: false,
    };
    let mut reply = Reply {
        context: request.context,
        target,
        response: Response::Finished(finished.clone()),
    };
    reply.target.nonce = nonce();
    assert!(client.observe(&delivery(&reply)).events.is_empty());
    reply.target = request.target.expect("target");
    let mut changed = finished.clone();
    changed.command = "different command".to_owned();
    reply.response = Response::Finished(changed);
    assert!(client.observe(&delivery(&reply)).events.is_empty());
    reply.response = Response::Finished(finished);
    let events = client.observe(&delivery(&reply)).events;
    assert_eq!(
        events.len(),
        2,
        "a dropped started frame is reconstructed from the pending request"
    );
    assert!(
        matches!(&events[1], Event::ShellCommandFinished(finished) if finished.command == "remote printf ok")
    );
    assert!(client.observe(&delivery(&reply)).events.is_empty());
}

/// Generic privacy metadata must exactly match the payload's scope; a changed
/// agent label cannot silently move private output into another transcript.
#[test]
fn protocol_scope_mismatch_is_rejected() {
    let request = Request {
        context: Context {
            session_id: "session".parse().expect("session"),
            agent_id: Some("agent".parse().expect("agent")),
            request_id: nonce(),
        },
        target: None,
        operation: Operation::Discover,
    };
    let HarnessInputMessage::Emit(emit) = request_message(&request) else {
        unreachable!()
    };
    assert_eq!(decode_request(&emit.event), Some(request));
    let Event::ExtensionEvent(event) = *emit.event else {
        unreachable!()
    };
    let changed = Event::ExtensionEvent(event.with_agent_id("other".parse().expect("agent")));
    assert!(decode_request(&changed).is_none());
}

/// Only an actual displayed result suppresses a duplicate live passive fact.
/// History, a dropped terminal, and a disconnect diagnostic must retain output.
#[test]
fn passive_output_is_retained_until_real_terminal_and_always_in_history() {
    for terminal in [false, true] {
        let (mut client, target) = discovered_client();
        let agent = "agent".parse().expect("agent");
        let request = client
            .execute("!remote printf ok", Some(agent))
            .expect("execute");
        let appended = Event::AgentPassiveContextAppended(tau_proto::AgentPassiveContextAppended {
            session_id: request.context.session_id.clone(),
            agent_id: request.context.agent_id.clone().expect("agent"),
            publisher: target.instance.clone(),
            operation_id: format!("{}:{}", target.nonce, request.context.request_id),
            text: "attributed output".to_owned(),
        });
        if terminal {
            let reply = Reply {
                context: request.context.clone(),
                target: target.clone(),
                response: Response::Finished(ShellCommandFinished {
                    session_id: request.context.session_id.clone(),
                    target_agent_id: request.context.agent_id.clone(),
                    command_id: request.context.request_id.parse().expect("command"),
                    command: "printf ok".to_owned(),
                    include_in_context: true,
                    output: "ok".to_owned(),
                    exit_code: Some(0),
                    cancelled: false,
                }),
            };
            assert_eq!(client.observe(&delivery(&reply)).events.len(), 2);
        } else {
            // A locally synthesized unknown terminal is not an output
            // observation.
            client.observe_parts(
                &Event::ExtensionExited(tau_proto::ExtensionExited {
                    instance_id: ExtensionInstanceId::new(1),
                    extension_name: target.instance.clone(),
                    pid: None,
                    exit_code: None,
                    signal: None,
                }),
                Some(&EventPublisher::Harness),
                false,
            );
        }
        assert_eq!(
            client
                .observe_parts(&appended, Some(&EventPublisher::Harness), false)
                .suppress_event,
            terminal
        );
        assert!(
            !client
                .observe_parts(&appended, Some(&EventPublisher::Harness), true)
                .suppress_event
        );
        assert!(!client.observe_parts(&appended, None, false).suppress_event);
        let result =
            Event::ExtPassiveContextAppendResult(tau_proto::ExtPassiveContextAppendResult {
                session_id: request.context.session_id,
                agent_id: request.context.agent_id.expect("agent"),
                publisher: target.instance,
                operation_id: format!("{}:{}", target.nonce, request.context.request_id),
                outcome: tau_proto::PassiveContextAppendOutcome::Failed {
                    reason: "storage unavailable".to_owned(),
                },
            });
        let update = client.observe_parts(&result, Some(&EventPublisher::Harness), false);
        assert_eq!(update.diagnostics.len(), 1);
        assert!(update.diagnostics[0].contains("context insertion failed"));
        assert!(
            update.requests.is_empty(),
            "append failure never retries execution"
        );
    }
}
