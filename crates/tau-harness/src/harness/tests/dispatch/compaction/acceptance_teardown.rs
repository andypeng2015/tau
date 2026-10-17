//! Staged manual-compaction acceptance teardown.

use super::*;

/// Agent unload must retire staged manual-compaction acceptance without leaking
/// deferred work.
#[test]
fn staged_model_compaction_acceptance_is_cleared_by_teardown() {
    for teardown in ["target_unload", "caller_unload"] {
        let (_td, mut h, caller_cid, target_cid, call, target_id) =
            setup_manual_cross_compaction_test();
        let interceptor = match teardown {
            "target_unload" => "model-acceptance-target-unload",
            "caller_unload" => "model-acceptance-caller-unload",
            _ => unreachable!("fixed teardown cases"),
        };
        connect_test_tool(&mut h, interceptor);
        h.handle_extension_event(
            interceptor,
            TestProtocolItem::Message(TestMessage::Intercept(Intercept {
                selectors: vec![EventSelector::Exact(
                    tau_proto::EventName::AGENT_MANUAL_COMPACTION_REQUESTED,
                )],
                priority: InterceptionPriority::new(0),
            })),
        )
        .expect("register acceptance interceptor");
        h.request_agent_tool_compaction(
            &caller_cid,
            &call,
            ToolName::new("agent_compact"),
            Some(&target_id),
        );
        assert_eq!(
            h.prompt_coordination
                .compaction_runtime
                .pending_manual_acceptances
                .len(),
            1
        );
        let request_key = h
            .prompt_coordination
            .compaction_runtime
            .pending_manual_acceptances
            .keys()
            .next()
            .cloned()
            .expect("staged request id");
        let request_id = request_key.request_id().clone();
        if teardown == "caller_unload" {
            reject_next_semantic_admission(&h);
            h.handle_extension_event(
                interceptor,
                TestProtocolItem::Message(TestMessage::InterceptReply(InterceptReply {
                    action: InterceptAction::Pass(None),
                })),
            )
            .expect("retain append-rejected acceptance");
            assert!(
                h.prompt_coordination
                    .prompt_runtime
                    .pending_publish_completions
                    .contains_key(&target_cid)
            );
        }
        match teardown {
            "target_unload" => h.remove_agent_expected(&target_cid),
            "caller_unload" => h.remove_agent_expected(&caller_cid),
            _ => unreachable!("fixed teardown cases"),
        }
        assert!(
            h.prompt_coordination
                .compaction_runtime
                .pending_manual_acceptances
                .is_empty()
                && !h
                    .tool_routing
                    .tool_runtime
                    .tool_turn
                    .is_backgrounded(&call.id),
            "teardown={teardown}"
        );
        assert!(
            h.runtime_io.publication.pending_intercept.is_none()
                && h.runtime_io.publication.deferred.iter().all(|pending| {
                    !matches!(
                        pending.event(),
                        Event::AgentManualCompactionRequested(request)
                            if request.request_id == request_id
                    )
                }),
            "teardown={teardown}"
        );
        assert!(
            h.prompt_coordination
                .compaction_runtime
                .accepted_manual_tools
                .is_empty()
                && h.prompt_coordination
                    .compaction_runtime
                    .active_manual_starts_is_empty()
                && h.prompt_coordination
                    .prompt_runtime
                    .pending_publish_completions
                    .is_empty(),
            "teardown={teardown}"
        );
    }
}
