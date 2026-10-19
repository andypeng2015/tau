use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use super::*;

/// Capture ordinary diagnostics so their privacy and flood bounds are tested.
#[derive(Clone, Default)]
struct LogWriter {
    /// Shared formatted log bytes.
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes
            .lock()
            .expect("log lock")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Repetition, novel-kind floods, hostile labels and private field values must
/// not turn compatibility logging into an unbounded payload sink.
#[test]
fn ignored_elements_log_once_with_bounded_private_free_labels() {
    let writer = LogWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer({
            let writer = writer.clone();
            move || writer.clone()
        })
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let mut diagnostics = UnknownDiagnostics::default();
        for _ in 0..100 {
            diagnostics.fields(
                "test.metadata",
                &serde_json::json!({"future_counter": "PRIVATE_VALUE_CANARY"}),
                &[],
            );
        }
        diagnostics.observe("test.kind", Some("private\nlabel"));
        for index in 0..100 {
            diagnostics.observe("test.kind", Some(&format!("future_kind_{index}")));
        }
    });
    let log = String::from_utf8(writer.bytes.lock().expect("lock").clone()).expect("utf8");
    assert_eq!(log.lines().count(), 17);
    assert_eq!(log.matches("future_counter").count(), 1);
    assert_eq!(log.matches("unknown_kind_limit").count(), 1);
    assert!(log.contains("<unknown>"));
    assert!(!log.contains("PRIVATE_VALUE_CANARY"));
    assert!(!log.contains("private\nlabel"));
}
