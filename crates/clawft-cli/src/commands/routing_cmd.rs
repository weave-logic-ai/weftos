//! `weft routing` — dump and replay context-router decisions (WEFT-336).
//!
//! Reads the mesh-canonical substrate prefix
//! `substrate/_derived/agent/routing/recent/<id>` (WEFT-335 writer) via
//! daemon `substrate.list` / `substrate.read`, or a local JSON / JSONL
//! dump via `--from-file` for offline / CI use.
//!
//! ## Commands
//!
//! - `weft routing trace` — last N decisions (optionally filtered by
//!   conversation / chat id): query, selected route, alternatives,
//!   confidence, latency, fallback.
//! - `weft routing replay <decision_id>` — rehydrate the logged request
//!   and either dry-run the recorded path or re-route through the
//!   current offline [`NullRouter`](clawft_core::agent::context_router::NullRouter)
//!   and diff against the log.

use std::path::Path;

use clap::{Args, Subcommand};
use clawft_core::agent::context_router::{ContextRouter, NullRouter};
use clawft_core::agent::routing_log::{
    compute_metrics, decision_id_from_path, diff_replay, is_routing_tombstone, select_recent_records,
    RouterDecisionRecord, RoutingLogMetrics, ROUTING_RECENT_SUBSTRATE_PREFIX,
};
use clawft_rpc::Request;
use serde::Deserialize;
use serde_json::json;

/// Arguments for `weft routing`.
#[derive(Args, Debug)]
pub struct RoutingArgs {
    #[command(subcommand)]
    pub action: RoutingAction,
}

/// Subcommands for `weft routing`.
#[derive(Subcommand, Debug)]
pub enum RoutingAction {
    /// Dump recent routing decisions from substrate (or a file dump).
    Trace {
        /// Filter to a single conversation / chat id.
        #[arg(long = "conversation", short = 'C', value_name = "ID")]
        conversation: Option<String>,

        /// Maximum number of decisions to show (newest first). Default 20.
        #[arg(long, short = 'n', default_value = "20")]
        limit: usize,

        /// Emit machine-readable JSON instead of human text.
        #[arg(long)]
        json: bool,

        /// Read decisions from a JSON array or JSONL file instead of
        /// the daemon substrate (offline / test path).
        #[arg(long = "from-file", value_name = "PATH")]
        from_file: Option<String>,
    },

    /// Replay a recorded decision path (dry-run from log by default).
    Replay {
        /// Decision id (ULID / `rd-…`) under the routing log prefix.
        decision_id: String,

        /// Emit machine-readable JSON instead of human text.
        #[arg(long)]
        json: bool,

        /// Read from a JSON array / JSONL dump instead of substrate.
        #[arg(long = "from-file", value_name = "PATH")]
        from_file: Option<String>,

        /// Re-route through the current offline router ([`NullRouter`])
        /// and compare against the logged selected route. Without this
        /// flag the command dry-runs the recorded path only.
        #[arg(long)]
        live: bool,
    },
}

/// Run `weft routing`.
pub async fn run(args: RoutingArgs) -> anyhow::Result<()> {
    match args.action {
        RoutingAction::Trace {
            conversation,
            limit,
            json,
            from_file,
        } => run_trace(conversation, limit, json, from_file).await,
        RoutingAction::Replay {
            decision_id,
            json,
            from_file,
            live,
        } => run_replay(decision_id, json, from_file, live).await,
    }
}

async fn run_trace(
    conversation: Option<String>,
    limit: usize,
    as_json: bool,
    from_file: Option<String>,
) -> anyhow::Result<()> {
    let source = load_records(from_file.as_deref()).await?;
    let records = select_recent_records(source.records, conversation.as_deref(), limit);
    let metrics = compute_metrics(&records);

    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "source": source.label,
                "count": records.len(),
                "metrics": metrics,
                "decisions": records.iter().map(|r| r.to_substrate_value()).collect::<Vec<_>>(),
            }))?
        );
        return Ok(());
    }

    println!("weft routing trace");
    println!("==================");
    println!();
    println!("Source: {}", source.label);
    println!("Showing: {} decision(s)", records.len());
    print_metrics_human(&metrics);
    println!();

    if records.is_empty() {
        println!("(no routing decisions found)");
        if from_file.is_none() {
            println!();
            println!(
                "Hint: start the daemon and chat so decisions land under \
                 {ROUTING_RECENT_SUBSTRATE_PREFIX}/, or pass --from-file."
            );
        }
        return Ok(());
    }

    for (i, rec) in records.iter().enumerate() {
        print_record_human(i + 1, rec);
        println!();
    }

    Ok(())
}

async fn run_replay(
    decision_id: String,
    as_json: bool,
    from_file: Option<String>,
    live: bool,
) -> anyhow::Result<()> {
    let source = load_records(from_file.as_deref()).await?;
    let rec = source
        .records
        .into_iter()
        .find(|r| r.decision_id == decision_id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "decision_id '{decision_id}' not found in {} \
                 (prefix {ROUTING_RECENT_SUBSTRATE_PREFIX})",
                source.label
            )
        })?;

    let request = rec.to_context_request();

    let (live_decision, diff) = if live {
        let router = NullRouter;
        let decision = router.route(&request).await;
        let d = diff_replay(&rec, &decision);
        (Some(decision), Some(d))
    } else {
        (None, None)
    };

    if as_json {
        let mut out = json!({
            "source": source.label,
            "mode": if live { "live" } else { "dry-run" },
            "decision_id": rec.decision_id,
            "request": {
                "query": request.content,
                "channel": request.channel,
                "chat_id": request.chat_id,
            },
            "logged": rec.to_substrate_value(),
        });
        if let Some(ref d) = live_decision {
            out["live_decision"] = json!({
                "skills": d.skills,
                "tool_subset": d.tool_subset,
                "complexity_hint": d.complexity_hint,
                "archetype": d.archetype,
                "fallback_used": d.fallback_used,
            });
        }
        if let Some(ref d) = diff {
            out["diff"] = serde_json::to_value(d)?;
        }
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("weft routing replay");
    println!("===================");
    println!();
    println!("Source:      {}", source.label);
    println!(
        "Mode:        {}",
        if live {
            "live (NullRouter offline re-route)"
        } else {
            "dry-run (recorded path only)"
        }
    );
    println!("Decision id: {}", rec.decision_id);
    println!();
    println!("Request (rehydrated):");
    println!("  query:    {}", request.content);
    println!("  channel:  {}", request.channel);
    println!("  chat_id:  {}", request.chat_id);
    println!();
    println!("Logged selected route:");
    print_selected_route(&rec);
    println!("  alternatives: {:?}", rec.alternatives);
    println!(
        "  confidence:   {}",
        rec.confidence
            .map(|c| format!("{c:.3}"))
            .unwrap_or_else(|| "(none)".into())
    );
    println!("  latency_ms:   {}", rec.latency_ms);
    println!("  fallback:     {}", rec.fallback_used);
    println!("  ts:           {}", rec.ts);

    if let (Some(decision), Some(d)) = (live_decision, diff) {
        println!();
        println!("Live re-route (NullRouter):");
        println!("  skills:           {:?}", decision.skills);
        println!("  complexity_hint:  {}", decision.complexity_hint);
        println!("  fallback_used:    {}", decision.fallback_used);
        println!();
        println!("Diff vs log:");
        println!("  match_ok:              {}", d.match_ok);
        println!("  skills_missing:        {:?}", d.skills_missing);
        println!("  skills_extra:          {:?}", d.skills_extra);
        println!(
            "  complexity:            logged={:.4} live={:.4} delta={:.4}",
            d.logged_complexity_hint, d.live_complexity_hint, d.complexity_delta
        );
        println!(
            "  fallback:              logged={} live={}",
            d.logged_fallback_used, d.live_fallback_used
        );
        if !d.match_ok {
            println!();
            println!(
                "Note: NullRouter always returns the empty decision. A mismatch \
                 against a non-empty logged route is expected offline; wire a \
                 live daemon router for production regression checks."
            );
        }
    }

    Ok(())
}

// ── Loading ──────────────────────────────────────────────────────────────

struct LoadedSource {
    label: String,
    records: Vec<RouterDecisionRecord>,
}

async fn load_records(from_file: Option<&str>) -> anyhow::Result<LoadedSource> {
    if let Some(path) = from_file {
        let records = load_records_from_file(Path::new(path))?;
        return Ok(LoadedSource {
            label: format!("file:{path}"),
            records,
        });
    }
    load_records_from_daemon().await
}

/// Parse a JSON array of decision objects, a single object, or JSONL.
pub fn load_records_from_file(path: &Path) -> anyhow::Result<Vec<RouterDecisionRecord>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    parse_records_text(&text)
}

/// Public for unit tests — parse dump text into records.
pub fn parse_records_text(text: &str) -> anyhow::Result<Vec<RouterDecisionRecord>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    // Prefer a single JSON value (array or object). Multi-line JSONL
    // that happens to start with `{` falls through when parse fails
    // with trailing characters.
    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(value) => {
                return match value {
                    serde_json::Value::Array(items) => {
                        let mut out = Vec::with_capacity(items.len());
                        for item in items {
                            if is_routing_tombstone(&item) {
                                continue;
                            }
                            out.push(
                                RouterDecisionRecord::from_substrate_value(&item).map_err(|e| {
                                    anyhow::anyhow!("bad decision entry: {e}")
                                })?,
                            );
                        }
                        Ok(out)
                    }
                    serde_json::Value::Object(_) => {
                        if is_routing_tombstone(&value) {
                            return Ok(Vec::new());
                        }
                        Ok(vec![RouterDecisionRecord::from_substrate_value(&value)
                            .map_err(|e| anyhow::anyhow!("bad decision entry: {e}"))?])
                    }
                    _ => anyhow::bail!("routing dump must be a JSON object or array"),
                };
            }
            Err(e) if trimmed.starts_with('[') => {
                return Err(anyhow::anyhow!("invalid routing dump JSON: {e}"));
            }
            Err(_) => {
                // fall through to JSONL
            }
        }
    }

    // JSONL: one decision object per non-empty line.
    let mut out = Vec::new();
    for (lineno, line) in trimmed.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(line)
            .map_err(|e| anyhow::anyhow!("invalid JSONL at line {}: {e}", lineno + 1))?;
        if is_routing_tombstone(&value) {
            continue;
        }
        out.push(
            RouterDecisionRecord::from_substrate_value(&value)
                .map_err(|e| anyhow::anyhow!("bad decision at line {}: {e}", lineno + 1))?,
        );
    }
    Ok(out)
}

async fn load_records_from_daemon() -> anyhow::Result<LoadedSource> {
    let mut client = crate::commands::daemon_conn::connect()
        .await
        .map_err(|e| anyhow::anyhow!("{e}\n  or pass --from-file <dump.json>"))?
        .client;

    let list_params = json!({
        "prefix": ROUTING_RECENT_SUBSTRATE_PREFIX,
        "depth": 1,
    });
    let resp = client
        .call(Request::with_params("substrate.list", list_params))
        .await?;
    if !resp.ok {
        anyhow::bail!(
            "substrate.list failed: {}",
            resp.error.unwrap_or_else(|| "unknown error".into())
        );
    }

    let list: SubstrateListWire =
        serde_json::from_value(resp.result.unwrap_or(json!({}))).unwrap_or(SubstrateListWire {
            children: Vec::new(),
        });

    let mut records = Vec::new();
    for child in list.children {
        if !child.has_value {
            continue;
        }
        // Skip the prefix node itself and non-entry paths.
        if decision_id_from_path(&child.path).is_none() {
            continue;
        }
        let read_params = json!({ "path": child.path });
        let rresp = client
            .call(Request::with_params("substrate.read", read_params))
            .await?;
        if !rresp.ok {
            continue;
        }
        let wire: SubstrateReadWire =
            serde_json::from_value(rresp.result.unwrap_or(json!({}))).unwrap_or_default();
        let Some(value) = wire.value else {
            continue;
        };
        if is_routing_tombstone(&value) {
            continue;
        }
        match RouterDecisionRecord::from_substrate_value(&value) {
            Ok(mut rec) => {
                if rec.decision_id.is_empty() {
                    if let Some(id) = decision_id_from_path(&child.path) {
                        rec.decision_id = id;
                    }
                }
                records.push(rec);
            }
            Err(_) => continue,
        }
    }

    Ok(LoadedSource {
        label: format!("substrate:{ROUTING_RECENT_SUBSTRATE_PREFIX}"),
        records,
    })
}

/// Wire subset of `substrate.list` result.
#[derive(Debug, Default, Deserialize)]
struct SubstrateListWire {
    #[serde(default)]
    children: Vec<SubstrateListChildWire>,
}

#[derive(Debug, Deserialize)]
struct SubstrateListChildWire {
    path: String,
    #[serde(default)]
    has_value: bool,
}

/// Wire subset of `substrate.read` result.
#[derive(Debug, Default, Deserialize)]
struct SubstrateReadWire {
    #[serde(default)]
    value: Option<serde_json::Value>,
}

// ── Display helpers ──────────────────────────────────────────────────────

fn print_metrics_human(m: &RoutingLogMetrics) {
    println!("Metrics (window):");
    println!("  count:          {}", m.count);
    match m.p50_latency_ms {
        Some(v) => println!("  p50 latency:    {v} ms"),
        None => println!("  p50 latency:    (n/a)"),
    }
    match m.p99_latency_ms {
        Some(v) => println!("  p99 latency:    {v} ms"),
        None => println!("  p99 latency:    (n/a)"),
    }
    println!(
        "  fallback rate:  {:.1}% ({}/{})",
        m.fallback_rate * 100.0,
        m.fallback_count,
        m.count
    );
}

fn print_record_human(index: usize, rec: &RouterDecisionRecord) {
    println!("── decision {index} ──");
    println!("  id:           {}", rec.decision_id);
    println!("  ts:           {}", rec.ts);
    println!("  chat_id:      {}", rec.chat_id);
    println!("  channel:      {}", rec.channel);
    println!("  query:        {}", truncate(&rec.query, 120));
    print_selected_route(rec);
    if !rec.alternatives.is_empty() {
        println!("  alternatives: {:?}", rec.alternatives);
    }
    println!(
        "  confidence:   {}",
        rec.confidence
            .map(|c| format!("{c:.3}"))
            .unwrap_or_else(|| "(none)".into())
    );
    println!("  latency_ms:   {}", rec.latency_ms);
    println!("  fallback:     {}", rec.fallback_used);
}

fn print_selected_route(rec: &RouterDecisionRecord) {
    let skills = rec.selected_skills();
    println!(
        "  skills:       {}",
        if skills.is_empty() {
            "(none)".into()
        } else {
            skills.join(", ")
        }
    );
    println!("  complexity:   {:.4}", rec.selected_complexity_hint());
    if let Some(arch) = rec.selected_archetype() {
        println!("  archetype:    {arch}");
    }
    if let Some(tools) = rec.selected_route.get("tool_subset") {
        if !tools.is_null() {
            println!("  tool_subset:  {tools}");
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Load metrics for `weft status` (best-effort; empty when offline).
pub async fn try_load_status_metrics() -> Option<RoutingLogMetrics> {
    let loaded = load_records_from_daemon().await.ok()?;
    // Status window: last 100 live decisions (chat-agent-v1 observability).
    let window = select_recent_records(loaded.records, None, 100);
    Some(compute_metrics(&window))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_core::agent::context_router::ContextRequest;

    fn sample_json_array() -> String {
        serde_json::to_string_pretty(&json!([
            {
                "decision_id": "rd-1",
                "query": "hello",
                "selected_route": {
                    "skills": ["skill-a"],
                    "tool_subset": null,
                    "complexity_hint": 0.1,
                    "archetype": null
                },
                "alternatives": ["skill-b"],
                "confidence": 0.8,
                "latency_ms": 12,
                "channel": "panel",
                "chat_id": "c1",
                "fallback_used": false,
                "ts": "2026-07-30T12:00:00Z"
            },
            {
                "decision_id": "rd-2",
                "query": "world",
                "selected_route": {
                    "skills": [],
                    "complexity_hint": 0.0
                },
                "alternatives": [],
                "confidence": null,
                "latency_ms": 50,
                "channel": "cli",
                "chat_id": "c2",
                "fallback_used": true,
                "ts": "2026-07-30T12:01:00Z"
            },
            { "_pruned": true }
        ]))
        .unwrap()
    }

    #[test]
    fn parse_json_array_skips_tombstones() {
        let recs = parse_records_text(&sample_json_array()).unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].decision_id, "rd-1");
        assert_eq!(recs[1].decision_id, "rd-2");
        assert!(recs[1].fallback_used);
    }

    #[test]
    fn parse_jsonl() {
        let text = r#"
{"decision_id":"a","query":"q","selected_route":{"skills":[]},"alternatives":[],"latency_ms":1,"channel":"c","chat_id":"x","fallback_used":false,"ts":"2026-01-01T00:00:00Z"}
{"decision_id":"b","query":"q2","selected_route":{"skills":["s"]},"alternatives":[],"latency_ms":2,"channel":"c","chat_id":"x","fallback_used":false,"ts":"2026-01-01T00:00:01Z"}
"#;
        let recs = parse_records_text(text).unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[1].selected_skills(), vec!["s".to_string()]);
    }

    #[test]
    fn parse_single_object() {
        let text = r#"{"decision_id":"solo","query":"q","selected_route":{},"alternatives":[],"latency_ms":3,"channel":"c","chat_id":"y","fallback_used":false,"ts":"t"}"#;
        let recs = parse_records_text(text).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].decision_id, "solo");
    }

    #[test]
    fn select_recent_from_parsed_dump() {
        let recs = parse_records_text(&sample_json_array()).unwrap();
        let recent = select_recent_records(recs, None, 1);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].decision_id, "rd-2"); // newer ts
    }

    #[test]
    fn metrics_from_dump() {
        let recs = parse_records_text(&sample_json_array()).unwrap();
        let m = compute_metrics(&recs);
        assert_eq!(m.count, 2);
        assert_eq!(m.fallback_count, 1);
        assert!((m.fallback_rate - 0.5).abs() < 1e-9);
        assert_eq!(m.p50_latency_ms, Some(12));
        assert_eq!(m.p99_latency_ms, Some(50));
    }

    #[tokio::test]
    async fn live_replay_diff_against_null_router() {
        let recs = parse_records_text(&sample_json_array()).unwrap();
        let rec = &recs[0];
        let req = rec.to_context_request();
        assert_eq!(req.content, "hello");
        let decision = NullRouter.route(&req).await;
        let d = diff_replay(rec, &decision);
        // NullRouter empties skills → mismatch with skill-a.
        assert!(!d.match_ok);
        assert_eq!(d.skills_missing, vec!["skill-a".to_string()]);
    }

    #[test]
    fn to_context_request_maps_fields() {
        let rec = RouterDecisionRecord {
            decision_id: "x".into(),
            query: "q".into(),
            selected_route: json!({}),
            alternatives: vec![],
            confidence: None,
            latency_ms: 0,
            channel: "panel".into(),
            chat_id: "cid".into(),
            fallback_used: false,
            ts: "t".into(),
        };
        let req: ContextRequest = rec.to_context_request();
        assert_eq!(req.channel, "panel");
        assert_eq!(req.chat_id, "cid");
    }

    #[test]
    fn routing_action_trace_fields() {
        let action = RoutingAction::Trace {
            conversation: Some("c1".into()),
            limit: 5,
            json: true,
            from_file: Some("dump.json".into()),
        };
        match action {
            RoutingAction::Trace {
                conversation,
                limit,
                json,
                from_file,
            } => {
                assert_eq!(conversation.as_deref(), Some("c1"));
                assert_eq!(limit, 5);
                assert!(json);
                assert_eq!(from_file.as_deref(), Some("dump.json"));
            }
            _ => panic!("expected Trace"),
        }
    }
}
