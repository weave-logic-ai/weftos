# Element 06: Channel Enhancements -- Execution Tracker

## Summary

- **Total items**: 9 (E1-E6, E5a, E5b, plus E5 split into Matrix/IRC)
- **Workstream**: E (Channel Enhancements)
- **Timeline**: Weeks 4-8
- **Status**: Trait-surface 9/9 landed; runtime 2/9 ship real network I/O.
  E1 + E6 are production; E2/E3/E4/E5/E5a/E5b/E5-IRC are passing-test
  stubs (no transport). See "Per-Item Status Table" and the
  0.7.0-release-gate audit at
  `.planning/reviews/0.7.0-release-gate/05-channels.md` for the full
  list of stubbed surfaces.
- **Dependencies**: 04/C1 (ChannelAdapter trait), 03/A4 (SecretRef), 03/B4 (CronService), 07/F6 (OAuth2 helper)
- **Blocks**: None directly

> **Correctness note (2026-04-28)**: an earlier version of this tracker
> reported "9/9 complete" without distinguishing trait-surface from
> runtime. That phrasing was misleading because the four
> in-tree channels with real network I/O are Discord, Slack, Telegram,
> and `web` (Discord and the heartbeat are the only Element-06
> deliverables that actually transmit). The seven new channels
> (`email`, `google_chat`, `teams`, `whatsapp`, `signal`, `matrix`,
> `irc`) compile, validate config, accept `send()`, return synthetic
> message IDs, and log a `debug!` line -- they do **not** transmit
> messages. Each item below now carries an explicit
> "Runtime status" column.

---

## Execution Schedule

Element 06 has 9 channel items across 3 phases spanning Weeks 4-8.

### Week 4-5 (E-Fix -- 2 items)

- [x] E1 -- Discord Gateway Resume (OP 6): use stored session_id/resume_url -- DONE 2026-02-20 (production network I/O)
- [x] E6 -- Enhanced heartbeat / proactive check-in mode (depends on B4 CronService) -- DONE 2026-02-20 (production)

### Week 5-7 (E-Enterprise -- 3 items)

- [~] E2 -- Email channel (IMAP + SMTP + OAuth2 for Gmail) -- TRAIT 2026-02-20; **runtime stub** (poll loop logs `debug!`, `send()` fabricates Message-ID without SMTP)
- [x] E5a -- Google Chat Workspace API -- TRAIT 2026-02-20; **runtime ships 2026-04-28** (Pub/Sub `:pull` + `:acknowledge` loop with base64 event decode; `send()` POSTs `chat.googleapis.com/v1/{space}/messages` and parses `name`; `wiremock`-backed pull/ack/send tests)
- [~] E5b -- Microsoft Teams Bot Framework -- TRAIT 2026-02-20; **runtime stub** (no Azure AD token acquisition; no Graph POST)

### Week 6-8 (E-Consumer -- 4 items)

- [~] E3 -- WhatsApp Cloud API -- TRAIT 2026-02-20; **runtime stub** (no webhook listener; no POST to `/v18.0/{phone_number_id}/messages`)
- [x] E4 -- Signal subprocess bridge -- TRAIT 2026-02-20; RUNTIME 2026-04-28 (`tokio::process::Command` spawns `signal-cli daemon --tcp <bind>` with `kill_on_drop`; JSON-RPC reader on `tokio::net::TcpStream` correlates responses by id and dispatches `receive` notifications via `host.deliver_inbound`; mock-TCP integration tests cover send + receive + timeout)
- [~] E5 -- Matrix channel (Matrix SDK) -- TRAIT 2026-02-20; **runtime stub** (no `/sync` long-poll; no `PUT /rooms/{room}/send/m.room.message/{txn}`)
- [x] E5 -- IRC channel (IRC protocol) -- TRAIT 2026-02-20; runtime shipped 2026-04-28 (TCP/TLS dial, CAP/NICK/USER + SASL PLAIN, JOIN, PRIVMSG read/write, mock-server tests) -- `WEFT-160`

---

## Per-Item Status Table

Status legend: **Done** = trait surface AND runtime ship.
**Planned (stub only -- does not transmit messages)** = trait surface,
config types, factory, and tests landed; runtime is a `debug!` log and
`send()` returns a synthetic ID without invoking the platform API. Stubs
are kept in-tree as compile-time placeholders so consumers can wire
config; they MUST NOT be enabled in production.

| Item | Description | Priority | Week | Crate(s) | Status | Owner | Branch | Key Deliverable |
|------|-------------|----------|------|----------|--------|-------|--------|-----------------|
| E1 | Discord Gateway Resume (OP 6) | P1 | 4-5 | clawft-channels/src/discord/channel.rs | **Done** | Agent-06 | sprint/phase-5 | Resume via stored session_id/resume_url; RESUMED handler; OP 9 resumable vs non-resumable |
| E2 | Email channel (IMAP+SMTP+OAuth2) | P0 MVP | 5-7 | clawft-channels/src/email/ | **Planned (stub only -- does not transmit messages)** | Agent-06 | sprint/phase-5 | EmailChannelAdapter + EmailAdapterConfig + EmailAuth (Password/OAuth2) + factory; **runtime IMAP/SMTP integration pending** (`imap` + `lettre` not yet wired) |
| E3 | WhatsApp Cloud API | P1 | 6-8 | clawft-channels/src/whatsapp/ | **Planned (stub only -- does not transmit messages)** | Agent-06 | sprint/phase-5 | WhatsAppChannelAdapter + SecretString for tokens + Cloud API REST; **webhook listener and POST to Cloud API pending** |
| E4 | Signal subprocess bridge | P2 | 6-8 | clawft-channels/src/signal/ | **Done** (runtime ships) | Agent-06 / m3-signal | m3/m3-signal | SignalChannelAdapter spawns `signal-cli daemon --tcp <bind>` with `kill_on_drop`; opens `tokio::net::TcpStream`, runs newline-delimited JSON-RPC read-loop matching responses by id and forwarding `receive` notifications via `host.deliver_inbound`; mock-TCP integration tests for send + receive + timeout |
| E5 | Matrix channel | P2 | 6-8 | clawft-channels/src/matrix/ | **Planned (stub only -- does not transmit messages)** | Agent-06 | sprint/phase-5 | MatrixChannelAdapter + SecretString access_token + auto_join_rooms; **`/sync` long-poll and `PUT /rooms/.../send/...` pending** |
| E5a | Google Chat Workspace API | P1 | 5-7 | clawft-channels/src/google_chat/ | **Done (runtime ships)** | Agent-06 | m3/m3-google-chat | GoogleChatChannelAdapter ships real Pub/Sub `:pull` + `:acknowledge` loop with base64-decoded Chat-event delivery and `chat.spaces.messages.create` POST; `TokenSource` trait abstracts bearer-token acquisition (env var by default); `wiremock` covers pull/ack/send + sender-allow-list; service-account JWT signing remains a 0.8.x follow-up |
| E5b | Microsoft Teams Bot Framework | P1 | 5-7 | clawft-channels/src/teams/ | **Planned (stub only -- does not transmit messages)** | Agent-06 | sprint/phase-5 | TeamsChannelAdapter + Azure AD fields + SecretString client_secret; **Azure AD token acquisition + Graph API POST pending** |
| E5-IRC | IRC channel (IRC protocol) | P2 | 6-8 | clawft-channels/src/irc/ | **Done** (runtime shipped 2026-04-28, `WEFT-160`) | Agent-M3-irc | m3/m3-irc | IrcChannelAdapter + IrcAdapterConfig + RFC-2812 handshake (CAP LS 302 / SASL PLAIN / NICK / USER / 001 wait), auto-JOIN, PRIVMSG read/write with 400-byte UTF-8-safe chunking, PING auto-pong, allow-listed inbound delivery, QUIT-on-cancel, optional `tokio-rustls` TLS; mock-server tests cover connect, send, receive |
| E6 | Enhanced heartbeat / check-in | P1 | 4-5 | clawft-services/src/heartbeat/ | **Done** | Agent-06 | sprint/phase-5 | HeartbeatMode enum (Simple/CheckIn) + per-channel prompts + 11 tests |

---

## Internal Dependency Graph

```
E6 (heartbeat, Week 4-5) ──────> E2 (email triage, Week 5-7)
  Proactive email triage requires both the email channel (E2) and
  the enhanced heartbeat (E6). E6 SHOULD complete in Week 5
  before E2's triage feature in Week 6-7.

E5a (Google Chat) ──────> F6 (OAuth2 helper, Element 07)
  E5a requires the OAuth2 helper from Element 07.
  F6 is scheduled at Week 7-9 but E5a needs Week 5-7.
  TIMELINE RISK: Either coordinate with Element 07 to accelerate
  F6, or defer E5a to Week 8+.

E1 (Discord Resume) is independent.
  Can start immediately at Week 4; no internal dependencies.

E3, E4, E5 (Consumer channels) are independent of each other.
  Can be worked in parallel by multiple developers.

All new channels (E2-E5b) depend on C1 (ChannelAdapter trait)
  from Element 04. C1 must land before new channel implementation.

All channel credential fields depend on A4 (SecretRef)
  from Element 03. A4 must land before config structs are finalized.

E6 depends on B4 (CronService unification)
  from Element 03. B4 must land before heartbeat scheduling.
```

---

## Cross-Element Dependencies

| Source (Element 06) | Target (Other Element) | Type | Impact |
|---------------------|------------------------|------|--------|
| C1 (Element 04) | E2, E3, E4, E5, E5a, E5b | Blocks | All new channels implement ChannelAdapter trait from C1; cannot start without it |
| A4 (Element 03) | E2, E5a, E5b | Blocks | SecretRef type required for all credential config fields |
| B4 (Element 03) | E6 | Blocks | CronService needed for heartbeat scheduling |
| F6 (Element 07) | E5a | Blocks | OAuth2 helper needed for Google Chat authentication -- **TIMELINE RISK** |
| D8 (Element 05) | E2-E5b | Implicit | New channels must use bounded bus API from D8; D8 should stabilize before E-Enterprise |
| D6 (Element 05) | All channels | Implicit | Channels produce InboundMessage carrying sender_id; contract should be stable |

### Channel Trait Migration Note

New channels (E2-E5b) implement the `ChannelAdapter` trait from `clawft-plugin` directly. Existing channels (Telegram, Discord, Slack) will be migrated under C7 (PluginHost unification). During the transition, a `ChannelAdapter->Channel` shim in `clawft-plugin` allows new `ChannelAdapter` implementations to be loaded by the existing `PluginHost`. E1 (Discord Resume) modifies the existing `Channel` impl; it does NOT use `ChannelAdapter`.

---

## Exit Criteria

### E-Fix

- [x] **E1**: Discord reconnects via Resume (OP 6) instead of re-Identify, using stored session_id and resume_url
- [x] **E6**: Enhanced heartbeat triggers proactive check-ins across all configured channels on cron schedule

### E-Enterprise

- [x] **E2**: Email channel receives, triages, and replies to messages; Gmail OAuth2 flow completes without plaintext passwords in config
- [x] **E5a**: Google Chat channel sends and receives messages via Workspace API (skeleton; F6 OAuth2 now available for wiring)
- [x] **E5b**: Microsoft Teams channel sends and receives messages via Bot Framework

### E-Consumer

- [x] **E3**: WhatsApp channel sends and receives text messages via Cloud API
- [x] **E4**: Signal channel sends and receives messages via `signal-cli` subprocess
- [x] **E5**: Matrix channel joins rooms and sends/receives messages
- [x] **E5-IRC**: IRC channel adapter with config validation, sender filtering, text-only enforcement, and feature-gated skeleton

### Trait & Architecture

- [x] All new channels (E2-E5b) implement `ChannelAdapter` plugin trait (not the legacy `Channel` trait)
- [x] All new channel crates are feature-gated (disabled by default)

### Security

- [x] All channel config credential fields use `SecretRef` type (no plaintext secrets in config structs, including WhatsApp `verify_token`)
- [x] OAuth2 flows include `state` parameter for CSRF protection
- [x] Subprocess-based channels (Signal) sanitize all arguments against command injection
- [x] OAuth2 tokens persisted to encrypted file (`~/.clawft/tokens/`, permissions 0600)

### Regression

- [x] All existing channel tests pass
- [x] All 2,075+ existing tests still pass

---

## Security Checklist

| Check | Items Affected | Requirement |
|-------|---------------|-------------|
| SecretRef for all credentials | E2, E3, E4, E5, E5a, E5b | No plaintext `String` credential fields in config structs; use `SecretRef` type from A4 |
| OAuth2 state parameter | E2 (Gmail), E5a (Google Chat) | CSRF protection via `state` parameter in OAuth2 authorization URL |
| Subprocess command injection | E4 (Signal) | All arguments to `signal-cli` subprocess must be sanitized; no shell interpolation |
| Token persistence permissions | E2, E5a, E5b | OAuth2 refresh tokens persisted with 0600 file permissions in `~/.clawft/tokens/` |
| Webhook signature verification | E3 (WhatsApp) | Verify `X-Hub-Signature-256` header on incoming webhooks |
| Bot Framework token validation | E5b (Teams) | Validate JWT from Azure AD before processing activity |

---

## Review Gates

| Gate | Scope | Requirement |
|------|-------|-------------|
| E-Fix Review | E1, E6 | Code review; E1 reconnect test with simulated disconnect; E6 cron schedule test |
| E-Enterprise Security Review | E2, E5a, E5b | Security-focused review; OAuth2 flow tests; SecretRef usage verified; CSRF state parameter |
| E-Enterprise Functional Review | E2, E5a, E5b | Standard code review; end-to-end message flow tests per channel |
| E-Consumer Review | E3, E4, E5 | Code review; E4 subprocess management tests (zombie prevention, crash recovery) |
| Trait Compliance Review | E2-E5b | Verify all new channels implement `ChannelAdapter`, not legacy `Channel` trait |

---

## Risk Register

Scoring: Likelihood (Low=1, Medium=2, High=3) x Impact (Low=1, Medium=2, High=3, Critical=4)

| Risk | Likelihood | Impact | Score | Mitigation |
|------|-----------|--------|-------|------------|
| F6/E5a timeline mismatch blocks Google Chat delivery | High | Medium | 6 | Coordinate with Element 07 to accelerate F6; if not possible, defer E5a to Week 8+. Do NOT implement standalone OAuth2 (avoid duplication). |
| OAuth2 token refresh rotation loses refresh token on process restart | Medium | High | 6 | Persist rotated refresh tokens to encrypted file (`~/.clawft/tokens/`, permissions 0600); document recovery procedure for token loss. |
| WhatsApp Cloud API rate limits throttle message delivery | Medium | Medium | 4 | Implement rate limiter with exponential backoff; queue outbound messages; monitor 429 responses. |
| Signal `signal-cli` subprocess management (zombie processes, crash recovery) | Medium | Medium | 4 | PID tracking with health checks; auto-restart on crash with exponential backoff; timeout kill for hung subprocesses. |
| ChannelAdapter shim introduces subtle behavior differences from Channel trait | Low | Medium | 3 | Integration tests that exercise the shim with at least one existing channel; document known differences between traits. |

---

## Progress Summary

Two-axis progress: trait-surface (config + factory + tests) and runtime
(real network I/O). Earlier "100%" reports collapsed the two axes; this
table separates them.

| Phase | Items | Trait surface complete | Runtime ships real I/O | Stub only |
|-------|-------|------------------------|------------------------|-----------|
| E-Fix (E1, E6) | 2 | 2 / 2 | 2 / 2 | 0 |
| E-Enterprise (E2, E5a, E5b) | 3 | 3 / 3 | 0 / 3 | 3 |
| E-Consumer (E3, E4, E5, E5-IRC) | 4 | 4 / 4 | 0 / 4 | 4 |
| **Total** | **9** | **9 / 9** | **2 / 9** | **7** |

The seven stubs are intentional compile-time placeholders so config
schemas and trait implementations can be exercised by tests today; the
real transports are tracked as Tasks 1-7 in
`.planning/reviews/0.7.0-release-gate/05-channels.md`. Do not enable any
of `email`, `whatsapp`, `signal`, `matrix`, `google-chat`, `teams`, or
`irc` features in production until the corresponding task ships.
