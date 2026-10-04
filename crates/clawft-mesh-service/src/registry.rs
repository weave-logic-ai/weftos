//! Registrations: one live mesh-local connection per user id, the project
//! addresses it owns and the topic-prefix table (plan 1.6, 2 S).
//!
//! One registration owns an address. A project id or topic prefix claimed by
//! two different users is refused for the second; prefixes also may not
//! overlap another user's (one being a prefix of the other), so a broad claim
//! cannot swallow another tenant's narrower one.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clawft_mesh_local::proto::{Frame, Rejected};
use clawft_mesh_local::UserCert;
use clawft_mesh_local::Principal;
use tokio::sync::{mpsc, watch};

/// Capacity of each registration's outbound queue for `deliver` traffic.
pub const QUEUE_CAP: usize = 256;
/// Extra slots only verdict requests may use, so a flood of `deliver` frames
/// can never starve an admission decision.
pub const RESERVED_SLOTS: usize = 16;
/// `send` budget per registration: at most this many per [`SEND_WINDOW`].
pub const SEND_LIMIT: u32 = 200;
pub const SEND_WINDOW: Duration = Duration::from_secs(1);
const MAX_PREFIX_LEN: usize = 128;
const MAX_PREFIXES_PER_USER: usize = 64;
const MAX_PROJECTS_PER_USER: usize = 256;

/// Per-registration counters.
#[derive(Debug, Default)]
pub struct Counters {
    pub delivered: AtomicU64,
    pub dropped_full: AtomicU64,
    pub sent: AtomicU64,
}

/// A live registered daemon connection.
pub struct Registration {
    pub conn_id: u64,
    pub principal: Principal,
    pub user_id: String,
    pub user_pubkey: [u8; 32],
    pub pid: u32,
    pub exe: String,
    pub capabilities: Vec<String>,
    pub registered_at: u64,
    pub counters: Counters,
    /// mesh-local protocol version negotiated on this connection; the
    /// `deliver` origin stamp is written only from `PROTO_ORIGIN` up.
    proto: AtomicU32,
    cert: Mutex<Option<UserCert>>,
    accept_from: Mutex<Vec<String>>,
    send_window: Mutex<(Instant, u32)>,
    tx: mpsc::Sender<Frame>,
    kill: watch::Sender<Option<String>>,
}

impl Registration {
    /// Build a registration plus the receiver its connection task drains and
    /// the watch it observes for a forced close.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        conn_id: u64,
        principal: Principal,
        user_id: String,
        user_pubkey: [u8; 32],
        pid: u32,
        exe: String,
        capabilities: Vec<String>,
        registered_at: u64,
    ) -> (Arc<Self>, mpsc::Receiver<Frame>, watch::Receiver<Option<String>>) {
        let (tx, rx) = mpsc::channel(QUEUE_CAP + RESERVED_SLOTS);
        let (kill, killed) = watch::channel(None);
        let reg = Arc::new(Self {
            conn_id,
            principal,
            user_id,
            user_pubkey,
            pid,
            exe,
            capabilities,
            registered_at,
            counters: Counters::default(),
            proto: AtomicU32::new(clawft_mesh_local::proto::PROTO_MIN),
            cert: Mutex::new(None),
            accept_from: Mutex::new(Vec::new()),
            send_window: Mutex::new((Instant::now(), 0)),
            tx,
            kill,
        });
        (reg, rx, killed)
    }

    /// Record the protocol version negotiated for this connection.
    pub fn set_proto(&self, proto: u32) {
        self.proto.store(proto, Ordering::Relaxed);
    }

    /// The protocol version negotiated for this connection.
    pub fn proto(&self) -> u32 {
        self.proto.load(Ordering::Relaxed)
    }

    /// The user certificate most recently issued to this registration.
    pub fn cert(&self) -> Option<UserCert> {
        self.cert.lock().expect("cert lock").clone()
    }

    pub fn set_cert(&self, cert: UserCert) {
        *self.cert.lock().expect("cert lock") = Some(cert);
    }

    /// Tenants allowed to send to this daemon (user ids, or `"*"`).
    pub fn set_accept_from(&self, list: Vec<String>) {
        *self.accept_from.lock().expect("accept lock") = list;
    }

    /// Whether the tenant `from_user` may send to this registration. A
    /// tenant always may send to itself; anyone else must be opted in.
    pub fn accepts(&self, from_user: &str) -> bool {
        from_user == self.user_id
            || self.accept_from.lock().expect("accept lock").iter().any(|a| a == "*" || a == from_user)
    }

    /// Count one `send`; false when over [`SEND_LIMIT`] in the current window.
    pub fn allow_send(&self) -> bool {
        self.allow_send_at(Instant::now())
    }

    pub(crate) fn allow_send_at(&self, now: Instant) -> bool {
        let mut w = self.send_window.lock().expect("send window lock");
        if now.duration_since(w.0) >= SEND_WINDOW {
            *w = (now, 0);
        }
        if w.1 >= SEND_LIMIT {
            return false;
        }
        w.1 += 1;
        true
    }

    /// Queue a `deliver` frame without waiting. The last [`RESERVED_SLOTS`]
    /// are kept for verdict requests; a full queue drops the frame (the
    /// newest) and counts it: a slow daemon must not stall the mesh.
    pub fn try_queue(&self, frame: Frame) -> Result<(), QueueError> {
        if self.tx.capacity() <= RESERVED_SLOTS {
            self.counters.dropped_full.fetch_add(1, Ordering::Relaxed);
            return Err(QueueError::Full);
        }
        self.try_queue_priority(frame)
    }

    /// Queue a frame that may use the reserved slots (verdict requests).
    pub fn try_queue_priority(&self, frame: Frame) -> Result<(), QueueError> {
        match self.tx.try_send(frame) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.counters.dropped_full.fetch_add(1, Ordering::Relaxed);
                Err(QueueError::Full)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Err(QueueError::Closed),
        }
    }

    /// Ask the connection task to close, with a reason sent to the daemon.
    pub fn kill(&self, reason: impl Into<String>) {
        let _ = self.kill.send(Some(reason.into()));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    Full,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterError {
    /// The user id already has a live registration (holder pid for the error).
    InUse { holder_pid: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeMiss {
    /// No registration for this user id.
    Unknown,
    /// The user is registered but does not own this project.
    UnknownProject,
}

#[derive(Default)]
struct Inner {
    by_user: HashMap<String, Arc<Registration>>,
    /// project id -> owning user id
    projects: HashMap<String, String>,
    /// Current project key claimed by the live user daemon with a verified
    /// user-key signature. Leaf scope authorization requires this entry.
    project_keys: HashMap<String, [u8; 32]>,
    /// prefix -> owning user id
    prefixes: HashMap<String, String>,
}

/// All live registrations.
#[derive(Default)]
pub struct Registry {
    inner: Mutex<Inner>,
}

/// What `register` accepted, in the shape of `register_ack`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub addresses: Vec<String>,
    pub topic_prefixes: Vec<String>,
    pub rejected: Vec<Rejected>,
}

fn valid_prefix(p: &str) -> Result<(), &'static str> {
    if p.is_empty() {
        return Err("empty prefix would claim every topic");
    }
    if p.len() > MAX_PREFIX_LEN {
        return Err("prefix too long");
    }
    if p.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("prefix contains whitespace or control characters");
    }
    Ok(())
}

/// 26 character Crockford ULID (same rule as `weft://` project addresses).
pub(crate) fn is_ulid(s: &str) -> bool {
    const ALPHABET: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    s.len() == 26 && s.chars().all(|c| ALPHABET.contains(c)) && s.as_bytes()[0] <= b'7'
}

fn overlaps(a: &str, b: &str) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

impl Inner {
    fn claim_project(&mut self, user: &str, project: &str) -> Result<(), &'static str> {
        if !is_ulid(project) {
            return Err("not a 26 character ULID");
        }
        match self.projects.get(project) {
            Some(o) if o != user => return Err("project is owned by another user"),
            Some(_) => return Ok(()),
            None => {}
        }
        if self.projects.values().filter(|o| *o == user).count() >= MAX_PROJECTS_PER_USER {
            return Err("too many projects");
        }
        self.projects.insert(project.to_string(), user.to_string());
        Ok(())
    }

    fn claim_prefix(&mut self, user: &str, prefix: &str) -> Result<(), &'static str> {
        valid_prefix(prefix)?;
        if clawft_mesh_local::proto::overlaps_reserved(prefix) {
            return Err("prefix overlaps a topic reserved for the cluster owner's daemon");
        }
        if self.prefixes.iter().any(|(p, o)| o != user && overlaps(p, prefix)) {
            return Err("prefix overlaps one claimed by another user");
        }
        if self.prefixes.get(prefix).is_some_and(|o| o == user) {
            return Ok(());
        }
        if self.prefixes.values().filter(|o| *o == user).count() >= MAX_PREFIXES_PER_USER {
            return Err("too many prefixes");
        }
        self.prefixes.insert(prefix.to_string(), user.to_string());
        Ok(())
    }
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("registry lock")
    }

    /// Insert `reg` with its requested projects and prefixes. Atomic: the
    /// in-use check and every claim happen under one lock.
    pub fn register(
        &self,
        reg: &Arc<Registration>,
        projects: &[String],
        prefixes: &[String],
    ) -> Result<Outcome, RegisterError> {
        let mut g = self.lock();
        if let Some(existing) = g.by_user.get(&reg.user_id) {
            return Err(RegisterError::InUse { holder_pid: existing.pid });
        }
        let mut out = Outcome::default();
        for p in projects {
            match g.claim_project(&reg.user_id, p) {
                Ok(()) => out.addresses.push(p.clone()),
                Err(why) => out.rejected.push(Rejected { what: format!("project {p}"), reason: why.into() }),
            }
        }
        for p in prefixes {
            match g.claim_prefix(&reg.user_id, p) {
                Ok(()) => out.topic_prefixes.push(p.clone()),
                Err(why) => out.rejected.push(Rejected { what: format!("prefix {p}"), reason: why.into() }),
            }
        }
        g.by_user.insert(reg.user_id.clone(), Arc::clone(reg));
        Ok(out)
    }

    /// Remove the registration if `conn_id` still owns it, with everything it
    /// claimed. A stale connection closing late cannot evict its successor.
    pub fn unregister(&self, user_id: &str, conn_id: u64) -> bool {
        let mut g = self.lock();
        if g.by_user.get(user_id).is_none_or(|r| r.conn_id != conn_id) {
            return false;
        }
        g.by_user.remove(user_id);
        g.projects.retain(|_, o| o != user_id);
        let active_projects: HashSet<String> = g.projects.keys().cloned().collect();
        g.project_keys.retain(|project, _| active_projects.contains(project));
        g.prefixes.retain(|_, o| o != user_id);
        true
    }

    pub fn add_project(&self, user: &str, project: &str) -> Result<(), &'static str> {
        self.lock().claim_project(user, project)
    }

    pub fn add_certified_project(&self, user: &str, project: &str, key: [u8; 32]) -> Result<(), &'static str> {
        let mut g = self.lock();
        g.claim_project(user, project)?;
        g.project_keys.insert(project.to_owned(), key);
        Ok(())
    }

    pub fn current_project_key(&self, user: &str, project: &str) -> Option<[u8; 32]> {
        let g = self.lock();
        (g.projects.get(project).is_some_and(|owner| owner == user))
            .then(|| g.project_keys.get(project).copied()).flatten()
    }

    pub fn remove_project(&self, user: &str, project: &str) -> bool {
        let mut g = self.lock();
        if g.projects.get(project).is_some_and(|o| o == user) {
            g.projects.remove(project);
            g.project_keys.remove(project);
            return true;
        }
        false
    }

    pub fn add_prefix(&self, user: &str, prefix: &str) -> Result<(), &'static str> {
        self.lock().claim_prefix(user, prefix)
    }

    pub fn remove_prefix(&self, user: &str, prefix: &str) -> bool {
        let mut g = self.lock();
        if g.prefixes.get(prefix).is_some_and(|o| o == user) {
            g.prefixes.remove(prefix);
            return true;
        }
        false
    }

    pub fn get(&self, user_id: &str) -> Option<Arc<Registration>> {
        self.lock().by_user.get(user_id).cloned()
    }

    /// The registration owning `user_id` (and `project`, when given).
    pub fn lookup_scope(
        &self,
        user_id: &str,
        project: Option<&str>,
    ) -> Result<Arc<Registration>, ScopeMiss> {
        let g = self.lock();
        let reg = g.by_user.get(user_id).ok_or(ScopeMiss::Unknown)?;
        if let Some(p) = project
            && g.projects.get(p).is_none_or(|o| o != user_id)
        {
            return Err(ScopeMiss::UnknownProject);
        }
        Ok(Arc::clone(reg))
    }

    /// The registration owning the longest prefix of `topic`.
    pub fn longest_prefix(&self, topic: &str) -> Option<Arc<Registration>> {
        let g = self.lock();
        g.prefixes
            .iter()
            .filter(|(p, _)| topic.starts_with(p.as_str()))
            .max_by_key(|(p, _)| p.len())
            .and_then(|(_, u)| g.by_user.get(u).cloned())
    }

    /// The only registration, when exactly one exists.
    pub fn sole(&self) -> Option<Arc<Registration>> {
        let g = self.lock();
        (g.by_user.len() == 1).then(|| g.by_user.values().next().cloned()).flatten()
    }

    pub fn by_principal(&self, p: &Principal) -> Option<Arc<Registration>> {
        self.lock().by_user.values().find(|r| &r.principal == p).cloned()
    }

    pub fn all(&self) -> Vec<Arc<Registration>> {
        let mut v: Vec<_> = self.lock().by_user.values().cloned().collect();
        v.sort_by(|a, b| a.user_id.cmp(&b.user_id));
        v
    }

    pub fn len(&self) -> usize {
        self.lock().by_user.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Prefixes and projects a user currently owns (for status listings).
    pub fn owned(&self, user_id: &str) -> (Vec<String>, Vec<String>) {
        let g = self.lock();
        let mut projects: Vec<_> =
            g.projects.iter().filter(|(_, o)| *o == user_id).map(|(p, _)| p.clone()).collect();
        let mut prefixes: Vec<_> =
            g.prefixes.iter().filter(|(_, o)| *o == user_id).map(|(p, _)| p.clone()).collect();
        projects.sort();
        prefixes.sort();
        (projects, prefixes)
    }
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
