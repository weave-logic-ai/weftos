//! `mcp` checks: every stdio server in the nearest `.mcp.json` resolves.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::env::DoctorEnv;
use super::{Component, Finding, Severity};

/// Nearest `.mcp.json` at or above `start`.
fn find_mcp_json(start: &Path) -> Option<PathBuf> {
    start.ancestors().map(|a| a.join(".mcp.json")).find(|p| p.is_file())
}

/// Does `cmd` resolve (absolute path, or found in `PATH` dirs)?
fn resolves(env: &DoctorEnv, cmd: &str) -> bool {
    let p = Path::new(cmd);
    if p.is_absolute() || cmd.contains('/') {
        return p.is_file();
    }
    env.path_dirs.iter().any(|d| d.join(cmd).is_file())
}

/// Findings for MCP wiring.
pub fn findings(env: &DoctorEnv) -> Vec<Finding> {
    let c = Component::Mcp;
    let Some(path) = find_mcp_json(&env.cwd) else {
        return vec![Finding::new(c, "mcp_json", Severity::Ok, "no .mcp.json in this directory tree (nothing to check)")];
    };
    let parsed: Option<Value> = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok());
    let Some(servers) = parsed.as_ref().and_then(|v| v.get("mcpServers")).and_then(Value::as_object) else {
        return vec![Finding::new(c, "mcp_json", Severity::Warn, format!("{} is not valid JSON with an mcpServers object", path.display()))
            .remedy(format!("fix the syntax of {}", path.display()))];
    };
    let mut out = Vec::new();
    for (name, spec) in servers {
        let Some(cmd) = spec.get("command").and_then(Value::as_str) else {
            continue; // http/sse servers have a url, not a command
        };
        // `npx --no-install pkg` style servers resolve through npx itself.
        if resolves(env, cmd) {
            out.push(Finding::new(c, format!("server:{name}"), Severity::Ok, format!("`{cmd}` resolves")));
        } else {
            out.push(
                Finding::new(c, format!("server:{name}"), Severity::Warn, format!("command `{cmd}` for MCP server `{name}` not found on PATH"))
                    .remedy("install it or use an absolute path in .mcp.json"),
            );
        }
    }
    if out.is_empty() {
        out.push(Finding::new(c, "mcp_json", Severity::Ok, format!("{} has no stdio servers", path.display())));
    }
    out
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::doctor::env::test_env;

    #[test]
    fn missing_command_warns_and_present_passes() {
        let d = tempfile::tempdir().unwrap();
        let mut env = test_env(d.path());
        std::fs::create_dir_all(&env.cwd).unwrap();
        let bin = d.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("weft"), "").unwrap();
        env.path_dirs = vec![bin];
        std::fs::write(
            env.cwd.join(".mcp.json"),
            r#"{"mcpServers":{"a":{"command":"weft","args":["mcp-server"]},"b":{"command":"nope-bin"},"c":{"url":"http://x"}}}"#,
        )
        .unwrap();
        let f = findings(&env);
        assert_eq!(f.len(), 2);
        assert_eq!(f.iter().find(|x| x.id == "server:a").unwrap().severity, Severity::Ok);
        assert_eq!(f.iter().find(|x| x.id == "server:b").unwrap().severity, Severity::Warn);
    }

    #[test]
    fn no_file_is_pass_and_bad_json_warns() {
        let d = tempfile::tempdir().unwrap();
        let env = test_env(d.path());
        std::fs::create_dir_all(&env.cwd).unwrap();
        assert_eq!(findings(&env)[0].severity, Severity::Ok);
        std::fs::write(env.cwd.join(".mcp.json"), "{oops").unwrap();
        assert_eq!(findings(&env)[0].severity, Severity::Warn);
    }
}
