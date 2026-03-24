use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

/// Run `cargo tree` on the workspace to get the set of packages actually
/// resolved for the current platform (as opposed to the full cross-platform
/// resolve graph from `cargo metadata`).
///
/// Returns a set of `"name version"` strings for fast lookup.
pub fn resolve_real_deps(manifest_path: &Path) -> Option<HashSet<String>> {
    let output = Command::new("cargo")
        .args([
            "tree",
            "--prefix",
            "none",
            "-e",
            "normal,build",
            "--manifest-path",
        ])
        .arg(manifest_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut set = HashSet::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Lines look like: "serde v1.0.228"
        // Some have extra info: "serde v1.0.228 (*)" or "(proc-macro)"
        // We want "name version".
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let name = parts[0];
            let version = parts[1].strip_prefix('v').unwrap_or(parts[1]);
            set.insert(format!("{name} {version}"));
        }
    }

    if set.is_empty() {
        None
    } else {
        Some(set)
    }
}

/// Check if a package (name + version) is in the real resolved set.
pub fn is_real_dep(real_deps: &Option<HashSet<String>>, name: &str, version: &str) -> bool {
    match real_deps {
        Some(set) => set.contains(&format!("{name} {version}")),
        // If we couldn't get real deps, assume everything is real.
        None => true,
    }
}
