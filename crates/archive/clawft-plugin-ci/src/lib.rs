//! CI/CD configuration parsing plugin for WeftOS.
//!
//! Parses GitHub Actions workflow YAML files and Vercel configuration to
//! extract job definitions, action references, secret usage, and deployment
//! settings.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Parsed representation of a GitHub Actions workflow file.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowInfo {
    pub name: String,
    pub triggers: Vec<String>,
    pub jobs: Vec<JobInfo>,
}

/// A single job within a workflow.
#[derive(Debug, Clone, PartialEq)]
pub struct JobInfo {
    pub name: String,
    pub runs_on: String,
    pub steps: Vec<StepInfo>,
}

/// A single step within a job.
#[derive(Debug, Clone, PartialEq)]
pub struct StepInfo {
    pub name: Option<String>,
    pub uses: Option<String>,
    pub run: Option<String>,
    pub secrets: Vec<String>,
}

/// Parsed representation of a `vercel.json` configuration file.
#[derive(Debug, Clone, PartialEq)]
pub struct VercelConfig {
    pub framework: Option<String>,
    pub build_command: Option<String>,
    pub output_directory: Option<String>,
    pub routes: Vec<String>,
    pub env: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Internal serde helpers — GitHub Actions
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawWorkflow {
    name: Option<String>,
    on: Option<serde_yaml::Value>,
    jobs: Option<HashMap<String, RawJob>>,
}

#[derive(Deserialize)]
struct RawJob {
    name: Option<String>,
    #[serde(rename = "runs-on")]
    runs_on: Option<serde_yaml::Value>,
    steps: Option<Vec<RawStep>>,
}

#[derive(Deserialize)]
struct RawStep {
    name: Option<String>,
    uses: Option<String>,
    run: Option<String>,
    #[serde(default)]
    with: Option<HashMap<String, serde_yaml::Value>>,
    #[serde(default)]
    env: Option<HashMap<String, serde_yaml::Value>>,
}

// ---------------------------------------------------------------------------
// Internal serde helpers — Vercel
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawVercelConfig {
    framework: Option<String>,
    #[serde(rename = "buildCommand")]
    build_command: Option<String>,
    #[serde(rename = "outputDirectory")]
    output_directory: Option<String>,
    routes: Option<Vec<RawVercelRoute>>,
    env: Option<HashMap<String, String>>,
}

#[derive(Deserialize)]
struct RawVercelRoute {
    src: Option<String>,
    dest: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract `${{ secrets.X }}` references from a string.
fn extract_secrets(text: &str) -> Vec<String> {
    let mut secrets = Vec::new();
    let pattern = "${{ secrets.";
    let mut remaining = text;
    while let Some(start) = remaining.find(pattern) {
        let after = &remaining[start + pattern.len()..];
        if let Some(end) = after.find("}}") {
            let name = after[..end].trim().to_string();
            if !name.is_empty() {
                secrets.push(name);
            }
            remaining = &after[end + 2..];
        } else {
            break;
        }
    }
    secrets
}

/// Collect secrets from all string values in a YAML value map.
fn secrets_from_map(map: &Option<HashMap<String, serde_yaml::Value>>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(m) = map {
        for v in m.values() {
            if let Some(s) = v.as_str() {
                out.extend(extract_secrets(s));
            }
        }
    }
    out
}

/// Extract trigger names from the `on` field.
fn parse_triggers(value: &serde_yaml::Value) -> Vec<String> {
    match value {
        serde_yaml::Value::String(s) => vec![s.clone()],
        serde_yaml::Value::Sequence(seq) => seq
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        serde_yaml::Value::Mapping(map) => map
            .keys()
            .filter_map(|k| k.as_str().map(String::from))
            .collect(),
        _ => vec![],
    }
}

/// Stringify a `runs-on` value (can be a plain string or an expression).
fn stringify_runs_on(value: &serde_yaml::Value) -> String {
    match value {
        serde_yaml::Value::String(s) => s.clone(),
        other => format!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a GitHub Actions workflow YAML string.
pub fn parse_github_workflow(content: &str) -> Result<WorkflowInfo, String> {
    let raw: RawWorkflow =
        serde_yaml::from_str(content).map_err(|e| format!("invalid workflow YAML: {e}"))?;

    let triggers = raw
        .on
        .as_ref()
        .map(parse_triggers)
        .unwrap_or_default();

    let mut jobs = Vec::new();
    if let Some(raw_jobs) = raw.jobs {
        let mut keys: Vec<_> = raw_jobs.keys().cloned().collect();
        keys.sort();
        for key in keys {
            let rj = &raw_jobs[&key];
            let job_name = rj.name.clone().unwrap_or_else(|| key.clone());
            let runs_on = rj
                .runs_on
                .as_ref()
                .map(stringify_runs_on)
                .unwrap_or_default();

            let mut steps = Vec::new();
            if let Some(raw_steps) = &rj.steps {
                for rs in raw_steps {
                    let mut secrets = Vec::new();
                    if let Some(run_cmd) = &rs.run {
                        secrets.extend(extract_secrets(run_cmd));
                    }
                    if let Some(uses_ref) = &rs.uses {
                        secrets.extend(extract_secrets(uses_ref));
                    }
                    secrets.extend(secrets_from_map(&rs.with));
                    secrets.extend(secrets_from_map(&rs.env));

                    steps.push(StepInfo {
                        name: rs.name.clone(),
                        uses: rs.uses.clone(),
                        run: rs.run.clone(),
                        secrets,
                    });
                }
            }

            jobs.push(JobInfo {
                name: job_name,
                runs_on,
                steps,
            });
        }
    }

    Ok(WorkflowInfo {
        name: raw.name.unwrap_or_default(),
        triggers,
        jobs,
    })
}

/// Parse a `vercel.json` configuration string.
pub fn parse_vercel_config(content: &str) -> Result<VercelConfig, String> {
    let raw: RawVercelConfig =
        serde_json::from_str(content).map_err(|e| format!("invalid vercel.json: {e}"))?;

    let routes = raw
        .routes
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| {
            let src = r.src.unwrap_or_default();
            let dest = r.dest.unwrap_or_default();
            if src.is_empty() && dest.is_empty() {
                None
            } else {
                Some(format!("{src} -> {dest}"))
            }
        })
        .collect();

    Ok(VercelConfig {
        framework: raw.framework,
        build_command: raw.build_command,
        output_directory: raw.output_directory,
        routes,
        env: raw.env.unwrap_or_default(),
    })
}

/// Scan a `.github/workflows/` directory and parse all `*.yml` / `*.yaml` files.
///
/// Silently skips files that fail to parse.
pub fn scan_workflows_dir(dir: &Path) -> Vec<WorkflowInfo> {
    let mut workflows = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return workflows,
    };

    let mut paths: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("yml" | "yaml")
            )
        })
        .collect();
    paths.sort();

    for path in paths {
        if let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(wf) = parse_github_workflow(&content)
        {
            workflows.push(wf);
        }
    }
    workflows
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const WORKFLOW_YAML: &str = r#"
name: CI
on:
  push:
    branches: [main]
  pull_request:

jobs:
  build:
    name: Build and Test
    runs-on: ubuntu-latest
    steps:
      - name: Checkout
        uses: actions/checkout@v4
      - name: Build
        run: cargo build --release
        env:
          CARGO_TOKEN: ${{ secrets.CARGO_TOKEN }}
      - name: Deploy
        uses: some/action@v1
        with:
          token: ${{ secrets.DEPLOY_TOKEN }}
"#;

    const VERCEL_JSON: &str = r#"{
        "framework": "nextjs",
        "buildCommand": "npm run build",
        "outputDirectory": ".next",
        "routes": [
            { "src": "/api/(.*)", "dest": "/api/$1" },
            { "src": "/(.*)", "dest": "/$1" }
        ],
        "env": {
            "NODE_ENV": "production",
            "API_URL": "https://api.example.com"
        }
    }"#;

    #[test]
    fn test_parse_github_workflow() {
        let wf = parse_github_workflow(WORKFLOW_YAML).unwrap();
        assert_eq!(wf.name, "CI");
        assert_eq!(wf.triggers.len(), 2);
        assert!(wf.triggers.contains(&"push".to_string()));
        assert!(wf.triggers.contains(&"pull_request".to_string()));

        assert_eq!(wf.jobs.len(), 1);
        let job = &wf.jobs[0];
        assert_eq!(job.name, "Build and Test");
        assert_eq!(job.runs_on, "ubuntu-latest");
        assert_eq!(job.steps.len(), 3);

        // Checkout step
        assert_eq!(job.steps[0].name.as_deref(), Some("Checkout"));
        assert_eq!(job.steps[0].uses.as_deref(), Some("actions/checkout@v4"));

        // Build step — should detect CARGO_TOKEN secret
        assert_eq!(job.steps[1].name.as_deref(), Some("Build"));
        assert!(job.steps[1].secrets.contains(&"CARGO_TOKEN".to_string()));

        // Deploy step — should detect DEPLOY_TOKEN secret
        assert_eq!(job.steps[2].name.as_deref(), Some("Deploy"));
        assert!(job.steps[2].secrets.contains(&"DEPLOY_TOKEN".to_string()));
    }

    #[test]
    fn test_parse_github_workflow_invalid() {
        assert!(parse_github_workflow("not: [valid: yaml: {{").is_err());
    }

    #[test]
    fn test_parse_vercel_config() {
        let cfg = parse_vercel_config(VERCEL_JSON).unwrap();
        assert_eq!(cfg.framework.as_deref(), Some("nextjs"));
        assert_eq!(cfg.build_command.as_deref(), Some("npm run build"));
        assert_eq!(cfg.output_directory.as_deref(), Some(".next"));
        assert_eq!(cfg.routes.len(), 2);
        assert_eq!(cfg.routes[0], "/api/(.*) -> /api/$1");
        assert_eq!(cfg.env.len(), 2);
        assert_eq!(cfg.env.get("NODE_ENV").unwrap(), "production");
    }

    #[test]
    fn test_parse_vercel_config_invalid() {
        assert!(parse_vercel_config("not json").is_err());
    }

    #[test]
    fn test_extract_secrets() {
        let secrets = extract_secrets("echo ${{ secrets.FOO }} and ${{ secrets.BAR }}");
        assert_eq!(secrets, vec!["FOO".to_string(), "BAR".to_string()]);
    }

    #[test]
    fn test_scan_workflows_dir_missing() {
        let result = scan_workflows_dir(Path::new("/nonexistent/path"));
        assert!(result.is_empty());
    }
}
