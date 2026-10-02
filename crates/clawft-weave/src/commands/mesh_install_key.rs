//! Which box key the printed install script gives the service (ADR-103 P3,
//! review M1). A machine that already ran the mesh collapsed has a node key
//! its peers pinned (`~/.weftos/run/node.key`). Installing without adopting it
//! makes the service generate a new one, so the node id changes and those
//! pins break. That is never done by omission: with a collapsed key present,
//! one of `--adopt-node-key` or `--fresh-node-key` is required.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::service_units_system::STATE_DIR;

/// `<home>/.weftos/run/node.key` when it exists (the collapsed daemon's key).
pub fn collapsed_key(home: &Path) -> Option<PathBuf> {
    let p = home.join(".weftos").join("run").join("node.key");
    std::fs::symlink_metadata(&p).is_ok().then_some(p)
}

/// Check the key flags against what is on this machine. Returns the warnings
/// for the script header and stderr.
pub fn key_notes(adopt: Option<&Path>, fresh: bool, collapsed: Option<&Path>) -> Result<Vec<String>> {
    if adopt.is_some() && fresh {
        bail!("--adopt-node-key and --fresh-node-key are mutually exclusive");
    }
    match (adopt, fresh, collapsed) {
        (None, false, Some(k)) => bail!(
            "this machine already has a mesh node key at {k} (the collapsed daemon's). Installing without it \
             gives the service a NEW node id, and peers that pinned this machine (the Pi) stop recognising it.\n\
             Keep the identity (recommended):  weaver mesh install-service --adopt-node-key {k} > install.sh\n\
             Or mint a new one deliberately:   weaver mesh install-service --fresh-node-key > install.sh",
            k = k.display()
        ),
        (None, true, Some(k)) => Ok(vec![format!(
            "--fresh-node-key: the service generates a NEW box key, so this machine's node id changes; the key \
             at {} is not used and peers that pinned the old id must re-pin it",
            k.display()
        )]),
        _ => Ok(Vec::new()),
    }
}

/// Remedy when the state dir already holds a different key than the one to
/// adopt (the service ran before and generated its own).
pub fn differs_remedy(stop: &str) -> String {
    format!(
        "remedy: the service already generated its own key (and signed its journal with it). To adopt instead:\n\
         \x20 {stop}\n\
         \x20 sudo mv {STATE_DIR} {STATE_DIR}.generated   (keep it until peers reconnect; its binds are lost)\n\
         \x20 then re-run this script"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_collapsed_key_needs_an_explicit_choice() {
        let k = Path::new("/h/.weftos/run/node.key");
        let e = key_notes(None, false, Some(k)).unwrap_err().to_string();
        assert!(e.contains("--adopt-node-key /h/.weftos/run/node.key") && e.contains("--fresh-node-key"), "{e}");
        assert!(key_notes(Some(k), false, Some(k)).unwrap().is_empty());
        let w = key_notes(None, true, Some(k)).unwrap();
        assert!(w[0].contains("node id changes"), "{w:?}");
        assert!(key_notes(None, false, None).unwrap().is_empty(), "a new machine generates silently");
        assert!(key_notes(Some(k), true, None).is_err());
    }

    #[test]
    fn collapsed_key_is_found_in_the_home_runtime_dir() {
        let h = tempfile::tempdir().unwrap();
        assert!(collapsed_key(h.path()).is_none());
        std::fs::create_dir_all(h.path().join(".weftos/run")).unwrap();
        std::fs::write(h.path().join(".weftos/run/node.key"), [0u8; 32]).unwrap();
        assert_eq!(collapsed_key(h.path()), Some(h.path().join(".weftos/run/node.key")));
    }
}
