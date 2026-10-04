//! Topic bank for long-term memory (WEFT-733, RMM prospective reflection).
//!
//! The consolidator's extracted facts become **topic nodes** in a managed section of
//! `MEMORY.md` instead of being appended as another block. Each node has a deterministic
//! topic key (its UNID). A new fact whose key matches an existing node **merges** (the newer
//! fact replaces the older text, so a reversed preference or a corrected allergy does not sit
//! beside the old one); a fact with a new key is **inserted**. Same transcript in, same bank
//! out: re-running changes nothing. Offline and deterministic, like the rest of the
//! consolidator; an LLM summarizer can replace [`topic_key`] later behind the same shape.
//!
//! Key rules (see [`topic_key`]):
//! - negation and filler words are dropped, so "is allergic to penicillin" and "is not
//!   allergic to penicillin" share a key and the later one wins;
//! - single-valued attributes (where you live or work, your name, timezone, what you prefer,
//!   a favourite X) key on the attribute alone, so "I live in Denver" then "I live in Austin"
//!   merge; multi-valued facts (allergies, likes, things you have) keep their object.

/// Start of the managed section in `MEMORY.md`.
pub const BANK_OPEN: &str = "<!-- topics -->";
/// End of the managed section.
pub const BANK_CLOSE: &str = "<!-- /topics -->";

/// One topic node: `key` is the UNID, `text` the latest fact, `source` where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicNode {
    pub key: String,
    pub text: String,
    pub source: String,
}

/// What a merge did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MergeStats {
    pub merged: usize,
    pub inserted: usize,
    pub unchanged: usize,
}

impl MergeStats {
    pub fn changed(&self) -> bool {
        self.merged + self.inserted > 0
    }
}

const DROP: &[&str] = &[
    "not", "no", "never", "dont", "doesnt", "isnt", "arent", "wasnt", "werent", "cant", "wont", "anymore", "longer",
    "am", "is", "are", "was", "were", "be", "been", "a", "an", "the", "i", "im", "my", "me", "user", "users", "now",
    "really", "very", "actually", "please", "remember", "that", "note", "fact", "do", "does", "also", "still", "to",
    "correction", "update", "updated", "fyi", "btw", "so", "and", "but", "ok", "okay", "well", "just",
];

/// Split a fact into clauses so each statement gets its own topic: sentences, and
/// "X and Y" when both sides carry at least two content words ("I am allergic to penicillin
/// and I live in Denver" is two facts; "salt and pepper" is not split).
pub fn clauses(fact: &str) -> Vec<String> {
    let content = |s: &str| words(s).iter().filter(|w| !DROP.contains(&w.as_str())).count();
    let mut out = Vec::new();
    for sentence in fact.split(['.', ';', '!', '\n']) {
        let sentence = sentence.trim().trim_start_matches(|c: char| !c.is_alphanumeric()).trim();
        if sentence.is_empty() {
            continue;
        }
        let parts: Vec<&str> = sentence.split(" and ").collect();
        if parts.len() > 1 && parts.iter().all(|p| content(p) >= 2) {
            out.extend(parts.iter().map(|p| p.trim().to_owned()));
        } else {
            out.push(sentence.to_owned());
        }
    }
    out
}

/// Single-valued attribute cues: the key is the cue itself (the object is the value).
const SINGLE: &[(&str, &str)] = &[
    ("live in", "lives in"),
    ("lives in", "lives in"),
    ("living in", "lives in"),
    ("moved to", "lives in"),
    ("work at", "works at"),
    ("works at", "works at"),
    ("working at", "works at"),
    ("name is", "name"),
    ("call me", "name"),
    ("timezone is", "timezone"),
    ("time zone is", "timezone"),
    ("prefer", "prefers"),
    ("prefers", "prefers"),
    ("would rather", "prefers"),
    ("rather", "prefers"),
];

fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .map(|w| w.replace('\'', ""))
        .filter(|w| !w.is_empty())
        .collect()
}

/// The deterministic topic key (UNID) of a fact.
pub fn topic_key(fact: &str) -> String {
    let lower = format!(" {} ", words(fact).join(" "));
    for (cue, key) in SINGLE {
        if lower.contains(&format!(" {cue} ")) {
            return key.to_string();
        }
    }
    // "my favourite colour is blue" -> "favorite colour"
    let ws = words(fact);
    if let Some(i) = ws.iter().position(|w| w == "favorite" || w == "favourite") {
        let attr: Vec<&str> = ws[i + 1..].iter().take_while(|w| *w != "is" && *w != "are").map(String::as_str).collect();
        if !attr.is_empty() {
            return format!("favorite {}", attr.join(" "));
        }
    }
    ws.into_iter().filter(|w| !DROP.contains(&w.as_str())).collect::<Vec<_>>().join(" ")
}

/// Split `MEMORY.md` into the text outside the bank and the bank's nodes.
pub fn parse_bank(md: &str) -> (String, Vec<TopicNode>) {
    let (Some(open), Some(close)) = (md.find(BANK_OPEN), md.find(BANK_CLOSE)) else {
        return (md.to_owned(), Vec::new());
    };
    if close < open {
        return (md.to_owned(), Vec::new());
    }
    let body = &md[open + BANK_OPEN.len()..close];
    let rest = format!("{}{}", &md[..open], &md[close + BANK_CLOSE.len()..]);
    let nodes = body
        .split("\n\n")
        .filter_map(|block| {
            let block = block.trim();
            let key = block.strip_prefix("<!-- topic:key=")?.split(" -->").next()?.to_owned();
            let line = block.lines().nth(1)?.trim();
            let text = line.strip_prefix("- ")?;
            let (text, source) = match text.rsplit_once(" _(from ") {
                Some((t, s)) => (t.to_owned(), s.trim_end_matches(")_").to_owned()),
                None => (text.to_owned(), String::new()),
            };
            Some(TopicNode { key, text, source })
        })
        .collect();
    (rest.trim_end().to_owned(), nodes)
}

/// Render the bank section (nodes separated by blank lines, so each node is one snippet for
/// retrieval).
pub fn render_bank(nodes: &[TopicNode]) -> String {
    let mut s = format!("{BANK_OPEN}\n");
    for n in nodes {
        let src = if n.source.is_empty() { String::new() } else { format!(" _(from {})_", n.source) };
        s.push_str(&format!("\n<!-- topic:key={} -->\n- {}{}\n", n.key, n.text, src));
    }
    s.push_str(&format!("\n{BANK_CLOSE}"));
    s
}

/// Merge `facts` (in transcript order, so a later fact about the same topic wins) into
/// `bank`. Same key: replace the text (merge) unless it is identical (unchanged). New key:
/// append (insert).
pub fn merge(mut bank: Vec<TopicNode>, facts: &[String], source: &str) -> (Vec<TopicNode>, MergeStats) {
    let mut stats = MergeStats::default();
    let split: Vec<String> = facts.iter().flat_map(|f| clauses(f)).collect();
    for fact in &split {
        let key = topic_key(fact);
        if key.is_empty() {
            continue;
        }
        match bank.iter_mut().find(|n| n.key == key) {
            Some(n) if n.text == *fact => stats.unchanged += 1,
            Some(n) => {
                n.text = fact.clone();
                n.source = source.to_owned();
                stats.merged += 1;
            }
            None => {
                bank.push(TopicNode { key, text: fact.clone(), source: source.to_owned() });
                stats.inserted += 1;
            }
        }
    }
    (bank, stats)
}

/// The full `MEMORY.md` after merging `facts`, and what changed.
pub fn merge_into_memory(md: &str, facts: &[String], source: &str) -> (String, MergeStats) {
    let (rest, bank) = parse_bank(md);
    let (bank, stats) = merge(bank, facts, source);
    let rendered = render_bank(&bank);
    let out = if rest.trim().is_empty() { rendered } else { format!("{rest}\n\n{rendered}") };
    (out, stats)
}

#[cfg(test)]
#[path = "topics_tests.rs"]
mod tests;
