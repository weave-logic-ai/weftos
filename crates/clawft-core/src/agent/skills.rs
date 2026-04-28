//! Skill discovery and loading.
//!
//! Scans a directory tree for skill definitions. Two formats are supported:
//!
//! **Legacy format** – each skill is a subdirectory containing `skill.json`
//! metadata and an optional `prompt.md` with LLM instructions.
//!
//! **SKILL.md format** – a single `SKILL.md` file with YAML frontmatter
//! (delimited by `---`) and a Markdown body that serves as the prompt.
//!
//! ```text
//! skills/
//! +-- research/
//! |   +-- skill.json   {"name":"research","description":"...","variables":["topic"]}
//! |   +-- prompt.md    # LLM instructions text
//! +-- claude-flow/
//!     +-- SKILL.md     # YAML frontmatter + markdown prompt
//! ```
//!
//! File locations follow the fallback chain:
//! `~/.clawft/workspace/skills/` then `~/.nanobot/workspace/skills/`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use crate::runtime::RwLock;
use tracing::{debug, warn};

use clawft_platform::Platform;
use clawft_types::{ClawftError, Result};

/// A loaded skill definition.
///
/// Skills consist of JSON metadata (`skill.json`) and an optional LLM
/// prompt file (`prompt.md`), **or** a single `SKILL.md` with YAML
/// frontmatter. The prompt is loaded lazily on first use and cached in
/// the [`SkillsLoader`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Skill identifier (matches the directory name).
    pub name: String,

    /// Human-readable description for skill listing.
    pub description: String,

    /// Template variable names expected by the skill prompt.
    #[serde(default)]
    pub variables: Vec<String>,

    /// LLM instructions loaded from `prompt.md` or the Markdown body of
    /// `SKILL.md`. `None` until the prompt file has been read.
    #[serde(skip)]
    pub prompt: Option<String>,

    /// Semantic version of the skill definition.
    #[serde(default = "default_version")]
    pub version: String,

    /// Tool name patterns this skill is allowed to invoke.
    #[serde(default)]
    pub allowed_tools: Vec<String>,

    /// Whether the skill can be invoked directly by the user (e.g. via
    /// `/skill-name` in the CLI or a Discord command).
    #[serde(default)]
    pub user_invocable: bool,

    /// Hint shown to the user about what arguments the skill accepts.
    #[serde(default)]
    pub argument_hint: Option<String>,
}

fn default_version() -> String {
    "1.0.0".into()
}

/// YAML frontmatter fields in a `SKILL.md` file.
///
/// Field names use kebab-case in the YAML (e.g. `allowed-tools`) and are
/// mapped to snake_case Rust fields via `#[serde(rename)]`.
#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: String,
    description: String,
    #[serde(default = "default_version")]
    version: String,
    #[serde(default)]
    variables: Vec<String>,
    #[serde(default, rename = "allowed-tools")]
    allowed_tools: Vec<String>,
    #[serde(default, rename = "user-invocable")]
    user_invocable: bool,
    #[serde(default, rename = "argument-hint")]
    argument_hint: Option<String>,
}

/// Parse a `SKILL.md` file into a [`Skill`].
///
/// Expects the file to start with `---\n`, followed by YAML metadata,
/// then a closing `---\n`. Everything after the closing delimiter is
/// used as the prompt.
fn parse_skill_md(content: &str, fallback_name: &str) -> std::result::Result<Skill, String> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Err("SKILL.md missing opening --- delimiter".into());
    }

    // Skip the opening "---" line.
    let after_open = match trimmed.strip_prefix("---") {
        Some(rest) => rest.trim_start_matches(['\r', '\n']),
        None => return Err("SKILL.md missing opening --- delimiter".into()),
    };

    // Find the closing "---" delimiter.
    let close_idx = after_open
        .find("\n---")
        .ok_or_else(|| "SKILL.md missing closing --- delimiter".to_string())?;

    let yaml_block = &after_open[..close_idx];
    let body_start = close_idx + 4; // skip "\n---"
    let body = if body_start < after_open.len() {
        after_open[body_start..].trim_start_matches(['\r', '\n'])
    } else {
        ""
    };

    let fm: SkillFrontmatter =
        serde_yaml::from_str(yaml_block).map_err(|e| format!("invalid YAML frontmatter: {e}"))?;

    Ok(Skill {
        name: if fm.name.is_empty() {
            fallback_name.to_string()
        } else {
            fm.name
        },
        description: fm.description,
        variables: fm.variables,
        prompt: if body.is_empty() {
            None
        } else {
            Some(body.to_string())
        },
        version: fm.version,
        allowed_tools: fm.allowed_tools,
        user_invocable: fm.user_invocable,
        argument_hint: fm.argument_hint,
    })
}

/// Loads and caches skill definitions from a workspace directory.
///
/// The loader scans for skill subdirectories, parses their `skill.json`
/// metadata, and lazily loads `prompt.md` content on first use. All
/// filesystem access goes through the [`Platform`] abstraction.
///
/// # Concurrency
///
/// The skill cache is wrapped in [`RwLock`] for many-reader / rare-writer
/// access. [`list_skills`](SkillsLoader::list_skills) and
/// [`get_skill`](SkillsLoader::get_skill) acquire read locks;
/// [`load_skill`](SkillsLoader::load_skill) and
/// [`load_all`](SkillsLoader::load_all) acquire write locks.
pub struct SkillsLoader<P: Platform> {
    skills_dir: PathBuf,
    /// Additional directories to scan for skills (e.g. project-level `skills/`).
    extra_dirs: Vec<PathBuf>,
    skills: Arc<RwLock<HashMap<String, Skill>>>,
    platform: Arc<P>,
}

impl<P: Platform> SkillsLoader<P> {
    /// Create a new skills loader.
    ///
    /// Resolves the skills directory via the fallback chain:
    /// 1. `~/.clawft/workspace/skills/`
    /// 2. `~/.nanobot/workspace/skills/`
    ///
    /// If neither exists, the `.clawft` path is used.
    ///
    /// # Errors
    ///
    /// Returns [`ClawftError::ConfigInvalid`] if no home directory can
    /// be determined.
    pub fn new(platform: Arc<P>) -> Result<Self> {
        let home = platform
            .fs()
            .home_dir()
            .ok_or_else(|| ClawftError::ConfigInvalid {
                reason: "could not determine home directory".into(),
            })?;

        let clawft_skills = home.join(".clawft").join("workspace").join("skills");
        let nanobot_skills = home.join(".nanobot").join("workspace").join("skills");

        let skills_dir = if nanobot_skills.exists() && !clawft_skills.exists() {
            debug!(path = %nanobot_skills.display(), "using legacy nanobot skills path");
            nanobot_skills
        } else {
            debug!(path = %clawft_skills.display(), "using clawft skills path");
            clawft_skills
        };

        Ok(Self {
            skills_dir,
            extra_dirs: Vec::new(),
            skills: Arc::new(RwLock::new(HashMap::new())),
            platform,
        })
    }

    /// Add an extra directory to scan for skills.
    ///
    /// Skills from extra directories are merged with the primary
    /// `skills_dir`. If the same skill name appears in multiple
    /// directories, the primary directory takes precedence.
    pub fn add_extra_dir(&mut self, dir: PathBuf) {
        if !self.extra_dirs.contains(&dir) {
            debug!(path = %dir.display(), "adding extra skills directory");
            self.extra_dirs.push(dir);
        }
    }

    /// Create a skills loader with an explicit skills directory.
    ///
    /// Skips the home-directory resolution that [`Self::new`] performs;
    /// useful for hermetic tests (point at a temp dir) and for
    /// embedded callers that want a custom skills root. Public so
    /// workspace-level integration tests outside `clawft-core` can
    /// compose an isolated `AgentLoop` without scanning the user's
    /// real `~/.clawft/skills`.
    pub fn with_dir(skills_dir: PathBuf, platform: Arc<P>) -> Self {
        Self {
            skills_dir,
            extra_dirs: Vec::new(),
            skills: Arc::new(RwLock::new(HashMap::new())),
            platform,
        }
    }

    /// List available skill names by scanning subdirectories.
    ///
    /// A directory is considered a skill if it contains `skill.json`
    /// **or** `SKILL.md`. Names are returned in filesystem order (not
    /// sorted).
    ///
    /// # Errors
    ///
    /// Returns an empty list if the skills directory does not exist.
    /// Propagates I/O errors for other failures.
    pub async fn list_skills(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // Scan all directories: primary first, then extras.
        let mut dirs_to_scan = vec![self.skills_dir.clone()];
        dirs_to_scan.extend(self.extra_dirs.iter().cloned());

        for dir in &dirs_to_scan {
            if !self.platform.fs().exists(dir).await {
                continue;
            }

            let entries = match self.platform.fs().list_dir(dir).await {
                Ok(e) => e,
                Err(e) => {
                    warn!(dir = %dir.display(), error = %e, "failed to list skills dir");
                    continue;
                }
            };

            for entry in entries {
                let has_skill_json = self.platform.fs().exists(&entry.join("skill.json")).await;
                let has_skill_md = self.platform.fs().exists(&entry.join("SKILL.md")).await;
                if (has_skill_json || has_skill_md)
                    && let Some(name) = entry.file_name()
                {
                    let name = name.to_string_lossy().into_owned();
                    if seen.insert(name.clone()) {
                        names.push(name);
                    }
                }
            }
        }

        Ok(names)
    }

    /// Load a specific skill by name.
    ///
    /// Tries `skill.json` + `prompt.md` first. If `skill.json` does not
    /// exist, falls back to parsing `SKILL.md` (YAML frontmatter +
    /// Markdown body). The loaded skill is cached for future
    /// [`get_skill`](SkillsLoader::get_skill) calls.
    ///
    /// # Errors
    ///
    /// Returns [`ClawftError::PluginLoadFailed`] if neither format can
    /// be loaded.
    pub async fn load_skill(&self, name: &str) -> Result<Skill> {
        // Search primary dir first, then extra dirs.
        let mut dirs_to_search = vec![self.skills_dir.clone()];
        dirs_to_search.extend(self.extra_dirs.iter().cloned());

        let mut skill: Option<Skill> = None;
        for dir in &dirs_to_search {
            let skill_dir = dir.join(name);
            let skill_json_path = skill_dir.join("skill.json");
            let skill_md_path = skill_dir.join("SKILL.md");

            if self.platform.fs().exists(&skill_json_path).await {
                skill = Some(self.load_skill_json(name, &skill_dir).await?);
                break;
            } else if self.platform.fs().exists(&skill_md_path).await {
                skill = Some(self.load_skill_md(name, &skill_md_path).await?);
                break;
            }
        }

        let skill = skill.ok_or_else(|| ClawftError::PluginLoadFailed {
            plugin: format!("skill/{name}: neither skill.json nor SKILL.md found"),
        })?;

        // Cache the loaded skill
        {
            let mut cache = self.skills.write().await;
            cache.insert(name.to_string(), skill.clone());
        }

        debug!(skill = name, "loaded skill");
        Ok(skill)
    }

    /// Load a skill from `skill.json` + optional `prompt.md`.
    async fn load_skill_json(&self, name: &str, skill_dir: &std::path::Path) -> Result<Skill> {
        let skill_json_path = skill_dir.join("skill.json");
        let prompt_path = skill_dir.join("prompt.md");

        let json_content = self
            .platform
            .fs()
            .read_to_string(&skill_json_path)
            .await
            .map_err(|e| ClawftError::PluginLoadFailed {
                plugin: format!("skill/{name}: {e}"),
            })?;

        let mut skill: Skill =
            serde_json::from_str(&json_content).map_err(|e| ClawftError::PluginLoadFailed {
                plugin: format!("skill/{name}: invalid skill.json: {e}"),
            })?;

        if self.platform.fs().exists(&prompt_path).await {
            match self.platform.fs().read_to_string(&prompt_path).await {
                Ok(prompt) => {
                    skill.prompt = Some(prompt);
                }
                Err(e) => {
                    warn!(skill = name, error = %e, "failed to read prompt.md");
                }
            }
        }

        Ok(skill)
    }

    /// Load a skill from a `SKILL.md` file with YAML frontmatter.
    async fn load_skill_md(&self, name: &str, path: &std::path::Path) -> Result<Skill> {
        let content = self
            .platform
            .fs()
            .read_to_string(path)
            .await
            .map_err(|e| ClawftError::PluginLoadFailed {
                plugin: format!("skill/{name}: {e}"),
            })?;

        parse_skill_md(&content, name).map_err(|e| ClawftError::PluginLoadFailed {
            plugin: format!("skill/{name}: {e}"),
        })
    }

    /// Get a cached skill by name.
    ///
    /// Returns `None` if the skill has not been loaded yet. Use
    /// [`load_skill`](SkillsLoader::load_skill) to load and cache a
    /// skill first.
    pub async fn get_skill(&self, name: &str) -> Option<Skill> {
        let cache = self.skills.read().await;
        cache.get(name).cloned()
    }

    /// Load all skills from the skills directory.
    ///
    /// Scans for skill subdirectories and loads each one. Errors on
    /// individual skills are logged as warnings but do not abort the
    /// overall load.
    pub async fn load_all(&self) -> Result<()> {
        let names = self.list_skills().await?;
        for name in &names {
            if let Err(e) = self.load_skill(name).await {
                warn!(skill = %name, error = %e, "failed to load skill, skipping");
            }
        }
        debug!(count = names.len(), "loaded all skills");
        Ok(())
    }

    /// Path to the skills directory (for diagnostics).
    pub fn skills_dir(&self) -> &PathBuf {
        &self.skills_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_platform::NativePlatform;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(prefix: &str) -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("clawft_skills_test_{prefix}_{pid}_{id}"))
    }

    fn test_loader(dir: &std::path::Path) -> SkillsLoader<NativePlatform> {
        let platform = Arc::new(NativePlatform::new());
        SkillsLoader::with_dir(dir.to_path_buf(), platform)
    }

    /// Create a skill directory with skill.json and optional prompt.md.
    async fn create_skill(dir: &std::path::Path, name: &str, desc: &str, prompt: Option<&str>) {
        let skill_dir = dir.join(name);
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();

        let json = serde_json::json!({
            "name": name,
            "description": desc,
            "variables": ["topic"],
        });
        tokio::fs::write(
            skill_dir.join("skill.json"),
            serde_json::to_string_pretty(&json).unwrap(),
        )
        .await
        .unwrap();

        if let Some(prompt_text) = prompt {
            tokio::fs::write(skill_dir.join("prompt.md"), prompt_text)
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn list_skills_empty_when_dir_missing() {
        let dir = temp_dir("missing");
        let loader = test_loader(&dir);
        let skills = loader.list_skills().await.unwrap();
        assert!(skills.is_empty());
    }

    #[tokio::test]
    async fn list_skills_finds_skill_dirs() {
        let dir = temp_dir("list");
        create_skill(&dir, "research", "Deep research", Some("Research prompt")).await;
        create_skill(&dir, "code_review", "Code review", None).await;

        let loader = test_loader(&dir);
        let mut skills = loader.list_skills().await.unwrap();
        skills.sort();

        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0], "code_review");
        assert_eq!(skills[1], "research");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn list_skills_ignores_dirs_without_skill_json() {
        let dir = temp_dir("no_json");
        tokio::fs::create_dir_all(dir.join("not_a_skill"))
            .await
            .unwrap();
        tokio::fs::write(dir.join("not_a_skill").join("readme.md"), "hi")
            .await
            .unwrap();

        create_skill(&dir, "valid_skill", "Valid", Some("prompt")).await;

        let loader = test_loader(&dir);
        let skills = loader.list_skills().await.unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0], "valid_skill");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn load_skill_parses_metadata_and_prompt() {
        let dir = temp_dir("load");
        create_skill(
            &dir,
            "research",
            "Deep research on a topic",
            Some("You are a research assistant."),
        )
        .await;

        let loader = test_loader(&dir);
        let skill = loader.load_skill("research").await.unwrap();

        assert_eq!(skill.name, "research");
        assert_eq!(skill.description, "Deep research on a topic");
        assert_eq!(skill.variables, vec!["topic"]);
        assert_eq!(
            skill.prompt.as_deref(),
            Some("You are a research assistant.")
        );
        assert_eq!(skill.version, "1.0.0");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn load_skill_without_prompt() {
        let dir = temp_dir("no_prompt");
        create_skill(&dir, "basic", "A basic skill", None).await;

        let loader = test_loader(&dir);
        let skill = loader.load_skill("basic").await.unwrap();

        assert_eq!(skill.name, "basic");
        assert!(skill.prompt.is_none());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn load_skill_caches_result() {
        let dir = temp_dir("cache");
        create_skill(&dir, "cached", "Cached skill", Some("cached prompt")).await;

        let loader = test_loader(&dir);

        // Not cached yet
        assert!(loader.get_skill("cached").await.is_none());

        // Load it
        loader.load_skill("cached").await.unwrap();

        // Now cached
        let cached = loader.get_skill("cached").await;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().name, "cached");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn load_skill_nonexistent_returns_error() {
        let dir = temp_dir("nonexistent");
        tokio::fs::create_dir_all(&dir).await.unwrap();

        let loader = test_loader(&dir);
        let result = loader.load_skill("does_not_exist").await;
        assert!(result.is_err());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn load_skill_invalid_json_returns_error() {
        let dir = temp_dir("bad_json");
        let skill_dir = dir.join("bad");
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        tokio::fs::write(skill_dir.join("skill.json"), "not valid json {{{")
            .await
            .unwrap();

        let loader = test_loader(&dir);
        let result = loader.load_skill("bad").await;
        assert!(result.is_err());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn load_all_loads_all_skills() {
        let dir = temp_dir("load_all");
        create_skill(&dir, "skill_a", "Skill A", Some("prompt a")).await;
        create_skill(&dir, "skill_b", "Skill B", Some("prompt b")).await;

        let loader = test_loader(&dir);
        loader.load_all().await.unwrap();

        assert!(loader.get_skill("skill_a").await.is_some());
        assert!(loader.get_skill("skill_b").await.is_some());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn load_all_skips_invalid_skills() {
        let dir = temp_dir("load_all_skip");
        create_skill(&dir, "good", "Good skill", Some("prompt")).await;

        // Create a bad skill
        let bad_dir = dir.join("bad");
        tokio::fs::create_dir_all(&bad_dir).await.unwrap();
        tokio::fs::write(bad_dir.join("skill.json"), "invalid")
            .await
            .unwrap();

        let loader = test_loader(&dir);
        // Should not return error -- bad skill is skipped
        loader.load_all().await.unwrap();

        assert!(loader.get_skill("good").await.is_some());
        assert!(loader.get_skill("bad").await.is_none());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn skill_serde_roundtrip() {
        let skill = Skill {
            name: "test".into(),
            description: "Test skill".into(),
            variables: vec!["var1".into(), "var2".into()],
            prompt: Some("prompt text".into()),
            version: "2.0.0".into(),
            allowed_tools: vec!["read_file".into()],
            user_invocable: true,
            argument_hint: Some("hint".into()),
        };

        let json = serde_json::to_string(&skill).unwrap();
        let restored: Skill = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.name, "test");
        assert_eq!(restored.description, "Test skill");
        assert_eq!(restored.variables, vec!["var1", "var2"]);
        // prompt is #[serde(skip)] so it should be None after deserialization
        assert!(restored.prompt.is_none());
        assert_eq!(restored.version, "2.0.0");
    }

    #[tokio::test]
    async fn skill_json_with_version() {
        let dir = temp_dir("version");
        let skill_dir = dir.join("versioned");
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("skill.json"),
            r#"{"name":"versioned","description":"With version","variables":[],"version":"3.1.0"}"#,
        )
        .await
        .unwrap();

        let loader = test_loader(&dir);
        let skill = loader.load_skill("versioned").await.unwrap();
        assert_eq!(skill.version, "3.1.0");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn get_skill_returns_none_for_unloaded() {
        let dir = temp_dir("unloaded");
        let loader = test_loader(&dir);
        assert!(loader.get_skill("anything").await.is_none());
    }

    #[tokio::test]
    async fn new_resolves_home_dir() {
        let platform = Arc::new(NativePlatform::new());
        let loader = SkillsLoader::new(platform);
        assert!(loader.is_ok());
        let loader = loader.unwrap();
        assert!(loader.skills_dir().is_absolute());
    }

    // ── SKILL.md format tests ──────────────────────────────────────────

    /// Create a skill directory with only a SKILL.md file.
    async fn create_skill_md(dir: &std::path::Path, name: &str, content: &str) {
        let skill_dir = dir.join(name);
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        tokio::fs::write(skill_dir.join("SKILL.md"), content)
            .await
            .unwrap();
    }

    const SAMPLE_SKILL_MD: &str = "\
---
name: claude-flow
description: Orchestrate multi-agent swarms
version: 2.0.0
variables:
  - objective
allowed-tools:
  - claude-flow__swarm_*
  - claude-flow__agent_*
user-invocable: true
argument-hint: Swarm objective
---

# Claude Flow Skill

This is the prompt body.
";

    #[test]
    fn parse_skill_md_full() {
        let skill = parse_skill_md(SAMPLE_SKILL_MD, "fallback").unwrap();
        assert_eq!(skill.name, "claude-flow");
        assert_eq!(skill.description, "Orchestrate multi-agent swarms");
        assert_eq!(skill.version, "2.0.0");
        assert_eq!(skill.variables, vec!["objective"]);
        assert_eq!(
            skill.allowed_tools,
            vec!["claude-flow__swarm_*", "claude-flow__agent_*"]
        );
        assert!(skill.user_invocable);
        assert_eq!(skill.argument_hint.as_deref(), Some("Swarm objective"));
        assert!(skill.prompt.as_ref().unwrap().contains("# Claude Flow Skill"));
        assert!(skill.prompt.as_ref().unwrap().contains("prompt body"));
    }

    #[test]
    fn parse_skill_md_minimal() {
        let content = "---\nname: minimal\ndescription: A minimal skill\n---\n";
        let skill = parse_skill_md(content, "fallback").unwrap();
        assert_eq!(skill.name, "minimal");
        assert_eq!(skill.version, "1.0.0");
        assert!(skill.variables.is_empty());
        assert!(skill.allowed_tools.is_empty());
        assert!(!skill.user_invocable);
        assert!(skill.argument_hint.is_none());
        assert!(skill.prompt.is_none());
    }

    #[test]
    fn parse_skill_md_missing_opening_delimiter() {
        let result = parse_skill_md("name: bad\n---\n", "x");
        assert!(result.is_err());
    }

    #[test]
    fn parse_skill_md_missing_closing_delimiter() {
        let result = parse_skill_md("---\nname: bad\n", "x");
        assert!(result.is_err());
    }

    #[test]
    fn parse_skill_md_uses_fallback_name() {
        let content = "---\nname: \"\"\ndescription: test\n---\nBody\n";
        let skill = parse_skill_md(content, "my-fallback").unwrap();
        assert_eq!(skill.name, "my-fallback");
    }

    #[tokio::test]
    async fn list_skills_finds_skill_md_dirs() {
        let dir = temp_dir("list_md");
        create_skill(&dir, "json_skill", "JSON skill", Some("prompt")).await;
        create_skill_md(&dir, "md_skill", SAMPLE_SKILL_MD).await;

        let loader = test_loader(&dir);
        let mut skills = loader.list_skills().await.unwrap();
        skills.sort();

        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0], "json_skill");
        assert_eq!(skills[1], "md_skill");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn load_skill_from_skill_md() {
        let dir = temp_dir("load_md");
        create_skill_md(&dir, "flow", SAMPLE_SKILL_MD).await;

        let loader = test_loader(&dir);
        let skill = loader.load_skill("flow").await.unwrap();

        assert_eq!(skill.name, "claude-flow");
        assert_eq!(skill.description, "Orchestrate multi-agent swarms");
        assert!(skill.prompt.is_some());
        assert!(skill.user_invocable);
        assert_eq!(skill.allowed_tools.len(), 2);

        // Verify it's cached
        let cached = loader.get_skill("flow").await;
        assert!(cached.is_some());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn load_skill_prefers_skill_json_over_skill_md() {
        let dir = temp_dir("prefer_json");
        // Create both formats in the same directory
        create_skill(&dir, "dual", "From JSON", Some("JSON prompt")).await;
        // Also add a SKILL.md
        tokio::fs::write(
            dir.join("dual").join("SKILL.md"),
            "---\nname: from-md\ndescription: From MD\n---\nMD prompt\n",
        )
        .await
        .unwrap();

        let loader = test_loader(&dir);
        let skill = loader.load_skill("dual").await.unwrap();

        // skill.json should win
        assert_eq!(skill.name, "dual");
        assert_eq!(skill.description, "From JSON");
        assert_eq!(skill.prompt.as_deref(), Some("JSON prompt"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn load_skill_neither_format_returns_error() {
        let dir = temp_dir("neither");
        let skill_dir = dir.join("empty_skill");
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        tokio::fs::write(skill_dir.join("readme.md"), "not a skill")
            .await
            .unwrap();

        let loader = test_loader(&dir);
        let result = loader.load_skill("empty_skill").await;
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("neither skill.json nor SKILL.md"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn load_all_includes_skill_md() {
        let dir = temp_dir("load_all_md");
        create_skill(&dir, "json_one", "JSON", Some("prompt")).await;
        create_skill_md(&dir, "md_one", SAMPLE_SKILL_MD).await;

        let loader = test_loader(&dir);
        loader.load_all().await.unwrap();

        assert!(loader.get_skill("json_one").await.is_some());
        assert!(loader.get_skill("md_one").await.is_some());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
