#[test]
fn js_tests_pass() {
    let status = std::process::Command::new("node")
        .arg("tests/js/run_tests.js")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()
        .expect("node must be installed to run JS tests");
    assert!(status.success(), "JS tests failed (see output above)");
}
