use rayon::prelude::*;
use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

/// Run `cargo tree` on the workspace to get the set of packages actually
/// resolved for the current platform (as opposed to the full cross-platform
/// resolve graph from `cargo metadata`).
///
/// Returns a set of `"name version"` strings for fast lookup.
pub fn resolve_real_deps(manifest_path: &Path) -> Option<HashSet<String>> {
    // Use --format "{p}" to get a stable output ("name vVERSION" per line)
    // without the (*) duplicate markers and (proc-macro) labels that appear
    // in the default tree display.
    let output = Command::new("cargo")
        .args([
            "tree",
            // No `--workspace`: this is what `cargo build` compiles, which is
            // also what the dep tree is rooted at (`default-members`).
            "--prefix",
            "none",
            "-e",
            "normal,build",
            "--format",
            "{p}",
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
        // --format "{p}" gives "name vVERSION (path)" per line.
        // We want "name VERSION" (without the "v" prefix).
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

/// Targets offered in the report's platform selector, alongside the host.
/// Kept to the platforms a Rust crate realistically ships to; each one costs a
/// `cargo metadata` resolve at analysis time (they run in parallel).
pub const COMMON_TARGETS: &[&str] = &[
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-musl",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "wasm32-unknown-unknown",
];

/// The packages that resolve for one target triple.
pub struct PlatformResolve {
    pub triple: String,
    pub is_host: bool,
    pub packages: HashSet<cargo_metadata::PackageId>,
}

/// The host triple, as rustc reports it ("host: x86_64-unknown-linux-gnu").
pub fn host_triple() -> Option<String> {
    let output = Command::new("rustc").arg("-vV").output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(|t| t.trim().to_string())
}

/// Resolve the dependency graph as it applies to one target.
///
/// `--filter-platform` makes cargo evaluate the `cfg(...)` expressions itself,
/// so the result is exact rather than a reimplementation of cfg matching.
/// `--all-features` matches the graph the flamegraph is built from.
fn resolve_for_target(
    manifest_path: &Path,
    triple: &str,
) -> Option<HashSet<cargo_metadata::PackageId>> {
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(manifest_path)
        .other_options(vec![
            "--all-features".to_string(),
            "--filter-platform".to_string(),
            triple.to_string(),
        ])
        .exec()
        .ok()?;

    let resolve = metadata.resolve.as_ref()?;
    Some(resolve.nodes.iter().map(|n| n.id.clone()).collect())
}

/// Resolve every offered target, host first. Targets rustc doesn't know are
/// skipped rather than failing the analysis.
pub fn resolve_all_platforms(manifest_path: &Path) -> Vec<PlatformResolve> {
    let host = host_triple();

    let mut triples: Vec<String> = Vec::new();
    if let Some(ref h) = host {
        triples.push(h.clone());
    }
    for t in COMMON_TARGETS {
        if Some(*t) != host.as_deref() {
            triples.push((*t).to_string());
        }
    }

    let resolved: Vec<Option<PlatformResolve>> = triples
        .par_iter()
        .map(|triple| {
            resolve_for_target(manifest_path, triple).map(|packages| PlatformResolve {
                triple: triple.clone(),
                is_host: Some(triple.as_str()) == host.as_deref(),
                packages,
            })
        })
        .collect();

    resolved.into_iter().flatten().collect()
}
