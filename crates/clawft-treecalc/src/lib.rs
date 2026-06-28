//! Tree-calculus `Form` triage — shared structural classifier.
//!
//! Originally inlined in `clawft-graphify::extract::treecalc`, lifted
//! here so other callers (kernel DEMOCRITUS cycle detection, causal
//! edge-decay dispatch) can use the same three-form dispatch without
//! pulling in graphify's extraction types.
//!
//! The idea (from Stay / Barendregt's tree calculus) is that *every*
//! finite tree is one of three shapes:
//!
//! - [`Form::Atom`] — a leaf; no children.
//! - [`Form::Sequence`] — a stem; ordered children all of the same
//!   kind.
//! - [`Form::Branch`] — a fork; children of mixed kinds.
//!
//! Once you know the form, you can dispatch the expensive operation
//! structurally instead of walking the whole tree. Used in this tree
//! for:
//!
//! - Rust-source entity triage in `clawft-graphify` (leaf constants
//!   vs. impl-blocks vs. structs-with-methods).
//! - Coherence-trajectory classification in the DEMOCRITUS loop
//!   (flat / monotone / oscillating), via [`triage_trajectory`].
//!
//! ```
//! use clawft_treecalc::{triage, triage_trajectory, Form};
//!
//! // Generic kind-based triage.
//! assert_eq!(triage::<_, i32>([]), Form::Atom);
//! assert_eq!(triage([1, 1, 1]), Form::Sequence);
//! assert_eq!(triage([1, 2, 1]), Form::Branch);
//!
//! // Numeric trajectory triage.
//! let eps = 0.01;
//! assert_eq!(triage_trajectory(&[], eps), Form::Atom);
//! assert_eq!(triage_trajectory(&[0.5, 0.5, 0.5], eps), Form::Atom);
//! assert_eq!(triage_trajectory(&[0.1, 0.5, 0.9], eps), Form::Sequence);
//! assert_eq!(triage_trajectory(&[0.1, 0.9, 0.1, 0.9], eps), Form::Branch);
//! ```

#![deny(missing_docs)]

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Tree-calculus form for an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Form {
    /// Leaf — no children. Constants, type aliases, use statements,
    /// numerically-flat trajectories.
    Atom,
    /// Stem — ordered same-kind children. Impl blocks, module bodies,
    /// monotone (converging or diverging) numeric trajectories.
    Sequence,
    /// Fork — heterogeneous children. Struct with methods, enum with
    /// variants, oscillating numeric trajectories.
    Branch,
}

impl Form {
    /// Short string tag (`"atom"` / `"sequence"` / `"branch"`) —
    /// handy for logging and wire formats.
    pub fn as_str(self) -> &'static str {
        match self {
            Form::Atom => "atom",
            Form::Sequence => "sequence",
            Form::Branch => "branch",
        }
    }
}

/// Classify a collection of child-kinds by their uniformity.
///
/// - Empty input → [`Form::Atom`].
/// - All equal → [`Form::Sequence`].
/// - Otherwise → [`Form::Branch`].
///
/// `K` needs `PartialEq` only. Works on any `IntoIterator` so callers
/// can pass slices, `Vec`s, or on-the-fly projections without
/// copying.
pub fn triage<I, K>(kinds: I) -> Form
where
    I: IntoIterator<Item = K>,
    K: PartialEq,
{
    let mut iter = kinds.into_iter();
    let first = match iter.next() {
        None => return Form::Atom,
        Some(k) => k,
    };
    if iter.all(|k| k == first) {
        Form::Sequence
    } else {
        Form::Branch
    }
}

/// Classify a numeric trajectory (e.g. a coherence history) by its
/// sign-of-difference pattern.
///
/// Walks the adjacent-pair differences. Differences whose absolute
/// value is below `epsilon` are ignored (noise floor).
///
/// - No above-`epsilon` deltas (or fewer than 2 samples) → [`Form::Atom`]
///   (flat trajectory).
/// - All above-`epsilon` deltas share a sign → [`Form::Sequence`]
///   (monotone: converging if positive, diverging if negative).
/// - Both signs appear → [`Form::Branch`] (oscillating).
///
/// Useful for cheap trend classification without having to fit a
/// model first. Callers can pair the returned `Form` with additional
/// features (net change, max swing, window length) to produce a
/// richer state.
pub fn triage_trajectory(history: &[f64], epsilon: f64) -> Form {
    if history.len() < 2 {
        return Form::Atom;
    }
    let mut saw_pos = false;
    let mut saw_neg = false;
    for pair in history.windows(2) {
        let d = pair[1] - pair[0];
        if d.abs() < epsilon {
            continue;
        }
        if d > 0.0 {
            saw_pos = true;
        } else {
            saw_neg = true;
        }
    }
    match (saw_pos, saw_neg) {
        (false, false) => Form::Atom,
        (true, false) | (false, true) => Form::Sequence,
        (true, true) => Form::Branch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atom_is_empty() {
        assert_eq!(triage::<_, i32>([]), Form::Atom);
        assert_eq!(triage::<_, &str>(Vec::<&str>::new()), Form::Atom);
    }

    #[test]
    fn sequence_is_uniform() {
        assert_eq!(triage(["a", "a", "a"]), Form::Sequence);
        assert_eq!(triage([1u32]), Form::Sequence);
    }

    #[test]
    fn branch_is_mixed() {
        assert_eq!(triage(["a", "b"]), Form::Branch);
        assert_eq!(triage([1, 1, 2, 1]), Form::Branch);
    }

    #[test]
    fn trajectory_short_is_atom() {
        assert_eq!(triage_trajectory(&[], 0.01), Form::Atom);
        assert_eq!(triage_trajectory(&[0.5], 0.01), Form::Atom);
    }

    #[test]
    fn trajectory_flat_is_atom() {
        // All within epsilon.
        assert_eq!(triage_trajectory(&[0.5, 0.5, 0.5], 0.01), Form::Atom);
        // Small noise below epsilon.
        assert_eq!(
            triage_trajectory(&[0.5, 0.502, 0.498, 0.501], 0.01),
            Form::Atom
        );
    }

    #[test]
    fn trajectory_monotone_is_sequence() {
        assert_eq!(
            triage_trajectory(&[0.1, 0.3, 0.5, 0.7], 0.01),
            Form::Sequence
        );
        assert_eq!(
            triage_trajectory(&[0.7, 0.5, 0.3, 0.1], 0.01),
            Form::Sequence
        );
    }

    #[test]
    fn trajectory_oscillating_is_branch() {
        assert_eq!(triage_trajectory(&[0.1, 0.9, 0.1, 0.9], 0.01), Form::Branch);
    }

    #[test]
    fn trajectory_epsilon_masks_noise() {
        // Oscillates strictly inside the noise floor → read as flat (Atom).
        assert_eq!(
            triage_trajectory(&[0.5, 0.502, 0.498, 0.501, 0.499], 0.01),
            Form::Atom
        );
    }

    #[test]
    fn form_as_str() {
        assert_eq!(Form::Atom.as_str(), "atom");
        assert_eq!(Form::Sequence.as_str(), "sequence");
        assert_eq!(Form::Branch.as_str(), "branch");
    }
}
