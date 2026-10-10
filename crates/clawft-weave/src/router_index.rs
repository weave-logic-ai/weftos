//! `/_weftos/` (ADR-116 §3, the "one query pane" of ADR-098): every route,
//! its project, the upstream's health, and that project's process-compose
//! state read from its own HTTP API (`GET /processes` on the port its
//! `ports.yaml` claims as `process-compose-http`). Read only; nothing is
//! started or stopped. JSON at `/_weftos/routes.json`; the same document
//! answers `weaver route list`.

use std::time::Duration;

use hyper::{Response, StatusCode};
use serde_json::{Value, json};

use crate::router_proxy::{BoxBody, esc, html_response, http_get, json_response};
use crate::router_routes::Route;
use crate::router_state::RouterHandle;

/// Most processes listed per project.
pub const MAX_PROCESSES: usize = 64;

/// Answer a request under `/_weftos`.
pub async fn respond(path: &str, h: &RouterHandle) -> Response<BoxBody> {
    match path {
        "/_weftos" | "/_weftos/" | "/_weftos/index.html" => html_response(StatusCode::OK, "Routes on this machine", &render_html(&snapshot(h).await)),
        "/_weftos/routes.json" => json_response(StatusCode::OK, &snapshot(h).await),
        _ => html_response(StatusCode::NOT_FOUND, "Not found", "<p><a href=\"/_weftos/\">Routes on this machine</a></p>"),
    }
}

/// Health of one upstream.
pub async fn probe_health(route: &Route, timeout: Duration) -> Value {
    let Some(path) = route.health.as_deref() else {
        return json!({ "state": "unknown", "detail": "no health path declared" });
    };
    match http_get(route.port, path, timeout).await {
        Ok((status, _)) if (200..400).contains(&status) => json!({ "state": "ok", "status": status }),
        Ok((status, _)) => json!({ "state": "down", "status": status, "detail": format!("{path} answered {status}") }),
        Err(e) => json!({ "state": "down", "detail": e }),
    }
}

/// Process state from a project's process-compose HTTP API.
pub async fn pc_state(pc_http: Option<u16>, timeout: Duration) -> Value {
    let Some(port) = pc_http else {
        return json!({ "state": "unknown", "detail": "no process-compose-http claim" });
    };
    match http_get(port, "/processes", timeout).await {
        Ok((200, body)) => match serde_json::from_slice::<Value>(&body) {
            Ok(v) => summarize_processes(&v, port),
            Err(e) => json!({ "state": "down", "port": port, "detail": format!("/processes is not JSON: {e}") }),
        },
        Ok((status, _)) => json!({ "state": "down", "port": port, "detail": format!("/processes answered {status}") }),
        Err(e) => json!({ "state": "down", "port": port, "detail": e }),
    }
}

/// Reduce `GET /processes` (`{"data":[{name,namespace,status,is_ready,...}]}`)
/// to counts and a short list.
pub fn summarize_processes(v: &Value, port: u16) -> Value {
    let list = v.get("data").and_then(Value::as_array).or_else(|| v.as_array()).cloned().unwrap_or_default();
    let mut running = 0usize;
    let procs: Vec<Value> = list
        .iter()
        .take(MAX_PROCESSES)
        .map(|p| {
            let status = p.get("status").and_then(Value::as_str).unwrap_or("-");
            if status.eq_ignore_ascii_case("running") {
                running += 1;
            }
            json!({
                "name": p.get("name").and_then(Value::as_str).unwrap_or("-"),
                "namespace": p.get("namespace").and_then(Value::as_str).unwrap_or(""),
                "status": status,
                "ready": p.get("is_ready").and_then(Value::as_str).unwrap_or(""),
                "restarts": p.get("restarts").and_then(Value::as_u64).unwrap_or(0),
            })
        })
        .collect();
    json!({ "state": "ok", "port": port, "running": running, "total": list.len(), "processes": procs })
}

/// The whole index as JSON: routes with live health, projects with their
/// process-compose state, refused declarations, and router facts.
pub async fn snapshot(h: &RouterHandle) -> Value {
    let table = h.table();
    let timeout = h.health_timeout();
    let health = join_all(table.routes.iter().cloned().map(|r| async move { probe_health(&r, timeout).await })).await;
    let pc = join_all(table.projects.iter().map(|p| p.pc_http).map(|port| async move { pc_state(port, timeout).await })).await;
    let routes: Vec<Value> = table
        .routes
        .iter()
        .zip(health)
        .map(|(r, hv)| {
            let mut v = serde_json::to_value(r).unwrap_or_default();
            v["upstream"] = json!(format!("http://127.0.0.1:{}", r.port));
            v["health"] = hv;
            v
        })
        .collect();
    let projects: Vec<Value> = table
        .projects
        .iter()
        .zip(pc)
        .map(|(p, state)| json!({ "slug": p.slug, "root": p.root, "pc_http": p.pc_http, "process_compose": state }))
        .collect();
    json!({
        "enabled": true,
        "listen": h.bound.to_string(),
        "generation": h.generation(),
        "reloaded_at": h.reloaded_at(),
        "default": table.default_route().map(|r| r.project.clone()),
        "routes": routes,
        "projects": projects,
        "refused": table.refused,
    })
}

/// Run the probes concurrently (each has its own deadline), results in order.
async fn join_all<F>(it: impl Iterator<Item = F>) -> Vec<Value>
where
    F: std::future::Future<Output = Value> + Send + 'static,
{
    let handles: Vec<_> = it.map(tokio::spawn).collect();
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        out.push(h.await.unwrap_or_else(|e| json!({ "state": "unknown", "detail": format!("probe failed: {e}") })));
    }
    out
}

fn health_cell(h: &Value) -> String {
    let state = h["state"].as_str().unwrap_or("unknown");
    let detail = h["detail"].as_str().unwrap_or("");
    match state {
        "ok" => "<span class=\"ok\">ok</span>".into(),
        "down" => format!("<span class=\"down\">down</span> <span class=\"muted\">{}</span>", esc(detail)),
        _ => format!("<span class=\"muted\">{}</span>", esc(detail)),
    }
}

fn pc_cell(p: &Value) -> String {
    match p["state"].as_str() {
        Some("ok") => {
            let names: Vec<String> = p["processes"]
                .as_array()
                .map(|a| a.iter().map(|x| format!("{} {}", esc(x["name"].as_str().unwrap_or("-")), esc(x["status"].as_str().unwrap_or("-")))).collect())
                .unwrap_or_default();
            format!("{}/{} running <span class=\"muted\">({})</span>", p["running"], p["total"], names.join(", "))
        }
        _ => format!("<span class=\"muted\">{}</span>", esc(p["detail"].as_str().unwrap_or("unknown"))),
    }
}

/// The index page body.
pub fn render_html(s: &Value) -> String {
    let projects = s["projects"].as_array().cloned().unwrap_or_default();
    let pc_for = |slug: &str| projects.iter().find(|p| p["slug"] == slug).map(|p| pc_cell(&p["process_compose"])).unwrap_or_default();
    let mut out = String::from("<table><tr><th>Prefix</th><th>Project</th><th>Upstream</th><th>Health</th><th>process-compose</th></tr>");
    for r in s["routes"].as_array().cloned().unwrap_or_default() {
        let prefix = r["prefix"].as_str().unwrap_or("/");
        let project = r["project"].as_str().unwrap_or("-");
        let default = if r["default"] == true { " <span class=\"muted\">(also serves /)</span>" } else { "" };
        out += &format!(
            "<tr><td><a href=\"{p}/\"><code>{p}/</code></a>{d}</td><td>{proj}</td><td><code>{up}</code></td><td>{h}</td><td>{pc}</td></tr>",
            p = esc(prefix),
            d = default,
            proj = esc(project),
            up = esc(r["upstream"].as_str().unwrap_or("")),
            h = health_cell(&r["health"]),
            pc = pc_for(project)
        );
    }
    out += "</table>";
    let refused = s["refused"].as_array().cloned().unwrap_or_default();
    if !refused.is_empty() {
        out += "<h2>Refused</h2><ul>";
        for x in refused {
            out += &format!(
                "<li><strong>{}</strong> <code>{}</code> :{} — {}</li>",
                esc(x["project"].as_str().unwrap_or("-")),
                esc(x["prefix"].as_str().unwrap_or("-")),
                x["port"],
                esc(x["reason"].as_str().unwrap_or(""))
            );
        }
        out += "</ul>";
    }
    out += &format!(
        "<p class=\"muted\">router {} · routes generation {} · reloaded {} · <a href=\"/_weftos/routes.json\">routes.json</a></p>",
        esc(s["listen"].as_str().unwrap_or("")),
        s["generation"],
        esc(s["reloaded_at"].as_str().unwrap_or("-"))
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processes_are_summarized_from_data_or_a_bare_array() {
        let v = json!({"data": [{"name": "web", "namespace": "a", "status": "Running", "is_ready": "Ready", "restarts": 1},
                                {"name": "job", "status": "Completed"}]});
        let s = summarize_processes(&v, 18110);
        assert_eq!(s["running"], 1);
        assert_eq!(s["total"], 2);
        assert_eq!(s["processes"][0]["name"], "web");
        let bare = summarize_processes(&json!([{"name": "x", "status": "running"}]), 1);
        assert_eq!(bare["running"], 1);
    }

    #[test]
    fn html_lists_routes_refusals_and_escapes() {
        let s = json!({
            "listen": "127.0.0.1:18000", "generation": 3, "reloaded_at": "t",
            "routes": [{"prefix": "/a", "project": "a", "port": 3000, "default": true, "upstream": "http://127.0.0.1:3000",
                        "health": {"state": "down", "detail": "<boom>"}}],
            "projects": [{"slug": "a", "process_compose": {"state": "ok", "running": 1, "total": 2,
                          "processes": [{"name": "web", "status": "Running"}]}}],
            "refused": [{"project": "b", "prefix": "/a", "port": 3001, "reason": "prefix /a is already routed by project a"}]
        });
        let html = render_html(&s);
        assert!(html.contains("href=\"/a/\"") && html.contains("also serves /"), "{html}");
        assert!(html.contains("&lt;boom&gt;") && !html.contains("<boom>"));
        assert!(html.contains("1/2 running") && html.contains("Refused") && html.contains("already routed"));
    }
}
