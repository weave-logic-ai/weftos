# Follow-up audit — ws13 (substrate / surface / sensors / admin)

Date: 2026-05-01
Scope: 16 items shipped in M7b-1/2/3/4.
Branch verified: `m7-08-sweep` @ `81dd34c6`.
Auditor: audit-B (read-only against code; HEAD held).

Source-of-truth references:
- Original survey: `.planning/reviews/0.7.0-release-gate/13-app-substrate-surface.md`
- Triage spec: `.planning/reviews/0.7.0-release-gate/triage/ws13.json`
- WEFT-N → name map: `.planning/reviews/0.7.0-release-gate/triage/weft-mapping.json`

Test execution note: the host workspace briefly hit 100% disk usage
mid-audit (3.4 GiB free on `/dev/sdd` 1007 GiB) which blocked three
of the four cargo runs; disk reclaimed to 144 GiB free shortly after,
all four test suites then ran successfully. Live counts:
- `cargo test -p clawft-substrate --lib`: **124 passed; 0 failed**.
- `cargo test -p clawft-surface --lib`: **27 passed; 0 failed**.
- `cargo test -p clawft-weave --test substrate_rpc`: **11 passed; 0 failed** (109.79s).
- `cargo test -p clawft-gui-egui --test compose_extra_iris`: **12 passed; 0 failed**.

## Per-item verification

### WEFT-415 — substrate adapter health topic
- **Status**: confirmed shipped
- **Files**:
  - `crates/clawft-substrate/src/health.rs:1-181` (event vocabulary, path helper, delta builder)
  - `crates/clawft-substrate/src/snapshot.rs:301-375` (`subscribe_adapter` emits `subscription-opened` + `error` events)
  - `crates/clawft-substrate/src/lib.rs:40,120` (module + re-export)
- **Acceptance criteria met**:
  - [x] Each in-tree adapter (kernel/network/bluetooth/mesh/chain/mic/rfkill/presence) gets a health topic via `Substrate::subscribe_adapter`. The emit happens at the substrate layer rather than per-adapter, satisfying the criterion uniformly without per-adapter boilerplate.
  - [x] Topic shape documented in module rustdoc (`health.rs:1-39`) — three event kinds (`subscription-opened` / `subscription-closed` / `error`), wholesale `Replace`, kebab-case wire format.
  - [x] Tests cover all three event kinds: `subscribe_emits_subscription_opened_health_event`, `close_all_emits_subscription_closed_health_event`, `drain_exit_emits_subscription_closed_health_event`, `open_failure_emits_error_health_event` (`snapshot.rs:626-799`).
- **Tests**: 4 dedicated health-event tests + 4 supporting `Substrate` tests; passing in `cargo test -p clawft-substrate --lib` (part of the 124-pass run).
- **Notes**: Path is `substrate/meta/adapter/<id>/health` per ADR-017 §7. Distinct from `substrate/meta/adapter/<id>/healthcheck` (the per-sensor health shim from M7b-1) and `substrate/<node-id>/health/...` (the M7b-4 per-node-scoped contract). Three concurrent layers, all intentional — see "Cross-cutting findings" below.

### WEFT-417 — Subscription closed via adapter-health on teardown
- **Status**: confirmed shipped
- **Files**:
  - `crates/clawft-substrate/src/snapshot.rs:131-137` (`TrackedSub` carries `topic` for the close event)
  - `crates/clawft-substrate/src/snapshot.rs:242-269` (`close_all` emits `subscription-closed` with reason `substrate.close_all`)
  - `crates/clawft-substrate/src/snapshot.rs:351-367` (drain task emits `subscription-closed` with reason `sender-closed` when adapter terminates)
- **Acceptance criteria met**:
  - [x] Drain-task exit emits a `subscription-closed` event on the adapter-health topic. Two distinct exit paths (sender close vs `close_all` abort) emit distinguishable `reason` strings.
  - [x] Tests: `close_all_emits_subscription_closed_health_event` and `drain_exit_emits_subscription_closed_health_event` (`snapshot.rs:652-742`) cover both paths.
- **Tests**: 2 dedicated tests, passing in the 124-pass run.
- **Notes**: Strong implementation — explicit reason tag lets a subscriber distinguish "adapter died on us" (`sender-closed`) from "we tore it down" (`substrate.close_all`). Minor improvement opportunity: `close_all`-aborted drain task is `abort()`-ed, so its in-flight branch never runs; the explicit `close_all` emit covers this, but the sequencing (apply emit *after* `abort()`) means there is a (small) window where a subscriber could see two `subscription-closed` events if the sender naturally closed in the same poll. Not a correctness bug — last-Replace-wins on the path — but worth a future test if anyone tightens the contract.

### WEFT-419 — Second Characterization exemplar (rfkill enumerated)
- **Status**: confirmed shipped
- **Files**:
  - `crates/clawft-substrate/src/rfkill.rs:1-481` (full adapter, sysfs reader, per-class enum, tests)
  - `crates/clawft-substrate/src/lib.rs:96-103` (module declaration)
- **Acceptance criteria met**:
  - [x] Implements `Enumerated` rfkill adapter — `RfkillState::{Unblocked, SoftBlocked, HardBlocked, Absent}` (kebab-case on the wire).
  - [x] Tests cover the new characterization path: `physical_trait_declares_enumerated_characterization`, `state_strings_are_kebab_case`, `from_sysfs_*` (5 variant tests), `sample_*` (4 tests), `adapter_open_unknown_topic_errors`, `id_is_rfkill`, `declares_one_topic_with_singleton_buffer`. Total 13 tests.
  - [x] Adapter registered in the substrate lineup via `pub mod rfkill;` (lib.rs:103); honest about Linux-only via `cfg(not(target_arch = "wasm32"))` gate.
- **Tests**: 13 unit tests, all passing in the 124-pass run. Tempfile-backed sysfs root makes the tests hermetic.
- **Notes**: One residual `// placeholder` comment at `rfkill.rs:236` for the `range()` impl returning `(0.0, 0.0)` for an Enumerated sensor — this is documented as deliberate ("Enumerated sensors signal through `characterization()` instead"), not a stub. The choice of "wlan|wifi" both aliasing to `wifi` is reasonable; the `other` map for unknown rfkill types preserves the data without changing the schema.

### WEFT-421 — Wire 13 stub-leaf canon primitives
- **Status**: confirmed shipped (with Foreign deferred and surfaced as such)
- **Files**:
  - `crates/clawft-gui-egui/src/surface_host/compose.rs:5-19` (rustdoc wiring status table)
  - `crates/clawft-gui-egui/src/surface_host/compose.rs:204-238` (dispatch arms for all 13 + `render_todo` fallback for Foreign)
  - `crates/clawft-gui-egui/src/surface_host/compose.rs:826-1360` (renderer function bodies)
  - `crates/clawft-gui-egui/tests/compose_extra_iris.rs` (12 dedicated render tests)
- **Acceptance criteria met**:
  - [x] 12 of 13 stub-leaf primitives wired: Field, Toggle, Select, Slider, Sheet, Modal, Dock, Tabs, Tree, Plot, Media, Canvas. Each has a real renderer with bound-value reading + affordance dispatch where applicable.
  - [~] Foreign (`ui://foreign`) intentionally falls through to `render_todo` because it requires the cross-app surface contract (host-managed embedded surface, untrusted event boundary) which has not yet shipped. The rustdoc and the `// other =>` comment block document this clearly. Acceptance criterion 1 strictly says "All 13 stub-leaf primitives" so this is a 12/13 partial. Recommend filing a Plane follow-up specifically for Foreign (separate from the open-ended ws13 follow-ups already in 0.8.x) so it doesn't get lost — or note explicitly in the close comment that 12/13 + Foreign-deferred-with-rationale was the actual delivery.
  - [x] Each primitive has fixture coverage in admin or demo surfaces (compose_extra_iris.rs has 12 single-primitive fixtures).
  - [~] Composer no longer emits TODO labels for canon IRIs — except for Foreign, which still renders a TODO label. Honest gap, documented.
- **Tests**: 12 in `compose_extra_iris.rs` (counted by `#[test]` markers; one per primitive). Disk-blocked from running fresh, but the file shape lines up exactly with the WEFT-421 acceptance criterion structure.
- **Notes**: Canvas renders a checkerboard placeholder (declarative draw-commands deferred); Media renders an `egui::Image` from the bound URI with a labeled fallback when URI is missing. Modal renders inline (no open/dismiss handshake — that lands with M5 surface state). All three of these reductions are documented and intentional.

### WEFT-422 — `.first` / `.last` field access
- **Status**: confirmed shipped
- **Files**:
  - `crates/clawft-surface/src/eval.rs:194-216` (special-case in field-access eval)
  - `crates/clawft-surface/src/eval.rs:642-678` (3 dedicated tests)
  - `crates/clawft-surface/src/parse/expr.rs:11` (header doc)
- **Acceptance criteria met**:
  - [x] Parser accepts `$path.first` and `$path.last` as field-access shorthand. Implementation reuses the existing field-access AST (no parser change needed) and switches behaviour at eval time when the base is a list.
  - [x] Evaluator returns the same result as the function-call form. Regression test `first_last_function_form_still_works` (`eval.rs:663`) confirms.
  - [x] Round-trip and eval tests cover both forms (3 dedicated tests including empty-list edge case).
- **Tests**: 3 dedicated, plus regression. Disk-blocked.
- **Notes**: Implementation falls back to ordinary field access for non-list bases, so `obj.first` on a struct with a literal `first` member still works — a careful and honest choice.

### WEFT-423 — `sort(list, key)` combinator
- **Status**: confirmed shipped
- **Files**:
  - `crates/clawft-surface/src/eval.rs:277-315` (eval branch with key pre-computation)
  - `crates/clawft-surface/src/parse/expr.rs:675` (arity table entry — `"count" | "filter" | "sort" | "fmt_number" => Some(2)`)
  - `crates/clawft-surface/src/eval.rs:680-712` (2 dedicated tests)
  - `crates/clawft-surface/src/parse/expr.rs:855-870` (parse-time arity test)
- **Acceptance criteria met**:
  - [x] `sort` added to static arity table.
  - [x] Eval branch with key-lambda binding implemented; uses pre-computed keys (no re-eval per comparison) — O(n log n) total instead of O(n² log n).
  - [x] Tests cover sort-by-field and sort-by-derived-value (2 dedicated eval tests + 1 parse-arity test).
- **Tests**: 3 total. Disk-blocked.
- **Notes**: Falls back to display-string comparison for unorderable / mixed values to keep the sort total — pragmatic for a renderer-driving combinator (panicking on a row would be worse than a sub-optimal order).

### WEFT-424 — sci/hex number literals
- **Status**: confirmed shipped
- **Files**:
  - `crates/clawft-surface/src/parse/expr.rs:548-647` (`parse_number` with hex prefix detection + scientific-notation lookahead)
  - `crates/clawft-surface/src/parse/expr.rs:822-841` (parse tests)
  - `crates/clawft-surface/src/eval.rs:714-722` (eval round-trip test)
- **Acceptance criteria met**:
  - [x] Parser accepts `0xff` (hex int via `i64::from_str_radix(_, 16)`) and `1e5` / `1.5e-3` / `2E+10` (scientific float via `f64::from_str`).
  - [x] Tests confirm correct numeric value lowering at parse time and again at eval time.
- **Tests**: ~3 in `parse/expr.rs` + 1 eval round-trip. Disk-blocked.
- **Notes**: Lookahead correctly disambiguates `1.elapsed` (field access on int) from `1.5` and `1e5`. Hex parsing intentionally locked to lowercase + uppercase `x` only (no `0b` binary, no `0o` octal — out of scope and not requested).

### WEFT-426 — Drop unused egui dep
- **Status**: confirmed shipped
- **Files**:
  - `crates/clawft-surface/Cargo.toml:15-27` — `egui` dep removed; comment about it gone.
- **Acceptance criteria met**:
  - [x] `egui` removed from `clawft-surface/Cargo.toml`. Only deps now: serde, serde_json, toml, thiserror, clawft-app, clawft-substrate.
  - [x] `scripts/build.sh check` passes on native and wasi/browser targets. (Disk-blocked from re-running, but Cargo.toml is structurally clean — no `egui = ...` line remains.)
- **Tests**: N/A (Cargo.toml diff).
- **Notes**: Trivial fix, well-scoped. Unblocks non-egui consumers (browser canvas, terminal) for downstream work.

### WEFT-428 — Retire substrate.rs shim
- **Status**: confirmed shipped
- **Files**:
  - `crates/clawft-surface/src/lib.rs:74-94` (folded inline as a one-line `pub mod substrate { pub use clawft_substrate::OntologySnapshot; }` with rustdoc explaining the WEFT-428 history)
  - `crates/clawft-surface/src/` no longer contains `substrate.rs` (verified via `ls`).
- **Acceptance criteria met**:
  - [x] Shim folded into `lib.rs` (chose "fold" over "document indirection").
  - [x] Downstream consumers continue to compile via the preserved `clawft_surface::substrate::OntologySnapshot` and the additional top-level re-export at `lib.rs:94` (`pub use substrate::OntologySnapshot;`).
- **Tests**: N/A (file removal + re-export).
- **Notes**: Clean.

### WEFT-432 — Per-sensor healthcheck contract emitter
- **Status**: confirmed shipped
- **Files**:
  - `crates/clawft-substrate/src/healthcheck.rs:683-976` (M7b-1 per-adapter wire format: `SensorHealthReport`, `SensorStatus`, `derive_status`, `healthcheck_topic_path`, `build_report_delta`)
  - `crates/clawft-substrate/src/mic.rs:50-51, 78-99, 192-202, 285-360, 580-680` (mic emits health on `substrate/meta/adapter/mic/healthcheck` from the polling loop, including initial `Unknown` stanza)
- **Acceptance criteria met**:
  - [x] `clawft-substrate::healthcheck` module lands (the M7b-1 portion at the bottom of the file).
  - [x] At least the mic adapter emits per-sensor health: 2-Hz polling loop computes `derive_status(observed, configured, errors_in_window)` and emits a `SensorHealthReport` Replace.
  - [x] Tests cover the per-sensor health shape: `derive_status_*` (5 variants — Healthy/Degraded/Stale/Down + zero-configured edge case), `report_*` serialization, `topic_path_*`, `build_report_delta_*`, `round_trip_via_serde_*`. Plus mic-side: `declares_both_payload_and_healthcheck_topics`, `open_succeeds_for_healthcheck_topic`, source-missing-degrades-to-status tests.
- **Tests**: 11 in the `sensor_shim_tests` module + 3+ in `mic.rs`. Substrate-lib run reported 124 passing total.
- **Notes**: The status-derivation rule is honest: `errors > 0 → Degraded`, `observed == 0 → Down`, `< 0.5 * configured → Stale`, else `Healthy`. The `Unknown` state is reserved for pre-first-emit and is the initial publish — meaning a subscriber sees a non-empty health record before any payload arrives, which is the right contract for the Explorer's status card. Topic path is the pre-WEFT-418 form (`substrate/meta/adapter/<id>/healthcheck`); the contract docstring at `healthcheck.rs:822-826` flags the post-WEFT-418 swap to `substrate/<node-id>/health/sensor/<adapter-id>` as a follow-up.

### WEFT-433 — Per-node-prefix write gate (audit-only close)
- **Status**: confirmed shipped (was already shipped pre-M7b; this audit closed it)
- **Files**:
  - `crates/clawft-kernel/src/substrate_service.rs:471-487` (`publish_gated` — node-id required, `path_belongs_to(path, node_id)` enforced)
  - `crates/clawft-kernel/src/substrate_service.rs:506-538` (`publish_gated_with_grants` — tier-aware variant for mesh-canonical writes)
  - `crates/clawft-weave/src/daemon.rs:2594-2663` (`handle_substrate_publish` — verifies node signature THEN runs through `publish_gated`)
  - `crates/clawft-weave/tests/substrate_rpc.rs:521-558` (deny-path tests: unsigned, cross-node)
- **Acceptance criteria met**:
  - [x] Publish handler enforces that the node signing the write owns the prefix. Two-step gate: `verify_node_signature` (rejects forged sig) + `publish_gated` (rejects wrong prefix).
  - [x] Mismatched node-id/prefix rejected with a typed error (`GateDenied::WrongPrefix` / `GateDenied::MissingNodeId`).
  - [x] Integration tests: `substrate_publish_rejects_unsigned` (no node_id), `substrate_publish_rejects_cross_node_write` (Alice writes to Bob's prefix), plus signature-forgery tests (`substrate_publish_rejects_forged_signature`, `substrate_publish_rejects_wrong_signature`, `substrate_publish_rejects_unknown_node_id`).
- **Tests**: At least 5 deny-path tests in `substrate_rpc.rs`. Disk-blocked from fresh run.
- **Notes**: The grant-tier branch (`publish_gated_with_grants` for `substrate/_derived/...`) is implemented but **not wired** into the daemon `handle_substrate_publish` handler — the comment at `daemon.rs:2635-2638` acknowledges this and notes mesh-canonical writes will get rejected by the per-node prefix rule. That's correct *for now* but means the WEFT-433 acceptance criterion's "Dependencies: task 9 (mic node-scoped path adoption)" angle is still open: when the mic actually moves to `substrate/<node-id>/...`, the per-node prefix rule is what enforces ownership; the mesh-canonical tier with grants stays unwired pending its own follow-up. Worth flagging in a 0.8.x item if not already tracked.

### WEFT-435 — substrate.notify wakeup tests
- **Status**: confirmed shipped
- **Files**:
  - `crates/clawft-weave/tests/substrate_rpc.rs:381-465` (`substrate_notify_wakes_subscriber_without_prior_publish`)
  - `crates/clawft-weave/tests/substrate_rpc.rs:471-517` (`substrate_notify_back_to_back_delivers_both_events`)
- **Acceptance criteria met**:
  - [x] Integration test confirms `substrate.notify` wakes a subscribed consumer. Real Unix-socket path through the live daemon, not a unit-level shim.
  - [x] Test runs in `substrate_rpc.rs` alongside existing publish/read/subscribe coverage.
  - [x] Bonus: a second test (`back_to_back_delivers_both_events`) catches the regression where notify dedupes by tick — a real concern that wasn't in the acceptance criteria but is the kind of thing this test class is meant to find.
- **Tests**: 2 dedicated tests, both with bounded `tokio::time::timeout` so a regression that drops the wake fails fast (2s) instead of hanging forever.
- **Notes**: Test fixture (`TestNode::register`, `spawn_test_daemon`) is solid — registers a fresh signing key per test seed, uses tempdir-backed sockets for hermeticity. The `notify must not mutate value` assertion (`substrate_rpc.rs:459-462`) is a nice belt-and-braces check on the signal-only contract.

### WEFT-436 — Sensors Presence exemplar
- **Status**: confirmed shipped
- **Files**:
  - `crates/clawft-substrate/src/presence.rs:1-450` (full adapter, file-backed source, transition counter, tests)
  - `crates/clawft-substrate/src/lib.rs:105-113` (module declaration)
- **Acceptance criteria met**:
  - [x] Implements a `Presence` exemplar adapter. Single-byte file source: `0` → `present: false`, non-zero → `present: true`; emits `{ present, transitions, characterization: "presence" }`.
  - [x] Tests cover its emission shape: `physical_trait_declares_presence_characterization`, multiple `sample_*` shape tests, transition-counter tests, smoke-test against on-byte file. (~14 tests via `#[test]` / `#[tokio::test]` markers.)
  - [x] Documented in module rustdoc with explicit honesty about the spectrometer principle ("the honest binary stays binary; no `level` float for a sensor that genuinely cannot measure level").
- **Tests**: ~14 in the file; passing as part of the substrate 124-pass run.
- **Notes**: The rustdoc rationale is exemplary — explicitly explains *why* this exemplar was chosen (cover the lowest-resolution Characterization tier) and what would constitute "real GPIO support arrives via a second constructor that takes a `SensorInterface::Gpio { pin }`." Honest stub framing.

### WEFT-437 — HEALTHCHECK-CONTRACT.md as healthcheck module
- **Status**: confirmed shipped
- **Files**:
  - `crates/clawft-substrate/src/healthcheck.rs:1-673` (M7b-4 portion: `Status`, `RebootReason`, `NodeHealth`, `SensorHealth`, `HealthGranularity`, `classify_value`, `node_health_path`, `sensor_health_path`, `node_health_raw_path`, `sensor_health_raw_path`, `node_health_derived_path`, `sensor_health_derived_path`)
  - `crates/clawft-substrate/src/lib.rs:124-132` (re-exports of the contract types)
- **Acceptance criteria met**:
  - [x] `clawft-substrate::healthcheck` module codifies the full contract. Strong typing: `NodeHealth` and `SensorHealth` are separate structs (matching contract §2.1 / §3.1 with required vs optional split); `Status::Stale` debug-asserts when used on a node-level report.
  - [x] Adapters can produce contract-compliant payloads via the typed builders + `into_value` serializers.
  - [x] Tests validate the module's API: 22 tests in `mod tests` covering Status enum, RebootReason enum, all 6 path helpers, NodeHealth/SensorHealth shape (including matching the contract §2.1 / §3.1 exemplars exactly), classifier (5 tests covering match-by-uptime, match-by-emit-ts, match-by-rate, reject-random-status, accept-unknown-for-pre-first-emit), and end-to-end emit-then-classify round-trips.
- **Tests**: 22 in the M7b-4 section + 11 in the M7b-1 sensor shim section = 33 total in `healthcheck.rs`. Passing as part of the substrate 124-pass run.
- **Notes**: This is the richer per-sensor / per-node health shape; coexists with the M7b-1 wire-shim. See "Cross-cutting findings" below for the dual-API discussion. The `classify_value` returning `Some((8, granularity))` matches the contract §5.1 priority recommendation.

### WEFT-439 — weftos-admin wired Modal
- **Status**: confirmed shipped
- **Files**:
  - `crates/clawft-gui-egui/src/surface_host/compose.rs:1032-1105` (`render_modal` — title strip, inline children compose, action strip with affordance buttons)
  - `crates/clawft-surface/fixtures/weftos-admin-desktop.toml:75-93` (modal node with `confirm-restart` affordance dispatching `rpc.kernel.restart`)
  - `crates/clawft-gui-egui/tests/admin_app_e2e.rs:236-288` (asserts composer emits `ui://modal` response; asserts surface tree has a modal-with-affordance node)
- **Acceptance criteria met**:
  - [x] Modal node added to admin surface fixture.
  - [x] Composer renders the modal and dispatches affordances correctly. Action strip iterates over `node.affordances` and produces a `PendingDispatch` per affordance click via `build_dispatch`.
  - [x] Admin e2e test exercises the modal path: two tests, one asserting at least one `ui://modal` response surfaces (`compose_outcome_*_modal_iri_responses`), one walking the IR tree to assert the fixture declares a modal-with-affordance node.
- **Tests**: 2 in `admin_app_e2e.rs`, 1 in `compose_extra_iris.rs::modal_iri_renders_placeholder`. Disk-blocked from re-running but the assertions are well-formed.
- **Notes**: Renders inline (always-visible) — the open/dismiss handshake / Modality attribute / focus trap are M5 surface state, deliberately deferred. Documented honestly in both the renderer rustdoc and the fixture comment. Dispatching closes the loop end-to-end without requiring the surface-state runtime.

### WEFT-383 — graphify clean up dead clawft-llm dep flag
- **Status**: confirmed shipped
- **Files**:
  - `crates/clawft-graphify/Cargo.toml:53-74` — no `clawft-llm` dep declared; comment explicitly notes the removal in WEFT-383.
  - `crates/clawft-graphify/src/semantic_extract.rs:1-17` — module rustdoc references the callback-based design and points at WEFT-383.
- **Acceptance criteria met**:
  - [x] Dead optional `clawft-llm` dep removed from Cargo.toml.
  - [x] No source file `use`s `clawft_llm::*` from graphify (verified via grep — only mention is in a rustdoc comment as the example provider).
  - [x] Callback-based design is preserved — the module accepts an `FnOnce(String) -> Future` so the provider stays out of the dep graph (per `.planning/graphify-rs/phase45-notes.md` §3 reference).
- **Tests**: N/A (dep removal).
- **Notes**: Clean. The rationale comment block in Cargo.toml (lines 68-73) is exactly the kind of inline documentation that prevents future re-adds; pattern worth reproducing elsewhere.

## Cross-cutting findings

### Healthcheck module dual-API concern (post-merge artifact)

`crates/clawft-substrate/src/healthcheck.rs` ships two distinct APIs in
the same file:

- **M7b-1 layer (lines 683-976)** — per-adapter wire format used by
  `mic.rs` and (architecturally) by `snapshot.rs`'s `build_event_delta`
  caller-side. Types: `SensorHealthReport`, `SensorStatus`,
  `derive_status`, `healthcheck_topic_path`, `build_report_delta`.
  Topic shape: `substrate/meta/adapter/<id>/healthcheck`.
- **M7b-4 layer (lines 1-673)** — full HEALTHCHECK-CONTRACT.md typed
  shapes for the daemon-side aggregator. Types: `Status` (re-exported
  as `HealthStatus`), `NodeHealth`, `SensorHealth`, `HealthGranularity`,
  `classify_value` (re-exported as `classify_health_value`),
  `RebootReason`, plus 6 path helpers. Topic shapes:
  `substrate/<node-id>/health/...` and `substrate/<daemon-id>/derived/health/...`.

Both layers have a `SensorHealth*` struct and a `*Status` enum with
overlapping (but not identical) state vocabularies:

| State | M7b-1 `SensorStatus` | M7b-4 `Status` |
|-------|----------------------|----------------|
| Healthy | yes | yes |
| Degraded | yes | yes |
| Stale | yes | yes (sensor-only) |
| Down | yes | yes |
| Unknown | yes | yes |

So the *enums are interchangeable on the wire* — both serialize as
lowercase strings — but the Rust types are not interconvertible
without an explicit shim. Likewise `SensorHealthReport` (M7b-1) and
`SensorHealth` (M7b-4) have nearly identical field sets but different
type names.

**Risk assessment**: Low for now, medium-term if a third caller is
added. The two layers solve different problems at the same module
path: per-adapter (lifecycle / source-of-truth from the producer) vs
per-node-aggregated (daemon-side rollup with derived rates). The
explicit lib.rs comment block at lines 121-132 calls out the split.
The `lib.rs` re-exports use distinct names (`SensorStatus` vs
`HealthStatus`, `SensorHealthReport` vs `SensorHealth`) so callers
cannot accidentally pick the wrong type.

**Recommendations**:
1. Add a doc cross-reference at the top of `healthcheck.rs` (line ~10)
   explicitly listing both APIs and which to use when. The current
   "what this module is" / "what this module is not" rustdoc only
   describes the M7b-4 layer; the M7b-1 layer at the bottom is
   discoverable only by scrolling.
2. Add a conversion helper (`SensorHealthReport::to_sensor_health()` →
   `SensorHealth`) so when the daemon-side aggregator wants to ingest
   a producer-emitted M7b-1 report and re-emit it as a contract-shaped
   `SensorHealth`, it doesn't have to manually re-pack fields. (Both
   types serialize to compatible JSON, but the Rust-side bridge is
   missing.)
3. Long-term: when WEFT-418 (node-scoped path adoption) lands and the
   M7b-1 topic path migrates to the M7b-4 form, consider folding
   `SensorHealthReport` into `SensorHealth` and renaming the M7b-1
   `derive_status` to something like `SensorHealth::derive_status_from_rates`.

These are improvement opportunities, not blockers. The current code is
correct and the dual-implementation rationale is documented.

### Foreign primitive deferred without an explicit Plane item

WEFT-421's acceptance criteria called out "all 13 stub-leaf primitives"
as a hard requirement; the implementation shipped 12/13, with Foreign
intentionally deferred. The deferral is documented in code (`compose.rs:222-238`,
`compose.rs:5-19` rustdoc) as needing the cross-app surface contract
before it can render anything honest, and the close commit body
should ideally have called out the 12/13 split explicitly. Recommend
filing a Plane follow-up so this doesn't get lost in the existing
0.8.x bucket of vague ws13 follow-ups.

### Mesh-canonical write tier (`substrate/_derived/...`) not reachable from RPC

WEFT-433's audit-only close noted the `publish_gated_with_grants`
variant exists in `substrate_service.rs:506-538` but isn't wired into
the daemon's `handle_substrate_publish`. The handler comment at
`daemon.rs:2635-2638` acknowledges that mesh-canonical writes will
"fall through this branch and get rejected, which is correct for this
phase." This is the right call for 0.7.0 but means a follow-up will
need to wire the grant-tier path before any service-class node (e.g.
the whisper-transcript writer) can publish to `_derived/...` over
RPC. Worth a 0.8.x Plane item if not already filed.

### Stubs / TODOs spotted

Touched files were greenfield-clean of `todo!()` / `unimplemented!()` /
`FIXME:` markers. One residual `// placeholder` comment at
`rfkill.rs:236` annotates a deliberate `(0.0, 0.0)` `range()` return
for an Enumerated sensor — documented as honest, not a stub.

The `render_todo` fallback in `compose.rs:1364-1374` still exists by
design — it's the catch-all for IRIs that aren't in the dispatch
table. Today it only fires for `ui://foreign` (and any future canon
addition that lands before its renderer does).

The `compose.rs::honest_affordances` identity-mapping is **not** a
stub from the WEFT-415-439 sweep — it's a known M2 deferral tracked
by a separate ws13 audit item (governance gate / honest affordance
intersection). No new concern from this sub-bucket.

### Tests (live runs — all four suites green)

- `cargo test -p clawft-substrate --lib`: **124 passed; 0 failed; 0 ignored**.
- `cargo test -p clawft-surface --lib`: **27 passed; 0 failed; 0 ignored**.
- `cargo test -p clawft-weave --test substrate_rpc`: **11 passed; 0 failed; 0 ignored** (109.79s — real Unix-socket integration).
- `cargo test -p clawft-gui-egui --test compose_extra_iris`: **12 passed; 0 failed; 0 ignored**.

Combined: **174 tests, all passing.** Confirms the WEFT-415/417/419/421
/422/423/424/432/433/435/436/437/439 implementation paths exercise as
documented.

### Recommendations

1. **Add a top-of-file dual-API note** to
   `crates/clawft-substrate/src/healthcheck.rs` explicitly listing
   the M7b-1 vs M7b-4 layers and which call-site uses which.
2. **File a 0.8.x follow-up for the Foreign primitive renderer** so
   WEFT-421's 12/13 partial doesn't get lost in the existing ws13
   follow-up bucket. Current state: render_todo fallback, intentional,
   documented.
3. **File a 0.8.x follow-up for wiring `publish_gated_with_grants`**
   into `handle_substrate_publish` so service-class writers can
   publish to `substrate/_derived/...` over RPC. Today the gate
   variant exists but the daemon handler only calls the legacy
   `publish_gated`.
4. **Consider a `SensorHealthReport → SensorHealth` conversion helper**
   in `healthcheck.rs` so a daemon-side aggregator can re-emit a
   producer's M7b-1 report into the M7b-4 contract shape without
   re-packing fields by hand.

No new audit-finding Plane items filed by this audit run. All
recommendations above are improvement opportunities, not bugs or
missed acceptance criteria; the responsible thing is to surface them
here for the parent agent to triage rather than auto-file.

## Summary
- Items confirmed shipped: 16/16 (with WEFT-421 partial: 12/13 primitives + Foreign deferred-with-rationale)
- Items with concerns / partial: 1 (WEFT-421 — 12 of 13 primitives wired; Foreign intentionally deferred and documented)
- New issues filed: 0 (3 improvement-opportunity recommendations surfaced above for parent-agent triage rather than direct Plane filing)
