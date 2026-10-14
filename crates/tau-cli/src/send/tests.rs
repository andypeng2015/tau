use std::io::BufWriter;
use std::os::unix::net as path_std_os_unix_net;
use std::time as path_std_time;

use tau_proto::{
    Event, HarnessInputMessage, HarnessOutputMessage, NodeId, PromptOriginator, UiRoleUpdateAction,
    UiTreeNavigationTarget,
};

use super::{
    command_response_stdout_text, event_for_test_line, message_for_line, read_command_response,
    run_send as run_send_typed,
};
use crate::test_support::TREE_PREVIEW_PARITY_NOTICE;
use crate::ui_prompt::DEFAULT_AGENT_ROLE;

const SESSION_ID: &str = "test-session";

/// Headless execution subscribes before discovery, targets the advertised
/// nonce, sends exactly once, and returns only after authenticated acceptance.
#[test]
fn headless_shell_socket_discovers_submits_once_and_waits_for_acceptance() {
    use tau_shell_interaction::{Operation, Reply, Response, Target};
    let session = SESSION_ID.parse().expect("session");
    let (client, server) = path_std_os_unix_net::UnixStream::pair().expect("socket pair");
    client
        .set_read_timeout(Some(path_std_time::Duration::from_secs(3)))
        .expect("timeout");
    server
        .set_read_timeout(Some(path_std_time::Duration::from_secs(3)))
        .expect("timeout");
    let mut reader = tau_proto::PeerInputReader::new(
        Box::new(client.try_clone().expect("clone")) as Box<dyn std::io::Read + Send>
    );
    let mut writer = tau_proto::PeerOutputWriter::new(BufWriter::new(
        Box::new(client) as Box<dyn std::io::Write + Send>
    ));
    let peer = std::thread::spawn(move || {
        let mut input = tau_proto::HarnessInputReader::new(server.try_clone().expect("clone"));
        let mut output = tau_proto::HarnessOutputWriter::new(server);
        assert!(matches!(
            input.read_message().expect("subscribe"),
            Some(HarnessInputMessage::Subscribe(_))
        ));
        let read_request =
            |input: &mut tau_proto::HarnessInputReader<path_std_os_unix_net::UnixStream>| {
                let Some(HarnessInputMessage::Emit(emit)) = input.read_message().expect("request")
                else {
                    panic!("emit")
                };
                tau_shell_interaction::decode_request(&emit.event).expect("endpoint request")
            };
        let discover = read_request(&mut input);
        assert!(matches!(discover.operation, Operation::Discover));
        let target = Target {
            instance: "remote".parse().expect("instance"),
            nonce: tau_shell_interaction::nonce(),
        };
        let publish =
            |output: &mut tau_proto::HarnessOutputWriter<path_std_os_unix_net::UnixStream>,
             reply: Reply| {
                let HarnessInputMessage::Emit(emit) = tau_shell_interaction::reply_message(&reply)
                else {
                    unreachable!()
                };
                let mut delivery =
                    tau_proto::EventDelivery::live(tau_proto::UnixMicros::now(), *emit.event);
                delivery.publisher = Some(tau_proto::EventPublisher::Extension {
                    name: reply.target.instance.clone(),
                    instance_id: tau_proto::ExtensionInstanceId::new(1),
                    connection_id: "remote-connection".parse().expect("connection"),
                });
                output
                    .write_message(&HarnessOutputMessage::Deliver(delivery))
                    .expect("reply");
                output.flush().expect("flush");
            };
        publish(
            &mut output,
            Reply {
                context: discover.context,
                target: target.clone(),
                response: Response::Available {
                    execute: true,
                    complete: true,
                },
            },
        );
        assert!(matches!(
            read_request(&mut input).operation,
            Operation::Running
        ));
        let execute = read_request(&mut input);
        assert_eq!(execute.target.as_ref(), Some(&target));
        assert_eq!(execute.context.agent_id, None);
        assert!(
            matches!(&execute.operation, Operation::Execute { command, include_in_context: false } if command == "printf 'hé llo'")
        );
        let started = tau_proto::UiShellCommand {
            session_id: execute.context.session_id.clone(),
            command_id: execute.context.request_id.parse().expect("command ID"),
            command: "printf 'hé llo'".to_owned(),
            include_in_context: false,
            target_agent_id: None,
        };
        publish(
            &mut output,
            Reply {
                context: execute.context,
                target,
                response: Response::Started(tau_shell_interaction::RunningCommand {
                    owner: "headless-ui".parse().expect("owner"),
                    command: started,
                }),
            },
        );
        assert!(
            input.read_message().expect("client detached").is_none(),
            "no execution retry"
        );
    });
    super::send_shell_connected(
        &session,
        "!!remote printf 'hé llo'",
        &"remote".parse().expect("instance"),
        &mut reader,
        &mut writer,
        path_std_time::Instant::now() + path_std_time::Duration::from_secs(3),
    )
    .expect("accepted");
    drop(reader);
    drop(writer);
    peer.join().expect("peer");
}

/// A spent absolute deadline cannot be extended by buffered irrelevant frames.
#[test]
fn headless_shell_deadline_never_retries_execution() {
    let mut reader = tau_proto::PeerInputReader::new(
        Box::new(std::io::empty()) as Box<dyn std::io::Read + Send>
    );
    let mut writer = tau_proto::PeerOutputWriter::new(BufWriter::new(
        Box::new(std::io::sink()) as Box<dyn std::io::Write + Send>
    ));
    let error = super::send_shell_connected(
        &SESSION_ID.parse().expect("session"),
        "!!remote pwd",
        &"remote".parse().expect("instance"),
        &mut reader,
        &mut writer,
        path_std_time::Instant::now(),
    )
    .expect_err("expired");
    assert!(
        error
            .to_string()
            .contains("discovery failed before submission")
    );
    assert!(error.to_string().contains("execution was not retried"));
}

fn event(text: &str) -> Option<Event> {
    event_for_test_line(SESSION_ID, text)
}

fn message(text: &str) -> Option<HarnessInputMessage> {
    message_for_line(SESSION_ID, text)
}

fn run_send(session_id: &str, line: &str) -> Result<(), crate::CliError> {
    run_send_typed(
        &session_id.parse().expect("test session id must be valid"),
        line,
    )
}

fn prompt_text(text: &str) -> String {
    match event(text).expect("prompt event") {
        Event::UiCreateAgent(req) => {
            assert_eq!(req.session_id, SESSION_ID);
            assert_eq!(req.role, DEFAULT_AGENT_ROLE);
            assert_eq!(req.model_override, None);
            assert_eq!(req.originator, PromptOriginator::User);
            let ctx_id = req.ctx_id.as_deref().expect("prompt correlation id");
            assert!(!ctx_id.is_empty());
            assert_ne!(ctx_id, req.request_id);
            req.initial_prompt.expect("initial prompt")
        }
        other => panic!("expected UiCreateAgent, got {other:?}"),
    }
}

/// Headless send intentionally treats interactive-only exit commands as
/// no-ops.
#[test]
fn quit_session_and_detach_are_no_ops() {
    assert_eq!(event(":quit"), None);
    assert_eq!(event(":quit-session"), None);
    assert_eq!(event(":detach"), None);
}

/// Headless send must accept exact interactive picker commands as explicit
/// no-ops while rejecting argument-bearing variants locally.
#[test]
fn agent_picker_commands_are_exact_headless_no_ops() {
    for command in [":pick-agent", ":pick-agent-all"] {
        run_send("definitely-not-running", command).expect("exact picker command must be a no-op");
        let malformed = format!("{command} unexpected");
        let error = run_send("definitely-not-running", &malformed)
            .expect_err("picker arguments must fail before daemon lookup");
        assert!(error.to_string().contains("unknown or unsupported command"));
    }
}

/// `:session-stats` is interactive-only, so headless send accepts its exact
/// form without daemon contact while rejecting accidental arguments.
#[test]
fn session_stats_is_an_exact_headless_no_op() {
    run_send("definitely-not-running", ":session-stats")
        .expect("exact session stats command must be a no-op");
    let error = run_send("definitely-not-running", ":session-stats unexpected")
        .expect_err("session stats arguments must fail before daemon lookup");
    assert!(error.to_string().contains("unknown or unsupported command"));
}

/// `:cancel` maps to the broadcast cancel form; the harness may later
/// retarget it.
#[test]
fn cancel_requests_prompt_cancellation() {
    match event(":cancel").expect("cancel event") {
        Event::UiCancelPrompt(cancel) => {
            assert_eq!(cancel.session_id, SESSION_ID);
            assert_eq!(cancel.agent_prompt_id, None);
        }
        other => panic!("expected UiCancelPrompt, got {other:?}"),
    }
}

/// Headless send must provide the same exact logical-retry control as the
/// interactive static command rather than submitting prompt text.
#[test]
fn retry_requests_exact_delayed_prompt_release() {
    match event(":retry").expect("retry event") {
        Event::UiRetryPrompt(retry) => {
            assert_eq!(retry.session_id, SESSION_ID);
            assert_eq!(retry.target_agent_id, None);
            assert_eq!(retry.agent_prompt_id, None);
        }
        other => panic!("expected UiRetryPrompt, got {other:?}"),
    }
    assert!(
        !matches!(event(":retry later"), Some(Event::UiRetryPrompt(_))),
        "arguments must not invoke retry"
    );
    assert!(matches!(event(":retry "), Some(Event::UiRetryPrompt(_))));
    for malformed in [":retry\tlater", ":retry\nlater"] {
        assert_eq!(
            event(malformed),
            None,
            "malformed retry syntax must neither retry nor become a prompt"
        );
    }
}

/// Headless send shares the approved named/default-all exhausted-extension
/// command grammar instead of converting it into prompt text.
#[test]
fn retry_extension_builds_dedicated_requests() {
    for (line, expected) in [
        (":retry-extension", None),
        (
            ":retry-extension tool-a",
            Some(tau_proto::ExtensionName::parse("tool-a").expect("valid extension name")),
        ),
    ] {
        let HarnessInputMessage::UiRetryExtensionRequest(request) =
            message(line).expect("retry extension message")
        else {
            panic!("expected dedicated retry extension request");
        };
        assert_eq!(request.extension_name, expected);
    }
    for malformed in [":retry-extension bad/name", ":retry-extension tool-a extra"] {
        let error = run_send("definitely-not-running", malformed)
            .expect_err("malformed request must fail before daemon lookup");
        assert!(error.to_string().contains("usage: :retry-extension"));
    }
}

/// Tree commands are daemon-side operations, while malformed navigation
/// stays a prompt.
#[test]
fn tree_commands_request_or_navigate_tree() {
    match message(":tree").expect("tree message") {
        HarnessInputMessage::UiTreeRequest(req) => assert_eq!(req.session_id, SESSION_ID),
        other => panic!("expected UiTreeRequest message, got {other:?}"),
    }

    match event(":tree 42").expect("navigate event") {
        Event::UiNavigateTree(req) => {
            assert_eq!(req.session_id, SESSION_ID);
            assert_eq!(req.target, UiTreeNavigationTarget::PromptAnchor(42));
        }
        other => panic!("expected UiNavigateTree, got {other:?}"),
    }

    match event(":tree 0").expect("root navigate event") {
        Event::UiNavigateTree(req) => {
            assert_eq!(req.session_id, SESSION_ID);
            assert_eq!(req.target, UiTreeNavigationTarget::Root);
        }
        other => panic!("expected UiNavigateTree, got {other:?}"),
    }

    match event(":tree node 42").expect("raw node navigate event") {
        Event::UiNavigateTree(req) => {
            assert_eq!(req.session_id, SESSION_ID);
            assert_eq!(req.target, UiTreeNavigationTarget::Node(NodeId::new(42)));
        }
        other => panic!("expected UiNavigateTree, got {other:?}"),
    }

    assert_eq!(event(":tree nope"), None);
}

/// The one-shot headless tree client consumes the same single multiline notice
/// sent to interactive UIs.
#[test]
fn headless_tree_result_reads_one_multiline_notice() {
    let (reader_stream, mut harness_stream) =
        path_std_os_unix_net::UnixStream::pair().expect("reader stream pair");
    reader_stream
        .set_read_timeout(Some(path_std_time::Duration::from_secs(2)))
        .expect("read timeout");
    let (writer_stream, _discard_stream) =
        path_std_os_unix_net::UnixStream::pair().expect("writer stream pair");
    let (mut reader, _writer) =
        crate::ui_client::connect_ui_streams(reader_stream, writer_stream, "tree-test", None)
            .expect("connect UI streams");
    let mut harness_writer = tau_proto::HarnessOutputWriter::new(&mut harness_stream);
    harness_writer
        .write_message(&HarnessOutputMessage::deliver_live(
            tau_proto::UnixMicros::now(),
            Event::HarnessNotice(tau_proto::HarnessNotice::diagnostic(
                tau_proto::notice_kind::HARNESS_NOTICE,
                "concurrent diagnostic",
                tau_proto::NoticeLevel::Info,
            )),
        ))
        .expect("write concurrent diagnostic");
    harness_writer
        .write_message(&HarnessOutputMessage::deliver_live(
            tau_proto::UnixMicros::now(),
            Event::HarnessNotice(tau_proto::HarnessNotice {
                kind: tau_proto::notice_kind::HARNESS_NOTICE.to_owned(),
                message: TREE_PREVIEW_PARITY_NOTICE.to_owned(),
                level: tau_proto::NoticeLevel::Info,
                purpose: tau_proto::NoticePurpose::Response,
            }),
        ))
        .expect("write tree result");
    harness_writer.flush().expect("flush tree result");

    let result = read_command_response(&mut reader).expect("read tree result");
    assert_eq!(result, TREE_PREVIEW_PARITY_NOTICE);
    let stdout = command_response_stdout_text(&result);
    assert_eq!(
        stdout.as_bytes(),
        format!("{TREE_PREVIEW_PARITY_NOTICE}\n").as_bytes()
    );
    assert!(
        stdout
            .chars()
            .all(|character| character == '\n' || !character.is_control())
    );
}

/// `:compact` must reach the harness instead of being sent as prompt text.
#[test]
fn compact_requests_compaction() {
    match event(":compact").expect("compact event") {
        Event::UiCompactRequest(req) => assert_eq!(req.session_id, SESSION_ID),
        other => panic!("expected UiCompactRequest, got {other:?}"),
    }
}

/// Local configuration commands are ignored by `tau send`; they only make
/// sense in chat UI.
#[test]
fn local_configuration_commands_are_ignored() {
    let command = ":fast";
    assert_eq!(event(command), None);
    assert!(
        run_send("definitely-not-running", command).is_ok(),
        "{command} must be a valid headless no-op"
    );
}

/// Role, model, and effort selection commands are forwarded to their distinct
/// control events.
#[test]
fn role_and_model_selection_commands_are_distinct() {
    assert_eq!(event(":role"), None);

    match event(":role reviewer").expect("role select") {
        Event::UiRoleSelect(select) => assert_eq!(select.role, "reviewer"),
        other => panic!("expected UiRoleSelect, got {other:?}"),
    }

    match event(":model test/model").expect("agent model select") {
        Event::UiAgentModelSelect(select) => {
            assert_eq!(select.session_id, SESSION_ID);
            assert_eq!(select.target_agent_id, None);
            assert_eq!(select.model.to_string(), "test/model");
        }
        other => panic!("expected UiAgentModelSelect, got {other:?}"),
    }

    assert_eq!(event(":model reviewer"), None);
    assert_eq!(event(":model "), None);

    match event(":effort 0.75").expect("agent effort select") {
        Event::UiAgentEffortSelect(select) => {
            assert_eq!(select.session_id, SESSION_ID);
            assert_eq!(select.target_agent_id, None);
            assert_eq!(select.effort, Some("0.75".parse().expect("effort")));
        }
        other => panic!("expected UiAgentEffortSelect, got {other:?}"),
    }
    assert_eq!(
        event(":effort reset"),
        Some(crate::ui_events::agent_effort_select(
            &SESSION_ID.parse().expect("session id"),
            None,
            None,
        ))
    );
    assert_eq!(event(":effort 1.1"), None);
}

/// `:role <role> delete` is the headless spelling for deleting a runtime
/// role override.
#[test]
fn role_delete_command_updates_roles() {
    match event(":role scratch delete").expect("role update") {
        Event::UiRoleUpdate(update) => {
            assert_eq!(update.role, "scratch");
            assert_eq!(update.action, UiRoleUpdateAction::Delete);
        }
        other => panic!("expected UiRoleUpdate, got {other:?}"),
    }
}

/// Headless execution requires explicit discovery and a named instance, never a
/// typed legacy shell event or a context append without a selected agent.
#[test]
fn shell_commands_require_endpoint_discovery() {
    let session = SESSION_ID.parse().expect("session");
    assert!(
        matches!(super::classify_send_line(&session, "!!shell echo hi").expect("explicit shell"),
        super::SendLineDisposition::Shell(line) if line == "!!shell echo hi")
    );
    assert!(super::classify_send_line(&session, "!shell echo hi").is_err());
    assert!(super::classify_send_line(&session, "!! echo hi").is_err());
    assert!(event("!!shell echo hi").is_none());
    assert_eq!(event("!!"), None);
    assert_eq!(event("!"), None);
}

/// Unrecognized text is submitted unchanged as a normal user prompt.
#[test]
fn normal_text_submits_user_prompt() {
    assert_eq!(prompt_text("explain this diff"), "explain this diff");
}

/// Ensures headless send does not retain aliases for the removed slash grammar.
#[test]
fn old_slash_commands_submit_as_prompt_text() {
    for line in ["/quit", "/tree", "/model test/model", "/skill demo"] {
        assert_eq!(prompt_text(line), line);
    }
}

/// Ensures headless send removes the doubled-colon escape before constructing
/// the canonical prompt event.
#[test]
fn literal_colon_escape_submits_canonical_prompt_text() {
    assert_eq!(prompt_text("::literal"), ":literal");
    assert_eq!(prompt_text("  ::literal"), ":literal");
    let Event::UiCreateAgent(request) = event("::skill demo").expect("literal prompt event") else {
        panic!("expected create-agent prompt");
    };
    assert_eq!(request.initial_prompt.as_deref(), Some(":skill demo"));
    assert!(request.literal);
}

/// Ensures headless literal Gmail OAuth finish prompts redact callback secrets
/// after canonicalization without broadening the exact shared matcher.
#[test]
fn literal_gmail_oauth_finish_prompt_is_redacted() {
    let Event::UiCreateAgent(request) = event(
        "  ::email auth google finish work \
         http://127.0.0.1:54321/?code=PLACEHOLDER_CODE&state=PLACEHOLDER_STATE",
    )
    .expect("literal prompt event") else {
        panic!("expected create-agent prompt");
    };
    let prompt = request.initial_prompt.as_deref().expect("initial prompt");
    assert_eq!(prompt, ":email auth google finish <redacted>");
    assert!(request.literal);
    assert!(!prompt.contains("PLACEHOLDER_CODE"));
    assert!(!prompt.contains("PLACEHOLDER_STATE"));

    assert_eq!(
        prompt_text(
            "::email auth google start work \
             http://127.0.0.1:54321/?code=PLACEHOLDER_CODE&state=PLACEHOLDER_STATE"
        ),
        ":email auth google start work \
         http://127.0.0.1:54321/?code=PLACEHOLDER_CODE&state=PLACEHOLDER_STATE"
    );
}

/// Ensures unknown colon commands do not silently become model prompts in the
/// headless parser.
#[test]
fn unknown_colon_commands_are_not_prompt_events() {
    assert_eq!(event(":unknown argument"), None);
    assert_eq!(event(":tree invalid"), None);
    let error = run_send("definitely-not-running", ":unknown argument")
        .expect_err("unknown command must fail before daemon lookup");
    assert!(error.to_string().contains("unknown or unsupported command"));
}

/// Ensures malformed interactive-only commands fail locally rather than being
/// silently accepted as headless no-ops.
#[test]
fn malformed_headless_noop_commands_fail_locally() {
    for line in [
        ":quit junk",
        ":new one two",
        ":ephemeral garbage",
        ":theme one two",
        ":set only-name",
        ":session nope",
        ":debug-show-event-stats",
        ":agent bogus",
    ] {
        let error = run_send("definitely-not-running", line)
            .expect_err("malformed command must fail before daemon lookup");
        assert!(
            error.to_string().contains("unknown or unsupported command"),
            "{line}: {error}"
        );
    }
}
