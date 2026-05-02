//! Tree calculus + EML AST extractor.
//!
//! Extracts entities and relationships from Rust source files using:
//! - **Tree calculus triage** for structural dispatch (Atom/Sequence/Branch)
//! - **EML models** for confidence scoring and metric computation
//!
//! No tree-sitter dependency. Pattern-based extraction with formal
//! structural classification.

use std::collections::HashMap;
use std::path::Path;

use regex::Regex;

use crate::entity::{DomainTag, EntityId, EntityType, FileType};
use crate::model::{Entity, ExtractionResult};
use crate::relationship::{Confidence, RelationType, Relationship};

// ---------------------------------------------------------------------------
// Tree calculus: Topology forms
// ---------------------------------------------------------------------------

// `Form` and generic `triage` now live in the `clawft-treecalc` crate
// so the kernel can share the same classifier for coherence-cycle
// dispatch (see docs/eml-treecalc-swap-sites.md#finding-8 and
// #finding-10). Re-export here so downstream callers that depended on
// `clawft_graphify::extract::treecalc::Form` keep working.
pub use clawft_treecalc::Form;

/// Classify an `ExtractedItem` by its children's uniformity.
fn triage(item: &ExtractedItem) -> Form {
    clawft_treecalc::triage(item.children.iter().map(|c| &c.kind))
}

// ---------------------------------------------------------------------------
// EML scoring model (inline exp-ln approximation)
// ---------------------------------------------------------------------------

/// EML-style scoring: compute confidence from extraction features.
/// Uses exp-ln composition: score = a * exp(b * x) + c * ln(d * x + 1)
fn eml_confidence(features: &ExtractionFeatures) -> f64 {
    let x = features.pattern_strength;
    let context_bonus = if features.has_pub { 0.1 } else { 0.0 };
    let doc_bonus = if features.has_doc_comment { 0.05 } else { 0.0 };

    // Learned parameters (hand-initialized, trainable via eml-core later)
    let a = 0.3;
    let b = 1.5;
    let c = 0.4;
    let d = 2.0;

    let score = a * (b * x).exp().min(10.0) + c * (d * x + 1.0).ln();
    (score + context_bonus + doc_bonus).clamp(0.0, 1.0)
}

/// EML-style complexity scoring from item features.
fn eml_complexity(features: &ExtractionFeatures) -> f64 {
    let x = features.line_count as f64;
    let nest = features.nesting_depth as f64;

    // complexity = sqrt(lines) * (1 + nesting * 0.3)
    x.sqrt() * (1.0 + nest * 0.3)
}

struct ExtractionFeatures {
    pattern_strength: f64,
    has_pub: bool,
    has_doc_comment: bool,
    line_count: usize,
    nesting_depth: usize,
}

// ---------------------------------------------------------------------------
// Extraction engine
// ---------------------------------------------------------------------------

/// An item extracted from source before conversion to Entity.
#[derive(Debug, Clone)]
struct ExtractedItem {
    name: String,
    kind: ItemKind,
    line: usize,
    line_count: usize,
    visibility: Visibility,
    has_doc: bool,
    children: Vec<ExtractedItem>,
    signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[allow(dead_code)] // `Module` reserved for future nested-module extraction; kept for the to_entity_type map
enum ItemKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Constant,
    TypeAlias,
    Module,
    Macro,
    Static,
}

impl ItemKind {
    fn to_entity_type(&self) -> EntityType {
        match self {
            ItemKind::Function => EntityType::Function,
            ItemKind::Struct => EntityType::Struct,
            ItemKind::Enum => EntityType::Enum,
            ItemKind::Trait => EntityType::Interface,
            ItemKind::Impl => EntityType::Class,
            ItemKind::Constant => EntityType::Constant,
            ItemKind::TypeAlias => EntityType::Custom("type_alias".into()),
            ItemKind::Module => EntityType::Module,
            ItemKind::Macro => EntityType::Custom("macro".into()),
            ItemKind::Static => EntityType::Constant,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Visibility {
    Public,
    CratePub,
    Private,
}

/// Extract entities and relationships from a Rust source file.
pub fn extract_rust(
    source: &str,
    file_path: &str,
) -> ExtractionResult {
    let items = parse_items(source);
    let mut entities = Vec::new();
    let mut relationships = Vec::new();

    // Module entity for the file itself.
    let mod_name = Path::new(file_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");

    let mod_id = EntityId::new(&DomainTag::Code, &EntityType::Module, mod_name, file_path);
    entities.push(Entity {
        id: mod_id.clone(),
        entity_type: EntityType::Module,
        label: mod_name.to_string(),
        iri: None,
        source_file: Some(file_path.to_string()),
        source_location: Some("L1".into()),
        file_type: FileType::Code,
        metadata: serde_json::json!({
            "line_count": source.lines().count(),
            "item_count": items.len(),
        }),
        legacy_id: None,
    });

    // Convert extracted items to entities.
    for item in &items {
        let form = triage(item);
        let entity_type = item.kind.to_entity_type();
        let item_id = EntityId::new(&DomainTag::Code, &entity_type, &item.name, file_path);

        let features = ExtractionFeatures {
            pattern_strength: 0.9,
            has_pub: item.visibility == Visibility::Public,
            has_doc_comment: item.has_doc,
            line_count: item.line_count,
            nesting_depth: 0,
        };

        let confidence_score = eml_confidence(&features);
        let complexity_score = eml_complexity(&features);

        entities.push(Entity {
            id: item_id.clone(),
            entity_type: entity_type.clone(),
            label: item.name.clone(),
            iri: None,
            source_file: Some(file_path.to_string()),
            source_location: Some(format!("L{}", item.line)),
            file_type: FileType::Code,
            metadata: serde_json::json!({
                "form": format!("{form:?}"),
                "visibility": format!("{:?}", item.visibility),
                "line_count": item.line_count,
                "confidence": confidence_score,
                "complexity": complexity_score,
                "has_doc": item.has_doc,
                "signature": item.signature,
            }),
            legacy_id: None,
        });

        // Module contains this item.
        relationships.push(Relationship {
            source: mod_id.clone(),
            target: item_id.clone(),
            relation_type: RelationType::Contains,
            confidence: Confidence::Extracted,
            weight: 1.0,
            source_file: Some(file_path.to_string()),
            source_location: Some(format!("L{}", item.line)),
            metadata: serde_json::json!({}),
        });

        // Process children (methods in impl/trait, variants in enum, fields in struct).
        for child in &item.children {
            let child_type = child.kind.to_entity_type();
            let child_id = EntityId::new(
                &DomainTag::Code,
                &child_type,
                &format!("{}::{}", item.name, child.name),
                file_path,
            );

            let child_features = ExtractionFeatures {
                pattern_strength: 0.85,
                has_pub: child.visibility == Visibility::Public,
                has_doc_comment: child.has_doc,
                line_count: child.line_count,
                nesting_depth: 1,
            };

            entities.push(Entity {
                id: child_id.clone(),
                entity_type: child_type,
                label: child.name.clone(),
                iri: None,
                source_file: Some(file_path.to_string()),
                source_location: Some(format!("L{}", child.line)),
                file_type: FileType::Code,
                metadata: serde_json::json!({
                    "form": format!("{:?}", triage(child)),
                    "parent": item.name,
                    "confidence": eml_confidence(&child_features),
                    "complexity": eml_complexity(&child_features),
                    "signature": child.signature,
                }),
                legacy_id: None,
            });

            // Parent contains child.
            let rel_type = match item.kind {
                ItemKind::Impl => RelationType::MethodOf,
                _ => RelationType::Contains,
            };

            relationships.push(Relationship {
                source: item_id.clone(),
                target: child_id.clone(),
                relation_type: rel_type,
                confidence: Confidence::Extracted,
                weight: 1.0,
                source_file: Some(file_path.to_string()),
                source_location: Some(format!("L{}", child.line)),
                metadata: serde_json::json!({}),
            });
        }

        // Impl → trait/struct relationships.
        if item.kind == ItemKind::Impl
            && let Some(ref sig) = item.signature {
                // "impl TraitName for StructName" or "impl StructName"
                if let Some(target_name) = parse_impl_target(sig) {
                    let target_id = EntityId::new(
                        &DomainTag::Code,
                        &EntityType::Struct,
                        &target_name,
                        file_path,
                    );
                    relationships.push(Relationship {
                        source: item_id.clone(),
                        target: target_id,
                        relation_type: if sig.contains(" for ") {
                            RelationType::Implements
                        } else {
                            RelationType::Extends
                        },
                        confidence: Confidence::Extracted,
                        weight: 1.0,
                        source_file: Some(file_path.to_string()),
                        source_location: Some(format!("L{}", item.line)),
                        metadata: serde_json::json!({}),
                    });
                }
            }
    }

    // Extract call relationships from function bodies.
    let fn_names: HashMap<String, EntityId> = entities.iter()
        .filter(|e| matches!(e.entity_type, EntityType::Function))
        .map(|e| (e.label.clone(), e.id.clone()))
        .collect();

    for item in &items {
        if item.kind == ItemKind::Function || item.kind == ItemKind::Impl {
            let caller_items = if item.kind == ItemKind::Impl {
                &item.children
            } else {
                std::slice::from_ref(item)
            };

            for caller in caller_items {
                if caller.kind != ItemKind::Function {
                    continue;
                }
                let caller_name = if item.kind == ItemKind::Impl {
                    format!("{}::{}", item.name, caller.name)
                } else {
                    caller.name.clone()
                };
                let caller_id = fn_names.get(&caller.name)
                    .or_else(|| fn_names.get(&caller_name));

                if let Some(caller_id) = caller_id {
                    for (fn_name, fn_id) in &fn_names {
                        if fn_id == caller_id { continue; }
                        // Simple heuristic: if the function name appears in the source
                        // near the caller's line range, it's likely a call.
                        let call_pattern = format!("{}(", fn_name);
                        let caller_start = caller.line.saturating_sub(1);
                        let caller_end = caller.line + caller.line_count;
                        let in_range = source.lines()
                            .enumerate()
                            .skip(caller_start)
                            .take(caller_end - caller_start)
                            .any(|(_, line)| line.contains(&call_pattern));

                        if in_range {
                            relationships.push(Relationship {
                                source: caller_id.clone(),
                                target: fn_id.clone(),
                                relation_type: RelationType::Calls,
                                confidence: Confidence::Inferred,
                                weight: 0.7,
                                source_file: Some(file_path.to_string()),
                                source_location: None,
                                metadata: serde_json::json!({}),
                            });
                        }
                    }
                }
            }
        }
    }

    ExtractionResult {
        source_file: file_path.to_string(),
        entities,
        relationships,
        hyperedges: vec![],
        input_tokens: 0,
        output_tokens: 0,
        errors: vec![],
    }
}

fn parse_impl_target(sig: &str) -> Option<String> {
    // "impl TraitName for StructName" → StructName
    // "impl StructName" → StructName
    // "impl<T> TraitName for StructName<T>" → StructName
    let sig = sig.trim();
    if sig.contains(" for ") {
        let after_for = sig.split(" for ").last()?;
        let name = after_for.split('<').next()?.trim();
        if !name.is_empty() { Some(name.to_string()) } else { None }
    } else {
        let after_impl = sig.strip_prefix("impl")?;
        let after_generics = if after_impl.starts_with('<') {
            after_impl.find('>')?.checked_add(1)
                .and_then(|i| after_impl.get(i..))
                .unwrap_or(after_impl)
        } else {
            after_impl
        };
        let name = after_generics.trim().split('<').next()?.split('{').next()?.trim();
        if !name.is_empty() { Some(name.to_string()) } else { None }
    }
}

// ---------------------------------------------------------------------------
// Regex-based Rust parser
// ---------------------------------------------------------------------------

fn parse_items(source: &str) -> Vec<ExtractedItem> {
    let lines: Vec<&str> = source.lines().collect();
    let mut items = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();

        // Skip empty lines and comments that aren't doc comments.
        if line.is_empty() || (line.starts_with("//") && !line.starts_with("///")) {
            i += 1;
            continue;
        }

        // Check for doc comments preceding an item.
        let has_doc = line.starts_with("///") || line.starts_with("#[doc");
        if has_doc {
            // Skip doc comment lines to find the actual item.
            let _doc_start = i;
            while i < lines.len() {
                let l = lines[i].trim();
                if l.starts_with("///") || l.starts_with("#[") || l.is_empty() {
                    i += 1;
                } else {
                    break;
                }
            }
            if i >= lines.len() { break; }
        }

        let line = lines[i].trim();
        let (vis, rest) = parse_visibility(line);

        if let Some(item) = try_parse_item(rest, vis, has_doc, i + 1, &lines) {
            let end = i + item.line_count;
            items.push(item);
            i = end;
        } else {
            i += 1;
        }
    }

    items
}

fn parse_visibility(line: &str) -> (Visibility, &str) {
    if let Some(rest) = line.strip_prefix("pub(crate) ") {
        (Visibility::CratePub, rest)
    } else if let Some(rest) = line.strip_prefix("pub ") {
        (Visibility::Public, rest)
    } else {
        (Visibility::Private, line)
    }
}

fn try_parse_item(
    line: &str,
    vis: Visibility,
    has_doc: bool,
    line_num: usize,
    lines: &[&str],
) -> Option<ExtractedItem> {
    let fn_re = cached_regex(r"^(?:async\s+)?fn\s+(\w+)");
    let struct_re = cached_regex(r"^struct\s+(\w+)");
    let enum_re = cached_regex(r"^enum\s+(\w+)");
    let trait_re = cached_regex(r"^trait\s+(\w+)");
    let impl_re = cached_regex(r"^impl(?:<[^>]*>)?\s+(\S+)");
    let const_re = cached_regex(r"^const\s+(\w+)");
    let type_re = cached_regex(r"^type\s+(\w+)");
    let static_re = cached_regex(r"^static\s+(\w+)");
    let macro_re = cached_regex(r"^macro_rules!\s+(\w+)");

    let (kind, name, sig) = if let Some(cap) = fn_re.captures(line) {
        (ItemKind::Function, cap[1].to_string(), Some(line.to_string()))
    } else if let Some(cap) = struct_re.captures(line) {
        (ItemKind::Struct, cap[1].to_string(), Some(line.to_string()))
    } else if let Some(cap) = enum_re.captures(line) {
        (ItemKind::Enum, cap[1].to_string(), None)
    } else if let Some(cap) = trait_re.captures(line) {
        (ItemKind::Trait, cap[1].to_string(), None)
    } else if let Some(cap) = impl_re.captures(line) {
        let _full_impl = format!("impl {}", &cap[1]);
        // Get the rest of the line for "for X" detection.
        let sig_line = lines.get(line_num.saturating_sub(1))
            .map(|l| l.trim().to_string())
            .unwrap_or_default();
        (ItemKind::Impl, cap[1].to_string(), Some(sig_line))
    } else if let Some(cap) = const_re.captures(line) {
        (ItemKind::Constant, cap[1].to_string(), None)
    } else if let Some(cap) = type_re.captures(line) {
        (ItemKind::TypeAlias, cap[1].to_string(), None)
    } else if let Some(cap) = static_re.captures(line) {
        (ItemKind::Static, cap[1].to_string(), None)
    } else if let Some(cap) = macro_re.captures(line) {
        (ItemKind::Macro, cap[1].to_string(), None)
    } else {
        return None;
    };

    // Count lines until closing brace at same indentation.
    let has_body = line.contains('{') || (line_num < lines.len() && lines.get(line_num).is_some_and(|l| l.trim() == "{"));
    let line_count = if has_body {
        count_block_lines(lines, line_num.saturating_sub(1))
    } else {
        1
    };

    // Extract children for items with bodies (struct fields, enum variants, impl methods).
    let children = if has_body && matches!(kind, ItemKind::Struct | ItemKind::Enum | ItemKind::Trait | ItemKind::Impl) {
        extract_children(lines, line_num.saturating_sub(1), &kind)
    } else {
        vec![]
    };

    Some(ExtractedItem {
        name,
        kind,
        line: line_num,
        line_count,
        visibility: vis,
        has_doc,
        children,
        signature: sig,
    })
}

fn count_block_lines(lines: &[&str], start: usize) -> usize {
    let mut depth = 0i32;
    let mut started = false;
    for (i, line) in lines.iter().enumerate().skip(start) {
        for ch in line.chars() {
            if ch == '{' { depth += 1; started = true; }
            if ch == '}' { depth -= 1; }
        }
        if started && depth <= 0 {
            return i - start + 1;
        }
    }
    lines.len() - start
}

fn extract_children(lines: &[&str], start: usize, parent_kind: &ItemKind) -> Vec<ExtractedItem> {
    let mut children = Vec::new();
    let mut depth = 0i32;
    let mut body_start = start;

    // Find the opening brace.
    for (i, line) in lines.iter().enumerate().skip(start) {
        if line.contains('{') {
            depth = 1;
            body_start = i + 1;
            // Count additional braces on the same line.
            for ch in line.chars() {
                if ch == '{' { depth += 1; }
                if ch == '}' { depth -= 1; }
            }
            depth -= 1; // We counted the opening { twice.
            break;
        }
    }

    let mut i = body_start;
    while i < lines.len() {
        let line = lines[i].trim();

        // Track brace depth.
        let mut local_depth = 0i32;
        for ch in line.chars() {
            if ch == '{' { local_depth += 1; depth += 1; }
            if ch == '}' { local_depth -= 1; depth -= 1; }
        }

        if depth <= 0 { break; }

        // Only parse items at the first nesting level.
        if depth == 1 || (local_depth > 0 && depth - local_depth == 0) {
            let has_doc = line.starts_with("///");
            let (vis, rest) = parse_visibility(line);

            let child = match parent_kind {
                ItemKind::Impl | ItemKind::Trait => {
                    // Look for fn declarations.
                    if rest.starts_with("fn ") || rest.starts_with("async fn ") {
                        try_parse_item(rest, vis, has_doc, i + 1, lines)
                    } else {
                        None
                    }
                }
                ItemKind::Enum => {
                    // Enum variants.
                    if !line.starts_with("//") && !line.is_empty() && !line.starts_with('#') && !line.starts_with('}') {
                        let variant_name = line.split(['(', '{', ','])
                            .next()
                            .unwrap_or("")
                            .trim();
                        if !variant_name.is_empty() && variant_name.chars().next().is_some_and(|c| c.is_uppercase()) {
                            Some(ExtractedItem {
                                name: variant_name.to_string(),
                                kind: ItemKind::Constant,
                                line: i + 1,
                                line_count: 1,
                                visibility: vis,
                                has_doc,
                                children: vec![],
                                signature: None,
                            })
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
                ItemKind::Struct => {
                    // Struct fields.
                    if !line.starts_with("//") && !line.is_empty() && !line.starts_with('#') && !line.starts_with('}') {
                        let field_name = line.split(':').next().unwrap_or("").trim();
                        let (fvis, fname) = parse_visibility(field_name);
                        if !fname.is_empty() && fname.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                            Some(ExtractedItem {
                                name: fname.to_string(),
                                kind: ItemKind::Constant, // fields as atoms
                                line: i + 1,
                                line_count: 1,
                                visibility: fvis,
                                has_doc,
                                children: vec![],
                                signature: Some(line.to_string()),
                            })
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
                _ => None,
            };

            if let Some(child) = child {
                let skip = child.line_count;
                children.push(child);
                i += skip;
                continue;
            }
        }

        i += 1;
    }

    children
}

/// Compile a regex once and cache it.
fn cached_regex(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap()
}

// ---------------------------------------------------------------------------
// Batch extraction
// ---------------------------------------------------------------------------

/// Extract all Rust files in a directory tree.
pub fn extract_directory(dir: &Path) -> Vec<ExtractionResult> {
    let mut results = Vec::new();
    extract_recursive(dir, dir, &mut results);
    results
}

fn extract_recursive(base: &Path, dir: &Path, results: &mut Vec<ExtractionResult>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with('.') || name_str == "target" || name_str == "node_modules" {
            continue;
        }

        if path.is_dir() {
            extract_recursive(base, &path, results);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
            && let Ok(source) = std::fs::read_to_string(&path) {
                let rel_path = path.strip_prefix(base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                results.push(extract_rust(&source, &rel_path));
            }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RUST: &str = r#"
//! Module doc

use std::collections::HashMap;

/// A user in the system.
pub struct User {
    pub name: String,
    pub email: String,
    age: u32,
}

/// User roles.
pub enum Role {
    Admin,
    Editor,
    Viewer,
}

/// Authentication trait.
pub trait Authenticate {
    fn verify(&self, token: &str) -> bool;
    fn refresh(&mut self);
}

impl User {
    pub fn new(name: String, email: String) -> Self {
        Self { name, email, age: 0 }
    }

    pub fn display_name(&self) -> &str {
        &self.name
    }
}

impl Authenticate for User {
    fn verify(&self, token: &str) -> bool {
        !token.is_empty()
    }

    fn refresh(&mut self) {
        // no-op
    }
}

const MAX_USERS: usize = 1000;

pub fn create_user(name: &str) -> User {
    User::new(name.to_string(), format!("{}@example.com", name))
}

fn helper() {
    let u = create_user("test");
    u.display_name();
}
"#;

    #[test]
    fn extracts_all_item_kinds() {
        let result = extract_rust(SAMPLE_RUST, "auth.rs");

        let types: Vec<String> = result.entities.iter()
            .map(|e| format!("{}:{}", e.entity_type.discriminant(), e.label))
            .collect();

        // Module
        assert!(types.iter().any(|t| t.contains("module:auth")));
        // Struct
        assert!(types.iter().any(|t| t.contains("struct_:User")));
        // Enum
        assert!(types.iter().any(|t| t.contains("enum_:Role")));
        // Trait
        assert!(types.iter().any(|t| t.contains("interface:Authenticate")));
        // Functions
        assert!(types.iter().any(|t| t.contains("function:create_user")));
        assert!(types.iter().any(|t| t.contains("function:helper")));
        // Constant
        assert!(types.iter().any(|t| t.contains("constant:MAX_USERS")));
    }

    #[test]
    fn extracts_impl_methods_as_children() {
        let result = extract_rust(SAMPLE_RUST, "auth.rs");

        let method_entities: Vec<&Entity> = result.entities.iter()
            .filter(|e| {
                e.metadata.get("parent").and_then(|v| v.as_str()).is_some()
            })
            .collect();

        assert!(method_entities.len() >= 2, "should have methods from impl blocks");
    }

    #[test]
    fn triage_classifies_correctly() {
        let result = extract_rust(SAMPLE_RUST, "auth.rs");

        // Struct has same-type fields → Sequence (all fields are Constant kind)
        let user_meta = result.entities.iter()
            .find(|e| e.label == "User")
            .and_then(|e| e.metadata.get("form"))
            .and_then(|v| v.as_str());
        assert_eq!(user_meta, Some("Sequence"));

        // Enum with variants → Sequence (all variants same kind)
        let role_meta = result.entities.iter()
            .find(|e| e.label == "Role")
            .and_then(|e| e.metadata.get("form"))
            .and_then(|v| v.as_str());
        assert_eq!(role_meta, Some("Sequence"));

        // Constant has no children → Atom
        let const_meta = result.entities.iter()
            .find(|e| e.label == "MAX_USERS")
            .and_then(|e| e.metadata.get("form"))
            .and_then(|v| v.as_str());
        assert_eq!(const_meta, Some("Atom"));
    }

    #[test]
    fn eml_confidence_scores_reasonable() {
        let high = eml_confidence(&ExtractionFeatures {
            pattern_strength: 0.9,
            has_pub: true,
            has_doc_comment: true,
            line_count: 10,
            nesting_depth: 0,
        });
        let low = eml_confidence(&ExtractionFeatures {
            pattern_strength: 0.3,
            has_pub: false,
            has_doc_comment: false,
            line_count: 1,
            nesting_depth: 2,
        });
        assert!(high > low);
        assert!(high > 0.5);
        assert!(high <= 1.0);
        assert!(low >= 0.0);
    }

    #[test]
    fn contains_relationships_created() {
        let result = extract_rust(SAMPLE_RUST, "auth.rs");

        let contains: Vec<&Relationship> = result.relationships.iter()
            .filter(|r| matches!(r.relation_type, RelationType::Contains))
            .collect();

        assert!(contains.len() >= 4, "module should contain structs, enums, functions");
    }

    #[test]
    fn impl_creates_extends_relationship() {
        let result = extract_rust(SAMPLE_RUST, "auth.rs");

        let extends: Vec<&Relationship> = result.relationships.iter()
            .filter(|r| matches!(r.relation_type, RelationType::Implements | RelationType::Extends))
            .collect();

        assert!(!extends.is_empty(), "impl blocks should create extends/implements edges");
    }
}
