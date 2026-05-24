//! Standard email extension foundation.
//!
//! Phase A intentionally provides only the harness integration surface: strict
//! config parsing, extension state-dir setup, a single `email` tool, and
//! deterministic structured not-implemented responses for the planned commands.

use std::collections::BTreeSet;
use std::error::Error;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;

use tau_proto::{
    Ack, CborValue, ConfigError, Event, Frame, FrameReader, FrameWriter, LogEventId, Message,
    ToolDisplay, ToolDisplayStatus, ToolError, ToolExecutionMode, ToolResult, ToolSpec,
    ToolStarted,
};

/// `tracing` target for events emitted from this extension.
pub const LOG_TARGET: &str = "email";

/// Tau-internal and model-visible tool name for email commands.
pub const TOOL_NAME: &str = "email";

/// Run the extension over stdio.
pub fn run_stdio() -> Result<(), Box<dyn Error>> {
    tau_extension::init_logging_for(LOG_TARGET);
    run(std::io::stdin(), std::io::stdout())
}

/// Run the extension over the supplied reader/writer pair.
pub fn run<R, W>(reader: R, writer: W) -> Result<(), Box<dyn Error>>
where
    R: Read,
    W: Write,
{
    let mut reader = FrameReader::new(BufReader::new(reader));
    let mut writer = FrameWriter::new(BufWriter::new(writer));
    let mut runtime = RuntimeState::default();

    tau_extension::Handshake::tool("tau-ext-email")
        .subscribe([tau_proto::EventName::TOOL_STARTED])
        .register_tool(email_tool_spec())
        .ready_message("email extension ready")
        .run(&mut writer)?;

    while let Some(frame) = reader.read_frame()? {
        let (log_id, inner) = frame.peel_log();
        match inner {
            Frame::Message(Message::Configure(configure)) => {
                if let Err(message) = runtime.configure(configure) {
                    writer.write_frame(&Frame::Message(Message::ConfigError(ConfigError {
                        message,
                    })))?;
                    writer.flush()?;
                }
            }
            Frame::Event(Event::ToolStarted(invoke)) if invoke.tool_name.as_str() == TOOL_NAME => {
                let event = runtime.dispatch(invoke);
                writer.write_frame(&Frame::Event(event))?;
                writer.flush()?;
            }
            Frame::Message(Message::Disconnect(_)) => break,
            _ => {}
        }
        if let Some(id) = log_id {
            ack_log_event(id, &mut writer)?;
        }
    }

    Ok(())
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ExtConfig {}

#[derive(Debug, Default)]
struct RuntimeState {
    config_state: ConfigState,
}

#[derive(Debug, Default)]
enum ConfigState {
    #[default]
    Unconfigured,
    Configured {
        state_dir: PathBuf,
    },
    Rejected {
        reason: String,
    },
}

impl RuntimeState {
    fn configure(&mut self, configure: tau_proto::Configure) -> Result<(), String> {
        match self.try_configure(configure) {
            Ok(state_dir) => {
                self.config_state = ConfigState::Configured { state_dir };
                Ok(())
            }
            Err(message) => {
                self.config_state = ConfigState::Rejected {
                    reason: message.clone(),
                };
                Err(message)
            }
        }
    }

    fn try_configure(&self, configure: tau_proto::Configure) -> Result<PathBuf, String> {
        let _cfg: ExtConfig = tau_extension::parse_config(&configure.config)?;
        let state_dir = configure
            .state_dir
            .ok_or_else(|| "email extension requires Configure.state_dir".to_owned())?;
        std::fs::create_dir_all(&state_dir).map_err(|error| {
            format!(
                "failed to create email extension state directory {}: {error}",
                state_dir.display()
            )
        })?;
        Ok(state_dir)
    }

    fn dispatch(&self, invoke: ToolStarted) -> Event {
        match &self.config_state {
            ConfigState::Configured { state_dir } => {
                let _ = state_dir;
            }
            ConfigState::Unconfigured => {
                let command = command_from_arguments(&invoke.arguments).map(str::to_owned);
                return tool_error(
                    invoke,
                    error_envelope(
                        command.as_deref(),
                        "not_configured",
                        "Configure.state_dir has not been received",
                    ),
                );
            }
            ConfigState::Rejected { reason } => {
                let command = command_from_arguments(&invoke.arguments).map(str::to_owned);
                return tool_error(
                    invoke,
                    error_envelope(
                        command.as_deref(),
                        "not_configured",
                        &format!("email extension configuration was rejected: {reason}"),
                    ),
                );
            }
        }
        match parse_command(&invoke.arguments) {
            Ok(command) => Event::ToolResult(ToolResult {
                call_id: invoke.call_id,
                tool_name: invoke.tool_name,
                tool_type: tau_proto::ToolType::Function,
                result: command.not_implemented_result(),
                kind: tau_proto::ToolResultKind::Final,
                display: Some(display(ToolDisplayStatus::Error, "not implemented")),
                originator: tau_proto::PromptOriginator::User,
            }),
            Err(error) => tool_error(invoke, error),
        }
    }
}

fn ack_log_event<W: Write>(
    id: LogEventId,
    writer: &mut FrameWriter<W>,
) -> Result<(), tau_proto::EncodeError> {
    writer.write_frame(&Frame::Message(Message::Ack(Ack { up_to: id })))?;
    writer.flush().map_err(tau_proto::EncodeError::Io)
}

fn email_tool_spec() -> ToolSpec {
    ToolSpec {
        name: tau_proto::ToolName::new(TOOL_NAME),
        model_visible_name: None,
        description: Some(
            "Interact with configured email accounts. Phase A exposes the stable command envelope only; account, folder, list, read, and send operations currently return structured not_implemented results."
                .to_owned(),
        ),
        tool_type: tau_proto::ToolType::Function,
        parameters: Some(serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "enum": ["list_accounts", "list_folders", "list", "read", "send"],
                    "description": "Email subcommand to execute."
                },
                "args": {
                    "type": "object",
                    "description": "Command-specific arguments.",
                    "additionalProperties": true
                }
            },
            "required": ["command", "args"],
            "additionalProperties": false
        })),
        format: None,
        enabled_by_default: true,
        execution_mode: ToolExecutionMode::Exclusive,
        background_support: None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum EmailCommand {
    ListAccounts,
    ListFolders {
        account: String,
    },
    List {
        account: String,
        folder: String,
        limit: u32,
        cursor: Option<String>,
    },
    Read {
        account: String,
        folder: String,
        uid: String,
    },
    Send {
        account: Option<String>,
        from: Option<String>,
        to: Vec<String>,
        cc: Vec<String>,
        bcc: Vec<String>,
        subject: String,
        body_text: String,
    },
}

impl EmailCommand {
    fn name(&self) -> &'static str {
        match self {
            Self::ListAccounts => "list_accounts",
            Self::ListFolders { .. } => "list_folders",
            Self::List { .. } => "list",
            Self::Read { .. } => "read",
            Self::Send { .. } => "send",
        }
    }

    fn not_implemented_result(&self) -> CborValue {
        error_envelope(
            Some(self.name()),
            "not_implemented",
            &format!("email.{} is not implemented yet", self.name()),
        )
    }
}

fn command_from_arguments(arguments: &CborValue) -> Option<&str> {
    let CborValue::Map(entries) = arguments else {
        return None;
    };
    entries.iter().find_map(|(key, value)| match (key, value) {
        (CborValue::Text(key), CborValue::Text(value)) if key == "command" => Some(value.as_str()),
        _ => None,
    })
}

type CborMapEntries<'a> = &'a [(CborValue, CborValue)];

type CommandEnvelope<'a> = (String, CborMapEntries<'a>);

fn parse_command(arguments: &CborValue) -> Result<EmailCommand, CborValue> {
    let (command, args) = parse_command_envelope(arguments)?;
    match command.as_str() {
        "list_accounts" => parse_list_accounts(&command, args),
        "list_folders" => parse_list_folders(&command, args),
        "list" => parse_list(&command, args),
        "read" => parse_read(&command, args),
        "send" => parse_send(&command, args),
        _ => Err(error_envelope(
            Some(&command),
            "unknown_command",
            "unsupported email command",
        )),
    }
}

fn parse_command_envelope(arguments: &CborValue) -> Result<CommandEnvelope<'_>, CborValue> {
    let CborValue::Map(entries) = arguments else {
        return Err(error_envelope(
            None,
            "invalid_arguments",
            "arguments must be an object",
        ));
    };
    let mut seen = BTreeSet::new();
    let command = required_string(entries, &mut seen, "command", None)?;
    let args = required_object(entries, &mut seen, "args", Some(&command))?;
    reject_extra(entries, &seen, Some(&command))?;
    Ok((command, args))
}

fn parse_list_accounts(
    command: &str,
    args: &[(CborValue, CborValue)],
) -> Result<EmailCommand, CborValue> {
    reject_extra(args, &BTreeSet::new(), Some(command))?;
    Ok(EmailCommand::ListAccounts)
}

fn parse_list_folders(
    command: &str,
    args: &[(CborValue, CborValue)],
) -> Result<EmailCommand, CborValue> {
    let mut seen = BTreeSet::new();
    let account = required_string(args, &mut seen, "account", Some(command))?;
    reject_extra(args, &seen, Some(command))?;
    Ok(EmailCommand::ListFolders { account })
}

fn parse_list(command: &str, args: &[(CborValue, CborValue)]) -> Result<EmailCommand, CborValue> {
    let mut seen = BTreeSet::new();
    let account = required_string(args, &mut seen, "account", Some(command))?;
    let folder = required_string(args, &mut seen, "folder", Some(command))?;
    let limit = required_positive_u32(args, &mut seen, "limit", Some(command))?;
    let cursor = optional_string(args, &mut seen, "cursor", Some(command))?;
    reject_extra(args, &seen, Some(command))?;
    Ok(EmailCommand::List {
        account,
        folder,
        limit,
        cursor,
    })
}

fn parse_read(command: &str, args: &[(CborValue, CborValue)]) -> Result<EmailCommand, CborValue> {
    let mut seen = BTreeSet::new();
    let account = required_string(args, &mut seen, "account", Some(command))?;
    let folder = required_string(args, &mut seen, "folder", Some(command))?;
    let uid = required_string(args, &mut seen, "uid", Some(command))?;
    reject_extra(args, &seen, Some(command))?;
    Ok(EmailCommand::Read {
        account,
        folder,
        uid,
    })
}

fn parse_send(command: &str, args: &[(CborValue, CborValue)]) -> Result<EmailCommand, CborValue> {
    let mut seen = BTreeSet::new();
    let account = optional_string(args, &mut seen, "account", Some(command))?;
    let from = optional_string(args, &mut seen, "from", Some(command))?;
    let to = required_string_array(args, &mut seen, "to", Some(command))?;
    let cc = optional_string_array(args, &mut seen, "cc", Some(command))?;
    let bcc = optional_string_array(args, &mut seen, "bcc", Some(command))?;
    let subject = required_string_allow_empty(args, &mut seen, "subject", Some(command))?;
    let body_text = required_string_allow_empty(args, &mut seen, "body_text", Some(command))?;
    let _reply_to = optional_nullable_string(args, &mut seen, "reply_to", Some(command))?;
    let _in_reply_to = optional_nullable_string(args, &mut seen, "in_reply_to", Some(command))?;
    optional_array(args, &mut seen, "attachments", Some(command))?;
    reject_extra(args, &seen, Some(command))?;
    Ok(EmailCommand::Send {
        account,
        from,
        to,
        cc,
        bcc,
        subject,
        body_text,
    })
}

fn required_string(
    entries: &[(CborValue, CborValue)],
    seen: &mut BTreeSet<String>,
    name: &str,
    command: Option<&str>,
) -> Result<String, CborValue> {
    match field(entries, seen, name, command)? {
        Some(CborValue::Text(value)) if !value.trim().is_empty() => Ok(value.clone()),
        Some(CborValue::Text(_)) => Err(error_envelope(
            command,
            "invalid_arguments",
            &format!("`{name}` must not be empty"),
        )),
        Some(_) => Err(error_envelope(
            command,
            "invalid_arguments",
            &format!("`{name}` must be a string"),
        )),
        None => Err(error_envelope(
            command,
            "invalid_arguments",
            &format!("missing `{name}`"),
        )),
    }
}

fn required_string_allow_empty(
    entries: &[(CborValue, CborValue)],
    seen: &mut BTreeSet<String>,
    name: &str,
    command: Option<&str>,
) -> Result<String, CborValue> {
    match field(entries, seen, name, command)? {
        Some(CborValue::Text(value)) => Ok(value.clone()),
        Some(_) => Err(error_envelope(
            command,
            "invalid_arguments",
            &format!("`{name}` must be a string"),
        )),
        None => Err(error_envelope(
            command,
            "invalid_arguments",
            &format!("missing `{name}`"),
        )),
    }
}

fn optional_string(
    entries: &[(CborValue, CborValue)],
    seen: &mut BTreeSet<String>,
    name: &str,
    command: Option<&str>,
) -> Result<Option<String>, CborValue> {
    match field(entries, seen, name, command)? {
        Some(CborValue::Text(value)) if !value.trim().is_empty() => Ok(Some(value.clone())),
        Some(CborValue::Text(_)) => Err(error_envelope(
            command,
            "invalid_arguments",
            &format!("`{name}` must not be empty"),
        )),
        Some(_) => Err(error_envelope(
            command,
            "invalid_arguments",
            &format!("`{name}` must be a string"),
        )),
        None => Ok(None),
    }
}

fn optional_nullable_string(
    entries: &[(CborValue, CborValue)],
    seen: &mut BTreeSet<String>,
    name: &str,
    command: Option<&str>,
) -> Result<Option<String>, CborValue> {
    match field(entries, seen, name, command)? {
        Some(CborValue::Null) | None => Ok(None),
        Some(CborValue::Text(value)) if !value.trim().is_empty() => Ok(Some(value.clone())),
        Some(CborValue::Text(_)) => Err(error_envelope(
            command,
            "invalid_arguments",
            &format!("`{name}` must not be empty"),
        )),
        Some(_) => Err(error_envelope(
            command,
            "invalid_arguments",
            &format!("`{name}` must be a string or null"),
        )),
    }
}

fn required_positive_u32(
    entries: &[(CborValue, CborValue)],
    seen: &mut BTreeSet<String>,
    name: &str,
    command: Option<&str>,
) -> Result<u32, CborValue> {
    match field(entries, seen, name, command)? {
        Some(CborValue::Integer(value)) => {
            let raw: i128 = (*value).into();
            if raw < 1 || i128::from(u32::MAX) < raw {
                return Err(error_envelope(
                    command,
                    "invalid_arguments",
                    &format!("`{name}` must be a positive integer"),
                ));
            }
            Ok(raw as u32)
        }
        Some(_) => Err(error_envelope(
            command,
            "invalid_arguments",
            &format!("`{name}` must be an integer"),
        )),
        None => Err(error_envelope(
            command,
            "invalid_arguments",
            &format!("missing `{name}`"),
        )),
    }
}

fn required_string_array(
    entries: &[(CborValue, CborValue)],
    seen: &mut BTreeSet<String>,
    name: &str,
    command: Option<&str>,
) -> Result<Vec<String>, CborValue> {
    let Some(value) = field(entries, seen, name, command)? else {
        return Err(error_envelope(
            command,
            "invalid_arguments",
            &format!("missing `{name}`"),
        ));
    };
    string_array_value(value, name, command, false)
}

fn optional_string_array(
    entries: &[(CborValue, CborValue)],
    seen: &mut BTreeSet<String>,
    name: &str,
    command: Option<&str>,
) -> Result<Vec<String>, CborValue> {
    match field(entries, seen, name, command)? {
        Some(value) => string_array_value(value, name, command, true),
        None => Ok(Vec::new()),
    }
}

fn string_array_value(
    value: &CborValue,
    name: &str,
    command: Option<&str>,
    allow_empty: bool,
) -> Result<Vec<String>, CborValue> {
    let CborValue::Array(values) = value else {
        return Err(error_envelope(
            command,
            "invalid_arguments",
            &format!("`{name}` must be an array"),
        ));
    };
    let mut out = Vec::new();
    for value in values {
        let CborValue::Text(text) = value else {
            return Err(error_envelope(
                command,
                "invalid_arguments",
                &format!("`{name}` entries must be strings"),
            ));
        };
        if text.trim().is_empty() {
            return Err(error_envelope(
                command,
                "invalid_arguments",
                &format!("`{name}` entries must not be empty"),
            ));
        }
        out.push(text.clone());
    }
    if out.is_empty() && !allow_empty {
        return Err(error_envelope(
            command,
            "invalid_arguments",
            &format!("`{name}` must not be empty"),
        ));
    }
    Ok(out)
}

fn optional_array(
    entries: &[(CborValue, CborValue)],
    seen: &mut BTreeSet<String>,
    name: &str,
    command: Option<&str>,
) -> Result<(), CborValue> {
    match field(entries, seen, name, command)? {
        Some(CborValue::Array(_)) | None => Ok(()),
        Some(_) => Err(error_envelope(
            command,
            "invalid_arguments",
            &format!("`{name}` must be an array"),
        )),
    }
}

fn required_object<'a>(
    entries: &'a [(CborValue, CborValue)],
    seen: &mut BTreeSet<String>,
    name: &str,
    command: Option<&str>,
) -> Result<&'a [(CborValue, CborValue)], CborValue> {
    match field(entries, seen, name, command)? {
        Some(CborValue::Map(values)) => Ok(values),
        Some(_) => Err(error_envelope(
            command,
            "invalid_arguments",
            &format!("`{name}` must be an object"),
        )),
        None => Err(error_envelope(
            command,
            "invalid_arguments",
            &format!("missing `{name}`"),
        )),
    }
}

fn field<'a>(
    entries: &'a [(CborValue, CborValue)],
    seen: &mut BTreeSet<String>,
    name: &str,
    command: Option<&str>,
) -> Result<Option<&'a CborValue>, CborValue> {
    let mut found = None;
    for (key, value) in entries {
        let CborValue::Text(key) = key else {
            return Err(error_envelope(
                command,
                "invalid_arguments",
                "argument object keys must be strings",
            ));
        };
        if key == name {
            if found.is_some() {
                return Err(error_envelope(
                    command,
                    "invalid_arguments",
                    &format!("duplicate `{name}`"),
                ));
            }
            found = Some(value);
            seen.insert(name.to_owned());
        }
    }
    Ok(found)
}

fn reject_extra(
    entries: &[(CborValue, CborValue)],
    seen: &BTreeSet<String>,
    command: Option<&str>,
) -> Result<(), CborValue> {
    for (key, _) in entries {
        let CborValue::Text(key) = key else {
            return Err(error_envelope(
                command,
                "invalid_arguments",
                "argument object keys must be strings",
            ));
        };
        if !seen.contains(key) {
            return Err(error_envelope(
                command,
                "invalid_arguments",
                &format!("unexpected argument `{key}`"),
            ));
        }
    }
    Ok(())
}

fn tool_error(invoke: ToolStarted, details: CborValue) -> Event {
    let message = cbor_nested_text_field(&details, "error", "message")
        .unwrap_or("invalid email tool request")
        .to_owned();
    Event::ToolError(ToolError {
        call_id: invoke.call_id,
        tool_name: invoke.tool_name,
        tool_type: tau_proto::ToolType::Function,
        message: message.clone(),
        details: Some(details),
        display: Some(display(ToolDisplayStatus::Error, &message)),
        originator: tau_proto::PromptOriginator::User,
    })
}

fn error_envelope(command: Option<&str>, code: &str, message: &str) -> CborValue {
    cbor_map(vec![
        ("ok", CborValue::Bool(false)),
        (
            "command",
            command
                .map(|command| CborValue::Text(command.to_owned()))
                .unwrap_or(CborValue::Null),
        ),
        ("error", structured_error(code, message)),
    ])
}

fn structured_error(code: &str, message: &str) -> CborValue {
    cbor_map(vec![
        ("code", CborValue::Text(code.to_owned())),
        ("message", CborValue::Text(message.to_owned())),
        ("details", CborValue::Map(Vec::new())),
    ])
}

fn cbor_map(entries: Vec<(&str, CborValue)>) -> CborValue {
    CborValue::Map(
        entries
            .into_iter()
            .map(|(key, value)| (CborValue::Text(key.to_owned()), value))
            .collect(),
    )
}

fn cbor_text_field<'a>(value: &'a CborValue, field: &str) -> Option<&'a str> {
    let CborValue::Map(entries) = value else {
        return None;
    };
    entries.iter().find_map(|(key, value)| match (key, value) {
        (CborValue::Text(key), CborValue::Text(value)) if key == field => Some(value.as_str()),
        _ => None,
    })
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

fn display(status: ToolDisplayStatus, status_text: &str) -> ToolDisplay {
    ToolDisplay {
        status,
        status_text: status_text.to_owned(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests;
