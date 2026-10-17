//! Worker-side prewarm admission and stable-prefix lowering.

use tau_provider_codex::{
    CodexRuntime, PrewarmOutcome, Prompt as CodexPrompt, ResolvedConfig, TurnAbort,
};

use crate::LOG_TARGET;

/// Borrow every stable prefix field without changing ordinary inference
/// lowering.
pub(crate) fn prewarm_prompt(
    prewarm: &tau_proto::AgentPromptPrewarmRequested,
    debug_provider_requests: bool,
) -> CodexPrompt<'_> {
    CodexPrompt {
        system_prompt: &prewarm.system_prompt,
        context: &prewarm.context,
        tools: &prewarm.tools,
        hosted_tools: &prewarm.hosted_tools,
        params: prewarm.model_params,
        tool_choice: prewarm.tool_choice,
        compaction: None,
        originator: &prewarm.originator,
        session_id: &prewarm.session_id,
        agent_id: &prewarm.agent_id,
        debug_provider_requests,
    }
}

/// Reject unresolved payloads, then execute the finite selected prewarm
/// operation.
pub(crate) fn handle_resolved_prewarm(
    prewarm: &tau_proto::AgentPromptPrewarmRequested,
    config: &ResolvedConfig,
    codex_runtime: &CodexRuntime,
    refresh_id: Option<&tau_proto::ProviderCacheRefreshId>,
    debug_provider_requests: bool,
    abort: &mut impl TurnAbort,
) -> tau_proto::ProviderCacheRefreshStatus {
    if prewarm.context.has_image_references() {
        return if abort.is_aborted() {
            tau_proto::ProviderCacheRefreshStatus::Cancelled
        } else {
            tau_proto::ProviderCacheRefreshStatus::Failed
        };
    }
    let session_id_str = prewarm.session_id.as_str();
    let request = prewarm_prompt(prewarm, debug_provider_requests);
    tracing::debug!(target: LOG_TARGET, session_id = session_id_str, "starting prompt prewarm");
    match codex_runtime.prewarm_operation(config, session_id_str, &request, refresh_id, abort) {
        PrewarmOutcome::Installed => {
            tracing::debug!(target: LOG_TARGET, session_id = session_id_str, "completed prompt prewarm");
            tau_proto::ProviderCacheRefreshStatus::Succeeded
        }
        PrewarmOutcome::SkippedBusy => {
            tracing::debug!(target: LOG_TARGET, session_id = session_id_str, "skipped prompt prewarm: websocket key is busy");
            tau_proto::ProviderCacheRefreshStatus::Failed
        }
        PrewarmOutcome::Retry(decision) => {
            tracing::debug!(target: LOG_TARGET, session_id = session_id_str, retry_class = ?decision.class, "prompt prewarm ended with retryable provider failure");
            tau_proto::ProviderCacheRefreshStatus::Failed
        }
        PrewarmOutcome::Canceled => {
            tracing::debug!(target: LOG_TARGET, session_id = session_id_str, "prompt prewarm canceled");
            tau_proto::ProviderCacheRefreshStatus::Cancelled
        }
        PrewarmOutcome::Terminal(error) => {
            tracing::debug!(target: LOG_TARGET, session_id = session_id_str, "prompt prewarm failed: {error}");
            tau_proto::ProviderCacheRefreshStatus::Failed
        }
    }
}
