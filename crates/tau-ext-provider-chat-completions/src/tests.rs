use super::*;

fn auth() -> ChatCompletionsAuth {
    ChatCompletionsAuth {
        providers: BTreeMap::from([(
            ProviderName::new("openai"),
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
            },
        )]),
    }
}

fn user_text(text: &str) -> ContextItem {
    ContextItem::Message(tau_proto::MessageItem {
        role: ContextRole::User,
        content: vec![ContentPart::Text {
            text: text.to_owned(),
        }],
        phase: None,
    })
}

fn restored_tool_call(call_id: &str) -> ContextItem {
    ContextItem::ToolCall(ToolCallItem {
        call_id: call_id.into(),
        name: tau_proto::ToolName::new("shell"),
        tool_type: ToolType::Function,
        arguments: tau_proto::CborValue::Map(vec![(
            tau_proto::CborValue::Text("command".to_owned()),
            tau_proto::CborValue::Text("sleep 30".to_owned()),
        )]),
    })
}

fn restored_internal_tool_error(call_id: &str, body: &str) -> ContextItem {
    ContextItem::ToolResult(tau_proto::ToolResultItem {
        call_id: call_id.into(),
        tool_type: ToolType::Function,
        status: ToolResultStatus::Error {
            message: format!(
                "{}: true\n\nTool call `{call_id}` was interrupted due to session restart. Side effects may have occurred.",
                tau_proto::TAU_INTERNAL_HEADER_NAME
            ),
        },
        output: tau_proto::ToolResponse::from_cbor(&tau_proto::CborValue::Text(body.to_owned())),
    })
}

#[test]
fn parse_model_list_rejects_empty_lists() {
    // Provider-specific CLI setup should not write a provider entry that
    // publishes no models.
    assert!(parse_model_list(" , ").is_err());
}

#[test]
fn parse_model_list_accepts_comma_separated_models() {
    // The interactive setup stores the exact upstream model ids that will be
    // published under the chosen provider namespace.
    let models = parse_model_list("gpt-4o, gpt-4o-mini").expect("models");

    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id.as_str(), "gpt-4o");
    assert_eq!(models[1].id.as_str(), "gpt-4o-mini");
}

#[test]
fn publishes_configured_models() {
    // Chat Completions has no built-in model registry; the auth file's
    // configured model list is the complete publication source.
    let models = models_for_auth(&auth());

    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id.to_string(), "openai/gpt-4o");
    assert!(!models[0].supports_compaction);
}

#[test]
fn publishes_keyless_configured_models() {
    // Local Chat Completions-compatible servers often do not require API
    // keys. Model publication should depend on configured models, not on a
    // secret being present in the auth file.
    let mut auth = auth();
    auth.providers
        .get_mut(&ProviderName::new("openai"))
        .expect("provider")
        .api_key
        .clear();

    let models = models_for_auth(&auth);

    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id.to_string(), "openai/gpt-4o");
}

#[test]
fn handles_logged_prompt_created_and_acks_it() {
    // The harness delivers subscribed events through LogEvent envelopes.
    // Providers must peel the envelope, process the prompt, and ack the log
    // id so durable event replay can advance.
    let prompt = tau_proto::SessionPromptCreated {
        session_prompt_id: "sp-logged".into(),
        session_id: "s1".into(),
        system_prompt: "sys".to_owned(),
        context_items: Vec::new(),
        tools: Vec::new(),
        tools_ref: None,
        model: Some("missing/model".into()),
        model_params: Default::default(),
        tool_choice: ToolChoice::Auto,
        originator: tau_proto::PromptOriginator::User,
        share_user_cache_key: false,
        ctx_id: None,
        previous_response_candidate: None,
    };
    let mut auth = ChatCompletionsAuth::default();
    let auth_file_auth = ChatCompletionsAuth::default();
    let mut output = Vec::new();
    let mut writer = FrameWriter::new(&mut output);

    let disconnected = handle_frame(
        Frame::Message(Message::LogEvent(tau_proto::LogEvent {
            id: tau_proto::LogEventId::new(7),
            recorded_at: tau_proto::UnixMicros::default(),
            event: Box::new(Event::SessionPromptCreated(prompt)),
        })),
        &mut writer,
        &mut auth,
        &auth_file_auth,
    )
    .expect("handle logged prompt");

    assert!(!disconnected);
    let mut reader = FrameReader::new(std::io::Cursor::new(output));
    let submitted = reader
        .read_frame()
        .expect("read submitted")
        .expect("submitted frame");
    assert!(matches!(
        submitted,
        Frame::Event(Event::ProviderPromptSubmitted(ProviderPromptSubmitted {
            session_prompt_id,
            ..
        })) if session_prompt_id.as_str() == "sp-logged"
    ));
    assert!(matches!(
        reader
            .read_frame()
            .expect("read finished")
            .expect("finished frame"),
        Frame::Event(Event::ProviderResponseFinished(ProviderResponseFinished {
            session_prompt_id,
            ..
        })) if session_prompt_id.as_str() == "sp-logged"
    ));
    assert!(matches!(
        reader.read_frame().expect("read ack").expect("ack frame"),
        Frame::Message(Message::Ack(Ack { up_to })) if up_to.get() == 7
    ));
    assert!(reader.read_frame().expect("read eof").is_none());
}

#[test]
fn auth_file_provider_replaces_matching_config_provider() {
    // Config and auth file entries are complete provider definitions. If
    // both define the same provider namespace, the auth-file entry replaces
    // the config entry as a whole instead of merging individual fields.
    let config_auth = ChatCompletionsAuth {
        providers: BTreeMap::from([(
            ProviderName::new("ollama"),
            ChatCompletionsProvider {
                base_url: "http://localhost:11434/v1".to_owned(),
                models: vec![ChatCompletionsModel {
                    id: ModelName::new("gemma-4"),
                    display_name: None,
                    context_window: 128_000,
                }],
                ..Default::default()
            },
        )]),
    };
    let auth_file_auth = ChatCompletionsAuth {
        providers: BTreeMap::from([(
            ProviderName::new("ollama"),
            ChatCompletionsProvider {
                base_url: "https://example.invalid/v1".to_owned(),
                api_key: "secret".to_owned(),
                models: vec![ChatCompletionsModel {
                    id: ModelName::new("other-model"),
                    display_name: None,
                    context_window: 128_000,
                }],
                ..Default::default()
            },
        )]),
    };
    let merged = merge_config_and_auth(config_auth, auth_file_auth);
    let provider = &merged.providers[&ProviderName::new("ollama")];

    assert_eq!(provider.base_url, "https://example.invalid/v1");
    assert_eq!(provider.api_key, "secret");
    assert_eq!(provider.models[0].id.as_str(), "other-model");
    assert_eq!(
        models_for_auth(&merged)[0].id.to_string(),
        "ollama/other-model"
    );
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
fn provider_with_reasoning_effort_publishes_effort_levels() {
    // Role effort selection is clamped to the provider-advertised levels.
    // Publishing only `off` made `compat.reasoning_effort` unusable because
    // a role configured with `effort: high` was downgraded before request
    // construction.
    let models = models_for_auth(&auth());

    assert!(models[0].efforts.contains(&tau_proto::Effort::High));
    assert!(models[0].efforts.contains(&tau_proto::Effort::Off));
}

#[test]
fn build_request_flattens_extra_body_for_reasoning_knobs() {
    // OpenAI-compatible local servers disagree on reasoning controls. The
    // provider-level `extra_body` map is intentionally flattened into the
    // request so users can pass backend-specific fields like
    // `chat_template_kwargs.enable_thinking` without Tau hard-coding each
    // variant.
    let mut auth = auth();
    auth.providers
        .get_mut(&ProviderName::new("openai"))
        .expect("provider")
        .extra_body
        .insert(
            "chat_template_kwargs".to_owned(),
            serde_json::json!({ "enable_thinking": true }),
        );
    let (provider, model) = resolve_backend(&auth, &"openai/gpt-4o".into()).expect("backend");
    let prompt = tau_proto::SessionPromptCreated {
        session_prompt_id: "sp-extra".into(),
        session_id: "s1".into(),
        system_prompt: String::new(),
        context_items: Vec::new(),
        tools: Vec::new(),
        tools_ref: None,
        model: Some("openai/gpt-4o".into()),
        model_params: tau_proto::ModelParams {
            effort: tau_proto::Effort::High,
            ..Default::default()
        },
        tool_choice: ToolChoice::Auto,
        originator: tau_proto::PromptOriginator::User,
        share_user_cache_key: false,
        ctx_id: None,
        previous_response_candidate: None,
    };

    let request = serde_json::to_value(build_request(&provider, &model, &prompt)).expect("json");

    assert_eq!(request["reasoning_effort"], "high");
    assert_eq!(request["chat_template_kwargs"]["enable_thinking"], true);
}

#[test]
fn build_request_full_replay_serializes_restored_tool_error_before_next_user_message() {
    // A restored session can contain a repaired foreground tool round: the
    // assistant tool call, the synthetic internal error result, then the
    // user's next prompt. Chat Completions has no chain field, so it must
    // ignore any candidate and serialize the full balanced transcript.
    let (provider, model) = resolve_backend(&auth(), &"openai/gpt-4o".into()).expect("backend");
    let prompt = tau_proto::SessionPromptCreated {
        session_prompt_id: "sp-restored".into(),
        session_id: "s1".into(),
        system_prompt: String::new(),
        context_items: vec![
            restored_tool_call("call-restored"),
            restored_internal_tool_error("call-restored", "partial stdout before restart"),
            user_text("after restart"),
        ],
        tools: Vec::new(),
        tools_ref: None,
        model: Some("openai/gpt-4o".into()),
        model_params: Default::default(),
        tool_choice: ToolChoice::Auto,
        originator: tau_proto::PromptOriginator::User,
        share_user_cache_key: false,
        ctx_id: None,
        previous_response_candidate: Some(tau_proto::PreviousResponseCandidate {
            provider_response_id: "resp_stale_after_restore".to_owned(),
            next_item_index: 2,
            backend: ProviderBackend {
                kind: ProviderBackendKind::Responses,
                base_url: "https://chatgpt.com/backend-api".to_owned(),
                transport: ProviderBackendTransport::HttpSse,
                stale_chain_fallback: false,
            },
        }),
    };

    let request = serde_json::to_value(build_request(&provider, &model, &prompt)).expect("json");
    let object = request.as_object().expect("request object");
    assert!(object.get("previous_response_id").is_none());
    assert!(object.get("previous_response_candidate").is_none());

    let messages = request["messages"].as_array().expect("messages");
    assert_eq!(
        messages.len(),
        3,
        "chat completions must ignore chain hints and replay every restored item"
    );
    assert_eq!(messages[0]["role"], "assistant");
    assert!(messages[0]["content"].is_null());
    assert_eq!(messages[0]["tool_calls"][0]["id"], "call-restored");
    assert_eq!(messages[0]["tool_calls"][0]["function"]["name"], "shell");
    assert_eq!(messages[1]["role"], "tool");
    assert_eq!(messages[1]["tool_call_id"], "call-restored");
    let output = messages[1]["content"].as_str().expect("tool output");
    assert!(output.contains("error: tau_internal: true"));
    assert!(output.contains("Tool call `call-restored` was interrupted"));
    assert!(output.contains("partial stdout before restart"));
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(messages[2]["content"], "after restart");
}

#[test]
fn apply_event_streams_reasoning_fields_and_think_tags() {
    // Reasoning-capable Chat Completions servers are not unified: some send
    // dedicated reasoning deltas, while others leave visible `<think>` tags
    // in content. Normalize both into ProviderResponseUpdated.thinking and
    // keep only answer text in the visible response.
    let mut state = StreamState::new();
    let mut updates = Vec::new();
    let mut on_update = |text: &str, thinking: Option<&str>| {
        updates.push((text.to_owned(), thinking.map(str::to_owned)));
    };

    apply_event(
        &mut state,
        &serde_json::json!({
            "choices": [{ "delta": { "reasoning_content": "plan " } }]
        }),
        &mut on_update,
    );
    apply_event(
        &mut state,
        &serde_json::json!({
            "choices": [{ "delta": { "content": "visible <thi" } }]
        }),
        &mut on_update,
    );
    apply_event(
        &mut state,
        &serde_json::json!({
            "choices": [{ "delta": { "content": "nk>tag</think> answer" } }]
        }),
        &mut on_update,
    );
    flush_pending_content(&mut state, &mut on_update);

    assert_eq!(state.text, "visible  answer");
    assert_eq!(state.thinking, "plan tag");
    assert_eq!(updates.last().expect("update").0, "visible  answer");
    assert_eq!(
        updates.last().expect("update").1.as_deref(),
        Some("plan tag")
    );
}

#[test]
fn build_request_skips_blank_user_messages_and_emits_tools() {
    // Some OpenAI-compatible APIs reject whitespace-only user messages. The
    // request builder drops them while preserving tool declarations.
    let (provider, model) = resolve_backend(&auth(), &"openai/gpt-4o".into()).expect("backend");
    let prompt = tau_proto::SessionPromptCreated {
        session_prompt_id: "sp-1".into(),
        session_id: "s1".into(),
        system_prompt: "sys".to_owned(),
        context_items: vec![ContextItem::Message(tau_proto::MessageItem {
            role: ContextRole::User,
            content: vec![ContentPart::Text {
                text: "   ".to_owned(),
            }],
            phase: None,
        })],
        tools: vec![ToolDefinition {
            name: tau_proto::ToolName::new("read"),
            model_visible_name: None,
            description: Some("Read a file".to_owned()),
            tool_type: ToolType::Function,
            parameters: Some(serde_json::json!({"type":"object"})),
            format: None,
        }],
        tools_ref: None,
        model: Some("openai/gpt-4o".into()),
        model_params: Default::default(),
        tool_choice: ToolChoice::Auto,
        originator: tau_proto::PromptOriginator::User,
        share_user_cache_key: false,
        ctx_id: None,
        previous_response_candidate: None,
    };

    let request = serde_json::to_value(build_request(&provider, &model, &prompt)).expect("json");

    assert_eq!(request["messages"].as_array().expect("messages").len(), 1);
    assert_eq!(request["tools"].as_array().expect("tools").len(), 1);
    assert_eq!(request["tool_choice"], "auto");
    assert_eq!(request["stream_options"]["include_usage"], true);
}
