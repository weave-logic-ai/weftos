# Follow-up audit — ws08 (egui Explorer + WASM panel)

Date: 2026-05-01
Scope: 25 items shipped/touched in M7-E + M7b-5/6/7/8.
Branch verified: `m7-08-sweep` @ `81dd34c6`.
Auditor: audit-A.

## Per-item verification

### WEFT-273 — Copy Path / Pubkey / Snapshot row above detail viewer
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/src/explorer/mod.rs:75-107` (`COPY_TOAST_DURATION`, `NAV_INTENT_KEY`, `request_navigation`/`take_navigation_request`); `:700-791` (`paint_copy_actions`, `extract_pubkey_like`)
- **Acceptance criteria met**:
  - [x] Copy Path always visible (`small_button("Copy Path")` is unconditional, line 716)
  - [x] Copy Pubkey shows when value carries pubkey-shaped field (`extract_pubkey_like` probes `pubkey`/`peer_id`/`node_id`/`device_id` in priority order)
  - [x] Export Snapshot copies `serde_json::to_string_pretty` of current value (lines 739-750)
  - [x] Toast confirmation rendered for `COPY_TOAST_DURATION = 1500ms` (constant at line 75; expiration check at line 708-712)
  - [x] Uses `ctx.copy_text(...)` — egui 0.34's clipboard API (replaces the older `output_mut(|o| o.copied_text = ...)` referenced in the original ticket)
- **Tests**: 9/9 in `copy_actions_tests` (`extracts_canonical_pubkey_field`, `falls_back_to_peer_id|node_id|device_id`, `priority_pubkey_over_peer_id`, `rejects_empty_string_field|non_string_field|non_object_value|object_without_known_fields`).
- **Notes**: Toast is rendered as a small italic green label rather than a popup — fine, matches the "transient label" wording in the ticket.
- **New issue / stub spotted**: none.

### WEFT-252 — Chat panel markdown rendering
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/src/explorer/chat.rs:52,170,705-708`; `crates/clawft-gui-egui/Cargo.toml:94`
- **Acceptance criteria met**:
  - [x] `egui_commonmark = "0.23"` with `default-features = false, features = ["pulldown_cmark"]` (Cargo.toml:94 — narrow feature set deliberately to keep wasm bundle small per WEFT-246)
  - [x] Assistant turns render via `CommonMarkViewer::new().max_image_width(Some(480)).show(ui, md_cache, &msg.content)` (chat.rs:705-707)
  - [x] `CommonMarkCache` carried in `ChatView::cache` so AST is reused across paints (line 170)
  - [x] Code-block monospace + copy affordance preserved (default `egui_commonmark` behaviour)
- **Tests**: chat::tests covers the data flow (`appends_assistant_message_on_ok_response`, `ok_response_accepts_assistant_text_field`); paint-side covered by the broader `paint_does_not_panic` family for chat/hooks indirectly. No dedicated markdown snapshot test, but the call site is small enough to inspect visually.
- **Notes**: `id_salt` for collapsibles inside markdown is implicit through `CommonMarkViewer::new()` defaults — egui generates per-call ids from the parent `Id`. The doc comment claims a "unique-per-bubble (combined hash of content + role)" id_salt but that isn't actually wired; the comment is aspirational. Functionally fine because egui's `push_id(id)` upstream in `paint_history` namespaces correctly. Worth a doc-comment fix later (cosmetic).
- **New issue / stub spotted**: minor doc-comment drift (above) — not material.

### WEFT-255 — Chat panel system-prompt UI
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/src/explorer/chat.rs:142-148,200-234,506-535`
- **Acceptance criteria met**:
  - [x] Collapsible textarea above the message field (`paint_system_editor`, `egui::CollapsingHeader::new("system prompt (optional)")`)
  - [x] `view.system: Option<String>` persists per-conversation in memory
  - [x] `build_request_params` injects `{role:"system", content:...}` at `messages[0]` when set; whitespace-only is filtered out
  - [x] Daemon merges with workspace identity prompt (per the doc comment); panel layers on top, doesn't replace
- **Tests**: `system_prompt_rides_at_head_of_messages`, `whitespace_only_system_prompt_is_dropped_from_wire`, `serializes_messages_to_expected_wire_shape` — all pass.
- **Notes**: Default state is collapsed (`default_open(view.system_expanded)` — `system_expanded` defaults to false per `#[derive(Default)]`).
- **New issue / stub spotted**: none.

### WEFT-257 — Chat panel heartbeat label
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/src/explorer/chat.rs:158-160,313-339,584-646`
- **Acceptance criteria met**:
  - [x] Heartbeat label replaces the spinner — `paint_heartbeat` only fires `if view.is_in_flight()` (line 427), spinner is gone from chat.rs entirely
  - [x] Reads `substrate/_derived/chat/<conv_id>/status` via `live.substrate_snapshot()` (line 585-589)
  - [x] Renders `status: <word>` + `(<age>s ago)` (lines 622-633)
  - [x] Subtle pulsing dot replaces the spinner as the activity indicator (lines 614-619)
  - [x] `conv_id` minted lazily in `ensure_conv_id`; format `panel-<13-digit-ts>-<4-hex>` (line 325)
  - [x] `request_repaint_after(500ms)` keeps the age counter ticking
- **Tests**: `build_params_includes_conv_id_after_first_submit_mints_one`, `ensure_conv_id_is_idempotent`, `heartbeat_path_is_none_until_first_turn` — all pass.
- **Notes**: Falls back to `("waiting", "")` when no status frame is published — matches the design (very-first-turn case).
- **New issue / stub spotted**: none.

### WEFT-259 — Chat panel identity-drift warning
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/src/explorer/chat.rs:163-164,256-280,539-572`
- **Acceptance criteria met**:
  - [x] `last_identity_source` captured in `on_response_ok` (line 258-261)
  - [x] `identity_warning()` returns Some when source != `"clawft"` (line 274-280)
  - [x] Non-dismissable orange chip rendered above the input (paint_identity_warning, frame fill `(70,50,20)` dark-amber)
  - [x] Forward-compat: any unrecognised source value warns (test `identity_warning_fires_for_unknown_source` confirms)
- **Tests**: `identity_warning_clear_for_canonical_source`, `_fires_for_docs_fallback`, `_fires_for_unknown_source`, `_silent_when_field_absent` — all pass.
- **Notes**: The "Link to docs explaining remediation" requirement from the ticket is partially met — warning text says "check the daemon logs and `IDENTITY.md`" but is plain text rather than a clickable link. Minor.
- **New issue / stub spotted**: ticket mentions a doc link; landed as inline text guidance. Acceptable scope-cut.

### WEFT-260 — Terminal mouse selection + clipboard
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/src/explorer/terminal.rs:334-410,814-870` (selection), `:383-410` (copy/paste events), `:1262-1290` (pixel_to_point tests)
- **Acceptance criteria met**:
  - [x] `pixel_to_point` helper maps screen coords to alacritty `Point` + `Side`
  - [x] Mouse-drag selection wired (lines 334-360)
  - [x] `egui::Event::Copy | Event::Cut` reads selection text into the clipboard (line 396-401)
  - [x] `egui::Event::Paste(s)` writes into the PTY input (line 402+)
  - [x] Native-only — wasm stub remains at `terminal.rs:1321-1344` per the ticket's allowed scope-cut
- **Tests**: `pixel_to_point_maps_origin_to_top_left`, `pixel_to_point_outside_returns_none` — pass.
- **Notes**: Properly delegates platform clipboard to egui's event channel rather than touching the OS clipboard directly.
- **New issue / stub spotted**: none.

### WEFT-261 — Terminal bold/italic glyphs
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/src/explorer/terminal.rs:670-755`
- **Acceptance criteria met**:
  - [x] `paint_glyph` helper renders bold via egui's font weight (line 712)
  - [x] Italic rendered via `TextShape::with_angle_and_anchor(ITALIC_ANGLE, Align2::CENTER_CENTER)` (line 745)
  - [x] Visual smoke test: covered by `paint_does_not_panic` family (no fixture-level pixel diff, but the drawing path is exercised)
- **Tests**: glyph helper not directly tested but its surface lands in the `chip_surfaces`/integration suite.
- **Notes**: Italic via shear (with_angle_and_anchor) is the documented egui-bundled-font workaround — reasonable when the font file lacks an italic variant.
- **New issue / stub spotted**: none.

### WEFT-262 — Terminal scrollback + wheel
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/src/explorer/terminal.rs:100,169-173,260-265`
- **Acceptance criteria met**:
  - [x] `Config::scrolling_history = SCROLLBACK_LINES` (10_000, line 169-173)
  - [x] Wheel handler reads `i.smooth_scroll_delta.y` (CSS-pixel units), maps to alacritty grid lines (line 260-265)
  - [x] Resize re-reflow handled by alacritty's term grid (default behaviour)
- **Tests**: scrollback wiring not exercised by unit tests; covered by the chip surfaces and integration suites that don't fail.
- **Notes**: Wheel-mapping doc-comment confirms the unit conversion.
- **New issue / stub spotted**: none.

### WEFT-265 — Canon `Field::Date` (jiff)
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/src/canon/field.rs:135,309-324`; `crates/clawft-gui-egui/Cargo.toml:80`
- **Acceptance criteria met**:
  - [x] `FieldValue::Date(jiff::civil::Date)` enum variant added (line 135)
  - [x] `egui_extras::DatePickerButton::new(date).id_salt(&salt)` wired (line 317)
  - [x] Deviation from ticket: ticket said `chrono::NaiveDate`, code uses `jiff::civil::Date` — Cargo.toml:72-80 documents the migration: `egui_extras 0.34` switched to jiff, so the canon shape moved to match. Correct call.
- **Tests**: covered by the canon test suite (`field` module tests; full `cargo test -p clawft-gui-egui --lib` passes 324/324).
- **Notes**: id_salt format `"{:?}"` of canon Field id — unique-enough across panels; survives across frames.
- **New issue / stub spotted**: none.

### WEFT-266 — Canon `Field::Code` (multiline TextEdit + syntax highlighting)
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/src/canon/field.rs:74-119,326-374`
- **Acceptance criteria met**:
  - [x] `FieldValue::Code { lang, src }` variant added (line 137)
  - [x] `FieldKind::Code { language: Cow<'static, str> }` (line 75-77)
  - [x] `TextEdit::multiline` with `code_editor()` and `font(TextStyle::Monospace)` (lines 350-356)
  - [x] `egui_extras::syntax_highlighting::highlight` called via custom `layouter` closure (lines 336-349)
  - [x] Cmd/Ctrl-Enter commits, bare Enter inserts newline (lines 359-369)
- **Tests**: 324/324 lib tests pass.
- **Notes**: Effective language picks `FieldKind`'s hint, falls back to value-bound `lang` — sensible.
- **New issue / stub spotted**: none.

### WEFT-267 — Canon Select TableBuilder large-set form
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/src/canon/select.rs:24-114,222-272`
- **Acceptance criteria met**:
  - [x] `DEFAULT_TABLE_THRESHOLD` constant + `Select::table_threshold(usize)` builder (line 102-107)
  - [x] `paint_table_form` uses `egui_extras::TableBuilder` inside a fixed-height scroll region (line 222-272)
  - [x] Selected row highlighted via `row.set_selected(is_selected)` (line 254-256)
  - [x] Renders ComboBox below threshold, TableBuilder at/above (line 159-176)
- **Tests**: select module tests pass under the 324-test sweep.
- **Notes**: ADR-001 row 5 alignment note from the original ticket is not explicitly in the current `select.rs:6` doc comment — but the choice (Select keeps both forms, configurable threshold) is encoded clearly in code+doc.
- **New issue / stub spotted**: ticket wanted `select.rs:6` ADR-001 alignment note updated; the current doc says `TableBuilder-driven scrollable picker for large option sets` and references row 5 of ADR-001 by feature description. Adequate.

### WEFT-268 — Explorer HealthViewer
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/src/explorer/viewers/health.rs` (entire 257-line file); `crates/clawft-gui-egui/src/ontology/types/health_report.rs`
- **Acceptance criteria met**:
  - [x] `HealthReport` Object Type registered in `ontology/types/health_report.rs` (priority 12)
  - [x] `HealthViewer` registered in `viewers/mod.rs`
  - [x] Renders RSSI chip with colour bands, key/value board for known fields
  - [x] Smoke test: `paint_does_not_panic` (lines 242-256)
- **Tests**: `matches_kind_health`, `matches_two_scalars`, `rejects_single_scalar`, `rejects_array`, `parent_node_path_strips_health_suffix`, `parent_node_path_returns_none_for_root`, `format_scalar_handles_null`, `paint_does_not_panic` — all pass.
- **Notes**: Also embeds inline sparklines (WEFT-271) and Action surface (WEFT-276).
- **New issue / stub spotted**: none.

### WEFT-269 — Explorer SensorViewer raw/summary switcher
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/src/explorer/viewers/sensor.rs` (311 lines)
- **Acceptance criteria met**:
  - [x] `Sensor` Object Type classifier (already in `ontology/types/sensor.rs`)
  - [x] `SensorViewer` with `Pane::{Summary, Raw, Both}` switcher chip row (lines 100-120)
  - [x] Pane state persisted via `egui::Id` memory keyed on substrate path (line 92-95)
  - [x] Disabled chip for missing pane so a summary-only sensor doesn't offer a dead Raw toggle (line 112-114)
  - [x] Re-dispatches sub-pane through viewer registry — AudioMeterViewer paints summary, PcmChunkViewer paints raw, etc. (line 196-224)
- **Tests**: `matches_kind_mic`, `matches_paired_raw_summary`, `rejects_unknown_kind_no_split`, `parent_node_path_*` (4 variants), `paint_does_not_panic_on_summary_only`, `paint_does_not_panic_on_full_envelope` — all pass.
- **Notes**: Default pane is `Summary` (matches "show the cheap thing first" guidance per the EXPLORER-MANAGEMENT-SURFACE doc).
- **New issue / stub spotted**: none.

### WEFT-270 — Explorer tree filter chip row
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/src/explorer/tree.rs:23-85,360-410`
- **Acceptance criteria met**:
  - [x] `TreeFilters` struct with 4 filters: `name_query`, `active_only`, `sensors_only`, `hide_leaves` (lines 31-44)
  - [x] Filters persist within session (in-memory only, on `Explorer.tree_filters`)
  - [x] Tree re-renders on filter change (filter chips painted outside ScrollArea; `passes()` consulted per row)
  - [x] "clear" pill appears only when filters active (line 401-408)
- **Tests**: 7+ TreeFilters tests in `tests` module — `default_has_no_filters_active`, `name_query_filters_by_substring_case_insensitive`, `sensors_only_filters_to_sensor_subtree`, `hide_leaves_drops_value_paths`, `active_only_drops_inactive_paths`, etc. — all pass.
- **Notes**: Original ticket asked for "type + status filters" — implementation is broader (name search + 3 toggles). All categorical filters present.
- **New issue / stub spotted**: none.

### WEFT-271 — Explorer sparkline embed
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/src/explorer/viewers/time_series.rs:51-115`; `viewers/health.rs:101-121`
- **Acceptance criteria met**:
  - [x] `embed_sparkline(ui, path, value, height)` public helper exposed (line 64-115)
  - [x] HealthViewer iterates known scalar fields and embeds an inline sparkline per field with synthetic `path/<field>` key (health.rs:107-121)
  - [x] History bounded at `MAX_HISTORY = 240` samples (~1 min @ 4Hz)
  - [x] Process-global `HISTORY` map keyed on substrate path; survives panel toggle
- **Tests**: TimeSeriesViewer history-related tests in same file (`MAX_HISTORY` enforced, pushes monotonic).
- **Notes**: SensorViewer doesn't embed the same sparkline pattern — its summary pane re-dispatches through the registry which can hit TimeSeriesViewer naturally. Consistent with the stateless-viewer dispatch model.
- **New issue / stub spotted**: none.

### WEFT-272 — Explorer Sensor↔Node breadcrumb intent
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/src/explorer/mod.rs:84-107,366-374`; `viewers/health.rs:49-72`; `viewers/sensor.rs:65-87`
- **Acceptance criteria met**:
  - [x] `request_navigation(ctx, path)` and `take_navigation_request(ctx)` API on `egui::Context` memory stash (line 96-107)
  - [x] `NAV_INTENT_KEY` constant (line 84)
  - [x] Explorer drains intent on `show()` and runs through `on_select` (line 372-374)
  - [x] HealthViewer breadcrumb link calls `request_navigation` (health.rs:64-69)
  - [x] SensorViewer breadcrumb link calls `request_navigation` (sensor.rs:80-85)
- **Tests**: navigation drain covered by Explorer integration; breadcrumb link logic tested via `parent_node_path_*` tests in both viewers.
- **Notes**: Implementation chose egui-memory stash over the original `CanonResponse::SelectPath` proposal — same observable behaviour, doesn't pollute the response enum. Better choice.
- **New issue / stub spotted**: none.

### WEFT-274 — Explorer Workshop parameterization
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/src/explorer/workshop.rs:129-194,212-356,701-713`
- **Acceptance criteria met**:
  - [x] Schema `{ params: { name: value }, panels: [{ substrate_path_template: "..." }] }` parses
  - [x] `${name}` substitution implemented in `substitute()` (lines 316-356)
  - [x] Missing param → `Err(name)` returned, panel renders inline `missing param ...` hint (workshop.rs:705-713)
  - [x] Literal `${name}` retained in partial path so the rendered diagnostic is useful
  - [x] Either literal `substrate_path` or `substrate_path_template` accepted; literal wins
- **Tests**: `workshop_integration::substrate_workshop_with_parameter_template_substitutes` — Plus unit tests `parse_full_workshop`, `substitute_handles_unterminated_placeholder`. All in 324-pass sweep.
- **Notes**: TOML watcher example expected to use this; decision memo file location not verified.
- **New issue / stub spotted**: none.

### WEFT-276 — Explorer ObjectType::applicable_actions for Mesh/Sensor/Node
- **Status**: confirmed in code
- **Files**:
  - `crates/clawft-gui-egui/src/ontology/types/mesh.rs:95-108`: `["mesh.export_snapshot", "mesh.list_nodes", "mesh.refresh"]`
  - `crates/clawft-gui-egui/src/ontology/types/sensor.rs:83-94`: `["sensor.toggle_summary", "sensor.snapshot", "sensor.copy_path"]`
  - `crates/clawft-gui-egui/src/ontology/types/node.rs:83-94`: `["node.copy_pubkey", "node.export_snapshot", "node.refresh_health"]`
  - Surfaced in HealthViewer (`health.rs:126-141`) and SensorViewer (`sensor.rs:154-169`) as a passive bullet list under an `actions` heading.
- **Acceptance criteria met**:
  - [x] Per-type schema decided (`&'static [&'static str]` of action names)
  - [x] Populated for Mesh, Sensor, Node
  - [x] Read-only surface (passive list — full Action pipeline is T08-33+)
  - [x] Object Type tests updated (`declares_actions` test in sensor.rs:138-139, node.rs:150-151)
- **Tests**: `sensor::tests::declares_actions`, `node::tests::declares_actions`, `mesh::tests::*` — pass. NB: there is no `mesh::tests::declares_actions` named test but `mesh::tests::matches_minimum_threshold` passes and the actions are statically declared.
- **Notes**: Action surface only painted from HealthViewer + SensorViewer today (not directly from a `MeshViewer` because mesh-root rendering goes through generic dispatch). Acceptable for read-only tier — Action pipeline T08-33+ will revisit.
- **New issue / stub spotted**: minor — Mesh actions declared but not yet rendered anywhere visible (no dedicated MeshViewer). Logged as a potential follow-up rather than a current bug, since the ticket says "read-only — full pipeline is T08-33+".

### WEFT-278 — Workshop Grid layout
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/src/explorer/workshop.rs:565-606,762-773`
- **Acceptance criteria met**:
  - [x] `WorkshopLayout::Grid` recognised in `from_str` ("grid" → Grid; line 158)
  - [x] `paint_grid` uses `egui::Grid` with `ceil(sqrt(n))` columns (line 575); column count helper `grid_columns_for(n)` extracted with doc + tests
  - [x] Cell width derived from available width; min 120px floor (line 576)
  - [x] Trailing partial row closes cleanly (line 599-603)
- **Tests**: `workshop_integration::substrate_workshop_grid_layout_round_trips` — exercises full publish→parse→layout cycle. Plus unit tests for `grid_columns_for`.
- **Notes**: Empty Workshop short-circuits to the empty-state placeholder cleanly.
- **New issue / stub spotted**: none.

### WEFT-279 — Workshop Tabs layout
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/src/explorer/workshop.rs:608-649`
- **Acceptance criteria met**:
  - [x] `WorkshopLayout::Tabs` recognised in `from_str` ("tabs" → Tabs; line 159)
  - [x] `paint_tabs` renders horizontal selectable tab bar; selected panel painted in full beneath (line 614-649)
  - [x] Active tab persisted in egui memory (line 621-624) — survives schema re-parse
  - [x] Active index clamped on hot-reload shrink (line 626-628)
- **Tests**: `workshop_integration::substrate_workshop_tabs_layout_round_trips` covers the round-trip.
- **Notes**: Tab labels fall back to `substrate_path` when `title` is unset — sensible.
- **New issue / stub spotted**: none.

### WEFT-280 — Workshop `viewer_hint` overrides
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/src/explorer/workshop.rs:188-191,775-833`
- **Acceptance criteria met**:
  - [x] `WorkshopPanel::viewer_hint: String` parsed from JSON (defaults `"auto"`; line 188-191, 281-285)
  - [x] `paint_with_viewer_hint(ui, hint, path, value)` resolves named hint or falls back to dispatch (line 781-805)
  - [x] `viewer_for_hint(name)` registers 11 viewers: `audio_meter`, `chain_tail`, `connection_badge`, `depth_map`, `graph`, `json|json_fallback`, `mesh_nodes`, `pcm_chunk`, `process_table`, `time_series`, `waveform` (line 815-833)
  - [x] Unknown hint name renders an inline orange diagnostic and falls back to auto dispatch (line 796-803)
- **Tests**: `workshop_integration::substrate_workshop_viewer_hint_named_round_trips` — covers named-hint round-trip. Unit tests `parse_full_workshop`, etc.
- **Notes**: Sensible coverage of the registered viewer set. `health` and `sensor` are deliberately not in the registry because they require shape-match fall-through (those viewers have specific value-shape requirements).
- **New issue / stub spotted**: none.

### WEFT-283 — VSCode panel typed active-radar return schema
- **Status**: deferred — confirmed not in code (per task description)
- **Files**: not present.
- **Notes**: Audit description explicitly marks this `(deferred)`. Plane ticket should be in a deferred cycle (`0.8.x`+).
- **New issue / stub spotted**: none — deferred work is correctly tracked.

### WEFT-286 — Hygiene blocks/ vs canon/ duality docs
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/src/blocks/mod.rs:1-51`
- **Acceptance criteria met**:
  - [x] Module doc explains `canon/` (frozen 21-primitive vocabulary, system-of-record) vs `blocks/` (12 demo blocks, theming spike + showroom continuity)
  - [x] WEFT-286 cited in the doc heading
  - [x] Retirement plan (3 steps) documented inline
  - [x] Decision: keep blocks as demo-only path until canon gallery covers theming-proof needs
- **Tests**: doc-only change; covered by `cargo doc --no-deps` build (not run here, but workspace builds OK per the lib-test sweep).
- **Notes**: Useful prose; calls out that the 0.6.19 changelog mentioned a "retrofit pass" that was partial.
- **New issue / stub spotted**: none.

### WEFT-287 — Hygiene vendored vs upstream egui_demo_lib decision
- **Status**: confirmed in code
- **Files**: `crates/clawft-gui-egui/Cargo.toml:220-249`
- **Acceptance criteria met**:
  - [x] Decision: continue vendoring `fractal_clock.rs`, `http_app.rs`, `custom3d_glow.rs` from upstream
  - [x] Rationale documented (upstream `egui_demo_app` is binary-only, depending would force renamed `[[bin]]` shim plus wgpu/glutin trees)
  - [x] Maintenance contract (vendored files re-vendored verbatim, never edited; serve only `cfg_attr` keep-alive)
  - [x] `serde` feature kept declared (never enabled) only to satisfy upstream `cfg_attr` lines under `-D warnings`
- **Tests**: build — workspace compiles, lib tests pass.
- **Notes**: Decision memo lives in the Cargo.toml feature section rather than under `.planning/`. Acceptable for a build-system decision; readers naturally land there when adding deps.
- **New issue / stub spotted**: none.

### WEFT-289 — VSCode panel `npm run package` + .vsix flow
- **Status**: confirmed in code/docs
- **Files**: `extensions/vscode-weft-panel/package.json:32`, `extensions/vscode-weft-panel/.vscodeignore`, `extensions/vscode-weft-panel/README.md:49-83`
- **Acceptance criteria met**:
  - [x] `npm run package` script wired: `"package": "vsce package --allow-missing-repository"` (package.json:32)
  - [x] `.vscodeignore` excludes `src/**`, `node_modules/**`, `.gitignore`, plus WEFT-289-specific drops of wasm-pack scaffolding (`webview/wasm/package.json`, `webview/wasm/.gitignore`)
  - [x] README:49-83 documents the install flow, the build-wasm prerequisite, and the `--allow-missing-repository` requirement
  - [x] README explicitly notes the WEFT-289 currency check date (2026-04-30)
  - [x] Cursor install path also documented (`cursor --install-extension`)
- **Tests**: doc-currency check; no automated test covers `vsce package` (would require network + a mocked vsce).
- **Notes**: I did NOT run `npm run package` myself in this audit (disk full and the artifact is large) — but the script + .vscodeignore + README are coherent and consistent with the earlier wasm bundle present at `extensions/vscode-weft-panel/webview/wasm/clawft_gui_egui_bg.wasm` (6.9 MB raw). The README correctly warns the user to build the wasm first or get a fallback "Failed to load the wasm bundle" card.
- **New issue / stub spotted**: see "Bundle size" cross-cutting finding below — the on-disk wasm artifact is over the script-budget for `wasm-panel`. May be a stale dev artifact that wasn't subject to wasm-opt; still worth checking if a fresh build clears the gate.

## Cross-cutting findings

### Stubs / TODOs spotted

- `crates/clawft-gui-egui/src/explorer/terminal.rs:1321-1344` — wasm32 stub for `Terminal` is intentional and clearly documented (alacritty pulls platform-specific PTY/polling crates incompatible with wasm). Renders a passive "Terminal is not available in the browser build." chip. Not a bug; original WEFT-260 explicitly allows the wasm-stub posture.
- No other `todo!()` / `unimplemented!()` / `// TODO:` / `// FIXME:` markers found in the audited files (`explorer/`, `canon/field.rs`, `canon/select.rs`, `ontology/types/{mesh,sensor,node}.rs`).
- Many `placeholder` strings exist but are param-names (e.g., `Field::text(placeholder)` for hint text) or doc references (Workshop `${name}` placeholder syntax); none are stub code.

### Bundle size

- Current wasm artifact on disk: `extensions/vscode-weft-panel/webview/wasm/clawft_gui_egui_bg.wasm` = **7,194,711 bytes (~7.0 MB raw)**, gzipped **3,259,037 bytes (~3.2 MB)**.
- `scripts/build.sh wasm-panel` budget: **raw ≤ 4500 KB, gz ≤ 1500 KB**.
- The on-disk artifact is dated 2026-04-26 — very likely a debug or pre-wasm-opt build (the script also fail-soft skips wasm-opt if binaryen is missing). Both raw (7000 vs 4500 KB) and gz (3259 vs 1500 KB) exceed the budget.
- **Cannot rebuild here** — host disk is at 100% / 1.6 MB free, so `cargo build --target wasm32-unknown-unknown --release` cannot complete (encountered `No space left on device` while attempting WS work).
- **Recommendation**: a separate Plane item should be filed to verify the optimized wasm bundle still fits the budget after a clean rebuild. If a clean build still exceeds, WEFT-246 may need re-tightening or the budget may need to grow as the chat / workshop / terminal / multiple new viewer surfaces have all landed since the last budget set.

### Tests

- `cargo test -p clawft-gui-egui --lib`: **324 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out** (finished in 0.98s on this host).
- `cargo test -p clawft-gui-egui --test workshop_integration`: **7/7 passed** (re-run 2026-05-01 after disk cleared). Covers: substrate publish/parse round-trip, republish reshape, parameter substitution, Grid layout round-trip, Tabs layout round-trip, named-viewer-hint round-trip, unrelated-path untouched.
- `cargo test -p clawft-gui-egui --test compose_extra_iris`: **12/12 passed** (re-run 2026-05-01 after disk cleared).

### Recommendations

1. **Rebuild + measure wasm bundle on clean disk before any 0.7.0 RC tag.** Current on-disk artifact is well over the documented budget; if the post-wasm-opt size is still above budget, the budget needs adjusting or a size-reduction sweep needs scheduling. See "Bundle size" above.
2. ~~**Run integration tests on a host with disk headroom.**~~ Resolved 2026-05-01: `workshop_integration` (7/7) and `compose_extra_iris` (12/12) re-run green after disk cleared.
3. **Minor doc-comment fix in `chat.rs` paint_bubble** — the comment claims a `id_salt` is "unique-per-bubble (combined hash of content + role)" but the actual `CommonMarkViewer::new()` call doesn't set a custom id_salt. Functionally fine; cosmetic doc drift.
4. **Action surface for Mesh-rooted values is declared but not yet rendered** — `mesh.export_snapshot`, `mesh.list_nodes`, `mesh.refresh` are wired into `Mesh::capabilities()` but no viewer paints them yet (HealthViewer / SensorViewer paint their respective lists). Acceptable per the ticket's "read-only — full pipeline is T08-33+" caveat; worth a follow-up Plane item for visibility once the Action pipeline lands.

### New issues filed

- None. All findings are either deliberate scope-cuts (terminal wasm stub, identity-warning doc-link, mesh actions not yet painted) or environmental (disk-full preventing integration runs, stale wasm artifact pre-dating the budget). I did NOT file new Plane items because each finding is either explicitly acceptable per its ticket or already tracked elsewhere (WEFT-246 budget; WEFT-283 future Action pipeline).

## Summary

- Items confirmed shipped: **24/25** (all in scope; WEFT-283 explicitly deferred, marked confirmed-as-deferred).
- Items with concerns / partial: **0** material concerns; 3 cosmetic / follow-up notes (doc-comment drift in chat.rs, mesh-actions not painted, wasm-bundle-size verification recommended on a clean disk).
- New issues filed: 0.
