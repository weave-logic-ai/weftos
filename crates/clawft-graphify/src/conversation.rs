//! KG-016: Conversational graph exploration.
//!
//! Provides a stateful [`ConversationContext`] that tracks multi-turn
//! dialogue over a knowledge graph. Each query narrows focus to
//! relevant entities, records what has been explored, and suggests
//! unexplored neighbors as follow-up questions.
//!
//! ## Reference
//!
//! Paper 12 -- "From Data to Dialogue" (structured dialogue patterns
//! for graph exploration).

use crate::entity::EntityId;
use crate::model::KnowledgeGraph;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Conversation context for multi-turn graph exploration.
///
/// Maintains focus entities, a visited set, and a topic breadcrumb
/// stack across successive queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationContext {
    /// Current entities being discussed.
    pub focus_entities: Vec<EntityId>,
    /// Entities already explored in this conversation.
    pub visited: HashSet<EntityId>,
    /// Breadcrumb stack of topic descriptions.
    pub topic_stack: Vec<String>,
    /// Number of turns so far.
    pub turn_count: usize,
}

/// Result of a single conversational query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationResponse {
    /// Matched entities with relevance scores (descending).
    pub results: Vec<(EntityId, f64)>,
    /// Human-readable explanation of the results.
    pub explanation: String,
    /// Suggested follow-up questions.
    pub followups: Vec<String>,
    /// Whether the query caused a topic shift (no overlap with
    /// previous focus entities).
    pub topic_shift: bool,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl Default for ConversationContext {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversationContext {
    /// Create a fresh conversation with no history.
    pub fn new() -> Self {
        Self {
            focus_entities: Vec::new(),
            visited: HashSet::new(),
            topic_stack: Vec::new(),
            turn_count: 0,
        }
    }

    /// Process a query in context, returning relevant results and
    /// suggested follow-ups.
    ///
    /// The algorithm:
    /// 1. Tokenize the question into lowercase keywords.
    /// 2. Score every entity by keyword overlap with its label.
    /// 3. Boost entities that are neighbors of current focus (context
    ///    continuity bonus).
    /// 4. Detect topic shifts when no results overlap with focus.
    /// 5. Update focus, visited set, and topic stack.
    /// 6. Generate follow-up suggestions from unexplored neighbors.
    pub fn query(
        &mut self,
        kg: &KnowledgeGraph,
        question: &str,
    ) -> ConversationResponse {
        self.turn_count += 1;

        let keywords = tokenize(question);
        if keywords.is_empty() {
            return ConversationResponse {
                results: Vec::new(),
                explanation: "No keywords found in query.".into(),
                followups: self.suggest_followups(kg),
                topic_shift: false,
            };
        }

        // Score entities by keyword match.
        let mut scored: Vec<(EntityId, f64)> = kg
            .entities()
            .filter_map(|e| {
                let label_lower = e.label.to_lowercase();
                let label_tokens: Vec<&str> = label_lower
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .filter(|s| !s.is_empty())
                    .collect();

                let mut score = 0.0;
                for kw in &keywords {
                    // Exact token match.
                    if label_tokens.iter().any(|t| t == kw) {
                        score += 1.0;
                    }
                    // Substring match (partial credit).
                    else if label_lower.contains(kw.as_str()) {
                        score += 0.5;
                    }
                }
                if score > 0.0 {
                    Some((e.id.clone(), score))
                } else {
                    None
                }
            })
            .collect();

        // Context continuity: boost entities that are neighbors of focus.
        let focus_set: HashSet<&EntityId> = self.focus_entities.iter().collect();
        for (id, score) in &mut scored {
            if focus_set.contains(id) {
                *score += 0.3; // direct focus bonus
            }
            // Check if this entity is a neighbor of any focus entity.
            for focus_id in &self.focus_entities {
                let neighbors = kg.neighbors(focus_id);
                if neighbors.iter().any(|n| n.id == *id) {
                    *score += 0.2; // neighbor bonus
                    break;
                }
            }
        }

        // Sort descending.
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
        });

        // Limit to top results.
        scored.truncate(10);

        // Detect topic shift.
        let result_ids: HashSet<EntityId> =
            scored.iter().map(|(id, _)| id.clone()).collect();
        let topic_shift = !self.focus_entities.is_empty()
            && self
                .focus_entities
                .iter()
                .all(|f| !result_ids.contains(f))
            && !scored.is_empty();

        // Build explanation.
        let explanation = if scored.is_empty() {
            format!(
                "No entities matched the query '{}'. Try broader terms.",
                question
            )
        } else if topic_shift {
            format!(
                "Topic shift detected. Found {} entities matching '{}'. \
                 Previous focus has been replaced.",
                scored.len(),
                question
            )
        } else {
            format!(
                "Found {} entities matching '{}' (turn {}).",
                scored.len(),
                question,
                self.turn_count
            )
        };

        // Update state.
        if topic_shift
            || (!scored.is_empty()
                && (self.topic_stack.is_empty()
                    || self.topic_stack.last().map(|s| s.as_str()) != Some(question)))
        {
            self.topic_stack.push(question.to_string());
        }

        // Update focus to new results.
        self.focus_entities = scored.iter().map(|(id, _)| id.clone()).collect();

        // Mark as visited.
        for (id, _) in &scored {
            self.visited.insert(id.clone());
        }

        let followups = self.suggest_followups(kg);

        ConversationResponse {
            results: scored,
            explanation,
            followups,
            topic_shift,
        }
    }

    /// Suggest follow-up questions based on unexplored neighbors of
    /// the current focus entities.
    pub fn suggest_followups(&self, kg: &KnowledgeGraph) -> Vec<String> {
        let mut suggestions = Vec::new();
        let mut seen_labels: HashSet<String> = HashSet::new();

        for focus_id in &self.focus_entities {
            let neighbors = kg.neighbors(focus_id);
            for neighbor in neighbors {
                if !self.visited.contains(&neighbor.id)
                    && seen_labels.insert(neighbor.label.clone())
                {
                    let focus_label = kg
                        .entity(focus_id)
                        .map(|e| e.label.as_str())
                        .unwrap_or("?");
                    suggestions.push(format!(
                        "Tell me about '{}' (connected to '{}')",
                        neighbor.label, focus_label
                    ));
                }
            }
        }

        // Limit suggestions.
        suggestions.truncate(5);
        suggestions
    }

    /// Reset the conversation to a clean state.
    pub fn reset(&mut self) {
        self.focus_entities.clear();
        self.visited.clear();
        self.topic_stack.clear();
        self.turn_count = 0;
    }

    /// Return the current topic (last item on the topic stack).
    pub fn current_topic(&self) -> Option<&str> {
        self.topic_stack.last().map(|s| s.as_str())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Tokenize a question into lowercase keywords, filtering stop words.
fn tokenize(question: &str) -> Vec<String> {
    const STOP_WORDS: &[&str] = &[
        "a", "an", "the", "is", "are", "was", "were", "be", "been", "being",
        "have", "has", "had", "do", "does", "did", "will", "would", "shall",
        "should", "may", "might", "must", "can", "could", "about", "above",
        "after", "again", "all", "also", "am", "and", "any", "as", "at",
        "because", "before", "between", "both", "but", "by", "came", "come",
        "each", "for", "from", "get", "got", "he", "her", "here", "him",
        "his", "how", "i", "if", "in", "into", "it", "its", "just", "know",
        "let", "like", "make", "me", "more", "most", "my", "no", "not", "now",
        "of", "on", "one", "only", "or", "other", "our", "out", "over",
        "said", "same", "she", "so", "some", "still", "such", "take", "tell",
        "than", "that", "their", "them", "then", "there", "these", "they",
        "this", "those", "through", "to", "too", "under", "up", "very",
        "want", "what", "when", "where", "which", "while", "who", "why",
        "with", "you", "your",
    ];

    question
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| !w.is_empty() && w.len() > 1 && !STOP_WORDS.contains(w))
        .map(|w| w.to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{DomainTag, EntityType, FileType};
    use crate::model::Entity;
    use crate::relationship::{Confidence, RelationType, Relationship};

    fn entity(name: &str) -> Entity {
        Entity {
            id: EntityId::new(&DomainTag::Code, &EntityType::Module, name, "test.rs"),
            entity_type: EntityType::Module,
            label: name.to_string(),
            source_file: Some("test.rs".into()),
            source_location: None,
            file_type: FileType::Code,
            metadata: serde_json::json!({}),
            legacy_id: None,
            iri: None,
        }
    }

    fn rel(src: &Entity, tgt: &Entity) -> Relationship {
        Relationship {
            source: src.id.clone(),
            target: tgt.id.clone(),
            relation_type: RelationType::Imports,
            confidence: Confidence::Extracted,
            weight: 1.0,
            source_file: None,
            source_location: None,
            metadata: serde_json::json!({}),
        }
    }

    fn sample_kg() -> KnowledgeGraph {
        let auth = entity("auth_service");
        let db = entity("db_pool");
        let config = entity("config_loader");
        let cache = entity("cache_layer");

        let mut kg = KnowledgeGraph::new();
        kg.add_entity(auth.clone());
        kg.add_entity(db.clone());
        kg.add_entity(config.clone());
        kg.add_entity(cache.clone());
        kg.add_relationship(rel(&auth, &db));
        kg.add_relationship(rel(&auth, &config));
        kg.add_relationship(rel(&db, &cache));
        kg
    }

    #[test]
    fn tokenize_filters_stop_words() {
        let tokens = tokenize("What is the auth service?");
        assert!(tokens.contains(&"auth".to_string()));
        assert!(tokens.contains(&"service".to_string()));
        assert!(!tokens.contains(&"what".to_string()));
        assert!(!tokens.contains(&"is".to_string()));
        assert!(!tokens.contains(&"the".to_string()));
    }

    #[test]
    fn tokenize_empty() {
        let tokens = tokenize("the is a");
        assert!(tokens.is_empty());
    }

    #[test]
    fn new_context() {
        let ctx = ConversationContext::new();
        assert!(ctx.focus_entities.is_empty());
        assert!(ctx.visited.is_empty());
        assert!(ctx.topic_stack.is_empty());
        assert_eq!(ctx.turn_count, 0);
    }

    #[test]
    fn default_context() {
        let ctx = ConversationContext::default();
        assert_eq!(ctx.turn_count, 0);
    }

    #[test]
    fn query_finds_matching_entity() {
        let kg = sample_kg();
        let mut ctx = ConversationContext::new();

        let response = ctx.query(&kg, "auth service");
        assert!(!response.results.is_empty(), "Expected results for 'auth service'");
        assert!(!response.topic_shift);
        assert_eq!(ctx.turn_count, 1);

        // auth_service should be in focus.
        let auth_id = EntityId::new(&DomainTag::Code, &EntityType::Module, "auth_service", "test.rs");
        assert!(ctx.focus_entities.contains(&auth_id));
        assert!(ctx.visited.contains(&auth_id));
    }

    #[test]
    fn query_no_results() {
        let kg = sample_kg();
        let mut ctx = ConversationContext::new();

        let response = ctx.query(&kg, "zzzzz_nonexistent");
        assert!(response.results.is_empty());
        assert!(
            response.explanation.contains("No entities matched"),
            "Expected 'no match' explanation"
        );
    }

    #[test]
    fn topic_shift_detected() {
        let kg = sample_kg();
        let mut ctx = ConversationContext::new();

        // First query: focus on auth.
        ctx.query(&kg, "auth");
        assert!(!ctx.focus_entities.is_empty());

        // Second query: completely different topic.
        let response = ctx.query(&kg, "cache layer");
        assert!(response.topic_shift, "Expected topic shift");
        assert!(response.explanation.contains("Topic shift"));
    }

    #[test]
    fn no_topic_shift_when_continuing() {
        let kg = sample_kg();
        let mut ctx = ConversationContext::new();

        // First query: auth.
        ctx.query(&kg, "auth");

        // Second query: db -- neighbor of auth, should be boosted.
        let response = ctx.query(&kg, "auth db");
        // Not a topic shift because auth is still in results.
        assert!(!response.topic_shift);
    }

    #[test]
    fn followups_suggest_unexplored() {
        let kg = sample_kg();
        let mut ctx = ConversationContext::new();

        ctx.query(&kg, "auth service");
        let followups = ctx.suggest_followups(&kg);
        // Should suggest neighbors of auth that haven't been visited.
        assert!(
            !followups.is_empty(),
            "Expected follow-up suggestions for unexplored neighbors"
        );
    }

    #[test]
    fn visited_set_grows() {
        let kg = sample_kg();
        let mut ctx = ConversationContext::new();

        ctx.query(&kg, "auth");
        let visited_after_1 = ctx.visited.len();

        ctx.query(&kg, "db pool");
        assert!(
            ctx.visited.len() >= visited_after_1,
            "Visited set should grow"
        );
    }

    #[test]
    fn turn_count_increments() {
        let kg = sample_kg();
        let mut ctx = ConversationContext::new();

        ctx.query(&kg, "auth");
        ctx.query(&kg, "db");
        ctx.query(&kg, "config");
        assert_eq!(ctx.turn_count, 3);
    }

    #[test]
    fn reset_clears_state() {
        let kg = sample_kg();
        let mut ctx = ConversationContext::new();

        ctx.query(&kg, "auth");
        ctx.reset();
        assert!(ctx.focus_entities.is_empty());
        assert!(ctx.visited.is_empty());
        assert!(ctx.topic_stack.is_empty());
        assert_eq!(ctx.turn_count, 0);
    }

    #[test]
    fn current_topic_tracks_stack() {
        let kg = sample_kg();
        let mut ctx = ConversationContext::new();

        assert!(ctx.current_topic().is_none());
        ctx.query(&kg, "auth service");
        assert_eq!(ctx.current_topic(), Some("auth service"));
    }

    #[test]
    fn empty_query_returns_no_results() {
        let kg = sample_kg();
        let mut ctx = ConversationContext::new();

        let response = ctx.query(&kg, "the is a");
        assert!(response.results.is_empty());
        assert!(response.explanation.contains("No keywords"));
    }

    #[test]
    fn context_continuity_boost() {
        let kg = sample_kg();
        let mut ctx = ConversationContext::new();

        // Focus on auth first.
        ctx.query(&kg, "auth service");

        // Query for "db" -- db_pool is a neighbor of auth_service,
        // so it should get a continuity boost.
        let response = ctx.query(&kg, "db");
        assert!(!response.results.is_empty());
        // db_pool should be the top result.
        let db_id = EntityId::new(&DomainTag::Code, &EntityType::Module, "db_pool", "test.rs");
        assert_eq!(
            response.results[0].0, db_id,
            "Expected db_pool as top result with continuity boost"
        );
    }

    #[test]
    fn empty_graph_returns_empty() {
        let kg = KnowledgeGraph::new();
        let mut ctx = ConversationContext::new();

        let response = ctx.query(&kg, "anything");
        assert!(response.results.is_empty());
        assert!(response.followups.is_empty());
    }
}
