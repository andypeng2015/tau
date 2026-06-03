use serde::Deserialize;
use tau_proto::{CborValue, PromptFragment, PromptPriority, ToolSpec};

use super::{TOOL_NAME, TOOL_PREFIX};

/// Parsed calendar tool invocation.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolInvocation {
    /// Calendar command to run.
    pub(crate) command: CalendarCommand,
    /// Raw command arguments, parsed into command-specific structs after the
    /// command is known.
    #[serde(default)]
    pub(crate) args: Option<CborValue>,
}

/// Calendar command names accepted by the model-visible tool.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CalendarCommand {
    /// List configured calendars.
    ListCalendars,
    /// List events in a bounded time range.
    ListEvents,
    /// Read one event by backend id.
    ReadEvent,
    /// Return busy blocks without event details.
    FreeBusy,
    /// Create a new event.
    CreateEvent,
    /// Update an existing event.
    UpdateEvent,
    /// Delete or cancel an event.
    DeleteEvent,
    /// Accept, tentatively accept, or decline an invitation.
    RespondInvite,
}

/// Arguments for listing calendars.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ListCalendarsArgs {}

/// Arguments for bounded calendar range reads.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct CalendarRangeArgs {
    /// Calendar id returned by calendar_list_calendars.
    pub(crate) calendar: Option<String>,
    /// Inclusive lower RFC3339 time bound.
    pub(crate) start: Option<String>,
    /// Exclusive upper RFC3339 time bound.
    pub(crate) end: Option<String>,
    /// Maximum rows to return.
    pub(crate) limit: Option<u32>,
    /// Pagination cursor.
    pub(crate) cursor: Option<String>,
    /// Case-insensitive substring filter for visible event summaries.
    pub(crate) title: Option<String>,
}

/// Arguments for reading one event by backend id.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ReadEventArgs {
    /// Calendar id returned by calendar_list_calendars.
    pub(crate) calendar: Option<String>,
    /// Backend event id.
    pub(crate) event_id: Option<String>,
}

/// Arguments for creating an event.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct CreateEventArgs {
    /// Calendar id returned by calendar_list_calendars.
    pub(crate) calendar: Option<String>,
    /// Event title.
    pub(crate) title: Option<String>,
    /// Event description.
    pub(crate) description: Option<String>,
    /// Event location.
    pub(crate) location: Option<String>,
    /// Event start as RFC3339 date-time or all-day date.
    pub(crate) start: Option<String>,
    /// Event end as RFC3339 date-time or all-day exclusive date.
    pub(crate) end: Option<String>,
    /// Attendee email addresses.
    pub(crate) attendees: Option<Vec<String>>,
}

/// Arguments for updating an existing event.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct UpdateEventArgs {
    /// Calendar id returned by calendar_list_calendars.
    pub(crate) calendar: Option<String>,
    /// Backend event id.
    pub(crate) event_id: Option<String>,
    /// Event title.
    pub(crate) title: Option<String>,
    /// Event description.
    pub(crate) description: Option<String>,
    /// Event location.
    pub(crate) location: Option<String>,
    /// Event start as RFC3339 date-time or all-day date.
    pub(crate) start: Option<String>,
    /// Event end as RFC3339 date-time or all-day exclusive date.
    pub(crate) end: Option<String>,
    /// Attendee email addresses.
    pub(crate) attendees: Option<Vec<String>>,
}

/// Arguments for deleting an existing event.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct DeleteEventArgs {
    /// Calendar id returned by calendar_list_calendars.
    pub(crate) calendar: Option<String>,
    /// Backend event id.
    pub(crate) event_id: Option<String>,
}

/// Arguments for responding to an invite.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct RespondInviteArgs {
    /// Calendar id returned by calendar_list_calendars.
    pub(crate) calendar: Option<String>,
    /// Backend event id.
    pub(crate) event_id: Option<String>,
    /// Invitation response: accepted, tentative, or declined.
    pub(crate) response: Option<String>,
}

const CALENDAR_TOOL_COMMANDS: &[(&str, &str)] = &[
    ("list_calendars", "list_calendars"),
    ("search", "list_events"),
    ("get", "read_event"),
    ("free_busy", "free_busy"),
    ("create", "create_event"),
    ("update", "update_event"),
    ("delete", "delete_event"),
    ("respond", "respond_invite"),
];

/// Return the model-visible split calendar tool specifications.
pub fn calendar_tool_specs() -> Vec<ToolSpec> {
    CALENDAR_TOOL_COMMANDS
        .iter()
        .map(|(tool_name, command)| calendar_command_tool_spec(tool_name, command))
        .collect()
}

/// Return the legacy model-visible calendar envelope tool specification.
pub fn calendar_tool_spec() -> ToolSpec {
    calendar_envelope_tool_spec(TOOL_NAME)
}

fn calendar_command_tool_spec(tool_name: &str, command: &str) -> ToolSpec {
    let mut spec = calendar_envelope_tool_spec(&format!("{TOOL_PREFIX}{tool_name}"));
    spec.description = Some(calendar_tool_description(tool_name, command).to_owned());
    spec.parameters = Some(calendar_command_parameters(command));
    spec
}

fn calendar_envelope_tool_spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: tau_proto::ToolName::new(name),
        model_visible_name: None,
        description: Some("Controlled calendar access. Calendar ids are opaque values returned by calendar_list_calendars. For event ranges, start is optional and defaults to midnight 2 days before the current date; omitted end defaults to 7 days after start. Time values may be RFC3339 with offset, YYYY-MM-DD all-day dates, natural expressions like today/tomorrow/next week, or local YYYY-MM-DDTHH:MM:SS interpreted in the configured or system timezone. Existing event writes require event_id; ETags are handled internally.".to_owned()),
        tool_type: tau_proto::ToolType::Function,
        parameters: Some(serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "enum": CALENDAR_TOOL_COMMANDS.iter().map(|(_, command)| command).collect::<Vec<_>>(),
                    "description": "Calendar operation to perform."
                },
                "args": {
                    "type": "object",
                    "description": "Command-specific arguments. `calendar` is an id returned by calendar_list_calendars and may be omitted when there is only one configured target.",
                    "properties": calendar_all_properties(),
                    "additionalProperties": false
                }
            },
            "required": ["command"],
            "additionalProperties": false
        })),
        format: None,
        enabled_by_default: false,
        background_support: None,
    }
}

fn calendar_command_parameters(command: &str) -> serde_json::Value {
    let schema = serde_json::json!({        "type": "object",
        "description": calendar_args_description(command),
        "properties": calendar_command_properties(command),
        "required": calendar_required(command),
        "additionalProperties": false
    });
    schema
}

fn calendar_all_properties() -> serde_json::Value {
    serde_json::json!({
        "calendar": {"type": "string", "description": "Calendar id from calendar_list_calendars."},
        "event_id": {"type": "string", "description": "Backend event id."},
        "limit": {"type": "integer", "minimum": 1, "maximum": 100},
        "cursor": {"type": "string", "description": "Cursor returned as next_cursor by calendar_search or calendar_free_busy."},
        "title": {"type": "string", "description": "Optional case-insensitive substring filter for event summaries."},
        "description": {"type": "string"},
        "location": {"type": "string"},
        "start": {"type": "string", "description": "Range or event start. Use RFC3339 with offset, YYYY-MM-DD, natural expressions like today/tomorrow/next week, or local YYYY-MM-DDTHH:MM:SS."},
        "end": {"type": "string", "description": "Range or event end. For calendar_create, omitted end defaults from start."},
        "attendees": {"type": "array", "items": {"type": "string"}},
        "response": {"type": "string", "enum": ["accepted", "tentative", "declined"]},
        "field": {"type": "string", "enum": ["title", "description", "location", "start", "attendees"], "description": "Event field to update. Use start to update event timing; end-only updates are not exposed because they are ambiguous."},
        "new_value": {"type": "string", "description": "New value for the selected field. For attendees, use comma-separated email addresses."}
    })
}

fn calendar_command_properties(command: &str) -> serde_json::Value {
    match command {
        "list_calendars" => serde_json::json!({}),
        "list_events" => {
            pick_calendar_properties(&["calendar", "start", "end", "limit", "cursor", "title"])
        }
        "free_busy" => pick_calendar_properties(&["calendar", "start", "end", "limit", "cursor"]),
        "read_event" | "delete_event" => pick_calendar_properties(&["calendar", "event_id"]),
        "create_event" => pick_calendar_properties(&[
            "calendar",
            "title",
            "description",
            "location",
            "start",
            "end",
            "attendees",
        ]),
        "update_event" => pick_calendar_properties(&["calendar", "event_id", "field", "new_value"]),
        "respond_invite" => pick_calendar_properties(&["calendar", "event_id", "response"]),
        _ => serde_json::json!({}),
    }
}

fn pick_calendar_properties(names: &[&str]) -> serde_json::Value {
    let all = calendar_all_properties();
    let mut out = serde_json::Map::new();
    for name in names {
        if let Some(value) = all.get(name) {
            out.insert((*name).to_owned(), value.clone());
        }
    }
    serde_json::Value::Object(out)
}

fn calendar_required(command: &str) -> serde_json::Value {
    match command {
        "read_event" | "delete_event" => serde_json::json!(["event_id"]),
        "create_event" => serde_json::json!(["title", "start"]),
        "update_event" => serde_json::json!(["event_id", "field", "new_value"]),
        "respond_invite" => serde_json::json!(["event_id", "response"]),
        _ => serde_json::json!([]),
    }
}

fn calendar_tool_description(tool_name: &str, command: &str) -> &'static str {
    match (tool_name, command) {
        ("list_calendars", _) => {
            "List configured calendars and return calendar ids for other calendar tools."
        }
        ("search", _) => {
            "Search/list visible calendar events in a bounded time range, with optional title filter and pagination."
        }
        ("get", _) => "Get one calendar event by calendar id and backend event id.",
        ("free_busy", _) => "Return busy blocks without event details in a bounded time range.",
        ("create", _) => "Create a new calendar event. Writes may require user approval.",
        ("update", _) => {
            "Update one field of an existing calendar event by event id. Writes may require user approval."
        }
        ("delete", _) => {
            "Delete or cancel an existing calendar event by event id. Writes may require user approval."
        }
        ("respond", _) => {
            "Accept, tentatively accept, or decline a calendar invitation. Writes may require user approval."
        }
        _ => "Run a calendar command.",
    }
}

fn calendar_args_description(command: &str) -> &'static str {
    match command {
        "list_calendars" => "No arguments.",
        "list_events" | "free_busy" => {
            "Range arguments. Omit calendar only when there is one configured target."
        }
        _ => "Calendar event arguments. Omit calendar only when there is one configured target.",
    }
}

/// Return the prompt fragment that teaches the model calendar tool policy.
pub fn calendar_prompt_fragment() -> PromptFragment {
    PromptFragment::new(
        "calendar.instructions",
        PromptPriority::new(120),
        include_str!("prompts/calendar_instructions.md"),
    )
}

#[cfg(test)]
mod tests;
