# Weave topology Phase 1: file-level implementation plan

Status: plan, not started. Implements Phase 1 of [ADR-103](../adr/adr-103-weave-topology-roles-and-instances.md) (card weave-topology-P1). Analysis: [daemon-topology/analysis.md](../research/daemon-topology/analysis.md) sections 4-5, 10.4-10.5. Date: 2026-09-30.

Assumes Phase 0 has landed: one `RuntimePaths` resolver (`WEFTOS_RUNTIME_DIR` > project marker walk-up, never `$HOME` > `~/.clawft` legacy), `kernel.lock` single instance with stale-socket recovery, the CLI names the socket it dials, node id = `hex(SHA-256(ed25519 pubkey)[..16])`, mesh port 9489 with fatal bind. Where this plan says "P0 resolver" it means that module; agents must read its real names first and adapt.

Phase 1 collapses the machine and user roles into one daemon per uid at `~/.weftos/run/`. Machine-wide roots (`/var/lib/weftos`, `/var/run/weftos`) are Phase 3 and out of scope. Project kernels as children are Phase 2 and out of scope: in Phase 1 every project is served by the user daemon and the manifest says so.

## 0. Findings that shape the work

1. `crates/clawft-weave/src/daemon.rs` is 9590 lines and has one choke point, `dispatch_json_line` (line ~3589), which already runs the WEFT-479 capability check, then `dispatch` (line ~5487, a big `match` on the method string). Every package that adds RPC surface would collide there. Package D0 adds an extension hook first so the others touch one line each.
2. `crates/clawft-weave/src/capability.rs::required_capability` defaults unknown methods to `Capability::Read`. The D12 rule must not be built on "capability is Read": an unclassified state-changing method would pass. D12 uses an explicit allow-list, default deny.
3. `Request` (`clawft-rpc/src/protocol.rs:~125`) has `method, params, id, auth`; `Response` has typed `error_kind`. New envelope fields must be `serde(default, skip_serializing_if)` so old peers interoperate. `KernelStatusResult` (`clawft-weave/src/protocol.rs:36`) already carries `build {sha,timestamp,version}` and is what `daemon_guard.rs` reads; the handshake extends that struct rather than adding a call.
4. The kernel already has `AuthService` (`clawft-kernel/src/auth_service.rs`: `request_token`, `validate_token`, `revoke_token`) and the daemon's bearer path is a stub (`daemon.rs:3552-3613`, "AuthService wiring is a 0.8.x followup"). There is no `auth.token.*` RPC anywhere in `crates/clawft-weave/src`. ADR-102 card 01 is therefore net-new code, not a move.
5. Chain files resolve through `clawft-types/src/config/chain_paths.rs` (`chain_root`, `resolve_checkpoint_path`, `resolve_anchor_ledger_path`): `chain.json`, `chain/anchors.jsonl`, plus `chain.rvf` and `chain.key` derived by extension in `boot.rs:~886`. The legacy files that must be migrated are exactly those four.
6. The chain signing key is the existing `chain.key`. Chain signatures are already made with it, so the migrated chain only verifies if that key travels with it. ADR-103 section 5 names `~/.weftos/user.key`; Phase 1 does not introduce a second copy (see D-1 below).
7. `weaver kernel start --foreground` already exists (`commands/kernel_cmd.rs:45`); launchd and systemd units must use it (daemonize forks, which supervisors dislike).

## 1. Contracts every package codes against (fixed here, land in A and B first)

**RPC envelope (package A).** `Request` gains `proto: Option<u32>` and `project: Option<String>` (project ULID); both skipped when `None`. `pub const PROTO_VERSION: u32 = 1; pub const PROTO_MIN: u32 = 1;` in `clawft-rpc`. Absent `proto` means a legacy client (treated as 0): accepted in Phase 1 for read-only methods with a warning in the daemon log, refused in Phase 2. Present and outside the daemon's `[PROTO_MIN, PROTO_VERSION]`: response `ok=false`, `error_kind="proto_mismatch"`, `error` = one line with the remedy, plus `data` (add `data: Option<Value>` to `Response`, same serde rule) `{client:{proto}, daemon:{proto,min,version,sha,exe}}`.

**Handshake (package D), returned inside the `kernel.status` result under `handshake`:**
```json
{"handshake": {
  "node_id": "9f2c...(32 hex)", "user_id": "41ab...(32 hex)|null",
  "project_id": null, "depth": 0, "parent": null,
  "roles": ["machine","user"], "profile": "user",
  "runtime_dir": "/Users/x/.weftos/run", "pid": 4242,
  "proto": {"current": 1, "min": 1}}}
```
`project_id` is null for the user daemon (it serves all projects; the request's `project` field scopes each call). `depth` 0 = not nested; `parent` = null or the parent's `node_id`. `user_id` = same hash function over the user (chain) verifying key. Older daemons omit `handshake`; the client treats that as "legacy daemon, proto 0".

**project.toml (package B), `<root>/.weftos/project.toml`, committed or gitignored at the owner's choice:**
```toml
schema = 1
id = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD"   # ULID, immutable once written
name = "example-project"
created = "2026-10-01T09:30:00Z"
# parent = "<ULID>"                  # optional, nesting (D10), unused in P1
```
Unknown keys are preserved on rewrite (`#[serde(flatten)] extra: toml::Table`).

**Manifest (package B), `~/.weftos/projects/<id>.toml`, one file per project, atomic write (tmp + rename), mode 0600:**
```toml
schema = 1
id = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD"
name = "example-project"
root = "/Users/x/src/example-project"   # canonicalised
state = "active"                 # active | missing | archived
created = "2026-10-01T09:30:00Z"
last_seen = "2026-10-01T09:30:00Z"
project_toml = "present"         # present | pending (seeded, not yet written into the tree)

[seed]
source = "workspaces.json"       # workspaces.json | project-init
legacy_name = "example-project"

[legacy]                         # observation only, never acted on in P1
runtime_dir = "/Users/x/src/example-project/.weftos/runtime"

[serve]
via = "user-daemon"              # P2 adds "child-kernel"; resolver reads this
# runtime_dir = "..."            # optional override: point weft at a legacy project-local daemon until the owner migrates

[binary]                         # written by weaver update (package H)
path = ""
sha = ""
version = ""
```
Seeding from `~/.clawft/workspaces.json` (`WorkspaceRegistry`, `clawft-types/src/workspace.rs`) is idempotent and never writes into a project tree: a seeded entry gets a fresh ULID and `project_toml = "pending"`; if `<path>/.weftos/project.toml` already exists its ULID is reused. Match is by canonical root path. Entries whose path no longer exists become `state = "missing"`, never deleted. `weft project init` in a directory whose canonical path matches a seeded manifest adopts that manifest's ULID instead of minting a new one.

**Decisions this plan takes (the lead should ratify; each is small and reversible):**
- D-1 The Phase 1 user key is the migrated chain key at `~/.weftos/chain/chain.key`. `~/.weftos/user.key` is introduced in Phase 3 with user certificates. ADR-103 section 5's table gets a one-line note.
- D-2 The user daemon loads config from `~/.weftos/weave.toml` layered over the legacy `~/.clawft/config.json` (existing `load_config_layered`). The owner copies the `[kernel.mesh]` section from `example-project/weave.toml` by hand.
- D-3 `kernel.governance.outside_project = "read_only" | "deny_all" | "allow_all"` (default `read_only`) selects the installed D12 rule set; operators can also edit the rule by id.

## 2. Work packages

Owner column lists files a package may edit. "Shared" files are edited by more than one package; rules are in section 3.

### A. rpc-proto (crate `clawft-rpc`)
Depends on: P0 resolver. Blocks: C, D, H (small; land first).
- `crates/clawft-rpc/src/protocol.rs`: add `proto`, `project` to `Request`; `data` to `Response`; `PROTO_VERSION`, `PROTO_MIN`; `pub struct Handshake {..}` and `pub struct ProtoRange` (serde, shared with the daemon so the shape has one definition); `pub fn remedy(build_sha:&str, dirty:bool)->String` returning the text for the mismatch (dirty/dev build: `scripts/build.sh install` then `weaver kernel restart`; release: `weaver update`, then restart; the install-update-review receipt will later pick the channel, keep this one function so that change is one place).
- new `crates/clawft-rpc/src/resolve.rs`: `resolve(flags:&ResolveFlags)->Resolution` implementing D14: flag (`--runtime`, `--project`) > env (`WEFTOS_RUNTIME_DIR`, `WEFTOS_PROJECT`) > manifest (walk up from CWD for `project.toml` using the P0 marker walk, read `~/.weftos/projects/<id>.toml`, honour `[serve].runtime_dir`) > user default (`~/.weftos/run/kernel.sock`). Returns `{socket, source, project_id, tried:Vec<Attempt>}`; `Display` for the "what I tried" output. Manifest parsing goes through package B's reader (add `clawft-types` dependency if not present; if that creates a cycle, duplicate a 30-line read-only parse behind `#[cfg]` and note it).
- `crates/clawft-rpc/src/client.rs`: `DaemonClient::connect_resolved(&Resolution)`; `set_context(ClientContext{project, proto})` (process-global `OnceLock`) so `call` stamps `proto` and `project` on every request without touching call sites; after connect, one `kernel.status` to verify the handshake (project mismatch when handshake.project_id is non-null and differs from the resolved id = hard error).
- `crates/clawft-rpc/src/version_check.rs` and `crates/clawft-cli/src/commands/daemon_guard.rs`: extend the guard to compare `handshake.proto` and print `remedy()`; guard remains warn-only for sha skew, errors only on `proto_mismatch`.
- `crates/clawft-rpc/src/lib.rs`: re-exports (Shared).
Tests: envelope round-trip with and without new fields (old JSON parses; new JSON parses on a struct without the fields via a copy of the old struct); `resolve` table test over a tempdir HOME covering each precedence step and the "$HOME is never a project" case; mismatch response shape; `PROTO_MIN > client` and `client > PROTO_VERSION` cases.
Acceptance: `scripts/build.sh test` (crate `clawft-rpc` tests green), `scripts/build.sh clippy`, `scripts/build.sh check`.

### B. project-model (crate `clawft-types`)
Depends on: P0 (user root helper `~/.weftos`). Blocks: A(resolve), C, D. Land first.
- new `crates/clawft-types/src/project.rs`: `ProjectToml`, `ProjectManifest`, `ProjectState`; `new_id()` (ULID via workspace dep `ulid`, add to this crate's `Cargo.toml`); `read/write_project_toml`, `read/write_manifest` (atomic, 0600, `extra` preserved); `manifests_dir()`; `seed_from_workspaces(&WorkspaceRegistry, &Path manifests_dir)->SeedReport{created,adopted,missing,skipped}`; `adopt_or_init(root)->ProjectManifest`; `find_project_toml(start)` reuses the P0 walk and stops at `$HOME`; `validate_id` (Crockford ULID, rejects path separators) used before any id becomes a path component.
- `crates/clawft-types/src/lib.rs`: `pub mod project;` (Shared, one line).
- `crates/clawft-types/src/config/kernel.rs`: add `governance.outside_project` (D-3) and `roles`/`profile = "user"` fields (Shared with D, G; B only adds nothing here, listed so the merge order is explicit: D owns this file).
Tests: seeding idempotent on second run; existing project.toml ULID reused; missing path becomes `missing`; malformed manifest is skipped with a report entry, not a panic; `id` containing `../` rejected; 0600 mode; unknown-key preservation; concurrent seed (two threads) leaves one file per project.
Acceptance: crate tests, clippy, check.

### C. project-cli (crate `clawft-cli`, binary `weft`)
Depends on: A, B (handshake shape is fixed above so C can start against a stub daemon). Blocks: none.
- new `crates/clawft-cli/src/commands/project_cmd.rs`: `weft project init [--name N] [--force]` (writes `.weftos/project.toml` using `adopt_or_init`, adds `.weftos/chain/` and `.weftos/project.key` to `.gitignore` only if a `.gitignore` already exists and the lines are missing, prints ULID; refuses inside `$HOME` itself; refuses when the daemon is up and the manifest is owned by another root), `weft project list [--json]` (reads manifests directly, no daemon needed, annotates `missing`), `weft project show [id|name|.]` (manifest + live handshake if a daemon answers, with the `tried` list when not). Name lookups that match more than one project are an error listing the ULIDs.
- `crates/clawft-cli/src/main.rs` (~lines 48 and 531, Shared): add `Project(commands::project_cmd::ProjectArgs)` and a global `--project`/`--runtime` flag pair feeding `ResolveFlags`, then `set_context` before dispatch.
- `crates/clawft-cli/src/commands/mod.rs` (Shared): `pub mod project_cmd;`.
- `crates/clawft-cli/src/commands/workspace_cmd.rs`: `workspace create` also calls `adopt_or_init` (so new workspaces get a manifest); no other behaviour change. `crates/clawft-cli/src/commands/kernel_cmd.rs` untouched.
- Replace the CLI's direct `DaemonClient::connect()` uses only where the command already calls `daemon_guard`; do not sweep all call sites (P0 owns the silent-fallback removal).
Tests: `crates/clawft-cli/tests/cli_integration.rs` gains init/list/show against a tempdir HOME (`HOME` and `WEFTOS_RUNTIME_DIR` set, no daemon); init twice is idempotent; init adopts a seeded manifest; list shows a `missing` entry.
Acceptance: `scripts/build.sh test`, clippy, check; manual: `weft project list` on the owner's machine after seeding shows the workspaces.json entries.

### D0. daemon extension hook (tiny, first in `clawft-weave`)
Depends on: none. Blocks: F, G, and D's arms. One commit.
- new `crates/clawft-weave/src/rpc_ext.rs`: `pub struct ExtCtx {kernel, transport: Transport(Local|Tcp), project: Option<String>, auth: Option<String>}`; `pub async fn dispatch_ext(method:&str, params:Value, ctx:&ExtCtx)->Option<Response>` iterating a `const EXTENSIONS: &[fn(...)->...]` list. Each later package adds one line to that list plus its own module.
- `daemon.rs`: (1) in `dispatch_json_line` after the JSON parse: proto check (calls into `handshake.rs`) then D12 scope gate (calls G's function via `rpc_ext`), both before the WEFT-479 capability check; (2) at the top of `dispatch`, `if let Some(r) = rpc_ext::dispatch_ext(..).await { return r; }`. Nothing else in daemon.rs changes in D0.
Acceptance: existing weave tests unchanged and green.

### D. user-daemon (crate `clawft-weave`)
Depends on: A, B, D0. Blocks: integration of E, F, G.
- new `crates/clawft-weave/src/handshake.rs`: `build_handshake(&Kernel, profile)->Handshake` (node id from the P0 node key, user id from the chain verifying key `ChainManager::verifying_key()` at `chain.rs:1157`), and `check_proto(&Request)->Result<(),Response>`.
- new `crates/clawft-weave/src/user_daemon.rs`: `pub fn user_runtime_paths()` (thin wrapper over the P0 resolver forced to the user root `~/.weftos/run`), `boot_user_profile(config)` (sets roles `machine,user`, points the chain root at `~/.weftos/chain/`, runs E's `ensure_user_chain()` before kernel boot, runs B's `seed_from_workspaces` after boot and logs the `SeedReport`, refuses to start if another user daemon holds the P0 lock, prints the holder's pid and exe).
- new `crates/clawft-weave/src/project_rpc.rs`: RPC `project.list`, `project.show`, `project.register` (writes a manifest; refuses a root already owned by another id). Registered through `rpc_ext.rs` (one line).
- `KernelStatusResult` in `crates/clawft-weave/src/protocol.rs:36`: add `#[serde(default)] handshake: Option<Handshake>`; `daemon.rs` `kernel.status` arm (~line 5494) fills it (Shared, one small edit: D owns it).
- `crates/clawft-weave/src/commands/kernel_cmd.rs`: `weaver kernel start --profile user` (also `WEAVER_PROFILE=user`); `status` prints the handshake block; `stop`/`restart` unchanged and already use `pid_path()`.
- `crates/clawft-types/src/config/kernel.rs` (Shared): `profile: Option<String>`, `roles: Vec<String>` defaults.
- Pid file: written by the P0 lock code; D adds an `exe` line (`std::env::current_exe`) as a second line, `kernel.pid` first line stays the bare pid so existing readers (`kernel_cmd.rs:754`, `read_daemon_pid`) keep working.
Tests (`crates/clawft-weave/tests/user_daemon.rs`, new): boot with `WEFTOS_RUNTIME_DIR` under a tempdir and profile user; `kernel.status` returns the handshake with a stable `node_id` across two boots; second start exits nonzero naming the holder; `project.list` returns the seeded projects; proto mismatch returns `proto_mismatch` with remedy; legacy request without `proto` is served for read methods.
Acceptance: `scripts/build.sh test`, clippy, check, `scripts/build.sh gate` before merge.

### E. chain-migrate (crates `clawft-kernel`, `clawft-types`, `clawft-weave`)
Depends on: P0 resolver. Blocks: D's boot hook (D can stub `ensure_user_chain` until E lands).
- new `crates/clawft-kernel/src/chain_migrate.rs`: `plan(legacy_root, dest_root)->MigrationPlan` (lists the four files with size and sha256); `execute(plan)->Result<MigrationReport>`; `ensure_user_chain(dest)->Outcome{AlreadyMigrated|Migrated|NothingToMigrate|Refused(reason)}`. Steps: (1) if `dest/MIGRATED_FROM.json` exists, stop (idempotent); (2) refuse if the legacy dir has a live chain writer: attempt an exclusive `flock` on `legacy/chain.rvf` (non-blocking) and check that no `kernel.pid` in the legacy dir names a live process; (3) stat (size, mtime) and hash sources; (4) copy to `dest/chain.rvf.partial` etc. with `fsync`; (5) re-stat the sources: any change since step 3 aborts and removes only the `.partial` files; (6) open the copies with `ChainManager`, run `verify_integrity()` (`chain.rs:1563`) and compare head hash and sequence to the source opened read-only; (7) rename `.partial` into place, write `MIGRATED_FROM.json` `{source, files:[{name,sha256,bytes}], head_hash, sequence, at, weaver_version}`, key file mode 0600. Originals are never modified, moved or deleted.
- `crates/clawft-types/src/config/chain_paths.rs` (Shared with nobody else): add `user_chain_root(home)` = `~/.weftos/chain`; when `dest/MIGRATED_FROM.json` exists and a non-user daemon would fall back to the legacy `~/.clawft` chain, `boot.rs` (one guarded call, Shared) refuses with "legacy chain was migrated to ~/.weftos/chain; use WEFTOS_RUNTIME_DIR or `--allow-legacy-chain`". This closes the fork risk in section 4.
- new `crates/clawft-weave/src/commands/migrate_cmd.rs` + `commands/mod.rs` line (Shared): `weaver migrate user-chain [--dry-run] [--from DIR]` printing the plan, the verification, and the exact rollback line.
Tests (`clawft-kernel` integration, real files in tempdirs): golden migrate of a fixture chain then verify head equality; source mutated mid-copy aborts and leaves the source byte-identical; live pid file refuses; second run is a no-op; truncated source (verify fails) refuses and leaves no dest files; permissions of `chain.key`; dry-run writes nothing. A metamorphic test: migrating twice from the same source gives identical hashes.
Acceptance: crate tests, clippy, check; manual on a copy of the real 46 MB chain (copy `~/.clawft` to a scratch dir first; never point the first run at the live one).

### F. token-authority (ADR-102 cards 01 and 02 land here)
Depends on: D0 (hook), A (envelope). Blocks: ADR-102 cards 04-06.
- new `crates/clawft-kernel/src/token_authority.rs`: `TokenAuthority{issue(ttl,label,project_id)->(secret,id), validate(secret)->Option<TokenInfo>, revoke(id), list()}`; stores only SHA-256 of a 256-bit secret; `id` = first 16 hex of the hash; table rebuilt on start from chain events `auth.token.issued` minus `auth.token.revoked` minus expired (uses `ChainManager::tail_from`, `chain.rs:1334`; if a by-kind scan is needed add a read-only accessor in `chain.rs`, Shared, additive only). Optional `project_id` scope (ADR-103 consequence). Constants: default TTL 15 min, max 24 h. `kernel/src/lib.rs` `pub mod` line (Shared).
- new `crates/clawft-weave/src/token_rpc.rs`: `auth.token.issue|revoke|validate|list` via `rpc_ext.rs`. `issue`, `revoke`, `list` are refused when `ctx.transport == Tcp` (the RPC-over-TCP listener at 127.0.0.1:9471 must never mint tokens) and record `issuer {node_id, uid, uid_verified:false}` (peer credentials are Phase 3, D3).
- `crates/clawft-weave/src/capability.rs`: add the four methods to `required_capability` (`issue/revoke/list` = `Admin`, `validate` = `Read` with a per-connection rate limit). `daemon.rs::resolve_caller_capabilities` (Shared, one function): consult `TokenAuthority` before the stub so a `Request.auth` bearer issued here yields the owner scope.
- new `crates/clawft-cli/src/commands/token_cmd.rs` (+ `main.rs`/`mod.rs` lines, Shared): `weft token issue [--ttl 15m] [--label L] [--project ID]`, `revoke <id>`, `list`. Prints the token once and, when a gateway address is configured, `http://<gateway>/playground#t=<token>`.
Tests: unit (hash-only: grep the serialised chain event and the table for the secret, must be absent); expiry; revoke; restart rebuild keeps a live token and drops a revoked and an expired one (spawn two `ChainManager` instances over the same dir); `Tcp` transport refusal; ttl above 24 h clamps or errors (pick error); `project_id` round-trips onto the event.
Acceptance: crate tests, daemon RPC test in `crates/clawft-weave/tests/token_rpc.rs`, clippy, check.
**ADR-102 cards:** 01 and 02 are Phase 1 work (F). 03 (`DaemonKernelFacade`) is independent of Phase 1 and should run in parallel as its own track because 04-06 need it. 04, 05, 06, 07, 08, 09 stay in the ADR-102 track and start after F merges; note for 05 that the daemon section of `/health` should show the handshake, and for 08 that the playground project selector reads `project.list`. Nothing in 04-09 blocks the Phase 1 exit.

### G. governance-d12
Depends on: D0 (hook), A (`project` on the envelope). Blocks: none.
- new `crates/clawft-kernel/src/governance_scope.rs`: `default_rules(mode)->Vec<GovernanceRule>` following the existing pattern (`workload_governance::default_rules()`, chained at `boot.rs:~1687`): rule id `scope.outside-project.deny-mutating`, action selector `rpc.outside_project.*`. `READ_ONLY_ALLOW: &[&str]` (explicit): `kernel.status`, `kernel.ps`, `kernel.logs` (read form), `project.list`, `project.show`, `daemon.health`, `doctor.*` read verbs, `version`. Anything not on the list is denied when there is no project. `mode` from D-3: `read_only` installs the rule, `deny_all` denies the allow-list too except `kernel.status` and `project.*`, `allow_all` installs nothing.
- `boot.rs` (Shared, one `.chain(...)` line next to the workload rules). `clawft-kernel/src/lib.rs` `pub mod` (Shared).
- new `crates/clawft-weave/src/scope_gate.rs`: `check(method, project:Option<&str>, gate)->Result<(),Response>` using `rpc_gate::decide` with action `rpc.outside_project.<method>` and context `{method, project_id:null}`; failure is `error_kind="project_required"`, message "not in a project; run `weft project init` or pass `--project`". `project` present but unknown to the manifest dir is also `project_required` (typos do not fall through to allowed).
- Honest limit, state it in code comments and the doc: `project` on the request is client-declared, so in Phase 1 this is a guard against operating in the wrong place, not an authorisation boundary between local processes of one uid.
Tests: a population test that enumerates every method string in `daemon.rs::dispatch` (regex over the match arms in the test) and asserts each is either on the allow-list or denied outside a project, and that the allow-list contains no method whose handler mutates (reviewed list in the test file, fails when a new method appears unclassified); the three modes; unknown method denied; rule editable (replace the rule, behaviour changes); with a project supplied everything falls through to the existing capability check.
Acceptance: crate tests, clippy, check.

### H. service-units and update restart
Depends on: D (profile flag, pid `exe` line), A (none). Blocks: none. Coordinate with the install-review team, which is replacing `update_cmd.rs` internals with axoupdater; H only adds a restart step and a call site.
- new `crates/clawft-weave/src/service_units.rs`: pure functions `launchd_plist(exe,home)->String`, `systemd_user_unit(exe,home)->String`. launchd: label `ai.weftos.user`, `ProgramArguments [exe, kernel, start, --foreground, --profile, user]`, `RunAtLoad`, `KeepAlive {SuccessfulExit=false}`, `StandardOutPath/StandardErrorPath ~/.weftos/run/kernel.log`, no hardcoded `/usr/local/bin` (the flaw in `scripts/com.clawft.wake.plist`); `exe` is `current_exe()` canonicalised. systemd: `weftos.service` for `systemctl --user`, `Type=simple`, `Restart=on-failure`, `WantedBy=default.target`.
- new `crates/clawft-weave/src/commands/service_cmd.rs` (+ `commands/mod.rs` and `main.rs` lines, Shared): `weaver service unit --kind launchd|systemd [--out FILE]` prints to stdout by default; with `--out` it writes the file (refuses to overwrite without `--force`) and prints the install commands as text (`launchctl bootstrap gui/$UID <file>`, `systemctl --user enable --now weftos`). It never runs them.
- new `crates/clawft-weave/src/commands/daemon_restart.rs`: `restart_user_daemon()`: read `~/.weftos/run/kernel.pid` (pid, exe), verify the pid is alive, connect and read `handshake` and `build.sha`; if a launchd label `ai.weftos.user` is loaded use `launchctl kickstart -k gui/<uid>/ai.weftos.user`, else if the systemd user unit is active `systemctl --user restart weftos`, else `kill -HUP` as `kernel restart` does today. Never signals any other pid. If exe differs from the just-installed binary path, report instead of restarting. Project-local daemons found via the manifest `[legacy].runtime_dir` are listed, not touched. After restart, re-handshake and compare sha; write `[binary]` into the manifests of nothing in P1 (per-project restart is Phase 2), only print.
- `crates/clawft-weave/src/commands/update_cmd.rs` (Shared with install-review, edit at the "restart the kernel" print near line 112 only): call `restart_user_daemon()` behind `--restart` (default: print the exact command, as today).
Tests: unit tests on the generated text (golden files under `crates/clawft-weave/tests/golden/`, `plutil -lint` and `systemd-analyze verify` run as optional checks in the script when present); restart refuses a pid file whose pid is dead; refuses a pid that is not a weaver (exe check); never targets pids outside the file.
Acceptance: crate tests, clippy, check; manual: `weaver service unit --kind launchd | plutil -lint -`.

## 3. Shared files, ownership rules and merge order

| File | Touched by | Rule |
|---|---|---|
| `crates/clawft-weave/src/daemon.rs` | D0, D, F | D0 lands the hook and the two call sites. D edits only the `kernel.status` arm. F edits only `resolve_caller_capabilities`. No package adds a new `match` arm here; new methods go through `rpc_ext.rs`. |
| `crates/clawft-weave/src/rpc_ext.rs` | D0 creates; D, F, G add one line each | Trivial conflicts, resolve by keeping both lines. |
| `crates/clawft-weave/src/commands/mod.rs`, `main.rs` | E, H | One `mod` and one enum variant each. |
| `crates/clawft-cli/src/main.rs`, `commands/mod.rs` | C, F | One variant and one `mod` each; C owns the global flags. |
| `crates/clawft-types/src/config/kernel.rs` | D (owner), G (reads) | D adds `profile`, `roles`, `governance.outside_project` in one edit before G starts coding against them. |
| `crates/clawft-kernel/src/boot.rs` | E, G | One guarded call (E) and one `.chain(...)` (G). |
| `crates/clawft-kernel/src/lib.rs`, `clawft-types/src/lib.rs`, `clawft-rpc/src/lib.rs` | all | `pub mod` lines only. |
| `crates/clawft-kernel/src/chain.rs` | F (additive accessor only) | No behaviour change; reviewed by the exochain specialist. |
| `crates/clawft-weave/src/commands/update_cmd.rs` | H, install-review | H edits one call site; rebase onto install-review's version before merge. |
| `scripts/build.sh` | nobody | No new flags needed. |

Merge order: B and A (in parallel, they touch different crates; B first if `resolve.rs` needs the manifest reader) -> D0 -> then E, F, G, H, D and C in any order, each rebased on D0 -> integration commit by D that wires `ensure_user_chain` and the seed call -> `scripts/build.sh gate`. Worktree layout: one worktree per package, branch names `wt/p1-<letter>`; E, F, G, H do not share a single non-hook file.

Every package runs `scripts/build.sh check`, `scripts/build.sh clippy`, `scripts/build.sh test` in its worktree before merging (CLAUDE.md rule). D runs `gate` before its merge because it changes boot.

## 4. Migration safety (no data loss)

**What is never touched by any Phase 1 code:** every `<root>/.weftos/runtime/` directory (`kernel.sock`, `kernel.pid`, `node.key`, `workloads.json`, `cluster_peers.json`, `kernel.log`); `~/.clawft/chain.rvf`, `chain.json`, `chain.key`, `chain/anchors.jsonl`, `workspaces.json`, `config.json`; the other `node.key` files (the four on the owner's Mac stay; no key is imported or merged). Phase 1 only reads them, copies the chain set (E), and writes under `~/.weftos/` and, on `weft project init`, `<root>/.weftos/project.toml`.

**Legacy chain.** Copy, verify, keep (package E steps 1-7). If verification fails the user daemon refuses to start with the reason and the command to retry, rather than booting a fresh genesis (that would fork history silently). `--fresh-chain` exists as an explicit, logged opt-out for throwaway installs. After success `~/.clawft/` gets no edits; an optional breadcrumb file `~/.clawft/MIGRATED-TO-WEFTOS.txt` is the only write there and is skipped if the directory is read-only.

**The fork hazard.** After migration two chains exist. Any daemon started later that still falls back to `~/.clawft` would append to the old copy and diverge. E's guard makes that a boot refusal unless `WEFTOS_RUNTIME_DIR` isolates it or `--allow-legacy-chain` is passed, and `doctor` (existing `weaver`/`weft doctor`, add one check) reports a `MIGRATED_FROM.json` next to a live legacy-rooted daemon.

**The running project-local daemon (example-project, PID 70730 on the owner's Mac, mesh on `*:9470`, chain writes into the shared `~/.clawft` chain).** Phase 1 does not stop it. What the OWNER must do by hand, in this order:
1. In `example-project`: `weaver kernel stop` (or `kill 70730`), then confirm with `lsof -i :9470` and that `kernel.pid` is gone. This must precede the chain copy; E refuses while a live pid is recorded and while the file is locked, but it cannot detect a writer that uses a different runtime dir. The plan never signals that process.
2. Copy the `[kernel.mesh]` (and Noise) settings from `example-project/weave.toml` into `~/.weftos/weave.toml` (D-2). Until this is done the user daemon runs with mesh off, and the old daemon's mesh peers see it disappear.
3. `weaver migrate user-chain --dry-run`, read the plan, then run without `--dry-run`.
4. `weaver kernel start --profile user` (or install the generated launchd unit by hand from `weaver service unit --kind launchd`). Check `weft project list` shows the seeded entries.
5. In `example-project`: `weft project init` (adopts the seeded manifest). Verify `weft project show .` and that `kernel.status` reports the handshake.
6. Remove the stale socket in `~/weftos/.weftos/runtime/` and archive or delete the extra `node.key` files only if wanted; nothing requires it.
Rollback: `weaver kernel stop`, restart the old daemon with the old binary in its project directory; `~/.clawft` is unchanged.
Until Phase 2 the project has no kernel of its own; its commands go to the user daemon with `project = <ULID>`. If the owner must run a project-local daemon before Phase 2, the supported way is `[serve].runtime_dir` in the manifest pointing at it plus `WEFTOS_RUNTIME_DIR` set for that daemon, with mesh off, accepting that it has its own chain.

## 5. Risks and adversarial-review focus per package

- **A.** Old client against new daemon and new client against old daemon (both directions, in tests). A `project` value with path characters reaching a filesystem call. The global `OnceLock` context leaking across tests (use per-test override). Review question: can any code path still dial a socket without going through `resolve`?
- **B.** Two writers seeding at once; a seeded entry whose root is now a different project; symlinked roots (canonicalise both sides); `$HOME` accepted as a root; ULID reuse across two different roots. Review: try to make `init` write outside `<root>/.weftos/` or `~/.weftos/projects/`.
- **C.** `weft project init` in a subdirectory of an existing project (must find the parent's project.toml, not mint a nested one unless `--nested`); `.gitignore` edits must be append-only and only lines missing. Review: run it with `HOME` unset and with a read-only tree.
- **D0/D.** Order of checks in `dispatch_json_line` (proto, then D12, then capability: an auth bypass here is the worst outcome); pid file format change breaking `read_daemon_pid`; user daemon started from launchd with `cwd=/` picking up a wrong `weave.toml`. Review: prove a second daemon for the same uid cannot start under a race (start two at once, 100 iterations).
- **E.** Highest data-loss risk in the plan. Live writer during copy; partial files surviving a crash; verification that only checks the copy against itself (must compare to the source head); key file permissions; disk full mid-copy. Review: kill the process at each step boundary and confirm source untouched and dest either absent or complete and verified; corrupt one byte in the source and confirm refusal.
- **F.** Secret leakage: the secret must not appear in the chain, logs, `Debug` output or error text (leak test). Token issuance reachable over the TCP RPC listener. Rebuild-from-chain skipping a revocation that landed after the last checkpoint. Timing of hash compare (constant time). Review: issue, revoke, restart, replay the revoked secret.
- **G.** Default-open by omission (the `required_capability` default-to-Read trap in section 0). A new RPC method added later without classification: the population test must fail, not pass silently. `project` spoofing is by design not a boundary; make sure the docs say so. Review: enumerate `dispatch` arms yourself and list any mutating method reachable without a project.
- **H.** Restarting the wrong process (pid reuse after a crash: check exe and start time, not just liveness); writing a unit that runs an old binary path after `weaver update`; the plist or unit text embedding an unescaped path. Review: pid file pointing at an unrelated process, path with spaces, `$HOME` containing non-ASCII.

## 6. Exit criteria for Phase 1

`weaver kernel start --profile user` on the owner's machine boots from `~/.weftos/run/`, holds the only lock for the uid, serves the migrated chain (`verify_integrity` green, head equal to the source), lists the seeded projects, answers `kernel.status` with the handshake, rejects a mismatched `proto` with a remedy, denies `agent.chat` and friends with `project_required` outside a project while `kernel.status` still works, issues and revokes a token that survives a daemon restart, and `weaver service unit` prints valid launchd and systemd text. `scripts/build.sh gate` is green. No file under `~/.clawft` or any project runtime dir has been modified.
