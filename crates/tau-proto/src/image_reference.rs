use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{ArtifactDescriptor, ImageContent, ImageDetail, ImageMediaType};

#[cfg(test)]
mod tests;

/// Exact prepared encoded bytes in the shared CAS, not the source image or
/// recipe.
///
/// A descriptor proves neither media validity nor tool authority. The harness
/// verifies both before canonical admission and establishes transcript
/// retention before admitting a durable reference. Providers verify fetched
/// bytes before use.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageReference {
    /// BLAKE3 and exact encoded size of the prepared canonical image.
    pub blob: ArtifactDescriptor,
    /// Closed media type validated against the prepared bytes.
    pub media_type: ImageMediaType,
    /// Validated decoded width in pixels.
    pub width: u32,
    /// Validated decoded height in pixels.
    pub height: u32,
    /// Provider detail selected during preparation.
    pub detail: ImageDetail,
}

impl ImageReference {
    /// Returns safe metadata in the established empty-image projection shape.
    ///
    /// The result is for generic observers only, never canonical image
    /// admission.
    #[must_use]
    pub fn metadata_only_image(&self) -> ImageContent {
        ImageContent {
            media_type: self.media_type,
            data: Arc::from([]),
            width: self.width,
            height: self.height,
            detail: self.detail,
        }
    }
}

impl fmt::Debug for ImageReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImageReference")
            .field("bytes", &self.blob.size.get())
            .field("media_type", &self.media_type)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("detail", &self.detail)
            .finish_non_exhaustive()
    }
}
