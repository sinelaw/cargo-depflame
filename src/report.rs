use crate::flamegraph::DepTreeData;
use crate::metrics::{Confidence, RemovalStrategy, UpstreamTarget};
use crate::scanner::display_path;
use serde::{Deserialize, Serialize};
use std::io::Write;

/// The full serializable analysis report.
#[derive(Debug, Serialize, Deserialize)]
pub struct AnalysisReport {
    pub tool_version: String,
    pub timestamp: String,
    pub workspace_root: String,
    pub threshold: f64,
    /// Total deps in the full cross-platform resolve graph.
    pub total_dependencies: usize,
    /// Deps actually resolved for the current platform (None if detection failed).
    #[serde(default)]
    pub platform_dependencies: Option<usize>,
    /// Number of phantom deps (in metadata but not on this platform).
    #[serde(default)]
    pub phantom_dependencies: usize,
    pub heavy_nodes_found: usize,
    pub targets: Vec<UpstreamTarget>,
    /// Serialized dependency tree for flamegraph rendering.
    /// Populated during `analyze`; allows `report --format svg` to work from saved JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dep_tree: Option<DepTreeData>,
    /// Edges where the parent crate doesn't reference the child in its source.
    /// Each entry is `(parent_name, child_name)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unused_edges: Vec<(String, String)>,
    /// Unused direct dependencies of workspace members detected in Phase 5d.
    /// Each entry describes a workspace member -> dep edge with 0 code references.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unused_direct_deps: Vec<UnusedDirectDep>,
    /// Direct dependencies of workspace members sorted by unique transitive dep count.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub direct_dep_summary: Vec<DirectDepSummary>,
}

/// Summary of a direct dependency's unique transitive dep contribution.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DirectDepSummary {
    /// The workspace member that declares this dependency.
    pub workspace_member: String,
    /// The dependency name.
    pub dep_name: String,
    /// The dependency version.
    pub dep_version: String,
    /// Number of transitive deps unique to this edge (would disappear if cut).
    pub unique_transitive_deps: usize,
    /// Total transitive deps (including shared ones).
    pub total_transitive_deps: usize,
    /// Number of unique ancestors (packages that transitively depend on this one).
    #[serde(default)]
    pub unique_ancestors: usize,
}

/// A direct dependency of a workspace member that appears unused (0 code references).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UnusedDirectDep {
    /// The workspace member that declares this dependency.
    pub from_crate: String,
    /// The dependency name.
    pub dep_name: String,
    /// The dependency version.
    pub dep_version: String,
    /// Number of real (non-phantom) deps that would be saved if removed.
    pub real_deps_saved: usize,
    /// Whether this dep is in a test/example/bench crate.
    pub is_test_example: bool,
}

/// Render the report as JSON.
pub fn render_json(report: &AnalysisReport, writer: &mut dyn Write) -> anyhow::Result<()> {
    serde_json::to_writer_pretty(&mut *writer, report)?;
    writeln!(writer)?;
    Ok(())
}

/// Render a concise actionable summary (default text mode).
pub fn render_text(
    report: &AnalysisReport,
    writer: &mut dyn Write,
    verbose: bool,
) -> anyhow::Result<()> {
    // Header.
    writeln!(writer)?;
    writeln!(writer, "depflame — Dependency Analysis Report")?;

    if let Some(platform_deps) = report.platform_dependencies {
        writeln!(
            writer,
            "{} dependencies ({} compiled on this platform)",
            report.total_dependencies, platform_deps
        )?;
    } else {
        writeln!(writer, "{} dependencies", report.total_dependencies)?;
    }
    writeln!(writer)?;

    // ── DIRECT DEPENDENCY OVERVIEW ──
    if !report.direct_dep_summary.is_empty() {
        writeln!(
            writer,
            "Direct dependencies by unique transitive dep count:"
        )?;
        writeln!(writer)?;

        let name_w = report
            .direct_dep_summary
            .iter()
            .map(|e| e.dep_name.len())
            .max()
            .unwrap_or(10)
            .max(10);
        let ver_w = report
            .direct_dep_summary
            .iter()
            .map(|e| e.dep_version.len())
            .max()
            .unwrap_or(7)
            .max(7);
        let idx_w = format!("{}", report.direct_dep_summary.len()).len().max(1);

        writeln!(
            writer,
            "  {:>idx_w$}  {:<name_w$}  {:<ver_w$}  {:>6}  {:>5}  {:>9}",
            "#",
            "Dependency",
            "Version",
            "Unique",
            "Total",
            "Ancestors",
            idx_w = idx_w,
            name_w = name_w,
            ver_w = ver_w,
        )?;
        writeln!(
            writer,
            "  {:─>idx_w$}  {:─<name_w$}  {:─<ver_w$}  {:─>6}  {:─>5}  {:─>9}",
            "",
            "",
            "",
            "",
            "",
            "",
            idx_w = idx_w,
            name_w = name_w,
            ver_w = ver_w,
        )?;
        for (i, entry) in report.direct_dep_summary.iter().enumerate() {
            writeln!(
                writer,
                "  {:>idx_w$}  {:<name_w$}  {:<ver_w$}  {:>6}  {:>5}  {:>9}",
                i + 1,
                entry.dep_name,
                entry.dep_version,
                entry.unique_transitive_deps,
                entry.total_transitive_deps,
                entry.unique_ancestors,
                idx_w = idx_w,
                name_w = name_w,
                ver_w = ver_w,
            )?;
        }
        writeln!(writer)?;
    }

    if report.targets.is_empty() {
        writeln!(
            writer,
            "No actionable targets found. Your dependency tree looks clean!"
        )?;
        return Ok(());
    }

    let is_actionable = |t: &UpstreamTarget| -> bool {
        matches!(t.confidence, Confidence::High | Confidence::Medium) && t.w_unique > 0
    };

    // ── YOUR CRATE: workspace-member findings (most actionable) ──

    let ws_remove: Vec<&UpstreamTarget> = report
        .targets
        .iter()
        .filter(|t| {
            is_actionable(t)
                && matches!(t.suggestion, RemovalStrategy::Remove)
                && t.intermediate_is_workspace_member
        })
        .collect();

    let ws_feature_gate: Vec<&UpstreamTarget> = report
        .targets
        .iter()
        .filter(|t| {
            is_actionable(t)
                && matches!(t.suggestion, RemovalStrategy::FeatureGate)
                && t.intermediate_is_workspace_member
        })
        .collect();

    let ws_already_gated: Vec<&UpstreamTarget> = report
        .targets
        .iter()
        .filter(|t| {
            is_actionable(t)
                && matches!(t.suggestion, RemovalStrategy::AlreadyGated { .. })
                && t.intermediate_is_workspace_member
        })
        .collect();

    let ws_std_replacements: Vec<&UpstreamTarget> = report
        .targets
        .iter()
        .filter(|t| {
            is_actionable(t)
                && matches!(t.suggestion, RemovalStrategy::ReplaceWithStd { .. })
                && t.intermediate_is_workspace_member
        })
        .collect();

    let has_ws_findings = !ws_remove.is_empty()
        || !ws_feature_gate.is_empty()
        || !ws_already_gated.is_empty()
        || !ws_std_replacements.is_empty();

    if has_ws_findings {
        writeln!(writer, "Your crate — recommended changes:")?;
        writeln!(writer)?;

        // Removals first (highest impact, easiest wins).
        for target in &ws_remove {
            let test_badge = if is_test_or_example_crate(&target.intermediate.name) {
                " [test/example]"
            } else {
                ""
            };
            writeln!(
                writer,
                "  (-{} deps) Remove `{}` from `{}`  (0 code references found){}",
                target.w_unique, target.heavy_dependency.name, target.intermediate.name, test_badge,
            )?;
        }

        // Feature-gate candidates.
        for target in &ws_feature_gate {
            let test_badge = if is_test_or_example_crate(&target.intermediate.name) {
                " [test/example]"
            } else {
                ""
            };
            writeln!(
                writer,
                "  (-{} deps) Make `{}` optional in `{}`  ({} refs){}",
                target.w_unique,
                target.heavy_dependency.name,
                target.intermediate.name,
                target.c_ref,
                test_badge,
            )?;
        }

        // Already-gated: user is enabling a heavy optional feature.
        for target in &ws_already_gated {
            writeln!(
                writer,
                "  (-{} deps) `{}` is optional in `{}` — check if you need it",
                target.w_unique, target.heavy_dependency.name, target.intermediate.name,
            )?;
        }

        // Std replacements in user's own crate.
        for target in &ws_std_replacements {
            if let RemovalStrategy::ReplaceWithStd { suggestion } = &target.suggestion {
                writeln!(
                    writer,
                    "  (-{} deps) Replace `{}` with {} in `{}`",
                    target.w_unique,
                    target.heavy_dependency.name,
                    suggestion,
                    target.intermediate.name,
                )?;
            }
        }

        writeln!(writer)?;
    }

    // ── UPSTREAM: findings in third-party crates ──

    // Group targets by action type for the summary.
    let upstream_feature_gate: Vec<&UpstreamTarget> = report
        .targets
        .iter()
        .filter(|t| {
            is_actionable(t)
                && matches!(t.suggestion, RemovalStrategy::FeatureGate)
                && !t.intermediate_is_workspace_member
        })
        .collect();

    let upstream_already_gated: Vec<&UpstreamTarget> = report
        .targets
        .iter()
        .filter(|t| {
            is_actionable(t)
                && matches!(t.suggestion, RemovalStrategy::AlreadyGated { .. })
                && !t.intermediate_is_workspace_member
        })
        .collect();

    let upstream_remove: Vec<&UpstreamTarget> = report
        .targets
        .iter()
        .filter(|t| {
            is_actionable(t)
                && matches!(t.suggestion, RemovalStrategy::Remove)
                && !t.intermediate_is_workspace_member
        })
        .collect();

    let std_replacements: Vec<&UpstreamTarget> = report
        .targets
        .iter()
        .filter(|t| {
            is_actionable(t)
                && matches!(t.suggestion, RemovalStrategy::ReplaceWithStd { .. })
                && !t.intermediate_is_workspace_member
        })
        .collect();

    let inline_candidates: Vec<&UpstreamTarget> = report
        .targets
        .iter()
        .filter(|t| {
            is_actionable(t) && matches!(t.suggestion, RemovalStrategy::InlineUpstream { .. })
        })
        .collect();

    // Section 0: Inline candidates (small deps / light usage)
    if !inline_candidates.is_empty() {
        writeln!(writer, "Consider inlining (small dep or light usage):")?;
        writeln!(writer)?;
        for target in &inline_candidates {
            let chain = format_short_chain(&target.dep_chain, &target.intermediate.name);
            let usage_str = if target.heavy_dep_loc > 0 {
                format!(" ({} LOC)", target.heavy_dep_loc)
            } else {
                String::new()
            };
            writeln!(
                writer,
                "  (-{} deps) Inline `{}`{} into `{}`  {}",
                target.w_unique,
                target.heavy_dependency.name,
                usage_str,
                target.intermediate.name,
                chain,
            )?;
        }
        writeln!(writer)?;
    }

    // Section 1: Upstream PRs to propose
    if !upstream_feature_gate.is_empty() {
        writeln!(writer, "Propose feature-gating in upstream crates:")?;
        writeln!(writer)?;
        for target in &upstream_feature_gate {
            let chain = format_short_chain(&target.dep_chain, &target.intermediate.name);
            writeln!(
                writer,
                "  (-{} deps) Make `{}` optional in `{}`  {}",
                target.w_unique, target.heavy_dependency.name, target.intermediate.name, chain,
            )?;
        }
        writeln!(writer)?;
    }

    // Section 2: Already-gated upstream deps you might be enabling unnecessarily
    if !upstream_already_gated.is_empty() {
        writeln!(
            writer,
            "Already optional — check if you need these features enabled:"
        )?;
        writeln!(writer)?;
        for target in &upstream_already_gated {
            let chain = format_short_chain(&target.dep_chain, &target.intermediate.name);
            writeln!(
                writer,
                "  (-{} deps) `{}` is optional in `{}`  {}",
                target.w_unique, target.heavy_dependency.name, target.intermediate.name, chain,
            )?;
        }
        writeln!(writer)?;
    }

    // Section 3: Unused deps in upstream
    if !upstream_remove.is_empty() {
        writeln!(writer, "Possibly unused (propose removal in upstream):")?;
        writeln!(writer)?;
        for target in &upstream_remove {
            writeln!(
                writer,
                "  (-{} deps) `{}` appears unused in `{}`",
                target.w_unique, target.heavy_dependency.name, target.intermediate.name,
            )?;
        }
        writeln!(writer)?;
    }

    // Section 4: std replacements
    if !std_replacements.is_empty() {
        writeln!(writer, "Replace with std equivalents:")?;
        writeln!(writer)?;
        for target in &std_replacements {
            if let RemovalStrategy::ReplaceWithStd { suggestion } = &target.suggestion {
                writeln!(
                    writer,
                    "  (-{} deps) Replace `{}` with {} in `{}`",
                    target.w_unique,
                    target.heavy_dependency.name,
                    suggestion,
                    target.intermediate.name,
                )?;
            }
        }
        writeln!(writer)?;
    }

    // Summary: noise targets suppressed.
    let noise_count = report
        .targets
        .iter()
        .filter(|t| t.confidence == Confidence::Low && t.w_unique == 0)
        .count();
    if noise_count > 0 {
        writeln!(
            writer,
            "({noise_count} low-impact targets with 0 unique deps hidden. Use -v to see all.)"
        )?;
        writeln!(writer)?;
    }

    // Verbose: full details
    if verbose {
        writeln!(writer)?;
        writeln!(writer, "=== Detailed Analysis ===")?;
        writeln!(writer)?;
        render_detailed(report, writer)?;
    }

    Ok(())
}

/// Check if a crate name looks like a test, example, benchmark, or doc crate.
fn is_test_or_example_crate(name: &str) -> bool {
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

/// Format a short dependency chain showing how the workspace reaches this dep.
fn format_short_chain(dep_chain: &[String], intermediate_name: &str) -> String {
    if dep_chain.len() <= 2 {
        return String::new();
    }
    // Show: workspace_crate -> ... -> intermediate
    let first = &dep_chain[0];
    if dep_chain.len() == 3 {
        format!("(via {first} -> {intermediate_name})")
    } else {
        format!("(via {first} -> ... -> {intermediate_name})",)
    }
}

/// Render the full detailed analysis (--verbose mode).
fn render_detailed(report: &AnalysisReport, writer: &mut dyn Write) -> anyhow::Result<()> {
    for (i, target) in report.targets.iter().enumerate() {
        let rank = i + 1;
        let hurrs_display = if target.hurrs.is_none() {
            "INF".to_string()
        } else {
            format!("{:.1}", target.hurrs.unwrap_or(0.0))
        };

        writeln!(writer, "--- #{rank} ---")?;

        writeln!(
            writer,
            "  Edge: {} v{}  ->  {} v{}",
            target.intermediate.name,
            target.intermediate.version,
            target.heavy_dependency.name,
            target.heavy_dependency.version,
        )?;
        writeln!(
            writer,
            "  Metrics: W_trans={}, W_uniq={}, C_ref={}, hURRS={}",
            target.w_transitive, target.w_unique, target.c_ref, hurrs_display,
        )?;
        writeln!(
            writer,
            "  Status: {} | Action: {}",
            target.confidence, target.suggestion,
        )?;

        // Badges.
        let mut badges = Vec::new();
        if target.phantom {
            badges.push("PHANTOM");
        }
        if target.intermediate_is_workspace_member {
            if is_test_or_example_crate(&target.intermediate.name) {
                badges.push("YOUR-CRATE (test/example)");
            } else {
                badges.push("YOUR-CRATE");
            }
        }
        if target.edge_meta.build_only {
            badges.push("BUILD-ONLY");
        }
        if target.edge_meta.already_optional {
            badges.push("ALREADY-OPTIONAL");
        }
        if target.edge_meta.platform_conditional {
            badges.push("PLATFORM-CONDITIONAL");
        }
        if !badges.is_empty() {
            writeln!(writer, "  Flags: [{}]", badges.join(", "))?;
        }

        // Dependency chain.
        if !target.dep_chain.is_empty() {
            writeln!(writer, "  Chain: {}", target.dep_chain.join(" -> "))?;
        }

        // File matches.
        if !target.scan_result.file_matches.is_empty() {
            writeln!(writer, "  Refs:")?;
            let mut current_file = String::new();
            for m in &target.scan_result.file_matches {
                let display = display_path(&m.path);
                if display != current_file {
                    let label = if m.in_generated_file {
                        format!("    {} (generated)", display)
                    } else {
                        format!("    {display}")
                    };
                    writeln!(writer, "{label}")?;
                    current_file = display;
                }
                writeln!(writer, "      L{}: {}", m.line_number, m.line_content)?;
            }
        }
        writeln!(writer)?;
    }

    // Summary table.
    writeln!(writer, "=== Summary Table ===")?;
    let int_w = report
        .targets
        .iter()
        .map(|t| t.intermediate.name.len())
        .max()
        .unwrap_or(12)
        .max(12);
    let dep_w = report
        .targets
        .iter()
        .map(|t| t.heavy_dependency.name.len())
        .max()
        .unwrap_or(9)
        .max(9);
    let act_w = report
        .targets
        .iter()
        .map(|t| format!("{}", t.suggestion).len())
        .max()
        .unwrap_or(6)
        .max(6);
    let idx_w = format!("{}", report.targets.len()).len().max(1);

    writeln!(
        writer,
        "  {:>idx_w$}  {:<int_w$}  {:<dep_w$}  {:>6}  {:>5}  {:>10}  {:<act_w$}",
        "#",
        "Intermediate",
        "Heavy Dep",
        "W_uniq",
        "C_ref",
        "Confidence",
        "Action",
        idx_w = idx_w,
        int_w = int_w,
        dep_w = dep_w,
        act_w = act_w,
    )?;
    writeln!(
        writer,
        "  {:─>idx_w$}  {:─<int_w$}  {:─<dep_w$}  {:─>6}  {:─>5}  {:─>10}  {:─<act_w$}",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        idx_w = idx_w,
        int_w = int_w,
        dep_w = dep_w,
        act_w = act_w,
    )?;
    for (i, target) in report.targets.iter().enumerate() {
        writeln!(
            writer,
            "  {:>idx_w$}  {:<int_w$}  {:<dep_w$}  {:>6}  {:>5}  {:>10}  {:<act_w$}",
            i + 1,
            target.intermediate.name,
            target.heavy_dependency.name,
            target.w_unique,
            target.c_ref,
            target.confidence,
            target.suggestion,
            idx_w = idx_w,
            int_w = int_w,
            dep_w = dep_w,
            act_w = act_w,
        )?;
    }
    writeln!(writer)?;

    Ok(())
}
