//! Email channel adapter factory.
//!
//! Creates [`EmailChannelAdapter`] instances from JSON configuration.

use std::sync::Arc;

use clawft_plugin::error::PluginError;
use clawft_plugin::traits::ChannelAdapter;

use super::channel::EmailChannelAdapter;
use super::types::EmailAdapterConfig;

/// Factory for creating [`EmailChannelAdapter`] instances from JSON.
///
/// Expected config shape matches [`EmailAdapterConfig`].
pub struct EmailChannelAdapterFactory;

impl EmailChannelAdapterFactory {
    /// Create an email channel adapter from a JSON config value.
    pub fn build(config: &serde_json::Value) -> Result<Arc<dyn ChannelAdapter>, PluginError> {
        let adapter_config: EmailAdapterConfig = serde_json::from_value(config.clone())
            .map_err(|e| PluginError::LoadFailed(format!("invalid email config: {e}")))?;

        // Validate required fields.
        if adapter_config.imap_host.is_empty() {
            return Err(PluginError::LoadFailed(
                "email config: imap_host is required".into(),
            ));
        }
        if adapter_config.email_address.is_empty() {
            return Err(PluginError::LoadFailed(
                "email config: email_address is required".into(),
            ));
        }
        if adapter_config.smtp_host.is_empty() {
            return Err(PluginError::LoadFailed(
                "email config: smtp_host is required".into(),
            ));
        }

        Ok(Arc::new(EmailChannelAdapter::new(adapter_config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_success() {
        let config = serde_json::json!({
            "imapHost": "imap.gmail.com",
            "smtpHost": "smtp.gmail.com",
            "emailAddress": "user@gmail.com",
            "auth": {
                "type": "password",
                "username": "user@gmail.com",
                "password": "secret"
            }
        });
        let adapter = EmailChannelAdapterFactory::build(&config);
        assert!(adapter.is_ok());
        let a = adapter.unwrap();
        assert_eq!(a.name(), "email");
    }

    #[test]
    fn build_missing_imap_host_fails() {
        let config = serde_json::json!({
            "smtpHost": "smtp.gmail.com",
            "emailAddress": "user@gmail.com"
        });
        let result = EmailChannelAdapterFactory::build(&config);
        match result {
            Err(e) => assert!(e.to_string().contains("imap_host")),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn build_missing_email_address_fails() {
        let config = serde_json::json!({
            "imapHost": "imap.gmail.com",
            "smtpHost": "smtp.gmail.com"
        });
        let result = EmailChannelAdapterFactory::build(&config);
        match result {
            Err(e) => assert!(e.to_string().contains("email_address")),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn build_missing_smtp_host_fails() {
        let config = serde_json::json!({
            "imapHost": "imap.gmail.com",
            "emailAddress": "user@gmail.com"
        });
        let result = EmailChannelAdapterFactory::build(&config);
        match result {
            Err(e) => assert!(e.to_string().contains("smtp_host")),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn build_with_oauth2() {
        let config = serde_json::json!({
            "imapHost": "imap.gmail.com",
            "smtpHost": "smtp.gmail.com",
            "emailAddress": "user@gmail.com",
            "auth": {
                "type": "oauth2",
                "client_id_env": "GMAIL_CLIENT_ID",
                "client_secret_env": "GMAIL_CLIENT_SECRET",
                "refresh_token_env": "GMAIL_REFRESH_TOKEN"
            }
        });
        let adapter = EmailChannelAdapterFactory::build(&config);
        assert!(adapter.is_ok());
    }

    #[test]
    fn build_with_allowed_senders() {
        let config = serde_json::json!({
            "imapHost": "imap.gmail.com",
            "smtpHost": "smtp.gmail.com",
            "emailAddress": "user@gmail.com",
            "allowedSenders": ["boss@company.com"]
        });
        let adapter = EmailChannelAdapterFactory::build(&config);
        assert!(adapter.is_ok());
    }

    #[test]
    fn build_invalid_json_fails() {
        let config = serde_json::json!("not an object");
        let result = EmailChannelAdapterFactory::build(&config);
        assert!(result.is_err());
    }
}
