//! `weft.capture.v1` session package parsing and job-input detection.
//!
//! See `docs/weftos/capture-protocol-v1.md`. Library only — splatd ingests
//! uploads/paths into `job_root/session/`; the pipeline reads that layout.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Protocol id written by full clients in `session.json`.
pub const CAPTURE_PROTOCOL_V1: &str = "weft.capture.v1";

/// Well-known relative name of the materialized session under a job root.
pub const SESSION_DIR_NAME: &str = "session";

/// Image extensions accepted for A0 image-set train path (case-insensitive).
pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "heic", "heif"];

/// What kind of input a job workspace holds after ingest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    Video,
    ImageSet,
}

/// Resolved job input: video file path or session directory path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobInput {
    Video(PathBuf),
    ImageSet(PathBuf),
}

impl JobInput {
    pub fn kind(&self) -> InputKind {
        match self {
            JobInput::Video(_) => InputKind::Video,
            JobInput::ImageSet(_) => InputKind::ImageSet,
        }
    }

    pub fn path(&self) -> &Path {
        match self {
            JobInput::Video(p) | JobInput::ImageSet(p) => p,
        }
    }
}

/// One line of `poses.jsonl` (required fields + optional coverage_bin).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PoseLine {
    pub frame_id: u64,
    pub t_ns: u64,
    pub path: String,
    pub quat_wxyz: [f64; 4],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage_bin: Option<u32>,
}

/// Parsed session package under a directory.
#[derive(Debug, Clone)]
pub struct SessionLayout {
    pub root: PathBuf,
    /// Raw `session.json` when present.
    pub session_json: Option<serde_json::Value>,
    pub poses: Vec<PoseLine>,
    /// Absolute paths to image files under `frames/` (sorted by file name).
    pub frame_paths: Vec<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session directory missing: {0}")]
    MissingDir(String),
    #[error("session has no frames under frames/: {0}")]
    NoFrames(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid pose line {line}: {msg}")]
    PoseLine { line: usize, msg: String },
}

/// True if `path` looks like a weft.capture.v1 session (has frames with images).
pub fn is_session_dir(path: &Path) -> bool {
    let frames = path.join("frames");
    if !frames.is_dir() {
        return false;
    }
    list_frame_files(&frames).is_ok_and(|f| !f.is_empty())
}

/// Detect video vs image-set under a job workspace root.
///
/// Prefers a materialized `session/` directory when valid; otherwise looks
/// for `input.<ext>` video files.
pub fn detect_job_input(job_root: &Path) -> Option<JobInput> {
    let session = job_root.join(SESSION_DIR_NAME);
    if is_session_dir(&session) {
        return Some(JobInput::ImageSet(session));
    }
    find_video_input(job_root).map(JobInput::Video)
}

/// Find `input.*` video file placed by splatd video ingest.
pub fn find_video_input(job_root: &Path) -> Option<PathBuf> {
    std::fs::read_dir(job_root).ok()?.flatten().find_map(|e| {
        let p = e.path();
        let name = p.file_name()?.to_str()?;
        (name.starts_with("input.") && p.is_file() && !is_session_marker_name(name)).then_some(p)
    })
}

fn is_session_marker_name(name: &str) -> bool {
    // Reserved for future markers; never treat as video.
    name.eq_ignore_ascii_case("input.session")
}

/// Parse and validate a session directory.
pub fn parse_session(root: &Path) -> Result<SessionLayout, SessionError> {
    if !root.is_dir() {
        return Err(SessionError::MissingDir(root.display().to_string()));
    }
    let frames_dir = root.join("frames");
    let frame_paths = list_frame_files(&frames_dir)?;
    if frame_paths.is_empty() {
        return Err(SessionError::NoFrames(root.display().to_string()));
    }

    let session_json = {
        let p = root.join("session.json");
        if p.is_file() {
            let text = std::fs::read_to_string(&p)?;
            Some(serde_json::from_str(&text)?)
        } else {
            None
        }
    };

    let poses = parse_poses_jsonl(&root.join("poses.jsonl"))?;

    Ok(SessionLayout {
        root: root.to_path_buf(),
        session_json,
        poses,
        frame_paths,
    })
}

/// Parse `poses.jsonl` if present; missing file → empty list (frames-only ok).
pub fn parse_poses_jsonl(path: &Path) -> Result<Vec<PoseLine>, SessionError> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<PoseLine>(line) {
            Ok(p) => out.push(p),
            Err(e) => {
                return Err(SessionError::PoseLine {
                    line: i + 1,
                    msg: e.to_string(),
                });
            }
        }
    }
    Ok(out)
}

/// List image files under `frames/` (sorted by file name).
pub fn list_frame_files(frames_dir: &Path) -> Result<Vec<PathBuf>, SessionError> {
    if !frames_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths: Vec<PathBuf> = std::fs::read_dir(frames_dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_image_path(p))
        .collect();
    paths.sort();
    Ok(paths)
}

pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let lower = e.to_ascii_lowercase();
            IMAGE_EXTENSIONS.iter().any(|x| *x == lower)
        })
        .unwrap_or(false)
}

/// Copy (or hard-link when possible) session frames into `dest_images/`.
///
/// Destination files are named with a stable `frame_%05d.ext` sequence so
/// COLMAP/Brush see a predictable set. Returns the number of images written.
pub fn materialize_frames_to_images(
    session: &SessionLayout,
    dest_images: &Path,
) -> Result<u32, SessionError> {
    std::fs::create_dir_all(dest_images)?;
    // Clear prior contents so re-runs are honest.
    for ent in std::fs::read_dir(dest_images)?.flatten() {
        let p = ent.path();
        if p.is_file() {
            let _ = std::fs::remove_file(&p);
        }
    }

    let mut n = 0u32;
    for (i, src) in session.frame_paths.iter().enumerate() {
        let ext = src
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("jpg")
            .to_ascii_lowercase();
        // Prefer jpeg for COLMAP; keep original extension.
        let dest = dest_images.join(format!("frame_{:05}.{}", i + 1, ext));
        copy_or_link(src, &dest)?;
        n += 1;
    }
    Ok(n)
}

fn copy_or_link(src: &Path, dest: &Path) -> Result<(), SessionError> {
    // Prefer hard link (cheap); fall back to copy across devices.
    match std::fs::hard_link(src, dest) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(src, dest)?;
            Ok(())
        }
    }
}

/// If the session was zipped with a single top-level folder, peel it so
/// `frames/` and `poses.jsonl` sit directly under `dest`.
///
/// Safe no-op when layout is already flat.
pub fn normalize_session_root(dest: &Path) -> Result<(), SessionError> {
    if is_session_dir(dest) {
        return Ok(());
    }
    // Single subdirectory that is a valid session?
    let mut kids: Vec<PathBuf> = std::fs::read_dir(dest)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    // Ignore __MACOSX etc.
    kids.retain(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| !n.starts_with('.') && n != "__MACOSX")
    });
    if kids.len() == 1 && is_session_dir(&kids[0]) {
        let inner = kids.into_iter().next().unwrap();
        // Move children of inner up into dest.
        for ent in std::fs::read_dir(&inner)?.flatten() {
            let from = ent.path();
            let name = ent.file_name();
            let to = dest.join(name);
            if to.exists() {
                continue;
            }
            std::fs::rename(&from, &to)?;
        }
        let _ = std::fs::remove_dir_all(&inner);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tiny_jpeg(path: &Path) {
        // Minimal JPEG (1x1) — valid enough for extension/layout tests.
        let bytes: &[u8] = &[
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00,
            0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0xFF, 0xDB, 0x00, 0x43, 0x00, 0x08, 0x06, 0x06,
            0x07, 0x06, 0x05, 0x08, 0x07, 0x07, 0x07, 0x09, 0x09, 0x08, 0x0A, 0x0C, 0x14, 0x0D,
            0x0C, 0x0B, 0x0B, 0x0C, 0x19, 0x12, 0x13, 0x0F, 0x14, 0x1D, 0x1A, 0x1F, 0x1E, 0x1D,
            0x1A, 0x1C, 0x1C, 0x20, 0x24, 0x2E, 0x27, 0x20, 0x22, 0x2C, 0x23, 0x1C, 0x1C, 0x28,
            0x37, 0x29, 0x2C, 0x30, 0x31, 0x34, 0x34, 0x34, 0x1F, 0x27, 0x39, 0x3D, 0x38, 0x32,
            0x3C, 0x2E, 0x33, 0x34, 0x32, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x01, 0x00, 0x01,
            0x01, 0x01, 0x11, 0x00, 0xFF, 0xC4, 0x00, 0x1F, 0x00, 0x00, 0x01, 0x05, 0x01, 0x01,
            0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02,
            0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0xFF, 0xC4, 0x00, 0xB5, 0x10,
            0x00, 0x02, 0x01, 0x03, 0x03, 0x02, 0x04, 0x03, 0x05, 0x05, 0x04, 0x04, 0x00, 0x00,
            0x01, 0x7D, 0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06,
            0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1, 0x08, 0x23, 0x42,
            0xB1, 0xC1, 0x15, 0x52, 0xD1, 0xF0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0A, 0x16,
            0x17, 0x18, 0x19, 0x1A, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x34, 0x35, 0x36, 0x37,
            0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55,
            0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x73,
            0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
            0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5,
            0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA,
            0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6,
            0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA,
            0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA, 0xFF, 0xDA, 0x00, 0x08,
            0x01, 0x01, 0x00, 0x00, 0x3F, 0x00, 0x7F, 0xFF, 0xD9,
        ];
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, bytes).unwrap();
    }

    fn make_session(root: &Path) {
        write_tiny_jpeg(&root.join("frames/000001.jpg"));
        write_tiny_jpeg(&root.join("frames/000002.jpg"));
        let mut poses = std::fs::File::create(root.join("poses.jsonl")).unwrap();
        writeln!(
            poses,
            r#"{{"frame_id":1,"t_ns":1000,"path":"frames/000001.jpg","quat_wxyz":[1,0,0,0],"coverage_bin":0}}"#
        )
        .unwrap();
        writeln!(
            poses,
            r#"{{"frame_id":2,"t_ns":2000,"path":"frames/000002.jpg","quat_wxyz":[0.6,0,0,0.8],"coverage_bin":1}}"#
        )
        .unwrap();
        std::fs::write(
            root.join("session.json"),
            r#"{"protocol":"weft.capture.v1","id":"demo","n_frames":2}"#,
        )
        .unwrap();
    }

    #[test]
    fn parse_session_layout() {
        let dir = tempfile_dir("parse_session");
        make_session(&dir);
        let layout = parse_session(&dir).expect("parse");
        assert_eq!(layout.frame_paths.len(), 2);
        assert_eq!(layout.poses.len(), 2);
        assert_eq!(layout.poses[0].frame_id, 1);
        assert_eq!(layout.poses[0].coverage_bin, Some(0));
        assert_eq!(layout.poses[1].quat_wxyz[0], 0.6);
        let proto = layout.session_json.as_ref().unwrap()["protocol"]
            .as_str()
            .unwrap();
        assert_eq!(proto, CAPTURE_PROTOCOL_V1);
    }

    #[test]
    fn detect_video_vs_image_set() {
        let job = tempfile_dir("detect_input");
        // Empty job: nothing
        assert!(detect_job_input(&job).is_none());

        // Video only
        std::fs::write(job.join("input.mp4"), b"fake").unwrap();
        match detect_job_input(&job) {
            Some(JobInput::Video(p)) => assert!(p.ends_with("input.mp4")),
            other => panic!("expected video, got {other:?}"),
        }

        // Session wins when present
        let session = job.join(SESSION_DIR_NAME);
        make_session(&session);
        match detect_job_input(&job) {
            Some(JobInput::ImageSet(p)) => assert!(p.ends_with(SESSION_DIR_NAME)),
            other => panic!("expected image-set, got {other:?}"),
        }
    }

    #[test]
    fn materialize_frames_copies() {
        let root = tempfile_dir("materialize");
        make_session(&root);
        let layout = parse_session(&root).unwrap();
        let dest = root.join("out_images");
        let n = materialize_frames_to_images(&layout, &dest).unwrap();
        assert_eq!(n, 2);
        assert!(dest.join("frame_00001.jpg").is_file());
        assert!(dest.join("frame_00002.jpg").is_file());
    }

    #[test]
    fn normalize_peels_single_wrapper() {
        let dest = tempfile_dir("normalize");
        let inner = dest.join("session-abc");
        make_session(&inner);
        assert!(!is_session_dir(&dest));
        normalize_session_root(&dest).unwrap();
        assert!(is_session_dir(&dest));
    }

    #[test]
    fn bad_pose_line_errors() {
        let dir = tempfile_dir("bad_pose");
        write_tiny_jpeg(&dir.join("frames/000001.jpg"));
        std::fs::write(dir.join("poses.jsonl"), "not-json\n").unwrap();
        let err = parse_session(&dir).unwrap_err();
        assert!(matches!(err, SessionError::PoseLine { line: 1, .. }));
    }

    fn tempfile_dir(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "clawft-splat-session-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }
}
