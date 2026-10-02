# ADR-103 Phase 2 review: `wt/p2-g-integration` @ e98e455cd

This is an independent, cross-package review of the integrated Phase 2 work:
per-project kernels as children of the user daemon, with certified keys and
anchored chains. Phase 2 covers the contracts (A), the workload-kind registry
(B), project identity (C), the project chain with parent anchors and
`rule_hash` (D), the governance overlay (E), shared services through the user
daemon (F), the verified project principal (I), child boot and the
`mesh-local/1` child registry (H), the supervisor (G), the integration commit
and its review-fix round, and the merge of `0.8-metaharness` @ b178f70d5
(Phases 1 and 3, both reviewed) on top. It is measured against:

- [ADR-103](../../adr/adr-103-weave-topology-roles-and-instances.md) D6-D9,
  D13, D14 and amendments A6-A10
- the [P2 plan](../../plans/weave-topology-p2-plan.md), in particular 1
  (contracts), 3 (merge order and the e2e), 4 (migration and safety) and 5
  (risks)
- the [Phase 1 review](phase-1-review.md) and the
  [Phase 3 review](phase-3-review.md)

The range is `b178f70d5..e98e455cd` (190 files, +31.8k/-0.2k). Fable
reviewed it on 2026-10-02 by reading the diff, the merge's combined diff
(`git show --cc e98e455cd`) and the merged sources, with four delegated
read-only passes (trust chain; principal, token and shared services;
supervisor, migration and Phase 3 interplay; overlay, `rule_hash`, subscribe
and anchors) whose findings were re-read at the cited lines before they were
kept. Nothing was run; nothing under `~/.weftos` or `~/.clawft` was read or
written; no process was signalled. Line numbers are from the
`wt/p2-g-integration` tree. The lead's gate result is recorded at the end.

## 1. Verdict

**SHIP-WITH-FIXES.** The trust chain for a project kernel composes end to end
and every identity seam fails closed:

- A child starts only from a 0600, single-use, 60 s `spawn.json` whose run
  dir is named for the project and matches `$WEFTOS_PROJECT_ID`; a copied
  file is void once the ledger nonce is consumed.
- Registration binds the challenge, the client nonce, the socket, the pid and
  the project id under the project key, is authorised by the spawn ledger,
  checks the spawned root against the manifest, and is acknowledged by a user
  key signature over child-chosen randomness, so a socket squatting at the
  parent path cannot forge an enrolment.
- One `RevocationView` decides TOFU, key reuse across projects, conflicts and
  revocation, and re-verifies every certificate it reads; the journal is
  written before the chain under an exclusive lock and a missing-but-expected
  journal is corrupt, not empty.
- The child verifies the certificate against the user key it was spawned
  with, pins `user.pub`, verifies the signed parent policy and the overlay
  before it opens its chain, and signs that chain only with the project key
  `pre_boot` handed over, confirmed against the 0600 file; nothing creates a
  `chain.key` beside a project chain.
- Anchors are project-key signatures under the certificate in force, with
  `seq`, `prev_anchor`, monotone `head_seq` and a 5 min future bound; the
  user daemon seals each accepted record with its own `rec_sig`, holds one
  lock with revoke and rekey, and refuses reserved sources on every foreign
  append door (RPC, tracing bridge, replication).
- `VerifiedProject` has exactly three constructors, all crate-private, and a
  population test enumerates every `GovernanceRequest` site; a bare
  `Request.project` is never more than a claim; the relay strips `forward`
  and `project`.
- The project token is `write` only, bound to one project, refreshable only
  by its own project through a live child, delivered only through the 0600
  `spawn.json`, and the allow-list refuses every method a `ParentLink` does
  not call.
- A child has no provider key, no voice, no mesh, no local model and no
  fallback: the parent down is a typed `parent_unavailable`.
- Revocation reaches a running child (`key_revoked` on re-register, SIGTERM),
  a stopped one (the marker) and the supervisor (`project_revoked`); the
  marker has one derivation and `project.revoke` fails closed when it cannot
  write it.
- Adoption verifies pid, executable name, lock holder and handshake before a
  child is managed, re-verifies before every signal, and never signals what
  it could not verify. The real-child e2e exercises registration, the
  overlay deny with its rule hash, idle stop with the final anchor, restart
  on demand, three kills, `restart --project` and revoke.

What does not yet hold:

1. **An overlay can soften every parent hard deny into an approval prompt.**
   `limits.human_approval_required` is merged tighten-only (`false -> true`
   allowed), but in the engine that flag means "escalate blocking verdicts
   instead of denying" for every rule that is not tagged as an overlay deny.
   Parent rules are plain blocking rules, so a committed, project-controlled
   `overlay.toml` with only `human_approval_required = true` turns the
   parent's denies into `Defer` on the agent tool path, which any caller with
   `Chat` (the anonymous baseline) can approve (section 2, M1). This is the
   one hole in D8.
2. **A `Write` caller on a child's socket can brick that child's boot with
   one `chain.append`.** The rollback floor reads `governance.overlay.applied`
   by kind only and the source `governance` is not reserved (M3).
3. **Owner-facing documentation.** There is no owner procedure for Phase 2 in
   `docs/guides/` (the plan's six steps live only in the plan), the
   configuration guide does not know `profile`, `kernel.shared_services`,
   the `[serve]` fields or `overlay.toml`, the revoke remedy `weft` prints
   is wrong, and the one downgrade that silently destroys a child chain is
   unrecorded (M2). Same class as Phase 1 B2 and Phase 3 M2.

M1 is a small change in `merge`; M3 is a filter plus one reserved source; M2
is text. The should-fix list (section 3) is longer than Phase 3's because
Phase 2 has more moving parts, but none of it blocks the merge once M1 and M3
are in.

## 2. Must-fix before merge

**M1. Tag parent denies as hard denies when an overlay raises
`human_approval_required`, or refuse the raise.**
`merge_limits` treats the flag as a boolean that may only go `false -> true`
(`crates/clawft-types/src/config/overlay.rs:284-301`); `engine_params` feeds
the merged value to the child's `GovernanceGate`
(`crates/clawft-kernel/src/overlay_runtime.rs:347-352`,
`boot.rs:1505-1507`). In `evaluate`, a blocking rule contributes `hard_deny`
only when it carries `OVERLAY_DENY_TAG`; parent rules fall into
`other_blocking` (`governance.rs:1326-1330`), and the verdict is
`EscalateToHuman` whenever `!hard_deny && self.human_approval_required`
(`governance.rs:1353-1355`). `GovernanceGate` maps that to `GateDecision::Defer`
(`gate.rs:527`). The child's `agent.chat` tool gate is the overlay gate
(`crates/clawft-weave/src/daemon.rs:2081-2093`), where a `Defer` is an
interactive prompt resolved by `agent.chat.defer_decide`, a `Chat`-capability
method (`capability.rs:148`) that anonymous callers hold (`capability.rs:226-231`).

- **Failure:** `<root>/.weftos/overlay.toml` containing only
  `[limits]\nhuman_approval_required = true`. The parent's blocking rules
  (`GOV-001`, `GOV-002`, the `workload.*` default denies exported in
  `parent-policy.json`) no longer deny on the tool path; a tool call the
  parent forbids is offered for approval and approved by the project's own
  chat client. `rpc_gate.rs:47`, A2A and placement still refuse a `Defer`,
  so the hole is the agent tool path, which is the path the overlay exists to
  hold. The suite does not cover it:
  `an_overlay_deny_is_a_deny_even_when_the_engine_escalates_to_a_human`
  (`governance_overlay_tests.rs:258`) tests overlay denies under the flag,
  never parent denies under an overlay-raised flag.
- **Fix (either):** in `merge` (`governance_overlay.rs:411-430`), when the
  effective flag is `true` and the parent's is not, re-tag every blocking
  parent rule from `parent_view` with `OVERLAY_DENY_TAG` so parent denies
  stay `Deny` and only non-denied actions escalate; or refuse an overlay
  `human_approval_required = true` over a parent `false` as `Relaxes`. Add
  the missing test (parent deny, overlay flag, expect `Deny` on the tool
  gate) and a line in A7.

**M2. Owner-facing documentation.** Each is a few lines; together they decide
whether the first real `migrate-kernel` goes right.

- `docs/guides/kernel.md` (or the deployment SOP): the plan's section 4 owner
  steps, per project, in order, with the checks: stop the old daemon from
  its own directory and confirm `kernel.pid` is gone; `migrate-kernel
  --dry-run`, read it, run it; add `.weftos/project.cert.json` and
  `.weftos/state/` to `.gitignore` (`weft project init` adds only
  `.weftos/chain/` and `.weftos/project.key`,
  `crates/clawft-cli/src/commands/project_cmd.rs:208`); start; verify the
  handshake `node_id` equals `project_key_id`; the rollback line.
  `git diff b178f70d5..e98e455cd -- docs/guides/` shows only the "chain won't
  restore" section; none of this exists in `docs/guides/`.
- `docs/guides/configuration.md`: `[kernel] profile = "project"`,
  `[kernel.shared_services]` (`clawft-types/src/config/kernel.rs` diff), and
  the manifest `[serve]` fields `via`, `idle_stop_secs`, `restart_max`,
  `restart_window_secs`, `kernel_version`, `kernel_sha`
  (`clawft-types/src/project/schema.rs`). Grep finds none of them.
- `overlay.toml` format: documented only in the ADR and the plan. One section
  in the kernel or configuration guide, including "`human_approval_required`
  does not weaken a parent deny" once M1 lands.
- The revoke remedy: `SupError::Revoked` renders "key was revoked;
  re-register or rekey it" (`crates/clawft-weave/src/project_supervisor/types.rs:188`),
  but revoke is terminal and the real remedy is deleting
  `<run_root>/<id>/revoked` (A7, `cli.md`). `ensure_child_running` forwards
  only `resp.error` and drops `error_kind` (`crates/clawft-rpc/src/connect.rs:324-325`),
  so that sentence is all the owner sees. Fix the string and surface the kind.
- Downgrade warning (CHANGELOG and `kernel.md`): a pre-P2 binary over a P2
  child chain does not refuse; it ignores the unknown field, fails
  verification and *starts fresh over it* (the removed "starting fresh"
  branch; `kernel.md:70` describes the old behaviour). Never run an older
  `weaver` against a `child-kernel` project without moving the chain aside.
- `cli.md` for the new doctor findings (`child:<id>:failed|unsupervised|
  orphan|unverified`, `children`, `children:unconfirmed`,
  `legacy:<id>[:child-kernel]`, `crates/clawft-rpc/src/doctor/children.rs:92-218`)
  and the children listing in `weaver kernel status`.
- ADR A7 records a pid check at registration ("checks it against the
  supervisor's pid when one is known") that never runs on a first
  registration (S8c below); D8 limits say a push reaches placement "on the
  next rebuild", which is "until restart" (S3). Correct both when the code
  changes, or now.

**M3. Reserve the governance source and filter the rollback floor by it.**
`chain_history` selects `governance.overlay.applied` by kind only
(`crates/clawft-kernel/src/overlay_trust.rs:43-57`); the applied events are
appended under source `governance` (`overlay_runtime.rs:375-378,542-546`),
and `RESERVED_SOURCES` is `["user.projects", "project.anchor"]`
(`chain.rs:202`). `chain.append` is a `Write` route (`capability.rs:110`)
and checks only that `kind` is non-empty and the source is not reserved
(`daemon.rs:6490-6505`).

- **Failure:** any `Write` caller on a child's socket, which includes the
  project's own agents through a tool, appends
  `{"source":"x","kind":"governance.overlay.applied","payload":{"parent_version":18446744073709551615,"user_pin":true,"overlay_hash":"ff.."}}`.
  Every later parent policy is a `Rollback` refusal
  (`overlay_runtime.rs:299-305`), or the boot fails `PinMissing`/
  `OverlayMissing` (`:295,308-320`), permanently. The only recovery is to
  move the project chain aside, which runs into S2.
- **Fix:** filter `source == "governance"` in `chain_history` and add
  `governance` to `RESERVED_SOURCES` (all three foreign doors already consult
  it: `daemon.rs:6502`, `chain_bridge.rs:225`, `chain.rs:1565`). Consider
  reserving `project` (genesis) and `project.supervisor` too (N3).

## 3. Should-fix (before the 0.8.2 cut, not merge blockers)

**S1. A late failure in `revoke`/`rekey` skips the after-hooks, so "revoke is
terminal" has a gap.** `revoke` appends the journal and chain events, evicts
the session, then removes `<id>.cert.json`; a removal error returns
`IssueError::Store` (`crates/clawft-weave/src/project_cert_rpc.rs:475-484`).
`handle` runs `on_identity_change` (marker write, supervisor stop) only on
`Ok` (`:659-680`). `rekey` has the same shape: `write_cert_file` failing
(`:447`) returns before `drop_session` (`:448`) and before `sup.rekeyed`.

- **Failure:** `~/.weftos/projects` on a full disk. The journal says revoked
  and the view will refuse the old key, so the running child dies on its next
  re-register; but no marker is written and the supervisor is not told, and
  because a revoked project "has no key" (`:453`), the next `ensure_running`
  re-enrols it with a fresh key as if the owner had deleted the marker.
- **Fix:** once the journal append succeeded, treat a store error as "done,
  degraded": run the hooks, then return `revoke_marker_unwritten`-style
  failure naming the file. Same for `rekey`.

**S2. A project chain reset is a permanent anchoring dead-end with no owner
verb.** After the `kernel.md` remedy ("move the chain aside and restart") on
a project chain, the child recovers no `project.anchored`
(`crates/clawft-kernel/src/chain_anchor_parent.rs:187-199`) and submits
`seq = 1`; the user daemon answers `Seq` with its `last`
(`crates/clawft-weave/src/anchor_rpc.rs:325-327`); adoption fails `has_head`
(`chain_anchor_parent.rs:220,316-323`); every retry fails the same way.
`project.anchor.restore` never rewinds (`anchor_rpc.rs:415`) and the user
chain holds the old statements (`:189-196`), so deleting `<id>.anchor.json`
does not help. Surfaced only as `anchor_pending: true` in `kernel.status`
(`project_boot_run.rs:144,173`) and a once-per-change warn (`:468-472`).
Also: acceptance checks `head_seq` regress but not `chain_id`
(`anchor_rpc.rs:329-333`).

- **Fix:** an Admin, chained `project.anchor.reset {project_id}` on the user
  daemon (records the old head, resets the index and the file), a doctor or
  status finding when a pending anchor outlives N polls, and the dead-end
  named in `kernel.md` next to the remedy.

**S3. The placement gate never sees a pushed policy.** `PLANE` is a `OnceCell`
(`crates/clawft-weave/src/workload_place_rpc.rs:61`) and its gate snapshots
`effective_rules()` at build (`:105-123`). A `governance.parent.push` that
adds a `workload.*` deny reaches the kernel gate, the chat tool gate and the
spawned-agent gate through `swap` (`overlay_runtime.rs:167,531`) but not
placement, for the life of the child. D8 limits ("next rebuild") overstate.

- **Fix:** bump a generation in `apply_locked` that `plane()` checks, or put
  the placement `WorkloadGate` behind the same swap; or amend the ADR to
  "until restart" and report `restart_required` for pushes touching
  `workload.*`.

**S4. The supervisor exports no parent limits, and a parent policy is not
bound to a project.** `child.rs:305-312` passes `Limits::default()`, so
`max_processes` and `spawn_budget` are `None` in every spawned child's
`parent-policy.json`; the overlay can only lower what the child's own
(project-controlled) config chose. Only a manual `governance.parent.push
{limits}` sets them. `ParentPolicy::body` (`parent_policy.rs:142-152`) has no
`project_id` and versions are clock-derived (`:289`), so sibling A's later,
looser-limits policy verifies on B through `governance.parent.update`
(same-uid only, the stated limit, but a `project_id` in the signed body is
cheap).

- **Fix:** export the user daemon's `kernel.max_processes` and
  `subagents.max_per_conv` as the parent limits at spawn; add `project_id`
  to the signed body and check it on the child.

**S5. The legacy guard lets a second kernel run over a `child-kernel`
project.** `legacy_guard_with` refuses only on `RootSource::Project` plus a
user socket that accepts, and the flag always wins
(`crates/clawft-weave/src/commands/kernel_children.rs:162-186`); it never
reads the manifest. The supervisor's `prepare` refuses to start a child while
`<root>/.weftos/runtime/kernel.lock` is held
(`project_supervisor/mod.rs:191-194`), but nothing refuses the reverse order.

- **Failure:** project migrated, child running; the owner runs
  `weaver kernel start --legacy-project-daemon` in the tree. Two kernels
  serve one project with disjoint files (no corruption) but split history
  and two governance surfaces; `weft` reaches the child; the doctor reports
  `legacy:<id>:child-kernel` after the fact.
- **Fix:** when the manifest for this root has `via = child-kernel`, refuse
  regardless of the flag, naming `weaver kernel start --project <id>`.

**S6. After `weaver update`, surviving children run the old binary and
nothing says so.** `record_kernel_build` writes `kernel_version`/`kernel_sha`
at start (`project_supervisor/mod.rs:361-374`) and nothing reads them;
adoption compares the executable name only (`adopt.rs:146-150`, by design).
A project that is never idle runs the stale build indefinitely with no WARN.

- **Fix:** a `child:<id>:stale-build` doctor finding and a note in
  `kernel status`, remedy `weaver kernel restart --project <id>`.

**S7. Two readiness windows.** (a) `ensure_running` returns
`started: false` with the socket when the state is `Running | Starting` and
the pid probe is alive (`mod.rs:267-271`); during an automatic restart
`restart_once` returns before readiness (`:509-517`, detached `wait_ready`),
so a `weft` call in that window dials an unbound socket and gets the
stale-socket remedy (`connect.rs:364-370`). Fix: when `Starting`,
`wait_ready` (bounded by `ready_timeout`) before returning. (b) The adoption
scan runs once; a child whose socket is not yet bound fails the handshake
check (`adopt.rs:183-186`, "no answer on kernel.sock") and is filed as
unverifiable, never entering `slots`; `stop_all` iterates `slots` only
(`mod.rs:606-615`), so `weaver kernel stop --profile user` right after a
daemon restart can leave a booting child running while the operator believes
the cascade happened. Fix: retry `HandshakeFailed` leftovers for
`ready_timeout` after the scan, or re-scan them in `stop_all`.

**S8. Three liveness gaps in the registry and ledger.**
(a) A child that booted degraded keeps `spawn_nonce` until a registration
succeeds (`project_boot.rs:357-368`; `project_boot_run.rs` `step()` clears it
only on `Ok`), and `authorise` takes the spawn path whenever a nonce is
present (`mesh_local_rpc.rs:142-150`). After a user-daemon restart the ledger
is empty, so every attempt is `spawn_not_expected` forever, although adoption
filed an expired tombstone that would authorise a nonce-less re-register
(`project_supervisor/boot.rs:11-28`, `mesh_local_registry.rs:380-397`). The
child then has no session, no heartbeats and is never idle-stopped. Fix: on
`spawn_not_expected | bad_spawn_nonce | spawn_expired` drop the nonce and
retry. (b) `ProjectRegistry::expired_ids` (`mesh_local_registry.rs:353-365`)
has no caller outside the registry: a wedged child that holds its pid is
never restarted (plan G listed lost heartbeat as a trigger) and, with no live
session, `RegistryActivity` returns `None` ("no data means busy",
`idle.rs:66-76`), so it is never idle-stopped either. Fix: an idle-pass
sibling that treats three missed beats on a `Running` slot as a crash, pid
re-verified. (c) The spawn ledger never learns the child's pid:
`expect_spawn` files `pid: 0` (`child.rs:333-340`) and `note_pid` after spawn
updates the session map (`mesh_local_registry.rs:401-405`), which
`write_run_files` evicted at `:330`; `peek_spawn`'s comparison
(`:493-498`) is unreachable on a first registration. No security loss (same
uid), but the code and A7 promise a check that does not run.

**S9. A compromised child escalates same-uid literal admin into a durable,
relay-usable owner credential.** The ADR states the limit (a same-uid caller
sending `auth: "admin"` bypasses the allow-list); the consequence is sharper.
On the user daemon `establish()` yields no `VerifiedProject` for such a
caller (unbound, no token, no forward, `caller_principal.rs:84-99`), so the
`auth.token.issue` project guard (`token_rpc.rs:168-180`) never fires and
the child mints an **owner** token; `sanitize_line` keeps `wft_` secrets
(`relay_auth.rs:32-47`), so that token is admin over TCP until revoked. The
same literal admin reaches `project.revoke`/`rekey`/`stop_all` on siblings,
`governance.parent.push`, `shared.*` naming any `project_id`
(`shared_rpc.rs:128-133`) and `chain.append` on the user chain.

- **Narrowing that does not wait for D3:** tokio's `UCred` exposes the peer
  pid on Linux and macOS, and children run in their own process group
  (`child.rs:471`). Treat a unix peer whose `getpgid(peer_pid)` is a
  supervised child's pgid as `peer_untrusted` for literal scopes (tokens
  still work). That closes the child -> owner-token hop. Record the
  consequence in A7 either way.

**S10. Shared services.** (a) A child whose width probe fails at boot has no
`EmbeddingRouter` for its lifetime: `build_embedding_router_or_warn` returns
`None` when `!remote.is_ready()` (`daemon.rs:898-906`) and is not re-run, and
the probe (`shared.embed {texts: []}`, `parent_services.rs:45-51`) still
spends `note_call` and `try_acquire` (`shared_rpc.rs:195-205`); four shared
calls in flight (`GLOBAL_CONCURRENT = 4`, `shared_state.rs:18`) at child boot
disable the router until restart. Fix: exempt the empty-texts probe, or
build the router lazily. (b) Budget overshoot: the reservation is
`prompt + max_tokens.unwrap_or(512)` (`shared_rpc.rs:378`) but an omitted
`max_tokens` goes upstream as `None` (`:385`) and `settle` records the
actual, possibly far larger, usage afterwards (`:394-400`). Pass
`Some(min(requested, cap))` upstream. (c) Limits are cached on first use and
dropped only by `shared.reload` or a restart (`shared_rpc.rs:141-152`,
`shared_state.rs:47-64`); the `Active` check runs at load only, so an
archived project's running child keeps being served, and two projects can
fill the four global slots (card 3503efb6, confirmed). (d) Token rotation:
`refresh_token` honours the `previous` slot (`child.rs:383-389`), so a leaked
older token and the child can ping-pong refreshes indefinitely; after a
daemon restart the slot table is empty and any live project token for that
id refreshes. Same-uid only; low. (e) The project root's `.env` is loaded
into the child before the scrub (`main.rs:139` `dotenvy::dotenv()`, cwd is
the root, `child.rs:467`); provider-secret names are removed
(`project_profile.rs:107-119`) and the profile ignores `LLM_SERVICE_URL`
anyway (`llm_service.rs:104-108`), but no test boots a child with a `.env`
in its root. Add one to `shared_services_child.rs`.

**S11. Smaller items.**

- `weaver doctor` children ignores `WEFTOS_RUNTIME_DIR`
  (`crates/clawft-rpc/src/doctor/children.rs:70` uses
  `user_runtime_root(&env.home)`; `supervisor_view` then dials whatever sits
  in the real `~/.weftos/run`, `:42-65`), and `child:<id>:unsupervised` fires
  for a child the supervisor lists as `starting` or `idle-stopping` (`:115`
  matches `"running"` only): a false WARN during every restart.
- `migrate-kernel --revert` reads `user_runtime_root(home)/<id>/kernel.pid`
  (`project_migrate.rs:256-263`), not the daemon's run root; with
  `WEFTOS_RUNTIME_DIR` set a running child is not seen and `via` flips under
  it. `plan` on a moved root succeeds (`lstat_dir` NotFound is Ok,
  `:137-144`) and the owner learns `root_missing` only at `project.start`.
- A daemon crash between `write_run_files` and `launch` leaves a live
  `write` token in `spawn.json` (`child.rs:291-348`); slots are in memory
  (`:119`), so the next daemon cannot revoke it and it lives to TTL
  (3600 s). 0600 in a 0700 dir; same uid.
- `note_activity` runs before authorization (`daemon.rs` `authorize_caller`,
  `project_boot_run.rs:106-117`); denied calls keep a child alive. The
  supervisor's own probes are excluded, so supervision does not defeat idle
  stop. Already card d302d81c item 4.
- Under an explicit `allow_all` (never the user daemon's default), the
  relay's anonymous `{Read, Chat}` reaches `mesh.challenge|register|
  heartbeat|unregister` and `project.anchor.submit` (`rpc_ext.rs:408-430`,
  all `Read`); signatures stop impersonation, but `mesh.challenge` for a
  project with a live session is accepted (`mesh_local_rpc.rs:104-108`) and
  `MAX_PER_PROJECT = 4` evicts the oldest (`project_cert_nonce.rs:18,57-60`),
  so a flood can keep a child in `challenge_unknown` retries, and `submit`
  builds `current_view` before the signature check (`anchor_rpc.rs:266-268`).
  Under the default `read_only` the scope gate denies these to a remote
  caller (claim stripped, `scope_gate.rs:270-275`). Refuse them when
  `peer_untrusted` anyway, and verify against a cached view first.
- The rollback pin lives in the project tree
  (`VERSION_PIN_FILE` under `<root>/.weftos/state/`, `overlay_trust.rs:15`)
  beside the project key, while `user.pub` was deliberately put out of tree.
  Phase 4 limit; the run dir would remove the easy half.
- `boot.rs:333` derives `project_profile` from `kernel_config.profile.is_some()`;
  correct only because `KernelProfile` has one variant
  (`config/kernel.rs:299-302`). A later `User` variant would send the user
  daemon down the project chain-key path. Match on the variant.
- Sources `project.supervisor` (user chain) and `project` (genesis) are not
  reserved; a `Write` caller can forge audit noise. `ensure_genesis` is
  kind-only but runs in `post_boot` before the listener serves.
- A rekeyed-out child whose `project.key` was not replaced boots degraded on
  its cached, still-verifying certificate while the parent is down (no marker
  on rekey, by design) and runs until the parent answers `key_revoked`. One
  sentence for the known limits.
- `project_supervisor_support/rpc_test.rs:157` entered the user profile
  without a run-root override; safe only because the global supervisor was
  installed first (`:71`). Fixed in 08f7efd1b (section 8, re-check).
- `scripts/build.sh` names neither `harness = false` target
  (`project_kernel_e2e`, `project_supervisor`); they run because nextest
  drives the libtest protocol they implement
  (`tests/project_kernel_e2e.rs:77-106`). Fine, undocumented.
- The user chain carries no `rule_hash` (the provider is installed only under
  the project profile, `boot.rs:333,1171-1177`); plan section 1 said the
  user daemon's own parent hash would ride on the user chain, A7 says child
  only. Plan drift; pick one and say so.
- Naming: Phase 2's `mesh_local_registry.rs`/`mesh_local_rpc.rs` and
  `mesh.*` JSON routes (child -> user daemon) and Phase 3's
  `clawft-mesh-local` crate plus `mesh_local_glue|chain|sink|verdict.rs`
  (user daemon -> service) share a name and nothing else (section 6). A
  rename of the Phase 2 pair to `child_registry` would spare the next reader
  the grep.

### Verified with no finding

Spawn and registration:

- `spawn.json`: 0600, `O_NOFOLLOW`, owner and size checked, deleted on read,
  60 s wall clock, nonce 32/64 hex, ULID, absolute paths
  (`clawft-types/src/project/spawn.rs` `read_and_consume`); the run dir must
  be named for the id and match `$WEFTOS_PROJECT_ID` (`project_boot.rs:298-309`);
  a stale or copied file is void once the single-use ledger nonce is consumed
  (`mesh_local_registry.rs:502-513`); ledger capped at 256, challenges at
  1024 and 4 per project.
- Register order: shape, `bind_sig` over (id, nonce, client nonce, socket,
  pid), ledger authorisation, spawned root equals manifest root by
  `root_sha256` (`mesh_local_rpc.rs:244-258`, `project_cert_rpc.rs:370`),
  challenge claimed single-use and project-bound (`project_cert_nonce.rs:73-86`),
  `issue_for_register`, nonce consumed, session created, ack signed by the
  user key over child randomness (`mesh_local_rpc.rs:310-315`; checked at
  `project_boot.rs:655-679` with a `user_key_id` double-check). Addresses and
  topics restricted to the project's own (`:201-213`). One live session per
  id; 3 x 15 s expiry; heartbeat and unregister proofs under
  `weftos-mesh-local-session-v1` with pid, 30 s window and activity digest
  (`mesh_local_registry.rs:256-301`).

Certificates and identity:

- Manifest required; PoP v2 binds op, user key id, nonce and id
  (`cert.rs` `pop_signed_bytes`); TOFU: existing key idempotent, second key
  `KeyConflict`, key owned by another project `KeyReuse`, revoked key
  `KeyRevoked` (`project_identity_view.rs:249-257,278-301`); journal before
  chain under an exclusive flock; a corrupt or missing-but-expected journal
  fails closed (`project_cert_store.rs:65-72`). Tests
  `no_cert_for_another_projects_id_or_root`,
  `no_cert_for_a_pubkey_the_caller_cannot_prove`,
  `a_key_cannot_certify_two_projects`.
- Child side: certificate must be user-signed by the `spawn.json` key, for
  this id and this key (`project_boot.rs:479-496`); `user.pub` must equal the
  spawn key (`:424-437`) and is rewritten atomically at every spawn
  (`overlay_trust.rs:65-70`); overlay and policy verified before the chain
  opens (`:388-389`); node id asserted equal to the certified key id
  (`:204-210`). Trust root re-checked on boot, reload and update: marker,
  certificate shape, issuer, expiry, project, `user.pub` disagreement refused
  (`overlay_trust.rs:77-125`), key beside the certificate must be the
  certified one (`:112-123`).
- One key: `load_or_create_project_key` refuses symlink, non-regular file,
  foreign owner, group or world bits, a loose parent dir and a wrong size,
  and never chmods (`project_identity.rs:148-248`); boot signs a project chain
  only with the seed `pre_boot` handed over, confirmed against the 0600 file,
  and the project branch precedes the `user.key` branch (`boot.rs:993-1005`);
  `post_boot` re-checks the chain's verifying key; the e2e asserts the chain
  verifies under the certified key and no `chain.key` exists
  (`scenario.rs:224-229`).
- `project.register` the RPC (manifest, Admin) and `project.register` the
  chain kind (source `user.projects`, reserved) do not collide; the
  certificate is issued only through `mesh.register` -> `issue_for_register`.

Anchors and chain:

- Project-key signature under the certificate in force, revoked and
  rekeyed-out keys refused (`anchor_rpc.rs:224-253`); `seq == last + 1`,
  `prev_anchor`, monotone `head_seq`, `at <= now + 300 s`; identical resend
  idempotent; backoff only after authenticated refusals (`:286-341`); record
  sealed with `rec_sig` by the user key and re-verified against every
  certificate ever issued (`anchor_record.rs:40-78`); publish before write
  (`:334-340`); `revoke`/`rekey` hold the accept lock
  (`project_cert_rpc.rs:416,456`); child adoption of a parent "last" requires
  own key, exact next seq and that the project chain holds that head
  (`chain_anchor_parent.rs:299-326`); single in-flight bounded submit, pending
  file written before send with only the latest kept (`:329-399`); recovery
  filters `source == project.anchor` so a `chain.append` caller cannot plant
  a baseline (`:187-199`).
- `rule_hash`: hashed only when `Some`, with a tag after a fixed-width tail
  (`chain.rs:329-336`); flag bits checked on RVF load (`:929-941,2062-2069`);
  JSON and RVF round trips and the old-binary strip test
  (`tests/chain_p2_rule_hash.rs:96,136`); pre-P2 fixture verifies unchanged;
  `append_signed` recomputes with `rule_hash` and refuses reserved sources
  (`chain.rs:1565,1612-1627`); the P2 binary refuses rather than overwrites an
  unrestorable chain (`boot.rs:1089-1130`) and `--new-chain` does not touch
  it; the e2e asserts every event after the first applied event is stamped
  (`scenario.rs:260-262`).
- Reserved sources enforced on all three foreign doors: `chain.append`
  (`daemon.rs:6502`), tracing bridge (`chain_bridge.rs:225`), replication
  (`chain.rs:1565`).
- Subscribe: receiver created and `end` recorded under the chain lock, paged
  replay, live events below `end` dropped (`chain_subscribe.rs:144-184`,
  `chain.rs:1460-1487`); bounded broadcast with `Lagged`; scoped tokens
  cannot read `user` and never inherit the bound project
  (`chain_subscribe_rpc.rs:80-93,183-213`); the intercept runs after
  `authorize_caller` (`daemon.rs:3864-3900`).

Overlay:

- `permit`, `deactivate` and unknown keys refused (`overlay.rs:325-330`); id
  shadowing by trim and case, duplicates (`:233-253`); empty or malformed
  globs (`:216-231`); `Relaxes` on numbers and `true -> false` (`:270-301`);
  a parent-unset threshold is capped at the engine default
  (`governance_overlay.rs:419-422`); `max_processes = 0` refused (`:432-436`);
  an approval glob overlapping a parent deny refused (`:437-446`); an
  approval never softens another blocking rule (`governance.rs:1326-1329`).
- Swap keeps exemptions, rate limit and scorer (`gate.rs:365-372`,
  `governance.rs:1172-1181`); the read lock spans evaluation so a decision
  never pairs with the wrong hash (`overlay_runtime.rs:167-186`).
- Rollback floor from pin plus chain max, `PinMissing`, `OverlayMissing`
  (`overlay_runtime.rs:292-323`); a `write_pin` failure refuses boot and
  aborts an update before the swap (`:323,528-531`); reload of a bad or
  deleted overlay keeps the running rules and chains
  `governance.overlay.rejected` (`:453-470,537-556`); an older file on disk
  is ignored (`:465-467`); `apply_parent_update` verifies the signature before
  anything (`:494`); `governance.parent.push` takes only `project_id` and
  `limits` from params and snapshots the daemon's own engine
  (`governance_push.rs:175-195`).

Principal and token:

- Constructors `from_bound`, `from_token`, `from_verified_forward`, all
  `pub(crate)` with a private field (`verified_project.rs:37-62`), with a
  compile-fail doc test; `reconcile` requires every present source and the
  claim to agree, else `project_scope_mismatch` (`caller_principal.rs:47-75`);
  the claim alone is never verified; `GatePrincipal.project_id/instance_id`
  are `skip_deserializing` (`governance.rs` diff), so no payload or client
  map can assert a project; `attributed_with` strips and re-stamps from the
  instance attestation only (`governance_project.rs:67-82`). The forward
  header binds method, params hash and target key id, 5 s window, single
  use, signature before the replay table (`project_forward.rs:125-132,202-240`);
  the only stamper is the supervisor's `ChildIo` (`project_supervisor/io.rs:77`).
- The population test scans every crate minus `#[cfg(test)]`, pins where
  `from_verified` and `.with_project(` may appear, enumerates every
  `GovernanceRequest::new` and literal site and forbids
  `"project"`/`"project_id"` reads there
  (`tests/project_principal_population.rs:260-431`).
- `TokenScope::Project` is `["write"]` -> `{Read, Write}`
  (`token_authority.rs` diff, `capability.rs:251-279`); TTL 3600 s, refresh
  at 900 s, at least 30 s apart (`token_consts.rs:5-7`,
  `parent_link.rs:58,285-299`); the allow-list is exactly `shared.*`,
  `kernel.handshake`, `project.token.refresh`, enforced before dispatch
  (`project_token_scope.rs:32-40`, `daemon.rs:3788-3793`); cross-project
  refresh refused (`child.rs:377-380`) and a live child required (`:390`);
  the chain stores a sha256 only (`token_authority.rs:535-549`); `Debug`
  redacts (`spawn.rs:64-72`, `parent_link.rs:178-187`); `kernel.status`
  exposes health words only (`daemon.rs:5790`). Phase 1 S3 is closed: a
  token's `project` is enforced (`caller_principal_tests.rs:45-55`,
  `shared_services.rs:85-90`).
- Scope gate: `chain.subscribe` read-only; `project.*` lifecycle and
  certificate verbs, `governance.parent.*`, `project.anchor.submit`,
  `project.token.refresh` and `mesh.*` are user-level (Admin, or inside a
  project) (`scope_gate.rs:92-94,130-158`); `shared.` added to the
  classified prefixes; the registry re-check applies to verified projects
  too (`:241-256`); unit tests never read the real store (`:197-199`).

Shared services and the child profile:

- `shared.*` refused on a child (`shared_rpc.rs:172-174`); anonymous, `read`,
  `chat` and forged tokens never reach the handler
  (`tests/shared_services.rs:95-103`); handlers are stateless, unknown params
  refused, no `conv_id` or session (`shared_rpc.rs:85-86,242-251,339-341`;
  test `a_project_only_gets_context_it_sent`); `stream: true` refused; upstream
  bodies never relayed (`:230-240`); `shared.use` carries ids and a count only
  (`:154-164`). Rate limit counted before parsing, minimum 8 tokens per call,
  reservation before work (`shared_meter.rs:33,152-210`); model only from the
  parent's list (`shared_state.rs:91-112`).
- Child side, no fallback: `project_embedder()` is `Some` whenever the link
  exists (`project_profile.rs:245-253`), so the `OPENAI_API_KEY` branch
  (`daemon.rs:908`) and `select_embedding_provider` (`:1755`) are unreachable
  in the profile; `new_client` attaches the parent backend
  (`llm_service.rs:198-203`) and `LlmClient::with_backend` never touches HTTP
  or an API key; parent down is a typed `parent_unavailable` with a 15 s
  monitor and 1-30 s backoff (`parent_link.rs:53,381-393,440-462`); tests
  `stopped_parent_gives_parent_unavailable_never_a_local_result` and the
  decoy listeners in `shared_services_child.rs:54-63,162-166`.
- Environment: `env_clear()` then exactly `HOME`, `PATH`, locale,
  `WEFTOS_RUNTIME_DIR`, `WEFTOS_PROJECT_ID` (`child.rs:55-57,125-146,465-466`),
  argv exactly `kernel start --foreground --profile project --project <id>`;
  `env_probe` reads the real environ (`env_probe.rs:17-36`). Voice: consumer,
  `talk_loop`, `voice_loop`, mesh, provider and web-search keys cleared for
  both config copies (`project_profile.rs:171-199`) at the top of `run()`
  (`daemon.rs:1021`); mic supervisor gated (`daemon.rs:1311`); voice tools
  stripped on process state, not config, so a project `weave.toml` cannot
  re-enable them (`project_profile.rs:125-136`, `daemon.rs:1650`).

Supervisor:

- Per-project async gate on start, stop, restart and revoke
  (`mod.rs:266,526,591,633`; test `concurrent_ensure_running`); a generation
  counter retires a superseded monitor (`:423-425,475,534`); backoff sleeps
  outside the gate and re-checks under it (`:473-477`); `stop_requested` is
  set before any signal so a stopped child is never restarted (`child.rs:502`,
  `mod.rs:427`); owned children get `killpg` on their own group
  (`child.rs:471,570-573`); adopted pids are re-verified before SIGTERM and
  SIGKILL and group-killed only when group leader (`:507-515,562-583`);
  zombies after a re-exec are reaped with `WNOHANG` (`:163-170`). Adoption
  needs pid alive, exe name from `state.json`, lock held by that pid, and a
  handshake naming id and pid (`adopt.rs:142-156,176-187`); adopted children
  get a tracker, a monitor and an expired registry session for re-register
  (`project_supervisor/boot.rs:45-66`); a revoked project's leftover is
  stopped, everything else listed, never signalled (`:69-90`). Idle:
  `should_stop` needs a live heartbeat session and no data means busy
  (`idle.rs:42-56,70-78`); `idle_stop_secs()` defaults to 1800 for
  `child-kernel` in the schema (`schema.rs:188-196`). Units:
  `AbandonProcessGroup` and `KillMode=process` in both goldens.
- The only permit for the `project` kind is `project_supervisor_permit()`
  on the supervisor's own gate; it is not installed in the operator gate and
  `ProjectKind::load` always refuses, so `workload.place` of a project stays
  denied (A7, confirmed by the kind tests).

## 4. Safety properties (plan 4) and the integration e2e (plan 3)

| Property | In the suite | Only the owner's machine |
|---|---|---|
| A second kernel for one project cannot start | Child `kernel.lock` plus one live session per id (`mesh_local.rs` second-session case); `prepare` refuses while the Phase 0 lock is held (`mod.rs:191-194`) | The reverse order with `--legacy-project-daemon` is not refused (S5) |
| A child never opens a mesh listener | `mesh.enabled` forced off, `prepare` skipped, state `off` (`daemon.rs:1119-1127`, `project_profile.rs:171-180`); `tests/project_profile_refusal.rs:42` asserts no `node.key` in the run dir | With the user daemon in service mode: child handshake `mesh.mode = off` |
| No provider key, token-authority secret or model in the child | `env_probe` over the real environ; decoy listeners untouched; no local embed or LLM path reachable (`shared_services_child.rs`) | A project root with a `.env` (S10e) |
| Project key refused unless 0600 | `project_identity.rs:148-248` and its tests | - |
| A corrupt or relaxed overlay never yields fewer rules than the parent | Merge tests, property test (deny superset, limits `<=`), boot refusal, reload keeps old rules | **Except `human_approval_required` (M1)** |
| User-daemon stop leaves no unsupervised child unless `--keep-children` | CLI cascade `project.stop_all` (`kernel_cmd.rs`) | A child mid-boot at that moment (S7b) |
| `weaver update` restarts only the user daemon; children adopted after | Adoption tests in `project_supervisor.rs`; e2e adoption scan | Real launchd/systemd units with live children, a separately installed binary as the child (A7 known limit); stale-build visibility (S6) |
| Registration, certificate, anchors, overlay deny with rule hash, idle stop with final anchor, restart on demand with the same node id and serial 1, three kills then `failed`, `restart --project`, revoke terminal | `tests/project_kernel_e2e.rs` (real `weaver kernel` code path as the child) | Revoke on a project whose cert file cannot be removed (S1); anchoring after a chain reset (S2) |
| Nothing touches `~/.weftos`, `~/.clawft`, a running daemon | See section 8 | - |

## 5. Migration safety (plan 4)

- **"Never touched" holds.** Reads under `<root>/.weftos/runtime/`:
  `kernel.lock` by shared non-blocking flock on `File::open`
  (`adopt.rs:118-126`) and `kernel.pid` (`project_migrate.rs:94-105`).
  Writes: `<root>/.weftos/state/` (0600, `O_NOFOLLOW` both sides, temp plus
  rename, `:201-237`), `~/.weftos/run/<id>/`, `<root>/.weftos/{project.key,
  project.cert.json,chain/}`, `~/.weftos/projects/<id>.cert.json`. `node.key`
  is never read or carried (`:316-323`). No `~/.clawft` write in the diff.
- **`migrate-kernel`.** Lock-or-live-pid refusal with "was not signalled"
  (`:163-166,246-250`); `Already` idempotency (`:167-169`); symlink checks
  repeated at apply (`:148-158,190`); `--revert` refuses while the child runs
  (`:256-263`, but reads the home-derived run root, S11) and leaves the
  copies; rollback line (`:271-279`); `plan` on a moved root does not refuse
  (S11).
- **The legacy guard.** Refuses only `RootSource::Project` beside a user
  socket that accepts (`kernel_children.rs:162-186`); the flag survives
  `daemonize` (`daemon.rs:684-688`) and the SIGHUP re-exec
  (`boot_refusal.rs:117-124`); `WEFTOS_RUNTIME_DIR` -> `RootSource::Env` ->
  skipped (test `:243-244`); no user daemon or a stale socket -> allowed
  (`:238`). It does not protect a migrated project from the flag (S5).
- **Existing chains and keys.** The project chain is fresh with
  `project.genesis` naming the user-chain head (plan decision 6); the legacy
  chain the old daemon used is untouched. The user chain gains
  `project.register`, `project.anchor`, `project.kernel.*` and
  `project.supervisor` events; that is by design and should be stated beside
  Phase 3's A10 line.
- **Resolution after migration.** flag > env > manifest `runtime_dir` > child
  run dir > `via = user-daemon` > user default > Phase 0
  (`resolve.rs:694-706`); `ensure` only when the child run dir decided the
  root (`:740-746`); explicit endpoints never ensure; a copied tree is refused
  the original's child (`:615-624`); one `ensure` call, only when the first
  dial fails (`connect.rs:353-363`), 45 s bound over the 30 s `ready_timeout`;
  user daemon down -> a clear message (`:311-315`); project mismatch after
  ensure stays hard (`:208-214`).
- **The one owner trap:** running an older `weaver` against a `child-kernel`
  project overwrites its chain (M2, downgrade line).

## 6. Phase 3 after the merge

- **Identity selection in `daemon.rs`.** A child takes
  `DaemonIdentity::local(project key)` from `pre_boot`, skips
  `mesh_boot::prepare` and sets mesh state `off`
  (`daemon.rs:1119-1127`); every other daemon runs Phase 3's decision first.
  `prepare` would in any case refuse `required` only when `m.enabled`
  (`clawft-kernel/src/boot.rs:276-285`) and `apply_to_config` forces
  `enabled = false` (`project_profile.rs:171-180`; `MeshConfig::default()` is
  disabled), so a project `weave.toml` with `service = "required"` cannot
  refuse a child boot. `refuse_fresh_identity` (Phase 3 S2) is reachable only
  inside `prepare` and depends on the user daemon's own key, so it cannot
  misfire for a child or a user daemon with children. A child never builds an
  endpoint, never pins `machine.pub`, never generates `node.key`, never
  appends `mesh.service.bound`.
- **Service mode on the user daemon.** `attest_instance` is a no-op when
  unbound (`caller_principal.rs:104-113`), so the machine node id is never
  stamped as an instance project. Certificate issuer, parent-policy signer and
  the pinned `user.pub` are all `chain.signing_key_clone()`
  (`project_supervisor/boot.rs:168-190`, `child.rs:298-311`), so Phase 3's
  `chain.key -> user.key` migration (same seed, diverged refused) cannot split
  issuer and pin. Nothing in the supervisor, lifecycle RPCs, migrate or the
  anchor RPC consults `daemon_identity`, so Phase 3 S9 (machine pubkey in
  three registries) does not reach Phase 2.
- **`resolve.rs` precedence** is as the merge message says and
  `resolve_tests.rs` now asserts `via = user-daemon` from the manifest
  (`ResolveSource::Manifest`), closing the Phase 1 B1 shape for both vias.
- **Handshake binding.** `compute_bound` binds `RootSource::Child` to its id
  first (`handshake_rpc.rs:61-67`), then a project root by `project.toml`,
  then a single manifest claiming the runtime dir; the e2e asserts the
  child's handshake names the project and the pid.
- **No route or scope collision.** Phase 2's `mesh.challenge|register|
  heartbeat|unregister` are JSON-RPC routes with capability `Read`
  (`rpc_ext.rs:408-430`) and are user-level in the scope gate; Phase 3
  speaks the framed `clawft-mesh-local` protocol on the service socket and
  has no JSON route starting with `mesh.`; the pre-existing
  `mesh.revoke|unrevoke|revoked` are the kernel's own revocation list. Phase
  3's `mesh.service.bound`/`mesh.journal.anchor` ride source `mesh.link`; a
  child chains none. The doctor compares `mesh.mode` only for `profile ==
  user` (`mesh_doctor.rs:194-202`); a child reports `off` and is reported,
  not compared. The shared name is the only overlap (S11, naming).

## 7. Doc and ADR drift

| Text | Code | Evidence | Recommendation |
|---|---|---|---|
| A7 overlay: `human_approval_required` "false->true only" is tightening | It escalates parent denies | `governance.rs:1353-1355` | M1, then say so |
| A7 (H): pid "checked against the supervisor's pid when one is known" | Unreachable on a first registration | `child.rs:333-340`, `mesh_local_registry.rs:330,401-405,493-498` | S8c, or drop the sentence |
| D8 limits: a push reaches placement "on the next rebuild" | `PLANE` is a `OnceCell`; never rebuilt | `workload_place_rpc.rs:61,105-123` | S3, or "until restart" |
| Plan 1: the user daemon's parent hash rides on the user chain | Provider installed under the project profile only | `boot.rs:333,1171-1177` | Amend the plan or A7 |
| Plan G: lost heartbeat is a restart trigger | `expired_ids` has no caller | `mesh_local_registry.rs:353-365` | S8b |
| `types.rs:188` "re-register or rekey it" after revoke | Revoke is terminal; delete the marker | A7, `cli.md` | M2 |
| `kernel.md:70` "earlier builds logged starting fresh" | A pre-P2 binary still does, over a P2 child chain | removed branch in `boot.rs` | M2 downgrade line |
| `kernel.md`, SOP: no Phase 2 owner procedure | Plan 4 steps exist only in the plan | `git diff -- docs/guides/` | M2 |
| `configuration.md`: `profile`, `kernel.shared_services`, `[serve]` fields, `overlay.toml` | Undocumented | grep | M2 |
| Plan 4 step 4: "`weft project init` already adds the first and third" | Adds `.weftos/chain/` and `.weftos/project.key` only, and only when a `.gitignore` already exists; `migrate-kernel` never touches it | `crates/clawft-cli/src/commands/project_cmd.rs:203-208` | Owner adds `project.cert.json`, `state/`; say so in M2 |
| `cli.md`: `weaver kernel status` "also lists the children" | Only with `--profile user` | `kernel_cmd.rs:351-354` | Add the flag to the example |
| `cli.md` `[serve]` fields | `kernel_version`, `kernel_sha` written by the supervisor, documented nowhere | `project_supervisor/mod.rs:361-374` | One line |
| Doctor children findings | All eight are `Warn` except `children` (`Ok`); none in `cli.md`/`kernel.md` | `doctor/children.rs:91-219` | M2 |
| A7: env allow-list, token allow-list, TTL 3600, refresh 900 s/30 s, spawn 60 s, heartbeat 15 s x 3, forward 5 s, replay 30 s, challenge retry 45 s, socket 103 bytes, idle 1800, backoff 1-30 s | Match | `child.rs:125-146`, `project_token_scope.rs:32-40`, `token_consts.rs:5-7`, `parent_link.rs:58,285-299`, `spawn.rs`, `mesh_local_registry.rs:256-301`, `project_forward.rs:125-132`, `project_boot.rs:255-284`, `schema.rs:188-196` | None |
| CHANGELOG, `cli.md` commands and flags | All exist in clap | `kernel_cmd.rs`, `kernel_children.rs`, `project_cmd.rs` | None |
| `cli.md` restart defaults 5 and 60 s | Match (`DEFAULT_RESTART_*`) | `schema.rs:180-190` | None |
| A7 known limits | Match the code as read | sections 3 and 6 | Add S1, S2, S9's consequence, the rekeyed-degraded child, and the user-chain event kinds |
| Phase 1 S3 (token `project` never enforced) | Closed | `caller_principal_tests.rs:45-55` | Note in A8 |
| Phase 1 S7, S8 | Unchanged by this range | - | Still open |

## 8. Test isolation

The earlier leak (a `mesh_local` test writing the revoked marker under the
real `~/.weftos/run`) was the manifest-derived marker path; the integration
commit replaced it with `runtime_paths::revoked_marker(run_root, id)` and the
e2e's first step asserts writer, supervisor and child name the same file
under `$WEFTOS_RUNTIME_DIR` with nothing under the manifest store
(`scenario.rs:117-134`). Remaining paths to the real home:

| Test | Isolation | Residual |
|---|---|---|
| `tests/mesh_local.rs` | `user_daemon::enter_at(scratch_run())` (a process-wide tempdir, `:51-58`), tempdir `HOME` and manifest store per world (`:64-76`) | None found; every socket it dials is under its tempdir |
| `tests/project_cert_rpc.rs` | `enter_at(<tempdir>/.weftos/run)` (`:155`) | None |
| `tests/project_kernel_e2e.rs` | `harness = false`; `HOME` and `WEFTOS_RUNTIME_DIR` set in `main` before any thread (`:107-116`); children get an allow-list env; `Reaper` kills only pids whose exe is the test binary | None; `/tmp` is used instead of `$TMPDIR` for `sun_path` (`world.rs:36-45`) |
| `tests/project_supervisor.rs` + `project_supervisor_support/` | `harness = false`, `/tmp/wsup*` fixture (`:108`) | Was latent at e98e455cd: `rpc_test.rs:157` called `set_user_profile(true)` with no run-root override and relied on `install_global` having run first (`:71`, `project_cert_rpc.rs:196`). Fixed in 08f7efd1b (`enter_at(&fx.run_root)`, `:156-159`). None now |
| `tests/project_cert_rpc.rs` | At e98e455cd the first two tests pinned nothing (order-dependent). Fixed in 08f7efd1b: one `init_run_root` tempdir for the binary, pinned in `spawn` before any daemon (`:26-38`) | None now |
| `tests/shared_services.rs` | - | Read-only: `token_rpc.rs:74,87` reads the legacy `~/.clawft/auth-tokens.jsonl` when present; no write |
| `tests/project_profile_refusal.rs`, `tests/shared_services_child.rs` | `set_var("WEFTOS_RUNTIME_DIR", <tempdir>)` in `unsafe` blocks (`:11-13`, `:44-49`) | Each is a one-test binary today; a second `#[test]` in either file would race the env. Note it in the file header |
| `tests/shared_services.rs`, `tests/common/mod.rs` | In-process stand-in, isolated chain, `SERIAL` mutex, stub embedder and LLM, no network (`common/mod.rs:1-30`) | None |
| `clawft-rpc` `resolve_tests.rs` | Tempdir worlds, `user_runtime_root(&w.home)` | None |
| Production helpers | `project_cert_rpc::user_run_root` prefers the supervisor's root, then `RUN_ROOT`, and under `cfg!(test)` returns `None` unless `WEFTOS_RUNTIME_DIR` is set (`:195-210`); `scope_gate::manifests_dir` returns `None` under `cfg!(test)` (`scope_gate.rs:197-199`) | Both guards cover unit tests only; integration tests must pin, and all of the above do |

## 9. Follow-ups (ranked) and the existing cards

1. **M1** overlay flag softens parent denies. Security; one merge change.
2. **M3** forged `governance.overlay.applied` bricks a child. One filter.
3. **S1** revoke/rekey late failure skips the hooks. Correctness of
   "terminal".
4. **S2** anchoring dead-end after a chain reset; `chain_id` unchecked.
   Needs an owner verb.
5. **S9** child -> owner token over the relay; pgid narrowing.
6. **S5** legacy guard versus a migrated project; **S6** stale build after
   update; **S7** two readiness windows. Operability on the owner's machine.
7. **S8** degraded child never re-registers, lost heartbeat not a trigger,
   dead pid check.
8. **S3** placement gate and pushes; **S4** parent limits and `project_id`
   in the policy body.
9. **S10** shared services (width probe, overshoot, cache, rotation, `.env`).
10. **S11** doctor run root and false WARN, migrate revert root, spawn.json
    token after a crash, activity before auth, relay cost under `allow_all`,
    in-tree pin, `KernelProfile` match, unreserved sources, naming,
    `build.sh` note, plan drift on the user-chain hash.
11. **M2** documentation (blocks the merge as text, not as code).

Existing cards, against this tree:

- **d302d81c (P2-G follow-ups):** item 1 (one marker derivation) is done in
  the integration commit and asserted by the e2e; close it. Items 2
  ("unmanaged kernel pid N" message), 3 (restart after a repair-revoke is
  refused), 4 (`note_activity` after authorization, confirmed open), 5
  (non-unix stub build, recycled pid between SIGTERM and SIGKILL) remain.
  Add S7b (mid-boot child escapes `stop_all`) and S8c here.
- **3503efb6 (P2-F follow-ups):** item 1 (invalidate shared limits on
  archive/unregister) and item 3 (fairness, four global slots) remain,
  confirmed (S10c). Item 2 (project-profile `daemon::run` smoke test with
  decoys) is substantially covered by `shared_services_child.rs` and the
  e2e's real child; keep only the `.env`-in-root case (S10e). Add S10a
  (width probe) and S10b (overshoot).
- **9a0594bb (P2-G requirements from F):** all four done (env allow-list
  with `env_probe`, 0600 `spawn.json` with a `write`-only TTL token and
  `ParentLink` refresh, `--profile project` forced in `daemon::run`, default
  parent socket asserted in `resolve_tests`). Close.
- **07f01814 (P2-A obligations):** H (hard-fail without a `Child` root or on
  an id mismatch) done (`project_boot.rs:298-309`; the `KernelProfile`
  brittleness in S11 is the residue). I (population test, forward binds
  project id) done. E (`max_processes`/`spawn_budget` enforced) is boot-only
  and the supervisor exports none: open, now S4. The signer-tag ADR note and
  the JS `u64` note remain. Keep for E and the two notes.
- **be785757 (P2-B follow-ups):** untouched by this range; unchanged.

New cards to file: P2-REVIEW M1, M3, M2 (docs), S1, S2 (anchor reset verb),
S3 (placement swap), S4 (limits export, policy `project_id`), S5, S6, S7a,
S8a/b, S9 (pgid narrowing), S11 (doctor run root, migrate revert root,
reserved sources, `KernelProfile` match, naming).

## 10. Owner real-use checklist (one Mac, one or two projects)

What only running real project kernels on the owner's machine can verify.
Pick a throwaway project first, then the real one.

Record before anything: `weaver kernel status --profile user` -> user daemon
node id; `ls ~/.weftos/projects/` and a copy of that directory and of
`~/.weftos/chain/` outside `~/.weftos`. Do **not** put
`human_approval_required = true` in any `overlay.toml` until M1 lands.

1. **Preconditions.** User daemon running (Phase 1; with Phase 3, note
   `Mesh:` mode). `weft project list` shows the project. From the project
   directory, if a Phase 0 daemon runs there: stop it with its own binary,
   then `lsof -U | grep '<root>/.weftos/runtime/kernel.sock'` empty and
   `<root>/.weftos/runtime/kernel.pid` gone.
2. **Dry run.** `weaver project migrate-kernel <id> --dry-run`. Expect: the
   two copies (`workloads.json`, `apps.json` -> `<root>/.weftos/state/`), the
   `via` flip, the rollback line; no mention of `node.key` or the chain.
   Then run it. Check `~/.weftos/projects/<id>.toml` has
   `via = "child-kernel"`, `<root>/.weftos/state/` holds the copies, and
   `<root>/.weftos/runtime/` has the same mtimes as before.
3. **gitignore.** Add `.weftos/project.cert.json` and `.weftos/state/`
   (init added only `.weftos/chain/` and `.weftos/project.key`). Optional:
   `<root>/.weftos/overlay.toml` with one `[[deny]]` (no `[limits]` yet).
4. **Start.** `weaver kernel start --project <id>`. Expect a handshake line
   naming the project. `weft project show .` -> `node_id` equals
   `project_key_id` from the user daemon's `project.cert.show` (serial 1).
   `ls -l <root>/.weftos/project.key` -> `-rw-------`; `<root>/.weftos/chain/`
   has no `chain.key`; `~/.weftos/projects/<id>.cert.json` exists.
   `~/.weftos/run/<id>/` holds `kernel.sock kernel.pid kernel.lock kernel.log
   parent-policy.json state.json user.pub` and no `spawn.json`.
   `weaver kernel status` (user daemon) lists the child as `running`.
5. **Chain and anchors.** On the user chain: one `project.register`; within
   5 minutes (or after step 7) a `project.anchor` naming the project. On the
   child: `kernel.status` shows `shared_services` `embeddings`/`llm` as
   `parent` and `anchor_pending` false.
6. **Overlay.** With the deny in place, the denied action fails with
   "governance denied ... <rule id>" on the child; the child chain's
   `governance.deny` carries `rule_hash` and `principal.project_id`. Edit the
   overlay to remove the deny; nothing changes until `governance.reload`
   (or a restart); then it permits.
7. **Idle and on-demand restart.** Leave the project quiet for
   `idle_stop_secs` (set `idle_stop_secs = 120` in the manifest for the
   drill). The child exits cleanly, `project.kernel.idle_stop` and a final
   `project.anchor` land on the user chain, `kernel status` shows `stopped`.
   Run any `weft` command in the project: the child starts on demand, same
   `node_id`, serial still 1, no second `project.register`.
8. **Crash.** `kill -9 $(cat ~/.weftos/run/<id>/kernel.pid)`. Within a few
   seconds `kernel status` shows it `running` with a new pid;
   `project.kernel.exited` on the user chain has `restart_in_ms: 1000`.
9. **User daemon restart with children.** Regenerate and reinstall the user
   unit (`weaver service unit`; check `AbandonProcessGroup` in the plist).
   `weaver kernel stop --profile user --keep-children`; the child pid is
   still alive; start the user daemon; `kernel status` lists the child as
   adopted with the same pid; `weaver doctor` has no `child:*` finding
   (expect a false `unsupervised` WARN only during a restart, S11). If the
   previous daemon exited uncleanly, the child's `shared_services` read
   `down` until `weaver kernel restart --project <id>` (A7 known limit).
   Then `weaver kernel stop --profile user` without the flag: the child stops
   first.
10. **Update.** After a new build, `weaver update --restart`; children stay up
    (old binary, S6: restart them yourself with
    `weaver kernel restart --project <id>`).
11. **Revoke drill (throwaway project only).** `project.revoke` from the user
    daemon (`weaver` admin path). The child is gone,
    `~/.weftos/run/<id>/revoked` exists, `weft` in the project reports the
    project is revoked (the remedy text is wrong until M2), `kernel status`
    shows `failed`. Re-enrol only by deleting the marker by hand.
12. **Rollback drill.** `weaver kernel stop --project <id>`;
    `weaver project migrate-kernel <id> --revert`; start the old daemon from
    the project directory with `--legacy-project-daemon`; `weft` reaches it;
    `<root>/.weftos/runtime/` is as it was.
13. **Phase 3 together.** With `kernel.mesh.service = "required"` on the user
    daemon, steps 4-8 behave the same; the child's handshake says
    `mesh.mode = off`; `weaver mesh status` is unchanged by children.
14. **Never** run an older `weaver` against this project without moving
    `<root>/.weftos/chain/` aside first (M2 downgrade line).

## Gate result (recorded by the lead)

This review ran nothing; the lead's runs are the test evidence.

- `wt/p2-g-integration` @ e98e455cd: `scripts/build.sh gate` 20/20 (workspace
  tests via nextest + doctests, release build, WASI and browser WASM, UI build,
  cargo and npm audit, mesh no-owned-state among them).
- `wt/p2-g-integration` @ 08f7efd1b (after the fix round): `scripts/build.sh gate`
  20/20. `~/.weftos/run/cluster_peers.json` was unchanged across the run
  (mtime still 10:50:54, from the leak fixed in 303b667d5).

## Re-check: fix round 08f7efd1b (diff e98e455cd..08f7efd1b)

Read-only pass on 2026-10-02 against sections 2, 3 and 8. Nothing was run;
the lead's gate on that commit is the test evidence. Line numbers are from
08f7efd1b (the commit was amended from 20839ddba; the amendment adds only the
two test run-root pins noted under "Section 8" below, `git diff 20839ddba
08f7efd1b` touches nothing else).

- **Section 8 latent paths closed.** `project_supervisor_support/rpc_test.rs:156-159`
  now enters the user profile with `enter_at(&fx.run_root)` instead of
  `set_user_profile(true)`, so the fixture no longer depends on the installed
  supervisor to keep `user_run_root` off the real home;
  `tests/project_cert_rpc.rs:26-38` pins one `init_run_root` tempdir for the
  whole binary before any daemon exists, so the two early tests are no longer
  order-dependent.

- **M1 fixed.** `merge` re-tags every parent `Blocking | Critical` rule that
  is not an overlay approval with `OVERLAY_DENY_TAG` when the overlay raises
  `human_approval_required` over a parent that does not set it
  (`crates/clawft-kernel/src/governance_overlay.rs:449-463`); a parent that
  set the flag itself keeps its semantics (test
  `a_parent_that_asks_for_human_approval_keeps_its_own_semantics`). The
  engine now sets `hard_deny` from the tag on the browser-policy and
  binding-thread paths too (`governance.rs:1285,1308`), and the general
  path already did (`:1330`). I looked for any other way from a parent deny
  to a prompt: `EscalateToHuman` has exactly one producer (`governance.rs:1358`,
  gated by `!hard_deny`); `GateDecision::Defer` is produced only from it
  (`gate.rs:527`); `agent.chat.defer_decide` resolves an existing `Defer`
  and creates none; the agent-side `other => Defer` in
  `clawft-service-agent/src/kernel_gate.rs:96-98` is a `#[non_exhaustive]`
  fallback unreachable from a `Deny`; `to_rvf_mode` (`governance.rs:1667-1675`)
  is witness metadata, not a decision. Side effect of the re-tag: it
  overwrites a parent rule's `sop_category` in the child, so an SOP label
  such as `browser_policy` disappears from the child's rule listing and the
  genesis payload (`boot.rs:1839`) and the effective hash changes
  accordingly; behaviour is unaffected because rule dispatch is by
  `rule_type` (`governance.rs:1281,1299`). Cosmetic; a dedicated flag on the
  rule would be cleaner later. Tests: merge
  (`governance_overlay_tests.rs:482-509`, which also shows the untagged rules
  would have escalated), property (`governance_overlay_prop_tests.rs:126-141`),
  booted child's tool gate returns `Deny` not `Defer`
  (`overlay_boot_tests.rs:207-222`).
- **M3 fixed.** `chain::KERNEL_SOURCES = [governance, project,
  project.supervisor]` and `is_caller_reserved_source` (`chain.rs:217-222`)
  are refused by the `chain.append` RPC (`daemon.rs:6502`) and the tracing
  bridge (`chain_bridge.rs:225`); the floor counts
  `governance.overlay.applied` only under source `governance`
  (`overlay_trust.rs:43-56`); test
  `a_forged_applied_event_cannot_raise_the_rollback_floor`. On the lead's
  question, replication: `append_signed` refuses only `RESERVED_SOURCES`
  (`chain.rs:1580`) and its single production caller is the mesh runtime
  (`mesh_runtime.rs:916`). The floor is evaluated only under the project
  profile (`boot.rs:333-344`, `overlay_runtime.rs:261,293,518`), and a project
  kernel has its mesh forced off and skips `mesh_boot::prepare`
  (`project_profile.rs:171-180`, `daemon.rs:1119-1127`), so no peer event
  can reach a chain whose floor is read. On a user daemon or any other
  mesh-enabled kernel a peer-injected `governance.overlay.applied` lands as
  inert replicated history (every chain already carries peers' governance
  events). Safe as built; the residual is the in-tree chain file plus
  in-tree key (S11, Phase 4), unchanged. Keep the rule "a kernel that
  evaluates the floor never replicates" in mind if a future phase gives
  children a mesh.
- **S1 fixed.** `RevocationView::was_revoked` counts only keys revoked by
  `project.revoke` (not rekey-retired, `project_identity_view.rs:221-227`),
  and `plan_registration` refuses any key for such a project with no
  certificate in force as `ProjectRevoked` -> `project_revoked`
  (`:299-305`, `project_cert_rpc.rs:146`), which is in the child's
  `FATAL_KINDS` (`project_boot_run.rs:64`). A late cert-file failure is
  `IssueError::Incomplete` -> `identity_change_incomplete`
  (`project_cert_rpc.rs:125-130,149`); `handle` runs `on_identity_change`
  for success and for `Incomplete` (`:673-699`); `rekey` drops the session
  before the store write (`:454-461`). Tests cover the marker written and
  the child stopped despite the store failure, `unknown_session` on the old
  heartbeat, `project_revoked` on re-register including after a view
  restart (`tests/mesh_local.rs`, `project_cert_rpc_tests.rs`,
  `project_identity_tests.rs`). Note, not a blocker: with the marker
  missing, `ensure_running` still spawns the revoked project; the child dies
  on `project_revoked`, the restart budget is spent, then `failed`. Loud and
  terminal, but five spawns; a view check in `prepare` would save them.
- **M2 done.** `kernel.md` gains "Project kernels (Phase 2)" with the
  owner procedure in order, the overlay format with the M1 sentence,
  revoke/rekey, and "Never downgrade a child-kernel project";
  `configuration.md` documents `kernel.profile`, `kernel.shared_services`
  and the `[serve]` table including `kernel_version`/`kernel_sha`
  (`:1200-1237`); `weft project init` ignores the four paths
  (`clawft-cli/src/commands/project_cmd.rs:208-213`, `cli.md:1079-1081`);
  `SupError::Revoked` names the real remedy (`types.rs:188-193`); `weft`
  keeps the user daemon's `error_kind` (`connect.rs:103,177-180,329-333`);
  `cli.md` says `status --profile user`; the spawn ledger learns the pid
  (`mesh_local_registry.rs:403-405,460-466`, test `:659-676`) and A7 now
  states the one race it cannot check; D8 says "until the kernel restarts";
  A7 has a "Phase 2 review, fixed before merge" bullet matching the code;
  CHANGELOG has the four entries and the downgrade warning. Doctor finding
  ids are still not in `cli.md` (M2, last bullet); text only.

**Final verdict: SHIP to `0.8-metaharness`.** No remaining must-fix. Carry
over as cards: the doctor finding ids in `cli.md`, the five-spawn note
above, the `sop_category` overwrite note, and S2-S11 from section 3
(section 9 lists the card consolidation).
