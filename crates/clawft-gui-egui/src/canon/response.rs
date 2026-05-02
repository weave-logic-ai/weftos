//! `CanonResponse` — `egui::Response` wrapper that carries the four
//! return-signal quantities (topology, doppler, range, bearing) from
//! session-5 §"Return-signal schema".
//!
//! Signals are *sampled*, not inferred. A primitive's `show()` builds
//! a `CanonResponse` from the underlying `egui::Response` plus its own
//! head; the caller (end-of-frame observation walker) is responsible
//! for pushing these onto the observation stream — this module does
//! not perform I/O.

use eframe::egui;

use super::types::{Affordance, IdentityUri, VariantId};

/// Where the primitive was touched and where it sat in its containing
/// layout. Reconstructed from `Response.rect` + the layer hierarchy
/// (`ctx.memory(|m| m.layer_id_at(pos))`). Path is the layout breadcrumb
/// (Stack/Grid/Strip/Dock ids leading to this primitive) — opaque to
/// the receiver; ECC uses it for co-location attribution.
#[derive(Clone, Debug, Default)]
pub struct Topology {
    pub id: Option<egui::Id>,
    pub rect: Option<egui::Rect>,
    /// Opaque layout breadcrumb. Empty = top-level.
    pub path: Vec<egui::Id>,
}

/// Motion signal — drag delta + pointer velocity + inter-press velocity.
/// Emitted as signed per-axis deltas rather than a single scalar so
/// directionality survives to ECC.
#[derive(Copy, Clone, Debug, Default)]
pub struct Doppler {
    pub drag: egui::Vec2,
    pub pointer: egui::Vec2,
    /// Seconds since the last click on this id, `None` if never clicked
    /// or if memory.data had no prior value.
    pub since_last_click_s: Option<f32>,
}

/// Latency signal — the delta between the frame the primitive first
/// appeared (`memory.data.get_persisted_mut_or`) and the frame the
/// user acted, in milliseconds. `None` when the primitive was not
/// acted upon this frame.
#[derive(Copy, Clone, Debug, Default)]
pub struct Range {
    pub first_seen_ms: Option<f64>,
    pub acted_ms: Option<f64>,
}

impl Range {
    pub fn latency_ms(&self) -> Option<f64> {
        match (self.first_seen_ms, self.acted_ms) {
            (Some(a), Some(b)) => Some(b - a),
            _ => None,
        }
    }
}

/// Which affordance out of the declared set was chosen — encoded as
/// the affordance name (stable across variants that reorder the list)
/// rather than a positional index.
#[derive(Clone, Debug, Default)]
pub struct Bearing {
    pub affordance: Option<std::borrow::Cow<'static, str>>,
}

/// Full return-signal envelope. Wraps the raw `egui::Response` so
/// callers retain full access, and adds the four reconstructable
/// quantities.
#[derive(Clone, Debug)]
pub struct CanonResponse {
    pub inner: egui::Response,
    pub identity: IdentityUri,
    pub variant: VariantId,
    pub topology: Topology,
    pub doppler: Doppler,
    pub range: Range,
    pub bearing: Bearing,
}

impl CanonResponse {
    /// Build a response from the underlying `egui::Response`, the
    /// primitive's IRI, its variant-id, and (optionally) the name of
    /// the affordance that fired this frame. This is the canonical
    /// construction path every primitive should use.
    pub fn from_egui(
        inner: egui::Response,
        identity: IdentityUri,
        variant: VariantId,
        chosen: Option<&'static str>,
    ) -> Self {
        let id = inner.id;
        let rect = inner.rect;
        let ctx = inner.ctx.clone();

        let topology = Topology {
            id: Some(id),
            rect: Some(rect),
            path: Vec::new(),
        };

        // `Response::drag_delta()` internally calls `ctx.input(...)`
        // to read the pointer delta on the active-drag path. If we call
        // it from *inside* a `ctx.input(|i| ...)` closure, we nest two
        // read-locks on egui's Context RwLock on the same thread.
        // `std::sync::RwLock::read()` recursion is documented as
        // platform-dependent: Linux (futex) typically permits it, but
        // Windows (SRWLock-backed) **deadlocks**. Symptom: the UI
        // thread freezes the moment a drag begins, no panic, no log.
        //
        // Observed end-to-end by the vector-synth-gui spike
        // (https://github.com/ … /vector-synth) on a Windows release
        // build of a canon `Slider` wired to a drag-able numeric
        // control. Read the drag delta BEFORE entering the closure to
        // avoid the reentrancy regardless of platform.
        let drag = inner.drag_delta();
        let pointer = ctx.input(|i| i.pointer.delta());
        let doppler = Doppler {
            drag,
            pointer,
            since_last_click_s: None,
        };

        let now_ms = ctx.input(|i| i.time) * 1000.0;
        let first_seen_ms = {
            let key = egui::Id::new(("canon.first_seen", id));
            ctx.memory_mut(|m| {
                let v = m.data.get_persisted_mut_or_insert_with::<f64>(key, || now_ms);
                *v
            })
        };
        let acted_ms = if inner.clicked() || inner.changed() || inner.drag_stopped() {
            Some(now_ms)
        } else {
            None
        };
        let range = Range {
            first_seen_ms: Some(first_seen_ms),
            acted_ms,
        };

        let bearing = Bearing {
            affordance: chosen.map(std::borrow::Cow::Borrowed),
        };

        Self {
            inner,
            identity,
            variant,
            topology,
            doppler,
            range,
            bearing,
        }
    }

    /// Did any affordance fire this frame? Shortcut for observation
    /// walkers that only want to serialise acted-upon primitives.
    pub fn acted(&self) -> bool {
        self.bearing.affordance.is_some() || self.range.acted_ms.is_some()
    }

    /// Stamp the chosen affordance post-hoc. Used by primitives whose
    /// affordance choice isn't known until after the underlying egui
    /// response is constructed (e.g. a `Stack` of buttons where the
    /// winning one was decided in the child loop).
    pub fn with_chosen_affordance(mut self, aff: &Affordance) -> Self {
        self.bearing.affordance = Some(aff.name.clone());
        self
    }

    /// Override the topology id. The constructor uses
    /// `inner.id`, but some primitives allocate their egui response
    /// against an auto-id and want to report their canonical
    /// ontology-keyed id instead.
    pub fn with_id_hint(mut self, id: egui::Id) -> Self {
        self.topology.id = Some(id);
        self
    }
}
