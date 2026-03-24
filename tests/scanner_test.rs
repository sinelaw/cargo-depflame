use std::io::Write;
use tempfile::TempDir;

fn write_file(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(content.as_bytes()).unwrap();
    path
}

#[test]
fn test_scan_use_statement() {
    let dir = TempDir::new().unwrap();
    let p = write_file(
        dir.path(),
        "lib.rs",
        r#"
use regex::Regex;

fn main() {
    let re = Regex::new(r"\d+").unwrap();
}
"#,
    );

    let result = cargo_depflame::scanner::scan_files(&[p], "regex");
    // `use regex::Regex` matches \bregex::, but `Regex::new` does not.
    assert_eq!(result.ref_count, 1);
    assert_eq!(result.files_with_matches, 1);
}

#[test]
fn test_scan_path_expression() {
    let dir = TempDir::new().unwrap();
    let p = write_file(
        dir.path(),
        "lib.rs",
        r#"
fn main() {
    let re = regex::Regex::new(r"\d+").unwrap();
    let caps = regex::Regex::captures(&re, "abc123");
}
"#,
    );

    let result = cargo_depflame::scanner::scan_files(&[p], "regex");
    assert_eq!(result.ref_count, 2);
}

#[test]
fn test_scan_extern_crate() {
    let dir = TempDir::new().unwrap();
    let p = write_file(
        dir.path(),
        "lib.rs",
        r#"
extern crate serde;
use serde::Serialize;
"#,
    );

    let result = cargo_depflame::scanner::scan_files(&[p], "serde");
    assert_eq!(result.ref_count, 2);
}

#[test]
fn test_scan_skips_comments() {
    let dir = TempDir::new().unwrap();
    let p = write_file(
        dir.path(),
        "lib.rs",
        r#"
// use regex::Regex;
fn main() {}
"#,
    );

    let result = cargo_depflame::scanner::scan_files(&[p], "regex");
    assert_eq!(result.ref_count, 0);
}

#[test]
fn test_scan_hyphenated_crate_name() {
    let dir = TempDir::new().unwrap();
    let p = write_file(
        dir.path(),
        "lib.rs",
        r#"
use lazy_static::lazy_static;
"#,
    );

    // Search with the hyphenated name; scanner normalizes to underscores.
    let result = cargo_depflame::scanner::scan_files(&[p], "lazy-static");
    assert_eq!(result.ref_count, 1);
}

#[test]
fn test_scan_no_matches() {
    let dir = TempDir::new().unwrap();
    let p = write_file(
        dir.path(),
        "lib.rs",
        r#"
fn main() {
    println!("hello world");
}
"#,
    );

    let result = cargo_depflame::scanner::scan_files(&[p], "regex");
    assert_eq!(result.ref_count, 0);
    assert_eq!(result.files_with_matches, 0);
}

#[test]
fn test_scan_multiple_files() {
    let dir = TempDir::new().unwrap();
    let p1 = write_file(dir.path(), "a.rs", "use serde::Serialize;\n");
    let p2 = write_file(dir.path(), "b.rs", "use serde::Deserialize;\n");
    let p3 = write_file(dir.path(), "c.rs", "fn main() {}\n");

    let result = cargo_depflame::scanner::scan_files(&[p1, p2, p3], "serde");
    assert_eq!(result.ref_count, 2);
    assert_eq!(result.files_with_matches, 2);
}
