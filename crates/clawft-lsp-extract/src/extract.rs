//! Extraction engine — walk source files, query LSP, build graph.

use std::collections::HashMap;
use std::path::Path;

use crate::config::LanguageConfig;
use crate::graph::*;
use crate::server::LspServer;

/// Extract a full semantic graph from a codebase using LSP.
pub fn extract(root: &Path, config: &LanguageConfig) -> anyhow::Result<LspGraph> {
    let root_str = root.to_string_lossy();
    let root_uri = format!("file://{}", std::fs::canonicalize(root)?.display());
    let start = std::time::Instant::now();

    let mut server = LspServer::start(config, &root_str)?;
    let mut graph = LspGraph::new(&config.name, &root_uri);
    let mut id_counter = 0u64;
    let mut symbol_id_map: HashMap<String, String> = HashMap::new();

    // Discover source files.
    let files = discover_files(root, &config.extensions);
    tracing::info!(files = files.len(), language = config.name, "discovered source files");

    for file_path in &files {
        let rel = file_path.strip_prefix(root).unwrap_or(file_path);
        let file_uri = format!("file://{}", file_path.display());

        // Query document symbols.
        let symbols = match server.document_symbols(&file_uri) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(file = %rel.display(), error = %e, "failed to get symbols");
                continue;
            }
        };

        // Parse symbols into nodes.
        let file_id = next_id(&mut id_counter);
        graph.nodes.push(LspNode {
            id: file_id.clone(),
            name: rel.to_string_lossy().to_string(),
            kind: LspNodeKind::File,
            file: rel.to_string_lossy().to_string(),
            line: 0,
            end_line: 0,
            detail: None,
            container: None,
            is_public: true,
            metadata: HashMap::new(),
        });

        if let Some(syms) = symbols.as_array() {
            parse_symbols(
                syms,
                &file_id,
                &rel.to_string_lossy(),
                &mut graph,
                &mut id_counter,
                &mut symbol_id_map,
            );
        }

        graph.stats.files_processed += 1;
    }

    graph.stats.symbols_extracted = graph.nodes.len();
    graph.stats.duration_ms = start.elapsed().as_millis() as u64;

    // Clean up.
    server.shutdown()?;

    tracing::info!(
        nodes = graph.nodes.len(),
        edges = graph.edges.len(),
        duration_ms = graph.stats.duration_ms,
        "extraction complete"
    );

    Ok(graph)
}

fn parse_symbols(
    symbols: &[serde_json::Value],
    parent_id: &str,
    file: &str,
    graph: &mut LspGraph,
    counter: &mut u64,
    id_map: &mut HashMap<String, String>,
) {
    for sym in symbols {
        let name = sym.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if name.is_empty() { continue; }

        let kind_num = sym.get("kind").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let kind = LspNodeKind::from_lsp(kind_num);

        let range = sym.get("range").or_else(|| sym.get("location").and_then(|l| l.get("range")));
        let (line, end_line) = range.map(|r| {
            let start = r.get("start").and_then(|s| s.get("line")).and_then(|l| l.as_u64()).unwrap_or(0) as u32;
            let end = r.get("end").and_then(|s| s.get("line")).and_then(|l| l.as_u64()).unwrap_or(0) as u32;
            (start, end)
        }).unwrap_or((0, 0));

        let detail = sym.get("detail").and_then(|v| v.as_str()).map(String::from);
        let is_public = name.starts_with("pub ") || !matches!(kind, LspNodeKind::Field | LspNodeKind::Variable);

        let node_id = next_id(counter);
        let key = format!("{}:{}", file, name);
        id_map.insert(key, node_id.clone());

        graph.nodes.push(LspNode {
            id: node_id.clone(),
            name,
            kind,
            file: file.to_string(),
            line,
            end_line,
            detail,
            container: Some(parent_id.to_string()),
            is_public,
            metadata: HashMap::new(),
        });

        // Parent contains this symbol.
        graph.edges.push(LspEdge {
            source: parent_id.to_string(),
            target: node_id.clone(),
            kind: LspEdgeKind::Contains,
            file: Some(file.to_string()),
            line: Some(line),
        });

        // Recurse into children (hierarchical document symbols).
        if let Some(children) = sym.get("children").and_then(|c| c.as_array()) {
            parse_symbols(children, &node_id, file, graph, counter, id_map);
        }
    }
}

fn next_id(counter: &mut u64) -> String {
    *counter += 1;
    format!("lsp_{counter}")
}

fn discover_files(root: &Path, extensions: &[String]) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    discover_recursive(root, extensions, &mut files);
    files.sort();
    files
}

fn discover_recursive(dir: &Path, extensions: &[String], out: &mut Vec<std::path::PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') || name_str == "target" || name_str == "node_modules" || name_str == "dist" {
            continue;
        }
        if path.is_dir() {
            discover_recursive(&path, extensions, out);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str())
            && extensions.iter().any(|e| e == ext)
        {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_finds_rust_files() {
        let files = discover_files(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            &["rs".to_string()],
        );
        assert!(files.len() >= 5, "should find crate source files");
    }

    #[test]
    fn parse_lsp_symbol_response() {
        let symbols = serde_json::json!([
            {
                "name": "main",
                "kind": 12,
                "range": {
                    "start": {"line": 10, "character": 0},
                    "end": {"line": 20, "character": 1}
                },
                "children": [
                    {
                        "name": "x",
                        "kind": 13,
                        "range": {
                            "start": {"line": 11, "character": 4},
                            "end": {"line": 11, "character": 20}
                        }
                    }
                ]
            }
        ]);

        let mut graph = LspGraph::new("rust", "file:///test");
        let mut counter = 0u64;
        let mut id_map = HashMap::new();

        parse_symbols(
            symbols.as_array().unwrap(),
            "file_0",
            "main.rs",
            &mut graph,
            &mut counter,
            &mut id_map,
        );

        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.nodes[0].name, "main");
        assert_eq!(graph.nodes[0].kind, LspNodeKind::Function);
        assert_eq!(graph.nodes[1].name, "x");
        assert_eq!(graph.nodes[1].kind, LspNodeKind::Variable);
        assert_eq!(graph.edges.len(), 2); // file→main, main→x
    }
}
