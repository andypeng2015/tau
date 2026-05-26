use super::*;

fn provider() -> ChatCompletionsProvider {
    ChatCompletionsProvider {
        base_url: "https://api.openai.com/v1".to_owned(),
        api_key: "key".to_owned(),
        models: vec![ChatCompletionsModel {
            id: ModelName::new("gpt-4o"),
            display_name: None,
            context_window: 128_000,
        }],
        extra_body: BTreeMap::new(),
        compat: ChatCompletionsCompat::openai_defaults(),
    }
}

#[test]
fn publishes_configured_models_for_registered_provider() {
    // Built-in provider profiles derive the Tau provider namespace from the
    // profile filename; the Chat Completions backend only turns one registered
    // profile into model publication records.
    let models = models_for_provider(&ProviderName::new("openai"), &provider());

    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id.to_string(), "openai/gpt-4o");
    assert!(!models[0].supports_compaction);
}

#[test]
fn provider_with_reasoning_effort_publishes_effort_levels() {
    // Role effort selection is clamped to provider-advertised levels. OpenAI
    // compatible profiles that opt into reasoning_effort must publish the
    // corresponding choices.
    let models = models_for_provider(&ProviderName::new("openai"), &provider());

    assert!(models[0].efforts.contains(&tau_proto::Effort::High));
    assert!(models[0].efforts.contains(&tau_proto::Effort::Off));
}

#[test]
fn tool_result_text_uses_structured_status_headers() {
    // Chat Completions and Responses API providers should expose identical
    // provider-facing text for non-success tool results, so model behavior
    // does not depend on the selected OpenAI-compatible API surface.
    let output = tau_proto::ToolResponse::from_cbor(&tau_proto::CborValue::Text("body".into()));

    assert_eq!(
        tool_result_text(
            ToolResultStatus::Error {
                message: "failed".to_owned(),
            },
            &output,
        ),
        "error: failed\n\nbody",
    );
    assert_eq!(
        tool_result_text(
            ToolResultStatus::Cancelled {
                reason: "stopped".to_owned(),
            },
            &output,
        ),
        "cancelled: stopped\n\n",
    );
}

#[test]
fn provider_config_rejects_unknown_fields() {
    // Chat Completions profiles are user-authored provider config. Unknown
    // fields should fail fast instead of silently disabling an intended setting.
    let error = serde_json::from_value::<ChatCompletionsProvider>(serde_json::json!({
        "base_url": "https://api.openai.com/v1",
        "models": [{ "id": "gpt-4o", "extra": true }],
    }))
    .expect_err("model entry should reject unknown fields");

    assert!(error.to_string().contains("unknown field"), "got: {error}");
}

#[test]
fn empty_end_turn_is_rejected_before_harness_completion() {
    // Regression: some local Chat Completions servers occasionally answer a
    // tool-result follow-up with `finish_reason: stop`, usage, and no content
    // or tool calls. Treating that as a normal turn silently marks the agent as
    // done with an empty message, so the backend must surface it as retryable.
    let state = StreamState::new();

    assert!(matches!(
        ensure_non_empty_end_turn(state),
        Err(LlmError::EmptyResponse)
    ));
}

#[test]
fn non_empty_end_turn_is_accepted() {
    // A normal assistant text response should not be affected by the empty-turn
    // guard.
    let mut state = StreamState::new();
    state.text = "done".to_owned();

    assert!(ensure_non_empty_end_turn(state).is_ok());
}

#[test]
fn tool_call_turn_is_accepted_without_text() {
    // Tool-call turns often have no assistant text; they are valid as long as a
    // parsed tool call is present.
    let mut state = StreamState::new();
    state.stop_reason = ProviderStopReason::ToolCalls;
    state.tool_calls.insert(
        0,
        ToolCallAccumulator {
            id: "call-1".to_owned(),
            name: "shell".to_owned(),
            arguments: "{}".to_owned(),
        },
    );

    assert!(ensure_non_empty_end_turn(state).is_ok());
}
