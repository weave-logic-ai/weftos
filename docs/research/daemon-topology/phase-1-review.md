# ADR-103 Phase 1 review: `integrate/p1` @ cc9087e2

This is an independent architecture and security review of the integrated
Phase 1 work. Phase 1 covers the weave topology: user daemon, project model,
D14 resolution, token authority, the D12 scope gate, the gateway facade,
service units and user-chain migration. It is measured against:

- [ADR-103](../../adr/adr-103-weave-topology-roles-and-instances.md) D1-D14 and A1-A6
- [ADR-102](../../adr/adr-102-gateway-health-and-api-playground.md) cards 01-03
- the [P1 plan](../../plans/weave-topology-p1-plan.md)
- the [Phase 0 review](phase-0-review.md)

The range is `0.8-metaharness...integrate/p1~1` (packages A, B, C, D, D0, E,
F, G, H and gw-facade), plus the merge of `0.8-metaharness` on top. Fable
reviewed it on 2026-10-02 by reading the diff and the merged sources. Nothing
was run and no runtime state was touched. Line numbers are from the
`integrate/p1` tree. The gate result is recorded at the end.

## 1. Verdict

**SHIP-AFTER-FIX.** The per-package work is sound, and the packages compose
well on the security seams:

- One authorization chokepoint covers JSON, RVF and voice.
- Literal scopes are honoured only from a same-uid unix peer, and the TCP
  relay strips them.
- Tokens are stored on the chain as hashes only, with a revocation-only
  journal.
- D12 denies by default, and a population test covers every dispatch arm.
- Chain choice refuses every path that would put two kernels on one chain or
  fork a migrated chain.
- Migration copies, verifies, then places, with the source locked and never
  modified.
- Restart acts only on the pid-file pid, after checking the exe and the
  handshake.

What does not compose is the topology itself: `weft` has no route to the user
daemon. The resolver's "user default" level (D14) was never written. The
manifest field meant to carry it (`[serve] via = "user-daemon"`) is written by
B and ignored by A. Until that lands, only `weaver --profile user` and
`weft --runtime ~/.weftos/run` can talk to the user daemon. Every project
command keeps dialing `<project>/.weftos/runtime/kernel.sock`.

Blocking:

1. **B1:** the D14 user-default resolution level (section 2).
2. **B2:** CHANGELOG `[Unreleased]` has no Phase 1 entry,
   `docs/reference/cli.md` documents a resolution order that isn't
   implemented, and the owner steps in `docs/guides/kernel.md` are stale and
   incomplete (section 2). This is the same class as Phase 0's second
   blocker: the upgrade is only safe if the notes say what to do.

## 2. Blockers

**B1. The resolver has no user-default level, so `weft` cannot reach the
user daemon without `--runtime`.** In `crates/clawft-rpc/src/resolve.rs:266-278`,
after the flag, env and manifest `[serve] runtime_dir` levels, the default is
`RuntimePaths::resolve_with(None, cwd, home)`. That is the Phase 0 answer: the
project's `.weftos/runtime`, otherwise `~/.clawft`
(`runtime_paths.rs:182-201`). It can never return `RootSource::User`.

The user root exists only as process state, set by
`weaver kernel --profile user` (`runtime_paths.rs:81-84`,
`user_daemon.rs:46-48`); `weft` never sets it. `adopt_or_init` and
`seed_from_workspaces` write the manifest
`ServeSection { via: UserDaemon, runtime_dir: None }`
(`project/adopt.rs:64`, `project/seed.rs:138`, `project/schema.rs:126-144`).
`resolve_with` reads only `runtime_dir` (`resolve.rs:227`), so `via` is dead
on the client side.

The tests and comments show the gap was not noticed:

- `resolve_tests.rs:80-90` pins the Phase 0 default as correct.
- `connect.rs:190-213` has a comment ("the Phase 1 user-daemon case") showing
  package A believed the default level reached the user daemon.

Both the plan and the ADR require the user-default level. The plan says so in
section 1 A and in section 4 ("its commands go to the user daemon with
`project = <ULID>`"). ADR D14 says "flag -> env -> manifest -> user default".

Failure scenarios after the documented migration, with the user daemon up at
`~/.weftos/run` and the legacy chain migrated:

- **The user daemon is never reached from a project.**
  - `cd ~/weftos && weft agent -m hi` resolves
    `~/weftos/.weftos/runtime/kernel.sock` (`resolve.rs:268,277`). Nothing
    listens there, so `connect_opt` returns `None` (`daemon_conn.rs:118-121`).
  - The command then falls back to local mode, or refuses with "start one with
    `weaver kernel start`", while the user daemon is running.
  - `weft project show .` prints "daemon: not verified" for the same reason
    (`project_cmd.rs:332-343`), so plan step 5 cannot be completed.
- **Commands silently land on the old daemon.**
  - If the old project-local daemon was not stopped, it is an older build and
    the handshake fails with `unknown method`.
  - Because the source is `Default` with no node pin, `connect_resolved`
    downgrades to a degraded handshake and proceeds with a warning
    (`connect.rs:319-344`).
  - Mutating commands then land on the legacy daemon, which appends to the
    legacy chain the owner believes is frozen.
  - The envelope and scope gates on the user daemon never see the call.
- **The D12 gate is unreachable from `$HOME`.** `weft status` dials
  `~/.clawft/kernel.sock`. The D12 `read_only` gate, the `project_required`
  error and the exit criterion in plan section 6 can only be reached with
  `--runtime`.

Fix:

1. **Resolver.** In `resolve_with`, when the project came from `project.toml`
   or `--project`/env, its manifest has no `runtime_dir`, and
   `serve.via == UserDaemon`, choose `user_runtime_root(home)` with
   `ResolveSource::Manifest`.
2. **No known project.** Try the user root before the Phase 0 default when
   `~/.weftos/run/kernel.sock` or `kernel.lock` exists. The `tried` list must
   show both.
3. **Handshake warning.** In `verify_handshake` (`connect.rs:204-213`), drop
   the "not bound to a project" warning when `h.profile == Some("user")`.
   Otherwise every command from a project directory prints it once B1 lands.

After that, `docs/reference/cli.md:18-21` becomes true.

**B2. Release notes.** `CHANGELOG.md` `[Unreleased]` carries the Phase 0 block
and nothing for Phase 1. Owner and operator consequences that must be stated:

- **Gateway bind.** The default host flipped from `0.0.0.0` to `127.0.0.1`,
  with a Host-header guard on loopback (`config/mod.rs:836-839`,
  `api/mod.rs:326-330`). Any install that relied on the implicit LAN bind
  loses it silently.
- **User daemon.**
  - `weaver kernel start --profile user` runs in `~/.weftos/run`.
  - `~/.weftos/weave.toml` takes precedence over `~/.clawft/config.json`.
  - The daemon's working directory is now `~/.weftos`.
- **Migration.** `weaver migrate user-chain` copies the chain. Afterwards any
  kernel that would land on `~/.clawft` is refused (exit 78).
- **Projects.**
  - New commands `weft project init|fork|list|show|seed`.
  - New flags `--project` and `--runtime`.
  - New environment variables `WEFTOS_PROJECT` and `WEFTOS_MANIFESTS_DIR`.
- **Tokens.** `weft token issue|revoke|list`, `wft_` secrets and
  `auth-tokens.jsonl`. Literal `auth` scopes no longer work over the TCP
  relay or from another uid (`peer_uid_mismatch`).
- **Scope gate.** The D12 `read_only` default on the user daemon, and what it
  blocks (S2).
- **Service management.**
  - `weaver service unit` and `weaver update --restart`.
  - Exit 78 semantics and `RestartPreventExitStatus`.
- **Compatibility.**
  - `kernel.status` now carries `handshake`.
  - Legacy clients without `proto` are accepted this release and refused in
    Phase 2.

`docs/guides/kernel.md:388` still says the migrate command "lands with Phase 1
package E". The owner steps there omit four things:

- `--runtime`, which is needed until B1 lands;
- `weaver service unit`;
- the `checkpoint_path` check (S5);
- the gateway bind change.

## 3. Should-fix (before the 0.8.2 cut, not merge blockers)

**S1. The gateway facade bypasses D14.** `DaemonKernelFacade::socket_path` is
`clawft_rpc::socket_path()` (`api/daemon_facade.rs:92-94,138`). That is the
gateway's own cwd walk, with no handshake and no `--runtime`.

- **Failure:** a gateway started from `$HOME` after migration dials
  `~/.clawft/kernel.sock` and answers 503 for every facade route while the
  user daemon is up.
- **Fix:** call `resolve()` (and accept a configured runtime dir), then use
  `connect_resolved`, which also brings the proto check.
- **Least privilege is otherwise right:** explicit `read`, mutating routes
  return 501, and `project_required` maps to 403 with a generic body.

**S2. `read_only` on the user daemon blocks the streams and substrate reads
that first-party clients depend on.** These are not on `READ_ONLY_ALLOW`
(`scope_gate.rs:71-99`), and `allow_list_excludes_known_sensitive_reads`
asserts they are excluded:

| Method | Used by |
|---|---|
| `kernel.logs_stream` | `clawft-substrate/src/kernel.rs:615` |
| `ipc.subscribe_stream` | |
| `substrate.subscribe` | |
| `substrate.read` | every egui explorer panel, `native_live.rs:436` |
| `cluster.facts` | |
| `voice.trace` | `voice_watch/mod.rs:100` |

These clients also call `DaemonClient::connect()` with no project claim
(`clawft-substrate/src/{mesh,chain,kernel}.rs`,
`clawft-gui-egui/src/live/native_live.rs`). Against the user daemon they get
`project_required` and go dark.

The ADR puts voice and the shared heavy services in the user role, so "voice
watch needs a project" is the wrong outcome. Decide one of:

- add the read-only streams and `substrate.read` to the list, or
- make these clients resolve a project and stamp it.

Either way the CHANGELOG must say which.

**S3. A token's `project` is recorded but never enforced.**

- **What happens:** `TokenInfo.project` rides the chain event
  (`token_authority.rs:71,484-486`). But `resolve_caller_capabilities` grants
  `admin` for any live token, and the scope gate reads only the request's own
  `project` claim (`scope_gate.rs:197-200`).
- **Effect:**
  - A token issued with `--project A` works for project B.
  - A token's owner scope plus any registered project id passes `read_only`.
- **How it reads:** this is consistent with ADR-102 D4 (owner scope), but the
  ADR-103 consequence "tokens can carry an optional project_id scope" reads as
  enforcement.
- **Decide:** either make the gate treat a token's `project` as the claim
  (rejecting a differing request claim), or amend the ADR to "recorded; for
  card 04".

**S4. The token journal lives beside the chain and is not migrated.**

- **Location:** `token_rpc.rs:63-76` puts `auth-tokens.jsonl` next to the
  checkpoint. On a user daemon that adopted the legacy chain, that is
  `~/.clawft/auth-tokens.jsonl`. That is a write into a directory the plan
  says Phase 1 never touches.
- **Migration:** `chain_migrate.rs:39-45` copies five files, and the journal
  is not one of them.
- **Effect:** the chain is saved on clean shutdown only
  (`token_authority.rs:19-27`). A crash followed by `migrate user-chain` loses
  the journaled revocation, so the revoked token validates again on the
  migrated chain for up to 24 h.
- **Fix:** put the journal in the runtime root (`RuntimePaths::root()`), or
  add it to the migration set.

**S5. An explicit `kernel.chain.checkpoint_path` bypasses every chain guard.**

- **Cause:** `pin_chain_storage_noted` skips `default_checkpoint_path` when
  the path is set (`chain_storage.rs:377-383`). So the migration marker, the
  user-chain-exists refusal and the first-adoption guard never run; only
  `ChainLock` remains.
- **Failure:** the user daemon loads `~/.clawft/config.json`
  (`user_daemon.rs:139-155`). If that file pins `checkpoint_path` to
  `~/.clawft/chain.json` (the operational-gotcha pattern), the user daemon
  keeps writing the legacy chain after migration. The migrated copy silently
  goes stale.
- **Fix:** `migrate user-chain` and boot should warn when an explicit path
  points into a directory carrying `MIGRATED-TO-WEFTOS.txt`. The owner steps
  should say to check `config.json`.

**S6. Doctor does not know the user root.** `doctor/env.rs:105-119` lists
`~/.clawft`, `~/.weftos/runtime` and ancestors. `~/.weftos/run` is absent, so
`weft doctor` reports no daemon while the user daemon runs. The Phase 0 R6
findings (lock, adoption, migration marker) are still missing.

**S7. `weaver kernel status` without `--profile user` boots an inspection
kernel next to a live user daemon.** This is R5 again: from `$HOME` it dials
`~/.clawft/kernel.sock`, finds nothing, and prints "booting ephemeral kernel"
(`weave/commands/kernel_cmd.rs:272-280`). It should mention the user daemon
when `~/.weftos/run/kernel.sock` answers.

**S8. `--runtime` is exported as `WEFTOS_RUNTIME_DIR`, but a manifest
`runtime_dir` override is not** (`cli/main.rs:393-403`). Code that reads
`socket_path()` or `is_daemon_running()` directly (`mcp_attach.rs:35,97`,
`client.rs:369`) disagrees with the resolver exactly when the manifest chose
the endpoint. Route those readers through `daemon_conn::resolve_current()`.

**S9. launchd restarts on exit 78.**

- **Cause:** `KeepAlive.SuccessfulExit=false` (`service_units.rs:109-113`)
  restarts on every non-zero exit, so only the 30 s throttle limits a
  permanent refusal (lock held, adoption needed, marker present).
  `boot_refusal.rs:5-7` documents this.
- **Effect:** it is not a correctness problem, since a refused boot writes
  nothing. But the log fills every 30 s until the owner notices.
- **Option:** on 78 under launchd, write `~/.weftos/run/REFUSED` and switch
  `KeepAlive` to `PathState { REFUSED: false }`.
- **Related card:** the filed P1-H follow-up covers the
  transient-refusal-as-78 half; this is the other half.

**S10. Degraded-handshake acceptance is broader than the plan.**

- A daemon that answers `unknown method: kernel.handshake` on the default
  endpoint is accepted unverified with one warning (`connect.rs:319-344`).
- A request with no `proto` is accepted for every method, not only read-only
  ones (`handshake.rs:7-11`).

Both are documented deviations and are reasonable for one release. The ADR
should record the Phase 2 refusal date so it is not forgotten.

Verified with no finding:

- **Envelope refusal** happens before authorization on the JSON and RVF paths
  (`daemon.rs` `dispatch_json_line`, `handle_rvf_connection`).
- **Peer credentials:** `peer_cred` fails closed (`unwrap_or(true)`).
- **TCP relay:** it strips literal scopes, refuses RVF framing and bounds
  lines (`relay_auth.rs`).
- **Tokens:**
  - Tokens cannot mint other tokens (`token_rpc.rs:125-131`).
  - `validate` reveals public metadata only, and hashes are compared in
    constant time.
  - The journal holds revocations only, and `issued` lines are ignored
    (`token_authority.rs:255-291`).
- **Project registration and manifests:**
  - `project.register` refuses `/`, `$HOME`, ancestors, state dirs and
    dot-dirs (`project_rpc.rs:98-118`).
  - ULIDs are validated before they become path components.
  - Manifests are 0600, and `project.toml` is created exclusively.
- **Chain choice:**
  - R1 is fixed, with a test (`chain_storage.rs:228-244,638-656`).
  - User-chain choice never starts a silent fresh genesis
    (`chain_storage.rs:286-347`).
- **Migration** (`chain_migrate.rs:417-440`):
  - It refuses a locked or recently written source.
  - It re-hashes after the copy and verifies head and signature against the
    source.
  - It removes its own transient lock, so the first-adoption guard still sees
    a never-locked chain.
- **Restart and boot refusal:**
  - Restart never signals a pid that the socket does not confirm
    (`daemon_restart.rs:167-198`).
  - SIGHUP replay keeps `--profile user` and drops one-shot chain flags
    (`boot_refusal.rs:112-139`).
  - Re-exec survives a replaced binary.
  - Exit 78 is used only for irreparable refusals (`boot_refusal.rs:36-42`).

Phase 0 R-items:

| Item | Status |
|---|---|
| R1 | Fixed |
| R2 | Moot: `clawft-cli/.../kernel_cmd.rs` was removed (f949e0af) |
| R3 | Fixed |
| R4 | Decided: cron is denied to voice |
| R5 | Open (S7) |
| R6 | Open (S6) |
| R7 | Not in this range |
| R8 | Not in this range |
| R9 | Gateway half done; the mesh still binds `0.0.0.0` |

## 4. ADR-103 drift

| ADR text | Code | Evidence | Recommendation |
|---|---|---|---|
| D14 "flag -> env -> manifest -> user default" | The fourth level is the Phase 0 default; there is no user default | `resolve.rs:266-278`, `resolve_tests.rs:80-90` | Fix (B1). Then state the four levels, and that `[serve] via` selects the user daemon |
| D14 handshake `{node_id, user_id, project_id, depth, parent}` | Adds `user_key_id`, `profile`, `roles`, `bound_via`, `runtime_dir`, `pid`, `version`, `sha` and `binary`; `user_id` is the local uid, unverified | `handshake.rs:67-114`, `handshake_rpc.rs:151-161` | Record the full shape. Say `user_id` = uid until D3, and `user_key_id` = hash of the chain verifying key |
| D12 "decided by governance ... operators may change the rule" | A fixed, reviewed allow-list plus a three-mode `kernel.governance.outside_project`; no `GovernanceRule` and no per-rule editing | `scope_gate.rs:71-182`, `config/governance.rs` | Amend: "a policy mode over a reviewed allow-list; rule-level editing deferred" |
| A6 D12 default | Implemented as written | `governance.rs:48-54`, `scope_gate.rs:120-215` | None; note the streams question (S2) |
| D4 "`~/.clawft/` becomes legacy, read for migration only" | Still the default runtime root outside any project; it receives `chain.lock`, `MIGRATED-TO-WEFTOS.txt` and `auth-tokens.jsonl` | `runtime_paths.rs:196-200`, `chain_migrate.rs:595-632`, `token_rpc.rs:63-76` | Amend: the legacy root stays the non-project default until Phase 2; list the three writes as permitted |
| D3 user binding (Phase 3) | A uid-equality peer-credential check for literal scopes landed in Phase 1 | `daemon.rs` accept loop, `resolve_caller_capabilities` | Add an amendment: literal scopes require the daemon's uid and other uids need a token; full binding stays in Phase 3 |
| Consequence: "tokens can carry an optional `project_id` scope" | Carried, not enforced (S3) | `token_authority.rs:71` | Say "recorded; enforcement with ADR-102 card 04", or enforce it |
| Consequence: "`weaver update` restarts the user daemon and per-project kernels, recording the binary in the manifest" | User daemon only, opt-in `--restart`, no manifest `[binary]` write | `update_cmd.rs:138-146`, `daemon_restart.rs` | Mark the per-project part and the manifest write as Phase 2 |
| Consequence: "every local protocol versioned and negotiated from Phase 1" | Daemon RPC `proto` 1 with a range; `mesh-local/N` not yet | `handshake.rs:19-52` | Note that mesh-local is Phase 3 |
| A1 migrate flag family | Matches | `chain_storage.rs:140-158,166-201` | None |
| Legacy clients (plan: read-only methods only) | Accepted for all methods this release | `handshake.rs:7-11` | Record, together with the Phase 2 refusal |
| Phase 1 row "`weft project init/list`" | Also `fork`, `show`, `seed` and the `project.register` RPC | `project_cmd.rs`, `project_rpc.rs` | Update the row |
| A5 gateway loopback | Done, plus a Host guard | `config/mod.rs:836-839`, `middleware.rs:343-369` | Mark as implemented |
| D5, D1, D2, A3 | As specified | manifests, port, mesh toggle, voice principal | None |

## 5. Owner upgrade steps (one machine)

1. **Install the new binaries.** Nothing signals old daemons.
2. **Stop every older daemon.** Do it from its own project directory with its
   own binary (`weaver kernel stop`, or `kill`). Confirm that
   `lsof -i :9470` is empty and that no `kernel.pid` remains.

   This step is load-bearing: migration refuses a locked or recently written
   chain, but it cannot see a writer that uses another runtime dir.
3. **Check `~/.clawft/config.json`.**
   - If it sets `kernel.chain.checkpoint_path`, remove it, or point it at the
     migrated location (S5).
   - Check `gateway.host`. The default is now loopback, so set `0.0.0.0` only
     if you want LAN exposure.
4. **Create `~/.weftos/weave.toml`.** Copy the `[kernel.mesh]` and Noise
   sections from the project's `weave.toml`. Without it the user daemon runs
   with the mesh off.
5. **Migrate the chain.**
   - Run `weaver migrate user-chain --dry-run` and read the plan. It lists
     five files, the head seq/hash, and a signature status of `verified`.
   - Then run it without `--dry-run`.
   - Afterwards `~/.weftos/chain/` holds the chain and `MIGRATED_FROM.json`,
     and `~/.clawft/` holds `MIGRATED-TO-WEFTOS.txt`. The source bytes are
     unchanged.
   - The migration is refused if `chain.key` is missing or the signature does
     not verify. `--allow-unsigned` overrides that, but is not recommended.
6. **Start the user daemon** with `weaver kernel start --profile user`.
   - It takes `~/.weftos/run/kernel.lock`, uses `~/.weftos/chain`, and seeds
     `~/.weftos/projects/` from `workspaces.json`.
   - `weaver kernel status --profile user` should show profile `user`, roles
     `machine, user`, runtime `~/.weftos/run` and project `(unbound)`.
7. **Optional: run it as a service.**
   - Run `weaver service unit --kind launchd --out ~/Library/LaunchAgents/ai.weftos.user.plist`,
     then the printed `launchctl bootstrap` line.
   - Stop the foreground daemon first; the lock refuses two user daemons.
   - After that, `weaver update --restart` restarts through launchd.
8. **Register each project** with `weft project init`, which adopts the seeded
   entry and prints the ULID. Until B1 is fixed, reach the user daemon with
   `weft --runtime ~/.weftos/run ...` or `WEFTOS_RUNTIME_DIR=~/.weftos/run`.
9. **What to expect afterwards.**
   - `weaver kernel start` in a project without flags is refused (exit 78).
     Only two flags override it: `--adopt-legacy-chain`, which forks history,
     or `--new-chain`, which starts a fresh project chain.
   - Against the user daemon, mutating commands outside a project return
     `project_required`.
   - Voice watch, log streams and the egui tray are affected until S2 is
     decided.
10. **Tokens.** `weft token issue` prints a `wft_` secret once, plus a
    playground link. Use `weft token list|revoke` to manage them.
11. **Rollback.**
    1. Run `weaver kernel stop --profile user`.
    2. Remove `~/.weftos/chain` and `~/.clawft/MIGRATED-TO-WEFTOS.txt`.
    3. Restart the old daemon from its project directory.

    The `~/.clawft` chain files are byte-identical; that directory only gained
    `chain.lock`.

## 6. Follow-ups (cards)

- **B1:** resolver user default, a quiet `verify_handshake` when
  `profile == user`, and `cli.md`.
- **B2:** CHANGELOG and `kernel.md`.
- **S1:** facade via `resolve()`.
- **S2:** streams vs `read_only`.
- **S3:** token project scope.
- **S4:** journal location.
- **S5:** explicit `checkpoint_path` warning.
- **S6:** doctor user root.
- **S7:** inspection-boot wording.
- **S8:** manifest override vs env readers.
- **S9:** launchd exit-78 sentinel.
- **S10:** record the Phase 2 refusal of legacy clients.
- **ADR drift:** the table in section 4.
- **Phase 0 leftovers:** R7, R8 and the mesh half of R9 remain open.

Already filed, and none is a merge blocker: P1-H transient refusals exiting
78 and the `-c path` replay, token-journal clock skew, mesh admission (Phase
3), and R5-R9.

## Gate result (recorded by the lead)

`scripts/build.sh gate` on the merged tree passed 17 of 19:

- **UI build:** failed because the worktree had no `clawft-ui/node_modules`.
  It was a setup problem; the dependencies were installed afterwards.
- **npm audit:** failed on two new upstream advisories, `@grpc/grpc-js` and
  `axios`, in the root `package-lock.json`. Both were bumped
  (1.14.4 -> 1.14.5, 1.19.0 -> 1.20.0) without touching the pinned
  `@claude-flow/*` packages.

The gate is re-run after the B1/B2 fixes.
