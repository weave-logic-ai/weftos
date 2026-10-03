//! `weaver mesh nonce generate` (ADR-106 section 3): a fresh `mesh_nonce`.
//!
//! The nonce is 32 random bytes, not a secret, and must be identical on every
//! node of the mesh. The operator generates it once per mesh and writes it
//! into each node's config as `kernel.mesh.mesh_nonce`, next to
//! `genesis_hash`. This command only prints it; it never touches a config.

use std::io::Write;

use anyhow::Result;
use clap::Subcommand;
use rand::RngCore;

/// `weaver mesh nonce`.
#[derive(Subcommand, Debug)]
pub enum NonceCmd {
    /// Print a new random mesh nonce (64 hex chars) and where it goes.
    Generate,
}

/// A fresh nonce, lower-case hex.
pub fn generate() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Run one `nonce` verb, writing the value to `w`.
pub fn run(cmd: NonceCmd, w: &mut dyn Write) -> Result<()> {
    match cmd {
        NonceCmd::Generate => {
            writeln!(w, "{}", generate())?;
            eprintln!(
                "Put this value in kernel.mesh.mesh_nonce on EVERY node of this mesh, next to \
                 kernel.mesh.genesis_hash, then restart the daemons. It is not a secret. \
                 Changing it later orphans the Seed binding (weaver doctor reports it)."
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nonce_is_32_random_bytes_as_hex_and_the_stdout_is_only_the_value() {
        let (a, b) = (generate(), generate());
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(a, b);
        let mut out = Vec::new();
        run(NonceCmd::Generate, &mut out).unwrap();
        let line = String::from_utf8(out).unwrap();
        assert_eq!(line.trim().len(), 64);
        assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
        // It is accepted as a mesh nonce by the id derivation.
        let pin = "ab".repeat(32);
        assert!(clawft_kernel::licence::mesh_id_from_config(Some(&pin), Some(line.trim())).unwrap().is_some());
    }
}
