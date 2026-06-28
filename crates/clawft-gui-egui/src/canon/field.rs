//! `ui://field` — typed input binding primitive (ADR-001 row 3).
//!
//! Session-5 §3 lists five candidate egui surfaces (`TextEdit`,
//! `DragValue`, `DatePickerButton`, `ComboBox`, `CodeEditor`). All
//! five surfaces are now wired:
//!
//! * `Text`   — `egui::TextEdit::singleline` / `multiline`.
//! * `Number` — `egui::DragValue`.
//! * `Choice` — `egui::ComboBox`.
//! * `Date`   — `egui_extras::DatePickerButton` (jiff::civil::Date —
//!   egui_extras 0.34 moved off chrono; the original Plane
//!   spec referenced chrono::NaiveDate but the underlying
//!   widget API is now jiff). [WEFT-265]
//! * `Code`   — `egui::TextEdit::multiline` + `egui_extras::syntax_highlighting`
//!   (built-in highlighter; supports rust/cpp/python/toml
//!   without needing the heavyweight `syntect` feature).
//!   [WEFT-266]

use std::borrow::Cow;

use eframe::egui;

use super::CanonWidget;
use super::response::CanonResponse;
use super::types::{Affordance, Confidence, IdentityUri, MutationAxis, Tooltip, VariantId};

const IDENTITY: &str = "ui://field";

static AFFORDANCES_ACTIVE: &[Affordance] = &[
    Affordance {
        name: Cow::Borrowed("edit"),
        verb: Cow::Borrowed("wsp.set"),
        actors: &[],
        args_schema: None,
        reorderable: false,
    },
    Affordance {
        name: Cow::Borrowed("commit"),
        verb: Cow::Borrowed("wsp.commit"),
        actors: &[],
        args_schema: None,
        reorderable: false,
    },
];
static AFFORDANCES_DISABLED: &[Affordance] = &[];

static MUTATION_AXES: &[MutationAxis] = &[
    MutationAxis::new("placeholder"),
    MutationAxis::new("mask"),
    MutationAxis::new("validation-hint"),
];

/// Which egui surface the field should render with.
#[derive(Clone, Debug)]
pub enum FieldKind {
    Text {
        placeholder: Cow<'static, str>,
        multiline: bool,
        password: bool,
    },
    Number {
        min: f64,
        max: f64,
        step: f64,
    },
    Choice {
        options: &'static [&'static str],
    },
    /// Calendar date picker. Pairs with [`FieldValue::Date`].
    Date,
    /// Multi-line code editor with built-in syntax highlighting.
    /// `language` is fed to `egui_extras::syntax_highlighting::highlight`
    /// — known values without the optional `syntect` feature are
    /// `rs`/`rust`, `c`/`cpp`/`c++`/`h`/`hpp`, `py`/`python`, `toml`.
    /// Unknown languages render as plain monospace.
    Code {
        language: Cow<'static, str>,
    },
}

impl FieldKind {
    pub fn text(placeholder: impl Into<Cow<'static, str>>) -> Self {
        Self::Text {
            placeholder: placeholder.into(),
            multiline: false,
            password: false,
        }
    }

    pub fn multiline(placeholder: impl Into<Cow<'static, str>>) -> Self {
        Self::Text {
            placeholder: placeholder.into(),
            multiline: true,
            password: false,
        }
    }

    pub fn password(placeholder: impl Into<Cow<'static, str>>) -> Self {
        Self::Text {
            placeholder: placeholder.into(),
            multiline: false,
            password: true,
        }
    }

    pub fn number(min: f64, max: f64, step: f64) -> Self {
        Self::Number { min, max, step }
    }

    pub fn choice(options: &'static [&'static str]) -> Self {
        Self::Choice { options }
    }

    pub fn date() -> Self {
        Self::Date
    }

    pub fn code(language: impl Into<Cow<'static, str>>) -> Self {
        Self::Code {
            language: language.into(),
        }
    }
}

/// Bound value that the caller threads through as a `&mut`. Enum
/// variants must line up with the `FieldKind` passed — mismatches
/// render a warning label rather than panicking.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    Text(String),
    Number(f64),
    Choice(usize),
    /// Calendar date. Uses `jiff::civil::Date` because that is the
    /// shape `egui_extras::DatePickerButton` consumes in 0.34.
    Date(jiff::civil::Date),
    /// Multi-line source code buffer. `lang` is the syntax-highlighter
    /// language hint (see [`FieldKind::Code::language`]); `src` is the
    /// edit buffer.
    Code {
        lang: String,
        src: String,
    },
}

impl FieldValue {
    pub const fn as_kind_tag(&self) -> &'static str {
        match self {
            Self::Text(_) => "Text",
            Self::Number(_) => "Number",
            Self::Choice(_) => "Choice",
            Self::Date(_) => "Date",
            Self::Code { .. } => "Code",
        }
    }
}

/// Typed input binding. Caller passes a `FieldKind` describing how to
/// render and a `&mut FieldValue` holding the current bound value.
pub struct Field<'b> {
    id_source: egui::Id,
    kind: FieldKind,
    value: &'b mut FieldValue,
    enabled: bool,
    tooltip: Option<Tooltip>,
    variant: VariantId,
}

impl<'b> Field<'b> {
    pub fn new(
        id_source: impl std::hash::Hash,
        kind: FieldKind,
        value: &'b mut FieldValue,
    ) -> Self {
        Self {
            id_source: egui::Id::new(("canon.field", id_source)),
            kind,
            value,
            enabled: true,
            tooltip: None,
            variant: 0,
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn tooltip(mut self, text: impl Into<Tooltip>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    pub fn variant(mut self, variant: VariantId) -> Self {
        self.variant = variant;
        self
    }
}

impl CanonWidget for Field<'_> {
    fn id(&self) -> egui::Id {
        self.id_source
    }

    fn identity_uri(&self) -> IdentityUri {
        Cow::Borrowed(IDENTITY)
    }

    fn affordances(&self) -> &[Affordance] {
        if self.enabled {
            AFFORDANCES_ACTIVE
        } else {
            AFFORDANCES_DISABLED
        }
    }

    fn confidence(&self) -> Confidence {
        Confidence::input()
    }

    fn variant_id(&self) -> VariantId {
        self.variant
    }

    fn mutation_axes(&self) -> &[MutationAxis] {
        MUTATION_AXES
    }

    fn tooltip(&self) -> Option<&Tooltip> {
        self.tooltip.as_ref()
    }

    fn show(self, ui: &mut egui::Ui) -> CanonResponse {
        let id = self.id_source;
        let variant = self.variant;
        let enabled = self.enabled;
        let tooltip = self.tooltip.clone();
        let kind = self.kind;
        let value = self.value;

        let (mut resp, chosen_affordance) = ui
            .scope(|ui| {
                if !enabled {
                    ui.disable();
                }
                match (&kind, &mut *value) {
                    (
                        FieldKind::Text {
                            placeholder,
                            multiline,
                            password,
                        },
                        FieldValue::Text(s),
                    ) => {
                        let mut edit = if *multiline {
                            egui::TextEdit::multiline(s)
                        } else {
                            egui::TextEdit::singleline(s)
                        };
                        edit = edit.hint_text(placeholder.as_ref()).password(*password);
                        let r = ui.add(edit);
                        let chosen: Option<&'static str> = if !enabled {
                            None
                        } else if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            Some("commit")
                        } else if r.changed() {
                            Some("edit")
                        } else {
                            None
                        };
                        (r, chosen)
                    }
                    (FieldKind::Number { min, max, step }, FieldValue::Number(n)) => {
                        let r = ui.add(egui::DragValue::new(n).range(*min..=*max).speed(*step));
                        let chosen: Option<&'static str> = if enabled && r.changed() {
                            Some("edit")
                        } else {
                            None
                        };
                        (r, chosen)
                    }
                    (FieldKind::Choice { options }, FieldValue::Choice(idx)) => {
                        let current = options.get(*idx).copied().unwrap_or("");
                        let mut changed = false;
                        let combo = egui::ComboBox::from_id_salt(id)
                            .selected_text(current)
                            .show_ui(ui, |ui| {
                                for (i, opt) in options.iter().enumerate() {
                                    if ui.selectable_label(*idx == i, *opt).clicked() && *idx != i {
                                        *idx = i;
                                        changed = true;
                                    }
                                }
                            });
                        let chosen: Option<&'static str> = if enabled && changed {
                            Some("edit")
                        } else {
                            None
                        };
                        (combo.response, chosen)
                    }
                    (FieldKind::Date, FieldValue::Date(date)) => {
                        // egui_extras 0.34 DatePickerButton needs a
                        // stable id-salt distinct per Field instance —
                        // reuse the canon Field id rendered as a
                        // string so the popup state survives across
                        // frames (id_salt takes &str, not Hash).
                        let salt = format!("{:?}", id);
                        let r = ui.add(egui_extras::DatePickerButton::new(date).id_salt(&salt));
                        let chosen: Option<&'static str> = if enabled && r.changed() {
                            Some("edit")
                        } else {
                            None
                        };
                        (r, chosen)
                    }
                    (FieldKind::Code { language }, FieldValue::Code { lang, src }) => {
                        // Effective language: prefer the FieldKind
                        // hint (the schema-declared shape) but fall
                        // back to the value-bound `lang` if the kind
                        // didn't pin one.
                        let effective_lang = if !language.is_empty() {
                            language.as_ref()
                        } else {
                            lang.as_str()
                        };
                        let mut layouter =
                            |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                                let mut layout_job = egui_extras::syntax_highlighting::highlight(
                                    ui.ctx(),
                                    ui.style(),
                                    &egui_extras::syntax_highlighting::CodeTheme::from_style(
                                        ui.style(),
                                    ),
                                    buf.as_str(),
                                    effective_lang,
                                );
                                layout_job.wrap.max_width = wrap_width;
                                ui.ctx().fonts_mut(|f| f.layout_job(layout_job))
                            };
                        let r = ui.add(
                            egui::TextEdit::multiline(src)
                                .font(egui::TextStyle::Monospace)
                                .code_editor()
                                .desired_rows(4)
                                .layouter(&mut layouter),
                        );
                        let chosen: Option<&'static str> = if !enabled {
                            None
                        } else if r.lost_focus()
                            && ui.input(|i| {
                                i.modifiers.command_only() && i.key_pressed(egui::Key::Enter)
                            })
                        {
                            // Cmd/Ctrl-Enter commits a code field; bare
                            // Enter should insert a newline (multiline).
                            Some("commit")
                        } else if r.changed() {
                            Some("edit")
                        } else {
                            None
                        };
                        (r, chosen)
                    }
                    _ => {
                        let r = ui.label(format!(
                            "[canon::field] kind/value mismatch — value is {}",
                            value.as_kind_tag()
                        ));
                        (r, None)
                    }
                }
            })
            .inner;

        if let Some(tt) = &tooltip {
            resp = resp.on_hover_text(tt.as_ref());
        }

        CanonResponse::from_egui(resp, Cow::Borrowed(IDENTITY), variant, chosen_affordance)
            .with_id_hint(id)
    }
}
