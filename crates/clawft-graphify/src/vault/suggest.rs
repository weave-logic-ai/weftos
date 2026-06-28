//! Link suggestion engine — scores potential connections between vault documents
//! based on shared tags, path proximity, content similarity, and orphan status.

use std::collections::{HashMap, HashSet};

use super::analyze::VaultNode;

/// A suggested connection between two vault documents.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Suggestion {
    pub source: String,
    pub target: String,
    pub score: f64,
    pub reason: String,
    pub bidirectional: bool,
    pub shared_tags: Vec<String>,
}

/// Configuration for the suggestion engine.
#[derive(Debug, Clone)]
pub struct SuggestConfig {
    pub min_score: f64,
    pub max_per_file: usize,
    pub max_connections_skip: usize,
}

impl Default for SuggestConfig {
    fn default() -> Self {
        Self {
            min_score: 5.0,
            max_per_file: 5,
            max_connections_skip: 20,
        }
    }
}

/// Generate link suggestions for all nodes in the vault graph.
pub fn suggest_links(
    nodes: &HashMap<String, VaultNode>,
    config: &SuggestConfig,
) -> Vec<Suggestion> {
    let orphans: HashSet<&String> = nodes
        .iter()
        .filter(|(_, n)| n.outgoing.is_empty() && n.incoming.is_empty())
        .map(|(k, _)| k)
        .collect();

    let keys: Vec<&String> = nodes.keys().collect();
    let mut processed: HashSet<(String, String)> = HashSet::new();
    let mut suggestions = Vec::new();

    for source_key in &keys {
        let source = &nodes[*source_key];
        let degree = source.outgoing.len() + source.incoming.len();
        if degree > config.max_connections_skip {
            continue;
        }

        let existing: HashSet<&String> = source
            .outgoing
            .iter()
            .chain(source.incoming.iter())
            .collect();
        let mut file_suggestions: Vec<Suggestion> = Vec::new();

        for target_key in &keys {
            if source_key == target_key || existing.contains(target_key) {
                continue;
            }
            let pair_key = if **source_key < **target_key {
                ((*source_key).clone(), (*target_key).clone())
            } else {
                ((*target_key).clone(), (*source_key).clone())
            };
            if processed.contains(&pair_key) {
                continue;
            }
            processed.insert(pair_key);

            let target = &nodes[*target_key];
            let (score, reason, shared_tags) =
                score_pair(source, target, source_key, target_key, &orphans);

            if score >= config.min_score {
                let bidirectional = score >= 7.0 || shared_tags.len() >= 2;
                file_suggestions.push(Suggestion {
                    source: (*source_key).clone(),
                    target: (*target_key).clone(),
                    score,
                    reason,
                    bidirectional,
                    shared_tags,
                });
            }
        }

        file_suggestions.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        file_suggestions.truncate(config.max_per_file);
        suggestions.extend(file_suggestions);
    }

    suggestions.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    suggestions
}

fn score_pair(
    source: &VaultNode,
    target: &VaultNode,
    source_key: &str,
    target_key: &str,
    orphans: &HashSet<&String>,
) -> (f64, String, Vec<String>) {
    let mut score = 0.0f64;
    let mut reasons = Vec::new();

    // Shared tags (highest weight).
    let src_tags: HashSet<&str> = source.tags.iter().map(|s| s.as_str()).collect();
    let tgt_tags: HashSet<&str> = target.tags.iter().map(|s| s.as_str()).collect();
    let shared_tags: Vec<String> = src_tags
        .intersection(&tgt_tags)
        .map(|s| s.to_string())
        .collect();

    if !shared_tags.is_empty() {
        score += shared_tags.len() as f64 * 2.5;
        reasons.push(format!("{} shared tags", shared_tags.len()));
    }

    // Path similarity.
    let path_sim = path_similarity(source_key, target_key);
    if path_sim > 0.5 {
        score += path_sim * 2.0;
        reasons.push("nearby paths".into());
    }

    // Content keyword similarity.
    let src_kw = extract_keywords(&source.title, &source.tags);
    let tgt_kw = extract_keywords(&target.title, &target.tags);
    let kw_sim = jaccard(&src_kw, &tgt_kw);
    if kw_sim > 0.3 {
        score += kw_sim * 2.5;
        reasons.push("keyword overlap".into());
    }

    // Word count similarity.
    if source.word_count > 0 && target.word_count > 0 {
        let ratio = source.word_count.min(target.word_count) as f64
            / source.word_count.max(target.word_count) as f64;
        if ratio > 0.7 {
            score += 0.5;
        }
    }

    // Orphan boost.
    let sk = source_key.to_string();
    let tk = target_key.to_string();
    if orphans.contains(&sk) || orphans.contains(&tk) {
        score += 1.5;
        reasons.push("orphan file".into());
    }

    let reason = if reasons.is_empty() {
        "general similarity".into()
    } else {
        reasons.join(", ")
    };

    (score, reason, shared_tags)
}

fn path_similarity(a: &str, b: &str) -> f64 {
    let a_parts: Vec<&str> = a.split('/').collect();
    let b_parts: Vec<&str> = b.split('/').collect();
    let max_len = a_parts.len().max(b_parts.len());
    if max_len <= 1 {
        return 1.0;
    }
    let shared = a_parts
        .iter()
        .zip(b_parts.iter())
        .take_while(|(x, y)| x == y)
        .count();
    shared as f64 / (max_len - 1) as f64 // -1 to exclude filename
}

fn extract_keywords(title: &Option<String>, tags: &[String]) -> HashSet<String> {
    let mut kw = HashSet::new();
    for tag in tags {
        kw.insert(tag.to_lowercase());
    }
    if let Some(t) = title {
        for word in t.split_whitespace() {
            let lower = word.to_lowercase();
            if lower.len() >= 3 {
                kw.insert(lower);
            }
        }
    }
    kw
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f64;
    let union = a.union(b).count() as f64;
    inter / union
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn node(path: &str, tags: &[&str], out: &[&str]) -> VaultNode {
        VaultNode {
            path: PathBuf::from(path),
            title: Some(path.to_string()),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            outgoing: out.iter().map(|s| s.to_string()).collect(),
            incoming: vec![],
            word_count: 100,
        }
    }

    #[test]
    fn shared_tags_boost_score() {
        let mut nodes = HashMap::new();
        nodes.insert("a.md".into(), node("a.md", &["rust", "api"], &[]));
        nodes.insert(
            "b.md".into(),
            node("b.md", &["rust", "api", "backend"], &[]),
        );
        nodes.insert("c.md".into(), node("c.md", &["design"], &[]));

        let suggestions = suggest_links(&nodes, &SuggestConfig::default());
        // a-b should score higher than a-c or b-c.
        assert!(!suggestions.is_empty());
        let top = &suggestions[0];
        let pair = [top.source.as_str(), top.target.as_str()];
        assert!(pair.contains(&"a.md") && pair.contains(&"b.md"));
        assert!(top.shared_tags.contains(&"rust".to_string()));
    }
}
