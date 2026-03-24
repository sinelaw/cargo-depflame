use cargo_metadata::{DependencyKind, Metadata, PackageId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

// ---------------------------------------------------------------------------
// Serializable dep-tree (embedded in JSON report so `report` subcommand can
// re-render as SVG without re-running analysis).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DepTreeData {
    pub nodes: Vec<DepTreeNode>,
    pub root_indices: Vec<usize>,
    /// Per-edge metadata for feature gating.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<DepTreeEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepTreeNode {
    pub name: String,
    pub version: String,
    /// Total transitive dep count (including self).
    pub transitive_weight: usize,
    pub is_workspace: bool,
    /// Indices into `DepTreeData::nodes`.
    pub children: Vec<usize>,
    /// Features currently enabled for this package in the resolved graph.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enabled_features: Vec<String>,
    /// All available features: feature_name -> list of what it activates.
    /// Entries can be `"dep:foo"`, `"foo/bar"`, or `"bar"` (sub-feature).
    /// Uses BTreeMap for deterministic JSON serialization order.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub available_features: BTreeMap<String, Vec<String>>,
}

/// Metadata about an edge in the dependency tree for feature gating.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepTreeEdge {
    /// Index of the parent node in `DepTreeData::nodes`.
    pub from: usize,
    /// Index of the child node in `DepTreeData::nodes`.
    pub to: usize,
    /// Whether this edge only exists because a feature is enabled on the parent.
    pub is_optional: bool,
    /// The feature name on the parent that gates this edge, if optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gating_feature: Option<String>,
    /// Features the parent enables on this child dependency.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enabled_child_features: Vec<String>,
}

// ---------------------------------------------------------------------------
// Build the serializable tree from cargo metadata.
//
// `full_metadata`:   resolved with --all-features (complete graph including
//                    every optional dep).
// `active_metadata`: resolved with the user's actual feature selections
//                    (used only to populate `enabled_features`).
// ---------------------------------------------------------------------------

pub fn build_dep_tree(full_metadata: &Metadata, active_metadata: &Metadata) -> DepTreeData {
    let full_resolve = match full_metadata.resolve.as_ref() {
        Some(r) => r,
        None => return DepTreeData::default(),
    };

    // Active resolve: features the user actually has enabled.
    let active_features: HashMap<&PackageId, &Vec<String>> = active_metadata
        .resolve
        .as_ref()
        .map(|r| r.nodes.iter().map(|n| (&n.id, &n.features)).collect())
        .unwrap_or_default();

    let workspace_members: HashSet<&PackageId> = full_metadata.workspace_members.iter().collect();

    let pkg_by_id: HashMap<&PackageId, &cargo_metadata::Package> =
        full_metadata.packages.iter().map(|p| (&p.id, p)).collect();

    // ── Pass 1: create nodes ──────────────────────────────────────────────

    let mut nodes: Vec<DepTreeNode> = Vec::new();
    let mut id_to_idx: HashMap<PackageId, usize> = HashMap::new();
    // Forward edges from the full resolve (parent → [children]).
    let mut forward: HashMap<PackageId, Vec<PackageId>> = HashMap::new();

    for rnode in &full_resolve.nodes {
        let pkg = match pkg_by_id.get(&rnode.id) {
            Some(p) => p,
            None => continue,
        };

        let idx = nodes.len();
        id_to_idx.insert(rnode.id.clone(), idx);

        let mut enabled_features: Vec<String> = active_features
            .get(&rnode.id)
            .map(|f| f.to_vec())
            .unwrap_or_default();
        enabled_features.sort();

        let available_features: BTreeMap<String, Vec<String>> = pkg
            .features
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        nodes.push(DepTreeNode {
            name: pkg.name.clone(),
            version: pkg.version.to_string(),
            transitive_weight: 0, // computed below
            is_workspace: workspace_members.contains(&rnode.id),
            children: Vec::new(),
            enabled_features,
            available_features,
        });

        // Collect forward edges (normal + build deps only).
        let mut deps: Vec<PackageId> = Vec::new();
        for dep_info in &rnode.deps {
            let dominated_by_normal_or_build = dep_info
                .dep_kinds
                .iter()
                .any(|dk| matches!(dk.kind, DependencyKind::Normal | DependencyKind::Build));
            if dominated_by_normal_or_build {
                deps.push(dep_info.pkg.clone());
            }
        }
        forward.insert(rnode.id.clone(), deps);
    }

    // ── Compute transitive weights via BFS ────────────────────────────────

    let ids: Vec<PackageId> = id_to_idx.keys().cloned().collect();
    let mut weight_cache: HashMap<PackageId, usize> = HashMap::new();

    for id in &ids {
        if weight_cache.contains_key(id) {
            continue;
        }
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(id.clone());
        visited.insert(id.clone());
        while let Some(cur) = queue.pop_front() {
            if let Some(deps) = forward.get(&cur) {
                for dep in deps {
                    if visited.insert(dep.clone()) {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }
        weight_cache.insert(id.clone(), visited.len());
    }

    for (id, &weight) in &weight_cache {
        if let Some(&idx) = id_to_idx.get(id) {
            nodes[idx].transitive_weight = weight;
        }
    }

    // ── Pass 2: wire children + build edge metadata ───────────────────────

    let mut edges: Vec<DepTreeEdge> = Vec::new();

    for (id, deps) in &forward {
        let Some(&parent_idx) = id_to_idx.get(id) else {
            continue;
        };
        let parent_pkg = pkg_by_id.get(id);

        let mut child_indices: Vec<usize> = deps
            .iter()
            .filter_map(|dep_id| id_to_idx.get(dep_id).copied())
            .collect();
        child_indices.sort_by(|&a, &b| nodes[b].transitive_weight.cmp(&nodes[a].transitive_weight));
        nodes[parent_idx].children = child_indices;

        for dep_id in deps {
            let Some(&child_idx) = id_to_idx.get(dep_id) else {
                continue;
            };
            let child_name = &nodes[child_idx].name;

            let dep_decl =
                parent_pkg.and_then(|pkg| pkg.dependencies.iter().find(|d| d.name == *child_name));

            let is_optional = dep_decl.is_some_and(|d| d.optional);

            let gating_feature = if is_optional {
                parent_pkg.and_then(|pkg| {
                    let dep_entry = format!("dep:{child_name}");
                    pkg.features.iter().find_map(|(feat_name, activates)| {
                        if activates
                            .iter()
                            .any(|a| a == &dep_entry || a == child_name.as_str())
                        {
                            Some(feat_name.clone())
                        } else {
                            None
                        }
                    })
                })
            } else {
                None
            };

            // Collect features the parent enables on this child (from the full resolve).
            let mut enabled_child_features: Vec<String> = Vec::new();
            if let Some(d) = dep_decl {
                enabled_child_features.extend(d.features.clone());
            }
            if let Some(pkg) = parent_pkg {
                let parent_enabled: Vec<&String> = full_resolve
                    .nodes
                    .iter()
                    .find(|n| &n.id == id)
                    .map(|n| n.features.iter().collect())
                    .unwrap_or_default();
                let prefix = format!("{child_name}/");
                for feat_name in &parent_enabled {
                    if let Some(activates) = pkg.features.get(feat_name.as_str()) {
                        for entry in activates {
                            if let Some(child_feat) = entry.strip_prefix(&prefix) {
                                if !enabled_child_features.contains(&child_feat.to_string()) {
                                    enabled_child_features.push(child_feat.to_string());
                                }
                            }
                        }
                    }
                }
            }
            enabled_child_features.sort();
            enabled_child_features.dedup();

            edges.push(DepTreeEdge {
                from: parent_idx,
                to: child_idx,
                is_optional,
                gating_feature,
                enabled_child_features,
            });
        }
    }

    // ── Root indices ──────────────────────────────────────────────────────

    let mut root_indices: Vec<usize> = workspace_members
        .iter()
        .filter_map(|id| id_to_idx.get(id).copied())
        .collect();
    root_indices.sort_by(|&a, &b| nodes[b].transitive_weight.cmp(&nodes[a].transitive_weight));

    DepTreeData {
        nodes,
        root_indices,
        edges,
    }
}
