use std::collections::BTreeMap;

use tau_proto::{CborValue, Event, SecretValue, ToolError, ToolResult, ToolStarted};

use super::actions;
use super::config::{
    CalendarExtensionConfig, ValidatedAccount, ValidatedBackendConfig, ValidatedConfig,
};
use super::ics_feed::{
    IcsEvent, IcsFeedBackend, TimeRange, default_calendar_id, parse_rfc3339_bound,
};
use super::tool::{CalendarArgs, CalendarCommand, ToolInvocation};

const LIST_ACCOUNTS_FORMAT: &str =
    "format: id flags backend default_calendar timezone display_name";
const LIST_CALENDARS_FORMAT: &str = "format: account calendar flags backend display_name";
const LIST_EVENTS_FORMAT: &str = "format: account calendar event_id start end flags status summary";
const FREE_BUSY_FORMAT: &str = "format: account calendar event_id start end flags";
const DEFAULT_EVENT_LIMIT: u32 = 50;
const MAX_EVENT_LIMIT: u32 = 100;

/// Runtime state for the calendar module.
pub struct RuntimeState {
    config_state: ConfigState,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            config_state: ConfigState::Unconfigured,
        }
    }
}

enum ConfigState {
    Unconfigured,
    Configured(Engine),
    Rejected { reason: String },
}

struct Engine {
    config: ValidatedConfig,
    ics_feed: IcsFeedBackend,
}

impl RuntimeState {
    /// Configure the calendar module from an already-decoded calendar config.
    pub fn configure_with_config(
        &mut self,
        cfg: CalendarExtensionConfig,
        secrets: BTreeMap<String, SecretValue>,
    ) -> Result<(), String> {
        match cfg.validate() {
            Ok(config) => {
                self.config_state = ConfigState::Configured(Engine {
                    config,
                    ics_feed: IcsFeedBackend::new(secrets),
                });
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

    /// Dispatch a model-visible `calendar` tool invocation.
    pub fn dispatch(&mut self, invoke: ToolStarted) -> Event {
        let result = match &self.config_state {
            ConfigState::Configured(engine) => engine.dispatch(&invoke.arguments),
            ConfigState::Unconfigured => Err("calendar module has not been configured".to_owned()),
            ConfigState::Rejected { reason } => Err(format!(
                "calendar module configuration was rejected: {reason}"
            )),
        };
        match result {
            Ok(text) => tool_result(invoke, text),
            Err(message) => tool_error(invoke, message),
        }
    }

    /// Dispatch a user `/calendar` action invocation.
    pub fn dispatch_action(&mut self, invoke: tau_proto::ActionInvoke) -> Event {
        actions::dispatch_action(invoke)
    }
}

impl Engine {
    fn dispatch(&self, arguments: &CborValue) -> Result<String, String> {
        let invocation: ToolInvocation = arguments
            .deserialized()
            .map_err(|error| format!("invalid calendar tool arguments: {error}"))?;
        match invocation.command {
            CalendarCommand::ListAccounts => Ok(self.list_accounts()),
            CalendarCommand::ListCalendars => self.list_calendars(&invocation.args),
            CalendarCommand::ListEvents => self.list_events(&invocation.args),
            CalendarCommand::ReadEvent => self.read_event(&invocation.args),
            CalendarCommand::FreeBusy => self.free_busy(&invocation.args),
            CalendarCommand::CreateEvent
            | CalendarCommand::UpdateEvent
            | CalendarCommand::DeleteEvent
            | CalendarCommand::RespondInvite => {
                invocation.args.note_reserved_write_fields();
                Err(format!(
                    "calendar command `{}` is not available for read-only backends yet",
                    command_name(invocation.command)
                ))
            }
        }
    }

    fn list_accounts(&self) -> String {
        let mut lines = vec![LIST_ACCOUNTS_FORMAT.to_owned()];
        if !self.config.enable {
            return lines.join("\n");
        }
        for account_id in &self.config.account_order {
            let Some(account) = self.config.accounts.get(account_id) else {
                continue;
            };
            if !account.enable {
                continue;
            }
            let default_calendar = account.default_calendar.as_deref().unwrap_or("-");
            let timezone = account.timezone.as_deref().unwrap_or("-");
            let display_name = account.display_name.as_deref().unwrap_or("-");
            lines.push(format!(
                "{} {} {} {} {} {}",
                safe_field(&account.id),
                "enabled",
                safe_field(account.backend_kind()),
                safe_field(default_calendar),
                safe_field(timezone),
                safe_field(display_name)
            ));
        }
        lines.join("\n")
    }

    fn list_calendars(&self, args: &CalendarArgs) -> Result<String, String> {
        let mut lines = vec![LIST_CALENDARS_FORMAT.to_owned()];
        if !self.config.enable {
            return Ok(lines.join("\n"));
        }
        let accounts = self.accounts_for_read(args.account.as_deref())?;
        for account in accounts {
            if let Some(ValidatedBackendConfig::IcsFeed { .. }) = &account.backend {
                for calendar in self.ics_feed.list_calendars(account) {
                    let flags = if calendar.read_only {
                        "read_only"
                    } else {
                        "writable"
                    };
                    lines.push(format!(
                        "{} {} {} {} {}",
                        safe_field(&account.id),
                        safe_field(&calendar.id),
                        flags,
                        safe_field(account.backend_kind()),
                        safe_field(&calendar.display_name)
                    ));
                }
            }
        }
        Ok(lines.join("\n"))
    }

    fn list_events(&self, args: &CalendarArgs) -> Result<String, String> {
        let limit = normalized_limit(args.limit)?;
        let range = parse_range(args)?;
        let account = self.single_account(args.account.as_deref())?;
        let calendar = self.calendar_arg(account, args.calendar.as_deref())?;
        let events = self.events_for_account(account, calendar, range, limit)?;
        let mut lines = vec![LIST_EVENTS_FORMAT.to_owned()];
        for event in events {
            lines.push(format_event_line(account, calendar, &event));
        }
        Ok(lines.join("\n"))
    }

    fn read_event(&self, args: &CalendarArgs) -> Result<String, String> {
        let event_id = required_arg(args.event_id.as_deref(), "event_id")?;
        let account = self.single_account(args.account.as_deref())?;
        let calendar = self.calendar_arg(account, args.calendar.as_deref())?;
        let event = match &account.backend {
            Some(ValidatedBackendConfig::IcsFeed { .. }) => {
                self.ics_feed.read_event(account, calendar, event_id)?
            }
            Some(_) | None => {
                return Err(format!(
                    "calendar account `{}` backend `{}` does not support read_event yet",
                    account.id,
                    account.backend_kind()
                ));
            }
        };
        Ok(format_event_detail(account, calendar, &event))
    }

    fn free_busy(&self, args: &CalendarArgs) -> Result<String, String> {
        let limit = normalized_limit(args.limit)?;
        let range = parse_range(args)?;
        let account = self.single_account(args.account.as_deref())?;
        let calendar = self.calendar_arg(account, args.calendar.as_deref())?;
        let events = self.events_for_account(account, calendar, range, limit)?;
        let mut lines = vec![FREE_BUSY_FORMAT.to_owned()];
        for event in events {
            lines.push(format!(
                "{} {} {} {} {} {}",
                safe_field(&account.id),
                safe_field(calendar),
                safe_field(&event.id),
                safe_field(&event.start),
                safe_field(&event.end),
                event_flags(&event)
            ));
        }
        Ok(lines.join("\n"))
    }

    fn events_for_account(
        &self,
        account: &ValidatedAccount,
        calendar: &str,
        range: TimeRange,
        limit: usize,
    ) -> Result<Vec<IcsEvent>, String> {
        match &account.backend {
            Some(ValidatedBackendConfig::IcsFeed { .. }) => {
                self.ics_feed.list_events(account, calendar, range, limit)
            }
            Some(_) | None => Err(format!(
                "calendar account `{}` backend `{}` does not support event reads yet",
                account.id,
                account.backend_kind()
            )),
        }
    }

    fn accounts_for_read(&self, account: Option<&str>) -> Result<Vec<&ValidatedAccount>, String> {
        if let Some(account_id) = account {
            return Ok(vec![self.account_by_id(account_id)?]);
        }
        Ok(self
            .config
            .account_order
            .iter()
            .filter_map(|id| self.config.accounts.get(id))
            .filter(|account| account.enable)
            .collect())
    }

    fn single_account(&self, account: Option<&str>) -> Result<&ValidatedAccount, String> {
        if let Some(account_id) = account {
            return self.account_by_id(account_id);
        }
        let mut accounts = self.accounts_for_read(None)?.into_iter();
        let Some(first) = accounts.next() else {
            return Err("no enabled calendar accounts are configured".to_owned());
        };
        if accounts.next().is_some() {
            return Err(
                "account is required when multiple calendar accounts are enabled".to_owned(),
            );
        }
        Ok(first)
    }

    fn account_by_id(&self, account_id: &str) -> Result<&ValidatedAccount, String> {
        let account = self
            .config
            .accounts
            .get(account_id)
            .ok_or_else(|| format!("unknown calendar account `{account_id}`"))?;
        if !self.config.enable {
            return Err("calendar module is disabled".to_owned());
        }
        if !account.enable {
            return Err(format!("calendar account `{account_id}` is disabled"));
        }
        Ok(account)
    }

    fn calendar_arg<'a>(
        &self,
        account: &'a ValidatedAccount,
        calendar: Option<&'a str>,
    ) -> Result<&'a str, String> {
        if let Some(calendar) = calendar {
            return Ok(calendar);
        }
        let Some(calendar) = default_calendar_id(account) else {
            return Err(format!(
                "calendar is required for account `{}` because no default calendar is configured",
                account.id
            ));
        };
        Ok(calendar)
    }
}

fn parse_range(args: &CalendarArgs) -> Result<TimeRange, String> {
    Ok(TimeRange {
        min: parse_rfc3339_bound(args.time_min.as_deref(), "time_min")?,
        max: parse_rfc3339_bound(args.time_max.as_deref(), "time_max")?,
    })
}

fn normalized_limit(limit: Option<u32>) -> Result<usize, String> {
    let limit = limit.unwrap_or(DEFAULT_EVENT_LIMIT);
    if limit == 0 {
        return Err("limit must be a positive integer".to_owned());
    }
    let capped = if MAX_EVENT_LIMIT < limit {
        MAX_EVENT_LIMIT
    } else {
        limit
    };
    Ok(capped as usize)
}

fn required_arg<'a>(value: Option<&'a str>, name: &str) -> Result<&'a str, String> {
    match value {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(format!("{name} is required")),
    }
}

fn format_event_line(account: &ValidatedAccount, calendar: &str, event: &IcsEvent) -> String {
    let status = event.status.as_deref().unwrap_or("-");
    format!(
        "{} {} {} {} {} {} {} {}",
        safe_field(&account.id),
        safe_field(calendar),
        safe_field(&event.id),
        safe_field(&event.start),
        safe_field(&event.end),
        event_flags(event),
        safe_field(status),
        safe_field(&event.summary)
    )
}

fn format_event_detail(account: &ValidatedAccount, calendar: &str, event: &IcsEvent) -> String {
    let mut lines = vec![
        "format: key value".to_owned(),
        format!("account {}", safe_field(&account.id)),
        format!("calendar {}", safe_field(calendar)),
        format!("event_id {}", safe_field(&event.id)),
        format!("uid {}", safe_field(&event.uid)),
        format!("start {}", safe_field(&event.start)),
        format!("end {}", safe_field(&event.end)),
        format!("flags {}", event_flags(event)),
        format!("summary {}", safe_field(&event.summary)),
    ];
    if let Some(status) = &event.status {
        lines.push(format!("status {}", safe_field(status)));
    }
    if let Some(location) = &event.location {
        lines.push(format!("location {}", safe_field(location)));
    }
    if let Some(organizer) = &event.organizer {
        lines.push(format!("organizer {}", safe_field(organizer)));
    }
    if !event.attendees.is_empty() {
        lines.push(format!(
            "attendees {}",
            safe_field(&event.attendees.join(","))
        ));
    }
    if let Some(description) = &event.description {
        lines.push(format!("description {}", safe_multiline(description)));
    }
    lines.join("\n")
}

fn event_flags(event: &IcsEvent) -> String {
    let mut flags = vec!["read_only"];
    if event.recurring {
        flags.push("recurring_unexpanded");
    }
    if event.time_unparsed {
        flags.push("time_unparsed");
    }
    flags.join(",")
}

fn command_name(command: CalendarCommand) -> &'static str {
    match command {
        CalendarCommand::ListAccounts => "list_accounts",
        CalendarCommand::ListCalendars => "list_calendars",
        CalendarCommand::ListEvents => "list_events",
        CalendarCommand::ReadEvent => "read_event",
        CalendarCommand::FreeBusy => "free_busy",
        CalendarCommand::CreateEvent => "create_event",
        CalendarCommand::UpdateEvent => "update_event",
        CalendarCommand::DeleteEvent => "delete_event",
        CalendarCommand::RespondInvite => "respond_invite",
    }
}

fn tool_result(invoke: ToolStarted, result: String) -> Event {
    Event::ToolResult(ToolResult {
        call_id: invoke.call_id,
        tool_name: invoke.tool_name,
        tool_type: tau_proto::ToolType::Function,
        result: CborValue::Text(result),
        kind: tau_proto::ToolResultKind::Final,
        display: None,
        originator: tau_proto::PromptOriginator::User,
    })
}

fn tool_error(invoke: ToolStarted, message: String) -> Event {
    Event::ToolError(ToolError {
        call_id: invoke.call_id,
        tool_name: invoke.tool_name,
        tool_type: tau_proto::ToolType::Function,
        message,
        details: None,
        display: None,
        originator: tau_proto::PromptOriginator::User,
    })
}

fn safe_field(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("_")
}

fn safe_multiline(value: &str) -> String {
    value
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::config::{CalendarAccountConfig, CalendarBackendConfig};

    #[test]
    fn list_accounts_reports_enabled_configured_accounts() {
        let cfg = CalendarExtensionConfig {
            enable: true,
            accounts: vec![CalendarAccountConfig {
                id: "work".to_owned(),
                enable: true,
                display_name: Some("Work Calendar".to_owned()),
                backend: Some(CalendarBackendConfig::Google {
                    oauth_profile: Some("work".to_owned()),
                }),
                calendars: Default::default(),
                timezone: Some("UTC".to_owned()),
            }],
        };
        let config = cfg.validate().expect("valid calendar config");
        let engine = Engine {
            config,
            ics_feed: IcsFeedBackend::new(BTreeMap::new()),
        };

        assert_eq!(
            engine.list_accounts(),
            "format: id flags backend default_calendar timezone display_name\nwork enabled google - UTC Work_Calendar"
        );
    }

    #[test]
    fn duplicate_account_ids_are_rejected() {
        let cfg = CalendarExtensionConfig {
            enable: true,
            accounts: vec![
                CalendarAccountConfig {
                    id: "work".to_owned(),
                    ..Default::default()
                },
                CalendarAccountConfig {
                    id: "work".to_owned(),
                    ..Default::default()
                },
            ],
        };

        let err = match cfg.validate() {
            Ok(_) => panic!("duplicate ids should fail"),
            Err(err) => err,
        };
        assert!(err.contains("duplicate calendar account id"), "{err}");
    }

    #[test]
    fn ics_feed_requires_exactly_one_url_source() {
        let cfg = CalendarExtensionConfig {
            enable: true,
            accounts: vec![CalendarAccountConfig {
                id: "feed".to_owned(),
                backend: Some(CalendarBackendConfig::IcsFeed {
                    url_secret: None,
                    url: None,
                }),
                ..Default::default()
            }],
        };

        let err = match cfg.validate() {
            Ok(_) => panic!("missing feed source should fail"),
            Err(err) => err,
        };
        assert!(err.contains("requires exactly one"), "{err}");
    }
}
