//! Canvas protocol types for the Live Canvas system.
//!
//! Defines the protocol for agents to render UI elements on a canvas,
//! receive interaction events, and manage canvas state. These types
//! are serialized over WebSocket for real-time UI updates.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Unique identifier for a canvas element.
pub type ElementId = String;

/// Unique identifier for a canvas instance.
pub type CanvasId = String;

// ── Default value helpers ─────────────────────────────────────────

fn default_text_format() -> String {
    "plain".into()
}

fn default_field_type() -> String {
    "text".into()
}

fn default_chart_type() -> String {
    "bar".into()
}

fn default_true() -> bool {
    true
}

// ── Canvas elements ───────────────────────────────────────────────

/// UI element types that agents can render on the canvas.
///
/// Each variant represents a different UI primitive. The `type` field
/// is used as the serde tag for JSON serialization.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CanvasElement {
    /// A text block with optional formatting.
    Text {
        content: String,
        #[serde(default = "default_text_format")]
        format: String,
    },
    /// A clickable button that triggers an action.
    Button {
        label: String,
        action: String,
        #[serde(default)]
        disabled: bool,
    },
    /// A text input field.
    Input {
        label: String,
        #[serde(default)]
        placeholder: String,
        #[serde(default)]
        value: String,
    },
    /// An image element.
    Image {
        src: String,
        #[serde(default)]
        alt: String,
    },
    /// A code block with optional syntax highlighting.
    Code {
        code: String,
        #[serde(default)]
        language: String,
    },
    /// A data table with headers and rows.
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// A form with multiple fields and a submit action.
    Form {
        fields: Vec<FormField>,
        submit_action: String,
    },
    /// A chart element for data visualization.
    Chart {
        data: Vec<ChartDataPoint>,
        #[serde(default = "default_chart_type")]
        chart_type: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        colors: Option<Vec<String>>,
    },
    /// A code editor element with optional editing and line numbers.
    CodeEditor {
        code: String,
        #[serde(default)]
        language: String,
        #[serde(default)]
        editable: bool,
        #[serde(default = "default_true")]
        line_numbers: bool,
    },
    /// An advanced form with typed fields and validation.
    FormAdvanced {
        fields: Vec<AdvancedFormField>,
        #[serde(default)]
        submit_action: Option<String>,
    },
}

/// A data point for chart elements.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChartDataPoint {
    /// Label for this data point (x-axis or slice label).
    pub label: String,
    /// Numeric value for this data point.
    pub value: f64,
}

/// A field within a [`CanvasElement::Form`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FormField {
    /// Machine-readable field name (used as the key in form submission).
    pub name: String,
    /// Human-readable label shown to the user.
    pub label: String,
    /// The HTML input type (e.g., "text", "email", "number").
    #[serde(default = "default_field_type")]
    pub field_type: String,
    /// Whether this field must be filled before submission.
    #[serde(default)]
    pub required: bool,
    /// Placeholder text shown when the field is empty.
    #[serde(default)]
    pub placeholder: Option<String>,
}

/// A field within a [`CanvasElement::FormAdvanced`] with richer type info.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdvancedFormField {
    /// Machine-readable field name.
    pub name: String,
    /// The field type: "text", "number", "select", "checkbox", "textarea".
    #[serde(default = "default_field_type")]
    pub field_type: String,
    /// Human-readable label.
    pub label: String,
    /// Whether this field is required.
    #[serde(default)]
    pub required: bool,
    /// Options for select fields.
    #[serde(default)]
    pub options: Option<Vec<String>>,
    /// Minimum value for number fields.
    #[serde(default)]
    pub min: Option<f64>,
    /// Maximum value for number fields.
    #[serde(default)]
    pub max: Option<f64>,
    /// Placeholder text.
    #[serde(default)]
    pub placeholder: Option<String>,
}

// ── Canvas commands ───────────────────────────────────────────────

/// Commands from agents to the canvas.
///
/// These are sent over WebSocket to create, update, remove, or batch
/// operations on canvas elements.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum CanvasCommand {
    /// Render a new element on the canvas.
    Render {
        id: ElementId,
        element: CanvasElement,
        #[serde(default)]
        position: Option<u32>,
    },
    /// Update an existing element on the canvas.
    Update {
        id: ElementId,
        element: CanvasElement,
    },
    /// Remove an element from the canvas.
    Remove { id: ElementId },
    /// Clear all elements from the canvas.
    Reset,
    /// Execute multiple commands atomically.
    Batch { commands: Vec<CanvasCommand> },
}

// ── Canvas interactions ───────────────────────────────────────────

/// Interaction events from the canvas back to agents.
///
/// These are generated by user interaction with canvas elements
/// and sent back to the agent via WebSocket.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "interaction", rename_all = "snake_case")]
pub enum CanvasInteraction {
    /// User clicked a button element.
    Click {
        element_id: ElementId,
        action: String,
    },
    /// User submitted an input field.
    InputSubmit {
        element_id: ElementId,
        value: String,
    },
    /// User submitted a form.
    FormSubmit {
        element_id: ElementId,
        values: HashMap<String, String>,
    },
    /// User submitted code from a code editor.
    CodeSubmit {
        element_id: ElementId,
        code: String,
        language: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── CanvasElement serialization ──────────────────────────────

    #[test]
    fn serialize_text_element() {
        let elem = CanvasElement::Text {
            content: "Hello, world!".into(),
            format: "markdown".into(),
        };
        let json = serde_json::to_value(&elem).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["content"], "Hello, world!");
        assert_eq!(json["format"], "markdown");
    }

    #[test]
    fn deserialize_text_element_with_default_format() {
        let json = serde_json::json!({ "type": "text", "content": "hi" });
        let elem: CanvasElement = serde_json::from_value(json).unwrap();
        match elem {
            CanvasElement::Text { content, format } => {
                assert_eq!(content, "hi");
                assert_eq!(format, "plain");
            }
            other => panic!("expected Text, got: {other:?}"),
        }
    }

    #[test]
    fn serialize_button_element() {
        let elem = CanvasElement::Button {
            label: "Click me".into(),
            action: "do_thing".into(),
            disabled: false,
        };
        let json = serde_json::to_value(&elem).unwrap();
        assert_eq!(json["type"], "button");
        assert_eq!(json["label"], "Click me");
        assert_eq!(json["action"], "do_thing");
        assert_eq!(json["disabled"], false);
    }

    #[test]
    fn deserialize_button_element_default_disabled() {
        let json = serde_json::json!({
            "type": "button",
            "label": "Go",
            "action": "run"
        });
        let elem: CanvasElement = serde_json::from_value(json).unwrap();
        match elem {
            CanvasElement::Button { disabled, .. } => assert!(!disabled),
            other => panic!("expected Button, got: {other:?}"),
        }
    }

    #[test]
    fn serialize_input_element() {
        let elem = CanvasElement::Input {
            label: "Name".into(),
            placeholder: "Enter name".into(),
            value: "".into(),
        };
        let json = serde_json::to_value(&elem).unwrap();
        assert_eq!(json["type"], "input");
        assert_eq!(json["label"], "Name");
        assert_eq!(json["placeholder"], "Enter name");
    }

    #[test]
    fn deserialize_input_element_defaults() {
        let json = serde_json::json!({ "type": "input", "label": "Email" });
        let elem: CanvasElement = serde_json::from_value(json).unwrap();
        match elem {
            CanvasElement::Input {
                label,
                placeholder,
                value,
            } => {
                assert_eq!(label, "Email");
                assert_eq!(placeholder, "");
                assert_eq!(value, "");
            }
            other => panic!("expected Input, got: {other:?}"),
        }
    }

    #[test]
    fn serialize_image_element() {
        let elem = CanvasElement::Image {
            src: "https://example.com/img.png".into(),
            alt: "Logo".into(),
        };
        let json = serde_json::to_value(&elem).unwrap();
        assert_eq!(json["type"], "image");
        assert_eq!(json["src"], "https://example.com/img.png");
        assert_eq!(json["alt"], "Logo");
    }

    #[test]
    fn deserialize_image_element_default_alt() {
        let json = serde_json::json!({ "type": "image", "src": "a.png" });
        let elem: CanvasElement = serde_json::from_value(json).unwrap();
        match elem {
            CanvasElement::Image { alt, .. } => assert_eq!(alt, ""),
            other => panic!("expected Image, got: {other:?}"),
        }
    }

    #[test]
    fn serialize_code_element() {
        let elem = CanvasElement::Code {
            code: "fn main() {}".into(),
            language: "rust".into(),
        };
        let json = serde_json::to_value(&elem).unwrap();
        assert_eq!(json["type"], "code");
        assert_eq!(json["code"], "fn main() {}");
        assert_eq!(json["language"], "rust");
    }

    #[test]
    fn deserialize_code_element_default_language() {
        let json = serde_json::json!({ "type": "code", "code": "x = 1" });
        let elem: CanvasElement = serde_json::from_value(json).unwrap();
        match elem {
            CanvasElement::Code { language, .. } => assert_eq!(language, ""),
            other => panic!("expected Code, got: {other:?}"),
        }
    }

    #[test]
    fn serialize_table_element() {
        let elem = CanvasElement::Table {
            headers: vec!["Name".into(), "Age".into()],
            rows: vec![vec!["Alice".into(), "30".into()]],
        };
        let json = serde_json::to_value(&elem).unwrap();
        assert_eq!(json["type"], "table");
        assert_eq!(json["headers"], serde_json::json!(["Name", "Age"]));
        assert_eq!(json["rows"], serde_json::json!([["Alice", "30"]]));
    }

    #[test]
    fn serialize_form_element() {
        let elem = CanvasElement::Form {
            fields: vec![FormField {
                name: "username".into(),
                label: "Username".into(),
                field_type: "text".into(),
                required: true,
                placeholder: Some("Enter username".into()),
            }],
            submit_action: "create_user".into(),
        };
        let json = serde_json::to_value(&elem).unwrap();
        assert_eq!(json["type"], "form");
        assert_eq!(json["submit_action"], "create_user");
        assert_eq!(json["fields"][0]["name"], "username");
        assert_eq!(json["fields"][0]["required"], true);
    }

    // ── FormField serialization ─────────────────────────────────

    #[test]
    fn deserialize_form_field_defaults() {
        let json = serde_json::json!({
            "name": "email",
            "label": "Email Address"
        });
        let field: FormField = serde_json::from_value(json).unwrap();
        assert_eq!(field.name, "email");
        assert_eq!(field.label, "Email Address");
        assert_eq!(field.field_type, "text");
        assert!(!field.required);
        assert!(field.placeholder.is_none());
    }

    #[test]
    fn form_field_roundtrip() {
        let field = FormField {
            name: "age".into(),
            label: "Age".into(),
            field_type: "number".into(),
            required: false,
            placeholder: Some("0".into()),
        };
        let json = serde_json::to_string(&field).unwrap();
        let restored: FormField = serde_json::from_str(&json).unwrap();
        assert_eq!(field, restored);
    }

    // ── Chart element tests ─────────────────────────────────────

    #[test]
    fn serialize_chart_element() {
        let elem = CanvasElement::Chart {
            data: vec![
                ChartDataPoint {
                    label: "Jan".into(),
                    value: 100.0,
                },
                ChartDataPoint {
                    label: "Feb".into(),
                    value: 200.0,
                },
            ],
            chart_type: "bar".into(),
            title: Some("Monthly Revenue".into()),
            colors: Some(vec!["#6366f1".into(), "#22c55e".into()]),
        };
        let json = serde_json::to_value(&elem).unwrap();
        assert_eq!(json["type"], "chart");
        assert_eq!(json["chart_type"], "bar");
        assert_eq!(json["title"], "Monthly Revenue");
        assert_eq!(json["data"].as_array().unwrap().len(), 2);
        assert_eq!(json["data"][0]["label"], "Jan");
        assert_eq!(json["data"][0]["value"], 100.0);
    }

    #[test]
    fn deserialize_chart_element_defaults() {
        let json = serde_json::json!({
            "type": "chart",
            "data": [{"label": "A", "value": 10}]
        });
        let elem: CanvasElement = serde_json::from_value(json).unwrap();
        match elem {
            CanvasElement::Chart {
                data,
                chart_type,
                title,
                colors,
            } => {
                assert_eq!(data.len(), 1);
                assert_eq!(data[0].label, "A");
                assert_eq!(data[0].value, 10.0);
                assert_eq!(chart_type, "bar");
                assert!(title.is_none());
                assert!(colors.is_none());
            }
            other => panic!("expected Chart, got: {other:?}"),
        }
    }

    #[test]
    fn chart_data_point_roundtrip() {
        let point = ChartDataPoint {
            label: "March".into(),
            value: 42.5,
        };
        let json = serde_json::to_string(&point).unwrap();
        let restored: ChartDataPoint = serde_json::from_str(&json).unwrap();
        assert_eq!(point, restored);
    }

    #[test]
    fn chart_element_pie_type() {
        let elem = CanvasElement::Chart {
            data: vec![
                ChartDataPoint {
                    label: "Desktop".into(),
                    value: 60.0,
                },
                ChartDataPoint {
                    label: "Mobile".into(),
                    value: 40.0,
                },
            ],
            chart_type: "pie".into(),
            title: Some("Device Share".into()),
            colors: None,
        };
        let json = serde_json::to_value(&elem).unwrap();
        assert_eq!(json["type"], "chart");
        assert_eq!(json["chart_type"], "pie");
        let roundtripped: CanvasElement = serde_json::from_value(json).unwrap();
        assert_eq!(elem, roundtripped);
    }

    // ── CodeEditor element tests ────────────────────────────────

    #[test]
    fn serialize_code_editor_element() {
        let elem = CanvasElement::CodeEditor {
            code: "console.log('hello')".into(),
            language: "javascript".into(),
            editable: true,
            line_numbers: true,
        };
        let json = serde_json::to_value(&elem).unwrap();
        assert_eq!(json["type"], "code_editor");
        assert_eq!(json["code"], "console.log('hello')");
        assert_eq!(json["language"], "javascript");
        assert_eq!(json["editable"], true);
        assert_eq!(json["line_numbers"], true);
    }

    #[test]
    fn deserialize_code_editor_element_defaults() {
        let json = serde_json::json!({
            "type": "code_editor",
            "code": "x = 1"
        });
        let elem: CanvasElement = serde_json::from_value(json).unwrap();
        match elem {
            CanvasElement::CodeEditor {
                code,
                language,
                editable,
                line_numbers,
            } => {
                assert_eq!(code, "x = 1");
                assert_eq!(language, "");
                assert!(!editable);
                assert!(line_numbers); // defaults to true
            }
            other => panic!("expected CodeEditor, got: {other:?}"),
        }
    }

    #[test]
    fn code_editor_roundtrip() {
        let elem = CanvasElement::CodeEditor {
            code: "fn main() {\n    println!(\"Hello\");\n}".into(),
            language: "rust".into(),
            editable: false,
            line_numbers: false,
        };
        let json = serde_json::to_string(&elem).unwrap();
        let restored: CanvasElement = serde_json::from_str(&json).unwrap();
        assert_eq!(elem, restored);
    }

    // ── FormAdvanced element tests ──────────────────────────────

    #[test]
    fn serialize_form_advanced_element() {
        let elem = CanvasElement::FormAdvanced {
            fields: vec![
                AdvancedFormField {
                    name: "name".into(),
                    field_type: "text".into(),
                    label: "Full Name".into(),
                    required: true,
                    options: None,
                    min: None,
                    max: None,
                    placeholder: Some("Enter your name".into()),
                },
                AdvancedFormField {
                    name: "age".into(),
                    field_type: "number".into(),
                    label: "Age".into(),
                    required: false,
                    options: None,
                    min: Some(0.0),
                    max: Some(150.0),
                    placeholder: None,
                },
                AdvancedFormField {
                    name: "role".into(),
                    field_type: "select".into(),
                    label: "Role".into(),
                    required: true,
                    options: Some(vec!["Admin".into(), "User".into(), "Guest".into()]),
                    min: None,
                    max: None,
                    placeholder: None,
                },
            ],
            submit_action: Some("create_user".into()),
        };
        let json = serde_json::to_value(&elem).unwrap();
        assert_eq!(json["type"], "form_advanced");
        assert_eq!(json["submit_action"], "create_user");
        assert_eq!(json["fields"].as_array().unwrap().len(), 3);
        assert_eq!(json["fields"][0]["name"], "name");
        assert_eq!(json["fields"][0]["required"], true);
        assert_eq!(json["fields"][1]["min"], 0.0);
        assert_eq!(
            json["fields"][2]["options"],
            serde_json::json!(["Admin", "User", "Guest"])
        );
    }

    #[test]
    fn deserialize_advanced_form_field_defaults() {
        let json = serde_json::json!({
            "name": "notes",
            "label": "Notes"
        });
        let field: AdvancedFormField = serde_json::from_value(json).unwrap();
        assert_eq!(field.name, "notes");
        assert_eq!(field.label, "Notes");
        assert_eq!(field.field_type, "text");
        assert!(!field.required);
        assert!(field.options.is_none());
        assert!(field.min.is_none());
        assert!(field.max.is_none());
        assert!(field.placeholder.is_none());
    }

    #[test]
    fn advanced_form_field_roundtrip() {
        let field = AdvancedFormField {
            name: "priority".into(),
            field_type: "select".into(),
            label: "Priority".into(),
            required: true,
            options: Some(vec!["Low".into(), "Medium".into(), "High".into()]),
            min: None,
            max: None,
            placeholder: Some("Select priority".into()),
        };
        let json = serde_json::to_string(&field).unwrap();
        let restored: AdvancedFormField = serde_json::from_str(&json).unwrap();
        assert_eq!(field, restored);
    }

    // ── CodeSubmit interaction tests ────────────────────────────

    #[test]
    fn serialize_code_submit_interaction() {
        let interaction = CanvasInteraction::CodeSubmit {
            element_id: "editor-1".into(),
            code: "print('done')".into(),
            language: "python".into(),
        };
        let json = serde_json::to_value(&interaction).unwrap();
        assert_eq!(json["interaction"], "code_submit");
        assert_eq!(json["element_id"], "editor-1");
        assert_eq!(json["code"], "print('done')");
        assert_eq!(json["language"], "python");
    }

    #[test]
    fn deserialize_code_submit_interaction() {
        let json = serde_json::json!({
            "interaction": "code_submit",
            "element_id": "ed-2",
            "code": "x = 1",
            "language": "python"
        });
        let interaction: CanvasInteraction = serde_json::from_value(json).unwrap();
        match interaction {
            CanvasInteraction::CodeSubmit {
                element_id,
                code,
                language,
            } => {
                assert_eq!(element_id, "ed-2");
                assert_eq!(code, "x = 1");
                assert_eq!(language, "python");
            }
            other => panic!("expected CodeSubmit, got: {other:?}"),
        }
    }

    // ── CanvasCommand serialization ─────────────────────────────

    #[test]
    fn serialize_render_command() {
        let cmd = CanvasCommand::Render {
            id: "el-1".into(),
            element: CanvasElement::Text {
                content: "Hello".into(),
                format: "plain".into(),
            },
            position: Some(0),
        };
        let json = serde_json::to_value(&cmd).unwrap();
        assert_eq!(json["command"], "render");
        assert_eq!(json["id"], "el-1");
        assert_eq!(json["element"]["type"], "text");
        assert_eq!(json["position"], 0);
    }

    #[test]
    fn deserialize_render_command_no_position() {
        let json = serde_json::json!({
            "command": "render",
            "id": "el-2",
            "element": { "type": "button", "label": "Go", "action": "run" }
        });
        let cmd: CanvasCommand = serde_json::from_value(json).unwrap();
        match cmd {
            CanvasCommand::Render { id, position, .. } => {
                assert_eq!(id, "el-2");
                assert!(position.is_none());
            }
            other => panic!("expected Render, got: {other:?}"),
        }
    }

    #[test]
    fn serialize_update_command() {
        let cmd = CanvasCommand::Update {
            id: "el-1".into(),
            element: CanvasElement::Text {
                content: "Updated".into(),
                format: "markdown".into(),
            },
        };
        let json = serde_json::to_value(&cmd).unwrap();
        assert_eq!(json["command"], "update");
        assert_eq!(json["id"], "el-1");
    }

    #[test]
    fn serialize_remove_command() {
        let cmd = CanvasCommand::Remove { id: "el-3".into() };
        let json = serde_json::to_value(&cmd).unwrap();
        assert_eq!(json["command"], "remove");
        assert_eq!(json["id"], "el-3");
    }

    #[test]
    fn serialize_reset_command() {
        let cmd = CanvasCommand::Reset;
        let json = serde_json::to_value(&cmd).unwrap();
        assert_eq!(json["command"], "reset");
    }

    #[test]
    fn serialize_batch_command() {
        let cmd = CanvasCommand::Batch {
            commands: vec![
                CanvasCommand::Reset,
                CanvasCommand::Render {
                    id: "el-1".into(),
                    element: CanvasElement::Text {
                        content: "Fresh".into(),
                        format: "plain".into(),
                    },
                    position: None,
                },
            ],
        };
        let json = serde_json::to_value(&cmd).unwrap();
        assert_eq!(json["command"], "batch");
        assert_eq!(json["commands"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn deserialize_reset_command() {
        let json = serde_json::json!({ "command": "reset" });
        let cmd: CanvasCommand = serde_json::from_value(json).unwrap();
        assert!(matches!(cmd, CanvasCommand::Reset));
    }

    // ── CanvasInteraction serialization ──────────────────────────

    #[test]
    fn serialize_click_interaction() {
        let interaction = CanvasInteraction::Click {
            element_id: "btn-1".into(),
            action: "submit".into(),
        };
        let json = serde_json::to_value(&interaction).unwrap();
        assert_eq!(json["interaction"], "click");
        assert_eq!(json["element_id"], "btn-1");
        assert_eq!(json["action"], "submit");
    }

    #[test]
    fn serialize_input_submit_interaction() {
        let interaction = CanvasInteraction::InputSubmit {
            element_id: "input-1".into(),
            value: "hello".into(),
        };
        let json = serde_json::to_value(&interaction).unwrap();
        assert_eq!(json["interaction"], "input_submit");
        assert_eq!(json["element_id"], "input-1");
        assert_eq!(json["value"], "hello");
    }

    #[test]
    fn serialize_form_submit_interaction() {
        let mut values = HashMap::new();
        values.insert("username".into(), "alice".into());
        values.insert("email".into(), "alice@example.com".into());

        let interaction = CanvasInteraction::FormSubmit {
            element_id: "form-1".into(),
            values,
        };
        let json = serde_json::to_value(&interaction).unwrap();
        assert_eq!(json["interaction"], "form_submit");
        assert_eq!(json["element_id"], "form-1");
        assert_eq!(json["values"]["username"], "alice");
        assert_eq!(json["values"]["email"], "alice@example.com");
    }

    #[test]
    fn deserialize_click_interaction() {
        let json = serde_json::json!({
            "interaction": "click",
            "element_id": "btn-x",
            "action": "delete"
        });
        let interaction: CanvasInteraction = serde_json::from_value(json).unwrap();
        match interaction {
            CanvasInteraction::Click { element_id, action } => {
                assert_eq!(element_id, "btn-x");
                assert_eq!(action, "delete");
            }
            other => panic!("expected Click, got: {other:?}"),
        }
    }

    #[test]
    fn deserialize_form_submit_interaction() {
        let json = serde_json::json!({
            "interaction": "form_submit",
            "element_id": "form-2",
            "values": { "name": "Bob" }
        });
        let interaction: CanvasInteraction = serde_json::from_value(json).unwrap();
        match interaction {
            CanvasInteraction::FormSubmit { element_id, values } => {
                assert_eq!(element_id, "form-2");
                assert_eq!(values.get("name").unwrap(), "Bob");
            }
            other => panic!("expected FormSubmit, got: {other:?}"),
        }
    }

    // ── Roundtrip tests ─────────────────────────────────────────

    #[test]
    fn canvas_element_roundtrip_all_variants() {
        let elements = vec![
            CanvasElement::Text {
                content: "test".into(),
                format: "markdown".into(),
            },
            CanvasElement::Button {
                label: "OK".into(),
                action: "confirm".into(),
                disabled: true,
            },
            CanvasElement::Input {
                label: "Query".into(),
                placeholder: "Type here".into(),
                value: "default".into(),
            },
            CanvasElement::Image {
                src: "logo.png".into(),
                alt: "Company Logo".into(),
            },
            CanvasElement::Code {
                code: "print('hi')".into(),
                language: "python".into(),
            },
            CanvasElement::Table {
                headers: vec!["Col1".into()],
                rows: vec![vec!["val".into()]],
            },
            CanvasElement::Form {
                fields: vec![FormField {
                    name: "f".into(),
                    label: "Field".into(),
                    field_type: "text".into(),
                    required: false,
                    placeholder: None,
                }],
                submit_action: "go".into(),
            },
            CanvasElement::Chart {
                data: vec![ChartDataPoint {
                    label: "Q1".into(),
                    value: 42.0,
                }],
                chart_type: "line".into(),
                title: Some("Quarterly".into()),
                colors: None,
            },
            CanvasElement::CodeEditor {
                code: "let x = 1;".into(),
                language: "typescript".into(),
                editable: true,
                line_numbers: true,
            },
            CanvasElement::FormAdvanced {
                fields: vec![AdvancedFormField {
                    name: "email".into(),
                    field_type: "text".into(),
                    label: "Email".into(),
                    required: true,
                    options: None,
                    min: None,
                    max: None,
                    placeholder: Some("you@example.com".into()),
                }],
                submit_action: Some("register".into()),
            },
        ];

        for elem in &elements {
            let json = serde_json::to_string(elem).unwrap();
            let restored: CanvasElement = serde_json::from_str(&json).unwrap();
            assert_eq!(*elem, restored);
        }
    }

    // ── Render command with new element types ───────────────────

    #[test]
    fn render_chart_command() {
        let cmd = CanvasCommand::Render {
            id: "chart-1".into(),
            element: CanvasElement::Chart {
                data: vec![
                    ChartDataPoint {
                        label: "A".into(),
                        value: 10.0,
                    },
                    ChartDataPoint {
                        label: "B".into(),
                        value: 20.0,
                    },
                ],
                chart_type: "bar".into(),
                title: None,
                colors: None,
            },
            position: None,
        };
        let json = serde_json::to_value(&cmd).unwrap();
        assert_eq!(json["command"], "render");
        assert_eq!(json["element"]["type"], "chart");
        assert_eq!(json["element"]["data"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn render_code_editor_command() {
        let cmd = CanvasCommand::Render {
            id: "editor-1".into(),
            element: CanvasElement::CodeEditor {
                code: "SELECT * FROM users;".into(),
                language: "sql".into(),
                editable: true,
                line_numbers: true,
            },
            position: Some(0),
        };
        let json = serde_json::to_value(&cmd).unwrap();
        assert_eq!(json["command"], "render");
        assert_eq!(json["element"]["type"], "code_editor");
        assert_eq!(json["element"]["editable"], true);
    }
}
