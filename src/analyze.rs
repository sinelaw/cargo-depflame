use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::cli::AnalyzeArgs;
use crate::crate_fetch;
use crate::flamegraph;
use crate::graph::{DepGraph, EdgeMeta, IntermediateEdge};
use crate::metrics::{
    self, ComputeTargetInput, Confidence, PackageInfo, RemovalStrategy, UpstreamTarget,
};
use crate::report::{AnalysisReport, DirectDepSummary, UnusedDirectDep};
use crate::{platform, registry, scanner};

/// Run the full analysis pipeline and return the report.
///
/// This function does NOT handle output formatting or writing — the caller
/// is responsible for rendering/writing the returned `AnalysisReport`.
pub fn run_analyze(args: &AnalyzeArgs) -> Result<AnalysisReport> {
    // If --crate is specified, fetch from crates.io and analyze the extracted crate.
    let _temp_dir; // keep tempdir alive for the duration of analysis
    let manifest_path = if let Some(ref spec_str) = args.common.crate_spec {
        let spec = crate_fetch::parse_crate_spec(spec_str)?;
        let tmp = tempfile::tempdir().context("failed to create temp directory")?;
        let crate_dir = crate_fetch::fetch_and_extract(&spec, tmp.path())?;
        _temp_dir = Some(tmp);
        crate_dir.join("Cargo.toml")
    } else {
        _temp_dir = None;
        args.common.manifest_path.clone()
    };

    // Phase 1: Load metadata.
    eprintln!("Loading workspace metadata...");
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(&manifest_path)
        .exec()
        .context("failed to run cargo metadata")?;

    let workspace_root = metadata.workspace_root.to_string();

    // Phase 2: Build dependency graph and find heavy nodes.
    eprintln!("Building dependency graph...");
    let dep_graph = crate::graph::DepGraph::from_metadata(&metadata)?;
    let total_deps = dep_graph.total_dependency_count();
    let heavy_nodes = dep_graph.heavy_nodes(args.common.heavy_threshold);
    eprintln!(
        "Found {} heavy nodes (W_transitive > {}) out of {} total dependencies",
        heavy_nodes.len(),
        args.common.heavy_threshold,
        total_deps
    );

    if heavy_nodes.is_empty() {
        eprintln!("No heavy nodes found. Your dependency tree is lean!");
        return Ok(empty_report(
            workspace_root,
            args.common.threshold,
            total_deps,
        ));
    }

    // Phase 2b: Find intermediate edges.
    let edges = dep_graph.intermediate_edges(&heavy_nodes);
    eprintln!("Found {} intermediate edges to scan", edges.len());

    if edges.is_empty() {
        eprintln!("No intermediate dependency edges to analyze.");
        return Ok(empty_report(
            workspace_root,
            args.common.threshold,
            total_deps,
        ));
    }

    // Phase 2c: Resolve real platform deps to detect phantom deps.
    eprintln!("Resolving platform-specific dependency tree...");
    let real_deps = platform::resolve_real_deps(&manifest_path);
    if real_deps.is_none() {
        eprintln!("  [WARN] Could not resolve platform deps, phantom detection disabled");
    }

    // Phase 3 & 4: Source retrieval + heuristic scanning (parallelized).
    eprintln!("Scanning intermediate crate sources...");
    let targets: Vec<UpstreamTarget> = edges
        .par_iter()
        .filter_map(|edge| scan_edge(edge, &dep_graph, &metadata, &real_deps))
        .collect();

    // Phase 5b: Rank.
    let mut ranked = metrics::rank_targets(targets, args.common.threshold, args.common.top);

    // Phase 5c: Enrich AlreadyGated suggestions with feature info.
    enrich_features(&mut ranked, &metadata);

    // Phase 5d: Detect completely unused direct dependencies of workspace members.
    eprintln!("Checking for unused direct dependencies...");
    let already_analyzed: HashSet<(&str, &str)> = ranked
        .iter()
        .map(|t| {
            (
                t.intermediate.name.as_str(),
                t.heavy_dependency.name.as_str(),
            )
        })
        .collect();
    let unused_deps = find_unused_deps(&dep_graph, &metadata, &real_deps, &already_analyzed);
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
    if !unused_deps.is_empty() {
        eprintln!("  Found {} unused direct dependencies", unused_deps.len());
        ranked = merge_unused(ranked, unused_deps);
    }

    // Phase 6: Build the report.
    let platform_deps = real_deps.as_ref().map(|s| s.len());
    let phantom_deps = platform_deps
        .map(|p| total_deps.saturating_sub(p))
        .unwrap_or(0);

    // Phase 5e: Build direct dependency summary for workspace members.
    let direct_dep_summary = build_direct_dep_summary(&dep_graph, &real_deps);

    eprintln!("Building dependency tree for visualization...");
    let dep_tree = flamegraph::build_dep_tree(&dep_graph);

    let unused_edges: Vec<(String, String)> = ranked
        .iter()
        .filter(|t| t.c_ref == 0)
        .map(|t| (t.intermediate.name.clone(), t.heavy_dependency.name.clone()))
        .collect();

    Ok(AnalysisReport {
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        workspace_root,
        threshold: args.common.threshold,
        total_dependencies: total_deps,
        platform_dependencies: platform_deps,
        phantom_dependencies: phantom_deps,
        heavy_nodes_found: heavy_nodes.len(),
        targets: ranked,
        dep_tree: Some(dep_tree),
        unused_edges,
        unused_direct_deps: unused_direct_deps_summary,
        direct_dep_summary,
    })
}

/// Check if a crate name looks like a test, example, benchmark, or doc crate.
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

/// Look up the lib target name for a package from cargo_metadata.
///
/// When a crate sets `[lib] name = "foo"` in its Cargo.toml, Rust code imports
/// it as `foo`, not the package name. `cargo_metadata` exposes this via the
/// `targets` array on each `Package`. Returns `None` if the package has no lib
/// target or if the lib target name matches the normalized package name.
fn lib_target_name(metadata: &cargo_metadata::Metadata, pkg_name: &str) -> Option<String> {
    let pkg = metadata.packages.iter().find(|p| p.name == pkg_name)?;
    let lib_target = pkg.targets.iter().find(|t| {
        t.kind
            .iter()
            .any(|k| k == "lib" || k == "rlib" || k == "proc-macro")
    })?;
    let normalized_pkg = pkg_name.replace('-', "_");
    if lib_target.name == normalized_pkg {
        None // No mismatch — the default name matches.
    } else {
        Some(lib_target.name.clone())
    }
}

/// Scan a single intermediate edge: locate source, run heuristic scanner,
/// and compute metrics.
fn scan_edge(
    edge: &IntermediateEdge,
    dep_graph: &DepGraph,
    metadata: &cargo_metadata::Metadata,
    real_deps: &Option<HashSet<String>>,
) -> Option<UpstreamTarget> {
    // Phase 3: Locate source.
    let src_dir = registry::find_crate_source(&edge.intermediate_name, &edge.intermediate_version);

    let intermediate_pkg = metadata
        .packages
        .iter()
        .find(|p| p.id == edge.intermediate_id);

    let src_dir: PathBuf = match src_dir {
        Some(d) => d,
        None => {
            // Workspace members — scan from metadata source path.
            if dep_graph.workspace_members.contains(&edge.intermediate_id) {
                intermediate_pkg?.manifest_path.parent().map(|p| p.into())?
            } else {
                eprintln!(
                    "  [WARN] Source not found for {} v{}, skipping",
                    edge.intermediate_name, edge.intermediate_version
                );
                return None;
            }
        }
    };

    // Phase 3b: Look up dependency info from cargo_metadata.
    let dep_meta = intermediate_pkg
        .and_then(|pkg| pkg.dependencies.iter().find(|d| d.name == edge.heavy_name));

    // Determine the local alias for the heavy dependency.
    let alias = dep_meta.and_then(|d| d.rename.clone()).or_else(|| {
        // If no explicit rename, the local name is the crate name with hyphens as underscores.
        Some(edge.heavy_name.replace('-', "_"))
    });
    let was_renamed = dep_meta
        .and_then(|d| d.rename.as_ref())
        .is_some_and(|r| *r != edge.heavy_name.replace('-', "_"));

    let mut aliases: Vec<String> = Vec::new();
    if let Some(ref a) = alias {
        aliases.push(a.clone());
    }

    // Also add the lib target name if it differs from the package name.
    // Crates like `natord-plus-plus` set `[lib] name = "natord"`, so Rust code
    // imports them under that name, not the package name.
    if let Some(lib_name) = lib_target_name(metadata, &edge.heavy_name) {
        let lib_norm = lib_name.replace('-', "_");
        if !aliases.contains(&lib_norm) {
            aliases.push(lib_norm);
        }
    }

    // Build enriched edge metadata from cargo_metadata.
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

    // Phase 4: Scan.
    let rs_files = registry::collect_rs_files(&src_dir);
    if rs_files.is_empty() {
        return None;
    }

    let scan = scanner::scan_files_with_aliases(&rs_files, &edge.heavy_name, &aliases);

    // Phase 4b: Measure heavy dep LOC and its own dep count.
    let heavy_dep_loc = registry::find_crate_source(&edge.heavy_name, &edge.heavy_version)
        .map(|heavy_dir| {
            let heavy_rs = registry::collect_rs_files(&heavy_dir);
            registry::count_loc(&heavy_rs)
        })
        .unwrap_or(0);
    let heavy_dep_own_deps = dep_graph.direct_dep_count(&edge.heavy_id);
    let has_re_export_all = scan.has_re_export_all;

    // Phase 4c: Compute unique subtree weight.
    let w_unique = dep_graph.unique_subtree_weight(&edge.intermediate_id, &edge.heavy_id);

    // Phase 4d: Compute dependency chain.
    let dep_chain = dep_graph.dependency_chain(&edge.heavy_id);

    // Phase 4e: Check if a sibling dep transitively requires the heavy dep.
    let required_by_sibling = dep_graph.sibling_requires(&edge.intermediate_id, &edge.heavy_id);

    // Phase 4e: Check if the heavy dep is a phantom (not on this platform).
    let phantom = !platform::is_real_dep(real_deps, &edge.heavy_name, &edge.heavy_version);

    // Phase 4f: Check if intermediate is a workspace member.
    let intermediate_is_ws = dep_graph.workspace_members.contains(&edge.intermediate_id);

    // Phase 4g: Check if the intermediate is a standalone integration crate
    // (not depended on by any other workspace member). In that case, the crate
    // is already effectively opt-in — suggesting "feature-gate" its primary dep
    // is misleading since users only pull it in explicitly.
    let is_standalone_integration =
        intermediate_is_ws && dep_graph.is_standalone_workspace_member(&edge.intermediate_id);

    // Phase 5: Compute metrics.
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
    }))
}

/// Enrich `AlreadyGated` suggestions with enabling-feature information
/// from `cargo_metadata`.
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
                // Find which features of the intermediate crate enable the heavy dep.
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

                // Check if any enabling feature is part of "default".
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

/// Detect completely unused direct dependencies of workspace members.
fn find_unused_deps(
    dep_graph: &DepGraph,
    metadata: &cargo_metadata::Metadata,
    real_deps: &Option<HashSet<String>>,
    already_analyzed: &HashSet<(&str, &str)>,
) -> Vec<UpstreamTarget> {
    let mut unused_deps: Vec<UpstreamTarget> = Vec::new();

    for ws_id in &dep_graph.workspace_members {
        let ws_pkg = match metadata.packages.iter().find(|p| &p.id == ws_id) {
            Some(p) => p,
            None => continue,
        };
        let ws_dir: PathBuf = match ws_pkg.manifest_path.parent() {
            Some(p) => p.into(),
            None => continue,
        };
        let ws_rs_files = registry::collect_rs_files(&ws_dir);
        if ws_rs_files.is_empty() {
            continue;
        }

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
            if already_analyzed.contains(&(ws_pkg.name.as_str(), dep_node.name.as_str())) {
                continue;
            }
            if let Some(meta) = dep_graph.edge_meta.get(&(ws_id.clone(), dep_id.clone())) {
                if meta.build_only {
                    continue;
                }
            }

            // Build aliases: include Cargo.toml rename and lib target name.
            let mut dep_aliases: Vec<String> = Vec::new();
            if let Some(dep_meta) = ws_pkg.dependencies.iter().find(|d| d.name == dep_node.name) {
                if let Some(ref rename) = dep_meta.rename {
                    let rename_norm = rename.replace('-', "_");
                    if !dep_aliases.contains(&rename_norm) {
                        dep_aliases.push(rename_norm);
                    }
                }
            }
            if let Some(lib_name) = lib_target_name(metadata, &dep_node.name) {
                let lib_norm = lib_name.replace('-', "_");
                if !dep_aliases.contains(&lib_norm) {
                    dep_aliases.push(lib_norm);
                }
            }

            let scan = scanner::scan_files_with_aliases(&ws_rs_files, &dep_node.name, &dep_aliases);
            if scan.ref_count == 0 && !scan.has_re_export_all {
                let w_unique = dep_graph.unique_subtree_weight(ws_id, dep_id);
                let dep_chain = vec![ws_pkg.name.clone(), dep_node.name.clone()];
                let edge_meta = dep_graph
                    .edge_meta
                    .get(&(ws_id.clone(), dep_id.clone()))
                    .cloned()
                    .unwrap_or(EdgeMeta {
                        build_only: false,
                        already_optional: false,
                        platform_conditional: false,
                    });

                unused_deps.push(UpstreamTarget {
                    intermediate: PackageInfo {
                        name: ws_pkg.name.clone(),
                        version: ws_pkg.version.to_string(),
                    },
                    heavy_dependency: PackageInfo {
                        name: dep_node.name.clone(),
                        version: dep_node.version.clone(),
                    },
                    w_transitive: dep_node.transitive_weight,
                    w_unique,
                    c_ref: 0,
                    hurrs: None,
                    confidence: Confidence::High,
                    scan_result: scan,
                    suggestion: RemovalStrategy::Remove,
                    edge_meta,
                    dep_chain,
                    required_by_sibling: None,
                    phantom: !platform::is_real_dep(real_deps, &dep_node.name, &dep_node.version),
                    intermediate_is_workspace_member: true,
                    is_standalone_integration: dep_graph.is_standalone_workspace_member(ws_id),
                    heavy_dep_loc: 0,
                    heavy_dep_own_deps: dep_graph.direct_dep_count(dep_id),
                    has_re_export_all: false,
                });
            }
        }
    }

    unused_deps
}

/// Merge unused deps into ranked results: impactful first, then existing, then cosmetic.
fn merge_unused(
    mut ranked: Vec<UpstreamTarget>,
    unused_deps: Vec<UpstreamTarget>,
) -> Vec<UpstreamTarget> {
    let (mut impactful, mut cosmetic): (Vec<_>, Vec<_>) =
        unused_deps.into_iter().partition(|t| t.w_unique > 0);
    impactful.sort_by(|a, b| b.w_unique.cmp(&a.w_unique));
    cosmetic.sort_by(|a, b| b.w_transitive.cmp(&a.w_transitive));
    for t in &mut cosmetic {
        t.confidence = Confidence::Low;
    }
    impactful.append(&mut ranked);
    impactful.append(&mut cosmetic);
    impactful
}

/// Build a summary of all direct dependencies for each workspace member,
/// ordered by how many unique (not shared) transitive deps each one brings in.
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
            // Skip workspace members (internal crates).
            if dep_node.is_workspace_member {
                continue;
            }
            // Skip phantom deps (not on this platform).
            if !platform::is_real_dep(real_deps, &dep_node.name, &dep_node.version) {
                continue;
            }

            let w_unique = dep_graph.unique_subtree_weight(ws_id, dep_id);
            // total_transitive_deps: count of non-self transitive deps
            let total_transitive = dep_node.transitive_weight.saturating_sub(1);

            entries.push(DirectDepSummary {
                workspace_member: ws_node.name.clone(),
                dep_name: dep_node.name.clone(),
                dep_version: dep_node.version.clone(),
                unique_transitive_deps: w_unique,
                total_transitive_deps: total_transitive,
            });
        }
    }

    // Sort by unique transitive deps descending.
    entries.sort_by(|a, b| b.unique_transitive_deps.cmp(&a.unique_transitive_deps));
    entries
}

/// Build an empty report (used when there are no heavy nodes or edges to analyze).
fn empty_report(workspace_root: String, threshold: f64, total_deps: usize) -> AnalysisReport {
    AnalysisReport {
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
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
