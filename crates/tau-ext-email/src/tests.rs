use std::io::{BufReader, BufWriter};
use std::os::unix::net::UnixStream;
use std::thread;

use super::*;

struct FramePair {
    reader: FrameReader<BufReader<UnixStream>>,
    writer: FrameWriter<BufWriter<UnixStream>>,
}

fn spawn_extension() -> FramePair {
    let (ext_stream, harness_stream) = UnixStream::pair().expect("pair");
    let reader_stream = ext_stream.try_clone().expect("clone");
    thread::spawn(move || {
        run(reader_stream, ext_stream).expect("run");
    });
    FramePair {
        reader: FrameReader::new(BufReader::new(
            harness_stream.try_clone().expect("harness clone"),
        )),
        writer: FrameWriter::new(BufWriter::new(harness_stream)),
    }
}

fn read_event(reader: &mut FrameReader<BufReader<UnixStream>>) -> Event {
    loop {
        match reader.read_frame().expect("read").expect("frame") {
            Frame::Event(event) => return event,
            Frame::Message(_) => continue,
        }
    }
}

fn drain_startup(reader: &mut FrameReader<BufReader<UnixStream>>) -> ToolSpec {
    loop {
        match reader.read_frame().expect("read").expect("frame") {
            Frame::Event(Event::ToolRegister(register)) => return register.tool,
            Frame::Message(Message::Ready(_)) => panic!("tool should be registered before ready"),
            _ => {}
        }
    }
}

fn configure(writer: &mut FrameWriter<BufWriter<UnixStream>>, state_dir: PathBuf) {
    writer
        .write_frame(&Frame::Message(Message::Configure(tau_proto::Configure {
            config: CborValue::Map(Vec::new()),
            state_dir: Some(state_dir),
        })))
        .expect("configure");
    writer.flush().expect("flush");
}

fn invoke(command: &str, args: Vec<(&str, CborValue)>) -> Event {
    Event::ToolStarted(ToolStarted {
        call_id: "call-1".into(),
        tool_name: tau_proto::ToolName::new(TOOL_NAME),
        arguments: cbor_map(vec![
            ("command", CborValue::Text(command.to_owned())),
            ("args", cbor_map(args)),
        ]),
        originator: tau_proto::PromptOriginator::User,
    })
}

#[test]
fn registers_single_email_tool() {
    let mut pair = spawn_extension();

    let tool = drain_startup(&mut pair.reader);
    assert_eq!(tool.name.as_str(), TOOL_NAME);
    assert_eq!(tool.execution_mode, ToolExecutionMode::Exclusive);
    assert!(tool.parameters.is_some());
}

#[test]
fn configure_requires_state_dir() {
    let mut pair = spawn_extension();
    let _tool = drain_startup(&mut pair.reader);

    pair.writer
        .write_frame(&Frame::Message(Message::Configure(tau_proto::Configure {
            config: CborValue::Map(Vec::new()),
            state_dir: None,
        })))
        .expect("configure");
    pair.writer.flush().expect("flush");

    loop {
        match pair.reader.read_frame().expect("read").expect("frame") {
            Frame::Message(Message::ConfigError(error)) => {
                assert!(error.message.contains("state_dir"), "{}", error.message);
                break;
            }
            Frame::Message(_) | Frame::Event(_) => {}
        }
    }
}

#[test]
fn known_command_returns_structured_not_implemented() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let state_dir = temp.path().join("email-state");
    let mut pair = spawn_extension();
    let _tool = drain_startup(&mut pair.reader);
    configure(&mut pair.writer, state_dir.clone());

    pair.writer
        .write_frame(&Frame::Event(invoke("list_accounts", vec![])))
        .expect("invoke");
    pair.writer.flush().expect("flush");

    let Event::ToolResult(result) = read_event(&mut pair.reader) else {
        panic!("expected tool result");
    };
    assert_eq!(result.tool_name.as_str(), TOOL_NAME);
    assert_eq!(
        cbor_text_field(&result.result, "command"),
        Some("list_accounts")
    );
    assert_eq!(
        cbor_nested_text_field(&result.result, "error", "code"),
        Some("not_implemented")
    );
    assert_eq!(cbor_bool_field(&result.result, "ok"), Some(false));
}

#[test]
fn rejects_unexpected_command_arguments() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let mut pair = spawn_extension();
    let _tool = drain_startup(&mut pair.reader);
    configure(&mut pair.writer, temp.path().join("email-state"));

    pair.writer
        .write_frame(&Frame::Event(invoke(
            "list_accounts",
            vec![("account", CborValue::Text("default".to_owned()))],
        )))
        .expect("invoke");
    pair.writer.flush().expect("flush");

    let Event::ToolError(error) = read_event(&mut pair.reader) else {
        panic!("expected tool error");
    };
    assert!(
        error.message.contains("unexpected argument"),
        "{}",
        error.message
    );
    let details = error.details.as_ref().expect("details");
    assert_eq!(cbor_text_field(details, "command"), Some("list_accounts"));
    assert_eq!(
        cbor_nested_text_field(details, "error", "code"),
        Some("invalid_arguments")
    );
}

#[test]
fn parser_accepts_phase_a_command_shapes() {
    // This covers the Phase A command envelope directly so refactoring the
    // dispatcher does not accidentally loosen or tighten the documented shapes.
    let cases = [
        (
            command_args("list_accounts", vec![]),
            EmailCommand::ListAccounts,
        ),
        (
            command_args(
                "list_folders",
                vec![("account", CborValue::Text("work".to_owned()))],
            ),
            EmailCommand::ListFolders {
                account: "work".to_owned(),
            },
        ),
        (
            command_args(
                "list",
                vec![
                    ("account", CborValue::Text("work".to_owned())),
                    ("folder", CborValue::Text("INBOX".to_owned())),
                    ("limit", CborValue::Integer(25.into())),
                    ("cursor", CborValue::Text("next-page".to_owned())),
                ],
            ),
            EmailCommand::List {
                account: "work".to_owned(),
                folder: "INBOX".to_owned(),
                limit: 25,
                cursor: Some("next-page".to_owned()),
            },
        ),
        (
            command_args(
                "read",
                vec![
                    ("account", CborValue::Text("work".to_owned())),
                    ("folder", CborValue::Text("INBOX".to_owned())),
                    ("uid", CborValue::Text("abc".to_owned())),
                ],
            ),
            EmailCommand::Read {
                account: "work".to_owned(),
                folder: "INBOX".to_owned(),
                uid: "abc".to_owned(),
            },
        ),
        (
            command_args(
                "send",
                vec![
                    ("from", CborValue::Text("me@example.com".to_owned())),
                    (
                        "to",
                        CborValue::Array(vec![CborValue::Text("you@example.com".to_owned())]),
                    ),
                    ("subject", CborValue::Text(String::new())),
                    ("body_text", CborValue::Text("hello".to_owned())),
                    ("reply_to", CborValue::Null),
                    ("attachments", CborValue::Array(Vec::new())),
                ],
            ),
            EmailCommand::Send {
                account: None,
                from: Some("me@example.com".to_owned()),
                to: vec!["you@example.com".to_owned()],
                cc: Vec::new(),
                bcc: Vec::new(),
                subject: String::new(),
                body_text: "hello".to_owned(),
            },
        ),
    ];

    for (arguments, expected) in cases {
        assert_eq!(parse_command(&arguments).expect("parse"), expected);
    }
}

#[test]
fn parser_rejects_invalid_phase_a_values() {
    // Exercise validation branches that convert malformed model arguments into
    // structured tool errors instead of panics or partially parsed commands.
    let cases = [
        (
            command_args(
                "list",
                vec![
                    ("account", CborValue::Text("work".to_owned())),
                    ("folder", CborValue::Text("INBOX".to_owned())),
                    ("limit", CborValue::Integer(0.into())),
                ],
            ),
            "positive integer",
        ),
        (
            command_args(
                "send",
                vec![
                    ("to", CborValue::Array(Vec::new())),
                    ("subject", CborValue::Text("hi".to_owned())),
                    ("body_text", CborValue::Text("body".to_owned())),
                ],
            ),
            "must not be empty",
        ),
        (
            command_args(
                "send",
                vec![
                    ("to", CborValue::Array(vec![CborValue::Text("".to_owned())])),
                    ("subject", CborValue::Text("hi".to_owned())),
                    ("body_text", CborValue::Text("body".to_owned())),
                ],
            ),
            "entries must not be empty",
        ),
        (
            command_args(
                "send",
                vec![
                    (
                        "to",
                        CborValue::Array(vec![CborValue::Text("you@example.com".to_owned())]),
                    ),
                    ("subject", CborValue::Text("hi".to_owned())),
                    ("body_text", CborValue::Text("body".to_owned())),
                    ("attachments", CborValue::Text("not-array".to_owned())),
                ],
            ),
            "attachments` must be an array",
        ),
    ];

    for (arguments, expected_message) in cases {
        let error = parse_command(&arguments).expect_err("reject invalid arguments");
        let message = cbor_nested_text_field(&error, "error", "message").expect("message");
        assert!(
            message.contains(expected_message),
            "{message:?} should contain {expected_message:?}"
        );
    }
}

fn command_args(command: &str, args: Vec<(&str, CborValue)>) -> CborValue {
    cbor_map(vec![
        ("command", CborValue::Text(command.to_owned())),
        ("args", cbor_map(args)),
    ])
}

fn cbor_nested_text_field<'a>(value: &'a CborValue, outer: &str, inner: &str) -> Option<&'a str> {
    let CborValue::Map(entries) = value else {
        return None;
    };
    let nested = entries.iter().find_map(|(key, value)| match key {
        CborValue::Text(key) if key == outer => Some(value),
        _ => None,
    })?;
    cbor_text_field(nested, inner)
}

#[test]
fn rejected_config_is_reported_on_later_tool_calls() {
    // If Configure is rejected, a subsequent tool call should explain that
    // rejected config state instead of claiming only that state_dir was absent.
    let temp = tempfile::TempDir::new().expect("tempdir");
    let mut pair = spawn_extension();
    let _tool = drain_startup(&mut pair.reader);

    pair.writer
        .write_frame(&Frame::Message(Message::Configure(tau_proto::Configure {
            config: cbor_map(vec![("unexpected", CborValue::Bool(true))]),
            state_dir: Some(temp.path().join("email-state")),
        })))
        .expect("configure");
    pair.writer.flush().expect("flush");

    loop {
        match pair.reader.read_frame().expect("read").expect("frame") {
            Frame::Message(Message::ConfigError(error)) => {
                assert!(error.message.contains("unexpected"), "{}", error.message);
                break;
            }
            Frame::Message(_) | Frame::Event(_) => {}
        }
    }

    pair.writer
        .write_frame(&Frame::Event(invoke("list_accounts", vec![])))
        .expect("invoke");
    pair.writer.flush().expect("flush");

    let Event::ToolError(error) = read_event(&mut pair.reader) else {
        panic!("expected tool error");
    };
    assert!(
        error.message.contains("configuration was rejected"),
        "{}",
        error.message
    );
    let details = error.details.as_ref().expect("details");
    assert_eq!(cbor_text_field(details, "command"), Some("list_accounts"));
    assert_eq!(
        cbor_nested_text_field(details, "error", "code"),
        Some("not_configured")
    );
}

fn cbor_bool_field(value: &CborValue, field: &str) -> Option<bool> {
    let CborValue::Map(entries) = value else {
        return None;
    };
    entries.iter().find_map(|(key, value)| match (key, value) {
        (CborValue::Text(key), CborValue::Bool(value)) if key == field => Some(*value),
        _ => None,
    })
}
