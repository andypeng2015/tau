use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

/// Top-level calendar module configuration.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CalendarExtensionConfig {
    /// Whether calendar access is enabled.
    pub enable: bool,
    /// Configured calendar accounts.
    pub accounts: Vec<CalendarAccountConfig>,
}

/// One configured calendar account.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CalendarAccountConfig {
    /// Stable account identifier used by tool commands.
    pub id: String,
    /// Per-account enable flag. Accounts are disabled unless explicitly
    /// enabled.
    pub enable: bool,
    /// Optional display name for user-facing account lists.
    pub display_name: Option<String>,
    /// Calendar backend configuration.
    pub backend: Option<CalendarBackendConfig>,
    /// Per-account calendar selection policy.
    pub calendars: CalendarSelectionConfig,
    /// Default IANA timezone for new events and date-only interpretation.
    pub timezone: Option<String>,
}

/// Backend-specific calendar account configuration.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CalendarBackendConfig {
    /// Generic read-only iCalendar feed.
    IcsFeed {
        /// Secret containing the feed URL.
        url_secret: Option<String>,
        /// Literal feed URL. Prefer `url_secret` for private feeds.
        url: Option<String>,
    },
    /// Native Google Calendar API backend.
    Google {
        /// Named OAuth profile holding Google tokens.
        oauth_profile: Option<String>,
    },
    /// Generic CalDAV backend.
    Caldav {
        /// CalDAV service URL.
        url: Option<String>,
        /// Login user name for Basic-style DAV servers.
        login: Option<String>,
        /// Secret containing a DAV password or app password.
        password_secret: Option<String>,
    },
}

/// Per-account calendar visibility configuration.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CalendarSelectionConfig {
    /// Default calendar id used when a safe command can omit `calendar`.
    pub default: Option<String>,
    /// Calendar ids or names the agent may see. Empty means none.
    pub allow: Vec<String>,
}

/// Validated calendar configuration.
pub struct ValidatedConfig {
    /// Whether calendar access is enabled.
    pub enable: bool,
    /// Accounts keyed by configured account ID.
    pub accounts: BTreeMap<String, ValidatedAccount>,
    /// Account IDs in configuration order for deterministic display.
    pub account_order: Vec<String>,
}

/// Validated calendar account configuration.
pub struct ValidatedAccount {
    /// Stable account identifier used by tool commands.
    pub id: String,
    /// Whether this account is enabled.
    pub enable: bool,
    /// Optional display name.
    pub display_name: Option<String>,
    /// Configured backend.
    pub backend: Option<ValidatedBackendConfig>,
    /// Default calendar id.
    pub default_calendar: Option<String>,
    /// Allowed calendar ids or names.
    pub allowed_calendars: Vec<String>,
    /// Default IANA timezone.
    pub timezone: Option<String>,
}

/// Validated backend-specific calendar account configuration.
pub enum ValidatedBackendConfig {
    /// Generic read-only iCalendar feed.
    IcsFeed {
        /// Secret containing the feed URL.
        url_secret: Option<String>,
        /// Literal feed URL.
        url: Option<String>,
    },
    /// Native Google Calendar API backend.
    Google {
        /// Named OAuth profile holding Google tokens.
        oauth_profile: Option<String>,
    },
    /// Generic CalDAV backend.
    Caldav {
        /// CalDAV service URL.
        url: Option<String>,
        /// Login user name for Basic-style DAV servers.
        login: Option<String>,
        /// Secret containing a DAV password or app password.
        password_secret: Option<String>,
    },
}

impl CalendarExtensionConfig {
    /// Validate this configuration and normalize account lookup structures.
    pub fn validate(self) -> Result<ValidatedConfig, String> {
        let mut ids = BTreeSet::new();
        let mut accounts = BTreeMap::new();
        let mut account_order = Vec::new();
        for account in self.accounts {
            if account.id.trim().is_empty() {
                return Err("calendar account id must not be empty".to_owned());
            }
            if !ids.insert(account.id.clone()) {
                return Err(format!("duplicate calendar account id `{}`", account.id));
            }
            validate_calendar_patterns(&account.calendars.allow)?;
            if let Some(default) = &account.calendars.default {
                validate_calendar_pattern(default)?;
            }
            let id = account.id.clone();
            account_order.push(id.clone());
            accounts.insert(id, ValidatedAccount::from_config(account)?);
        }
        Ok(ValidatedConfig {
            enable: self.enable,
            accounts,
            account_order,
        })
    }
}

impl ValidatedAccount {
    fn from_config(value: CalendarAccountConfig) -> Result<Self, String> {
        let backend = match value.backend {
            Some(CalendarBackendConfig::IcsFeed { url_secret, url }) => {
                validate_ics_feed_source(url_secret.as_deref(), url.as_deref())?;
                Some(ValidatedBackendConfig::IcsFeed { url_secret, url })
            }
            Some(CalendarBackendConfig::Google { oauth_profile }) => {
                Some(ValidatedBackendConfig::Google { oauth_profile })
            }
            Some(CalendarBackendConfig::Caldav {
                url,
                login,
                password_secret,
            }) => Some(ValidatedBackendConfig::Caldav {
                url,
                login,
                password_secret,
            }),
            None => None,
        };
        Ok(Self {
            id: value.id,
            enable: value.enable,
            display_name: value.display_name,
            backend,
            default_calendar: value.calendars.default,
            allowed_calendars: value.calendars.allow,
            timezone: value.timezone,
        })
    }

    /// Return the stable backend kind name.
    pub fn backend_kind(&self) -> &'static str {
        match &self.backend {
            Some(ValidatedBackendConfig::IcsFeed { .. }) => "ics_feed",
            Some(ValidatedBackendConfig::Google { .. }) => "google",
            Some(ValidatedBackendConfig::Caldav { .. }) => "caldav",
            None => "none",
        }
    }
}

fn validate_ics_feed_source(url_secret: Option<&str>, url: Option<&str>) -> Result<(), String> {
    match (url_secret, url) {
        (Some(secret), None) if secret.trim().is_empty() => {
            Err("ics_feed url_secret must not be empty".to_owned())
        }
        (None, Some(url)) if url.trim().is_empty() => {
            Err("ics_feed url must not be empty".to_owned())
        }
        (Some(_), None) | (None, Some(_)) => Ok(()),
        (None, None) => Err("ics_feed requires exactly one of url_secret or url".to_owned()),
        (Some(_), Some(_)) => Err("ics_feed accepts only one of url_secret or url".to_owned()),
    }
}

fn validate_calendar_patterns(patterns: &[String]) -> Result<(), String> {
    for pattern in patterns {
        validate_calendar_pattern(pattern)?;
    }
    Ok(())
}

fn validate_calendar_pattern(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("calendar id pattern must not be empty".to_owned());
    }
    if value.chars().any(|c| c.is_control()) {
        return Err("calendar id pattern must not contain control characters".to_owned());
    }
    Ok(())
}
