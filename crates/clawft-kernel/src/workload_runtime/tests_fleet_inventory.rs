//! The Cognitum cloud fleet inventory adapter: parsing the `fleet_status`
//! fixture, read-only enforcement, the local-versus-cloud key check, and
//! the MCP transport against a local stub (never the real cloud).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use clawft_types::secret::SecretString;
use serde_json::{Value, json};
use wiremock::matchers::{header, method};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use super::fleet_inventory::{
    EVENT_KIND_FLEET_INVENTORY, FleetCheck, FleetError, FleetInventory, FleetMcp, READ_TOOLS,
    ReadOnlyFleet, parse_fleet_status,
};
use super::fleet_mcp::{HttpFleetMcp, OAuthTokens};
use super::seed::{SEED_CONCURRENCY_CAP, SeedApiRuntime, SeedConfig, SeedPin};
use super::seed_http::HttpSeedTransport;
use super::seed_tls::SeedTls;
use super::test_support::MemoryCredentials;
use crate::chain::{ChainManager, EVENT_KIND_WORKLOAD_REFUSE};

const FIXTURE: &str = include_str!("fixtures/cognitum_fleet_status.json");
const OAUTH: &str = "oauth-access-token-fleet-0123456789";
const SEED_TOKEN: &str = "seed-token-fleet-0123456789";
const DEVICE: &str = "4b0c1b1e-7a52-4f0e-8d3a-0d3c8f9a1e11";
const KEY: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";

/// A transport that records every tool it is asked for and answers
/// `fleet_status` with the fixture. Any non-read tool fails the test.
#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<String>>,
}

#[async_trait]
impl FleetMcp for Recorder {
    async fn call_tool(&self, name: &str, _args: Value) -> Result<Value, FleetError> {
        self.calls.lock().unwrap().push(name.to_string());
        assert!(
            READ_TOOLS.contains(&name),
            "non-read MCP tool invoked: {name}"
        );
        Ok(serde_json::from_str(FIXTURE).unwrap())
    }
}

fn inventory() -> (FleetInventory, Arc<Recorder>, Arc<ChainManager>) {
    let rec = Arc::new(Recorder::default());
    let chain = Arc::new(ChainManager::new(0, 1000));
    (
        FleetInventory::new(
            ReadOnlyFleet::new(rec.clone()),
            chain.clone(),
            "cognitum-oauth",
        ),
        rec,
        chain,
    )
}

#[test]
fn the_fixture_parses_in_every_accepted_spelling() {
    let (c, skipped) = parse_fleet_status(&serde_json::from_str(FIXTURE).unwrap()).unwrap();
    assert_eq!(skipped, 1, "an entry without a device id is skipped");
    assert_eq!(c.len(), 3);
    assert_eq!(c[0].device_id, DEVICE);
    assert_eq!(c[0].firmware.as_deref(), Some("0.24.2"));
    assert_eq!(c[0].online, Some(true));
    assert_eq!(c[0].device_pubkey.as_deref(), Some(KEY));
    assert_eq!(c[1].online, Some(false), "status: offline");
    assert_eq!(c[1].firmware.as_deref(), Some("0.22.20"));
    assert_eq!(c[2].device_pubkey, None);
    assert!(parse_fleet_status(&json!("nope")).is_err());
    assert!(parse_fleet_status(&json!({"unrelated": 1})).is_err());
    assert_eq!(
        parse_fleet_status(&json!([{"id": "x"}])).unwrap().0.len(),
        1
    );
}

#[tokio::test]
async fn the_inventory_only_ever_calls_the_read_tool_and_chains_the_read() {
    let (inv, rec, chain) = inventory();
    let list = inv.candidates(Some("us-east")).await.unwrap();
    assert_eq!(list.len(), 3);
    assert_eq!(*rec.calls.lock().unwrap(), ["fleet_status"]);
    let events: Vec<_> = chain
        .tail(chain.len())
        .into_iter()
        .filter(|e| e.kind == EVENT_KIND_FLEET_INVENTORY)
        .collect();
    assert_eq!(events.len(), 1);
    let p = events[0].payload.clone().unwrap();
    assert_eq!(p["tool"], "fleet_status");
    assert_eq!(p["credential"], "cognitum-oauth");
    assert_eq!(p["candidates"][0]["device_id"], DEVICE);
}

#[tokio::test]
async fn a_write_tool_is_refused_before_it_reaches_the_transport() {
    let rec = Arc::new(Recorder::default());
    let guard = ReadOnlyFleet::new(rec.clone());
    for tool in [
        "device_register",
        "contact_send",
        "leads_create",
        "lead_subscribe",
        "fleet_status ",
        "",
    ] {
        let e = guard.call(tool, json!({})).await.unwrap_err();
        assert!(matches!(e, FleetError::NonReadTool(_)), "{tool:?}: {e}");
    }
    assert!(rec.calls.lock().unwrap().is_empty(), "nothing was sent");
    guard.call("fleet_status", json!({})).await.unwrap();
    assert_eq!(rec.calls.lock().unwrap().len(), 1);
}

async fn local_seed(identity: Value) -> (MockServer, SeedApiRuntime) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(wiremock::matchers::path("/api/v1/identity"))
        .respond_with(ResponseTemplate::new(200).set_body_json(identity))
        .mount(&server)
        .await;
    let rt = SeedApiRuntime::new(
        SeedConfig {
            node_id: "seed-fleet".into(),
            pins: vec![SeedPin::new("fall-detect", "1.0.0")],
            concurrency_cap: SEED_CONCURRENCY_CAP,
        },
        Arc::new(HttpSeedTransport::new(&server.uri(), SeedTls::WebPki).unwrap()),
        Arc::new(MemoryCredentials::with("seed-fleet", SEED_TOKEN)),
    )
    .unwrap();
    (server, rt)
}

fn identity(device: &str, key: &str) -> Value {
    json!({"device_id": device, "public_key": key, "firmware_version": "0.24.2"})
}

#[tokio::test]
async fn the_paired_seed_matches_the_cloud_listing_or_a_key_mismatch_is_refused() {
    let (inv, _, chain) = inventory();
    let list = inv.candidates(None).await.unwrap();

    let (_s, rt) = local_seed(identity(DEVICE, KEY)).await;
    assert_eq!(
        inv.cross_check(&list, &rt).await.unwrap(),
        FleetCheck::Verified
    );

    // Same device id, another key: refused and chained.
    let (_s2, bad) = local_seed(identity(DEVICE, &"00".repeat(32))).await;
    let e = inv.cross_check(&list, &bad).await.unwrap_err();
    assert!(e.to_string().contains("differs"), "{e}");
    let refusals: Vec<_> = chain
        .tail(chain.len())
        .into_iter()
        .filter(|e| e.kind == EVENT_KIND_WORKLOAD_REFUSE)
        .collect();
    assert_eq!(refusals.len(), 1);
    assert_eq!(
        refusals[0].payload.as_ref().unwrap()["code"],
        "key_mismatch"
    );

    // A Seed the cloud does not list, and one listed without a key.
    let (_s3, other) = local_seed(identity("not-in-the-cloud", KEY)).await;
    assert_eq!(
        inv.cross_check(&list, &other).await.unwrap(),
        FleetCheck::NotListed
    );
    let (_s4, nokey) = local_seed(identity("c0ffee00-0000-4000-8000-000000000003", KEY)).await;
    assert_eq!(
        inv.cross_check(&list, &nokey).await.unwrap(),
        FleetCheck::KeyNotReported
    );
}

struct Token;
impl OAuthTokens for Token {
    fn access_token(&self) -> Result<SecretString, FleetError> {
        Ok(SecretString::new(OAUTH.to_string()))
    }
}

/// A stub MCP server: `initialize`, the `initialized` notification and
/// `tools/call` of `fleet_status` (the fixture as a text content block).
struct Mcp(Mutex<Vec<String>>);

impl Respond for Mcp {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        let m = body["method"].as_str().unwrap_or("").to_string();
        self.0.lock().unwrap().push(match m.as_str() {
            "tools/call" => format!(
                "tools/call {}",
                body["params"]["name"].as_str().unwrap_or("")
            ),
            other => other.to_string(),
        });
        match m.as_str() {
            "initialize" => ResponseTemplate::new(200)
                .insert_header("mcp-session-id", "sess-1")
                .set_body_json(json!({"jsonrpc": "2.0", "id": 1, "result": {
                    "protocolVersion": "2025-03-26", "capabilities": {}}})),
            "notifications/initialized" => ResponseTemplate::new(202),
            "tools/call" => {
                assert_eq!(
                    req.headers
                        .get("mcp-session-id")
                        .map(|v| v.to_str().unwrap()),
                    Some("sess-1")
                );
                ResponseTemplate::new(200).set_body_raw(
                    format!(
                        "event: message\ndata: {}\n\n",
                        json!({"jsonrpc": "2.0", "id": 2, "result": {"content": [
                            {"type": "text", "text": FIXTURE}]}})
                    ),
                    "text/event-stream",
                )
            }
            _ => ResponseTemplate::new(400),
        }
    }
}

#[tokio::test]
async fn the_http_transport_speaks_mcp_with_the_bearer_and_only_calls_fleet_status() {
    let server = MockServer::start().await;
    let seen = Arc::new(Mcp(Mutex::new(Vec::new())));
    let log = seen.clone();
    struct Shared(Arc<Mcp>);
    impl Respond for Shared {
        fn respond(&self, r: &Request) -> ResponseTemplate {
            self.0.respond(r)
        }
    }
    Mock::given(method("POST"))
        .and(header("authorization", format!("Bearer {OAUTH}").as_str()))
        .respond_with(Shared(seen))
        .mount(&server)
        .await;
    let t = HttpFleetMcp::new(&server.uri(), Box::new(Token)).unwrap();
    let chain = Arc::new(ChainManager::new(0, 1000));
    let inv = FleetInventory::new(
        ReadOnlyFleet::new(Arc::new(t)),
        chain.clone(),
        "cognitum-oauth",
    );
    let list = inv.candidates(None).await.unwrap();
    assert_eq!(list.len(), 3);
    assert_eq!(
        *log.0.lock().unwrap(),
        [
            "initialize",
            "notifications/initialized",
            "tools/call fleet_status"
        ]
    );
    let export = serde_json::to_string(&chain.tail(chain.len())).unwrap();
    assert!(!export.contains(OAUTH), "OAuth token in the chain export");
}

#[tokio::test]
async fn a_rejected_credential_is_reported_without_the_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let t = HttpFleetMcp::new(&server.uri(), Box::new(Token)).unwrap();
    let e = t.call_tool("fleet_status", json!({})).await.unwrap_err();
    assert_eq!(e, FleetError::Unauthorized(401));
    assert!(!e.to_string().contains(OAUTH));
    // A 500 and a dead port also keep the token out of the message.
    let dead = HttpFleetMcp::new("http://127.0.0.1:1", Box::new(Token)).unwrap();
    let e = dead.call_tool("fleet_status", json!({})).await.unwrap_err();
    assert!(!e.to_string().contains(OAUTH), "{e}");
}

#[test]
fn the_transport_accepts_https_and_exact_loopback_only() {
    for ok in [
        "https://api.cognitum.one/v1/mcp",
        "http://127.0.0.1:8080/mcp",
        "http://localhost/mcp",
        "http://[::1]:9/mcp",
    ] {
        assert!(HttpFleetMcp::new(ok, Box::new(Token)).is_ok(), "{ok}");
    }
    for bad in [
        "http://api.cognitum.one/v1/mcp",
        "http://127.0.0.1.evil.com/mcp",
        "http://localhost.evil.com/mcp",
        "http://127.0.0.1@evil.com/mcp",
        "http://user@127.0.0.1/mcp",
        "ftp://127.0.0.1/mcp",
        "https://",
        "not a url",
    ] {
        assert!(HttpFleetMcp::new(bad, Box::new(Token)).is_err(), "{bad}");
    }
    assert!(ReadOnlyFleet::http("http://127.0.0.1.evil.com/", Box::new(Token)).is_err());
}

#[tokio::test]
async fn an_oversize_reply_is_cut_off_at_the_cap() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(vec![b' '; 5 * 1024 * 1024], "application/json"),
        )
        .mount(&server)
        .await;
    let t = HttpFleetMcp::new(&server.uri(), Box::new(Token)).unwrap();
    let e = t.call_tool("fleet_status", json!({})).await.unwrap_err();
    assert!(e.to_string().contains("too large"), "{e}");
}

#[tokio::test]
async fn the_public_http_guard_refuses_a_write_tool() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let g = ReadOnlyFleet::http(&server.uri(), Box::new(Token)).unwrap();
    let e = g.call("device_register", json!({})).await.unwrap_err();
    assert!(matches!(e, FleetError::NonReadTool(_)));
}
