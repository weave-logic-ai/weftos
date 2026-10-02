# ADR-103 follow-up round: cross-package review

- **Scope:** `integrate/fu` @ `af48386d8` against `0.8-metaharness` @ `71dbbe97f`
  (7 packages: fu-docs, fu-mesh-kernel, fu-chain-authority, fu-mesh-service,
  fu-supervisor, fu-p1-cli, fu-identity-trust; merge commits; one semantic test
  fix). 140 files, +7205/-660.
- **Method:** read-only. `git diff`, `grep`, file reads in the worktree. No
  builds or tests were run here (the lead is running the gate). Each
  per-package review already said SHIP; this pass looks only at what those
  reviews could not see: how the packages interact after the merge.
- **Date:** 2026-10-02

## Verdict: SHIP-WITH-FIXES

Three items should land before the push (1 and 2 are documentation-sized; 3 is
either a small code change or a documented known limit). Nothing found that
blocks on correctness or security.

## Must-fix

### 1. The new `kernel.mesh` limit keys do not reach the mesh service, and the docs say they do

- `crates/clawft-mesh-service/src/main_loop.rs:302` calls `serve_listener(...)`,
  which is `crates/clawft-kernel/src/mesh_serve.rs:69-80` and passes
  `Limits::default()` (`mesh_serve.rs:77`).
- The only place `max_connections_per_ip` / `first_frame_timeout_secs` are read
  is the collapsed daemon's boot, `crates/clawft-kernel/src/boot.rs:784-787`.
- `MeshServiceConfig` (`crates/clawft-mesh-service/src/config.rs:92-117`) has
  no equivalent keys, so an operator on a service install cannot tune them from
  `/etc/weftos/mesh.toml` either.
- The behaviour change itself (cap 64, first frame 10 s under every mode) does
  apply to the service, because the defaults match. Only tunability is missing.
- Docs that state tunability without qualification: `CHANGELOG.md:12-20`
  ("Tune with `kernel.mesh.max_connections_per_ip` ..."), ADR-103 A10
  "Connection limits apply in every mode" (`docs/adr/adr-103-...md`, the
  bullet reads "Both are configurable"), and the two new rows in
  `docs/guides/configuration.md`.

Fix, minimum for this push: add "collapsed daemon only; the machine mesh
service uses the defaults until `mesh.toml` grows the two keys" to those three
places. Proper fix (follow-up card): two keys on `MeshServiceConfig`, build a
`Limits` from them and call `serve_listener_with` at `main_loop.rs:302`.

### 2. CHANGELOG `[Unreleased]` covers three of the seven packages

`CHANGELOG.md:12-66` has sections for mesh (A10), chain authority (A7 a-c) and
Phase 1 follow-ups (A8). Nothing for:

- **fu-identity-trust (A13/A14):** `weaver migrate user-key --rotate`;
  `weaver project anchor reset` / `project.anchor.reset`; the new refusal
  `anchor_chain_id`; `user_key_history_invalid`; and the behaviour change that a
  same-uid peer inside a supervised child's process group has its literal
  `auth: "admin"` ignored (anything an agent runs inside a child kernel that
  relied on the ADR-070 shortcut against the user daemon now gets anonymous
  read+chat).
- **fu-supervisor (A7 S5-S8):** a `running` child whose registry session stays
  expired for 30 s is stopped and restarted (new liveness pass,
  `project_supervisor/boot.rs`); `stale_build` on `project.status` and the
  doctor finding; `--legacy-project-daemon` refused in a `child-kernel` tree;
  `project.stop` on an unmanaged leftover answers `unmanaged_pid`; shared-slot
  fairness.
- **fu-mesh-service (A10):** the service now modifies the journal at start when
  it finds a lone torn final line (`state.rs:158-166`, journalled as
  `journal.accept_truncate` with `auto: "torn_tail"`); `service.json` is
  published after the bind instead of before; `read_record` waits for a
  mid-write record. The lead listed torn-tail and publication order as
  behaviour-change notes to verify: both are in the ADR, neither is in the
  CHANGELOG.
- **fu-chain-authority (d):** placement RPCs refuse after a `governance.parent.push`
  until the kernel restarts. This is in ADR A7 but not in the CHANGELOG
  chain-authority section.

Also in that file: the A8 bullet (`CHANGELOG.md:40-51`) says the relay/proto
clause twice, and `CHANGELOG.md:57` is one long line that breaks the wrap.

### 3. `REFUSED` + launchd `PathState`: a manual `kernel start --profile user` beside an installed unit now causes a 30 s cycle

What changed: the plist's `KeepAlive` is `PathState {~/.weftos/run/REFUSED: false}`
(`service_units.rs:120-128`); a clean exit writes the sentinel
(`kernel_cmd.rs:277-283`); **every** booting daemon clears it
(`daemon.rs:1151`), and a boot refused because the lock is held deliberately
does not write it (`boot_refusal.rs:44-48`, "a duplicate `kernel start` must
not mark the live daemon as refused").

Sequence:

1. `weaver kernel stop --profile user` under launchd: clean exit, sentinel
   written, launchd leaves the job down. Same as the old `SuccessfulExit=false`.
2. Operator runs a bare `weaver kernel start --profile user` (the command the
   docs give: `docs/guides/kernel.md:431`, `docs/reference/cli.md:203`
   for `child:<id>:orphan`, `cli.md:1130`). It boots and removes the sentinel.
3. launchd watches the path; when it disappears the condition for keeping the
   job alive is true again, so it launches its own instance. That instance is
   refused (`LockError::Held`), exits 78, writes nothing, and launchd retries
   every `ThrottleInterval` (30 s) for as long as the manual daemon runs.

Before this round the launchd job stayed dormant after a clean exit and a
manual start did not wake it, so this is a new way to get the cycle the
feature set out to remove. I have not run launchd to confirm; this is my
reading of `launchd.plist(5)` `PathState` semantics (launchd starts the job
when the path condition becomes true), which is how the "disable file"
pattern is normally used.

Fix options:

- (a) Code: only an instance the service manager started clears the sentinel.
  Set an `EnvironmentVariables` marker in the generated plist (and the systemd
  unit for symmetry) and check it at `daemon.rs:1151`; have
  `kernel start --profile user` print the kickstart hint when the unit is
  loaded (`daemon_restart.rs:409-413` already probes `launchctl print`).
- (b) Docs only, acceptable for this push: known limit in the CHANGELOG A8
  bullet, ADR A8 "Refusal exits", and `kernel.md` step 7: once the unit is
  installed, start and stop through `launchctl` or
  `weaver kernel restart --profile user`, never a bare `kernel start`.

## Checked and clean

### (a) `authorize_caller` after the merge

Order in `crates/clawft-weave/src/daemon.rs:3787-3829` with the earlier
envelope gate at `:3893-3900`:

1. `handshake_rpc::envelope_refusal` (proto). A no-`proto` request passes only
   if the method is on `READ_ONLY_METHODS` (`clawft-rpc/src/handshake.rs:176-206`),
   and a test asserts every entry is `Capability::Read`
   (`handshake_rpc.rs:338-343`). `kernel.handshake` exempt.
2. The `peer_uid_mismatch` message for a literal scope from another uid, now
   skipped for a child peer (`:3796-3797`).
3. `caller_principal::establish` (token project vs claim). Does not read the
   peer flags.
4. Project-token method allow-list (`project_token_scope.rs:34-41`).
5. `resolve_caller_capabilities`: literal scope from `Child` → anonymous
   (read+chat, `capability.rs:226-231`); from `OtherUid` → denied; token →
   token scope.
6. `rpc_ext::authorize`, then `note_activity` last (`:3827`), so a denied call
   does not keep a child awake. `note_activity` ignores
   `kernel.status|health|handshake` and `mesh.*` (`project_boot_run.rs:115-125`)
   and only counts when the child's `RUNNING` is set, so the user daemon's
   copy is a no-op.
7. `chain.append` checks `is_caller_reserved_source` in the handler
   (`daemon.rs:6534`). Reserved *kinds* are not checked there, but the only
   folder of `governance.overlay.applied` reads it under source `governance`,
   which is a `KERNEL_SOURCE` the handler refuses; consistent with ADR A7.

No path lets a child or a no-proto client do more than intended. Owner CLI,
doctor, VS Code panel, child link and the relay all still work:

- macOS peer pid: tokio 1.53.1 fills `UCred::pid` via `LOCAL_PEEREPID`
  (`~/.cargo/registry/.../tokio-1.53.1/src/net/unix/ucred.rs:297-331`), so the
  fail-closed branch of `child_peer::classify` (`child_peer.rs:66`) does not
  catch the owner's CLI. A real-credential test covers it
  (`child_peer_tests.rs:25-41`).
- `handle_connection` (defaults to Owner) is reached only from tests and the
  Windows pipe loop (`daemon.rs:3319`, `:3325`); the mesh-service function of
  the same name is unrelated.
- Raw writers that bypass `stamp_request` all send `proto`:
  `clawft-rpc/src/doctor/children.rs:55`, `doctor/daemon.rs:192`,
  `commands/daemon_restart.rs:359`, `project_boot_link.rs:81-86`,
  `extensions/vscode-weft-panel/src/rpc.ts:124-125`. No other production
  writer to the kernel socket was found (the other `"method"` JSON builders
  are MCP/HTTP, not the daemon socket). The TCP relay case is documented.
- `supervised_pids` keeps a group until `killpg(pgid, 0)` is `ESRCH`
  (`project_supervisor/child.rs:62`, test
  `child_peer_tests.rs:46-66`), matching ADR A14.

Note, not a finding: a child peer with a literal scope gets `Chat`, so any
process in a child's group can `agent.chat` on the user daemon. That is the
pre-existing anonymous posture (ADR A14 says so); not a regression.

### (b) Mesh: kernel vs service after the merge

- `RouteTally`, `register_authenticated`, `ConnRoutes` RAII, the shared
  `pump` for accepted and dialled connections, and the seed redial loop are
  consistent across `mesh_runtime.rs`, `mesh_serve.rs` and
  `mesh_system_service.rs` (`adopt_tasks` / `abort_tasks` end the loops on
  stop). Dialled seeds use `Limits { idle: 300 s, ..default }`
  (`mesh_serve.rs:660`); `first_frame`/`per_ip` are irrelevant there.
- `ChainSyncOutcome` lives beside the mesh changes without conflict. Note:
  `handle_chain_sync_response` / `_outcome` have no production caller (grep:
  only `mesh_runtime.rs:993` and tests). The CHANGELOG line "chain sync stops
  cleanly at the first authority event" describes a handler nothing wires to
  a transport yet. Pre-existing, informational.
- Limits plumbing to the service: see must-fix 1.
- Low: after `register_authenticated` at admission
  (`mesh_serve.rs:406-420`) `routed` stays `false` until the first application
  frame, so a `weaver mesh peer revoke` against an admitted peer that has not
  yet sent a frame is not caught by the 250 ms route check; it is caught on
  its next frame, or by the 900 s idle timer under `enforce`. One line
  (`routed = true` after a successful `register_authenticated`) closes it.
- Torn-tail auto-accept (`mesh-service/src/state.rs:158-166`) and staged
  record publication (`main_loop.rs:316-328`, `345-430`) are self-contained;
  `status_json` exposes `last_auto_accept`.

### (c) Identity

- **Reserved sources.** `user.key.rotated` is appended under
  `project_identity::SOURCE` = `user.projects` (`project_cert_rpc.rs:425`);
  `project.anchor.reset` under `ANCHOR_SOURCE` = `project.anchor`
  (`anchor_rpc.rs:500`). Both are in `RESERVED_SOURCES`
  (`clawft-kernel/src/chain.rs:205`), so `chain.append`, `chain_bridge` and
  `append_signed` refuse forgeries of either. Clean.
- **Anchor reset epochs vs the D anchor path.** `current_epoch`
  (`anchor_reset.rs:124-141`) folds only chain events with strict +1
  continuity; `load_last` filters file and chain anchors by epoch
  (`anchor_rpc.rs:178-190`); `Accepted.epoch` is in the signed record bytes
  only when non-zero so old records keep verifying (`anchor_record.rs:31-42`);
  `restore` reads the epoch after `cached_last` has populated it
  (`anchor_rpc.rs:444-448`). `accept` adds the `chain_id` check within an
  epoch (`anchor_rpc.rs:362-366`). Consistent.
- **Rotation vs the supervisor's `user.pub` pin.** A child spawned after the
  rotation is pinned to the new key (`project_supervisor/mod.rs:213`). A child
  already running keeps the old pin; `governance.parent.push` under the new
  key is refused by the child (`overlay_runtime.rs:452`), as A13 says. One
  wording issue: A13 says "a child that re-registers (`mesh.register`) after
  the rotation is re-certified under the new key". That holds for a child
  spawned after the rotation. An adopted or still-running child signs its PoP
  with the pinned `user_key_id` (`project_boot.rs:584-587`) while the daemon
  verifies against the current key (`project_cert_rpc.rs:366`), so it gets
  `pop_failed`, which is neither fatal nor a spawn-nonce kind
  (`project_boot_run.rs:59-73`); it keeps running unregistered until the new
  liveness pass restarts it (3 missed beats + 30 s grace), and the restart
  pins the new key. Self-healing, but by a forced restart, not by
  re-certification. Suggest one clause in A13 and `kernel.md` "Rotating the
  user key". The rotate verb's output already tells the operator to restart
  children and run `weaver mesh bind rebind` (`migrate_cmd.rs`,
  `describe_rotation`).
- Not verified here: what the user daemon prints at boot when the mesh
  service's binding still names the retired user key (before `bind rebind`).

### (d) `REFUSED` sentinel

- Only the user profile writes it (`kernel_cmd.rs:230`,
  `refused_sentinel = user_profile.then(...)`). Children and the project
  profile never write it; they are supervised by the supervisor's restart
  budget, not launchd, so that is right. `clear_refused` runs for every
  profile (`daemon.rs:1151`) and is a harmless `remove_file` on a child's run
  dir.
- Mesh service units: the launchd plist is `KeepAlive true`,
  `ThrottleInterval 10` (`tests/golden/mesh-install-launchd.sh:134-137`), so a
  permanent service refusal (bind conflict, bad `mesh.toml`) cycles every
  10 s. Pre-existing, not changed by this round; follow-up card.
- The manual-start interaction is must-fix 3.

### (e) Docs / ADR / CHANGELOG

- Amendment numbering A6-A14 is continuous and the `Updated` line names each.
  CHANGELOG headers map to A10 / A7 / A8 correctly. The duplicate
  chain-authority merge (`a43bf48ad`, then `9ac7ea9e5`) differs only by a
  3-line CHANGELOG tweak; no duplicated bullets (`sort | uniq -d` empty).
- A13 says both "probes the user chain's `chain.lock`" and "holds the chain
  lock through the swap"; code holds it (`user_key_rotate.rs:110-113`). Fine.
- Behaviour-change notes present: proto refusal, overlay cap refusal, per-IP
  cap, first-frame timeout, revoke terminal (Phase 2 section). Missing: see
  must-fix 2.
- `scripts/build.sh` gains `check-doc-commands` (not in `gate`) and the
  mesh-service lane now runs the real `weaver` binary. Fine.

## Must-fix list (file:line)

1. `crates/clawft-mesh-service/src/main_loop.rs:302` (`serve_listener` →
   default `Limits`); docs `CHANGELOG.md:16-18`, ADR-103 A10 "Connection
   limits apply in every mode", `docs/guides/configuration.md` new rows.
   Minimum: qualify the docs. Proper: plumb two keys through
   `crates/clawft-mesh-service/src/config.rs:92-117` to `serve_listener_with`.
2. `CHANGELOG.md:12-66`: add entries for identity-trust (A13/A14), supervisor
   (S5-S8), mesh-service (torn-tail, publication order, read_record wait) and
   chain-authority (d); dedupe the relay clause at `:40-51`; wrap `:57`.
3. `crates/clawft-weave/src/daemon.rs:1151` + `kernel_cmd.rs:277-283` +
   `service_units.rs:120-128`: either gate the clear on a service-manager
   marker and hint from `kernel start --profile user`, or document the
   known limit in the CHANGELOG A8 bullet, ADR A8 "Refusal exits" and
   `docs/guides/kernel.md:431-440`.

## Suggested follow-ups (not blocking)

- `mesh_serve.rs:406-420`: `routed = true` after `register_authenticated`.
- ADR A13 / `kernel.md`: adopted children after a rotation heal through the
  liveness restart, not through re-certification.
- Mesh service plist: a refusal sentinel or exit-code filter, as the user
  daemon now has.
- `handle_chain_sync_*` has no production caller; either wire it or say so
  where it is described.
