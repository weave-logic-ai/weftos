---
title: "Channels (Discord/Telegram/Slack/Gateway)"
slug: channels
workstream_id: "05"
release_gate: "0.7.0"
status: "audit"
audit_type: "comprehensive"
filter_by_release_scope: false
last_updated: 2026-04-28
crates:
  - clawft-channels
  - clawft-services (api/ feature-gated)
  - clawft-plugin-oauth2 (cross-ref to ws04)
related_planning:
  - .planning/sparc/phase4/06-channel-enhancements/
  - .planning/development_notes/06-channel-enhancements/
  - .planning/development_notes/05-axum-api-layer/
  - .planning/development_notes/social-media-integration.md
  - .planning/development_notes/feature-request-whitelabel.md
  - .planning/development_notes/step3-vs1.3-voice-channel.md
---

# Channels (Discord / Telegram / Slack / Gateway)

## General Description

Workstream 05 covers all inbound surfaces by which users reach the agent
and the gateway that exposes the daemon over HTTP/WebSocket. The
`clawft-channels` crate hosts a trait-based plugin system: each channel
implements the `Channel` (legacy) or `ChannelAdapter` (new) trait, is
instantiated from JSON config by a `ChannelFactory`, and is owned by a
`PluginHost` that drives the per-channel start/stop lifecycle, routes
outbound messages, and reports status. The HTTP/WS gateway lives in
`clawft-services/src/api/` (feature-gated under `api`) and includes a
`web` channel that converts inbound REST calls into pipeline messages
and broadcasts assistant replies back to subscribed browser clients via
a `TopicBroadcaster`.

Two trait families coexist by design:

- **Legacy `Channel` trait** (`crates/clawft-channels/src/traits.rs`) --
  used by the in-tree Telegram, Discord, Slack, and `web` channels. Has
  its own `ChannelHost` for delivering inbound messages and registering
  slash-style commands.
- **New `ChannelAdapter` trait** (`crates/clawft-plugin/src/traits.rs`,
  cross-crate) -- used by the newer email, google_chat, teams,
  whatsapp, signal, matrix, and irc adapters. C7 of Element 04 promises
  PluginHost unification; in the meantime `ChannelAdapterShim` and
  `ChannelAdapterHostBridge` in `plugin_host.rs` bridge the two.

Per-channel permissioning is wired through `routing.permissions.channels`
in the global config and resolved by `PermissionResolver` in
`crates/clawft-core/src/pipeline/permissions.rs`, where channel-level
overrides beat user-level overrides (see test
`test_channel_overrides_beat_user_overrides`). Each channel additionally
carries its own `allow_from` / `allowed_users` / policy fields that
short-circuit unwanted senders before any pipeline work happens, and
Discord exposes an `allow_from_match` metadata flag that the resolver
uses to promote an allowed sender out of the zero-trust tier.

## Status & Timeline

Element 06 (the SPARC tracker for this workstream) reports
**9 / 9 items complete** as of 2026-02-20 across three phases:

| Phase | Items | Done | Notes |
|-------|-------|------|-------|
| E-Fix (W4-5) | E1 Discord Resume, E6 Heartbeat | 2/2 | Resume + RESUMED + OP 9 done; heartbeat lives in `clawft-services` |
| E-Enterprise (W5-7) | E2 Email, E5a Google Chat, E5b Teams | 3/3 | E2 has a stub IMAP/SMTP runtime; E5a/E5b have stub send/receive |
| E-Consumer (W6-8) | E3 WhatsApp, E4 Signal, E5 Matrix, E5-IRC | 4/4 | All adapters are skeleton-with-tests; outbound returns synthetic IDs |

Trait/architecture exit criteria are checked off, security exit criteria
(SecretRef, OAuth2 state, signal sanitization, 0600 token files) are
checked off, and "all 2,075+ existing tests pass" is checked off. The
**caveat** is that "complete" in the tracker means "trait surface,
config types, validation, and tests landed"; the network I/O for the
new channels is largely stub. See "What's Left" below for the actual
runtime gaps.

`clawft-channels/Cargo.toml` defaults to **no feature flags on**:
`email`, `whatsapp`, `signal`, `matrix`, `google-chat`, `irc`, `teams`
are each behind their own feature, and `default = []`. Discord, Slack,
Telegram, and `web` are always built. Element 06 also lists an
`imessage` adapter under E4, but `crates/clawft-channels/src/imessage/`
does not exist on disk; this is orphaned scope.

The companion gateway/API stream (Axum API layer) has its own decision
record at `.planning/development_notes/05-axum-api-layer/decisions.md`,
covering 10 design decisions (D-1 through D-10) -- bridge-pattern,
broadcast topics, SSE streaming, SPA fallback, and crucially **D-7: auth
middleware exists but is NOT wired into the router**.

## Released Features

The following items are wired up, exercised by tests, and reachable from
config:

- **PluginHost** (`crates/clawft-channels/src/host.rs`) with parallel
  `start_all` / `stop_all`, per-channel `CancellationToken`, status
  introspection, and outbound routing via `send_to_channel`. 12 unit
  tests in `host.rs` covering register / init / start / stop / send /
  status / unknown-channel error paths.
- **Discord channel** (`crates/clawft-channels/src/discord/`):
  - Full Gateway v10 WebSocket loop with Hello, Identify, Resume (OP 6),
    Heartbeat / Heartbeat-ACK, Reconnect (OP 7), and Invalid Session
    (OP 9, both resumable and non-resumable branches) handling.
  - `chunk_message` helper that splits at newline, then space, then
    hard-splits at 2000 chars, with eight unit tests (`chunk_short_message`,
    `chunk_at_newline_boundary`, `chunk_at_space_boundary`,
    `chunk_hard_split_no_boundaries`, `chunk_exactly_at_limit`,
    `chunk_one_over_limit`, `chunk_preserves_all_content`,
    `chunk_empty_message`).
  - REST API client with rate-limit header parsing
    (`x-ratelimit-remaining`, `-reset`, `-reset-after`, `-bucket`) and
    `is_limited()` / `retry_after_ms()` accessors.
  - Factory supports both `token` and `token_env` resolution,
    camelCase aliases (`allowFrom`, `gatewayUrl`), and emits
    `allow_from_match` metadata for the permission resolver.
- **Slack channel** (`crates/clawft-channels/src/slack/`):
  - Socket Mode WebSocket loop with envelope ack and reconnect.
  - Per-DM and per-group policies (`open` | `allowlist` | `mention`),
    bot-message loop suppression, and `app_mention` filtering.
  - Signature verification helper in `signature.rs` (HMAC-SHA256).
- **Telegram channel** (`crates/clawft-channels/src/telegram/`):
  - Long-poll loop with offset advancement-on-error, error backoff
    that respects cancellation, and bot-token verification on start.
  - Factory supports `token` / `token_env` and `allowed_users` allow
    list. Reply-to chaining via `OutboundMessage.reply_to`.
- **Web channel** (`crates/clawft-channels/src/web/`):
  - Backed by the `TopicBroadcaster` from the API layer; `start()` is a
    no-op that waits for cancellation because inbound arrives via the
    REST endpoint `POST /api/sessions/{key}/messages`.
  - Publishes to `sessions:{chat_id}` topic for the conversation feed
    and to `sessions` for the session-list refresh.
- **PluginHost C7 bridges** (`plugin_host.rs`):
  - `ChannelAdapterShim` -- wraps a legacy `Channel` so the new
    `ChannelAdapter` host can drive it; cancels by polling the plugin
    `CancellationToken` from a tokio task on a 100 ms tick.
  - `ChannelAdapterHostBridge` -- inverse direction, downcasting
    `MessagePayload::{Text, Structured, Binary}` into the legacy
    `InboundMessage.content` string (binary becomes a placeholder).
  - `SoulConfig` -- loads SOUL.md from `.clawft/SOUL.md`, `SOUL.md`,
    or `~/.clawft/SOUL.md` and injects it into the assembler system
    prompt with hot-reload staleness detection.
- **Axum gateway** (`crates/clawft-services/src/api/`, feature `api`):
  - Routers for chat, sessions, channels, skills, memory, config,
    cron, voice, monitoring, delegation, and a WebSocket / SSE pair
    backed by `TopicBroadcaster`.
  - `D-6` API-only mode: the gateway can run with zero messaging
    channels enabled and still serve the web dashboard.
  - `TokenStore` + `auth_middleware` exist but are intentionally not
    enabled (D-7).
- **Channel Adapter skeletons** (feature-gated, `ChannelAdapter` trait):
  email, google_chat, teams, whatsapp, signal, matrix, irc -- each with
  trait impl, config validation, allow-list filtering, and tests, but
  no production network I/O (see "What's Left").
- **Per-channel permissions integration**: channel overrides
  (`routing.permissions.channels.<name>`) merged on top of the global
  default by `PermissionResolver`, ahead of user-level overrides.
- **Social-media via skills, not crates** (ADR
  `.planning/development_notes/social-media-integration.md`): the deleted
  `crates/clawft-twitter` is intentional; X, LinkedIn, Bluesky, Mastodon,
  GitHub etc. are handled by SKILL.md skills calling `rest_request` from
  `clawft-plugin-oauth2`. No Rust crate per platform.

## What's Left -- Total Depth

### TODOs / FIXMEs in code

In-channels-crate code is unusually clean. Direct grep yields only:

- `crates/clawft-channels/src/irc/channel.rs:82` -- `// TODO: Connect to
  the IRC server using an IRC client library.` (entire start loop is a
  log-only stub waiting on the `irc` crate to be added as a dep).
- `crates/clawft-channels/src/irc/channel.rs:128` -- `// TODO: Send the
  message using the connected IRC client.` (PRIVMSG never goes out;
  `send` returns a synthetic `irc-{target}-{ts}` id even though no
  socket exists).

Adjacent gateway/API TODOs (cross-stream but in scope for this audit):

- `clawft-services/src/api/handlers.rs:130` -- TODO: Content-Security-Policy
  via tower layer.
- `clawft-services/src/api/handlers.rs:133` -- TODO: rate limiting
  (`tower::limit::RateLimitLayer` or `tower-governor`), spec'd
  per-endpoint (auth/token 5 rpm, delegation 60 rpm, monitoring 30 rpm).
- `clawft-services/src/api/bridge.rs:282` -- TODO: skill installation
  via ClawHub registry (channels/skills bridge stub).
- `clawft-services/src/api/bridge.rs:287` -- TODO: skill uninstallation.
- `clawft-services/src/api/bridge.rs:395` -- TODO: memory entry deletion.
- `clawft-services/src/api/bridge.rs:467` -- TODO: config persistence
  (deserialize, validate, write to file).

### Stubbed runtime (the big one)

Every `ChannelAdapter`-based channel ships with a passing test suite but
the actual transport is a `debug!` log. Each marks itself as a "stub" or
"would start here" in source:

| Channel | File | Stubbed surface |
|---------|------|-----------------|
| email | `email/channel.rs:166-175` | poll loop is `debug!("polling for new emails (stub)")`; needs `imap` + `lettre` integration behind the `email` feature |
| email | `email/channel.rs:200-217` | `send` fabricates a `<ts-target@host>` Message-ID without invoking SMTP |
| google_chat | `google_chat/channel.rs:78-90` | OAuth2 + Pub/Sub event subscription deferred to F6 (now landed in `clawft-plugin-oauth2`); rewire pending |
| google_chat | `google_chat/channel.rs:112-125` | `send` builds a `spaces/{target}/messages/gchat-{ts}` id; no POST to `chat.spaces.messages.create` |
| teams | `teams/channel.rs:88-98` | Bot Framework registration + Azure AD client-credentials token acquisition not implemented |
| teams | `teams/channel.rs:122-135` | Graph API `/teams/{team-id}/channels/{channel-id}/messages` POST not implemented |
| whatsapp | `whatsapp/channel.rs:74-79` | Cloud API webhook listener missing; no signature verification (E3 risk register flagged 429 backoff but no handler exists yet) |
| whatsapp | `whatsapp/channel.rs:96-105` | `send` fabricates `wamid.{ts}` without POST to `/v18.0/{phone_number_id}/messages` |
| signal | `signal/channel.rs:90-105` | `signal-cli daemon` subprocess + JSON-RPC reader missing; no PID tracking, no auto-restart |
| signal | `signal/channel.rs:132-148` | `send` does not actually `tokio::process::Command` `signal-cli` -- argument sanitization runs but no process is spawned |
| matrix | `matrix/channel.rs:82-95` | `/sync` long-poll, room auto-join, m.room.message parsing not implemented |
| matrix | `matrix/channel.rs:115-128` | `send` returns `${ts}` -- no `PUT /_matrix/client/v3/rooms/{room}/send/m.room.message/{txn}` |
| irc | `irc/channel.rs:82-101` | TCP/TLS dial, NICK/USER/CAP, JOIN, PRIVMSG reader -- all missing; pending `irc` crate selection |
| irc | `irc/channel.rs:128-149` | PRIVMSG send missing; returns synthetic id |

This means `weft gateway --features email,whatsapp,signal,matrix,
google-chat,teams,irc` will start cleanly, validate config, accept
outbound `send()` calls, and silently drop every message. There are no
runtime warnings beyond a single `debug!` line per stub. Production
deployment of any of these flips a foot-gun.

### Deferred / orphaned items

- **`clawft-channels/src/imessage/`** -- listed in
  `00-orchestrator.md` and `04-element-06-tracker.md` E4 description as
  paired with Signal, but the directory does not exist. Iteration-1
  review (`.planning/sparc/reviews/iteration-1-spec-05-06.md:259`)
  flagged that it is "Not in orchestrator". Either the scope was
  silently dropped or the macOS AppleScript bridge is intended for a
  later release.
- **PluginHost C7 unification** -- the trait shim (`ChannelAdapterShim`)
  exists, but Telegram / Discord / Slack still implement the legacy
  `Channel` trait directly. Migrating them to `ChannelAdapter` is the
  C7 deliverable from Element 04, currently not started in
  `clawft-channels`. The migration changes the cancellation contract
  (poll-based plugin token vs `tokio_util::sync::CancellationToken`)
  and the inbound payload type (`MessagePayload` vs
  `InboundMessage.content: String`).
- **Slash-command registration** -- `ChannelHost::register_command`
  is part of the trait surface and tested in mock hosts, but no
  in-tree channel actually calls it. Discord slash-command
  registration, Telegram BotFather-style commands, and Slack slash
  commands are all unimplemented despite the trait surface. No
  scheduled tracker entry covers this.
- **Discord message chunking improvements** (called out in MEMORY.md
  project context) -- current chunker handles newline / space / hard
  split at 2000 chars but does not:
  - Preserve fenced code blocks across splits (a code block split mid
    body becomes two malformed half-blocks rendered as monospace text).
  - Re-balance Markdown emphasis tokens (`**bold**` opened in chunk 1
    will not be auto-closed/reopened across chunks).
  - Use 4000-char limit for Nitro / boost detection.
  - Honor 6000-char embed total / 25-field limits when the agent emits
    embeds (today only `content` is sent; embeds aren't supported).
  - Fall back to file upload when the message exceeds N chunks.
  No tracker entry exists for these; they're implicit in MEMORY.md.
- **Channel failover chain** -- MEMORY.md mentions "failover chain
  improvements"; the only `failover` machinery in tree is in
  `clawft-llm/src/failover.rs` (provider failover), not channel
  failover. There is no concept of "if Discord drops, deliver to
  Telegram instead" -- the PluginHost treats each channel as
  independent. Whether to keep it that way or to grow a fallback
  chain is an open product question; no design doc exists.
- **iMessage and AppleScript bridge** -- listed under E4 in the SPARC
  orchestrator but never created (see orphaned section above).
- **WeftOS white-label feature** (`feature-request-whitelabel.md`) --
  P1 for Valtech, P2 generally. Hard-coded "WeftOS" / "clawft" strings
  appear in channel banners (Discord identify `browser: "clawft"`,
  `device: "clawft"`), CLI help, web UI header. No work has started
  on a `brand()` accessor.
- **Voice channel** (`step3-vs1.3-voice-channel.md`) -- ships
  `VoiceChannel` implementing `ChannelAdapter` but `start()` waits for
  cancellation without capturing audio, `send()` logs TTS text without
  playing it, CLI `weft voice talk` shows status only, and
  `deliver_inbound` from real transcriptions is "deferred". This lives
  in `crates/clawft-plugin/src/voice/` rather than `clawft-channels/`,
  so it's adjacent rather than in-crate, but in scope as an inbound
  surface.

### Open questions

1. **Trait migration cutover.** When Telegram / Discord / Slack move to
   `ChannelAdapter`, do we keep the shim long-term for third-party
   plugins, or delete it and force everyone onto the new trait? The
   shim's polling-based cancellation costs a 100 ms-tick spawn per
   channel start.
2. **Auth middleware enablement (D-7).** The middleware is wired in
   code but not in the router. Enabling it requires a UI login page;
   the timeline for that is not in any tracker. Until then, every
   non-localhost gateway deployment is unauthenticated.
3. **Rate limiting policy.** Handlers.rs lists target rates per
   endpoint, but no decision has been made on whether to use
   `tower-governor`, `tower::limit::RateLimitLayer`, or a custom
   per-token bucket scheme; this also intersects with the
   yet-to-be-deployed `TokenStore`.
4. **CSP / security headers.** Same handlers.rs TODO -- no
   Content-Security-Policy is emitted today. The web channel publishes
   user-generated content to a JS dashboard with no CSP.
5. **Web channel authentication boundary.** `WebChannel::is_allowed`
   returns `true` unconditionally because "auth is handled by the API
   middleware, not here." With D-7 still off, *nothing* authenticates
   web-channel inbound. This isn't documented as a known gap.
6. **`reset_after` rate-limit handling on edit.** `DiscordApiClient::edit_message`
   reads rate-limit headers but does NOT actually sleep before
   returning (compare to `create_message` which does
   `tokio::time::sleep`). Likely a copy-paste oversight; the warning
   log fires but no backoff happens.
7. **Telegram poll-loop double-sleep.** The long-poll already blocks
   for `DEFAULT_POLL_TIMEOUT_SECS = 30` server-side, then yields
   another `DEFAULT_POLL_INTERVAL_SECS = 1`. Whether the extra sleep
   adds value or just delays inbound messages by 1 s is undocumented.
8. **Discord intents bitmask default.** Factory default is `37377`
   (GUILDS | GUILD_MESSAGES | DIRECT_MESSAGES + a few). No doc points
   at the chosen bits, no test covers what happens if the user sets
   `intents = 0`. Privileged intents (MESSAGE_CONTENT, GUILD_MEMBERS)
   are not negotiated.
9. **Slack envelope unknown-type fallthrough.** The processor logs
   `non-envelope message` on parse failure and silently moves on. There
   is no metric or counter to detect a regression where every payload
   becomes "non-envelope" after a Slack API change.
10. **Permission promotion via `allow_from_match`.** Discord sets this
    metadata flag, but Slack and Telegram do not, so a user listed in
    `slack.dm.allow_from` is *passed through to the pipeline* but is
    not promoted from zero-trust to user level. Behavior asymmetry.

### Orphaned work

- `crates/clawft-channels/src/imessage/` (mentioned in tracker, no
  files).
- `clawft-twitter/` (deleted; ADR replaces with skills, see
  `social-media-integration.md`). Audit-relevant only as a sanity check
  that the deletion is final.
- `skills/twitter-bookmarks/` (also deleted in the same ADR).
- C7 "PluginHost unification" -- the shim exists in plugin_host.rs but
  no consumer uses it; in-tree adapters still construct the legacy host.
- `register_command` -- present on `ChannelHost`, used by no live
  channel.

## Task List

| # | Task | Severity | Effort | Owner | Notes |
|---|------|----------|--------|-------|-------|
| 1 | Replace IRC stub with real `irc` crate integration: connect, auth (`auth_method`), JOIN listed channels, PRIVMSG read/write, reconnect with `reconnect_delay_secs` | High | L | unassigned | TODOs at irc/channel.rs:82 and :128 |
| 2 | Wire WhatsApp Cloud API: webhook receiver with `X-Hub-Signature-256` verify, POST `/v{api}/{phone_number_id}/messages`, 429 backoff | High | M | unassigned | E3 risk in tracker |
| 3 | Wire Signal `signal-cli daemon` subprocess: spawn with sanitized args, JSON-RPC stdout reader, PID tracking, auto-restart on crash with timeout-kill | High | L | unassigned | E4 risk in tracker; sanitization already lands |
| 4 | Wire Email IMAP poll + SMTP send via `imap` + `lettre` crates behind `email` feature | High | M | unassigned | E2 stubs at email/channel.rs:166 and :200 |
| 5 | Wire Matrix `/sync` long-poll, room auto-join, `m.room.message` parse, `PUT /rooms/{room}/send/...` outbound | High | M | unassigned | E5 stubs at matrix/channel.rs:82 |
| 6 | Wire Google Chat now that F6 OAuth2 is available: service-account creds, Pub/Sub subscription, `chat.spaces.messages.create` | High | M | unassigned | E5a was blocked on F6; F6 now landed |
| 7 | Wire Teams Bot Framework: Azure AD client-credentials token, register webhook, parse `Activity` JSON, POST via Graph | High | L | unassigned | E5b stubs at teams/channel.rs:88 |
| 8 | Fix `DiscordApiClient::edit_message` to actually sleep when rate-limited (parity with `create_message`) | Med | XS | unassigned | discord/api.rs:153-161 |
| 9 | Discord chunker upgrades: code-fence preservation across splits, Markdown emphasis re-balance, Nitro 4000-char detection, embed support, file-upload fallback | Med | M | unassigned | MEMORY.md mention; no tracker entry |
| 10 | Add `allow_from_match` metadata emission in Slack and Telegram channels for zero-trust promotion parity with Discord | Med | XS | unassigned | Behavior asymmetry, see open question 10 |
| 11 | C7: Migrate Telegram, Discord, Slack to `ChannelAdapter` trait; retire or formalize the shim | Med | L | unassigned | Element 04 / C7; affects every plugin host call site |
| 12 | Decide and implement slash-command surface: who calls `register_command`, where do registrations land, are they shared across channels? | Med | M | unassigned | Trait surface present, no consumer |
| 13 | Wire Axum auth middleware (D-7) once UI login page exists; gate every non-`/api/auth/token` and non-`/api/health` route | High | S (after UI) | unassigned | Today every gateway deployment is open |
| 14 | Add CSP middleware via `tower_http::set_header` (handlers.rs:130) | Med | S | unassigned | Web channel publishes user content to dashboard |
| 15 | Add per-endpoint rate limiting (handlers.rs:133): auth/token 5 rpm, delegation 60 rpm, monitoring 30 rpm | Med | S | unassigned | Choose `tower-governor` vs custom |
| 16 | Document or remove Telegram secondary 1 s poll-interval sleep | Low | XS | unassigned | open question 7 |
| 17 | Document Discord intents default bitmask 37377 and add coverage for `intents = 0` and privileged-intent rejection | Low | S | unassigned | open question 8 |
| 18 | Add a `slack.unknown_envelope` counter / metric so a Slack API drift is observable | Low | XS | unassigned | open question 9 |
| 19 | Resolve iMessage scope: implement `clawft-channels/src/imessage/` AppleScript bridge or remove from tracker | Low | M (impl) / XS (drop) | unassigned | orphaned in 04-element-06-tracker.md |
| 20 | Implement WeftOS white-label `brand()` token in kernel config, replace hard-coded strings in Discord identify, CLI help, WebUI header | Low | S | unassigned | feature-request-whitelabel.md |
| 21 | Voice channel: real STT capture in `start()`, real TTS playback in `send()`, agent pipeline `deliver_inbound` integration | High (for voice GA) | L | unassigned | step3-vs1.3-voice-channel.md "What's Still Stub" |
| 22 | Decide channel failover chain semantics (per-message? per-session? cross-channel quorum?) and either implement or close as out-of-scope | Low | XS (decision) / L (impl) | unassigned | MEMORY.md mention; no design doc |
| 23 | Document that `WebChannel::is_allowed` is permanently `true` and depends on auth middleware; gate behind D-7 enablement | Med | XS | unassigned | open question 5 |
| 24 | Add bridge stubs for skill install/uninstall, memory delete, and config persistence (api/bridge.rs:282/287/395/467) | Med | M | unassigned | gateway-side, cross-cutting |

## Sources

- `crates/clawft-channels/Cargo.toml`
- `crates/clawft-channels/src/lib.rs`
- `crates/clawft-channels/src/traits.rs`
- `crates/clawft-channels/src/host.rs`
- `crates/clawft-channels/src/plugin_host.rs`
- `crates/clawft-channels/src/discord/{mod,channel,api,events,factory}.rs`
- `crates/clawft-channels/src/slack/{mod,channel,api,events,factory,signature}.rs`
- `crates/clawft-channels/src/telegram/{mod,channel,client,types}.rs`
- `crates/clawft-channels/src/web/channel.rs`
- `crates/clawft-channels/src/email/channel.rs`
- `crates/clawft-channels/src/google_chat/channel.rs`
- `crates/clawft-channels/src/whatsapp/channel.rs`
- `crates/clawft-channels/src/signal/channel.rs`
- `crates/clawft-channels/src/matrix/channel.rs`
- `crates/clawft-channels/src/teams/channel.rs`
- `crates/clawft-channels/src/irc/channel.rs`
- `crates/clawft-services/src/api/{mod,handlers,auth,bridge,channels_api,broadcaster,ws,chat,memory_api,config_api,cron_api,voice_api,monitoring,delegation,skills}.rs`
- `crates/clawft-core/src/pipeline/permissions.rs`
- `crates/clawft-plugin-oauth2/src/{lib,types,token_store}.rs` (cross-ref ws04)
- `.planning/sparc/phase4/06-channel-enhancements/00-orchestrator.md`
- `.planning/sparc/phase4/06-channel-enhancements/01-phase-EFix-discord-heartbeat.md`
- `.planning/sparc/phase4/06-channel-enhancements/02-phase-EEnterprise-email-gchat-teams.md`
- `.planning/sparc/phase4/06-channel-enhancements/03-phase-EConsumer-whatsapp-signal-matrix.md`
- `.planning/sparc/phase4/06-channel-enhancements/04-element-06-tracker.md`
- `.planning/sparc/reviews/iteration-1-spec-05-06.md` (iMessage orphan)
- `.planning/development_notes/06-channel-enhancements/README.md`
- `.planning/development_notes/06-channel-enhancements/{e-fix,e-enterprise,e-consumer}/{decisions,blockers,difficult-tasks,notes}.md` (placeholders)
- `.planning/development_notes/05-axum-api-layer/decisions.md` (D-1 through D-10)
- `.planning/development_notes/social-media-integration.md`
- `.planning/development_notes/feature-request-whitelabel.md`
- `.planning/development_notes/step3-vs1.3-voice-channel.md`
- `.planning/development_notes/00-initial-sprint/phase1/wave2/telegram-plugin.md`
- MEMORY.md (project context: Discord chunking, failover chain mention)

<!-- TRIAGED-STAMP:BEGIN -->
## Triaged into Plane — 2026-04-28

All open items in this audit have been filed as Plane work items in the WeftOS workspace under the `ws05-channels` label.

- **Range**: WEFT-154 … WEFT-177 (24 items)
- **Per cycle**: 0.7.x: 15, 0.8.x: 7, 0.9.x: 2
- **Triage spec**: `.planning/reviews/0.7.0-release-gate/triage/`
- **WEFT-N → name map**: `.planning/reviews/0.7.0-release-gate/triage/weft-mapping.json`

Per the project rule (CLAUDE.md → "Plane is the authoritative work tracker"): future updates to these items happen in Plane, not in this audit doc. This doc remains the source-of-truth for the original survey.
<!-- TRIAGED-STAMP:END -->
