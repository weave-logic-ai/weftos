//! [`DiscordChannelFactory`] -- creates Discord channels from JSON config.

use std::sync::Arc;

use clawft_types::config::DiscordConfig;
use clawft_types::error::ChannelError;

use crate::traits::{Channel, ChannelFactory};

use super::channel::DiscordChannel;

/// Factory for creating [`DiscordChannel`] instances from JSON configuration.
///
/// Expected config shape matches [`DiscordConfig`]:
///
/// ```json
/// {
///   "enabled": true,
///   "token": "Bot-Token-Here",
///   "allow_from": ["123456789"],
///   "gateway_url": "wss://gateway.discord.gg/?v=10&encoding=json",
///   "intents": 37377
/// }
/// ```
pub struct DiscordChannelFactory;

impl ChannelFactory for DiscordChannelFactory {
    fn channel_name(&self) -> &str {
        "discord"
    }

    fn build(&self, config: &serde_json::Value) -> Result<Arc<dyn Channel>, ChannelError> {
        let mut discord_config: DiscordConfig = serde_json::from_value(config.clone())
            .map_err(|e| ChannelError::Other(format!("invalid discord config: {e}")))?;

        // Resolve token: explicit value > token_env env var > error
        if discord_config.token.is_empty() {
            if let Some(ref env_var) = discord_config.token_env {
                match std::env::var(env_var) {
                    Ok(val) if !val.is_empty() => discord_config.token = val.into(),
                    _ => {
                        return Err(ChannelError::Other(format!(
                            "discord token_env '{env_var}' is not set or empty"
                        )));
                    }
                }
            } else {
                return Err(ChannelError::Other(
                    "missing 'token' (or 'token_env') in discord config".into(),
                ));
            }
        }

        Ok(Arc::new(DiscordChannel::new(discord_config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_channel_name() {
        let factory = DiscordChannelFactory;
        assert_eq!(factory.channel_name(), "discord");
    }

    #[test]
    fn factory_build_success() {
        let factory = DiscordChannelFactory;
        let config = serde_json::json!({
            "enabled": true,
            "token": "my-bot-token",
            "intents": 37377
        });
        let channel = factory.build(&config);
        assert!(channel.is_ok());
        let ch = channel.unwrap();
        assert_eq!(ch.name(), "discord");
    }

    #[test]
    fn factory_build_missing_token_errors() {
        let factory = DiscordChannelFactory;
        let config = serde_json::json!({
            "enabled": true
        });
        let result = factory.build(&config);
        match result {
            Err(ChannelError::Other(msg)) => {
                assert!(msg.contains("token"), "error should mention token: {msg}");
            }
            Err(other) => panic!("expected ChannelError::Other, got: {other:?}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    #[test]
    fn factory_build_empty_config_errors() {
        let factory = DiscordChannelFactory;
        let config = serde_json::json!({});
        let result = factory.build(&config);
        assert!(result.is_err());
    }

    #[test]
    fn factory_build_with_allow_from() {
        let factory = DiscordChannelFactory;
        let config = serde_json::json!({
            "token": "my-bot-token",
            "allow_from": ["123", "456"]
        });
        let channel = factory.build(&config).unwrap();
        assert!(channel.is_allowed("123"));
        assert!(channel.is_allowed("456"));
        assert!(!channel.is_allowed("789"));
    }

    #[test]
    fn factory_build_empty_allow_from_allows_all() {
        let factory = DiscordChannelFactory;
        let config = serde_json::json!({
            "token": "my-bot-token",
            "allow_from": []
        });
        let channel = factory.build(&config).unwrap();
        assert!(channel.is_allowed("anyone"));
    }

    #[test]
    fn factory_build_no_allow_from_allows_all() {
        let factory = DiscordChannelFactory;
        let config = serde_json::json!({
            "token": "my-bot-token"
        });
        let channel = factory.build(&config).unwrap();
        assert!(channel.is_allowed("anyone"));
    }

    #[test]
    fn factory_build_with_custom_gateway_url() {
        let factory = DiscordChannelFactory;
        let config = serde_json::json!({
            "token": "my-bot-token",
            "gateway_url": "wss://custom-gateway.example.com"
        });
        let channel = factory.build(&config);
        assert!(channel.is_ok());
    }

    #[test]
    fn factory_build_with_custom_intents() {
        let factory = DiscordChannelFactory;
        let config = serde_json::json!({
            "token": "my-bot-token",
            "intents": 513
        });
        let channel = factory.build(&config);
        assert!(channel.is_ok());
    }

    #[test]
    fn factory_build_camel_case_aliases() {
        let factory = DiscordChannelFactory;
        let config = serde_json::json!({
            "token": "my-bot-token",
            "allowFrom": ["123"],
            "gatewayUrl": "wss://example.com"
        });
        let channel = factory.build(&config);
        assert!(channel.is_ok());
        let ch = channel.unwrap();
        assert!(ch.is_allowed("123"));
        assert!(!ch.is_allowed("999"));
    }

    #[test]
    fn factory_build_token_env_resolves() {
        let env_var = "CLAWFT_TEST_DISCORD_TOKEN_12345";
        // SAFETY: test-only, single-threaded test runner for this module.
        unsafe { std::env::set_var(env_var, "token-from-env") };
        let factory = DiscordChannelFactory;
        let config = serde_json::json!({
            "token_env": env_var
        });
        let channel = factory.build(&config);
        unsafe { std::env::remove_var(env_var) };
        assert!(channel.is_ok());
        assert_eq!(channel.unwrap().name(), "discord");
    }

    #[test]
    fn factory_build_token_env_missing_var_errors() {
        let factory = DiscordChannelFactory;
        let config = serde_json::json!({
            "token_env": "CLAWFT_TEST_NONEXISTENT_VAR_99999"
        });
        let result = factory.build(&config);
        match result {
            Err(ChannelError::Other(msg)) => {
                assert!(
                    msg.contains("CLAWFT_TEST_NONEXISTENT_VAR_99999"),
                    "error should mention env var: {msg}"
                );
            }
            _ => panic!("expected ChannelError::Other"),
        }
    }

    #[test]
    fn factory_build_explicit_token_takes_priority_over_env() {
        let env_var = "CLAWFT_TEST_DISCORD_PRIO_TOKEN";
        // SAFETY: test-only, single-threaded test runner for this module.
        unsafe { std::env::set_var(env_var, "env-token") };
        let factory = DiscordChannelFactory;
        let config = serde_json::json!({
            "token": "explicit-token",
            "token_env": env_var
        });
        let channel = factory.build(&config);
        unsafe { std::env::remove_var(env_var) };
        assert!(channel.is_ok());
    }

    #[test]
    fn factory_build_token_not_string_uses_default() {
        let factory = DiscordChannelFactory;
        // token defaults to empty string when not provided as string,
        // which triggers the validation check.
        let config = serde_json::json!({
            "token": 12345
        });
        let result = factory.build(&config);
        // serde will fail to deserialize a number as String
        assert!(result.is_err());
    }
}
