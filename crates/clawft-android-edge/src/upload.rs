//! HTTPS batch upload of a weft.capture.v1 session ZIP to splatd.

use crate::error::EdgeError;
use crate::pair_store::HostPair;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

/// Result of `POST /v1/jobs` with multipart field `session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct UploadResult {
    pub job_id: String,
}

/// Job status snapshot from `GET /v1/jobs/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct JobStatus {
    pub job_id: String,
    pub status: String,
    pub stage: String,
    /// Optional SOG / artifact URL if host embeds one; else empty.
    pub sog_url: String,
    /// Raw JSON body for debugging / progressive UI.
    pub raw_json: String,
}

/// Upload a session ZIP (must contain frames/ + poses.jsonl per capture-protocol-v1).
pub fn upload_session_zip(pair: &HostPair, zip_path: &Path) -> Result<UploadResult, EdgeError> {
    if !zip_path.is_file() {
        return Err(EdgeError::upload(format!(
            "zip not found: {}",
            zip_path.display()
        )));
    }
    let mut file = File::open(zip_path).map_err(EdgeError::io)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(EdgeError::io)?;

    let file_name = zip_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("session.zip")
        .to_string();

    let part = reqwest::blocking::multipart::Part::bytes(bytes)
        .file_name(file_name)
        .mime_str("application/zip")
        .map_err(|e| EdgeError::upload(e.to_string()))?;

    let form = reqwest::blocking::multipart::Form::new().part("session", part);

    let client = build_client()?;
    let mut req = client.post(pair.jobs_url()).multipart(form);
    if !pair.token.is_empty() {
        req = req.bearer_auth(&pair.token);
    }

    let resp = req.send().map_err(|e| EdgeError::upload(e.to_string()))?;
    let status = resp.status();
    let body = resp.text().map_err(|e| EdgeError::upload(e.to_string()))?;
    if !(status.as_u16() == 202 || status.is_success()) {
        return Err(EdgeError::upload(format!(
            "POST /v1/jobs HTTP {status}: {body}"
        )));
    }
    let v: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| EdgeError::upload(format!("json: {e}")))?;
    let job_id = v
        .get("job_id")
        .and_then(|x| x.as_str())
        .ok_or_else(|| EdgeError::upload("response missing job_id"))?
        .to_string();
    Ok(UploadResult { job_id })
}

/// Poll job status; extracts a best-effort SOG artifact URL when present.
pub fn get_job_status(pair: &HostPair, job_id: &str) -> Result<JobStatus, EdgeError> {
    if job_id.is_empty() {
        return Err(EdgeError::invalid("empty job_id"));
    }
    let client = build_client()?;
    let mut req = client.get(pair.job_url(job_id));
    if !pair.token.is_empty() {
        req = req.bearer_auth(&pair.token);
    }
    let resp = req.send().map_err(|e| EdgeError::upload(e.to_string()))?;
    let status_code = resp.status();
    let body = resp.text().map_err(|e| EdgeError::upload(e.to_string()))?;
    if !status_code.is_success() {
        return Err(EdgeError::upload(format!(
            "GET /v1/jobs/{job_id} HTTP {status_code}: {body}"
        )));
    }
    let v: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| EdgeError::upload(format!("json: {e}")))?;

    let status = v
        .get("status")
        .and_then(|x| x.as_str())
        .unwrap_or("unknown")
        .to_string();
    let stage = v
        .get("stage")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let sog_url = extract_sog_url(pair, &v, job_id);

    Ok(JobStatus {
        job_id: job_id.to_string(),
        status,
        stage,
        sog_url,
        raw_json: body,
    })
}

/// Probe host health (`GET /healthz`).
pub fn health_check(pair: &HostPair) -> Result<String, EdgeError> {
    let client = build_client()?;
    let mut req = client.get(pair.healthz_url());
    if !pair.token.is_empty() {
        req = req.bearer_auth(&pair.token);
    }
    let resp = req.send().map_err(|e| EdgeError::upload(e.to_string()))?;
    let code = resp.status();
    let body = resp.text().map_err(|e| EdgeError::upload(e.to_string()))?;
    if !code.is_success() {
        return Err(EdgeError::upload(format!("healthz HTTP {code}: {body}")));
    }
    Ok(body)
}

fn extract_sog_url(pair: &HostPair, v: &serde_json::Value, job_id: &str) -> String {
    // Prefer explicit fields if host provides them.
    for key in ["sog_url", "result_url", "viewer_url"] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str())
            && !s.is_empty() {
                return s.to_string();
            }
    }
    // splatd serves artifacts at /v1/jobs/{id}/artifacts/{name}
    if let Some(arts) = v.get("artifacts").and_then(|a| a.as_object()) {
        for name in ["scene.sog", "output.sog", "sog", "result.sog"] {
            if arts.contains_key(name) {
                return format!(
                    "{}/v1/jobs/{job_id}/artifacts/{name}",
                    pair.base_url.trim_end_matches('/')
                );
            }
        }
        // any .sog key
        for (k, _) in arts {
            if k.ends_with(".sog") {
                return format!(
                    "{}/v1/jobs/{job_id}/artifacts/{k}",
                    pair.base_url.trim_end_matches('/')
                );
            }
        }
    }
    // Convention fallback when job is done — viewer can still 404 until artifact exists.
    if v.get("status").and_then(|s| s.as_str()) == Some("done")
        || v.get("status").and_then(|s| s.as_str()) == Some("completed")
    {
        return format!(
            "{}/v1/jobs/{job_id}/artifacts/scene.sog",
            pair.base_url.trim_end_matches('/')
        );
    }
    String::new()
}

fn build_client() -> Result<reqwest::blocking::Client, EdgeError> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(600))
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| EdgeError::upload(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pair_store::HostPair;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use tempfile::NamedTempFile;

    #[test]
    fn upload_and_poll_against_stub() {
        let job_id = "job-test-001".to_string();
        let state = Arc::new(Mutex::new(job_id.clone()));

        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let state_clone = state.clone();
        let handle = thread::spawn(move || {
            // One POST + one GET
            for _ in 0..2 {
                let mut req = server.recv().unwrap();
                let url = req.url().to_string();
                let method = req.method().as_str().to_string();
                if method == "POST" && url.starts_with("/v1/jobs") {
                    let mut body = String::new();
                    let _ = req.as_reader().read_to_string(&mut body);
                    assert!(body.contains("session") || !body.is_empty());
                    let jid = state_clone.lock().unwrap().clone();
                    let resp = tiny_http::Response::from_string(format!(r#"{{"job_id":"{jid}"}}"#))
                        .with_status_code(202);
                    req.respond(resp).unwrap();
                } else if method == "GET" && url.contains("/v1/jobs/") {
                    let jid = state_clone.lock().unwrap().clone();
                    let resp = tiny_http::Response::from_string(format!(
                        r#"{{"job_id":"{jid}","status":"running","stage":"sfm","artifacts":{{}}}}"#
                    ));
                    req.respond(resp).unwrap();
                } else {
                    let resp = tiny_http::Response::from_string("nope").with_status_code(404);
                    req.respond(resp).unwrap();
                }
            }
        });

        let mut zip = NamedTempFile::new().unwrap();
        zip.write_all(b"PK\x03\x04fake-zip-content").unwrap();
        zip.flush().unwrap();

        let pair = HostPair {
            base_url: format!("http://127.0.0.1:{port}"),
            token: String::new(),
            host_node_id: None,
            pair_version: 1,
            paired_at_ms: 0,
        };

        let up = upload_session_zip(&pair, zip.path()).unwrap();
        assert_eq!(up.job_id, job_id);
        let st = get_job_status(&pair, &up.job_id).unwrap();
        assert_eq!(st.status, "running");
        assert_eq!(st.stage, "sfm");

        handle.join().unwrap();
    }
}
