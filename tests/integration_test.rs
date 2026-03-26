use cargo_depflame::analyze;
use cargo_depflame::cli::{AnalyzeArgs, CommonArgs, OutputFormat};
use cargo_depflame::metrics::{Confidence, RemovalStrategy};
use std::path::PathBuf;

fn fixture_workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-workspace/Cargo.toml")
}

fn default_args() -> AnalyzeArgs {
    AnalyzeArgs {
        common: CommonArgs {
            manifest_path: fixture_workspace(),
            threshold: 0.0, // include everything
            top: 100,
            heavy_threshold: 1, // low threshold so we get results
            include_noise: true,
            ..Default::default()
        },
        format: OutputFormat::Text,
        output: None,
    }
}

/// The full pipeline runs without error on a real workspace.
/// This exercises cargo metadata, cargo tree, graph building,
/// source scanning, ranking, and report assembly.
#[test]
fn full_pipeline_runs_without_error() {
    let report = analyze::run_analyze(&default_args()).expect("analysis should succeed");

    assert!(report.total_dependencies > 0, "should find dependencies");
    assert!(!report.workspace_root.is_empty());
    assert!(!report.tool_version.is_empty());
}

/// Platform dep resolution (cargo tree --format) works and detects real deps.
#[test]
fn platform_deps_are_resolved() {
    let report = analyze::run_analyze(&default_args()).expect("analysis should succeed");

    assert!(
        report.platform_dependencies.is_some(),
        "platform deps should be resolved (cargo tree must work)"
    );
    let platform_deps = report.platform_dependencies.unwrap();
    assert!(platform_deps > 0, "should have platform deps");
}

/// once_cell is declared as a dep of crate-a but never used in source.
/// The pipeline should detect it as unused.
#[test]
fn detects_unused_dependency() {
    let report = analyze::run_analyze(&default_args()).expect("analysis should succeed");

    let once_cell_target = report
        .targets
        .iter()
        .find(|t| t.heavy_dependency.name == "once_cell" && t.intermediate.name == "crate-a");

    assert!(
        once_cell_target.is_some(),
        "once_cell should appear as a target (it's unused in crate-a). targets: {:?}",
        report
            .targets
            .iter()
            .map(|t| format!(
                "{} -> {} (c_ref={})",
                t.intermediate.name, t.heavy_dependency.name, t.c_ref
            ))
            .collect::<Vec<_>>()
    );

    let target = once_cell_target.unwrap();
    assert_eq!(target.c_ref, 0, "once_cell should have 0 code references");
    assert!(
        matches!(
            target.suggestion,
            RemovalStrategy::Remove | RemovalStrategy::ReplaceWithStd { .. }
        ),
        "once_cell should suggest Remove or ReplaceWithStd, got: {:?}",
        target.suggestion
    );
}

/// regex is used in crate-a, so it should not be flagged as unused.
#[test]
fn does_not_flag_used_dependency() {
    let report = analyze::run_analyze(&default_args()).expect("analysis should succeed");

    let regex_unused = report
        .unused_direct_deps
        .iter()
        .find(|d| d.dep_name == "regex" && d.from_crate == "crate-a");

    assert!(
        regex_unused.is_none(),
        "regex is used in crate-a and should not appear in unused_direct_deps"
    );
}

/// The dep tree for the flamegraph is populated.
#[test]
fn dep_tree_is_populated() {
    let report = analyze::run_analyze(&default_args()).expect("analysis should succeed");

    let dep_tree = report
        .dep_tree
        .as_ref()
        .expect("dep_tree should be present");
    assert!(!dep_tree.nodes.is_empty(), "dep tree should have nodes");
    assert!(
        !dep_tree.root_indices.is_empty(),
        "dep tree should have roots"
    );

    // Workspace members should be roots.
    let root_names: Vec<&str> = dep_tree
        .root_indices
        .iter()
        .map(|&i| dep_tree.nodes[i].name.as_str())
        .collect();
    assert!(root_names.contains(&"crate-a"), "crate-a should be a root");
    assert!(root_names.contains(&"crate-b"), "crate-b should be a root");
}

/// Reports can be rendered in all formats without error.
#[test]
fn report_renders_all_formats() {
    let report = analyze::run_analyze(&default_args()).expect("analysis should succeed");

    let mut buf = Vec::new();
    cargo_depflame::report::render_text(&report, &mut buf, false)
        .expect("text render should succeed");
    assert!(!buf.is_empty());

    let mut buf = Vec::new();
    cargo_depflame::report::render_text(&report, &mut buf, true)
        .expect("verbose text render should succeed");
    assert!(!buf.is_empty());

    let mut buf = Vec::new();
    cargo_depflame::report::render_json(&report, &mut buf).expect("JSON render should succeed");
    let json: serde_json::Value =
        serde_json::from_slice(&buf).expect("JSON output should be valid");
    assert!(json.is_object());

    let mut buf = Vec::new();
    cargo_depflame::html_report::render_html_report(&report, &mut buf)
        .expect("HTML render should succeed");
    let html = String::from_utf8(buf).expect("HTML should be valid UTF-8");
    assert!(html.contains("<html"));
}

/// itoa is declared as a dep of crate-a but never used, and is listed in
/// [package.metadata.cargo-machete] ignored. It should not appear as unused.
#[test]
fn machete_ignored_deps_are_skipped() {
    let report = analyze::run_analyze(&default_args()).expect("analysis should succeed");

    let itoa_unused = report
        .unused_direct_deps
        .iter()
        .find(|d| d.dep_name == "itoa" && d.from_crate == "crate-a");

    assert!(
        itoa_unused.is_none(),
        "itoa is in cargo-machete ignored list and should not appear in unused_direct_deps"
    );

    let itoa_target = report
        .targets
        .iter()
        .find(|t| t.heavy_dependency.name == "itoa" && t.intermediate.name == "crate-a");

    assert!(
        itoa_target.is_none(),
        "itoa should not appear as a target at all when ignored"
    );
}

/// Noise filtering works: with include_noise=false, noise targets are excluded.
#[test]
fn noise_filtering_works_end_to_end() {
    let mut args_no_noise = default_args();
    args_no_noise.common.include_noise = false;
    let report_filtered = analyze::run_analyze(&args_no_noise).expect("analysis should succeed");

    let mut args_with_noise = default_args();
    args_with_noise.common.include_noise = true;
    let report_all = analyze::run_analyze(&args_with_noise).expect("analysis should succeed");

    let noise_in_filtered = report_filtered
        .targets
        .iter()
        .filter(|t| t.confidence == Confidence::Noise)
        .count();
    assert_eq!(
        noise_in_filtered, 0,
        "filtered report should have no Noise targets"
    );

    // The unfiltered report should have >= as many targets.
    assert!(
        report_all.targets.len() >= report_filtered.targets.len(),
        "unfiltered report should have >= targets"
    );
}
