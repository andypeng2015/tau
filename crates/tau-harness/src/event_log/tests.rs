use super::*;

fn info(message: &str) -> Event {
    Event::HarnessInfo(tau_proto::HarnessInfo {
        message: message.to_owned(),
        level: tau_proto::HarnessInfoLevel::Normal,
    })
}

#[test]
fn append_and_get() {
    let log = EventLog::new();
    let (seq, recorded_at) = log.append(Some("conn-1".into()), info("hello"));
    assert_eq!(seq, 0);
    assert!(recorded_at.get() > 0, "append should stamp wall-clock time");

    let entry = log.get_next_from(0).expect("entry should exist");
    assert_eq!(entry.seq, 0);
    assert_eq!(entry.recorded_at, recorded_at);
    assert_eq!(entry.source, Some("conn-1".into()));

    assert!(log.get_next_from(1).is_none());
}

#[test]
fn get_next_from_skips_earlier() {
    let log = EventLog::new();
    log.append(None, info("a"));
    log.append(None, info("b"));
    log.append(None, info("c"));

    let entry = log.get_next_from(1).expect("entry should exist");
    assert_eq!(entry.seq, 1);
    let Event::HarnessInfo(info) = &entry.event else {
        panic!("expected HarnessInfo");
    };
    assert_eq!(info.message, "b");
}
