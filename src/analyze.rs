use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use tracing::{debug, info, info_span, warn};

use crate::cli::AnalyzeArgs;
use crate::flamegraph;
use crate::graph::{DepGraph, EdgeMeta, HeavyNode, IntermediateEdge};
use crate::metrics::{
    self, ComputeTargetInput, Confidence, PackageInfo, RemovalStrategy, UpstreamTarget,
};
use crate::platform;
use crate::registry::FsCache;
use crate::report::{AnalysisReport, DirectDepSummary, UnusedDirectDep};
use crate::scanner::{self, RegexCache};

/// Metadata loaded in Phase 1: default features and all-features variants.
struct LoadedMetadata {
    metadata: cargo_metadata::Metadata,
    metadata_all_features: cargo_metadata::Metadata,
    workspace_root: String,
}

/// Result of Phase 2: dependency graph construction and heavy-node detection.
struct GraphAnalysis {
    dep_graph: DepGraph,
    total_deps: usize,
    /// `None` when no heavy nodes were found (lean dependency tree).
    heavy_edges: Option<HeavyEdges>,
}

/// The heavy nodes and intermediate edges found during graph analysis.
struct HeavyEdges {
    heavy_nodes: Vec<HeavyNode>,
    edges: Vec<IntermediateEdge>,
}

/// Pre-computed data and shared caches used during scanning phases.
struct ScanContext {
    real_deps: Option<HashSet<String>>,
    dep_chains: HashMap<cargo_metadata::PackageId, Vec<String>>,
    fs_cache: FsCache,
    regex_cache: RegexCache,
}

/// Result of scanning edges and ranking targets (Phases 3-5c).
struct ScanPhaseResult {
    ranked: Vec<UpstreamTarget>,
}

/// Result of detecting unused direct dependencies (Phase 5d).
struct UnusedDepsResult {
    ranked: Vec<UpstreamTarget>,
    unused_direct_deps_summary: Vec<UnusedDirectDep>,
}

/// Run the full analysis pipeline and return the report.
pub fn run_analyze(args: &AnalyzeArgs) -> Result<AnalysisReport> {
    let _temp_dir: Option<std::path::PathBuf>;
    let manifest_path = if let Some(ref spec_str) = args.common.crate_spec {
        #[cfg(not(feature = "remote"))]
        {
            let _ = spec_str;
            anyhow::bail!(
                "The --crate flag requires the `remote` feature. \
                 Rebuild with: cargo install --features remote"
            );
        }
        #[cfg(feature = "remote")]
        {
            let (path, tmp) = crate::crate_fetch::resolve_remote_manifest(spec_str)?;
            _temp_dir = Some(tmp);
            path
        }
    } else {
        _temp_dir = None;
        args.common.manifest_path.clone()
    };

    // Phase 1: Load metadata.
    let loaded = load_metadata(&manifest_path)?;

    // Phase 2: Build dependency graph and find heavy nodes + intermediate edges.
    let GraphAnalysis {
        dep_graph,
        total_deps,
        heavy_edges,
    } = build_graph_and_find_edges(&loaded.metadata, args)?;

    let HeavyEdges { heavy_nodes, edges } = match heavy_edges {
        Some(he) => he,
        None => {
            return Ok(empty_report(
                loaded.workspace_root,
                args.common.threshold,
                total_deps,
            ));
        }
    };

    if edges.is_empty() {
        eprintln!("No intermediate dependency edges to analyze.");
        return Ok(empty_report(
            loaded.workspace_root,
            args.common.threshold,
            total_deps,
        ));
    }

    // Phase 2c + pre-compute: resolve platform deps, batch dep chains, create caches.
    let scan_ctx = build_scan_context(&manifest_path, &dep_graph);

    // Phases 3-5c: Scan edges, rank targets, enrich features.
    let scan_result = scan_and_rank(&edges, &dep_graph, &loaded.metadata, &scan_ctx, args);

    // Phase 5d: Detect unused direct deps and merge into ranked list.
    let unused_result =
        detect_unused_deps(scan_result.ranked, &dep_graph, &loaded.metadata, &scan_ctx);

    // Phase 6: Build the report.
    build_report(
        loaded,
        &dep_graph,
        total_deps,
        heavy_nodes.len(),
        unused_result,
        &scan_ctx.real_deps,
        args,
    )
}

/// Phase 1: Load workspace metadata (default features and all-features).
fn load_metadata(manifest_path: &PathBuf) -> Result<LoadedMetadata> {
    eprintln!("Loading workspace metadata...");
    let metadata = {
        let _span = info_span!("load_metadata").entered();
        cargo_metadata::MetadataCommand::new()
            .manifest_path(manifest_path)
            .exec()
            .context("failed to run cargo metadata")?
    };

    eprintln!("Loading full feature graph for visualization...");
    let metadata_all_features = {
        let _span = info_span!("load_metadata_all_features").entered();
        match cargo_metadata::MetadataCommand::new()
            .manifest_path(manifest_path)
            .other_options(vec!["--all-features".to_string()])
            .exec()
        {
            Ok(m) => m,
            Err(e) => {
                warn!("--all-features metadata failed, falling back to default features: {e}");
                eprintln!(
                    "  [WARN] --all-features failed (conflicting features?), \
                     falling back to default feature set for visualization"
                );
                metadata.clone()
            }
        }
    };

    let workspace_root = metadata.workspace_root.to_string();

    Ok(LoadedMetadata {
        metadata,
        metadata_all_features,
        workspace_root,
    })
}

/// Phase 2: Build the dependency graph, find heavy nodes, and collect intermediate edges.
///
/// When no heavy nodes are found, `heavy_edges` will be `None`.
fn build_graph_and_find_edges(
    metadata: &cargo_metadata::Metadata,
    args: &AnalyzeArgs,
) -> Result<GraphAnalysis> {
    eprintln!("Building dependency graph...");
    let dep_graph = {
        let _span = info_span!("build_dep_graph").entered();
        crate::graph::DepGraph::from_metadata(metadata)?
    };
    let total_deps = dep_graph.total_dependency_count();
    let heavy_nodes = dep_graph.heavy_nodes(args.common.heavy_threshold);
    info!(
        heavy_nodes = heavy_nodes.len(),
        threshold = args.common.heavy_threshold,
        total_deps,
        "found heavy nodes"
    );
    eprintln!(
        "Found {} heavy nodes (W_transitive > {}) out of {} total dependencies",
        heavy_nodes.len(),
        args.common.heavy_threshold,
        total_deps
    );

    if heavy_nodes.is_empty() {
        eprintln!("No heavy nodes found. Your dependency tree is lean!");
        return Ok(GraphAnalysis {
            dep_graph,
            total_deps,
            heavy_edges: None,
        });
    }

    let edges = dep_graph.intermediate_edges(&heavy_nodes);
    eprintln!("Found {} intermediate edges to scan", edges.len());

    Ok(GraphAnalysis {
        dep_graph,
        total_deps,
        heavy_edges: Some(HeavyEdges { heavy_nodes, edges }),
    })
}

/// Phase 2c + pre-compute: resolve platform deps, batch dependency chains, create caches.
fn build_scan_context(manifest_path: &std::path::Path, dep_graph: &DepGraph) -> ScanContext {
    // Phase 2c: Resolve real platform deps.
    eprintln!("Resolving platform-specific dependency tree...");
    let real_deps = {
        let _span = info_span!("resolve_platform_deps").entered();
        platform::resolve_real_deps(manifest_path)
    };
    if real_deps.is_none() {
        warn!("could not resolve platform deps, phantom detection disabled");
        eprintln!("  [WARN] Could not resolve platform deps, phantom detection disabled");
    }

    // Pre-compute batch data: dependency chains (single BFS for all nodes).
    let dep_chains = {
        let _span = info_span!("batch_dependency_chains").entered();
        dep_graph.all_dependency_chains()
    };
    info!(chains = dep_chains.len(), "precomputed dependency chains");

    // Create shared caches for parallel scanning.
    let fs_cache = FsCache::new();
    let regex_cache = RegexCache::new();

    ScanContext {
        real_deps,
        dep_chains,
        fs_cache,
        regex_cache,
    }
}

/// Phases 3-5c: Parallel edge scanning, ranking, and feature enrichment.
fn scan_and_rank(
    edges: &[IntermediateEdge],
    dep_graph: &DepGraph,
    metadata: &cargo_metadata::Metadata,
    scan_ctx: &ScanContext,
    args: &AnalyzeArgs,
) -> ScanPhaseResult {
    // Phase 3 & 4: Source retrieval + heuristic scanning (parallelized by edge).
    eprintln!("Scanning {} intermediate crate sources...", edges.len());
    let targets: Vec<UpstreamTarget> = {
        let _span = info_span!("scan_edges", count = edges.len()).entered();
        let scanned = std::sync::atomic::AtomicUsize::new(0);
        let total_edges = edges.len();
        let result: Vec<UpstreamTarget> = edges
            .par_iter()
            .filter_map(|edge| {
                let n = scanned.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if n.is_multiple_of(50) || n == total_edges {
                    debug!(progress = n, total = total_edges, "scanning edges");
                }
                scan_edge(
                    edge,
                    dep_graph,
                    metadata,
                    &scan_ctx.real_deps,
                    &scan_ctx.dep_chains,
                    &scan_ctx.fs_cache,
                    &scan_ctx.regex_cache,
                )
            })
            .collect();
        info!(targets = result.len(), "edge scanning complete");
        result
    };

    // Phase 5b: Rank.
    let mut ranked = metrics::rank_targets(
        targets,
        args.common.threshold,
        args.common.top,
        args.common.include_noise,
    );

    // Phase 5c: Enrich AlreadyGated suggestions with feature info.
    enrich_features(&mut ranked, metadata);

    ScanPhaseResult { ranked }
}

/// Phase 5d: Detect unused direct dependencies and merge them into the ranked list.
fn detect_unused_deps(
    mut ranked: Vec<UpstreamTarget>,
    dep_graph: &DepGraph,
    metadata: &cargo_metadata::Metadata,
    scan_ctx: &ScanContext,
) -> UnusedDepsResult {
    eprintln!(
        "Checking for unused direct dependencies ({} workspace members)...",
        dep_graph.workspace_members.len()
    );
    let already_analyzed: HashSet<(String, String)> = ranked
        .iter()
        .map(|t| (t.intermediate.name.clone(), t.heavy_dependency.name.clone()))
        .collect();
    let unused_deps = find_unused_deps(
        dep_graph,
        metadata,
        &scan_ctx.real_deps,
        &already_analyzed,
        &scan_ctx.dep_chains,
        &scan_ctx.fs_cache,
        &scan_ctx.regex_cache,
    );
    let unused_direct_deps_summary: Vec<UnusedDirectDep> = unused_deps
        .iter()
        .map(|t| UnusedDirectDep {
            from_crate: t.intermediate.name.clone(),
            dep_name: t.heavy_dependency.name.clone(),
            dep_version: t.heavy_dependency.version.clone(),
            real_deps_saved: t.w_unique,
            is_test_example: is_test_or_example_crate_name(&t.intermediate.name),
        })
        .collect();
    info!(unused = unused_deps.len(), "unused dep scan complete");
    if !unused_deps.is_empty() {
        eprintln!("  Found {} unused direct dependencies", unused_deps.len());
        ranked = merge_unused(ranked, unused_deps);
    }

    UnusedDepsResult {
        ranked,
        unused_direct_deps_summary,
    }
}

/// Phase 6: Assemble the final analysis report.
fn build_report(
    loaded: LoadedMetadata,
    dep_graph: &DepGraph,
    total_deps: usize,
    heavy_nodes_found: usize,
    unused_result: UnusedDepsResult,
    real_deps: &Option<HashSet<String>>,
    args: &AnalyzeArgs,
) -> Result<AnalysisReport> {
    let platform_deps = real_deps.as_ref().map(|s| s.len());
    let phantom_deps = platform_deps
        .map(|p| total_deps.saturating_sub(p))
        .unwrap_or(0);

    let direct_dep_summary = build_direct_dep_summary(dep_graph, real_deps);

    eprintln!("Building dependency tree for visualization...");
    let dep_tree = {
        let _span = info_span!("build_dep_tree").entered();
        flamegraph::build_dep_tree(&loaded.metadata_all_features, &loaded.metadata)
    };

    let unused_edges: Vec<(String, String)> = unused_result
        .ranked
        .iter()
        .filter(|t| t.c_ref == 0)
        .map(|t| (t.intermediate.name.clone(), t.heavy_dependency.name.clone()))
        .collect();

    Ok(AnalysisReport {
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        timestamp: now_timestamp(),
        workspace_root: loaded.workspace_root,
        threshold: args.common.threshold,
        total_dependencies: total_deps,
        platform_dependencies: platform_deps,
        phantom_dependencies: phantom_deps,
        heavy_nodes_found,
        targets: unused_result.ranked,
        dep_tree: Some(dep_tree),
        unused_edges,
        unused_direct_deps: unused_result.unused_direct_deps_summary,
        direct_dep_summary,
    })
}

fn is_test_or_example_crate_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    let patterns = [
        "test",
        "example",
        "bench",
        "doc-example",
        "stress",
        "tester",
        "poc",
        "guide",
        "wasm-example",
    ];
    patterns.iter().any(|p| lower.contains(p))
}

fn lib_target_name(metadata: &cargo_metadata::Metadata, pkg_name: &str) -> Option<String> {
    let pkg = metadata.packages.iter().find(|p| p.name == pkg_name)?;
    let lib_target = pkg.targets.iter().find(|t| {
        t.kind
            .iter()
            .any(|k| k == "lib" || k == "rlib" || k == "proc-macro")
    })?;
    let normalized_pkg = pkg_name.replace('-', "_");
    if lib_target.name == normalized_pkg {
        None
    } else {
        Some(lib_target.name.clone())
    }
}

/// Scan a single intermediate edge using shared caches.
fn scan_edge(
    edge: &IntermediateEdge,
    dep_graph: &DepGraph,
    metadata: &cargo_metadata::Metadata,
    real_deps: &Option<HashSet<String>>,
    dep_chains: &HashMap<cargo_metadata::PackageId, Vec<String>>,
    fs_cache: &FsCache,
    regex_cache: &RegexCache,
) -> Option<UpstreamTarget> {
    // Locate source.
    let src_dir = fs_cache.find_crate_source(&edge.intermediate_name, &edge.intermediate_version);

    let intermediate_pkg = metadata
        .packages
        .iter()
        .find(|p| p.id == edge.intermediate_id);

    let src_dir: PathBuf = match src_dir {
        Some(d) => d,
        None => {
            if dep_graph.workspace_members.contains(&edge.intermediate_id) {
                intermediate_pkg?.manifest_path.parent().map(|p| p.into())?
            } else {
                warn!(
                    name = %edge.intermediate_name,
                    version = %edge.intermediate_version,
                    "source not found, skipping"
                );
                return None;
            }
        }
    };

    // Look up dependency info.
    let dep_meta = intermediate_pkg
        .and_then(|pkg| pkg.dependencies.iter().find(|d| d.name == edge.heavy_name));

    let alias = dep_meta
        .and_then(|d| d.rename.clone())
        .or_else(|| Some(edge.heavy_name.replace('-', "_")));
    let was_renamed = dep_meta
        .and_then(|d| d.rename.as_ref())
        .is_some_and(|r| *r != edge.heavy_name.replace('-', "_"));

    let mut aliases: Vec<String> = Vec::new();
    if let Some(ref a) = alias {
        aliases.push(a.clone());
    }
    if let Some(lib_name) = lib_target_name(metadata, &edge.heavy_name) {
        let lib_norm = lib_name.replace('-', "_");
        if !aliases.contains(&lib_norm) {
            aliases.push(lib_norm);
        }
    }

    // Build enriched edge metadata.
    let mut edge_meta = edge.edge_meta.clone();
    if let Some(d) = dep_meta {
        if d.optional {
            edge_meta.already_optional = true;
        }
        if d.target.is_some() {
            edge_meta.platform_conditional = true;
        }
        if d.kind == cargo_metadata::DependencyKind::Build {
            edge_meta.build_only = true;
        }
    }

    // Scan source files (cached).
    let rs_files = fs_cache.collect_rs_files(&src_dir);
    if rs_files.is_empty() {
        debug!(intermediate = %edge.intermediate_name, "no .rs files found, skipping");
        return None;
    }

    debug!(
        intermediate = %edge.intermediate_name,
        heavy = %edge.heavy_name,
        rs_files = rs_files.len(),
        "scanning edge"
    );
    let scan = scanner::scan_files_with_aliases(
        &rs_files,
        &edge.heavy_name,
        &aliases,
        fs_cache,
        regex_cache,
    );

    // Measure heavy dep LOC (cached).
    let heavy_dep_loc = fs_cache
        .find_crate_source(&edge.heavy_name, &edge.heavy_version)
        .map(|heavy_dir| {
            let heavy_rs = fs_cache.collect_rs_files(&heavy_dir);
            fs_cache.count_loc(&heavy_dir, &heavy_rs)
        })
        .unwrap_or(0);
    let heavy_dep_own_deps = dep_graph.direct_dep_count(&edge.heavy_id);
    let has_re_export_all = scan.has_re_export_all;

    // Unique subtree weight (per-edge BFS — can't easily batch).
    let w_unique = dep_graph.unique_subtree_weight(&edge.intermediate_id, &edge.heavy_id);

    // Dependency chain (pre-computed via batch BFS).
    let dep_chain = dep_chains.get(&edge.heavy_id).cloned().unwrap_or_default();

    let required_by_sibling = dep_graph.sibling_requires(&edge.intermediate_id, &edge.heavy_id);
    let phantom = !platform::is_real_dep(real_deps, &edge.heavy_name, &edge.heavy_version);
    let intermediate_is_ws = dep_graph.workspace_members.contains(&edge.intermediate_id);
    let is_standalone_integration =
        intermediate_is_ws && dep_graph.is_standalone_workspace_member(&edge.intermediate_id);

    Some(metrics::compute_target(ComputeTargetInput {
        intermediate_name: edge.intermediate_name.clone(),
        intermediate_version: edge.intermediate_version.clone(),
        heavy_name: edge.heavy_name.clone(),
        heavy_version: edge.heavy_version.clone(),
        w_transitive: edge.heavy_transitive_weight,
        w_unique,
        scan_result: scan,
        edge_meta,
        dep_chain,
        was_renamed,
        required_by_sibling,
        phantom,
        intermediate_is_workspace_member: intermediate_is_ws,
        is_standalone_integration,
        heavy_dep_loc,
        heavy_dep_own_deps,
        has_re_export_all,
        is_proc_macro: is_proc_macro_crate(metadata, &edge.heavy_name),
    }))
}

fn enrich_features(ranked: &mut [UpstreamTarget], metadata: &cargo_metadata::Metadata) {
    let pkg_map: HashMap<&str, &cargo_metadata::Package> = metadata
        .packages
        .iter()
        .map(|p| (p.name.as_str(), p))
        .collect();

    for target in ranked.iter_mut() {
        if let RemovalStrategy::AlreadyGated {
            enabling_features,
            recommended_defaults,
            ..
        } = &mut target.suggestion
        {
            if let Some(pkg) = pkg_map.get(target.intermediate.name.as_str()) {
                let heavy_name = &target.heavy_dependency.name;
                let mut found = Vec::new();
                for (feat_name, feat_deps) in &pkg.features {
                    if feat_name == "default" {
                        continue;
                    }
                    for dep_entry in feat_deps {
                        let enables = dep_entry == heavy_name
                            || dep_entry == &format!("dep:{heavy_name}")
                            || dep_entry.starts_with(&format!("{heavy_name}/"));
                        if enables {
                            found.push(feat_name.clone());
                            break;
                        }
                    }
                }
                found.sort();

                if let Some(defaults) = pkg.features.get("default") {
                    let dominated: HashSet<&str> = found.iter().map(|s| s.as_str()).collect();
                    let dep_prefix = format!("dep:{heavy_name}");
                    let enables_fat =
                        |d: &str| dominated.contains(d) || d == heavy_name || d == dep_prefix;
                    let any_in_default = defaults.iter().any(|d| enables_fat(d));
                    if any_in_default {
                        let keep: Vec<String> = defaults
                            .iter()
                            .filter(|d| !enables_fat(d))
                            .cloned()
                            .collect();
                        *recommended_defaults = Some(keep);
                    }
                }

                *enabling_features = found;
            }
        }
    }
}

/// A candidate edge to check for unused-ness.
struct UnusedCandidate<'a> {
    ws_id: &'a cargo_metadata::PackageId,
    ws_name: String,
    ws_version: String,
    ws_dir: PathBuf,
    dep_id: &'a cargo_metadata::PackageId,
    dep_name: String,
    dep_version: String,
    dep_transitive_weight: usize,
}

/// Detect completely unused direct dependencies (parallelized across all candidates).
fn find_unused_deps(
    dep_graph: &DepGraph,
    metadata: &cargo_metadata::Metadata,
    real_deps: &Option<HashSet<String>>,
    already_analyzed: &HashSet<(String, String)>,
    dep_chains: &HashMap<cargo_metadata::PackageId, Vec<String>>,
    fs_cache: &FsCache,
    regex_cache: &RegexCache,
) -> Vec<UpstreamTarget> {
    let _span = info_span!(
        "find_unused_deps",
        workspace_members = dep_graph.workspace_members.len()
    )
    .entered();

    // Flatten all (workspace_member, dep) pairs into a single candidate list.
    let candidates = collect_unused_candidates(dep_graph, metadata, already_analyzed, fs_cache);
    info!(
        total_candidates = candidates.len(),
        "scanning unused dep candidates in parallel"
    );

    // Scan all candidates in parallel.
    candidates
        .par_iter()
        .filter_map(|c| {
            check_unused_candidate(
                c,
                dep_graph,
                metadata,
                real_deps,
                dep_chains,
                fs_cache,
                regex_cache,
            )
        })
        .collect()
}

/// Extract the `[package.metadata.cargo-machete] ignored = [...]` list from a package.
/// Returns an empty set if the key is absent or malformed.
fn machete_ignored(pkg: &cargo_metadata::Package) -> HashSet<String> {
    pkg.metadata
        .get("cargo-machete")
        .and_then(|v| v.get("ignored"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Build the flat list of (workspace_member, dep) candidates to check.
fn collect_unused_candidates<'a>(
    dep_graph: &'a DepGraph,
    metadata: &'a cargo_metadata::Metadata,
    already_analyzed: &HashSet<(String, String)>,
    fs_cache: &FsCache,
) -> Vec<UnusedCandidate<'a>> {
    let mut candidates = Vec::new();

    for ws_id in &dep_graph.workspace_members {
        let ws_pkg = match metadata.packages.iter().find(|p| p.id == *ws_id) {
            Some(p) => p,
            None => continue,
        };
        let ws_dir: PathBuf = match ws_pkg.manifest_path.parent() {
            Some(p) => p.into(),
            None => continue,
        };
        let direct_deps = match dep_graph.forward.get(ws_id) {
            Some(deps) => deps,
            None => continue,
        };

        // Pre-warm the file cache.
        let ws_rs_files = fs_cache.collect_rs_files(&ws_dir);
        if ws_rs_files.is_empty() {
            continue;
        }

        let ignored = machete_ignored(ws_pkg);

        let mut dep_count = 0;
        for dep_id in direct_deps {
            let dep_node = match dep_graph.nodes.get(dep_id) {
                Some(n) => n,
                None => continue,
            };
            if dep_node.is_workspace_member {
                continue;
            }
            if ignored.contains(&dep_node.name)
                || ignored.contains(&dep_node.name.replace('-', "_"))
            {
                continue;
            }
            if already_analyzed.contains(&(ws_pkg.name.clone(), dep_node.name.clone())) {
                continue;
            }
            if let Some(meta) = dep_graph.edge_meta.get(&(ws_id.clone(), dep_id.clone())) {
                if meta.build_only {
                    continue;
                }
            }
            candidates.push(UnusedCandidate {
                ws_id,
                ws_name: ws_pkg.name.clone(),
                ws_version: ws_pkg.version.to_string(),
                ws_dir: ws_dir.clone(),
                dep_id,
                dep_name: dep_node.name.clone(),
                dep_version: dep_node.version.clone(),
                dep_transitive_weight: dep_node.transitive_weight,
            });
            dep_count += 1;
        }

        info!(
            workspace_member = %ws_pkg.name,
            direct_deps = dep_count,
            rs_files = ws_rs_files.len(),
            "workspace member queued for unused dep scan"
        );
    }

    candidates
}

/// Check a single candidate: scan source files, return UpstreamTarget if unused.
fn check_unused_candidate(
    c: &UnusedCandidate,
    dep_graph: &DepGraph,
    metadata: &cargo_metadata::Metadata,
    real_deps: &Option<HashSet<String>>,
    dep_chains: &HashMap<cargo_metadata::PackageId, Vec<String>>,
    fs_cache: &FsCache,
    regex_cache: &RegexCache,
) -> Option<UpstreamTarget> {
    let ws_rs_files = fs_cache.collect_rs_files(&c.ws_dir);

    let dep_aliases = build_dep_aliases(metadata, &c.ws_name, &c.dep_name);

    let scan = scanner::scan_files_with_aliases(
        &ws_rs_files,
        &c.dep_name,
        &dep_aliases,
        fs_cache,
        regex_cache,
    );
    if scan.ref_count > 0 || scan.has_re_export_all {
        return None;
    }

    let w_unique = dep_graph.unique_subtree_weight(c.ws_id, c.dep_id);
    let dep_chain = dep_chains
        .get(c.dep_id)
        .cloned()
        .unwrap_or_else(|| vec![c.ws_name.clone(), c.dep_name.clone()]);
    let edge_meta = dep_graph
        .edge_meta
        .get(&(c.ws_id.clone(), c.dep_id.clone()))
        .cloned()
        .unwrap_or(EdgeMeta {
            build_only: false,
            already_optional: false,
            platform_conditional: false,
        });

    // Check if the dep declaration has explicit features — it may exist purely
    // for feature unification across the workspace rather than direct code use.
    // But only if the dep is also reachable through other paths (w_unique == 0),
    // meaning removing it wouldn't eliminate it from the dep tree. If w_unique > 0,
    // nobody else pulls it in, so the features argument doesn't apply.
    let has_explicit_features = dep_has_explicit_features(metadata, &c.ws_name, &c.dep_name);
    let likely_feature_unification = has_explicit_features && w_unique == 0;
    let (confidence, suggestion) = if likely_feature_unification {
        (Confidence::Low, RemovalStrategy::Remove)
    } else {
        (Confidence::High, RemovalStrategy::Remove)
    };

    Some(UpstreamTarget {
        intermediate: PackageInfo {
            name: c.ws_name.clone(),
            version: c.ws_version.clone(),
        },
        heavy_dependency: PackageInfo {
            name: c.dep_name.clone(),
            version: c.dep_version.clone(),
        },
        w_transitive: c.dep_transitive_weight,
        w_unique,
        c_ref: 0,
        hurrs: None,
        confidence,
        scan_result: scan,
        suggestion,
        edge_meta,
        dep_chain,
        required_by_sibling: None,
        phantom: !platform::is_real_dep(real_deps, &c.dep_name, &c.dep_version),
        intermediate_is_workspace_member: true,
        is_standalone_integration: dep_graph.is_standalone_workspace_member(c.ws_id),
        heavy_dep_loc: 0,
        heavy_dep_own_deps: dep_graph.direct_dep_count(c.dep_id),
        has_re_export_all: false,
    })
}

/// Build alias list for a dependency (rename + lib target name).
fn build_dep_aliases(
    metadata: &cargo_metadata::Metadata,
    ws_name: &str,
    dep_name: &str,
) -> Vec<String> {
    let mut aliases = Vec::new();
    if let Some(ws_pkg) = metadata.packages.iter().find(|p| p.name == ws_name) {
        if let Some(dep_meta) = ws_pkg.dependencies.iter().find(|d| d.name == dep_name) {
            if let Some(ref rename) = dep_meta.rename {
                let rename_norm = rename.replace('-', "_");
                if !aliases.contains(&rename_norm) {
                    aliases.push(rename_norm);
                }
            }
        }
    }
    if let Some(lib_name) = lib_target_name(metadata, dep_name) {
        let lib_norm = lib_name.replace('-', "_");
        if !aliases.contains(&lib_norm) {
            aliases.push(lib_norm);
        }
    }
    aliases
}

/// Check whether a dependency declaration has explicit feature customization
/// (non-empty `features` list or `default-features = false`). Such deps may
/// exist purely for feature unification across a workspace, even if the crate
/// never references the dep's symbols directly.
/// Check whether a package is a proc-macro crate or a thin facade over one.
/// Proc macros are invoked via attributes/derives whose names often differ from
/// the crate name, so regex-based scanning fundamentally can't detect their usage.
///
/// Also detects facade crates like `derive_builder` that re-export from a
/// companion `derive_builder_macro` proc-macro crate.
fn is_proc_macro_crate(metadata: &cargo_metadata::Metadata, dep_name: &str) -> bool {
    let pkg = match metadata.packages.iter().find(|p| p.name == dep_name) {
        Some(p) => p,
        None => return false,
    };

    // Direct proc-macro target.
    let has_proc_macro_target = pkg
        .targets
        .iter()
        .any(|t| t.kind.iter().any(|k| k == "proc-macro"));
    if has_proc_macro_target {
        return true;
    }

    // Facade pattern: check if the crate has a direct dependency whose name
    // is `<crate>-macro`, `<crate>-macros`, `<crate>-derive`, or `<crate>-impl`
    // and that dep is a proc-macro crate.
    let suffixes = [
        "-macro", "-macros", "-derive", "-impl", "_macro", "_macros", "_derive",
    ];
    pkg.dependencies.iter().any(|d| {
        suffixes
            .iter()
            .any(|suffix| d.name == format!("{dep_name}{suffix}"))
            && metadata
                .packages
                .iter()
                .find(|p| p.name == d.name)
                .is_some_and(|dep_pkg| {
                    dep_pkg
                        .targets
                        .iter()
                        .any(|t| t.kind.iter().any(|k| k == "proc-macro"))
                })
    })
}

fn dep_has_explicit_features(
    metadata: &cargo_metadata::Metadata,
    ws_name: &str,
    dep_name: &str,
) -> bool {
    let ws_pkg = match metadata.packages.iter().find(|p| p.name == ws_name) {
        Some(p) => p,
        None => return false,
    };
    let dep_meta = match ws_pkg.dependencies.iter().find(|d| d.name == dep_name) {
        Some(d) => d,
        None => return false,
    };
    !dep_meta.features.is_empty() || !dep_meta.uses_default_features
}

fn merge_unused(
    mut ranked: Vec<UpstreamTarget>,
    unused_deps: Vec<UpstreamTarget>,
) -> Vec<UpstreamTarget> {
    let (mut impactful, mut cosmetic): (Vec<_>, Vec<_>) =
        unused_deps.into_iter().partition(|t| t.w_unique > 0);
    impactful.sort_by(|a, b| b.w_unique.cmp(&a.w_unique));
    cosmetic.sort_by(|a, b| b.w_transitive.cmp(&a.w_transitive));
    for t in &mut cosmetic {
        // Deps with explicit features that save no unique deps are likely
        // kept for feature unification — demote to Noise.
        if t.confidence == Confidence::Low {
            t.confidence = Confidence::Noise;
        } else {
            t.confidence = Confidence::Low;
        }
    }
    impactful.append(&mut ranked);
    impactful.append(&mut cosmetic);
    impactful
}

/// Compute unique ancestor count for a package: how many distinct packages
/// transitively depend on it (via reverse edges in the dep graph).
fn unique_ancestor_count(
    dep_graph: &DepGraph,
    pkg_id: &cargo_metadata::PackageId,
) -> usize {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    if let Some(parents) = dep_graph.reverse.get(pkg_id) {
        for p in parents {
            if visited.insert(p.clone()) {
                queue.push_back(p.clone());
            }
        }
    }
    while let Some(cur) = queue.pop_front() {
        if let Some(parents) = dep_graph.reverse.get(&cur) {
            for p in parents {
                if visited.insert(p.clone()) {
                    queue.push_back(p.clone());
                }
            }
        }
    }
    visited.len()
}

fn build_direct_dep_summary(
    dep_graph: &DepGraph,
    real_deps: &Option<HashSet<String>>,
) -> Vec<DirectDepSummary> {
    let mut entries = Vec::new();

    for ws_id in &dep_graph.workspace_members {
        let ws_node = match dep_graph.nodes.get(ws_id) {
            Some(n) => n,
            None => continue,
        };

        let direct_deps = match dep_graph.forward.get(ws_id) {
            Some(deps) => deps,
            None => continue,
        };

        for dep_id in direct_deps {
            let dep_node = match dep_graph.nodes.get(dep_id) {
                Some(n) => n,
                None => continue,
            };
            if dep_node.is_workspace_member {
                continue;
            }
            if !platform::is_real_dep(real_deps, &dep_node.name, &dep_node.version) {
                continue;
            }

            let w_unique = dep_graph.unique_subtree_weight(ws_id, dep_id);
            let total_transitive = dep_node.transitive_weight.saturating_sub(1);
            let ancestors = unique_ancestor_count(dep_graph, dep_id);

            entries.push(DirectDepSummary {
                workspace_member: ws_node.name.clone(),
                dep_name: dep_node.name.clone(),
                dep_version: dep_node.version.clone(),
                unique_transitive_deps: w_unique,
                total_transitive_deps: total_transitive,
                unique_ancestors: ancestors,
            });
        }
    }

    entries.sort_by(|a, b| b.unique_transitive_deps.cmp(&a.unique_transitive_deps));
    entries
}

fn now_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("epoch:{secs}")
}

fn empty_report(workspace_root: String, threshold: f64, total_deps: usize) -> AnalysisReport {
    AnalysisReport {
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        timestamp: now_timestamp(),
        workspace_root,
        threshold,
        total_dependencies: total_deps,
        platform_dependencies: None,
        phantom_dependencies: 0,
        heavy_nodes_found: 0,
        targets: Vec::new(),
        dep_tree: None,
        unused_edges: Vec::new(),
        unused_direct_deps: Vec::new(),
        direct_dep_summary: Vec::new(),
    }
}
