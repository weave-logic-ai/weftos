use super::*;

const ULID_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const ULID_B: &str = "01BX5ZZKBKACTAV9WEVGEMMVRZ";

fn reg(conn: u64, uid: u32, user: &str, pid: u32) -> (Arc<Registration>, mpsc::Receiver<Frame>) {
    let (r, rx, _) = Registration::new(
        conn,
        Principal::Uid(uid),
        user.into(),
        [uid as u8; 32],
        pid,
        String::new(),
        vec![],
        0,
    );
    (r, rx)
}

#[test]
fn second_registration_for_a_user_names_the_holder_pid() {
    let reg_ = Registry::new();
    let (a, _ra) = reg(1, 501, "u1", 111);
    let (b, _rb) = reg(2, 501, "u1", 222);
    reg_.register(&a, &[], &[]).unwrap();
    assert_eq!(reg_.register(&b, &[], &[]), Err(RegisterError::InUse { holder_pid: 111 }));
}

#[test]
fn stale_unregister_cannot_evict_the_successor() {
    let r = Registry::new();
    let (a, _ra) = reg(1, 501, "u1", 1);
    r.register(&a, &[], &["x/".into()]).unwrap();
    assert!(!r.unregister("u1", 99));
    assert!(r.get("u1").is_some());
    assert!(r.unregister("u1", 1));
    assert!(r.get("u1").is_none());
    assert!(r.longest_prefix("x/y").is_none(), "claims leave with the registration");
}

#[test]
fn prefixes_conflict_across_users_including_overlap() {
    let r = Registry::new();
    let (a, _ra) = reg(1, 501, "u1", 1);
    let (b, _rb) = reg(2, 502, "u2", 2);
    r.register(&a, &[], &["substrate/".into()]).unwrap();
    let out = r.register(&b, &[], &["substrate/".into(), "substrate/x/".into(), "other/".into()]).unwrap();
    assert_eq!(out.topic_prefixes, vec!["other/".to_string()]);
    assert_eq!(out.rejected.len(), 2);
}

#[test]
fn longest_prefix_wins_and_sole_needs_exactly_one() {
    let r = Registry::new();
    let (a, _ra) = reg(1, 501, "u1", 1);
    r.register(&a, &[], &["a/".into()]).unwrap();
    assert_eq!(r.longest_prefix("a/b").unwrap().user_id, "u1");
    assert!(r.longest_prefix("z").is_none());
    assert!(r.sole().is_some());
    let (b, _rb) = reg(2, 502, "u2", 2);
    r.register(&b, &[], &[]).unwrap();
    assert!(r.sole().is_none());
}

#[test]
fn projects_are_owned_and_validated() {
    let r = Registry::new();
    let (a, _ra) = reg(1, 501, "u1", 1);
    let (b, _rb) = reg(2, 502, "u2", 2);
    let out = r.register(&a, &[ULID_A.into(), "not-a-ulid".into()], &[]).unwrap();
    assert_eq!(out.addresses, vec![ULID_A.to_string()]);
    let out = r.register(&b, &[ULID_A.into(), ULID_B.into()], &[]).unwrap();
    assert_eq!(out.addresses, vec![ULID_B.to_string()], "cannot take another user's project");
    assert_eq!(r.lookup_scope("u1", Some(ULID_A)).unwrap().user_id, "u1");
    assert_eq!(r.lookup_scope("u1", Some(ULID_B)).err(), Some(ScopeMiss::UnknownProject));
    assert_eq!(r.lookup_scope("nobody", None).err(), Some(ScopeMiss::Unknown));
}

#[test]
fn certified_project_key_tracks_rekey_revoke_and_disconnect() {
    let r = Registry::new();
    let (a, _rx) = reg(1, 501, "u1", 1);
    r.register(&a, &[], &[]).unwrap();
    assert_eq!(r.current_project_key("u1", ULID_A), None);
    r.add_certified_project("u1", ULID_A, [1; 32]).unwrap();
    assert_eq!(r.current_project_key("u1", ULID_A), Some([1; 32]));
    r.add_certified_project("u1", ULID_A, [2; 32]).unwrap();
    assert_eq!(r.current_project_key("u1", ULID_A), Some([2; 32]));
    assert_eq!(r.current_project_key("u2", ULID_A), None);
    assert!(r.remove_project("u1", ULID_A));
    assert_eq!(r.current_project_key("u1", ULID_A), None);
    r.add_certified_project("u1", ULID_A, [3; 32]).unwrap();
    assert!(r.unregister("u1", 1));
    assert_eq!(r.current_project_key("u1", ULID_A), None);
}

#[test]
fn empty_and_whitespace_prefixes_are_refused() {
    let r = Registry::new();
    let (a, _ra) = reg(1, 501, "u1", 1);
    let out = r.register(&a, &[], &["".into(), "a b".into()]).unwrap();
    assert!(out.topic_prefixes.is_empty());
    assert_eq!(out.rejected.len(), 2);
}

#[test]
fn deliver_cannot_use_the_slots_reserved_for_verdicts() {
    let (a, _rx) = reg(1, 501, "u1", 1);
    for _ in 0..QUEUE_CAP {
        a.try_queue(Frame::new(clawft_mesh_local::Message::Ping {})).unwrap();
    }
    assert_eq!(a.try_queue(Frame::new(clawft_mesh_local::Message::Ping {})), Err(QueueError::Full));
    for _ in 0..RESERVED_SLOTS {
        a.try_queue_priority(Frame::new(clawft_mesh_local::Message::Ping {})).unwrap();
    }
    assert_eq!(a.try_queue_priority(Frame::new(clawft_mesh_local::Message::Ping {})), Err(QueueError::Full));
}

#[test]
fn send_budget_is_per_window_and_accept_from_is_opt_in() {
    let (a, _rx) = reg(1, 501, "u1", 1);
    let t0 = Instant::now();
    for _ in 0..SEND_LIMIT {
        assert!(a.allow_send_at(t0));
    }
    assert!(!a.allow_send_at(t0));
    assert!(a.allow_send_at(t0 + SEND_WINDOW));
    assert!(a.accepts("u1"), "a tenant may send to itself");
    assert!(!a.accepts("u2"));
    a.set_accept_from(vec!["u2".into()]);
    assert!(a.accepts("u2") && !a.accepts("u3"));
    a.set_accept_from(vec!["*".into()]);
    assert!(a.accepts("u3"));
}

#[test]
fn full_queue_drops_and_counts() {
    let (a, _rx) = reg(1, 501, "u1", 1);
    for _ in 0..QUEUE_CAP {
        a.try_queue(Frame::new(clawft_mesh_local::Message::Ping {})).unwrap();
    }
    assert_eq!(a.try_queue(Frame::new(clawft_mesh_local::Message::Ping {})), Err(QueueError::Full));
    assert_eq!(a.counters.dropped_full.load(Ordering::Relaxed), 1);
}
