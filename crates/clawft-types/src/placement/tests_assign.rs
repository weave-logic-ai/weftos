//! Whole-workload matching: exclusive requirements never share a
//! capability, and the outcome does not depend on request order.

use super::assign::match_all;
use super::capability::{Capability, CapabilityId, Provenance};
use super::requirement::{MatchFailure, Requirement};

fn cap(s: &str) -> Capability {
    Capability::new(CapabilityId::new(s).unwrap(), Provenance::Probed)
}
fn req(s: &str) -> Requirement {
    Requirement::exact(CapabilityId::new(s).unwrap())
}

#[test]
fn exclusive_requirements_do_not_share_a_capability() {
    let one = vec![cap("accel.tpu.coral").exclusive()];
    let r = req("accel.tpu.coral").exclusive();
    assert_eq!(match_all(std::slice::from_ref(&r), &one), Ok(vec![vec![0]]));
    let err = match_all(&[r.clone(), r], &one).unwrap_err();
    assert_eq!(
        err,
        vec![
            (0, MatchFailure::Contended { need: 1 }),
            (1, MatchFailure::Contended { need: 1 })
        ]
    );
}

#[test]
fn exclusive_assignment_does_not_depend_on_request_order() {
    // The greedy pass gave the prefix requirement `cuda` and then failed the
    // exact one; a joint assignment (prefix -> rocm, exact -> cuda) exists.
    let caps = vec![
        cap("accel.gpu.cuda").exclusive(),
        cap("accel.gpu.rocm").exclusive(),
    ];
    let any = Requirement::prefix("accel.gpu").unwrap().exclusive();
    let cuda = req("accel.gpu.cuda").exclusive();
    assert_eq!(
        match_all(&[any.clone(), cuda.clone()], &caps),
        Ok(vec![vec![1], vec![0]])
    );
    assert_eq!(match_all(&[cuda, any], &caps), Ok(vec![vec![0], vec![1]]));
}

#[test]
fn exclusive_never_shares_with_a_shared_requirement() {
    let one = vec![cap("accel.gpu.cuda")];
    let shared = req("accel.gpu.cuda");
    let excl = req("accel.gpu.cuda").exclusive();
    for order in [
        vec![shared.clone(), excl.clone()],
        vec![excl.clone(), shared.clone()],
    ] {
        let err = match_all(&order, &one).unwrap_err();
        assert_eq!(err.len(), 2, "both requirements are blamed: {err:?}");
        assert!(
            err.iter()
                .all(|(_, f)| *f == MatchFailure::Contended { need: 1 })
        );
    }
    // With a second device both fit, and the shared one avoids the claim.
    let two = vec![cap("accel.gpu.cuda"), cap("accel.gpu.cuda")];
    for order in [
        [shared.clone(), excl.clone()],
        [excl.clone(), shared.clone()],
    ] {
        let got = match_all(&order, &two).unwrap();
        assert_eq!((got[0].len(), got[1].len()), (1, 1));
        assert_ne!(got[0], got[1], "exclusive claim shared: {got:?}");
    }
    // Shared requirements may share with each other.
    assert_eq!(
        match_all(&[shared.clone(), shared], &one),
        Ok(vec![vec![0], vec![0]])
    );
}

#[test]
fn match_all_reports_individual_failures_by_index() {
    let caps = vec![cap("accel.gpu.metal")];
    let err = match_all(
        &[
            req("accel.gpu.metal"),
            req("accel.tpu.coral"),
            req("accel.gpu.metal").with_count(2),
        ],
        &caps,
    )
    .unwrap_err();
    assert_eq!(err.len(), 2);
    assert_eq!(err[0].0, 1);
    assert!(matches!(err[0].1, MatchFailure::NoSuchId { .. }));
    assert_eq!(
        err[1],
        (2, MatchFailure::InsufficientCount { have: 1, need: 2 })
    );
}

#[test]
fn exclusive_counts_are_assigned_jointly() {
    // Three NPUs; a two-device exclusive prefix and a one-device exclusive
    // exact requirement fit only if the prefix avoids the named device.
    let caps = vec![
        cap("accel.npu.hailo").exclusive(),
        cap("accel.npu.ane").exclusive(),
        cap("accel.npu.rknn").exclusive(),
    ];
    let two = Requirement::prefix("accel.npu")
        .unwrap()
        .with_count(2)
        .exclusive();
    let hailo = req("accel.npu.hailo").exclusive();
    let got = match_all(&[two.clone(), hailo.clone()], &caps).unwrap();
    assert_eq!(got[1], vec![0]);
    assert_eq!(got[0].len(), 2);
    assert!(!got[0].contains(&0));
    // Four devices wanted, three exist: contention, not a false match.
    let err = match_all(&[two.clone(), hailo, two], &caps).unwrap_err();
    assert_eq!(err.len(), 3);
}

#[test]
fn search_is_bounded_and_never_falsely_matches() {
    // 12 identical exclusive devices, 13 exclusive requirements: impossible,
    // with a factorial search space. The budget stops it; the answer is
    // contention, not a match.
    let caps: Vec<Capability> = (0..12)
        .map(|_| cap("accel.npu.hailo").exclusive())
        .collect();
    let reqs: Vec<Requirement> = (0..13)
        .map(|_| req("accel.npu.hailo").exclusive())
        .collect();
    let started = std::time::Instant::now();
    let err = match_all(&reqs, &caps).unwrap_err();
    assert_eq!(err.len(), 13);
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    // One fewer requirement fits exactly.
    let got = match_all(&reqs[..12], &caps).unwrap();
    let mut all: Vec<usize> = got.into_iter().flatten().collect();
    all.sort_unstable();
    assert_eq!(all, (0..12).collect::<Vec<_>>());
}
