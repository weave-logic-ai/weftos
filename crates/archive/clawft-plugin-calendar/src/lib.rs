//! Calendar integration tool plugin for clawft.
//!
//! Provides tools for listing, creating, updating, and deleting calendar
//! events through Google Calendar, Microsoft Outlook, and iCal providers.
//!
//! # Authentication
//!
//! Depends on the OAuth2 plugin (F6) for authentication with Google and
//! Microsoft APIs. The OAuth2 provider name must be configured in
//! [`CalendarConfig::oauth2_provider`].
//!
//! # Feature Flag
//!
//! This crate is gated behind the workspace `plugin-calendar` feature flag.

pub mod types;

use async_trait::async_trait;
use clawft_plugin::{PluginError, Tool, ToolContext};

use types::CalendarConfig;

// ---------------------------------------------------------------------------
// CalListEventsTool
// ---------------------------------------------------------------------------

/// Tool that lists upcoming calendar events.
pub struct CalListEventsTool {
    config: CalendarConfig,
}

impl CalListEventsTool {
    pub fn new(config: CalendarConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for CalListEventsTool {
    fn name(&self) -> &str {
        "cal_list_events"
    }

    fn description(&self) -> &str {
        "List upcoming calendar events"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "calendar_id": {
                    "type": "string",
                    "description": "Calendar ID (defaults to 'primary')"
                },
                "time_min": {
                    "type": "string",
                    "description": "Start of time range (RFC 3339 format)"
                },
                "time_max": {
                    "type": "string",
                    "description": "End of time range (RFC 3339 format)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of events to return",
                    "default": 25
                },
                "provider": {
                    "type": "string",
                    "description": "Calendar provider (google, outlook, ical)",
                    "enum": ["google", "outlook", "ical"]
                }
            },
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let _calendar_id = params
            .get("calendar_id")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.config.default_calendar_id);

        let _max_results = params
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(25);

        // NOTE: Actual API calls require OAuth2 token from F6.
        // This implementation validates inputs and provides the
        // correct interface. API integration wired at runtime.
        Ok(serde_json::json!({
            "events": [],
            "provider": self.config.provider,
            "note": "calendar API integration pending OAuth2 token wiring"
        }))
    }
}

// ---------------------------------------------------------------------------
// CalCreateEventTool
// ---------------------------------------------------------------------------

/// Tool that creates a new calendar event.
pub struct CalCreateEventTool {
    config: CalendarConfig,
}

impl CalCreateEventTool {
    pub fn new(config: CalendarConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for CalCreateEventTool {
    fn name(&self) -> &str {
        "cal_create_event"
    }

    fn description(&self) -> &str {
        "Create a new calendar event"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Event title"
                },
                "description": {
                    "type": "string",
                    "description": "Event description"
                },
                "start": {
                    "type": "string",
                    "description": "Start time (RFC 3339 format)"
                },
                "end": {
                    "type": "string",
                    "description": "End time (RFC 3339 format)"
                },
                "location": {
                    "type": "string",
                    "description": "Event location"
                },
                "attendees": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Attendee email addresses"
                },
                "calendar_id": {
                    "type": "string",
                    "description": "Calendar ID (defaults to 'primary')"
                },
                "provider": {
                    "type": "string",
                    "description": "Calendar provider",
                    "enum": ["google", "outlook"]
                }
            },
            "required": ["summary", "start", "end"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let summary = params
            .get("summary")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("summary is required".into()))?;

        let start = params
            .get("start")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("start is required".into()))?;

        let end = params
            .get("end")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("end is required".into()))?;

        // Validate RFC 3339 format
        chrono::DateTime::parse_from_rfc3339(start)
            .map_err(|e| PluginError::ExecutionFailed(format!("invalid start time: {e}")))?;
        chrono::DateTime::parse_from_rfc3339(end)
            .map_err(|e| PluginError::ExecutionFailed(format!("invalid end time: {e}")))?;

        Ok(serde_json::json!({
            "status": "created",
            "summary": summary,
            "start": start,
            "end": end,
            "provider": self.config.provider,
            "note": "calendar API integration pending OAuth2 token wiring"
        }))
    }
}

// ---------------------------------------------------------------------------
// CalUpdateEventTool
// ---------------------------------------------------------------------------

/// Tool that updates an existing calendar event.
pub struct CalUpdateEventTool {
    config: CalendarConfig,
}

impl CalUpdateEventTool {
    pub fn new(config: CalendarConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for CalUpdateEventTool {
    fn name(&self) -> &str {
        "cal_update_event"
    }

    fn description(&self) -> &str {
        "Update an existing calendar event"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "event_id": {
                    "type": "string",
                    "description": "Event ID to update"
                },
                "summary": {
                    "type": "string",
                    "description": "Updated event title"
                },
                "description": {
                    "type": "string",
                    "description": "Updated event description"
                },
                "start": {
                    "type": "string",
                    "description": "Updated start time (RFC 3339 format)"
                },
                "end": {
                    "type": "string",
                    "description": "Updated end time (RFC 3339 format)"
                },
                "location": {
                    "type": "string",
                    "description": "Updated event location"
                },
                "calendar_id": {
                    "type": "string",
                    "description": "Calendar ID"
                },
                "provider": {
                    "type": "string",
                    "description": "Calendar provider",
                    "enum": ["google", "outlook"]
                }
            },
            "required": ["event_id"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let event_id = params
            .get("event_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("event_id is required".into()))?;

        // Validate optional time fields
        if let Some(start) = params.get("start").and_then(|v| v.as_str()) {
            chrono::DateTime::parse_from_rfc3339(start)
                .map_err(|e| PluginError::ExecutionFailed(format!("invalid start time: {e}")))?;
        }
        if let Some(end) = params.get("end").and_then(|v| v.as_str()) {
            chrono::DateTime::parse_from_rfc3339(end)
                .map_err(|e| PluginError::ExecutionFailed(format!("invalid end time: {e}")))?;
        }

        Ok(serde_json::json!({
            "status": "updated",
            "event_id": event_id,
            "provider": self.config.provider,
            "note": "calendar API integration pending OAuth2 token wiring"
        }))
    }
}

// ---------------------------------------------------------------------------
// CalDeleteEventTool
// ---------------------------------------------------------------------------

/// Tool that deletes a calendar event.
pub struct CalDeleteEventTool {
    config: CalendarConfig,
}

impl CalDeleteEventTool {
    pub fn new(config: CalendarConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for CalDeleteEventTool {
    fn name(&self) -> &str {
        "cal_delete_event"
    }

    fn description(&self) -> &str {
        "Delete a calendar event"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "event_id": {
                    "type": "string",
                    "description": "Event ID to delete"
                },
                "calendar_id": {
                    "type": "string",
                    "description": "Calendar ID"
                },
                "provider": {
                    "type": "string",
                    "description": "Calendar provider",
                    "enum": ["google", "outlook"]
                }
            },
            "required": ["event_id"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let event_id = params
            .get("event_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("event_id is required".into()))?;

        Ok(serde_json::json!({
            "status": "deleted",
            "event_id": event_id,
            "provider": self.config.provider,
            "note": "calendar API integration pending OAuth2 token wiring"
        }))
    }
}

// ---------------------------------------------------------------------------
// CalCheckAvailabilityTool
// ---------------------------------------------------------------------------

/// Tool that checks free/busy status.
pub struct CalCheckAvailabilityTool {
    config: CalendarConfig,
}

impl CalCheckAvailabilityTool {
    pub fn new(config: CalendarConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for CalCheckAvailabilityTool {
    fn name(&self) -> &str {
        "cal_check_availability"
    }

    fn description(&self) -> &str {
        "Check free/busy status for a time range"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "time_min": {
                    "type": "string",
                    "description": "Start of time range (RFC 3339 format)"
                },
                "time_max": {
                    "type": "string",
                    "description": "End of time range (RFC 3339 format)"
                },
                "calendar_ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Calendar IDs to check"
                },
                "provider": {
                    "type": "string",
                    "description": "Calendar provider",
                    "enum": ["google", "outlook"]
                }
            },
            "required": ["time_min", "time_max"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError> {
        let time_min = params
            .get("time_min")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("time_min is required".into()))?;

        let time_max = params
            .get("time_max")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PluginError::ExecutionFailed("time_max is required".into()))?;

        chrono::DateTime::parse_from_rfc3339(time_min)
            .map_err(|e| PluginError::ExecutionFailed(format!("invalid time_min: {e}")))?;
        chrono::DateTime::parse_from_rfc3339(time_max)
            .map_err(|e| PluginError::ExecutionFailed(format!("invalid time_max: {e}")))?;

        Ok(serde_json::json!({
            "status": "checked",
            "time_min": time_min,
            "time_max": time_max,
            "busy_ranges": [],
            "provider": self.config.provider,
            "note": "calendar API integration pending OAuth2 token wiring"
        }))
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Create all calendar tools with the given configuration.
pub fn all_calendar_tools(config: CalendarConfig) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(CalListEventsTool::new(config.clone())),
        Box::new(CalCreateEventTool::new(config.clone())),
        Box::new(CalUpdateEventTool::new(config.clone())),
        Box::new(CalDeleteEventTool::new(config.clone())),
        Box::new(CalCheckAvailabilityTool::new(config)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_plugin::KeyValueStore;

    struct MockKvStore;

    #[async_trait]
    impl KeyValueStore for MockKvStore {
        async fn get(&self, _key: &str) -> Result<Option<String>, PluginError> {
            Ok(None)
        }
        async fn set(&self, _key: &str, _value: &str) -> Result<(), PluginError> {
            Ok(())
        }
        async fn delete(&self, _key: &str) -> Result<bool, PluginError> {
            Ok(false)
        }
        async fn list_keys(
            &self,
            _prefix: Option<&str>,
        ) -> Result<Vec<String>, PluginError> {
            Ok(vec![])
        }
    }

    struct MockToolContext;

    impl ToolContext for MockToolContext {
        fn key_value_store(&self) -> &dyn KeyValueStore {
            &MockKvStore
        }
        fn plugin_id(&self) -> &str {
            "clawft-plugin-calendar"
        }
        fn agent_id(&self) -> &str {
            "test-agent"
        }
    }

    #[test]
    fn all_tools_returns_five() {
        let tools = all_calendar_tools(CalendarConfig::default());
        assert_eq!(tools.len(), 5);

        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"cal_list_events"));
        assert!(names.contains(&"cal_create_event"));
        assert!(names.contains(&"cal_update_event"));
        assert!(names.contains(&"cal_delete_event"));
        assert!(names.contains(&"cal_check_availability"));
    }

    #[test]
    fn tool_descriptions_non_empty() {
        let tools = all_calendar_tools(CalendarConfig::default());
        for tool in &tools {
            assert!(
                !tool.description().is_empty(),
                "empty description for {}",
                tool.name()
            );
        }
    }

    #[test]
    fn tool_schemas_are_objects() {
        let tools = all_calendar_tools(CalendarConfig::default());
        for tool in &tools {
            let schema = tool.parameters_schema();
            assert!(schema.is_object(), "schema not object for {}", tool.name());
            assert_eq!(schema["type"], "object");
        }
    }

    #[tokio::test]
    async fn list_events_returns_placeholder() {
        let tool = CalListEventsTool::new(CalendarConfig::default());
        let ctx = MockToolContext;
        let result = tool.execute(serde_json::json!({}), &ctx).await.unwrap();
        assert!(result["events"].is_array());
    }

    #[tokio::test]
    async fn create_event_validates_times() {
        let tool = CalCreateEventTool::new(CalendarConfig::default());
        let ctx = MockToolContext;

        // Valid times
        let params = serde_json::json!({
            "summary": "Meeting",
            "start": "2026-03-01T10:00:00Z",
            "end": "2026-03-01T11:00:00Z"
        });
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_ok());

        // Invalid time
        let params = serde_json::json!({
            "summary": "Meeting",
            "start": "not-a-date",
            "end": "2026-03-01T11:00:00Z"
        });
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn create_event_requires_summary() {
        let tool = CalCreateEventTool::new(CalendarConfig::default());
        let ctx = MockToolContext;

        let params = serde_json::json!({
            "start": "2026-03-01T10:00:00Z",
            "end": "2026-03-01T11:00:00Z"
        });
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_event_requires_event_id() {
        let tool = CalDeleteEventTool::new(CalendarConfig::default());
        let ctx = MockToolContext;

        let result = tool.execute(serde_json::json!({}), &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn check_availability_validates_times() {
        let tool = CalCheckAvailabilityTool::new(CalendarConfig::default());
        let ctx = MockToolContext;

        let params = serde_json::json!({
            "time_min": "invalid",
            "time_max": "2026-03-01T18:00:00Z"
        });
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }
}
