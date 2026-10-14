use super::*;

/// Manual-compaction acceptance carries the request and source identity needed
/// to correlate a later transaction start, so UI payload projection must not
/// redact it like provider-only content.
#[test]
fn replay_projection_preserves_manual_compaction_acceptance() {
    let event = Event::AgentManualCompactionRequested(tau_proto::AgentManualCompactionRequested {
        request_id: tau_proto::CompactionRequestId::parse("cr-1-0").expect("request id"),
        target_agent_id: crate::parse_agent_id("worker"),
        source: tau_proto::ManualCompactionSource::Tool(tau_proto::ManualToolCompactionSource {
            caller_agent_id: crate::parse_agent_id("manager"),
            initiating_agent_prompt_id: "ap-manager-1"
                .parse::<tau_proto::AgentPromptId>()
                .expect("known-safe AgentPromptId must be valid"),
            initiating_tool_call_id: "call-1".into(),
            initiating_tool_name: tau_proto::ManualCompactionTool::AgentCompact,
            visible_tool_name: tau_proto::ToolName::new("agent_compact"),
            resume_inference: false,
        }),
        requested_target_head: tau_proto::AgentHead::Root,
        target_generation: tau_proto::MaterializedPromptGeneration::from_inference_generation(0),
        model: "test/model".parse().expect("model id"),
    });

    assert_eq!(project_agent_replay_event(event.clone(), true), event);
}

/// A completed context shell command is a self-contained transcript fact:
/// command, output, and exit status must survive UI payload projection even
/// though its transient running observations have no durable history.
#[test]
fn replay_projection_preserves_completed_context_shell_fact() {
    let finished = Event::ShellCommandFinished(tau_proto::ShellCommandFinished {
        command_id: tau_proto::ShellCommandId::parse("shell-replay").expect("command id"),
        session_id: tau_proto::SessionId::parse("session-replay").expect("session id"),
        command: "sh -c 'printf output; exit 7'".to_owned(),
        include_in_context: true,
        target_agent_id: Some(crate::parse_agent_id("worker")),
        output: "output".to_owned(),
        exit_code: Some(7),
        cancelled: false,
    });

    assert_eq!(project_agent_replay_event(finished.clone(), true), finished);
}

/// Payload projection does not redact exact-owner termination for either peer
/// kind; historical selection is the subscriber's responsibility.
#[test]
fn replay_projection_preserves_durable_prompt_termination() {
    let event = Event::AgentPromptTerminated(tau_proto::AgentPromptTerminated {
        automatic_compaction_decision: None,
        agent_id: crate::parse_agent_id("worker"),
        agent_prompt_id: tau_proto::AgentPromptId::parse("ap-terminated").expect("prompt id"),
        reason: tau_proto::AgentPromptTerminationReason::Stale,
        originator: tau_proto::PromptOriginator::User,
    });
    for is_ui in [false, true] {
        assert_eq!(project_agent_replay_event(event.clone(), is_ui), event);
    }
}
