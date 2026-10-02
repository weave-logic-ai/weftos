//! Two in-process nodes exchange signed facts and deltas over the mesh
//! runtime; the receiver assigns the tier and caps the provenance, and a
//! forged block is refused. Nothing here touches the operator's runtime
//! directory or chain.

use super::*;
use crate::cluster::ClusterConfig;
use crate::mesh_admit::PeerClass;
use crate::node_facts_advert::{FACTS_DOMAIN, verify_node_facts};
use clawft_types::placement::{Capability, CapabilityId};
use ed25519_dalek::Signer;

struct TestNode {
    id: String,
    ex: Arc<FactsExchange>,
    rt: Arc<MeshRuntime>,
}

fn node(n: u8) -> TestNode {
    let key = SigningKey::from_bytes(&[n; 32]);
    let rt = Arc::new(MeshRuntime::new(node_id_from_pubkey(
        &key.verifying_key().to_bytes(),
    )));
    let membership = Arc::new(ClusterMembership::new(ClusterConfig::default()));
    let ex = FactsExchange::new(key, membership, rt.clone(), FactsTrustPolicy::default());
    ex.start();
    TestNode {
        id: ex.node_id().to_string(),
        ex,
        rt,
    }
}

fn ctx(peer: &str, verified: bool) -> PeerCtx {
    if verified {
        PeerCtx {
            peer_id: peer.to_string(),
            node_verified: true,
            class: PeerClass::Node,
            remote_static: None,
            src_scope: None,
        }
    } else {
        PeerCtx::unauthenticated(peer)
    }
}

/// Connect `a` and `b` in process. `b_verifies_a` / `a_verifies_b` say
/// whether each side's admission marked the other verified.
fn link(a: &TestNode, b: &TestNode, a_verifies_b: bool, b_verifies_a: bool) {
    let (tx_ab, mut rx_ab) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    let (tx_ba, mut rx_ba) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    let (b_rt, a_id, back) = (b.rt.clone(), a.id.clone(), tx_ba.clone());
    tokio::spawn(async move {
        while let Some(bytes) = rx_ab.recv().await {
            let c = ctx(&a_id, b_verifies_a);
            let _ = b_rt.handle_incoming_peer(&bytes, back.clone(), Some(&c)).await;
        }
    });
    let (a_rt, b_id, back) = (a.rt.clone(), b.id.clone(), tx_ab.clone());
    tokio::spawn(async move {
        while let Some(bytes) = rx_ba.recv().await {
            let c = ctx(&b_id, a_verifies_b);
            let _ = a_rt.handle_incoming_peer(&bytes, back.clone(), Some(&c)).await;
        }
    });
    a.rt.add_peer(b.id.clone(), tx_ab);
    b.rt.add_peer(a.id.clone(), tx_ba);
}

fn cap(id: &str, p: Provenance) -> Capability {
    Capability::new(CapabilityId::new(id).unwrap(), p)
}

fn facts_of(n: &TestNode, caps: Vec<Capability>) -> NodeFacts {
    let mut f = NodeFacts::new(n.id.clone(), now(), 600, 0);
    f.capabilities = caps;
    f
}

fn now() -> u64 {
    FactsExchange::now()
}

async fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    for _ in 0..400 {
        if ok() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

fn held(on: &TestNode, of: &TestNode) -> Option<crate::node_facts::CachedNodeFacts> {
    on.ex.membership().facts().get(&of.id, now())
}

fn sample(a: &TestNode) -> NodeFacts {
    facts_of(
        a,
        vec![
            cap("cpu.arch.aarch64", Provenance::Probed),
            cap("accel.tpu.coral", Provenance::Probed).exclusive(),
            cap("perf.cog.cycle_ms", Provenance::Measured).with_attr("value", 790.0),
        ],
    )
}

#[tokio::test]
async fn verified_peer_facts_are_paired_and_never_measured() {
    let (a, b) = (node(1), node(2));
    link(&a, &b, true, true);
    let seq = a.ex.publish(sample(&a), now()).await.unwrap();

    wait_for("b to cache a's facts", || held(&b, &a).is_some()).await;
    let got = held(&b, &a).unwrap();
    assert_eq!(got.trust_tier(), TrustTier::Paired);
    assert_eq!(got.facts.seq, seq);
    assert!(
        got.capabilities().iter().all(|c| c.provenance <= Provenance::Probed),
        "a remote node's measured claim must not be held as measured"
    );
    let perf = got.facts.find("perf.cog.cycle_ms").next().unwrap();
    assert_eq!(perf.provenance, Provenance::Probed);
    // The envelope is kept as received, so it still verifies and still says measured.
    let original = verify_node_facts(&got.signed, now()).unwrap();
    assert_eq!(
        original.find("perf.cog.cycle_ms").next().unwrap().provenance,
        Provenance::Measured
    );
    // The sender's own cache holds its facts as the local node.
    assert_eq!(held(&a, &a).unwrap().trust_tier(), TrustTier::Pinned);
}

#[tokio::test]
async fn unverified_or_observe_mode_peer_gets_the_lowest_tier_and_claimed_data() {
    let (a, b) = (node(3), node(4));
    link(&a, &b, false, false); // observe mode: no peer is marked verified
    a.ex.publish(sample(&a), now()).await.unwrap();

    wait_for("b to cache a's facts", || held(&b, &a).is_some()).await;
    let got = held(&b, &a).unwrap();
    assert_eq!(got.trust_tier(), TrustTier::Discovered);
    assert!(got.capabilities().iter().all(|c| c.provenance == Provenance::Claimed));
}

#[tokio::test]
async fn deltas_carry_live_state_and_a_changed_shape_rebases() {
    let (a, b) = (node(5), node(6));
    link(&a, &b, true, true);
    let base = sample(&a);
    let s1 = a.ex.publish(base.clone(), now()).await.unwrap();
    wait_for("base", || held(&b, &a).is_some()).await;

    // Nothing changed: nothing sent.
    assert_eq!(a.ex.update_live(base.clone(), now()).await.unwrap(), Published::Nothing);

    // The Coral is now busy: a delta, not a new base.
    let mut busy = base.clone();
    busy.capabilities[1].state = CapabilityState::Busy;
    let p = a.ex.update_live(busy.clone(), now()).await.unwrap();
    assert_eq!(p, Published::Delta { seq: 1, changes: 1 });
    wait_for("delta", || {
        held(&b, &a).is_some_and(|c| c.delta_seq == 1)
    })
    .await;
    let got = held(&b, &a).unwrap();
    assert_eq!(got.facts.seq, s1, "a delta does not change the base");
    assert_eq!(got.facts.capabilities[1].state, CapabilityState::Busy);
    assert_eq!(got.load().busy, 1);

    // Free again, with free memory reported: another delta.
    let mut free = base.clone();
    free.capabilities.push(cap("mem.system", Provenance::Probed).with_attr("free", 100i64));
    // A new capability is a shape change: a new base, higher seq.
    let p = a.ex.update_live(free, now()).await.unwrap();
    let Published::Base(s2) = p else { panic!("expected a new base, got {p:?}") };
    assert!(s2 > s1);
    wait_for("rebase", || held(&b, &a).is_some_and(|c| c.facts.seq == s2)).await;
    let got = held(&b, &a).unwrap();
    assert_eq!(got.delta_seq, 0);
    assert_eq!(got.facts.capabilities[1].state, CapabilityState::Available);
}

#[tokio::test]
async fn a_peer_that_joins_later_is_sent_the_current_facts_and_delta() {
    let (a, b) = (node(7), node(8));
    let base = sample(&a);
    a.ex.publish(base.clone(), now()).await.unwrap();
    let mut busy = base;
    busy.capabilities[1].state = CapabilityState::Busy;
    a.ex.update_live(busy, now()).await.unwrap();

    link(&a, &b, true, true); // join after the fact
    wait_for("announce on join", || {
        held(&b, &a).is_some_and(|c| c.delta_seq == 1)
    })
    .await;
    assert_eq!(held(&b, &a).unwrap().load().busy, 1);
}

#[tokio::test]
async fn forged_facts_are_rejected() {
    let (a, b, c) = (node(9), node(10), node(11));
    let good = sign_node_facts(&sample(&a), &SigningKey::from_bytes(&[9; 32])).unwrap();
    let mut replies = Vec::new();
    let from_a = ctx(&a.id, true);

    // Payload altered after signing.
    let mut tampered = good.clone();
    tampered.payload = tampered.payload.replace("790", "1");
    let r = b.ex.ingest(&from_a, FactsWire::Facts { signed: tampered }, now(), &mut replies);
    assert!(matches!(r, Err(IngestError::Cache(CacheError::Verify(NodeFactsAdvertError::BadSignature)))), "{r:?}");

    // Signature bit flipped.
    let mut flipped = good.clone();
    flipped.signature[0] ^= 1;
    let r = b.ex.ingest(&from_a, FactsWire::Facts { signed: flipped }, now(), &mut replies);
    assert!(r.is_err());

    // C re-signs A's facts with its own key: a valid signature, but the key
    // does not own A's node id.
    let ckey = SigningKey::from_bytes(&[11; 32]);
    let mut spoof = good.clone();
    spoof.public_key = ckey.verifying_key().to_bytes().to_vec();
    let mut msg = FACTS_DOMAIN.to_vec();
    msg.extend_from_slice(spoof.payload.as_bytes());
    spoof.signature = ckey.sign(&msg).to_bytes().to_vec();
    let r = b.ex.ingest(&from_a, FactsWire::Facts { signed: spoof }, now(), &mut replies);
    assert!(matches!(r, Err(IngestError::Cache(CacheError::Verify(NodeFactsAdvertError::NodeMismatch { .. })))), "{r:?}");

    assert!(held(&b, &a).is_none(), "nothing forged may be cached");

    // Genuine facts, but relayed by a connection that is not their subject.
    let from_c = ctx(&c.id, true);
    let r = b.ex.ingest(&from_c, FactsWire::Facts { signed: good.clone() }, now(), &mut replies);
    assert!(matches!(r, Err(IngestError::NotSender { .. })), "{r:?}");

    // Nobody can replace this node's own facts from the wire.
    let own = sign_node_facts(&facts_of(&b, vec![]), &SigningKey::from_bytes(&[10; 32])).unwrap();
    let r = b.ex.ingest(&ctx(&b.id, true), FactsWire::Facts { signed: own }, now(), &mut replies);
    assert_eq!(r, Err(IngestError::LocalNode));

    // And the genuine block from its owner is accepted.
    assert!(b.ex.ingest(&from_a, FactsWire::Facts { signed: good }, now(), &mut replies).is_ok());
}

#[tokio::test]
async fn a_later_frame_never_lowers_a_nodes_tier_and_old_facts_cannot_roll_back() {
    let (a, b) = (node(12), node(13));
    let ka = SigningKey::from_bytes(&[12; 32]);
    let mut replies = Vec::new();
    let mut f1 = sample(&a);
    f1.seq = 10;
    let s1 = sign_node_facts(&f1, &ka).unwrap();
    let mut f2 = sample(&a);
    f2.seq = 11;
    let s2 = sign_node_facts(&f2, &ka).unwrap();

    let out = b.ex.ingest(&ctx(&a.id, true), FactsWire::Facts { signed: s1.clone() }, now(), &mut replies).unwrap();
    assert!(matches!(out, IngestOutcome::Accepted { tier: TrustTier::Paired, .. }));
    // Newer facts over an unverified connection keep the earned tier.
    let out = b.ex.ingest(&ctx(&a.id, false), FactsWire::Facts { signed: s2 }, now(), &mut replies).unwrap();
    assert!(matches!(out, IngestOutcome::Accepted { tier: TrustTier::Paired, outcome: InsertOutcome::Replaced, .. }), "{out:?}");
    // Replaying the older block is stale.
    let r = b.ex.ingest(&ctx(&a.id, true), FactsWire::Facts { signed: s1 }, now(), &mut replies);
    assert!(matches!(r, Err(IngestError::Cache(CacheError::Stale { .. }))), "{r:?}");
}

#[tokio::test]
async fn a_delta_without_its_base_asks_for_the_facts() {
    let (a, b) = (node(14), node(15));
    let base = sample(&a);
    a.ex.publish(base.clone(), now()).await.unwrap();
    let mut busy = base;
    busy.capabilities[1].state = CapabilityState::Busy;
    a.ex.update_live(busy, now()).await.unwrap();
    let delta = a.ex.current().into_iter().find(|w| matches!(w, FactsWire::Delta { .. })).unwrap();

    let mut replies = Vec::new();
    let r = b.ex.ingest(&ctx(&a.id, true), delta, now(), &mut replies);
    assert!(matches!(r, Err(IngestError::Cache(CacheError::UnknownNode(_)))), "{r:?}");
    assert_eq!(replies, vec![FactsWire::Request]);

    // A answers the request with its base and latest delta.
    let mut answers = Vec::new();
    a.ex.ingest(&ctx(&b.id, true), FactsWire::Request, now(), &mut answers).unwrap();
    assert_eq!(answers.len(), 2);
    for w in answers {
        b.ex.ingest(&ctx(&a.id, true), w, now(), &mut Vec::new()).unwrap();
    }
    assert_eq!(held(&b, &a).unwrap().load().busy, 1);
}
