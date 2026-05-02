# Channels Guide

This guide covers the channel plugin system in clawft: architecture, setup
instructions for each built-in channel, the outbound message pipeline, and
how to implement a custom channel plugin.

---

## 1. Overview

Channels are bidirectional bridges between external chat platforms and the
agent pipeline. Each channel receives inbound messages from users and delivers
outbound messages produced by agents.

clawft has channel adapter scaffolding for eleven platforms, but only
**four ship with real network I/O today**: Telegram, Slack, Discord,
and the in-process `web` channel served by the Axum gateway. The other
seven (`email`, `whatsapp`, `signal`, `matrix`, `irc`, `google_chat`,
`teams`) are **planning stubs**: trait implementations, config
validation, factories, and tests landed, but the transports are
unimplemented -- `start()` waits for cancellation and `send()` returns
a synthetic message ID without contacting the platform. Do **not**
enable the corresponding feature in production until the runtime ships.

| Channel        | Transport                         | Threading | Media | Feature Gate      | Status  |
|----------------|-----------------------------------|-----------|-------|-------------------|---------|
| Telegram       | HTTP long polling (Bot API)       | No        | Yes   | `telegram`        | Ships   |
| Slack          | WebSocket (Socket Mode)           | Yes       | Yes   | `slack`           | Ships   |
| Discord        | WebSocket (Gateway v10)           | Yes       | Yes   | `discord`         | Ships   |
| Email          | IMAP + SMTP                       | Yes       | Yes   | `email`           | Roadmap (stub only) |
| WhatsApp       | WhatsApp Business API (webhook)   | No        | Yes   | `whatsapp`        | Roadmap (stub only) |
| Signal         | Signal CLI / signald bridge       | No        | Yes   | `signal`          | Roadmap (stub only) |
| Matrix         | Matrix client-server API          | Yes       | No    | `matrix`          | Roadmap (stub only) |
| IRC            | TCP / TLS (RFC 2812)              | No        | No    | `irc`             | Roadmap (stub only) |
| Google Chat    | Google Chat API (webhook / SA)    | Yes       | No    | `google-chat`     | Roadmap (stub only) |
| Microsoft Teams| Bot Framework / Graph API         | Yes       | Yes   | `teams`           | Roadmap (stub only) |
| Discord Resume | WebSocket (Gateway v10, resume)   | Yes       | Yes   | `discord-resume`  | Folded into Discord (E1; same crate) |

The roadmap for the seven stubs lives in
`.planning/reviews/0.7.0-release-gate/05-channels.md` (Tasks 1-7) and
the Element 06 tracker at
`.planning/sparc/phase4/06-channel-enhancements/04-element-06-tracker.md`.

All channels share the same trait-based interface and lifecycle, so they can
be enabled, disabled, and swapped without changes to the rest of the system.

Each channel adapter follows the 3-file pattern (`mod.rs`, `channel.rs`,
`types.rs`) inside `crates/clawft-channels/src/<channel>/` and has its own
feature gate in `crates/clawft-channels/Cargo.toml`.

---

## 2. Channel Architecture

The plugin system is built on three core traits and a host that manages the
lifecycle:

```text
                          config.json
                              |
                    ChannelFactory::build(config)
                              |
                       Arc<dyn Channel>
                              |
               PluginHost.init_channel("telegram", config)
                              |
               PluginHost.start_channel("telegram")
                     /                 \
         CancellationToken        Arc<dyn ChannelHost>
                     \                 /
               Channel::start(host, cancel)
                              |
               .-------------------------------.
               |     Inbound message loop       |
               |   host.deliver_inbound(msg)    |
               '-------------------------------'

Outbound path:

  Agent -> OutboundMessage -> MarkdownDispatcher -> PluginHost.send_to_channel()
                                                          |
                                                   Channel::send(msg)
```

### Traits

**`ChannelFactory`** -- Creates a `Channel` from a JSON config section.

```rust
pub trait ChannelFactory: Send + Sync {
    fn channel_name(&self) -> &str;
    fn build(&self, config: &serde_json::Value) -> Result<Arc<dyn Channel>, ChannelError>;
}
```

**`Channel`** -- The plugin itself. Handles both receiving and sending.

```rust
pub trait Channel: Send + Sync {
    fn name(&self) -> &str;
    fn metadata(&self) -> ChannelMetadata;
    fn status(&self) -> ChannelStatus;
    fn is_allowed(&self, sender_id: &str) -> bool;

    async fn start(
        &self,
        host: Arc<dyn ChannelHost>,
        cancel: CancellationToken,
    ) -> Result<(), ChannelError>;

    async fn send(&self, msg: &OutboundMessage) -> Result<MessageId, ChannelError>;
}
```

**`ChannelHost`** -- The bridge back to the agent pipeline. Plugins call
these methods to deliver inbound messages and register commands.

```rust
pub trait ChannelHost: Send + Sync {
    async fn deliver_inbound(&self, msg: InboundMessage) -> Result<(), ChannelError>;
    async fn register_command(&self, cmd: Command) -> Result<(), ChannelError>;
    async fn publish_inbound(
        &self,
        channel: &str,
        sender_id: &str,
        chat_id: &str,
        content: &str,
        media: Vec<String>,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Result<(), ChannelError>;
}
```

### PluginHost Lifecycle

`PluginHost` orchestrates the full channel lifecycle:

1. **Register factories** -- `register_factory(Arc<dyn ChannelFactory>)`
2. **Initialize channels** -- `init_channel(name, config)` calls the
   factory's `build()` and stores the resulting `Arc<dyn Channel>`
3. **Start channels** -- `start_channel(name)` spawns a tokio task that
   calls `Channel::start()` with a `CancellationToken`
4. **Route outbound** -- `send_to_channel(msg)` looks up the channel by
   name and calls `Channel::send()`
5. **Stop channels** -- `stop_channel(name)` cancels the token and awaits
   task completion

All channels run in separate tokio tasks. Stopping a channel is cooperative:
the `CancellationToken` signals the `start()` loop to exit.

### Channel Status

Every channel reports its lifecycle state:

| Status     | Meaning                                |
|------------|----------------------------------------|
| `Stopped`  | Not yet started or cleanly shut down   |
| `Starting` | Connecting / authenticating             |
| `Running`  | Processing messages                     |
| `Error(s)` | Encountered an error (may auto-retry)  |
| `Stopping` | Shutting down                           |

Check status from the CLI:

```bash
weft channels status
```

---

## 3. Telegram Setup

The Telegram channel uses the Bot API with HTTP long polling (`getUpdates`).

### 3.1 Create a Bot

1. Open Telegram and search for **@BotFather**.
2. Send `/newbot` and follow the prompts to choose a name and username.
3. BotFather will respond with a token in the format `123456789:ABCdef...`.
   Copy this token.

### 3.2 Configure

Add the Telegram section to your `config.json`:

```json
{
  "channels": {
    "telegram": {
      "enabled": true,
      "token": "123456789:ABCdef-your-bot-token",
      "allow_from": []
    }
  }
}
```

**Fields:**

| Field          | Type       | Required | Description                                      |
|----------------|------------|----------|--------------------------------------------------|
| `enabled`      | `bool`     | No       | Enable this channel. Default: `false`.            |
| `token`        | `string`   | Yes      | Bot token from BotFather.                         |
| `allow_from`   | `string[]` | No       | User IDs permitted to interact. Empty = all.      |
| `proxy`        | `string`   | No       | HTTP or SOCKS5 proxy URL.                         |

The `allow_from` field also accepts the alias `allowFrom` (camelCase).

### 3.3 How It Works

On startup, the Telegram channel:

1. Calls `getMe` to verify the bot token.
2. Enters a long-polling loop calling `getUpdates` with a 30-second timeout.
   The Bot API blocks server-side until an update arrives or the timeout
   elapses, so no extra client-side sleep is needed between cycles
   (default `poll_interval_secs = 0`). Operators can raise this value if
   they want explicit back-pressure on tight retry loops.
3. For each update containing a text message:
   - Extracts `sender_id`, `chat_id`, and `content`.
   - Checks the allow-list. Disallowed users are silently ignored.
   - Constructs an `InboundMessage` with metadata (`message_id`,
     `first_name`, `username`, `chat_type`).
   - Delivers it to the pipeline via `host.deliver_inbound()`.
4. On error, backs off for 5 seconds before retrying.
5. On cancellation, exits cleanly.

Outbound messages are sent via `sendMessage` to the `chat_id` from the
original message. Reply threading uses `reply_to_message_id`.

### 3.4 Supported Content

- **Inbound:** Text messages, with metadata for photos and documents.
- **Outbound:** Text with Telegram HTML formatting (`<b>`, `<i>`, `<code>`,
  `<pre>`, `<a>`, `<s>`). The `MarkdownDispatcher` converts CommonMark
  automatically.

---

## 4. Slack Setup

The Slack channel uses Socket Mode, which connects over a WebSocket using
an app-level token. No public URL or ingress is required.

### 4.1 Create a Slack App

1. Go to [api.slack.com/apps](https://api.slack.com/apps) and click
   **Create New App** > **From scratch**.
2. Name the app and select the workspace.

### 4.2 Enable Socket Mode

1. In the app settings, go to **Socket Mode** and toggle it on.
2. Generate an **app-level token** with the `connections:write` scope.
   The token starts with `xapp-`. Copy it.

### 4.3 Add Bot Scopes

Go to **OAuth & Permissions** and add these Bot Token Scopes:

| Scope                | Purpose                                |
|----------------------|----------------------------------------|
| `chat:write`         | Send messages                          |
| `app_mentions:read`  | Receive @mention events in channels    |
| `channels:read`      | List public channels                   |
| `im:read`            | Receive direct messages                |
| `im:write`           | Send direct messages                   |

### 4.4 Subscribe to Events

Go to **Event Subscriptions** and enable events. Subscribe to these
bot events:

- `message.im` -- Direct messages to the bot
- `app_mention` -- @mentions in channels

### 4.5 Install the App

Go to **Install App** and install it to your workspace. Copy the
**Bot User OAuth Token** (starts with `xoxb-`).

### 4.6 Configure

```json
{
  "channels": {
    "slack": {
      "enabled": true,
      "bot_token": "xoxb-your-bot-token",
      "app_token": "xapp-your-app-level-token",
      "group_policy": "mention",
      "dm": {
        "enabled": true,
        "policy": "open"
      }
    }
  }
}
```

**Fields:**

| Field              | Type       | Required | Description                                             |
|--------------------|------------|----------|---------------------------------------------------------|
| `enabled`          | `bool`     | No       | Enable this channel. Default: `false`.                  |
| `bot_token`        | `string`   | Yes      | Bot User OAuth Token (`xoxb-...`).                      |
| `app_token`        | `string`   | Yes      | App-level token for Socket Mode (`xapp-...`).           |
| `mode`             | `string`   | No       | Connection mode. Default: `"socket"`.                   |
| `group_policy`     | `string`   | No       | Group message policy. Default: `"mention"`.             |
| `group_allow_from` | `string[]` | No       | Channel IDs for `"allowlist"` group policy.             |
| `dm.enabled`       | `bool`     | No       | Accept direct messages. Default: `true`.                |
| `dm.policy`        | `string`   | No       | DM policy: `"open"` or `"allowlist"`. Default: `"open"`.|
| `dm.allow_from`    | `string[]` | No       | User IDs for `"allowlist"` DM policy.                   |

CamelCase aliases are supported: `botToken`, `appToken`, `groupPolicy`,
`groupAllowFrom`, `allowFrom`.

### 4.7 Group Policies

The `group_policy` field controls how the bot responds in group channels:

| Policy       | Behavior                                              |
|--------------|-------------------------------------------------------|
| `"mention"`  | Responds only when @mentioned. (Default)              |
| `"open"`     | Responds to all messages in channels it is in.        |
| `"allowlist"`| Responds only in channels listed in `group_allow_from`.|

### 4.8 How It Works

On startup, the Slack channel:

1. Calls `apps.connections.open` with the app-level token to obtain a
   WebSocket URL.
2. Connects to the WebSocket.
3. For each Socket Mode envelope:
   - Sends an acknowledgement (`envelope_id`) back immediately.
   - Filters for `events_api` envelopes containing `message` or
     `app_mention` events.
   - Skips bot messages (checks `bot_id`) to avoid loops.
   - Applies the DM or group policy to determine if the sender is allowed.
   - Constructs an `InboundMessage` with metadata (`ts`, `thread_ts`,
     `event_type`, `channel_type`).
   - Delivers it via `host.deliver_inbound()`.
4. If the WebSocket closes, reconnects after 5 seconds.
5. On cancellation, closes the WebSocket and exits.

Outbound messages use `chat.postMessage`. Thread replies are supported by
passing `thread_ts` in the message metadata.

### 4.9 Slack mrkdwn Formatting

Slack uses its own markup format. The `MarkdownDispatcher` automatically
converts CommonMark to Slack mrkdwn before sending:

| CommonMark     | Slack mrkdwn    |
|----------------|-----------------|
| `**bold**`     | `*bold*`        |
| `*italic*`     | `_italic_`      |
| `` `code` ``   | `` `code` ``    |
| `~~strike~~`   | `~strike~`      |
| `[text](url)`  | `<url\|text>`   |
| `> quote`      | `> quote`       |

### 4.10 Signature Verification

For HTTP event subscriptions (when not using Socket Mode), Slack signs
requests with HMAC-SHA256. The `verify_signature` function validates:

1. Concatenates `v0:{timestamp}:{body}` as the base string.
2. Computes `HMAC-SHA256(signing_secret, base_string)`.
3. Compares `v0={hex_digest}` against the `X-Slack-Signature` header.
4. Rejects requests with timestamps older than 5 minutes (anti-replay).

---

## 5. Discord Setup

The Discord channel connects to the Gateway WebSocket (API v10) and sends
messages via the REST API.

### 5.1 Create a Discord Application

1. Go to [discord.com/developers/applications](https://discord.com/developers/applications)
   and click **New Application**.
2. Name the application.

### 5.2 Create a Bot

1. Go to the **Bot** section in the application settings.
2. Click **Add Bot**.
3. Copy the bot **Token**. Keep it secret.

### 5.3 Enable Privileged Intents

Under the Bot section, enable these Privileged Gateway Intents:

- **Message Content Intent** -- Required to read message text.

### 5.4 Generate an Invite Link

Go to **OAuth2** > **URL Generator**:

1. Select the `bot` scope.
2. Select permissions: **Send Messages**, **Read Message History**.
3. Copy the generated URL and open it to invite the bot to your server.

### 5.5 Configure

```json
{
  "channels": {
    "discord": {
      "enabled": true,
      "token": "your-bot-token",
      "allow_from": [],
      "intents": 37377
    }
  }
}
```

**Fields:**

| Field         | Type       | Required | Description                                           |
|---------------|------------|----------|-------------------------------------------------------|
| `enabled`     | `bool`     | No       | Enable this channel. Default: `false`.                |
| `token`       | `string`   | Yes      | Bot token from the Developer Portal.                  |
| `allow_from`  | `string[]` | No       | User IDs (snowflakes) permitted. Empty = all.         |
| `gateway_url` | `string`   | No       | Gateway URL. Default: `wss://gateway.discord.gg/?v=10&encoding=json`. |
| `intents`     | `u32`      | No       | Gateway intents bitmask. Default: `37377`.            |

CamelCase aliases are supported: `allowFrom`, `gatewayUrl`.

### 5.6 Gateway Intents

The default intents value of `37377` enables:

| Intent           | Bit    | Value  | Description                    |
|------------------|--------|--------|--------------------------------|
| GUILDS           | 0      | 1      | Guild create/update/delete     |
| GUILD_MESSAGES   | 9      | 512    | Messages in guild channels     |
| DIRECT_MESSAGES  | 12     | 4096   | Messages in DM channels        |
| MESSAGE_CONTENT  | 15     | 32768  | Access to message text content |
| **Total**        |        | **37377** |                             |

To compute a custom intents value, bitwise-OR the flags you need.

### 5.7 How It Works

On startup, the Discord channel:

1. Connects to the Gateway WebSocket.
2. Waits for the Hello payload (opcode 10) containing `heartbeat_interval`.
3. Sends an Identify payload (opcode 2) with the bot token and intents.
4. Starts a heartbeat timer at the interval specified by Hello.
5. Processes dispatch events (opcode 0):
   - **READY** -- Stores `session_id` and `resume_gateway_url` for
     reconnection.
   - **MESSAGE_CREATE** -- Processes user messages:
     - Skips bot authors to avoid loops.
     - Checks the allow-list.
     - Constructs an `InboundMessage` with metadata (`message_id`,
       `username`, `guild_id`, `reply_to_message_id`).
     - Delivers via `host.deliver_inbound()`.
6. Handles Gateway lifecycle opcodes:
   - Opcode 1 (Heartbeat) -- Responds immediately.
   - Opcode 7 (Reconnect) -- Reconnects to the gateway.
   - Opcode 9 (Invalid Session) -- Clears session state and re-identifies.
   - Opcode 11 (Heartbeat ACK) -- Confirms heartbeat was received.
7. On disconnect, attempts to reconnect after 5 seconds. Uses the
   `resume_gateway_url` from the READY event when available.

Outbound messages are sent via the REST API (`POST /channels/{id}/messages`).
Rate limit headers (`x-ratelimit-remaining`, `x-ratelimit-reset-after`) are
tracked and respected.

### 5.8 Discord Markdown

Discord natively supports standard Markdown, so the `MarkdownDispatcher`
passes content through with minimal transformation:

| CommonMark     | Discord          |
|----------------|------------------|
| `**bold**`     | `**bold**`       |
| `*italic*`     | `*italic*`       |
| `` `code` ``   | `` `code` ``     |
| `~~strike~~`   | `~~strike~~`     |
| `[text](url)`  | `[text](url)`    |
| `> quote`      | `> quote`        |

---

## 6. Multi-Channel Gateway

clawft can run multiple channels simultaneously. Each channel operates in
its own tokio task with independent connection management, error recovery,
and cancellation.

### Configuration

Enable multiple channels in the same config:

```json
{
  "channels": {
    "telegram": {
      "enabled": true,
      "token": "telegram-bot-token"
    },
    "slack": {
      "enabled": true,
      "bot_token": "xoxb-...",
      "app_token": "xapp-..."
    },
    "discord": {
      "enabled": true,
      "token": "discord-bot-token"
    }
  }
}
```

### Outbound Message Routing

When an agent produces an `OutboundMessage`, it specifies the target
`channel` name and `chat_id`. The outbound pipeline works as follows:

1. The agent emits an `OutboundMessage` with fields:
   - `channel` -- Target channel name (e.g., `"telegram"`, `"slack"`)
   - `chat_id` -- Platform-specific chat/channel identifier
   - `content` -- Message body in CommonMark
   - `reply_to` -- Optional message ID to reply to
   - `media` -- Optional media attachments
   - `metadata` -- Arbitrary key-value pairs (e.g., `thread_ts` for Slack)

2. The `MarkdownDispatcher` converts the CommonMark `content` to the
   channel's native format (HTML for Telegram, mrkdwn for Slack,
   passthrough for Discord).

3. `PluginHost::send_to_channel()` looks up the channel by name and calls
   `Channel::send()`.

4. The channel plugin sends the message via its platform API.

### Allow-Lists

Each channel supports an allow-list that restricts which users can interact
with the bot. When the list is empty, all users are permitted. Allow-list
checks happen at the channel level before messages reach the pipeline.

Slack has a more granular policy system with separate controls for DMs and
group channels (see Section 4.7).

---

## 7. Additional Channels (roadmap stubs)

Seven additional channel adapters landed as compile-time stubs in the
improvements sprint: Email, WhatsApp, Signal, Matrix, IRC, Google Chat,
and Microsoft Teams. Discord Resume (E1) is folded into the production
Discord adapter. **None of the seven adapters above currently transmit
messages**: `start()` waits for cancellation and `send()` returns a
synthetic ID without contacting the platform. The configuration
schemas in [channels-additional.md](channels-additional.md) are correct
and stable, but enabling any of those features in production today
will silently drop every outbound message. Track the runtime work in
`.planning/reviews/0.7.0-release-gate/05-channels.md` (Tasks 1-7).

---

## 8. Creating Custom Channels

To add a new channel plugin, implement `ChannelFactory` and `Channel`, then
register the factory with the `PluginHost`.

### 8.1 Implement `ChannelFactory`

The factory parses JSON configuration and produces a `Channel` instance:

```rust
use std::sync::Arc;
use clawft_channels::{Channel, ChannelFactory, ChannelError};

pub struct MyChannelFactory;

impl ChannelFactory for MyChannelFactory {
    fn channel_name(&self) -> &str {
        "my_channel"
    }

    fn build(
        &self,
        config: &serde_json::Value,
    ) -> Result<Arc<dyn Channel>, ChannelError> {
        let token = config
            .get("token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ChannelError::Other("missing 'token'".into()))?;

        Ok(Arc::new(MyChannel::new(token.to_owned())))
    }
}
```

### 8.2 Implement `Channel`

The channel handles both the inbound receive loop and outbound sends:

```rust
use std::collections::HashMap;
use std::sync::Arc;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use clawft_channels::{
    Channel, ChannelHost, ChannelMetadata, ChannelStatus, MessageId, ChannelError,
};
use clawft_types::event::{InboundMessage, OutboundMessage};

pub struct MyChannel {
    token: String,
}

impl MyChannel {
    pub fn new(token: String) -> Self {
        Self { token }
    }
}

#[async_trait]
impl Channel for MyChannel {
    fn name(&self) -> &str {
        "my_channel"
    }

    fn metadata(&self) -> ChannelMetadata {
        ChannelMetadata {
            name: "my_channel".into(),
            display_name: "My Channel".into(),
            supports_threads: false,
            supports_media: false,
        }
    }

    fn status(&self) -> ChannelStatus {
        ChannelStatus::Running // Track real status in production
    }

    fn is_allowed(&self, _sender_id: &str) -> bool {
        true // Implement allow-list logic as needed
    }

    async fn start(
        &self,
        host: Arc<dyn ChannelHost>,
        cancel: CancellationToken,
    ) -> Result<(), ChannelError> {
        // Main receive loop.
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                // Replace with your platform's receive mechanism:
                msg = receive_next_message(&self.token) => {
                    let inbound = InboundMessage {
                        channel: "my_channel".into(),
                        sender_id: msg.sender,
                        chat_id: msg.chat,
                        content: msg.text,
                        timestamp: chrono::Utc::now(),
                        media: vec![],
                        metadata: HashMap::new(),
                    };
                    let _ = host.deliver_inbound(inbound).await;
                }
            }
        }
        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> Result<MessageId, ChannelError> {
        // Send via your platform's API.
        let id = send_to_platform(&self.token, &msg.chat_id, &msg.content).await?;
        Ok(MessageId(id))
    }
}
```

### 8.3 Register the Factory

Register your factory with the `PluginHost` before starting channels:

```rust
use std::sync::Arc;
use clawft_channels::PluginHost;

let plugin_host = PluginHost::new(host_impl);

// Register built-in channels.
plugin_host.register_factory(Arc::new(TelegramChannelFactory)).await;
plugin_host.register_factory(Arc::new(SlackChannelFactory)).await;
plugin_host.register_factory(Arc::new(DiscordChannelFactory)).await;

// Register your custom channel.
plugin_host.register_factory(Arc::new(MyChannelFactory)).await;

// Initialize and start from config.
plugin_host.init_channel("my_channel", &config).await?;
plugin_host.start_channel("my_channel").await?;
```

### 8.4 Add a Markdown Converter (Optional)

If your platform uses a formatting language other than CommonMark, implement
`MarkdownConverter` and register it with the `MarkdownDispatcher`:

```rust
use clawft_cli::markdown::{MarkdownConverter, MarkdownDispatcher};

pub struct MyMarkdownConverter;

impl MarkdownConverter for MyMarkdownConverter {
    fn convert(&self, markdown: &str) -> String {
        // Transform CommonMark to your platform's format.
        markdown.to_owned()
    }
}

let mut dispatcher = MarkdownDispatcher::new();
dispatcher.register("my_channel", Box::new(MyMarkdownConverter));
```

If no converter is registered for a channel name, the dispatcher passes
content through unchanged.

### 8.5 Checklist

Before shipping a custom channel:

- [ ] `ChannelFactory::build()` validates all required config fields
- [ ] `Channel::start()` respects the `CancellationToken` for clean shutdown
- [ ] `Channel::start()` handles reconnection on connection drops
- [ ] `Channel::is_allowed()` enforces the allow-list
- [ ] Bot messages are filtered to prevent loops
- [ ] `Channel::send()` returns a meaningful `MessageId`
- [ ] Error states are reported via `ChannelStatus::Error`
- [ ] A `MarkdownConverter` is registered if the platform does not use
  standard Markdown

---

## Further Reading

- `clawft-channels/src/traits.rs` -- Trait definitions
- `clawft-channels/src/host.rs` -- PluginHost implementation
- `clawft-channels/src/telegram/` -- Telegram plugin
- `clawft-channels/src/slack/` -- Slack plugin (Socket Mode, signature)
- `clawft-channels/src/discord/` -- Discord plugin (Gateway, REST)
- `clawft-channels/src/email/` -- Email plugin (IMAP + SMTP)
- `clawft-channels/src/whatsapp/` -- WhatsApp plugin (Business API)
- `clawft-channels/src/signal/` -- Signal plugin (signald bridge)
- `clawft-channels/src/matrix/` -- Matrix plugin (CS API)
- `clawft-channels/src/irc/` -- IRC plugin (RFC 2812, TLS)
- `clawft-channels/src/google_chat/` -- Google Chat plugin
- `clawft-channels/src/teams/` -- Microsoft Teams plugin (Bot Framework)
- `clawft-channels/src/discord_resume/` -- Discord Resume plugin
- `clawft-cli/src/markdown/` -- Markdown conversion and dispatch
- `clawft-types/src/config.rs` -- Configuration schema
