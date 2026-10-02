# Weave topology Phase 3: file-level implementation plan

> **Decisions (2026-10-01):** the owner accepted D-1..D-10 as recommended (ADR-103 A6). macOS: a non-root LaunchDaemon can write `/var/run/weftos` only when the service account is in the directory's group, so package H's installer creates `/var/run/weftos` group-owned by `_weftos` (mode 0770 or 0775) and adds the service account to that group; verify on the owner's Mac.

Status: plan, not started. Implements Phase 3 of [ADR-103](../adr/adr-103-weave-topology-roles-and-instances.md) (card weave-topology-P3): the machine mesh service as an OS service. Analysis: [daemon-topology/analysis.md](../research/daemon-topology/analysis.md) sections 4, 5, 9, 10.4. Builds on [P1](weave-topology-p1-plan.md) and the Phase 0 review (R2, R9). Date: 2026-09-30.

Assumes P0 and P1 have landed (P0 resolver and fatal mesh bind; P1 user daemon at `~/.weftos/run/` with roles `machine,user`, `kernel.status` handshake, `rpc_ext.rs` hook, `service_units.rs`, token authority, chain migrated to `~/.weftos/chain/`). Where this plan names a P1 module it means that branch's real name; agents read it first. P2 (project kernels) is independent: a project kernel never talks to the mesh listener, it registers its address with the user daemon, which forwards it (package U).

## 0. Findings that shape the work (verified in the tree)

1. **Admission is not enforced on the mesh listener today.** The accept loop (`clawft-kernel/src/boot.rs` ~578-700) does an optional Noise XX responder handshake and then `MeshRuntime::handle_incoming_from` registers the sender under `envelope.source_node`, a self-asserted string. `JoinRequest`/`JoinResponse` (`mesh_listener.rs:158-185`) and `FrameType::JoinRequest` exist but nothing verifies the ADR-025 steps (genesis hash, signature, skew, gate). `RevocationList::is_revoked` is only consulted by `ClusterMembership::add_peer_checked` (`cluster.rs:865`), which the listener never calls. "Cryptographic admission and revocation" in the service is therefore **net-new code (package K1), not a move**.
2. **Noise is not bound to node identity.** The Noise static key is a fresh random X25519 key per boot unless `noise_key_path` is set (`boot.rs:549-572`), unrelated to the Ed25519 node key. Admission must bind the two by signing the Noise handshake hash (channel binding); `NoiseChannel` (`mesh_noise.rs:120-225`) keeps `remote_static` but does not expose the handshake hash. K1 adds it.
3. **The mesh runtime is welded to the kernel in three places:** `MeshRuntime.local_router: Option<Arc<A2ARouter>>` (`mesh_runtime.rs:56,203`), `set_chain_manager` (`:262`, peer.envelope events), and the assessment transport (`:143`). The accept loop, seed-connect loop and `MeshService` registration live inline in `boot.rs` (~464-800). The service cannot reuse any of it until K0 extracts it behind small traits.
4. **Reusable as is:** `node_id_from_pubkey` (`node_id.rs:18`), `load_or_generate_node_key` (`node_key.rs:53`, refuses symlinks/insecure files, atomic publish), `sign_node_facts`/`verify_node_facts` (`node_facts_advert.rs:127,149`, domain-separated, pure functions) and the `node_facts` probe, `RevocationList`, `MeshTransport` implementations (tcp/ws/quic), `mesh_discovery`, `mesh_heartbeat`, `mesh_mdns`, `mesh_kad`.
5. **Peer credentials exist exactly once** (package F branch, `clawft-weave/src/daemon.rs:3133`): tokio `UnixStream::peer_cred()` compared to `geteuid()`, failing closed on error. tokio's `UCred` gives uid and gid on Linux and macOS and pid where the OS reports it (pid is informational only; macOS `LOCAL_PEERCRED` has no pid). `relay_auth.rs` is the TCP-to-unix sanitiser, unrelated to mesh. Windows accept loop uses `clawft_rpc::named_pipe` (no credential check).
6. `MeshIpcEnvelope` (`mesh_ipc.rs:77`) has `source_node, dest_node, message, hop_count, envelope_id` and serde defaults are the compatibility tool: new scope fields must be `#[serde(default, skip_serializing_if = "Option::is_none")]`. `ClusterMembership`, `NodeIdentity::sign` and `capability_claim` sign with the node key inside the kernel; in service mode the daemon does **not** hold the box key, so those signing uses move to the user key (cert chain) or to the service (package U lists the call sites).
7. The kernel `mesh` feature is separable from `exochain` in Cargo.toml (`mesh = [ed25519-dalek, rand, tokio-tungstenite, futures-util, snow]`), but no one builds `--no-default-features --features native,mesh` today. Whether it compiles is **unverified**; K0 proves it or Risk R1 triggers.

## 1. Contracts every package codes against (fixed here)

### 1.1 Files and roots (D4)

| Path | Owner / mode | Content |
|---|---|---|
| `/var/lib/weftos/mesh/` | service user, 0700 | state dir |
| `.../node.key` | 0600 | box Ed25519 seed (`node_key.rs` format) |
| `.../journal.jsonl` (+ `journal.NNN.jsonl` segments) | 0600 | machine journal, section 1.3 |
| `.../revoked.json` | 0600 | `RevocationList` file |
| `.../facts.json` | 0600 | last signed machine facts |
| `.../service.json` | 0644 | public record `{node_id, machine_pubkey, service_uid, proto:{min,max}, build_sha, started_at}` clients pin |
| `.../mesh.toml` | root:service 0640 | `listen`, `transport`, `bind_policy`, `admission`, `cluster_owner_uid`, `admin_uids`, `cert_ttl_s` |
| `/var/run/weftos/mesh.sock` | service user, 0666 | mesh-local socket; authorization is by peer credential, never by file mode |
| `~/.weftos/user.key` | user, 0600 | user Ed25519 seed (package U) |
| `~/.weftos/mesh/machine.pub` | user, 0600 | pinned machine pubkey (client TOFU pin) |
| `~/.weftos/mesh/cert.json` | user, 0600 | current user certificate |

Tests and probes override with `WEFTOS_MESH_STATE_DIR` and `WEFTOS_MESH_SOCKET` (full isolation, same spirit as `WEFTOS_RUNTIME_DIR`); production units never set them.

### 1.2 mesh-local/1 (package L)

*Updated 2026-10-01 after the package L review: `hello.client_nonce` and `hello_ack.machine_sig` (server key-possession proof), length-prefixed register signing bytes, reply-class id correlation with service-originated ids flagged `1<<63`.*

Transport: unix stream (named pipe on Windows, section 6), newline-delimited JSON, one object per line, max line 1 MiB, 5 s deadline for `hello`, every object `{"t": "<type>", "id": <u64>?, ...}`. Binary payloads are not needed: mesh bodies are already JSON `KernelMessage`s (ADR-031 shipped encoding).

```
C->S  hello      {proto_min, proto_max, features:[..], role:"user"|"admin", build_sha, exe, pid,
                  client_nonce:"<32B hex>"}
S->C  hello_ack  {proto, features:[..], node_id, machine_pubkey, service_build_sha, deprecated_below?,
                  uid, challenge:"<32B hex>", machine_sig}  // uid = what the service read from the peer credential
      machine_sig = Ed25519(machine_key, "weftos/mesh-local/hello/v1\0" || client_nonce || challenge || machine_pubkey || principal(uid))
      // the client also requires ack.uid == its own euid (a trusted-uid relay cannot register under its own uid)
      // verified by the client against the pinned machine key BEFORE pinning or registering (key-possession proof)
S->C  error      {kind:"proto_mismatch"|"bind_conflict"|"bind_pending"|"address_in_use"|"forbidden"|"bad_sig"|
                  "rate_limited"|"scope_required"|..., message, remedy, data?}   // then close for fatal kinds
C->S  register   {user_pubkey, sig, addresses:{user_id, projects:[{project_id, project_pubkey, cert_sig}]},
                  topic_prefixes:[..], capabilities:[..], version, build_sha}
      sig = Ed25519(user_key, "weftos/mesh-local/register/v1\0" || challenge || principal || node_id)
      principal = tag(1=uid, 2=sid) || len_u16be || bytes;  node_id = len_u16be || bytes
S->C  register_ack {user_id, cert:<UserCert>, accepted:{addresses,topic_prefixes}, rejected:[{what,reason}],
                  bind:"new"|"existing"|"pending"}
C->S  renew {} -> S->C {cert}                     // client renews at 50% of lifetime
C->S  address.add|address.remove {project_id, project_pubkey, cert_sig} -> ack|error
C->S  subscribe|unsubscribe {prefix}              -> ack|error
C->S  send       {dest:"weft://...", message:<KernelMessage json>, request_id?}    // outbound
S->C  deliver    {source_node, source_cert?:<UserCert>, scope:{user_id, project_id?}, envelope_id, message}
S->C  verdict.request {id, subject:"peer.admit"|"cluster.join"|"publish"|"subscribe", peer:{node_id, pubkey,
                  platform, capabilities, genesis_hash, chain_seq}, topic?}
C->S  verdict.reply   {id, allow, ttl_s, reason, rule_hash}      // daemon evaluates GateBackend; service only caches
C->S  journal.head {} -> S->C {seq, hash, ts, sig}                // daemon anchors this into its user chain
C->S  status|peers.list|facts.get                -> reply          // read-only
admin (role:"admin", uid in admin_uids or root):
C->S  bindings.list | bind.approve {uid} | bind.revoke {uid, reason} | bind.rebind {uid, user_pubkey?}
      | peer.revoke {node_id, reason} | peer.unrevoke {node_id} | policy.set {admission?, cluster_owner_uid?}
C->S  ping/pong, bye
```

Negotiation: `proto = min(client.proto_max, service.max)` and must be `>= max(client.proto_min, service.min)`, else `proto_mismatch` carrying `{service:{min,max,sha}, client:{min,max,sha}}` and the remedy line ("restart the service after `weaver update`" or "update the user daemon"). `features` are intersected; unknown features are ignored, not errors. Support window: the service accepts `[PROTO_MIN, PROTO_MAX]` (1..=1 now); raising `PROTO_MIN` needs one release of `deprecated_below` in `hello_ack` and a doctor WARN on the daemon side. The daemon RPC proto (`clawft-rpc PROTO_VERSION`) is a separate number. `build_sha` is the same key `daemon_guard.rs` reads, so skew is reportable per tier.

### 1.3 Machine journal schema (package J)

JSON lines, hash-chained and signed by the box key. One record:

```json
{"v":1,"seq":42,"ts":1790000000,"prev":"<hex sha256 of previous line bytes; 64 zeros at seq 0>",
 "kind":"user.bind","body":{...},"sig":"<hex Ed25519(box_key, \"weftos/mesh-journal/v1\\0\" || canonical line without sig)>"}
```

`canonical line without sig` = the record serialised by the typed struct with `sig` absent (field order fixed by the struct, tested by a golden file). Kinds and bodies:

| kind | body |
|---|---|
| `machine.init` | `{node_id, machine_pubkey, key_origin:"generated"\|"adopted", build_sha}` |
| `service.start` | `{build_sha, proto:{min,max}, pid, listen, admission, bind_policy}` |
| `user.bind` | `{principal:{kind:"uid",id:501}\|{kind:"sid",id:"S-1-5-.."}, user_pubkey, user_id, how:"tofu"\|"approved"\|"rebind", by?:principal, peer_pid?, exe?}` |
| `user.bind_pending` | `{principal, user_pubkey, user_id, peer_pid?, exe?}` (policy `approve`) |
| `user.cert.issue` | `{user_id, serial, issued_at, not_after}` |
| `user.revoke` | `{principal, user_id, reason, by, serials_revoked_through}` |
| `peer.admit` / `peer.refuse` | `{node_id, pubkey, class:"node"\|"leaf", verdict_rule_hash?, reason?, mode:"enforce"\|"observe"}` |
| `peer.revoke` / `peer.unrevoke` | `{node_id, reason, by}` |
| `policy.set` | `{key, old, new, by}` |
| `journal.quarantine` | `{lost_from_seq, lost_count, serial_high_water, raw_serial_high_water, revoked_user_ids, quarantine:[file names], ts, recorded}` (reserved: written by the service when it quarantines a bad tail; `serial_high_water` is clamped to the last accounted serial plus `lost_count`) |
| `journal.accept_truncate` | `{quarantine_seq, marker_only?, serial_floor, quarantine, by:principal, auto?:"torn_tail"}` (reserved: written by `Bindings::accept_truncate` behind the admin gate, or by the service itself for a lone torn final line) |
| `facts.sign` | `{facts_hash, valid_until}` (hash only, not the facts) |

The journal records decisions, not traffic: no route churn, no message metadata (chatty and sensitive). `bindings` (principal to user key, serials, revocations) is a fold of the journal rebuilt at start; the journal is the source of truth and `bindings` is never persisted separately. Segment at 8 MiB (`journal.NNN.jsonl`, first record of a segment repeats the last `prev`). On start: verify the full hash chain and every signature; a bad tail record is quarantined to `journal.corrupt.<ts>` and the service starts **read-only for binds** (refuses new binds and cert issues, keeps serving existing registrations) until an admin acks with `weaver mesh journal verify --accept-truncate` (the one exception: a lone torn final line with no readable facts, the signature of a crash mid-append, was never acknowledged to any caller; the service accepts it at start and journals `auto:"torn_tail"`). Anchoring: the daemon periodically asks `journal.head` and appends `mesh.journal.anchor {seq, hash, node_id}` to its user chain (ADR-022), so truncation of the tail is detectable by any user chain holding a later anchor. The service owns no chain.

### 1.4 User certificate (package L types, J issues, U stores)

```json
{"v":1,"node_id":"..","machine_pubkey":"<hex32>","user_pubkey":"<hex32>","user_id":"<32 hex>",
 "serial":17,"issued_at":1790000000,"not_after":1790086400,"sig":"<hex64>"}
```

Signed bytes (fixed binary layout, not JSON, so no canonicalisation dispute): `"weftos/user-cert/v1\0" || machine_pubkey(32) || user_pubkey(32) || serial(u64 BE) || issued_at(u64 BE) || not_after(u64 BE)`. `user_id = node_id_from_pubkey(user_pubkey)` (the one hash, D11). The uid is deliberately **not** in the cert (meaningless to remote peers and a local-account leak); the uid binding lives only in the journal. Default lifetime 24 h, client renews at 12 h, verifiers allow 5 min leeway. Remote verification: cert chain machine -> user (and, from P2, user -> project via the `project.register` event); the machine key is the one remote peers already admitted, so no new trust root. Revocation: `user.revoke` journals the serials, the service stops renewing and drops the registration, and the revoked-serial list rides in the signed machine facts so peers can refuse a revoked cert before its expiry.

### 1.5 Admission handshake on the listener (package K1)

After Noise (when enabled) the first frame in each direction is `AdmitHello`:
`{v:1, node_id, pubkey, platform, capabilities, genesis_hash, ts, noise_static_pub, sig}` with `sig = Ed25519(node_key, "weftos/mesh-admit/v1\0" || noise_handshake_hash || genesis_hash || ts_be)`. The service checks, in order: `node_id == node_id_from_pubkey(pubkey)`; `noise_static_pub` equals the handshake's remote static; `sig` verifies over the *session's* handshake hash (replay-proof); `|now - ts| <= 60 s`; genesis hash equals the cluster genesis pinned in `mesh.toml`/journal; not on `RevocationList`; then a **verdict** (cached `ttl_s`, default 300) from the cluster-owner daemon via `verdict.request peer.admit`. Admitted peers are registered in `MeshRuntime` under the **verified** `node_id`; `source_node` in later envelopes must equal it or the frame is dropped. `admission = observe` (default for the first release, decision D-2) journals `peer.refuse mode:"observe"` for would-be refusals and admits anyway. Peers that never send `AdmitHello` (ESP32 leaves, Pi on an old build) are class `leaf`/`legacy`: allowed under `observe`; under `enforce` leaves are limited to publish on `substrate/<own-id>/` and subscribe by prefix, and `doctor` flags them (the bring-up exception from the leaf track).

### 1.6 `weft://` addressing (package L `addr.rs`)

Grammar: `weft://<node>/<user>/<project>/<topic...>` where `node` = 32 hex or `local`; `user` = 32 hex or `_` (node-level, no tenant); `project` = 26-char Crockford ULID or `_` (user-level); `topic` = existing dotted/slashed topic, may be empty. `local` resolves to the sender's registered node id at the service (never accepted on the wire between nodes). Wire: `MeshIpcEnvelope` gains `dest_scope: Option<Scope>` and `src_scope: Option<Scope>` with `Scope {user_id, project_id?}`; the service **stamps** `src_scope` from the sending registration (a daemon cannot claim another user's id) and strips any `src_scope` an unregistered peer sends. Routing rule at the service for inbound: scope present: deliver to that registration or reply `unknown_scope`; scope absent: match the longest registered `topic_prefix`; no match: deliver to the single registered user if exactly one, else drop and count (`scope_required`). One registration owns an address; a prefix claimed by two different uids is refused for the second.

## 2. Work packages

Crates: new `clawft-mesh-local` (protocol, certs, addr, peer identity, client; deps: serde, serde_json, ed25519-dalek, sha2, tokio, nix; **no clawft-kernel dependency**) and new `clawft-mesh-service` (the service; depends on `clawft-mesh-local` and `clawft-kernel` with `default-features = false, features = ["native","mesh"]`; **must not enable `exochain`, `cluster`, `tilezero`, `ecc`**). `weaver mesh ...` subcommands live in `clawft-weave` and call the service crate.

### K0. mesh-seam (crate `clawft-kernel`, behaviour-preserving refactor, first)
Depends on: none. Blocks: K1, S, U.
- new `mesh_serve.rs`: move the accept loop, per-connection Noise + pump loop and seed-connect loop out of `boot.rs` into `pub async fn serve_listener(runtime, listener, noise, gate: Arc<dyn AdmissionGate>)` and `pub async fn connect_seeds(..)`. `boot.rs` calls them with `AllowAll` (identical behaviour).
- `mesh_runtime.rs`: replace `local_router: Option<Arc<A2ARouter>>` with `Arc<dyn LocalDelivery>` (`async fn deliver(&self, scope: Option<&Scope>, msg: KernelMessage) -> KernelResult<()>`); `A2ARouter` gets an impl; `set_chain_manager` and assessment stay optional and `cfg(exochain)`-gated so the service never sets them.
- `mesh_ipc.rs`: add `dest_scope`/`src_scope` (serde-default). `mesh_noise.rs`: `NoiseChannel::handshake_hash()`.
- new `mesh_mode.rs`: `pub fn select(cfg:&MeshConfig) -> MeshMode {Service{sock}|Collapsed|Off}` (`kernel.mesh.service = auto|required|off`, section 4). Called from `Kernel::boot` **and** `weft kernel boot --foreground`, closing the R2 hole where the CLI foreground path bypasses daemon-level checks.
- `Cargo.toml`: nothing new; add a `scripts/build.sh` target `check-mesh-only` running `cargo check -p clawft-kernel --no-default-features --features native,mesh` (CLAUDE.md: extend the script, never raw cargo).
Tests: existing `mesh_*` tests unchanged and green; a new test that `serve_listener` with `AllowAll` and a fake `LocalDelivery` delivers a scoped envelope; old-JSON envelope without scope fields still deserialises; `boot_registers_mesh_system_service` (`boot.rs:2744`) unchanged.
Acceptance: `scripts/build.sh check`, `clippy`, `test`, `check-mesh-only`; `gate` before merge (touches boot).

### L. mesh-local (new crate `clawft-mesh-local`)
Depends on: none (types fixed in section 1). Blocks: J, S, U.
- `proto.rs` (all messages in 1.2, `PROTO_MIN/MAX`, `negotiate()`), `cert.rs` (`UserCert`, `sign`, `verify(chain, now)`, binary signed layout), `addr.rs` (`WeftAddr::parse/Display`, section 1.6), `peer.rs` (`trait PeerIdentity { fn principal(&self)->Result<Principal> }`; unix impl over `peer_cred()` fail-closed; `Principal::{Uid(u32), Sid(String)}`; windows impl returns `Unsupported` and callers refuse), `framing.rs` (bounded line reader, deadlines), `client.rs` (`MeshLocalClient`: connect, verify the **server** (below), hello, register, renew, reconnect with jittered backoff, request/reply correlation, event stream), `testing.rs` (a loopback in-memory server for U's tests before S merges).
- Client-side server verification (anti-squat): after connect, `peer_cred().uid()` of the *server* must be `0` or equal to `service_uid` in `service.json`; `hello_ack.machine_pubkey` must equal the pin in `~/.weftos/mesh/machine.pub` (written on first contact, then compared; mismatch is a hard `machine_key_changed` error that tells the user to run `weaver mesh trust` after verifying out of band).
Tests: golden JSON for every message; cert sign/verify, tamper each field, expiry, leeway; addr round-trips plus malformed (path characters, uppercase node, bad ULID); negotiation matrix (older/newer client, feature intersection); line-length and deadline limits; client refuses a server whose uid differs.
Acceptance: crate tests, clippy, check; `cargo tree -p clawft-mesh-local` contains no `clawft-kernel`.

### J. journal-and-bindings (crate `clawft-mesh-service`, modules `journal.rs`, `bindings.rs`)
Depends on: L (types). Blocks: S.
- `Journal::open(dir, key)` (verify chain and signatures, quarantine bad tail), `append(kind, body)` (fsync, seq/prev/sig), `head()`, `iter()`; `Bindings::fold(&Journal)` with `bind(principal, key, how)`, `check(principal, key) -> Existing|New|Conflict|Pending`, `revoke`, `rebind`, `serials`. Invariants enforced in `Bindings`, not in callers: one key per principal, one principal per key, rebind revokes the old key's serials.
- single writer: `flock` on `mesh.lock` in the state dir; a second service exits naming the holder pid.
Tests (tempdir, no root): append/replay golden; tamper one byte at each position (every position detected); truncated tail quarantined and binds refused; segment rollover keeps the chain; concurrent appenders serialised by the lock; bind conflict, key-reuse-by-second-uid refused, rebind semantics; fold is deterministic and equals incremental state (metamorphic).
Acceptance: crate tests, clippy, check.

### K1. admission (crate `clawft-kernel`, `mesh_admit.rs`; kernel stays policy-free)
Depends on: K0. Blocks: S (service wires it), no effect on collapsed boot until enabled.
- `AdmitHello` encode/verify (section 1.5), `trait AdmissionGate { async fn admit(&self, hello:&VerifiedHello, class)->Admission }` plus `AllowAll` (K0 default) and `CryptoGate { genesis, revocations, verdicts: Arc<dyn VerdictSource>, mode }`. `serve_listener` enforces `source_node == verified node_id` after admission. In collapsed mode `VerdictSource` is the local `GateBackend` wrapper (`cluster.join` action), so collapsed and service mode share one code path.
- `mesh.toml`/`MeshConfig`: `admission = off|observe|enforce` (default `observe`), `genesis_hash`.
Tests: bad sig, wrong node_id for pubkey, handshake-hash replay on a second session, skewed `ts`, wrong genesis, revoked peer, `observe` admits and journals, `enforce` refuses, leaf class limits, source_node spoof after admission dropped; both Noise and passthrough channels (passthrough has no handshake hash: `enforce` refuses passthrough peers, `observe` records them).
Acceptance: kernel tests, clippy, check, `check-mesh-only`.

### S. mesh-service (crate `clawft-mesh-service` + `clawft-weave/src/commands/mesh_cmd.rs`)
Depends on: K0, K1, L, J. Blocks: U integration, H.
- `config.rs` (`mesh.toml` + env overrides + flags `--state-dir --socket --listen --config`), `main_loop.rs` (`run(cfg)`: refuse `euid==0`; verify state dir owner and mode 0700, socket dir owner; load/generate box key via `load_or_generate_node_key`; open journal; sign facts with the existing `node_facts` probe and `sign_node_facts`, refresh on change; start `MeshRuntime` + `serve_listener` + discovery/heartbeat/mdns/kad with a `TenantRouter`; write `service.json`), `registry.rs` (registrations keyed by user id, prefix table, per-registration bounded queue 256, counters), `local_server.rs` (accept on the unix socket, `PeerIdentity` per connection, hello/negotiate/challenge/register state machine, 64 connections and 5 registrations per minute per principal, admin gate), `verdicts.rs` (cache with TTL, `VerdictSource` that asks the cluster-owner registration, fail-closed after `verdict_timeout` except the stale-grace rule in D-3), `facts.rs`, `health.rs` (loopback-only `127.0.0.1` health JSON, nothing else listens).
- `TenantRouter implements LocalDelivery`: inbound envelope to registration lookup to `deliver` frame; outbound `send` to `MeshIpcEnvelope` with stamped `src_scope`, `source_node = machine node id`.
- `weaver mesh serve [--config --state-dir --socket --listen]`, `weaver mesh status` (reads `service.json` + `status` over the socket), `weaver mesh bindings|bind approve|revoke|rebind`, `weaver mesh peer revoke|unrevoke`, `weaver mesh journal verify [--accept-truncate]`, `weaver mesh trust`. Admin verbs connect as `role:"admin"`; they hold no keys.
- **Must not own** (enforced three ways): no `Kernel`, `ChainManager`, `GateBackend`, token or secret type is constructed (a `scripts/build.sh` gate fails if `cargo tree -p clawft-mesh-service -e normal,features` contains `exo-resource-tree`, `rvf-runtime`, `cognitum-gate-tilezero`, `clawft-weave` or the kernel `exochain` feature); a runtime test lists the state dir after a full session and asserts the filename allow-list of section 1.1; the service exposes no RPC method that evaluates governance (grep test over `local_server.rs` message handlers).
Tests (as the current user against tempdirs, loopback `--listen 127.0.0.1:0`, socket under a tempdir; two real clients in one process using different keys and an injected `PeerIdentity` for the second uid): register new/existing/conflict/pending; second daemon for the same uid gets `address_in_use` with holder pid; cert issue and renewal and expiry; admin verbs refused for a non-admin principal; scoped and unscoped delivery routing incl. prefix conflict and the single-user default; `src_scope` stamping and stripping; verdict cache TTL and fail-closed; service restart keeps bindings (journal fold) and clients reconnect; version mismatch both directions; kill -9 mid-append then restart is consistent; two services on one state dir (second refuses). A real-peercred test (same uid) runs always; a **two-uid** test is `#[ignore]` and driven by an optional root-run script (never in the gate).
Acceptance: crate tests, clippy, check, `check-mesh-only`, the no-owned-state gate, a manual run: `weaver mesh serve --state-dir $(mktemp -d) --socket ... --listen 127.0.0.1:19489` plus `weaver mesh status`.

### U. user-daemon-mesh-client (crate `clawft-weave`, `clawft-kernel` boot glue, `clawft-types` config)
Depends on: L (S for integration). Blocks: integration commit.
- new `clawft-weave/src/mesh_local_glue.rs`: on boot with `MeshMode::Service`, connect via `MeshLocalClient`, register (user key, addresses = user id plus P2 project ids, topic prefixes, capabilities), keep the cert fresh, expose `MeshSink` (`LocalDelivery` into the daemon's `A2ARouter`) for `deliver`, forward outbound `A2ARouter` remote targets as `send`, answer `verdict.request` with `GateBackend` (`peer.admit`, `cluster.join`, `publish`, `subscribe` actions, context includes `rule_hash`), periodically `journal.head` then `chain.append("mesh.journal.anchor")` and `mesh.service.bound {node_id, cert_serial}`. If the service drops, the daemon keeps running, queues anchors, serves local RPC, and reconnects with backoff; `weaver health` shows `mesh: service (connected|reconnecting)`.
- Node identity in service mode: the daemon's `node_id` is the service's (`hello_ack.node_id`); `~/.weftos/run/node.key` is not read; `NodeIdentity::sign` call sites (`capability_claim.rs:146`, cluster join signing, `node_facts` advert) are replaced by: facts and admission signed by the service; capability claims signed by the **user key** and carried with the user cert. List and patch every `from_signing_key`/`sign` use in `boot.rs:~346-363` and `cluster.rs`; none may silently fall back to a locally generated key (fail boot with the reason).
- `handshake.rs` (P1): `roles` becomes `["user"]` in service mode and `["machine","user"]` collapsed; add `mesh: {mode:"service"|"collapsed"|"off", service_node_id?, cert_serial?, cert_not_after?, proto?}` (serde-default, older clients ignore it).
- user key (decision D-5): new `clawft-types/src/config/chain_paths.rs::user_key_path(home)` = `~/.weftos/user.key`; new `clawft-weave/src/commands/migrate_cmd.rs` verb `weaver migrate user-key [--dry-run]`: copies the 32-byte seed of the migrated `~/.weftos/chain/chain.key` to `user.key` (0600, atomic hard-link publish as in `node_key.rs`), proves the two public keys are equal, writes `user.key.MIGRATED_FROM.json {source_sha256_of_pubkey, at}`. The chain loader prefers `user.key` when present and falls back to `chain.key`; `chain.key` is never deleted by code; `user_id` is unchanged. `doctor` reports both files during the transition and WARNs if their pubkeys differ (a split-identity bug).
- `crates/clawft-types/src/config/kernel.rs`: `mesh.service = "auto"|"required"|"off"` (default `auto`).
Tests (clawft-weave integration over S in-process or L's loopback): boot in service mode registers and shows the handshake; collapsed when no socket; `required` without a service fails boot with the reason; service restart then re-register and cert refresh; verdict round trip through a real `GateBackend` rule; anchors survive a service outage and flush after reconnect; user-key migration idempotent, equal pubkeys, never touches `chain.key`; machine pin mismatch is fatal.
Acceptance: crate tests, clippy, check, `scripts/build.sh gate` (touches boot).

### H. service-units-and-installer (crate `clawft-weave`, `scripts/build.sh`, docs)
Depends on: S (binary and flags), P1-H (`service_units.rs`). Blocks: none.
- `service_units.rs` adds pure generators: `launchd_system_plist(exe,state,sock)->String` (label `ai.weftos.mesh`, `UserName _weftos`, `GroupName _weftos`, `KeepAlive`, `RunAtLoad`, `ProgramArguments [exe, mesh, serve, --config, /etc/weftos/mesh.toml]`, logs under `/var/log/weftos/`), `systemd_system_unit(..)->String` (`weftos-mesh.service`: `User=weftos`, `StateDirectory=weftos/mesh`, `StateDirectoryMode=0700`, `RuntimeDirectory=weftos`, `RuntimeDirectoryMode=0755`, `NoNewPrivileges=yes`, `ProtectSystem=strict`, `ProtectHome=yes`, `PrivateTmp=yes`, `CapabilityBoundingSet=` empty, `RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6`, `Restart=always`), `sysusers_conf()` and a macOS `dscl` account snippet. The existing user-unit generators (P1 H) gain the `mesh.service` mode flag only.
- new `commands/mesh_install.rs`: `weaver mesh install-service` **prints** a reviewed shell script (create account, create dirs with modes, copy the binary to `/usr/local/libexec/weftos/weaver` root-owned 0755, write `mesh.toml`, install the unit, the exact enable command); `--adopt-node-key PATH` adds the copy of an existing key (decision D-4); it never executes anything and refuses to print an `--apply` form in P3. `weaver mesh uninstall-service` prints the inverse and never deletes `/var/lib/weftos/mesh/node.key` unless `--purge-key` is spelled out.
- installer tier (amends `install-update-review.md` 6.4 and the receipt): receipt gains `tiers: [{name:"service"|"user"|"project", exe, sha, version, owner}]`; `weaver doctor install` reads the service tier from `service.json`/`mesh status` (never trusts the receipt alone) and reports skew; `weaver update` restarts the user daemon as in P1, and for the service **only prints** `sudo launchctl kickstart -k system/ai.weftos.mesh` / `sudo systemctl restart weftos-mesh` plus the `cp` to `/usr/local/libexec/weftos/` when the packaged sha differs; it never invokes sudo. `install.sh` stays user-only.
- `doctor` gains `mesh` checks: service reachable, proto window, pinned machine key, journal verifies, box key mode and owner, `node.key` still present in `~/.weftos/run/` while the service is active (WARN), leaf peers unsigned, two listeners on 9489 (FAIL).
Tests: golden files for plist/unit/sysusers/script under `crates/clawft-weave/tests/golden/`; `plutil -lint` and `systemd-analyze verify` as optional script steps when present; the script printer escapes paths with spaces; restart logic never signals the service pid.
Acceptance: crate tests, clippy, check; manual: `weaver mesh install-service | sh -n` and `plutil -lint`.

### W. windows-design (doc only in P3, section 6) and X. integration commit (owner: U)
X wires `MeshMode`, the doctor checks and the end-to-end script `scripts/dev/mesh-p3-e2e.sh` (service + two user daemons as the current user in tempdirs; no root); it runs under `scripts/build.sh test-mesh-service` (new target: `cargo test -p clawft-mesh-local -p clawft-mesh-service`, the no-owned-state gate, `check-mesh-only`).

## 3. Shared files, ownership and merge order

| File | Touched by | Rule |
|---|---|---|
| `clawft-kernel/src/boot.rs` | K0 (extract), U (identity guard) | K0 first and alone; U adds one guarded block after K0. |
| `clawft-kernel/src/mesh_runtime.rs`, `mesh_ipc.rs`, `mesh_noise.rs` | K0, K1 | K1 rebases on K0; K1 adds only new functions and the hook call. |
| `clawft-kernel/src/lib.rs`, `Cargo.toml` (workspace) | K0, K1, L, S | `pub mod`/member lines only. |
| `clawft-weave/src/commands/mod.rs`, `main.rs` | S, H, U | one `mod` and one subcommand each (`mesh`, `migrate user-key`). |
| `clawft-weave/src/service_units.rs` | H (owner), P1-H | H rebases on P1-H. |
| `clawft-weave/src/handshake.rs` | U | additive field only. |
| `clawft-types/src/config/kernel.rs`, `chain_paths.rs` | U | `mesh.service`, `user_key_path`. |
| `scripts/build.sh` | K0 (`check-mesh-only`), S (no-owned-state gate), X (`test-mesh-service`) | each adds one function plus one dispatch line. |

Merge order: **K0 and L in parallel** -> J (needs L types) -> K1 (needs K0) and S (needs K0, L, J; K1 lands in the same merge window, S ships with `admission=off` until K1 is in) -> U (develops against L's loopback, merges after S) -> H -> X integration + `scripts/build.sh gate`. One worktree per package, branches `wt/p3-<id>`. Every package runs `scripts/build.sh check`, `clippy`, `test` in its worktree before merging; K0, U and X run `gate` (boot changes).

## 4. Collapsed mode and mode selection (D2)

`kernel.mesh.service`: `auto` (default), `required`, `off`.
- `auto`: at boot, `mesh_mode::select` probes `mesh.sock` (connect, hello, verify server uid and pin). Reachable: **Service** (no listener bound by the daemon, mesh role is a client). Not reachable: **Collapsed** (the daemon binds 9489 itself with its own `node.key`, exactly Phase 1 behaviour). Mode is decided once per boot and logged; no hot switching (a service appearing later is reported by `weaver mesh status` and takes effect at the next daemon restart).
- `required`: boot fails when no service answers (servers, shared hosts). `off`: never probe; Collapsed.
- Mutual exclusion is the port: service and a collapsed daemon cannot both bind 9489 (P0 fatal bind), and the error names the holder; `weft kernel boot --foreground` goes through the same `select` (closes R2, and R9's default-bind surprise is documented in the service doctor check).

## 5. Migration from collapsed Phase 1 to a separate service (owner steps, nothing automatic)

Never touched by any P3 code: `~/.weftos/run/node.key`, `~/.weftos/chain/*`, `~/.clawft/*`, every project runtime dir, the Pi's `cluster_peers.json`. P3 only reads them (adopt copy) and writes under `/var/lib/weftos/mesh` (by the service) and `~/.weftos/{user.key,mesh/}`.
1. Build and install the packaged binary; `weaver mesh install-service --adopt-node-key ~/.weftos/run/node.key > install.sh`; read it; run it as admin (creates account and dirs, copies the binary and the key, writes `mesh.toml`). The key is now in two places; node id and the Pi's pinned id are unchanged.
2. `weaver migrate user-key --dry-run`, then for real (user.key = chain.key seed, same `user_id`).
3. Stop the collapsed user daemon (frees 9489), start the service (`launchctl bootstrap system ...` / `systemctl enable --now weftos-mesh`), `weaver mesh status`.
4. Set `kernel.mesh.service = "required"` in `~/.weftos/weave.toml`, restart the user daemon: it registers, the first bind is journalled (`how:"tofu"` or pending approval), `weft kernel status` shows `mesh.mode = service` and the same `node_id` as before.
5. Verify from the Pi that the box still appears with the same id. Only then remove `~/.weftos/run/node.key` (doctor WARNs until you do).
Rollback (before step 5's removal): stop the service, set `service = "off"`, restart the daemon; it binds 9489 with its own `node.key` and nothing in chains or ids has changed. After removal, rollback needs the key copied back from `/var/lib/weftos/mesh/node.key` by an admin. If the owner chooses a fresh box key instead of adopting, the Pi's pin must be re-established (open item in the analysis: which `node.key` the Pi pinned is unverified).

## 6. Windows design (not implemented in P3)

Service: a Windows Service via the `windows-service` crate running as the virtual account `NT SERVICE\WeftOSMesh`, state in `%ProgramData%\WeftOS\mesh\` (ACL: service + Administrators). Local channel: named pipe `\\.\pipe\weftos-mesh-local` created with an explicit DACL (Authenticated Users connect, service owns), `PIPE_REJECT_REMOTE_CLIENTS`, first instance flag (`FILE_FLAG_FIRST_PIPE_INSTANCE`) so a squatter cannot pre-create it. Peer identity: `GetNamedPipeClientProcessId` for the pid and `ImpersonateNamedPipeClient` + `GetTokenInformation(TokenUser)` for the SID, implemented behind `PeerIdentity` returning `Principal::Sid`; the journal already stores `principal:{kind:"sid"}` so no schema change. The client verifies the server with `GetNamedPipeServerProcessId` + token owner equals the service SID recorded in `service.json`. Until the Windows `PeerIdentity` exists the service refuses to start on Windows (fail closed, not "no check"). Admin set = members of the local `Administrators` group. Unit generation: `sc create` text printed by `install-service`, never executed. Open: UAC-less per-user pipe instances, and whether ConPTY/ProgramData ACLs behave under Defender; both need a Windows box (none here, unverified).

## 7. Safety: what must never run as root, and testing without installing services

- **Never root:** `weaver mesh serve` (refuses `euid == 0`, no override flag; tests run as the current user), the user daemon, project kernels, any code that reads `user.key` or `chain.key`. Root is used only by a human running the printed install script.
- The service never runs from a user-writable path: the unit points at `/usr/local/libexec/weftos/weaver` (root-owned, 0755). A service started from `~/.cargo/bin` would let that user replace the binary and steal the box key. `doctor` FAILs when the running service's `exe` is under a user-writable directory.
- The service never reads anything under a home directory (`ProtectHome=yes`), never opens a user chain, key or token, never listens beyond the mesh port, the unix socket and a loopback health port.
- Unix socket anti-squat (both directions): server verifies state/socket dir owner at start; client verifies server uid and the pinned machine key (section 2 L). A user cannot pre-create `/var/run/weftos` to impersonate the service without the pin mismatching (first-contact TOFU is the residual risk, documented; the install script prints the machine pubkey fingerprint for out-of-band comparison).
- Registration proves key possession (challenge signature) and uid (peer credential); it does not prove the caller is the user's *daemon* rather than any process running as that uid. That is the same trust domain as reading `~/.weftos/user.key` and is stated as such.
- **No-root testing:** every package runs the service as the current user with `WEFTOS_MESH_STATE_DIR`, `WEFTOS_MESH_SOCKET` and `--listen 127.0.0.1:0` in tempdirs; uid separation is exercised by the injected `PeerIdentity` and, optionally, a root-run script that uses `sudo -u nobody` clients (never part of `gate`); unit text is validated by golden files plus optional `plutil -lint` / `systemd-analyze verify`; nothing in the test suite touches `/var`, `/etc`, launchd or systemd. The owner does the one real install by hand using the printed script.

## 8. Risks and adversarial-review focus

- **R1 kernel without exochain.** `clawft-kernel --no-default-features --features native,mesh` may not compile, or `clawft-core/native` drags too much. Probe first in K0. Fallback: move the `mesh_*` modules into a new `clawft-mesh` crate (large, mechanical, ~12 files); decide before S starts.
- **K0/K1.** Behaviour drift in the extracted accept loop (cancel-safety comment at `boot.rs` mesh loop, ADR-010); admission bypass by a frame sent before `AdmitHello`; handshake-hash binding done on the wrong value; `source_node` spoof after admission; downgrade by dropping Noise (passthrough accepted under `enforce`). Review: replay a captured `AdmitHello` on a new session; send envelopes before the hello; connect with a revoked key; run old-build peers against `observe` and `enforce`.
- **J.** A journal whose verification passes while a middle record was edited and re-signed (needs the box key: assert that is the only way); tail truncation (anchors detect it, the service alone does not); fsync gaps; rebuilding `bindings` order-dependently. Review: edit, delete and reorder records at every position; kill at each syscall boundary.
- **S/L.** Identity confusion: uid reuse after account deletion (a new person gets the old uid and inherits the bind; mitigated by key possession, plus `bind.revoke` on account removal in docs), cross-uid address stealing, prefix hijack, a daemon claiming another user's `src_scope`, resource exhaustion by many registrations or slow-loris hello, verdict cache poisoning (cache key must include the peer's key, not just id), admin check by group membership resolved at connect time only. Review: two principals racing the same bind, a registration replay with a captured challenge (challenge is per-connection and single-use), a downgraded `proto` request.
- **U.** The daemon silently falling back to a self-generated node key (wrong identity on the mesh); anchors lost during outages; cert expiry with the service down (daemon must degrade, not exit); verdict handler becoming an unauthenticated governance oracle (only the registered connection can reach it); `user.key`/`chain.key` divergence.
- **H.** Unit hardening that breaks mDNS (the sandbox options above must be tested on a real Linux box; `RestrictAddressFamilies` may also need `AF_NETLINK`); macOS `/var/run` is recreated at boot and the LaunchDaemon cannot create `/var/run/weftos` as a non-root user (the plan needs either launchd `Sockets` socket activation with `SockPathName`/`SockPathMode`, or an install-time `ProgramArguments` pre-step; **unverified**, test by hand on this Mac before the plan commits to `/var/run/weftos` on macOS); binary path drift after `weaver update`; the printed script being run twice.
- **Cross-cutting.** The listener default `0.0.0.0:9489` (R9) becomes a standing LAN service on every install; macOS will prompt the firewall for `/usr/local/libexec/weftos/weaver`. Leaf devices stay unsigned under `observe`, so P3 improves nothing for them until the leaf track lands. Version skew across three tiers is only as good as the hello `build_sha` reporting; test each pairing.

## 9. Open owner decisions (with recommendations)

- **D-1 Bind policy (D3 left TOFU vs approval open).** Recommend `tofu` when the installer sees at most one human uid, `approve` otherwise (bind stays `pending`, `weaver mesh bind approve <uid>`); written into `mesh.toml`, journalled either way.
- **D-2 Admission default.** Recommend `observe` for the first release (journal would-be refusals, admit), flipping to `enforce` after the Pi and ESP32 peers are redeployed; leaves allowed as the flagged `leaf` class until the leaf track.
- **D-3 Who answers cluster verdicts.** Recommend a designated `cluster_owner_uid` in `mesh.toml` (default: first bound uid), fail-closed after `verdict_timeout`, with a 10 minute stale-grace for already-admitted peers and a static deny for new ones when the owner daemon is down.
- **D-4 Box key on first install.** Recommend adopting the existing `~/.weftos/run/node.key` on the owner's Mac (keeps the id remote peers pin) via `--adopt-node-key`, generating fresh on new machines.
- **D-5 User key.** Recommend one identity: `user.key` carries the same seed as the migrated `chain.key` (same `user_id`, no new signing key), `chain.key` kept for one release, then removed by the owner. Alternative (a distinct user key certifying a separate chain key) costs a chain re-key and buys nothing here.
- **D-6 Service binary.** Recommend `weaver mesh serve` from `/usr/local/libexec/weftos/weaver` now, guarded by the cargo-tree gate; a slim `weftos-meshd` binary is deferred until the exochain-free build (R1) is cheap.
- **D-7 Socket mode.** Recommend 0666 with peer-credential authorization (users self-register, admin by `admin_uids` and root) over a group-gated 0660 that forces admins to manage group membership.
- **D-8 Service account names.** Recommend `weftos` (Linux, sysusers.d) and `_weftos` (macOS, uid from the service range via `dscl`), no login shell, no home.
- **D-9 Certificate lifetime.** Recommend 24 h with renewal at 12 h and 5 min leeway; revocation by journal plus the serial list in signed facts.
- **D-10 Windows.** Recommend design only in P3 (section 6); implement when a Windows host is available to verify the pipe ACL and token details.

## 10. Exit criteria for Phase 3

On the owner's machine (installed by hand from the printed script): the service runs as an unprivileged account on 9489, `weaver mesh status` shows the adopted `node_id` unchanged, the journal verifies, the user daemon registers over the socket and is bound to the owner's uid (`how` recorded), `kernel.status` shows `mesh.mode = service` with the same `node_id`, a scoped `weft://` send between two registered user daemons (test accounts or the injected-identity e2e) is delivered and stamped, a second uid cannot take the first uid's address, `bind.rebind` revokes the old key, `observe` admission journals a deliberately bad peer, killing and restarting the service leaves chains and bindings intact and daemons reconnect, `kernel.mesh.service = off` returns to Phase 1 behaviour with no data change, `weaver update` prints (does not run) the service restart, and the no-owned-state gate, `scripts/build.sh test-mesh-service` and `scripts/build.sh gate` are green. No file under `~/.clawft`, `~/.weftos/chain` or any project runtime dir changed, and nothing in the suite touched `/var`, `/etc`, launchd or systemd.
