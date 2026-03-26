mod style;
mod targets;

use crate::flamegraph;
use crate::report::AnalysisReport;
use std::io::Write;

/// Render a self-contained HTML report with three tabs:
///   1. Flamegraph (interactive SVG)
///   2. Targets table (basic summary + expandable verbose detail)
///   3. Raw JSON
pub fn render_html_report(report: &AnalysisReport, writer: &mut dyn Write) -> anyhow::Result<()> {
    // Generate SVG into a buffer.
    let svg_content = if let Some(tree) = &report.dep_tree {
        let mut buf = Vec::new();
        flamegraph::render_flamegraph_with_unused(
            tree,
            report.total_dependencies,
            &report.unused_edges,
            &mut buf,
        )?;
        String::from_utf8(buf)?
    } else {
        String::from("<p>No dependency tree data available.</p>")
    };

    // Generate JSON.
    let json_raw = serde_json::to_string_pretty(report)?;
    let json_escaped = html_escape(&json_raw);

    // Build targets table rows.
    let targets_html = targets::build_targets_html(report);

    // Platform deps display.
    let platform_info = match report.platform_dependencies {
        Some(p) => format!("{p} platform"),
        None => String::new(),
    };

    let css = style::css();
    let js = style::js();

    write!(
        writer,
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>depflame — Dependency Analysis Report</title>
<style>
{css}
</style>
</head>
<body>

<div class="header">
  <h1>depflame — Dependency Analysis Report</h1>
  <div class="stats">
    <span title="Total number of crate dependencies in the full cross-platform resolve graph (includes all targets/platforms).">{total_deps} total deps</span>
    {platform_html}
    <span title="Dependencies that appear in metadata but are not compiled on your current platform (e.g. windows-only crates on linux). These are typically not actionable.">{phantom} phantom deps</span>
    <span title="Crates with a high transitive dependency count (above the --heavy-threshold). These are the heavy hitters that the tool analyzes for removal opportunities.">{heavy_nodes} heavy crates analyzed</span>
    <span title="Number of upstream edges identified as potential optimization targets, ranked by impact.">{n_targets} targets found</span>
    <span style="color:#aaa">v{version} &middot; {timestamp}</span>
  </div>
</div>

<div class="tabs">
  <button class="tab-btn active" onclick="showTab('flamegraph')">Flamegraph</button>
  <button class="tab-btn" onclick="showTab('targets')">Suggestions ({n_targets})</button>
  <button class="tab-btn" onclick="showTab('json')">Raw JSON</button>
</div>

<div id="tab-flamegraph" class="tab-content active">
{svg_content}
</div>

<div id="tab-targets" class="tab-content">
{targets_html}
</div>

<div id="tab-json" class="tab-content">
<div class="json-container">
  <button class="copy-btn" onclick="copyJson()">Copy</button>
  <pre><code>{json_escaped}</code></pre>
</div>
</div>

<script>
{js}
</script>

</body>
</html>
"##,
        total_deps = report.total_dependencies,
        platform_html = if platform_info.is_empty() {
            String::new()
        } else {
            format!("<span title=\"Dependencies actually compiled for your current platform/target. This is the number that matters for your build times.\">{platform_info} deps</span>")
        },
        phantom = report.phantom_dependencies,
        heavy_nodes = report.heavy_nodes_found,
        n_targets = report.targets.len(),
        version = html_escape(&report.tool_version),
        timestamp = html_escape(&report.timestamp),
        svg_content = svg_content,
        targets_html = targets_html,
        json_escaped = json_escaped,
    )?;

    Ok(())
}

pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Render a crate name as a link to crates.io.
pub(crate) fn crate_link(name: &str) -> String {
    let escaped = html_escape(name);
    format!(
        "<a href=\"https://crates.io/crates/{escaped}\" target=\"_blank\" \
         style=\"color:inherit;text-decoration:underline dotted\">{escaped}</a>"
    )
}
