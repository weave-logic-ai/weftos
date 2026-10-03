//! The owner's project stores survive a restart (ADR-100 decision 5).
//! Temp dirs only; nothing touches `~/.weftos` or `~/.clawft`.

use std::io::Write;
use std::sync::Arc;

use super::store_log::{LogRecord, VectorLog};
use super::*;

const A: &str = "01J9ZXW0PRJCTAAAAAAAAAAAAA";
const B: &str = "01J9ZXW0PRJCTBBBBBBBBBBBBB";

fn from(inst: &str) -> Provenance {
    Provenance { instance_id: inst.into(), source_node: "node-a".into() }
}

/// Distinct directions for ids 0..8, so a nearest-neighbour query has one answer.
fn vec_of(i: u64) -> [f32; DIMS] {
    let mut v = [0.0; DIMS];
    v[i as usize % DIMS] = 1.0;
    v[(i as usize + 3) % DIMS] += 0.5;
    v
}

fn vecs(range: std::ops::Range<u64>) -> Vec<IngestVector> {
    range.map(|i| IngestVector { id: i, values: vec_of(i) }).collect()
}

fn rec(id: u64) -> LogRecord {
    LogRecord { instance: "inst".into(), node: "n".into(), id, values: [id as f32; DIMS] }
}

fn dir_over(path: &std::path::Path) -> VectorDirectory {
    VectorDirectory::new([A.to_string(), B.to_string()], true).with_persistence(path.to_path_buf())
}

#[test]
fn a_restarted_owner_has_the_vectors_it_had_and_keeps_dedup_and_namespaces() {
    let tmp = tempfile::tempdir().unwrap();
    {
        let d = dir_over(tmp.path());
        let a = d.store_for(Some(A)).unwrap();
        assert_eq!(a.ingest(&from("inst-1"), &vecs(0..5), true).unwrap().accepted, 5);
        d.store_for(Some(B)).unwrap().ingest(&from("inst-2"), &vecs(0..2), true).unwrap();
        d.store_for(None).unwrap().ingest(&from("inst-3"), &vecs(0..1), true).unwrap();
    }
    // New process: a fresh directory over the same dir.
    let d = dir_over(tmp.path());
    let a = d.store_for(Some(A)).unwrap();
    assert_eq!(a.len(), 5);
    assert_eq!(d.store_for(Some(B)).unwrap().len(), 2);
    assert_eq!(d.store_for(None).unwrap().len(), 1);
    let hit = &a.query(&vec_of(3), 1)[0];
    assert_eq!((hit.instance_id.as_str(), hit.id), ("inst-1", 3));
    // Dedup state came back: re-posting the same batch adds nothing.
    let again = a.ingest(&from("inst-1"), &vecs(0..5), true).unwrap();
    assert_eq!((again.accepted, again.deduped, again.total), (0, 5, 5));
    // Another instance's ids are still its own namespace.
    assert_eq!(a.ingest(&from("inst-9"), &vecs(0..2), true).unwrap().accepted, 2);
    // And what is new after the restart survives the next one.
    drop(d);
    let d = dir_over(tmp.path());
    assert_eq!(d.store_for(Some(A)).unwrap().len(), 7);
}

#[test]
fn projects_are_separate_files_and_unowned_projects_get_no_store() {
    let tmp = tempfile::tempdir().unwrap();
    let d = dir_over(tmp.path());
    d.store_for(Some(A)).unwrap().ingest(&from("i"), &vecs(0..1), true).unwrap();
    assert!(tmp.path().join(format!("{A}.vec")).is_file());
    assert!(!tmp.path().join(format!("{B}.vec")).exists(), "created on first use only");
    assert!(d.store_for(Some("01J9ZXW0PRJCTCCCCCCCCCCCCC")).is_none());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(tmp.path().join(format!("{A}.vec"))).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "owner-only");
    }
}

#[test]
fn a_full_log_refuses_the_batch_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    // Room for the magic plus a little over one single-vector frame.
    let one = super::store_log::frame_len(&[LogRecord {
        instance: "inst-1".into(),
        node: "node-a".into(),
        id: 0,
        values: [0.0; DIMS],
    }]);
    let cap = 8 + one * 2;
    let d = VectorDirectory::new([A.to_string()], false).with_persistence_capped(tmp.path().to_path_buf(), cap);
    let s = d.store_for(Some(A)).unwrap();
    s.ingest(&from("inst-1"), &vecs(0..1), true).unwrap();
    s.ingest(&from("inst-1"), &vecs(1..2), true).unwrap();
    let before = std::fs::metadata(tmp.path().join(format!("{A}.vec"))).unwrap().len();
    assert!(matches!(s.ingest(&from("inst-1"), &vecs(2..3), true), Err(StoreError::Full(_))));
    assert_eq!(s.len(), 2, "memory did not get ahead of the log");
    assert_eq!(std::fs::metadata(tmp.path().join(format!("{A}.vec"))).unwrap().len(), before);
    assert!(before <= cap);
}

#[test]
fn a_torn_tail_is_dropped_and_appends_resume() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("p.vec");
    {
        let (mut log, existing) = VectorLog::open(&path, 1 << 20).unwrap();
        assert!(existing.is_empty());
        log.append(&[rec(1), rec(2)]).unwrap();
        log.append(&[rec(3)]).unwrap();
    }
    let good = std::fs::metadata(&path).unwrap().len();
    // A crash mid-frame: half a frame at the end.
    std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(&[9, 0, 0, 0, 1, 2, 3]).unwrap();
    let (mut log, records) = VectorLog::open(&path, 1 << 20).unwrap();
    assert_eq!(records, vec![rec(1), rec(2), rec(3)]);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), good, "tail truncated");
    log.append(&[rec(4)]).unwrap();
    drop(log);
    assert_eq!(VectorLog::open(&path, 1 << 20).unwrap().1.len(), 4);

    // A flipped byte inside a frame fails its checksum: that frame and later ones are dropped.
    let mut bytes = std::fs::read(&path).unwrap();
    let n = bytes.len();
    bytes[n - 8] ^= 0xff;
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(VectorLog::open(&path, 1 << 20).unwrap().1, vec![rec(1), rec(2), rec(3)]);
}

#[test]
fn an_unreadable_log_is_moved_aside_not_overwritten() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(format!("{A}.vec")), b"not a vector log at all").unwrap();
    let d = dir_over(tmp.path());
    let s = d.store_for(Some(A)).unwrap();
    assert!(s.is_empty());
    assert_eq!(std::fs::read(tmp.path().join(format!("{A}.vec.unreadable"))).unwrap(), b"not a vector log at all");
    s.ingest(&from("i"), &vecs(0..1), true).unwrap();
    assert_eq!(dir_over(tmp.path()).store_for(Some(A)).unwrap().len(), 1);
}

#[test]
fn a_memory_only_directory_still_forgets_on_restart() {
    let d = VectorDirectory::new([A.to_string()], false);
    d.store_for(Some(A)).unwrap().ingest(&from("i"), &vecs(0..3), true).unwrap();
    let d2 = VectorDirectory::new([A.to_string()], false);
    assert!(d2.store_for(Some(A)).unwrap().is_empty());
    let _: Arc<dyn IngestStore> = d.store_for(Some(A)).unwrap();
}
