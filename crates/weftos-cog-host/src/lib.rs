//! weft-cog-host (COG-009): the on-appliance cog store + record model.
//!
//! A host root (default `~/.weftos/cogs`) holds one directory per cog:
//! `<root>/<id>/{cog-<id>-arm, manifest.json, cog.json}`. `cog.json` is our record — where the cog
//! came from, its version, whether it should be running (`enabled`), and its args. The agent's own
//! `/var/lib/cognitum/apps` is untouched; this is a separate, uncapped lifecycle that runs cogs which
//! still POST to the agent store on `:80`.

pub mod supervise;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[serde(alias = "weavelogic")]
    WeaveLogic,
    #[serde(alias = "cognitum")]
    Cognitum,
    /// Added by hand / staged locally.
    Local,
}

/// The persisted record for one installed cog (`<root>/<id>/cog.json`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CogRecord {
    pub id: String,
    #[serde(default)]
    pub version: String,
    pub source: Source,
    /// Desired state: true means the supervisor keeps it running (and restarts it on exit).
    #[serde(default)]
    pub enabled: bool,
    /// Binary filename inside the cog dir (e.g. `cog-bridge-arm`).
    pub binary: String,
    /// CLI args the cog is launched with (e.g. `["--interval", "1"]`).
    #[serde(default)]
    pub args: Vec<String>,
    /// Whether the WeaveLogic signature was verified at install time (always true for WeaveLogic).
    #[serde(default)]
    pub signed: bool,
}

impl CogRecord {
    pub fn dir(&self, root: &Path) -> PathBuf {
        root.join(&self.id)
    }
    pub fn binary_path(&self, root: &Path) -> PathBuf {
        self.dir(root).join(&self.binary)
    }
}

/// Resolve the host root: `$WEFTOS_COG_ROOT`, else `~/.weftos/cogs`.
pub fn default_root() -> PathBuf {
    if let Ok(r) = std::env::var("WEFTOS_COG_ROOT") {
        return PathBuf::from(r);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".weftos").join("cogs")
}

/// Load every `<root>/<id>/cog.json` into records, sorted by id.
pub fn load_records(root: &Path) -> Vec<CogRecord> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(root) else {
        return out;
    };
    for e in rd.flatten() {
        let cj = e.path().join("cog.json");
        let Ok(bytes) = std::fs::read(&cj) else { continue };
        let Ok(rec) = serde_json::from_slice::<CogRecord>(&bytes) else { continue };
        out.push(rec);
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

pub fn save_record(root: &Path, rec: &CogRecord) -> std::io::Result<()> {
    let dir = rec.dir(root);
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::to_vec_pretty(rec).map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::write(dir.join("cog.json"), json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_round_trips_and_resolves_paths() {
        let rec = CogRecord {
            id: "bridge".into(),
            version: "0.1.0".into(),
            source: Source::WeaveLogic,
            enabled: true,
            binary: "cog-bridge-arm".into(),
            args: vec!["--interval".into(), "1".into()],
            signed: true,
        };
        let s = serde_json::to_string(&rec).unwrap();
        let back: CogRecord = serde_json::from_str(&s).unwrap();
        assert_eq!(rec, back);
        let root = Path::new("/tmp/r");
        assert_eq!(rec.binary_path(root), Path::new("/tmp/r/bridge/cog-bridge-arm"));
    }

    #[test]
    fn source_accepts_aliases() {
        let r: CogRecord = serde_json::from_str(
            r#"{"id":"x","source":"weavelogic","binary":"cog-x-arm"}"#,
        )
        .unwrap();
        assert_eq!(r.source, Source::WeaveLogic);
        assert!(!r.enabled && r.args.is_empty());
    }

    #[test]
    fn load_records_reads_a_root() {
        let root = std::env::temp_dir().join(format!("cog-host-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let rec = CogRecord {
            id: "a".into(),
            version: "1".into(),
            source: Source::Local,
            enabled: false,
            binary: "cog-a-arm".into(),
            args: vec![],
            signed: false,
        };
        save_record(&root, &rec).unwrap();
        let got = load_records(&root);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "a");
        let _ = std::fs::remove_dir_all(&root);
    }
}
