//! Signed boot contract for a distinct nested `--profile user` daemon.
//! The launcher provides a pinned master key, never a machine-service registration.
use nix::libc;
use anyhow::{Context, bail, ensure};
use clawft_kernel::parent_policy::{ParentPolicy, verify_parent_policy};
use clawft_types::config::nested::{NestedInstance, NestedRegistration};
use clawft_types::config::{Config, MeshAdmissionMode, MeshServicePolicy};
use clawft_types::project::canon::{canonical_json, hex_decode, hex_encode};
use clawft_types::project::cert::key_id;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const CONTRACT_ENV: &str = "WEFTOS_NESTED_BOOT";
pub const MASTER_ENV: &str = "WEFTOS_NESTED_MASTER";
const DOMAIN: &[u8] = b"weftos-nested-user-boot-v1\n";
static ACTIVE: OnceLock<BootContract> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootContract {
    pub schema: u32,
    pub generation: u64,
    pub cap: clawft_types::config::overlay::OverlayFile,
    pub instance: NestedInstance,
    pub home: PathBuf,
    pub runtime: PathBuf,
    pub config: PathBuf,
    pub config_hash: String,
    pub inner_pubkey: String,
    pub registration: NestedRegistration,
    pub policy: ParentPolicy,
    pub expires_at: i64,
    pub sig: String,
}

impl BootContract {
    fn bytes(&self) -> Vec<u8> {
        let mut body = serde_json::to_value(self).expect("boot contract serializes");
        body.as_object_mut().unwrap().remove("sig");
        let mut bytes = DOMAIN.to_vec();
        bytes.extend_from_slice(canonical_json(&body).as_bytes());
        bytes
    }
    pub fn sign(&mut self, master: &SigningKey) {
        self.sig = hex_encode(&master.sign(&self.bytes()).to_bytes());
    }
    pub fn verify(&self, master: &[u8; 32], now: i64) -> anyhow::Result<()> {
        ensure!(self.generation > 0, "invalid boot generation");
        ensure!(
            self.schema == 1 && self.expires_at > now,
            "expired or unsupported nested boot"
        );
        clawft_types::project::validate_id(&self.instance.id)?;
        ensure!(
            self.instance.depth > 0 && self.instance.depth <= 32,
            "invalid nested depth"
        );
        ensure!(
            self.instance.parent == key_id(master),
            "wrong master identity"
        );
        let signature = hex_decode::<64>(&self.sig).context("malformed nested signature")?;
        VerifyingKey::from_bytes(master)?
            .verify_strict(&self.bytes(), &Signature::from_bytes(&signature))?;
        verify_parent_policy(&self.policy, master)?;
        let inner = hex_decode::<32>(&self.inner_pubkey).context("invalid inner key")?;
        ensure!(inner != *master, "inner must have its own user key");
        self.registration
            .validate(&key_id(&inner))
            .map_err(anyhow::Error::msg)?;
        ensure!(
            self.home.is_absolute() && self.runtime.is_absolute() && self.config.is_absolute(),
            "nested paths must be absolute"
        );
        ensure!(
            self.runtime == self.home.join("r"),
            "runtime must be private to inner HOME"
        );
        Ok(())
    }
}

pub fn config_hash(bytes: &[u8]) -> String {
    hex_encode(&Sha256::digest(bytes))
}
pub fn active() -> Option<&'static BootContract> {
    ACTIVE.get()
}

/// Force master-owned listeners and identity. No inherited socket, discovery,
/// default port, open membership or caller-selected machine service is retained.
pub fn constrain_config(config: &mut Config, contract: &BootContract) -> anyhow::Result<()> {
    config.weave.nested = Some(contract.instance.clone());
    config.kernel.ipc_tcp = None;
    config.gateway.host = "127.0.0.1".into();
    config.gateway.port = 0;
    config.gateway.api_port = 0;
    config.gateway.api_enabled = false;
    config.kernel.profile = None; // CLI user profile; never a project kernel.
    let mesh = config.kernel.mesh.get_or_insert_with(Default::default);
    mesh.service = MeshServicePolicy::Off;
    mesh.service_socket = None;
    mesh.discovery = false;
    mesh.seed_peers.clear();
    mesh.admission_open_membership = false;
    mesh.noise_key_path = Some(contract.runtime.join("node.key").to_string_lossy().into());
    match &contract.registration {
        NestedRegistration::Isolated => {
            mesh.enabled = false;
        }
        NestedRegistration::Collapsed {
            listen,
            genesis_hash,
            peers,
            ..
        } => {
            mesh.enabled = true;
            mesh.transport = "tcp".into();
            mesh.listen_addr = listen.to_string();
            mesh.noise = true;
            mesh.admission = MeshAdmissionMode::Enforce;
            mesh.genesis_hash = Some(genesis_hash.clone());
            mesh.seed_peers = peers.clone();
        }
    }
    let chain = config.kernel.chain.get_or_insert_with(Default::default);
    chain.enabled = true;
    chain.checkpoint_path = Some(
        contract
            .home
            .join(".weftos/chain/chain.json")
            .to_string_lossy()
            .into(),
    );
    chain.external_anchor = None;
    Ok(())
}

/// Called before daemon boot/config cloning. Config alone cannot opt out of the
/// signed envelope when the supervisor's boot environment is present.
pub fn enter(
    config: &mut Config,
    config_path: Option<&str>,
    user_profile: bool,
) -> anyhow::Result<()> {
    let path = std::env::var_os(CONTRACT_ENV);
    if path.is_none() && config.weave.nested.is_none() {
        return Ok(());
    }
    ensure!(user_profile, "nested instances require --profile user");
    let path = PathBuf::from(path.context("nested instance needs a supervisor boot contract")?);
    let master = std::env::var(MASTER_ENV).context("nested boot needs pinned master")?;
    let master = hex_decode::<32>(&master).context("malformed master pin")?;
    let contract: BootContract = serde_json::from_slice(&read_private(&path)?)?;
    contract.verify(&master, chrono::Utc::now().timestamp())?;
    ensure!(
        !path.with_extension("revoked").exists(),
        "nested registration revoked"
    );
    ensure!(
        std::env::var_os("HOME").map(PathBuf::from).as_ref() == Some(&contract.home),
        "HOME differs from master contract"
    );
    ensure!(
        std::env::var_os("WEFTOS_RUNTIME_DIR")
            .map(PathBuf::from)
            .as_ref()
            == Some(&contract.runtime),
        "runtime differs from master contract"
    );
    ensure!(
        config_path.map(Path::new) == Some(contract.config.as_path()),
        "config differs from master contract"
    );
    ensure!(
        config_hash(&read_private(&contract.config)?) == contract.config_hash,
        "nested config changed after signing"
    );
    let seed = crate::user_key::read_seed(&contract.home.join(".weftos/user.key"))?;
    let key = SigningKey::from_bytes(&seed);
    ensure!(
        hex_encode(&key.verifying_key().to_bytes()) == contract.inner_pubkey,
        "inner key differs from grant"
    );
    ensure!(
        crate::user_key::read_seed(&contract.runtime.join("node.key"))? == seed,
        "collapsed node must use granted inner key"
    );
    consume_generation(&contract)?;
    constrain_config(config, &contract)?;
    clawft_kernel::overlay_runtime::install_nested_policy(
        contract.runtime.clone(),
        contract.home.clone(),
        &contract.instance.id,
        master,
        contract.policy.clone(),
    )
    .map_err(anyhow::Error::msg)?;
    #[cfg(feature = "mesh")]
    {
        let peers = match &contract.registration {
            NestedRegistration::Isolated => Vec::new(),
            NestedRegistration::Collapsed { peers, .. } => peers
                .iter()
                .map(|p| p.rsplit_once('#').unwrap().1.to_owned())
                .collect(),
        };
        clawft_kernel::mesh_admit::install_nested_peer_ceiling(peers)
            .map_err(anyhow::Error::msg)?;
    }
    // Parent owns the pipe's write end. Even SIGKILL of the master closes it;
    // do not leave an unregistered inner mesh listener running after parent death.
    std::thread::spawn(|| {
        use std::io::Read;
        let mut byte = [0u8; 1];
        loop {
            match std::io::stdin().read(&mut byte) {
                Ok(0) | Err(_) => {
                    // Use the normal shutdown cascade, then bound any wedged exit.
                    unsafe {
                        libc::kill(libc::getpid(), libc::SIGTERM);
                    }
                    std::thread::sleep(std::time::Duration::from_secs(10));
                    std::process::exit(78);
                }
                Ok(_) => {}
            }
        }
    });
    ACTIVE
        .set(contract)
        .map_err(|_| anyhow::anyhow!("nested boot already installed"))?;
    Ok(())
}

fn read_private(path: &Path) -> anyhow::Result<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let m = f.metadata()?;
    ensure!(
        m.is_file() && m.mode() & 0o077 == 0 && m.uid() == unsafe { libc::geteuid() },
        "boot material is not owner-private"
    );
    let mut bytes = Vec::new();
    (&mut f).take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        bail!("boot material exceeds size limit");
    }
    Ok(bytes)
}

/// Durable replay floor independent of the parent's registry. Refuse missing or
/// corrupt history once written; parent restarts must issue a newer generation.
pub fn consume_generation(contract: &BootContract) -> anyhow::Result<()> {
    let path = contract.runtime.join("nested-generation");
    let previous = match std::fs::read_to_string(&path) {
        Ok(s) => s
            .trim()
            .parse::<u64>()
            .context("invalid nested generation floor")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => return Err(e.into()),
    };
    ensure!(
        contract.generation > previous,
        "replayed nested boot contract"
    );
    // Exclusive consumption also closes the concurrent-start race between the
    // floor read and the daemon's later runtime-lock acquisition.
    use std::os::unix::fs::OpenOptionsExt;
    let used = contract.runtime.join("nested-boot-generations");
    std::fs::create_dir_all(&used)?;
    let mut marker = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(used.join(contract.generation.to_string()))
        .context("nested boot generation already consumed")?;
    std::io::Write::write_all(&mut marker, contract.sig.as_bytes())?;
    marker.sync_all()?;
    clawft_kernel::parent_policy::write_atomic_0600(
        &path,
        contract.generation.to_string().as_bytes(),
    )?;
    Ok(())
}
