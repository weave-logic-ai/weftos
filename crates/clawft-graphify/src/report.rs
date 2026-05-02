//! GRAPH_REPORT.md generation -- human-readable audit trail.
//!
//! Ported from Python `graphify/report.py`.

use crate::analyze::{is_concept_node, is_file_node};
use crate::entity::EntityId;
use crate::model::{DetectionResult, KnowledgeGraph};
use crate::pipeline::AnalysisResult;
use crate::relationship::Confidence;
use std::collections::HashMap;

/// Token cost breakdown.
#[derive(Debug, Clone, Default)]
pub struct TokenCost {
    pub input: usize,
    pub output: usize,
}

/// Generate the GRAPH_REPORT.md content.
pub fn generate(
    kg: &KnowledgeGraph,
    analysis: &AnalysisResult,
    detection: &DetectionResult,
    token_cost: &TokenCost,
    root: &str,
) -> String {
    let today = chrono_stub_today();

    // Confidence breakdown
    let confidences: Vec<Confidence> = kg.edges().map(|(_, _, r)| r.confidence).collect();
    let total = confidences.len().max(1);
    let ext_count = confidences.iter().filter(|&&c| c == Confidence::Extracted).count();
    let inf_count = confidences.iter().filter(|&&c| c == Confidence::Inferred).count();
    let amb_count = confidences.iter().filter(|&&c| c == Confidence::Ambiguous).count();
    let ext_pct = (ext_count as f64 / total as f64 * 100.0).round() as usize;
    let inf_pct = (inf_count as f64 / total as f64 * 100.0).round() as usize;
    let amb_pct = (amb_count as f64 / total as f64 * 100.0).round() as usize;

    // INFERRED average confidence score
    let inf_scores: Vec<f64> = kg
        .edges()
        .filter(|(_, _, r)| r.confidence == Confidence::Inferred)
        .map(|(_, _, r)| r.confidence.to_score())
        .collect();
    let inf_avg: Option<f64> = if inf_scores.is_empty() {
        None
    } else {
        Some((inf_scores.iter().sum::<f64>() / inf_scores.len() as f64 * 100.0).round() / 100.0)
    };

    let mut lines: Vec<String> = Vec::new();

    // Header
    lines.push(format!("# Graph Report - {root}  ({today})"));
    lines.push(String::new());

    // Corpus Check
    lines.push("## Corpus Check".to_owned());
    if let Some(warning) = &detection.warning {
        lines.push(format!("- {warning}"));
    } else {
        lines.push(format!(
            "- {} files · ~{} words",
            detection.total_files, detection.total_words
        ));
        lines.push(
            "- Verdict: corpus is large enough that graph structure adds value.".to_owned(),
        );
    }

    // Summary
    lines.push(String::new());
    lines.push("## Summary".to_owned());
    lines.push(format!(
        "- {} nodes · {} edges · {} communities detected",
        kg.node_count(),
        kg.edge_count(),
        analysis.communities.len()
    ));
    let mut extraction_line = format!(
        "- Extraction: {ext_pct}% EXTRACTED · {inf_pct}% INFERRED · {amb_pct}% AMBIGUOUS"
    );
    if let Some(avg) = inf_avg {
        extraction_line.push_str(&format!(
            " · INFERRED: {} edges (avg confidence: {avg})",
            inf_count
        ));
    }
    lines.push(extraction_line);
    lines.push(format!(
        "- Token cost: {} input · {} output",
        token_cost.input, token_cost.output
    ));

    // God Nodes
    lines.push(String::new());
    lines.push("## God Nodes (most connected - your core abstractions)".to_owned());
    for (i, node) in analysis.god_nodes.iter().enumerate() {
        lines.push(format!("{}. `{}` - {} edges", i + 1, node.label, node.edges));
    }

    // Surprising Connections
    lines.push(String::new());
    lines.push("## Surprising Connections (you probably didn't know these)".to_owned());
    if analysis.surprising_connections.is_empty() {
        lines
            .push("- None detected - all connections are within the same source files.".to_owned());
    } else {
        for s in &analysis.surprising_connections {
            let conf_tag = s.confidence.as_str();
            let sem_tag = if s.relation == "semantically_similar_to" {
                " [semantically similar]"
            } else {
                ""
            };
            lines.push(format!(
                "- `{}` --{}--> `{}`  [{conf_tag}]{sem_tag}",
                s.source, s.relation, s.target
            ));
            let files = &s.source_files;
            let file_line = if files.len() >= 2 {
                format!("  {} -> {}", files[0], files[1])
            } else {
                String::new()
            };
            if let Some(note) = &s.note {
                lines.push(format!("{file_line}  _{note}_"));
            } else if !file_line.is_empty() {
                lines.push(file_line);
            }
        }
    }

    // Hyperedges
    if !kg.hyperedges.is_empty() {
        lines.push(String::new());
        lines.push("## Hyperedges (group relationships)".to_owned());
        for h in &kg.hyperedges {
            let node_labels: Vec<String> = h.entity_ids.iter().map(|n| {
                kg.entity(n).map(|e| e.label.clone()).unwrap_or_else(|| n.to_hex())
            }).collect();
            lines.push(format!(
                "- **{}** -- {}",
                h.label,
                node_labels.join(", ")
            ));
        }
    }

    // Communities
    lines.push(String::new());
    lines.push("## Communities".to_owned());
    let mut sorted_cids: Vec<usize> = analysis.communities.keys().copied().collect();
    sorted_cids.sort();
    for cid in sorted_cids {
        let nodes = &analysis.communities[&cid];
        let label = analysis
            .community_labels
            .get(&cid)
            .cloned()
            .unwrap_or_else(|| format!("Community {cid}"));
        let score = analysis.cohesion_scores.get(&cid).copied().unwrap_or(0.0);

        // Filter file nodes from display
        let real_nodes: Vec<&EntityId> = nodes
            .iter()
            .filter(|n| !is_file_node(kg, n))
            .collect();
        let display: Vec<String> = real_nodes
            .iter()
            .take(8)
            .map(|n| {
                kg.entity(n)
                    .map(|e| e.label.clone())
                    .unwrap_or_else(|| n.to_hex())
            })
            .collect();
        let suffix = if real_nodes.len() > 8 {
            format!(" (+{} more)", real_nodes.len() - 8)
        } else {
            String::new()
        };

        lines.push(String::new());
        lines.push(format!("### Community {cid} - \"{label}\""));
        lines.push(format!("Cohesion: {score}"));
        lines.push(format!(
            "Nodes ({}): {}{suffix}",
            real_nodes.len(),
            display.join(", ")
        ));
    }

    // Ambiguous Edges
    let ambiguous: Vec<_> = kg
        .edges()
        .filter(|(_, _, r)| r.confidence == Confidence::Ambiguous)
        .collect();
    if !ambiguous.is_empty() {
        lines.push(String::new());
        lines.push("## Ambiguous Edges - Review These".to_owned());
        for (src_ent, tgt_ent, r) in &ambiguous {
            lines.push(format!("- `{}` -> `{}`  [AMBIGUOUS]", src_ent.label, tgt_ent.label));
            let source_file_str = r.source_file.as_deref().unwrap_or("");
            lines.push(format!(
                "  {} · relation: {}",
                source_file_str, r.relation_type_str()
            ));
        }
    }

    // Knowledge Gaps
    let isolated: Vec<&EntityId> = kg
        .entity_ids()
        .filter(|id| kg.degree(id) <= 1 && !is_file_node(kg, id) && !is_concept_node(kg, id))
        .collect();
    let thin_communities: HashMap<usize, &Vec<EntityId>> = analysis
        .communities
        .iter()
        .filter(|(_, nodes): &(_, _)| nodes.len() < 3)
        .map(|(&cid, nodes)| (cid, nodes))
        .collect();

    let gap_count = isolated.len() + thin_communities.len();
    if gap_count > 0 || amb_pct > 20 {
        lines.push(String::new());
        lines.push("## Knowledge Gaps".to_owned());
        if !isolated.is_empty() {
            let labels: Vec<String> = isolated
                .iter()
                .take(5)
                .map(|id| {
                    kg.entity(id)
                        .map(|e| format!("`{}`", e.label))
                        .unwrap_or_else(|| format!("`{}`", id.to_hex()))
                })
                .collect();
            let suffix = if isolated.len() > 5 {
                format!(" (+{} more)", isolated.len() - 5)
            } else {
                String::new()
            };
            lines.push(format!(
                "- **{} isolated node(s):** {}{suffix}",
                isolated.len(),
                labels.join(", ")
            ));
            lines.push(
                "  These have <=1 connection - possible missing edges or undocumented components."
                    .to_owned(),
            );
        }
        if !thin_communities.is_empty() {
            for (&cid, nodes) in &thin_communities {
                let label = analysis
                    .community_labels
                    .get(&cid)
                    .cloned()
                    .unwrap_or_else(|| format!("Community {cid}"));
                let node_labels: Vec<String> = nodes
                    .iter()
                    .map(|n| {
                        kg.entity(n)
                            .map(|e| format!("`{}`", e.label))
                            .unwrap_or_else(|| format!("`{}`", n.to_hex()))
                    })
                    .collect();
                lines.push(format!(
                    "- **Thin community `{label}`** ({} nodes): {}",
                    nodes.len(),
                    node_labels.join(", ")
                ));
                lines.push(
                    "  Too small to be a meaningful cluster - may be noise or needs more connections extracted."
                        .to_owned(),
                );
            }
        }
        if amb_pct > 20 {
            lines.push(format!(
                "- **High ambiguity: {amb_pct}% of edges are AMBIGUOUS.** Review the Ambiguous Edges section above."
            ));
        }
    }

    // Suggested Questions
    if !analysis.questions.is_empty() {
        lines.push(String::new());
        lines.push("## Suggested Questions".to_owned());
        let is_no_signal = analysis.questions.len() == 1
            && analysis.questions[0].question.is_none();
        if is_no_signal {
            lines.push(format!("_{}_", analysis.questions[0].why));
        } else {
            lines.push(
                "_Questions this graph is uniquely positioned to answer:_".to_owned(),
            );
            lines.push(String::new());
            for q in &analysis.questions {
                if let Some(question) = &q.question {
                    lines.push(format!("- **{question}**"));
                    lines.push(format!("  _{}_", q.why));
                }
            }
        }
    }

    lines.join("\n")
}

/// Simple date stub (avoids pulling in chrono dependency).
fn chrono_stub_today() -> String {
    // Use a fixed format; in production, use chrono::Local::now()
    // For now, we just format from system time
    let now = std::time::SystemTime::now();
    let duration = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    // Simple days since epoch calculation
    let days = secs / 86400;
    let y = 1970 + (days * 400 / 146097); // approximate year
    format!("{y}-01-01") // Rough approximation; replace with chrono later
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{DomainTag, EntityType, FileType};
    use crate::model::Entity;
    use crate::relationship::{Confidence, RelationType, Relationship};

    fn entity(name: &str, source_file: &str) -> Entity {
        Entity {
            id: EntityId::new(&DomainTag::Code, &EntityType::Function, name, source_file),
            entity_type: EntityType::Function,
            label: name.to_owned(),
            source_file: Some(source_file.to_owned()),
            source_location: None,
            file_type: FileType::Code,
            metadata: serde_json::json!({}),
            legacy_id: None,
            iri: None,
        }
    }

    fn rel(src_name: &str, src_file: &str, tgt_name: &str, tgt_file: &str, conf: Confidence) -> Relationship {
        Relationship {
            source: EntityId::new(&DomainTag::Code, &EntityType::Function, src_name, src_file),
            target: EntityId::new(&DomainTag::Code, &EntityType::Function, tgt_name, tgt_file),
            relation_type: RelationType::Calls,
            confidence: conf,
            weight: conf.to_weight(),
            source_file: None,
            source_location: None,
            metadata: serde_json::json!({}),
        }
    }

    fn sample_analysis(kg: &KnowledgeGraph) -> AnalysisResult {
        let communities = crate::cluster::cluster(kg);
        let community_labels = crate::cluster::auto_label_all(kg, &communities);
        let cohesion_scores = crate::cluster::score_all(kg, &communities);
        let gn = crate::analyze::god_nodes(kg, 5);
        let sc = crate::analyze::surprising_connections(kg, &communities, 5);
        let qs = crate::analyze::suggest_questions(kg, &communities, &community_labels, 7);

        AnalysisResult {
            god_nodes: gn,
            surprising_connections: sc,
            questions: qs,
            communities,
            community_labels,
            cohesion_scores,
            community_summaries: HashMap::new(),
        }
    }

    #[test]
    fn report_contains_header() {
        let kg = KnowledgeGraph::from_parts(
            vec![entity("a", "a.py"), entity("b", "b.py")],
            vec![rel("a", "a.py", "b", "b.py", Confidence::Extracted)],
            vec![],
        );
        let analysis = sample_analysis(&kg);
        let detection = DetectionResult {
            total_files: 2,
            total_words: 500,
            warning: None,
        };
        let report = generate(&kg, &analysis, &detection, &TokenCost::default(), "test-project");
        assert!(report.contains("# Graph Report - test-project"));
    }

    #[test]
    fn report_contains_sections() {
        let kg = KnowledgeGraph::from_parts(
            vec![entity("a", "a.py"), entity("b", "b.py"), entity("c", "c.py")],
            vec![
                rel("a", "a.py", "b", "b.py", Confidence::Extracted),
                rel("b", "b.py", "c", "c.py", Confidence::Ambiguous),
            ],
            vec![],
        );
        let analysis = sample_analysis(&kg);
        let detection = DetectionResult {
            total_files: 3,
            total_words: 1000,
            warning: None,
        };
        let report = generate(&kg, &analysis, &detection, &TokenCost::default(), "test");
        assert!(report.contains("## Corpus Check"));
        assert!(report.contains("## Summary"));
        assert!(report.contains("## God Nodes"));
        assert!(report.contains("## Communities"));
        assert!(report.contains("## Ambiguous Edges"));
    }

    #[test]
    fn report_shows_token_cost() {
        let kg = KnowledgeGraph::from_parts(vec![entity("a", "a.py")], vec![], vec![]);
        let analysis = sample_analysis(&kg);
        let detection = DetectionResult::default();
        let cost = TokenCost {
            input: 1500,
            output: 300,
        };
        let report = generate(&kg, &analysis, &detection, &cost, "test");
        assert!(report.contains("1500 input"));
        assert!(report.contains("300 output"));
    }

    #[test]
    fn report_corpus_check_with_warning() {
        let kg = KnowledgeGraph::new();
        let analysis = AnalysisResult {
            god_nodes: vec![],
            surprising_connections: vec![],
            questions: vec![],
            communities: HashMap::new(),
            community_labels: HashMap::new(),
            cohesion_scores: HashMap::new(),
            community_summaries: HashMap::new(),
        };
        let detection = DetectionResult {
            total_files: 0,
            total_words: 0,
            warning: Some("No files found".to_owned()),
        };
        let report = generate(&kg, &analysis, &detection, &TokenCost::default(), "empty");
        assert!(report.contains("No files found"));
    }
}
