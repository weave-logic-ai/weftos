//! Integration test — a mock adapter emits canned deltas and we assert
//! the substrate snapshot matches.
//!
//! ADR-017 §9 — every shipped adapter has a mock/stub so surfaces
//! cannot tell it apart. This test validates the mock pattern itself
//! and the substrate-plumbing that will be reused by the real kernel
//! adapter once the daemon is running.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use clawft_substrate::{
    AdapterError, BufferPolicy, OntologyAdapter, PermissionReq, RefreshHint, Sensitivity,
    StateDelta, SubId, Subscription, Substrate, TopicDecl,
};

const MOCK_TOPICS: &[TopicDecl] = &[TopicDecl {
    path: "substrate/mock/items",
    shape: "ontology://mock-item-list",
    refresh_hint: RefreshHint::Periodic { ms: 100 },
    sensitivity: Sensitivity::Public,
    buffer_policy: BufferPolicy::BlockCapped,
    max_len: None,
}];

struct MockAdapter {
    canned: Vec<StateDelta>,
}

#[async_trait]
impl OntologyAdapter for MockAdapter {
    fn id(&self) -> &'static str {
        "mock"
    }
    fn topics(&self) -> &'static [TopicDecl] {
        MOCK_TOPICS
    }
    fn permissions(&self) -> &'static [PermissionReq] {
        &[]
    }
    async fn open(&self, topic: &str, _args: Value) -> Result<Subscription, AdapterError> {
        if topic != "substrate/mock/items" {
            return Err(AdapterError::UnknownTopic(topic.into()));
        }
        let (tx, rx) = mpsc::channel(16);
        let deltas = self.canned.clone();
        tokio::spawn(async move {
            for d in deltas {
                let _ = tx.send(d).await;
            }
            // tx dropped at end of scope → substrate task exits.
        });
        Ok(Subscription { id: SubId(42), rx })
    }
    async fn close(&self, _sub_id: SubId) -> Result<(), AdapterError> {
        Ok(())
    }
}

#[tokio::test]
async fn mock_adapter_deltas_land_in_substrate() {
    let adapter = Arc::new(MockAdapter {
        canned: vec![
            StateDelta::Replace {
                path: "substrate/mock/items/by-id/a".into(),
                value: json!({ "name": "alpha" }),
            },
            StateDelta::Replace {
                path: "substrate/mock/items/by-id/b".into(),
                value: json!({ "name": "beta" }),
            },
            StateDelta::Append {
                path: "substrate/mock/items/log".into(),
                value: json!("hello"),
            },
            StateDelta::Append {
                path: "substrate/mock/items/log".into(),
                value: json!("world"),
            },
            StateDelta::Remove {
                path: "substrate/mock/items/by-id/a".into(),
            },
        ],
    });

    let substrate = Arc::new(Substrate::new());
    substrate
        .subscribe_adapter(
            adapter.clone() as Arc<dyn OntologyAdapter>,
            "substrate/mock/items",
            Value::Null,
        )
        .await
        .expect("subscribe");

    // Wait for all five deltas to be drained. The mock sender exits
    // when it finishes, so we poll the snapshot until `log` length is 2
    // and `by-id/a` is absent.
    for _ in 0..50 {
        let snap = substrate.snapshot();
        let log_len = snap
            .get("substrate/mock/items/log")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let a_removed = snap.get("substrate/mock/items/by-id/a").is_none();
        if log_len == 2 && a_removed {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let snap = substrate.snapshot();
    assert_eq!(
        snap.get("substrate/mock/items/by-id/b"),
        Some(&json!({ "name": "beta" })),
        "beta should have landed"
    );
    assert!(
        snap.get("substrate/mock/items/by-id/a").is_none(),
        "alpha should have been removed"
    );
    let log = snap.get("substrate/mock/items/log").expect("log present");
    assert_eq!(log.as_array().map(|a| a.len()), Some(2));
    assert_eq!(log[0], json!("hello"));
    assert_eq!(log[1], json!("world"));
}

#[tokio::test]
async fn mock_adapter_unknown_topic_errors() {
    let adapter = Arc::new(MockAdapter { canned: vec![] });
    let substrate = Arc::new(Substrate::new());
    let result = substrate
        .subscribe_adapter(
            adapter as Arc<dyn OntologyAdapter>,
            "substrate/mock/not-a-topic",
            Value::Null,
        )
        .await;
    match result {
        Err(AdapterError::UnknownTopic(t)) => {
            assert_eq!(t, "substrate/mock/not-a-topic");
        }
        other => panic!("expected UnknownTopic, got {other:?}"),
    }
}
