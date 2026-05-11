use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ModelId;

/// Token usage bucket for one provider/model or the session total.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenUsageCounts {
    /// Number of requests made to the LLM backend.
    pub requests: u64,
    /// Tokens sent to the LLM backend.
    pub sent_tokens: u64,
    /// Sent tokens reported as provider-cache hits.
    pub cached_tokens: u64,
    /// Completed tokens received from the LLM backend.
    pub received_tokens: u64,
    /// Tokens received for the currently streaming response.
    pub in_progress_received_tokens: u64,
}

impl TokenUsageCounts {
    #[must_use]
    pub fn with_in_progress_received(mut self, in_progress_received_tokens: u64) -> Self {
        self.in_progress_received_tokens = in_progress_received_tokens;
        self
    }
}

/// Session-scoped token usage totals, plus per-provider/model buckets.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenUsageStats {
    pub total: TokenUsageCounts,
    pub by_model: BTreeMap<ModelId, TokenUsageCounts>,
}

impl TokenUsageStats {
    pub fn start_request(&mut self, model: &ModelId) {
        self.total.requests = self.total.requests.saturating_add(1);
        self.by_model.entry(model.clone()).or_default().requests = self
            .by_model
            .get(model)
            .map_or(1, |counts| counts.requests.saturating_add(1));
    }

    pub fn add_sent(&mut self, model: &ModelId, sent_tokens: u64, cached_tokens: u64) {
        self.total.sent_tokens = self.total.sent_tokens.saturating_add(sent_tokens);
        self.total.cached_tokens = self.total.cached_tokens.saturating_add(cached_tokens);
        let counts = self.by_model.entry(model.clone()).or_default();
        counts.sent_tokens = counts.sent_tokens.saturating_add(sent_tokens);
        counts.cached_tokens = counts.cached_tokens.saturating_add(cached_tokens);
    }

    pub fn add_received(&mut self, model: &ModelId, received_tokens: u64) {
        self.total.received_tokens = self.total.received_tokens.saturating_add(received_tokens);
        self.by_model
            .entry(model.clone())
            .or_default()
            .received_tokens = self.by_model.get(model).map_or(received_tokens, |counts| {
            counts.received_tokens.saturating_add(received_tokens)
        });
    }
}

/// Usage stats attached to one completed agent response.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentTokenUsage {
    pub model: ModelId,
    pub prompt_sent_tokens: u64,
    pub prompt_cached_tokens: u64,
    pub response_received_tokens: u64,
    pub stats: TokenUsageStats,
}
