//! `weaver graphify` subcommand implementation.
//!
//! Provides knowledge graph commands:
//! - `weaver graphify ingest <path|url>`  -- run extraction pipeline
//! - `weaver graphify query <question>`   -- semantic search against the graph
//! - `weaver graphify export <format>`    -- export graph to file
//! - `weaver graphify diff`               -- compare current vs cached graph
//! - `weaver graphify rebuild`            -- force full re-extraction
//! - `weaver graphify watch`              -- start file watcher
//! - `weaver graphify hooks install|uninstall|status` -- manage git hooks

use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::path::PathBuf;

/// Knowledge graph management subcommand.
#[derive(Parser)]
#[command(about = "Knowledge graph extraction, query, and export (graphify)")]
pub struct GraphifyArgs {
    #[command(subcommand)]
    pub action: GraphifyAction,
}

/// Graphify subcommands.
#[derive(Subcommand)]
pub enum GraphifyAction {
    /// Ingest a local path or URL into the knowledge graph.
    Ingest {
        /// Path to a local file/directory or a URL to fetch.
        target: String,

        /// Output directory for ingested files.
        #[arg(short, long, default_value = "graphify-out/memory")]
        output: PathBuf,

        /// Contributor name for metadata.
        #[arg(long)]
        contributor: Option<String>,
    },

    /// Search the knowledge graph with a natural-language question.
    Query {
        /// The question or keyword search.
        question: String,

        /// Graph JSON path.
        #[arg(short, long, default_value = "graphify-out/graph.json")]
        graph: PathBuf,

        /// Traversal mode: bfs or dfs.
        #[arg(short, long, default_value = "bfs")]
        mode: String,

        /// Traversal depth (1-6).
        #[arg(short, long, default_value_t = 3)]
        depth: usize,
    },

    /// Export the knowledge graph to a file.
    Export {
        /// Export format: json, graphml, cypher, html, obsidian, wiki, svg.
        format: String,

        /// Output path (default: graphify-out/<format>).
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Graph JSON path.
        #[arg(short, long, default_value = "graphify-out/graph.json")]
        graph: PathBuf,
    },

    /// Compare the current graph against a cached/previous version.
    Diff {
        /// Path to the old graph JSON.
        #[arg(default_value = "graphify-out/graph.json.bak")]
        old: PathBuf,

        /// Path to the current graph JSON.
        #[arg(default_value = "graphify-out/graph.json")]
        current: PathBuf,
    },

    /// Force a full re-extraction of the knowledge graph.
    Rebuild {
        /// Root directory to scan.
        #[arg(default_value = ".")]
        root: PathBuf,

        /// Clear cache before rebuilding.
        #[arg(long)]
        clean: bool,
    },

    /// Start the file watcher for automatic re-extraction.
    Watch {
        /// Root directory to watch.
        #[arg(default_value = ".")]
        root: PathBuf,

        /// Debounce window in seconds.
        #[arg(short, long, default_value_t = 2.0)]
        debounce: f64,
    },

    /// Manage git hooks for automatic graph rebuilding.
    Hooks {
        #[command(subcommand)]
        action: HooksAction,
    },
}

/// Git hook management subcommands.
#[derive(Subcommand)]
pub enum HooksAction {
    /// Install graphify post-commit and post-checkout hooks.
    Install {
        /// Repository root (default: current directory).
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Remove graphify hooks.
    Uninstall {
        /// Repository root (default: current directory).
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Check hook installation status.
    Status {
        /// Repository root (default: current directory).
        #[arg(default_value = ".")]
        path: PathBuf,
    },
}

/// Run the graphify subcommand.
pub async fn run(args: GraphifyArgs) -> anyhow::Result<()> {
    match args.action {
        GraphifyAction::Ingest { target, output, contributor } => {
            run_ingest(&target, &output, contributor.as_deref()).await
        }
        GraphifyAction::Query { question, graph, mode, depth } => {
            run_query(&question, &graph, &mode, depth).await
        }
        GraphifyAction::Export { format, output, graph } => {
            run_export(&format, output.as_deref(), &graph).await
        }
        GraphifyAction::Diff { old, current } => {
            run_diff(&old, &current).await
        }
        GraphifyAction::Rebuild { root, clean } => {
            run_rebuild(&root, clean).await
        }
        GraphifyAction::Watch { root, debounce } => {
            run_watch(&root, debounce).await
        }
        GraphifyAction::Hooks { action } => {
            run_hooks(action).await
        }
    }
}

// ---------------------------------------------------------------------------
// Subcommand implementations
// ---------------------------------------------------------------------------

async fn run_ingest(
    target: &str,
    output: &std::path::Path,
    contributor: Option<&str>,
) -> anyhow::Result<()> {
    // Detect if target is a URL or local path.
    if target.starts_with("http://") || target.starts_with("https://") {
        println!("Ingesting URL: {target}");
        // URL ingestion uses the graphify ingest module.
        // In production, this would use a real HTTP client.
        // For now, report the action.
        use clawft_graphify::ingest;
        let client = ingest::StubHttpClient;
        match ingest::ingest(target, output, &client, contributor) {
            Ok(result) => {
                println!("Saved {:?}: {}", result.url_type, result.path.display());
            }
            Err(e) => {
                eprintln!("Ingest failed: {e}");
                std::process::exit(1);
            }
        }
    } else {
        println!("Ingesting local path: {target}");
        let path = std::path::Path::new(target);
        if !path.exists() {
            anyhow::bail!("Path does not exist: {target}");
        }
        if path.is_dir() {
            run_graphify_pipeline(path)?;
        } else {
            println!("Single-file ingestion not yet supported. Pass a directory.");
            println!("Hint: use `weaver graphify rebuild` to scan from the project root.");
        }
    }
    Ok(())
}

async fn run_query(
    question: &str,
    graph_path: &std::path::Path,
    mode: &str,
    depth: usize,
) -> anyhow::Result<()> {
    if !graph_path.exists() {
        anyhow::bail!(
            "Graph file not found: {}. Run `weaver graphify rebuild` first.",
            graph_path.display()
        );
    }

    println!("Querying graph: {}", graph_path.display());
    println!("Question: {question}");
    println!("Mode: {mode}, Depth: {depth}");

    // Load graph into KnowledgeGraph for structured queries.
    let data = std::fs::read_to_string(graph_path)?;
    let json_value: serde_json::Value = serde_json::from_str(&data)?;

    // Remap "links" -> "edges" if needed for build_from_json.
    let json_for_build = if json_value.get("edges").is_none() && json_value.get("links").is_some() {
        let mut obj = json_value.clone();
        if let Some(links) = obj.get("links").cloned() {
            obj.as_object_mut().unwrap().insert("edges".to_string(), links);
        }
        obj
    } else {
        json_value
    };

    let kg = clawft_graphify::build::build_from_json(&json_for_build)
        .map_err(|e| anyhow::anyhow!("Failed to load graph: {e}"))?;

    if kg.node_count() == 0 {
        println!("No nodes found in graph.");
        return Ok(());
    }

    // Run community detection for proximity and community features.
    let communities = clawft_graphify::cluster::cluster(&kg);
    let community_labels = clawft_graphify::cluster::auto_label_all(&kg, &communities);
    let community_summaries =
        clawft_graphify::summary::generate_community_summaries(&kg, &communities, &community_labels);

    // Build node -> community map for quick lookups.
    let node_community = clawft_graphify::analyze::node_community_map(&communities);

    let terms: Vec<String> = question
        .split_whitespace()
        .filter(|t| t.len() > 2)
        .map(|t| t.to_lowercase())
        .collect();

    if terms.is_empty() {
        println!("Query too short (all terms <= 2 chars). Try a longer question.");
        return Ok(());
    }

    // --- KG-002: Community summary search for global queries ---
    let summary_hits = clawft_graphify::summary::search_summaries(&community_summaries, question);
    if !summary_hits.is_empty() {
        println!("\nCommunity matches:");
        for (cid, score) in summary_hits.iter().take(3) {
            if let Some(s) = community_summaries.get(cid) {
                println!("  [community {cid}, score={score:.1}] {}", s.label);
                println!("    {}", s.description);
            }
        }
    }

    // --- KG-001: Hybrid score fusion ---
    let fusion = clawft_graphify::eml_models::QueryFusionModel::new();

    // 1. Keyword scoring (existing logic, per entity).
    let mut keyword_scores: HashMap<clawft_graphify::EntityId, f64> = HashMap::new();
    let mut max_keyword = 0.0_f64;
    for id in kg.entity_ids() {
        if let Some(entity) = kg.entity(id) {
            let label = entity.label.to_lowercase();
            let source = entity
                .source_file
                .as_deref()
                .unwrap_or("")
                .to_lowercase();
            let score: f64 = terms
                .iter()
                .map(|t| {
                    (if label.contains(t.as_str()) { 1.0 } else { 0.0 })
                        + (if source.contains(t.as_str()) { 0.5 } else { 0.0 })
                })
                .sum();
            if score > 0.0 {
                keyword_scores.insert(id.clone(), score);
                if score > max_keyword {
                    max_keyword = score;
                }
            }
        }
    }

    // 2. Graph proximity: for matched nodes, boost 1-hop neighbors.
    let mut proximity_scores: HashMap<clawft_graphify::EntityId, f64> = HashMap::new();
    for (id, kw_score) in &keyword_scores {
        let neighbors = kg.neighbors(id);
        let bonus = kw_score * 0.5; // neighbors get 50% of parent's keyword score
        for neighbor in neighbors {
            *proximity_scores.entry(neighbor.id.clone()).or_insert(0.0) += bonus;
        }
    }
    let max_proximity = proximity_scores
        .values()
        .copied()
        .fold(0.0_f64, f64::max)
        .max(1.0);

    // 3. Community context: nodes sharing a community with top keyword matches
    //    get a boost.
    let top_match_community: Option<usize> = keyword_scores
        .iter()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .and_then(|(id, _)| node_community.get(id).copied());

    // 4. Entity type relevance: code entities score higher than generic ones.
    let code_types = ["function", "module", "class", "method"];

    // 5. Fuse all scores and rank.
    let mut all_candidate_ids: std::collections::HashSet<clawft_graphify::EntityId> =
        std::collections::HashSet::new();
    all_candidate_ids.extend(keyword_scores.keys().cloned());
    all_candidate_ids.extend(proximity_scores.keys().cloned());

    #[allow(dead_code)] // `id` is carried for provenance even though display paths only use the label
    struct ScoredResult {
        id: clawft_graphify::EntityId,
        fused_score: f64,
        keyword_score: f64,
        proximity_score: f64,
        community_score: f64,
        label: String,
        source_file: String,
        community_id: Option<usize>,
    }

    let mut results: Vec<ScoredResult> = all_candidate_ids
        .into_iter()
        .filter_map(|id| {
            let entity = kg.entity(&id)?;
            let kw = keyword_scores.get(&id).copied().unwrap_or(0.0);
            let prox = proximity_scores.get(&id).copied().unwrap_or(0.0);
            let cid = node_community.get(&id).copied();

            // Normalize scores to [0, 1].
            let kw_norm = if max_keyword > 0.0 {
                kw / max_keyword
            } else {
                0.0
            };
            let prox_norm = prox / max_proximity;
            let community_score = match (cid, top_match_community) {
                (Some(c), Some(top_c)) if c == top_c => 1.0,
                _ => 0.0,
            };
            let type_str = entity.entity_type.discriminant().to_lowercase();
            let entity_type_relevance = if code_types
                .iter()
                .any(|t| type_str.contains(t))
            {
                1.0
            } else {
                0.5
            };

            let features = [kw_norm, prox_norm, community_score, entity_type_relevance];
            let fused = fusion.fuse(&features);

            // Skip nodes with zero fused score.
            if fused <= 0.0 {
                return None;
            }

            Some(ScoredResult {
                id,
                fused_score: fused,
                keyword_score: kw,
                proximity_score: prox,
                community_score,
                label: entity.label.clone(),
                source_file: entity.source_file.clone().unwrap_or_default(),
                community_id: cid,
            })
        })
        .collect();

    results.sort_by(|a, b| {
        b.fused_score
            .partial_cmp(&a.fused_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    if results.is_empty() {
        println!("\nNo matching nodes found.");
    } else {
        println!("\nMatching nodes (hybrid fusion):");
        for r in results.iter().take(10) {
            let cid_str = r
                .community_id
                .map(|c| c.to_string())
                .unwrap_or_default();
            let explanation = format!(
                "kw={:.2} prox={:.2} comm={:.0} type_rel",
                r.keyword_score, r.proximity_score, r.community_score
            );
            println!(
                "  [{:.3}] {} (src={}, community={}) [{}]",
                r.fused_score, r.label, r.source_file, cid_str, explanation
            );
        }
    }

    Ok(())
}

async fn run_export(
    format: &str,
    output: Option<&std::path::Path>,
    graph_path: &std::path::Path,
) -> anyhow::Result<()> {
    if !graph_path.exists() {
        anyhow::bail!(
            "Graph file not found: {}. Run `weaver graphify rebuild` first.",
            graph_path.display()
        );
    }

    let format_lower = format.to_lowercase();
    let default_output = PathBuf::from(match format_lower.as_str() {
        "json" => "graphify-out/graph.json",
        "obsidian" => "graphify-out/obsidian",
        "wiki" => "graphify-out/wiki",
        "html" => "graphify-out/graph.html",
        "graphml" => "graphify-out/graph.graphml",
        "cypher" => "graphify-out/graph.cypher",
        "svg" => "graphify-out/graph.svg",
        _ => "graphify-out/export",
    });

    let output = output.unwrap_or(&default_output);

    println!("Exporting graph as {format} to {}", output.display());
    println!("Source: {}", graph_path.display());

    // Parse the export format
    let export_format = clawft_graphify::export::ExportFormat::from_str_loose(&format_lower)
        .ok_or_else(|| anyhow::anyhow!(
            "Unknown export format: {format}. Supported: json, graphml, cypher, html, obsidian, svg, wiki"
        ))?;

    // Load the graph JSON and deserialize into a KnowledgeGraph
    let data = std::fs::read_to_string(graph_path)?;
    let json_value: serde_json::Value = serde_json::from_str(&data)?;

    // build_from_json expects "nodes" and "edges" keys, but our export uses
    // "nodes" and "links". Remap "links" -> "edges" if needed.
    let json_for_build = if json_value.get("edges").is_none() && json_value.get("links").is_some() {
        let mut obj = json_value.clone();
        if let Some(links) = obj.get("links").cloned() {
            obj.as_object_mut().unwrap().insert("edges".to_string(), links);
        }
        obj
    } else {
        json_value
    };

    let kg = clawft_graphify::build::build_from_json(&json_for_build)
        .map_err(|e| anyhow::anyhow!("Failed to load graph: {e}"))?;

    // Ensure parent directory exists for the output
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }

    clawft_graphify::export::export(&kg, export_format, output)
        .map_err(|e| anyhow::anyhow!("Export failed: {e}"))?;

    println!("Export complete: {}", output.display());
    Ok(())
}

async fn run_diff(
    old_path: &std::path::Path,
    current_path: &std::path::Path,
) -> anyhow::Result<()> {
    if !current_path.exists() {
        anyhow::bail!("Current graph not found: {}", current_path.display());
    }

    println!("Comparing graphs:");
    println!("  Old:     {}", old_path.display());
    println!("  Current: {}", current_path.display());

    if !old_path.exists() {
        println!("No previous graph found -- this is the first build.");
        return Ok(());
    }

    let old_data: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(old_path)?)?;
    let cur_data: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(current_path)?)?;

    let old_nodes = old_data["nodes"].as_array().map(|a| a.len()).unwrap_or(0);
    let cur_nodes = cur_data["nodes"].as_array().map(|a| a.len()).unwrap_or(0);
    // Prefer "edges" (current writer); fall back to "links" for old graph.json
    // files produced before the writer was fixed.
    let count_edges = |v: &serde_json::Value| -> usize {
        v.get("edges")
            .or_else(|| v.get("links"))
            .and_then(|a| a.as_array())
            .map(|a| a.len())
            .unwrap_or(0)
    };
    let old_edges = count_edges(&old_data);
    let cur_edges = count_edges(&cur_data);

    let node_diff = cur_nodes as i64 - old_nodes as i64;
    let edge_diff = cur_edges as i64 - old_edges as i64;

    println!("\nGraph diff:");
    println!("  Nodes: {old_nodes} -> {cur_nodes} ({node_diff:+})");
    println!("  Edges: {old_edges} -> {cur_edges} ({edge_diff:+})");

    Ok(())
}

async fn run_rebuild(root: &std::path::Path, clean: bool) -> anyhow::Result<()> {
    let graph_path = root.join("graphify-out").join("graph.json");
    let manifest_path = root.join("graphify-out").join("manifest.json");

    if clean {
        println!("Clearing cache...");
        let cache_dir = root.join(".weftos").join("graphify-cache");
        if cache_dir.exists() {
            std::fs::remove_dir_all(&cache_dir)?;
            println!("Cache cleared.");
        }
        // Also remove manifest to force full rebuild
        if manifest_path.exists() {
            std::fs::remove_file(&manifest_path)?;
        }
    }

    // If a previous graph and manifest exist, run incremental.
    if !clean && graph_path.exists() && manifest_path.exists() {
        println!("Previous graph found -- running incremental update...");
        run_graphify_pipeline_incremental(root, &graph_path, &manifest_path)
    } else {
        println!("Rebuilding knowledge graph from: {}", root.display());
        run_graphify_pipeline(root)
    }
}

/// Shared extraction pipeline used by both `rebuild` and local `ingest`.
fn run_graphify_pipeline(root: &std::path::Path) -> anyhow::Result<()> {
    use clawft_graphify::extract::detect;
    use clawft_graphify::pipeline::{Pipeline, PipelineConfig};
    use clawft_graphify::export;
    use clawft_graphify::report;

    // 1. Detect files
    println!("Scanning files...");
    let detection = detect::detect(root)
        .map_err(|e| anyhow::anyhow!("Detection failed: {e}"))?;

    println!(
        "Detected {} files ({} code, {} doc, {} paper, {} image)",
        detection.total_files,
        detection.files.get("code").map(|v| v.len()).unwrap_or(0),
        detection.files.get("document").map(|v| v.len()).unwrap_or(0),
        detection.files.get("paper").map(|v| v.len()).unwrap_or(0),
        detection.files.get("image").map(|v| v.len()).unwrap_or(0),
    );

    if !detection.skipped_sensitive.is_empty() {
        println!(
            "Skipped {} sensitive file(s)",
            detection.skipped_sensitive.len()
        );
    }

    if let Some(ref warning) = detection.warning {
        println!("Note: {warning}");
    }

    // 2. Build extraction results from detected files.
    //    Without tree-sitter (ast-extract feature), we create file-level
    //    entities from detection results rather than parsing ASTs.
    let extractions = build_extractions_from_detection(&detection);

    // Convert detect::DetectionResult -> model::DetectionResult for pipeline
    let pipeline_detection = clawft_graphify::model::DetectionResult {
        total_files: detection.total_files,
        total_words: detection.total_words,
        warning: detection.warning.clone(),
    };

    // 3. Run the pipeline: build -> cluster -> analyze
    let config = PipelineConfig::default();
    let pipeline = Pipeline::new(config);
    let result = pipeline
        .run_from_extractions(extractions, pipeline_detection.clone())
        .map_err(|e| anyhow::anyhow!("Pipeline failed: {e}"))?;

    // 4. Ensure output directory exists
    let out_dir = root.join("graphify-out");
    std::fs::create_dir_all(&out_dir)?;

    // 5. Store community assignments and summaries on the graph for export
    let mut graph = result.graph;
    if let Some(ref analysis) = result.analysis {
        graph.communities = Some(analysis.communities.clone());
        graph.community_summaries = Some(analysis.community_summaries.clone());
    }

    // 6. Export to JSON
    let json_path = out_dir.join("graph.json");
    export::export(
        &graph,
        export::ExportFormat::Json,
        &json_path,
    )
    .map_err(|e| anyhow::anyhow!("JSON export failed: {e}"))?;
    println!("Wrote {}", json_path.display());

    // 7. Generate GRAPH_REPORT.md
    if let Some(ref analysis) = result.analysis {
        let token_cost = report::TokenCost {
            input: result.stats.input_tokens as usize,
            output: result.stats.output_tokens as usize,
        };
        let root_str = root.to_string_lossy();
        let report_content =
            report::generate(&graph, analysis, &pipeline_detection, &token_cost, &root_str);
        let report_path = out_dir.join("GRAPH_REPORT.md");
        std::fs::write(&report_path, &report_content)?;
        println!("Wrote {}", report_path.display());
    }

    // 8. Save manifest for incremental detection on next run
    let manifest = clawft_graphify::extract::detect::Manifest::from_detection(&detection);
    let manifest_path = out_dir.join("manifest.json");
    manifest
        .save(&manifest_path)
        .map_err(|e| anyhow::anyhow!("Failed to save manifest: {e}"))?;

    // 9. Print summary
    println!("\nGraph summary:");
    println!("  Nodes: {}", graph.node_count());
    println!("  Edges: {}", graph.edge_count());
    if let Some(ref analysis) = result.analysis {
        println!("  Communities: {}", analysis.communities.len());
        if !analysis.god_nodes.is_empty() {
            println!(
                "  Top god node: {} ({} edges)",
                analysis.god_nodes[0].label, analysis.god_nodes[0].edges
            );
        }
    }
    println!("  Files processed: {}", result.stats.files_processed);

    Ok(())
}

/// Incremental pipeline: detect changes, extract only changed files, merge.
fn run_graphify_pipeline_incremental(
    root: &std::path::Path,
    graph_path: &std::path::Path,
    manifest_path: &std::path::Path,
) -> anyhow::Result<()> {
    use clawft_graphify::build;
    use clawft_graphify::extract::detect;
    use clawft_graphify::export;
    use clawft_graphify::pipeline::{Pipeline, PipelineConfig};
    use clawft_graphify::report;

    let start = std::time::Instant::now();

    // 1. Load manifest and run incremental detection.
    let manifest = detect::Manifest::load(manifest_path);
    let incr = detect::detect_incremental(root, &manifest)
        .map_err(|e| anyhow::anyhow!("Incremental detection failed: {e}"))?;

    let new_count: usize = incr.new_files.values().map(|v| v.len()).sum();
    let deleted_count = incr.deleted_files.len();

    if new_count == 0 && deleted_count == 0 {
        println!("No changes detected -- graph is up to date.");
        return Ok(());
    }

    println!(
        "Incremental: {} changed/new file(s), {} deleted file(s)",
        new_count, deleted_count
    );

    // 2. Load existing graph from JSON.
    let data = std::fs::read_to_string(graph_path)?;
    let json_value: serde_json::Value = serde_json::from_str(&data)?;
    let json_for_build = if json_value.get("edges").is_none() && json_value.get("links").is_some() {
        let mut obj = json_value.clone();
        if let Some(links) = obj.get("links").cloned() {
            obj.as_object_mut().unwrap().insert("edges".to_string(), links);
        }
        obj
    } else {
        json_value
    };
    let existing_graph = build::build_from_json(&json_for_build)
        .map_err(|e| anyhow::anyhow!("Failed to load existing graph: {e}"))?;

    // 3. Build extractions only for new/changed files.
    let incr_detection = detect::DetectionResult {
        files: incr.new_files.clone(),
        total_files: new_count,
        total_words: 0, // not critical for incremental
        needs_graph: true,
        warning: None,
        skipped_sensitive: vec![],
    };
    let extractions = build_extractions_from_detection(&incr_detection);

    println!(
        "Extracting {} file(s)...",
        extractions.len()
    );

    // 4. Run incremental pipeline: merge -> cluster -> analyze.
    let pipeline_detection = clawft_graphify::model::DetectionResult {
        total_files: incr.full.total_files,
        total_words: incr.full.total_words,
        warning: incr.full.warning.clone(),
    };
    let config = PipelineConfig::default();
    let pipeline = Pipeline::new(config);
    let result = pipeline
        .run_incremental(
            existing_graph,
            extractions,
            &incr.deleted_files,
            pipeline_detection.clone(),
        )
        .map_err(|e| anyhow::anyhow!("Incremental pipeline failed: {e}"))?;

    let elapsed = start.elapsed();

    // 5. Print merge stats.
    if let Some(ref ms) = result.merge_stats {
        println!(
            "Incremental: +{} entities, ~{} updated, -{} removed, \
             +{} rels, -{} rels ({:.1}s)",
            ms.entities_added,
            ms.entities_updated,
            ms.entities_removed,
            ms.relationships_added,
            ms.relationships_removed,
            elapsed.as_secs_f64(),
        );
    }

    // 6. Output directory & exports.
    let out_dir = root.join("graphify-out");
    std::fs::create_dir_all(&out_dir)?;

    let mut graph = result.graph;
    if let Some(ref analysis) = result.analysis {
        graph.communities = Some(analysis.communities.clone());
        graph.community_summaries = Some(analysis.community_summaries.clone());
    }

    export::export(&graph, export::ExportFormat::Json, graph_path)
        .map_err(|e| anyhow::anyhow!("JSON export failed: {e}"))?;
    println!("Wrote {}", graph_path.display());

    if let Some(ref analysis) = result.analysis {
        let token_cost = report::TokenCost {
            input: result.stats.input_tokens as usize,
            output: result.stats.output_tokens as usize,
        };
        let root_str = root.to_string_lossy();
        let report_content =
            report::generate(&graph, analysis, &pipeline_detection, &token_cost, &root_str);
        let report_path = out_dir.join("GRAPH_REPORT.md");
        std::fs::write(&report_path, &report_content)?;
        println!("Wrote {}", report_path.display());
    }

    // 7. Save updated manifest.
    let new_manifest = detect::Manifest::from_detection(&incr.full);
    new_manifest
        .save(manifest_path)
        .map_err(|e| anyhow::anyhow!("Failed to save manifest: {e}"))?;

    // 8. Print summary.
    println!("\nGraph summary:");
    println!("  Nodes: {}", graph.node_count());
    println!("  Edges: {}", graph.edge_count());
    if let Some(ref analysis) = result.analysis {
        println!("  Communities: {}", analysis.communities.len());
        if !analysis.god_nodes.is_empty() {
            println!(
                "  Top god node: {} ({} edges)",
                analysis.god_nodes[0].label, analysis.god_nodes[0].edges
            );
        }
    }
    println!("  Files processed (incremental): {}", result.stats.files_processed);

    Ok(())
}

/// Build `ExtractionResult`s from file detection without requiring tree-sitter.
///
/// Creates one entity per detected file, with relationships between files in
/// the same directory (co-location relationship). This gives a useful graph
/// structure even without AST parsing.
fn build_extractions_from_detection(
    detection: &clawft_graphify::extract::detect::DetectionResult,
) -> Vec<clawft_graphify::ExtractionResult> {
    use clawft_graphify::entity::{DomainTag, EntityId, EntityType, FileType};
    use clawft_graphify::model::{Entity, ExtractionResult};
    use clawft_graphify::relationship::{Confidence, RelationType, Relationship};

    let mut extractions: Vec<ExtractionResult> = Vec::new();

    // Map of directory -> list of entity IDs in that directory (for co-location edges)
    let mut dir_entities: HashMap<String, Vec<(EntityId, String)>> = HashMap::new();

    let type_map: &[(&str, FileType, EntityType)] = &[
        ("code", FileType::Code, EntityType::Module),
        ("document", FileType::Document, EntityType::Custom("document".into())),
        ("paper", FileType::Paper, EntityType::Custom("paper".into())),
        ("image", FileType::Image, EntityType::Custom("image".into())),
    ];

    for &(key, ref file_type, ref entity_type) in type_map {
        let files = match detection.files.get(key) {
            Some(f) => f,
            None => continue,
        };

        for file_path in files {
            let label = std::path::Path::new(file_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(file_path)
                .to_string();

            let domain = match file_type {
                FileType::Code => DomainTag::Code,
                _ => DomainTag::Forensic,
            };

            let id = EntityId::new(&domain, entity_type, &label, file_path);

            // Track directory for co-location relationships
            let dir = std::path::Path::new(file_path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            dir_entities
                .entry(dir)
                .or_default()
                .push((id.clone(), label.clone()));

            let entity = Entity {
                id,
                entity_type: entity_type.clone(),
                label,
                source_file: Some(file_path.clone()),
                source_location: None,
                file_type: *file_type,
                metadata: serde_json::json!({}),
                iri: None,
                legacy_id: Some(file_path.clone()),
            };

            extractions.push(ExtractionResult {
                source_file: file_path.clone(),
                entities: vec![entity],
                relationships: vec![],
                hyperedges: vec![],
                input_tokens: 0,
                output_tokens: 0,
                errors: vec![],
            });
        }
    }

    // Add co-location relationships: files in the same directory are related.
    // Only create edges within reasonably-sized directories to avoid noise.
    for entities in dir_entities.values() {
        if entities.len() < 2 || entities.len() > 50 {
            continue;
        }
        // Connect first entity to all others in the directory (star topology)
        let (hub_id, _) = &entities[0];
        for (other_id, _) in entities.iter().skip(1) {
            let rel = Relationship {
                source: hub_id.clone(),
                target: other_id.clone(),
                relation_type: RelationType::RelatedTo,
                confidence: Confidence::Inferred,
                weight: 0.5,
                source_file: None,
                source_location: None,
                metadata: serde_json::json!({"co_located": true}),
            };
            // Append to the first extraction for this directory's hub
            if let Some(ext) = extractions.iter_mut().find(|e| {
                e.entities.first().map(|ent| &ent.id) == Some(hub_id)
            }) {
                ext.relationships.push(rel);
            }
        }
    }

    extractions
}

async fn run_watch(root: &std::path::Path, debounce: f64) -> anyhow::Result<()> {
    use clawft_graphify::watch::{WatchConfig, WatchEvent};

    let config = WatchConfig {
        root: root.to_path_buf(),
        debounce_secs: debounce,
    };

    println!("Starting file watcher...");

    // Run the polling watcher (blocks).
    clawft_graphify::watch::watch_poll(&config, |event: WatchEvent| {
        println!("[graphify watch] {} file(s) changed", event.changed.len());
        if event.has_non_code {
            println!("[graphify watch] Non-code files changed -- run `weaver graphify rebuild` for full re-extraction.");
        } else {
            println!("[graphify watch] Code-only changes -- auto-rebuild would trigger here.");
        }
    }).map_err(|e| anyhow::anyhow!("Watch error: {e}"))?;

    Ok(())
}

async fn run_hooks(action: HooksAction) -> anyhow::Result<()> {
    match action {
        HooksAction::Install { path } => {
            let msg = clawft_graphify::hooks::install_hooks(&path)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("{msg}");
        }
        HooksAction::Uninstall { path } => {
            let msg = clawft_graphify::hooks::uninstall_hooks(&path)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("{msg}");
        }
        HooksAction::Status { path } => {
            let msg = clawft_graphify::hooks::hook_status(&path)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("{msg}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn graphify_args_parses() {
        GraphifyArgs::command().debug_assert();
    }
}
