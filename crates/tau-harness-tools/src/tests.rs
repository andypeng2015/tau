use super::*;

fn cbor_map_text<'a>(value: &'a CborValue, key: &str) -> Option<&'a str> {
    let CborValue::Map(entries) = value else {
        return None;
    };
    entries.iter().find_map(|(entry_key, entry_value)| {
        matches!(entry_key, CborValue::Text(text) if text == key)
            .then_some(entry_value)
            .and_then(|value| match value {
                CborValue::Text(text) => Some(text.as_str()),
                _ => None,
            })
    })
}

fn cbor_map_bool(value: &CborValue, key: &str) -> Option<bool> {
    let CborValue::Map(entries) = value else {
        return None;
    };
    entries.iter().find_map(|(entry_key, entry_value)| {
        matches!(entry_key, CborValue::Text(text) if text == key)
            .then_some(entry_value)
            .and_then(|value| match value {
                CborValue::Bool(value) => Some(*value),
                _ => None,
            })
    })
}
fn wait_args_exact(call_id: &str) -> CborValue {
    CborValue::Map(vec![(
        CborValue::Text("tool_call_id".to_owned()),
        CborValue::Text(call_id.to_owned()),
    )])
}

fn wait_call(target_call_id: &str) -> AgentToolCall {
    AgentToolCall {
        id: "wait-call".into(),
        name: ToolName::new(WAIT_TOOL_NAME),
        tool_type: ToolType::Function,
        arguments: wait_args_exact(target_call_id),
    }
}

fn tool_result(call_id: &str, kind: ToolResultKind) -> ToolResult {
    ToolResult {
        call_id: call_id.into(),
        tool_name: ToolName::new("shell"),
        tool_type: ToolType::Function,
        result: CborValue::Text("done".to_owned()),
        kind,
        display: None,
        originator: PromptOriginator::User,
    }
}

fn tool_background_result(call_id: &str) -> tau_proto::ToolBackgroundResult {
    tau_proto::ToolBackgroundResult {
        call_id: call_id.into(),
        tool_name: ToolName::new("shell"),
        tool_type: ToolType::Function,
        result: CborValue::Text("done".to_owned()),
        display: None,
        originator: PromptOriginator::User,
    }
}

#[test]
fn agent_start_spec_advertises_only_current_tool_name() {
    // Tau does not preserve compatibility for renamed tool call names yet.
    // Only the current public spelling should be advertised or handled.
    let tools = BuiltinTools::default();
    let names: Vec<String> = tools
        .tool_specs()
        .into_iter()
        .map(|spec| spec.name.to_string())
        .collect();

    assert!(names.iter().any(|name| name == AGENT_START_TOOL_NAME));
    assert!(!names.iter().any(|name| name == "delegate"));
    assert!(tools.handles(&ToolName::new(AGENT_START_TOOL_NAME)));
    assert!(!tools.handles(&ToolName::new("delegate")));
}

#[test]
fn wait_initial_display_uses_tracked_target_tool_name() {
    // Regression for provider-owned running display: the wait tool should
    // show the logical source tool name, not the opaque target call id.
    let mut state = BuiltinState::default();
    state.record_tool_started("shell-call".into(), ToolName::new("shell"));

    let display = state
        .initial_display(&wait_call("shell-call"))
        .expect("wait display");

    assert_eq!(display.args, "shell");
    assert_eq!(display.status, ToolUseStatus::InProgress);
}

#[test]
fn wait_initial_display_tracks_only_running_or_backgrounded_tools() {
    let mut state = BuiltinState::default();
    state.record_tool_started("shell-call".into(), ToolName::new("shell"));

    state.record_tool_lifecycle_event(&Event::ProviderToolResult(tool_result(
        "shell-call",
        ToolResultKind::BackgroundPlaceholder,
    )));
    let display = state
        .initial_display(&wait_call("shell-call"))
        .expect("wait display after placeholder");
    assert_eq!(display.args, "shell");

    state.record_tool_lifecycle_event(&Event::ToolBackgroundResult(tool_background_result(
        "shell-call",
    )));
    let display = state
        .initial_display(&wait_call("shell-call"))
        .expect("wait display after finish");
    assert_eq!(display.args, "");
}

#[test]
fn delegate_instruction_names_parent_and_message_followup_path() {
    // Delegated agents get a fresh context, so their injected instruction
    // must explicitly name the parent and explain that only the first final
    // response flows back through the agent_start tool result.
    let instruction = delegate_instruction("engineer_parent", "inspect the change");

    assert!(
        instruction
            .contains("You were started by agent `engineer_parent` using `agent_start` tool")
    );
    assert!(instruction.contains("Only your first final response"));
    assert!(
        instruction
            .contains("you can use `message` tool to communicate with any agent at any time")
    );
    assert!(instruction.contains("### Task\n\ninspect the change"));
}

#[test]
fn delegate_result_includes_only_caller_and_sub_agent_ids() {
    let value = delegate_result_value(
        "done".to_owned(),
        None,
        Some("engineer_parent"),
        Some("engineer_child"),
    );

    assert_eq!(
        cbor_map_text(&value, "self_agent_id"),
        Some("engineer_parent")
    );
    assert_eq!(
        cbor_map_text(&value, "sub_agent_id"),
        Some("engineer_child")
    );
    assert_eq!(cbor_map_text(&value, "agent_id"), None);
    assert_eq!(cbor_map_text(&value, "output"), Some("done"));
}

#[test]
fn skill_search_guidance_omits_content_hint_when_content_was_already_searched() {
    let (result, _) = skill_search_result(
        &["missing".to_owned()],
        true,
        SkillSearchOutcome {
            hits: Vec::new(),
            total_matches: 0,
            truncated: false,
            auto_load_name: None,
            warnings: Vec::new(),
        },
    );

    assert_eq!(cbor_map_bool(&result, "search_content"), Some(true));
    let guidance = cbor_map_text(&result, "guidance").expect("guidance");
    assert!(guidance.contains("No skills matched"));
    assert!(!guidance.contains("search_content: true"));
}

#[test]
fn skill_search_guidance_suggests_content_search_only_when_not_already_enabled() {
    let (result, _) = skill_search_result(
        &["missing".to_owned()],
        false,
        SkillSearchOutcome {
            hits: Vec::new(),
            total_matches: 0,
            truncated: false,
            auto_load_name: None,
            warnings: Vec::new(),
        },
    );

    let guidance = cbor_map_text(&result, "guidance").expect("guidance");
    assert!(guidance.contains("search_content: true"));
}

#[test]
fn skill_query_rejects_whitespace_without_echoing_raw_input() {
    let args = CborValue::Map(vec![(
        CborValue::Text("query".to_owned()),
        CborValue::Text("  \n\t  ".to_owned()),
    )]);

    let err = extract_skill_search_queries(&args).expect_err("whitespace query should fail");

    assert_eq!(err, "query must include at least one non-empty term");
    assert!(!err.contains('\n'));
    assert!(!err.contains('\t'));
}
