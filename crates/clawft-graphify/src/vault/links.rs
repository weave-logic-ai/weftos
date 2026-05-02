//! Wikilink parsing, extraction, and generation for Obsidian-compatible markdown.

use regex::Regex;
use std::collections::HashSet;

/// A parsed wikilink: `[[target]]` or `[[target|alias]]` or `[[target#heading]]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WikiLink {
    pub target: String,
    pub alias: Option<String>,
    pub heading: Option<String>,
}

impl WikiLink {
    /// Render as `[[target]]`, `[[target|alias]]`, or `[[target#heading|alias]]`.
    pub fn render(&self) -> String {
        let mut s = String::from("[[");
        s.push_str(&self.target);
        if let Some(h) = &self.heading {
            s.push('#');
            s.push_str(h);
        }
        if let Some(a) = &self.alias {
            s.push('|');
            s.push_str(a);
        }
        s.push_str("]]");
        s
    }
}

/// Extract all wikilinks from markdown content.
pub fn extract_wikilinks(content: &str) -> Vec<WikiLink> {
    let re = Regex::new(r"\[\[([^\]|#]+)(?:#([^\]|]*))?(?:\|([^\]]+))?\]\]")
        .expect("valid regex");

    re.captures_iter(content)
        .map(|cap| WikiLink {
            target: cap[1].trim().to_string(),
            alias: cap.get(3).map(|m| m.as_str().trim().to_string()),
            heading: cap.get(2).map(|m| m.as_str().trim().to_string()),
        })
        .collect()
}

/// Extract standard markdown links `[text](path.md)`, excluding images and
/// external URLs.
pub fn extract_markdown_links(content: &str) -> Vec<(String, String)> {
    let re = Regex::new(r"\[([^\]]*)\]\(([^)]+\.md)\)")
        .expect("valid regex");

    re.captures_iter(content)
        .filter_map(|cap| {
            let full = cap.get(0)?;
            // Skip images: `![alt](path.md)`.
            if full.start() > 0 && content.as_bytes()[full.start() - 1] == b'!' {
                return None;
            }
            let path = cap[2].trim();
            if path.starts_with("http://") || path.starts_with("https://") {
                return None;
            }
            Some((cap[1].to_string(), path.to_string()))
        })
        .collect()
}

/// Insert wikilinks into content for targets that appear as plain text.
/// Only links the first occurrence of each target per document.
pub fn auto_link(content: &str, known_titles: &[String]) -> String {
    let existing: HashSet<String> = extract_wikilinks(content)
        .into_iter()
        .map(|wl| wl.target.to_lowercase())
        .collect();

    let mut result = content.to_string();

    for title in known_titles {
        let lower = title.to_lowercase();
        if existing.contains(&lower) {
            continue;
        }
        // Match the title as a whole word, case-insensitive, but only replace
        // the first occurrence outside of existing wikilinks and code blocks.
        let pattern = format!(r"(?i)\b({})\b", regex::escape(title));
        if let Ok(re) = Regex::new(&pattern)
            && let Some(m) = re.find(&result) {
                let matched_text = m.as_str();
                // Don't link inside code fences or existing links.
                let before = &result[..m.start()];
                let in_code = !before.matches("```").count().is_multiple_of(2);
                let in_link = before.ends_with("[[");
                if !in_code && !in_link {
                    let replacement = format!("[[{matched_text}]]");
                    result = format!(
                        "{}{}{}",
                        &result[..m.start()],
                        replacement,
                        &result[m.end()..]
                    );
                }
            }
    }

    result
}

/// Convert a label to a safe filename (no slashes, colons, etc.).
pub fn safe_filename(name: &str) -> String {
    name.replace(['/', '\\'], "-")
        .replace(' ', "_")
        .replace(':', "-")
        .replace(['<', '>', '"'], "")
        .replace('|', "-")
        .replace(['?', '*'], "")
}

/// Generate a markdown "## Backlinks" section from a list of source files.
pub fn render_backlinks_section(backlinks: &[(String, String)]) -> String {
    if backlinks.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        String::new(),
        "## Backlinks".to_string(),
        String::new(),
    ];

    for (source, context) in backlinks {
        lines.push(format!("- [[{source}]] — {context}"));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_simple_wikilink() {
        let links = extract_wikilinks("See [[Auth Service]] for details.");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Auth Service");
        assert!(links[0].alias.is_none());
    }

    #[test]
    fn extract_aliased_wikilink() {
        let links = extract_wikilinks("See [[auth-service|Auth]] here.");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "auth-service");
        assert_eq!(links[0].alias.as_deref(), Some("Auth"));
    }

    #[test]
    fn extract_heading_wikilink() {
        let links = extract_wikilinks("See [[auth-service#setup]] here.");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "auth-service");
        assert_eq!(links[0].heading.as_deref(), Some("setup"));
    }

    #[test]
    fn render_wikilink() {
        let wl = WikiLink {
            target: "Auth".into(),
            alias: Some("Authentication".into()),
            heading: None,
        };
        assert_eq!(wl.render(), "[[Auth|Authentication]]");
    }

    #[test]
    fn auto_link_first_occurrence() {
        let content = "We use the Auth Service for login. The Auth Service is fast.";
        let result = auto_link(content, &["Auth Service".to_string()]);
        assert!(result.contains("[[Auth Service]]"));
        // Only one link inserted.
        assert_eq!(result.matches("[[Auth Service]]").count(), 1);
    }

    #[test]
    fn auto_link_skips_existing() {
        let content = "We use [[Auth Service]] already.";
        let result = auto_link(content, &["Auth Service".to_string()]);
        assert_eq!(result.matches("[[Auth Service]]").count(), 1);
    }

    #[test]
    fn safe_filename_strips() {
        assert_eq!(safe_filename("a/b:c"), "a-b-c");
        assert_eq!(safe_filename("hello world"), "hello_world");
    }

    #[test]
    fn markdown_links_extracted() {
        let content = "[Guide](./setup.md) and ![img](pic.md) and [ext](https://x.com/f.md)";
        let links = extract_markdown_links(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].1, "./setup.md");
    }
}
