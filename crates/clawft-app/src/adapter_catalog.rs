//! Minimal ontology-adapter capability catalogue for ADR-015 rule 6.
//!
//! ADR-015 §Validation rule 6 requires every declared
//! [`crate::manifest::Permission`] to have a corresponding adapter in
//! `subscriptions` that **consumes** that channel. Full host-side
//! introspection lives with ADR-017 (`OntologyAdapter::permissions` /
//! topic declarations). The standalone `clawft-adapter` crate has not
//! landed yet, so this module ships a **static catalogue** of known
//! adapters (ADR-017 reference set + in-tree `clawft-substrate`
//! adapters) that structural validation can query without taking a
//! substrate dependency.
//!
//! When a real adapter registry is available, prefer
//! [`AdapterCatalog::from_entries`] (or a future host-supplied
//! catalogue) over the builtin set.

use crate::manifest::Permission;

/// A capture / resource channel an adapter is known to consume.
///
/// Mirrors the ADR-012 permission grammar used by
/// [`crate::manifest::Permission`], with path/domain scoping for
/// `fs:` / `net:` entries. Empty path or domain lists mean the adapter
/// can consume **any** path / domain of that kind (parameterised
/// adapters such as `fs` and `deploy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterChannel {
    /// Camera capture channel.
    Camera,
    /// Microphone capture channel.
    Mic,
    /// Screen-capture channel.
    Screen,
    /// Filesystem read under zero or more path prefixes.
    ///
    /// Empty `path_prefixes` → any `fs:` permission is covered by a
    /// subscription to this adapter. Non-empty → the app's path must
    /// fall under at least one declared prefix (or equal it).
    Fs {
        /// Path prefixes this adapter reads (empty = wildcard).
        path_prefixes: &'static [&'static str],
    },
    /// Network egress to zero or more domains.
    ///
    /// Empty `domains` → any `net:` permission is covered.
    Net {
        /// Domains this adapter contacts (empty = wildcard).
        domains: &'static [&'static str],
    },
}

/// Static capability metadata for one known ontology adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterCapability {
    /// Stable adapter id (`"kernel"`, `"mic"`, `"git"`, …).
    pub id: &'static str,
    /// Topic path prefixes this adapter serves.
    ///
    /// A subscription topic matches when it equals a prefix, or starts
    /// with `{prefix}/` (or the prefix already ends with `/` and the
    /// topic starts with it).
    pub topic_prefixes: &'static [&'static str],
    /// Additional path-substring matchers (e.g. `"/sensor/mic"` for
    /// node-scoped mic topics `substrate/<node>/sensor/mic/...`).
    pub path_contains: &'static [&'static str],
    /// Channels this adapter consumes (ADR-017 `permissions()`).
    pub channels: &'static [AdapterChannel],
}

/// Lookup table used by ADR-015 rule 6.
///
/// Builtins come from [`AdapterCatalog::builtin`]. Callers with a host
/// registry (tests, future `clawft-adapter` bridge) can construct a
/// custom catalogue via [`AdapterCatalog::from_entries`].
#[derive(Debug, Clone)]
pub struct AdapterCatalog {
    entries: Vec<AdapterCapability>,
}

impl AdapterCatalog {
    /// ADR-017 reference adapters + in-tree `clawft-substrate` adapters.
    ///
    /// Keep this list aligned with substrate `PERMISSIONS` / `TOPICS`
    /// constants when those change. Residual until a live registry
    /// replaces the static set (WEFT-413).
    pub fn builtin() -> Self {
        Self::from_entries(BUILTIN.iter().copied())
    }

    /// Build a catalogue from an explicit entry list (tests / host
    /// override).
    pub fn from_entries(entries: impl IntoIterator<Item = AdapterCapability>) -> Self {
        Self {
            entries: entries.into_iter().collect(),
        }
    }

    /// Borrow the registered capabilities.
    pub fn entries(&self) -> &[AdapterCapability] {
        &self.entries
    }

    /// Adapters whose topic declarations match `topic`.
    pub fn adapters_for_topic(&self, topic: &str) -> impl Iterator<Item = &AdapterCapability> {
        self.entries
            .iter()
            .filter(move |cap| topic_served_by(cap, topic))
    }

    /// Whether any adapter serving one of `subscriptions` consumes
    /// `permission` (ADR-015 rule 6).
    pub fn permission_has_reader(&self, permission: &Permission, subscriptions: &[String]) -> bool {
        for topic in subscriptions {
            for cap in self.adapters_for_topic(topic) {
                if channels_cover(cap.channels, permission) {
                    return true;
                }
            }
        }
        false
    }
}

impl Default for AdapterCatalog {
    fn default() -> Self {
        Self::builtin()
    }
}

fn topic_served_by(cap: &AdapterCapability, topic: &str) -> bool {
    for prefix in cap.topic_prefixes {
        if topic_matches_prefix(topic, prefix) {
            return true;
        }
    }
    for needle in cap.path_contains {
        if topic.contains(needle) {
            return true;
        }
    }
    false
}

fn topic_matches_prefix(topic: &str, prefix: &str) -> bool {
    if prefix.ends_with('/') {
        topic.starts_with(prefix) || topic == prefix.trim_end_matches('/')
    } else {
        topic == prefix || topic.starts_with(&format!("{prefix}/"))
    }
}

fn channels_cover(channels: &[AdapterChannel], permission: &Permission) -> bool {
    channels.iter().any(|ch| channel_covers(ch, permission))
}

fn channel_covers(channel: &AdapterChannel, permission: &Permission) -> bool {
    match (channel, permission) {
        (AdapterChannel::Camera, Permission::Camera)
        | (AdapterChannel::Mic, Permission::Mic)
        | (AdapterChannel::Screen, Permission::Screen) => true,
        (AdapterChannel::Fs { path_prefixes }, Permission::FsPath(need)) => {
            if path_prefixes.is_empty() {
                true
            } else {
                path_prefixes
                    .iter()
                    .any(|have| fs_prefix_covers(have, need))
            }
        }
        (AdapterChannel::Net { domains }, Permission::NetDomain(need)) => {
            if domains.is_empty() {
                true
            } else {
                domains.contains(&need.as_str())
            }
        }
        _ => false,
    }
}

/// `have` covers `need` when equal or `have` is a directory prefix of
/// `need` (same discipline as
/// [`crate::lifecycle::governance::permission_covered`] for fs grants).
fn fs_prefix_covers(have: &str, need: &str) -> bool {
    if need == have {
        return true;
    }
    let sep = if have.ends_with('/') { "" } else { "/" };
    need.starts_with(&format!("{have}{sep}"))
}

/// Builtin catalogue — ADR-017 §5 reference set + substrate adapters.
///
/// Permissions match each adapter's `PERMISSIONS` constant where one
/// exists in-tree; reference-only adapters (git/gh/fs/…) follow the
/// ADR text until those crates land.
const BUILTIN: &[AdapterCapability] = &[
    // ── ADR-017 reference ──────────────────────────────────────────
    AdapterCapability {
        id: "kernel",
        topic_prefixes: &["substrate/kernel/"],
        path_contains: &[],
        // Kernel adapter `PERMISSIONS` is empty (daemon reach only).
        channels: &[],
    },
    AdapterCapability {
        id: "git",
        topic_prefixes: &["substrate/git/"],
        path_contains: &[],
        channels: &[AdapterChannel::Fs {
            path_prefixes: &["/workspace/.git"],
        }],
    },
    AdapterCapability {
        id: "gh",
        topic_prefixes: &["substrate/gh/"],
        path_contains: &[],
        channels: &[AdapterChannel::Net {
            domains: &["api.github.com"],
        }],
    },
    AdapterCapability {
        id: "workspace",
        topic_prefixes: &["substrate/editor/"],
        path_contains: &[],
        channels: &[],
    },
    AdapterCapability {
        id: "fs",
        topic_prefixes: &["substrate/fs/"],
        path_contains: &[],
        // Parameterised roots — any fs: permission has a reader once
        // the app subscribes to an fs topic.
        channels: &[AdapterChannel::Fs {
            path_prefixes: &[],
        }],
    },
    AdapterCapability {
        id: "lsp",
        topic_prefixes: &["substrate/lsp/"],
        path_contains: &[],
        channels: &[],
    },
    AdapterCapability {
        id: "deployment",
        topic_prefixes: &["substrate/deploy/"],
        path_contains: &[],
        channels: &[AdapterChannel::Net { domains: &[] }],
    },
    // ── In-tree clawft-substrate ───────────────────────────────────
    AdapterCapability {
        id: "mic",
        topic_prefixes: &[
            "substrate/sensor/mic",
            "substrate/meta/adapter/mic/",
        ],
        // Node-scoped: substrate/<node-id>/sensor/mic/{summary,rms,pcm_chunk}
        path_contains: &["/sensor/mic"],
        channels: &[AdapterChannel::Mic],
    },
    AdapterCapability {
        id: "network",
        topic_prefixes: &["substrate/network/"],
        path_contains: &[],
        channels: &[],
    },
    AdapterCapability {
        id: "bluetooth",
        topic_prefixes: &["substrate/bluetooth"],
        path_contains: &[],
        channels: &[],
    },
    AdapterCapability {
        id: "rfkill",
        topic_prefixes: &["substrate/sensor/rfkill"],
        path_contains: &["/sensor/rfkill"],
        channels: &[],
    },
    // Capture stubs (not yet shipped as substrate adapters) so rule 6
    // can reject orphan camera/screen declarations with a clear
    // subscription target once those topics exist.
    AdapterCapability {
        id: "camera",
        topic_prefixes: &["substrate/sensor/camera"],
        path_contains: &["/sensor/camera"],
        channels: &[AdapterChannel::Camera],
    },
    AdapterCapability {
        id: "screen",
        topic_prefixes: &["substrate/sensor/screen"],
        path_contains: &["/sensor/screen"],
        channels: &[AdapterChannel::Screen],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mic_subscription_covers_mic_permission() {
        let cat = AdapterCatalog::builtin();
        assert!(cat.permission_has_reader(
            &Permission::Mic,
            &["substrate/sensor/mic".into()]
        ));
        assert!(cat.permission_has_reader(
            &Permission::Mic,
            &["substrate/host-local/sensor/mic/rms".into()]
        ));
    }

    #[test]
    fn kernel_subscription_does_not_cover_mic() {
        let cat = AdapterCatalog::builtin();
        assert!(!cat.permission_has_reader(
            &Permission::Mic,
            &["substrate/kernel/status".into()]
        ));
    }

    #[test]
    fn fs_adapter_covers_any_path() {
        let cat = AdapterCatalog::builtin();
        assert!(cat.permission_has_reader(
            &Permission::FsPath("/var/log/weftos".into()),
            &["substrate/fs/tree".into()]
        ));
    }

    #[test]
    fn git_adapter_covers_only_git_prefix() {
        let cat = AdapterCatalog::builtin();
        assert!(cat.permission_has_reader(
            &Permission::FsPath("/workspace/.git".into()),
            &["substrate/git/status".into()]
        ));
        assert!(cat.permission_has_reader(
            &Permission::FsPath("/workspace/.git/objects".into()),
            &["substrate/git/status".into()]
        ));
        assert!(!cat.permission_has_reader(
            &Permission::FsPath("/var/log/weftos".into()),
            &["substrate/git/status".into()]
        ));
    }

    #[test]
    fn gh_adapter_covers_github_domain() {
        let cat = AdapterCatalog::builtin();
        assert!(cat.permission_has_reader(
            &Permission::NetDomain("api.github.com".into()),
            &["substrate/gh/issues".into()]
        ));
        assert!(!cat.permission_has_reader(
            &Permission::NetDomain("evil.example".into()),
            &["substrate/gh/issues".into()]
        ));
    }

    #[test]
    fn custom_catalog_overrides_builtin() {
        let cat = AdapterCatalog::from_entries([AdapterCapability {
            id: "custom-cam",
            topic_prefixes: &["app/custom/camera"],
            path_contains: &[],
            channels: &[AdapterChannel::Camera],
        }]);
        assert!(cat.permission_has_reader(
            &Permission::Camera,
            &["app/custom/camera".into()]
        ));
        // Builtin mic is absent from this catalogue.
        assert!(!cat.permission_has_reader(
            &Permission::Mic,
            &["substrate/sensor/mic".into()]
        ));
    }
}
