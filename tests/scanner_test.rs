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

#[test]
fn test_scan_with_lib_name_alias() {
    // Simulates crates like `natord-plus-plus` which set `[lib] name = "natord"`.
    // The package name is "natord-plus-plus" but code imports as `natord`.
    let dir = TempDir::new().unwrap();
    let p = write_file(
        dir.path(),
        "filter.rs",
        r#"
use natord::compare;
use natord::compare_ignore_case;

fn sort_items(a: &str, b: &str) -> std::cmp::Ordering {
    natord::compare(a, b)
}
"#,
    );

    // Without alias: package name "natord-plus-plus" finds nothing.
    let result = cargo_depflame::scanner::scan_files(&[p.clone()], "natord-plus-plus");
    assert_eq!(result.ref_count, 0, "package name alone should not match");

    // With lib name alias: finds the actual usage.
    let fs_cache = cargo_depflame::registry::FsCache::new();
    let regex_cache = cargo_depflame::scanner::RegexCache::new();
    let result = cargo_depflame::scanner::scan_files_with_aliases(
        &[p],
        "natord-plus-plus",
        &["natord".to_string()],
        &fs_cache,
        &regex_cache,
    );
    assert!(
        result.ref_count > 0,
        "lib name alias should find references"
    );
    assert_eq!(result.files_with_matches, 1);
}

#[test]
fn test_scan_with_renamed_lib_name() {
    // Simulates `uutils_term_grid` which sets `[lib] name = "term_grid"`.
    let dir = TempDir::new().unwrap();
    let p = write_file(
        dir.path(),
        "grid.rs",
        r#"
use term_grid::{Direction, Filling, Grid, GridOptions};
"#,
    );

    // Without alias: finds nothing under the package name.
    let result = cargo_depflame::scanner::scan_files(&[p.clone()], "uutils_term_grid");
    assert_eq!(result.ref_count, 0);

    // With alias: finds the usage.
    let fs_cache = cargo_depflame::registry::FsCache::new();
    let regex_cache = cargo_depflame::scanner::RegexCache::new();
    let result = cargo_depflame::scanner::scan_files_with_aliases(
        &[p],
        "uutils_term_grid",
        &["term_grid".to_string()],
        &fs_cache,
        &regex_cache,
    );
    assert!(result.ref_count > 0);
}

#[test]
fn test_scan_detects_test_only_refs() {
    // Simulates wiremock usage only inside #[cfg(test)] — should be dev-deps.
    let dir = TempDir::new().unwrap();
    let p = write_file(
        dir.path(),
        "lib.rs",
        r#"
pub fn publish() {
    // production code, no wiremock here
}

#[cfg(test)]
mod tests {
    use wiremock::MockServer;
    use wiremock::matchers::method;

    #[test]
    fn test_publish() {
        let server = wiremock::MockServer::start();
    }
}
"#,
    );

    let result = cargo_depflame::scanner::scan_files(&[p], "wiremock");
    assert_eq!(result.ref_count, 3);
    assert_eq!(
        result.test_only_refs, 3,
        "all refs should be detected as test-only"
    );
    // All refs are in test code, so all FileMatch entries should have in_test_code = true.
    for m in &result.file_matches {
        assert!(
            m.in_test_code,
            "match at line {} should be in_test_code",
            m.line_number
        );
    }
}

#[test]
fn test_scan_serde_with_attribute() {
    let dir = TempDir::new().unwrap();
    let p = write_file(
        dir.path(),
        "lib.rs",
        r#"
use serde::Deserialize;

#[derive(Deserialize)]
struct Config {
    #[serde(with = "humantime_serde")]
    timeout: std::time::Duration,

    #[serde(deserialize_with = "humantime_serde::deserialize")]
    interval: std::time::Duration,

    #[serde(serialize_with = "humantime_serde::serialize")]
    delay: std::time::Duration,
}
"#,
    );

    let result = cargo_depflame::scanner::scan_files(&[p], "humantime-serde");
    assert!(
        result.ref_count >= 3,
        "should detect serde with/serialize_with/deserialize_with references, got {}",
        result.ref_count
    );
}

#[test]
fn test_scan_mixed_test_and_prod_refs() {
    let dir = TempDir::new().unwrap();
    let p = write_file(
        dir.path(),
        "lib.rs",
        r#"
use serde::Serialize;

pub fn do_thing() {
    let _ = serde::de::value::Error::custom("oops");
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
}
"#,
    );

    let result = cargo_depflame::scanner::scan_files(&[p], "serde");
    assert!(result.ref_count >= 3); // use + path + test use
    assert!(
        result.test_only_refs >= 1,
        "should detect at least 1 test-only ref"
    );
    assert!(
        result.test_only_refs < result.ref_count,
        "not all refs should be test-only"
    );
}
