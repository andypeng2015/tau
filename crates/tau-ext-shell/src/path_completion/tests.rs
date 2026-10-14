use std::time::Duration;

use super::*;

/// Completion uses its supplied extension cwd, preserves spaces/UTF-8 and
/// directory suffixes, and respects an expired fuzzy enumeration deadline.
#[test]
fn completion_uses_extension_workdir_and_preserves_tokens() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(temp.path().join("hé llo")).expect("directory");
    std::fs::write(temp.path().join("hé file"), "").expect("file");
    let result = complete(
        temp.path(),
        "./hé",
        CompletionKind::Path,
        Instant::now() + Duration::from_secs(1),
    );
    assert_eq!(
        result,
        vec![
            PathCandidate {
                value: "./hé llo/".to_owned(),
                directory: true
            },
            PathCandidate {
                value: "./hé file".to_owned(),
                directory: false
            },
        ]
    );
    assert!(complete(temp.path(), "", CompletionKind::PathFuzzy, Instant::now()).is_empty());
}
