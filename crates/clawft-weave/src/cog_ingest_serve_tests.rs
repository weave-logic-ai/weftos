use super::*;

const PROJECT: &str = "01J9ZXW0PRJCTAAAAAAAAAAAAA";

fn key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}

fn cfg(json: &str) -> Result<IngestConfig, String> {
    let c: IngestConfig = serde_json::from_str(json).map_err(|e| e.to_string())?;
    c.validate()?;
    Ok(c)
}

#[test]
fn defaults_are_loopback_port_80_with_the_documented_budgets() {
    let tmp = tempfile::tempdir().unwrap();
    let c = load_config(tmp.path()).unwrap();
    assert_eq!(c.bridge.bind, "127.0.0.1:80");
    assert_eq!((c.bridge.requests_per_sec, c.bridge.vectors_per_sec), (20, 2048));
    assert!(c.routes.is_empty() && c.store_owner.is_none() && c.bridge.container_bind.is_none());
}

#[test]
fn config_is_validated_at_the_boundary() {
    assert!(cfg("{}").is_ok());
    for (name, j) in [
        ("public bind", r#"{"bridge":{"bind":"0.0.0.0:80"}}"#.to_string()),
        ("zero budget", r#"{"bridge":{"requests_per_sec":0}}"#.to_string()),
        ("bad container bind", r#"{"bridge":{"container_bind":"nope"}}"#.to_string()),
        ("unknown key", r#"{"bridge":{"x":1}}"#.to_string()),
        ("route needs a selector", r#"{"routes":[{"owner":"local"}]}"#.to_string()),
        ("route with both", format!(r#"{{"routes":[{{"project":"{PROJECT}","controller":"n","owner":"local"}}]}}"#)),
        ("bad project", r#"{"routes":[{"project":"nope","owner":"local"}]}"#.to_string()),
        ("bad owner key", format!(r#"{{"routes":[{{"project":"{PROJECT}","owner":{{"node":"n","key":"zz","addr":"h:1"}}}}]}}"#)),
        ("owner without forwarders", r#"{"store_owner":{"listen":"127.0.0.1:0","forwarders":[]}}"#.to_string()),
        ("bad forwarder key", r#"{"store_owner":{"listen":"127.0.0.1:0","forwarders":[{"key":"zz","projects":"*"}]}}"#.to_string()),
        ("forwarder scope is required", format!(r#"{{"store_owner":{{"listen":"127.0.0.1:0","forwarders":[{{"key":"{}"}}]}}}}"#, "ab".repeat(32))),
        ("forwarder scope must be * or ids", format!(r#"{{"store_owner":{{"listen":"127.0.0.1:0","forwarders":[{{"key":"{}","projects":"all"}}]}}}}"#, "ab".repeat(32))),
        ("unspecified container bind", r#"{"bridge":{"container_bind":"0.0.0.0"}}"#.to_string()),
        ("multicast container bind", r#"{"bridge":{"container_bind":"224.0.0.1"}}"#.to_string()),
        ("controllers on a controller route", r#"{"routes":[{"controller":"c","owner":"local","controllers":["x"]}]}"#.to_string()),
    ] {
        assert!(cfg(&j).is_err(), "{name} must be refused");
    }
    let k = "ab".repeat(32);
    let ok = format!(
        r#"{{"bridge":{{"container_bind":"192.168.64.1"}},
            "routes":[{{"project":"{PROJECT}","controllers":["peer-node"],"owner":{{"node":"n","key":"{k}","addr":"h:9472"}}}},
                      {{"controller":"c","owner":"local"}}],
            "store_owner":{{"listen":"127.0.0.1:0","forwarders":[{{"key":"{k}","projects":["{PROJECT}"]}}],
                           "projects":["{PROJECT}"],"fallback":true}}}}"#
    );
    assert!(cfg(&ok).is_ok());
}

#[tokio::test]
async fn start_binds_the_bridge_on_loopback_with_no_owner_service_by_default() {
    let c = cfg(r#"{"bridge":{"bind":"127.0.0.1:0"}}"#).unwrap();
    let k = key(5);
    let rt = start(&c, &k, None).await.unwrap();
    assert!(rt.bridge_addr.unwrap().ip().is_loopback() && rt.owner_addr.is_none());
    assert!(rt.hooks.is_enabled() && rt.bridge_error.is_none());
    assert!(rt.hooks.registry().is_empty());
}

#[tokio::test]
async fn an_unbindable_bridge_leaves_disabled_hooks_and_a_reported_owner_service() {
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let k = key(5);
    let pub_k = hex(&k.verifying_key().to_bytes());
    let c = cfg(&format!(
        r#"{{"bridge":{{"bind":"{}"}},
            "store_owner":{{"listen":"127.0.0.1:0","forwarders":[{{"key":"{pub_k}","projects":"*"}}]}}}}"#,
        held.local_addr().unwrap()
    ))
    .unwrap();
    let rt = start(&c, &k, None).await.unwrap();
    assert!(!rt.hooks.is_enabled() && rt.hooks.state() == "disabled");
    assert!(rt.bridge_addr.is_none());
    assert!(rt.bridge_error.as_deref().unwrap().contains("cannot bind"));
    assert!(rt.owner_addr.is_some(), "the owner service started and is reported");
}

/// Node B owns the project's store and serves `cog-store` over Noise TCP;
/// node A's daemon routes the project to it through its configured route.
#[tokio::test]
async fn bridge_on_one_daemon_delivers_to_the_store_owner_daemon_over_noise_tcp() {
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (ka, kb) = (key(6), key(7));
    let pub_a = hex(&ka.verifying_key().to_bytes());
    let pub_b = hex(&kb.verifying_key().to_bytes());
    let cb = cfg(&format!(
        r#"{{"bridge":{{"bind":"127.0.0.1:0"}},
            "routes":[{{"project":"{PROJECT}","owner":"local"}}],
            "store_owner":{{"listen":"127.0.0.1:0","forwarders":[{{"key":"{pub_a}","projects":"*"}}],
                           "projects":["{PROJECT}"]}}}}"#
    ))
    .unwrap();
    let rb = start(&cb, &kb, None).await.unwrap();
    let owner_addr = rb.owner_addr.unwrap();
    let node_b = clawft_kernel::node_id_from_pubkey(&kb.verifying_key().to_bytes());
    let ca = cfg(&format!(
        r#"{{"bridge":{{"bind":"127.0.0.1:0"}},
            "routes":[{{"project":"{PROJECT}","owner":{{"node":"{node_b}","key":"{pub_b}","addr":"{owner_addr}"}}}}]}}"#
    ))
    .unwrap();
    let ra = start(&ca, &ka, None).await.unwrap();

    // Register a cog instance on A and post as it.
    let contract = clawft_kernel::workload_runtime::HostContract::default_feed();
    let token = contract.token.expose().to_string();
    ra.hooks
        .registry()
        .register(
            clawft_kernel::cog_ingest::InstanceBinding::new("inst", Some(PROJECT.into()), "ctl"),
            &contract,
        )
        .unwrap();
    let body = r#"{"vectors":[[1,[0,1,2,3,4,5,6,7]]],"dedup":true}"#;
    let raw = format!(
        "POST /api/v1/store/ingest HTTP/1.1\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut s = tokio::net::TcpStream::connect(ra.bridge_addr.unwrap()).await.unwrap();
    s.write_all(raw.as_bytes()).await.unwrap();
    let mut out = String::new();
    tokio::time::timeout(Duration::from_secs(10), s.read_to_string(&mut out))
        .await
        .unwrap()
        .unwrap();
    assert!(out.starts_with("HTTP/1.1 200"), "{out}");
    assert!(out.contains(r#""accepted":1"#), "{out}");
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

struct Fake(Option<&'static str>);
impl ProjectDirectory for Fake {
    fn bound_node(&self, _: &str) -> Option<String> {
        self.0.map(String::from)
    }
}

#[test]
fn the_place_rpc_refuses_an_unregistered_revoked_or_malformed_project() {
    assert!(check_project_registered(Some(&Fake(Some("node"))), PROJECT).is_ok());
    let e = check_project_registered(Some(&Fake(None)), PROJECT).unwrap_err();
    assert!(e.contains("not registered, or its key is revoked"), "{e}");
    assert!(check_project_registered(Some(&Fake(Some("node"))), "nope").is_err());
    // No identity records here (a project daemon): the host's policy decides.
    assert!(check_project_registered(None, PROJECT).is_ok());
}

/// B owns only what `store_owner` lists, whatever B's own local routes say.
#[tokio::test]
async fn the_owner_service_serves_only_what_store_owner_lists() {
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    const P2: &str = "01J9ZXW0PRJCTBBBBBBBBBBBBB";
    let (ka, kb) = (key(8), key(9));
    let (pub_a, pub_b) = (hex(&ka.verifying_key().to_bytes()), hex(&kb.verifying_key().to_bytes()));
    // B: local route for P2 (its own cogs), store_owner lists P1 only, no fallback.
    let cb = cfg(&format!(
        r#"{{"bridge":{{"bind":"127.0.0.1:0"}},
            "routes":[{{"project":"{P2}","owner":"local"}},{{"controller":"b","owner":"local"}}],
            "store_owner":{{"listen":"127.0.0.1:0","forwarders":[{{"key":"{pub_a}","projects":"*"}}],
                           "projects":["{PROJECT}"],"fallback":false}}}}"#
    ))
    .unwrap();
    let rb = start(&cb, &kb, None).await.unwrap();
    let node_b = clawft_kernel::node_id_from_pubkey(&kb.verifying_key().to_bytes());
    let remote = format!(
        r#"{{"node":"{node_b}","key":"{pub_b}","addr":"{}"}}"#,
        rb.owner_addr.unwrap()
    );
    let ca = cfg(&format!(
        r#"{{"bridge":{{"bind":"127.0.0.1:0"}},
            "routes":[{{"project":"{PROJECT}","owner":{remote}}},
                      {{"project":"{P2}","owner":{remote}}},
                      {{"controller":"c","owner":{remote}}}]}}"#
    ))
    .unwrap();
    let ra = start(&ca, &ka, None).await.unwrap();
    let post = |project: Option<&'static str>, inst: &'static str| {
        let reg = ra.hooks.registry().clone();
        let addr = ra.bridge_addr.unwrap();
        async move {
            let c = clawft_kernel::workload_runtime::HostContract::default_feed();
            let token = c.token.expose().to_string();
            reg.register(
                clawft_kernel::cog_ingest::InstanceBinding::new(inst, project.map(String::from), "c"),
                &c,
            )
            .unwrap();
            let body = r#"{"vectors":[[1,[0,1,2,3,4,5,6,7]]]}"#;
            let raw = format!(
                "POST /api/v1/store/ingest HTTP/1.1\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
            s.write_all(raw.as_bytes()).await.unwrap();
            let mut out = String::new();
            tokio::time::timeout(Duration::from_secs(10), s.read_to_string(&mut out))
                .await
                .unwrap()
                .unwrap();
            out.split(' ').nth(1).unwrap().to_string()
        }
    };
    assert_eq!(post(Some(PROJECT), "i1").await, "200", "listed project");
    assert_eq!(post(Some(P2), "i2").await, "502", "B's local-route project is not served to remotes");
    assert_eq!(post(None, "i3").await, "502", "fallback is off");
}
