//! One connected session of the mesh service link (split out of
//! [`super`]; behaviour unchanged): the event/command/timer loop, certificate
//! renewal and journal anchoring.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clawft_mesh_local::proto::{Frame, Message};
use clawft_mesh_local::UserCert;
use clawft_mesh_local::{ClientError, MeshLocalClient};
use tokio::sync::{mpsc, watch};
use tracing::{debug, warn};

use super::{LinkDeps, SessionEnd, publish_cert, sync_chain_counts};
use crate::mesh_local_sink::{MeshSink, OutCmd};
use crate::mesh_local_verdict;

fn dead(e: &ClientError) -> bool {
    !matches!(e, ClientError::Server(_) | ClientError::Unexpected(_) | ClientError::Addr(_))
}

/// Bound on deliveries waiting for the router; past it new ones are dropped
/// and counted, so a slow inbox cannot stall anything else.
const DELIVER_QUEUE: usize = 256;
/// Bound on concurrent outbound sends.
const MAX_SENDS: usize = 64;

/// One connected session. The loop only waits on events, commands and timers;
/// deliveries, sends, verdicts and anchors run on their own tasks (verdicts
/// and sends concurrently, deliveries in order), so renewal, verdict replies
/// and forwarding never queue behind a slow inbox.
pub(super) async fn session(
    mut c: MeshLocalClient,
    sink: &Arc<MeshSink>,
    deps: &LinkDeps,
    out_rx: &mut mpsc::Receiver<OutCmd>,
    shutdown: &mut watch::Receiver<bool>,
    node_id: &str,
) -> SessionEnd {
    let mut events = c.take_events();
    let c = Arc::new(c);
    let mut tasks = tokio::task::JoinSet::new();
    let (lost_tx, mut lost_rx) = mpsc::unbounded_channel::<String>();
    let (deliver_tx, mut deliver_rx) = mpsc::channel::<clawft_mesh_local::proto::Deliver>(DELIVER_QUEUE);
    let worker_sink = sink.clone();
    tasks.spawn(async move {
        while let Some(d) = deliver_rx.recv().await {
            if let Err(why) = worker_sink.deliver(d).await {
                warn!(%why, "inbound mesh delivery failed");
            }
        }
    });
    let sends = Arc::new(tokio::sync::Semaphore::new(MAX_SENDS));
    let anchoring = Arc::new(AtomicBool::new(false));
    let last_anchor: Arc<std::sync::Mutex<Option<(u64, String)>>> = Arc::default();
    let mut dropped_deliveries = 0u64;
    let first_cert = c.cert().clone();
    let mut anchor_tick = tokio::time::interval(deps.timings.anchor_every);
    let mut renew_at = tokio::time::Instant::now() + renew_after(&first_cert);
    let end = loop {
        tokio::select! {
            _ = shutdown.changed() => break SessionEnd::Shutdown,
            Some(why) = lost_rx.recv() => break SessionEnd::Lost(why),
            ev = events.recv() => match ev {
                None => break SessionEnd::Lost("service closed the connection".into()),
                Some(Frame { msg: Message::Deliver(d), .. }) => {
                    if deliver_tx.try_send(d).is_err() {
                        dropped_deliveries += 1;
                        if dropped_deliveries.is_power_of_two() {
                            warn!(dropped_deliveries, "inbound mesh deliveries are backing up; dropping");
                        }
                    }
                }
                Some(Frame { id: Some(id), msg: Message::VerdictRequest(req) }) => {
                    let (c, gate, user) = (c.clone(), deps.gate.clone(), c.register_ack().user_id.clone());
                    tasks.spawn(async move {
                        let a = mesh_local_verdict::answer(gate.as_deref(), &user, &req);
                        let reply = Message::VerdictReply {
                            allow: a.allow,
                            ttl_s: a.ttl_s,
                            reason: a.reason,
                            rule_hash: a.rule_hash,
                        };
                        if let Err(e) = c.reply(id, reply).await {
                            debug!(error = %e, "verdict reply failed");
                        }
                    });
                }
                Some(other) => debug!(?other, "ignored mesh service event"),
            },
            Some(cmd) = out_rx.recv() => {
                let Ok(permit) = sends.clone().try_acquire_owned() else {
                    let _ = cmd.reply.send(Err("too many sends in flight (busy)".into()));
                    continue;
                };
                let (c, lost_tx) = (c.clone(), lost_tx.clone());
                tasks.spawn(async move {
                    let r = c.send(&cmd.dest, cmd.message).await;
                    if let Some(why) = r.as_ref().err().filter(|e| dead(e)).map(ToString::to_string) {
                        let _ = lost_tx.send(why);
                    }
                    let _ = cmd.reply.send(r.map(|_| ()).map_err(|e| e.to_string()));
                    drop(permit);
                });
            }
            () = tokio::time::sleep_until(renew_at) => match renew(&c).await {
                Ok(fresh) => {
                    publish_cert(&deps.state, &c, &fresh, "connected");
                    renew_at = tokio::time::Instant::now() + renew_after(&fresh);
                }
                Err(e) => break SessionEnd::Lost(format!("certificate renewal failed: {e}")),
            },
            _ = anchor_tick.tick() => {
                if !anchoring.swap(true, Ordering::AcqRel) {
                    let (c, chain, last, flag, lost_tx, node) = (
                        c.clone(), deps.chain.clone(), last_anchor.clone(), anchoring.clone(),
                        lost_tx.clone(), node_id.to_owned(),
                    );
                    tasks.spawn(async move {
                        match c.request(Message::JournalHead {}).await {
                            Ok(Message::JournalHeadReply { seq, hash, ts, .. }) => {
                                let mut l = last.lock().unwrap_or_else(|e| e.into_inner());
                                if l.as_ref() != Some(&(seq, hash.clone())) {
                                    chain.anchor(seq, &hash, &node, ts);
                                    *l = Some((seq, hash));
                                }
                            }
                            Ok(other) => debug!(?other, "unexpected journal.head reply"),
                            Err(e) if dead(&e) => { let _ = lost_tx.send(format!("journal.head: {e}")); }
                            Err(e) => debug!(error = %e, "journal.head refused"),
                        }
                        chain.flush();
                        flag.store(false, Ordering::Release);
                    });
                }
                deps.chain.flush();
                sync_chain_counts(deps);
            }
        }
    };
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    if let Ok(c) = Arc::try_unwrap(c)
        && matches!(end, SessionEnd::Shutdown)
    {
        c.close().await;
    }
    end
}

/// Ask for a fresh certificate and verify it against the machine key and our
/// user key (what `MeshLocalClient::renew` does, without needing `&mut`).
async fn renew(c: &MeshLocalClient) -> Result<UserCert, ClientError> {
    match c.request(Message::Renew {}).await? {
        Message::Cert { cert } => {
            cert.verify(&c.hello_ack().machine_pubkey, clawft_mesh_local::client::now_unix())?;
            if cert.user_pubkey != c.cert().user_pubkey {
                return Err(ClientError::Unexpected("renewed cert has a different user key".into()));
            }
            Ok(cert)
        }
        other => Err(ClientError::Unexpected(format!("{other:?}"))),
    }
}

/// Renew at half the certificate's lifetime (at least a second).
fn renew_after(cert: &UserCert) -> Duration {
    let life = cert.not_after.saturating_sub(cert.issued_at);
    Duration::from_secs((life / 2).max(1))
}
