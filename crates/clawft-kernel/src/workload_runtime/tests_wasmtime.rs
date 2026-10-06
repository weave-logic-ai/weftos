//! Wasmtime project adapter: distinct identity through load/start/stop, and
//! refusal of the logical adapter's payload.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;

use super::logical::{ChildIdentity, ChildLauncher, ChildProbe, ChildRef, ChildSpec, LogicalRuntime};
use super::types::{
    InstanceState, ProjectPayload, RunMode, RuntimeError, VerifiedWorkload, WorkloadConfig, WorkloadRuntime,
    WorkloadSource,
};
use super::HostContract;

#[derive(Default)]
struct Runner {
    live: AtomicBool,
}

#[async_trait]
impl ChildLauncher for Runner {
    async fn spawn(&self, spec: &ChildSpec) -> Result<ChildRef, RuntimeError> {
        assert_eq!(spec.adapter, "wasmtime-project-v1");
        self.live.store(true, Ordering::SeqCst);
        Ok(ChildRef { project_id: spec.project_id.clone(), identity: ChildIdentity::Native { host_pid: 4242 } })
    }
    async fn terminate(&self, _: &ChildRef, _: Duration) -> Result<Option<i32>, RuntimeError> {
        self.live.store(false, Ordering::SeqCst);
        Ok(Some(0))
    }
    async fn probe(&self, _: &str) -> ChildProbe {
        if self.live.load(Ordering::SeqCst) {
            ChildProbe::Running { identity: ChildIdentity::Native { host_pid: 4242 } }
        } else {
            ChildProbe::NotStarted
        }
    }
}

fn workload(adapter: &str) -> VerifiedWorkload {
    VerifiedWorkload {
        kind: "project".into(),
        id: "p".into(),
        version: "cert-1".into(),
        source: WorkloadSource::Project(ProjectPayload {
            adapter: adapter.into(),
            project_id: "p".into(),
            key_id: "k".into(),
            cert_serial: 1,
            user_key_id: "u".into(),
            policy_hash: String::new(),
            root: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        }),
    }
}

#[tokio::test]
async fn wasmtime_identity_survives_load_start_stop_and_rejects_logical_admission() {
    let launcher = Arc::new(Runner::default());
    let wasm = LogicalRuntime::wasmtime_project(launcher.clone());
    let logical = LogicalRuntime::new(launcher);
    let w = workload("wasmtime-project-v1");
    assert!(logical.admit(&w).await.is_err());
    assert!(wasm.admit(&workload("logical")).await.is_err());
    assert_eq!(wasm.admit(&w).await.unwrap().arch, "wasm32-wasip1");
    let cfg = WorkloadConfig {
        mode: RunMode::Listener,
        args: vec![],
        host: HostContract::default_feed(),
        node_id: "p".into(),
    };
    let h = wasm.load(&w, &cfg).await.unwrap();
    assert_eq!(h.runtime, "wasmtime-project-v1");
    wasm.start(&h).await.unwrap();
    assert_eq!(wasm.status(&h).await.state, InstanceState::Running);
    assert!(wasm.start(&h).await.is_err());
    let evidence = wasm.stop(&h, Duration::ZERO).await.unwrap();
    assert_eq!(evidence.runtime, "wasmtime-project-v1");
    assert_eq!(evidence.exit_code, Some(0));
    wasm.unload(h).await.unwrap();
}
