//! Strict execution tiers (ADR-099 section 3, decided 2026-09-29): native on
//! real target hardware, then emulated, then the dev-mac fallback. A lower
//! tier never outranks a higher one, whatever its other scores.

use super::fixtures::*;
use super::{placed_on, run};
use crate::placement::CapabilityId;
use crate::placement::engine::{
    Decision, PlacementRequest, Preference, ScoringWeights, Tier, explain,
};
use crate::placement::requirement::Requirement;

/// An aarch64 cog every tier can take: pi5 natively (target hardware),
/// x86 through Docker emulation, the dev Mac natively (dev fallback).
fn request() -> PlacementRequest {
    let mut r = PlacementRequest::new(sensor_spec("cog", "anomaly-detect", &["aarch64"], None));
    r.allow_emulated = true;
    r
}

fn node_for(t: Tier) -> TestNode {
    match t {
        Tier::Native => pi5(),
        Tier::Emulated => x86(),
        Tier::DevFallback => mac(),
    }
}

/// Every bonus scoring and affinity can hand one node.
#[derive(Clone, Copy, Debug)]
enum Bonus {
    None,
    Locality,
    Sticky,
    Affinity,
    InvertedWeights,
    Everything,
}

fn favour(req: &mut PlacementRequest, node: &str, b: Bonus) {
    let all = matches!(b, Bonus::Everything);
    if all || matches!(b, Bonus::Locality) {
        // A preference only `node` meets, with an enormous weight.
        req.spec.policy.preferences = vec![Preference {
            name: "only-this-node".into(),
            requirement: Requirement::exact(CapabilityId::new(node_class(node)).unwrap()),
            weight: 1.0e6,
        }];
    }
    if all || matches!(b, Bonus::Sticky) {
        req.spec.policy.sticky = true;
        req.current_node = Some(node.into());
    }
    if all || matches!(b, Bonus::Affinity) {
        req.affinity.prefer = vec![node.into()];
    }
    if all || matches!(b, Bonus::InvertedWeights) {
        req.weights = ScoringWeights {
            native: 0.0,
            emulated: 500.0,
            dev_fallback: 1000.0,
            ..ScoringWeights::default()
        };
    }
}

/// A capability id only this fixture node advertises.
fn node_class(node: &str) -> &'static str {
    match node {
        "pi5" => "node.class.pi5",
        "x86" => "cpu.arch.x86_64",
        "mac-dev" => "node.class.dev-mac",
        other => panic!("no unique id for {other}"),
    }
}

fn tier_of(d: &Decision, node: &str) -> Option<Tier> {
    d.candidates
        .iter()
        .find(|c| c.node_id == node)
        .and_then(|c| c.tier)
}

#[test]
fn default_weights_follow_the_adr_order() {
    let w = ScoringWeights::default();
    assert_eq!((w.native, w.emulated, w.dev_fallback), (100.0, 40.0, 20.0));
    assert_eq!(
        Tier::ORDER,
        [Tier::Native, Tier::Emulated, Tier::DevFallback]
    );
    assert!(Tier::Native.rank() < Tier::Emulated.rank());
    assert!(Tier::Emulated.rank() < Tier::DevFallback.rank());
}

/// Table: for every (higher, lower) tier pair and every bonus given to the
/// lower-tier node, the higher tier wins and ranks first.
#[test]
fn lower_tier_never_beats_higher_tier_regardless_of_other_scores() {
    let pairs = [
        (Tier::Native, Tier::Emulated),
        (Tier::Native, Tier::DevFallback),
        (Tier::Emulated, Tier::DevFallback),
    ];
    let bonuses = [
        Bonus::None,
        Bonus::Locality,
        Bonus::Sticky,
        Bonus::Affinity,
        Bonus::InvertedWeights,
        Bonus::Everything,
    ];
    for (high, low) in pairs {
        for bonus in bonuses {
            let (hi, lo) = (node_for(high), node_for(low));
            let mut req = request();
            favour(&mut req, &lo.id, bonus);
            // Also make the lower node idle and the higher one saturated.
            let mut lo = lo;
            lo.load = Some(0.0);
            let mut hi = hi;
            hi.load = Some(1.0);
            let case = format!("{high:?} vs {low:?} with {bonus:?}");
            for nodes in [vec![hi.clone(), lo.clone()], vec![lo.clone(), hi.clone()]] {
                let d = run(&req, &nodes);
                assert_eq!(tier_of(&d, &hi.id), Some(high), "{case}");
                assert_eq!(tier_of(&d, &lo.id), Some(low), "{case}");
                assert_eq!(placed_on(&d), Some(hi.id.as_str()), "{case}");
                assert_eq!(d.candidates[0].node_id, hi.id, "{case}: ranks first");
                assert_eq!(d.placement.as_ref().unwrap().tier, high, "{case}");
            }
        }
    }
}

#[test]
fn full_ranking_is_native_then_emulated_then_dev_mac() {
    let mut req = request();
    // Give the dev Mac, then the emulated node, big score bonuses.
    req.spec.policy.preferences = vec![
        Preference {
            name: "mac".into(),
            requirement: Requirement::exact(CapabilityId::new("node.class.dev-mac").unwrap()),
            weight: 9_000.0,
        },
        Preference {
            name: "x86".into(),
            requirement: Requirement::exact(CapabilityId::new("cpu.arch.x86_64").unwrap()),
            weight: 5_000.0,
        },
    ];
    let d = run(&req, &[mac(), x86(), pi5()]);
    let order: Vec<(&str, Option<Tier>)> = d
        .candidates
        .iter()
        .map(|c| (c.node_id.as_str(), c.tier))
        .collect();
    assert_eq!(
        order,
        vec![
            ("pi5", Some(Tier::Native)),
            ("x86", Some(Tier::Emulated)),
            ("mac-dev", Some(Tier::DevFallback)),
        ]
    );
    // Without target hardware, the emulated route beats the dev Mac.
    assert_eq!(placed_on(&run(&req, &[mac(), x86()])), Some("x86"));
    // Without emulation allowed, the dev Mac is the last resort.
    let mut no_emu = req.clone();
    no_emu.allow_emulated = false;
    assert_eq!(placed_on(&run(&no_emu, &[mac(), x86()])), Some("mac-dev"));
}

#[test]
fn pin_still_overrides_the_tier_order() {
    let mut req = request();
    req.pin = Some("mac-dev".into());
    let d = run(&req, &[pi5(), x86(), mac()]);
    assert_eq!(placed_on(&d), Some("mac-dev"));
    assert_eq!(d.placement.unwrap().tier, Tier::DevFallback);
}

#[test]
fn explain_shows_the_tier_order_and_each_candidates_tier() {
    let d = run(&request(), &[mac(), x86(), pi5()]);
    let text = explain(&d);
    assert!(
        text.contains("tier order (strict): native > emulated > dev_fallback"),
        "{text}"
    );
    assert!(
        text.contains("decision: PLACED on pi5 via aarch64-native (tier native,"),
        "{text}"
    );
    for (rank, node, tier) in [
        (1, "pi5", "native"),
        (2, "x86", "emulated"),
        (3, "mac-dev", "dev_fallback"),
    ] {
        let line = format!("  {rank}. {node} eligible via ");
        let found = text
            .lines()
            .find(|l| l.starts_with(&line))
            .unwrap_or_else(|| panic!("no line for {node}:\n{text}"));
        assert!(found.contains(&format!("(tier {tier})")), "{found}");
    }
}
