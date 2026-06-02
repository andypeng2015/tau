use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use super::*;
use crate::calendar::config::{
    CalendarAccountConfig, CalendarBackendConfig, CalendarExtensionConfig, CalendarSelectionConfig,
};

#[test]
fn parser_unfolds_and_extracts_basic_events() {
    let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:abc\r\nSUMMARY:Hello\r\n world\r\nDTSTART:20260528T120000Z\r\nDTEND:20260528T130000Z\r\nLOCATION:Room\\, 1\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    let events = parse_ics_events(ics).expect("ics parses");

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, "abc");
    assert_eq!(events[0].summary, "Helloworld");
    assert_eq!(events[0].location.as_deref(), Some("Room, 1"));
    assert!(events[0].start_utc.is_some());
}

#[test]
fn parser_keeps_tzid_times_but_marks_them_unparsed() {
    let ics = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:abc\nSUMMARY:Local\nDTSTART;TZID=America/Chicago:20260528T120000\nDTEND;TZID=America/Chicago:20260528T130000\nEND:VEVENT\nEND:VCALENDAR\n";

    let events = parse_ics_events(ics).expect("ics parses");

    assert_eq!(events[0].start, "20260528T120000");
    assert!(events[0].time_unparsed);
}

#[test]
fn range_filter_uses_exclusive_event_overlap() {
    let ics = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:abc\nSUMMARY:UTC\nDTSTART:20260528T120000Z\nDTEND:20260528T130000Z\nEND:VEVENT\nEND:VCALENDAR\n";
    let event = parse_ics_events(ics)
        .expect("ics parses")
        .into_iter()
        .next()
        .expect("event");
    let before = OffsetDateTime::parse("2026-05-28T13:00:00Z", &Rfc3339).expect("time");
    let after = OffsetDateTime::parse("2026-05-28T14:00:00Z", &Rfc3339).expect("time");

    assert!(!event_overlaps(
        &event,
        TimeRange {
            min: Some(before),
            max: Some(after)
        }
    ));
}
#[test]
fn backend_fetches_and_lists_http_feed_events() {
    let ics = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:abc\nSUMMARY:UTC\nDTSTART:20260528T120000Z\nDTEND:20260528T130000Z\nEND:VEVENT\nEND:VCALENDAR\n";
    let (url, handle) = serve_ics_once(ics);
    let cfg = CalendarExtensionConfig {
        enable: true,
        accounts: vec![CalendarAccountConfig {
            id: "feed".to_owned(),
            enable: true,
            backend: Some(CalendarBackendConfig::IcsFeed {
                url_secret: None,
                url: Some(url),
            }),
            calendars: CalendarSelectionConfig {
                default: Some("main".to_owned()),
                allow: vec!["main".to_owned()],
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    let config = cfg.validate().expect("valid config");
    let account = config.accounts.get("feed").expect("feed account");
    let backend = IcsFeedBackend::new(BTreeMap::new());

    let events = backend
        .list_events(account, "main", TimeRange::default(), 10)
        .expect("events list");

    handle.join().expect("server exits");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].summary, "UTC");
}

#[test]
fn backend_lists_ics_events_with_cursor_pages() {
    // Cursor paging should let the model continue a bounded calendar read
    // without asking for an unbounded feed dump.
    let ics = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:first\nSUMMARY:First\nDTSTART:20260528T120000Z\nDTEND:20260528T130000Z\nEND:VEVENT\nBEGIN:VEVENT\nUID:second\nSUMMARY:Second\nDTSTART:20260529T120000Z\nDTEND:20260529T130000Z\nEND:VEVENT\nEND:VCALENDAR\n";
    let (url, handle) = serve_ics_once(ics);
    let cfg = CalendarExtensionConfig {
        enable: true,
        accounts: vec![CalendarAccountConfig {
            id: "feed".to_owned(),
            enable: true,
            backend: Some(CalendarBackendConfig::IcsFeed {
                url_secret: None,
                url: Some(url),
            }),
            calendars: CalendarSelectionConfig {
                default: Some("main".to_owned()),
                allow: vec!["main".to_owned()],
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    let config = cfg.validate().expect("valid config");
    let account = config.accounts.get("feed").expect("feed account");
    let backend = IcsFeedBackend::new(BTreeMap::new());

    let page = backend
        .list_events_page(account, "main", TimeRange::default(), 1, None)
        .expect("events page");

    handle.join().expect("server exits");
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].summary, "First");
    assert_eq!(page.next_cursor.as_deref(), Some("ics:1"));
    assert!(page.truncated);
}

#[test]
fn ics_cursor_rejects_other_backend_cursors() {
    // Cursor values are intentionally backend-prefixed so agents cannot
    // accidentally replay a Google page token into an ICS feed query.
    assert_eq!(parse_ics_cursor(None), Ok(0));
    assert_eq!(parse_ics_cursor(Some("ics:12")), Ok(12));
    assert!(parse_ics_cursor(Some("google:token")).is_err());
}

fn serve_ics_once(body: &'static str) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
    let addr = listener.local_addr().expect("local addr");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept request");
        let mut buf = [0_u8; 1024];
        let _ = stream.read(&mut buf);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/calendar\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write response");
    });
    (format!("http://{addr}/calendar.ics"), handle)
}
