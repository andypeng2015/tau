//! Image-specific provider services sharing the existing Artifact transport.

/// Main-loop ownership for image production and referenced prompt consumption.
#[derive(Default)]
pub(crate) struct ProviderImages {
    /// Ordinary image-generation tool calls and uploads.
    pub(crate) tools: crate::image_tools::ImageTools,
    /// Verified input bytes and worker RPC correlation.
    pub(crate) resolution: crate::image_resolution::State,
}
