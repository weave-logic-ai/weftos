//! Unit tests for [`crate::parent_link`]: the fail-closed behaviour with no
//! parent. The live-parent paths are in `tests/shared_services.rs`.

use super::*;
use serde_json::json;
use clawft_core::embeddings::Embedder;
use clawft_service_llm::{ChatMessage, LlmClient, LlmConfig};

const ID: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";

fn down_link(dir: &Path) -> Arc<ParentLink> {
    Arc::new(
        ParentLink::new(dir.join("no-such.sock"), ID.into(), "wft_secret".into()).with_backoff(
            Backoff {
                attempts: 1,
                initial: Duration::from_millis(1),
                max: Duration::from_millis(1),
            },
        ),
    )
}

#[test]
fn spawn_json_is_read_and_the_token_is_never_debug_printed() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run").join(ID);
    std::fs::create_dir_all(&run).unwrap();
    let path = run.join("spawn.json");
    std::fs::write(
        &path,
        json!({"project_id": ID, "project_token": "wft_supersecret", "spawn_nonce": "n"}).to_string(),
    )
    .unwrap();
    let link = ParentLink::from_spawn_json(&path);
    assert_eq!(link.project_id(), Some(ID));
    // The parent socket defaults to `<run root>/kernel.sock`.
    assert_eq!(link.socket, dir.path().join("run").join("kernel.sock"));
    assert!(!format!("{link:?}").contains("supersecret"));
}

#[tokio::test]
async fn missing_or_token_less_spawn_json_fails_every_call_closed() {
    let dir = tempfile::tempdir().unwrap();
    let missing = ParentLink::from_spawn_json(&dir.path().join("spawn.json"));
    let r = missing.call(Service::Embeddings, "shared.embed", json!({"texts": []})).await;
    assert_eq!(r.unwrap_err().kind(), PARENT_UNAVAILABLE_KIND);

    let path = dir.path().join("spawn.json");
    std::fs::write(&path, json!({"project_id": ID}).to_string()).unwrap();
    let r = ParentLink::from_spawn_json(&path)
        .call(Service::Llm, "shared.llm.models", Value::Null)
        .await;
    assert_eq!(r.unwrap_err().kind(), PARENT_UNAVAILABLE_KIND);
}

#[tokio::test]
async fn a_stopped_parent_is_a_typed_error_and_health_says_down() {
    let dir = tempfile::tempdir().unwrap();
    let link = down_link(dir.path());
    let err = link
        .call(Service::Embeddings, "shared.embed", json!({"texts": ["x"]}))
        .await
        .unwrap_err();
    assert!(matches!(err, ParentError::Unavailable(_)));
    assert_eq!(err.kind(), "parent_unavailable");
    let h = link.health();
    assert_eq!((h.embeddings.as_str(), h.llm.as_str(), h.voice.as_str()), ("down", "down", "down"));
    assert!(!link.probe().await);
}

#[tokio::test]
async fn remote_embedder_never_returns_a_local_vector() {
    let dir = tempfile::tempdir().unwrap();
    let e = RemoteEmbedder::new(down_link(dir.path()));
    let err = Embedder::embed(&e, "hello").await.unwrap_err().to_string();
    assert!(err.contains("parent_unavailable"), "{err}");
    let err = Embedder::embed_batch(&e, &["a".into(), "b".into()])
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("parent_unavailable"), "{err}");
    assert_eq!(Embedder::dimension(&e), 0, "no guessed width");
    assert!(!e.is_ready());
}

#[tokio::test]
async fn parent_llm_client_fails_closed_without_http() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(ParentLlmBackend::new(down_link(dir.path())));
    // An unroutable base_url proves nothing was sent over HTTP.
    let client = LlmClient::with_backend(
        LlmConfig {
            base_url: "http://203.0.113.1:9".into(),
            api_key: Some("sk-must-not-be-used".into()),
            request_timeout: Duration::from_secs(2),
            ..LlmConfig::default()
        },
        backend,
    )
    .unwrap();
    let err = client
        .complete(vec![ChatMessage::user("hi")], None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("parent_unavailable"), "{err}");
    assert!(client.list_models().await.is_err());
    assert!(!client.health().await.unwrap());
}
