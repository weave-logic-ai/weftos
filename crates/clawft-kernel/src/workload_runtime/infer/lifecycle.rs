//! Start, stop, unload, status, restart and reconcile for [`InferRuntime`].

use std::time::{Duration, Instant};

use super::ollama::{self, LoadState};
use super::probe::{Health, ServerReport};
use super::runtime::{InferRuntime, Instance, Managed, ManagedPlan};
use super::spec::InferFlavor;
use crate::workload_runtime::evidence::RunEvidence;
use crate::workload_runtime::supervise::{LaunchSpec, ProcLimits, Supervised};
use crate::workload_runtime::types::{InstanceHandle, InstanceState, InstanceStatus, RuntimeError};

/// Output kept per stream from a model server (bytes).
const OUTPUT_LIMIT: usize = 1024 * 1024;
/// Grace given to a server when its instance is unloaded.
const UNLOAD_GRACE: Duration = Duration::from_secs(5);
/// Timeout of an Ollama load request (a large model can take minutes).
const OLLAMA_LOAD_TIMEOUT: Duration = Duration::from_secs(900);

/// What [`InferRuntime::reconcile`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reconcile {
    /// Running and answering, or not wanted running.
    Healthy,
    /// Not a supervised process (adopted, or Ollama).
    NotManaged,
    /// It died; the restart waits out its backoff.
    Backoff {
        /// Time left before the next restart is allowed.
        retry_in: Duration,
    },
    /// It was restarted.
    Restarted {
        /// Restarts so far, this one included.
        attempt: u32,
    },
    /// The restart budget is spent; the instance stays `Exited`.
    GaveUp {
        /// Restarts attempted.
        attempts: u32,
    },
}

fn unknown(h: &InstanceHandle) -> RuntimeError {
    RuntimeError::UnknownInstance(h.instance_id.clone())
}

fn evidence(rt: &InferRuntime, h: &InstanceHandle) -> RunEvidence {
    RunEvidence {
        runtime: crate::workload_runtime::types::WorkloadRuntime::id(rt).to_string(),
        instance_id: h.instance_id.clone(),
        ..RunEvidence::default()
    }
}

fn unlimited() -> ProcLimits {
    // Model servers map tens of GiB of weights and many files; the cog
    // limits do not apply. Large finite values: an explicit RLIM_INFINITY
    // is refused by some kernels when set as the hard limit.
    ProcLimits {
        mem_bytes: 1 << 46,
        cpu_secs: None,
        nofile: 4096,
        fsize_bytes: 1 << 44,
    }
}

fn spawn(rt: &InferRuntime, inst: &mut Instance) -> Result<(), RuntimeError> {
    let run_as = rt.managed_cfg().and_then(|m| m.run_as);
    let Some(Managed {
        plan: ManagedPlan::Process(l),
        proc,
        wanted,
        exited_at,
        ..
    }) = inst.managed.as_mut()
    else {
        return Err(RuntimeError::Unsupported("not a supervised process".into()));
    };
    let (ip, port) = (inst.spec.bind_ip()?, inst.spec.port()?);
    if super::launch::port_in_use(ip, port) {
        return Err(RuntimeError::Backend(format!(
            "{ip}:{port} is already taken by something else; not starting over it"
        )));
    }
    *proc = Some(Supervised::spawn(LaunchSpec {
        program: l.program.clone(),
        args: l.args.clone(),
        env: l.env.clone(),
        cwd: l.dir.clone(),
        limits: unlimited(),
        output_limit: OUTPUT_LIMIT,
        run_as,
    })?);
    *wanted = true;
    *exited_at = None;
    Ok(())
}

/// Collect a process that exited on its own, once.
async fn reap(mg: &mut Managed) -> Option<Option<i32>> {
    let exited = mg.proc.as_mut()?.try_exit()?;
    let ev = mg.proc.take()?.terminate(Duration::ZERO).await;
    mg.exited_at.get_or_insert_with(Instant::now);
    mg.last = Some(ev);
    Some(exited.code())
}

fn map_report(
    flavor: InferFlavor,
    model: Option<&str>,
    r: &ServerReport,
    alive: bool,
    adopted: bool,
    load: Option<&LoadState>,
) -> InstanceStatus {
    let degraded = |d: String| InstanceStatus {
        state: InstanceState::Degraded,
        exit_code: None,
        detail: Some(d),
    };
    match &r.health {
        Health::Loading => degraded("loading model".into()),
        Health::Unhealthy(w) => degraded(w.clone()),
        Health::Unreachable(w) if alive => degraded(format!("starting: not listening yet ({w})")),
        Health::Unreachable(w) => InstanceStatus {
            state: InstanceState::Exited,
            exit_code: None,
            detail: Some(if adopted {
                format!("unreachable ({w}); adopted servers are not restarted")
            } else {
                format!("unreachable ({w})")
            }),
        },
        Health::Up => match (flavor, model) {
            (InferFlavor::Ollama, Some(m)) => {
                if !r.lists(m) {
                    degraded(format!("model {m} is not pulled"))
                } else if r.resident(m) {
                    InstanceStatus {
                        detail: Some("model resident".into()),
                        ..InstanceStatus::of(InstanceState::Running)
                    }
                } else if let Some(LoadState::Failed(e)) = load {
                    degraded(format!("load failed: {e}"))
                } else if matches!(load, Some(LoadState::Loading)) {
                    degraded("loading model".into())
                } else if adopted {
                    InstanceStatus {
                        detail: Some("server up; model not resident".into()),
                        ..InstanceStatus::of(InstanceState::Running)
                    }
                } else {
                    InstanceStatus::of(InstanceState::Loaded)
                }
            }
            (_, Some(m)) if !r.models.is_empty() && !r.lists(m) => {
                degraded(format!("model {m} is not listed by the server"))
            }
            _ => InstanceStatus::of(InstanceState::Running),
        },
    }
}

impl InferRuntime {
    pub(super) async fn start_instance(&self, h: &InstanceHandle) -> Result<(), RuntimeError> {
        let mut g = self.instances.lock().await;
        let inst = g.get_mut(&h.instance_id).ok_or_else(|| unknown(h))?;
        let Some(mg) = inst.managed.as_mut() else {
            // Adopted: nothing to start; confirm the server answers.
            let (client, flavor) = (inst.client.clone(), inst.spec.runtime);
            drop(g);
            return match client.probe(flavor).await.health {
                Health::Unreachable(w) => Err(RuntimeError::Backend(format!(
                    "adopted server is not answering ({w}); WeftOS does not start adopted servers"
                ))),
                _ => Ok(()),
            };
        };
        match &mg.plan {
            ManagedPlan::Process(_) => {
                if mg.proc.as_mut().is_some_and(|p| p.try_exit().is_none()) {
                    return Err(RuntimeError::InvalidState("already running".into()));
                }
                mg.proc = None;
                spawn(self, inst)
            }
            ManagedPlan::Ollama { tag } => {
                if matches!(&mg.load, Some((s, _)) if matches!(*s.lock().unwrap_or_else(|e| e.into_inner()), LoadState::Loading))
                {
                    return Err(RuntimeError::InvalidState("already loading".into()));
                }
                let keep = self
                    .managed_cfg()
                    .map_or("5m".to_string(), |m| m.keep_alive.clone());
                let client = super::probe::ServerClient::new(
                    inst.client.base().to_string(),
                    OLLAMA_LOAD_TIMEOUT,
                )?;
                let state = std::sync::Arc::new(std::sync::Mutex::new(LoadState::Loading));
                let (s2, tag) = (state.clone(), tag.clone());
                let task = tokio::spawn(async move {
                    let r = ollama::load(&client, &tag, &keep).await;
                    *s2.lock().unwrap_or_else(|e| e.into_inner()) =
                        r.map_or_else(LoadState::Failed, |()| LoadState::Done);
                });
                mg.load = Some((state, task));
                mg.wanted = true;
                Ok(())
            }
        }
    }

    pub(super) async fn stop_instance(
        &self,
        h: &InstanceHandle,
        grace: Duration,
    ) -> Result<RunEvidence, RuntimeError> {
        let mut g = self.instances.lock().await;
        let inst = g.get_mut(&h.instance_id).ok_or_else(|| unknown(h))?;
        let Some(mg) = inst.managed.as_mut() else {
            return Err(RuntimeError::Unsupported(format!(
                "{} is adopted: WeftOS observes it and never stops it; unload forgets the registration",
                h.instance_id
            )));
        };
        mg.wanted = false;
        match &mg.plan {
            ManagedPlan::Process(_) => {
                let p = mg
                    .proc
                    .take()
                    .ok_or_else(|| RuntimeError::InvalidState("not running".into()))?;
                let mut ev = p.terminate(grace).await;
                ev.runtime = crate::workload_runtime::types::WorkloadRuntime::id(self).to_string();
                ev.instance_id = h.instance_id.clone();
                mg.exited_at = Some(Instant::now());
                mg.last = Some(ev.clone());
                Ok(ev)
            }
            ManagedPlan::Ollama { tag } => {
                if let Some((_, t)) = mg.load.take() {
                    t.abort();
                }
                let (client, tag) = (inst.client.clone(), tag.clone());
                drop(g);
                ollama::unload(&client, &tag)
                    .await
                    .map_err(RuntimeError::Backend)?;
                Ok(evidence(self, h))
            }
        }
    }

    pub(super) async fn unload_instance(&self, h: &InstanceHandle) -> Result<(), RuntimeError> {
        let mut inst = self
            .instances
            .lock()
            .await
            .remove(&h.instance_id)
            .ok_or_else(|| unknown(h))?;
        if let Some(mg) = inst.managed.as_mut() {
            // Only ever a process this adapter spawned itself.
            if let Some(p) = mg.proc.take() {
                p.terminate(UNLOAD_GRACE).await;
            }
            if let Some((_, t)) = mg.load.take() {
                t.abort();
            }
            match &mg.plan {
                ManagedPlan::Process(l) => {
                    let _ = std::fs::remove_dir_all(&l.dir);
                }
                ManagedPlan::Ollama { tag } => {
                    // Best effort: leave Ollama without our model resident.
                    let _ = ollama::unload(&inst.client, tag).await;
                }
            }
        }
        Ok(())
    }

    pub(super) async fn status_of(&self, h: &InstanceHandle) -> InstanceStatus {
        let (client, spec, model, alive, load) = {
            let mut g = self.instances.lock().await;
            let Some(inst) = g.get_mut(&h.instance_id) else {
                return InstanceStatus::of(InstanceState::Unknown);
            };
            let (client, spec) = (inst.client.clone(), inst.spec.clone());
            let Some(mg) = inst.managed.as_mut() else {
                let m = spec.model.clone();
                drop(g);
                let r = client.probe(spec.runtime).await;
                return map_report(spec.runtime, m.as_deref(), &r, false, true, None);
            };
            if let Some(code) = reap(mg).await {
                return InstanceStatus {
                    state: InstanceState::Exited,
                    exit_code: code,
                    detail: Some("server process exited".into()),
                };
            }
            let alive = mg.proc.is_some();
            if matches!(mg.plan, ManagedPlan::Process(_)) && !alive {
                let state = if mg.last.is_some() {
                    InstanceState::Exited
                } else {
                    InstanceState::Loaded
                };
                return InstanceStatus {
                    state,
                    exit_code: mg.last.as_ref().and_then(|e| e.exit_code),
                    detail: None,
                };
            }
            let load = mg
                .load
                .as_ref()
                .map(|(s, _)| s.lock().unwrap_or_else(|e| e.into_inner()).clone());
            let model = match &mg.plan {
                ManagedPlan::Ollama { tag } => tag.clone(),
                ManagedPlan::Process(_) => mg.model_name.clone(),
            };
            (client, spec, Some(model), alive, load)
        };
        let r = client.probe(spec.runtime).await;
        map_report(
            spec.runtime,
            model.as_deref(),
            &r,
            alive,
            false,
            load.as_ref(),
        )
    }

    /// Stop (when running) then start again.
    pub async fn restart(&self, h: &InstanceHandle, grace: Duration) -> Result<(), RuntimeError> {
        match self.stop_instance(h, grace).await {
            Ok(_) | Err(RuntimeError::InvalidState(_)) => {}
            Err(e) => return Err(e),
        }
        self.start_instance(h).await
    }

    /// Restart a managed server that died while it should be running,
    /// under the configured backoff and budget. Meant to be called on a
    /// timer; each call does at most one restart.
    pub async fn reconcile(&self, h: &InstanceHandle) -> Result<Reconcile, RuntimeError> {
        let Some(policy) = self.managed_cfg().map(|m| m.restart) else {
            return Ok(Reconcile::NotManaged);
        };
        let (client, flavor, healthy_check) = {
            let mut g = self.instances.lock().await;
            let inst = g.get_mut(&h.instance_id).ok_or_else(|| unknown(h))?;
            let (client, flavor) = (inst.client.clone(), inst.spec.runtime);
            let Some(mg) = inst.managed.as_mut() else {
                return Ok(Reconcile::NotManaged);
            };
            if !matches!(mg.plan, ManagedPlan::Process(_)) {
                return Ok(Reconcile::NotManaged);
            }
            if !mg.wanted {
                return Ok(Reconcile::Healthy);
            }
            reap(mg).await;
            if mg.proc.is_none() {
                if mg.restarts >= policy.max_restarts {
                    return Ok(Reconcile::GaveUp {
                        attempts: mg.restarts,
                    });
                }
                let wait = policy.delay(mg.restarts);
                let since = mg.exited_at.map_or(wait, |t| t.elapsed());
                if since < wait {
                    return Ok(Reconcile::Backoff {
                        retry_in: wait - since,
                    });
                }
                mg.restarts += 1;
                let attempt = mg.restarts;
                spawn(self, inst)?;
                return Ok(Reconcile::Restarted { attempt });
            }
            (client, flavor, true)
        };
        if healthy_check && client.probe(flavor).await.health == Health::Up {
            let mut g = self.instances.lock().await;
            if let Some(mg) = g.get_mut(&h.instance_id).and_then(|i| i.managed.as_mut()) {
                mg.restarts = 0;
            }
        }
        Ok(Reconcile::Healthy)
    }
}
