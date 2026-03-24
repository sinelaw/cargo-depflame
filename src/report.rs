use crate::metrics::{RemovalStrategy, UpstreamTarget};
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
    pub total_dependencies: usize,
    pub fat_nodes_found: usize,
    pub targets: Vec<UpstreamTarget>,
}

/// Render the report as JSON.
pub fn render_json(report: &AnalysisReport, writer: &mut dyn Write) -> anyhow::Result<()> {
    serde_json::to_writer_pretty(&mut *writer, report)?;
    writeln!(writer)?;
    Ok(())
}

/// Render the report as human-readable text.
pub fn render_text(report: &AnalysisReport, writer: &mut dyn Write) -> anyhow::Result<()> {
    writeln!(writer)?;
    writeln!(
        writer,
        "{}",
        "=== Upstream Dependency Triage Report ===".bold()
    )?;
    writeln!(writer, "Workspace:          {}", report.workspace_root)?;
    writeln!(writer, "Total dependencies: {}", report.total_dependencies)?;
    writeln!(writer, "Fat nodes found:    {}", report.fat_nodes_found)?;
    writeln!(
        writer,
        "hURRS threshold:    {:.1}",
        report.threshold
    )?;
    writeln!(writer, "Targets found:      {}", report.targets.len())?;
    writeln!(writer)?;

    if report.targets.is_empty() {
        writeln!(
            writer,
            "{}",
            "No high-ROI upstream targets found. Your dependency tree looks clean!"
                .green()
                .bold()
        )?;
        return Ok(());
    }

    for (i, target) in report.targets.iter().enumerate() {
        let rank = i + 1;
        let hurrs_display = if target.hurrs.is_none() {
            "INF (unused!)".to_string()
        } else {
            format!("{:.1}", target.hurrs.unwrap_or(0.0))
        };

        writeln!(
            writer,
            "{}",
            format!(
                "--- #{rank} (hURRS: {hurrs_display}) {}",
                "-".repeat(50)
            )
            .yellow()
            .bold()
        )?;

        writeln!(
            writer,
            "{}  {} v{}",
            "Upstream Crate:".bold(),
            target.intermediate.name.cyan(),
            target.intermediate.version
        )?;
        writeln!(
            writer,
            "{}  {} (brings in {} transitive crates)",
            "Offending Dep:".bold(),
            target.fat_dependency.name.red(),
            target.w_transitive
        )?;
        writeln!(
            writer,
            "{} {} across {} file(s)",
            "References Found:".bold(),
            target.c_ref,
            target.scan_result.files_with_matches
        )?;
        writeln!(writer)?;

        if !target.scan_result.file_matches.is_empty() {
            writeln!(writer, "{}", "  Files:".bold())?;
            let mut current_file = String::new();
            for m in &target.scan_result.file_matches {
                let display = display_path(&m.path);
                if display != current_file {
                    writeln!(writer, "    {}", display.dimmed())?;
                    current_file = display;
                }
                writeln!(
                    writer,
                    "      L{}: {}",
                    m.line_number,
                    m.line_content.bright_white()
                )?;
            }
            writeln!(writer)?;
        }

        writeln!(
            writer,
            "  {} {}",
            "Suggested Action:".bold(),
            format!("{}", target.suggestion).green().bold()
        )?;
        render_action_detail(writer, target)?;
        writeln!(writer)?;
    }

    // Summary table.
    writeln!(writer, "{}", "=== Summary ===".bold())?;
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_header(vec![
            "#",
            "Upstream Crate",
            "Fat Dep",
            "W_trans",
            "C_ref",
            "hURRS",
            "Action",
        ]);

    for (i, target) in report.targets.iter().enumerate() {
        let hurrs_display = if target.hurrs.is_none() {
            "INF".to_string()
        } else {
            format!("{:.1}", target.hurrs.unwrap_or(0.0))
        };
        table.add_row(vec![
            format!("{}", i + 1),
            format!("{} v{}", target.intermediate.name, target.intermediate.version),
            format!("{} v{}", target.fat_dependency.name, target.fat_dependency.version),
            format!("{}", target.w_transitive),
            format!("{}", target.c_ref),
            hurrs_display,
            format!("{}", target.suggestion),
        ]);
    }

    writeln!(writer, "{table}")?;
    writeln!(writer)?;

    Ok(())
}

fn render_action_detail(writer: &mut dyn Write, target: &UpstreamTarget) -> anyhow::Result<()> {
    match &target.suggestion {
        RemovalStrategy::Remove => {
            writeln!(
                writer,
                "    The dependency `{}` appears to be unused in `{}`'s source code.",
                target.fat_dependency.name, target.intermediate.name
            )?;
            writeln!(
                writer,
                "    It may be safe to remove it from {}'s Cargo.toml entirely.",
                target.intermediate.name
            )?;
            writeln!(
                writer,
                "    This would drop {} transitive crates from builds.",
                target.w_transitive
            )?;
        }
        RemovalStrategy::FeatureGate => {
            writeln!(
                writer,
                "    Put `{}` behind a feature flag in `{}`'s Cargo.toml:",
                target.fat_dependency.name, target.intermediate.name
            )?;
            writeln!(writer, "      [features]")?;
            writeln!(
                writer,
                "      {name} = [\"dep:{name}\"]",
                name = target.fat_dependency.name.replace('-', "-")
            )?;
            writeln!(writer, "      [dependencies]")?;
            writeln!(
                writer,
                "      {} = {{ version = \"{}\", optional = true }}",
                target.fat_dependency.name, target.fat_dependency.version
            )?;
            writeln!(writer)?;
            writeln!(
                writer,
                "    This would drop {} transitive crates for users who don't need",
                target.w_transitive
            )?;
            writeln!(
                writer,
                "    the `{}` functionality.",
                target.fat_dependency.name
            )?;
        }
        RemovalStrategy::ReplaceWithStd { suggestion } => {
            writeln!(
                writer,
                "    Replace `{}` with: {}",
                target.fat_dependency.name, suggestion
            )?;
            writeln!(
                writer,
                "    This would drop {} transitive crates.",
                target.w_transitive
            )?;
        }
        RemovalStrategy::ReplaceWithLighter { alternative } => {
            writeln!(
                writer,
                "    Replace `{}` with the lighter alternative: `{}`",
                target.fat_dependency.name, alternative
            )?;
            writeln!(
                writer,
                "    This would drop {} transitive crates.",
                target.w_transitive
            )?;
        }
    }
    Ok(())
}
