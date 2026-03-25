mod layout;
mod render;

use crate::graph::DepGraph;
use cargo_metadata::PackageId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;

// ---------------------------------------------------------------------------
// Serializable dep-tree (embedded in JSON report so `report` subcommand can
// re-render as SVG without re-running analysis).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DepTreeData {
    pub nodes: Vec<DepTreeNode>,
    pub root_indices: Vec<usize>,
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
}

// ---------------------------------------------------------------------------
// Build the serializable tree from the live DepGraph.
// ---------------------------------------------------------------------------

pub fn build_dep_tree(graph: &DepGraph) -> DepTreeData {
    let mut nodes: Vec<DepTreeNode> = Vec::new();
    let mut id_to_idx: HashMap<PackageId, usize> = HashMap::new();

    // First pass: create all nodes.
    for (id, node) in &graph.nodes {
        let idx = nodes.len();
        id_to_idx.insert(id.clone(), idx);
        nodes.push(DepTreeNode {
            name: node.name.clone(),
            version: node.version.clone(),
            transitive_weight: node.transitive_weight,
            is_workspace: node.is_workspace_member,
            children: Vec::new(),
        });
    }

    // Second pass: wire up children (sorted by weight desc for determinism).
    for (id, deps) in &graph.forward {
        if let Some(&parent_idx) = id_to_idx.get(id) {
            let mut child_indices: Vec<usize> = deps
                .iter()
                .filter_map(|dep_id| id_to_idx.get(dep_id).copied())
                .collect();
            child_indices
                .sort_by(|&a, &b| nodes[b].transitive_weight.cmp(&nodes[a].transitive_weight));
            nodes[parent_idx].children = child_indices;
        }
    }

    let mut root_indices: Vec<usize> = graph
        .workspace_members
        .iter()
        .filter_map(|id| id_to_idx.get(id).copied())
        .collect();
    root_indices.sort_by(|&a, &b| nodes[b].transitive_weight.cmp(&nodes[a].transitive_weight));

    DepTreeData {
        nodes,
        root_indices,
    }
}

// ---------------------------------------------------------------------------
// Public rendering entry points — delegate to render module.
// ---------------------------------------------------------------------------

pub fn render_flamegraph(
    tree: &DepTreeData,
    total_deps: usize,
    writer: &mut dyn Write,
) -> anyhow::Result<()> {
    render_flamegraph_with_unused(tree, total_deps, &[], writer)
}

/// Render with unused edge highlighting. Each entry in `unused_edges` is
/// a `(parent_name, dep_name)` pair where the parent doesn't reference the dep.
pub fn render_flamegraph_with_unused(
    tree: &DepTreeData,
    total_deps: usize,
    unused_edges: &[(String, String)],
    writer: &mut dyn Write,
) -> anyhow::Result<()> {
    render::render_svg(tree, total_deps, unused_edges, writer)
}
