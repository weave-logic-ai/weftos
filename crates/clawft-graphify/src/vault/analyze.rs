//! Vault-wide link analysis: graph metrics, orphan detection, broken links,
//! and cluster counting via union-find.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::frontmatter;
use super::links;

/// A node in the vault link graph.
#[derive(Debug, Clone)]
pub struct VaultNode {
    pub path: PathBuf,
    pub title: Option<String>,
    pub tags: Vec<String>,
    pub outgoing: Vec<String>,
    pub incoming: Vec<String>,
    pub word_count: usize,
}

/// Summary statistics for a vault's link graph.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VaultMetrics {
    pub total_files: usize,
    pub files_with_links: usize,
    pub total_links: usize,
    pub wiki_links: usize,
    pub markdown_links: usize,
    pub orphan_files: Vec<String>,
    pub orphan_rate: f64,
    pub link_density: f64,
    pub avg_links_per_file: f64,
    pub max_links_in_file: usize,
    pub max_links_file: String,
    pub clusters: usize,
    pub broken_links: Vec<BrokenLink>,
}

/// A link that points to a non-existent file.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BrokenLink {
    pub source: String,
    pub target: String,
}

/// Scan a directory of markdown files and compute link graph metrics.
pub fn analyze_vault(
    vault_path: &Path,
) -> Result<(HashMap<String, VaultNode>, VaultMetrics), crate::GraphifyError> {
    let md_files = discover_markdown(vault_path)?;
    let mut nodes: HashMap<String, VaultNode> = HashMap::new();

    // Pass 1: parse all files, extract outgoing links.
    for path in &md_files {
        let rel = path.strip_prefix(vault_path).unwrap_or(path);
        let key = rel.to_string_lossy().to_string();
        let content = std::fs::read_to_string(path).map_err(|e| {
            crate::GraphifyError::CacheError(format!("read {}: {e}", path.display()))
        })?;

        let doc = frontmatter::parse(&content);
        let wikilinks = links::extract_wikilinks(&content);
        let md_links = links::extract_markdown_links(&content);

        let outgoing: Vec<String> = wikilinks
            .iter()
            .map(|wl| wl.target.clone())
            .chain(md_links.iter().map(|(_, p)| p.clone()))
            .collect();

        let word_count = content.split_whitespace().count();

        nodes.insert(
            key,
            VaultNode {
                path: path.clone(),
                title: doc.frontmatter.title,
                tags: doc.frontmatter.tags,
                outgoing,
                incoming: Vec::new(),
                word_count,
            },
        );
    }

    // Pass 2: resolve links and populate incoming edges.
    let all_keys: Vec<String> = nodes.keys().cloned().collect();
    let key_basenames: HashMap<String, String> = all_keys
        .iter()
        .map(|k| {
            let base = Path::new(k)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(k)
                .to_lowercase();
            (base, k.clone())
        })
        .collect();

    let mut broken = Vec::new();
    let mut resolved_outgoing: HashMap<String, Vec<String>> = HashMap::new();

    for (source_key, node) in &nodes {
        let mut resolved = Vec::new();
        for target in &node.outgoing {
            let target_lower = target.to_lowercase();
            let target_key = key_basenames
                .get(&target_lower)
                .cloned()
                .or_else(|| {
                    let with_md = format!("{target_lower}.md");
                    key_basenames.get(&with_md.replace(".md", "")).cloned()
                })
                .or_else(|| {
                    all_keys
                        .iter()
                        .find(|k| k.to_lowercase().ends_with(&format!("/{target_lower}.md")))
                        .cloned()
                });

            match target_key {
                Some(ref tk) if tk != source_key => {
                    resolved.push(tk.clone());
                }
                Some(_) => {} // self-link, skip
                None => {
                    broken.push(BrokenLink {
                        source: source_key.clone(),
                        target: target.clone(),
                    });
                }
            }
        }
        resolved_outgoing.insert(source_key.clone(), resolved);
    }

    // Update nodes with resolved outgoing and compute incoming.
    for (key, resolved) in &resolved_outgoing {
        if let Some(node) = nodes.get_mut(key) {
            node.outgoing = resolved.clone();
        }
        for target in resolved {
            if let Some(target_node) = nodes.get_mut(target) {
                target_node.incoming.push(key.clone());
            }
        }
    }

    // Compute metrics.
    let total_files = nodes.len();
    let mut total_links = 0usize;
    let mut wiki_links = 0usize;
    let mut max_links = 0usize;
    let mut max_links_file = String::new();
    let mut files_with_links = 0usize;
    let mut orphans = Vec::new();

    for (key, node) in &nodes {
        let degree = node.outgoing.len() + node.incoming.len();
        total_links += node.outgoing.len();
        wiki_links += node.outgoing.len();
        if degree > 0 {
            files_with_links += 1;
        } else {
            orphans.push(key.clone());
        }
        if node.outgoing.len() > max_links {
            max_links = node.outgoing.len();
            max_links_file = key.clone();
        }
    }

    let clusters = count_clusters(&nodes);
    let orphan_rate = if total_files > 0 {
        orphans.len() as f64 / total_files as f64
    } else {
        0.0
    };
    let link_density = if total_files > 0 {
        total_links as f64 / total_files as f64
    } else {
        0.0
    };
    let avg_links = if total_files > 0 {
        total_links as f64 / total_files as f64
    } else {
        0.0
    };

    orphans.sort();

    let metrics = VaultMetrics {
        total_files,
        files_with_links,
        total_links,
        wiki_links,
        markdown_links: 0,
        orphan_files: orphans,
        orphan_rate,
        link_density,
        avg_links_per_file: avg_links,
        max_links_in_file: max_links,
        max_links_file,
        clusters,
        broken_links: broken,
    };

    Ok((nodes, metrics))
}

fn discover_markdown(dir: &Path) -> Result<Vec<PathBuf>, crate::GraphifyError> {
    let mut files = Vec::new();
    discover_recursive(dir, &mut files)?;
    files.sort();
    Ok(files)
}

fn discover_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), crate::GraphifyError> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        crate::GraphifyError::CacheError(format!("read dir {}: {e}", dir.display()))
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| crate::GraphifyError::CacheError(e.to_string()))?;
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with('.')
            || name_str == "node_modules"
            || name_str == "dist"
            || name_str == "target"
        {
            continue;
        }

        if path.is_dir() {
            discover_recursive(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path);
        }
    }
    Ok(())
}

/// Union-find cluster counting.
fn count_clusters(nodes: &HashMap<String, VaultNode>) -> usize {
    let keys: Vec<&String> = nodes.keys().collect();
    let n = keys.len();
    if n == 0 {
        return 0;
    }

    let key_idx: HashMap<&String, usize> = keys.iter().enumerate().map(|(i, k)| (*k, i)).collect();
    let mut parent: Vec<usize> = (0..n).collect();
    let mut rank: Vec<usize> = vec![0; n];

    fn find(parent: &mut [usize], i: usize) -> usize {
        if parent[i] != i {
            parent[i] = find(parent, parent[i]);
        }
        parent[i]
    }

    fn union(parent: &mut [usize], rank: &mut [usize], a: usize, b: usize) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra == rb {
            return;
        }
        if rank[ra] < rank[rb] {
            parent[ra] = rb;
        } else if rank[ra] > rank[rb] {
            parent[rb] = ra;
        } else {
            parent[rb] = ra;
            rank[ra] += 1;
        }
    }

    for (key, node) in nodes {
        if let Some(&src_idx) = key_idx.get(key) {
            for target in &node.outgoing {
                if let Some(&tgt_idx) = key_idx.get(target) {
                    union(&mut parent, &mut rank, src_idx, tgt_idx);
                }
            }
        }
    }

    let mut roots = HashSet::new();
    for i in 0..n {
        roots.insert(find(&mut parent, i));
    }
    roots.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_counting() {
        let mut nodes = HashMap::new();
        nodes.insert(
            "a.md".to_string(),
            VaultNode {
                path: PathBuf::from("a.md"),
                title: None,
                tags: vec![],
                outgoing: vec!["b.md".to_string()],
                incoming: vec![],
                word_count: 10,
            },
        );
        nodes.insert(
            "b.md".to_string(),
            VaultNode {
                path: PathBuf::from("b.md"),
                title: None,
                tags: vec![],
                outgoing: vec![],
                incoming: vec!["a.md".to_string()],
                word_count: 10,
            },
        );
        nodes.insert(
            "c.md".to_string(),
            VaultNode {
                path: PathBuf::from("c.md"),
                title: None,
                tags: vec![],
                outgoing: vec![],
                incoming: vec![],
                word_count: 10,
            },
        );

        let clusters = count_clusters(&nodes);
        assert_eq!(clusters, 2); // {a,b} and {c}
    }
}
