//! Real two-uid check: a connection from another account gets that account's
//! uid from the peer credential and cannot use admin verbs. Needs
//! `sudo -n -u <user>` and a world-traversable directory, so it is `#[ignore]`
//! and never part of the gate. Run with `scripts/dev/mesh-two-uid.sh`.

mod common;

use std::process::Command;

use clawft_mesh_local::proto::Message;
use common::*;

const CLIENT: &str = r#"
import json, socket, sys
s = socket.socket(socket.AF_UNIX)
s.connect(sys.argv[1])
hello = {"t": "hello", "proto_min": 1, "proto_max": 1, "role": sys.argv[2], "client_nonce": "00" * 32}
s.sendall((json.dumps(hello) + "\n").encode())
print(s.makefile().readline().strip())
"#;

fn as_user(user: &str, socket: &std::path::Path, role: &str) -> serde_json::Value {
    let out = Command::new("sudo")
        .args(["-n", "-u", user, "python3", "-c", CLIENT])
        .arg(socket)
        .arg(role)
        .output()
        .expect("sudo python3 runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).expect("one JSON line")
}

#[tokio::test]
#[ignore = "needs sudo -n -u <user> and WEFTOS_TWO_UID_DIR; run scripts/dev/mesh-two-uid.sh"]
async fn a_real_second_uid_is_identified_by_the_kernel_and_is_not_an_admin() {
    let user = std::env::var("WEFTOS_TWO_UID_USER").unwrap_or_else(|_| "nobody".into());
    let base = std::env::var("WEFTOS_TWO_UID_DIR").expect("WEFTOS_TWO_UID_DIR (world-traversable)");
    let uid: u32 = String::from_utf8(Command::new("id").args(["-u", &user]).output().unwrap().stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let h = Harness::with(|c, _| {
        c.state_dir = std::path::Path::new(&base).join(format!("st{}", std::process::id()));
        c.socket = std::path::Path::new(&base).join(format!("r{}", std::process::id())).join("s");
    })
    .await;

    let ack = as_user(&user, &h.socket(), "user");
    assert_eq!(ack["t"], "hello_ack");
    assert_eq!(ack["uid"], uid, "the service read the other account's uid from the kernel");
    assert_ne!(uid, h.euid);

    let refused = as_user(&user, &h.socket(), "admin");
    assert_eq!(refused["t"], "error");
    assert_eq!(refused["kind"], "forbidden");

    // And the service owner (an admin) is unaffected.
    assert!(
        h.admin_ok(Message::Status {}).await["node_id"]
            .as_str()
            .is_some()
    );
}
