//! Locality decision, placer preference and exchange-sharing tests.

use std::collections::BTreeSet;

use clawft_types::placement::engine::{
    Liveness, PlacementFacts, PlacementPolicy, PlacementRequest, TrustTier, WorkloadRequirements,
    WorkloadSpec, ClusterState, place,
};
use clawft_types::placement::{Capability, CapabilityId, MemoryDemand, Provenance, Requirement};

use super::tests::{adopt_fake, fake_model, new_reg};
use super::*;

const GIB: u64 = 1 << 30;

fn body_of(total_gib: &[u64], redistributable: bool) -> ModelPackageBody {
    ModelPackageBody {
        name: "Big-Model".into(),
        format: ModelFormat::Mlx,
        shards: total_gib
            .iter()
            .enumerate()
            .map(|(i, g)| crate::workload_pkg::FileRef {
                path: format!("model-{i:05}.safetensors"),
                size: g * GIB,
                blake3: format!("{i:064x}"),
            })
            .collect(),
        tokenizer_blake3: None,
        tokenizer_path: None,
        template_blake3: None,
        template_path: None,
        source: ModelSource { hf_repo: Some("o/m".into()), ..ModelSource::default() },
        redistributable,
    }
}

fn node(id: &str, eligible: bool, held: &[usize], complete: bool, free_gib: Option<u64>) -> NodeHolding {
    NodeHolding {
        node_id: id.into(),
        eligible,
        shards: held.iter().map(|i| format!("{i:064x}")).collect::<BTreeSet<_>>(),
        complete,
        free_bytes: free_gib.map(|g| g * GIB),
    }
}

fn allow() -> TransferPolicy {
    TransferPolicy { allow_weight_transfer: true, ..TransferPolicy::default() }
}

fn reason(plan: &LocalityPlan) -> &Unplaceable {
    match &plan.decision {
        LocalityDecision::Unplaceable(u) => u,
        other => panic!("expected unplaceable, got {other:?}"),
    }
}

#[test]
fn eligible_holder_wins_even_when_transfer_is_allowed() {
    let body = body_of(&[20, 25], true);
    let nodes = [
        node("a-empty", true, &[], false, Some(500)),
        node("b-holder", true, &[0, 1], true, Some(10)),
    ];
    let plan = decide(&body, &nodes, &allow(), Sharing::Allowed);
    assert_eq!(plan.decision, LocalityDecision::PlaceAtHolder { node_id: "b-holder".into() });
    assert!(plan.notes.last().unwrap().contains("nothing is copied"));
}

#[test]
fn transfer_is_off_by_default_and_names_the_reason() {
    let body = body_of(&[20, 25], true);
    let nodes = [node("a", true, &[], false, Some(500)), node("held", false, &[0, 1], true, None)];
    let plan = decide(&body, &nodes, &TransferPolicy::default(), Sharing::Allowed);
    assert_eq!(reason(&plan), &Unplaceable::TransferDisabled);
}

#[test]
fn not_redistributable_is_never_fetched() {
    let body = body_of(&[20, 25], false);
    assert_eq!(Sharing::from_manifest(&body), Sharing::Refused);
    let nodes = [node("a", true, &[], false, Some(500)), node("held", false, &[0, 1], true, None)];
    let plan = decide(&body, &nodes, &allow(), Sharing::from_manifest(&body));
    assert_eq!(reason(&plan), &Unplaceable::NotRedistributable);
}

#[test]
fn fetch_picks_the_target_with_the_fewest_missing_bytes() {
    let body = body_of(&[20, 25, 5], true);
    let nodes = [
        node("held", false, &[0, 1, 2], true, None),
        node("big-empty", true, &[], false, Some(500)),
        node("part", true, &[0, 1], false, Some(500)),
    ];
    let plan = decide(&body, &nodes, &allow(), Sharing::Allowed);
    assert_eq!(
        plan.decision,
        LocalityDecision::FetchThenPlace {
            target: "part".into(),
            sources: vec!["held".into()],
            bytes: 5 * GIB,
            missing_shards: 1,
        }
    );
}

#[test]
fn ceiling_and_space_are_enforced() {
    let body = body_of(&[20, 25], true);
    let held = node("held", false, &[0, 1], true, None);
    let capped = TransferPolicy { max_bytes: Some(10 * GIB), ..allow() };
    let plan = decide(&body, &[held.clone(), node("a", true, &[], false, Some(500))], &capped, Sharing::Allowed);
    assert_eq!(reason(&plan), &Unplaceable::OverCeiling { bytes: 45 * GIB, max: 10 * GIB });
    // 45 GiB plus the 1 GiB default headroom does not fit in 40 GiB.
    let plan = decide(&body, &[held.clone(), node("a", true, &[], false, Some(40))], &allow(), Sharing::Allowed);
    assert_eq!(reason(&plan), &Unplaceable::NoSpace { needed: 46 * GIB, free: Some(40 * GIB) });
    // Unknown free space fails closed.
    let plan = decide(&body, &[held, node("a", true, &[], false, None)], &allow(), Sharing::Allowed);
    assert!(matches!(reason(&plan), Unplaceable::NoSpace { free: None, .. }));
}

#[test]
fn no_holder_or_no_eligible_node_is_reported() {
    let body = body_of(&[1], true);
    let plan = decide(&body, &[node("a", true, &[], false, Some(9))], &allow(), Sharing::Allowed);
    assert_eq!(reason(&plan), &Unplaceable::NoHolder);
    let plan = decide(&body, &[node("a", false, &[0], true, None)], &allow(), Sharing::Allowed);
    assert_eq!(reason(&plan), &Unplaceable::NoEligibleNode);
    let plan = decide(&body, &[], &allow(), Sharing::Allowed);
    assert_eq!(reason(&plan), &Unplaceable::NoEligibleNode);
}

#[test]
fn decision_does_not_depend_on_node_order() {
    let body = body_of(&[10, 10], true);
    let a = node("a", true, &[], false, Some(100));
    let b = node("b", true, &[], false, Some(100));
    let src = node("src", false, &[0, 1], true, None);
    let p1 = decide(&body, &[a.clone(), b.clone(), src.clone()], &allow(), Sharing::Allowed);
    let p2 = decide(&body, &[src, b, a], &allow(), Sharing::Allowed);
    assert_eq!(p1, p2);
}

// ── from advertised capabilities, and through the real placer ────

struct TestNode {
    id: String,
    caps: Vec<Capability>,
}

impl PlacementFacts for TestNode {
    fn node_id(&self) -> &str { &self.id }
    fn capabilities(&self) -> &[Capability] { &self.caps }
    fn liveness(&self) -> Liveness { Liveness::Alive }
    fn trust_tier(&self) -> TrustTier { TrustTier::Pinned }
    fn facts_expire_at_ms(&self) -> Option<u64> { Some(10_000) }
    fn load(&self) -> Option<f64> { Some(0.5) }
}

fn runtime() -> Capability {
    Capability::new(CapabilityId::new("runtime.native").unwrap(), Provenance::Probed)
}

fn spec_with(pref: Option<clawft_types::placement::engine::Preference>) -> WorkloadSpec {
    let mut policy = PlacementPolicy::default();
    policy.preferences.extend(pref);
    WorkloadSpec {
        kind: "inference".into(),
        name: "coder-daily".into(),
        requirements: WorkloadRequirements {
            common: vec![Requirement::exact(CapabilityId::new("runtime.native").unwrap())],
            variants: vec![],
            memory: MemoryDemand::default(),
        },
        policy,
    }
}

#[test]
fn placer_prefers_the_node_holding_the_shards_and_explains_it() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("quant");
    fake_model(&dir);
    let reg = new_reg();
    let (adopted, _, _) = adopt_fake(&reg, &dir, "Fake-Model-4bit");
    let body = adopted.verified.body.clone();
    let tiers = TierResolver::with_external_roots(vec![]);
    let holder_caps: Vec<Capability> =
        std::iter::once(runtime()).chain(model_capabilities(&reg, &tiers)).collect();
    let holder = TestNode { id: "a-holder".into(), caps: holder_caps };
    // Sorts first on ties, so only the preference can make "z-holder" win.
    let other = TestNode { id: "a-other".into(), caps: vec![runtime()] };
    let holder = TestNode { id: "z-holder".into(), ..holder };
    let facts = [other, holder];

    let pref = locality_preference(&body, &adopted.package_id, 50.0);
    let d = place(&PlacementRequest::new(spec_with(Some(pref))), &facts, &ClusterState::default()).unwrap();
    assert!(clawft_types::placement::engine::explain(&d).contains("z-holder"));
    let p = d.placement.expect("placed");
    assert_eq!(p.node_id, "z-holder");
    assert!(p.score.locality > 0.0);

    // Without the preference the tie goes to the first node id.
    let d = place(&PlacementRequest::new(spec_with(None)), &facts, &ClusterState::default()).unwrap();
    assert_eq!(d.placement.unwrap().node_id, "a-other");

    // As a hard requirement only the holder qualifies.
    let mut spec = spec_with(None);
    spec.requirements.common.push(model_present_requirement(&adopted.package_id));
    let d = place(&PlacementRequest::new(spec), &facts, &ClusterState::default()).unwrap();
    assert_eq!(d.placement.unwrap().node_id, "z-holder");
}

#[test]
fn detached_drive_makes_the_placer_stop_preferring_it() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let drive = base.join("Volumes/Drive");
    let dir = drive.join("quant");
    fake_model(&dir);
    let tiers = TierResolver::with_external_roots(vec![base.join("Volumes")]);
    let reg = new_reg();
    let (adopted, _, _) = adopt_fake(&reg, &dir, "Fake");
    let body = adopted.verified.body.clone();
    let mk = |reg: &ModelRegistry| {
        let caps = std::iter::once(runtime()).chain(model_capabilities(reg, &tiers)).collect();
        [
            TestNode { id: "a-other".into(), caps: vec![runtime()] },
            TestNode { id: "z-holder".into(), caps },
        ]
    };
    let req = || {
        let pref = locality_preference(&body, &adopted.package_id, 50.0);
        PlacementRequest::new(spec_with(Some(pref)))
    };
    let d = place(&req(), &mk(&reg), &ClusterState::default()).unwrap();
    assert_eq!(d.placement.unwrap().node_id, "z-holder");
    std::fs::rename(&drive, base.join("ejected")).unwrap();
    let d = place(&req(), &mk(&reg), &ClusterState::default()).unwrap();
    assert_eq!(d.placement.unwrap().node_id, "a-other");
}

#[test]
fn holding_is_read_back_from_advertised_capabilities() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("quant");
    fake_model(&dir);
    let reg = new_reg();
    let (adopted, _, _) = adopt_fake(&reg, &dir, "Fake");
    let body = adopted.verified.body.clone();
    let tiers = TierResolver::with_external_roots(vec![]);
    let caps = model_capabilities(&reg, &tiers);
    let h = NodeHolding::from_capabilities("n", &adopted.package_id, &body, &caps, true);
    assert!(h.complete);
    assert_eq!(h.shards.len(), 2);
    // A different attested model is not held.
    let h = NodeHolding::from_capabilities("n", &"0".repeat(64), &body, &caps, true);
    assert!(!h.complete);
    // A partial holding counts the shards it has.
    std::fs::remove_file(dir.join("model-00002-of-00002.safetensors")).unwrap();
    let caps = model_capabilities(&reg, &tiers);
    let h = NodeHolding::from_capabilities("n", &adopted.package_id, &body, &caps, true);
    assert!(!h.complete);
    assert_eq!(h.shards.len(), 1);
}

// ── exchange sharing (fail closed) ───────────────────────────────

#[cfg(feature = "mesh")]
mod sharing_tests {
    use std::sync::Arc;

    use super::*;
    use crate::artifact_store::ArtifactStore;
    use crate::mesh_artifact::{ArtifactExchange, ExchangeConfig};
    use crate::mesh_swarm_state::{Audience, ManifestPolicy, RedistributionPolicy};
    use crate::model_manifest::sharing::{SeedModelError, seed_model, sharing_for};
    use crate::model_manifest::tests::{input, new_reg, operator};

    fn exchange() -> ArtifactExchange {
        ArtifactExchange::new("node-test", Arc::new(ArtifactStore::new_memory()), ExchangeConfig::default())
            .unwrap()
    }

    #[test]
    fn default_policy_never_shares_an_unflagged_model() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("quant");
        fake_model(&dir);
        let reg = new_reg();
        let (adopted, _, anchors) = adopt_fake(&reg, &dir, "Fake");
        let ex = exchange();
        let err = seed_model(&ex, &reg, "Fake", &anchors).unwrap_err();
        assert!(matches!(err, SeedModelError::NotShareable(_)), "{err}");
        let hash = adopted.verified.body.shards[0].blake3.clone();
        let h = crate::workload_pkg::codec::hex_decode_exact::<32>(&hash).unwrap();
        let d = ex.resolve(&crate::mesh_artifact_types::ArtifactKey::Content(h));
        assert!(d.is_none(), "nothing was seeded, so nothing is servable");
        assert_eq!(
            sharing_for(&ManifestPolicy, &adopted.package_id, &adopted.verified.body),
            Sharing::Refused
        );
    }

    #[test]
    fn opted_in_model_is_seeded_verified_and_servable() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("quant");
        fake_model(&dir);
        let reg = new_reg();
        let (key, kid, anchors) = operator();
        let mut inp = input("Shared");
        inp.redistributable = true;
        let scanned = scan_dir(&dir, inp).unwrap();
        let adopted = reg.adopt(scanned, &key, &kid, &anchors, false).unwrap();
        let ex = exchange();
        let seeded = seed_model(&ex, &reg, "Shared", &anchors).unwrap();
        assert_eq!(seeded.shards.len(), 2);
        for (_, id) in &seeded.shards {
            let d = ex.descriptor(id).unwrap();
            assert!(ex.is_servable(&d), "opted-in shard is servable");
        }
        assert_eq!(
            sharing_for(&ManifestPolicy, &adopted.package_id, &adopted.verified.body),
            Sharing::Allowed
        );
        // A policy that vetoes still wins over the manifest's opt-in.
        #[derive(Debug)]
        struct Never;
        impl RedistributionPolicy for Never {
            fn allows(&self, _: &[u8; 32], _: &[crate::mesh_swarm_state::GrantInfo], _: &Audience<'_>) -> bool {
                false
            }
        }
        assert_eq!(
            sharing_for(&Never, &adopted.package_id, &adopted.verified.body),
            Sharing::Refused
        );
    }

    #[test]
    fn a_tampered_model_is_not_seeded() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("quant");
        fake_model(&dir);
        let reg = new_reg();
        let (key, kid, anchors) = operator();
        let mut inp = input("Shared");
        inp.redistributable = true;
        reg.adopt(scan_dir(&dir, inp).unwrap(), &key, &kid, &anchors, false).unwrap();
        let shard = dir.join("model-00001-of-00002.safetensors");
        std::fs::write(&shard, vec![0u8; 4096]).unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(&shard).unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(60)).unwrap();
        let err = seed_model(&exchange(), &reg, "Shared", &anchors).unwrap_err();
        assert!(matches!(err, SeedModelError::Model(ModelError::NotReady { .. })), "{err}");
    }
}
