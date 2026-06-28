//! Presence reference adapter — exemplar for `Characterization::Presence`.
//!
//! Companion to [`crate::mic`] (which exemplifies `Rate`). Presence is
//! the lowest-resolution honest characterization: a binary "something
//! is happening / not happening" signal with no further structure
//! (PIR motion sensor, geiger tube on/off, limit switch, ToF
//! present/absent reduction).
//!
//! ## Characterization level (spectrometer principle)
//!
//! This adapter declares [`Characterization::Presence`]: it produces
//! `{ present: bool, … }` with NO derived "intensity" / "confidence" /
//! magnitude field. A consumer that wants more must upgrade the
//! adapter to a higher characterization (e.g. a ToF reduced to
//! `Enumerated` `near|mid|far`, or a camera-backed `Identifying`
//! classifier). The honest binary stays binary.
//!
//! ## Why this exemplar exists
//!
//! Pre-WEFT-436 the sensor framework was exercised only by the mic
//! adapter (`Characterization::Rate`). That left three of the five
//! Characterization levels uncovered by an in-tree exemplar — anyone
//! adding a Presence-style sensor had to extrapolate from a Rate
//! sensor and risk getting the rendering-honesty rules subtly wrong
//! (for example: emitting a `level` float for a sensor that genuinely
//! cannot measure level).
//!
//! By shipping a Presence adapter that produces ONLY `present` +
//! transition counters (NO scalar magnitude), the framework
//! demonstrates the discipline: the adapter signature constrains
//! what consumers can depend on.
//!
//! ## File-backed source (today) → real GPIO (next)
//!
//! Reads a single byte from a configurable file path:
//!
//! - `0` → `present: false`
//! - any non-zero byte → `present: true`
//!
//! The file is re-read every TICK_MS so a test (or a future systemd
//! sysfs-gpio wrapper) can flip the bit between ticks. A missing file
//! emits `{ available: false, reason: "source-missing" }` — the same
//! convention `mic.rs` uses, so consumers handle both adapters with
//! one code path.
//!
//! Real GPIO support arrives via a second constructor that takes a
//! [`SensorInterface::Gpio { pin }`] — the file-backed loop is
//! replaced with a sysfs-gpio reader.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::adapter::{
    AdapterError, BufferPolicy, OntologyAdapter, PermissionReq, RefreshHint, Sensitivity, SubId,
    Subscription, TopicDecl,
};
use crate::delta::StateDelta;
use crate::physical::{
    Characterization, PhysicalSensorAdapter, SensorCalibration, SensorInterface,
};

/// Channel depth for the singleton topic.
const CHAN: usize = 1;
/// Emission cadence — 1 Hz nominal. Presence is low-bandwidth; faster
/// polling adds noise without adding signal.
const TICK_MS: u64 = 1000;

/// Declared topics.
pub const TOPICS: &[TopicDecl] = &[TopicDecl {
    path: "substrate/sensor/presence",
    shape: "ontology://presence",
    refresh_hint: RefreshHint::Periodic { ms: TICK_MS },
    // Presence data is generally low-sensitivity (it doesn't reveal
    // *who* or *what*) — mark as Workspace so the install-time prompt
    // is one line, not a full disclosure.
    sensitivity: Sensitivity::Workspace,
    // Singleton — refuse on overflow rather than buffer transitions.
    buffer_policy: BufferPolicy::Refuse,
    max_len: None,
}];

/// Permissions — none for the file-backed preview. A real GPIO-backed
/// presence adapter would require an `fs:/sys/class/gpio` permission
/// or equivalent hardware grant (out of scope for this exemplar).
pub const PERMISSIONS: &[PermissionReq] = &[];

type CancelTx = oneshot::Sender<()>;

struct Registry {
    next_id: u64,
    live: HashMap<SubId, CancelTx>,
}

impl Registry {
    fn new() -> Self {
        Self {
            next_id: 1,
            live: HashMap::new(),
        }
    }

    fn allocate(&mut self) -> SubId {
        let id = SubId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        id
    }
}

/// Presence adapter.
pub struct PresenceAdapter {
    reg: Mutex<Registry>,
    source_path: PathBuf,
    /// Human-readable model — defaults to a generic PIR string but
    /// `with_model` lets a consumer brand it (`HC-SR501`, `RCWL-0516`,
    /// …) without changing the trait surface.
    model: &'static str,
}

impl Default for PresenceAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl PresenceAdapter {
    /// Build an adapter reading from the default file-backed source
    /// at `/tmp/weftos/presence/state.bin` (one byte: 0 / non-zero).
    pub fn new() -> Self {
        Self::with_source(PathBuf::from("/tmp/weftos/presence/state.bin"))
    }

    /// Build with an explicit source path — used by tests and by
    /// host-shim wrappers that point at `/sys/class/gpio/gpio<N>/value`.
    pub fn with_source(source_path: PathBuf) -> Self {
        Self {
            reg: Mutex::new(Registry::new()),
            source_path,
            model: "file-backed PIR (preview stub)",
        }
    }

    /// Override the reported model string. Use for branding when the
    /// underlying source is a known device (e.g. `"HC-SR501 PIR"`).
    pub fn with_model(mut self, model: &'static str) -> Self {
        self.model = model;
        self
    }
}

#[async_trait]
impl OntologyAdapter for PresenceAdapter {
    fn id(&self) -> &'static str {
        "presence"
    }

    fn topics(&self) -> &'static [TopicDecl] {
        TOPICS
    }

    fn permissions(&self) -> &'static [PermissionReq] {
        PERMISSIONS
    }

    async fn open(&self, topic: &str, _args: Value) -> Result<Subscription, AdapterError> {
        if topic != "substrate/sensor/presence" {
            return Err(AdapterError::UnknownTopic(topic.into()));
        }
        let id = {
            let mut reg = self.reg.lock();
            reg.allocate()
        };
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (tx, rx) = mpsc::channel::<StateDelta>(CHAN);
        self.reg.lock().live.insert(id, cancel_tx);

        let source_path = self.source_path.clone();
        tokio::spawn(async move {
            poll_presence(source_path, tx, cancel_rx).await;
        });
        Ok(Subscription { id, rx })
    }

    async fn close(&self, sub_id: SubId) -> Result<(), AdapterError> {
        let _ = self.reg.lock().live.remove(&sub_id);
        Ok(())
    }
}

#[async_trait]
impl PhysicalSensorAdapter for PresenceAdapter {
    fn model(&self) -> &'static str {
        self.model
    }

    fn interface(&self) -> SensorInterface {
        // FileBacked for the exemplar; a real PIR flips this to
        // `SensorInterface::Gpio { pin }` via a sysfs wrapper.
        SensorInterface::FileBacked {
            path: self.source_path.clone(),
        }
    }

    fn unit(&self) -> &'static str {
        // Presence is unitless — the byte is interpreted as a flag.
        // Calling this `"bool"` rather than `""` keeps Explorer
        // tooltips honest: "1 bool" reads as "one boolean reading"
        // rather than as a missing label.
        "bool"
    }

    fn range(&self) -> (f64, f64) {
        // Boolean: 0 or 1.
        (0.0, 1.0)
    }

    fn calibration(&self) -> SensorCalibration {
        // Identity calibration — there's nothing to scale.
        SensorCalibration {
            scale: 1.0,
            offset: 0.0,
            reference: Some("PIR/limit-switch boolean: 0 = absent, !=0 = present".into()),
        }
    }

    fn characterization(&self) -> Characterization {
        // Honest: binary present/absent. This adapter MUST NOT pretend
        // to know how-much / how-confident / how-near. Upgrade path
        // for those is a different Characterization tier.
        Characterization::Presence
    }
}

async fn poll_presence(
    source_path: PathBuf,
    tx: mpsc::Sender<StateDelta>,
    mut cancel_rx: oneshot::Receiver<()>,
) {
    let mut ticker = tokio::time::interval(Duration::from_millis(TICK_MS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut transitions: u64 = 0;
    let mut last: Option<bool> = None;

    loop {
        tokio::select! {
            _ = &mut cancel_rx => return,
            _ = ticker.tick() => {
                let read = read_presence(&source_path);
                if read.get("reason").and_then(Value::as_str) == Some("source-missing") {
                    // Same convention as `mic`: don't overwrite a
                    // healthy-then-missing publish with `available: false`,
                    // because consumers would lose the last good value.
                    continue;
                }
                // Increment transition counter when `present` changes.
                if let Some(present) = read.get("present").and_then(Value::as_bool)
                    && last != Some(present)
                {
                    transitions = transitions.saturating_add(1);
                    last = Some(present);
                }
                let mut value = read;
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("transitions".into(), json!(transitions));
                }
                let delta = StateDelta::Replace {
                    path: "substrate/sensor/presence".to_string(),
                    value,
                };
                if tx.send(delta).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Read one byte at the path and turn it into a presence emission.
/// Pulled out of the polling loop so unit tests can exercise the
/// shape without spinning the tokio runtime.
fn read_presence(source_path: &Path) -> Value {
    let Ok(mut file) = std::fs::File::open(source_path) else {
        return json!({
            "available": false,
            "reason": "source-missing",
            "characterization": Characterization::Presence.as_str(),
        });
    };
    let mut buf = [0u8; 1];
    let n = file.read(&mut buf).unwrap_or(0);
    if n == 0 {
        // Empty file — treat as `absent` rather than as `unavailable`,
        // since the source did exist (this is a common "freshly
        // truncated" pattern when the writer is restarting).
        return json!({
            "available": true,
            "present": false,
            "characterization": Characterization::Presence.as_str(),
        });
    }
    json!({
        "available": true,
        "present": buf[0] != 0,
        "characterization": Characterization::Presence.as_str(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_byte(dir: &TempDir, name: &str, byte: u8) -> PathBuf {
        let path = dir.path().join(name);
        fs::write(&path, [byte]).unwrap();
        path
    }

    // ---- shape ------------------------------------------------------

    #[test]
    fn zero_byte_reports_absent() {
        let dir = TempDir::new().unwrap();
        let path = write_byte(&dir, "off.bin", 0);
        let v = read_presence(&path);
        assert_eq!(v["available"], true);
        assert_eq!(v["present"], false);
        assert_eq!(v["characterization"], "presence");
    }

    #[test]
    fn nonzero_byte_reports_present() {
        let dir = TempDir::new().unwrap();
        let path = write_byte(&dir, "on.bin", 0x01);
        let v = read_presence(&path);
        assert_eq!(v["available"], true);
        assert_eq!(v["present"], true);
    }

    #[test]
    fn high_byte_also_reports_present() {
        // Sysfs gpio writes "1\n" (ASCII 0x31). Any non-zero counts.
        let dir = TempDir::new().unwrap();
        let path = write_byte(&dir, "ascii.bin", b'1');
        let v = read_presence(&path);
        assert_eq!(v["present"], true);
    }

    #[test]
    fn empty_file_reports_absent_not_unavailable() {
        // Mid-restart writers truncate then write — read in that
        // window must NOT bounce subscribers to unavailable.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.bin");
        fs::write(&path, []).unwrap();
        let v = read_presence(&path);
        assert_eq!(v["available"], true);
        assert_eq!(v["present"], false);
    }

    #[test]
    fn missing_source_emits_unavailable() {
        let v = read_presence(Path::new("/nonexistent/weftos/presence.bin"));
        assert_eq!(v["available"], false);
        assert_eq!(v["reason"], "source-missing");
        assert_eq!(v["characterization"], "presence");
    }

    // ---- adapter ----------------------------------------------------

    #[tokio::test]
    async fn adapter_open_unknown_topic_errors() {
        let a = PresenceAdapter::new();
        let r = a.open("substrate/sensor/bogus", Value::Null).await;
        assert!(matches!(r, Err(AdapterError::UnknownTopic(_))));
    }

    #[tokio::test]
    async fn adapter_emits_with_transitions_field() {
        // Smoke test: open the adapter against an on-byte file and
        // wait for one delta. Asserts the live-emit code path adds
        // the `transitions` counter that read_presence() does NOT.
        let dir = TempDir::new().unwrap();
        let path = write_byte(&dir, "on.bin", 1);
        let a = PresenceAdapter::with_source(path);
        let mut sub = a
            .open("substrate/sensor/presence", Value::Null)
            .await
            .unwrap();
        // Wait up to 2s for the first tick.
        let delta = tokio::time::timeout(Duration::from_millis(2_500), sub.rx.recv())
            .await
            .expect("timeout waiting for presence emission")
            .expect("channel closed before emission");
        let StateDelta::Replace { path, value } = delta else {
            panic!("expected Replace, got {:?}", delta);
        };
        assert_eq!(path, "substrate/sensor/presence");
        assert_eq!(value["present"], true);
        // First emission counts as transition #1 (None -> Some(true)).
        assert_eq!(value["transitions"], 1);
        a.close(sub.id).await.unwrap();
    }

    // ---- characterization ------------------------------------------

    #[test]
    fn physical_trait_declares_presence_characterization() {
        let a = PresenceAdapter::new();
        // The point of this exemplar: HONESTLY declare presence-only.
        assert_eq!(a.characterization(), Characterization::Presence);
        assert_eq!(a.unit(), "bool");
        assert_eq!(a.range(), (0.0, 1.0));
    }

    #[test]
    fn model_string_overrideable() {
        let a = PresenceAdapter::with_source(PathBuf::from("/dev/null")).with_model("HC-SR501 PIR");
        assert_eq!(a.model(), "HC-SR501 PIR");
    }

    #[test]
    fn file_backed_interface_roundtrips_path() {
        let a = PresenceAdapter::with_source(PathBuf::from("/tmp/weftos/demo.bin"));
        match a.interface() {
            SensorInterface::FileBacked { path } => {
                assert_eq!(path, PathBuf::from("/tmp/weftos/demo.bin"));
            }
            other => panic!("expected FileBacked, got {other:?}"),
        }
    }

    #[test]
    fn calibration_is_identity_with_reference() {
        let a = PresenceAdapter::new();
        let cal = a.calibration();
        assert_eq!(cal.scale, 1.0);
        assert_eq!(cal.offset, 0.0);
        assert!(cal.reference.is_some());
    }
}
