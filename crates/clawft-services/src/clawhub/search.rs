//! Skill search with vector search integration and keyword fallback.
//!
//! When the H2 vector store (from Element 08) is available, uses semantic
//! search via embeddings. Falls back to keyword-based search otherwise.

use serde::{Deserialize, Serialize};

use super::registry::SkillEntry;

/// Search result with relevance score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSearchResult {
    /// The matched skill entry.
    pub skill: SkillEntry,
    /// Relevance score (0.0 to 1.0, higher is better).
    pub score: f32,
    /// Whether the match was via vector (semantic) or keyword search.
    pub match_type: MatchType,
}

/// How a search result was matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchType {
    /// Matched via vector/semantic search (HNSW).
    Vector,
    /// Matched via keyword/text search (fallback).
    Keyword,
}

/// Keyword-based search over skill entries.
///
/// Used as fallback when vector search (H2) is unavailable.
pub fn keyword_search(skills: &[SkillEntry], query: &str, limit: usize) -> Vec<SkillSearchResult> {
    let query_lower = query.to_lowercase();
    let query_terms: Vec<&str> = query_lower.split_whitespace().collect();

    let mut scored: Vec<(f32, &SkillEntry)> = skills
        .iter()
        .filter_map(|skill| {
            let score = compute_keyword_score(skill, &query_terms);
            if score > 0.0 {
                Some((score, skill))
            } else {
                None
            }
        })
        .collect();

    // Sort by score descending.
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    scored
        .into_iter()
        .take(limit)
        .map(|(score, skill)| SkillSearchResult {
            skill: skill.clone(),
            score,
            match_type: MatchType::Keyword,
        })
        .collect()
}

/// Compute a keyword relevance score for a skill entry.
fn compute_keyword_score(skill: &SkillEntry, query_terms: &[&str]) -> f32 {
    let mut score = 0.0_f32;
    let name_lower = skill.name.to_lowercase();
    let desc_lower = skill.description.to_lowercase();

    for term in query_terms {
        // Name matches are worth more.
        if name_lower.contains(term) {
            score += 2.0;
        }
        // Description matches.
        if desc_lower.contains(term) {
            score += 1.0;
        }
        // Tag matches.
        for tag in &skill.tags {
            if tag.to_lowercase().contains(term) {
                score += 1.5;
            }
        }
    }

    // Normalize to 0.0-1.0 range.
    let max_possible = query_terms.len() as f32 * 4.5; // max per term
    if max_possible > 0.0 {
        (score / max_possible).min(1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_skills() -> Vec<SkillEntry> {
        vec![
            SkillEntry {
                id: "skill-001".into(),
                name: "coding-agent".into(),
                description: "An AI coding assistant for writing code".into(),
                version: "1.0.0".into(),
                author: "clawft".into(),
                stars: 42,
                content_hash: "hash1".into(),
                signed: true,
                signature: Some("sig1".into()),
                published_at: "2024-01-01T00:00:00Z".into(),
                tags: vec!["coding".into(), "ai".into()],
            },
            SkillEntry {
                id: "skill-002".into(),
                name: "web-search".into(),
                description: "Search the web and summarize results".into(),
                version: "1.0.0".into(),
                author: "clawft".into(),
                stars: 30,
                content_hash: "hash2".into(),
                signed: true,
                signature: Some("sig2".into()),
                published_at: "2024-01-02T00:00:00Z".into(),
                tags: vec!["search".into(), "web".into()],
            },
            SkillEntry {
                id: "skill-003".into(),
                name: "file-management".into(),
                description: "Manage files and directories".into(),
                version: "1.0.0".into(),
                author: "clawft".into(),
                stars: 15,
                content_hash: "hash3".into(),
                signed: true,
                signature: Some("sig3".into()),
                published_at: "2024-01-03T00:00:00Z".into(),
                tags: vec!["files".into(), "filesystem".into()],
            },
        ]
    }

    #[test]
    fn keyword_search_finds_matching_skills() {
        let skills = sample_skills();
        let results = keyword_search(&skills, "coding", 10);
        assert!(!results.is_empty());
        assert_eq!(results[0].skill.name, "coding-agent");
    }

    #[test]
    fn keyword_search_respects_limit() {
        let skills = sample_skills();
        let results = keyword_search(&skills, "a", 1);
        assert!(results.len() <= 1);
    }

    #[test]
    fn keyword_search_empty_query_returns_nothing() {
        let skills = sample_skills();
        let results = keyword_search(&skills, "", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn keyword_search_no_match() {
        let skills = sample_skills();
        let results = keyword_search(&skills, "quantum_physics_simulator", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn keyword_search_tag_match() {
        let skills = sample_skills();
        let results = keyword_search(&skills, "filesystem", 10);
        assert!(!results.is_empty());
        assert_eq!(results[0].skill.name, "file-management");
    }

    #[test]
    fn match_type_is_keyword() {
        let skills = sample_skills();
        let results = keyword_search(&skills, "search", 10);
        for r in &results {
            assert_eq!(r.match_type, MatchType::Keyword);
        }
    }
}
