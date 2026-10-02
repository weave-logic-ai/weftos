//! Process state behind `shared.*` (ADR-103 Phase 2 F): the usage meter,
//! cached project limits, concurrency permits, the model allow-list cache and
//! the embedder/LLM sources. Kept apart from the handlers in `shared_rpc`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use clawft_kernel::embedding::EmbeddingProvider;
use clawft_service_llm::{LlmClient, SharedLlmClient};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::shared_meter::{SharedLimits, UsageMeter};

/// In-flight `shared.*` calls allowed per project.
pub const PER_PROJECT_CONCURRENT: usize = 2;
/// In-flight `shared.*` calls allowed in the whole daemon.
pub const GLOBAL_CONCURRENT: usize = 4;
/// Wall-clock cap on one `shared.llm.chat`: it bounds how long the user's own
/// next turn can wait behind it on the single model slot.
pub const LLM_CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// Wall-clock cap on one `shared.embed`.
pub const EMBED_CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the parent's model list is trusted.
const MODELS_TTL: Duration = Duration::from_secs(60);

static METER: OnceLock<UsageMeter> = OnceLock::new();
static LIMITS: Mutex<Option<HashMap<String, SharedLimits>>> = Mutex::new(None);
static GLOBAL: OnceLock<Arc<Semaphore>> = OnceLock::new();
static PER_PROJECT: Mutex<Option<HashMap<String, Arc<Semaphore>>>> = Mutex::new(None);
static MODELS: Mutex<Option<(Instant, Vec<String>)>> = Mutex::new(None);
static EMBEDDER: tokio::sync::OnceCell<Arc<dyn EmbeddingProvider>> =
    tokio::sync::OnceCell::const_new();
static LLM: RwLock<Option<SharedLlmClient>> = RwLock::new(None);

/// The process usage meter.
pub fn meter() -> &'static UsageMeter {
    METER.get_or_init(UsageMeter::default)
}

/// The project's limits: loaded once with `load`, then served from memory
/// until [`reload`] or a restart.
pub fn cached_limits<E>(
    project: &str,
    load: impl FnOnce() -> Result<SharedLimits, E>,
) -> Result<SharedLimits, E> {
    let mut g = LIMITS.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    if let Some(l) = map.get(project) {
        return Ok(*l);
    }
    let l = load()?;
    map.insert(project.to_owned(), l);
    Ok(l)
}

/// Drop every cached limit (operator `shared.reload`).
pub fn reload() {
    *LIMITS.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Held for the duration of one call.
pub struct Permits {
    _global: OwnedSemaphorePermit,
    _project: OwnedSemaphorePermit,
}

/// Take a per-project and a global slot without waiting; `None` when either
/// is full.
pub fn try_acquire(project: &str) -> Option<Permits> {
    let global = GLOBAL.get_or_init(|| Arc::new(Semaphore::new(GLOBAL_CONCURRENT)));
    let per = {
        let mut g = PER_PROJECT.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(
            g.get_or_insert_with(HashMap::new)
                .entry(project.to_owned())
                .or_insert_with(|| Arc::new(Semaphore::new(PER_PROJECT_CONCURRENT))),
        )
    };
    let project_permit = per.try_acquire_owned().ok()?;
    let global_permit = Arc::clone(global).try_acquire_owned().ok()?;
    Some(Permits { _global: global_permit, _project: project_permit })
}

/// `model` if the parent lists it, else `None` (use the parent default). A
/// failed listing also gives `None`: never trust the child's choice blindly.
pub async fn allowed_model(client: &LlmClient, requested: Option<&str>) -> Option<String> {
    let want = requested.map(str::trim).filter(|m| !m.is_empty())?;
    let cached = {
        let g = MODELS.lock().unwrap_or_else(|e| e.into_inner());
        g.as_ref()
            .filter(|(at, _)| at.elapsed() < MODELS_TTL)
            .map(|(_, l)| l.clone())
    };
    let list = match cached {
        Some(l) => l,
        None => {
            let l = client.list_models().await.ok()?;
            *MODELS.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), l.clone()));
            l
        }
    };
    list.iter().any(|m| m == want).then(|| want.to_owned())
}

/// The LLM `shared.llm.*` serves: an installed override, else the daemon's.
pub fn llm_client() -> Option<SharedLlmClient> {
    LLM.read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .or_else(crate::daemon::daemon_llm)
}

/// The embedder `shared.embed` serves, loaded once (the model load runs off
/// the async threads and concurrent first calls share one load).
pub async fn embedder() -> Arc<dyn EmbeddingProvider> {
    EMBEDDER
        .get_or_init(|| async {
            tokio::task::spawn_blocking(|| {
                Arc::from(clawft_kernel::embedding::select_embedding_provider(None))
            })
            .await
            .expect("embedder selection panicked")
        })
        .await
        .clone()
}

/// Serve `shared.llm.*` from `client` (tests and the `test-support` feature).
#[cfg(any(test, feature = "test-support"))]
pub fn install_llm(client: SharedLlmClient) {
    *LLM.write().unwrap_or_else(|e| e.into_inner()) = Some(client);
}

/// Serve `shared.embed` from `embedder`; the first install wins.
#[cfg(any(test, feature = "test-support"))]
pub fn install_embedder(embedder: Arc<dyn EmbeddingProvider>) {
    let _ = EMBEDDER.set(embedder);
}
