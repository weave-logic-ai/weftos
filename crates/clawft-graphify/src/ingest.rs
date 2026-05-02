//! URL ingestion: fetch URLs (tweets, arXiv, PDFs, webpages) and save as
//! annotated markdown ready for extraction into the knowledge graph.
//!
//! Ported from Python `graphify/ingest.py`. Security: blocks private IPs and
//! `file://` schemes to prevent SSRF.

use std::path::{Path, PathBuf};

use regex::Regex;

use crate::GraphifyError;

// ---------------------------------------------------------------------------
// URL type detection
// ---------------------------------------------------------------------------

/// Classified URL type for targeted extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlType {
    Tweet,
    Arxiv,
    Github,
    Youtube,
    Pdf,
    Image,
    Webpage,
}

/// Classify a URL for targeted extraction.
pub fn detect_url_type(url: &str) -> UrlType {
    let lower = url.to_lowercase();
    if lower.contains("twitter.com") || lower.contains("x.com") {
        return UrlType::Tweet;
    }
    if lower.contains("arxiv.org") {
        return UrlType::Arxiv;
    }
    if lower.contains("github.com") {
        return UrlType::Github;
    }
    if lower.contains("youtube.com") || lower.contains("youtu.be") {
        return UrlType::Youtube;
    }
    if let Some(path) = url.split('?').next() {
        let path_lower = path.to_lowercase();
        if path_lower.ends_with(".pdf") {
            return UrlType::Pdf;
        }
        for ext in &[".png", ".jpg", ".jpeg", ".webp", ".gif"] {
            if path_lower.ends_with(ext) {
                return UrlType::Image;
            }
        }
    }
    UrlType::Webpage
}

// ---------------------------------------------------------------------------
// SSRF protection
// ---------------------------------------------------------------------------

/// Validate that a URL is safe to fetch (no SSRF).
pub fn validate_url(url: &str) -> Result<(), GraphifyError> {
    let lower = url.to_lowercase();

    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return Err(GraphifyError::IngestError(format!(
            "only http:// and https:// URLs are allowed, got: {url}"
        )));
    }

    let after_scheme = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .unwrap_or(&lower);
    let host = after_scheme
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");

    if host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "[::1]" {
        return Err(GraphifyError::IngestError(
            "cannot fetch localhost URLs (SSRF protection)".into(),
        ));
    }

    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        let octets = ip.octets();
        let is_private = octets[0] == 10
            || (octets[0] == 172 && (16..=31).contains(&octets[1]))
            || (octets[0] == 192 && octets[1] == 168)
            || octets[0] == 127;
        if is_private {
            return Err(GraphifyError::IngestError(
                "cannot fetch private IP addresses (SSRF protection)".into(),
            ));
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Filename helpers
// ---------------------------------------------------------------------------

/// Turn a URL into a safe filename.
pub fn safe_filename(url: &str, suffix: &str) -> String {
    let re = Regex::new(r"[^\w\-]").unwrap();
    let multi_underscore = Regex::new(r"_+").unwrap();

    let after_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);

    let name = re.replace_all(after_scheme, "_");
    let name = multi_underscore.replace_all(&name, "_");
    let name = name.trim_matches('_');

    let truncated = if name.len() > 80 { &name[..80] } else { name };
    format!("{truncated}{suffix}")
}

/// Escape a string for embedding in YAML double-quoted scalar.
fn yaml_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\n', '\r'], " ")
}

// ---------------------------------------------------------------------------
// IngestResult
// ---------------------------------------------------------------------------

/// Result of URL ingestion.
#[derive(Debug)]
pub struct IngestResult {
    /// Path to the saved file.
    pub path: PathBuf,
    /// The detected URL type.
    pub url_type: UrlType,
    /// Filename that was saved.
    pub filename: String,
}

// ---------------------------------------------------------------------------
// HTTP client trait
// ---------------------------------------------------------------------------

/// Abstraction over HTTP fetching so callers can inject their own client.
pub trait HttpClient: Send + Sync {
    /// Fetch a URL and return the body as a string.
    fn fetch_text(&self, url: &str) -> Result<String, GraphifyError>;
    /// Fetch a URL and return raw bytes.
    fn fetch_bytes(&self, url: &str) -> Result<Vec<u8>, GraphifyError>;
}

/// A no-op HTTP client that always errors (for compile-time gating).
pub struct StubHttpClient;

impl HttpClient for StubHttpClient {
    fn fetch_text(&self, url: &str) -> Result<String, GraphifyError> {
        Err(GraphifyError::IngestError(format!(
            "HTTP client not configured, cannot fetch: {url}"
        )))
    }
    fn fetch_bytes(&self, url: &str) -> Result<Vec<u8>, GraphifyError> {
        Err(GraphifyError::IngestError(format!(
            "HTTP client not configured, cannot fetch: {url}"
        )))
    }
}

// ---------------------------------------------------------------------------
// HTML helpers
// ---------------------------------------------------------------------------

fn strip_html(html: &str) -> String {
    let script_re = Regex::new(r"(?is)<script[^>]*>.*?</script>").unwrap();
    let style_re = Regex::new(r"(?is)<style[^>]*>.*?</style>").unwrap();
    let tag_re = Regex::new(r"<[^>]+>").unwrap();
    let ws_re = Regex::new(r"\s+").unwrap();

    let text = script_re.replace_all(html, "");
    let text = style_re.replace_all(&text, "");
    let text = tag_re.replace_all(&text, " ");
    let text = ws_re.replace_all(&text, " ");
    let text = text.trim().to_string();

    if text.len() > 12_000 { text[..12_000].to_string() } else { text }
}

fn extract_title(html: &str) -> Option<String> {
    let re = Regex::new(r"(?is)<title[^>]*>(.*?)</title>").ok()?;
    re.captures(html).map(|c| {
        let ws_re = Regex::new(r"\s+").unwrap();
        ws_re.replace_all(c.get(1).unwrap().as_str(), " ").trim().to_string()
    })
}

// ---------------------------------------------------------------------------
// Fetch helpers
// ---------------------------------------------------------------------------

fn fetch_tweet(
    client: &dyn HttpClient,
    url: &str,
    contributor: Option<&str>,
) -> Result<(String, String), GraphifyError> {
    let oembed_url = url.replace("x.com", "twitter.com");
    let api_url = format!(
        "https://publish.twitter.com/oembed?url={}&omit_script=true",
        urlencoding_encode(&oembed_url)
    );

    let (tweet_text, tweet_author) = match client.fetch_text(&api_url) {
        Ok(body) => {
            let data: serde_json::Value =
                serde_json::from_str(&body).unwrap_or(serde_json::json!({}));
            let html = data["html"].as_str().unwrap_or("");
            let tag_re = Regex::new(r"<[^>]+>").unwrap();
            let text = tag_re.replace_all(html, "").trim().to_string();
            let author = data["author_name"].as_str().unwrap_or("unknown").to_string();
            (text, author)
        }
        Err(_) => (
            format!("Tweet at {url} (could not fetch content)"),
            "unknown".to_string(),
        ),
    };

    let now = chrono_now_iso();
    let content = format!(
        "---\nsource_url: {url}\ntype: tweet\nauthor: {tweet_author}\ncaptured_at: {now}\ncontributor: {cont}\n---\n\n# Tweet by @{tweet_author}\n\n{tweet_text}\n\nSource: {url}\n",
        url = url, tweet_author = tweet_author, now = now,
        cont = contributor.unwrap_or("unknown"), tweet_text = tweet_text,
    );
    let filename = safe_filename(url, ".md");
    Ok((content, filename))
}

fn fetch_arxiv(
    client: &dyn HttpClient,
    url: &str,
    contributor: Option<&str>,
) -> Result<(String, String), GraphifyError> {
    let arxiv_re = Regex::new(r"(\d{4}\.\d{4,5})").unwrap();
    let arxiv_id = match arxiv_re.captures(url) {
        Some(caps) => caps.get(1).unwrap().as_str().to_string(),
        None => return fetch_webpage(client, url, contributor),
    };

    let api_url = format!("https://export.arxiv.org/abs/{arxiv_id}");
    let (title, abstract_text, authors) = match client.fetch_text(&api_url) {
        Ok(html) => {
            let tag_re = Regex::new(r"<[^>]+>").unwrap();
            let abs_re = Regex::new(r#"(?is)class="abstract[^"]*"[^>]*>(.*?)</blockquote>"#).unwrap();
            let title_re = Regex::new(r#"(?is)class="title[^"]*"[^>]*>(.*?)</h1>"#).unwrap();
            let auth_re = Regex::new(r#"(?is)class="authors"[^>]*>(.*?)</div>"#).unwrap();

            let abstract_text = abs_re.captures(&html)
                .map(|c| tag_re.replace_all(c.get(1).unwrap().as_str(), "").trim().to_string())
                .unwrap_or_default();
            let title = title_re.captures(&html)
                .map(|c| tag_re.replace_all(c.get(1).unwrap().as_str(), " ").trim().to_string())
                .unwrap_or_else(|| arxiv_id.clone());
            let authors = auth_re.captures(&html)
                .map(|c| tag_re.replace_all(c.get(1).unwrap().as_str(), "").trim().to_string())
                .unwrap_or_default();
            (title, abstract_text, authors)
        }
        Err(_) => (arxiv_id.clone(), String::new(), String::new()),
    };

    let now = chrono_now_iso();
    let content = format!(
        "---\nsource_url: {url}\narxiv_id: {aid}\ntype: paper\ntitle: \"{t}\"\npaper_authors: \"{a}\"\ncaptured_at: {now}\ncontributor: {cont}\n---\n\n# {title}\n\n**Authors:** {authors}\n**arXiv:** {aid}\n\n## Abstract\n\n{abs}\n\nSource: {url}\n",
        url = url, aid = arxiv_id, t = yaml_escape(&title), a = yaml_escape(&authors),
        now = now, cont = contributor.unwrap_or("unknown"),
        title = title, authors = authors, abs = abstract_text,
    );
    let filename = format!("arxiv_{}.md", arxiv_id.replace('.', "_"));
    Ok((content, filename))
}

fn fetch_webpage(
    client: &dyn HttpClient,
    url: &str,
    contributor: Option<&str>,
) -> Result<(String, String), GraphifyError> {
    let html = client.fetch_text(url)?;
    let title = extract_title(&html).unwrap_or_else(|| url.to_string());
    let markdown = strip_html(&html);

    let now = chrono_now_iso();
    let content = format!(
        "---\nsource_url: {url}\ntype: webpage\ntitle: \"{t}\"\ncaptured_at: {now}\ncontributor: {cont}\n---\n\n# {title}\n\nSource: {url}\n\n---\n\n{md}\n",
        url = url, t = yaml_escape(&title), now = now,
        cont = contributor.unwrap_or("unknown"), title = title, md = markdown,
    );
    let filename = safe_filename(url, ".md");
    Ok((content, filename))
}

// ---------------------------------------------------------------------------
// Core ingestion
// ---------------------------------------------------------------------------

/// Chain event kind for URL ingestion.
pub const EVENT_KIND_GRAPHIFY_INGEST: &str = "graphify.ingest";

/// Ingest a URL: fetch, classify, and save to `target_dir`.
pub fn ingest(
    url: &str,
    target_dir: &Path,
    client: &dyn HttpClient,
    contributor: Option<&str>,
) -> Result<IngestResult, GraphifyError> {
    validate_url(url)?;

    std::fs::create_dir_all(target_dir).map_err(|e| {
        GraphifyError::IngestError(format!("failed to create target dir: {e}"))
    })?;

    let url_type = detect_url_type(url);

    let result = match url_type {
        UrlType::Pdf => {
            let bytes = client.fetch_bytes(url)?;
            let filename = safe_filename(url, ".pdf");
            let out_path = target_dir.join(&filename);
            std::fs::write(&out_path, bytes)?;
            IngestResult { path: out_path, url_type, filename }
        }
        UrlType::Image => {
            let ext = url.rsplit('.').next()
                .map(|e| format!(".{}", e.split('?').next().unwrap_or("jpg")))
                .unwrap_or_else(|| ".jpg".to_string());
            let bytes = client.fetch_bytes(url)?;
            let filename = safe_filename(url, &ext);
            let out_path = target_dir.join(&filename);
            std::fs::write(&out_path, bytes)?;
            IngestResult { path: out_path, url_type, filename }
        }
        _ => {
            let (content, filename) = match url_type {
                UrlType::Tweet => fetch_tweet(client, url, contributor)?,
                UrlType::Arxiv => fetch_arxiv(client, url, contributor)?,
                _ => fetch_webpage(client, url, contributor)?,
            };

            let mut out_path = target_dir.join(&filename);
            let mut counter = 1u32;
            while out_path.exists() {
                let stem = Path::new(&filename).file_stem()
                    .and_then(|s| s.to_str()).unwrap_or("file");
                out_path = target_dir.join(format!("{stem}_{counter}.md"));
                counter += 1;
            }

            std::fs::write(&out_path, &content)?;
            let final_filename = out_path.file_name()
                .and_then(|n| n.to_str()).unwrap_or(&filename).to_string();

            IngestResult { path: out_path, url_type, filename: final_filename }
        }
    };

    // Chain event marker -- daemon subscriber forwards to ExoChain.
    tracing::info!(
        target: "chain_event",
        source = "graphify",
        kind = EVENT_KIND_GRAPHIFY_INGEST,
        url = url,
        url_type = ?result.url_type,
        filename = %result.filename,
        "chain"
    );

    Ok(result)
}

// ---------------------------------------------------------------------------
// Query result storage (feedback loop)
// ---------------------------------------------------------------------------

/// Save a Q&A result as markdown for re-extraction into the graph.
pub fn save_query_result(
    question: &str,
    answer: &str,
    memory_dir: &Path,
    query_type: &str,
    source_nodes: Option<&[String]>,
) -> Result<PathBuf, GraphifyError> {
    std::fs::create_dir_all(memory_dir)?;

    let now = chrono_now_iso();
    let slug_re = Regex::new(r"[^\w]").unwrap();
    let lowered = question.to_lowercase();
    let slug = slug_re.replace_all(&lowered, "_");
    let slug = if slug.len() > 50 { &slug[..50] } else { &slug };
    let slug = slug.trim_matches('_');

    let ts = now.replace([':', '-', 'T'], "").split('.').next().unwrap_or("0").to_string();
    let filename = format!("query_{ts}_{slug}.md");

    let mut lines = vec![
        "---".to_string(),
        format!("type: \"{query_type}\""),
        format!("date: \"{now}\""),
        format!("question: \"{}\"", yaml_escape(question)),
        "contributor: \"graphify\"".to_string(),
    ];

    if let Some(nodes) = source_nodes {
        let nodes_str: Vec<String> = nodes.iter().take(10).map(|n| format!("\"{n}\"")).collect();
        lines.push(format!("source_nodes: [{}]", nodes_str.join(", ")));
    }

    lines.push("---".to_string());
    lines.push(String::new());
    lines.push(format!("# Q: {question}"));
    lines.push(String::new());
    lines.push("## Answer".to_string());
    lines.push(String::new());
    lines.push(answer.to_string());

    if let Some(nodes) = source_nodes {
        lines.push(String::new());
        lines.push("## Source Nodes".to_string());
        lines.push(String::new());
        for n in nodes {
            lines.push(format!("- {n}"));
        }
    }

    let content = lines.join("\n");
    let out_path = memory_dir.join(&filename);
    std::fs::write(&out_path, &content)?;
    Ok(out_path)
}

// ---------------------------------------------------------------------------
// Utility helpers
// ---------------------------------------------------------------------------

fn urlencoding_encode(s: &str) -> String {
    s.replace('%', "%25")
        .replace(' ', "%20")
        .replace('#', "%23")
        .replace('&', "%26")
        .replace('?', "%3F")
}

fn chrono_now_iso() -> String {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    format!("1970-01-01T00:00:00Z+{secs}s")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_type_detection() {
        assert_eq!(detect_url_type("https://twitter.com/user/status/123"), UrlType::Tweet);
        assert_eq!(detect_url_type("https://x.com/user/status/456"), UrlType::Tweet);
        assert_eq!(detect_url_type("https://arxiv.org/abs/2301.12345"), UrlType::Arxiv);
        assert_eq!(detect_url_type("https://github.com/user/repo"), UrlType::Github);
        assert_eq!(detect_url_type("https://example.com/doc.pdf"), UrlType::Pdf);
        assert_eq!(detect_url_type("https://example.com/img.png"), UrlType::Image);
        assert_eq!(detect_url_type("https://example.com/page"), UrlType::Webpage);
    }

    #[test]
    fn ssrf_protection() {
        assert!(validate_url("file:///etc/passwd").is_err());
        assert!(validate_url("http://127.0.0.1/admin").is_err());
        assert!(validate_url("http://localhost/api").is_err());
        assert!(validate_url("http://10.0.0.1/internal").is_err());
        assert!(validate_url("http://192.168.1.1/router").is_err());
        assert!(validate_url("http://172.16.0.1/private").is_err());
        assert!(validate_url("https://example.com/page").is_ok());
    }

    #[test]
    fn safe_filename_generation() {
        let name = safe_filename("https://example.com/path/to/page", ".md");
        assert!(name.ends_with(".md"));
        assert!(!name.contains('/'));
        assert!(!name.contains(':'));
    }

    #[test]
    fn strip_html_basic() {
        let html = "<html><body><p>Hello <b>world</b></p></body></html>";
        let text = strip_html(html);
        assert!(text.contains("Hello"));
        assert!(text.contains("world"));
        assert!(!text.contains('<'));
    }

    #[test]
    fn save_query_result_creates_file() {
        let dir = std::env::temp_dir().join("graphify_test_query");
        let _ = std::fs::remove_dir_all(&dir);

        let path = save_query_result(
            "What is the main service?",
            "The AuthService handles authentication.",
            &dir,
            "query",
            Some(&["AuthService".to_string()]),
        ).unwrap();

        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("AuthService"));
        assert!(content.contains("question:"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn yaml_escape_special_chars() {
        assert_eq!(yaml_escape("hello \"world\""), "hello \\\"world\\\"");
        assert_eq!(yaml_escape("line\nbreak"), "line break");
    }
}
