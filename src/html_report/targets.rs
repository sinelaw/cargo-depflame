use crate::metrics::RemovalStrategy;
use crate::report::AnalysisReport;

use super::{crate_link, html_escape};

/// Build the targets tab HTML: action summary + ranked table with expandable rows.
pub(super) fn build_targets_html(report: &AnalysisReport) -> String {
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

    // --- Categorize and render action sections ---
    let sections = categorize_targets(report);
    render_action_sections(&mut html, report, &sections);

    // --- Ranked targets table ---
    render_detail_table(&mut html, report);

    html
}

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

fn categorize_targets(report: &AnalysisReport) -> Vec<Section> {
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
            || matches!(
                t.suggestion,
                RemovalStrategy::AlreadyGated { .. } | RemovalStrategy::RequiredBySibling { .. }
            );
        let badge = if is_local_action {
            ""
        } else {
            " <span title=\"This change would be a PR to an upstream library, not your own code\" style=\"cursor:help\">\u{1f4e4}</span>"
        };
        sections[section_idx].targets.push((i, badge));
    }

    sections
}

fn render_action_sections(html: &mut String, report: &AnalysisReport, sections: &[Section]) {
    let mut any_rendered = false;
    for section in sections {
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
                || matches!(
                    t.suggestion,
                    RemovalStrategy::AlreadyGated { .. }
                        | RemovalStrategy::RequiredBySibling { .. }
                );
            let diff_block = if is_local {
                build_cargo_diff(t)
            } else {
                String::new()
            };

            if diff_block.is_empty() {
                html.push_str(&format!("<li>{action_line}{upstream_badge}</li>\n",));
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
}

fn render_detail_table(html: &mut String, report: &AnalysisReport) {
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
            || matches!(
                t.suggestion,
                RemovalStrategy::AlreadyGated { .. } | RemovalStrategy::RequiredBySibling { .. }
            );
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
            format!("{prefix} {fat_link} is already optional in {int_link} ({detail}){feat_hint}")
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
            line!(
                "diff-add",
                "+ {fat} = {{ version = \"{fat_ver}\", optional = true }}"
            );
            line!("diff-comment", "");
            line!(
                "diff-comment",
                "# add a feature flag so users can opt in to this dependency:"
            );
            line!("diff-file", "# {toml_path} — [features]");
            line!(
                "diff-add",
                "+ {feat_name} = [\"dep:{fat}\"]  # pick a name that makes sense for your crate"
            );
        }
        RemovalStrategy::AlreadyGated {
            enabling_features,
            recommended_defaults,
            ..
        } => {
            line!("diff-file", "# Cargo.toml");
            if enabling_features.is_empty() {
                line!(
                    "diff-comment",
                    "# check your [{int}] dependency — a feature is pulling in {fat}"
                );
                line!(
                    "diff-rm",
                    "- {int} = {{ version = \"...\", features = [\"...\"] }}"
                );
                line!(
                    "diff-add",
                    "+ {int} = {{ version = \"...\" }}  # try removing features that pull in {fat}"
                );
            } else if let Some(keep) = recommended_defaults {
                // The enabling feature is part of "default" — suggest disabling defaults.
                let bad_feats = enabling_features
                    .iter()
                    .map(|f| html_escape(f))
                    .collect::<Vec<_>>();
                let bad_str = bad_feats
                    .iter()
                    .map(|f| format!("\"{f}\""))
                    .collect::<Vec<_>>()
                    .join(", ");
                line!(
                    "diff-comment",
                    "# default feature(s) {bad_str} of {int} pull in {fat}"
                );
                line!("diff-rm", "- {int} = \"...\"");
                if keep.is_empty() {
                    line!(
                        "diff-add",
                        "+ {int} = {{ version = \"...\", default-features = false }}"
                    );
                } else {
                    let keep_str = keep
                        .iter()
                        .map(|f| format!("\"{}\"", html_escape(f)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    line!("diff-add", "+ {int} = {{ version = \"...\", default-features = false, features = [{keep_str}] }}");
                }
            } else {
                let feats = enabling_features
                    .iter()
                    .map(|f| html_escape(f))
                    .collect::<Vec<_>>();
                let feats_str = feats
                    .iter()
                    .map(|f| format!("\"{f}\""))
                    .collect::<Vec<_>>()
                    .join(", ");
                line!(
                    "diff-comment",
                    "# feature(s) {feats_str} of {int} pull in {fat}"
                );
                line!(
                    "diff-rm",
                    "- {int} = {{ version = \"...\", features = [{feats_str}] }}"
                );
                line!(
                    "diff-add",
                    "+ {int} = {{ version = \"...\" }}  # without {feats_str}"
                );
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
            line!(
                "diff-comment",
                "# copy the {n} item(s) you use directly into your code"
            );
        }
        RemovalStrategy::RequiredBySibling { .. } => {
            return String::new();
        }
    }

    d.push_str("</div>");
    d
}
