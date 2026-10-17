use super::ImageVerificationRequest;

/// Private completion after physical guard custody and decoded bytes are
/// dropped.
pub(crate) struct ImageVerificationCompleted {
    /// Exact request and cancellation identity returned to its runtime owner.
    pub(crate) request: ImageVerificationRequest,
    /// Closed result with no digest, image bytes or filesystem path in errors.
    pub(crate) outcome: Result<(), tau_proto::ArtifactError>,
    /// The shared eight-operation permit survives until main-loop consumption.
    pub(crate) _permit: crate::artifact_worker::Permit,
}
