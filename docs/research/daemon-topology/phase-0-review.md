# ADR-103 Phase 0 review: `integrate/p0` @ e2695fcf

Independent review of the integrated Phase 0 ("stop the bleeding") against
[ADR-103](../../adr/adr-103-weave-topology-roles-and-instances.md) Phase 0,
D1, D11 and D14, and against [analysis.md](analysis.md) sections 2 and 10.4.
Range: `cbe25dda..integrate/p0` (branches wt/doctor, wt/p1-d0-hook,
wt/p0-mesh-id, wt/audit-fix, wt/p0-runtime, integration merge). Reviewed
2026-09-30 by reading the diff and the merged sources; nothing was run against
real runtime state. The lead's `scripts/build.sh gate` on this commit finished
19/19 PASS (`GATE_EXIT=0`, run with `WEFTOS_RUNTIME_DIR` pointed at a scratch
dir). Line numbers below are from the `integrate/p0` tree.

## 1. Does Phase 0 meet the ADR?

| ADR-103 item | Verdict | Evidence |
|---|---|---|
| One resolver for node key, chain, workloads, cluster peers, socket, PID, log | Met | `crates/clawft-types/src/runtime_paths.rs:99-221` is the only resolver; `clawft-rpc/src/protocol.rs` delegates `runtime_dir/socket_path/pid_path/log_path` to it; chain default via `clawft-kernel/src/chain_storage.rs:100-160`; cluster peers, apps, revoked hosts at `clawft-kernel/src/boot.rs:838-866`; workloads at `clawft-weave/src/daemon.rs:1071`. Outside the resolver by design: `~/.clawft/cron.jsonl` (`clawft-cli/src/commands/cron.rs:9-10`) and `~/.clawft/secrets/` (documented in `docs/guides/kernel.md`). |
| Walk-up stops at `project.toml`/`weave.toml`, never `$HOME` | Met, broadened | `runtime_paths.rs:70-93`: markers also include an existing `.weftos/runtime/` and `.weftos/` in a git top-level (worktrees). Tests cover `$HOME` refusal with markers present (`:352-362`) and projects below home (`:365-372`). Note the `.weftos/runtime` marker is self-perpetuating: any dir a kernel once ran in stays a project. |
| Single-instance lock and stale-socket recovery | Met | `clawft-weave/src/instance_lock.rs:54-91` (flock, holder PID in the message), `:167-196` (reclaim only a refused socket, never a live one); taken first thing in `daemon.rs:973-982`; PID file written only after boot succeeds (`daemon.rs:1050-1053`). Beyond scope: a second lock per chain (`chain_storage.rs:293-311`, `boot.rs:232-237`). |
| `weft` prints the socket it tried and stops silent in-process fallback | Met for the swept commands | `clawft-cli/src/commands/daemon_fallback.rs:1-30`; `weft agent` requires `--local` (`agent.rs:151-163`); `weft cron add/remove/enable` refuse (`cron.rs:250,271,304`); `cron run` says unimplemented (`cron.rs:314`); `connect_or_bail` names the socket and its state (`clawft-rpc/src/lib.rs:44-49`, `probe.rs:104-111`). `mcp_*` commands still work from local config, which is CLI-owned by design (WEFT-188); I did not audit every `DaemonClient::connect()` site in `weft`. |
| Node id from the Ed25519 key, one hash (D11) | Met | `clawft-kernel/src/node_id.rs:17-25` is the one derivation; kernel boot takes the seed (`boot.rs:348-378`), cluster (`cluster.rs:from_signing_key`), daemon (`node_identity.rs:88-89`), registry, ACL (`clawft-substrate/src/acl.rs:153-157`, still accepts stored `n-` ids), ADR-025 and the ESP32 journal amended. Tests: id equal across two boots (`boot.rs` `node_id_derives_from_node_key_and_survives_reboot`), mesh id == cluster id. One-shot inspection boots use an ephemeral id and say so (`boot.rs:356`). |
| Mesh port 9489, bind failure fatal (D1) | Met | `clawft-types/src/config/kernel.rs:881` default, `listen` alias accepted; `boot.rs:594-606` binds synchronously and fails boot with the address; mesh-off never binds (test `boot_with_mesh_disabled_never_binds`). Remaining `9470` strings are historical (analysis.md) or deliberate (`docs/cogs/test-pi.md:61,92`: a Pi not yet redeployed). |
| `kernel.md:389-391` port fix | Met | `docs/guides/kernel.md` mesh example now `listen_addr = "0.0.0.0:9489"`, plus a new "Runtime directory" section. |
| Operator housekeeping of the stale socket and four `node.key` files (§10.4) | Tooling provided, action left to the operator (as the ADR intends) | `weaver doctor` / `weft doctor` list every runtime dir, stale sockets/pids (fixable with `--fix`, provably stale only: `clawft-rpc/src/doctor/runtime.rs:1-9,154-180`) and all `node.key` files with a WARN when there are several (`runtime.rs:214-227`); keys are never touched. |
| D14 resolution + handshake | Partial, by scope | The "prints what it tried" and "no silent fallback" halves landed. Resolution is still CWD → socket, and there is no `kernel.status` handshake with `{node_id, user_id, project_id, depth, parent}`; the P1 plan (package A/D) owns that and the ADR's Phase 0 row only claims the two halves. |

Also landed beyond the Phase 0 row: `chain.lock` per chain; explicit first
adoption of the legacy chain (`--adopt-legacy-chain`) with a 120 s recency
guard, and `--new-chain` (`chain_storage.rs:53-160`); honest `weaver kernel
start` (waits for socket + matching pid, prints the log tail on failure,
`daemon.rs:742-783`); the D0 authorization chokepoint (`daemon.rs:3561-3603`,
`rpc_ext.rs`); fail-closed mic selection (`mic_source.rs:1-19,93-110`);
`cron.enable/disable` RPC; cargo-audit ignore expiries and a wasmtime usage
guard (`scripts/build.sh:1080-1114`); `for_inspection()` boots with mesh and
chain off (`config/kernel.rs:309-320`).

## 2. Cross-branch integration risks

**R1 (blocking). The legacy-chain guard does not apply to the `~/.clawft`
root.** `choose_default_chain` (`chain_storage.rs:100-160`) only runs the
"no `chain.lock` beside it → require `--adopt-legacy-chain`, refuse if
modified <120 s" logic when `legacy_chain_left_behind` returns `Some`, and
that helper returns `None` unless the resolver picked a *project* root
(`runtime_paths.rs:245-253`). When the root is `~/.clawft` itself (any cwd
outside a project, which after P0 includes `$HOME`), the early return at
`chain_storage.rs:108-115` pins `~/.clawft/chain.json` with no flag and no
recency check; `ChainLock::acquire` then succeeds because the old, lock-unaware
daemon (PID 70730) holds no flock. Result: `weaver kernel start` from `$HOME`
with the old daemon still up gives two writers on `~/.clawft/chain.rvf`, the
exact failure P0 exists to stop. p0-runtime added the guard for the project
case and p0-mesh-id/doctor never looked at the legacy-root case, so no
per-branch review could see it. Fix: apply the same "no `chain.lock` yet ⇒
explicit adoption + recency check" whenever the chosen checkpoint has no lock
file, regardless of `RootSource`; one branch and one test in
`chain_storage.rs`.

**R2. `weft kernel boot --foreground` skips `kernel.lock`.** It probes the
socket, then loads the daemon's `node.key` and boots
(`clawft-cli/src/commands/kernel_cmd.rs:158-178`). `InstanceLock` lives in
`clawft-weave`, so `weft` cannot take it. While a daemon is booting (lock held,
socket not yet bound; a 46 MB chain restore takes long enough) both run with
the same node id; only `chain.lock` catches it, and only when the chain is
enabled and unpinned. Move `InstanceLock` to `clawft-rpc` (already depends on
`clawft-types`) and take it there, or refuse when `kernel.lock` is held.

**R3. The chokepoint authorizes, but identity is still self-asserted.**
`resolve_caller_capabilities` grants any comma list of
`admin,write,chat,read` a client puts in `auth` (`daemon.rs:3544-3548`). The
voice principal rides the same shortcut (`rpc_ext.rs:86-88`,
`daemon.rs:8539`), so any local client can present the identical string. Fine
for one uid on a 0600 socket, but Phase 1 F must replace the shortcut and the
D12 gate must not read `auth` contents as trust.

**R4. Voice principal × new cron capabilities.** p1-d0-hook fixed voice at
`read,chat,write`; p0-runtime classified `cron.add/remove/enable/disable` as
`Write` (`capability.rs:125-130`). A spoken Level-2 command can therefore
schedule a recurring autonomous agent job. Strictly tighter than before (the
voice path had no check), and voice routing is opt-in, but neither branch
decided this. Decide in P1 G: cron mutations `Admin`, or a voice deny-list.

**R5. Inspection boots vs RPC-first CLI.** `weft kernel status|ps|services`
and `weaver kernel status` ask a daemon first (`kernel_cmd.rs:89-115`,
`weave/.../kernel_cmd.rs:214-228`) and only then boot with mesh and chain off.
The inspection boot still resolves the root and reads `cluster_peers.json`,
`apps.json`, `revoked_hosts.json` (load-only at construction:
`cluster.rs:552`, `app.rs:485`) and prints an ephemeral node id; the text
should say it is an inspection boot, and `weft kernel boot` without
`--foreground` now prints a boot log of a chain-less kernel.

**R6. Doctor's view vs the resolver.** Doctor derives its `RuntimeSource` by
comparing the resolved dir to `~/.clawft` (`doctor/env.rs:81-88`) and lists
candidates with the old "any ancestor with `.weftos/`" rule (`env.rs:138-146`).
That is a reporting superset, acceptable, but doctor has no finding for
`kernel.lock`, `chain.lock` or "legacy chain never adopted", so it cannot
answer the one question the operator must answer before
`--adopt-legacy-chain`. A daemon from another project (PID 70730) appears
with exe and version but no socket (`doctor/daemon.rs:1-7`).

**R7. `build.sh test` isolation vs the gate.** `isolate_test_runtime`
(`scripts/build.sh:566-573`) exports one shared `WEFTOS_RUNTIME_DIR` for
every test binary in the run and installs an `EXIT` trap that would clobber
any later `EXIT` trap (only `RETURN`/`INT` traps exist today). With the env
set, `RootSource::Env` disables the legacy-adoption path, so the adoption
refusal is covered only by unit tests of `choose_default_chain`, never by a
real boot. The gate that passed here had the variable set by the caller.

**R8. Node-id and port change fallout.** The daemon's substrate prefix moves
from `n-<6hex>` to 32 hex; `cluster_peers.json` (32 KB in
`~/weftos/.weftos/runtime/`) keeps peers under the old UUID ids, which will
linger next to the re-joined ones; the Pi in `docs/cogs/test-pi.md:92` still
listens on 9470 until redeployed; `transcript_topic` defaults to empty (auto,
`config/voice.rs:709-712`) so a config that pinned `n-bfc4cd` explicitly keeps
working only while that ESP32 keeps its MAC-derived id (leaf track).

**R9. Bind defaults.** Mesh defaults to `0.0.0.0:9489` and the gateway to
`0.0.0.0` (`clawft-types/src/config/mod.rs:835`). D1 set the port, not the
address. Not a P0 regression, but the fatal bind now makes it visible.

## 3. Operator migration on the real machine

Observed read-only at review time: PID 70730 (`./bin/weaver -v kernel start
--foreground`, old build, cwd `weave-coordinator`, mesh on `*:9470`);
`~/.clawft/` holds `chain.rvf` (45.9 MB, last modified 2026-09-29 15:48),
`chain.tree.json`, `chain.key`, `node.key`, no `chain.json`, no `chain.lock`,
no socket; `~/weftos/.weftos/runtime/` holds `node.key`, `cluster_peers.json`,
logs and no socket or pid (the stale socket from analysis §2 is already gone);
`~/.weftos/runtime/node.key` exists. Four `node.key` files.

1. Install the new binaries. The old daemon keeps running from its own
   `./bin/weaver`; nothing in P0 signals it (ADR consequence honored).
2. `weaver doctor`: lists PID 70730 with exe path and version (from
   `--version`, since its socket is not in a candidate dir unless doctor runs
   inside `weave-coordinator`), four `node.key` files (WARN), and any stale
   socket/pid. It does not say that `~/.clawft/chain.*` has no `chain.lock`
   or that 70730 writes it (R6). The operator has to know.
3. Stop the old daemon from its project dir with its old binary
   (`weaver kernel stop`) or `kill 70730`; confirm with `lsof -i :9470`. This
   step is load-bearing: the new build can only detect lock-aware kernels.
4. From a project dir (e.g. `~/weftos`, a project via `.weftos/` + `.git`):
   `weaver kernel start --adopt-legacy-chain`. Refused if the chain was
   written <120 s ago (it was not); otherwise WARN naming both paths,
   `~/.clawft/chain.lock` is created, the socket/pid/`node.key` are the
   project's, the chain and key stay in `~/.clawft`. Later starts and
   `weaver kernel restart` need no flag. Alternative: `--new-chain` for a
   fresh genesis at `~/weftos/.weftos/runtime/chain.json`; `~/.clawft` is
   left untouched. Without either flag the start is refused with the recipe.
5. From `$HOME` (or any non-project dir) `weaver kernel start` roots at
   `~/.clawft` and, today, adopts the legacy chain with no flag and no guard
   (R1). If step 3 was skipped this is the two-writer case. Until R1 is fixed
   the release notes must forbid it.
6. Checks: a second `weaver kernel start` in the same project exits non-zero
   with `another kernel owns <root> (pid N)`; a kernel in another project on
   the default chain is refused by `chain.lock` naming the holder and
   `--new-chain`; `weaver kernel start` returns only once the socket answers
   and `kernel.pid` holds the child's pid, or fails with the log tail.
7. Identity: the daemon's id is now SHA-256 of `~/weftos/.weftos/runtime/node.key`,
   32 hex; peers that pinned a UUID or `n-` id see a new node. `~/.clawft/node.key`
   and `~/.weftos/runtime/node.key` become orphans doctor lists and never
   deletes.
8. Rollback: stop the new daemon and start the old binary in its project dir;
   `~/.clawft` is unmodified apart from `chain.lock`. Note that once
   `chain.lock` exists as a file the recency guard is disabled too
   (`chain_storage.rs:86-98`), so a temporary rollback to the old build
   removes the safety net for the next upgrade.

Safe and clear provided R1 is fixed or step 5 is documented as forbidden, and
provided the release notes carry: stop every pre-P0 daemon first and verify;
first start in a project needs `--adopt-legacy-chain` or `--new-chain`; mesh
default port is 9489 (update `seed_peers`, firewalls, the Pi); node ids are
32-hex and change on upgrade (re-pin peers and the mic); `$HOME` is no longer
a project; `weft agent` needs `--local` without a daemon and `weft cron
add/remove/enable/disable` need one, `weft cron run` is gone; `weaver kernel
start` can block up to 90 s and exits non-zero on a failed boot; `kernel.lock`
and `chain.lock` appear and must not be deleted while a kernel runs;
`weaver doctor` / `weft doctor` exist and `--fix` removes only provably stale
socket and pid files.

## 4. What Phase 1 inherits, and ADR-103 amendments

- **D11 is implemented**; amend the status note ("reversible until leaf keys
  are provisioned") to "landed in kernel, daemon, ACL, journal; ACL still
  parses stored `n-` ids". ADR-025 was already amended in this range.
- **Legacy-chain adoption**: the Phase 0 row does not mention `chain.lock`,
  `--adopt-legacy-chain` or `--new-chain`; record them. The P1 plan (§4,
  package E) introduces a differently named `--allow-legacy-chain` with the
  opposite direction; make E reuse the P0 family and put its
  `MIGRATED_FROM.json` refusal inside `choose_default_chain`.
- **Mic source policy** (pin > unique publisher > refuse; `node.register`
  carries no capability, `mic_source.rs:17-19`) is a D11 consequence the ADR
  does not state; add it and hand the capability declaration to the leaf track.
- **Voice principal scope**: fixed in code at `read,chat,write`; state it in
  ADR-102/103 and let P1 G decide cron exposure (R4) and how voice is placed
  relative to the D12 project gate.
- **Bind defaults**: D1 should say mesh binds `0.0.0.0` only when the machine
  role is on, and the gateway should default to loopback (R9); P1 H owns it.
- **Identity resolution**: Phase 1 F replaces the `auth` literal-scope
  shortcut (R3); the D0 seam already carries an explicit capability per route
  and runs gates after capability resolution, so F and G plug in as planned.
- **Shared lock**: move `InstanceLock` to `clawft-rpc` so `weft` can take it
  (R2). Doctor gains `kernel.lock`/`chain.lock`/adoption findings (R6).
- **Tests**: per-binary runtime isolation (nextest setup script or per-test
  `RuntimePaths::at`) instead of one shared `mktemp` and `EXIT` trap (R7); one
  real-boot test of the adoption refusal with `WEFTOS_RUNTIME_DIR` unset and a
  fake `HOME`.
- **Cleanup**: ignore or prune `cluster_peers.json` entries that fail
  `is_node_id` (R8); label inspection-boot output (R5).

## 5. Verdict

**Ship to 0.8-metaharness after one fix.** The integration does what the
Phase 0 row says, the branches compose cleanly (the resolver, node key, chain
storage and doctor agree on paths; the chokepoint covers JSON, RVF and voice),
and the gate is green. One cross-branch gap undoes the core promise in the
most common operator position and is a small change.

Blocking:

1. R1: extend the explicit-adoption and recency guard to a legacy chain with
   no `chain.lock` regardless of root source (`chain_storage.rs:100-160`),
   with a test for the `~/.clawft`-rooted boot; re-run the scoped tests.
2. Release notes as listed at the end of section 3 (the migration is safe
   only with the "stop old daemons first" step stated as mandatory).

Follow-ups (file as cards, not merge blockers): R2 shared instance lock; R3
identity shortcut (P1 F); R4 voice × cron decision (P1 G); R5 inspection-boot
labeling; R6 doctor lock/adoption findings; R7 test isolation; R8 peer-file
cleanup and Pi redeploy; R9 bind defaults; the ADR-103 amendments in section 4.
