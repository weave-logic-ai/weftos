# ADR-057: Substrate per-path read ACLs as a MUST-HAVE gate

**Date**: 2026-05-12
**Status**: Accepted — MUST-HAVE for 0.8.x (subscriber-node landing cycle)
**Deciders**: Main-thread decision 2026-05-12 (watch-as-Actor discussion)
**Depends-On**: ADR-025 (Ed25519 node identity), ADR-022 (ExoChain mandatory audit)
**Supersedes**: nothing — fills a gap in the read-gate model that the capability layer (`crates/clawft-weave/src/capability.rs`) leaves open

## Context

The daemon's JSON-RPC dispatcher gates verbs by a four-class capability
model: `Read`, `Chat`, `Write`, `Admin`
(`crates/clawft-weave/src/capability.rs`). All three substrate read verbs
— `substrate.read`, `substrate.list`, and `substrate.subscribe` — are
classified as `Capability::Read`, and the anonymous baseline grants
`Read` by default. Net effect today: **any caller that can open the
daemon's IPC channel can read or subscribe to *every* path in the
substrate**, including:

- `substrate/<node-id>/sensor/mic/pcm_chunk` — raw mic audio from a
  journaled ESP32 (`.planning/sensors/JOURNALED-NODE-ESP32.md` §3.1).
- `substrate/<node-id>/sensor/imu/samples` — wearable motion data.
- `substrate/<actor-id>/...` — Actor-private state if we ever colocate
  Actors in the substrate as proposed in the JOURNALED-NODE doc §8.4.
- `substrate/<mesh-id>/cluster/nodes/<node-id>` — the pubkey directory
  (less sensitive, but exposes the mesh topology to any subscriber).

This was fine when the substrate's only readers were the daemon's own
egui shell on the same host. It is **not** fine the moment we admit
subscriber-only nodes onto the mesh — the watch
(`infinition/waveshare-watch-rs` family, see Hive Mind session
2026-05-12), the Elecrow HMI display, future Tidbyt-replacement display
nodes, or any third-party Actor that joins via the existing capability
token flow. The write side already has a per-path gate (the publish
prefix `substrate/<publisher-node-id>/` is enforced by signature check
per ADR-025 + JOURNALED-NODE-ESP32.md §3.5). The read side currently has
no equivalent.

The trigger for writing this down: the watch is a confirmed Actor (not
just a Node — main-thread decision 2026-05-12). An Actor that can sign
Actions on tap MUST also have a constrained read scope; otherwise any
compromised display device on the mesh is a full mesh wiretap.

## Decision

**Substrate read RPCs MUST enforce a per-path Access Control List (ACL)
in addition to the existing capability-class gate.** This is binding
for 0.8.x and is a release-blocker for any mesh feature that admits
remote subscribers.

### What the read gate checks

For every call to `substrate.read`, `substrate.list`, or
`substrate.subscribe`, the daemon MUST:

1. Resolve the caller identity (Node or Actor) from the `auth` field on
   the JSON-RPC envelope. Anonymous local UDS callers retain their
   anonymous baseline only for paths explicitly marked `public` (see
   §"Default ACL" below).
2. Look up the requested path against the substrate ACL table.
3. Allow the call iff the caller's identity satisfies at least one
   `allow` rule for that path and no `deny` rule matches. Unknown paths
   resolve to the default ACL for their first path segment (deny-by-
   default for sensor/actor private subtrees).
4. Reject with a typed `acl_denied` error that is **distinguishable
   from "path not found"**, so clients can't probe for path existence
   by error-shape comparison.
5. Record the denial on the ExoChain (per ADR-022) as a
   `substrate.read.denied` event when the caller is authenticated; rate-
   limit-summarize when the caller is anonymous.

### ACL data model

ACLs are stored under the Mesh-owned subtree, not under any single
Node, so they survive a node rejoining with a fresh keypair:

```
substrate/<mesh-id>/acl/<path-glob>
  { allow: ["node:n-<id>" | "actor:a-<id>" | "scope:<name>" | "public"],
    deny:  ["node:n-<id>" | "actor:a-<id>"],
    inherit: bool,
    sealed_by: "<actor-id>",   // who last wrote this rule
    sealed_at: <unix-ms> }
```

- **Path globs**: support trailing `**` for subtree wildcards and a
  single `*` for one-segment wildcards. No regex.
- **Identity strings**: `node:n-<6-hex>`, `actor:a-<6-hex>`, or
  `scope:<name>` for capability-token scopes. The literal `public`
  matches anonymous callers.
- **`inherit`**: when `true`, child paths without their own ACL inherit
  this rule. When `false`, child paths must declare their own ACL or
  fall to the default.

### Default ACL (deny-by-default for private subtrees)

The kernel boot-time ACL table MUST seal in:

| Path glob | Allow | Deny | Notes |
|---|---|---|---|
| `substrate/<mesh-id>/cluster/nodes/**` | `public` | — | pubkey directory is readable — needed for sig verify |
| `substrate/<mesh-id>/cluster/health/**` | `public` | — | overall mesh status; no per-node detail leaks |
| `substrate/<node-id>/meta` | `public` | — | hardware/firmware descriptor is intentionally public |
| `substrate/<node-id>/health` | `public` | — | aggregate health is public; per-sensor health is not |
| `substrate/<node-id>/sensor/**` | `node:<node-id>`, `scope:admin` | — | **private by default — the owning node MUST opt-in** |
| `substrate/<node-id>/health/sensor/**` | `node:<node-id>`, `scope:admin` | — | sensor health can leak presence-of-activity |
| `substrate/<actor-id>/**` | `actor:<actor-id>`, `scope:admin` | — | Actor state is private |
| `substrate/<mesh-id>/chain/**` | `public` | — | the audit chain is public-readable by design |

The substrate write gate (already enforced via ADR-025) is reused as
the write gate for the ACL table itself: only the mesh's bootstrap
Actor (typically the daemon's own identity, see JOURNALED-NODE-ESP32.md
§8.5) may write `substrate/<mesh-id>/acl/**`.

### Opt-in publishing helpers

To make the default-deny posture ergonomic, nodes get a single helper:

- `Substrate::publish_public(path, value)` — writes the value AND
  ensures an `allow: ["public"]` rule exists on that exact path.

This is the only way a node opts a path *out* of the default-private
posture for its `sensor/**` subtree. The helper is signed by the node's
key like any other publish.

## Consequences

### Positive

- Closes the wiretap-by-default hole that exists today. The mic audio
  example alone is a privacy gate.
- Lets Actor-capable subscriber nodes (the watch, future HMI displays)
  exist on the mesh without exposing every other node's raw sensor
  stream to them.
- Composes cleanly with ADR-022 (ExoChain audit) — denials are
  chain-logged, so policy violations are forensically traceable.
- The chain itself stays public, so the audit guarantee from ADR-022
  is unaffected.

### Negative

- Adds a per-request ACL lookup to `substrate.read`/`list`/`subscribe`.
  Implementation must use a path-trie keyed structure to keep the
  lookup O(depth) rather than O(rules); a naive linear scan is a
  release-blocker performance bug.
- Existing internal callers (the egui shell, `clawft-service-whisper`,
  the explorer's tree-browse) must be re-audited. Most run under the
  daemon's own identity, which gets `scope:admin` and is unaffected,
  but any path that crosses an Actor identity boundary needs an
  explicit allow rule.
- The `inherit: bool` semantics are easy to get wrong. Tests must
  cover the "child overrides parent" and "deny-leaf under allow-tree"
  cases.
- The ACL table itself is a new substrate subtree that needs UI
  representation in the explorer; otherwise it's an invisible policy
  surface.

### Neutral

- Per-path ACLs are a *complement* to, not a replacement for, the
  existing capability-class gate. A caller must still hold
  `Capability::Read` AND satisfy the path ACL. This means lowering
  `Capability::Read` from the anonymous baseline (a possible future
  tightening) remains an independent decision.
- The `acl_denied`-vs-`not_found` error distinction leaks one bit of
  information per probe (does the path exist?). We accept that
  deliberately: silent same-error responses would prevent operators
  from diagnosing legitimate access failures. The chain log on
  denials closes the abuse loop.

## MUST-HAVE acceptance criteria for 0.8.x

- [ ] `Substrate` traits expose `read`/`list`/`subscribe` variants that
      take a caller-identity argument.
- [ ] Daemon dispatch threads caller identity into substrate reads
      (not just writes).
- [ ] ACL table data type defined in `crates/clawft-substrate` with
      path-trie lookup.
- [ ] Boot-time defaults seeded per the table above.
- [ ] `acl_denied` error type wired through the IPC error envelope.
- [ ] ExoChain emits `substrate.read.denied` events for authenticated
      denials.
- [ ] `publish_public` helper available on the no_std substrate client
      so ESP32 nodes can opt in from firmware.
- [ ] Explorer's substrate tree-browse handles `acl_denied` gracefully
      (renders as a lock icon, not a crash).
- [ ] Integration test: a subscriber Actor with no allow rule cannot
      see `sensor/mic/pcm_chunk` even though it can see `meta`.

## Open questions deferred (not blockers for the MUST-HAVE)

1. **Per-topic vs per-path granularity inside a single subtree.** The
   model above is path-glob based. The `substrate.subscribe` protocol
   already speaks in topics, which may not map 1:1 to substrate paths.
   First implementation: topic = path. If topic-graph fanout shows up
   later, extend the ACL identity to allow `topic:<id>` rules.
2. **Group / role identities.** Above we use raw `node:` and `actor:`
   identities. Once the Actor pipeline lands, we'll likely want
   role-based rules (`role:operator`, `role:read-only`). The ACL data
   model already accepts arbitrary identity strings, so this is
   forward-compatible.
3. **ACL rule rotation / TTL.** No expiry semantics in v1. If a
   rule needs to expire, the operator deletes it. TTL is a follow-up.
4. **Federation across meshes.** When meshes link (ADOPTION.md), the
   ACL identity strings will need a `mesh:<id>:` prefix. Reserved but
   not implemented.
