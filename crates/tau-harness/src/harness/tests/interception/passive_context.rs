//! Publication-failure and provenance coverage for generic passive output.

use super::*;

/// A canonical append that is dropped or rejected by storage must produce an
/// operation-correlated failure, with no context and no inference activation.
#[test]
fn passive_context_publication_failures_are_not_execution_retries() {
    for intercept in [false, true] {
        let tmp = TempDir::new().expect("tempdir");
        let mut h = quiet_provider_harness(tmp.path()).expect("harness");
        let cid = ensure_test_user_agent(&mut h);
        let agent_id = durable_agent_id_for_conversation(&h, &cid);
        connect_ready_configured_extension(&mut h, "writer", "writer", tau_proto::ClientKind::Tool);
        if intercept {
            connect_test_tool(&mut h, "passive-interceptor");
            h.handle_extension_event(
                "passive-interceptor",
                TestProtocolItem::Message(TestMessage::Intercept(Intercept {
                    selectors: vec![EventSelector::Exact(
                        tau_proto::EventName::AGENT_PASSIVE_CONTEXT_APPENDED,
                    )],
                    priority: InterceptionPriority::new(0),
                })),
            )
            .expect("register");
        } else {
            reject_semantic_admissions(&h, 1);
        }
        h.handle_extension_event_inner(
            &crate::test_connection_id("writer"),
            Event::ExtPassiveContextAppendRequest(tau_proto::ExtPassiveContextAppendRequest {
                session_id: h.session_runtime.current_session_id.clone(),
                agent_id,
                operation_id: "failure-1".to_owned(),
                body: "already completed external operation".to_owned(),
            }),
        )
        .expect("request");
        if intercept {
            h.handle_extension_event(
                "passive-interceptor",
                TestProtocolItem::Message(TestMessage::InterceptReply(InterceptReply {
                    action: InterceptAction::Drop,
                })),
            )
            .expect("drop");
        }
        let events = event_log_events(&h);
        assert!(!events.iter().any(|event| matches!(
            event,
            Event::AgentPassiveContextAppended(_) | Event::AgentPromptStarted(_)
        )));
        assert!(events.iter().any(|event| matches!(
            event, Event::ExtPassiveContextAppendResult(result)
                if result.operation_id == "failure-1"
                    && matches!(result.outcome, tau_proto::PassiveContextAppendOutcome::Failed { .. })
        )));
        h.shutdown().expect("shutdown");
    }
}

/// Interceptors cannot forge canonical publisher, target, or output, and
/// configured peers cannot bypass the generic request's attribution.
#[test]
fn passive_context_canonical_output_is_immutable_and_harness_owned() {
    let tmp = TempDir::new().expect("tempdir");
    let mut h = quiet_provider_harness(tmp.path()).expect("harness");
    let cid = ensure_test_user_agent(&mut h);
    let agent_id = durable_agent_id_for_conversation(&h, &cid);
    connect_ready_configured_extension(&mut h, "writer", "writer", tau_proto::ClientKind::Core);
    connect_test_tool(&mut h, "passive-interceptor");
    h.handle_extension_event(
        "passive-interceptor",
        TestProtocolItem::Message(TestMessage::Intercept(Intercept {
            selectors: vec![EventSelector::Exact(
                tau_proto::EventName::AGENT_PASSIVE_CONTEXT_APPENDED,
            )],
            priority: InterceptionPriority::new(0),
        })),
    )
    .expect("register");
    h.handle_extension_event_inner(
        &crate::test_connection_id("writer"),
        Event::ExtPassiveContextAppendRequest(tau_proto::ExtPassiveContextAppendRequest {
            session_id: h.session_runtime.current_session_id.clone(),
            agent_id: agent_id.clone(),
            operation_id: "immutable-1".to_owned(),
            body: "original output".to_owned(),
        }),
    )
    .expect("request");
    let forged = Event::AgentPassiveContextAppended(tau_proto::AgentPassiveContextAppended {
        session_id: h.session_runtime.current_session_id.clone(),
        agent_id,
        publisher: "pretend".parse().expect("publisher"),
        operation_id: "forged".to_owned(),
        text: "forged output".to_owned(),
    });
    h.handle_extension_event_inner(&crate::test_connection_id("writer"), forged.clone())
        .expect("forbidden peer fact");
    h.handle_extension_event(
        "passive-interceptor",
        TestProtocolItem::Message(TestMessage::InterceptReply(InterceptReply {
            action: InterceptAction::Pass(Some(Box::new(forged))),
        })),
    )
    .expect("replace");
    let events = event_log_events(&h);
    assert!(events.iter().any(|event| matches!(
        event, Event::AgentPassiveContextAppended(appended)
            if appended.publisher.as_str() == "writer" && appended.text.contains("original output")
    )));
    assert!(!events.iter().any(|event| matches!(
        event, Event::AgentPassiveContextAppended(appended) if appended.text == "forged output"
    )));
    h.shutdown().expect("shutdown");
}
