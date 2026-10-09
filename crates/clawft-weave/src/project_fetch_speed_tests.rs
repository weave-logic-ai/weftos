//! Pipelined chunk transfer: revocation mid-pipeline, the plain-channel
//! fallback, and a benchmark over the in-process signed wire
//! (`WEFTOS_FETCH_BENCH_MB` sets the non-git size; default 8).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use async_trait::async_trait;
use serde_json::{Value, json};

use clawft_kernel::mesh::{MeshError, MeshStream};
use clawft_kernel::workload_ctl::transport::CtlConnector;
use clawft_kernel::workload_ctl::{MeshConnector, PlacementControlPlane};
use clawft_kernel::workload_pkg::TrustAnchors;
use clawft_kernel::node_id_from_pubkey;
use clawft_types::placement::TrustTier;

use super::*;
use crate::project_fetch_mesh::MeshFetcher;
use crate::project_fetch_mesh::tests::{ULID, commit, exchange, g, gate, rig, w};
use crate::project_install::{InstallRequest, PrimaryRef, ProjectFetcher};

const OTHER: &str = "01K6ZQ8N3T4V5W6X7Y8Z9A0B1D";

/// The in-process wire has no latency; this adds one round-trip time to every
/// answer the member reads, so round trips cost what they cost on a tailnet.
struct Latency {
    inner: Arc<MeshConnector>,
    rtt: std::time::Duration,
}

struct Delayed {
    inner: Box<dyn MeshStream>,
    rtt: std::time::Duration,
}

#[async_trait]
impl MeshStream for Delayed {
    async fn send(&mut self, data: &[u8]) -> Result<(), MeshError> {
        self.inner.send(data).await
    }
    async fn recv(&mut self) -> Result<Vec<u8>, MeshError> {
        let m = self.inner.recv().await?;
        tokio::time::sleep(self.rtt).await;
        Ok(m)
    }
    async fn close(&mut self) -> Result<(), MeshError> {
        self.inner.close().await
    }
    fn remote_addr(&self) -> Option<std::net::SocketAddr> {
        None
    }
}

#[async_trait]
impl CtlConnector for Latency {
    async fn connect(&self, addr: &str) -> Result<Box<dyn MeshStream>, MeshError> {
        Ok(Box::new(Delayed { inner: self.inner.connect(addr).await?, rtt: self.rtt }))
    }
}

/// A member plane whose connections carry `rtt` per answer.
async fn plane_with_latency(r: &crate::project_fetch_mesh::tests::Rig, rtt: std::time::Duration) -> Arc<PlacementControlPlane> {
    let id = node_id_from_pubkey(&r.member_key.verifying_key().to_bytes());
    let conn: Arc<dyn CtlConnector> = Arc::new(Latency { inner: r.conn.clone(), rtt });
    let plane = PlacementControlPlane::new(r.member_key.clone(), gate(&r.member_chain), r.member_chain.clone(), exchange(&id, &r.member_chain), TrustAnchors::default(), conn);
    plane.add_target(&r.addr, TrustTier::Pinned).await.unwrap();
    Arc::new(plane)
}

/// Bench lines go to stderr and, when `WEFTOS_FETCH_BENCH_OUT` names a file,
/// are appended there too (the test runner hides a passing test's output).
fn report(line: String) {
    eprintln!("{line}");
    if let Some(p) = std::env::var_os("WEFTOS_FETCH_BENCH_OUT") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// Pseudo-random bytes (tar does not compress, but keep it honest).
fn blob(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

/// `mb` MiB of ignored data under `root/data/` and `commits` commits of history.
fn grow(root: &Path, mb: u64, commits: usize) {
    for i in 0..mb {
        std::fs::write(root.join("data").join(format!("blob-{i:03}.bin")), blob(i, 1024 * 1024)).unwrap();
    }
    for i in 0..commits {
        w(&root.join("src").join(format!("f{}.txt", i % 10)), &format!("{}\n", blob(i as u64, 2048).iter().map(|b| (b'a' + b % 26) as char).collect::<String>()));
        commit(root, &format!("c{i}"));
    }
}

/// Withdraws this project's grant (another project's stays, so the key still
/// reaches the hook) after `after` chunk calls have gone out.
struct Revoking {
    inner: Arc<PlaneChannel>,
    shared: Arc<(AtomicU64, u64, std::path::PathBuf, String)>,
}

struct RevokingLink {
    inner: Box<dyn FetchLink>,
    shared: Arc<(AtomicU64, u64, std::path::PathBuf, String)>,
}

#[async_trait]
impl FetchLink for RevokingLink {
    async fn call(&mut self, body: Value) -> Result<Value, String> {
        let (count, after, rt, member) = &*self.shared;
        if body["op"] == "chunk" && count.fetch_add(1, Ordering::SeqCst) == *after {
            w(&rt.join(crate::project_fetch_grants::FETCH_FILE), &format!("{{\"version\":1,\"grants\":[{{\"peer_node\":\"{member}\",\"projects\":[\"{OTHER}\"]}}]}}"));
        }
        self.inner.call(body).await
    }
}

#[async_trait]
impl FetchChannel for Revoking {
    async fn call(&self, body: Value) -> Result<Value, String> {
        self.inner.call(body).await
    }
    async fn links(&self, n: usize) -> Result<Vec<Box<dyn FetchLink>>, String> {
        Ok(self.inner.links(n).await?.into_iter().map(|l| Box::new(RevokingLink { inner: l, shared: self.shared.clone() }) as Box<dyn FetchLink>).collect())
    }
    fn tuning(&self) -> FetchTuning {
        self.inner.tuning()
    }
}

#[tokio::test]
async fn a_grant_withdrawn_mid_pipeline_stops_the_transfer_and_nothing_is_unpacked() {
    let r = rig().await;
    r.peers("pinned");
    r.grant(&[ULID]);
    grow(&r.root, 4, 1);
    let small = FetchTuning { chunk_bytes: 64 * 1024, window: 6, reuse: true };
    let inner = Arc::new(PlaneChannel::new(r.plane().await.unwrap(), r.host_id.clone()).with_tuning(small));
    let member = clawft_kernel::node_id_from_pubkey(&r.member_key.verifying_key().to_bytes());
    let ch = Revoking { inner, shared: Arc::new((AtomicU64::new(0), 9, r.rt.clone(), member)) };
    let dest = tempfile::tempdir().unwrap();
    let e = fetch_tar(&ch, ULID, dest.path()).await.unwrap_err();
    assert!(e.contains("no fetch grant") || e.contains("no such transfer"), "{e}");
    assert!(std::fs::read_dir(dest.path()).unwrap().next().is_none(), "nothing unpacked");
    let refused = r.host_chain.tail(r.host_chain.len()).into_iter().filter(|ev| ev.kind == "project.fetch").filter_map(|ev| ev.payload).filter(|p| p["ok"] == false && p["op"] == "chunk").count();
    assert!(refused >= 1, "the mid-transfer refusal is chained");
    // The host dropped the spool session.
    assert_eq!(r.fetch_host.open_sessions(), 0);
}

#[tokio::test]
async fn the_per_call_path_still_works_and_small_chunks_are_honoured() {
    let r = rig().await;
    r.peers("paired");
    r.grant(&[ULID]);
    grow(&r.root, 1, 1);
    let old = FetchTuning { chunk_bytes: 128 * 1024, window: 1, reuse: false };
    let ch = PlaneChannel::new(r.plane().await.unwrap(), r.host_id.clone()).with_tuning(old);
    let dest = tempfile::tempdir().unwrap();
    let t = fetch_tar(&ch, ULID, dest.path()).await.unwrap().unwrap();
    assert!(t.round_trips > 8, "{} chunks of 128 KiB plus open and close", t.round_trips);
    assert_eq!(std::fs::read(dest.path().join("data/blob-000.bin")).unwrap(), blob(0, 1024 * 1024));
    // A request below the floor is clamped up, above the cap clamped down.
    let v = ch.call(json!({"op": "tar.open", "project": ULID, "chunk_bytes": 1})).await.unwrap();
    assert_eq!(v["chunk_bytes"], crate::project_fetch_serve::MIN_CHUNK_BYTES);
    ch.call(json!({"op": "close", "session": v["session"]})).await.unwrap();
    let v = ch.call(json!({"op": "tar.open", "project": ULID, "chunk_bytes": u64::MAX})).await.unwrap();
    assert_eq!(v["chunk_bytes"], MAX_CHUNK_BYTES);
    ch.call(json!({"op": "close", "session": v["session"]})).await.unwrap();
}

#[tokio::test]
async fn bench_clone_of_a_large_tree_before_and_after() {
    let mb: u64 = std::env::var("WEFTOS_FETCH_BENCH_MB").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
    let r = rig().await;
    r.peers("pinned");
    r.grant(&[ULID]);
    grow(&r.root, mb, 40);
    let before = FetchTuning { chunk_bytes: 128 * 1024, window: 1, reuse: false };
    let after = FetchTuning::default();
    let rtt_ms: u64 = std::env::var("WEFTOS_FETCH_BENCH_RTT_MS").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    for (label, tuning) in [("before", before), ("after", after)] {
        let plane = if rtt_ms == 0 { r.plane().await.unwrap() } else { plane_with_latency(&r, std::time::Duration::from_millis(rtt_ms)).await };
        let ch = PlaneChannel::new(plane, r.host_id.clone()).with_tuning(tuning);
        let dest = tempfile::tempdir().unwrap();
        let t0 = Instant::now();
        let tar = fetch_tar(&ch, ULID, dest.path()).await.unwrap().unwrap();
        let t_tar = t0.elapsed();
        let repo = dest.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        g(&repo, &["init", "-q"]);
        let tmp = repo.join("b.bundle");
        let t1 = Instant::now();
        let b = download(&ch, json!({"op": "bundle.open", "project": ULID, "dir": ".", "want": ["refs/heads/main"]}), &tmp).await.unwrap().unwrap();
        let t_bundle = t1.elapsed();
        let mbs = |bytes: u64, d: std::time::Duration| bytes as f64 / (1024.0 * 1024.0) / d.as_secs_f64().max(1e-6);
        report(format!(
            "[bench {label} rtt {rtt_ms} ms] chunk {} KiB, window {}, reuse {}: tar {:.1} MiB in {:.2}s = {:.1} MiB/s over {} round trips; bundle {:.2} MiB in {:.2}s = {:.1} MiB/s over {} round trips",
            tuning.chunk_bytes >> 10, tuning.window, tuning.reuse,
            tar.bytes as f64 / 1048576.0, t_tar.as_secs_f64(), mbs(tar.bytes, t_tar), tar.round_trips,
            b.bytes as f64 / 1048576.0, t_bundle.as_secs_f64(), mbs(b.bytes, t_bundle), b.round_trips,
        ));
        // The blobs plus the rig's own `data/set.csv`.
        assert_eq!(tar.unpacked.files as u64, mb + 1);
        assert_eq!(std::fs::read(dest.path().join(format!("data/blob-{:03}.bin", mb - 1))).unwrap(), blob(mb - 1, 1024 * 1024));
    }
    // And the whole install path with the default tuning.
    let plane = r.plane().await.unwrap();
    let f = MeshFetcher::with_plane(plane, None);
    let dest = r.dest();
    let req = InstallRequest { project_ulid: ULID.into(), target_path: Some(dest.to_string_lossy().into_owned()), slug: None, sources: vec![], primary: Some(PrimaryRef { node_id: r.host_id.clone() }) };
    let t0 = Instant::now();
    let rep = f.fetch(&req, &dest).await.unwrap();
    report(format!("[bench install] {} repos, {:.1} MiB non-git in {:.2}s", rep.repos.len(), rep.bytes as f64 / 1048576.0, t0.elapsed().as_secs_f64()));
    assert!(dest.join(format!("data/blob-{:03}.bin", mb - 1)).exists());
}
