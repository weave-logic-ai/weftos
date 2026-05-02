//! Types for calendar integration.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Supported calendar providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CalendarProvider {
    Google,
    Outlook,
    #[serde(rename = "ical")]
    ICal,
}

impl CalendarProvider {
    /// Parse a provider string.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "google" => Some(Self::Google),
            "outlook" | "microsoft" => Some(Self::Outlook),
            "ical" | "ics" => Some(Self::ICal),
            _ => None,
        }
    }

    /// Base API URL for the provider.
    pub fn api_base_url(&self) -> Option<&'static str> {
        match self {
            Self::Google => Some("https://www.googleapis.com/calendar/v3"),
            Self::Outlook => Some("https://graph.microsoft.com/v1.0/me"),
            Self::ICal => None, // Local file-based, no API
        }
    }
}

/// Configuration for the calendar plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarConfig {
    /// Default provider to use.
    #[serde(default = "default_provider")]
    pub provider: CalendarProvider,

    /// OAuth2 provider name to use for token lookup (from F6).
    /// Must match a configured provider in clawft-plugin-oauth2.
    #[serde(default)]
    pub oauth2_provider: Option<String>,

    /// Default calendar ID for Google Calendar.
    #[serde(default = "default_calendar_id")]
    pub default_calendar_id: String,
}

fn default_provider() -> CalendarProvider {
    CalendarProvider::Google
}

fn default_calendar_id() -> String {
    "primary".to_string()
}

impl Default for CalendarConfig {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            oauth2_provider: None,
            default_calendar_id: default_calendar_id(),
        }
    }
}

/// A calendar event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarEvent {
    /// Event ID (provider-specific).
    #[serde(default)]
    pub id: String,

    /// Event title/summary.
    pub summary: String,

    /// Event description/body.
    #[serde(default)]
    pub description: String,

    /// Start time.
    pub start: DateTime<Utc>,

    /// End time.
    pub end: DateTime<Utc>,

    /// Location (optional).
    #[serde(default)]
    pub location: String,

    /// Attendee email addresses.
    #[serde(default)]
    pub attendees: Vec<String>,

    /// Calendar ID this event belongs to.
    #[serde(default)]
    pub calendar_id: String,
}

/// Parameters for listing events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListEventsParams {
    /// Calendar ID (defaults to "primary" for Google).
    #[serde(default = "default_calendar_id")]
    pub calendar_id: String,

    /// Start of the time range.
    pub time_min: Option<DateTime<Utc>>,

    /// End of the time range.
    pub time_max: Option<DateTime<Utc>>,

    /// Maximum number of events to return.
    #[serde(default = "default_max_results")]
    pub max_results: u32,
}

fn default_max_results() -> u32 {
    25
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_parse() {
        assert_eq!(
            CalendarProvider::parse("google"),
            Some(CalendarProvider::Google)
        );
        assert_eq!(
            CalendarProvider::parse("outlook"),
            Some(CalendarProvider::Outlook)
        );
        assert_eq!(
            CalendarProvider::parse("microsoft"),
            Some(CalendarProvider::Outlook)
        );
        assert_eq!(
            CalendarProvider::parse("ical"),
            Some(CalendarProvider::ICal)
        );
        assert_eq!(CalendarProvider::parse("unknown"), None);
    }

    #[test]
    fn provider_api_urls() {
        assert!(CalendarProvider::Google.api_base_url().is_some());
        assert!(CalendarProvider::Outlook.api_base_url().is_some());
        assert!(CalendarProvider::ICal.api_base_url().is_none());
    }

    #[test]
    fn provider_serde() {
        let json = serde_json::to_string(&CalendarProvider::Google).unwrap();
        assert_eq!(json, r#""google""#);
        let restored: CalendarProvider = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, CalendarProvider::Google);
    }

    #[test]
    fn config_default() {
        let config = CalendarConfig::default();
        assert_eq!(config.provider, CalendarProvider::Google);
        assert!(config.oauth2_provider.is_none());
        assert_eq!(config.default_calendar_id, "primary");
    }

    #[test]
    fn event_serde_roundtrip() {
        let event = CalendarEvent {
            id: "evt-1".into(),
            summary: "Team standup".into(),
            description: "Daily standup meeting".into(),
            start: Utc::now(),
            end: Utc::now(),
            location: "Room 42".into(),
            attendees: vec!["alice@example.com".into()],
            calendar_id: "primary".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let restored: CalendarEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, "evt-1");
        assert_eq!(restored.summary, "Team standup");
    }
}
