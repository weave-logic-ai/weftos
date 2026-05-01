//! Rfkill enumerated-state sensor adapter — second
//! [`Characterization`] exemplar after [`crate::mic`]'s `Rate`.
//!
//! Reads `/sys/class/rfkill/*/{type,soft,hard}` and emits one of
//! `unblocked | soft-blocked | hard-blocked | absent` per radio class.
//! No DBus, no rfkill(8) binary — sysfs only. Honest about its
//! resolution: rfkill returns a small enum of states; that's exactly
//! what [`Characterization::Enumerated`] is for.
//!
//! ## Topic
//!
//! | Topic | Shape | Refresh | Emits |
//! |-------|-------|---------|-------|
//! | `substrate/sensor/rfkill` | `{ wifi: state, bluetooth: state, wwan: state }` | 5s | one of `unblocked | soft-blocked | hard-blocked | absent` per radio |
//!
//! [`Sensitivity::Public`]; no [`PermissionReq`]. Radio block state is
//! kernel-exposed metadata; no user content involved.
//!
//! ## Why this is the second exemplar (vs Spectral FFT-mic)
//!
//! WEFT-419 asks for *either* an Enumerated rfkill adapter OR a
//! Spectral FFT-mic. Rfkill ships first because:
//!
//! - **Zero new deps.** Sysfs is already in scope (`network`,
//!   `bluetooth` adapters use it).
//! - **Cross-adapter test.** Rfkill state intersects with the
//!   `bluetooth` adapter's `enabled` field — having both adapters
//!   exercises the substrate's "two adapters, overlapping signal
//!   domain" path.
//! - **FFT is its own can.** A Spectral mic needs an FFT crate
//!   (rustfft or similar), windowing, bin-decimation choices — all of
//!   which deserve their own review pass.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use crate::adapter::{
    AdapterError, BufferPolicy, OntologyAdapter, PermissionReq, RefreshHint, Sensitivity, SubId,
    Subscription, TopicDecl,
};
use crate::delta::StateDelta;
use crate::physical::{
    Characterization, PhysicalSensorAdapter, SensorCalibration, SensorInterface,
};

/// Channel depth — singleton topic.
const CHAN: usize = 1;
/// Poll cadence; matches `bluetooth` adapter so the two stay aligned
/// when a user toggles airplane-mode.
const TICK_SECS: u64 = 5;

/// Declared topic.
pub const TOPICS: &[TopicDecl] = &[TopicDecl {
    path: "substrate/sensor/rfkill",
    shape: "ontology://rfkill-state",
    refresh_hint: RefreshHint::Periodic { ms: TICK_SECS * 1000 },
    sensitivity: Sensitivity::Public,
    buffer_policy: BufferPolicy::Refuse,
    max_len: None,
}];

/// Permissions — sysfs reads only; no install-time grant required.
pub const PERMISSIONS: &[PermissionReq] = &[];

/// One of N discrete states a single radio's rfkill entry can take —
/// the textbook [`Characterization::Enumerated`] case.
///
/// Stable kebab-case strings on the wire so Explorer / tray match by
/// string without a Rust dep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RfkillState {
    /// No rfkill block — the radio is free to operate.
    Unblocked,
    /// Soft-blocked — set by software (airplane mode toggle, `rfkill
    /// block`, etc.). Reversible from userspace.
    SoftBlocked,
    /// Hard-blocked — physical kill switch / firmware. Userspace cannot
    /// clear this.
    HardBlocked,
    /// No rfkill entry of this type exists on the host.
    Absent,
}

impl RfkillState {
    /// Stable kebab-case form for serialization. Same as Serde would
    /// emit; explicit so callers can build wire values without a serde
    /// round-trip.
    pub fn as_str(self) -> &'static str {
        match self {
            RfkillState::Unblocked => "unblocked",
            RfkillState::SoftBlocked => "soft-blocked",
            RfkillState::HardBlocked => "hard-blocked",
            RfkillState::Absent => "absent",
        }
    }

    /// Derive from sysfs `soft` + `hard` flag strings (raw file
    /// contents). `hard=1` wins over `soft=1` since the user can do
    /// nothing about hard blocks from software.
    pub fn from_sysfs(soft: &str, hard: &str) -> Self {
        let hard_blocked = hard.trim() == "1";
        let soft_blocked = soft.trim() == "1";
        if hard_blocked {
            RfkillState::HardBlocked
        } else if soft_blocked {
            RfkillState::SoftBlocked
        } else {
            RfkillState::Unblocked
        }
    }
}

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

/// Host-local rfkill adapter.
pub struct RfkillAdapter {
    reg: Mutex<Registry>,
    /// `/sys/class/rfkill` root — overridable for tests.
    rfkill_root: PathBuf,
}

impl Default for RfkillAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl RfkillAdapter {
    /// Build with the real `/sys/class/rfkill` root.
    pub fn new() -> Self {
        Self::with_root(PathBuf::from("/sys/class/rfkill"))
    }

    /// Construct with an arbitrary sysfs root — used by unit tests to
    /// feed canned `/sys/class/rfkill` directories.
    pub fn with_root(rfkill_root: PathBuf) -> Self {
        Self {
            reg: Mutex::new(Registry::new()),
            rfkill_root,
        }
    }
}

#[async_trait]
impl OntologyAdapter for RfkillAdapter {
    fn id(&self) -> &'static str {
        "rfkill"
    }

    fn topics(&self) -> &'static [TopicDecl] {
        TOPICS
    }

    fn permissions(&self) -> &'static [PermissionReq] {
        PERMISSIONS
    }

    async fn open(
        &self,
        topic: &str,
        _args: Value,
    ) -> Result<Subscription, AdapterError> {
        if topic != "substrate/sensor/rfkill" {
            return Err(AdapterError::UnknownTopic(topic.into()));
        }
        let id = {
            let mut reg = self.reg.lock();
            reg.allocate()
        };
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (tx, rx) = mpsc::channel::<StateDelta>(CHAN);
        self.reg.lock().live.insert(id, cancel_tx);

        let rfkill_root = self.rfkill_root.clone();
        tokio::spawn(async move {
            poll_rfkill(rfkill_root, tx, cancel_rx).await;
        });
        Ok(Subscription { id, rx })
    }

    async fn close(&self, sub_id: SubId) -> Result<(), AdapterError> {
        let _ = self.reg.lock().live.remove(&sub_id);
        Ok(())
    }
}

#[async_trait]
impl PhysicalSensorAdapter for RfkillAdapter {
    fn model(&self) -> &'static str {
        "linux rfkill (sysfs)"
    }

    fn interface(&self) -> SensorInterface {
        // Sysfs reads aren't a real bus — model as a file-backed
        // interface rooted at the rfkill class dir.
        SensorInterface::FileBacked {
            path: self.rfkill_root.clone(),
        }
    }

    fn unit(&self) -> &'static str {
        // Enumerated state — no scalar unit. Use a stable label that
        // the Explorer's `unit` field can render without a special-case.
        "state"
    }

    fn range(&self) -> (f64, f64) {
        // Not a numeric reading; return a placeholder zero range so
        // consumers that always probe `range()` get a defined answer
        // without trying to plot it. The trait deliberately doesn't
        // ship a "no range" sentinel — Enumerated sensors signal
        // through `characterization()` instead.
        (0.0, 0.0)
    }

    fn calibration(&self) -> SensorCalibration {
        SensorCalibration {
            scale: 1.0,
            offset: 0.0,
            reference: Some(
                "rfkill sysfs: soft=1 → soft-blocked; hard=1 → hard-blocked".into(),
            ),
        }
    }

    fn characterization(&self) -> Characterization {
        // Honest: one of N discrete states, no in-between, no rate.
        Characterization::Enumerated
    }
}

async fn poll_rfkill(
    rfkill_root: PathBuf,
    tx: mpsc::Sender<StateDelta>,
    mut cancel_rx: oneshot::Receiver<()>,
) {
    let mut ticker = tokio::time::interval(Duration::from_secs(TICK_SECS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = &mut cancel_rx => return,
            _ = ticker.tick() => {
                let value = sample_rfkill(&rfkill_root);
                let delta = StateDelta::Replace {
                    path: "substrate/sensor/rfkill".to_string(),
                    value,
                };
                if tx.send(delta).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Walk `/sys/class/rfkill/*` and emit `{ wifi, bluetooth, wwan,
/// characterization }`. The three-radio schema matches what the tray
/// actually wants to render; other rfkill types (nfc, gps, fm, …) are
/// retained under an `other` map so a future Explorer panel can show
/// them without re-walking sysfs.
pub fn sample_rfkill(rfkill_root: &Path) -> Value {
    let mut wifi = RfkillState::Absent;
    let mut bluetooth = RfkillState::Absent;
    let mut wwan = RfkillState::Absent;
    let mut other: serde_json::Map<String, Value> = serde_json::Map::new();

    if let Ok(entries) = std::fs::read_dir(rfkill_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            let kind = std::fs::read_to_string(path.join("type"))
                .unwrap_or_default()
                .trim()
                .to_string();
            let soft = std::fs::read_to_string(path.join("soft")).unwrap_or_default();
            let hard = std::fs::read_to_string(path.join("hard")).unwrap_or_default();
            let state = RfkillState::from_sysfs(&soft, &hard);
            match kind.as_str() {
                "wlan" | "wifi" => wifi = state,
                "bluetooth" => bluetooth = state,
                "wwan" => wwan = state,
                "" => continue, // unreadable; skip
                other_kind => {
                    other.insert(other_kind.to_string(), json!(state.as_str()));
                }
            }
        }
    }

    let mut obj = serde_json::Map::new();
    obj.insert("wifi".into(), json!(wifi.as_str()));
    obj.insert("bluetooth".into(), json!(bluetooth.as_str()));
    obj.insert("wwan".into(), json!(wwan.as_str()));
    obj.insert(
        "characterization".into(),
        json!(Characterization::Enumerated.as_str()),
    );
    if !other.is_empty() {
        obj.insert("other".into(), Value::Object(other));
    }
    Value::Object(obj)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn fake_rfkill_root(
        dir: &TempDir,
        entries: &[(&str /* name */, &str /* type */, &str /* soft */, &str /* hard */)],
    ) -> PathBuf {
        let root = dir.path().join("rfkill");
        fs::create_dir_all(&root).unwrap();
        for (name, kind, soft, hard) in entries {
            let entry = root.join(name);
            fs::create_dir_all(&entry).unwrap();
            fs::write(entry.join("type"), kind).unwrap();
            fs::write(entry.join("soft"), soft).unwrap();
            fs::write(entry.join("hard"), hard).unwrap();
        }
        root
    }

    #[test]
    fn from_sysfs_unblocked_when_both_zero() {
        assert_eq!(RfkillState::from_sysfs("0", "0"), RfkillState::Unblocked);
    }

    #[test]
    fn from_sysfs_soft_blocked_when_only_soft_set() {
        assert_eq!(
            RfkillState::from_sysfs("1", "0"),
            RfkillState::SoftBlocked
        );
    }

    #[test]
    fn from_sysfs_hard_blocked_takes_priority() {
        // hard=1 trumps soft=1: even if userspace toggles soft off,
        // the radio stays down until the physical switch flips.
        assert_eq!(
            RfkillState::from_sysfs("1", "1"),
            RfkillState::HardBlocked
        );
        assert_eq!(
            RfkillState::from_sysfs("0", "1"),
            RfkillState::HardBlocked
        );
    }

    #[test]
    fn from_sysfs_tolerates_trailing_newlines() {
        // Real sysfs files end in `\n`. Honour the contract.
        assert_eq!(
            RfkillState::from_sysfs("1\n", "0\n"),
            RfkillState::SoftBlocked
        );
    }

    #[test]
    fn state_strings_are_kebab_case() {
        assert_eq!(RfkillState::Unblocked.as_str(), "unblocked");
        assert_eq!(RfkillState::SoftBlocked.as_str(), "soft-blocked");
        assert_eq!(RfkillState::HardBlocked.as_str(), "hard-blocked");
        assert_eq!(RfkillState::Absent.as_str(), "absent");
    }

    #[test]
    fn sample_returns_absent_when_no_entries() {
        let dir = TempDir::new().unwrap();
        let root = fake_rfkill_root(&dir, &[]);
        let v = sample_rfkill(&root);
        assert_eq!(v["wifi"], "absent");
        assert_eq!(v["bluetooth"], "absent");
        assert_eq!(v["wwan"], "absent");
        assert_eq!(v["characterization"], "enumerated");
    }

    #[test]
    fn sample_reports_each_radio_class() {
        let dir = TempDir::new().unwrap();
        let root = fake_rfkill_root(
            &dir,
            &[
                ("rfkill0", "wlan", "0", "0"),
                ("rfkill1", "bluetooth", "1", "0"),
                ("rfkill2", "wwan", "0", "1"),
            ],
        );
        let v = sample_rfkill(&root);
        assert_eq!(v["wifi"], "unblocked");
        assert_eq!(v["bluetooth"], "soft-blocked");
        assert_eq!(v["wwan"], "hard-blocked");
        assert_eq!(v["characterization"], "enumerated");
    }

    #[test]
    fn sample_groups_unknown_radio_types_under_other() {
        let dir = TempDir::new().unwrap();
        let root = fake_rfkill_root(
            &dir,
            &[
                ("rfkill0", "wlan", "0", "0"),
                ("rfkill1", "nfc", "1", "0"),
                ("rfkill2", "gps", "0", "0"),
            ],
        );
        let v = sample_rfkill(&root);
        assert_eq!(v["wifi"], "unblocked");
        let other = v["other"].as_object().expect("other should be object");
        assert_eq!(other["nfc"], "soft-blocked");
        assert_eq!(other["gps"], "unblocked");
    }

    #[test]
    fn sample_treats_wifi_alias_synonymously_with_wlan() {
        // Some kernels surface `type=wifi`; alias to wlan in our schema.
        let dir = TempDir::new().unwrap();
        let root = fake_rfkill_root(&dir, &[("rfkill0", "wifi", "0", "0")]);
        let v = sample_rfkill(&root);
        assert_eq!(v["wifi"], "unblocked");
    }

    #[tokio::test]
    async fn adapter_open_unknown_topic_errors() {
        let a = RfkillAdapter::new();
        let r = a.open("substrate/sensor/bogus", Value::Null).await;
        assert!(matches!(r, Err(AdapterError::UnknownTopic(_))));
    }

    #[test]
    fn id_is_rfkill() {
        assert_eq!(RfkillAdapter::new().id(), "rfkill");
    }

    #[test]
    fn physical_trait_declares_enumerated_characterization() {
        // The whole point of WEFT-419: a real second exemplar that
        // exercises the Enumerated arm of the framework.
        let a = RfkillAdapter::new();
        assert_eq!(a.characterization(), Characterization::Enumerated);
        assert_eq!(a.unit(), "state");
    }

    #[test]
    fn declares_one_topic_with_singleton_buffer() {
        assert_eq!(TOPICS.len(), 1);
        assert_eq!(TOPICS[0].path, "substrate/sensor/rfkill");
        assert_eq!(TOPICS[0].buffer_policy, BufferPolicy::Refuse);
    }
}
