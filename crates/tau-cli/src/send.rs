//! Headless command submission client.

use std::io as path_std_io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tau_proto::{Event, HarnessInputMessage, HarnessOutputMessage};

use crate::CliError;
use crate::ui_prompt::{
    CreateUserAgentPromptOptions, DEFAULT_AGENT_ROLE, PromptCommandHandling,
    create_user_agent_prompt,
};

const COMMAND_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

enum SendLineDisposition {
    Message(Box<HarnessInputMessage>),
    /// Requires discovery and explicit process selection before submission.
    Shell(String),
    Noop,
}

pub(crate) fn run_send(session_id: &tau_proto::SessionId, line: &str) -> Result<(), CliError> {
    let disposition = classify_send_line(session_id, line)?;
    match disposition {
        SendLineDisposition::Message(message) => send_message(session_id, *message),
        SendLineDisposition::Shell(line) => send_shell(session_id, &line),
        SendLineDisposition::Noop => Ok(()),
    }
}

fn classify_send_line(
    session_id: &tau_proto::SessionId,
    line: &str,
) -> Result<SendLineDisposition, CliError> {
    let canonical_line = tau_cli_term::canonical_literal_colon_prompt(line);
    let text = canonical_line.as_deref().unwrap_or(line).trim();
    if text.is_empty() {
        return Ok(SendLineDisposition::Noop);
    }
    if text.starts_with('!') {
        let (_, _, include) = tau_shell_interaction::parse_command(text)
            .map_err(|reason| CliError::Participant(reason.to_owned()))?;
        if include {
            return Err(CliError::Participant("headless send has no selected agent; use !!instance command or attach and select an agent for !".to_owned()));
        }
        return Ok(SendLineDisposition::Shell(text.to_owned()));
    }

    if canonical_line.is_some() {
        let text = crate::chat::redacted_command_echo_line(text);
        return Ok(SendLineDisposition::Message(Box::new(
            HarnessInputMessage::emit(Event::UiCreateAgent(create_user_agent_prompt(
                session_id,
                DEFAULT_AGENT_ROLE,
                text.as_ref(),
                CreateUserAgentPromptOptions {
                    command_handling: PromptCommandHandling::LiteralEscape,
                    ..CreateUserAgentPromptOptions::default()
                },
            ))),
        )));
    }
    if text == ":tree" {
        return Ok(SendLineDisposition::Message(Box::new(
            crate::ui_events::tree_request_message(session_id, None),
        )));
    }
    if let Some(extension_name) = crate::chat::parse_retry_extension_command(text) {
        return extension_name
            .map(|extension_name| {
                SendLineDisposition::Message(Box::new(
                    HarnessInputMessage::UiRetryExtensionRequest(
                        tau_proto::UiRetryExtensionRequest { extension_name },
                    ),
                ))
            })
            .map_err(|message| CliError::Participant(message.to_owned()));
    }
    if let Some(event) = event_for_line(session_id, text) {
        return Ok(SendLineDisposition::Message(Box::new(
            HarnessInputMessage::emit(event),
        )));
    }
    if valid_headless_noop(text) {
        return Ok(SendLineDisposition::Noop);
    }
    Err(CliError::Participant(format!(
        "unknown or unsupported command `{}`",
        text.split_whitespace().next().unwrap_or(text)
    )))
}

/// Discover and send once, then confirm extension acceptance before detaching.
fn send_shell(session_id: &tau_proto::SessionId, line: &str) -> Result<(), CliError> {
    let (instance, _, _) = tau_shell_interaction::parse_command(line)
        .map_err(|reason| CliError::Participant(reason.to_owned()))?;
    let harness = find_daemon_for_session(session_id.as_str()).ok_or_else(|| {
        CliError::Participant(format!("no running daemon for session `{session_id}`"))
    })?;
    let deadline = Instant::now() + COMMAND_RESPONSE_TIMEOUT;
    let (mut reader, mut writer) = crate::ui_client::connect_ui_client_until(
        &tau_harness::runtime_dir::socket_path(&harness),
        "tau-send",
        session_id,
        deadline,
    )?;
    send_shell_connected(
        session_id,
        line,
        &instance,
        &mut reader,
        &mut writer,
        deadline,
    )
}

/// Complete discovery and acceptance on an already admitted exact-session
/// socket.
fn send_shell_connected(
    session_id: &tau_proto::SessionId,
    line: &str,
    instance: &tau_proto::ExtensionName,
    reader: &mut crate::ui_client::UiInputReader,
    writer: &mut crate::ui_client::UiOutputWriter,
    deadline: Instant,
) -> Result<(), CliError> {
    let selectors = [
        tau_shell_interaction::REPLY_EVENT
            .parse()
            .expect("event name"),
        tau_proto::EventName::EXTENSION_READY,
        tau_proto::EventName::EXTENSION_EXITED,
    ]
    .into_iter()
    .map(tau_proto::EventSelector::Exact)
    .collect();
    crate::ui_client::send_message(
        writer,
        &HarnessInputMessage::Subscribe(tau_proto::Subscribe {
            historical_selectors: Vec::new(),
            live_selectors: selectors,
        }),
    )?;
    let mut client = tau_shell_interaction::Client::new(session_id.clone());
    crate::ui_client::send_message(
        writer,
        &tau_shell_interaction::request_message(&client.discover()),
    )?;
    let mut sent = false;
    let mut command_id = None;
    loop {
        if Instant::now() >= deadline {
            return Err(CliError::Participant(format!(
                "shell {}: deadline elapsed; execution was not retried",
                if sent {
                    "outcome unconfirmed"
                } else {
                    "discovery failed before submission"
                },
            )));
        }
        let message = reader.read_message().map_err(|error| {
            CliError::Participant(format!(
                "shell {}: {error}; execution was not retried",
                if sent {
                    "outcome unconfirmed"
                } else {
                    "discovery failed before submission"
                },
            ))
        })?;
        let delivery = match message {
            Some(HarnessOutputMessage::Deliver(delivery)) => delivery,
            None | Some(HarnessOutputMessage::Disconnect(_)) => {
                return Err(CliError::Participant(
                    "shell connection ended; execution was not retried".to_owned(),
                ));
            }
            _ => continue,
        };
        let update = client.observe(&delivery);
        if let Some(diagnostic) = update.diagnostics.into_iter().next() {
            return Err(CliError::Participant(diagnostic));
        }
        if update.events.iter().any(|event| {
            matches!(event, Event::UiShellCommand(command)
            if Some(command.command_id.as_str()) == command_id.as_deref())
        }) {
            println!(
                "{instance}: command accepted; execution continues after this client detaches"
            );
            return Ok(());
        }
        for request in update.requests {
            crate::ui_client::send_message(
                writer,
                &tau_shell_interaction::request_message(&request),
            )?;
        }
        if !sent
            && client
                .endpoints()
                .any(|endpoint| endpoint.execute && &endpoint.target.instance == instance)
        {
            let request = client.execute(line, None).map_err(CliError::Participant)?;
            command_id = Some(request.context.request_id.clone());
            sent = true;
            crate::ui_client::send_message(
                writer,
                &tau_shell_interaction::request_message(&request),
            )
            .map_err(|error| {
                CliError::Participant(format!(
                    "shell send outcome unconfirmed: {error}; execution was not retried"
                ))
            })?;
        }
    }
}

fn send_message(
    session_id: &tau_proto::SessionId,
    message: HarnessInputMessage,
) -> Result<(), CliError> {
    let harness_path = find_daemon_for_session(session_id.as_str()).ok_or_else(|| {
        CliError::Participant(format!("no running daemon for session `{session_id}`"))
    })?;
    let socket_path = tau_harness::runtime_dir::socket_path(&harness_path);
    let effort_command = matches!(
        &message,
        HarnessInputMessage::Emit(tau_proto::Emit { event, .. })
            if matches!(event.as_ref(), Event::UiAgentEffortSelect(_))
    );
    if effort_command
        || matches!(
            message,
            HarnessInputMessage::UiTreeRequest(_) | HarnessInputMessage::UiRetryExtensionRequest(_)
        )
    {
        let deadline = Instant::now() + COMMAND_RESPONSE_TIMEOUT;
        let (mut reader, mut writer, harness_protocol_version) =
            crate::ui_client::connect_ui_client_until_with_version(
                &socket_path,
                "tau-dev-send",
                session_id,
                deadline,
            )?;
        if matches!(message, HarnessInputMessage::UiRetryExtensionRequest(_))
            && !crate::chat::supports_retry_extension(harness_protocol_version)
        {
            return Err(CliError::Participant(
                ":retry-extension requires a harness with protocol 4.1 or newer".to_owned(),
            ));
        }
        if effort_command
            && let Some(error) = crate::chat::agent_effort_support_error(harness_protocol_version)
        {
            return Err(CliError::Participant(error.to_owned()));
        }
        crate::ui_client::send_message(&mut writer, &message)?;
        print!(
            "{}",
            command_response_stdout_text(&read_command_response(&mut reader)?)
        );
    } else {
        let (_reader, mut writer) =
            crate::ui_client::connect_ui_client(&socket_path, "tau-dev-send", Some(session_id))?;
        crate::ui_client::send_message(&mut writer, &message)?;
    }
    Ok(())
}

/// Formats one requester-directed command result for unconditional headless
/// output.
fn command_response_stdout_text(result: &str) -> String {
    format!("{result}\n")
}

#[cfg(test)]
fn message_for_line(session_id: &str, line: &str) -> Option<HarnessInputMessage> {
    let session_id = tau_proto::SessionId::parse(session_id).ok()?;
    match classify_send_line(&session_id, line).ok()? {
        SendLineDisposition::Message(message) => Some(*message),
        SendLineDisposition::Noop | SendLineDisposition::Shell(_) => None,
    }
}

#[cfg(test)]
fn event_for_test_line(session_id: &str, line: &str) -> Option<Event> {
    let HarnessInputMessage::Emit(emit) = message_for_line(session_id, line)? else {
        return None;
    };
    Some(*emit.event)
}

fn event_for_line(session_id: &tau_proto::SessionId, text: &str) -> Option<Event> {
    if matches!(text, ":quit" | ":q" | ":quit-session" | ":detach") {
        return None;
    }
    if text == ":cancel" {
        return Some(crate::ui_events::cancel_prompt(session_id, None));
    }
    if text == ":retry" {
        return Some(crate::ui_events::retry_prompt(session_id, None));
    }
    if let Some(arg) = text.strip_prefix(":tree ")
        && let Ok(target) = crate::ui_commands::parse_tree_navigation_target(arg)
    {
        return Some(crate::ui_events::navigate_tree(session_id, None, target));
    }
    if text == ":compact" {
        return Some(crate::ui_events::compact_request(session_id, None));
    }
    if let Some(rest) = text.strip_prefix(":role ") {
        return role_event_for_command(rest.trim());
    }
    if let Some(model) = text.strip_prefix(":model ") {
        let model = model.trim();
        if let Ok(model) = model.parse::<tau_proto::ModelId>() {
            return Some(crate::ui_events::agent_model_select(
                session_id, None, model,
            ));
        }
        return None;
    }
    if let Some(value) = text.strip_prefix(":effort ") {
        let effort = crate::ui_commands::parse_effort_override(value.trim()).ok()?;
        return Some(crate::ui_events::agent_effort_select(
            session_id, None, effort,
        ));
    }
    if text == ":skill" || text.starts_with(":skill ") || text.starts_with(":skill:") {
        return Some(Event::UiCreateAgent(create_user_agent_prompt(
            session_id,
            DEFAULT_AGENT_ROLE,
            text,
            CreateUserAgentPromptOptions::default(),
        )));
    }
    if !text.starts_with(':') {
        return Some(Event::UiCreateAgent(create_user_agent_prompt(
            session_id,
            DEFAULT_AGENT_ROLE,
            text,
            CreateUserAgentPromptOptions::default(),
        )));
    }
    None
}

fn valid_headless_noop(text: &str) -> bool {
    let args = text.split_whitespace().collect::<Vec<_>>();
    matches!(
        args.as_slice(),
        [":quit"
            | ":q"
            | ":quit-session"
            | ":detach"
            | ":fast"
            | ":suspend"
            | ":resume"
            | ":version"
            | ":role"
            | ":pick-agent"
            | ":pick-agent-all"]
            | [":session-stats"]
            | [":new"]
            | [":new", _]
            | [":name", _, ..]
            | [":ephemeral"]
            | [":ephemeral", "on" | "off"]
            | [":set", _, _]
            | [":theme"]
            | [":theme", _]
            | [":prompt", _]
            | [":session", "new"]
            | [":provider-auth"]
            | [":provider-auth", _]
            | [":debug-show-ui-event-stats"]
            | [":debug-show-event-stats", _]
            | [":agent"]
            | [":agent", "new"]
            | [":agent", "switch" | "suspend" | "resume" | "auto"]
            | [":agent", "switch" | "suspend" | "resume" | "auto", _]
            | [":agent", "name", _, _, ..]
    )
}

fn read_command_response(reader: &mut crate::ui_client::UiInputReader) -> Result<String, CliError> {
    loop {
        let Some(message) = reader.read_message().map_err(path_std_io::Error::other)? else {
            return Err(CliError::Participant(
                "daemon disconnected before returning the command response".to_owned(),
            ));
        };
        match message {
            HarnessOutputMessage::Deliver(delivery) => {
                if let Event::HarnessNotice(notice) = delivery.into_event()
                    && notice.kind == tau_proto::notice_kind::HARNESS_NOTICE
                    && notice.purpose == tau_proto::NoticePurpose::Response
                {
                    return Ok(notice.message);
                }
            }
            HarnessOutputMessage::Disconnect(disconnect) => {
                return Err(CliError::Participant(disconnect.reason.unwrap_or_else(
                    || "daemon disconnected before returning the command response".to_owned(),
                )));
            }
            _ => {}
        }
    }
}

fn role_event_for_command(rest: &str) -> Option<Event> {
    crate::ui_commands::parse_role_command(rest).ok()?
}

fn find_daemon_for_session(session_id: &str) -> Option<PathBuf> {
    tau_harness::runtime_dir::find_harness_for_session(session_id)
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests;
