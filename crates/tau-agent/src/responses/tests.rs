use tau_config::settings::PromptCacheRetention;
use tau_proto::{ContentBlock, ConversationMessage, ConversationRole, Effort};

use super::*;
use crate::common::{LlmError, PreviousResponse};

#[test]
fn build_request_includes_prompt_cache_fields_when_configured() {
    let config = ResponsesConfig {
        base_url: "https://chatgpt.com/backend-api".into(),
        api_key: "test".into(),
        model_id: "gpt-5-codex".into(),
        account_id: None,
        supports_reasoning_effort: false,
        supports_reasoning_summary: false,
        prompt_cache_key: Some("tau:seed".into()),
        prompt_cache_retention: Some(PromptCacheRetention::InMemory),
    };
    let request = PromptPayload {
        system_prompt: "system",
        messages: &[],
        tools: &[],
        effort: Effort::Off,
        thinking_summary: tau_proto::ThinkingSummary::Off,
        previous_response: None,
    };

    let body = serde_json::to_value(build_request(&config, &request)).expect("serialize");
    let prompt_cache_key = body["prompt_cache_key"].as_str().expect("prompt_cache_key");

    assert_eq!(prompt_cache_key, "tau:seed");
    assert_eq!(body["prompt_cache_retention"], "in_memory");
}

#[test]
fn build_request_omits_prompt_cache_fields_without_seed_or_retention() {
    let config = ResponsesConfig {
        base_url: "https://chatgpt.com/backend-api".into(),
        api_key: "test".into(),
        model_id: "gpt-5-codex".into(),
        account_id: None,
        supports_reasoning_effort: false,
        supports_reasoning_summary: false,
        prompt_cache_key: None,
        prompt_cache_retention: None,
    };
    let request = PromptPayload {
        system_prompt: "system",
        messages: &[],
        tools: &[],
        effort: Effort::Off,
        thinking_summary: tau_proto::ThinkingSummary::Off,
        previous_response: None,
    };

    let body = serde_json::to_value(build_request(&config, &request)).expect("serialize");
    let object = body.as_object().expect("request object");

    assert!(!object.contains_key("prompt_cache_key"));
    assert!(!object.contains_key("prompt_cache_retention"));
}

/// First turn (no chain established): the request must contain the
/// full transcript, `store: false`, and no `previous_response_id`.
/// This is the baseline that future stateful-chain optimizations are
/// compared against; if it ever flips, every turn would start
/// charging for stored responses by accident.
#[test]
fn build_request_first_turn_replays_full_history_without_chain() {
    let config = chain_test_config();
    let messages = vec![user_text("hello"), assistant_text("hi there")];
    let request = PromptPayload {
        system_prompt: "sys",
        messages: &messages,
        tools: &[],
        effort: Effort::Off,
        thinking_summary: tau_proto::ThinkingSummary::Off,
        previous_response: None,
    };

    let body = serde_json::to_value(build_request(&config, &request)).expect("serialize");

    assert_eq!(body["store"], false);
    assert!(
        body.as_object()
            .unwrap()
            .get("previous_response_id")
            .is_none()
    );
    let input = body["input"].as_array().expect("input array");
    // Two messages → two `input` items (one user text, one assistant message).
    assert_eq!(
        input.len(),
        2,
        "full history must be replayed when chain is absent"
    );
}

/// Stateful-chain turn: when the harness supplies a
/// `previous_response`, the request body slices off the prefix
/// already covered by that response and pins the prior `response.id`.
/// `store` stays `false` — the Codex endpoint *rejects* `store: true`
/// (`HTTP 400 {"detail":"Store must be set to false"}`) even when
/// chaining, in contrast with the public Responses API. Tau today
/// only routes Responses through Codex, so this asserts the Codex
/// shape; a future public-API path would need a separate test.
#[test]
fn build_request_chain_turn_sends_delta_and_previous_response_id() {
    let config = chain_test_config();
    // Full transcript: 1 user, 1 assistant, 1 user tool-result.
    // Chain anchor was captured after the assistant turn
    // (message_index = 2), so only the trailing tool-result should
    // make it into the request.
    let messages = vec![
        user_text("first turn"),
        assistant_text("first response"),
        user_text("second turn"),
    ];
    let request = PromptPayload {
        system_prompt: "sys",
        messages: &messages,
        tools: &[],
        effort: Effort::Off,
        thinking_summary: tau_proto::ThinkingSummary::Off,
        previous_response: Some(PreviousResponse {
            id: "resp_abc",
            message_index: 2,
        }),
    };

    let body = serde_json::to_value(build_request(&config, &request)).expect("serialize");

    assert_eq!(
        body["store"], false,
        "Codex rejects store=true even when chaining"
    );
    assert_eq!(body["previous_response_id"], "resp_abc");
    let input = body["input"].as_array().expect("input array");
    assert_eq!(
        input.len(),
        1,
        "only messages after the anchor should be sent"
    );
    assert_eq!(input[0]["content"][0]["text"], "second turn");
}

/// Defensive: a stale `message_index` (somehow larger than the
/// assembled transcript) must NOT panic and must NOT chain — fall
/// back to a full-replay first-turn-style request so the conversation
/// keeps working instead of crashing the agent.
#[test]
fn build_request_chain_with_oob_index_falls_back_to_full_replay() {
    let config = chain_test_config();
    let messages = vec![user_text("only")];
    let request = PromptPayload {
        system_prompt: "sys",
        messages: &messages,
        tools: &[],
        effort: Effort::Off,
        thinking_summary: tau_proto::ThinkingSummary::Off,
        previous_response: Some(PreviousResponse {
            id: "resp_abc",
            message_index: 99,
        }),
    };

    let body = serde_json::to_value(build_request(&config, &request)).expect("serialize");

    assert_eq!(body["store"], false);
    assert!(
        body.as_object()
            .unwrap()
            .get("previous_response_id")
            .is_none()
    );
    let input = body["input"].as_array().expect("input array");
    assert_eq!(input.len(), 1);
}

/// A 4xx body that mentions `previous_response` is the signal that
/// the upstream's stored state for our chain has been evicted. We
/// detect this so the in-agent fallback can re-send with a full
/// transcript without bothering the user.
#[test]
fn stale_chain_error_detection() {
    let stale = LlmError::HttpStatus(
        400,
        r#"{"error":{"message":"previous_response_id 'resp_x' not found"}}"#.into(),
    );
    assert!(is_stale_chain_error(&stale));

    let unrelated = LlmError::HttpStatus(400, r#"{"error":{"message":"bad request"}}"#.into());
    assert!(!is_stale_chain_error(&unrelated));

    // 500s flow through the harness's transient-retry path, not the
    // chain-fallback path — even if the body happens to mention the
    // chain.
    let server = LlmError::HttpStatus(503, "previous_response upstream blip".into());
    assert!(!is_stale_chain_error(&server));
}

fn chain_test_config() -> ResponsesConfig {
    ResponsesConfig {
        base_url: "https://chatgpt.com/backend-api".into(),
        api_key: "test".into(),
        model_id: "gpt-5-codex".into(),
        account_id: None,
        supports_reasoning_effort: false,
        supports_reasoning_summary: false,
        prompt_cache_key: None,
        prompt_cache_retention: None,
    }
}

fn user_text(text: &str) -> ConversationMessage {
    ConversationMessage {
        role: ConversationRole::User,
        content: vec![ContentBlock::Text { text: text.into() }],
    }
}

fn assistant_text(text: &str) -> ConversationMessage {
    ConversationMessage {
        role: ConversationRole::Assistant,
        content: vec![ContentBlock::Text { text: text.into() }],
    }
}
