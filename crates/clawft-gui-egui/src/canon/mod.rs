//! Primitive canon — the 21-item vocabulary every renderer speaks.
//!
//! Frozen by ADR-001 (amended 2026-04-20 to add `ui://tabs` as row 21).
//! Each primitive implements [`CanonWidget`] and returns a
//! [`CanonResponse`] that carries the four return-signals
//! (topology / doppler / range / bearing) from session-5.
//!
//! Head fields (ADR-006) are exposed via the trait methods so the
//! kernel boundary can validate frames without reaching into the
//! widget's internal state.

pub mod canvas;
pub mod chip;
pub mod dock;
pub mod field;
pub mod gauge;
pub mod grid;
pub mod media;
pub mod modal;
pub mod plot;
pub mod pressable;
pub mod response;
pub mod select;
pub mod sheet;
pub mod slider;
pub mod stack;
pub mod stream_view;
pub mod strip;
pub mod table;
pub mod tabs;
pub mod toggle;
pub mod tree;
pub mod types;

pub use canvas::{Canvas, CanvasTransform};
pub use chip::{Chip, ChipTone};
pub use dock::Dock;
pub use field::{Field, FieldKind, FieldValue};
pub use gauge::{Gauge, Thresholds};
pub use grid::Grid;
pub use media::{Media, MediaFit};
pub use modal::Modal;
pub use plot::Plot;
pub use pressable::Pressable;
pub use response::{Bearing, CanonResponse, Doppler, Range, Topology};
pub use select::Select;
pub use sheet::Sheet;
pub use slider::Slider;
pub use stack::{Stack, StackAxis};
pub use stream_view::StreamView;
pub use strip::{CellSize, Strip, StripAxis};
pub use table::{Table, TableColumn, TableOutcome};
pub use tabs::Tabs;
pub use toggle::{Toggle, ToggleStyle};
pub use tree::{Tree, TreeNode, TreeOutcome};
pub use types::{
    ActorKind, Affordance, Confidence, ConfidenceSource, FrozenBy, IdentityUri, Modality,
    MutationAxis, Tooltip, VariantId,
};

#[cfg(test)]
mod tests {
    //! Head-metadata smoke tests. Each primitive must expose its IRI,
    //! at least one affordance when enabled (or an empty slice when
    //! it's read-only / pure container), and its declared mutation
    //! axes. These assertions guard the kernel-boundary invariants
    //! (ADR-006) so drift in the head fields fails loudly here, not
    //! downstream in the frame codec.

    use super::*;

    #[test]
    fn identity_uris_are_stable() {
        // Sanity: every primitive's static IRI matches `ui://<name>`.
        let pressable = Pressable::new("p", "press");
        assert_eq!(pressable.identity_uri().as_ref(), "ui://pressable");
    }

    #[test]
    fn toggle_affordances_toggle_on_disable() {
        let mut v = false;
        let enabled = Toggle::new("t", "T", &mut v);
        assert_eq!(enabled.affordances().len(), 1);
        let mut v = false;
        let disabled = Toggle::new("t", "T", &mut v).enabled(false);
        assert!(disabled.affordances().is_empty());
    }

    #[test]
    fn modal_mutation_axes_are_frozen_on_modal_modality() {
        // ADR-014 + foundations §active-radar loop — consent flows
        // declare zero mutation axes.
        let m = Modal::new("m", Modality::Modal, "Confirm", |_ui: &mut eframe::egui::Ui| {});
        assert!(m.mutation_axes().is_empty());

        let f = Modal::new(
            "m",
            Modality::Floating,
            "Inspector",
            |_ui: &mut eframe::egui::Ui| {},
        );
        assert!(!f.mutation_axes().is_empty());
    }

    #[test]
    fn canvas_transform_defaults_identity() {
        let t = CanvasTransform::default();
        assert_eq!(t.scale, 1.0);
        assert_eq!(t.offset, eframe::egui::Vec2::ZERO);
    }

    #[test]
    fn field_value_kind_tags_are_stable() {
        assert_eq!(FieldValue::Text(String::new()).as_kind_tag(), "Text");
        assert_eq!(FieldValue::Number(0.0).as_kind_tag(), "Number");
        assert_eq!(FieldValue::Choice(0).as_kind_tag(), "Choice");
        // [WEFT-265 / WEFT-266] Date + Code variants land on
        // FieldValue alongside the original three.
        let d = FieldValue::Date(jiff::civil::Date::new(2026, 1, 1).unwrap());
        assert_eq!(d.as_kind_tag(), "Date");
        let c = FieldValue::Code {
            lang: "rust".into(),
            src: String::new(),
        };
        assert_eq!(c.as_kind_tag(), "Code");
    }

    #[test]
    fn field_kind_constructors_match_values() {
        // Smoke: builder constructors produce the right kind variants
        // so a caller can pair them with a matching FieldValue.
        match FieldKind::date() {
            FieldKind::Date => {}
            other => panic!("date() must produce Date, got {other:?}"),
        }
        match FieldKind::code("rust") {
            FieldKind::Code { language } => assert_eq!(language.as_ref(), "rust"),
            other => panic!("code() must produce Code, got {other:?}"),
        }
    }

    #[test]
    fn select_crosses_over_to_table_form_at_threshold() {
        // [WEFT-267] Default threshold flips Select to TableBuilder
        // form once the option count grows past a typical screen.
        // 12-option Select stays as ComboBox; 64-option Select goes
        // table.
        const SMALL: &[&str] = &["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"];
        const LARGE_LEN: usize = 64;
        const LARGE: &[&str; LARGE_LEN] = &[""; LARGE_LEN];
        let mut sel = 0usize;
        let small = Select::new("s", "", SMALL, &mut sel);
        assert!(!small.uses_table_form(), "12 options stay as ComboBox");
        let mut sel2 = 0usize;
        let large = Select::new("l", "", LARGE.as_slice(), &mut sel2);
        assert!(large.uses_table_form(), "64 options switch to table");
        // Override flips the decision both ways.
        let mut sel3 = 0usize;
        let forced_table = Select::new("ft", "", SMALL, &mut sel3).table_threshold(0);
        assert!(forced_table.uses_table_form());
        let mut sel4 = 0usize;
        let forced_combo =
            Select::new("fc", "", LARGE.as_slice(), &mut sel4).table_threshold(usize::MAX);
        assert!(!forced_combo.uses_table_form());
    }

    #[test]
    fn grid_has_no_own_affordances() {
        let g = Grid::new("g", 2, |_ui: &mut eframe::egui::Ui| {});
        assert!(g.affordances().is_empty());
    }

    #[test]
    fn tabs_exposes_switch_tab() {
        let labels = ["a", "b"];
        let mut sel = 0usize;
        let t = Tabs::new("t", &labels, &mut sel, |_ui: &mut eframe::egui::Ui, _| {});
        assert_eq!(t.affordances().len(), 1);
        assert_eq!(t.affordances()[0].name.as_ref(), "switch-tab");
    }
}

use eframe::egui;

/// Typed state payload. Primitives return whatever shape their
/// ontology IRI demands (`ui://field.value`, `ui://gauge.value`, …).
/// The kernel serialises this via `serde_json::Value` at the frame
/// boundary; renderer-side we keep it an opaque type-erased slot so
/// primitives stay zero-cost.
///
/// This is deliberately lighter than a full trait object: the renderer
/// never reads foreign primitive state, it only reads its own.
pub trait CanonState: std::fmt::Debug {}

/// Default no-state marker for primitives whose entire state is the
/// user's interaction (e.g. a bare `Pressable` with no toggle state).
#[derive(Copy, Clone, Debug, Default)]
pub struct Unit;
impl CanonState for Unit {}

/// Every primitive in the canon implements this trait. The renderer
/// calls `show()` once per frame; the six head-getter methods exist
/// so the kernel boundary (and observation walker) can interrogate
/// the primitive without forcing it to render.
///
/// See session-5-renderer-contracts.md:238-255 for the trait shape
/// and ADR-006 for field semantics.
pub trait CanonWidget {
    /// The egui id this primitive will use — stable across frames,
    /// derived from its ontology path. Used for memory-keyed state
    /// (first-seen frame, open-sets, variant echoes).
    fn id(&self) -> egui::Id;

    /// Ontology IRI stem under `ui://` (e.g. `ui://pressable`).
    fn identity_uri(&self) -> IdentityUri;

    /// Non-empty list for any primitive the user or agent may act
    /// upon. Already intersected with governance (ADR-006 §2).
    /// Empty slice = read-only *right now*, not malformed.
    fn affordances(&self) -> &[Affordance];

    /// ADR-006 §3 — how state was produced.
    fn confidence(&self) -> Confidence;

    /// Composer-assigned id, echoed on every return-signal (ADR-007).
    fn variant_id(&self) -> VariantId;

    /// Legal GEPA mutation axes. Empty slice = no mutation legal.
    fn mutation_axes(&self) -> &[MutationAxis];

    /// Optional hover / accessibility help (ADR-006 §7).
    fn tooltip(&self) -> Option<&Tooltip> {
        None
    }

    /// Render the primitive for one frame. Consumes `self` because
    /// most primitives carry their bound state by value and we don't
    /// want the caller to hold onto them across frames.
    fn show(self, ui: &mut egui::Ui) -> CanonResponse;
}
