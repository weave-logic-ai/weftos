//! Desktop compositor: warped grid wallpaper + floating windows + tray.

use eframe::egui;

use super::{grid, sidebar, tray};
use crate::blocks;
use crate::canon_demos::{self, CanonDemoState, CanonKind};
use crate::explorer::Explorer;
use crate::live::Snapshot;
use crate::surface_host;
use clawft_app::registry::AppRegistry;
use clawft_surface::SurfaceTree;

/// Admin app manifest, loaded inline so the reference panel always
/// boots with at least one app installed even on a fresh workspace.
/// Path-resolved at compile time via `include_str!`.
const WEFTOS_ADMIN_MANIFEST: &str =
    include_str!("../../../clawft-app/fixtures/weftos-admin.toml");

/// Admin desktop surface description (ADR-016 §10). Loaded inline for
/// the same reason as the manifest above.
const WEFTOS_ADMIN_DESKTOP_SURFACE: &str =
    include_str!("../../../clawft-surface/fixtures/weftos-admin-desktop.toml");

/// Per-tray-chip detail surfaces. Each one binds to the substrate
/// subtree its chip reflects. Loaded inline so there's always a
/// fixture to render when a chip is clicked — no disk IO on the
/// wasm path.
const CHIP_SURFACE_KERNEL: &str =
    include_str!("../../../clawft-surface/fixtures/weftos-chip-kernel.toml");
const CHIP_SURFACE_MESH: &str =
    include_str!("../../../clawft-surface/fixtures/weftos-chip-mesh.toml");
const CHIP_SURFACE_EXOCHAIN: &str =
    include_str!("../../../clawft-surface/fixtures/weftos-chip-exochain.toml");

/// TOML surface fixture for chips that render through the composer.
/// Returns `None` for chips rendered by bespoke panel code (today:
/// Explorer — see `render_explorer_placeholder`).
fn chip_surface_toml(chip: tray::ChipId) -> Option<&'static str> {
    match chip {
        tray::ChipId::Kernel => Some(CHIP_SURFACE_KERNEL),
        tray::ChipId::Mesh => Some(CHIP_SURFACE_MESH),
        tray::ChipId::ExoChain => Some(CHIP_SURFACE_EXOCHAIN),
        tray::ChipId::Explorer => None,
    }
}

/// Which section of the Blocks window is active.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum PanelSection {
    Blocks,
    Canon,
    Apps,
}

pub struct Desktop {
    pub launcher_open: bool,
    /// Tray chip whose detail window is currently open, if any.
    /// Clicking the chip again (or closing the window) resets to `None`.
    pub open_chip: Option<super::tray::ChipId>,
    pub blocks_state: blocks::DemoState,
    pub canon_state: CanonDemoState,
    pub selected_block: BlockKind,
    pub selected_canon: CanonKind,
    pub section: PanelSection,
    pub boot_started: web_time::Instant,

    /// App registry — seeded with WeftOS Admin at startup (M1.5
    /// reference app). Future apps register themselves via
    /// `registry::install`.
    pub app_registry: AppRegistry,
    /// Id of the app currently shown in the Apps panel (e.g.
    /// `app://weftos.admin`). `None` means nothing selected yet.
    pub selected_app: Option<String>,
    /// Cached parsed surface trees keyed by app id, so we don't
    /// re-parse the TOML every frame.
    pub app_surfaces: std::collections::BTreeMap<String, SurfaceTree>,
    /// Cached parsed surface trees for tray-chip detail windows.
    /// Populated at startup from `CHIP_SURFACE_*` fixtures; `None`
    /// entries mean the fixture failed to parse (fallback to the
    /// raw-JSON view kicks in).
    pub chip_surfaces: std::collections::BTreeMap<tray::ChipId, SurfaceTree>,
    /// Ontology Explorer panel state — left-tree + right-detail. The
    /// Explorer tray chip opens this; see `.planning/explorer/PROJECT-PLAN.md`.
    pub explorer: Explorer,

    /// Canonical desktop sidebar — DESIGN.md §5. Phase 2a (0.8.0) of
    /// the desktop revision. Replaces the launcher window and the
    /// tray chips as the primary launcher; the legacy chrome remains
    /// alongside for now and is retired one app at a time during
    /// Phase 3.
    pub sidebar: sidebar::Sidebar,

    /// Per-field debounce + selection buffer for the Settings app.
    /// Survives across frames so an in-flight edit is not clobbered
    /// by the next snapshot tick. WEFT-583.
    pub settings_state: crate::apps::settings::SettingsState,
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum BlockKind {
    Overview,
    Text,
    Button,
    Code,
    Status,
    Budget,
    Table,
    Tree,
    Tabs,
    Terminal,
    Layout,
    Oscilloscope,
}

impl BlockKind {
    pub const ALL: [(BlockKind, &'static str); 12] = [
        (BlockKind::Overview, "Overview"),
        (BlockKind::Text, "Text"),
        (BlockKind::Button, "Button"),
        (BlockKind::Code, "Code"),
        (BlockKind::Status, "Status"),
        (BlockKind::Budget, "Budget"),
        (BlockKind::Table, "Table"),
        (BlockKind::Tree, "Tree"),
        (BlockKind::Tabs, "Tabs"),
        (BlockKind::Terminal, "Terminal"),
        (BlockKind::Layout, "Layout"),
        (BlockKind::Oscilloscope, "Oscilloscope"),
    ];
}

impl Desktop {
    /// Apply a sidebar click. After WEFT-579..591 graduation, every
    /// target is owned by its `apps/<id>.rs` module — sidebar actions
    /// are pure delegations to `Sidebar::apply`. The legacy chip-detail
    /// window and floating Blocks launcher were retired together with
    /// the bottom tray when this graduation wave landed.
    pub fn apply_sidebar_action(&mut self, action: sidebar::SidebarAction) {
        self.sidebar.apply(action);
    }
}

impl Default for Desktop {
    fn default() -> Self {
        // Session-local registry path: persists to XDG-standard
        // location on native, but we never block startup on save
        // success. On wasm `default_path()` returns an err because
        // HOME is unset — we fall back to an in-memory-only path
        // under `/tmp/weftos` (the save error is ignored below).
        let registry_path = AppRegistry::default_path()
            .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/weftos/apps.json"));
        let mut app_registry = AppRegistry::new(registry_path);
        // Best-effort load: missing file is not an error.
        let _ = app_registry.load();

        let mut app_surfaces = std::collections::BTreeMap::new();
        let mut selected_app = None;

        // Ensure the WeftOS Admin reference app is installed so the
        // Apps tab has content on first boot. Parse errors here are
        // programmer errors in the bundled fixture, not user input —
        // log and skip rather than panic so the desktop still comes up.
        match clawft_app::manifest::AppManifest::from_toml_str(WEFTOS_ADMIN_MANIFEST) {
            Ok(manifest) => {
                let id = manifest.id.clone();
                if app_registry.get(&id).is_none() {
                    // `install` persists to disk; if that fails (wasm,
                    // read-only fs, …) we still keep the in-memory
                    // entry so the Apps panel has something to show.
                    if let Err(e) = app_registry.install(manifest.clone()) {
                        log::warn!(
                            "couldn't persist WeftOS Admin to registry: {e} (continuing in-memory)"
                        );
                    }
                }
                selected_app = Some(id.clone());
                match clawft_surface::parse::parse_surface_toml(
                    WEFTOS_ADMIN_DESKTOP_SURFACE,
                ) {
                    Ok(tree) => {
                        app_surfaces.insert(id, tree);
                    }
                    Err(e) => {
                        log::warn!(
                            "failed to parse WeftOS Admin desktop surface: {e}"
                        );
                    }
                }
            }
            Err(e) => {
                log::warn!("failed to parse WeftOS Admin manifest: {e}");
            }
        }

        // Parse each tray-chip detail fixture once at startup. Failures
        // are programmer errors in the bundled TOML (not user input) so
        // we log + skip and let the chip panel fall back to the raw
        // JSON dump.
        let mut chip_surfaces = std::collections::BTreeMap::new();
        for chip in [
            tray::ChipId::Kernel,
            tray::ChipId::Mesh,
            tray::ChipId::ExoChain,
            tray::ChipId::Explorer,
        ] {
            // Explorer has no surface fixture — its detail window is
            // rendered by `render_explorer_placeholder` (MVP) until the
            // tree-view panel from `.planning/explorer/PROJECT-PLAN.md`
            // ships. Skip it here.
            let Some(toml) = chip_surface_toml(chip) else {
                continue;
            };
            match clawft_surface::parse::parse_surface_toml(toml) {
                Ok(tree) => {
                    chip_surfaces.insert(chip, tree);
                }
                Err(e) => {
                    log::warn!(
                        "failed to parse chip surface for {:?}: {e} (raw JSON fallback)",
                        chip
                    );
                }
            }
        }

        Self {
            launcher_open: false,
            open_chip: None,
            blocks_state: blocks::DemoState::default(),
            canon_state: CanonDemoState::default(),
            selected_block: BlockKind::Overview,
            selected_canon: CanonKind::Pressable,
            section: PanelSection::Blocks,
            boot_started: web_time::Instant::now(),
            app_registry,
            selected_app,
            app_surfaces,
            chip_surfaces,
            explorer: Explorer::default(),
            sidebar: sidebar::Sidebar::default(),
            settings_state: crate::apps::settings::SettingsState::default(),
        }
    }
}

pub fn show(
    ui: &mut egui::Ui,
    desk: &mut Desktop,
    live: &std::sync::Arc<crate::live::Live>,
    snap: &Snapshot,
) {
    let total = ui.max_rect();

    // Sidebar reserves the left edge — DESIGN.md §5.
    let sidebar_w = desk.sidebar.reserved_width();
    let sidebar_rect = egui::Rect::from_min_max(
        total.min,
        egui::pos2(total.left() + sidebar_w, total.bottom()),
    );
    let rect = egui::Rect::from_min_max(
        egui::pos2(total.left() + sidebar_w, total.top()),
        total.max,
    );

    // Wallpaper — warped grid (right of the sidebar only).
    let t = desk.boot_started.elapsed().as_secs_f32();
    grid::paint(ui, rect, t);

    // Sidebar paint + click handling. WEFT-579..591 graduation: every
    // sidebar target is owned by its `apps/<id>.rs` module, so the
    // sidebar action is a pure delegation. No more dual-render with
    // legacy chip-detail or launcher floating windows.
    let sidebar_action = sidebar::paint(ui, sidebar_rect, &desk.sidebar, snap);
    if let Some(action) = sidebar_action {
        desk.apply_sidebar_action(action);
    }

    // Active app body — DESIGN.md §4.1 / §9. Each app owns its own
    // panel; the dispatcher hands it `&mut Desktop` and `&Arc<Live>`
    // so it can mutate its own state and submit RPC commands.
    crate::apps::dispatch(ui, rect, desk, live, snap);
}

/// Render the Explorer panel body inside the chip-detail window.
/// Splits header + two-pane layout; the Explorer itself draws the
/// SidePanel / CentralPanel pair internally.
#[allow(dead_code)] // wired up by `apps/explorer.rs` (WEFT-590) graduation
pub(crate) fn render_explorer(
    ui: &mut egui::Ui,
    desk: &mut Desktop,
    live: &std::sync::Arc<crate::live::Live>,
    snap: &Snapshot,
) {
    ui.horizontal(|ui| {
        ui.heading("Explorer");
        ui.separator();
        ui.monospace("substrate/");
        ui.separator();
        connection_pill(ui, snap.connection);
    });
    ui.separator();
    desk.explorer.show(ui, live);
}

/// Render the chip-detail window. Prefers the per-chip surface
/// fixture (composer path); falls back to a pretty-printed JSON dump
/// of the substrate subtree if the fixture is missing / failed to
/// parse, so the window is never blank.
#[allow(dead_code)] // wired up by `apps/network.rs` (WEFT-582) graduation
pub(crate) fn render_chip_detail(
    ui: &mut egui::Ui,
    desk: &Desktop,
    chip: tray::ChipId,
    live: &std::sync::Arc<crate::live::Live>,
    snap: &Snapshot,
) {
    ui.horizontal(|ui| {
        ui.heading(chip.label());
        ui.separator();
        ui.monospace(chip.substrate_path());
        ui.separator();
        connection_pill(ui, snap.connection);
    });
    ui.separator();

    // Explorer is handled out-of-band by `render_explorer` in the
    // window-rendering code above — it needs `&mut Desktop` to mutate
    // its panel state, which this helper (`&Desktop`) can't provide.
    debug_assert!(!matches!(chip, tray::ChipId::Explorer));

    // Surface-composer path (preferred). The ontology snapshot is the
    // same source of truth the Admin app reads, so fixtures written
    // here stay valid for the M1.6+ substrate-over-postMessage bridge.
    // The `ui://heatmap` primitive handles the ToF grid declaratively
    // now; the native escape hatch that used to live here was
    // retired when the composer primitive shipped.
    if let Some(tree) = desk.chip_surfaces.get(&chip) {
        let ontology = live.substrate_snapshot();
        let outcome = surface_host::compose(tree, &ontology, ui);
        for d in outcome.dispatches {
            log::info!(
                "chip-detail affordance: {} -> {} ({:?}) from {}",
                d.affordance,
                d.verb,
                d.params,
                d.source_path
            );
            live.submit(crate::live::Command::Raw {
                method: d.verb,
                params: d.params,
                reply: None,
            });
        }
        // Below the composer: a diagnostic footer. Distinguishes the
        // three "empty panel" failure modes — connection down, poll in
        // flight, feature not wired — so a blank panel reads as signal
        // instead of a bug.
        if tray::chip_subtree(chip, snap).is_none() {
            ui.add_space(6.0);
            render_empty_hint(ui, chip, snap);
        }
        return;
    }

    // Fallback: raw JSON dump. Only hit if the TOML fixture above
    // failed to parse at startup (logged in `Desktop::default`).
    match tray::chip_subtree(chip, snap) {
        Some(value) => {
            let pretty = serde_json::to_string_pretty(value)
                .unwrap_or_else(|_| value.to_string());
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut pretty.as_str())
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY)
                            .code_editor()
                            .interactive(false),
                    );
                });
        }
        None => {
            ui.colored_label(
                egui::Color32::from_rgb(170, 170, 180),
                "No adapter data yet for this subsystem.",
            );
        }
    }
}

/// Small coloured pill showing the daemon-link state — green for
/// connected, amber for connecting (first poll in flight), red for
/// disconnected. Shown in the chip-detail header so the user can see
/// the connection's state without hunting for it.
#[allow(dead_code)] // wired up by chip-detail-using app graduations
pub(crate) fn connection_pill(ui: &mut egui::Ui, conn: crate::live::Connection) {
    let (text, color) = match conn {
        crate::live::Connection::Connected => (
            "● connected",
            egui::Color32::from_rgb(110, 210, 160),
        ),
        crate::live::Connection::Connecting => (
            "◐ connecting…",
            egui::Color32::from_rgb(240, 200, 110),
        ),
        crate::live::Connection::Disconnected => (
            "◯ disconnected",
            egui::Color32::from_rgb(240, 150, 150),
        ),
    };
    ui.label(
        egui::RichText::new(text)
            .monospace()
            .color(color),
    );
}

/// Diagnostic footer for chip-detail windows whose subsystem has no
/// data yet. Pulls the reason out of `snap.connection` and
/// `snap.last_error` so the user sees *why* they're looking at an
/// empty panel — stale extension JS / daemon crashed / adapter not
/// wired / etc.
#[allow(dead_code)] // wired up by chip-detail-using app graduations
pub(crate) fn render_empty_hint(ui: &mut egui::Ui, chip: tray::ChipId, snap: &Snapshot) {
    let (body, show_error) = match snap.connection {
        crate::live::Connection::Connecting => (
            "Waiting for first poll tick (~1s).".to_string(),
            false,
        ),
        crate::live::Connection::Disconnected => (
            "Daemon link is down — the extension's RPC to the daemon \
             failed on the last tick. Check `weaver kernel status` and \
             that the socket at .weftos/runtime/kernel.sock is the one \
             your editor's workspace root resolves to."
                .to_string(),
            true,
        ),
        crate::live::Connection::Connected => match chip {
            tray::ChipId::Kernel => (
                "Daemon is connected but no kernel.status data has landed \
                 yet. If this persists, the RPC is succeeding but returning \
                 an unexpected shape — check the daemon log."
                    .to_string(),
                true,
            ),
            tray::ChipId::Mesh | tray::ChipId::ExoChain => (
                "Daemon connected but this subsystem isn't reporting. On \
                 native, MeshAdapter/ChainAdapter populate these via \
                 cluster.status/chain.status. On wasm the extension's RPC \
                 allowlist doesn't include those verbs yet — the substrate-\
                 over-postMessage bridge lands in M1.6+."
                    .to_string(),
                true,
            ),
            tray::ChipId::Explorer => (
                // Unreachable in practice — Explorer short-circuits into
                // `render_explorer_placeholder` before chip_subtree is
                // consulted. Kept exhaustive for the compiler.
                "Explorer uses a dedicated detail view.".to_string(),
                false,
            ),
        },
    };
    ui.label(
        egui::RichText::new(body)
            .italics()
            .color(egui::Color32::from_rgb(170, 170, 180)),
    );
    if show_error
        && let Some(err) = &snap.last_error {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!("last error: {err}"))
                    .monospace()
                    .color(egui::Color32::from_rgb(220, 140, 140)),
            );
        }
}

#[allow(dead_code)] // wired up by graduations needing floating windows
pub(crate) fn window_frame() -> egui::Frame {
    egui::Frame::window(&egui::Style::default())
        .fill(egui::Color32::from_rgba_unmultiplied(18, 18, 24, 235))
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 25),
        ))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::same(8))
}

#[allow(dead_code)] // wired up by `apps/launcher.rs` (WEFT-591) graduation
pub(crate) fn render_blocks_window(
    ui: &mut egui::Ui,
    desk: &mut Desktop,
    live: &std::sync::Arc<crate::live::Live>,
    snap: &Snapshot,
) {
    egui::Panel::left("blocks_nav")
        .resizable(false)
        .default_size(170.0)
        .show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(desk.section == PanelSection::Blocks, "Blocks")
                    .clicked()
                {
                    desk.section = PanelSection::Blocks;
                }
                if ui
                    .selectable_label(desk.section == PanelSection::Canon, "Canon")
                    .clicked()
                {
                    desk.section = PanelSection::Canon;
                }
                if ui
                    .selectable_label(desk.section == PanelSection::Apps, "Apps")
                    .clicked()
                {
                    desk.section = PanelSection::Apps;
                }
            });
            ui.separator();
            // Launcher-menu entry for the Ontology Explorer (plan §6 Q1
            // default). Clicking opens the Explorer chip-detail window
            // the same way the Explorer tray chip does.
            if ui
                .button(egui::RichText::new("⌸ Open Explorer").monospace())
                .on_hover_text("Tree view of the substrate namespace")
                .clicked()
            {
                desk.open_chip = Some(tray::ChipId::Explorer);
            }
            ui.separator();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| match desk.section {
                    PanelSection::Blocks => {
                        for (kind, label) in BlockKind::ALL {
                            let selected = desk.selected_block == kind;
                            if ui.selectable_label(selected, label).clicked() {
                                desk.selected_block = kind;
                            }
                        }
                    }
                    PanelSection::Canon => {
                        for (kind, label) in CanonKind::ALL {
                            let selected = desk.selected_canon == kind;
                            if ui.selectable_label(selected, label).clicked() {
                                desk.selected_canon = kind;
                            }
                        }
                    }
                    PanelSection::Apps => {
                        // Snapshot `(id, name)` up front so we don't
                        // hold a borrow over the click handler that
                        // writes `desk.selected_app`.
                        let entries: Vec<(String, String)> = desk
                            .app_registry
                            .list()
                            .iter()
                            .map(|a| {
                                (a.manifest.id.clone(), a.manifest.name.clone())
                            })
                            .collect();
                        if entries.is_empty() {
                            ui.label(
                                egui::RichText::new("No apps installed")
                                    .italics()
                                    .color(egui::Color32::GRAY),
                            );
                        } else {
                            for (id, name) in entries {
                                let selected = desk.selected_app.as_deref() == Some(id.as_str());
                                if ui.selectable_label(selected, name).clicked() {
                                    desk.selected_app = Some(id);
                                }
                            }
                        }
                    }
                });
        });

    egui::CentralPanel::default().show_inside(ui, |ui| match desk.section {
        PanelSection::Blocks => match desk.selected_block {
            BlockKind::Overview => blocks::overview::show(ui, snap),
            BlockKind::Text => blocks::text::show(ui),
            BlockKind::Button => blocks::button::show(ui, &mut desk.blocks_state),
            BlockKind::Code => blocks::code::show(ui, snap),
            BlockKind::Status => blocks::status::show(ui, snap),
            BlockKind::Budget => blocks::budget::show(ui),
            BlockKind::Table => blocks::table::show(ui, &mut desk.blocks_state, snap),
            BlockKind::Tree => blocks::tree::show(ui, &mut desk.blocks_state, snap),
            BlockKind::Tabs => blocks::tabs::show(ui, &mut desk.blocks_state),
            BlockKind::Terminal => blocks::terminal::show(ui, &mut desk.blocks_state, live),
            BlockKind::Layout => blocks::layout::show(ui),
            BlockKind::Oscilloscope => blocks::oscilloscope::show(ui, &mut desk.blocks_state),
        },
        PanelSection::Canon => {
            canon_demos::show(ui, desk.selected_canon, &mut desk.canon_state);
        }
        PanelSection::Apps => render_selected_app(ui, desk, live, snap),
    });
}

/// Render whichever app is currently selected in `desk.selected_app`.
/// Builds an `OntologySnapshot` from the live substrate and drives the
/// surface-description composer against the app's cached surface tree.
///
/// **M1.5.1a**: drains the composer's `PendingDispatch` list and
/// submits each one through the `Live` RPC bridge. This closes the
/// loop from "admin surface declares an affordance" → "user clicks
/// the primitive" → "daemon handler fires." Replies are
/// fire-and-forget for now; the substrate's next poll tick surfaces
/// the result (e.g. the killed PID disappears from `kernel.ps`).
#[allow(dead_code)] // wired up by `apps/admin.rs` (WEFT-589) graduation
pub(crate) fn render_selected_app(
    ui: &mut egui::Ui,
    desk: &Desktop,
    live: &std::sync::Arc<crate::live::Live>,
    snap: &crate::live::Snapshot,
) {
    let Some(app_id) = desk.selected_app.as_deref() else {
        ui.label(
            egui::RichText::new("Select an app from the list")
                .italics()
                .color(egui::Color32::GRAY),
        );
        return;
    };
    let Some(tree) = desk.app_surfaces.get(app_id) else {
        ui.colored_label(
            egui::Color32::from_rgb(220, 160, 60),
            format!("No surface description loaded for {app_id}"),
        );
        return;
    };

    if let Some(installed) = desk.app_registry.get(app_id) {
        ui.horizontal(|ui| {
            ui.heading(&installed.manifest.name);
            ui.separator();
            ui.monospace(&installed.manifest.id);
        });
    }

    // Offline banner — before the composer runs so it's always visible
    // at the top of the app pane, not buried under the 2x2 grid. The
    // admin surface binds to `substrate/kernel/*` topics; when the
    // daemon link is down every binding resolves to empty and the
    // panel would otherwise look silently broken.
    match snap.connection {
        crate::live::Connection::Connected => {}
        crate::live::Connection::Connecting => {
            ui.colored_label(
                egui::Color32::from_rgb(220, 180, 60),
                "⏳ Connecting to kernel daemon…",
            );
        }
        crate::live::Connection::Disconnected => {
            ui.colored_label(
                egui::Color32::from_rgb(240, 150, 150),
                "◉ Demo mode — kernel daemon offline. \
                 Start with:  weaver kernel start",
            );
        }
    }
    ui.separator();

    // Compose against the current substrate snapshot, then dispatch
    // any affordance activations through the RPC bridge.
    let snapshot = live.substrate_snapshot();
    let outcome = surface_host::compose(tree, &snapshot, ui);
    for d in outcome.dispatches {
        log::info!(
            "admin app affordance: {} -> {} ({:?}) from {}",
            d.affordance,
            d.verb,
            d.params,
            d.source_path
        );
        live.submit(crate::live::Command::Raw {
            method: d.verb,
            params: d.params,
            reply: None,
        });
    }
}
