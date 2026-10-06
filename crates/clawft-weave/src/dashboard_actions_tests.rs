//! Action queue and acknowledgement tests against the fake dashboard (never
//! the real one) and a temp token directory.

use std::sync::Arc;

use serde_json::{Value, json};

use super::*;
use crate::dashboard_report::Beat;
use crate::dashboard_test_support::*;

const HB: &str = "/api/nodes/heartbeat";

fn res(id: &str) -> String {
    format!("/api/nodes/actions/{id}/result")
}

fn answer(actions: Value) -> String {
    json!({ "ok": true, "actions": actions }).to_string()
}

fn action(id: &str, kind: &str) -> Value {
    json!({ "id": id, "kind": kind, "payload": {"project": "p", "secret": "do-not-log-me"}, "created_at": "2026-10-05T10:00:00Z" })
}

fn statuses(fake: &FakeDash, id: &str) -> Vec<Value> {
    fake.requests(&res(id)).iter().map(|r| serde_json::from_str(&r.body).unwrap()).collect()
}

#[tokio::test]
async fn an_answer_without_actions_still_works() {
    let fake = FakeDash::start().await;
    for body in ["{}", r#"{"ok":true}"#, "not json", r#"{"ok":true,"actions":"oops"}"#, r#"{"actions":[{"nope":1}]}"#] {
        fake.answer(HB, &[(200, body)]);
        let (_d, path) = token_dir(&token('a'));
        let dash = dashboard(config(&fake.url, &path));
        assert_eq!(dash.heartbeat_once().await, Beat::Ok, "{body}");
        assert_eq!(dash.actions_json(10)["recorded"], 0);
    }
    assert_eq!(fake.total(), 5, "no result posts without actions");
}

#[tokio::test]
async fn each_known_kind_is_acknowledged_running_then_failed_not_implemented() {
    let fake = FakeDash::start().await;
    let list: Vec<Value> = ["install", "update", "remove", "pair"].iter().enumerate().map(|(i, k)| action(&format!("a{i}"), k)).collect();
    fake.answer(HB, &[(200, &answer(json!(list)))]);
    for i in 0..4 {
        fake.answer(&res(&format!("a{i}")), &[(200, r#"{"ok":true}"#)]);
    }
    let tok = token('a');
    let (_d, path) = token_dir(&tok);
    let dash = dashboard(config(&fake.url, &path));

    assert_eq!(dash.heartbeat_once().await, Beat::Ok);

    for (i, kind) in ["install", "update", "remove", "pair"].iter().enumerate() {
        let id = format!("a{i}");
        let st = statuses(&fake, &id);
        assert_eq!(st.len(), 2, "{id}: {st:?}");
        assert_eq!(st[0], json!({"status": "running"}));
        assert_eq!(st[1]["status"], "failed");
        assert_eq!(st[1]["result"]["error"], "not implemented in this build (ADR-108 P3/P4)");
        assert_eq!(st[1]["result"]["kind"], *kind);
        let seen = fake.requests(&res(&id));
        assert_eq!(seen[0].authorization.as_deref(), Some(format!("Bearer {tok}").as_str()));
    }
    let j = dash.actions_json(10);
    assert_eq!((j["queued"].clone(), j["recorded"].clone()), (json!(0), json!(4)));
    assert!(j["actions"].as_array().unwrap().iter().all(|a| a["status"] == "failed" && a["acked"] == true));
    assert!(!j.to_string().contains("do-not-log-me"), "payloads are not kept");
    let st = dash.status_json();
    assert_eq!((st["actions_recorded"].clone(), st["actions_queued"].clone()), (json!(4), json!(0)));
    assert!(!st.to_string().contains(&tok));
}

#[tokio::test]
async fn an_unknown_kind_fails_with_a_clear_error() {
    let fake = FakeDash::start().await;
    fake.answer(HB, &[(200, &answer(json!([action("x1", "format-disk")])))]);
    fake.answer(&res("x1"), &[(200, "{}")]);
    let (_d, path) = token_dir(&token('a'));
    let dash = dashboard(config(&fake.url, &path));
    dash.heartbeat_once().await;
    let st = statuses(&fake, "x1");
    assert_eq!(st[1]["status"], "failed");
    assert_eq!(st[1]["result"]["error"], "unknown action kind \"format-disk\"");
    assert_eq!(st[1]["result"]["kind"], "format-disk");
}

#[tokio::test]
async fn a_redelivered_action_is_not_run_twice_and_an_unaccepted_result_is_resent() {
    let fake = FakeDash::start().await;
    fake.answer(HB, &[(200, &answer(json!([action("r1", "install")])))]);
    // First: running ok, final refused (500). Then everything accepted.
    fake.answer(&res("r1"), &[(200, "{}"), (500, "{}"), (200, "{}")]);
    let (_d, path) = token_dir(&token('a'));
    let dash = dashboard(config(&fake.url, &path));

    dash.heartbeat_once().await;
    assert_eq!(dash.actions_json(10)["actions"][0]["acked"], false);
    dash.heartbeat_once().await; // redelivered; only the stored final result is re-sent
    let st = statuses(&fake, "r1");
    assert_eq!(st.iter().filter(|s| s["status"] == "running").count(), 1, "{st:?}");
    assert_eq!(st.iter().filter(|s| s["status"] == "failed").count(), 2, "{st:?}");
    assert_eq!(dash.actions_json(10)["actions"][0]["acked"], true);
    assert_eq!(dash.actions_json(10)["recorded"], 1);
}

#[tokio::test]
async fn a_pluggable_handler_replaces_the_placeholder_and_the_queue_is_bounded() {
    struct Ok1;
    #[async_trait::async_trait]
    impl ActionHandler for Ok1 {
        fn kind(&self) -> &str {
            "pair"
        }
        async fn handle(&self, _a: &Action) -> ActionOutcome {
            ActionOutcome { status: "succeeded", result: json!({"paired": true}) }
        }
    }
    let fake = FakeDash::start().await;
    fake.answer(HB, &[(200, &answer(json!([action("p1", "pair")])))]);
    fake.answer(&res("p1"), &[(200, "{}")]);
    let (_d, path) = token_dir(&token('a'));
    let dash = dashboard(config(&fake.url, &path));
    dash.set_action_handler(Arc::new(Ok1));
    dash.heartbeat_once().await;
    assert_eq!(statuses(&fake, "p1")[1], json!({"status": "succeeded", "result": {"paired": true}}));

    let mut book = ActionBook::default();
    let many: Vec<Action> = (0..MAX_QUEUE + 10).map(|i| Action { id: format!("q{i}"), kind: "install".into(), payload: Value::Null, created_at: None }).collect();
    assert_eq!(book.enqueue(many), MAX_QUEUE);
    assert_eq!(book.queued(), MAX_QUEUE);
}

#[test]
fn hostile_ids_and_kinds_are_dropped() {
    let body = answer(json!([
        action("../../etc/passwd", "install"),
        action("has space", "install"),
        action("ok-1", "install"),
        action("", "install"),
        action("k", ""),
    ]));
    let got = ActionBook::parse(&body);
    assert_eq!(got.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["ok-1"]);
}

#[tokio::test]
async fn the_token_never_reaches_the_logs() {
    let cap = LogCapture::default();
    let _g = cap.install();
    let fake = FakeDash::start().await;
    fake.answer(HB, &[(200, &answer(json!([action("l1", "install")])))]);
    fake.answer(&res("l1"), &[(500, "{}")]);
    let tok = token('b');
    let (_d, path) = token_dir(&tok);
    let dash = dashboard(config(&fake.url, &path));
    dash.heartbeat_once().await;
    let text = cap.text();
    assert!(text.contains("did not accept an action result"), "{text}");
    assert!(!text.contains(&tok) && !text.contains("do-not-log-me"), "{text}");
}
