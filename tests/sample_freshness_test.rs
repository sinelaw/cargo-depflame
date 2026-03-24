#[test]
fn sample_json_is_up_to_date() {
    let report = cargo_depflame::sample::generate_sample_report();
    let generated = serde_json::to_string_pretty(&report).unwrap() + "\n";
    let fixture_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample_report.json");
    let committed = std::fs::read_to_string(&fixture_path).unwrap_or_else(|e| {
        panic!(
            "Could not read {}: {e}. Run `cargo run --example gen_sample` to create it.",
            fixture_path.display()
        )
    });
    assert_eq!(
        generated, committed,
        "Committed sample_report.json is stale. Regenerate with: cargo run --example gen_sample"
    );
}
