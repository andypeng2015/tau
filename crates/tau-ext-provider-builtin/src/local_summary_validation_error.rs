//! Preserve narrow local-summary retry authority without inspecting error
//! prose.

/// Failure to validate a completed private, no-tool local summary.
#[derive(Debug)]
pub(crate) enum LocalSummaryValidationError {
    /// No assistant narrative message was present.
    MissingNarrative,
    /// The structurally valid narrative contained only whitespace.
    EmptyNarrative,
    /// Structural, role, tool, or byte-limit failures remain terminal.
    Invalid(String),
}

impl LocalSummaryValidationError {
    /// Typed recovery category, independent of the diagnostic string.
    pub(crate) fn failure_kind(&self) -> tau_proto::ProviderFailureKind {
        match self {
            Self::MissingNarrative | Self::EmptyNarrative => {
                tau_proto::ProviderFailureKind::LocalSummaryUnusable
            }
            Self::Invalid(_) => tau_proto::ProviderFailureKind::RequestRejected,
        }
    }
}

impl From<String> for LocalSummaryValidationError {
    fn from(message: String) -> Self {
        Self::Invalid(message)
    }
}

impl std::fmt::Display for LocalSummaryValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::MissingNarrative => "summary compactor did not return exactly one message",
            Self::EmptyNarrative => "summary compactor output is empty",
            Self::Invalid(message) => message,
        })
    }
}
