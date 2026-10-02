//! The client's pin of the machine public key (`~/.weftos/mesh/machine.pub`).

use std::io::Write;
use std::path::Path;

use crate::client::ClientError;
use crate::hexser;

/// Creates `to` as a hard link of `from`, failing if `to` exists. Injected so
/// the fallback for filesystems without hard links can be tested.
pub type LinkFn<'a> = &'a dyn Fn(&Path, &Path) -> std::io::Result<()>;

/// First contact writes the pin atomically; afterwards a different key is a
/// hard error. Comparison is case-insensitive (the pin is normalised to
/// lowercase on write); an empty or malformed pin is `PinCorrupt`.
pub(crate) fn check_pin(path: &Path, presented: &[u8; 32]) -> Result<(), ClientError> {
    check_pin_with(path, presented, &|from, to| std::fs::hard_link(from, to))
}

pub(crate) fn check_pin_with(
    path: &Path,
    presented: &[u8; 32],
    link: LinkFn<'_>,
) -> Result<(), ClientError> {
    let compare = |text: &str| -> Result<(), ClientError> {
        let norm = text.trim().to_ascii_lowercase();
        match hexser::decode::<32>(&norm) {
            None => Err(ClientError::PinCorrupt(path.to_path_buf())),
            Some(p) if &p == presented => Ok(()),
            Some(_) => Err(ClientError::MachineKeyChanged {
                pinned: norm,
                presented: hexser::encode(presented),
            }),
        }
    };
    match std::fs::read_to_string(path) {
        Ok(s) => compare(&s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            write_pin(path, presented, link)?;
            // A concurrent first contact may have won the race: re-read.
            compare(&std::fs::read_to_string(path)?)
        }
        Err(e) => Err(e.into()),
    }
}

fn private_options() -> std::fs::OpenOptions {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o
}

/// temp file (0600) + fsync + create-if-absent link + fsync of the dir, so a
/// crash never leaves a partial pin and two writers cannot clobber each other.
/// When the filesystem cannot hard link (FAT, some FUSE mounts) it falls back
/// to `create_new` on the final path + write + fsync: still exclusive, but a
/// crash mid-write can leave a short file, which the next run reports as
/// `PinCorrupt` rather than trusting.
fn write_pin(path: &Path, key: &[u8; 32], link: LinkFn<'_>) -> Result<(), ClientError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut db = std::fs::DirBuilder::new();
    db.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut db, 0o700);
    db.create(dir)?;
    let hex = hexser::encode(key);
    let tmp = dir.join(format!(".machine.pub.tmp.{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut f = private_options().open(&tmp)?;
    f.write_all(hex.as_bytes())?;
    f.sync_all()?;
    drop(f);
    let linked = link(&tmp, path);
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => match private_options().open(path) {
            Ok(mut f) => {
                f.write_all(hex.as_bytes())?;
                f.sync_all()?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        },
    }
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    const KEY: [u8; 32] = [0x5a; 32];

    fn no_links(_: &Path, _: &Path) -> std::io::Result<()> {
        Err(Error::new(ErrorKind::Unsupported, "no hard links here"))
    }

    fn files(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir).unwrap().map(|d| d.unwrap().file_name().to_string_lossy().into()).collect()
    }

    #[test]
    fn falls_back_when_hard_links_are_unsupported() {
        let d = tempfile::tempdir().unwrap();
        let pin = d.path().join("mesh/machine.pub");
        check_pin_with(&pin, &KEY, &no_links).unwrap();
        assert_eq!(std::fs::read_to_string(&pin).unwrap(), hexser::encode(&KEY));
        assert_eq!(files(pin.parent().unwrap()), vec!["machine.pub"], "temp file removed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&pin).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // The pin now holds, and a different key is still a hard error.
        check_pin_with(&pin, &KEY, &no_links).unwrap();
        assert!(matches!(check_pin_with(&pin, &[1; 32], &no_links), Err(ClientError::MachineKeyChanged { .. })));
    }

    #[test]
    fn fallback_does_not_overwrite_a_pin_that_appeared_meanwhile() {
        let d = tempfile::tempdir().unwrap();
        let pin = d.path().join("machine.pub");
        let racer = pin.clone();
        // The link "fails" but another process has created a different pin.
        let link = move |_: &Path, _: &Path| {
            std::fs::write(&racer, hexser::encode(&[9; 32]))?;
            Err(Error::new(ErrorKind::Unsupported, "no links"))
        };
        let err = check_pin_with(&pin, &KEY, &link).unwrap_err();
        assert!(matches!(err, ClientError::MachineKeyChanged { .. }), "{err:?}");
        assert_eq!(std::fs::read_to_string(&pin).unwrap(), hexser::encode(&[9; 32]));
    }

    #[test]
    fn already_exists_from_link_is_a_lost_race_not_an_error() {
        let d = tempfile::tempdir().unwrap();
        let pin = d.path().join("machine.pub");
        let racer = pin.clone();
        let link = move |_: &Path, _: &Path| {
            std::fs::write(&racer, hexser::encode(&KEY))?;
            Err(Error::new(ErrorKind::AlreadyExists, "raced"))
        };
        check_pin_with(&pin, &KEY, &link).unwrap();
    }

    #[test]
    fn real_hard_link_path_leaves_one_file() {
        let d = tempfile::tempdir().unwrap();
        let pin = d.path().join("machine.pub");
        check_pin(&pin, &KEY).unwrap();
        assert_eq!(files(d.path()), vec!["machine.pub"]);
    }
}
