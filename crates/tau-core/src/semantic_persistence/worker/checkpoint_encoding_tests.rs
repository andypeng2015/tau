use super::*;

/// Valid pretty JSON remains byte-identical, while an escape-heavy name
/// cannot grow the independently charged checkpoint buffer past its limit.
#[test]
fn checkpoint_encoding_bounds_escaped_names_and_preserves_valid_bytes() {
    let mut checkpoint = crate::AgentCheckpoint::new(
        "checkpoint-bound".parse().expect("agent"),
        crate::AgentSummary::default(),
        crate::PersistedAgentEventSeq::new(1),
        &crate::agent_checkpoint::CommittedJournalPosition {
            device: 1,
            inode: 2,
            end_offset: 3,
            boundary: vec![1, 2, 3],
        },
    );
    checkpoint.summary.display_name = Some("small \"escaped\" name".to_owned());
    let bytes = encode_checkpoint(&checkpoint).expect("bounded valid checkpoint");
    assert_eq!(bytes.capacity(), MAX_CHECKPOINT_BYTES);
    assert_eq!(
        bytes,
        serde_json::to_vec_pretty(&checkpoint).expect("legacy pretty bytes")
    );
    checkpoint.summary.display_name = Some("\0".repeat(MAX_CHECKPOINT_BYTES / 2));
    assert!(encode_checkpoint(&checkpoint).is_err());
}
