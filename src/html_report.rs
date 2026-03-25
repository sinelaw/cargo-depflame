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
.action-summary li {{ font-size: 13px; padding: 3px 0; }}
.cargo-diff {{
  background: #1e1e1e; border-radius: 4px; padding: 8px 12px;
  margin: 6px 0 10px 20px; font-family: "Consolas", "Fira Code", monospace;
  font-size: 12px; line-height: 1.6; overflow-x: auto;
  display: none;
}}
.show-diff-btn {{
  display: inline-block; font-size: 11px; color: #0066cc;
  border: 1px solid #0066cc; border-radius: 3px; padding: 1px 6px;
  margin-left: 6px; cursor: pointer; vertical-align: middle;
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
}}
.cargo-diff .diff-file {{ color: #888; }}
.cargo-diff .diff-rm {{ color: #f44; }}
.cargo-diff .diff-add {{ color: #4c4; }}
.cargo-diff .diff-comment {{ color: #888; font-style: italic; }}
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
function toggleDiff(li) {{
  var diff = li.querySelector('.cargo-diff');
  var btn = li.querySelector('.show-diff-btn');
  if (diff) {{
    var show = diff.style.display !== 'block';
    diff.style.display = show ? 'block' : 'none';
    if (btn) btn.textContent = show ? 'hide diff' : 'show diff';
  }}
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

    // --- Categorize targets into sections ---
    enum Category {
        RemoveUnused,
        ChangeFeatures,
        MakeOptional,
        Upstream,
        NotActionable,
    }

    fn categorize(t: &crate::metrics::UpstreamTarget) -> Category {
        match &t.suggestion {
            RemovalStrategy::Remove if t.intermediate_is_workspace_member => Category::RemoveUnused,
            RemovalStrategy::AlreadyGated { .. } => Category::ChangeFeatures,
            _ if !t.intermediate_is_workspace_member => Category::Upstream,
            RemovalStrategy::RequiredBySibling { .. } => Category::NotActionable,
            _ => Category::MakeOptional,
        }
    }

    struct Section {
        title: &'static str,
        description: &'static str,
        targets: Vec<(usize, &'static str)>, // (original index, upstream_badge)
    }

    let mut sections = vec![
        Section {
            title: "Remove unused dependencies",
            description: "These dependencies are in your <code>Cargo.toml</code> but no references were found in your source code. \
                          You can remove them by deleting the line from <code>[dependencies]</code>. \
                          If a dependency has 0 deps saved, it's also pulled in transitively by something else, \
                          so removing it only cleans up your manifest without shrinking the build.",
            targets: Vec::new(),
        },
        Section {
            title: "Disable unnecessary features",
            description: "These dependencies are already <em>optional</em> in their upstream crate, \
                          but are being pulled in by a feature you have enabled (often the <code>default</code> features). \
                          You can reduce your dependency count by changing the feature flags in your <code>Cargo.toml</code> &mdash; \
                          for example, adding <code>default-features = false</code> and listing only the features you actually need.",
            targets: Vec::new(),
        },
        Section {
            title: "Make dependencies optional",
            description: "These dependencies are always compiled but could be made optional by adding <code>optional = true</code> \
                          in your <code>Cargo.toml</code> and defining a feature flag in <code>[features]</code>. \
                          This lets downstream users (or your own binary crate) opt out of them when they're not needed.",
            targets: Vec::new(),
        },
        Section {
            title: "Proposals for upstream libraries",
            description: "These changes would need to happen in an external library's repository, not your own code. \
                          You would need to open an issue or submit a pull request to the upstream maintainer. \
                          The impact listed is the savings <em>you</em> would see if the change were accepted.",
            targets: Vec::new(),
        },
        Section {
            title: "Not actionable",
            description: "These dependencies can't be easily removed because they're required by sibling dependencies.",
            targets: Vec::new(),
        },
    ];

    for (i, t) in report.targets.iter().enumerate() {
        let section_idx = match categorize(t) {
            Category::RemoveUnused => 0,
            Category::ChangeFeatures => 1,
            Category::MakeOptional => 2,
            Category::Upstream => 3,
            Category::NotActionable => 4,
        };
        let is_local_action = t.intermediate_is_workspace_member
            || matches!(t.suggestion, RemovalStrategy::AlreadyGated { .. } | RemovalStrategy::RequiredBySibling { .. });
        let badge = if is_local_action { "" } else {
            " <span title=\"This change would be a PR to an upstream library, not your own code\" style=\"cursor:help\">\u{1f4e4}</span>"
        };
        sections[section_idx].targets.push((i, badge));
    }

    // Render each non-empty section.
    let mut any_rendered = false;
    for section in &sections {
        if section.targets.is_empty() {
            continue;
        }
        any_rendered = true;
        html.push_str(&format!(
            "<div class=\"action-summary\">\n\
             <h3>{}</h3>\n\
             <p style=\"font-size:13px;color:#666;margin-bottom:8px\">{}</p>\n<ul>\n",
            section.title, section.description,
        ));

        for &(i, upstream_badge) in &section.targets {
            let t = &report.targets[i];
            let prefix = format!("(-{} deps)", t.w_unique);
            let fat_link = crate_link(&t.fat_dependency.name);
            let int_link = crate_link(&t.intermediate.name);
            let action_line = format_action_line(t, &prefix, &fat_link, &int_link);
            let is_local = t.intermediate_is_workspace_member
                || matches!(t.suggestion, RemovalStrategy::AlreadyGated { .. } | RemovalStrategy::RequiredBySibling { .. });
            let diff_block = if is_local { build_cargo_diff(t) } else { String::new() };

            if diff_block.is_empty() {
                html.push_str(&format!(
                    "<li>{action_line}{upstream_badge}</li>\n",
                ));
            } else {
                html.push_str(&format!(
                    "<li>{action_line}{upstream_badge} \
                     <span class=\"show-diff-btn\" onclick=\"toggleDiff(this.parentElement)\">show diff</span>\n{diff_block}</li>\n",
                ));
            }
        }
        html.push_str("</ul>\n</div>\n\n");
    }
    if !any_rendered {
        html.push_str("<div class=\"action-summary\"><p style=\"color:#888\">No actionable suggestions found.</p></div>\n\n");
    }

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

        let is_local = t.intermediate_is_workspace_member
            || matches!(t.suggestion, RemovalStrategy::AlreadyGated { .. } | RemovalStrategy::RequiredBySibling { .. });
        let upstream_indicator = if is_local {
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

/// Format a single suggestion action line.
fn format_action_line(
    t: &crate::metrics::UpstreamTarget,
    prefix: &str,
    fat_link: &str,
    int_link: &str,
) -> String {
    match &t.suggestion {
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
        RemovalStrategy::AlreadyGated {
            detail,
            enabling_features,
            recommended_defaults,
        } => {
            let feat_hint = if !enabling_features.is_empty() {
                let feats = enabling_features
                    .iter()
                    .map(|f| format!("<code>{}</code>", html_escape(f)))
                    .collect::<Vec<_>>()
                    .join(", ");
                if recommended_defaults.is_some() {
                    format!(" &mdash; default feature(s) {feats} pull it in; disable defaults and keep only what you need")
                } else {
                    format!(" &mdash; enabled by feature(s) {feats}")
                }
            } else {
                String::new()
            };
            format!(
                "{prefix} {fat_link} is already optional in {int_link} ({detail}){feat_hint}"
            )
        }
        RemovalStrategy::FeatureGate => {
            if t.intermediate_is_workspace_member {
                format!(
                    "{prefix} Make {fat_link} optional in {int_link} \
                     &mdash; put it behind a Cargo feature flag"
                )
            } else {
                format!(
                    "{prefix} Propose making {fat_link} optional in {int_link} \
                     &mdash; submit a PR to put it behind a feature flag"
                )
            }
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
                 it's also required by sibling dep {}",
                crate_link(sibling)
            )
        }
    }
}

/// Build a colored diff block showing the Cargo.toml change for a suggestion.
fn build_cargo_diff(t: &crate::metrics::UpstreamTarget) -> String {
    let fat = html_escape(&t.fat_dependency.name);
    let fat_ver = html_escape(&t.fat_dependency.version);
    let int = html_escape(&t.intermediate.name);
    let toml_path = if t.intermediate_is_workspace_member {
        "Cargo.toml".to_string()
    } else {
        format!("{int}/Cargo.toml")
    };

    let mut d = String::from("<div class=\"cargo-diff\">");

    macro_rules! line {
        ($cls:expr, $($arg:tt)*) => {
            d.push_str(&format!("<div class=\"{}\">{}</div>", $cls, format!($($arg)*)));
        };
    }

    match &t.suggestion {
        RemovalStrategy::Remove => {
            line!("diff-file", "# {toml_path}");
            line!("diff-rm", "- {fat} = \"{fat_ver}\"");
        }
        RemovalStrategy::FeatureGate => {
            let feat_name = format!("use-{fat}");
            line!("diff-file", "# {toml_path} — [dependencies]");
            line!("diff-rm", "- {fat} = \"{fat_ver}\"");
            line!("diff-add", "+ {fat} = {{ version = \"{fat_ver}\", optional = true }}");
            line!("diff-comment", "");
            line!("diff-comment", "# add a feature flag so users can opt in to this dependency:");
            line!("diff-file", "# {toml_path} — [features]");
            line!("diff-add", "+ {feat_name} = [\"dep:{fat}\"]  # pick a name that makes sense for your crate");
        }
        RemovalStrategy::AlreadyGated { enabling_features, recommended_defaults, .. } => {
            line!("diff-file", "# Cargo.toml");
            if enabling_features.is_empty() {
                line!("diff-comment", "# check your [{int}] dependency — a feature is pulling in {fat}");
                line!("diff-rm", "- {int} = {{ version = \"...\", features = [\"...\"] }}");
                line!("diff-add", "+ {int} = {{ version = \"...\" }}  # try removing features that pull in {fat}");
            } else if let Some(keep) = recommended_defaults {
                // The enabling feature is part of "default" — suggest disabling defaults.
                let bad_feats = enabling_features.iter().map(|f| html_escape(f)).collect::<Vec<_>>();
                let bad_str = bad_feats.iter().map(|f| format!("\"{f}\"")).collect::<Vec<_>>().join(", ");
                line!("diff-comment", "# default feature(s) {bad_str} of {int} pull in {fat}");
                line!("diff-rm", "- {int} = \"...\"");
                if keep.is_empty() {
                    line!("diff-add", "+ {int} = {{ version = \"...\", default-features = false }}");
                } else {
                    let keep_str = keep.iter().map(|f| format!("\"{}\"", html_escape(f))).collect::<Vec<_>>().join(", ");
                    line!("diff-add", "+ {int} = {{ version = \"...\", default-features = false, features = [{keep_str}] }}");
                }
            } else {
                let feats = enabling_features.iter().map(|f| html_escape(f)).collect::<Vec<_>>();
                let feats_str = feats.iter().map(|f| format!("\"{f}\"")).collect::<Vec<_>>().join(", ");
                line!("diff-comment", "# feature(s) {feats_str} of {int} pull in {fat}");
                line!("diff-rm", "- {int} = {{ version = \"...\", features = [{feats_str}] }}");
                line!("diff-add", "+ {int} = {{ version = \"...\" }}  # without {feats_str}");
            }
        }
        RemovalStrategy::ReplaceWithStd { suggestion } => {
            let sug = html_escape(suggestion);
            line!("diff-file", "# {toml_path}");
            line!("diff-rm", "- {fat} = \"{fat_ver}\"");
            line!("diff-comment", "# replace usage with {sug}");
        }
        RemovalStrategy::ReplaceWithLighter { alternative } => {
            let alt = html_escape(alternative);
            line!("diff-file", "# {toml_path}");
            line!("diff-rm", "- {fat} = \"{fat_ver}\"");
            line!("diff-add", "+ {alt} = \"...\"");
        }
        RemovalStrategy::InlineUpstream { .. } => {
            let n = t.scan_result.distinct_items.len();
            line!("diff-file", "# {toml_path}");
            line!("diff-rm", "- {fat} = \"{fat_ver}\"");
            line!("diff-comment", "# copy the {n} item(s) you use directly into your code");
        }
        RemovalStrategy::RequiredBySibling { .. } => {
            return String::new();
        }
    }

    d.push_str("</div>");
    d
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
