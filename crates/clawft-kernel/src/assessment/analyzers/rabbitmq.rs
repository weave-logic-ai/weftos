//! RabbitMQ analyzer — discovers messaging topology (queues, exchanges, AMQP connections).

use std::path::{Path, PathBuf};

use crate::assessment::analyzer::{AnalysisContext, Analyzer};
use crate::assessment::Finding;

/// Analyzer that identifies RabbitMQ configuration, AMQP connections, and messaging topology.
pub struct RabbitMQAnalyzer;

impl Analyzer for RabbitMQAnalyzer {
    fn id(&self) -> &str {
        "rabbitmq"
    }

    fn name(&self) -> &str {
        "RabbitMQ Analyzer"
    }

    fn categories(&self) -> &[&str] {
        &["messaging", "security"]
    }

    fn analyze(
        &self,
        project: &Path,
        files: &[PathBuf],
        _context: &AnalysisContext,
    ) -> Vec<Finding> {
        let mut findings = Vec::new();

        for path in files {
            let rel = path.strip_prefix(project).unwrap_or(path);
            let rel_str = rel.display().to_string();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");

            // rabbitmq.conf
            if name == "rabbitmq.conf" {
                let content = match std::fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                findings.push(Finding {
                    severity: "info".into(),
                    category: "messaging".into(),
                    file: rel_str.clone(),
                    line: None,
                    message: "RabbitMQ configuration file detected".into(),
                });
                extract_rabbitmq_conf(&content, &rel_str, &mut findings);
                continue;
            }

            // definitions.json (RabbitMQ exported definitions)
            if name == "definitions.json" {
                let content = match std::fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                if content.contains("\"exchanges\"")
                    || content.contains("\"queues\"")
                    || content.contains("\"bindings\"")
                {
                    findings.push(Finding {
                        severity: "info".into(),
                        category: "messaging".into(),
                        file: rel_str.clone(),
                        line: None,
                        message: "RabbitMQ definitions file detected".into(),
                    });
                    extract_definitions_json(&content, &rel_str, &mut findings);
                }
                continue;
            }

            // Docker compose — look for rabbitmq service images
            if name.starts_with("docker-compose")
                && (name.ends_with(".yml") || name.ends_with(".yaml"))
            {
                let content = match std::fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                extract_compose_rabbitmq(&content, &rel_str, &mut findings);
                continue;
            }

            // All other files — scan for AMQP URIs, env vars, and queue/exchange declarations
            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            scan_file_content(&content, &rel_str, &mut findings);
        }

        findings
    }
}

/// Extract listener and vhost settings from `rabbitmq.conf`.
fn extract_rabbitmq_conf(content: &str, rel_str: &str, findings: &mut Vec<Finding>) {
    for (i, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }

        if let Some(val) = trimmed.strip_prefix("listeners.tcp.default") {
            let val = val.trim_start_matches(|c: char| c == '=' || c.is_whitespace());
            findings.push(Finding {
                severity: "info".into(),
                category: "messaging".into(),
                file: rel_str.to_string(),
                line: Some(i + 1),
                message: format!("RabbitMQ TCP listener: {val}"),
            });
        }

        if let Some(val) = trimmed.strip_prefix("listeners.ssl.default") {
            let val = val.trim_start_matches(|c: char| c == '=' || c.is_whitespace());
            findings.push(Finding {
                severity: "info".into(),
                category: "messaging".into(),
                file: rel_str.to_string(),
                line: Some(i + 1),
                message: format!("RabbitMQ SSL listener: {val}"),
            });
        }

        if let Some(val) = trimmed.strip_prefix("default_vhost") {
            let val = val.trim_start_matches(|c: char| c == '=' || c.is_whitespace());
            findings.push(Finding {
                severity: "info".into(),
                category: "messaging".into(),
                file: rel_str.to_string(),
                line: Some(i + 1),
                message: format!("RabbitMQ default vhost: {val}"),
            });
        }

        // Detect default credentials
        if let Some(val) = trimmed.strip_prefix("default_user") {
            let val = val
                .trim_start_matches(|c: char| c == '=' || c.is_whitespace())
                .trim();
            if val == "guest" {
                findings.push(Finding {
                    severity: "medium".into(),
                    category: "messaging".into(),
                    file: rel_str.to_string(),
                    line: Some(i + 1),
                    message: "RabbitMQ default user is 'guest' (default credentials)".into(),
                });
            }
        }

        if let Some(val) = trimmed.strip_prefix("default_pass") {
            let val = val
                .trim_start_matches(|c: char| c == '=' || c.is_whitespace())
                .trim();
            if val == "guest" {
                findings.push(Finding {
                    severity: "medium".into(),
                    category: "messaging".into(),
                    file: rel_str.to_string(),
                    line: Some(i + 1),
                    message: "RabbitMQ default password is 'guest' (default credentials)".into(),
                });
            }
        }
    }
}

/// Extract exchanges, queues, bindings, and users from `definitions.json` (heuristic JSON scan).
fn extract_definitions_json(content: &str, rel_str: &str, findings: &mut Vec<Finding>) {
    // Simple line-by-line scan for "name" fields inside known sections.
    // We track which section we're in by looking for top-level keys.
    let mut section = "";

    for line in content.lines() {
        let trimmed = line.trim().trim_matches(',');

        if trimmed.contains("\"exchanges\"") && trimmed.contains('[') {
            section = "exchanges";
            continue;
        }
        if trimmed.contains("\"queues\"") && trimmed.contains('[') {
            section = "queues";
            continue;
        }
        if trimmed.contains("\"bindings\"") && trimmed.contains('[') {
            section = "bindings";
            continue;
        }
        if trimmed.contains("\"users\"") && trimmed.contains('[') {
            section = "users";
            continue;
        }

        // Detect end of section
        if trimmed == "]" {
            section = "";
            continue;
        }

        if let Some(name) = extract_json_string_field(trimmed, "name") {
            match section {
                "exchanges" => {
                    if !name.is_empty() {
                        findings.push(Finding {
                            severity: "info".into(),
                            category: "messaging".into(),
                            file: rel_str.to_string(),
                            line: None,
                            message: format!("RabbitMQ exchange: {name}"),
                        });
                    }
                }
                "queues" => {
                    findings.push(Finding {
                        severity: "info".into(),
                        category: "messaging".into(),
                        file: rel_str.to_string(),
                        line: None,
                        message: format!("RabbitMQ queue: {name}"),
                    });
                }
                "users" => {
                    findings.push(Finding {
                        severity: "info".into(),
                        category: "messaging".into(),
                        file: rel_str.to_string(),
                        line: None,
                        message: format!("RabbitMQ user: {name}"),
                    });
                    if name == "guest" {
                        findings.push(Finding {
                            severity: "medium".into(),
                            category: "messaging".into(),
                            file: rel_str.to_string(),
                            line: None,
                            message: "RabbitMQ guest user present (default credentials)"
                                .into(),
                        });
                    }
                }
                _ => {}
            }
        }

        // Bindings: look for source/destination
        if section == "bindings"
            && let Some(src) = extract_json_string_field(trimmed, "source")
                && let Some(dst) = extract_json_string_field(trimmed, "destination") {
                    findings.push(Finding {
                        severity: "info".into(),
                        category: "messaging".into(),
                        file: rel_str.to_string(),
                        line: None,
                        message: format!("RabbitMQ binding: {src} -> {dst}"),
                    });
                }
    }
}

/// Extract a JSON string value for a given key from a line like `"key": "value"`.
fn extract_json_string_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let pattern = format!("\"{key}\"");
    let idx = line.find(&pattern)?;
    let after_key = &line[idx + pattern.len()..];
    // Skip optional whitespace and colon
    let after_colon = after_key.trim_start().strip_prefix(':')?;
    let after_ws = after_colon.trim_start();
    // Extract quoted string
    let after_quote = after_ws.strip_prefix('"')?;
    let end = after_quote.find('"')?;
    Some(&after_quote[..end])
}

/// Look for RabbitMQ services in docker-compose content.
fn extract_compose_rabbitmq(content: &str, rel_str: &str, findings: &mut Vec<Finding>) {
    let mut in_services = false;
    let mut current_service: Option<String> = None;
    let mut in_rabbitmq_service = false;
    let mut in_environment = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed == "services:" {
            in_services = true;
            continue;
        }

        if in_services {
            if !line.starts_with(' ') && !line.is_empty() {
                in_services = false;
                current_service = None;
                in_rabbitmq_service = false;
                in_environment = false;
                continue;
            }

            let stripped = line.trim_start();

            // Service name detection
            if stripped.ends_with(':') && !stripped.contains(' ') {
                let svc_name = stripped.trim_end_matches(':');
                current_service = Some(svc_name.to_string());
                in_rabbitmq_service = false;
                in_environment = false;
            }

            // Image reference
            if let Some(rest) = stripped.strip_prefix("image:") {
                let image = rest.trim().trim_matches('"').trim_matches('\'');
                if image.starts_with("rabbitmq:") || image == "rabbitmq" {
                    in_rabbitmq_service = true;
                    if let Some(ref svc) = current_service {
                        findings.push(Finding {
                            severity: "info".into(),
                            category: "messaging".into(),
                            file: rel_str.to_string(),
                            line: None,
                            message: format!(
                                "RabbitMQ Docker service '{svc}' image: {image}"
                            ),
                        });
                    }
                }
            }

            // Port mappings for RabbitMQ services
            if in_rabbitmq_service
                && (stripped.starts_with("- \"")
                    || stripped.starts_with("- '")
                    || stripped.starts_with("- "))
            {
                let val = stripped
                    .trim_start_matches("- ")
                    .trim_matches('"')
                    .trim_matches('\'');
                if val.contains(':')
                    && val.chars().all(|c| c.is_ascii_digit() || c == ':')
                    && let Some(ref svc) = current_service {
                        findings.push(Finding {
                            severity: "info".into(),
                            category: "messaging".into(),
                            file: rel_str.to_string(),
                            line: None,
                            message: format!(
                                "RabbitMQ service '{svc}' port mapping: {val}"
                            ),
                        });
                    }
            }

            // Environment section
            if stripped == "environment:" {
                in_environment = true;
                continue;
            }
            if in_environment && in_rabbitmq_service {
                if let Some(rest) = stripped.strip_prefix("- ") {
                    check_rabbitmq_env_var(rest, rel_str, findings);
                } else if stripped.contains(':') && !stripped.ends_with(':') {
                    check_rabbitmq_env_var(stripped, rel_str, findings);
                }
                // Leaving environment block
                if !stripped.starts_with('-') && !stripped.contains('=') && stripped.ends_with(':')
                {
                    in_environment = false;
                }
            }
        }
    }
}

/// Check for RabbitMQ-related environment variable assignments (detect default creds).
fn check_rabbitmq_env_var(entry: &str, rel_str: &str, findings: &mut Vec<Finding>) {
    let clean = entry.trim_matches('"').trim_matches('\'');
    if clean.contains("RABBITMQ_DEFAULT_USER=guest")
        || clean.contains("RABBITMQ_DEFAULT_PASS=guest")
    {
        findings.push(Finding {
            severity: "medium".into(),
            category: "messaging".into(),
            file: rel_str.to_string(),
            line: None,
            message: format!("RabbitMQ default credentials in compose: {clean}"),
        });
    }
}

/// Scan arbitrary file content for AMQP URIs, environment variable references,
/// and queue/exchange declaration patterns.
fn scan_file_content(content: &str, rel_str: &str, findings: &mut Vec<Finding>) {
    for (i, line) in content.lines().enumerate() {
        // AMQP connection strings
        if line.contains("amqp://") || line.contains("amqps://") {
            findings.push(Finding {
                severity: "info".into(),
                category: "messaging".into(),
                file: rel_str.to_string(),
                line: Some(i + 1),
                message: "AMQP connection string detected".into(),
            });
            // Check for guest credentials in URI
            if line.contains("amqp://guest:guest@") || line.contains("amqps://guest:guest@") {
                findings.push(Finding {
                    severity: "medium".into(),
                    category: "messaging".into(),
                    file: rel_str.to_string(),
                    line: Some(i + 1),
                    message: "AMQP URI contains default guest:guest credentials".into(),
                });
            }
        }

        // Environment variable references
        for var in &[
            "RABBITMQ_HOST",
            "RABBITMQ_PORT",
            "AMQP_URL",
        ] {
            if line.contains(var) {
                findings.push(Finding {
                    severity: "info".into(),
                    category: "messaging".into(),
                    file: rel_str.to_string(),
                    line: Some(i + 1),
                    message: format!("RabbitMQ environment variable reference: {var}"),
                });
            }
        }

        // Queue/exchange declaration patterns (Python, Node.js, etc.)
        for pattern in &[
            "channel.queue_declare",
            "channel.exchange_declare",
            "assertQueue",
            "assertExchange",
        ] {
            if line.contains(pattern) {
                findings.push(Finding {
                    severity: "info".into(),
                    category: "messaging".into(),
                    file: rel_str.to_string(),
                    line: Some(i + 1),
                    message: format!("RabbitMQ queue/exchange declaration: {pattern}"),
                });
            }
        }
    }
}
