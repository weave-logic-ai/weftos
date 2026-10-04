use super::*;

/// Retriever stub: returns snippets whose text contains any query word, in document order.
struct WordRetriever;
impl MemoryRetriever for WordRetriever {
    fn top_k(&self, query: &str, snippets: &[MemorySnippet], k: usize) -> Vec<MemorySnippet> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        snippets
            .iter()
            .filter(|s| words.iter().any(|w| s.text.to_lowercase().contains(w.as_str())))
            .take(k)
            .cloned()
            .collect()
    }
}

/// One observed turn: the query and `(snippet text, reward)` per candidate.
type Observed = (String, Vec<(String, i8)>);

/// Reranker mock: reverses order (to prove rerank is applied) and records every observe call.
#[derive(Default)]
struct RecordingReranker {
    reverse: bool,
    seen: Mutex<Vec<Observed>>,
}
impl MemoryReranker for RecordingReranker {
    fn rerank(&self, _q: &str, mut c: Vec<MemorySnippet>) -> Vec<MemorySnippet> {
        if self.reverse {
            c.reverse();
        }
        c
    }
    fn observe(&self, q: &str, rewards: &[(MemorySnippet, i8)]) {
        self.seen.lock().unwrap().push((q.to_owned(), rewards.iter().map(|(s, r)| (s.text.clone(), *r)).collect()));
    }
}

const MEM: &str = "# Pets\n\nThe user has a dog named Rex.\n\nThe user is allergic to cats.\n\nThe user lives in Denver.\n\nThe user prefers tea over coffee.";

fn recall(reranker: Arc<RecordingReranker>, k: usize, m: usize) -> MemoryRecall {
    MemoryRecall::new(Arc::new(WordRetriever), reranker, k, m)
}

#[test]
fn snippets_split_on_blank_lines_keep_headings_and_have_stable_keys() {
    let s = split_snippets(MEM);
    assert_eq!(s.len(), 4);
    assert_eq!(s[0].text, "# Pets\nThe user has a dog named Rex.");
    assert_eq!(split_snippets(MEM)[2].key, s[2].key, "same text, same key");
    assert_ne!(s[1].key, s[2].key);
    assert!(split_snippets("  \n\n \n").is_empty());
}

#[test]
fn select_caps_at_top_m_labels_in_order_and_applies_the_rerank() {
    let rr = Arc::new(RecordingReranker { reverse: true, ..Default::default() });
    let r = recall(rr, 20, 2);
    // "user" matches all four; the reranker reverses; top_m = 2.
    let picked = r.select("s1", "user", MEM);
    assert_eq!(picked.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(), ["m1", "m2"]);
    assert_eq!(picked[0].1.text, "The user prefers tea over coffee.");
    assert_eq!(picked[1].1.text, "The user lives in Denver.");
    let body = MemoryRecall::render(&picked);
    assert!(body.starts_with(RECALL_HEADER));
    assert!(body.contains("[m1] The user prefers tea") && body.contains("[m2] The user lives in Denver"));
    assert!(!body.contains("Rex") && !body.contains("allergic"), "only the picked snippets are injected");
}

#[test]
fn nothing_retrieved_or_no_query_selects_nothing_so_the_caller_fails_open() {
    let r = recall(Arc::new(RecordingReranker::default()), 20, 5);
    assert!(r.select("s1", "zebra", MEM).is_empty());
    assert!(r.select("s1", "   ", MEM).is_empty());
    assert!(r.select("s1", "user", "").is_empty());
}

#[test]
fn citations_reward_cited_plus_one_ignored_minus_one_and_strip_markers() {
    let rr = Arc::new(RecordingReranker::default());
    let r = recall(rr.clone(), 20, 3);
    let picked = r.select("s1", "user", MEM); // m1 Rex, m2 cats, m3 Denver (doc order)
    assert_eq!(picked.len(), 3);
    let out = r.attribute("s1", "Rex is your dog [m1], and you live in Denver [m3].");
    assert_eq!(out, "Rex is your dog, and you live in Denver.");
    let seen = rr.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let (q, rewards) = &seen[0];
    assert_eq!(q, "user");
    assert_eq!(
        rewards,
        &vec![
            ("# Pets\nThe user has a dog named Rex.".to_string(), 1),
            ("The user is allergic to cats.".to_string(), -1),
            ("The user lives in Denver.".to_string(), 1),
        ]
    );
    // The tea snippet was never retrieved (top_m = 3): it gets no reward at all.
    assert!(!rewards.iter().any(|(t, _)| t.contains("tea")));
}

#[test]
fn attribution_is_once_per_selection_and_per_session() {
    let rr = Arc::new(RecordingReranker::default());
    let r = recall(rr.clone(), 20, 2);
    r.select("a", "user", MEM);
    // Another session's reply does not consume session a's selection.
    assert_eq!(r.attribute("b", "text [m1]"), "text [m1]");
    r.attribute("a", "no citations");
    r.attribute("a", "again [m1]"); // nothing pending any more
    let seen = rr.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].1.iter().all(|(_, r)| *r == -1), "nothing cited: every retrieved id is -1");
}

#[test]
fn a_poisoned_memory_file_is_not_reinjected_whole_when_retrieval_is_on() {
    // WEFT-665 class: graft debris in MEMORY.md. Retrieval injects only what matched.
    let poisoned = format!("{MEM}\n\n```\n\n\n```\n\n[[graft debris 0xdeadbeef]]\n\n<empty block>");
    let r = recall(Arc::new(RecordingReranker::default()), 20, 5);
    let picked = r.select("s1", "dog", &poisoned);
    let body = MemoryRecall::render(&picked);
    assert!(body.contains("Rex"));
    assert!(!body.contains("graft debris") && !body.contains("<empty block>"), "{body}");
}

#[test]
fn cited_labels_parses_only_well_formed_markers() {
    assert_eq!(cited_labels("a [m1] b [m12] c [m1] [mx] [m] m3] [m4"), vec!["m1", "m12"]);
    assert!(cited_labels("").is_empty());
}

#[test]
fn identity_reranker_preserves_retriever_order() {
    let s = split_snippets(MEM);
    assert_eq!(IdentityMemoryReranker.rerank("q", s.clone()), s);
}

#[cfg(feature = "vector-memory")]
#[test]
fn hash_retriever_ranks_the_matching_snippet_first() {
    let s = split_snippets(MEM);
    let top = HashRetriever::default().top_k("tea or coffee preference", &s, 2);
    assert_eq!(top.len(), 2);
    assert_eq!(top[0].text, "The user prefers tea over coffee.");
}

#[test]
fn disabled_config_builds_no_recall() {
    assert!(from_config(&clawft_types::config::MemoryRecallConfig::default()).is_none());
}
