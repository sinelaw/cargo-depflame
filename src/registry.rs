use std::path::{Path, PathBuf};

/// Locate the Cargo registry source directory.
pub fn cargo_registry_src_dirs() -> Vec<PathBuf> {
    let cargo_home = std::env::var("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            home::home_dir()
                .map(|h| h.join(".cargo"))
                .unwrap_or_else(|| PathBuf::from(".cargo"))
        });

    let registry_src = cargo_home.join("registry").join("src");
    if !registry_src.is_dir() {
        return Vec::new();
    }

    match std::fs::read_dir(&registry_src) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Resolve a crate name + version to its source directory in the Cargo registry.
pub fn find_crate_source(name: &str, version: &str) -> Option<PathBuf> {
    let dir_name = format!("{name}-{version}");

    for registry_dir in cargo_registry_src_dirs() {
        let candidate = registry_dir.join(&dir_name);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

/// Recursively collect all `.rs` files under a directory.
pub fn collect_rs_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_rs_files_recursive(root, &mut files);
    files
}

fn collect_rs_files_recursive(dir: &Path, files: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files_recursive(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            // Skip very large files (likely generated).
            if let Ok(meta) = path.metadata() {
                if meta.len() <= 1_000_000 {
                    files.push(path);
                }
            }
        }
    }
}
