use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;

/// What `cargo build` actually resolves: the packages it compiles, and the
/// features it enables on each of them.
#[derive(Debug, Default)]
pub struct BuildResolve {
    /// `"name version"` for every package compiled by a default build.
    pub packages: HashSet<String>,
    /// `"name version"` -> the features cargo enables on it.
    pub features: HashMap<String, Vec<String>>,
}

/// Run `cargo tree` on the workspace to get the packages actually resolved for
/// the current platform (as opposed to the full cross-platform resolve graph
/// from `cargo metadata`), along with the features enabled on each.
///
/// This is narrower than `cargo metadata` in a second way that matters:
/// metadata unifies features across every workspace member, so a member
/// outside `default-members` can enable features — and so pull in optional
/// deps — that a default build never sees.
pub fn resolve_build(manifest_path: &Path) -> Option<BuildResolve> {
    // "{p}|{f}": the package, then its enabled features, comma separated.
    let output = Command::new("cargo")
        .args([
            "tree",
            "--prefix",
            "none",
            "-e",
            "normal,build",
            "--format",
            "{p}|{f}",
            "--manifest-path",
        ])
        .arg(manifest_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut resolve = BuildResolve::default();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (pkg, feats) = match line.split_once('|') {
            Some(parts) => parts,
            None => continue,
        };
        // "{p}" is "name vVERSION (source)".
        let parts: Vec<&str> = pkg.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        let key = format!(
            "{} {}",
            parts[0],
            parts[1].strip_prefix('v').unwrap_or(parts[1])
        );
        // cargo tree marks an already-printed subtree with a trailing "(*)",
        // which lands after the feature list: "serde v1.0.228|default,std (*)".
        let feats = feats.trim().strip_suffix("(*)").unwrap_or(feats).trim();
        let features: Vec<String> = feats
            .split(',')
            .map(|f| f.trim())
            .filter(|f| !f.is_empty() && *f != "(*)")
            .map(|f| f.to_string())
            .collect();

        resolve.packages.insert(key.clone());
        // A package can appear more than once with different feature sets —
        // resolver v2 resolves build-dependencies separately from normal ones,
        // so the same crate may be built twice with different features. Union
        // them: a crate is in the graph if any of those builds pulls it in.
        let entry = resolve.features.entry(key).or_default();
        for feature in features {
            if !entry.contains(&feature) {
                entry.push(feature);
            }
        }
    }

    if resolve.packages.is_empty() {
        None
    } else {
        Some(resolve)
    }
}

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
