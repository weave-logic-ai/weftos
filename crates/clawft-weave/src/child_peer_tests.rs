//! Tests for [`super`]: pure classification plus a real process group.

use super::*;

#[test]
fn only_a_same_uid_peer_outside_every_child_group_is_the_owner() {
    let pgids = |pid: i32| Some(if pid == 100 { 4242 } else { pid });
    // Inside child 4242's group (the child itself, or anything it spawned).
    assert_eq!(classify(501, Some(100), 501, &[4242, 7], pgids), PeerClass::Child);
    assert_eq!(classify(501, Some(4242), 501, &[4242], pgids), PeerClass::Child);
    // The owner's CLI.
    assert_eq!(classify(501, Some(555), 501, &[4242], pgids), PeerClass::Owner);
    // No supervised children, unknown pid or unreadable group: still the owner (same uid).
    assert_eq!(classify(501, Some(100), 501, &[], pgids), PeerClass::Owner);
    assert_eq!(classify(501, None, 501, &[4242], pgids), PeerClass::Owner);
    assert_eq!(classify(501, Some(100), 501, &[4242], |_| None), PeerClass::Owner);
    // Another uid is never the owner, children or not.
    assert_eq!(classify(0, Some(555), 501, &[], pgids), PeerClass::OtherUid);
    assert!(PeerClass::Owner.is_owner() && !PeerClass::Child.is_owner() && !PeerClass::OtherUid.is_owner());
}

#[cfg(unix)]
#[tokio::test]
async fn a_process_in_a_childs_own_group_is_classified_from_real_credentials() {
    use std::os::unix::process::CommandExt;
    // A "child kernel": its own process group, so pgid == pid.
    let mut child = std::process::Command::new("sleep").arg("30").process_group(0).spawn().unwrap();
    let child_pid = child.id();
    let (a, _b) = tokio::net::UnixStream::pair().unwrap();
    // This test process is the peer. It is not in the child's group...
    let cred = a.peer_cred();
    assert_eq!(classify_peer_with(cred, &[child_pid]), PeerClass::Owner);
    // ...but it is once the supervised set names its own group.
    let my_group = nix::unistd::getpgrp().as_raw() as u32;
    let cred = a.peer_cred();
    assert_eq!(classify_peer_with(cred, &[my_group]), PeerClass::Child);
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn classify_peer_with(cred: std::io::Result<tokio::net::unix::UCred>, children: &[u32]) -> PeerClass {
    let c = cred.unwrap();
    classify(c.uid(), c.pid(), nix::unistd::geteuid().as_raw(), children, |pid| {
        nix::unistd::getpgid(Some(nix::unistd::Pid::from_raw(pid))).ok().map(nix::unistd::Pid::as_raw)
    })
}
