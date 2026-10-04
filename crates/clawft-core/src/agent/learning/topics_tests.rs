use super::*;

fn f(s: &str) -> String {
    s.to_owned()
}

#[test]
fn negation_shares_a_key_so_a_correction_merges() {
    assert_eq!(topic_key("I am allergic to penicillin"), topic_key("I'm not allergic to penicillin anymore"));
    assert_ne!(topic_key("I am allergic to penicillin"), topic_key("I am allergic to cats"));
}

#[test]
fn single_valued_attributes_key_on_the_attribute() {
    assert_eq!(topic_key("I live in Denver"), "lives in");
    assert_eq!(topic_key("We moved to Austin last year"), "lives in");
    assert_eq!(topic_key("I prefer tea over coffee"), topic_key("I'd rather have coffee now"));
    assert_eq!(topic_key("My favourite colour is blue"), "favorite colour");
    assert_eq!(topic_key("my favorite colour is green"), "favorite colour");
}

#[test]
fn merge_supersedes_the_earlier_allergy_node_instead_of_sitting_beside_it() {
    let (md, s1) = merge_into_memory("", &[f("I am allergic to penicillin")], "conv-1");
    assert_eq!((s1.inserted, s1.merged), (1, 0));
    let (md, s2) = merge_into_memory(&md, &[f("I am not allergic to penicillin")], "conv-2");
    assert_eq!((s2.inserted, s2.merged), (0, 1));
    let (_, nodes) = parse_bank(&md);
    assert_eq!(nodes.len(), 1, "{md}");
    assert_eq!(nodes[0].text, "I am not allergic to penicillin");
    assert_eq!(nodes[0].source, "conv-2");
    assert!(!md.contains("I am allergic to penicillin"));
}

#[test]
fn a_new_topic_inserts_without_touching_unrelated_nodes() {
    let (md, _) = merge_into_memory("", &[f("I live in Denver"), f("I am allergic to cats")], "conv-1");
    let (md, s) = merge_into_memory(&md, &[f("My favourite colour is blue")], "conv-2");
    assert_eq!((s.inserted, s.merged, s.unchanged), (1, 0, 0));
    let (_, nodes) = parse_bank(&md);
    assert_eq!(nodes.len(), 3);
    assert_eq!(nodes[0], TopicNode { key: "lives in".into(), text: "I live in Denver".into(), source: "conv-1".into() });
    assert_eq!(nodes[1].text, "I am allergic to cats");
}

#[test]
fn re_running_the_same_transcript_is_idempotent() {
    let facts = [f("I live in Denver"), f("I prefer tea"), f("I am allergic to cats")];
    let (once, s1) = merge_into_memory("", &facts, "conv-1");
    let (twice, s2) = merge_into_memory(&once, &facts, "conv-1");
    assert!(s1.changed());
    assert!(!s2.changed(), "{s2:?}");
    assert_eq!(s2.unchanged, 3);
    assert_eq!(once, twice);
}

#[test]
fn within_one_transcript_the_later_fact_wins() {
    let (md, s) = merge_into_memory("", &[f("I live in Denver"), f("Actually I live in Austin now")], "conv-1");
    assert_eq!((s.inserted, s.merged), (1, 1));
    let (_, nodes) = parse_bank(&md);
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].text, "Actually I live in Austin now");
}

#[test]
fn text_outside_the_bank_is_kept() {
    let start = "# Notes\n\nHand-written note.";
    let (md, _) = merge_into_memory(start, &[f("I live in Denver")], "c");
    assert!(md.starts_with("# Notes\n\nHand-written note.\n\n<!-- topics -->"), "{md}");
    let (rest, nodes) = parse_bank(&md);
    assert_eq!(rest, start);
    assert_eq!(nodes.len(), 1);
}

#[test]
fn bank_round_trips_and_nodes_are_separate_paragraphs() {
    let nodes = vec![
        TopicNode { key: "lives in".into(), text: "I live in Denver".into(), source: "c1".into() },
        TopicNode { key: "allergic cats".into(), text: "I am allergic to cats".into(), source: String::new() },
    ];
    let md = render_bank(&nodes);
    assert_eq!(parse_bank(&md).1, nodes);
    // Each node is its own blank-line paragraph, so retrieval sees one snippet per topic.
    let paras: Vec<&str> = md.split("\n\n").filter(|p| p.contains("topic:key=")).collect();
    assert_eq!(paras.len(), 2);
}

#[test]
fn compound_facts_split_into_one_node_per_statement() {
    assert_eq!(
        clauses("I am allergic to penicillin and I live in Denver."),
        vec!["I am allergic to penicillin", "I live in Denver"]
    );
    assert_eq!(clauses("I like salt and pepper"), vec!["I like salt and pepper"], "not split: one side lacks content");
    let (md, s) = merge_into_memory("", &[f("I am allergic to penicillin and I live in Denver.")], "c1");
    assert_eq!(s.inserted, 2, "{md}");
}

#[test]
fn lead_in_words_do_not_change_the_key() {
    assert_eq!(topic_key("Correction: I am not allergic to penicillin"), topic_key("I am allergic to penicillin"));
    assert_eq!(topic_key("Update - I am allergic to cats"), topic_key("I am allergic to cats"));
}
