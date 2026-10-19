//! Bounded, payload-free observations of ignored provider additions.
//!
//! Implements the backend compatibility diagnostics in
//! `SPEC-provider-response-streaming`.

use std::collections::BTreeSet;

/// Per-attempt compatibility breadcrumbs, independent of semantic state.
#[derive(Clone, Debug, Default)]
pub struct UnknownDiagnostics {
    /// Sanitized location/kind pairs; never provider payload values.
    seen: BTreeSet<(&'static str, String)>,
    /// Whether the single overflow notice has already been emitted.
    overflow_reported: bool,
}

impl UnknownDiagnostics {
    /// Ignore one unknown element and log its bounded kind once per attempt.
    pub fn observe(&mut self, location: &'static str, kind: Option<&str>) {
        let label = safe_label(kind);
        let key = (location, label.to_owned());
        if self.seen.contains(&key) {
            return;
        }
        if self.seen.len() == 16 {
            if !self.overflow_reported {
                self.overflow_reported = true;
                tracing::info!(
                    target: "tau_provider::compatibility",
                    disposition = "unknown_kind_limit",
                    "additional unknown provider elements suppressed",
                );
            }
            return;
        }
        self.seen.insert(key);
        tracing::info!(
            target: "tau_provider::compatibility",
            location, kind = label, disposition = "ignored",
            "unrecognized provider element ignored",
        );
    }

    /// Observe unknown object keys without visiting or logging their values.
    pub fn fields(&mut self, location: &'static str, value: &serde_json::Value, known: &[&str]) {
        if let Some(object) = value.as_object() {
            for key in object.keys().filter(|key| !known.contains(&key.as_str())) {
                self.observe(location, Some(key));
            }
        }
    }
}

/// Render only short identifier-like labels, never arbitrary provider prose.
pub fn safe_label(value: Option<&str>) -> &str {
    value
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 96
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        })
        .unwrap_or("<unknown>")
}

#[cfg(test)]
mod tests;
