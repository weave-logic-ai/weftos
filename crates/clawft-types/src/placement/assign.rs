//! Matching a workload's whole requirement set against one node.
//!
//! Rules (ADR-099 section 2): each requirement needs `count` distinct
//! capabilities. A capability chosen by an **exclusive** requirement is used
//! by nothing else in the workload, neither another exclusive requirement
//! nor a shared one. Shared (non-exclusive) requirements may share
//! capabilities with each other.
//!
//! The result must not depend on the order requirements are listed in, so
//! this is a search, not a greedy pass: exclusive requirements are assigned
//! by backtracking (most constrained first, least contested capability
//! first), then each shared requirement takes what is left. The search is
//! bounded by [`SEARCH_BUDGET`] steps; realistic workloads (a handful of
//! requirements, a node's worth of capabilities) finish in a few steps.
//! Exhausting the budget is reported as [`MatchFailure::Contended`], never
//! as a false match.

use super::capability::Capability;
use super::requirement::{MatchFailure, Requirement};

/// Most search steps before giving up and reporting contention.
pub const SEARCH_BUDGET: usize = 100_000;

/// Chosen capability indices, one list per requirement, in request order.
pub type Assignment = Vec<Vec<usize>>;

struct Search<'a> {
    /// (requirement index, count, candidates in preference order).
    exclusive: Vec<(usize, usize, Vec<usize>)>,
    /// (requirement index, count, candidates).
    shared: Vec<(usize, usize, &'a [usize])>,
    used: Vec<bool>,
    chosen: Vec<Vec<usize>>,
    steps: usize,
}

impl Search<'_> {
    /// Assign exclusive requirement `k` onward; true on a full assignment.
    fn exclusive_from(&mut self, k: usize) -> bool {
        if k == self.exclusive.len() {
            return self.shared_fits();
        }
        let (_, need, ref cands) = self.exclusive[k];
        let cands = cands.clone();
        let mut pick = Vec::with_capacity(need);
        self.choose(k, &cands, 0, need, &mut pick)
    }

    /// Choose `need` more unused capabilities from `cands[from..]`.
    fn choose(
        &mut self,
        k: usize,
        cands: &[usize],
        from: usize,
        need: usize,
        pick: &mut Vec<usize>,
    ) -> bool {
        self.steps += 1;
        if self.steps > SEARCH_BUDGET {
            return false;
        }
        if need == 0 {
            self.chosen[k] = pick.clone();
            return self.exclusive_from(k + 1);
        }
        for i in from..cands.len() {
            if cands.len() - i < need {
                break;
            }
            let c = cands[i];
            if self.used[c] {
                continue;
            }
            self.used[c] = true;
            pick.push(c);
            let ok = self.choose(k, cands, i + 1, need - 1, pick);
            pick.pop();
            self.used[c] = false;
            if ok {
                return true;
            }
            if self.steps > SEARCH_BUDGET {
                return false;
            }
        }
        false
    }

    /// Every shared requirement still has `count` capabilities left over.
    fn shared_fits(&self) -> bool {
        self.shared
            .iter()
            .all(|(_, need, cands)| cands.iter().filter(|&&c| !self.used[c]).count() >= *need)
    }
}

/// Match every requirement against one node's capabilities.
///
/// On success, returns the capabilities chosen for each requirement, in
/// request order. On failure, returns each failing requirement's index and
/// reason, sorted by index: a requirement that cannot match on its own
/// reports why ([`Requirement::candidates`]); when every requirement could
/// match alone but no joint assignment exists, each requirement that
/// competes for an exclusively claimable capability reports
/// [`MatchFailure::Contended`].
pub fn match_all(
    reqs: &[Requirement],
    caps: &[Capability],
) -> Result<Assignment, Vec<(usize, MatchFailure)>> {
    let mut failures = Vec::new();
    let mut cands: Vec<Vec<usize>> = vec![Vec::new(); reqs.len()];
    for (ri, req) in reqs.iter().enumerate() {
        match req.candidates(caps) {
            Ok(c) if c.len() >= req.count as usize => cands[ri] = c,
            Ok(c) => failures.push((
                ri,
                MatchFailure::InsufficientCount {
                    have: c.len() as u32,
                    need: req.count,
                },
            )),
            Err(f) => failures.push((ri, f)),
        }
    }
    if !failures.is_empty() {
        return Err(failures);
    }

    // How many requirements want each capability: prefer uncontested ones.
    let mut demand = vec![0usize; caps.len()];
    cands.iter().flatten().for_each(|&c| demand[c] += 1);

    let mut exclusive: Vec<(usize, usize, Vec<usize>)> = reqs
        .iter()
        .enumerate()
        .filter(|(_, r)| r.exclusive)
        .map(|(ri, r)| {
            let mut c = cands[ri].clone();
            c.sort_by_key(|&i| (demand[i], i));
            (ri, r.count as usize, c)
        })
        .collect();
    // Most constrained first: fewest spare candidates.
    exclusive.sort_by_key(|(ri, need, c)| (c.len() - need, *ri));
    let shared = reqs
        .iter()
        .enumerate()
        .filter(|(_, r)| !r.exclusive)
        .map(|(ri, r)| (ri, r.count as usize, cands[ri].as_slice()))
        .collect();

    let mut s = Search {
        exclusive,
        shared,
        used: vec![false; caps.len()],
        chosen: vec![Vec::new(); reqs.len()],
        steps: 0,
    };
    if s.exclusive_from(0) {
        // `used` is unwound by the search; rebuild the claims from `chosen`.
        let mut claimed = vec![false; caps.len()];
        let mut out: Assignment = vec![Vec::new(); reqs.len()];
        for (k, (ri, _, _)) in s.exclusive.iter().enumerate() {
            s.chosen[k].iter().for_each(|&i| claimed[i] = true);
            out[*ri] = s.chosen[k].clone();
        }
        for (ri, need, c) in &s.shared {
            out[*ri] = c
                .iter()
                .copied()
                .filter(|&i| !claimed[i])
                .take(*need)
                .collect();
        }
        return Ok(out);
    }

    // No joint assignment: blame every requirement that touches a capability
    // some exclusive requirement could claim.
    let mut claimable = vec![false; caps.len()];
    for (_, _, c) in &s.exclusive {
        c.iter().for_each(|&i| claimable[i] = true);
    }
    Err(reqs
        .iter()
        .enumerate()
        .filter(|(ri, r)| r.exclusive || cands[*ri].iter().any(|&i| claimable[i]))
        .map(|(ri, r)| (ri, MatchFailure::Contended { need: r.count }))
        .collect())
}
