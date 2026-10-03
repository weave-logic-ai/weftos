use super::*;

fn bits(len: u32, set: &[u32]) -> Bitfield {
    let mut b = Bitfield::new(len);
    for &i in set {
        b.set(i, true);
    }
    b
}

#[test]
fn rarest_piece_is_requested_first() {
    let mut p = PiecePicker::new(Bitfield::new(6));
    p.add_peer("a", bits(6, &[0, 1, 2, 3, 4, 5]));
    p.add_peer("b", bits(6, &[0, 1, 2, 3, 4]));
    p.add_peer("c", bits(6, &[0, 1, 2, 3]));
    // Piece 5 is held only by `a`, 4 by `a` and `b`; 0-3 by everyone.
    assert_eq!(p.availability(5), 1);
    assert_eq!(p.availability(4), 2);
    assert_eq!(p.pick("a"), Pick::Piece(5));
    assert_eq!(p.pick("b"), Pick::Piece(4));
    // Common pieces last, lowest index first.
    assert_eq!(p.pick("c"), Pick::Piece(0));
}

#[test]
fn a_piece_is_never_in_flight_twice_and_returns_when_released() {
    let mut p = PiecePicker::new(Bitfield::new(2));
    p.add_peer("a", bits(2, &[0, 1]));
    p.add_peer("b", bits(2, &[0, 1]));
    assert_eq!(p.pick("a"), Pick::Piece(0));
    assert_eq!(p.pick("b"), Pick::Piece(1));
    assert_eq!(p.pick("a"), Pick::Wait, "everything left is in flight at b");
    p.complete(1);
    p.release(0);
    assert_eq!(p.pick("b"), Pick::Piece(0));
    p.complete(0);
    assert!(p.is_complete());
    assert_eq!(p.pick("a"), Pick::Nothing);
}

#[test]
fn losing_a_peer_frees_its_pieces_and_drops_its_holdings() {
    let mut p = PiecePicker::new(Bitfield::new(3));
    p.add_peer("dies", bits(3, &[0, 1, 2]));
    p.add_peer("lives", bits(3, &[0, 1]));
    assert_eq!(p.pick("dies"), Pick::Piece(2)); // rarest: only `dies` has it
    p.remove_peer("dies");
    assert_eq!(p.unavailable(), 1, "piece 2 now has no holder");
    assert_eq!(p.pick("lives"), Pick::Piece(0));
    assert_eq!(p.pick("gone"), Pick::Nothing);
}

#[test]
fn a_peer_that_does_not_have_the_piece_after_all_is_not_asked_again() {
    let mut p = PiecePicker::new(Bitfield::new(2));
    p.add_peer("a", bits(2, &[0, 1]));
    assert_eq!(p.pick("a"), Pick::Piece(0));
    p.peer_lacks("a", 0);
    assert_eq!(p.pick("a"), Pick::Piece(1));
    assert_eq!(p.availability(0), 0);
}

#[test]
fn candidates_rank_by_lan_then_measured_speed_then_id_and_bans_drop() {
    let links = LinkStats::default();
    links.record("slow-lan", 1_000, 1.0);
    links.record("fast-wan", 9_000_000_000, 1.0);
    links.record("fast-lan", 5_000_000_000, 1.0);
    let cands = vec![
        PeerCandidate::new("slow-lan").on_lan("home"),
        PeerCandidate::new("fast-wan").on_lan("cloud"),
        PeerCandidate::new("fast-lan").on_lan("home"),
        PeerCandidate::new("never-measured"),
        PeerCandidate::new("banned").on_lan("home"),
    ];
    let ordered = order_peers(cands, Some("home"), &links, &|p| p == "banned");
    let ids: Vec<_> = ordered.iter().map(|c| c.peer_id.as_str()).collect();
    // Same LAN first (fast before slow), then the rest by speed: the
    // unmeasured peer ranks at the assumed default, below the 9 GB/s one.
    assert_eq!(ids, vec!["fast-lan", "slow-lan", "fast-wan", "never-measured"]);
}

#[test]
fn link_stats_smooth_new_samples() {
    let l = LinkStats::default();
    assert_eq!(l.bytes_per_sec("p"), None);
    l.record("p", 1000, 1.0);
    l.record("p", 2000, 1.0);
    let v = l.bytes_per_sec("p").unwrap();
    assert!(v > 1000.0 && v < 2000.0, "{v}");
    l.record("p", 0, 1.0); // ignored
    assert_eq!(l.bytes_per_sec("p"), Some(v));
}
