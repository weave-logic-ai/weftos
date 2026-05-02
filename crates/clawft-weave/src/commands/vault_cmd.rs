//! `weaver vault` subcommand — Obsidian vault cultivation.
//!
//! - `weaver vault enrich <path>`    -- add/update YAML frontmatter for all .md files
//! - `weaver vault analyze <path>`   -- link graph analysis (orphans, clusters, density)
//! - `weaver vault suggest <path>`   -- suggest new connections between documents
//! - `weaver vault auto-link <path>` -- insert wikilinks for known document titles
//! - `weaver vault backlinks <path>` -- generate backlink sections in documents

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "Obsidian vault cultivation — frontmatter, links, and graph analysis")]
pub struct VaultArgs {
    #[command(subcommand)]
    pub action: VaultAction,
}

#[derive(Subcommand)]
pub enum VaultAction {
    /// Add or update YAML frontmatter for markdown files.
    Enrich {
        /// Path to the vault directory.
        path: PathBuf,
        /// Overwrite existing frontmatter fields (default: only fill empty fields).
        #[arg(long)]
        force: bool,
    },
    /// Analyze the vault link graph (orphans, clusters, broken links).
    Analyze {
        /// Path to the vault directory.
        path: PathBuf,
        /// Output format: table (default) or json.
        #[arg(short, long, default_value = "table")]
        format: String,
    },
    /// Suggest new connections between documents.
    Suggest {
        /// Path to the vault directory.
        path: PathBuf,
        /// Minimum score threshold (default: 5.0).
        #[arg(long, default_value_t = 5.0)]
        min_score: f64,
        /// Max suggestions per file (default: 5).
        #[arg(long, default_value_t = 5)]
        max_per_file: usize,
    },
    /// Insert wikilinks for known document titles in the vault.
    AutoLink {
        /// Path to the vault directory.
        path: PathBuf,
        /// Dry-run: show what would change without modifying files.
        #[arg(long)]
        dry_run: bool,
    },
    /// Generate backlink sections in vault documents.
    Backlinks {
        /// Path to the vault directory.
        path: PathBuf,
        /// Dry-run: show what would change without modifying files.
        #[arg(long)]
        dry_run: bool,
    },
}

pub async fn run(args: VaultArgs) -> anyhow::Result<()> {
    match args.action {
        VaultAction::Enrich { path, force } => cmd_enrich(&path, force),
        VaultAction::Analyze { path, format } => cmd_analyze(&path, &format),
        VaultAction::Suggest { path, min_score, max_per_file } => {
            cmd_suggest(&path, min_score, max_per_file)
        }
        VaultAction::AutoLink { path, dry_run } => cmd_auto_link(&path, dry_run),
        VaultAction::Backlinks { path, dry_run } => cmd_backlinks(&path, dry_run),
    }
}

fn cmd_enrich(vault_path: &PathBuf, force: bool) -> anyhow::Result<()> {
    use clawft_graphify::vault::frontmatter;

    let files = collect_markdown(vault_path)?;
    let mut enriched = 0usize;
    let mut skipped = 0usize;

    for file_path in &files {
        let content = std::fs::read_to_string(file_path)?;
        let mut doc = frontmatter::parse(&content);

        let had_frontmatter = content.starts_with("---");
        if had_frontmatter && !force {
            let fm = &doc.frontmatter;
            if fm.title.is_some() && !fm.tags.is_empty() && fm.r#type.is_some() {
                skipped += 1;
                continue;
            }
        }

        frontmatter::enrich(&mut doc, file_path);

        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        if doc.frontmatter.created.is_none() {
            doc.frontmatter.created = Some(now.clone());
        }
        doc.frontmatter.updated = Some(now);

        let output = frontmatter::render(&doc);
        std::fs::write(file_path, output)?;
        enriched += 1;
    }

    println!("Enriched {enriched} files, skipped {skipped} (already complete).");
    Ok(())
}

fn cmd_analyze(vault_path: &std::path::Path, format: &str) -> anyhow::Result<()> {
    use clawft_graphify::vault::analyze;

    let (_nodes, metrics) = analyze::analyze_vault(vault_path)?;

    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&metrics)?);
        return Ok(());
    }

    let mut table = comfy_table::Table::new();
    table.set_header(vec!["Metric", "Value"]);
    table.add_row(vec!["Total files", &metrics.total_files.to_string()]);
    table.add_row(vec!["Files with links", &metrics.files_with_links.to_string()]);
    table.add_row(vec!["Total links", &metrics.total_links.to_string()]);
    table.add_row(vec!["Avg links/file", &format!("{:.1}", metrics.avg_links_per_file)]);
    table.add_row(vec!["Max links", &format!("{} ({})", metrics.max_links_in_file, metrics.max_links_file)]);
    table.add_row(vec!["Orphan files", &format!("{} ({:.0}%)", metrics.orphan_files.len(), metrics.orphan_rate * 100.0)]);
    table.add_row(vec!["Clusters", &metrics.clusters.to_string()]);
    table.add_row(vec!["Broken links", &metrics.broken_links.len().to_string()]);
    table.add_row(vec!["Link density", &format!("{:.2}", metrics.link_density)]);
    println!("{table}");

    let orphan_ok = metrics.orphan_rate < 0.10;
    let density_ok = metrics.link_density > 5.0;

    println!("\nHealth:");
    println!(
        "  Orphan rate < 10%: {} ({:.0}%)",
        if orphan_ok { "PASS" } else { "FAIL" },
        metrics.orphan_rate * 100.0,
    );
    println!(
        "  Link density > 5.0: {} ({:.2})",
        if density_ok { "PASS" } else { "FAIL" },
        metrics.link_density,
    );

    if !metrics.broken_links.is_empty() {
        println!("\nBroken links:");
        for bl in &metrics.broken_links {
            println!("  {} -> {}", bl.source, bl.target);
        }
    }

    if !metrics.orphan_files.is_empty() && metrics.orphan_files.len() <= 20 {
        println!("\nOrphan files:");
        for f in &metrics.orphan_files {
            println!("  {f}");
        }
    }

    Ok(())
}

fn cmd_suggest(vault_path: &std::path::Path, min_score: f64, max_per_file: usize) -> anyhow::Result<()> {
    use clawft_graphify::vault::{analyze, suggest};

    let (nodes, _metrics) = analyze::analyze_vault(vault_path)?;
    let config = suggest::SuggestConfig {
        min_score,
        max_per_file,
        ..Default::default()
    };

    let suggestions = suggest::suggest_links(&nodes, &config);

    if suggestions.is_empty() {
        println!("No suggestions above score {min_score}.");
        return Ok(());
    }

    let mut table = comfy_table::Table::new();
    table.set_header(vec!["Score", "Source", "Target", "Reason", "Bidir"]);

    for s in &suggestions {
        table.add_row(vec![
            format!("{:.1}", s.score),
            s.source.clone(),
            s.target.clone(),
            s.reason.clone(),
            if s.bidirectional { "yes" } else { "no" }.to_string(),
        ]);
    }

    println!("{table}");
    println!("\n{} suggestions found.", suggestions.len());

    Ok(())
}

fn cmd_auto_link(vault_path: &PathBuf, dry_run: bool) -> anyhow::Result<()> {
    use clawft_graphify::vault::{frontmatter, links};

    let files = collect_markdown(vault_path)?;

    // Collect all known titles/filenames.
    let mut titles: Vec<String> = Vec::new();
    for file_path in &files {
        let content = std::fs::read_to_string(file_path)?;
        let doc = frontmatter::parse(&content);
        if let Some(title) = &doc.frontmatter.title
            && title.len() >= 3 {
                titles.push(title.clone());
            }
        if let Some(stem) = file_path.file_stem().and_then(|s| s.to_str()) {
            let name = stem.replace(['-', '_'], " ");
            if name.len() >= 3 && !titles.contains(&name) {
                titles.push(name);
            }
        }
    }

    let mut linked = 0usize;

    for file_path in &files {
        let content = std::fs::read_to_string(file_path)?;
        let doc = frontmatter::parse(&content);
        let new_body = links::auto_link(&doc.body, &titles);

        if new_body != doc.body {
            let rel = file_path.strip_prefix(vault_path).unwrap_or(file_path);
            if dry_run {
                println!("Would link: {}", rel.display());
            } else {
                let new_doc = frontmatter::Document {
                    frontmatter: doc.frontmatter,
                    body: new_body,
                };
                std::fs::write(file_path, frontmatter::render(&new_doc))?;
                println!("Linked: {}", rel.display());
            }
            linked += 1;
        }
    }

    if dry_run {
        println!("\n{linked} files would be modified (dry-run).");
    } else {
        println!("\n{linked} files updated with new wikilinks.");
    }

    Ok(())
}

fn cmd_backlinks(vault_path: &PathBuf, dry_run: bool) -> anyhow::Result<()> {
    use clawft_graphify::vault::{analyze, frontmatter, links};

    let (nodes, _metrics) = analyze::analyze_vault(vault_path)?;
    let mut updated = 0usize;

    for node in nodes.values() {
        if node.incoming.is_empty() {
            continue;
        }

        let content = std::fs::read_to_string(&node.path)?;

        // Skip if already has backlinks section.
        if content.contains("## Backlinks") {
            continue;
        }

        let backlink_entries: Vec<(String, String)> = node.incoming.iter().filter_map(|src_key| {
            let src_node = nodes.get(src_key)?;
            let title = src_node.title.as_deref().unwrap_or(src_key);
            let stem = std::path::Path::new(src_key).file_stem()?.to_str()?;
            Some((stem.to_string(), title.to_string()))
        }).collect();

        if backlink_entries.is_empty() {
            continue;
        }

        let section = links::render_backlinks_section(&backlink_entries);
        let mut doc = frontmatter::parse(&content);
        doc.body.push_str(&section);
        doc.body.push('\n');

        let rel = node.path.strip_prefix(vault_path).unwrap_or(&node.path);
        if dry_run {
            println!("Would add {} backlinks to: {}", backlink_entries.len(), rel.display());
        } else {
            std::fs::write(&node.path, frontmatter::render(&doc))?;
            println!("Added {} backlinks to: {}", backlink_entries.len(), rel.display());
        }
        updated += 1;
    }

    if dry_run {
        println!("\n{updated} files would be modified (dry-run).");
    } else {
        println!("\n{updated} files updated with backlinks.");
    }

    Ok(())
}

fn collect_markdown(dir: &PathBuf) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_recursive(dir, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_recursive(dir: &PathBuf, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with('.') || name_str == "node_modules" || name_str == "dist" || name_str == "target" {
            continue;
        }

        if path.is_dir() {
            collect_recursive(&path.to_path_buf(), out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path);
        }
    }
    Ok(())
}
