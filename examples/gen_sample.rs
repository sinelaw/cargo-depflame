//! Regenerate the committed sample report fixture.
//!
//! Run with: `cargo run --example gen_sample`

fn main() {
    let report = cargo_depflame::sample::generate_sample_report();
    let json = serde_json::to_string_pretty(&report).unwrap();
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample_report.json");
    std::fs::write(&path, format!("{json}\n")).unwrap();
    eprintln!("Wrote {}", path.display());
}
