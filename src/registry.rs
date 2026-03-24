use dashmap::DashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Maximum file size to scan (10 MB). Covers most generated files.
const MAX_FILE_SIZE: u64 = 10_000_000;

/// Thread-safe cache for expensive filesystem operations.
/// Shared across `par_iter` calls to avoid redundant work.
pub struct FsCache {
    /// (crate_name, version) → source directory
    source_dirs: DashMap<(String, String), Option<PathBuf>>,
    /// source directory → list of .rs files
    rs_files: DashMap<PathBuf, Arc<Vec<PathBuf>>>,
    /// file path → content
    file_contents: DashMap<PathBuf, Arc<String>>,
    /// source directory → LOC count
    loc_counts: DashMap<PathBuf, usize>,
    /// Cargo registry source directories (computed once)
    registry_dirs: Vec<PathBuf>,
}

impl Default for FsCache {
    fn default() -> Self {
        Self::new()
    }
}

impl FsCache {
    pub fn new() -> Self {
        Self {
            source_dirs: DashMap::new(),
            rs_files: DashMap::new(),
            file_contents: DashMap::new(),
            loc_counts: DashMap::new(),
            registry_dirs: cargo_registry_src_dirs(),
        }
    }

    /// Cached crate source directory lookup.
    pub fn find_crate_source(&self, name: &str, version: &str) -> Option<PathBuf> {
        let key = (name.to_string(), version.to_string());
        if let Some(cached) = self.source_dirs.get(&key) {
            return cached.clone();
        }
        let dir_name = format!("{name}-{version}");
        let result = self
            .registry_dirs
            .iter()
            .map(|d| d.join(&dir_name))
            .find(|c| c.is_dir());
        self.source_dirs.insert(key, result.clone());
        result
    }

    /// Cached recursive .rs file collection.
    pub fn collect_rs_files(&self, root: &Path) -> Arc<Vec<PathBuf>> {
        let key = root.to_path_buf();
        if let Some(cached) = self.rs_files.get(&key) {
            return cached.clone();
        }
        let mut files = Vec::new();
        collect_rs_files_recursive(root, &mut files);
        let result = Arc::new(files);
        self.rs_files.insert(key, result.clone());
        result
    }

    /// Cached file read.
    pub fn read_file(&self, path: &Path) -> Option<Arc<String>> {
        let key = path.to_path_buf();
        if let Some(cached) = self.file_contents.get(&key) {
            return Some(cached.clone());
        }
        let content = std::fs::read_to_string(path).ok()?;
        let result = Arc::new(content);
        self.file_contents.insert(key, result.clone());
        Some(result)
    }

    /// Cached LOC count. Reuses cached file contents.
    pub fn count_loc(&self, root: &Path, rs_files: &[PathBuf]) -> usize {
        let key = root.to_path_buf();
        if let Some(cached) = self.loc_counts.get(&key) {
            return *cached;
        }
        let mut count = 0;
        for path in rs_files {
            if let Some(content) = self.read_file(path) {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() && !trimmed.starts_with("//") {
                        count += 1;
                    }
                }
            }
        }
        self.loc_counts.insert(key, count);
        count
    }
}

fn cargo_registry_src_dirs() -> Vec<PathBuf> {
    let cargo_home = std::env::var("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".cargo"))
                .unwrap_or_else(|_| PathBuf::from(".cargo"))
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
            if let Ok(meta) = path.metadata() {
                if meta.len() <= MAX_FILE_SIZE {
                    files.push(path);
                }
            }
        }
    }
}
