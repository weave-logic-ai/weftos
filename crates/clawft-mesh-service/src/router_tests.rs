use std::str::FromStr;

use clawft_mesh_local::proto::Frame;
use clawft_mesh_local::{node_id_from_pubkey, Principal};
use clawft_types::config::MeshAdmissionMode;
use ed25519_dalek::SigningKey;
use tokio::sync::mpsc;

use super::*;

const ULID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

struct Tenant {
    reg: Arc<Registration>,
    rx: mpsc::Receiver<Frame>,
    id: String,
}

struct Rig {
    router: Arc<TenantRouter>,
    registry: Arc<Registry>,
}

fn rig() -> Rig {
    let registry = Arc::new(Registry::new());
    let policy = PolicyCell::new(Some(501), MeshAdmissionMode::Observe);
    let router = TenantRouter::new(Arc::clone(&registry), policy, "node-local".into());
    Rig { router, registry }
}

fn tenant(r: &Rig, uid: u32, projects: &[&str], prefixes: &[&str]) -> Tenant {
    let id = node_id_from_pubkey(&[uid as u8; 32]);
    let (reg, rx, _) =
        Registration::new(uid as u64, Principal::Uid(uid), id.clone(), [uid as u8; 32], uid, String::new(), vec![], 0);
    let cert = user_cert_for(uid as u8);
    reg.set_cert(cert);
    let p: Vec<String> = projects.iter().map(|s| (*s).to_string()).collect();
    let x: Vec<String> = prefixes.iter().map(|s| (*s).to_string()).collect();
    r.registry.register(&reg, &p, &x).unwrap();
    Tenant { reg, rx, id }
}

fn user_cert_for(n: u8) -> clawft_mesh_local::UserCert {
    clawft_mesh_local::UserCert::issue(&SigningKey::from_bytes(&[9; 32]), [n; 32], 1, 0, 100)
}

fn msg(topic: &str) -> KernelMessage {
    KernelMessage::text(0, MessageTarget::Topic(topic.into()), "hi")
}

fn ctx(verified: bool) -> PeerCtx {
    PeerCtx {
        peer_id: "remote".into(),
        node_verified: verified,
        class: if verified { PeerClass::Node } else { PeerClass::Legacy },
        remote_static: None,
        src_scope: None,
    }
}

fn scope(user: &str, project: Option<&str>) -> WireScope {
    WireScope { user_id: user.into(), project_id: project.map(String::from) }
}

fn got(t: &mut Tenant) -> Option<Deliver> {
    match t.rx.try_recv() {
        Ok(Frame { msg: Message::Deliver(d), .. }) => Some(d),
        _ => None,
    }
}

fn count(a: &AtomicU64) -> u64 {
    a.load(Ordering::Relaxed)
}

#[tokio::test]
async fn admitted_peer_with_scope_reaches_that_tenant_and_project() {
    let r = rig();
    let mut a = tenant(&r, 501, &[], &[]);
    let mut b = tenant(&r, 502, &[ULID], &[]);
    r.router.deliver(&ctx(true), Some(&scope(&b.id, Some(ULID))), msg("t")).await.unwrap();
    let d = got(&mut b).expect("delivered to B");
    assert_eq!(d.scope, Scope { user_id: b.id.clone(), project_id: Some(ULID.into()) });
    assert_eq!(d.source_node, "remote");
    assert!(d.source_cert.is_none(), "a remote peer's certificate is not vouched for here");
    assert!(got(&mut a).is_none());
}

#[tokio::test]
async fn admitted_peer_with_unknown_scope_is_dropped_and_counted() {
    let r = rig();
    let mut a = tenant(&r, 501, &[], &[]);
    let b = tenant(&r, 502, &[], &[]);
    r.router.deliver(&ctx(true), Some(&scope("f".repeat(32).as_str(), None)), msg("t")).await.unwrap();
    r.router.deliver(&ctx(true), Some(&scope(&b.id, Some(ULID))), msg("t")).await.unwrap();
    assert_eq!(count(&r.router.counters.unknown_scope), 2, "unknown user, and a project B does not own");
    assert!(got(&mut a).is_none());
}

#[tokio::test]
async fn admitted_unscoped_uses_longest_prefix_then_sole_user_else_scope_required() {
    let r = rig();
    let mut a = tenant(&r, 501, &[], &["sub/"]);
    r.router.deliver(&ctx(true), None, msg("other")).await.unwrap();
    assert!(got(&mut a).is_some(), "single registered user is the default");
    let mut b = tenant(&r, 502, &[], &["sub/x/"]);
    // B's claim overlaps A's, so it was refused: prefix routing sends sub/x/1 to A.
    r.router.deliver(&ctx(true), None, msg("sub/x/1")).await.unwrap();
    assert!(got(&mut a).is_some());
    let mut c = tenant(&r, 503, &[], &["mine/"]);
    r.router.deliver(&ctx(true), None, msg("mine/1")).await.unwrap();
    assert!(got(&mut c).is_some());
    r.router.deliver(&ctx(true), None, msg("nothing/matches")).await.unwrap();
    assert_eq!(count(&r.router.counters.scope_required), 1);
    assert!(got(&mut a).is_none() && got(&mut b).is_none() && got(&mut c).is_none());
}

#[tokio::test]
async fn unadmitted_peer_cannot_reach_a_non_default_tenant() {
    let r = rig();
    let mut a = tenant(&r, 501, &[], &[]); // cluster owner, so the default tenant
    let mut b = tenant(&r, 502, &[], &["b/"]);
    r.router.deliver(&ctx(false), Some(&scope(&b.id, None)), msg("t")).await.unwrap();
    assert_eq!(count(&r.router.counters.denied_scope), 1);
    assert!(got(&mut b).is_none(), "scope is only a claim for an unadmitted peer");
    assert!(got(&mut a).is_none(), "and it is not rerouted either");
    // A topic inside B's prefix does not move traffic to B for an unadmitted peer.
    r.router.deliver(&ctx(false), None, msg("b/secret")).await.unwrap();
    assert!(got(&mut b).is_none());
    assert!(got(&mut a).is_some(), "it goes to the default tenant");
    // Naming the default tenant is fine.
    r.router.deliver(&ctx(false), Some(&scope(&a.id, None)), msg("t")).await.unwrap();
    assert!(got(&mut a).is_some());
}

#[tokio::test]
async fn unadmitted_peer_with_one_user_reaches_that_user() {
    let r = rig();
    let mut b = tenant(&r, 502, &[], &[]);
    r.router.deliver(&ctx(false), None, msg("t")).await.unwrap();
    assert!(got(&mut b).is_some(), "the sole user is the default");
}

#[tokio::test]
async fn nobody_registered_drops_and_counts() {
    let r = rig();
    r.router.deliver(&ctx(false), None, msg("t")).await.unwrap();
    r.router.deliver(&ctx(true), None, msg("t")).await.unwrap();
    assert_eq!(count(&r.router.counters.no_tenant), 2);
}

#[tokio::test]
async fn a_full_queue_is_an_error_and_counted() {
    let r = rig();
    let a = tenant(&r, 501, &[], &[]);
    for _ in 0..crate::registry::QUEUE_CAP {
        r.router.deliver(&ctx(false), None, msg("t")).await.unwrap();
    }
    assert!(r.router.deliver(&ctx(false), None, msg("t")).await.is_err());
    assert_eq!(count(&r.router.counters.dropped_full), 1);
    assert_eq!(count(&a.reg.counters.dropped_full), 1);
}

#[tokio::test]
async fn local_send_between_tenants_carries_the_senders_certificate() {
    let r = rig();
    let a = tenant(&r, 501, &[], &[]);
    let mut b = tenant(&r, 502, &[], &[]);
    b.reg.set_accept_from(vec![node_id_from_pubkey(&[501u32 as u8; 32])]);
    let dest = WeftAddr::from_str(&format!("weft://local/{}/_/chat", b.id)).unwrap();
    r.router.route_outbound(&a.reg, &dest, msg("ignored")).await.unwrap();
    let d = got(&mut b).expect("B receives it");
    assert_eq!(d.source_node, "node-local");
    assert_eq!(d.source_cert.as_ref().map(|c| c.user_pubkey), Some([501u32 as u8; 32]));
    let km: KernelMessage = serde_json::from_value(d.message).unwrap();
    assert!(matches!(km.target, MessageTarget::Topic(t) if t == "chat"), "the address topic wins");
}

#[tokio::test]
async fn local_send_to_an_unregistered_user_is_unknown_scope() {
    let r = rig();
    let a = tenant(&r, 501, &[], &[]);
    let ghost = format!("weft://local/{}/_/t", "e".repeat(32));
    let dest = WeftAddr::from_str(&ghost).unwrap();
    assert!(matches!(
        r.router.route_outbound(&a.reg, &dest, msg("t")).await,
        Err(SendError::UnknownScope(_))
    ));
}

#[tokio::test]
async fn remote_send_without_a_runtime_fails_cleanly() {
    let r = rig();
    let a = tenant(&r, 501, &[], &[]);
    let dest = WeftAddr::from_str(&format!("weft://{}/_/_/t", "d".repeat(32))).unwrap();
    assert!(matches!(r.router.route_outbound(&a.reg, &dest, msg("t")).await, Err(SendError::Failed(_))));
}

#[tokio::test]
async fn subscribe_authorisation_follows_the_same_tenant_rule() {
    let r = rig();
    let a = tenant(&r, 501, &[], &[]);
    let b = tenant(&r, 502, &[], &[]);
    assert!(r.router.authorize_subscribe(&ctx(true), "t", Some(&scope(&b.id, None))).await);
    assert!(!r.router.authorize_subscribe(&ctx(false), "t", Some(&scope(&b.id, None))).await);
    assert!(r.router.authorize_subscribe(&ctx(false), "t", Some(&scope(&a.id, None))).await);
    assert!(r.router.authorize_subscribe(&ctx(false), "t", None).await);
}

#[tokio::test]
async fn cross_tenant_local_send_needs_the_recipient_to_opt_in() {
    let r = rig();
    let a = tenant(&r, 501, &[], &[]);
    let mut b = tenant(&r, 502, &[], &[]);
    let dest = WeftAddr::from_str(&format!("weft://local/{}/_/chat", b.id)).unwrap();
    assert!(matches!(
        r.router.route_outbound(&a.reg, &dest, msg("t")).await,
        Err(SendError::Forbidden(_))
    ));
    assert!(got(&mut b).is_none());
    // A tenant may always send to itself.
    let me = WeftAddr::from_str(&format!("weft://local/{}/_/chat", a.id)).unwrap();
    r.router.route_outbound(&a.reg, &me, msg("t")).await.unwrap();
    // "*" opts everyone in.
    b.reg.set_accept_from(vec!["*".into()]);
    r.router.route_outbound(&a.reg, &dest, msg("t")).await.unwrap();
    assert!(got(&mut b).is_some());
}
