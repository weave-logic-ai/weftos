//! Probe orchestration and the probes shared by every OS.
//!
//! [`probe_capabilities`] runs every probe against a [`ProbeHost`] and
//! returns capabilities plus notes. It never assumes: a capability is
//! emitted only when a probe saw it (`probed`), was told about it by the
//! operator (`claimed`), or a harness measured it (`measured`).

use clawft_types::placement::{
    AttrValue, Capability, CapabilityId, NodeFacts, ProbeNote, Provenance,
};

use super::host::ProbeHost;
use super::{accel, linux, macos, runtimes};

/// Default TTL for self-probed facts (ten minutes).
pub const DEFAULT_FACTS_TTL_SECS: u64 = 600;

/// Longest accepted container image reference.
pub const MAX_IMAGE_REF_LEN: usize = 255;

/// True for a plausible local image reference: `[A-Za-z0-9][A-Za-z0-9._/:@-]*`.
/// Rejects a leading `-` (it would be read as an option by `docker`), spaces
/// and any shell or control character.
pub fn valid_image_ref(s: &str) -> bool {
    let mut chars = s.chars();
    s.len() <= MAX_IMAGE_REF_LEN
        && matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | ':' | '@' | '-'))
}

/// Remembers what the privileged emulation probe (`docker run --privileged`)
/// listed, so it runs once per `ttl` and not on every facts refresh.
#[derive(Debug)]
pub struct EmulationCache {
    ttl: std::time::Duration,
    slot: std::sync::Mutex<Option<(std::time::Instant, String, String)>>,
}

impl EmulationCache {
    /// Cache entries live `ttl`.
    pub fn new(ttl: std::time::Duration) -> Self {
        Self {
            ttl,
            slot: std::sync::Mutex::new(None),
        }
    }

    /// The remembered binfmt listing for `image`, if fresh.
    pub(crate) fn get(&self, image: &str) -> Option<String> {
        let g = self.slot.lock().unwrap_or_else(|p| p.into_inner());
        g.as_ref()
            .filter(|(at, img, _)| img == image && at.elapsed() < self.ttl)
            .map(|(_, _, l)| l.clone())
    }

    pub(crate) fn put(&self, image: &str, listing: &str) {
        *self.slot.lock().unwrap_or_else(|p| p.into_inner()) =
            Some((std::time::Instant::now(), image.to_string(), listing.to_string()));
    }
}

/// What the probe is told rather than what it sees.
#[derive(Debug, Clone)]
pub struct ProbeConfig {
    /// Local image used to list binfmt handlers inside a VM-backed container
    /// engine (OrbStack, Docker Desktop). Never pulled: skipped if absent.
    pub docker_probe_image: Option<String>,
    /// Operator-declared sensor feeds; always advertised as `claimed`.
    pub declared_feeds: Vec<Capability>,
    /// Harness / admission-probe results; only `measured` `perf.*` kept.
    pub measured: Vec<Capability>,
    /// Remembers the privileged emulation probe between runs (none: always run it).
    pub emulation_cache: Option<std::sync::Arc<EmulationCache>>,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            docker_probe_image: Some("alpine:3.20".into()),
            declared_feeds: Vec::new(),
            measured: Vec::new(),
            emulation_cache: None,
        }
    }
}

/// Probe output: capabilities plus notes explaining them.
#[derive(Debug, Clone, Default)]
pub struct Collected {
    /// Capabilities in emission order.
    pub caps: Vec<Capability>,
    /// Notes (sources, things not visible).
    pub notes: Vec<ProbeNote>,
}

impl Collected {
    /// Add a capability.
    pub fn push(&mut self, cap: Capability) {
        self.caps.push(cap);
    }
    /// Add a note.
    pub fn note(&mut self, probe: &str, note: impl Into<String>) {
        self.notes.push(ProbeNote::new(probe, note));
    }
    /// True if a capability with this id was emitted.
    pub fn has(&self, id: &str) -> bool {
        self.caps.iter().any(|c| c.id.as_str() == id)
    }
}

/// Build a capability, or `None` if the id is not valid (for example an
/// arch string from a tool that does not fit the id grammar).
pub fn cap(id: &str, provenance: Provenance) -> Option<Capability> {
    CapabilityId::new(id)
        .ok()
        .map(|id| Capability::new(id, provenance))
}

/// A list attribute of strings.
pub fn str_list<S: AsRef<str>>(items: &[S]) -> AttrValue {
    AttrValue::List(
        items
            .iter()
            .map(|s| AttrValue::Str(s.as_ref().into()))
            .collect(),
    )
}

/// Clamp a byte count into an `Int` attribute.
pub fn bytes_attr(n: u64) -> AttrValue {
    AttrValue::Int(i64::try_from(n).unwrap_or(i64::MAX))
}

/// Canonical arch name for ids and `arches_*` lists.
pub fn normalize_arch(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "aarch64" | "arm64" | "arm64/v8" => "aarch64".into(),
        "arm" | "armv7" | "armv7l" | "armhf" | "arm/v7" => "armv7".into(),
        "armv6" | "armv6l" | "arm/v6" => "armv6".into(),
        "x86_64" | "amd64" | "x64" | "amd64/v2" | "amd64/v3" => "x86_64".into(),
        "i386" | "i686" | "x86" | "386" => "i386".into(),
        "mips64el" | "mips64le" => "mips64le".into(),
        other => other
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect(),
    }
}

/// Parse `df -kP <path>` output: `(available bytes, mount point)`.
///
/// Parsed from the right so filesystem names with spaces do not shift the
/// columns: the capacity column is the one ending in `%`.
pub fn parse_df(out: &str) -> Option<(u64, String)> {
    let line = out.lines().nth(1)?;
    let toks: Vec<&str> = line.split_whitespace().collect();
    let pct = toks.iter().rposition(|t| t.ends_with('%'))?;
    let avail_kb: u64 = toks.get(pct.checked_sub(1)?)?.parse().ok()?;
    let mount = toks.get(pct + 1..)?.join(" ");
    if mount.is_empty() {
        return None;
    }
    Some((avail_kb.saturating_mul(1024), mount))
}

/// Run every probe. Order: cpu/os, memory, storage, runtimes,
/// accelerators, formats, feeds, measured.
pub fn probe_capabilities(host: &dyn ProbeHost, cfg: &ProbeConfig) -> Collected {
    let mut c = Collected::default();
    let arch = normalize_arch(&host.arch());
    match host.os().as_str() {
        "macos" => macos::probe(host, &arch, &mut c),
        "linux" => linux::probe(host, &arch, &mut c),
        other => {
            if let Some(cp) = cap(&format!("cpu.arch.{arch}"), Provenance::Probed) {
                c.push(cp);
            }
            c.note(
                "os",
                format!("no probes for os {other:?}; only the arch is advertised"),
            );
        }
    }
    runtimes::probe(host, &arch, cfg, &mut c);
    accel::probe(host, &mut c);
    derive_formats(&mut c);
    for feed in &cfg.declared_feeds {
        if feed.id.family() == "feed" {
            let mut f = feed.clone();
            f.provenance = Provenance::Claimed;
            c.push(f);
        }
    }
    if !cfg.declared_feeds.is_empty() {
        c.note(
            "feed",
            "sensor feeds are operator-declared (claimed), not probed",
        );
    }
    let before = c.caps.len();
    c.caps.extend(
        cfg.measured
            .iter()
            .filter(|m| m.id.family() == "perf" && m.provenance == Provenance::Measured)
            .cloned(),
    );
    if c.caps.len() > before {
        c.note(
            "perf",
            format!(
                "{} measured perf.* values from the conformance harness",
                c.caps.len() - before
            ),
        );
    }
    c
}

/// `format.*` ids for formats an advertised runtime or accelerator
/// handles, so a requirement can name the format directly (ADR-101).
fn derive_formats(c: &mut Collected) {
    // Format -> (sources, best source provenance). A format is only as
    // trustworthy as the best runtime/accelerator that handles it: a format
    // known only through the claimed ANE is itself claimed.
    let mut by_format: std::collections::BTreeMap<String, (Vec<String>, Provenance)> =
        Default::default();
    for cp in &c.caps {
        if !matches!(cp.id.family(), "runtime" | "accel") {
            continue;
        }
        if let Some(AttrValue::List(fmts)) = cp.attrs.get("formats") {
            for f in fmts {
                if let AttrValue::Str(f) = f {
                    let e = by_format
                        .entry(f.clone())
                        .or_insert_with(|| (Vec::new(), cp.provenance));
                    e.0.push(cp.id.to_string());
                    e.1 = e.1.max(cp.provenance);
                }
            }
        }
    }
    for (fmt, (via, prov)) in by_format {
        if let Some(fc) = cap(&format!("format.{fmt}"), prov) {
            c.push(fc.with_attr("via", str_list(&via)));
        }
    }
}

/// Probe this machine and wrap the result as unsigned [`NodeFacts`].
///
/// Capabilities and notes beyond the NodeFacts bounds are dropped (with a
/// note) rather than producing facts that would not validate.
pub fn build_facts(
    node_id: &str,
    issued_at: u64,
    ttl_secs: u64,
    seq: u64,
    mut collected: Collected,
) -> NodeFacts {
    use clawft_types::placement::node_facts::{MAX_FACTS_CAPABILITIES, MAX_PROBE_NOTES};
    let mut facts = NodeFacts::new(node_id, issued_at, ttl_secs, seq);
    collected
        .caps
        .retain(|c| c.validate().is_ok() && c.id.family() != "trust");
    if collected.caps.len() > MAX_FACTS_CAPABILITIES {
        collected.caps.truncate(MAX_FACTS_CAPABILITIES);
        collected.note("facts", "capability list truncated to the NodeFacts bound");
    }
    collected.notes.truncate(MAX_PROBE_NOTES);
    facts.capabilities = collected.caps;
    facts.notes = collected.notes;
    facts
}

/// Cheap live refresh of `base`: only free memory is re-read (no container
/// engine, no subprocess beyond `vm_stat` / `/proc/meminfo`). Capability
/// states are carried over from `base`; the caller owns busy tracking.
pub fn refresh_live(host: &dyn ProbeHost, base: &NodeFacts, now: u64) -> NodeFacts {
    let mut f = base.clone();
    f.issued_at = now;
    let free = match host.os().as_str() {
        "macos" => macos::live_free(host),
        "linux" => linux::live_free(host),
        _ => None,
    };
    if let Some(free) = free {
        for c in f
            .capabilities
            .iter_mut()
            .filter(|c| matches!(c.id.as_str(), "mem.unified" | "mem.system"))
        {
            let total = match c.attrs.get("total") {
                Some(AttrValue::Int(t)) if *t >= 0 => *t as u64,
                _ => u64::MAX,
            };
            c.attrs.insert("free".into(), bytes_attr(free.min(total)));
        }
    }
    f
}
