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

mod refresh {
    use super::*;
    use clawft_rpc::{Request, Response};
    use std::sync::Mutex as StdMutex;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    type Seen = Arc<StdMutex<Vec<(String, Option<String>)>>>;

    /// A parent that records `(method, auth)` and answers refresh with
    /// `wft_new` (or refuses when `refuse`).
    fn parent(sock: &Path, refuse: bool) -> Seen {
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let l = UnixListener::bind(sock).unwrap();
        let seen2 = seen.clone();
        tokio::spawn(async move {
            loop {
                let (s, _) = l.accept().await.unwrap();
                let seen = seen2.clone();
                tokio::spawn(async move {
                    let (r, mut w) = s.into_split();
                    let mut lines = BufReader::new(r).lines();
                    while let Some(line) = lines.next_line().await.unwrap() {
                        let req: Request = serde_json::from_str(&line).unwrap();
                        seen.lock().unwrap().push((req.method.clone(), req.auth.clone()));
                        let resp = match req.method.as_str() {
                            "project.token.refresh" if refuse => {
                                Response::error_with_kind("token_refresh_refused", "no")
                            }
                            "project.token.refresh" => Response::success(
                                json!({"token": "wft_new", "expires_at": "2099-01-01T00:00:00Z"}),
                            ),
                            _ => Response::success(json!({})),
                        };
                        let mut out = serde_json::to_string(&resp).unwrap();
                        out.push('\n');
                        w.write_all(out.as_bytes()).await.unwrap();
                    }
                });
            }
        });
        seen
    }

    fn link(sock: &Path, expires_in: u64) -> ParentLink {
        ParentLink::new(sock.to_path_buf(), ID.into(), "wft_old".into())
            .with_token_expiry(now_unix() + expires_in)
    }

    #[tokio::test]
    async fn a_token_near_expiry_is_renewed_before_the_call_and_the_new_one_is_used() {
        let d = tempfile::Builder::new().prefix("plink").tempdir_in("/tmp").unwrap();
        let sock = d.path().join("kernel.sock");
        let seen = parent(&sock, false);
        let l = link(&sock, 10);
        l.call(Service::Voice, "kernel.handshake", Value::Null).await.unwrap();
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen[0].0, "project.token.refresh");
        assert_eq!(seen[0].1.as_deref(), Some("wft_old"), "refresh is authenticated by the old token");
        assert_eq!(seen[1], ("kernel.handshake".into(), Some("wft_new".into())));
        // Fresh now: the next call does not refresh again.
        l.call(Service::Voice, "kernel.handshake", Value::Null).await.unwrap();
        assert_eq!(seen.len(), 2);
    }

    #[tokio::test]
    async fn a_fresh_token_is_not_renewed() {
        let d = tempfile::Builder::new().prefix("plink").tempdir_in("/tmp").unwrap();
        let sock = d.path().join("kernel.sock");
        let seen = parent(&sock, false);
        let l = link(&sock, 3000);
        l.call(Service::Voice, "kernel.handshake", Value::Null).await.unwrap();
        assert!(seen.lock().unwrap().iter().all(|(m, _)| m != "project.token.refresh"));
    }

    #[tokio::test]
    async fn a_refused_refresh_keeps_the_old_token_and_does_not_panic() {
        let d = tempfile::Builder::new().prefix("plink").tempdir_in("/tmp").unwrap();
        let sock = d.path().join("kernel.sock");
        let seen = parent(&sock, true);
        let l = link(&sock, 10);
        assert!(matches!(
            l.refresh_token().await,
            Err(ParentError::Refused { ref kind, .. }) if kind == "token_refresh_refused"
        ));
        l.call(Service::Voice, "kernel.handshake", Value::Null).await.unwrap();
        l.call(Service::Voice, "kernel.handshake", Value::Null).await.unwrap();
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.last().unwrap().1.as_deref(), Some("wft_old"));
        // A refusing parent is not hammered: one refresh attempt, not three.
        assert_eq!(seen.iter().filter(|(m, _)| m == "project.token.refresh").count(), 2,
            "the explicit refresh above plus at most one throttled automatic attempt");
    }

    #[test]
    fn a_link_from_spawn_tracks_the_token_expiry() {
        let l = ParentLink::new_from_spawn("/h/.weftos/run/kernel.sock".into(), ID.into(), "wft_t".into());
        let st = l.token.lock().unwrap();
        let left = st.expires_unix.unwrap() - now_unix();
        assert!(left <= PROJECT_TOKEN_TTL_SECS && left > PROJECT_TOKEN_TTL_SECS - 2 * SPAWN_TTL_SECS);
        assert_eq!(st.secret.as_deref(), Some("wft_t"));
    }
}
