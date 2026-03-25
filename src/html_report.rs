use crate::flamegraph;
use crate::metrics::RemovalStrategy;
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
        flamegraph::render_flamegraph(tree, report.total_dependencies, &mut buf)?;
        String::from_utf8(buf)?
    } else {
        String::from("<p>No dependency tree data available.</p>")
    };

    // Generate JSON.
    let json_raw = serde_json::to_string_pretty(report)?;
    let json_escaped = html_escape(&json_raw);

    // Build targets table rows.
    let targets_html = build_targets_html(report);

    // Platform deps display.
    let platform_info = match report.platform_dependencies {
        Some(p) => format!("{p} platform"),
        None => String::new(),
    };

    write!(
        writer,
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Upstream Dependency Triage Report</title>
<style>
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", sans-serif;
  background: #f5f5f5; color: #333; line-height: 1.5;
}}
.header {{
  background: #fff; border-bottom: 1px solid #ddd; padding: 16px 24px;
}}
.header h1 {{ font-size: 20px; margin-bottom: 4px; }}
.header .stats {{ font-size: 13px; color: #666; }}
.header .stats span {{ margin-right: 16px; }}
.header .stats span[title] {{ cursor: help; border-bottom: 1px dashed #aaa; }}
.tabs {{
  display: flex; background: #fff; border-bottom: 2px solid #ddd;
  padding: 0 24px; gap: 0;
}}
.tab-btn {{
  padding: 10px 20px; border: none; background: none; cursor: pointer;
  font-size: 14px; font-weight: 500; color: #666;
  border-bottom: 2px solid transparent; margin-bottom: -2px;
  transition: color 0.15s, border-color 0.15s;
}}
.tab-btn:hover {{ color: #333; }}
.tab-btn.active {{ color: #0066cc; border-bottom-color: #0066cc; }}
.tab-content {{ display: none; }}
.tab-content.active {{ display: block; }}

/* Flamegraph tab */
#tab-flamegraph {{ background: #fff; }}
#tab-flamegraph svg {{ display: block; width: 100%; height: auto; }}

/* Targets tab */
#tab-targets {{ padding: 24px; }}
.action-summary {{
  background: #fff; border: 1px solid #ddd; border-radius: 6px;
  padding: 16px 20px; margin-bottom: 20px;
}}
.action-summary h3 {{ font-size: 14px; margin-bottom: 8px; color: #444; }}
.action-summary ul {{ list-style: none; padding: 0; }}
.action-summary li {{ font-size: 13px; padding: 3px 0; font-family: "Consolas", monospace; }}
.targets-table {{
  width: 100%; border-collapse: collapse; background: #fff;
  border: 1px solid #ddd; border-radius: 6px; overflow: hidden;
  font-size: 13px;
}}
.targets-table th {{
  background: #f8f8f8; text-align: left; padding: 10px 12px;
  border-bottom: 2px solid #ddd; font-weight: 600; font-size: 12px;
  text-transform: uppercase; color: #555; white-space: nowrap;
}}
.targets-table th[title] {{
  cursor: help; border-bottom: 1px dashed #999;
}}
.targets-table td {{
  padding: 8px 12px; border-bottom: 1px solid #eee;
  vertical-align: top;
}}
.targets-table tr:hover {{ background: #f9f9f9; }}
.targets-table tr.expandable {{ cursor: pointer; }}
.detail-row {{ display: none; }}
.detail-row.open {{ display: table-row; }}
.detail-row td {{
  background: #fafafa; padding: 12px 20px;
  border-bottom: 1px solid #ddd;
}}
.detail-box {{
  font-family: "Consolas", monospace; font-size: 12px; line-height: 1.6;
}}
.detail-box .label {{ color: #888; }}
.badge {{
  display: inline-block; padding: 1px 6px; border-radius: 3px;
  font-size: 11px; font-weight: 600;
}}
.badge-high {{ background: #e8f5e9; color: #2e7d32; }}
.badge-medium {{ background: #fff3e0; color: #e65100; }}
.badge-low {{ background: #fce4ec; color: #c62828; }}
.badge-noise {{ background: #f3e5f5; color: #6a1b9a; }}
.badge-flag {{
  background: #e3f2fd; color: #1565c0; margin-right: 4px;
}}
.ref-file {{ color: #0066cc; }}
.ref-line {{ color: #888; margin-left: 16px; }}

/* JSON tab */
#tab-json {{ padding: 24px; }}
.json-container {{
  position: relative; background: #1e1e1e; border-radius: 6px;
  overflow: hidden;
}}
.json-container pre {{
  padding: 20px; overflow-x: auto; color: #d4d4d4;
  font-family: "Consolas", "Fira Code", monospace; font-size: 12px;
  line-height: 1.5; margin: 0;
}}
.copy-btn {{
  position: absolute; top: 8px; right: 8px; padding: 6px 14px;
  background: #333; color: #ccc; border: 1px solid #555;
  border-radius: 4px; cursor: pointer; font-size: 12px;
}}
.copy-btn:hover {{ background: #444; }}
</style>
</head>
<body>

<div class="header">
  <h1>Upstream Dependency Triage Report</h1>
  <div class="stats">
    <span title="Total number of crate dependencies in the full cross-platform resolve graph (includes all targets/platforms).">{total_deps} total deps</span>
    {platform_html}
    <span title="Dependencies that appear in metadata but are not compiled on your current platform (e.g. windows-only crates on linux). These are typically not actionable.">{phantom} phantom deps</span>
    <span title="Crates with a high transitive dependency count (above the --fat-threshold). These are the heavy hitters that the tool analyzes for removal opportunities.">{fat_nodes} heavy crates analyzed</span>
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
function showTab(name) {{
  document.querySelectorAll('.tab-content').forEach(function(el) {{
    el.classList.remove('active');
  }});
  document.querySelectorAll('.tab-btn').forEach(function(btn) {{
    btn.classList.remove('active');
  }});
  document.getElementById('tab-' + name).classList.add('active');
  // Find the button whose onclick contains the tab name.
  document.querySelectorAll('.tab-btn').forEach(function(btn) {{
    if (btn.getAttribute('onclick').indexOf(name) !== -1) {{
      btn.classList.add('active');
    }}
  }});
}}
function toggleDetail(n) {{
  var row = document.getElementById('detail-' + n);
  if (row) row.classList.toggle('open');
}}
function copyJson() {{
  var text = document.querySelector('#tab-json pre code').textContent;
  navigator.clipboard.writeText(text).then(function() {{
    var btn = document.querySelector('.copy-btn');
    btn.textContent = 'Copied!';
    setTimeout(function() {{ btn.textContent = 'Copy'; }}, 1500);
  }});
}}
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
        fat_nodes = report.fat_nodes_found,
        n_targets = report.targets.len(),
        version = html_escape(&report.tool_version),
        timestamp = html_escape(&report.timestamp),
        svg_content = svg_content,
        targets_html = targets_html,
        json_escaped = json_escaped,
    )?;

    Ok(())
}

/// Build the targets tab HTML: action summary + ranked table with expandable rows.
fn build_targets_html(report: &AnalysisReport) -> String {
    let mut html = String::new();

    // --- Disclaimer banner ---
    html.push_str(
        "<div style=\"background:#fff3cd;border:1px solid #ffc107;border-radius:6px;\
         padding:12px 16px;margin-bottom:20px;font-size:13px;color:#664d03\">\
         <strong>\u{26a0} Use your judgement.</strong> \
         These suggestions are based on automated analysis of dependency metadata and source references. \
         They may be wrong or impractical. Before acting on any suggestion, make sure you understand \
         why the dependency exists, what features it provides, and whether removing it would break \
         functionality or degrade the library for other users.\
         </div>\n",
    );

    // --- Action summary ---
    html.push_str("<div class=\"action-summary\">\n");
    html.push_str("<h3>What you can do</h3>\n<p style=\"font-size:13px;color:#666;margin-bottom:8px\">Each suggestion below can reduce your dependency count. Items are ranked by impact (most deps saved first). Some changes are in your own crates; others require <strong>contributing a PR to an upstream library</strong> (marked with \u{1f4e4}).</p>\n<ul>\n");

    let mut has_items = false;
    for (i, t) in report.targets.iter().enumerate() {
        let upstream_badge = if t.intermediate_is_workspace_member {
            ""
        } else {
            " <span title=\"This change would be a PR to an upstream library, not your own code\" style=\"cursor:help\">\u{1f4e4}</span>"
        };
        let prefix = format!("(-{} deps)", t.w_unique);
        let fat_link = crate_link(&t.fat_dependency.name);
        let int_link = crate_link(&t.intermediate.name);
        let action_line = match &t.suggestion {
            RemovalStrategy::Remove => {
                format!("{prefix} Remove {fat_link} from {int_link} &mdash; it appears unused")
            }
            RemovalStrategy::InlineUpstream {
                fat_loc,
                api_items_used,
            } => {
                format!(
                    "{prefix} Copy the code you need from {fat_link} directly into {int_link} \
                     &mdash; only {api_items_used} API items used from a {fat_loc}-line crate"
                )
            }
            RemovalStrategy::ReplaceWithStd { suggestion } => {
                format!(
                    "{prefix} Replace {fat_link} with <code>{}</code> in {int_link} \
                     &mdash; the standard library now covers this",
                    html_escape(suggestion),
                )
            }
            RemovalStrategy::AlreadyGated { detail } => {
                format!(
                    "{prefix} Check whether you actually need {fat_link} enabled in {int_link} \
                     &mdash; it's already optional ({detail})"
                )
            }
            RemovalStrategy::FeatureGate => {
                format!(
                    "{prefix} Propose making {fat_link} optional in {int_link} \
                     &mdash; put it behind a Cargo feature flag"
                )
            }
            RemovalStrategy::ReplaceWithLighter { alternative } => {
                format!(
                    "{prefix} Switch from {fat_link} to <code>{}</code> in {int_link} \
                     &mdash; a lighter alternative",
                    html_escape(alternative),
                )
            }
            RemovalStrategy::RequiredBySibling { sibling } => {
                format!(
                    "{prefix} {fat_link} can't be removed &mdash; \
                     it's also required by sibling dep {}", crate_link(sibling)
                )
            }
        };
        html.push_str(&format!("<li>#{} {action_line}{upstream_badge}</li>\n", i + 1));
        has_items = true;
    }
    if !has_items {
        html.push_str("<li style=\"color:#888\">No actionable targets found.</li>\n");
    }
    html.push_str("</ul>\n</div>\n\n");

    // --- Ranked targets table ---
    html.push_str(
        r#"<div style="margin-bottom:12px">
<p style="font-size:14px;color:#444;margin-bottom:4px"><strong>Detailed breakdown</strong></p>
<p style="font-size:13px;color:#666">Each row below is a dependency edge you could optimize. <em>Upstream Crate</em> is where the change would happen; <em>Heavy Dep</em> is the dependency to reduce. Rows marked \u{1f4e4} require contributing a PR upstream. Click any row to see exactly where the dependency is used in the source code.</p>
</div>
<table class="targets-table">
<thead><tr>
  <th>#</th>
  <th title="The crate that directly depends on the heavy dependency. This is the crate where a change (feature-gate, removal, etc.) would need to happen.">Upstream Crate</th>
  <th title="The heavy dependency being pulled in. Removing or gating this dep is the goal.">Heavy Dep</th>
  <th title="Deps Saved: how many transitive dependencies would be removed from your build if this edge were cut. Higher = more impact.">Deps Saved</th>
  <th title="Total Deps: the total number of transitive dependencies this heavy dep brings (including shared ones that may still be needed by other crates).">Total Deps</th>
  <th title="Code Refs: number of source-level references to this dependency found in the upstream crate. Fewer refs = easier to remove or inline.">Code Refs</th>
  <th title="Score: W_transitive / C_ref. Higher score = heavier dependency relative to how much it's used. Measures ROI of removal.">Score</th>
  <th title="How confident we are in this suggestion. HIGH = clear signal, NOISE = already gated or platform-specific.">Confidence</th>
  <th title="The recommended action: FEATURE GATE (make optional), REMOVE (appears unused), INLINE (small enough to copy), ALREADY GATED (check if you need it enabled), etc.">Suggested Action</th>
</tr></thead>
<tbody>
"#,
    );

    for (i, t) in report.targets.iter().enumerate() {
        let idx = i + 1;
        let conf_class = match t.confidence {
            crate::metrics::Confidence::High => "badge-high",
            crate::metrics::Confidence::Medium => "badge-medium",
            crate::metrics::Confidence::Low => "badge-low",
            crate::metrics::Confidence::Noise => "badge-noise",
        };

        let hurrs_display = match t.hurrs {
            Some(h) => format!("{h:.1}"),
            None => "\u{221e}".to_string(), // infinity
        };

        let upstream_indicator = if t.intermediate_is_workspace_member {
            ""
        } else {
            " <span title=\"Requires a PR to this upstream library\">\u{1f4e4}</span>"
        };

        // Summary row (clickable).
        html.push_str(&format!(
            r#"<tr class="expandable" onclick="toggleDetail({idx})">
  <td>{idx}</td>
  <td><code>{intermediate_link}</code>{upstream_indicator}</td>
  <td><code>{fat_link}</code></td>
  <td>{w_uniq}</td><td>{w_trans}</td><td>{c_ref}</td><td>{hurrs}</td>
  <td><span class="badge {conf_class}">{confidence}</span></td>
  <td>{action}</td>
</tr>
"#,
            intermediate_link = crate_link(&t.intermediate.name),
            fat_link = crate_link(&t.fat_dependency.name),
            w_uniq = t.w_unique,
            w_trans = t.w_transitive,
            c_ref = t.c_ref,
            hurrs = hurrs_display,
            confidence = t.confidence,
            action = html_escape(&t.suggestion.to_string()),
        ));

        // Detail row (hidden until clicked).
        let mut flags = Vec::new();
        if t.phantom {
            flags.push("PHANTOM");
        }
        if t.intermediate_is_workspace_member {
            flags.push("YOUR-CRATE");
        }
        if t.edge_meta.build_only {
            flags.push("BUILD-ONLY");
        }
        if t.edge_meta.already_optional {
            flags.push("ALREADY-OPTIONAL");
        }
        if t.edge_meta.platform_conditional {
            flags.push("PLATFORM-CONDITIONAL");
        }
        if t.has_re_export_all {
            flags.push("RE-EXPORTS-ALL");
        }

        let flags_html = if flags.is_empty() {
            String::from("<span style=\"color:#aaa\">none</span>")
        } else {
            flags
                .iter()
                .map(|f| format!("<span class=\"badge badge-flag\">{f}</span>"))
                .collect::<Vec<_>>()
                .join(" ")
        };

        let chain_html = t
            .dep_chain
            .iter()
            .map(|c| crate_link(c))
            .collect::<Vec<_>>()
            .join(" &rarr; ");

        let mut refs_html = String::new();
        for fm in &t.scan_result.file_matches {
            let display = crate::scanner::display_path(&fm.path);
            let generated = if fm.in_generated_file {
                " <span style=\"color:#999\">[generated]</span>"
            } else {
                ""
            };
            refs_html.push_str(&format!(
                "<div class=\"ref-file\">{display}:{line}{generated}</div>\n\
                 <div class=\"ref-line\"><code>{content}</code></div>\n",
                display = html_escape(&display),
                line = fm.line_number,
                content = html_escape(&fm.line_content),
            ));
        }
        if refs_html.is_empty() {
            refs_html = String::from("<span style=\"color:#aaa\">no references found</span>");
        }

        html.push_str(&format!(
            r#"<tr class="detail-row" id="detail-{idx}">
<td colspan="9"><div class="detail-box">
  <div><span class="label">Edge:</span> {int_link} v{iv} &rarr; {fat_link} v{fv}</div>
  <div><span class="label">Flags:</span> {flags_html}</div>
  <div><span class="label">Chain:</span> {chain_html}</div>
  <div style="margin-top:8px"><span class="label">References ({ref_count}):</span></div>
  <div style="margin-left:8px">{refs_html}</div>
</div></td>
</tr>
"#,
            int_link = crate_link(&t.intermediate.name),
            iv = html_escape(&t.intermediate.version),
            fat_link = crate_link(&t.fat_dependency.name),
            fv = html_escape(&t.fat_dependency.version),
            ref_count = t.scan_result.ref_count,
        ));
    }

    html.push_str("</tbody>\n</table>\n");
    html
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Render a crate name as a link to crates.io.
fn crate_link(name: &str) -> String {
    let escaped = html_escape(name);
    format!(
        "<a href=\"https://crates.io/crates/{escaped}\" target=\"_blank\" \
         style=\"color:inherit;text-decoration:underline dotted\">{escaped}</a>"
    )
}
