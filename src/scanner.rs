use regex::Regex;
use std::path::PathBuf;

/// A single matching location within a file.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileMatch {
    pub path: String,
    pub line_number: usize,
    pub line_content: String,
}

/// Result of scanning an intermediate crate for references to a fat dependency.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScanResult {
    pub fat_crate_name: String,
    pub ref_count: usize,
    pub file_matches: Vec<FileMatch>,
    pub files_with_matches: usize,
}

/// Scan the given `.rs` files for references to `fat_crate_name`.
///
/// Uses lexical scanning with regex. Searches for:
/// - `use <crate>::...`
/// - `<crate>::...` in expressions
/// - `extern crate <crate>`
///
/// Skips single-line comments. Not perfect, but fast and good enough.
pub fn scan_files(rs_files: &[PathBuf], fat_crate_name: &str) -> ScanResult {
    // Cargo normalizes hyphens to underscores in Rust code.
    let normalized = fat_crate_name.replace('-', "_");

    // Build patterns.
    let pattern_path = format!(r"\b{}::", regex::escape(&normalized));
    let pattern_use = format!(r"\buse\s+{}\b", regex::escape(&normalized));
    let pattern_extern = format!(r"\bextern\s+crate\s+{}\b", regex::escape(&normalized));

    let combined = format!("(?:{pattern_path})|(?:{pattern_use})|(?:{pattern_extern})");
    let re = Regex::new(&combined).expect("valid regex");

    let mut file_matches = Vec::new();
    let mut files_with_matches = 0;

    for path in rs_files {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let mut found_in_file = false;

        for (line_idx, line) in content.lines().enumerate() {
            let trimmed = line.trim_start();

            // Skip single-line comments.
            if trimmed.starts_with("//") {
                continue;
            }

            if re.is_match(line) {
                found_in_file = true;
                file_matches.push(FileMatch {
                    path: path.display().to_string(),
                    line_number: line_idx + 1,
                    line_content: line.trim().to_string(),
                });
            }
        }

        if found_in_file {
            files_with_matches += 1;
        }
    }

    let ref_count = file_matches.len();
    ScanResult {
        fat_crate_name: fat_crate_name.to_string(),
        ref_count,
        file_matches,
        files_with_matches,
    }
}

/// Relativize a path for display purposes, replacing the cargo registry prefix
/// with `~/.cargo/registry/src/.../<crate>/`.
pub fn display_path(path: &str) -> String {
    if let Some(idx) = path.find(".cargo/registry/src/") {
        let home_prefix = &path[..idx];
        let rest = &path[idx + ".cargo/registry/src/".len()..];
        // Strip the registry hash directory.
        if let Some(slash_idx) = rest.find('/') {
            return format!(
                "{}/.cargo/registry/src/.../{}",
                home_prefix.trim_end_matches('/'),
                &rest[slash_idx + 1..]
            );
        }
    }
    path.to_string()
}

