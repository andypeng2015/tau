//! Original-event shape validation for native Codex standalone compaction.

use crate::common::LlmError;

mod unknown_diagnostics;

const INVALID_COMPACT_SHAPE: &str =
    "compaction response did not contain exactly one completed compaction item";

/// Progress through the compact-only provider event language.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum CompactItemPhase {
    /// No output slot has appeared.
    #[default]
    Missing,
    /// The selected slot contains an added, but not completed, compaction item.
    Added,
    /// The selected slot contains the one completed compaction item.
    Done,
}

/// Validates native compact shape from original provider events before
/// projection.
#[derive(Debug, Default)]
pub(super) struct CompactStreamShape {
    /// Current output-slot progress.
    item: CompactItemPhase,
    /// Identity observed on the optional added event.
    added_item_id: Option<Option<String>>,
    /// Wire index of the recognized compact item; ignored items may precede it.
    output_index: Option<u64>,
    /// Validated activity notifications, independent of provider sequence
    /// numbers.
    pub(super) progress_updates: u64,
    /// Per-attempt bounded compatibility observations, never output state.
    unknown: unknown_diagnostics::UnknownDiagnostics,
}

impl CompactStreamShape {
    /// Validates live input and reports unknown presence without logging
    /// payloads.
    pub(super) fn validate_observed(
        &mut self,
        event: &serde_json::Value,
        prompt: &str,
    ) -> Result<(), LlmError> {
        let kind = event["type"].as_str().unwrap_or("");
        if let Some(fields) = event.as_object() {
            for key in fields.keys().filter(|key| {
                !matches!(
                    key.as_str(),
                    "type"
                        | "sequence_number"
                        | "response"
                        | "output_index"
                        | "item_id"
                        | "item"
                        | "error"
                        | "code"
                        | "message"
                )
            }) {
                self.unknown.observe(
                    prompt,
                    self.item,
                    "event_field",
                    Some(key),
                    "ignored_informational",
                );
            }
        }
        if matches!(
            kind,
            "response.output_item.added" | "response.output_item.done"
        ) {
            if event["item"]["type"].as_str() != Some("compaction") {
                self.unknown.observe(
                    prompt,
                    self.item,
                    "output_item_type",
                    event["item"]["type"].as_str(),
                    if self.is_ignored_notification(event) {
                        "ignored_informational"
                    } else {
                        "rejected_output_item"
                    },
                );
            }
        } else if matches!(
            kind,
            "response.content_part.added"
                | "response.content_part.done"
                | "response.reasoning_summary_part.added"
                | "response.reasoning_summary_part.done"
        ) {
            if self.is_ignored_notification(event) {
                self.unknown.observe(
                    prompt,
                    self.item,
                    "content_part_type",
                    event["part"]["type"].as_str(),
                    "ignored_informational",
                );
            }
        } else if !matches!(
            kind,
            "response.compaction.compacting"
                | "response.created"
                | "response.in_progress"
                | "response.completed"
                | "response.done"
                | "response.incomplete"
                | "response.failed"
                | "error"
                | "codex.rate_limits"
                | "codex.response.metadata"
                | "responsesapi.websocket_timing"
        ) {
            let disposition = if self.is_ignored_notification(event) {
                "ignored_informational"
            } else {
                "rejected_unknown_shape"
            };
            self.unknown.observe(
                prompt,
                self.item,
                "event_type",
                event["type"].as_str(),
                disposition,
            );
        }
        self.validate(event)
    }

    /// Unknown elements have no semantic authority, regardless of their fields
    /// or namespace. Known incompatible output remains subject to validation.
    pub(super) fn is_ignored_notification(&self, event: &serde_json::Value) -> bool {
        match event["type"].as_str().unwrap_or("") {
            "response.content_part.added"
            | "response.content_part.done"
            | "response.reasoning_summary_part.added"
            | "response.reasoning_summary_part.done" => !matches!(
                event["part"]["type"].as_str(),
                Some("output_text" | "text" | "refusal" | "reasoning_text" | "summary_text")
            ),
            "response.output_item.added" | "response.output_item.done" => !matches!(
                event["item"]["type"].as_str(),
                Some(
                    "compaction"
                        | "message"
                        | "reasoning"
                        | "function_call"
                        | "custom_tool_call"
                        | "web_search_call"
                )
            ),
            "response.compaction.compacting"
            | "response.created"
            | "response.in_progress"
            | "response.completed"
            | "response.done"
            | "response.incomplete"
            | "response.failed"
            | "error"
            | "response.output_text.delta"
            | "response.output_text.done"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_summary_text.done"
            | "response.function_call_arguments.delta"
            | "response.function_call_arguments.done"
            | "response.custom_tool_call_input.delta"
            | "response.custom_tool_call_input.done"
            | "response.web_search_call.in_progress"
            | "response.web_search_call.searching"
            | "response.web_search_call.completed"
            | "response.refusal.delta"
            | "response.refusal.done" => false,
            _ => true,
        }
    }

    /// Validates one original provider event in arrival order.
    pub(super) fn validate(&mut self, event: &serde_json::Value) -> Result<(), LlmError> {
        let event_type = event["type"].as_str().unwrap_or("");
        match event_type {
            "response.compaction.compacting" => {
                if self.item != CompactItemPhase::Added
                    || event["output_index"].as_u64() != self.output_index
                    || event["item_id"].as_str().is_none()
                    || self.added_item_id.as_ref().and_then(|id| id.as_deref())
                        != event["item_id"].as_str()
                {
                    return self.reject("invalid_compacting_notification");
                }
                self.progress_updates = self.progress_updates.saturating_add(1);
                Ok(())
            }
            "response.output_item.added" => {
                if self.is_ignored_notification(event) {
                    return Ok(());
                }
                self.validate_compaction_item(event, CompactItemPhase::Added)
            }
            "response.output_item.done" => {
                if self.is_ignored_notification(event) {
                    return Ok(());
                }
                self.validate_compaction_item(event, CompactItemPhase::Done)
            }
            "response.completed" if self.item == CompactItemPhase::Done => Ok(()),
            "response.completed" | "response.done" => {
                self.reject("terminal_before_completed_item_or_unsupported_done")
            }
            "response.created" | "response.in_progress"
                if self.item == CompactItemPhase::Missing =>
            {
                Ok(())
            }
            "codex.rate_limits" | "codex.response.metadata" | "responsesapi.websocket_timing" => {
                Ok(())
            }
            "response.incomplete" | "response.failed" | "error" => Ok(()),
            _ if self.is_ignored_notification(event) => Ok(()),
            _ => self.reject("unexpected_response_event"),
        }
    }

    /// Validates the sole output slot without projecting or retaining its
    /// payload.
    fn validate_compaction_item(
        &mut self,
        event: &serde_json::Value,
        next: CompactItemPhase,
    ) -> Result<(), LlmError> {
        let index = event["output_index"].as_u64();
        if index.is_none() || event["item"]["type"].as_str() != Some("compaction") {
            return self.reject("expected_compaction_item_at_valid_index");
        }
        if self.output_index.is_some() && self.output_index != index {
            return self.reject("multiple_compaction_item_indices");
        }
        let valid_transition = matches!(
            (self.item, next),
            (
                CompactItemPhase::Missing,
                CompactItemPhase::Added | CompactItemPhase::Done
            ) | (CompactItemPhase::Added, CompactItemPhase::Done)
        );
        if !valid_transition {
            return self.reject("invalid_item_phase_transition");
        }
        let item_id = event["item"]["id"].as_str().map(str::to_owned);
        if next == CompactItemPhase::Added {
            self.added_item_id = Some(item_id);
        } else if self.item == CompactItemPhase::Added
            && self.added_item_id.as_ref() != Some(&item_id)
        {
            return self.reject("added_done_item_identity_mismatch");
        }
        self.output_index = index;
        self.item = next;
        Ok(())
    }

    /// Preserve the exact local rejection branch without logging provider
    /// payloads.
    fn reject(&self, reason: &'static str) -> Result<(), LlmError> {
        Err(LlmError::InvalidResponse(format!(
            "{INVALID_COMPACT_SHAPE}: {reason} (phase={:?})",
            self.item
        )))
    }
}

#[cfg(test)]
mod tests;
