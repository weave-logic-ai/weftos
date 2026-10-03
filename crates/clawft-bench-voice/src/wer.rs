//! Word Error Rate (WER) against reference transcripts.
//!
//! Classic ASR metric:
//!
//! ```text
//! WER = (S + D + I) / N
//! ```
//!
//! where `S` = substitutions, `D` = deletions, `I` = insertions, and `N` is
//! the number of words in the **reference**. When the reference is empty and
//! the hypothesis is non-empty, WER is defined as `1.0` (total error); both
//! empty → `0.0`.
//!
//! Alignment uses word-level Levenshtein (Wagner–Fischer). Normalization is
//! intentionally light (lowercase + Unicode whitespace split) so fixture
//! English corpora stay predictable without pulling a full NLP stack.

use serde::{Deserialize, Serialize};

/// Result of a single reference/hypothesis comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WerResult {
    /// Reference transcript (as provided).
    pub reference: String,
    /// Hypothesis transcript (as provided).
    pub hypothesis: String,
    /// Substitution count.
    pub substitutions: usize,
    /// Deletion count.
    pub deletions: usize,
    /// Insertion count.
    pub insertions: usize,
    /// Reference word count `N` (denominator; 0 when reference empty).
    pub reference_words: usize,
    /// Hypothesis word count.
    pub hypothesis_words: usize,
    /// `(S + D + I) / N`, or the empty-reference convention above.
    pub wer: f64,
}

impl WerResult {
    /// Total edit operations `S + D + I`.
    pub fn edits(&self) -> usize {
        self.substitutions + self.deletions + self.insertions
    }

    /// True when hypothesis exactly matches reference under normalization.
    pub fn is_perfect(&self) -> bool {
        self.wer == 0.0
    }
}

/// Tokenize for WER: lowercase, split on Unicode whitespace, drop empties.
pub fn normalize_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| w.to_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Compute WER for one reference / hypothesis pair.
pub fn word_error_rate(reference: &str, hypothesis: &str) -> WerResult {
    let ref_words = normalize_words(reference);
    let hyp_words = normalize_words(hypothesis);
    let (s, d, i) = align_counts(&ref_words, &hyp_words);
    let n = ref_words.len();
    let wer = if n == 0 {
        if hyp_words.is_empty() {
            0.0
        } else {
            1.0
        }
    } else {
        (s + d + i) as f64 / n as f64
    };
    WerResult {
        reference: reference.to_string(),
        hypothesis: hypothesis.to_string(),
        substitutions: s,
        deletions: d,
        insertions: i,
        reference_words: n,
        hypothesis_words: hyp_words.len(),
        wer,
    }
}

/// Corpus-level WER: micro-average over utterances
/// (`total_edits / total_ref_words`). Empty corpus → `0.0`.
pub fn word_error_rate_corpus(pairs: &[(&str, &str)]) -> WerResult {
    let mut total_s = 0usize;
    let mut total_d = 0usize;
    let mut total_i = 0usize;
    let mut total_n = 0usize;
    let mut total_hyp = 0usize;
    let mut refs = Vec::new();
    let mut hyps = Vec::new();

    for (r, h) in pairs {
        let one = word_error_rate(r, h);
        total_s += one.substitutions;
        total_d += one.deletions;
        total_i += one.insertions;
        total_n += one.reference_words;
        total_hyp += one.hypothesis_words;
        refs.push((*r).to_string());
        hyps.push((*h).to_string());
    }

    let wer = if total_n == 0 {
        if total_hyp == 0 {
            0.0
        } else {
            1.0
        }
    } else {
        (total_s + total_d + total_i) as f64 / total_n as f64
    };

    WerResult {
        reference: refs.join(" | "),
        hypothesis: hyps.join(" | "),
        substitutions: total_s,
        deletions: total_d,
        insertions: total_i,
        reference_words: total_n,
        hypothesis_words: total_hyp,
        wer,
    }
}

/// Built-in tiny English fixture corpus for CI (no audio files required).
///
/// Each entry is `(reference, perfect_hypothesis)`. Tests / harnesses can
/// deliberately corrupt the hypothesis to exercise non-zero WER paths.
pub fn fixture_english_corpus() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "ask not what your country can do for you",
            "ask not what your country can do for you",
        ),
        (
            "the quick brown fox jumps over the lazy dog",
            "the quick brown fox jumps over the lazy dog",
        ),
        (
            "weftos voice pipeline benchmark fixture one",
            "weftos voice pipeline benchmark fixture one",
        ),
        (
            "hello world this is a speech recognition test",
            "hello world this is a speech recognition test",
        ),
        (
            "turn the lights off in the living room",
            "turn the lights off in the living room",
        ),
    ]
}

/// Word-level Levenshtein returning `(substitutions, deletions, insertions)`.
///
/// Backtrace over the DP table so S/D/I are distinguished (not just distance).
fn align_counts(reference: &[String], hypothesis: &[String]) -> (usize, usize, usize) {
    let n = reference.len();
    let m = hypothesis.len();
    // dist[i][j] = edit distance for ref[..i] vs hyp[..j]
    let mut dist = vec![vec![0usize; m + 1]; n + 1];
    // op: 0=match/sub, 1=del, 2=ins — only needed via backtrace comparison
    for (i, row) in dist.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in dist[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = if reference[i - 1] == hypothesis[j - 1] {
                0
            } else {
                1
            };
            let sub = dist[i - 1][j - 1] + cost;
            let del = dist[i - 1][j] + 1;
            let ins = dist[i][j - 1] + 1;
            dist[i][j] = sub.min(del).min(ins);
        }
    }

    // Backtrace
    let mut i = n;
    let mut j = m;
    let mut s = 0usize;
    let mut d = 0usize;
    let mut ins_c = 0usize;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 {
            let cost = if reference[i - 1] == hypothesis[j - 1] {
                0
            } else {
                1
            };
            if dist[i][j] == dist[i - 1][j - 1] + cost {
                if cost == 1 {
                    s += 1;
                }
                i -= 1;
                j -= 1;
                continue;
            }
        }
        if i > 0 && dist[i][j] == dist[i - 1][j] + 1 {
            d += 1;
            i -= 1;
            continue;
        }
        // insertion
        ins_c += 1;
        j -= 1;
    }
    (s, d, ins_c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perfect_match_zero_wer() {
        let r = word_error_rate("Hello World", "hello world");
        assert!(r.is_perfect());
        assert_eq!(r.edits(), 0);
        assert_eq!(r.reference_words, 2);
    }

    #[test]
    fn substitution() {
        let r = word_error_rate("hello world", "hello there");
        assert_eq!(r.substitutions, 1);
        assert_eq!(r.deletions, 0);
        assert_eq!(r.insertions, 0);
        assert!((r.wer - 0.5).abs() < 1e-9);
    }

    #[test]
    fn deletion() {
        let r = word_error_rate("one two three", "one three");
        assert_eq!(r.deletions, 1);
        assert_eq!(r.substitutions, 0);
        assert_eq!(r.insertions, 0);
        assert!((r.wer - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn insertion() {
        let r = word_error_rate("one two", "one extra two");
        assert_eq!(r.insertions, 1);
        assert_eq!(r.substitutions, 0);
        assert_eq!(r.deletions, 0);
        assert!((r.wer - 0.5).abs() < 1e-9);
    }

    #[test]
    fn empty_reference_conventions() {
        assert_eq!(word_error_rate("", "").wer, 0.0);
        assert_eq!(word_error_rate("", "oops").wer, 1.0);
    }

    #[test]
    fn fixture_corpus_perfect() {
        let pairs = fixture_english_corpus();
        let corpus = word_error_rate_corpus(&pairs);
        assert!(corpus.is_perfect(), "fixture self-match must be WER 0");
        assert!(corpus.reference_words > 0);
    }

    #[test]
    fn corpus_micro_average() {
        // utt1: 1 sub / 2 words; utt2: 0 / 2 words → 1/4
        let pairs = [("a b", "a c"), ("d e", "d e")];
        let c = word_error_rate_corpus(&pairs);
        assert_eq!(c.reference_words, 4);
        assert_eq!(c.edits(), 1);
        assert!((c.wer - 0.25).abs() < 1e-9);
    }
}
