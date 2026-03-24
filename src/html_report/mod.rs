mod style;

use crate::report::AnalysisReport;
use std::io::Write;

const SHELL_HTML: &str = include_str!("../html/shell.html");
const FLAMEGRAPH_JS: &str = include_str!("../js/flamegraph.js");
const FEATURES_JS: &str = include_str!("../js/features.js");
const CONTENT_JS: &str = include_str!("../js/content.js");

/// Render a self-contained HTML report.
///
/// The output is a minimal HTML shell with embedded CSS, JS, and JSON data.
/// All visible content (header, tabs, tables, suggestions, flamegraph, JSON view)
/// is generated client-side by JavaScript from the serialized report JSON.
pub fn render_html_report(report: &AnalysisReport, writer: &mut dyn Write) -> anyhow::Result<()> {
    let report_json = serde_json::to_string(report)?;
    let css = style::css();
    let report_js = style::js();

    // Build the <script> blocks.
    let mut scripts = String::with_capacity(report_json.len() + 64 * 1024);

    // 1. Report data.
    scripts.push_str("<script>\nwindow.__DEPFLAME_REPORT__ = ");
    scripts.push_str(&report_json);
    scripts.push_str(";\n</script>\n");

    // 2. Flamegraph engine.
    scripts.push_str("<script>\n");
    scripts.push_str(FLAMEGRAPH_JS);
    scripts.push_str("\n</script>\n");

    // 3. Feature toggle engine.
    scripts.push_str("<script>\n");
    scripts.push_str(FEATURES_JS);
    scripts.push_str("\n</script>\n");

    // 4. Content generator (builds all HTML from JSON).
    scripts.push_str("<script>\n");
    scripts.push_str(CONTENT_JS);
    scripts.push_str("\n</script>\n");

    // 5. Report UI (tab switching, toggles, etc).
    scripts.push_str("<script>\n");
    scripts.push_str(report_js);
    scripts.push_str("\n</script>\n");

    // 6. Initialize.
    scripts.push_str("<script>\nDepflameContent.init();\n</script>\n");

    // Assemble final HTML from shell template.
    let html = SHELL_HTML
        .replace("__CSS__", css)
        .replace("__SCRIPTS__", &scripts);

    writer.write_all(html.as_bytes())?;
    Ok(())
}
