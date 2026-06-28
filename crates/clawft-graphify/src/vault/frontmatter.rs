//! YAML frontmatter parsing, generation, and enrichment for markdown documents.

use std::collections::BTreeMap;
use std::path::Path;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Parsed frontmatter with its raw body.
#[derive(Debug, Clone)]
pub struct Document {
    pub frontmatter: Frontmatter,
    pub body: String,
}

/// YAML frontmatter fields compatible with Obsidian / Dataview.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Frontmatter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// Parse YAML frontmatter from markdown content.
pub fn parse(content: &str) -> Document {
    if !content.starts_with("---") {
        return Document {
            frontmatter: Frontmatter::default(),
            body: content.to_string(),
        };
    }
    let rest = &content[3..];
    let end = rest.find("\n---");
    match end {
        Some(pos) => {
            let yaml_block = &rest[..pos];
            let body_start = 3 + pos + 4; // "---" + yaml + "\n---"
            let body = if body_start < content.len() {
                content[body_start..].trim_start_matches('\n').to_string()
            } else {
                String::new()
            };
            let fm: Frontmatter = serde_yaml::from_str(yaml_block).unwrap_or_default();
            Document {
                frontmatter: fm,
                body,
            }
        }
        None => Document {
            frontmatter: Frontmatter::default(),
            body: content.to_string(),
        },
    }
}

/// Serialize a `Document` back to markdown with YAML frontmatter.
pub fn render(doc: &Document) -> String {
    let yaml = serde_yaml::to_string(&doc.frontmatter).unwrap_or_default();
    format!("---\n{}---\n\n{}", yaml, doc.body)
}

/// Enrich a document's frontmatter by inferring missing fields from content
/// and file path.
pub fn enrich(doc: &mut Document, file_path: &Path) {
    let fm = &mut doc.frontmatter;

    if fm.title.is_none() {
        fm.title = extract_title(&doc.body, file_path);
    }
    if fm.r#type.is_none() {
        fm.r#type = infer_type(file_path, &doc.body);
    }
    if fm.status.is_none() {
        fm.status = infer_status(&doc.body);
    }
    if fm.tags.is_empty() {
        fm.tags = infer_tags(file_path, &doc.body);
    }
    if fm.aliases.is_empty() {
        fm.aliases = generate_aliases(file_path);
    }
    if fm.description.is_none() {
        fm.description = extract_description(&doc.body);
    }
    if fm.priority.is_none() {
        fm.priority = infer_priority(&doc.body);
    }
}

fn extract_title(body: &str, path: &Path) -> Option<String> {
    let h1 = Regex::new(r"^#\s+(.+)$").ok()?;
    for line in body.lines() {
        if let Some(cap) = h1.captures(line) {
            return Some(cap[1].trim().to_string());
        }
    }
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.replace(['-', '_'], " "))
}

fn extract_description(body: &str) -> Option<String> {
    let mut paragraph = String::new();
    let mut in_para = false;

    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if in_para {
                break;
            }
            continue;
        }
        if trimmed.starts_with('#') {
            if in_para {
                break;
            }
            continue;
        }
        in_para = true;
        if !paragraph.is_empty() {
            paragraph.push(' ');
        }
        paragraph.push_str(trimmed);
    }

    if paragraph.is_empty() {
        return None;
    }
    if paragraph.len() > 200 {
        paragraph.truncate(200);
        paragraph.push_str("...");
    }
    Some(paragraph)
}

struct TypePattern {
    pattern: &'static str,
    doc_type: &'static str,
}

const TYPE_PATTERNS: &[TypePattern] = &[
    TypePattern {
        pattern: "readme|index",
        doc_type: "guide",
    },
    TypePattern {
        pattern: "api|endpoint",
        doc_type: "technical",
    },
    TypePattern {
        pattern: "standard|convention",
        doc_type: "standard",
    },
    TypePattern {
        pattern: "adr|decision",
        doc_type: "architecture",
    },
    TypePattern {
        pattern: "spec|requirement",
        doc_type: "specification",
    },
    TypePattern {
        pattern: "changelog|release",
        doc_type: "documentation",
    },
];

fn infer_type(path: &Path, body: &str) -> Option<String> {
    let name = path.file_stem()?.to_str()?.to_lowercase();

    for tp in TYPE_PATTERNS {
        for keyword in tp.pattern.split('|') {
            if name.contains(keyword) {
                return Some(tp.doc_type.to_string());
            }
        }
    }

    let has_code_blocks = body.contains("```");
    let has_install_section =
        body.to_lowercase().contains("## install") || body.to_lowercase().contains("## setup");
    let has_api_section =
        body.to_lowercase().contains("## api") || body.to_lowercase().contains("## endpoints");
    let has_overview = body.to_lowercase().contains("## overview")
        || body.to_lowercase().contains("## introduction");

    if has_api_section {
        Some("technical".into())
    } else if has_install_section {
        Some("guide".into())
    } else if has_code_blocks {
        Some("technical".into())
    } else if has_overview {
        Some("concept".into())
    } else {
        Some("documentation".into())
    }
}

fn infer_status(body: &str) -> Option<String> {
    let lower = body.to_lowercase();
    if lower.contains("[wip]") || lower.contains("work in progress") || lower.contains("todo") {
        Some("in-progress".into())
    } else if lower.contains("[draft]") || lower.contains("draft") {
        Some("draft".into())
    } else if lower.contains("[archived]") || lower.contains("deprecated") {
        Some("archived".into())
    } else {
        Some("active".into())
    }
}

struct TagPattern {
    keywords: &'static [&'static str],
    tag: &'static str,
}

const TAG_PATTERNS: &[TagPattern] = &[
    TagPattern {
        keywords: &["api", "endpoint", "rest", "graphql"],
        tag: "api",
    },
    TagPattern {
        keywords: &["database", "sql", "postgres", "mysql", "mongo"],
        tag: "database",
    },
    TagPattern {
        keywords: &["test", "testing", "jest", "vitest"],
        tag: "testing",
    },
    TagPattern {
        keywords: &["docker", "container", "kubernetes", "k8s"],
        tag: "devops",
    },
    TagPattern {
        keywords: &["security", "auth", "authentication", "oauth"],
        tag: "security",
    },
    TagPattern {
        keywords: &["performance", "optimization", "cache"],
        tag: "performance",
    },
    TagPattern {
        keywords: &["react", "vue", "angular", "frontend"],
        tag: "frontend",
    },
    TagPattern {
        keywords: &["node", "express", "backend", "server"],
        tag: "backend",
    },
    TagPattern {
        keywords: &["typescript", "javascript", "python", "rust"],
        tag: "programming",
    },
    TagPattern {
        keywords: &["guide", "tutorial", "howto"],
        tag: "guide",
    },
    TagPattern {
        keywords: &["architecture", "design", "pattern"],
        tag: "architecture",
    },
    TagPattern {
        keywords: &["config", "configuration", "setup"],
        tag: "configuration",
    },
];

fn infer_tags(path: &Path, body: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let lower_body = body.to_lowercase();
    let lower_path = path.to_string_lossy().to_lowercase();
    let combined = format!("{lower_path} {lower_body}");

    for tp in TAG_PATTERNS {
        for kw in tp.keywords {
            if combined.contains(kw) {
                tags.push(tp.tag.to_string());
                break;
            }
        }
    }

    // Extract inline hashtags (not headings).
    if let Ok(re) = Regex::new(r"(?:^|\s)#([a-zA-Z][a-zA-Z0-9_-]*)") {
        for cap in re.captures_iter(body) {
            let tag = cap[1].to_lowercase();
            if !tags.contains(&tag) {
                tags.push(tag);
            }
        }
    }

    tags.sort();
    tags.dedup();
    tags
}

fn generate_aliases(path: &Path) -> Vec<String> {
    let stem = match path.file_stem().and_then(|s| s.to_str()) {
        Some(s) => s,
        None => return vec![],
    };

    let mut aliases = Vec::new();

    // Lowercase variant.
    let lower = stem.to_lowercase();
    if lower != stem {
        aliases.push(lower);
    }

    // Space-separated variant.
    let spaced = stem.replace(['-', '_'], " ");
    if spaced != stem {
        aliases.push(spaced);
    }

    // Acronym for multi-word names.
    let words: Vec<&str> = stem.split(['-', '_', ' ']).collect();
    if words.len() >= 2 {
        let acronym: String = words.iter().filter_map(|w| w.chars().next()).collect();
        let upper_acronym = acronym.to_uppercase();
        if upper_acronym.len() >= 2 && !aliases.contains(&upper_acronym) {
            aliases.push(upper_acronym);
        }
    }

    aliases.truncate(5);
    aliases
}

fn infer_priority(body: &str) -> Option<String> {
    let lower = body.to_lowercase();
    if lower.contains("critical") || lower.contains("urgent") || lower.contains("p0") {
        Some("critical".into())
    } else if lower.contains("high priority") || lower.contains("important") || lower.contains("p1")
    {
        Some("high".into())
    } else if lower.contains("low priority")
        || lower.contains("nice to have")
        || lower.contains("p3")
    {
        Some("low".into())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_roundtrip() {
        let input = "---\ntitle: Test\ntags:\n  - rust\n---\n\n# Test\n\nBody here.\n";
        let doc = parse(input);
        assert_eq!(doc.frontmatter.title.as_deref(), Some("Test"));
        assert_eq!(doc.frontmatter.tags, vec!["rust"]);
        assert!(doc.body.contains("Body here."));

        let rendered = render(&doc);
        assert!(rendered.starts_with("---\n"));
        assert!(rendered.contains("title: Test"));
    }

    #[test]
    fn parse_no_frontmatter() {
        let doc = parse("# Hello\n\nContent.");
        assert!(doc.frontmatter.title.is_none());
        assert_eq!(doc.body, "# Hello\n\nContent.");
    }

    #[test]
    fn enrich_infers_fields() {
        let mut doc = Document {
            frontmatter: Frontmatter::default(),
            body: "# My Auth Guide\n\nThis guide covers OAuth2 setup.\n\n## Setup\n\nDo things.\n```rust\nfn main() {}\n```\n".to_string(),
        };
        enrich(&mut doc, &PathBuf::from("docs/auth-guide.md"));
        assert_eq!(doc.frontmatter.title.as_deref(), Some("My Auth Guide"));
        assert_eq!(doc.frontmatter.r#type.as_deref(), Some("guide"));
        assert!(doc.frontmatter.tags.contains(&"security".to_string()));
    }

    #[test]
    fn aliases_from_filename() {
        let aliases = generate_aliases(Path::new("my-cool-project.md"));
        assert!(aliases.contains(&"my cool project".to_string()));
        assert!(aliases.contains(&"MCP".to_string()));
    }
}
