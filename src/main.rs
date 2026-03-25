use anyhow::{Context, Result};
use cargo_upstream_triage::cargo_toml::CrateDepInfo;
use cargo_upstream_triage::cli::{AnalyzeArgs, Cli, Command, FlameArgs, OutputFormat, ReportArgs};
use cargo_upstream_triage::flamegraph;
use cargo_upstream_triage::report::AnalysisReport;
use cargo_upstream_triage::{graph, metrics, platform, registry, report, scanner};
use clap::Parser;
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Analyze(args) => run_analyze(args),
        Command::Report(args) => run_report(args),
        Command::Flame(args) => run_flame(args),
    }
}

fn run_analyze(args: AnalyzeArgs) -> Result<()> {
    // Phase 1: Load metadata.
    eprintln!("Loading workspace metadata...");
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(&args.manifest_path)
        .exec()
        .context("failed to run cargo metadata")?;

    let workspace_root = metadata.workspace_root.to_string();

    // Phase 2: Build dependency graph and find fat nodes.
    eprintln!("Building dependency graph...");
    let dep_graph = graph::DepGraph::from_metadata(&metadata)?;
    let total_deps = dep_graph.total_dependency_count();
    let fat_nodes = dep_graph.fat_nodes(args.fat_threshold);
    eprintln!(
        "Found {} fat nodes (W_transitive > {}) out of {} total dependencies",
        fat_nodes.len(),
        args.fat_threshold,
        total_deps
    );

    if fat_nodes.is_empty() {
        eprintln!("No fat nodes found. Your dependency tree is lean!");
        return Ok(());
    }

    // Phase 2b: Find intermediate edges.
    let edges = dep_graph.intermediate_edges(&fat_nodes);
    eprintln!("Found {} intermediate edges to scan", edges.len());

    if edges.is_empty() {
        eprintln!("No intermediate dependency edges to analyze.");
        return Ok(());
    }

    // Phase 2c: Resolve real platform deps to detect phantom deps.
    eprintln!("Resolving platform-specific dependency tree...");
    let real_deps = platform::resolve_real_deps(&args.manifest_path);
    if real_deps.is_none() {
        eprintln!("  [WARN] Could not resolve platform deps, phantom detection disabled");
    }

    // Phase 2d: Parse Cargo.toml for each intermediate crate (for renames + optional info).
    eprintln!("Parsing Cargo.toml files for dependency metadata...");
    let cargo_toml_cache: Mutex<HashMap<String, CrateDepInfo>> = Mutex::new(HashMap::new());

    // Phase 3 & 4: Source retrieval + heuristic scanning (parallelized).
    eprintln!("Scanning intermediate crate sources...");
    let targets: Vec<metrics::UpstreamTarget> = edges
        .par_iter()
        .filter_map(|edge| {
            // Phase 3: Locate source.
            let src_dir =
                registry::find_crate_source(&edge.intermediate_name, &edge.intermediate_version);

            let src_dir: PathBuf = match src_dir {
                Some(d) => d,
                None => {
                    // Workspace members — scan from metadata source path.
                    if dep_graph.workspace_members.contains(&edge.intermediate_id) {
                        if let Some(pkg) = metadata
                            .packages
                            .iter()
                            .find(|p| p.id == edge.intermediate_id)
                        {
                            pkg.manifest_path.parent().map(|p| p.into())?
                        } else {
                            return None;
                        }
                    } else {
                        eprintln!(
                            "  [WARN] Source not found for {} v{}, skipping",
                            edge.intermediate_name, edge.intermediate_version
                        );
                        return None;
                    }
                }
            };

            // Phase 3b: Parse Cargo.toml for rename/optional info.
            let cache_key = format!("{}-{}", edge.intermediate_name, edge.intermediate_version);
            let dep_info = {
                let mut cache = cargo_toml_cache.lock().unwrap();
                cache
                    .entry(cache_key)
                    .or_insert_with(|| {
                        let manifest = src_dir.join("Cargo.toml");
                        CrateDepInfo::from_manifest(&manifest)
                    })
                    .clone()
            };

            // Determine the local alias for the fat dependency.
            let alias = dep_info.local_alias(&edge.fat_name);
            let was_renamed = alias
                .as_ref()
                .is_some_and(|a| *a != edge.fat_name.replace('-', "_"));

            let mut aliases: Vec<String> = Vec::new();
            if let Some(ref a) = alias {
                aliases.push(a.clone());
            }

            // Build enriched edge metadata.
            let mut edge_meta = edge.edge_meta.clone();
            if dep_info.is_optional(&edge.fat_name) {
                edge_meta.already_optional = true;
            }
            if dep_info.is_platform_conditional(&edge.fat_name) {
                edge_meta.platform_conditional = true;
            }
            if dep_info.is_build_dep(&edge.fat_name) {
                edge_meta.build_only = true;
            }

            // Phase 4: Scan.
            let rs_files = registry::collect_rs_files(&src_dir);
            if rs_files.is_empty() {
                return None;
            }

            let scan = scanner::scan_files_with_aliases(&rs_files, &edge.fat_name, &aliases);

            // Phase 4b: Measure fat dep LOC and its own dep count.
            let fat_dep_loc = registry::find_crate_source(&edge.fat_name, &edge.fat_version)
                .map(|fat_dir| {
                    let fat_rs = registry::collect_rs_files(&fat_dir);
                    registry::count_loc(&fat_rs)
                })
                .unwrap_or(0);
            let fat_dep_own_deps = dep_graph.direct_dep_count(&edge.fat_id);
            let has_re_export_all = scan.has_re_export_all;

            // Phase 4c: Compute unique subtree weight.
            let w_unique = dep_graph.unique_subtree_weight(&edge.intermediate_id, &edge.fat_id);

            // Phase 4d: Compute dependency chain.
            let dep_chain = dep_graph.dependency_chain(&edge.fat_id);

            // Phase 4e: Check if a sibling dep transitively requires the fat dep.
            let required_by_sibling =
                dep_graph.sibling_requires(&edge.intermediate_id, &edge.fat_id);

            // Phase 4e: Check if the fat dep is a phantom (not on this platform).
            let phantom = !platform::is_real_dep(&real_deps, &edge.fat_name, &edge.fat_version);

            // Phase 4f: Check if intermediate is a workspace member.
            let intermediate_is_ws = dep_graph.workspace_members.contains(&edge.intermediate_id);

            // Phase 5: Compute metrics.
            Some(metrics::compute_target(
                &edge.intermediate_name,
                &edge.intermediate_version,
                &edge.fat_name,
                &edge.fat_version,
                edge.fat_transitive_weight,
                w_unique,
                scan,
                edge_meta,
                dep_chain,
                was_renamed,
                required_by_sibling,
                phantom,
                intermediate_is_ws,
                fat_dep_loc,
                fat_dep_own_deps,
                has_re_export_all,
            ))
        })
        .collect();

    // Phase 5b: Rank.
    let ranked = metrics::rank_targets(targets, args.threshold, args.top);

    // Phase 6: Report.
    let platform_deps = real_deps.as_ref().map(|s| s.len());
    let phantom_deps = platform_deps
        .map(|p| total_deps.saturating_sub(p))
        .unwrap_or(0);

    // Build serializable dep tree for flamegraph rendering.
    eprintln!("Building dependency tree for visualization...");
    let dep_tree = flamegraph::build_dep_tree(&dep_graph);

    let analysis = AnalysisReport {
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        workspace_root,
        threshold: args.threshold,
        total_dependencies: total_deps,
        platform_dependencies: platform_deps,
        phantom_dependencies: phantom_deps,
        fat_nodes_found: fat_nodes.len(),
        targets: ranked,
        dep_tree: Some(dep_tree),
    };

    let mut writer: Box<dyn Write> = match &args.output {
        Some(path) => {
            let file = std::fs::File::create(path)
                .with_context(|| format!("failed to create output file: {}", path.display()))?;
            Box::new(std::io::BufWriter::new(file))
        }
        None => Box::new(std::io::stdout().lock()),
    };

    match args.format {
        OutputFormat::Json => report::render_json(&analysis, &mut writer)?,
        OutputFormat::Text => report::render_text(&analysis, &mut writer, args.verbose)?,
        OutputFormat::Svg => {
            let tree = analysis
                .dep_tree
                .as_ref()
                .expect("dep_tree should always be present during analyze");
            flamegraph::render_flamegraph(tree, analysis.total_dependencies, &mut writer)?;
        }
    }

    // If writing to file, also save JSON alongside for the report subcommand.
    if let Some(path) = &args.output {
        let json_path = path.with_extension("json");
        if json_path != *path {
            let file = std::fs::File::create(&json_path)?;
            let mut json_writer = std::io::BufWriter::new(file);
            report::render_json(&analysis, &mut json_writer)?;
            eprintln!("JSON report saved to: {}", json_path.display());
        }
    }

    Ok(())
}

fn run_flame(args: FlameArgs) -> Result<()> {
    let analyze_args = AnalyzeArgs {
        manifest_path: args.manifest_path,
        threshold: args.threshold,
        top: args.top,
        fat_threshold: args.fat_threshold,
        format: OutputFormat::Svg,
        output: None,
        verbose: args.verbose,
    };

    // Create a named temp file for the SVG output.
    // Use keep() so the file persists for the browser to read.
    let tmp_file = tempfile::Builder::new()
        .prefix("upstream-triage-")
        .suffix(".svg")
        .tempfile()
        .context("failed to create temp file")?;
    let svg_path = tmp_file
        .into_temp_path()
        .keep()
        .context("failed to persist temp file")?;

    // Run the analyze pipeline, writing SVG to the temp file.
    let analyze_args = AnalyzeArgs {
        output: Some(svg_path.clone()),
        ..analyze_args
    };
    run_analyze(analyze_args)?;

    // Open the SVG in the user's default browser.
    let uri = format!("file://{}", svg_path.display());
    eprintln!("Opening flamegraph: {}", uri);
    open::that(&uri).with_context(|| format!("failed to open browser for {}", uri))?;

    Ok(())
}

fn run_report(args: ReportArgs) -> Result<()> {
    let content = std::fs::read_to_string(&args.input)
        .with_context(|| format!("failed to read {}", args.input.display()))?;
    let analysis: AnalysisReport =
        serde_json::from_str(&content).context("failed to parse JSON report")?;

    let mut writer: Box<dyn Write> = match &args.output {
        Some(path) => {
            let file = std::fs::File::create(path)
                .with_context(|| format!("failed to create output file: {}", path.display()))?;
            Box::new(std::io::BufWriter::new(file))
        }
        None => Box::new(std::io::stdout().lock()),
    };

    match args.format {
        OutputFormat::Json => report::render_json(&analysis, &mut writer)?,
        OutputFormat::Text => report::render_text(&analysis, &mut writer, args.verbose)?,
        OutputFormat::Svg => {
            let tree = analysis.dep_tree.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "This JSON report has no dep_tree data. \
                     Re-run `analyze` to generate a report with tree data for SVG rendering."
                )
            })?;
            flamegraph::render_flamegraph(tree, analysis.total_dependencies, &mut writer)?;
        }
    }

    Ok(())
}
