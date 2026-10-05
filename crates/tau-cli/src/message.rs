//! Single-delivery shell messaging, with response-only stdout and no retries.

use std::io::{Read as _, Write as _};

use tau_proto::{
    ExternalAgentMessageRecipient, HarnessInputMessage, HarnessOutputMessage, SessionId,
};

use crate::CliError;

#[cfg(test)]
mod tests;

/// Deliver one message and optionally wait for its single final response.
pub(crate) fn run(
    recipient: &str,
    message: Option<String>,
    wait_response: bool,
) -> Result<(), CliError> {
    let message = match message {
        Some(message) => message,
        None => {
            let mut message = String::new();
            std::io::stdin()
                .take(64 * 1024 + 1)
                .read_to_string(&mut message)?;
            message
        }
    };
    if message.len() > 64 * 1024 {
        return Err(error("message exceeds the 64 KiB limit"));
    }
    let (session_id, recipient) = resolve_recipient(recipient)?;
    let harness = tau_harness::runtime_dir::find_harness_for_session(session_id.as_str())
        .map_err(|err| error(err.to_string()))?
        .ok_or_else(|| error("recipient session is not running"))?;
    let (mut reader, mut writer, version) = crate::ui_client::connect_ui_client_with_version(
        &tau_harness::runtime_dir::socket_path(&harness),
        "tau-message",
        Some(&session_id),
    )?;
    if !version.is_some_and(|version| version >= tau_proto::ProtocolVersion::new(10, 4)) {
        return Err(error(
            "recipient harness does not support tau message (requires protocol 10.4)",
        ));
    }
    crate::ui_client::send_message(
        &mut writer,
        &HarnessInputMessage::CliMessage(tau_proto::CliMessageRequest {
            session_id,
            recipient,
            message,
            wait_response,
        }),
    )
    .map_err(|err| error(format!("{}: {err}", disconnect_diagnostic(false))))?;
    let mut delivered = false;
    loop {
        let result = reader
            .read_message()
            .map_err(|err| error(format!("{}: {err}", disconnect_diagnostic(delivered))))?
            .ok_or_else(|| error(disconnect_diagnostic(delivered)))?;
        match result {
            HarnessOutputMessage::CliMessageResult(tau_proto::CliMessageResult::Delivered {
                ..
            }) => {
                if delivered {
                    return Err(error("duplicate delivery acknowledgement"));
                }
                delivered = true;
                eprintln!("message delivered");
                if !wait_response {
                    return Ok(());
                }
            }
            HarnessOutputMessage::CliMessageResult(tau_proto::CliMessageResult::Response {
                message,
            }) => {
                if !delivered || !wait_response {
                    return Err(error("unexpected response before delivery acknowledgement"));
                }
                let mut stdout = std::io::stdout().lock();
                stdout.write_all(message.as_bytes())?;
                stdout.flush()?;
                return Ok(());
            }
            HarnessOutputMessage::CliMessageResult(tau_proto::CliMessageResult::Rejected {
                reason,
            }) => {
                return Err(error(format!("message rejected: {reason}")));
            }
            HarnessOutputMessage::Disconnect(_) => {
                return Err(error(disconnect_diagnostic(delivered)));
            }
            _ => {}
        }
    }
}

fn error(message: impl Into<String>) -> CliError {
    CliError::Participant(message.into())
}

fn disconnect_diagnostic(delivered: bool) -> &'static str {
    if delivered {
        "caller disconnected before final response; delivered recipient work was not cancelled"
    } else {
        "connection ended before delivery acknowledgement; delivery is uncertain; no retry was attempted"
    }
}

fn resolve_recipient(raw: &str) -> Result<(SessionId, ExternalAgentMessageRecipient), CliError> {
    if raw == "user" {
        return Err(error("unsupported message recipient: `user`"));
    }
    let address = raw.strip_prefix('&').unwrap_or(raw);
    if let Some((session, agent)) = address.split_once('/') {
        let session = SessionId::parse(session).map_err(|err| error(err.to_string()))?;
        let agent = tau_proto::AgentId::parse(agent.strip_prefix('@').unwrap_or(agent))
            .map_err(|err| error(err.to_string()))?;
        return Ok((session, ExternalAgentMessageRecipient::Exact(agent)));
    }
    if raw.starts_with('&') {
        return Ok((
            SessionId::parse(address).map_err(|err| error(err.to_string()))?,
            ExternalAgentMessageRecipient::BareEntrypoint,
        ));
    }
    let agent = tau_proto::AgentId::parse(raw).map_err(|err| error(err.to_string()))?;
    // Inside a harness-launched shell, local addressing retains its session
    // meaning. Outside one, discover a unique live owner rather than guessing.
    if let Some(session) = std::env::var_os("TAU_SESSION_ID") {
        let session = session
            .into_string()
            .map_err(|_| error("TAU_SESSION_ID is not UTF-8"))?;
        return Ok((
            SessionId::parse(&session).map_err(|err| error(err.to_string()))?,
            ExternalAgentMessageRecipient::Exact(agent),
        ));
    }
    let mut owners = Vec::new();
    for session in tau_harness::runtime_dir::list_running_sessions()? {
        let Some(harness) =
            tau_harness::runtime_dir::find_harness_for_session(session.session_id.as_str())
                .map_err(|err| error(err.to_string()))?
        else {
            return Err(error(
                "session disappeared during recipient lookup; use an exact session address",
            ));
        };
        let agents = crate::list_agents::request_at_socket(
            &tau_harness::runtime_dir::socket_path(&harness),
            &session.session_id,
            tau_proto::SessionAgentListScope::Current,
        )?;
        if agents.iter().any(|entry| {
            entry.agent_id == agent
                && matches!(
                    entry.lifecycle,
                    tau_proto::SessionAgentLifecycle::Live { .. }
                )
        }) {
            owners.push(session.session_id);
        }
    }
    let session = unique_owner(owners)?;
    Ok((session, ExternalAgentMessageRecipient::Exact(agent)))
}

fn unique_owner(mut owners: Vec<SessionId>) -> Result<SessionId, CliError> {
    if owners.len() != 1 {
        return Err(error(
            "agent recipient is unavailable or ambiguous; use [&]session-id/[@]agent-id",
        ));
    }
    Ok(owners.remove(0))
}
