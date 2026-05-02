# Additional Channels Guide

> **Status -- planning stubs, not production**: with the exception of
> Discord Resume (E1, folded into the production Discord adapter), the
> seven adapters in this guide -- Email, WhatsApp, Signal, Matrix, IRC,
> Google Chat, Microsoft Teams -- are compile-time placeholders. Their
> trait implementations, config types, factories, and unit tests have
> landed, but the network transports have **not**. `start()` waits for
> cancellation and `send()` returns a synthetic ID without contacting
> the platform. Enabling any of these features in production today
> will cause outbound messages to be silently dropped. The runtime
> work is tracked as Tasks 1-7 in
> `.planning/reviews/0.7.0-release-gate/05-channels.md` and per-item in
> `.planning/sparc/phase4/06-channel-enhancements/04-element-06-tracker.md`.
> The configuration schemas below are stable and correct -- they are
> safe to draft against -- but you should treat the channels themselves
> as roadmap until the linked tasks ship.

This guide covers the seven channel-adapter stubs added in the
improvements sprint, plus the Discord Resume enhancement that ships
inside the production Discord adapter. For the core channel
architecture, original channels (Telegram, Slack, Discord),
multi-channel gateway, and custom channel development, see
[channels.md](channels.md).

All adapters below follow the 3-file pattern (`mod.rs`, `channel.rs`,
`types.rs`) inside `crates/clawft-channels/src/<channel>/` and have their own
feature gate in `crates/clawft-channels/Cargo.toml`.

---

## 1. Email Setup

The Email channel uses IMAP for receiving messages and SMTP for sending.
It supports both HTML and plain-text bodies, and handles attachments.

### 1.1 Transport

- **Inbound:** Connects to an IMAP mailbox via TLS and polls for new messages
  using the IDLE command (push) or a configurable poll interval (fallback).
- **Outbound:** Sends messages via SMTP with STARTTLS or direct TLS.

### 1.2 Authentication

Username and password for both IMAP and SMTP. OAuth2 bearer tokens are also
supported when the server advertises `XOAUTH2`.

### 1.3 Configuration

```json
{
  "channels": {
    "email": {
      "enabled": true,
      "imap_host": "imap.example.com",
      "imap_port": 993,
      "smtp_host": "smtp.example.com",
      "smtp_port": 587,
      "username": "bot@example.com",
      "password": "app-specific-password",
      "folder": "INBOX",
      "poll_interval_secs": 30
    }
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `imap_host` | `string` | Yes | IMAP server hostname. |
| `imap_port` | `u16` | No | IMAP port. Default: `993`. |
| `smtp_host` | `string` | Yes | SMTP server hostname. |
| `smtp_port` | `u16` | No | SMTP port. Default: `587`. |
| `username` | `string` | Yes | Login username (typically the email address). |
| `password` | `string` | Yes | Login password or app-specific password. |
| `folder` | `string` | No | IMAP folder to monitor. Default: `"INBOX"`. |
| `poll_interval_secs` | `u64` | No | Fallback poll interval in seconds. Default: `30`. |

---

## 2. WhatsApp Setup

The WhatsApp channel integrates via the WhatsApp Business API using webhooks.

### 2.1 Transport

- **Inbound:** Receives webhook POST requests from the WhatsApp Business
  platform when messages arrive.
- **Outbound:** Sends messages via the WhatsApp Cloud API REST endpoint.

### 2.2 Authentication

Requires a permanent access token from the Meta Business dashboard and a
phone number ID. Webhook verification uses a user-defined verify token.

### 2.3 Configuration

```json
{
  "channels": {
    "whatsapp": {
      "enabled": true,
      "access_token": "your-permanent-access-token",
      "phone_number_id": "123456789",
      "verify_token": "your-webhook-verify-token",
      "webhook_port": 8080
    }
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `access_token` | `string` | Yes | WhatsApp Cloud API access token. |
| `phone_number_id` | `string` | Yes | Phone number ID from Meta dashboard. |
| `verify_token` | `string` | Yes | Token for webhook verification handshake. |
| `webhook_port` | `u16` | No | Local port for the webhook listener. Default: `8080`. |

---

## 3. Signal Setup

The Signal channel communicates through Signal CLI or the signald bridge
daemon.

### 3.1 Transport

- **Inbound:** Listens on a Unix socket or TCP connection to Signal CLI /
  signald for incoming message events.
- **Outbound:** Sends messages by issuing commands to the Signal CLI / signald
  JSON-RPC interface.

### 3.2 Authentication

Requires a registered Signal phone number. Registration is performed
out-of-band via `signal-cli register` or the signald registration flow.

### 3.3 Configuration

```json
{
  "channels": {
    "signal": {
      "enabled": true,
      "account": "+15551234567",
      "signald_socket": "/var/run/signald/signald.sock",
      "mode": "signald"
    }
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `account` | `string` | Yes | Registered Signal phone number (E.164 format). |
| `signald_socket` | `string` | No | Path to the signald Unix socket. Default: `/var/run/signald/signald.sock`. |
| `mode` | `string` | No | Bridge mode: `"signald"` or `"signal-cli"`. Default: `"signald"`. |

---

## 4. Matrix Setup

The Matrix channel uses the native Matrix client-server API (CS API) to join
rooms and exchange messages.

### 4.1 Transport

- **Inbound:** Long-polls the `/sync` endpoint for new room events.
- **Outbound:** Sends `m.room.message` events via the CS API.

### 4.2 Authentication

Access token authentication. The token can be obtained via password login or
generated from the Matrix admin console. The bot must be invited to rooms
before it can participate.

### 4.3 Configuration

```json
{
  "channels": {
    "matrix": {
      "enabled": true,
      "homeserver_url": "https://matrix.example.com",
      "access_token": "syt_your_access_token",
      "user_id": "@bot:example.com"
    }
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `homeserver_url` | `string` | Yes | Matrix homeserver base URL. |
| `access_token` | `string` | Yes | Bot access token. |
| `user_id` | `string` | Yes | Fully-qualified Matrix user ID (e.g. `@bot:example.com`). |

---

## 5. IRC Setup

The IRC channel implements RFC 2812 with TLS support and NickServ/SASL
authentication. It is feature-gated behind the `irc` feature flag.

### 5.1 Transport

- **Inbound:** Maintains a persistent TCP/TLS connection to the IRC server
  and parses PRIVMSG events.
- **Outbound:** Sends PRIVMSG commands to target channels or users.

### 5.2 Authentication

Supports three authentication methods:

- **NickServ:** Sends `IDENTIFY` to NickServ after connecting.
- **SASL PLAIN:** Authenticates during connection registration via the SASL
  capability.
- **Server password:** Passes a password in the IRC `PASS` command.

### 5.3 Feature Gate

The IRC adapter must be explicitly enabled at compile time:

```bash
cargo build --features irc
```

### 5.4 Configuration

```json
{
  "channels": {
    "irc": {
      "enabled": true,
      "server": "irc.libera.chat",
      "port": 6697,
      "tls": true,
      "nick": "clawft-bot",
      "channels_to_join": ["#my-channel"],
      "auth_method": "sasl",
      "password": "nickserv-or-sasl-password"
    }
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `server` | `string` | Yes | IRC server hostname. |
| `port` | `u16` | No | Server port. Default: `6697` (TLS) or `6667` (plain). |
| `tls` | `bool` | No | Enable TLS. Default: `true`. |
| `nick` | `string` | Yes | Bot nickname. |
| `channels_to_join` | `string[]` | Yes | IRC channels to join on connect. |
| `auth_method` | `string` | No | One of `"nickserv"`, `"sasl"`, `"server_pass"`. Default: `"nickserv"`. |
| `password` | `string` | No | Authentication password. |

---

## 6. Google Chat Setup

The Google Chat channel integrates via the Google Chat API using either
incoming webhooks or a service account with the Chat API.

### 6.1 Transport

- **Inbound (service account mode):** Receives events via Google Cloud
  Pub/Sub subscription or HTTP push endpoint.
- **Inbound (webhook mode):** Not supported -- webhooks are outbound-only.
- **Outbound:** Posts messages via the Google Chat REST API.

### 6.2 Authentication

- **Service account:** Uses a Google service account JSON key file with the
  `https://www.googleapis.com/auth/chat.bot` scope.
- **Webhook:** Uses a pre-generated webhook URL (outbound only, no inbound).

### 6.3 Configuration

```json
{
  "channels": {
    "google_chat": {
      "enabled": true,
      "mode": "service_account",
      "credentials_file": "/path/to/service-account.json",
      "space_id": "spaces/AAAA"
    }
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `mode` | `string` | No | `"service_account"` or `"webhook"`. Default: `"service_account"`. |
| `credentials_file` | `string` | Conditional | Path to service account JSON key. Required for `service_account` mode. |
| `webhook_url` | `string` | Conditional | Webhook URL. Required for `webhook` mode. |
| `space_id` | `string` | No | Default space to post messages to. |

---

## 7. Microsoft Teams Setup

The Microsoft Teams channel uses the Bot Framework or Microsoft Graph API to
exchange messages.

### 7.1 Transport

- **Inbound:** Receives activity POSTs from the Bot Framework Service at a
  configured messaging endpoint.
- **Outbound:** Sends messages via the Bot Framework REST API or Graph API.

### 7.2 Authentication

Requires an Azure Bot registration with a Microsoft App ID and client secret.
The adapter exchanges the client secret for a Bearer token via the Azure AD
token endpoint.

### 7.3 Configuration

```json
{
  "channels": {
    "teams": {
      "enabled": true,
      "app_id": "your-azure-app-id",
      "app_secret": "your-azure-client-secret",
      "tenant_id": "your-azure-tenant-id",
      "webhook_port": 3978
    }
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `app_id` | `string` | Yes | Azure Bot App ID. |
| `app_secret` | `string` | Yes | Azure Bot client secret. |
| `tenant_id` | `string` | No | Azure AD tenant. Default: `"common"` (multi-tenant). |
| `webhook_port` | `u16` | No | Local port for the messaging endpoint. Default: `3978`. |

---

## 8. Discord Resume Setup

The Discord Resume adapter is an enhanced version of the standard Discord
channel with robust gateway session resume and reconnect handling.

### 8.1 Transport

Same as the standard Discord channel (Gateway WebSocket v10 + REST API), with
added support for:

- **Session resume:** Stores `session_id` and `resume_gateway_url` and
  replays missed events on reconnect (opcode 6 Resume).
- **Exponential backoff:** Reconnection attempts use jittered exponential
  backoff instead of a fixed 5-second delay.
- **Sequence tracking:** Maintains the last seen sequence number (`s`) for
  accurate resume payloads.

### 8.2 Authentication

Same as the standard Discord channel -- a bot token from the Developer Portal.

### 8.3 Configuration

```json
{
  "channels": {
    "discord_resume": {
      "enabled": true,
      "token": "your-bot-token",
      "allow_from": [],
      "intents": 37377,
      "max_reconnect_attempts": 10,
      "resume_timeout_secs": 30
    }
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `token` | `string` | Yes | Bot token from the Developer Portal. |
| `allow_from` | `string[]` | No | User IDs permitted. Empty = all. |
| `intents` | `u32` | No | Gateway intents bitmask. Default: `37377`. |
| `max_reconnect_attempts` | `u32` | No | Maximum resume attempts before full re-identify. Default: `10`. |
| `resume_timeout_secs` | `u64` | No | Timeout for resume handshake. Default: `30`. |

---

## Further Reading

- `clawft-channels/src/email/` -- Email plugin (IMAP + SMTP)
- `clawft-channels/src/whatsapp/` -- WhatsApp plugin (Business API)
- `clawft-channels/src/signal/` -- Signal plugin (signald bridge)
- `clawft-channels/src/matrix/` -- Matrix plugin (CS API)
- `clawft-channels/src/irc/` -- IRC plugin (RFC 2812, TLS)
- `clawft-channels/src/google_chat/` -- Google Chat plugin
- `clawft-channels/src/teams/` -- Microsoft Teams plugin (Bot Framework)
- `clawft-channels/src/discord_resume/` -- Discord Resume plugin
- [Core channels guide](channels.md) -- Architecture, Telegram, Slack, Discord
