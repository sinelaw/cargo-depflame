use crate::flamegraph::DepTreeData;
use crate::metrics::{Confidence, RemovalStrategy, UpstreamTarget};
use crate::scanner::display_path;
use colored::Colorize;
use comfy_table::{modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL, Table};
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
    writeln!(writer, "{}", "depflame — Dependency Analysis Report".bold())?;

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
            "{}",
            "Direct dependencies by unique transitive dep count:".bold()
        )?;
        writeln!(writer)?;

        let mut table = Table::new();
        table
            .load_preset(UTF8_FULL)
            .apply_modifier(UTF8_ROUND_CORNERS)
            .set_header(vec!["#", "Dependency", "Version", "Unique Deps", "Total Deps"]);

        for (i, entry) in report.direct_dep_summary.iter().enumerate() {
            table.add_row(vec![
                format!("{}", i + 1),
                entry.dep_name.clone(),
                entry.dep_version.clone(),
                format!("{}", entry.unique_transitive_deps),
                format!("{}", entry.total_transitive_deps),
            ]);
        }

        writeln!(writer, "{table}")?;
        writeln!(writer)?;
    }

    if report.targets.is_empty() {
        writeln!(
            writer,
            "{}",
            "No actionable targets found. Your dependency tree looks clean!"
                .green()
                .bold()
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
        writeln!(
            writer,
            "{}",
            "Your crate — recommended changes:".bold().green()
        )?;
        writeln!(writer)?;

        // Removals first (highest impact, easiest wins).
        for target in &ws_remove {
            let test_badge = if is_test_or_example_crate(&target.intermediate.name) {
                " [test/example]".dimmed().to_string()
            } else {
                String::new()
            };
            writeln!(
                writer,
                "  {} Remove `{}` from `{}`  (0 code references found){}",
                format!("(-{} deps)", target.w_unique).green(),
                target.heavy_dependency.name.yellow(),
                target.intermediate.name.cyan(),
                test_badge,
            )?;
        }

        // Feature-gate candidates.
        for target in &ws_feature_gate {
            let test_badge = if is_test_or_example_crate(&target.intermediate.name) {
                " [test/example]".dimmed().to_string()
            } else {
                String::new()
            };
            writeln!(
                writer,
                "  {} Make `{}` optional in `{}`  ({} refs){}",
                format!("(-{} deps)", target.w_unique).green(),
                target.heavy_dependency.name.yellow(),
                target.intermediate.name.cyan(),
                target.c_ref,
                test_badge,
            )?;
        }

        // Already-gated: user is enabling a heavy optional feature.
        for target in &ws_already_gated {
            writeln!(
                writer,
                "  {} `{}` is optional in `{}` — check if you need it",
                format!("(-{} deps)", target.w_unique).green(),
                target.heavy_dependency.name.yellow(),
                target.intermediate.name.cyan(),
            )?;
        }

        // Std replacements in user's own crate.
        for target in &ws_std_replacements {
            if let RemovalStrategy::ReplaceWithStd { suggestion } = &target.suggestion {
                writeln!(
                    writer,
                    "  {} Replace `{}` with {} in `{}`",
                    format!("(-{} deps)", target.w_unique).green(),
                    target.heavy_dependency.name.yellow(),
                    suggestion,
                    target.intermediate.name.cyan(),
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
        writeln!(
            writer,
            "{}",
            "Consider inlining (small dep or light usage):".bold()
        )?;
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
                "  {} Inline `{}`{} into `{}`  {}",
                format!("(-{} deps)", target.w_unique).green(),
                target.heavy_dependency.name.yellow(),
                usage_str.dimmed(),
                target.intermediate.name.cyan(),
                chain.dimmed(),
            )?;
        }
        writeln!(writer)?;
    }

    // Section 1: Upstream PRs to propose
    if !upstream_feature_gate.is_empty() {
        writeln!(
            writer,
            "{}",
            "Propose feature-gating in upstream crates:".bold()
        )?;
        writeln!(writer)?;
        for target in &upstream_feature_gate {
            let chain = format_short_chain(&target.dep_chain, &target.intermediate.name);
            writeln!(
                writer,
                "  {} Make `{}` optional in `{}`  {}",
                format!("(-{} deps)", target.w_unique).green(),
                target.heavy_dependency.name.yellow(),
                target.intermediate.name.cyan(),
                chain.dimmed(),
            )?;
        }
        writeln!(writer)?;
    }

    // Section 2: Already-gated upstream deps you might be enabling unnecessarily
    if !upstream_already_gated.is_empty() {
        writeln!(
            writer,
            "{}",
            "Already optional — check if you need these features enabled:".bold()
        )?;
        writeln!(writer)?;
        for target in &upstream_already_gated {
            let chain = format_short_chain(&target.dep_chain, &target.intermediate.name);
            writeln!(
                writer,
                "  {} `{}` is optional in `{}`  {}",
                format!("(-{} deps)", target.w_unique).green(),
                target.heavy_dependency.name.yellow(),
                target.intermediate.name.cyan(),
                chain.dimmed(),
            )?;
        }
        writeln!(writer)?;
    }

    // Section 3: Unused deps in upstream
    if !upstream_remove.is_empty() {
        writeln!(
            writer,
            "{}",
            "Possibly unused (propose removal in upstream):".bold()
        )?;
        writeln!(writer)?;
        for target in &upstream_remove {
            writeln!(
                writer,
                "  {} `{}` appears unused in `{}`",
                format!("(-{} deps)", target.w_unique).green(),
                target.heavy_dependency.name.yellow(),
                target.intermediate.name.cyan(),
            )?;
        }
        writeln!(writer)?;
    }

    // Section 4: std replacements
    if !std_replacements.is_empty() {
        writeln!(writer, "{}", "Replace with std equivalents:".bold())?;
        writeln!(writer)?;
        for target in &std_replacements {
            if let RemovalStrategy::ReplaceWithStd { suggestion } = &target.suggestion {
                writeln!(
                    writer,
                    "  {} Replace `{}` with {} in `{}`",
                    format!("(-{} deps)", target.w_unique).green(),
                    target.heavy_dependency.name.yellow(),
                    suggestion,
                    target.intermediate.name.cyan(),
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
            "{}",
            format!(
                "({noise_count} low-impact targets with 0 unique deps hidden. Use -v to see all.)"
            )
            .dimmed()
        )?;
        writeln!(writer)?;
    }

    // Verbose: full details
    if verbose {
        writeln!(writer)?;
        writeln!(writer, "{}", "=== Detailed Analysis ===".bold())?;
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

        let confidence_colored = match target.confidence {
            Confidence::High => format!("{}", target.confidence).green(),
            Confidence::Medium => format!("{}", target.confidence).yellow(),
            Confidence::Low => format!("{}", target.confidence).red(),
            Confidence::Noise => format!("{}", target.confidence).dimmed(),
        };

        writeln!(writer, "{}", format!("--- #{rank} ---").yellow().bold())?;

        writeln!(
            writer,
            "  {} {} v{}  ->  {} v{}",
            "Edge:".bold(),
            target.intermediate.name.cyan(),
            target.intermediate.version,
            target.heavy_dependency.name.red(),
            target.heavy_dependency.version,
        )?;
        writeln!(
            writer,
            "  {} W_trans={}, W_uniq={}, C_ref={}, hURRS={}",
            "Metrics:".bold(),
            target.w_transitive,
            target.w_unique,
            target.c_ref,
            hurrs_display,
        )?;
        writeln!(
            writer,
            "  {} {} | {} {}",
            "Status:".bold(),
            confidence_colored,
            "Action:".bold(),
            format!("{}", target.suggestion).green(),
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
            writeln!(
                writer,
                "  {} [{}]",
                "Flags:".bold(),
                badges.join(", ").dimmed()
            )?;
        }

        // Dependency chain.
        if !target.dep_chain.is_empty() {
            writeln!(
                writer,
                "  {} {}",
                "Chain:".bold(),
                target.dep_chain.join(" -> ").dimmed()
            )?;
        }

        // File matches.
        if !target.scan_result.file_matches.is_empty() {
            writeln!(writer, "  {}:", "Refs".bold())?;
            let mut current_file = String::new();
            for m in &target.scan_result.file_matches {
                let display = display_path(&m.path);
                if display != current_file {
                    let label = if m.in_generated_file {
                        format!("    {} (generated)", display)
                    } else {
                        format!("    {display}")
                    };
                    writeln!(writer, "{}", label.dimmed())?;
                    current_file = display;
                }
                writeln!(
                    writer,
                    "      L{}: {}",
                    m.line_number,
                    m.line_content.bright_white()
                )?;
            }
        }
        writeln!(writer)?;
    }

    // Summary table.
    writeln!(writer, "{}", "=== Summary Table ===".bold())?;
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_header(vec![
            "#",
            "Intermediate",
            "Heavy Dep",
            "W_uniq",
            "C_ref",
            "Confidence",
            "Action",
        ]);

    for (i, target) in report.targets.iter().enumerate() {
        table.add_row(vec![
            format!("{}", i + 1),
            target.intermediate.name.clone(),
            target.heavy_dependency.name.clone(),
            format!("{}", target.w_unique),
            format!("{}", target.c_ref),
            format!("{}", target.confidence),
            format!("{}", target.suggestion),
        ]);
    }

    writeln!(writer, "{table}")?;
    writeln!(writer)?;

    Ok(())
}
