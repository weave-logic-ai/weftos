//! `weftos init --claude --grok --codex`: render a synthetic agents/ tree into
//! a temp project for all three hosts and check the file set, frontmatter,
//! idempotence and drift handling.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use weftos::init::Host;
use weftos::init::apply::{apply, format_plan};
use weftos::init::plan::{Action, AgentInitOptions, plan};

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

/// Two agents, one skill-only package, a template, a legacy dir and a team.
fn fixture_agents(dir: &Path) {
    write(
        dir,
        "stew/weftos-package.yaml",
        "name: stew\nkind: specialist\nrequired_project_context:\n  - key: board.cmd\n    description: board command\n  - key: goal\n    required: false\n    description: goal object\n",
    );
    write(
        dir,
        "stew/AGENT.md",
        "---\nname: stew\nnickname: Stew\ndescription: >\n  Writes the board. Carefully.\n  Use when: filing work.\ntools: [Read, Grep, Bash]\nmodel_hint: default\n---\n\n# Stew\n\nBody text.\n",
    );
    write(
        dir,
        "stew/skills/board/SKILL.md",
        "---\nname: board\ndescription: Board skill.\n---\n\n# Board\n",
    );
    write(dir, "stew/skills/board/references/notes.md", "notes\n");
    write(dir, "stew/evals/scenarios.md", "not rendered\n");
    write(dir, "rev/weftos-package.yaml", "name: rev\nkind: lane\n");
    write(
        dir,
        "rev/AGENT.md",
        "---\nname: rev\ndescription: \"Reviews: read-only.\"\ntools: Read, Grep\n---\nReview body.\n",
    );
    write(
        dir,
        "rev/skills/review/SKILL.md",
        "---\nname: review\ndescription: Review skill.\n---\nR\n",
    );
    write(
        dir,
        "lead/weftos-package.yaml",
        "name: lead\nkind: skill\nrequired_project_context:\n  - key: handoff_path\n    description: handoff record\n",
    );
    write(
        dir,
        "lead/SKILL.md",
        "---\nname: lead\ndescription: Lead doctrine. Not spawnable.\n---\n# Lead\n",
    );
    write(
        dir,
        "tmpl/weftos-package.yaml",
        "name: tmpl\nkind: template\n",
    );
    write(
        dir,
        "tmpl/AGENT.md",
        "---\nname: tmpl\ndescription: template\n---\nT\n",
    );
    write(dir, "weftos/weftos-package.yaml", "name: legacy\n");
    write(
        dir,
        "weftos/AGENT.md",
        "---\nname: legacy\ndescription: legacy\n---\nL\n",
    );
    write(
        dir,
        "teams/core/team.yaml",
        "name: core\nlead: lead\nmembers:\n  - agent: stew\n  - rev\n  - ghost\n  - agent: tmpl\n    active: false\n",
    );
}

struct Env {
    _tmp: tempfile::TempDir,
    agents: PathBuf,
    project: PathBuf,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let agents = tmp.path().join("agents");
    let project = tmp.path().join("project");
    fixture_agents(&agents);
    fs::create_dir_all(&project).unwrap();
    Env {
        _tmp: tmp,
        agents,
        project,
    }
}

fn opts(e: &Env, apply: bool) -> AgentInitOptions {
    AgentInitOptions {
        root: e.project.clone(),
        hosts: vec![Host::Claude, Host::Grok, Host::Codex],
        apply,
        from: Some(e.agents.clone()),
        team: Some("core".into()),
        ..Default::default()
    }
}

fn run(o: &AgentInitOptions) -> weftos::init::Plan {
    let p = plan(o).unwrap();
    if o.apply {
        apply(&p).unwrap();
    }
    p
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap_or_else(|_| panic!("missing {rel}"))
}

#[test]
fn renders_expected_file_set_for_all_hosts() {
    let e = env();
    let p = run(&opts(&e, true));
    assert_eq!(p.selected, vec!["lead", "rev", "stew"]);
    assert!(
        p.warnings.iter().any(|w| w.contains("ghost")),
        "{:?}",
        p.warnings
    );

    let mut paths: Vec<&str> = p.items.iter().map(|i| i.path.as_str()).collect();
    paths.sort();
    let expected = [
        ".agents/project-context.md",
        ".agents/skills/board/SKILL.md",
        ".agents/skills/board/references/notes.md",
        ".agents/skills/lead/SKILL.md",
        ".agents/skills/review/SKILL.md",
        ".claude/agents/rev.md",
        ".claude/agents/stew.md",
        ".claude/skills/board/SKILL.md",
        ".claude/skills/board/references/notes.md",
        ".claude/skills/lead/SKILL.md",
        ".claude/skills/review/SKILL.md",
        ".codex/agents/rev.toml",
        ".codex/agents/stew.toml",
        ".grok/agents/rev.md",
        ".grok/agents/stew.md",
        ".grok/skills/board/SKILL.md",
        ".grok/skills/board/references/notes.md",
        ".grok/skills/lead/SKILL.md",
        ".grok/skills/review/SKILL.md",
        "AGENTS.md",
    ];
    assert_eq!(paths, expected);
    for rel in expected {
        assert!(e.project.join(rel).is_file(), "{rel} not written");
    }
    // Skill-only packages never become agents; evals, templates, legacy dirs never render.
    assert!(!e.project.join(".claude/agents/lead.md").exists());
    assert!(!e.project.join(".claude/agents/tmpl.md").exists());
    assert!(!e.project.join(".claude/agents/legacy.md").exists());
    assert!(e.project.join(".weftos/agents.lock.json").is_file());
}

#[test]
fn host_frontmatter_matches_each_host() {
    let e = env();
    run(&opts(&e, true));
    let claude = read(&e.project, ".claude/agents/stew.md");
    assert!(claude.starts_with(
        "---\nname: stew\ndescription: \"Writes the board. Carefully. Use when: filing work.\"\n"
    ));
    assert!(claude.contains("\ntools: Read, Grep, Bash\n"));
    assert!(claude.contains("\nskills: board\n"));
    assert!(claude.contains("\nmodel: inherit\n---\n\n# Stew\n\nBody text.\n"));
    assert!(!claude.contains("model_hint"));

    let grok = read(&e.project, ".grok/agents/stew.md");
    assert!(grok.starts_with("---\nname: stew\ndescription: "));
    assert!(grok.contains("\nprompt_mode: full\nagents_md: true\n---\n"));
    assert!(!grok.contains("\ntools:"));
    assert!(grok.ends_with("# Stew\n\nBody text.\n"));

    let codex: toml::Value = toml::from_str(&read(&e.project, ".codex/agents/stew.toml")).unwrap();
    assert_eq!(codex["name"].as_str(), Some("stew"));
    assert!(
        codex["description"]
            .as_str()
            .unwrap()
            .starts_with("Writes the board.")
    );
    let instr = codex["developer_instructions"].as_str().unwrap();
    assert!(instr.contains("`board`") && instr.ends_with("# Stew\n\nBody text.\n"));

    // Skills are verbatim.
    assert_eq!(
        read(&e.project, ".claude/skills/board/SKILL.md"),
        read(&e.agents, "stew/skills/board/SKILL.md")
    );
    assert_eq!(
        read(&e.project, ".agents/skills/lead/SKILL.md"),
        read(&e.agents, "lead/SKILL.md")
    );

    let agents_md = read(&e.project, "AGENTS.md");
    assert!(
        agents_md
            .contains("- **stew** (Stew): Writes the board. Agent: `.codex/agents/stew.toml`.")
    );
    assert!(agents_md.contains("- **lead** (skill, not a spawnable agent): Lead doctrine."));

    let ctx = read(&e.project, ".agents/project-context.md");
    assert!(
        ctx.contains("`board.cmd`: TODO")
            && ctx.contains("`goal` (optional)")
            && ctx.contains("`handoff_path`")
    );
}

#[test]
fn second_apply_is_a_noop() {
    let e = env();
    run(&opts(&e, true));
    let lock_before = read(&e.project, ".weftos/agents.lock.json");
    let p = run(&opts(&e, true));
    assert!(p.is_noop(), "{}", format_plan(&p, true));
    assert_eq!(p.count(Action::Unchanged), 19);
    assert_eq!(p.count(Action::Keep), 1);
    assert_eq!(read(&e.project, ".weftos/agents.lock.json"), lock_before);
    assert!(format_plan(&p, true).contains("No changes"));
}

#[test]
fn agents_md_block_preserves_user_content() {
    let e = env();
    write(&e.project, "AGENTS.md", "# House rules\n\nBe kind.\n");
    run(&opts(&e, true));
    let text = read(&e.project, "AGENTS.md");
    assert!(text.starts_with("# House rules\n\nBe kind.\n\n<!-- weftos:agents:begin -->"));
    // Edits outside the block are not drift.
    fs::write(
        e.project.join("AGENTS.md"),
        format!("{text}\nMore notes.\n"),
    )
    .unwrap();
    assert!(plan(&opts(&e, false)).unwrap().is_noop());
}

#[test]
fn drift_is_reported_and_kept_unless_forced() {
    let e = env();
    run(&opts(&e, true));
    let rel = ".claude/agents/stew.md";
    fs::write(e.project.join(rel), "local edit\n").unwrap();
    // Unmanaged: a file WeftOS never wrote.
    fs::remove_file(e.project.join(".weftos/agents.lock.json")).unwrap();
    let p = plan(&opts(&e, false)).unwrap();
    assert!(
        p.items
            .iter()
            .any(|i| i.path == rel && i.action == Action::Unmanaged)
    );
    run(&opts(&e, true)); // re-adopts the untouched files, keeps the edit
    assert_eq!(read(&e.project, rel), "local edit\n");

    // Managed then locally edited: drifted.
    let mut forced = opts(&e, true);
    forced.force = true;
    run(&forced);
    fs::write(e.project.join(rel), "edited again\n").unwrap();
    let p = run(&opts(&e, true));
    let item = p.items.iter().find(|i| i.path == rel).unwrap();
    assert_eq!(item.action, Action::Drifted);
    assert_eq!(read(&e.project, rel), "edited again\n");
    assert!(format_plan(&p, true).contains("--force"));
    // Still drifted on the next run (lock keeps the old hash).
    let p = plan(&opts(&e, false)).unwrap();
    assert_eq!(
        p.items.iter().find(|i| i.path == rel).unwrap().action,
        Action::Drifted
    );

    let p = run(&forced);
    assert_eq!(
        p.items.iter().find(|i| i.path == rel).unwrap().action,
        Action::Overwrite
    );
    assert!(read(&e.project, rel).starts_with("---\nname: stew\n"));
    assert!(plan(&opts(&e, false)).unwrap().is_noop());
}

#[test]
fn source_update_and_removal_are_applied() {
    let e = env();
    run(&opts(&e, true));
    write(
        &e.agents,
        "stew/AGENT.md",
        "---\nname: stew\ndescription: New.\ntools: [Read]\n---\nNew body.\n",
    );
    fs::remove_file(e.agents.join("stew/skills/board/references/notes.md")).unwrap();
    let p = run(&opts(&e, true));
    let action = |path: &str| p.items.iter().find(|i| i.path == path).map(|i| i.action);
    assert_eq!(action(".claude/agents/stew.md"), Some(Action::Update));
    assert_eq!(
        action(".grok/skills/board/references/notes.md"),
        Some(Action::Delete)
    );
    assert!(
        !e.project.join(".grok/skills/board/references").exists(),
        "empty dir pruned"
    );
    assert!(read(&e.project, ".codex/agents/stew.toml").contains("New body."));
    assert!(plan(&opts(&e, false)).unwrap().is_noop());
}

#[test]
fn single_agent_install_is_cumulative_and_plan_writes_nothing() {
    let e = env();
    let mut o = opts(&e, false);
    o.team = None;
    o.agents = vec!["rev".into()];
    o.hosts = vec![Host::Claude];
    let p = run(&o);
    assert!(!p.is_noop());
    assert!(
        fs::read_dir(&e.project).unwrap().next().is_none(),
        "--plan wrote files"
    );

    o.apply = true;
    run(&o);
    o.agents = vec!["stew".into()];
    let p = run(&o);
    assert_eq!(p.selected, vec!["rev", "stew"]);
    assert!(e.project.join(".claude/agents/rev.md").is_file());
    assert!(!e.project.join(".grok").exists());

    o.agents = vec!["nope".into()];
    assert!(plan(&o).is_err());
}

#[test]
fn missing_team_falls_back_to_all_non_template_packages() {
    let e = env();
    let mut o = opts(&e, false);
    o.team = Some("absent".into());
    let p = plan(&o).unwrap();
    assert_eq!(p.selected, vec!["lead", "rev", "stew"]);
    assert!(p.warnings.iter().any(|w| w.contains("not found")));
}

#[test]
fn cli_plan_then_apply_then_noop() {
    let e = env();
    let bin = env!("CARGO_BIN_EXE_weftos");
    let cli = |extra: &[&str]| {
        let out = Command::new(bin)
            .arg("init")
            .arg(&e.project)
            .args(["--claude", "--grok", "--codex", "--team", "core", "--from"])
            .arg(&e.agents)
            .args(extra)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    assert!(cli(&["--plan"]).contains("Plan only."));
    assert!(!e.project.join(".claude").exists());
    assert!(cli(&["--apply"]).contains("19 create"));
    assert!(cli(&["--apply"]).contains("No changes"));

    let out = Command::new(bin)
        .arg("init")
        .arg(&e.project)
        .arg("--apply")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "--apply without a host flag must fail"
    );
}
