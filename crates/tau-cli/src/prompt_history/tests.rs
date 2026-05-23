use super::*;

#[test]
fn appends_and_loads_prompt_history_in_order() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = PromptHistoryStore {
        path: Some(tmp.path().join(HISTORY_FILE)),
    };

    store.append("one").expect("append one");
    store.append("two\nlines").expect("append two");

    assert_eq!(store.load().expect("load"), vec!["one", "two\nlines"]);
}

#[test]
fn ignores_torn_tail_record() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join(HISTORY_FILE);
    let store = PromptHistoryStore {
        path: Some(path.clone()),
    };

    store.append("kept").expect("append kept");
    let mut file = OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open history");
    file.write_all(&8_u64.to_le_bytes()).expect("write length");
    file.write_all(b"torn").expect("write partial payload");

    assert_eq!(store.load().expect("load"), vec!["kept"]);
}

#[test]
fn ignores_malformed_record_and_keeps_reading() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join(HISTORY_FILE);
    let store = PromptHistoryStore {
        path: Some(path.clone()),
    };

    store.append("before").expect("append before");
    let mut file = OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open history");
    file.write_all(&4_u64.to_le_bytes()).expect("write length");
    file.write_all(b"junk").expect("write malformed payload");
    drop(file);
    store.append("after").expect("append after");

    assert_eq!(store.load().expect("load"), vec!["before", "after"]);
}

#[test]
fn append_does_not_read_existing_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join(HISTORY_FILE);
    fs::write(&path, u64::MAX.to_le_bytes()).expect("write corrupt prefix");
    let store = PromptHistoryStore { path: Some(path) };

    store
        .append("new")
        .expect("append skips reading corrupt file");
}

#[test]
fn prompt_history_record_rejects_unknown_fields() {
    // Prompt history is an internal append-only format with an explicit version.
    // Extra fields indicate a schema mismatch, so the record should be skipped.
    let error = serde_json::from_value::<PromptHistoryRecord>(serde_json::json!({
        "version": 1,
        "recorded_at_micros": 42,
        "text": "prompt",
        "extra": true,
    }))
    .expect_err("prompt history record should reject unknown fields");

    assert!(error.to_string().contains("unknown field"), "got: {error}");
}
