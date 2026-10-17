//! Bounded worker-side media verification and durable transcript protection.

mod completed;
mod owner;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) use completed::ImageVerificationCompleted;
pub(crate) use owner::ImageVerificationOwner;
use tau_proto::{ArtifactError, ToolResult, ToolResultContentPart};

use crate::artifact_store::ArtifactStore;

/// Named worker cuts used by deterministic I/O/cancellation tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ImageVerificationCut {
    /// Exact agent deletion exclusion has been acquired, before CAS access.
    Guarded,
    /// Actual media was validated, before protected backing and roots publish.
    Validated,
    /// Durable roots exist, before custody ends and completion is delivered.
    Retained,
}

/// One tool's immutable reference result awaiting bounded verification.
pub(crate) struct ImageVerificationRequest {
    /// Runtime/source/lease authority, rechecked again on completion.
    pub(crate) owner: ImageVerificationOwner,
    /// Causal result whose one reference must become safe before publication.
    pub(crate) terminal: ToolResult,
    /// Exact logical request identity and sticky cancellation state.
    pub(crate) live: Arc<AtomicBool>,
}

impl ImageVerificationRequest {
    /// Rejects malformed or overlarge requests without starting filesystem
    /// work.
    pub(crate) fn validate_shape(&self) -> Result<(), ArtifactError> {
        let [ToolResultContentPart::ImageReference(reference)] =
            self.terminal.provider_content.as_slice()
        else {
            return Err(ArtifactError::Invalid);
        };
        if self.terminal.tool_type != tau_proto::ToolType::Function
            || self.terminal.kind != tau_proto::ToolResultKind::Final
        {
            return Err(ArtifactError::Invalid);
        }
        tau_core::validate_provider_image_reference(reference).map_err(|_| ArtifactError::Invalid)
    }

    /// Verifies actual media and publishes roots under scoped Agent then CAS
    /// custody. No scope or decoded bytes may escape into the completion queue.
    pub(crate) fn verify(
        &self,
        store: &ArtifactStore,
        connection_live: &AtomicBool,
        cut: impl Fn(ImageVerificationCut),
    ) -> Result<(), ArtifactError> {
        self.validate_shape()?;
        let alive = || self.live.load(Ordering::Acquire) && connection_live.load(Ordering::Acquire);
        if !alive() {
            return Err(ArtifactError::Unavailable);
        }
        let [ToolResultContentPart::ImageReference(reference)] =
            self.terminal.provider_content.as_slice()
        else {
            unreachable!("shape checked");
        };
        self.owner
            .lease
            .with_agent_io_guard(|agent_id| {
                cut(ImageVerificationCut::Guarded);
                if !alive() {
                    return Err(ArtifactError::Unavailable);
                }
                store.retain_checked_transcript_artifact(agent_id, &reference.blob, |bytes| {
                    if !alive() {
                        return Err(ArtifactError::Unavailable);
                    }
                    let mut image = reference.metadata_only_image();
                    image.data = Arc::from(bytes);
                    tau_core::validate_inline_provider_image(&image)
                        .map_err(|_| ArtifactError::Invalid)?;
                    cut(ImageVerificationCut::Validated);
                    if !alive() {
                        return Err(ArtifactError::Unavailable);
                    }
                    Ok(())
                })?;
                cut(ImageVerificationCut::Retained);
                if !alive() {
                    return Err(ArtifactError::Unavailable);
                }
                Ok(())
            })
            .map_err(|_| ArtifactError::Unavailable)?
    }
}
