use dashmap::DashMap;
use regex::Regex;
use std::path::PathBuf;
use std::sync::Arc;

use crate::registry::FsCache;

/// A single matching location within a file.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileMatch {
    pub path: String,
    pub line_number: usize,
    pub line_content: String,
    #[serde(default)]
    pub in_generated_file: bool,
    #[serde(default)]
    pub in_test_code: bool,
}

/// Result of scanning an intermediate crate for references to a heavy dependency.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScanResult {
    pub heavy_crate_name: String,
    #[serde(default)]
    pub searched_names: Vec<String>,
    pub ref_count: usize,
    pub file_matches: Vec<FileMatch>,
    pub files_with_matches: usize,
    #[serde(default)]
    pub generated_file_refs: usize,
    #[serde(default)]
    pub test_only_refs: usize,
    #[serde(default)]
    pub distinct_items: Vec<String>,
    #[serde(default)]
    pub has_re_export_all: bool,
}

/// Thread-safe cache for compiled regex patterns.
pub struct RegexCache {
    cache: DashMap<String, Arc<(Regex, Regex)>>,
}

impl Default for RegexCache {
    fn default() -> Self {
        Self::new()
    }
}

impl RegexCache {
    pub fn new() -> Self {
        Self {
            cache: DashMap::new(),
        }
    }

    fn get_or_compile(&self, heavy_crate_name: &str, names: &[String]) -> Arc<(Regex, Regex)> {
        let mut key_parts: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        key_parts.sort();
        let key = format!("{}:{}", heavy_crate_name, key_parts.join(","));

        if let Some(cached) = self.cache.get(&key) {
            return cached.clone();
        }

        let result = Arc::new(compile_patterns(heavy_crate_name, names));
        self.cache.insert(key, result.clone());
        result
    }
}

fn compile_patterns(heavy_crate_name: &str, names: &[String]) -> (Regex, Regex) {
    let mut pattern_parts = Vec::new();
    for name in names {
        let escaped = regex::escape(name);
        pattern_parts.push(format!(r"\b{escaped}::"));
        pattern_parts.push(format!(r"\buse\s+{escaped}\b"));
        pattern_parts.push(format!(r"\bextern\s+crate\s+{escaped}\b"));
        // #[serde(with = "crate_name")] and serialize_with / deserialize_with variants
        pattern_parts.push(format!(
            r#"#\[serde\([^)]*\b(?:(?:de)?serialize_)?with\s*=\s*"{escaped}"#
        ));
    }
    add_macro_patterns(&mut pattern_parts, heavy_crate_name, names);

    let combined = pattern_parts
        .iter()
        .map(|p| format!("(?:{p})"))
        .collect::<Vec<_>>()
        .join("|");
    let re = Regex::new(&combined).expect("valid regex");

    let re_export_patterns: Vec<String> = names
        .iter()
        .map(|name| format!(r"pub\s+use\s+{}::\*", regex::escape(name)))
        .collect();
    let re_export_re = Regex::new(&re_export_patterns.join("|")).expect("valid regex");

    (re, re_export_re)
}

fn build_names(heavy_crate_name: &str, aliases: &[String]) -> Vec<String> {
    let normalized = heavy_crate_name.replace('-', "_");
    let mut names: Vec<String> = vec![normalized];
    for alias in aliases {
        let alias_norm = alias.replace('-', "_");
        if !names.contains(&alias_norm) {
            names.push(alias_norm);
        }
    }
    names
}

/// Convenience wrapper: scan without caches (creates temporary ones).
pub fn scan_files(rs_files: &[PathBuf], heavy_crate_name: &str) -> ScanResult {
    let fs_cache = FsCache::new();
    let regex_cache = RegexCache::new();
    scan_files_with_aliases(rs_files, heavy_crate_name, &[], &fs_cache, &regex_cache)
}

/// Scan .rs files for references to `heavy_crate_name`, using caches.
pub fn scan_files_with_aliases(
    rs_files: &[PathBuf],
    heavy_crate_name: &str,
    aliases: &[String],
    fs_cache: &FsCache,
    regex_cache: &RegexCache,
) -> ScanResult {
    let names = build_names(heavy_crate_name, aliases);
    let regexes = regex_cache.get_or_compile(heavy_crate_name, &names);
    let (ref re, ref re_export_re) = *regexes;

    let mut file_matches = Vec::new();
    let mut files_with_matches = 0;
    let mut generated_file_refs = 0;
    let mut test_only_refs = 0;
    let mut has_re_export_all = false;

    for path in rs_files {
        let content = match fs_cache.read_file(path) {
            Some(c) => c,
            None => continue,
        };

        let generated = is_generated_file(&content);
        let in_test_file = is_test_file(path);
        let mut found_in_file = false;
        let mut in_test_mod = in_test_file;
        let mut test_brace_depth: Option<usize> = if in_test_file { Some(0) } else { None };
        let mut brace_depth: usize = 0;

        for (line_idx, line) in content.lines().enumerate() {
            let trimmed = line.trim_start();

            if trimmed.starts_with("#[cfg(test)]") {
                test_brace_depth = Some(brace_depth);
            }

            for ch in line.chars() {
                match ch {
                    '{' => brace_depth += 1,
                    '}' => {
                        brace_depth = brace_depth.saturating_sub(1);
                        if let Some(test_depth) = test_brace_depth {
                            if brace_depth <= test_depth && !in_test_file {
                                in_test_mod = false;
                                test_brace_depth = None;
                            }
                        }
                    }
                    _ => {}
                }
            }

            if let Some(test_depth) = test_brace_depth {
                if brace_depth > test_depth {
                    in_test_mod = true;
                }
            }

            if trimmed.starts_with("//") {
                continue;
            }

            if !has_re_export_all && re_export_re.is_match(line) {
                has_re_export_all = true;
            }

            if re.is_match(line) {
                found_in_file = true;
                if generated {
                    generated_file_refs += 1;
                }
                if in_test_mod {
                    test_only_refs += 1;
                }
                file_matches.push(FileMatch {
                    path: path.display().to_string(),
                    line_number: line_idx + 1,
                    line_content: line.trim().to_string(),
                    in_generated_file: generated,
                    in_test_code: in_test_mod,
                });
            }
        }

        if found_in_file {
            files_with_matches += 1;
        }
    }

    let ref_count = file_matches.len();
    let distinct_items = extract_distinct_items(&file_matches, &names);
    ScanResult {
        heavy_crate_name: heavy_crate_name.to_string(),
        searched_names: names,
        ref_count,
        file_matches,
        files_with_matches,
        generated_file_refs,
        test_only_refs,
        distinct_items,
        has_re_export_all,
    }
}

pub fn display_path(path: &str) -> String {
    if let Some(idx) = path.find(".cargo/registry/src/") {
        let home_prefix = &path[..idx];
        let rest = &path[idx + ".cargo/registry/src/".len()..];
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

const MACRO_CRATES: &[(&str, &[&str])] = &[
    ("serde", &[r"#\[derive\([^)]*\b(Serialize|Deserialize)\b"]),
    (
        "serde_json",
        &[
            r"\bserde_json!\b",
            r"\bjson!\b",
            r#"serde_json::from_"#,
            r#"serde_json::to_"#,
        ],
    ),
    (
        "clap",
        &[
            r"#\[derive\([^)]*\b(Parser|Args|Subcommand|ValueEnum)\b",
            r"#\[command\b",
            r"#\[arg\b",
        ],
    ),
    ("thiserror", &[r"#\[derive\([^)]*\bError\b"]),
    (
        "tokio",
        &[r"#\[tokio::", r"tokio::spawn\b", r"tokio::select!\b"],
    ),
    (
        "tracing",
        &[
            r"\btracing::(info|debug|warn|error|trace|instrument)\b",
            r"#\[instrument\b",
            r"#\[tracing::instrument\b",
            r"\b(info|debug|warn|error|trace)!\(",
        ],
    ),
    ("log", &[r"\b(info|debug|warn|error|trace)!\("]),
    (
        "anyhow",
        &[
            r"\banyhow!\(",
            r"\bbail!\(",
            r"\bensure!\(",
            r"\bResult<",
            r"\banyhow::Result\b",
            r"\bContext\b",
        ],
    ),
    ("async_trait", &[r"#\[async_trait\b"]),
    (
        "strum",
        &[r"#\[derive\([^)]*\b(EnumString|Display|EnumIter|IntoStaticStr)\b"],
    ),
    (
        "derive_more",
        &[r"#\[derive\([^)]*\b(From|Into|Display|Deref|Constructor)\b"],
    ),
];

const ALLOCATOR_CRATES: &[(&str, &str)] = &[
    ("mimalloc", "MiMalloc"),
    ("tikv-jemallocator", "Jemalloc"),
    ("jemallocator", "Jemalloc"),
    ("snmalloc-rs", "SnMalloc"),
];

fn add_macro_patterns(patterns: &mut Vec<String>, heavy_crate_name: &str, names: &[String]) {
    let normalized = heavy_crate_name.replace('-', "_");
    for (crate_name, extra_patterns) in MACRO_CRATES {
        if normalized == crate_name.replace('-', "_")
            || names.iter().any(|n| *n == crate_name.replace('-', "_"))
        {
            for p in *extra_patterns {
                patterns.push(p.to_string());
            }
        }
    }
    for (crate_name, type_name) in ALLOCATOR_CRATES {
        if normalized == crate_name.replace('-', "_")
            || names.iter().any(|n| *n == crate_name.replace('-', "_"))
        {
            patterns.push(r"#\[global_allocator\]".to_string());
            patterns.push(format!(r"\b{type_name}\b"));
        }
    }
}

fn is_test_file(path: &std::path::Path) -> bool {
    let path_str = path.to_string_lossy();
    if path_str.contains("/tests/") || path_str.contains("\\tests\\") {
        return true;
    }
    if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
        if name.ends_with("_test") || name.starts_with("test_") {
            return true;
        }
    }
    false
}

fn is_generated_file(content: &str) -> bool {
    for line in content.lines().take(10) {
        let lower = line.to_lowercase();
        if lower.contains("@generated")
            || lower.contains("this file was auto-generated")
            || lower.contains("this file is auto-generated")
            || lower.contains("automatically generated")
            || lower.contains("do not edit")
            || lower.contains("generated by")
            || lower.contains("auto-generated by")
            || lower.contains("this file has been auto-generated")
        {
            return true;
        }
    }
    false
}

fn extract_distinct_items(matches: &[FileMatch], crate_names: &[String]) -> Vec<String> {
    let mut items = std::collections::HashSet::new();
    for m in matches {
        let line = &m.line_content;
        for name in crate_names {
            let prefix = format!("{name}::");
            for (idx, _) in line.match_indices(&prefix) {
                let after = &line[idx + prefix.len()..];
                if after.starts_with('{') {
                    if let Some(close) = after.find('}') {
                        for item in after[1..close].split(',') {
                            let item = item.trim().split("::").next().unwrap_or("").trim();
                            if !item.is_empty() && item != "*" {
                                items.insert(item.to_string());
                            }
                        }
                    }
                } else {
                    let item: String = after
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if !item.is_empty() && item != "*" {
                        items.insert(item);
                    }
                }
            }
        }
    }
    let mut sorted: Vec<String> = items.into_iter().collect();
    sorted.sort();
    sorted
}
