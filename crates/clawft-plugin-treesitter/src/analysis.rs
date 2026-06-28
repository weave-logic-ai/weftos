//! Core tree-sitter analysis operations.
//!
//! Provides AST parsing, symbol extraction, and complexity metrics
//! for supported languages. Each function takes source code and a
//! tree-sitter `Language` grammar, operating purely on in-memory data.

use tracing::debug;
use tree_sitter::{Node, Parser, Tree};

use crate::types::{AstNode, ComplexityMetrics, FunctionComplexity, Language, Symbol};

/// Parse source code into a tree-sitter [`Tree`].
pub fn parse_source(source: &str, language: Language) -> Result<Tree, String> {
    let ts_language = get_tree_sitter_language(language)?;
    let mut parser = Parser::new();
    parser
        .set_language(&ts_language)
        .map_err(|e| format!("failed to set language: {e}"))?;

    parser
        .parse(source, None)
        .ok_or_else(|| "parsing failed (timeout or cancellation)".to_string())
}

/// Convert a parse tree into a serializable AST structure.
pub fn tree_to_ast(tree: &Tree, max_depth: usize) -> AstNode {
    node_to_ast(tree.root_node(), 0, max_depth)
}

fn node_to_ast(node: Node, depth: usize, max_depth: usize) -> AstNode {
    let children = if depth < max_depth {
        let mut cursor = node.walk();
        node.named_children(&mut cursor)
            .map(|child| node_to_ast(child, depth + 1, max_depth))
            .collect()
    } else {
        Vec::new()
    };

    AstNode {
        kind: node.kind().to_string(),
        start_line: node.start_position().row,
        end_line: node.end_position().row,
        is_named: node.is_named(),
        children,
    }
}

/// Extract symbols (functions, structs, classes, methods) from a parse tree.
pub fn extract_symbols(tree: &Tree, source: &str, language: Language) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    let root = tree.root_node();
    collect_symbols(root, source, language, &mut symbols);
    symbols
}

fn collect_symbols(node: Node, source: &str, language: Language, out: &mut Vec<Symbol>) {
    if is_symbol_node(&node, language)
        && let Some(name) = find_name_child(&node, source, language)
    {
        out.push(Symbol {
            name,
            kind: symbol_kind(&node, language),
            start_line: node.start_position().row,
            end_line: node.end_position().row,
            start_col: node.start_position().column,
        });
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_symbols(child, source, language, out);
    }
}

/// Determine if a node represents a symbol definition.
fn is_symbol_node(node: &Node, language: Language) -> bool {
    let kind = node.kind();
    match language {
        Language::Rust => matches!(
            kind,
            "function_item"
                | "struct_item"
                | "enum_item"
                | "trait_item"
                | "impl_item"
                | "type_item"
                | "const_item"
                | "static_item"
        ),
        Language::TypeScript | Language::JavaScript => matches!(
            kind,
            "function_declaration"
                | "class_declaration"
                | "method_definition"
                | "interface_declaration"
                | "type_alias_declaration"
                | "arrow_function"
                | "variable_declarator"
        ),
        Language::Python => matches!(
            kind,
            "function_definition" | "class_definition" | "decorated_definition"
        ),
    }
}

/// Extract the name of a symbol node.
fn find_name_child(node: &Node, source: &str, language: Language) -> Option<String> {
    let name_field = match language {
        Language::Rust => "name",
        Language::TypeScript | Language::JavaScript => "name",
        Language::Python => "name",
    };

    if let Some(name_node) = node.child_by_field_name(name_field) {
        let start = name_node.start_byte();
        let end = name_node.end_byte();
        if end <= source.len() {
            return Some(source[start..end].to_string());
        }
    }
    None
}

/// Map a node kind to a human-readable symbol kind.
fn symbol_kind(node: &Node, language: Language) -> String {
    let kind = node.kind();
    match language {
        Language::Rust => match kind {
            "function_item" => "function",
            "struct_item" => "struct",
            "enum_item" => "enum",
            "trait_item" => "trait",
            "impl_item" => "impl",
            "type_item" => "type",
            "const_item" => "const",
            "static_item" => "static",
            _ => kind,
        },
        Language::TypeScript | Language::JavaScript => match kind {
            "function_declaration" => "function",
            "class_declaration" => "class",
            "method_definition" => "method",
            "interface_declaration" => "interface",
            "type_alias_declaration" => "type",
            "arrow_function" => "arrow_function",
            "variable_declarator" => "variable",
            _ => kind,
        },
        Language::Python => match kind {
            "function_definition" => "function",
            "class_definition" => "class",
            "decorated_definition" => "decorated",
            _ => kind,
        },
    }
    .to_string()
}

/// Calculate cyclomatic complexity metrics for a source file.
///
/// Cyclomatic complexity starts at 1 per function, with +1 for each
/// branching construct (if, while, for, match arm, &&, ||, etc.).
pub fn calculate_complexity(tree: &Tree, source: &str, language: Language) -> ComplexityMetrics {
    let root = tree.root_node();
    let mut functions = Vec::new();

    collect_function_complexity(root, source, language, &mut functions);

    let total_complexity: usize = functions.iter().map(|f| f.complexity).sum();
    let function_count = functions.len();

    debug!(
        total_complexity,
        function_count, "calculated complexity metrics"
    );

    ComplexityMetrics {
        total_complexity,
        function_count,
        functions,
    }
}

fn collect_function_complexity(
    node: Node,
    source: &str,
    language: Language,
    out: &mut Vec<FunctionComplexity>,
) {
    let kind = node.kind();
    let is_function = match language {
        Language::Rust => kind == "function_item",
        Language::TypeScript | Language::JavaScript => {
            matches!(
                kind,
                "function_declaration" | "method_definition" | "arrow_function"
            )
        }
        Language::Python => kind == "function_definition",
    };

    if is_function {
        let name =
            find_name_child(&node, source, language).unwrap_or_else(|| "<anonymous>".to_string());
        let complexity = count_branches(node, language) + 1;

        out.push(FunctionComplexity {
            name,
            complexity,
            start_line: node.start_position().row,
        });
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_function_complexity(child, source, language, out);
    }
}

/// Count branching constructs within a node (recursive).
fn count_branches(node: Node, language: Language) -> usize {
    let mut count = 0;
    let kind = node.kind();

    match language {
        Language::Rust => {
            if matches!(
                kind,
                "if_expression"
                    | "while_expression"
                    | "for_expression"
                    | "loop_expression"
                    | "match_arm"
                    | "else_clause"
            ) {
                count += 1;
            }
        }
        Language::TypeScript | Language::JavaScript => {
            if matches!(
                kind,
                "if_statement"
                    | "while_statement"
                    | "for_statement"
                    | "for_in_statement"
                    | "switch_case"
                    | "catch_clause"
                    | "ternary_expression"
                    | "else_clause"
            ) {
                count += 1;
            }
        }
        Language::Python => {
            if matches!(
                kind,
                "if_statement"
                    | "elif_clause"
                    | "while_statement"
                    | "for_statement"
                    | "except_clause"
                    | "with_statement"
            ) {
                count += 1;
            }
        }
    }

    // Check for logical operators (&&, ||, and, or)
    if matches!(kind, "binary_expression" | "boolean_operator")
        && let Some(op) = node.child_by_field_name("operator")
    {
        let op_text = op.kind();
        if matches!(op_text, "&&" | "||" | "and" | "or") {
            count += 1;
        }
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        count += count_branches(child, language);
    }

    count
}

/// Get the tree-sitter language grammar for a supported language.
fn get_tree_sitter_language(language: Language) -> Result<tree_sitter::Language, String> {
    match language {
        #[cfg(feature = "rust")]
        Language::Rust => Ok(tree_sitter_rust::LANGUAGE.into()),
        #[cfg(not(feature = "rust"))]
        Language::Rust => Err("tree-sitter-rust feature not enabled".to_string()),

        #[cfg(feature = "typescript")]
        Language::TypeScript => Ok(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        #[cfg(not(feature = "typescript"))]
        Language::TypeScript => Err("tree-sitter-typescript feature not enabled".to_string()),

        #[cfg(feature = "python")]
        Language::Python => Ok(tree_sitter_python::LANGUAGE.into()),
        #[cfg(not(feature = "python"))]
        Language::Python => Err("tree-sitter-python feature not enabled".to_string()),

        #[cfg(feature = "javascript")]
        Language::JavaScript => Ok(tree_sitter_javascript::LANGUAGE.into()),
        #[cfg(not(feature = "javascript"))]
        Language::JavaScript => Err("tree-sitter-javascript feature not enabled".to_string()),
    }
}

/// Check whether a language feature is enabled at compile time.
pub fn is_language_available(language: Language) -> bool {
    match language {
        Language::Rust => cfg!(feature = "rust"),
        Language::TypeScript => cfg!(feature = "typescript"),
        Language::Python => cfg!(feature = "python"),
        Language::JavaScript => cfg!(feature = "javascript"),
    }
}

/// List all languages that have their feature enabled.
pub fn available_languages() -> Vec<Language> {
    let mut langs = Vec::new();
    if cfg!(feature = "rust") {
        langs.push(Language::Rust);
    }
    if cfg!(feature = "typescript") {
        langs.push(Language::TypeScript);
    }
    if cfg!(feature = "python") {
        langs.push(Language::Python);
    }
    if cfg!(feature = "javascript") {
        langs.push(Language::JavaScript);
    }
    langs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_language_returns_error() {
        // Without any language features enabled, all should fail.
        // This test is valid when run with default features (none).
        if !cfg!(feature = "rust") {
            let result = parse_source("fn main() {}", Language::Rust);
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("not enabled"));
        }
    }

    #[test]
    fn available_languages_returns_list() {
        let langs = available_languages();
        // With default features (none), list should be empty.
        // With any features enabled, it should contain those.
        // This test just verifies it doesn't panic.
        for lang in &langs {
            assert!(is_language_available(*lang));
        }
    }

    #[cfg(feature = "rust")]
    mod rust_tests {
        use super::*;

        const RUST_SOURCE: &str = r#"
fn simple() -> i32 {
    42
}

fn branching(x: i32) -> i32 {
    if x > 0 {
        if x > 10 {
            100
        } else {
            50
        }
    } else {
        0
    }
}

struct MyStruct {
    field: i32,
}

enum MyEnum {
    A,
    B(i32),
}
"#;

        #[test]
        fn parse_rust_source() {
            let tree = parse_source(RUST_SOURCE, Language::Rust).unwrap();
            let root = tree.root_node();
            assert_eq!(root.kind(), "source_file");
            assert!(!root.has_error());
        }

        #[test]
        fn extract_rust_symbols() {
            let tree = parse_source(RUST_SOURCE, Language::Rust).unwrap();
            let symbols = extract_symbols(&tree, RUST_SOURCE, Language::Rust);
            let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
            assert!(names.contains(&"simple"), "missing 'simple': {names:?}");
            assert!(
                names.contains(&"branching"),
                "missing 'branching': {names:?}"
            );
            assert!(names.contains(&"MyStruct"), "missing 'MyStruct': {names:?}");
            assert!(names.contains(&"MyEnum"), "missing 'MyEnum': {names:?}");
        }

        #[test]
        fn rust_complexity_simple() {
            let tree = parse_source(RUST_SOURCE, Language::Rust).unwrap();
            let metrics = calculate_complexity(&tree, RUST_SOURCE, Language::Rust);
            assert_eq!(metrics.function_count, 2);

            let simple = metrics.functions.iter().find(|f| f.name == "simple");
            assert!(simple.is_some());
            assert_eq!(simple.unwrap().complexity, 1); // no branches

            let branching = metrics.functions.iter().find(|f| f.name == "branching");
            assert!(branching.is_some());
            assert!(branching.unwrap().complexity > 1); // has if/else branches
        }

        #[test]
        fn ast_output_respects_max_depth() {
            let tree = parse_source(RUST_SOURCE, Language::Rust).unwrap();
            let ast = tree_to_ast(&tree, 1);
            // At depth 1, children of children should be empty.
            for child in &ast.children {
                assert!(
                    child.children.is_empty(),
                    "depth 1 should have no grandchildren"
                );
            }
        }

        #[test]
        fn ast_output_deep() {
            let tree = parse_source(RUST_SOURCE, Language::Rust).unwrap();
            let ast = tree_to_ast(&tree, 20);
            assert_eq!(ast.kind, "source_file");
            assert!(!ast.children.is_empty());
        }
    }
}
