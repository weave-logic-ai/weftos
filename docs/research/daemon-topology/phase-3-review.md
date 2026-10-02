# ADR-103 Phase 3 review: `integrate/p3` @ 86ff8c936

This is an independent, cross-package review of the integrated Phase 3 work:
the machine mesh service. Phase 3 covers the kernel mesh seam (K0), the
`mesh-local/1` protocol crate (L), the machine journal and bindings (J),
listener admission (K1), the service itself (S), the user daemon as its client
plus the `user.key` migration (U), system service units with the printed
installer (H), and the integration commit (X). It is measured against:

- [ADR-103](../../adr/adr-103-weave-topology-roles-and-instances.md) D1-D14,
  A1-A9 and the "Known limits" paragraphs added by Phase 3
- the [P3 plan](../../plans/weave-topology-p3-plan.md), in particular 1
  (contracts), 5 (migration), 7 (safety), 8 (risks) and 10 (exit criteria)
- the [Phase 1 review](phase-1-review.md)

The range is `a11cb7a70..integrate/p3` (140 files, +23k/-0.8k). Fable reviewed
it on 2026-10-02 by reading the diff and the merged sources, with three
delegated read-only passes (installer and units; K0/K1 admission; the test
suite against plan 10). Nothing under `~/.weftos`, `~/.clawft`, `/var` or
`/etc` was touched and no service was started. Line numbers are from the
`integrate/p3` tree. The lead's gate result is recorded at the end.

## 1. Verdict

**SHIP-WITH-FIXES.** The trust chain composes end to end and every seam fails
closed:

- The service refuses root, verifies its own directories, journals before it
  binds, and writes the record clients pin before it opens the socket.
- A client trusts a service only after four independent checks: socket owner,
  `service.json` owner and `service_uid`, a machine-key proof over a fresh
  nonce, and the uid the service read from the peer credential equalling its
  own. The pin is checked after all of that.
- Registration binds the challenge, the uid and the node id into the user
  key's signature; the challenge is single-use per connection.
- Bindings are a pure fold of a hash-chained, box-key-signed journal with
  one-key-per-principal, one-principal-per-key and never-rebind-a-revoked-key
  enforced in one place. Revocation is allowed while the journal is read-only
  and even when it cannot be written.
- `src_scope` is stamped by the service from the registration and trusted on
  the receiving daemon only when the sender's certificate verifies against the
  pinned machine key and names the delivering node.
- In service mode the daemon holds no node key, generates none, and every
  former node-key signer is turned off with a warning rather than falling back.
- The suite runs everything as the current user on tempdirs; the shipped
  installer prints and runs nothing.

What does not yet hold is at the edges rather than the core:

1. **Identity fork by omission.** `weaver mesh install-service` without
   `--adopt-node-key` prints a script that silently gives the machine a fresh
   box key when a collapsed `~/.weftos/run/node.key` exists. The SOP's first
   example command is exactly that form. The Pi's pin breaks with no warning
   (section 5, M1).
2. **The service defaults to loopback, the collapsed daemon did not.** After
   the documented migration the Pi can no longer reach the Mac unless the
   owner also sets `listen = "0.0.0.0:9489"`. The SOP says LAN exposure is an
   explicit choice but the owner steps never say the Pi needs it (M2).
3. **`weaver mesh peer revoke` does not disconnect.** The route is removed, the
   socket stays open and the next frame re-registers it; under `observe` (the
   default) revocation has no effect on traffic at all. `docs/reference/cli.md`
   says "refuses and disconnects" (S1, M2).
4. **Doc drift** in `configuration.md` (none of the five new `kernel.mesh`
   keys), pin wording that contradicts itself between the CHANGELOG and the
   SOP, an overstated "never calls `sudo`", and two stale error strings (M2).

None of these needs a redesign; all are under an hour each. The must-fix list
is in section 2, should-fixes in section 3.

## 2. Must-fix before merge

**M1. Warn when the installer would mint a new box key on a machine that has
one.** `Plan::from_args` and `run_install`
(`crates/clawft-weave/src/commands/mesh_install.rs:109-125,296-303`) treat
`adopt_node_key: None` as nothing to say. The service then generates a key and
journals `key_origin: "generated"` (`clawft-mesh-service/src/main_loop.rs:191-198,241-250`).

- **Failure:** the owner runs the SOP's first example,
  `weaver mesh install-service > install.sh`
  (`docs/guides/weftos-deployment-sops.md`, "Installing and removing"), installs,
  starts, and the node id the Pi pinned is gone. Nothing in the printed script,
  on stderr, or in `weaver mesh status` says the key is new.
- **Fix:** when `--adopt-node-key` is absent and `$HOME/.weftos/run/node.key`
  exists, print a `# WARNING:` header line and a stderr note naming the flag.
  Also give the "exists and differs; not overwriting" branch
  (`mesh_install.rs:255-260`) a remedy (stop the service, move the generated
  key aside, re-run).

**M2. Owner-facing documentation.** Each is a few lines; together they decide
whether the real install goes right.

- `docs/guides/weftos-deployment-sops.md`, owner migration step 1: say that the
  Pi (and any LAN peer) needs `--listen 0.0.0.0:9489` because the service
  defaults to loopback (`clawft-mesh-service/src/config.rs:125`) while the
  collapsed daemon's documented default was `0.0.0.0:9489`
  (`docs/guides/configuration.md:1141`); and that the first `weaver mesh`
  admin verb needs either `--admin-uid "$(id -u)"` at install or `sudo`
  (`config.rs:255-257`; the script writes `admin_uids` only when given,
  `mesh_install.rs:177-179`).
- Same guide, step 4: state that `service = "required"` is load-bearing after
  step 5. Under `auto`, a stopped service plus a removed `node.key` makes the
  next daemon boot generate a fresh node key (`mesh_local_glue.rs:159-171`,
  `node_identity.rs` `load_or_generate`), which is the identity fork the plan
  forbids (S2 proposes a guard).
- Same guide, rollback after step 5: the copied-back key must be `chown`ed to
  the user and `chmod 600`, or `load_or_generate_node_key` refuses it.
- Pin wording: CHANGELOG says "pinned on first contact"; the SOP says "there is
  no pin by default". Both are true of different clients: the daemon pins on
  first contact (`mesh_local_glue/endpoint.rs:76`, `pin.rs:36-42`); the
  `weaver mesh` verbs compare a pin only when one exists and write one only via
  `trust` (`admin_client.rs:107-121,194-215`). Say so in one sentence in both.
- `docs/guides/configuration.md` "Mesh networking": add `service`,
  `service_socket`, `admission`, `genesis_hash`, `admission_open_membership`
  (`clawft-types/src/config/kernel.rs:880-953`).
- `docs/reference/cli.md` `peer revoke`: "refuses and disconnects" is not what
  happens (S1). Until S1 lands, say "refuses at the next admission; under
  `observe` the revocation is recorded only".
- "`weaver update` never calls `sudo`" (CHANGELOG, SOP, `install_tiers.rs:9`):
  true for the service tier; `update_cmd.rs:266-275` still has the Phase 0
  `sudo cp` fallback for the user binary. Say "never for the service" or
  remove the fallback.
- Stale strings: `clawft-mesh-local/src/client.rs:44-49` ("a `weaver mesh
  trust` command arrives with Phase 3 package S") and
  `clawft-weave/src/mesh_local_glue.rs:374` ("from the mesh service package").
  Both verbs shipped in this range.
- `docs/guides/kernel.md` "Owner migration" has no pointer to the Phase 3
  procedure; add one line.

**M3. ADR-103 record.** Add to A9 or the known limits: the kernel gate and the
service broker each cache a permit for up to 300 s, so a rule change can take
up to ~600 s to reach an already-permitted peer
(`clawft-kernel/src/mesh_admit_gate.rs:268-293`,
`clawft-mesh-service/src/verdicts.rs:291-307`); `observe` never marks a peer
verified, so route takeover by `source_node` claim remains possible under the
default (`mesh_admit_gate.rs:344`, `mesh_runtime.rs:315-318`); and in service
mode the user chain gains `mesh.service.bound` and `mesh.journal.anchor`
events (`clawft-weave/src/mesh_local_chain.rs:16-18,105-111`), so plan 5's
"never touched: `~/.weftos/chain/*`" is true of the migration, not of
steady-state operation.

## 3. Should-fix (before the 0.8.2 cut, not merge blockers)

**S1. `peer revoke` must close the connection and revocation must be
re-checked after admission.** `MeshRuntime::disconnect_peer` only removes the
map entry (`clawft-kernel/src/mesh_runtime.rs:658-668`); `serve_connection`
holds its own `out_tx` for the life of the loop
(`clawft-kernel/src/mesh_serve.rs:317,377,396`), so `out_rx.recv()` never
yields `None`. The next envelope passes `screen_frame` (bound id still matches)
and `register_peer` re-inserts the route (`mesh_runtime.rs:322-329`).
`RevocationList` is consulted only at admission (`mesh_admit_gate.rs:318`).

- **Failure:** an admin runs `weaver mesh peer revoke <id>`
  (`clawft-mesh-service/src/admin.rs:197-212`); the peer keeps publishing and
  receiving until it reconnects on its own.
- **Fix:** a per-connection cancel (store an `AbortHandle` or
  `CancellationToken` in `PeerConnection`, or have `serve_connection` watch the
  map), and re-check `is_revoked(bound)` in `screen_frame`. Print a note from
  the CLI when the service's admission mode is not `enforce`.

**S2. Guard `auto` against a fresh node key on a machine that has used the
service.** In `mesh_boot::prepare` (`crates/clawft-weave/src/mesh_boot.rs:66-76`),
when the probe is `Absent` under `auto`, the pin file
`~/.weftos/mesh/machine.pub` exists and `<runtime>/node.key` does not, refuse
to boot with "this machine has used the mesh service; set
`kernel.mesh.service` to `required` or `off`". That closes the plan 8 U risk
("silently falling back to a self-generated node key") that M2's doc line only
warns about.

**S3. Per-IP cap and first-frame timeout apply only under `enforce`;
the 1024-connection cap applies always.** `strict()` is false for `AllowAll`
and `observe` (`mesh_admit.rs:403`, `mesh_admit_gate.rs:301-303`), so `IpSlot`
and `limits.first_frame` are skipped (`mesh_serve.rs:119-128,342-343`) while
`MAX_CONNECTIONS` is enforced in every mode (`mesh_serve.rs:107,114`).

- **Failure:** with an explicit `listen = 0.0.0.0:9489`, one LAN host opens
  1024 idle TCP connections and every new peer is dropped at `:115`.
- **Fix:** apply the per-IP cap regardless of strictness; the lenient idle
  timeout can stay for leaves. Loopback default makes this low today.

**S4. Collapsed-mode `enforce` without `noise = true` is accepted and refuses
everyone.** The service validates it (`config.rs:233-238`, `gate.rs:116-118`);
the kernel boot path does not, so every peer fails `NoHandshakeBinding`
(`mesh_admit.rs:251`). Fail-closed, but silent. Mirror the service's check.

**S5. `build_endpoint` runs before `wants_probe` is consulted.**
`mesh_boot.rs:55-56` builds the endpoint for every user daemon;
`build_endpoint_in` resolves the user key with `create = true`
(`mesh_local_glue/endpoint.rs:73-74`). With `service = "off"` and a socket
present, a daemon that has neither `user.key` nor `chain.key` and no legacy
chain gets a `user.key` generated as a side effect of a probe it asked not to
make. Gate the call on `mesh_mode::wants_probe`.

**S6. A torn journal tail drops every user link within 12 h.** Any bad tail,
including the crash signature (torn last line, `chain.rs:65`), is quarantined
and the journal goes read-only (`journal.rs:227-232,361-363`). Read-only
refuses `issue_cert` (`bindings.rs:274-291,315-319`), so renewals at half life
fail (`session.rs:107-113`), the session ends, re-registration cannot reuse a
cert past half life (`register.rs:215-221`) and the daemon retries slowly
forever (`mesh_local_glue.rs:378-389`). This is the plan's design (1.3), the
doctor shows a FAIL (`mesh_doctor.rs:138-139`) and `weaver mesh status` prints
READ-ONLY (`mesh_cmd.rs:370`), but on a single-owner Mac a power loss during
an append becomes a 12 h deadline to run `--accept-truncate`. Consider: allow
renewal of an *existing* binding while read-only (no trust increase), or
auto-accept a torn final line when `lost_count == 1` and the harvested facts
are empty, journalling that it did so.

**S7. The lane never runs the shipped binary.** `scripts/dev/mesh-p3-e2e.sh`
starts `weaver mesh serve` only when `WEAVER_BIN` is set (`:48-63`), and
`scripts/build.sh test-mesh-service` (`:915-924`) never sets it. Argument
parsing, the default paths, `--health-listen off` and the root refusal in the
real binary are unexecuted. Set `WEAVER_BIN` from `target/debug/weaver` in the
lane. Related gaps: no test places a `node.key` before start and asserts the
service's `node_id` and `key_origin: "adopted"` (the golden journal has only
`"generated"`, `tests/golden/journal.jsonl`); no test runs
`weaver mesh journal verify --accept-truncate` against a real quarantine
(`mesh_cmd.rs:68-72` covers only the refusal); cert expiry while the service
is down (plan 8 U) is unasserted.

**S8. `mesh.exe` checks the unit file, not the process, with a prefix
heuristic.** `mesh_doctor.rs:52-54,186-194,228-234`: `user_writable` matches
`$HOME`, `/tmp`, `/private/tmp`. A unit pointing at `/opt/homebrew/bin/weaver`
(user-owned on Apple-silicon Homebrew) passes. Stat the parent chain (owner
root, no group/other write).

**S9. The daemon registers the machine public key as its own in three
registries.** `daemon.rs:1163-1171` (node registry, labelled
"service-attested"), `:1962-1966` (concierge agent) and `:2330-2332`
(`caller_pubkey`). Registries are per process, so two user daemons on one box
do not collide, but anything that later publishes or verifies against that key
will attribute it to the machine. The code comment says a follow-up card
exists; it should land before a second human account uses service mode.

**S10. Smaller items.**

- systemd `StartLimitBurst=5` in 60 s with `RestartSec=3`
  (`service_units_system.rs:150-157`): a leftover collapsed daemon on 9489
  leaves the service `failed` after ~15 s until `systemctl reset-failed`;
  launchd loops. The SOP says "stop the daemon first" but not the systemd
  dead-end.
- `weaver update` reads `/var/run/weftos/service.json` directly
  (`update_cmd.rs:317`) and ignores `$WEFTOS_MESH_SOCKET`; the doctor goes
  through `socket_of` (`doctor_cmd.rs:119-125`).
- A cleanly stopped installed service removes `service.json`
  (`main_loop.rs:117-129`), so the doctor reports "no machine mesh service
  (collapsed mode)" as OK while a `service`-mode daemon is reconnecting; only
  `mesh.mode` WARNs (`mesh_doctor.rs:206-220`). After a SIGKILL the record
  stays and it is a FAIL. One sentence in the SOP.
- The daemon registers with the service before the kernel boots
  (`daemon.rs:1085-1091` then `:1109-1114`). During the boot seconds, inbound
  `deliver` frames queue in the client's 256-slot event queue and
  `verdict.request`s wait until `session` starts (`client.rs:476-493`); a
  cluster owner that is booting answers verdicts late and the broker falls to
  stale-grace. Harmless today; worth a comment or a deferred `register`.
- `read_record` in the daemon anchors trust on the socket owner
  (`endpoint.rs:118-130`); the `weaver mesh` verbs load `service.json` with no
  owner check (`mesh_cmd.rs:251-256`) and rely on `AdminClient::connect`'s
  server-uid check instead. Same outcome because the directory is
  root/service-owned, but the two readers should share one function.
- `K0` is not strictly behaviour-preserving: new in every mode are the 1024
  cap, the 10 s Noise handshake timeout (`mesh_serve.rs:292-297`) and
  `disconnect_channel` emitting `Left` (`mesh_runtime.rs:350-371`). All benign;
  the module header overstates.
- Seed dials still have no inbound reader (`mesh_serve.rs:453-465`), unchanged
  from Phase 1.
- Timing-sensitive assertions: `mesh_p3_e2e.rs:179,185`,
  `service_register.rs:111`.

### Verified with no finding

Service start and anti-squat:

- Refuses `euid == 0` with no override (`main_loop.rs:133-138,231`); the e2e
  script refuses root too (`scripts/dev/mesh-p3-e2e.sh:22-25`).
- Socket directory must be a real directory owned by the service uid or root
  with no group/other write (`main_loop.rs:140-159`); a 0750 group-owned
  `/var/run/weftos` passes, which is what the installer creates
  (`tests/golden/mesh-install-launchd.sh:46`).
- A foreign socket is never removed, a live one is never replaced
  (`main_loop.rs:163-180`); the record is written before the socket binds
  (`:317-323`); the socket is 0666 by D-7 (`:187`), the directory is the gate.
- State dir 0700, `O_NOFOLLOW` opens, `flock` with holder pid
  (`fsutil.rs:26-105`, `journal.rs:199-202`).
- Health endpoint must be loopback (`config.rs:240-246`); listener default
  `127.0.0.1:9489` (`:125`); a non-loopback `listen` logs a warning
  (`main_loop.rs:279-285`); `mesh.toml` rejects unknown keys (`config.rs:59`).
- `enforce` needs `genesis_hash`, an explicit `cluster_owner_uid` and Noise
  (`config.rs:223-239`); the owner is never inferred from a TOFU bind
  (`state.rs:43-48,163-167`).

Client verification of the server:

- Socket owner must be root or `service.json`'s `service_uid`
  (`client.rs:186-192`); the record itself is trusted only when owned by root
  or the socket's owner and naming the socket's owner (`endpoint.rs:118-130`),
  read from the opened file (no stat-then-read race).
- `hello_ack`: `node_id == hash(machine_pubkey)`, pubkey equals the record,
  Ed25519 proof over the client's fresh nonce, challenge, pubkey and uid,
  then `ack.uid == own euid` (`client.rs:299-317`,
  `local_server.rs:270-288`, `proto.rs:402-416`). Pin compared last
  (`client.rs:318-320`).
- Pin: written atomically on first contact, case-normalised, corrupt or
  different is a hard error, concurrent first contacts cannot clobber each
  other (`pin.rs:16-93`). `MachineKeyChanged`, `ServerUid`, `BadServerProof`,
  `UidMismatch` and `PinCorrupt` are fatal to the boot under every policy
  (`mesh_mode.rs:100-103`, `mesh_local_glue.rs:297-306`).
- Permission denied on the socket is a refusal, not a fallback
  (`endpoint.rs:56-69`).

Registration, binding, certificates:

- `register` signature covers the per-connection challenge, the uid the
  service read, and the node id, with tagged length-prefixed principal bytes
  (`proto.rs:444-456`, `peer.rs:16-30`); the challenge is spent by the first
  attempt (`register.rs:107-111`); `addresses.user_id` must equal
  `hash(user_pubkey)` (`:116-120`).
- Peer credential fails closed on both sides (`local_server.rs:224-236`,
  `client.rs:188`); the injectable identity exists only behind the `testing`
  feature (`peer.rs`, `local_server.rs:46-52`, `main_loop.rs:216-224`;
  `docs/guides/build.md` keeps `--all-features` off release gates).
- Bindings invariants are enforced in one validator (`bindings.rs:329-405`):
  one key per principal, one principal per key, revoked keys never rebind,
  a revoked principal needs an approver, rebind revokes every old serial
  (`:446-455`). Pending binds under `approve`; `approve --user-id` refuses
  when the pending key changed (`admin.rs:120-137`).
- Certificate: fixed binary layout, `verify_strict`, node and user ids checked
  against keys, 5 min leeway (`cert.rs:76-153`); the uid is not in it.
  Renewal at half life; a connection whose cert lapses unrenewed is closed
  (`local_server.rs:345-383`). The client verifies the renewed cert's issuer
  and key (`session.rs:154-165`).
- Revocation is allowed while read-only (`bindings.rs:295-313`) and enforced
  in memory plus `force-revoked.json` when the journal cannot record it
  (`admin.rs:139-171`, `force_revoked.rs`); revoked serials ride the signed
  facts (`bindings.rs:190-201`).
- `status` shows registrations, force-revocations and the peer table to
  admins (and peers to registered daemons) only (`state.rs:240-299`); admin
  verbs need root or `admin_uids`, checked once at connect
  (`local_server.rs:260-268`, `:407-414`).
- Rate limits: 64 connections per principal, 512 total, 5 registers per
  minute per principal (`limits.rs:12-18`); a refused daemon retries at the
  30 s ceiling instead of spending that budget (`mesh_local_glue.rs:347,378-389`).

Journal:

- Signature over the raw line bytes with the `sig` tail cut, domain-separated;
  `prev`, `seq` and version checked per line (`chain.rs:17-46`); appends
  fsync and roll back a short write by `set_len`, poisoning on a failed
  rollback (`journal.rs:464-480`).
- `user.*`, `journal.quarantine` and `journal.accept_truncate` kinds cannot be
  appended through the generic door (`journal.rs:168-171,437-439`).
- Quarantine order is crash-safe (marker, then bytes, then truncate, then the
  signed record), a bad first record is a hard error, and an acceptance must
  name the newest pending quarantine (`journal.rs:239-337`,
  `bindings.rs:332-343`); the harvested serial high-water is clamped so a
  forged tail cannot exhaust the serial space (`journal.rs:327-336`).
- Admin verbs journal first and apply after, except tightening
  (`admin.rs:1-12,225-295`); `AdminAck` is crate-private (`journal.rs:75-92`).
- The service owns no chain, token, secret or governance type
  (`main_loop.rs:15-16`); the cargo-tree gate is check 20
  (`scripts/build.sh:1964`).

Routing and scope:

- `dest_scope` from an unadmitted peer can reach only the default tenant
  (`router.rs:156-182`); admitted peers get scope, then longest prefix, then
  sole user, else `scope_required` (`:126-154`).
- Outbound `src_scope` is stamped from the registration, never from the
  message (`router.rs:236-238`); local cross-tenant delivery needs
  `accept_from` (`:265-271`), which the daemon leaves empty
  (`endpoint.rs:87`) and the service sanitises (`register.rs:236-257`).
- Prefix claims cannot overlap another user's (`registry.rs:242-253`); a
  second daemon of the same user gets `AddressInUse` with the holder pid
  (`register.rs:122-124,259-267`).
- The receiving daemon refuses deliveries addressed to another user and
  exposes `src_scope` only from a certificate signed by its pinned machine key
  for the delivering node (`mesh_local_sink.rs:50-76`, test `:183-203`).
- The kernel's listener screens `source_node` against the bound id on every
  frame and strips `src_scope` unless admitted (`mesh_serve.rs:228-271`);
  envelopes before `AdmitHello` are refused under `enforce`
  (`mesh_serve.rs:319-321,357-371`); the hello binds this session's Noise
  handshake hash (`mesh_admit.rs:234-283`); the wire format without scopes is
  byte-identical (`mesh_ipc.rs` frozen test).
- Verdicts are answered only on the daemon's own registered connection and
  denied with `ttl 0` when there is no gate (`session.rs:75-89`,
  `mesh_local_verdict.rs:49-59`, `client.rs:476-493`); the broker accepts a
  reply only from the connection it asked (`verdicts.rs:361-370`).

Service mode in the daemon and kernel:

- Only the user daemon can enter service mode; project and legacy daemons
  refuse `required` (`mesh_boot.rs:42-53`, `boot.rs:270-283`).
- In service mode the identity is the service's public key; `signing_key()`
  is an error, never a generated key (`node_identity.rs:60-91`); the kernel
  takes the id as given and skips the ephemeral branch
  (`boot.rs:418-423`), binds no listener (`:559-562,805-813`); placement and
  node facts are disabled with a warning (`daemon.rs:1132-1149`).
- Roles become `user` only; the handshake carries `mesh.mode`, state, cert
  serial and queue counters for every daemon (`user_daemon.rs:74-85`,
  `handshake_rpc.rs:155-163`, `handshake.rs` `MeshHandshake`).
- A service appearing after a collapsed boot is logged once and never
  switched to live (`mesh_local_glue.rs:189-207`); a service whose node id
  changed on reconnect is not adopted (`:359-366`).
- Remote-node A2A messages go through the forwarder when present, which
  refuses fast while the link is down (`a2a.rs:431-436`,
  `mesh_local_sink.rs:105-144`).
- Chain events are queued, bounded, flushed in order, and surfaced as
  `events_pending`/`events_dropped` in the handshake with a WARNING line in
  `weaver kernel status` (`mesh_local_chain.rs`, `kernel_cmd.rs:596-605`).

User key:

- `user.key` wins for the user chain only; an unrelated chain beside a
  `user.key` keeps its own key (`chain_paths.rs` tests); a symlinked or loose
  `user.key` is refused by both the chain loader and the mesh client
  (`boot.rs:971-985`, `user_key.rs:107-125`).
- Migration is idempotent, verifies equal public keys after the copy, refuses
  a diverged `user.key`, and never writes `chain.key` (`user_key.rs:251-294`).
  The migrated `chain.key` is 0600 (`chain_migrate.rs:529-535`) and the chain
  loader repairs a loose legacy key to 0600 (`chain.rs:1196-1203`), so the
  `LooseMode` refusal is not a likely trap.
- A fresh key is generated only when no `user.key`, no `chain.key` and no
  unmigrated legacy chain exist (`user_key.rs:187-207`); the doctor WARNs on
  a split (`:302-348`).

Installer and units:

- Script hygiene: `sh_quote` on paths, `u32` admin uids, `SocketAddr`-parsed
  listen, control characters refused, quoted heredocs, `sh -n` in tests
  (`mesh_install.rs:94-99,393-407`); `--apply` refused (`:111-113`);
  `--purge-key` only on uninstall (`:84-86`); account removal commented out
  with the uid-reuse warning (`:266-278`); re-runnable (`mesh.toml` kept,
  `dseditgroup` idempotent, adopt is `cmp -s` then `install -m 0600`
  (`:251-263`)); refuses to copy the binary onto itself (`:124`).
- `SUDO_USER` is required and must not be root (`golden/mesh-install-launchd.sh:16-17`).
- plist runs `_weftos:_weftos`, `KeepAlive`, `ThrottleInterval 10`
  (`golden/weftos-mesh.plist:7-10,23-26`); systemd unit has
  `NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome=yes`, `PrivateTmp`,
  empty `CapabilityBoundingSet`, `RuntimeDirectoryMode=0750`
  (`service_units_system.rs:165-176`); no environment overrides in either.
- `weaver update` prints the service lines and runs nothing
  (`install_tiers.rs:138-156`, `update_cmd.rs:309-330`); the restart guard
  refuses `SERVICE_EXE` with a test (`daemon_restart.rs:160-167`,
  `daemon_restart_tests.rs:300-315`).
- Doctor: `mesh.service|proto|pin|journal|box_key|node_key_dup|exe|listeners|force_revoked|mode`
  and `user_key_split` exist with the severities the CHANGELOG claims
  (`mesh_doctor.rs:63-246`, `user_key.rs:302-348`).

## 4. Exit criteria (plan 10)

What the suite proves as the current user on tempdirs, and what only the
owner's real install can.

| Criterion | In the suite | Only the real install |
|---|---|---|
| Service runs unprivileged on 9489 | Starts as the current user on `127.0.0.1:0` (`tests/mesh_e2e/mod.rs:79`); root refusal is a code path, untestable non-root | `_weftos` on 9489 |
| `weaver mesh status` shows the adopted `node_id` unchanged | Status prints the live id (`tests/mesh_cmd.rs:35-36`); adopt refuses a differing key in script text (`mesh_install.rs:485`) | Adopted key, same id as before migration (no test places a key before start, S7) |
| Journal verifies | Admin RPC and CLI (`mesh_p3_e2e.rs:251-252`, `mesh_cmd.rs:66-67`); every byte position tampered is detected (`tests/journal.rs:59`) | - |
| Daemon registers over the socket, bound to uid, `how` recorded | `user.bind` with the real euid and `how: "tofu"` through `mesh_boot::prepare` (`mesh_p3_e2e.rs:74-79`, `mesh_boot_user.rs:49-62`) | - |
| `kernel.status` shows `mesh.mode = service` with the same `node_id` | Asserted on the state cell and the handshake profile (`mesh_p3_e2e.rs:66-69`, `mesh_boot_user.rs:51-78`), not on the RPC output | The `Node:` line equals the pre-migration id |
| Scoped `weft://` send delivered and stamped | `src_scope` from A's cert, forged `from` ignored, cross-tenant refused without `accept_from`, unadmitted scope claim dropped (`mesh_p3_e2e.rs:89-121`, `tests/service_routing.rs:43-121`); delivery lands in a stub inbox, not the kernel router | - |
| Second uid cannot take the first uid's address | `AddressInUse` then `BindConflict`, prefix refused, A intact (`mesh_p3_e2e.rs:123-159`) with an injected second uid | `scripts/dev/mesh-two-uid.sh` (sudo, outside the gate) |
| `bind.rebind` revokes the old key | Old link dropped, old key `BindConflict`, serials in signed facts, `how: "rebind"` (`mesh_p3_e2e.rs:161-201`, `tests/service_register.rs:155`) | - |
| `observe` journals a bad peer | Plaintext signed hello -> `peer.refuse mode: "observe"`, still delivered (`mesh_p3_e2e.rs:203-225`); replay/skew/revoked in `mesh_admit_tests` | - |
| Kill and restart leaves chains and bindings intact, daemons reconnect | Graceful `shutdown()`, node id persists, A reconnects, a second `KIND_BOUND` and a later anchor, bind count unchanged (`mesh_p3_e2e.rs:227-256`, `tests/service_ops.rs:29-46`); torn append covered at the journal level only | `kickstart -k` (SIGKILL path, `service.json` left behind) |
| `service = off` returns to Phase 1 with no data change | No connection, no bind, no pin; non-user daemon keeps `node.key`, mode `collapsed` (`mesh_p3_e2e.rs:263-270`, `mesh_boot_plain.rs`); not asserted that `node.key`/chain bytes are untouched across a round trip | Round trip on the owner's home |
| `weaver update` prints, never runs, the restart | Pure function (`install_tiers.rs:217`); the runtime read of `service.json` is untested | Against the real record |
| No-owned-state gate | `scripts/build.sh` check 20 plus a source-level test (`tests/service_ops.rs:277,305`) | - |
| Nothing touches `/var`, `/etc`, launchd, systemd, `~/.weftos`, `~/.clawft` | No `home_dir`/`launchctl`/`systemctl` in tests; `HOME` redirected in the e2e (`mesh-p3-e2e.sh:36-41`); `cargo test -p clawft-mesh-local -p clawft-mesh-service` runs with the real `HOME` but nothing there reads it | - |
| `scripts/build.sh test-mesh-service` and `gate` green | Reported by the lead (below); the lane never runs the shipped `weaver` binary (S7) | - |

Owner-only in addition: the Pi sees the same node id (needs a LAN `listen`,
M2); the user LaunchAgent picks up the new group membership after re-login;
launchd and the rundir helper converge after a reboot; the macOS firewall
prompt for `/usr/local/libexec/weftos/weaver` when `listen` is not loopback.

## 5. Migration safety (plan 5)

Could any step lose or fork `node_id`, `user_id`, or the Pi's pin?

- **`node_id`.** Adopting copies the seed with `install -m 0600 -o _weftos`
  after `cmp -s` (`mesh_install.rs:251-263`); the service derives the id from
  the same loader (`main_loop.rs:191-198`) and journals `key_origin:
  "adopted"`. In service mode the daemon takes the id from `hello_ack` and
  refuses one that does not match the key (`node_identity.rs:60-66`). Two
  ways to fork remain, both by omission: installing without
  `--adopt-node-key` (M1), and leaving `auto` after removing `node.key` so a
  later boot with the service down generates a new key (M2, S2). Rollback
  before removal is clean: `off` plus a restart uses the untouched
  `~/.weftos/run/node.key` (`mesh_mode.rs:98`).
- **`user_id`.** `user.key` is not required for service mode: the client
  falls back to the migrated `chain.key` (`user_key.rs:176-186`), so skipping
  step 2 changes nothing. `migrate user-key` copies the same seed and verifies
  the public keys are equal (`:281-284`); a different existing `user.key` is
  refused (`:258-265`). A fresh key is generated only when no chain key and no
  unmigrated legacy chain exist (`:187-207`), and a legacy chain that was
  migrated `--allow-unsigned` is the one case where a new identity is correct
  (`:197-204`). The chain loader prefers `user.key` only under
  `~/.weftos/chain` (`chain_paths.rs`). The doctor WARNs on a split. No fork.
- **The Pi's pin.** Unchanged as long as the box key is adopted. But the Pi
  cannot *reach* the service unless `listen` is set to a LAN address (M2); the
  plan's step 5 check cannot pass with the default `mesh.toml` the installer
  writes.
- **Bindings on first contact.** `tofu` binds the first key the owner's uid
  presents (`register.rs:193-202`). If the owner's daemon first registers
  with `chain.key` and later `migrate user-key` copies the same seed, the key
  is identical and the binding is `Existing` (`bindings.rs:139-147`). No
  rebind needed.
- **Group membership.** A user not yet in `_weftos` gets `EACCES` on the
  socket, which is a refusal and a boot failure under `auto` and `required`
  (`endpoint.rs:56-69`, `mesh_mode.rs:100-103`) with the remedy in the
  message. Under launchd the user agent retries every 30 s until the owner
  logs out and in. Loud, correct.
- **`chain.key` mode.** The user-key reader refuses anything looser than
  0600 without repairing it (`user_key.rs:107-125`). The migrated copy is
  0600 (`chain_migrate.rs:529-535`) and the kernel's loader repairs the legacy
  original (`chain.rs:1196-1203`), so this should not trigger; it is on the
  checklist anyway.
- **What changes under `~/.weftos` in steady state.** `~/.weftos/user.key`
  and `user.key.MIGRATED_FROM.json` (step 2), `~/.weftos/mesh/machine.pub`
  (first contact), and the user chain gains `mesh.service.bound` and
  `mesh.journal.anchor` events (`mesh_local_chain.rs`). The last is by design
  and should be stated (M3).
- **Rollback after removing `node.key`.** Needs an admin copy from
  `/var/lib/weftos/mesh/node.key` plus `chown` and `chmod 600`, or the loader
  refuses it (M2).

## 6. Doc and ADR drift

| Text | Code | Evidence | Recommendation |
|---|---|---|---|
| Plan 5 "never touched by any P3 code: `~/.weftos/chain/*`" | The daemon appends `mesh.service.bound` and `mesh.journal.anchor` to the user chain | `mesh_local_chain.rs:16-18,105-111`, `mesh_boot.rs:97-103` | Amend: true of the migration; steady state anchors the journal into the chain (ADR-022) |
| CHANGELOG "machine key is pinned on first contact"; SOP "there is no pin by default" | Daemon pins on first contact; verbs compare only an existing pin and write via `trust` | `endpoint.rs:76`, `pin.rs:36-42`, `admin_client.rs:107-121,194-215` | One sentence in both naming which client does what |
| `cli.md` `peer revoke`: "refuses and disconnects" | Route removed, connection stays, route re-registers; no effect under `observe` | `mesh_runtime.rs:658-668`, `mesh_serve.rs:317-396`, `mesh_admit_gate.rs:318` | Fix S1 or reword |
| ADR known limits (U): "cache a permit for up to 300 s" | Kernel gate 300 s plus broker 300 s compound to ~600 s | `mesh_admit_gate.rs:268-293`, `verdicts.rs:291-307` | State 600 s |
| CHANGELOG/SOP/`install_tiers.rs:9` "`weaver update` never calls `sudo`" | Phase 0 `sudo cp` fallback for the user binary remains | `update_cmd.rs:266-275` | "never for the service", or remove the fallback |
| `configuration.md` `[kernel.mesh]` | Five new keys undocumented | `config/kernel.rs:880-953` | Add them |
| `configuration.md:1141` example `listen_addr = "0.0.0.0:9489"` vs service default | Service listens on loopback | `config.rs:125` | SOP step 1 must say so for the Pi |
| SOP install bullets: `admin_uids` "if given" | Admin verbs need root or a listed uid | `config.rs:255-257`, `local_server.rs:260-268` | Owner step 1 passes `--admin-uid "$(id -u)"` |
| SOP "launchd retries every 10 s" (service) vs `kernel.md` step 7 "every 30 s" (user agent) | Two different units, both correct | `weftos-mesh.plist:25-26`, `service_units.rs` | Say which unit each number is about |
| `client.rs:44-49`, `mesh_local_glue.rs:374` "arrives with Phase 3 package S" | `weaver mesh trust` shipped | `mesh_cmd.rs:439-453` | Update the strings |
| A9 "Journal principal `{"kind":"uid","id":501}`" | Matches | `peer.rs:8-13` | None |
| A9 socket-dir "group-owned (0750)" vs plan "0770 or 0775" | 0750, and the service accepts it | `golden/mesh-install-launchd.sh:46`, `main_loop.rs:150-157` | None (A9 wins) |
| A9 "stale keys retry at 30 s" | Matches | `mesh_local_glue.rs:347,378-389` | None |
| ADR known limits (H/X): rundir helper convergence, log rotation, Windows refusal, receipt tier, `unsigned_leaf_peers` | All as stated | `service_units_system.rs:120-121`, `mesh_doctor.rs:174` | None |
| `kernel.md` "Owner migration" | No Phase 3 pointer | `docs/guides/kernel.md:376-420` | One line to the SOP |
| `build.md` `--all-features` note | Matches (`testing` feature) | `clawft-mesh-service/Cargo.toml` | None |

## 7. Residual risks and follow-ups (ranked)

1. **S1** `peer revoke` without disconnect, revocation not re-checked after
   admission. Security-relevant under `enforce`.
2. **S2** `auto` can mint a new node key on a machine that has used the
   service. Identity.
3. **S6** torn journal tail is a 12 h deadline for every user link.
   Availability on the owner's machine.
4. **S7** the lane never executes the shipped binary; adoption, CLI
   `--accept-truncate` and cert expiry during an outage are unasserted.
5. **S3** per-IP cap only under `enforce`; 1024 global cap always. Matters
   once `listen` leaves loopback.
6. **S9** service-attested machine pubkey standing in for the daemon in three
   registries. Blocks a second human account on one box.
7. **S4** kernel accepts `enforce` without Noise and refuses everyone silently.
8. **S8** `mesh.exe` heuristic.
9. **S5** `build_endpoint` side effects under `service = off`.
10. **S10** systemd start-limit dead-end, `$WEFTOS_MESH_SOCKET` in `update`,
    stopped-vs-crashed doctor wording, pre-boot registration window, two
    `service.json` readers, K0 header, seed dials, flaky sleeps.
11. First-contact TOFU with no out-of-band check is the documented residual
    (plan 7). For the adopt case the owner's check is simply that
    `weaver mesh status` prints the pre-migration node id.
12. Leaves stay unsigned under `observe`; `enforce` is a later flip after the
    Pi and ESP32 peers are redeployed (plan D-2).
13. Phase 1 S-items still open and touched by this range: S6 (doctor user
    root) is now covered by `weaver doctor runtime`; S9 (launchd exit-78
    sentinel) is unchanged and now also applies to the service's port-conflict
    loop.

## 8. Owner real-install checklist (one Mac, the Pi as the remote peer)

Record before anything: `weaver kernel status --profile user` -> note the
`Node:` id (call it N0). `ls -l ~/.weftos/chain/chain.key ~/.weftos/run/node.key`
should both be `-rw-------`. Copy `~/.weftos/run/node.key` to a 0600 backup
outside `~/.weftos`.

1. **Print and read the installer.**
   `weaver mesh install-service --adopt-node-key ~/.weftos/run/node.key --admin-uid "$(id -u)" --listen 0.0.0.0:9489 > install.sh`
   (drop `--listen` if the Pi does not need to reach this machine). In the
   script check: the `install -m 0755 -o root` source is the packaged binary
   you mean; the adopt block has `cmp -s` and `install -m 0600 -o _weftos`;
   `admin_uids = [501]` in the `mesh.toml` heredoc; the `# WARNING: listen`
   header is present only if you asked for LAN. Then `sudo sh install.sh`.
   Expect no service running yet.
2. **Group.** Log out and in. `id -Gn | tr ' ' '\n' | grep -x _weftos`.
   `ls -ld /var/run/weftos` -> `drwxr-x--- _weftos _weftos`.
3. **User key.** `weaver migrate user-key --dry-run`, then without. `weaver
   doctor runtime` -> `user_key` OK, same identity.
4. **Swap the listener.** `weaver kernel stop --profile user`;
   `lsof -nP -iTCP:9489` empty. `sudo launchctl bootstrap system
   /Library/LaunchDaemons/ai.weftos.mesh.plist`. Then:
   `sudo launchctl print system/ai.weftos.mesh | grep -E 'state|pid'`;
   `ps -o user= -p <pid>` -> `_weftos`;
   `lsof -nP -iTCP:9489 -sTCP:LISTEN` -> that pid on the address you chose;
   `ls -l /var/run/weftos` -> `mesh.sock` and `service.json`;
   `tail /var/log/weftos/mesh.log` -> "mesh service started" with node N0.
5. **Identity and pin.** `weaver mesh status` -> `node N0`. `weaver mesh
   trust` -> writes `~/.weftos/mesh/machine.pub`. `sudo head -c 400
   /var/lib/weftos/mesh/journal.jsonl` -> `"kind":"machine.init"` with
   `"key_origin":"adopted"`. `weaver mesh journal verify` -> verifies.
6. **Require the service.** `[kernel.mesh] service = "required"` in
   `~/.weftos/weave.toml`; start the user daemon. `weaver kernel status
   --profile user` -> `Mesh: service (connected)`, `Node: N0`, roles `user`.
   `weaver mesh bindings` -> `bound uid 501 ... (registered)`. `weaver doctor
   runtime` -> every `mesh.*` OK except `mesh.node_key_dup` WARN.
7. **The Pi.** From the Pi, the peer list shows N0 (only if step 1 set a LAN
   `listen`; expect the macOS firewall prompt for
   `/usr/local/libexec/weftos/weaver` the first time).
8. **Kill and recover.** `sudo launchctl kickstart -k system/ai.weftos.mesh`.
   Daemon log: "mesh service link lost; reconnecting" then "mesh service
   link up". `weaver mesh status` -> same node, journal seq advanced by one
   `service.start`, `weaver mesh bindings` unchanged, no `READ-ONLY`. If
   `READ-ONLY` ever appears: `weaver mesh journal verify --accept-truncate`.
9. **Rollback drill (before removing `node.key`).** Stop the service
   (`sudo launchctl bootout system/ai.weftos.mesh`), set `service = "off"`,
   restart the daemon -> `Mesh: collapsed`, `Node: N0`. Then reverse it.
10. **Remove the duplicate key** only after step 7 passed:
    `rm ~/.weftos/run/node.key` (keep the backup). `weaver doctor runtime`
    -> no `node_key_dup`. Leave `service = "required"`.
11. **Update path.** After the next build, `weaver update` prints
    `sudo install ... /usr/local/libexec/weftos/weaver` and
    `sudo launchctl kickstart -k system/ai.weftos.mesh` and runs neither
    (service pid unchanged).
12. **Steady state.** The user chain now gains `mesh.service.bound` and
    `mesh.journal.anchor` events; that is expected.

## Gate result (recorded by the lead)

Reported by the lead for `integrate/p3` @ 86ff8c936: the full suite passed
(10526 tests) and `scripts/build.sh gate` passed 20 of 20, with the UI build
needing `npm ci` in the worktree first. This review ran nothing; the lane
`scripts/build.sh test-mesh-service` was not re-run here.

## Re-check: fix round `wt/p3-fable-fixes` @ 3a53fcd58 (diff 83663fdf3..3a53fcd58)

Read-only pass on 2026-10-02 against sections 2 and 3 above. Nothing was run
here; the lead's gate on that worktree is the test evidence.

- **M1 fixed.** `Plan::from_args` now takes the collapsed key and
  `mesh_install_key::key_notes` refuses when `~/.weftos/run/node.key` exists
  and neither `--adopt-node-key` nor `--fresh-node-key` is given, with both
  commands in the error (`commands/mesh_install_key.rs:59-78`,
  `mesh_install.rs:127,375-378`). `--fresh-node-key` puts a `# WARNING` in the
  script header and on stderr; the two flags are mutually exclusive. The
  "exists and differs" branch now prints a remedy with the right stop command
  per manager (`mesh_install.rs:321-345`, golden `mesh-install-launchd.sh:61-67`).
  A new machine (no collapsed key) still generates silently, which is right.
  Tests: `a_collapsed_key_is_never_replaced_by_omission`,
  `a_collapsed_key_needs_an_explicit_choice`.
- **M2 a-j: all present and accurate against the code.** Listen default and
  the Pi, `--admin-uid`, `required` after step 5, rollback `chown`/`chmod`,
  pin wording (daemon pins, verbs compare, `trust` writes), the five
  `configuration.md` keys (match `config/kernel.rs:880-953`), `peer revoke`
  wording, "never `sudo` for the service", both stale strings, the `kernel.md`
  pointer. One line survives from before: SOP "Moving to the machine mesh
  service" step 3 still says to compare the fingerprint "with the one the
  install script printed"; the script prints no fingerprint (a shell script
  cannot derive the public key from the seed; `mesh_install.rs:322` only
  echoes "compare ... out of band"). For the adopt case the real check is
  that `weaver mesh status` prints the pre-migration node id. Doc nit, not a
  blocker.
- **M3 fixed.** ADR-103 A10 records the ~600 s compounded cache, that
  `observe` is not protection, the steady-state chain events, and the two
  identity guards. Matches the code.
- **S1 fixed, with one cost note.** `serve_connection` now ticks every 250 ms
  (`mesh_limits.rs:12`, `mesh_serve.rs:341-343,365-375`): a connection whose
  route no longer sends through its channel (`MeshRuntime::routes_via`,
  `mesh_runtime.rs:351-354`) is closed, and under `enforce` a revoked bound
  id is closed on the tick and before every frame
  (`mesh_admit_gate.rs:305-309`, `mesh_serve.rs:398-405`); `observe` keeps
  the old admission-only semantics (`live_revocation_applies_under_enforce_only`).
  `disconnect_peer` has no other production caller than admin revoke and
  shutdown (`admin.rs:205`, `main_loop.rs:121`, `mesh_system_service.rs:115`),
  so no reaper now closes live sockets. The idle timer regression is handled:
  the silence deadline is `sleep_until(last_activity + d)` and only inbound
  and outbound frames advance `last_activity` (`mesh_serve.rs:347,356,409,420`),
  so the tick cannot keep a silent peer alive. No new liveness bug found.
  Two notes for follow-up cards: (1) `routes_via` scans the whole peer
  `DashMap` per tick per routed connection, O(peers) x connections x 4/s;
  at the 1024 cap that is a few million entry visits per second. An O(1)
  check (look up the ids this channel registered) or a per-connection flag
  flipped by `disconnect_peer` would remove it. (2) Under `observe`, a
  `source_node` claim already replaced an unverified route
  (`mesh_runtime.rs:313-329`); it now also closes the legitimate peer's
  connection within 250 ms, so a takeover becomes a flap between the two
  connections. A10's "observe is not protection" should say so. Under
  `enforce` the takeover is refused as before.
  Test `a_peer_revoke_closes_the_live_connection` covers the route-removed
  path end to end.
- **S2 fixed.** `mesh_boot::refuse_fresh_identity` refuses under `auto` when
  `~/.weftos/mesh/machine.pub` exists and `<runtime>/node.key` does not,
  before any key is generated (`mesh_boot.rs:67-70,86-97`); `off` collapses
  deliberately and `auto` with a key present (rollback) still works.
  Process-level test `tests/mesh_boot_guard.rs` asserts no key is written;
  it is in the e2e lane (`scripts/dev/mesh-p3-e2e.sh:46`).

**Final verdict: SHIP to `0.8-metaharness`.** No remaining must-fix. Carry
over as cards: the SOP fingerprint sentence (step 3), the two S1 notes above,
and S3-S10 from section 3.
