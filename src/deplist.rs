//! Build a depflame report from a flat list of crates instead of a local
//! workspace.
//!
//! Input is one crate per line ("name-1.2.3"). Dependency edges between the
//! listed crates are reconstructed from crates.io registry index metadata,
//! restricted to crates present in the list. The minimal set of "root"
//! crates — those nothing else in the list depends on — is computed, and a
//! synthetic workspace root is placed above them so the standard report and
//! flamegraph pipeline can run unchanged.
//!
//! Caveats: optional dependency edges are followed by default (the list
//! doesn't say which features were enabled, but a listed optional dep was
//! enabled by *something*), and dev-dependencies are always ignored.

use cargo_metadata::PackageId;
use semver::{Version, VersionReq};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::flamegraph::{DepTreeData, DepTreeEdge, DepTreeNode};
use crate::graph::{DepGraph, DepNode, EdgeMeta};
use crate::report::AnalysisReport;

/// One crate from the input list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedCrate {
    pub name: String,
    pub version: Version,
}

/// One published version of a crate, as recorded in the registry index.
#[derive(Debug, Clone, Deserialize)]
pub struct IndexVersion {
    pub vers: String,
    #[serde(default)]
    pub deps: Vec<IndexDep>,
    #[serde(default)]
    pub features: BTreeMap<String, Vec<String>>,
    /// Features using the `dep:` / weak-dep syntax (index schema v2).
    #[serde(default)]
    pub features2: Option<BTreeMap<String, Vec<String>>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexDep {
    /// Name the dependency is declared under (the rename, if renamed).
    pub name: String,
    pub req: String,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub kind: Option<String>,
    /// Real crate name when the dependency is renamed.
    #[serde(default)]
    pub package: Option<String>,
}

impl IndexVersion {
    /// All features, merging the v1 and v2 feature maps.
    fn all_features(&self) -> BTreeMap<String, Vec<String>> {
        let mut merged = self.features.clone();
        if let Some(f2) = &self.features2 {
            for (k, v) in f2 {
                merged
                    .entry(k.clone())
                    .or_default()
                    .extend(v.iter().cloned());
            }
        }
        merged
    }
}

pub struct ListGraphOptions {
    pub include_build: bool,
    pub skip_optional: bool,
}

/// A resolved dependency edge between two listed crates.
#[derive(Debug, Clone)]
pub struct ListEdge {
    pub from: usize,
    pub to: usize,
    pub optional: bool,
    pub build_only: bool,
    /// Feature on `from` that gates this edge, when optional.
    pub gating_feature: Option<String>,
    /// Features `from` enables on `to`.
    pub child_features: Vec<String>,
}

/// The dependency graph induced on the listed crates.
pub struct ListGraph {
    pub crates: Vec<ListedCrate>,
    pub edges: Vec<ListEdge>,
    /// Available features per crate (indexed like `crates`).
    pub features: Vec<BTreeMap<String, Vec<String>>>,
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Input parsing
// ---------------------------------------------------------------------------

/// Parse the input list. Returns the crates (deduplicated, in input order)
/// and warnings for lines that could not be parsed.
pub fn parse_dep_list(text: &str) -> (Vec<ListedCrate>, Vec<String>) {
    let mut crates = Vec::new();
    let mut warnings = Vec::new();
    let mut seen = HashSet::new();
    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        match parse_line(line) {
            Some(c) => {
                if seen.insert((c.name.clone(), c.version.clone())) {
                    crates.push(c);
                }
            }
            None => warnings.push(format!(
                "line {}: cannot parse '{}' as name-version, skipping",
                lineno + 1,
                line
            )),
        }
    }
    (crates, warnings)
}

/// Parse one line: "name-1.2.3", "name@1.2.3" or "name 1.2.3".
fn parse_line(line: &str) -> Option<ListedCrate> {
    let (name, vers) = if let Some((n, v)) = line.split_once('@') {
        (n.trim(), v.trim())
    } else if let Some((n, v)) = line.split_once(char::is_whitespace) {
        (n.trim(), v.trim())
    } else {
        // "name-1.2.3": split at the first '-' whose suffix parses as a
        // version, so multi-segment names like "zstd-safe-7.2.4" work.
        let (i, _) = line
            .match_indices('-')
            .find(|(i, _)| Version::parse(&line[i + 1..]).is_ok())?;
        (&line[..i], &line[i + 1..])
    };
    let version = Version::parse(vers).ok()?;
    let valid_name = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !valid_name {
        return None;
    }
    Some(ListedCrate {
        name: name.to_string(),
        version,
    })
}

// ---------------------------------------------------------------------------
// Graph construction
// ---------------------------------------------------------------------------

/// Build the dependency graph induced on the listed crates, resolving each
/// dependency requirement against the listed versions (picking the highest
/// match, like cargo).
pub fn build_list_graph(
    crates: Vec<ListedCrate>,
    index: &HashMap<String, Vec<IndexVersion>>,
    opts: &ListGraphOptions,
) -> ListGraph {
    let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, c) in crates.iter().enumerate() {
        by_name.entry(c.name.as_str()).or_default().push(i);
    }

    let mut edges: HashMap<(usize, usize), ListEdge> = HashMap::new();
    let mut features: Vec<BTreeMap<String, Vec<String>>> = vec![BTreeMap::new(); crates.len()];
    let mut warnings = Vec::new();

    for (i, c) in crates.iter().enumerate() {
        let entry = index.get(&c.name).and_then(|vs| {
            vs.iter().find(|v| {
                Version::parse(&v.vers).is_ok_and(|parsed| same_version(&parsed, &c.version))
            })
        });
        let entry = match entry {
            Some(e) => e,
            None => {
                warnings.push(format!(
                    "{}-{}: not found in registry index, treating as leaf",
                    c.name, c.version
                ));
                continue;
            }
        };
        let parent_features = entry.all_features();
        features[i] = parent_features.clone();

        for dep in &entry.deps {
            let kind = dep.kind.as_deref().unwrap_or("normal");
            let follow = match kind {
                "normal" => true,
                "build" => opts.include_build,
                _ => false, // dev deps never ship
            };
            if !follow || (dep.optional && opts.skip_optional) {
                continue;
            }

            let real_name = dep.package.as_deref().unwrap_or(&dep.name);
            let Some(candidates) = by_name.get(real_name) else {
                // Dependency not in the list (proc-macro, build dep, or a
                // feature that wasn't enabled). Expected; skip silently.
                continue;
            };
            let req = match VersionReq::parse(&dep.req) {
                Ok(r) => r,
                Err(e) => {
                    warnings.push(format!(
                        "{}-{}: unparseable requirement '{}' on {}: {}",
                        c.name, c.version, dep.req, real_name, e
                    ));
                    continue;
                }
            };
            let best = candidates
                .iter()
                .copied()
                .filter(|&j| j != i && req.matches(&crates[j].version))
                .max_by(|&a, &b| crates[a].version.cmp(&crates[b].version));
            let Some(target) = best else { continue };

            let gating_feature = if dep.optional {
                find_gating_feature(&parent_features, &dep.name)
            } else {
                None
            };

            // Merge duplicate edges (e.g. platform-specific declarations):
            // an edge is optional/build-only only if every declaration is.
            let e = edges.entry((i, target)).or_insert_with(|| ListEdge {
                from: i,
                to: target,
                optional: dep.optional,
                build_only: kind == "build",
                gating_feature: gating_feature.clone(),
                child_features: Vec::new(),
            });
            e.optional &= dep.optional;
            e.build_only &= kind == "build";
            if e.gating_feature.is_none() {
                e.gating_feature = gating_feature;
            }
            for f in &dep.features {
                if !e.child_features.contains(f) {
                    e.child_features.push(f.clone());
                }
            }
        }
    }

    let mut edges: Vec<ListEdge> = edges.into_values().collect();
    edges.sort_by_key(|e| (e.from, e.to));
    for e in &mut edges {
        e.child_features.sort();
    }

    ListGraph {
        crates,
        edges,
        features,
        warnings,
    }
}

/// Version equality ignoring build metadata: published versions often carry
/// it (e.g. "0.9.34+deprecated", "0.18.2+1.9.1") while dep lists don't.
fn same_version(a: &Version, b: &Version) -> bool {
    a.major == b.major && a.minor == b.minor && a.patch == b.patch && a.pre == b.pre
}

/// Find a feature on the parent that enables the (optional) dependency
/// declared as `dep_name`.
fn find_gating_feature(features: &BTreeMap<String, Vec<String>>, dep_name: &str) -> Option<String> {
    let dep_entry = format!("dep:{dep_name}");
    let slash_prefix = format!("{dep_name}/");
    features.iter().find_map(|(feat, activates)| {
        activates
            .iter()
            .any(|a| a == &dep_entry || a == dep_name || a.starts_with(&slash_prefix))
            .then(|| feat.clone())
    })
}

// ---------------------------------------------------------------------------
// Minimal root set
// ---------------------------------------------------------------------------

/// Compute the minimal set of crates that transitively covers the whole
/// list: every crate with no incoming edge must be a root, and in a DAG
/// their closure covers everything. If dependency cycles leave crates
/// uncovered, roots are greedily added (with a warning).
pub fn minimal_roots(n: usize, edges: &[ListEdge]) -> (Vec<usize>, Vec<String>) {
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut indegree = vec![0usize; n];
    for e in edges {
        out[e.from].push(e.to);
        indegree[e.to] += 1;
    }

    let mut roots: Vec<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
    let mut covered = vec![false; n];
    bfs_cover(&roots, &out, &mut covered);

    let mut warnings = Vec::new();
    while covered.iter().any(|&c| !c) {
        let best = (0..n)
            .filter(|&i| !covered[i])
            .max_by_key(|&i| bfs_count_new(i, &out, &covered))
            .expect("uncovered node must exist");
        warnings.push(format!(
            "dependency cycle not reachable from any natural root; adding extra root #{best}"
        ));
        roots.push(best);
        bfs_cover(&[best], &out, &mut covered);
    }

    roots.sort_unstable();
    (roots, warnings)
}

fn bfs_cover(starts: &[usize], out: &[Vec<usize>], covered: &mut [bool]) {
    let mut queue: VecDeque<usize> = VecDeque::new();
    for &s in starts {
        if !covered[s] {
            covered[s] = true;
            queue.push_back(s);
        }
    }
    while let Some(cur) = queue.pop_front() {
        for &next in &out[cur] {
            if !covered[next] {
                covered[next] = true;
                queue.push_back(next);
            }
        }
    }
}

fn bfs_count_new(start: usize, out: &[Vec<usize>], covered: &[bool]) -> usize {
    let mut visited = vec![false; out.len()];
    let mut queue = VecDeque::new();
    visited[start] = true;
    queue.push_back(start);
    let mut count = 0;
    while let Some(cur) = queue.pop_front() {
        if !covered[cur] {
            count += 1;
        }
        for &next in &out[cur] {
            if !visited[next] {
                visited[next] = true;
                queue.push_back(next);
            }
        }
    }
    count
}

// ---------------------------------------------------------------------------
// Report assembly
// ---------------------------------------------------------------------------

fn pid(c: &ListedCrate) -> PackageId {
    PackageId {
        repr: format!("{} {} (from-list)", c.name, c.version),
    }
}

/// Assemble a standard AnalysisReport from the list graph. A synthetic
/// workspace root named `root_name` is placed above the minimal roots, so
/// the existing renderers (text table, flamegraph, HTML) work unchanged.
pub fn build_report(
    root_name: &str,
    graph: &ListGraph,
    roots: &[usize],
    heavy_threshold: usize,
    threshold: f64,
) -> AnalysisReport {
    let n = graph.crates.len();
    let root_pid = PackageId {
        repr: format!("{root_name} 0.0.0 (from-list-root)"),
    };

    // ── DepGraph (for weights, direct-dep summary, heavy count) ──────────
    let mut nodes: HashMap<PackageId, DepNode> = graph
        .crates
        .iter()
        .map(|c| {
            (
                pid(c),
                DepNode {
                    name: c.name.clone(),
                    version: c.version.to_string(),
                    is_workspace_member: false,
                    transitive_weight: 0,
                    transitive_set: HashSet::new(),
                },
            )
        })
        .collect();
    nodes.insert(
        root_pid.clone(),
        DepNode {
            name: root_name.to_string(),
            version: "0.0.0".to_string(),
            is_workspace_member: true,
            transitive_weight: 0,
            transitive_set: HashSet::new(),
        },
    );

    let mut forward: HashMap<PackageId, Vec<PackageId>> = HashMap::new();
    let mut reverse: HashMap<PackageId, Vec<PackageId>> = HashMap::new();
    let mut edge_meta: HashMap<(PackageId, PackageId), EdgeMeta> = HashMap::new();
    for c in &graph.crates {
        forward.entry(pid(c)).or_default();
    }
    let mut add_edge = |from: PackageId, to: PackageId, meta: EdgeMeta| {
        forward.entry(from.clone()).or_default().push(to.clone());
        reverse.entry(to.clone()).or_default().push(from.clone());
        edge_meta.insert((from, to), meta);
    };
    for &r in roots {
        add_edge(
            root_pid.clone(),
            pid(&graph.crates[r]),
            EdgeMeta {
                build_only: false,
                already_optional: false,
                platform_conditional: false,
            },
        );
    }
    for e in &graph.edges {
        add_edge(
            pid(&graph.crates[e.from]),
            pid(&graph.crates[e.to]),
            EdgeMeta {
                build_only: e.build_only,
                already_optional: e.optional,
                platform_conditional: false,
            },
        );
    }

    let workspace_members: HashSet<PackageId> = [root_pid.clone()].into_iter().collect();
    let dep_graph = DepGraph::from_parts(nodes, forward, reverse, workspace_members, edge_meta);

    let total_deps = dep_graph.total_dependency_count();
    let heavy_nodes_found = dep_graph.heavy_nodes(heavy_threshold).len();
    let mut direct_dep_summary = crate::analyze::build_direct_dep_summary(&dep_graph, &None);
    let transitive_sharing = crate::analyze::build_sharing_scopes(&dep_graph, &None);
    crate::analyze::attach_owner_counts(&mut direct_dep_summary, &transitive_sharing);

    // ── DepTreeData (flamegraph) ──────────────────────────────────────────
    // Tree node i corresponds to graph.crates[i]; the synthetic root is n.
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n + 1];
    let mut rev: Vec<Vec<usize>> = vec![Vec::new(); n + 1];
    for e in &graph.edges {
        out[e.from].push(e.to);
        rev[e.to].push(e.from);
    }
    for &r in roots {
        out[n].push(r);
        rev[r].push(n);
    }

    let mut tree_nodes: Vec<DepTreeNode> = Vec::with_capacity(n + 1);
    for (i, c) in graph.crates.iter().enumerate() {
        tree_nodes.push(DepTreeNode {
            name: c.name.clone(),
            version: c.version.to_string(),
            transitive_weight: dep_graph
                .nodes
                .get(&pid(c))
                .map(|nd| nd.transitive_weight)
                .unwrap_or(1),
            is_workspace: false,
            unique_ancestors: count_ancestors(i, &rev),
            children: Vec::new(),
            enabled_features: Vec::new(),
            available_features: graph.features[i].clone(),
        });
    }
    tree_nodes.push(DepTreeNode {
        name: root_name.to_string(),
        version: "0.0.0".to_string(),
        transitive_weight: dep_graph
            .nodes
            .get(&root_pid)
            .map(|nd| nd.transitive_weight)
            .unwrap_or(n + 1),
        is_workspace: true,
        unique_ancestors: 0,
        children: Vec::new(),
        enabled_features: Vec::new(),
        available_features: BTreeMap::new(),
    });
    for (i, children) in out.iter().enumerate() {
        let mut sorted = children.clone();
        sorted.sort_by(|&a, &b| {
            tree_nodes[b]
                .transitive_weight
                .cmp(&tree_nodes[a].transitive_weight)
        });
        tree_nodes[i].children = sorted;
    }

    let mut tree_edges: Vec<DepTreeEdge> = graph
        .edges
        .iter()
        .map(|e| DepTreeEdge {
            from: e.from,
            to: e.to,
            is_optional: e.optional,
            gating_feature: e.gating_feature.clone(),
            enabled_child_features: e.child_features.clone(),
        })
        .collect();
    for &r in roots {
        tree_edges.push(DepTreeEdge {
            from: n,
            to: r,
            is_optional: false,
            gating_feature: None,
            enabled_child_features: Vec::new(),
        });
    }

    let dep_tree = DepTreeData {
        nodes: tree_nodes,
        root_indices: vec![n],
        edges: tree_edges,
        // A flat crate list carries no target information.
        platforms: Vec::new(),
    };

    AnalysisReport {
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        timestamp: now_timestamp(),
        workspace_root: format!("dependency list: {root_name}"),
        threshold,
        total_dependencies: total_deps,
        platform_dependencies: None,
        phantom_dependencies: 0,
        heavy_nodes_found,
        targets: Vec::new(),
        dep_tree: Some(dep_tree),
        unused_edges: Vec::new(),
        unused_direct_deps: Vec::new(),
        direct_dep_summary,
        transitive_sharing,
    }
}

/// Number of distinct nodes that transitively depend on `start`.
fn count_ancestors(start: usize, rev: &[Vec<usize>]) -> usize {
    let mut visited = vec![false; rev.len()];
    let mut queue = VecDeque::new();
    for &p in &rev[start] {
        if !visited[p] {
            visited[p] = true;
            queue.push_back(p);
        }
    }
    let mut count = 0;
    while let Some(cur) = queue.pop_front() {
        count += 1;
        for &p in &rev[cur] {
            if !visited[p] {
                visited[p] = true;
                queue.push_back(p);
            }
        }
    }
    count
}

fn now_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("epoch:{secs}")
}

// ---------------------------------------------------------------------------
// Registry index fetching + command entry point (network: remote feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "remote")]
mod fetch {
    use super::IndexVersion;
    use anyhow::{Context, Result};
    use rayon::prelude::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const USER_AGENT: &str = "cargo-depflame (https://github.com/sinelaw/cargo-depflame)";
    const INDEX_BASE: &str = "https://index.crates.io";

    fn index_url(name: &str) -> String {
        let n = name.to_lowercase();
        let prefix = match n.len() {
            1 => "1".to_string(),
            2 => "2".to_string(),
            3 => format!("3/{}", &n[..1]),
            _ => format!("{}/{}", &n[..2], &n[2..4]),
        };
        format!("{INDEX_BASE}/{prefix}/{n}")
    }

    /// Fetch all published versions (with dependency metadata) for one crate
    /// from the sparse index. Unknown crates (404) yield an empty list.
    fn fetch_one(agent: &ureq::Agent, name: &str) -> Result<Vec<IndexVersion>> {
        let url = index_url(name);
        let mut last_err = None;
        for attempt in 0..3u64 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(300 * attempt));
            }
            match agent.get(&url).set("User-Agent", USER_AGENT).call() {
                Ok(resp) => {
                    let body = resp
                        .into_string()
                        .with_context(|| format!("failed to read index response for {name}"))?;
                    return body
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .map(|l| {
                            serde_json::from_str::<IndexVersion>(l)
                                .with_context(|| format!("failed to parse index entry for {name}"))
                        })
                        .collect();
                }
                Err(ureq::Error::Status(404, _)) => return Ok(Vec::new()),
                Err(e) => last_err = Some(e),
            }
        }
        Err(anyhow::Error::from(last_err.expect("retries exhausted")))
            .with_context(|| format!("failed to fetch registry index for {name}"))
    }

    /// Fetch index metadata for all names in parallel.
    pub fn fetch_index(names: &[String]) -> Result<HashMap<String, Vec<IndexVersion>>> {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(30))
            .build();
        let done = AtomicUsize::new(0);
        let total = names.len();
        names
            .par_iter()
            .map(|name| {
                let result = fetch_one(&agent, name);
                let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                if d % 100 == 0 || d == total {
                    eprintln!("  fetched {d}/{total}");
                }
                result.map(|versions| (name.clone(), versions))
            })
            .collect()
    }
}

/// Run the `from-list` command: parse the list, fetch index metadata,
/// reverse-engineer the minimal root set and assemble the report.
#[cfg(feature = "remote")]
pub fn run_from_list(args: &crate::cli::FromListArgs) -> anyhow::Result<AnalysisReport> {
    use anyhow::Context;

    let text = if args.input == "-" {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
            .context("failed to read dependency list from stdin")?;
        buf
    } else {
        std::fs::read_to_string(&args.input)
            .with_context(|| format!("failed to read {}", args.input))?
    };

    let (crates, warnings) = parse_dep_list(&text);
    for w in &warnings {
        eprintln!("  [WARN] {w}");
    }
    if crates.is_empty() {
        anyhow::bail!("no crates parsed from input");
    }
    eprintln!("Parsed {} crates from list", crates.len());

    let mut names: Vec<String> = crates.iter().map(|c| c.name.clone()).collect();
    names.sort_unstable();
    names.dedup();
    eprintln!(
        "Fetching registry index metadata for {} crates...",
        names.len()
    );
    let index = fetch::fetch_index(&names)?;

    let opts = ListGraphOptions {
        include_build: args.include_build,
        skip_optional: args.skip_optional,
    };
    let graph = build_list_graph(crates, &index, &opts);
    for w in &graph.warnings {
        eprintln!("  [WARN] {w}");
    }

    let (roots, root_warnings) = minimal_roots(graph.crates.len(), &graph.edges);
    for w in &root_warnings {
        eprintln!("  [WARN] {w}");
    }
    eprintln!(
        "Minimal root set: {} of {} crates transitively cover the whole list",
        roots.len(),
        graph.crates.len()
    );

    let root_name = args.root_name.clone().unwrap_or_else(|| {
        if args.input == "-" {
            "dep-list".to_string()
        } else {
            std::path::Path::new(&args.input)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "dep-list".to_string())
        }
    });

    Ok(build_report(
        &root_name,
        &graph,
        &roots,
        args.heavy_threshold,
        0.0,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lc(name: &str, version: &str) -> ListedCrate {
        ListedCrate {
            name: name.to_string(),
            version: Version::parse(version).unwrap(),
        }
    }

    fn index_version(vers: &str, deps: Vec<IndexDep>) -> IndexVersion {
        IndexVersion {
            vers: vers.to_string(),
            deps,
            features: BTreeMap::new(),
            features2: None,
        }
    }

    fn dep(name: &str, req: &str) -> IndexDep {
        IndexDep {
            name: name.to_string(),
            req: req.to_string(),
            features: Vec::new(),
            optional: false,
            kind: None,
            package: None,
        }
    }

    // ── parse_line ────────────────────────────────────────────────────────

    #[test]
    fn parse_simple_name_version() {
        assert_eq!(parse_line("serde-1.0.228"), Some(lc("serde", "1.0.228")));
    }

    #[test]
    fn parse_multi_segment_name() {
        assert_eq!(
            parse_line("zstd-safe-7.2.4"),
            Some(lc("zstd-safe", "7.2.4"))
        );
        assert_eq!(
            parse_line("aws-smithy-runtime-api-1.9.1"),
            Some(lc("aws-smithy-runtime-api", "1.9.1"))
        );
    }

    #[test]
    fn parse_name_with_digits() {
        assert_eq!(
            parse_line("html5ever-0.26.0"),
            Some(lc("html5ever", "0.26.0"))
        );
        assert_eq!(parse_line("base64-0.22.1"), Some(lc("base64", "0.22.1")));
    }

    #[test]
    fn parse_at_and_space_separators() {
        assert_eq!(parse_line("serde@1.0.228"), Some(lc("serde", "1.0.228")));
        assert_eq!(parse_line("serde 1.0.228"), Some(lc("serde", "1.0.228")));
    }

    #[test]
    fn parse_prerelease_version() {
        assert_eq!(
            parse_line("foo-1.0.0-alpha.1"),
            Some(lc("foo", "1.0.0-alpha.1"))
        );
    }

    #[test]
    fn parse_rejects_garbage() {
        assert_eq!(parse_line("== crates.io (434) =="), None);
        assert_eq!(parse_line("nucleo (git rev 5b74652; fuzzy matcher)"), None);
        assert_eq!(parse_line("just-a-name"), None);
    }

    #[test]
    fn parse_dep_list_dedups_and_warns() {
        let (crates, warnings) = parse_dep_list("a-1.0.0\n\nnot a crate!\na-1.0.0\nb-2.0.0\n");
        assert_eq!(crates, vec![lc("a", "1.0.0"), lc("b", "2.0.0")]);
        assert_eq!(warnings.len(), 1);
    }

    // ── build_list_graph ──────────────────────────────────────────────────

    #[test]
    fn graph_resolves_req_to_highest_listed_match() {
        // a depends on b "^1"; both b 1.1.0 and 1.2.0 are listed.
        let crates = vec![lc("a", "1.0.0"), lc("b", "1.1.0"), lc("b", "1.2.0")];
        let mut index = HashMap::new();
        index.insert(
            "a".to_string(),
            vec![index_version("1.0.0", vec![dep("b", "^1")])],
        );
        index.insert(
            "b".to_string(),
            vec![
                index_version("1.1.0", vec![]),
                index_version("1.2.0", vec![]),
            ],
        );
        let g = build_list_graph(
            crates,
            &index,
            &ListGraphOptions {
                include_build: false,
                skip_optional: false,
            },
        );
        assert_eq!(g.edges.len(), 1);
        assert_eq!((g.edges[0].from, g.edges[0].to), (0, 2));
    }

    #[test]
    fn graph_respects_exact_pins_and_incompatible_majors() {
        let crates = vec![lc("a", "1.0.0"), lc("b", "0.1.0"), lc("b", "0.2.5")];
        let mut index = HashMap::new();
        index.insert(
            "a".to_string(),
            vec![index_version("1.0.0", vec![dep("b", "^0.1")])],
        );
        index.insert("b".to_string(), Vec::new());
        let g = build_list_graph(
            crates,
            &index,
            &ListGraphOptions {
                include_build: false,
                skip_optional: false,
            },
        );
        // ^0.1 must not match 0.2.5.
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].to, 1);
    }

    #[test]
    fn graph_skips_dev_deps_and_honors_flags() {
        let mut d_dev = dep("b", "^1");
        d_dev.kind = Some("dev".to_string());
        let mut d_build = dep("c", "^1");
        d_build.kind = Some("build".to_string());
        let mut d_opt = dep("d", "^1");
        d_opt.optional = true;

        let crates = vec![
            lc("a", "1.0.0"),
            lc("b", "1.0.0"),
            lc("c", "1.0.0"),
            lc("d", "1.0.0"),
        ];
        let mut index = HashMap::new();
        index.insert(
            "a".to_string(),
            vec![index_version("1.0.0", vec![d_dev, d_build, d_opt])],
        );
        for name in ["b", "c", "d"] {
            index.insert(name.to_string(), vec![index_version("1.0.0", vec![])]);
        }

        let g = build_list_graph(
            crates.clone(),
            &index,
            &ListGraphOptions {
                include_build: false,
                skip_optional: false,
            },
        );
        // Only the optional normal dep d survives (dev skipped, build off).
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].to, 3);
        assert!(g.edges[0].optional);

        let g = build_list_graph(
            crates,
            &index,
            &ListGraphOptions {
                include_build: true,
                skip_optional: true,
            },
        );
        // Now only the build dep c survives.
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].to, 2);
        assert!(g.edges[0].build_only);
    }

    #[test]
    fn graph_resolves_renamed_deps() {
        let mut renamed = dep("b_alias", "^1");
        renamed.package = Some("b".to_string());
        let crates = vec![lc("a", "1.0.0"), lc("b", "1.0.0")];
        let mut index = HashMap::new();
        index.insert("a".to_string(), vec![index_version("1.0.0", vec![renamed])]);
        index.insert("b".to_string(), vec![index_version("1.0.0", vec![])]);
        let g = build_list_graph(
            crates,
            &index,
            &ListGraphOptions {
                include_build: false,
                skip_optional: false,
            },
        );
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].to, 1);
    }

    // ── minimal_roots ─────────────────────────────────────────────────────

    #[test]
    fn roots_simple_dag() {
        // 0 -> 1 -> 2, 3 -> 2 : roots are {0, 3}
        let edges: Vec<ListEdge> = [(0, 1), (1, 2), (3, 2)]
            .iter()
            .map(|&(from, to)| ListEdge {
                from,
                to,
                optional: false,
                build_only: false,
                gating_feature: None,
                child_features: Vec::new(),
            })
            .collect();
        let (roots, warnings) = minimal_roots(4, &edges);
        assert_eq!(roots, vec![0, 3]);
        assert!(warnings.is_empty());
    }

    #[test]
    fn roots_cycle_gets_extra_root() {
        // 0 -> 1, and a detached cycle 2 <-> 3.
        let edges: Vec<ListEdge> = [(0, 1), (2, 3), (3, 2)]
            .iter()
            .map(|&(from, to)| ListEdge {
                from,
                to,
                optional: false,
                build_only: false,
                gating_feature: None,
                child_features: Vec::new(),
            })
            .collect();
        let (roots, warnings) = minimal_roots(4, &edges);
        assert_eq!(roots.len(), 2);
        assert!(roots.contains(&0));
        assert!(roots.contains(&2) || roots.contains(&3));
        assert_eq!(warnings.len(), 1);
    }

    // ── build_report ──────────────────────────────────────────────────────

    #[test]
    fn report_has_synthetic_root_and_full_coverage() {
        let crates = vec![lc("a", "1.0.0"), lc("b", "1.0.0"), lc("c", "1.0.0")];
        let mut index = HashMap::new();
        index.insert(
            "a".to_string(),
            vec![index_version("1.0.0", vec![dep("b", "^1")])],
        );
        index.insert(
            "b".to_string(),
            vec![index_version("1.0.0", vec![dep("c", "^1")])],
        );
        index.insert("c".to_string(), vec![index_version("1.0.0", vec![])]);
        let g = build_list_graph(
            crates,
            &index,
            &ListGraphOptions {
                include_build: false,
                skip_optional: false,
            },
        );
        let (roots, _) = minimal_roots(g.crates.len(), &g.edges);
        assert_eq!(roots, vec![0]);

        let report = build_report("mylist", &g, &roots, 10, 0.0);
        assert_eq!(report.total_dependencies, 3);

        let tree = report.dep_tree.expect("dep tree present");
        assert_eq!(tree.nodes.len(), 4);
        assert_eq!(tree.root_indices, vec![3]);
        let root = &tree.nodes[3];
        assert_eq!(root.name, "mylist");
        assert!(root.is_workspace);
        // Synthetic root covers everything: weight = all crates + itself.
        assert_eq!(root.transitive_weight, 4);
        assert_eq!(root.children, vec![0]);

        // Direct-dep summary lists exactly the minimal roots.
        assert_eq!(report.direct_dep_summary.len(), 1);
        assert_eq!(report.direct_dep_summary[0].dep_name, "a");
        assert_eq!(report.direct_dep_summary[0].total_transitive_deps, 2);

        // Sharing: one scope (the synthetic root), a single top-level dep, so
        // every crate in the chain a -> b -> c has exactly that one owner.
        assert_eq!(report.transitive_sharing.len(), 1);
        let scope = &report.transitive_sharing[0];
        assert_eq!(scope.scope, "workspace");
        assert!(scope.is_workspace);
        assert_eq!(scope.total_direct_deps, 1);
        assert_eq!(scope.deps.len(), 3);
        for entry in &scope.deps {
            assert_eq!(entry.owner_count, 1, "{} should have one owner", entry.name);
            assert_eq!(entry.owners, vec!["a".to_string()]);
        }
        assert!(scope.deps.iter().any(|d| d.name == "a" && d.is_direct));
        assert!(scope.deps.iter().any(|d| d.name == "c" && !d.is_direct));
    }

    #[test]
    fn sharing_counts_owners_across_multiple_roots() {
        // Two independent roots that share a leaf:
        //   a -> shared, b -> shared
        let crates = vec![lc("a", "1.0.0"), lc("b", "1.0.0"), lc("shared", "1.0.0")];
        let mut index = HashMap::new();
        index.insert(
            "a".to_string(),
            vec![index_version("1.0.0", vec![dep("shared", "^1")])],
        );
        index.insert(
            "b".to_string(),
            vec![index_version("1.0.0", vec![dep("shared", "^1")])],
        );
        index.insert("shared".to_string(), vec![index_version("1.0.0", vec![])]);
        let g = build_list_graph(
            crates,
            &index,
            &ListGraphOptions {
                include_build: false,
                skip_optional: false,
            },
        );
        let (roots, _) = minimal_roots(g.crates.len(), &g.edges);

        let report = build_report("mylist", &g, &roots, 10, 0.0);
        let scope = &report.transitive_sharing[0];
        assert_eq!(scope.total_direct_deps, 2);

        let shared = scope
            .deps
            .iter()
            .find(|d| d.name == "shared")
            .expect("shared crate present");
        assert_eq!(shared.owner_count, 2);
        assert_eq!(shared.owners, vec!["a".to_string(), "b".to_string()]);

        // Loosely unique first: the single-owner roots sort ahead of `shared`.
        assert_eq!(scope.deps.last().unwrap().name, "shared");
    }
}
