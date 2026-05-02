//! App lifecycle types — the data shapes the desktop compositor and
//! sibling crates consume when launching an app.
//!
//! Implementation status:
//!
//! * [`SessionConfig`], [`AppLaunchRequest`], [`AppLaunchResult`],
//!   [`LaunchError`] — stable for M1.5.
//! * [`governance::Gate`] — trait only. [`governance::NoopGate`] is
//!   always-grant and appropriate for tests / dev. [`governance::StrictGate`]
//!   enforces the narrow slice of ADR-015 that doesn't need the full
//!   ADR-012 capture governance machinery. Real governance is M1.6+.
//!
//! None of this actually *launches* anything — that's the
//! `clawft-gui-egui::shell` compositor's job. The types here are what
//! gets handed to it.

use serde::{Deserialize, Serialize};

use crate::manifest::{AppManifest, Input, Mode};

/// Session-level config: the `(mode, input)` pair a caller wants.
///
/// Fixed at startup per Session 10 §2. The compositor intersects this
/// with the manifest's `supported_modes` / `supported_inputs`; a
/// mismatch is a hard refusal (no best-effort downgrade, ADR-015
/// §Launch step 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionConfig {
    pub mode: Mode,
    pub input: Input,
}

/// A request to launch an installed app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppLaunchRequest {
    /// `app://...` IRI, resolved through the registry.
    pub app_id: String,
    pub session: SessionConfig,
}

/// The verdict from a [`governance::Gate`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AppLaunchResult {
    /// Launch approved; caller may instantiate these surfaces.
    Granted {
        /// The surface ids the compositor should instantiate. For
        /// `single-app` sessions this is always one element; for
        /// desktop / ide sessions it's the manifest's full list.
        surface_ids: Vec<String>,
    },
    /// Launch refused with a human-readable reason.
    Denied { reason: String },
}

/// Hard failures before governance gets a say. These are structural —
/// the manifest is known but doesn't admit this launch at all.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum LaunchError {
    #[error("no installed app with id `{0}`")]
    UnknownApp(String),
    #[error("app `{app_id}` does not declare mode `{mode:?}` in supported_modes")]
    ModeUnsupported { app_id: String, mode: Mode },
    #[error("app `{app_id}` does not declare input `{input:?}` in supported_inputs")]
    InputUnsupported { app_id: String, input: Input },
    #[error("governance denied launch of `{app_id}`: {reason}")]
    GovernanceDenied { app_id: String, reason: String },
}

/// Pluggable governance for launch-time authorisation.
///
/// This is a placeholder for M1.5: the real gate (ADR-012) inspects
/// capture-channel permissions, per-goal consent (ADR-008), and the
/// per-frame `affordance ∩ permit` intersection (ADR-006 rule 2).
/// None of that is implemented here. What's here is enough to let
/// sibling crates wire a pluggable trait object and substitute the
/// real gate later without refactor.
pub mod governance {
    use crate::manifest::AppManifest;

    use super::{AppLaunchRequest, AppLaunchResult};

    /// Runtime authorisation decision for a launch request.
    pub trait Gate: std::fmt::Debug + Send + Sync {
        /// Return [`AppLaunchResult::Granted`] or
        /// [`AppLaunchResult::Denied`]. Implementations may consult
        /// the manifest and request together — *structural* launch
        /// constraints (mode / input intersection) are already
        /// validated by the caller before this is invoked.
        fn authorize_launch(
            &self,
            req: &AppLaunchRequest,
            manifest: &AppManifest,
        ) -> AppLaunchResult;
    }

    /// Always grants; useful for tests and M1.5 dev loops.
    #[derive(Debug, Clone, Copy, Default)]
    pub struct NoopGate;

    impl Gate for NoopGate {
        fn authorize_launch(
            &self,
            _req: &AppLaunchRequest,
            manifest: &AppManifest,
        ) -> AppLaunchResult {
            AppLaunchResult::Granted {
                surface_ids: manifest
                    .surfaces
                    .iter()
                    .map(|s| s.as_str().to_string())
                    .collect(),
            }
        }
    }

    /// Enforces a thin slice of ADR-015 §Launch: a manifest that
    /// requests any capture channel stronger than `fs:*` / `net:*`
    /// (i.e. `camera`, `mic`, `screen`) must be denied until the full
    /// ADR-012 gate is wired. Everything else is granted.
    ///
    /// This is intentionally narrow — enough to exercise the
    /// "sometimes grants, sometimes denies" code path in sibling
    /// crates' tests without pretending to implement real governance.
    #[derive(Debug, Clone, Copy, Default)]
    pub struct StrictGate;

    impl Gate for StrictGate {
        fn authorize_launch(
            &self,
            _req: &AppLaunchRequest,
            manifest: &AppManifest,
        ) -> AppLaunchResult {
            use crate::manifest::Permission;
            for perm in &manifest.permissions {
                if matches!(
                    perm,
                    Permission::Camera | Permission::Mic | Permission::Screen
                ) {
                    return AppLaunchResult::Denied {
                        reason: format!(
                            "capture channel `{}` requires ADR-012 \
                             governance (not implemented in M1.5)",
                            perm.to_token()
                        ),
                    };
                }
            }
            AppLaunchResult::Granted {
                surface_ids: manifest
                    .surfaces
                    .iter()
                    .map(|s| s.as_str().to_string())
                    .collect(),
            }
        }
    }
}

/// Shape-check a launch request against a manifest. Callers compose
/// this with a [`governance::Gate`] to do the full authorisation
/// dance; it's factored out so the structural / policy concerns stay
/// separate.
pub fn check_launch_shape(
    req: &AppLaunchRequest,
    manifest: &AppManifest,
) -> Result<(), LaunchError> {
    if !manifest.supported_modes.contains(&req.session.mode) {
        return Err(LaunchError::ModeUnsupported {
            app_id: req.app_id.clone(),
            mode: req.session.mode,
        });
    }
    if !manifest.supported_inputs.contains(&req.session.input) {
        return Err(LaunchError::InputUnsupported {
            app_id: req.app_id.clone(),
            input: req.session.input,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use semver::Version;

    use super::governance::{Gate, NoopGate, StrictGate};
    use super::*;
    use crate::manifest::{EntryPoint, Permission, SurfaceRef};

    fn mk_manifest() -> AppManifest {
        AppManifest {
            id: "app://example.lifecycle".to_string(),
            name: "Lifecycle".to_string(),
            version: Version::new(0, 1, 0),
            icon: None,
            supported_modes: vec![Mode::Desktop, Mode::Ide],
            supported_inputs: vec![Input::Pointer],
            entry_points: vec![EntryPoint::Cli {
                flag: "ex".to_string(),
            }],
            surfaces: vec![SurfaceRef::from("s.toml")],
            surface_states: None,
            subscriptions: vec![],
            influences: vec![],
            permissions: vec![],
            narration: None,
        }
    }

    fn req(mode: Mode, input: Input) -> AppLaunchRequest {
        AppLaunchRequest {
            app_id: "app://example.lifecycle".to_string(),
            session: SessionConfig { mode, input },
        }
    }

    #[test]
    fn shape_check_rejects_unsupported_mode() {
        let m = mk_manifest();
        let err =
            check_launch_shape(&req(Mode::SingleApp, Input::Pointer), &m)
                .unwrap_err();
        assert!(matches!(err, LaunchError::ModeUnsupported { .. }));
    }

    #[test]
    fn shape_check_rejects_unsupported_input() {
        let m = mk_manifest();
        let err =
            check_launch_shape(&req(Mode::Desktop, Input::Voice), &m)
                .unwrap_err();
        assert!(matches!(err, LaunchError::InputUnsupported { .. }));
    }

    #[test]
    fn noop_gate_grants_and_passes_surface_ids() {
        let m = mk_manifest();
        let r = NoopGate.authorize_launch(&req(Mode::Desktop, Input::Pointer), &m);
        match r {
            AppLaunchResult::Granted { surface_ids } => {
                assert_eq!(surface_ids, vec!["s.toml".to_string()]);
            }
            AppLaunchResult::Denied { .. } => panic!("NoopGate must grant"),
        }
    }

    #[test]
    fn strict_gate_denies_camera_capture() {
        let mut m = mk_manifest();
        m.permissions.push(Permission::Camera);
        let r =
            StrictGate.authorize_launch(&req(Mode::Desktop, Input::Pointer), &m);
        assert!(matches!(r, AppLaunchResult::Denied { .. }));
    }

    #[test]
    fn strict_gate_grants_fs_only_permissions() {
        let mut m = mk_manifest();
        m.permissions.push(Permission::FsPath("/tmp".to_string()));
        m.permissions.push(Permission::NetDomain("x.com".to_string()));
        let r =
            StrictGate.authorize_launch(&req(Mode::Desktop, Input::Pointer), &m);
        assert!(matches!(r, AppLaunchResult::Granted { .. }));
    }
}
