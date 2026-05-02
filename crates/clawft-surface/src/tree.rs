//! Surface-description IR (ADR-016 §1 tree shape).
//!
//! Authored either via TOML (see [`crate::parse`]) or via the Rust
//! builder (see [`crate::builder`]); both produce the same
//! [`SurfaceTree`]. The composer runtime ([`crate::compose`]) walks
//! this tree every frame, resolves bindings against an
//! [`crate::substrate::OntologySnapshot`], and drives the canon
//! primitives from `clawft-gui-egui`.

use std::collections::BTreeMap;

/// Session mode axis (ADR-015 §modes, session-10 §2.1).
///
/// Re-export of the canonical manifest enum from [`clawft_app`] so the
/// surface IR and the manifest speak the same type (unified in M1.5-D).
pub use clawft_app::manifest::Mode;

/// Session input axis (ADR-019, session-10 §2.2).
///
/// Re-export of the canonical manifest enum from [`clawft_app`] so the
/// surface IR and the manifest speak the same type (unified in M1.5-D).
pub use clawft_app::manifest::Input;

/// Local helpers mirroring the `.as_str()` / `.parse()` shape the
/// TOML parser and tests in this crate relied on before the M1.5-D
/// unification. `clawft_app`'s canonical enum uses serde kebab-case
/// tokens internally, but the surface-description parser works
/// directly on raw strings and expects these helpers.
pub trait ModeExt: Sized {
    fn as_str(&self) -> &'static str;
    fn parse(s: &str) -> Option<Self>;
}

impl ModeExt for Mode {
    fn as_str(&self) -> &'static str {
        match self {
            Mode::SingleApp => "single-app",
            Mode::Desktop => "desktop",
            Mode::Ide => "ide",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "single-app" => Some(Mode::SingleApp),
            "desktop" => Some(Mode::Desktop),
            "ide" => Some(Mode::Ide),
            _ => None,
        }
    }
}

pub trait InputExt: Sized {
    fn as_str(&self) -> &'static str;
    fn parse(s: &str) -> Option<Self>;
}

impl InputExt for Input {
    fn as_str(&self) -> &'static str {
        match self {
            Input::Pointer => "pointer",
            Input::Touch => "touch",
            Input::Voice => "voice",
            Input::Hybrid => "hybrid",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "pointer" => Some(Input::Pointer),
            "touch" => Some(Input::Touch),
            "voice" => Some(Input::Voice),
            "hybrid" => Some(Input::Hybrid),
            _ => None,
        }
    }
}

/// Declared voice-invocation channel (ADR-019 amendment to ADR-006).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Invocation {
    Pointer,
    Touch,
    Voice,
    Gesture,
}

impl Invocation {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pointer" => Some(Invocation::Pointer),
            "touch" => Some(Invocation::Touch),
            "voice" => Some(Invocation::Voice),
            "gesture" => Some(Invocation::Gesture),
            _ => None,
        }
    }
}

/// The 21 typed canon IRIs from ADR-001, covering every `ui://…` the
/// surface description is allowed to mention.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IdentityIri {
    // Containers
    Stack,
    Strip,
    Grid,
    Dock,
    Modal,
    Tabs,
    Sheet,
    Tree,
    // Leaves
    Pressable,
    Chip,
    Gauge,
    Table,
    StreamView,
    Field,
    Toggle,
    Select,
    Slider,
    Plot,
    Media,
    Canvas,
    // ui://foreign (host-app boundary, ADR-001 row 21) is the only
    // canon leaf still routed through the composer's TODO fallback —
    // it needs the cross-app surface contract before it can render
    // anything honest.
    Foreign,
    // Sensor-oriented leaves (M1.5.3). Not in the ADR-001 canonical 21;
    // added as the hardware loop matured past what Plot/Canvas could
    // express declaratively. Both read a numeric array from a binding
    // and render it directly — no affordances.
    Heatmap,
    Waveform,
}

impl IdentityIri {
    pub fn as_iri(self) -> &'static str {
        match self {
            IdentityIri::Stack => "ui://stack",
            IdentityIri::Strip => "ui://strip",
            IdentityIri::Grid => "ui://grid",
            IdentityIri::Dock => "ui://dock",
            IdentityIri::Modal => "ui://modal",
            IdentityIri::Tabs => "ui://tabs",
            IdentityIri::Sheet => "ui://sheet",
            IdentityIri::Tree => "ui://tree",
            IdentityIri::Pressable => "ui://pressable",
            IdentityIri::Chip => "ui://chip",
            IdentityIri::Gauge => "ui://gauge",
            IdentityIri::Table => "ui://table",
            IdentityIri::StreamView => "ui://stream-view",
            IdentityIri::Field => "ui://field",
            IdentityIri::Toggle => "ui://toggle",
            IdentityIri::Select => "ui://select",
            IdentityIri::Slider => "ui://slider",
            IdentityIri::Plot => "ui://plot",
            IdentityIri::Media => "ui://media",
            IdentityIri::Canvas => "ui://canvas",
            IdentityIri::Foreign => "ui://foreign",
            IdentityIri::Heatmap => "ui://heatmap",
            IdentityIri::Waveform => "ui://waveform",
        }
    }

    pub fn parse(iri: &str) -> Option<Self> {
        Some(match iri {
            "ui://stack" => IdentityIri::Stack,
            "ui://strip" => IdentityIri::Strip,
            "ui://grid" => IdentityIri::Grid,
            "ui://dock" => IdentityIri::Dock,
            "ui://modal" => IdentityIri::Modal,
            "ui://tabs" => IdentityIri::Tabs,
            "ui://sheet" => IdentityIri::Sheet,
            "ui://tree" => IdentityIri::Tree,
            "ui://pressable" => IdentityIri::Pressable,
            "ui://chip" => IdentityIri::Chip,
            "ui://gauge" => IdentityIri::Gauge,
            "ui://table" => IdentityIri::Table,
            "ui://stream-view" => IdentityIri::StreamView,
            "ui://field" => IdentityIri::Field,
            "ui://toggle" => IdentityIri::Toggle,
            "ui://select" => IdentityIri::Select,
            "ui://slider" => IdentityIri::Slider,
            "ui://plot" => IdentityIri::Plot,
            "ui://media" => IdentityIri::Media,
            "ui://canvas" => IdentityIri::Canvas,
            "ui://foreign" => IdentityIri::Foreign,
            "ui://heatmap" => IdentityIri::Heatmap,
            "ui://waveform" => IdentityIri::Waveform,
            _ => return None,
        })
    }

    /// Containers may carry `children`; leaves must not. Used by the
    /// TOML parser to reject structural mistakes early.
    pub fn is_container(self) -> bool {
        matches!(
            self,
            IdentityIri::Stack
                | IdentityIri::Strip
                | IdentityIri::Grid
                | IdentityIri::Dock
                | IdentityIri::Modal
                | IdentityIri::Tabs
                | IdentityIri::Sheet
                | IdentityIri::Tree
        )
    }
}

/// Static attribute values authored in the surface description. No
/// expressions — those live in [`Binding`]. Kept small on purpose.
#[derive(Clone, Debug, PartialEq)]
pub enum AttrValue {
    Bool(bool),
    Number(f64),
    Int(i64),
    Str(String),
    Array(Vec<AttrValue>),
}

impl AttrValue {
    pub fn as_str(&self) -> Option<&str> {
        if let AttrValue::Str(s) = self {
            Some(s)
        } else {
            None
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            AttrValue::Number(n) => Some(*n),
            AttrValue::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            AttrValue::Int(i) => Some(*i),
            AttrValue::Number(n) => Some(*n as i64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        if let AttrValue::Bool(b) = self {
            Some(*b)
        } else {
            None
        }
    }

    pub fn as_array(&self) -> Option<&[AttrValue]> {
        if let AttrValue::Array(v) = self {
            Some(v)
        } else {
            None
        }
    }
}

/// A bound slot on a primitive: either a literal baked in at parse
/// time or an expression evaluated every frame.
#[derive(Clone, Debug)]
pub enum Binding {
    Literal(AttrValue),
    Expr(crate::parse::expr::Expr),
}

/// ADR-006 §2 + ADR-019 extension. Affordances on a node declare
/// which WSP verb the caller may invoke and on which input channels.
///
/// For M1.5 the composer passes these through unfiltered — real
/// governance intersection lands in M1.6+. (See `compose.rs` TODO.)
#[derive(Clone, Debug)]
pub struct AffordanceDecl {
    pub name: String,
    pub verb: String,
    pub invocations: Vec<Invocation>,
    pub args_schema: Option<String>,
}

/// One node in the primitive tree. Structural identity (`kind`,
/// `path`) + behavioural contract (`bindings`, `affordances`) +
/// presentation (`attrs`) + layout (`children`) + conditional
/// (`when`). Head fields from ADR-006 (`confidence`, `variant`,
/// `mutation-axes`, `privacy`) are filled at compose time and are
/// *not* authored here (ADR-016 §1 note).
#[derive(Clone, Debug)]
pub struct SurfaceNode {
    pub kind: IdentityIri,
    pub path: String,
    pub bindings: BTreeMap<String, Binding>,
    pub affordances: Vec<AffordanceDecl>,
    pub attrs: BTreeMap<String, AttrValue>,
    pub children: Vec<SurfaceNode>,
    pub when: Option<Binding>,
}

impl SurfaceNode {
    pub fn new(kind: IdentityIri, path: impl Into<String>) -> Self {
        Self {
            kind,
            path: path.into(),
            bindings: BTreeMap::new(),
            affordances: Vec::new(),
            attrs: BTreeMap::new(),
            children: Vec::new(),
            when: None,
        }
    }
}

/// One complete surface description variant — an id, its mode/input
/// targeting, optional title, subscription paths, and the root node
/// (ADR-016 §9).
#[derive(Clone, Debug)]
pub struct SurfaceTree {
    pub id: String,
    pub modes: Vec<Mode>,
    pub inputs: Vec<Input>,
    pub title: Option<String>,
    pub subscriptions: Vec<String>,
    pub root: SurfaceNode,
}

impl SurfaceTree {
    pub fn new(id: impl Into<String>, root: SurfaceNode) -> Self {
        Self {
            id: id.into(),
            modes: Vec::new(),
            inputs: Vec::new(),
            title: None,
            subscriptions: Vec::new(),
            root,
        }
    }

    /// Depth-first count of descendants (plus root) matching an IRI.
    /// Used by integration tests to assert primitive counts.
    pub fn count_of(&self, iri: &str) -> usize {
        fn walk(n: &SurfaceNode, iri: &str, acc: &mut usize) {
            if n.kind.as_iri() == iri {
                *acc += 1;
            }
            for c in &n.children {
                walk(c, iri, acc);
            }
        }
        let mut n = 0usize;
        walk(&self.root, iri, &mut n);
        n
    }

    /// True iff any node exposes an affordance with the given verb.
    pub fn any_affordance_with_verb(&self, verb: &str) -> bool {
        fn walk(n: &SurfaceNode, verb: &str) -> bool {
            if n.affordances.iter().any(|a| a.verb == verb) {
                return true;
            }
            n.children.iter().any(|c| walk(c, verb))
        }
        walk(&self.root, verb)
    }
}
