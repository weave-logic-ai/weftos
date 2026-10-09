//! A reusable signed connection from the controller to one known target for
//! read-only node-admin calls (ADR-108 P3b: the chunk reads of a project
//! fetch). [`PlacementControlPlane::call`] opens and closes a connection per
//! request; a bulk transfer of thousands of chunks wants the Noise handshake
//! and TCP connect paid once.
//!
//! What stays per request: a fresh [`CtlRequest`] with its own nonce and
//! expiry, the signature, the target's replay guard and controller check, and
//! the signed response bound to the request nonce and the pinned key. A
//! session therefore carries no trust of its own: it is the same wire with the
//! connection kept open. Only non-mutating node-admin methods may travel on
//! it, so no decision id or chain record is involved; the caller chains what
//! the calls add up to.
//!
//! A session is one request at a time (the wire correlates one response per
//! connection). Callers that want several chunks in flight open several
//! sessions.

use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use serde_json::Value;

use super::msg::{CtlOutcome, CtlRequest, method, verify_response};
use super::plane::{CallFailure, PlacementControlPlane, PlaneError, now_ms};
use super::session::CtlConnection;
use super::transport::CtlConnector;

/// An open connection to one target, signing as this controller.
pub struct CtlSession {
    local: String,
    key: SigningKey,
    target: String,
    addr: String,
    pk: [u8; 32],
    ttl_ms: u64,
    timeout: Duration,
    connector: Arc<dyn CtlConnector>,
    conn: Option<CtlConnection>,
    calls: u64,
}

impl PlacementControlPlane {
    /// Connect to the known node `node_id` for repeated read-only node-admin
    /// calls. The connection is opened now; the first call needs no second
    /// handshake.
    pub async fn open_session(&self, node_id: &str) -> Result<CtlSession, PlaneError> {
        let (t, pk) = self.target(node_id)?;
        let stream = self
            .connector
            .connect(&t.addr)
            .await
            .map_err(|e| CallFailure::Unreachable(e.to_string()))?;
        Ok(CtlSession {
            local: self.node_id.clone(),
            key: self.key.clone(),
            target: node_id.to_owned(),
            addr: t.addr,
            pk,
            ttl_ms: self.cfg.request_ttl_ms,
            timeout: self.cfg.call_timeout,
            connector: self.connector.clone(),
            conn: Some(CtlConnection::new(stream, self.node_id.clone())),
            calls: 0,
        })
    }
}

impl CtlSession {
    /// Per-call timeout (default: the plane's read-call timeout). A bulk
    /// chunk read over a slow link may want more.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The target node id.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Calls answered on this session so far.
    pub fn calls(&self) -> u64 {
        self.calls
    }

    /// One signed call. The connection is reopened before sending if the
    /// previous call lost it (an idle close); never after a request went out,
    /// so no request is ever sent twice.
    pub async fn call(&mut self, m: &str, body: Value) -> Result<Value, PlaneError> {
        self.call_raw(m, body).await.map(|(v, _)| v)
    }

    /// [`Self::call`], also returning the raw frame the target sent after the
    /// response when the body asked for one. The frame is unsigned: the
    /// caller checks it against the hash in the signed result.
    pub async fn call_raw(&mut self, m: &str, body: Value) -> Result<(Value, Option<Vec<u8>>), PlaneError> {
        if !method::is_node_admin(m) || method::mutates(m) {
            return Err(PlaneError::Invalid(format!("{m} is not a read-only node-admin method")));
        }
        if self.conn.is_none() {
            let stream = self
                .connector
                .connect(&self.addr)
                .await
                .map_err(|e| CallFailure::Unreachable(e.to_string()))?;
            self.conn = Some(CtlConnection::new(stream, self.local.clone()));
        }
        let req = CtlRequest::new(&self.key, m, &self.target, now_ms(), self.ttl_ms, None, body);
        let signed = req.sign(&self.key).map_err(CallFailure::Refused)?;
        let conn = self.conn.as_mut().expect("connected above");
        let (resp_signed, raw) = match conn.call_raw(&self.target, m, &signed, None, self.timeout).await {
            Ok(s) => s,
            Err(e) => {
                self.conn = None;
                return Err(CallFailure::Unreachable(e.to_string()).into());
            }
        };
        let (resp, _) = verify_response(&resp_signed, &req, Some(&self.pk))
            .map_err(|e| CallFailure::Unreachable(format!("bad response: {e}")))?;
        self.calls += 1;
        match resp.outcome {
            CtlOutcome::Ok { result } => Ok((result, raw)),
            CtlOutcome::Refused { refusal } => Err(CallFailure::Refused(refusal).into()),
        }
    }

    /// Close the connection.
    pub async fn close(mut self) {
        if let Some(c) = self.conn.take() {
            c.close().await;
        }
    }
}
