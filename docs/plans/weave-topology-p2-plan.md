# Weave topology Phase 2: file-level implementation plan

> **Decisions (2026-10-01):** the owner accepted all seven open decisions as recommended (ADR-103 A6).

Status: plan, not started. Implements Phase 2 of [ADR-103](../adr/adr-103-weave-topology-roles-and-instances.md) (card weave-topology-P2): per-project kernels as children of the user daemon. Analysis: [analysis.md](../research/daemon-topology/analysis.md) sections 4-7, 10.3-10.4. Audit inputs: [cog-boundary/audit.md](../research/cog-boundary/audit.md) M1, M3, M12. Date: 2026-09-30.

Assumes Phase 1 is integrated (`integrate/p1`: `RuntimePaths`/`RootSource::{Env,Project,User,LegacyHome}`, project store in `clawft-types/src/project/`, `kernel.handshake`/`resolve.rs`/`connect_resolved` in `clawft-rpc`, `rpc_ext.rs` `ROUTES`/`GATES`/`CallerCtx`/`ClaimedProject`, `scope_gate.rs`, `user_daemon.rs`, `chain_storage.rs`, `token_authority.rs`). File paths below are relative to `crates/`; read the real names first. Phase 3 (machine mesh service, peer credentials) and Phase 4 (sandbox drivers) are out of scope.

## 0. Findings that shape the work

1. **Reuse the cog runtime's process supervisor, not `SpawnBackend`.** `SpawnBackend` (`clawft-kernel/src/supervisor.rs:~298`) spawns *agents*; `Remote/Container/Wasm` are stubs. The code that actually launches and kills OS processes is `workload_runtime/supervise.rs` (`LaunchSpec`, `Supervised::spawn` with its own process group, `terminate(grace)`, `try_exit`). Restart policy types exist in `supervisor.rs`: `RestartStrategy::OneForOne`, `RestartBudget {max_restarts:5, within_secs:60}`, `RestartTracker` (backoff 100 ms to 30 s), all behind kernel feature `os-patterns`; package G must confirm `clawft-weave` builds the kernel with it.
2. **`cog` is hard-coded in two places** (audit section 5): `workload_pkg/manifest.rs:143` (`cog_body`) and `workload_ctl/plane_place.rs:183` (`prepare` always calls `cog_workload_spec`). `VerifiedWorkload.source` is `SignedPackage(SignedPayload{spec: CogSpec})` or `StorePin`, both cog-shaped. A `project` kind needs a third source (verified by certificate, not package signature).
3. **`GatePrincipal`** (`clawft-kernel/src/governance.rs:617`) has `agent_id, user_id, parent_agent_id, conv_id`; it already rides `GovernanceRequest.principal` and `chain.rs:2723` audit payloads, so adding `project_id` puts it on the chain for free. `ClaimedProject` (`rpc_ext.rs`) is explicitly unverified today.
4. **`ChainEvent` has no rule-hash field** (`chain.rs:139`); `compute_event_hash` (`chain.rs:283`) covers all fields; `idempotency_key` is the precedent for an optional field kept out of the hash. D8 needs the hash covered by the signature, so the new field is hashed only when present (old chains verify unchanged).
5. **Chain signing:** `ChainManager::signing_key_clone()` and `verifying_key()` exist (`chain.rs:1157,1173`); no new signing accessor is needed. `ChainManager::load_or_create_key` (`chain.rs:1188`) is the key-file format to reuse.
6. **Heavy services are built inline in the daemon** (`weave/src/daemon.rs`: embedder ~890, `DAEMON_LLM`, `DAEMON_TALK_LOOP`/`DAEMON_VOICE_LOOP` ~1550-2400, voice consumer ~2404). The mesh listener is `kernel.mesh.enabled`. No `mesh-local` code exists anywhere (grep); Phase 2 defines the message and the user-daemon side only.
7. `daemon.rs` is ~9.6k lines and the contention point. Package A adds three one-line seams (section 3) so no later package edits it more than once.

## 1. Contracts every package codes against (fixed here, land in A)

**Layout.** User-daemon-owned ephemeral state: `~/.weftos/run/<id>/` = `kernel.sock kernel.pid kernel.lock kernel.log spawn.json parent-policy.json state.json`. Project-owned durable state (moves with the project, gitignored): `<root>/.weftos/project.key` (0600), `project.cert.json`, `chain/{chain.json,chain.rvf,chain.tree.json,anchors.jsonl}`, `state/{workloads.json,apps.json}`, and the committed `<root>/.weftos/overlay.toml`. New `RootSource::Child { id, project_root }`; `RuntimePaths::child_with(home, id, root)` maps the ephemeral files to the run dir, `chain_*()` to `<root>/.weftos/chain/`, `node_key()` and `chain_key()` both to `project.key`. The walk-up never applies to a child.

**One project key (recommended, decision 1).** `project.key` is the node key, chain signing key and anchor/cert-PoP key. Therefore `node_id == project_key_id == hex(SHA-256(pubkey)[..16])` (D11) and the handshake `node_id` of a child equals its `project_key_id`. Every signature uses a domain tag (`weftos-project-cert-v1`, `weftos-project-anchor-v1`, `weftos-mesh-local-pop-v1`, chain events keep their existing canonical form) so one key does not sign confusable bytes.

**Project certificate (D7)**, `~/.weftos/projects/<id>.cert.json`, mirrored to `<root>/.weftos/project.cert.json`:
```json
{"v":1,"type":"project-cert","project_id":"01JB8Z3Q0V6X9KQ4M2N7T5R1WD",
 "project_pubkey":"<hex32>","project_key_id":"<hex32=sha256[..16]>",
 "user_key_id":"<hex32>","user_pubkey":"<hex32>","serial":1,
 "issued_at":"2026-10-01T09:30:00Z","expires_at":null,
 "sig":"<hex64 Ed25519 by user key>"}
```
Signed bytes: `"weftos-project-cert-v1\n" + canonical JSON (sorted keys, no sig)`. No root path inside (projects move). The user key is the chain key per Phase 1 D-1. `expires_at` null in P2; the verifier honours it.

**`project.register` (user chain, source `user.projects`):** `{cert, name, root_sha256, manifest_schema, spawn:{pid,exe_sha}}`. `project.rekey` and `project.revoke` carry `{project_id, old_key_id, new_cert?, reason}`. First key seen for a project id is certified (TOFU against the manifest, which is the owner's registration act); a different key for the same id is refused until an explicit `project.rekey` (Admin).

**`project.anchor` (user chain, source `project.anchor`):** the project signs a statement, the user daemon verifies and appends it:
```json
{"project_id":"..","project_key_id":"..","cert_serial":1,"seq":17,
 "chain_id":0,"head_hash":"<hex32>","head_seq":4210,"rule_hash":"<hex32>",
 "at":"2026-10-01T10:00:00Z","prev_anchor":"<hex32|null>","sig":"<hex64>"}
```
Signed bytes `"weftos-project-anchor-v1\n"+canonical JSON`. User daemon checks: cert valid and not revoked, signature, `seq` is last+1, `at` not more than 5 minutes ahead, `prev_anchor` equals the hash of the last accepted statement. It returns `{user_seq, user_event_hash}`; the project appends `project.anchored` to its own chain (two-way link). Honest limit, stated in code and docs: the user daemon attests "this key claimed head X at time T"; it cannot verify X without subscribing to the project chain.

**Overlay (D8)**, `<root>/.weftos/overlay.toml`:
```toml
schema = 1
[[deny]]                       # add denies (action globs, prefix with trailing *)
id = "project.no-shell"
actions = ["tool.shell_exec", "workload.place*"]
reason = "this project never runs shell tools"
[[require_approval]]           # tighten: force human approval
actions = ["workload.start*"]
[limits]                       # numbers may only go down, flags only up
risk_threshold = 0.5           # effective = min(parent, project)
max_processes = 32
spawn_budget = 4
human_approval_required = true # false->true only
```
Merge rule, enforced at load, hard error naming the key: `deny` and `require_approval` are unioned; every numeric limit is `min(parent, overlay)`; booleans are `parent OR overlay`; an overlay may not contain `permit`, `deactivate`, a rule id that exists in the parent (no shadowing), or a limit above the parent value. Parent rules arrive in `parent-policy.json` written by the user daemon: `{schema, user_key_id, version, issued_at, rules:[GovernanceRule..], limits, rule_hash, sig}` signed by the user key and verified by the child against `user_pubkey` from its certificate. `effective_hash = SHA-256("weftos-effective-rules-v1\n" + canonical JSON of {parent_hash, overlay_hash, merged rules sorted by id, merged limits})`. `ChainEvent.rule_hash: Option<[u8;32]>` carries it on every event of the child (and the user daemon's own parent hash on the user chain).

**mesh-local/1 (JSON lines over the user daemon socket, `clawft-rpc/src/mesh_local.rs`):**
```json
// child -> user daemon: method "mesh.register"
{"protocol":"mesh-local/1","role":"project","project_id":"..","project_pubkey":"<hex32>",
 "cert":{..}|null,"addresses":["<project_id>"],"topic_prefixes":["chain/<project_id>/"],
 "version":"0.8.2","build_sha":"..","pid":4242,"socket":"/Users/x/.weftos/run/<id>/kernel.sock",
 "features":["anchor","subscribe"],"nonce_reply":{"nonce":"<hex>","sig":"<hex64 PoP>"}}
// ack
{"ok":true,"session":"<ulid>","cert":{..},"accepted":["<project_id>"],
 "proto":{"current":1,"min":1},"heartbeat_secs":15,"machine_cert":null}
```
Flow: `mesh.challenge{project_id}` returns a nonce, then `mesh.register` carries the PoP over `weftos-mesh-local-pop-v1\n<nonce>\n<project_id>` plus the spawn nonce from `spawn.json`. `mesh.heartbeat{session, activity:{last_activity_unix,busy:{agents,workloads,streams}}}` and `mesh.unregister{session,reason}` complete it. Phase 3 re-registers the same message upstream to the machine service and fills `machine_cert`. Peer-credential binding (D3) is Phase 3: until then the guard is the 0600 socket plus spawn nonce plus PoP, and the docs say so.

**Manifest additions (`clawft-types/src/project/schema.rs`, `ServeSection`):** `via = "child-kernel"` (the enum value already reserved), `idle_stop_secs` (u64, 0 = never), `restart_max` (default 5), `restart_window_secs` (default 60), `kernel_version`/`kernel_sha` written by the supervisor. State machine in `state.json`: `stopped|starting|running|idle-stopping|failed`.

**`GatePrincipal` (M12):** add `project_id: Option<String>` and `instance_id: Option<String>` (node id of the evaluating kernel), `#[serde(default, skip_serializing_if)]`, builder `with_project(&VerifiedProject)`. `VerifiedProject` is a newtype in `rpc_ext.rs` whose constructors are private to three sources: (a) the kernel's own bound project (the socket you reached is the project's own, so a child's handshake `project_id` is verified by construction), (b) a validated token whose `TokenScope` carries `project_id` (P1 package F), (c) a user-daemon-forwarded request carrying a user-signed forward header (see package I). A bare `Request.project` on the user daemon's socket stays `ClaimedProject`, is recorded as `claimed_project` in audit context, and is never used for an allow decision beyond P1's registry check.

## 2. Work packages

Owner column = files the package may edit. "Shared" files are in section 3. Every package runs `scripts/build.sh check`, `clippy`, `test` in its worktree; G, H run `scripts/build.sh gate`. Worktree branches `wt/p2-<letter>-<name>` off `integrate/p1`.

### A. contracts (types, paths, seams) - first, small
Depends on: P1 integrated. Blocks: all.
- `clawft-types/src/runtime_paths.rs`: `RootSource::Child`, `RuntimePaths::child_with`, accessors (`spawn_json()`, `parent_policy()`, `state_json()`, `project_key()`, `overlay()`); tests for the layout table above and "walk-up never selects a child".
- new `clawft-types/src/project/cert.rs`: `ProjectCert`, `canonical_bytes()`, `key_id()`, `ProjectAnchorStmt` (no crypto dependency; hex strings). `project/schema.rs`: `ServeSection` fields. new `clawft-types/src/config/overlay.rs`: `OverlayFile` serde types only. `config/kernel.rs`: `profile = "project"`, `kernel.shared_services = {embeddings,llm,voice} = "parent"`.
- new `clawft-rpc/src/mesh_local.rs`: the message structs, `PROTO_MESH_LOCAL = 1`.
- `clawft-kernel/src/governance.rs`: `GatePrincipal.project_id/instance_id` + `with_project`. `weave/src/rpc_ext.rs`: `VerifiedProject` newtype (private constructors, no callers yet).
- `weave/src/daemon.rs`, three seam calls only, bodies in new `weave/src/project_hooks.rs` as no-ops: `project_hooks::pre_boot(&mut Config) -> PreBoot` at the top of `run()` (~line 973), `project_hooks::adjust_services(&mut Config)` before the embedder at ~890, `project_hooks::post_boot(&kernel)` after boot.
- `docs/adr/adr-103-...md`: amendment A6 recording one project key, overlay merge rule, anchor statement, `Child` root, mesh-local/1 shape.
Tests: serde round trips with and without new fields; `GatePrincipal` old JSON parses; canonical bytes golden vectors (published in the test file so other packages can match them).
Acceptance: crate tests for types, rpc, kernel `governance`; `check`, `clippy`.

### B. workload-kind registry (M3)
Depends on: none (may start now). Blocks: G. Files: kernel only.
- new `clawft-kernel/src/workload_kind/{mod.rs,cog.rs}`: `trait WorkloadKind { fn id(&self)->&'static str; fn validate(&self,&ManifestEnvelope)->Result<(),ManifestError>; fn prepare(&self,&PlaceOrder,&Anchors)->Result<(VerifiedWorkload,WorkloadSpec,String),PlaneError>; fn adapters(&self)->&'static [&'static str]; fn health(&self,&InstanceStatus)->Health }`, `KindRegistry {register, get}` with `KindRegistry::builtin()` containing `cog`; unknown kind is `PlaneError::UnknownKind`, never a panic. `cog.rs` moves `cog_workload_spec` and the `verify_dir` call behind the trait with no behaviour change.
- `workload_ctl/plane_place.rs`: `prepare` becomes `registry.get(order.kind)?.prepare(..)`; `PlaceOrder.kind: Option<String>` (default `cog`, serde default). `workload_pkg/manifest.rs:143`: `cog_body` stays, called only by `cog.rs`. `workload_runtime/types.rs`: add `WorkloadSource::Project(ProjectPayload{project_id, key_id, cert_serial, user_key_id, policy_hash})` (type only; G fills it).
- `workload_governance`: no change; a `project` kind needs a permit rule, which G installs (default-deny stays).
Tests: existing workload suites unchanged and green (the regression proof); a test-only `echo` kind registered at runtime places and starts through the same path; an unregistered kind is refused and chained as `workload.refuse`; registry rejects duplicate ids.
Acceptance: `scripts/build.sh test` (all `workload_*` tests), `clippy`, `check`.

### C. project identity (key, certificate, register/rekey/revoke)
Depends on: A. Blocks: D, G, H, I.
- new `clawft-kernel/src/project_identity.rs`: `load_or_create_project_key(path)` (0600, refuse group/world-readable, reuses `ChainManager::load_or_create_key` format), `sign_cert(user_signing_key, &CertRequest)->ProjectCert`, `verify_cert(&ProjectCert, trusted_user_pubkey)->Result<(),CertError>` (checks signature, `key_id` recomputed from pubkey, `expires_at`), `pop_sign/pop_verify(nonce, project_id)`, `RevocationView` over user-chain `project.revoke` events.
- new `weave/src/project_cert_rpc.rs` (registered by one `ROUTES` line, capability `Admin`, never reachable from the TCP relay): `project.cert.show`, `project.rekey`, `project.revoke`; shared function `issue_for_register(ctx, &RegisterRequest)` used by package H's `mesh.register` handler. Appends `project.register|rekey|revoke` to the user chain; writes `~/.weftos/projects/<id>.cert.json` atomically (0600).
- `clawft-kernel/src/lib.rs` `pub mod` (Shared).
Tests: golden cert vectors from A; tampering each field fails; wrong user key fails; `key_id` mismatch fails; TOFU (second key for an id refused, `rekey` accepts, old cert then fails the revoke view); key file with 0644 refused; chain contains `project.register` with no private bytes (grep the serialised events); metamorphic: issuing twice with the same input gives byte-identical signed bytes.
Acceptance: crate tests, `weave/tests/project_cert_rpc.rs`, clippy, check.

### D. project chain, anchors, `rule_hash`, chain subscription (M1)
Depends on: A (C for cert verification in the anchor RPC). Blocks: E, G, H.
- `clawft-kernel/src/chain.rs` (Shared, regions: `ChainEvent`, `compute_event_hash`, a new `subscribe`): `ChainEvent.rule_hash: Option<[u8;32]>`, serde default, included in `compute_event_hash` only when `Some`; `ChainManager::set_rule_hash_provider(Arc<dyn Fn()->Option<[u8;32]>+Send+Sync>)` stamps every append; `ChainManager::subscribe(filter: ChainFilter{kind_prefix, from_seq})->ChainSubscription` over a bounded `tokio::sync::broadcast` plus a replay-from-seq catch-up that never loses an event between replay and live (subscribe first, then replay, dedupe by sequence). Slow subscribers get a `Lagged` marker, never block the append path.
- `clawft-kernel/src/chain_anchor.rs`: new backend `ChainAnchorBackend::Parent` implementing the existing `ChainAnchor` trait: builds and signs the anchor statement with the project key, sends it to the user daemon (`project.anchor.submit`), on success appends `project.anchored`, on failure appends to `<run>/anchor.pending.jsonl` and retries with backoff; only the latest pending statement is replayed (a later head covers earlier ones, `seq` still increments per accepted statement). Defaults: at most one per 300 s and at least 100 new events, plus one on graceful shutdown. `clawft-types/src/config/kernel.rs` `ChainAnchorBackend` variant (Shared with A).
- new `weave/src/anchor_rpc.rs`: `project.anchor.submit` (project-key authenticated, no capability token), verification list in section 1, appends the user-chain `project.anchor` event; `chain.subscribe` streaming intercept in `daemon.rs` is **not** added there: new streaming handler registered through the existing intercept list pattern used by `kernel.logs_stream`, one line (Shared). `chain.subscribe{chain:"user"|"project/<id>", kind_prefix, from_seq}` requires `Read` and, for `project/<id>`, a `VerifiedProject` match or Admin (ADR-057-style ACL); on the user daemon `project/<id>` is a proxied subscription to that child's socket.
- `weave/src/commands/` none.
Tests: golden hash vectors: events without `rule_hash` hash exactly as before (load a pre-P2 fixture chain, `verify_integrity` green); with it, flip one byte of the field and verification fails; subscribe/replay race (100 iterations with concurrent appends, no gap and no duplicate); backpressure (stalled subscriber does not slow `append`); anchor RPC refuses bad signature, replayed `seq`, skipped `seq`, forged `prev_anchor`, revoked cert, future `at`; user daemon down: pending file written, replayed once on return, project keeps appending.
Acceptance: `scripts/build.sh test` (kernel `chain*`, weave `anchor_rpc`), clippy, check. Reviewer: the exochain specialist reads the `chain.rs` diff.

### E. governance overlay (D8)
Depends on: A, D (`rule_hash` provider). Blocks: G.
- new `clawft-kernel/src/governance_overlay.rs`: `load_overlay(path)->Result<Overlay,OverlayError>`, `merge(parent:&ParentPolicy, &Overlay)->Effective{rules, limits, parent_hash, overlay_hash, effective_hash}` implementing the rule in section 1 (reject-by-key errors; no silent clamp), `Effective::into_rules()` for `GovernanceEngine::add_rule`, `verify_parent_policy(&ParentPolicy, user_pubkey)`.
- new `clawft-kernel/src/parent_policy.rs`: user-side `export(engine, limits, signing_key)->ParentPolicy`, atomic write to `<run>/<id>/parent-policy.json`, `version` counter.
- `clawft-kernel/src/boot.rs` (Shared, one guarded block near the `.chain(workload_governance::default_rules())` at ~1769): when the profile is `project`, replace the default rule set with the effective rules and register the rule-hash provider on the `ChainManager`; on overlay or signature failure boot fails (`BootError::Overlay(key, reason)`), decision 2.
- new `weave/src/governance_push.rs`: user-daemon method `governance.parent.push` (child to parent direction is none; the user daemon calls the child's `governance.parent.update`, Admin, accepted only with a valid user signature) so a rule change reaches running children; child appends `governance.overlay.applied {parent_hash, overlay_hash, effective_hash}`.
Tests: table test of the merge (each of: add deny ok; lower limit ok; raise limit, `permit`, shadow a parent id, deactivate a parent rule, malformed glob all rejected with the offending key); property test: for random parent and overlay, effective deny set is a superset of the parent's and every effective limit is `<=` the parent's; tampered parent policy refused; overlay file edited while running does nothing until `governance.reload` (explicit); `rule_hash` present on every event after boot and changes after an applied update; a denied action under the overlay returns `Deny` and is chained with the hash.
Acceptance: crate tests, clippy, check.

### F. shared services via the user daemon
Depends on: A. Blocks: G.
- new `weave/src/project_profile.rs` (hook bodies from A): `adjust_services(&mut Config)` for profile `project` forces `kernel.mesh.enabled=false` (a project `weave.toml` mesh section is ignored with a warn), voice consumer/`talk_loop`/`voice_loop` off, local embedding model not loaded, no API-key providers, `ecc` vector store kept (it is the project's memory, only the *embedding model* is shared).
- new `weave/src/parent_link.rs`: `ParentLink` (client to the user daemon socket from `spawn.json`; reconnect with backoff; fail closed, never fall back to a local model). `RemoteEmbedder: Embedder` (calls `shared.embed`), `ParentLlm` provider adapter (calls `shared.llm.chat`, streaming), health surfaced in `kernel.status` as `shared_services: {embeddings, llm, voice: "parent|down"}`.
- new `weave/src/shared_rpc.rs` (user-daemon side, `ROUTES` line): `shared.embed`, `shared.llm.chat`, `shared.llm.models`; require a `VerifiedProject` principal (package I) or Admin; per-project rate limit and token budget from the manifest; every call chained on the user chain as `shared.use {project_id, service, tokens}` (no payload text). The loopback `127.0.0.1:8090` proxy (ADR-101) is Phase 4 because only the sandbox needs it.
- `weave/src/daemon.rs`: the existing embedder and `DAEMON_LLM` construction at ~890 and ~1408 are wrapped so profile `project` builds the remote variants (Shared with H; edit only those two blocks).
Tests: project-profile boot lists no `embedding`, `voice`, `talk_loop`, mesh services (`kernel.services` snapshot); with the user daemon stopped `shared.embed` returns a typed `parent_unavailable`, never a local result; key material check: no provider API key reachable in the child's environment or config dump; rate limit and budget refusal; chained `shared.use` has no prompt text.
Acceptance: crate tests, `weave/tests/shared_services.rs` (user daemon plus a stub child), clippy, check. Open fact for the implementer: read `llm_service.rs` and the `agent.chat` path for the real provider seam before choosing `ParentLlm`'s shape.

### G. supervisor, `project` kind and `logical` driver (user-daemon side)
Depends on: A, B, C, D, E, F (stubs allowed until they land). Blocks: integration.
- new `clawft-kernel/src/workload_kind/project.rs`: `ProjectKind` (`id()="project"`, `adapters()=["logical"]`, `prepare` verifies the certificate chain and manifest and returns `WorkloadSource::Project`, requirements `[exact("runtime.project.logical")]`, health = handshake `project_id` equals expected and `shared_services` not all down). Registered in `KindRegistry::builtin()`. A default permit rule for kind `project` limited to the internal supervisor principal is added to `workload_governance` defaults (default deny remains for everyone else).
- new `clawft-kernel/src/workload_runtime/logical.rs`: `LogicalRuntime: WorkloadRuntime` (`id()="logical"`, `provides()=[runtime.project.logical]`, `network_exposure()=None-listener`, `control_mode()=Managed`). It owns no process itself: it calls a kernel-defined `trait ChildLauncher { spawn(&ChildSpec)->ChildRef; terminate; probe }` that the weave supervisor implements, so the kernel crate never depends on weave.
- new `weave/src/project_supervisor/{mod.rs,child.rs,restart.rs,adopt.rs,idle.rs}`:
  - `spawn`: writes `spawn.json {nonce, parent_socket, user_pubkey, user_key_id, project_id, root, expires 60 s}` (0600) and `parent-policy.json`, then `Supervised::spawn(LaunchSpec{program: current_exe, args: [kernel, start, --foreground, --profile, project, --project, <id>], env: cleared plus WEFTOS_RUNTIME_DIR=<run>/<id>, WEFTOS_PROJECT_ID, HOME; cwd: root; limits: unlimited})`. Refuses a root that is `$HOME`, missing, or whose `project.toml` id differs from the manifest.
  - `restart` (OTP `one_for_one` plus `Transient`): restart only the crashed child on non-zero exit, signal or lost heartbeat; exit 0 (idle stop, explicit stop) is not restarted; `RestartTracker` backoff 1 s doubling to 30 s; budget `restart_max` in `restart_window_secs` then state `failed`, chained `project.kernel.failed`, no further automatic start until `project.restart`; the user daemon itself never exits because a child failed.
  - `adopt`: on user-daemon boot (hook `post_boot`) scan `~/.weftos/run/*/kernel.pid`, verify pid alive, exe, `kernel.lock` held, handshake `project_id`; adopted children are `Adopted{pid}` handles (polled, not waited on) and re-register when their `ParentLink` reconnects. Unverifiable leftovers are listed by `weaver doctor`, never signalled.
  - `idle`: polls `mesh.heartbeat.activity` every 30 s; a project with `idle_stop_secs > 0` and no activity (RPC other than status/health/handshake, running agents or workloads, open streams) for that long gets graceful `kernel.shutdown` (final anchor first), then SIGTERM after a 10 s grace; chained `project.kernel.idle_stop`.
- new `weave/src/project_lifecycle_rpc.rs` (`ROUTES`, Admin): `project.start|stop|restart|status|ensure_running`; `ensure_running` is idempotent and returns the child socket. `weave/src/commands/kernel_cmd.rs` (owned here): `weaver kernel start --project <id|name>` calls `project.start` on the user daemon and errors "user daemon not running; run `weaver kernel start --profile user`" (never auto-starts it); `weaver kernel start` inside a project while a user daemon exists on this uid prints the migration hint and refuses unless `--legacy-project-daemon` (one release of deprecation); `stop --all-children`, `--keep-children` for the user daemon's own shutdown (default cascades with clean shutdown). `status` lists children.
- `clawft-rpc/src/resolve.rs` and `connect.rs` (owned here): for a manifest with `via="child-kernel"` the resolver returns the child socket and, if absent, calls `project.ensure_running` on the user daemon once, waits for the handshake, and prints it; handshake `project_id` mismatch stays a hard error.
- `weave/src/service_units.rs` (P1 H file, small edit): launchd `AbandonProcessGroup=true`, systemd `KillMode=process`, so a user-daemon restart by `weaver update` keeps children (analysis 10.5); golden files updated. `clawft-rpc/src/doctor/`: findings for project-rooted daemons not under a user daemon, failed or orphaned children.
Tests: `weave/tests/project_supervisor.rs` with a fake child binary (a shell script) for restart limits (3 crashes inside the window then `failed`; clean exit not restarted; backoff schedule), idle stop, adoption after the user daemon is killed and restarted (pid unchanged), spawn refusal cases; kind and driver unit tests (`ProjectKind::prepare` with a revoked cert fails; `logical` admit fails when the root vanished); `project` workload refused without the supervisor principal.
Acceptance: crate tests, `scripts/build.sh gate` before merge.

### H. child bootstrap and `mesh-local/1` registry
Depends on: A, C, D (E, F at integration). Blocks: integration.
- new `weave/src/project_boot.rs` (hook bodies `pre_boot` and `post_boot` for profile `project`, child side): refuse to start unless `spawn.json` exists, is unexpired and its nonce is accepted (delete it after reading; message: "project kernels are started by the user daemon"); `load_or_create_project_key`; `mesh.challenge` then `mesh.register` with PoP to the user daemon, receive and persist the cert (`project.cert.json`); verify the cert and the signed parent policy; pass `node_key_seed` and the chain signing key to boot; first-boot writes `project.genesis {cert, parent_head: {user_seq, user_event_hash}}` into the new project chain. Heartbeat task (15 s) with the activity struct; on parent loss keep running, queue anchors, fail closed on token validation and shared calls (analysis 10.3), re-register on reconnect; exit only on `kernel.shutdown` or SIGTERM.
- new `weave/src/mesh_local_rpc.rs` (user-daemon side, `ROUTES`; spawn-nonce and PoP authenticated, not capability-token): `mesh.challenge|register|heartbeat|unregister`; `ProjectRegistry {id -> session, socket, pid, state, last_heartbeat, addresses}`; registration for a project the supervisor did not spawn (no spawn nonce) is refused in P2; a second live session for the same id is refused (`kernel.lock` is the child's own guard); sessions expire after 3 missed heartbeats. Registry lookup `route_for(project_id)` is the Phase 3 upstream hook. No change to `MeshIpcEnvelope` (`dest_scope` wire change is Phase 3, decision 5).
- `weave/src/commands/kernel_cmd.rs` is G's; H adds only the `--profile project` value to `user_daemon::parse_profile` (`weave/src/user_daemon.rs`, Shared).
Tests: `weave/tests/mesh_local.rs`: register happy path; wrong PoP, replayed nonce, expired spawn.json, unknown project, wrong root, second session all refused; chain checks: child chain genesis names the user-chain head and `verify_integrity` is green; child restart keeps the same `node_id` and does not mint a second cert (serial stays 1); parent-down boot of an already-registered child (cached cert) continues degraded, a never-registered child refuses to boot.
Acceptance: crate tests, `gate`.

### I. verified project principal (M12)
Depends on: A, F (token scope from P1 F). Blocks: F's `shared_rpc` enforcement and G's project permit rule at integration.
- `weave/src/rpc_ext.rs` (Shared): `VerifiedProject` constructors for the three sources in section 1; `CallerCtx` gains `verified_project: Option<VerifiedProject>` set by the entry path. `weave/src/daemon.rs` `resolve_caller_capabilities` and principal construction (one region, Shared): token path sets it from `TokenInfo.scope.project_id` and refuses a `Request.project` that disagrees with it (`project_scope_mismatch`); a child's own socket sets it from the bound project (`handshake_rpc::compute_bound`); proxied calls from the user daemon to a child carry `forward {project_id, issued_at, sig}` signed by the user key and verified against the cert's `user_pubkey` (5 s window, single use). Kernel `GovernanceRequest` construction sites take `principal.with_project(..)`; `workload_governance/gate.rs` context includes `project_id` so permit rules can match on it.
- `weave/src/scope_gate.rs` (P1 G): `inside_project` now prefers `verified_project`; the claim-only path keeps its P1 behaviour and is labelled `claimed` in audit context.
Tests: claim with no token is never `VerifiedProject`; token scoped to P1 cannot act as P2 (`project_scope_mismatch`); forged or replayed forward header refused; an event audited under a child carries `principal.project_id`; population test: grep-style test that `GatePrincipal::with_project` is only called with a `VerifiedProject` (compile-level: the only constructor takes one).
Acceptance: crate tests, clippy, check.

## 3. Shared files, ownership and merge order

| File | Touched by | Rule |
|---|---|---|
| `weave/src/daemon.rs` | A (3 seams), F (embedder, `DAEMON_LLM` blocks), I (principal region), D (one streaming-intercept line) | No other edits. Bodies live in `project_hooks.rs`, `project_profile.rs`, `project_boot.rs`. |
| `weave/src/rpc_ext.rs` | A (`VerifiedProject`), I; C, D, E, F, G, H one `ROUTES` line each | Keep both lines on conflict. |
| `clawft-kernel/src/chain.rs` | D only | Reviewed by the exochain specialist; `rule_hash` region and `subscribe` region are separate. |
| `clawft-kernel/src/boot.rs` | E (one guarded block), H (key seed plumb) | One region each. |
| `clawft-types/src/config/kernel.rs`, `runtime_paths.rs`, `project/schema.rs` | A (owner), D (one enum variant) | A lands first. |
| `weave/src/commands/kernel_cmd.rs`, `service_units.rs`, `clawft-rpc/src/{resolve,connect}.rs` | G only | |
| `weave/src/user_daemon.rs` | H (one match arm), G (adoption call) | |
| `lib.rs` of kernel/types/rpc/weave | all | `pub mod` lines only. |

Merge order: **A**, then **B, C, D** in parallel (disjoint files; D waits for C only for the anchor RPC verification, so D merges the chain and subscribe halves first), then **E** (needs D's provider), **F**, **I**, then **H** and **G**, then an integration commit by G that wires `adopt` into `post_boot`, installs the `project` permit rule and adds `weave/tests/project_kernel_e2e.rs`: user daemon (tempdir `HOME`) starts a fake-project child, registers it, anchors, applies an overlay denial, stops it idle, restarts it, kills it three times; `scripts/build.sh gate` green. Tests use a per-test `HOME` and `WEFTOS_RUNTIME_DIR` (Phase 0 review R7), never the shared `mktemp`.

## 4. Migration and safety

**Never touched by Phase 2 code:** `~/.clawft/*`, the user chain (only appended to by the user daemon), every `<root>/.weftos/runtime/` directory and its `node.key`, `kernel.sock`, `workloads.json`; no key is imported or merged. New writes only under `~/.weftos/run/<id>/`, `~/.weftos/projects/`, and `<root>/.weftos/{project.key,project.cert.json,chain/,state/}`; the one committed file is `overlay.toml`, which only the owner writes.

**Existing project-rooted (Phase 0) daemons.** Nothing is converted automatically. A project becomes a child only when the owner opts in per project, and Phase 1's rule still holds that an old daemon is never signalled. Opt-in is `weaver project migrate-kernel <id> [--dry-run]` (package G, `project_lifecycle_rpc`): refuses while `<root>/.weftos/runtime/kernel.lock` is held or a `kernel.pid` there names a live process; copies (never moves) `<root>/.weftos/runtime/{workloads.json,apps.json}` to `<root>/.weftos/state/`; generates nothing else; sets `[serve] via = "child-kernel"` in the manifest; prints the rollback line. The project chain starts fresh with `project.genesis` naming the user-chain head, so earlier events stay on the user chain unscoped (they are history, not lost; decision 6). `weave.toml` mesh sections are ignored for children (warned).

**Owner steps, per project, in order:**
1. User daemon running (Phase 1) and the project registered (`weft project list` shows it).
2. Stop any pre-existing project-rooted daemon for this project from its own directory with its own binary; confirm `lsof` and that `kernel.pid` is gone.
3. `weaver project migrate-kernel <id> --dry-run`, read it, run it for real.
4. Optional: write `<root>/.weftos/overlay.toml` (deny rules and lower limits only) and add `.weftos/project.key`, `.weftos/project.cert.json`, `.weftos/chain/`, `.weftos/state/` to the project's `.gitignore` (`weft project init` already adds the first and third).
5. `weaver kernel start --project <id>` (or just run `weft ...` inside the project; the resolver starts it on demand). Check `weaver kernel status` lists the child, `weft project show .` prints a handshake whose `node_id` is the project key id, and the user chain shows `project.register` then `project.anchor` within 5 minutes.
6. Rollback: `weaver kernel stop --project <id>`, set `via = "user-daemon"` in the manifest (or `weaver project migrate-kernel <id> --revert`), restart the old daemon. The original runtime dir was never modified.

**Safety properties the tests must prove:** a second kernel for one project cannot start (child `kernel.lock` plus the registry refusing a second session); a child never opens a mesh listener; no provider API key or token-authority secret is readable in a child's environment, config or `kernel.status`; project key file is refused unless 0600; a corrupt or relaxed overlay never results in a running kernel with fewer rules than the parent; user daemon stop does not leave unsupervised children unless `--keep-children`; `weaver update` restarts only the user daemon and children are adopted afterwards.

## 5. Risks and adversarial-review focus per package

- **A.** Seams that do more than call a function; `GatePrincipal` serde compatibility with older chain payloads; canonical-bytes ambiguity (key order, number formats) in cert and anchor vectors. Review: have a second implementation (a Python snippet in the test) reproduce the golden bytes.
- **B.** The refactor must be invisible to cogs: any change in refusal text or chain event order. Review: diff `workload.*` chain output before and after for the same scripted run; try to register a kind that shadows `cog`.
- **C.** One key signing several message types (domain tags missing anywhere); TOFU window (what if the first registrant is not the real project: spawn nonce and manifest binding are the only guard, state it); revoke view lag. Review: try to get a cert issued for another project's id or root, then for a pubkey the caller does not hold.
- **D.** Highest correctness risk: the hash change on `ChainEvent`. Mixed old and new binaries on one chain; snapshot or RVF codec dropping the new field; subscribe race losing events; unbounded memory from slow subscribers; anchor replay after long outage. Review: verify a pre-P2 chain with the P2 binary and a P2 chain with a pre-P2 binary (expected: fail loudly, not silently); kill between append and anchor.
- **E.** Fail-open on parse errors, glob semantics (`workload.place*` also matching `workload.placement.x`), shadowing by id case or whitespace, parent policy swapped on disk. Review: fuzz overlay and parent files; check the rule hash covers `active` and severity.
- **F.** A "temporary" local fallback creeping in (the whole point is that secrets stay in the user tier); budget bypass by many small calls; confused deputy: a project using `shared.llm` to read another project's context. Review: grep the child binary's reachable config for provider keys; run the project profile with the user daemon down and watch for any network call.
- **G.** Orphan and double-start races (two `ensure_running` at once, 100 iterations); restart storm hiding a crash loop; adopting the wrong process (pid reuse: check exe, start time, lock, handshake); `AbandonProcessGroup` leaving zombies when the owner expected cascade; the legacy-daemon refusal blocking a legitimate single-project user. Review: kill -9 the user daemon during a child restart; pid file pointing at an unrelated process.
- **H.** Pre-boot ordering (chain opened before the cert exists); spawn nonce replay and file permissions; parent-down behaviour that quietly drops anchors or accepts tokens it cannot validate. Review: start a child by hand with a copied `spawn.json`; run with the parent socket replaced by an attacker-owned socket.
- **I.** The weakest point of the plan: `VerifiedProject` is only as strong as the transport. Same-uid local callers can still lie via `Request.project`; the only cryptographic sources are tokens, the child's own socket and the user-signed forward header. Say so in code and docs (it is an isolation guard between projects of one user, not a boundary against a hostile local process, which is Phase 3 peer credentials plus Phase 4 sandboxes). Review: enumerate every place a `GovernanceRequest` is built and confirm none derives `project_id` from request params.

## 6. Open decisions for the owner

1. **One project key (node, chain, anchor, PoP) or separate keys?** Recommend one key (`project.key`) with domain tags: fewest files and the handshake `node_id` equals the certified key id. Separate keys only if a project node must rotate without breaking its chain.
2. **Invalid overlay at boot: refuse to start or start with parent rules only?** Recommend refuse (fail closed); a silently ignored deny is worse than a project that will not start. `weaver doctor` prints the offending key.
3. **Idle stop default.** Recommend `idle_stop_secs = 1800` for `child-kernel` projects once lazy start works (heavy services are off, so a stopped project costs nothing), and never idle while an agent, workload or stream is active. Alternative: 0 (never) until the owner has used it for a week.
4. **Children and user-daemon restarts.** Recommend: `weaver update` and service restarts keep children (adoption, unit changes in G); `weaver kernel stop` of the user daemon cascades with clean shutdown unless `--keep-children`. This contradicts nothing in ADR-103 but changes the launchd and systemd unit text from P1.
5. **Mesh delivery to children in Phase 2.** Recommend register-only: the address and `route_for` lookup exist, but `MeshIpcEnvelope.dest_scope` and inbound routing wait for Phase 3 with the machine service; a child is not reachable from the mesh yet. Alternative: add `dest_scope` now and route in the collapsed user daemon.
6. **Fresh project chain or import of earlier user-chain events?** Recommend fresh with a `parent_head` link; importing re-signs history under a key that did not exist then.
7. **Default for existing projects: opt-in or flip all?** Recommend opt-in per project in Phase 2 (`migrate-kernel`), flip the default for new `weft project init` and for the refusal in `weaver kernel start` (one release warning first) in Phase 3.

Out of scope here, tracked elsewhere: M2 (per-instance capability grants for cogs) and cog-scoped chain append stay in the cog track; M5 federation links, M6 generic delegation cert, M7 attestations are Phase 4 and later. Package D's `chain.subscribe` is the M1 primitive those build on.
