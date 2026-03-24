use cargo_metadata::{DependencyKind, Metadata, PackageId};
use std::collections::{HashMap, HashSet, VecDeque};

use crate::error::TriageError;

/// A node in the dependency graph.
#[derive(Debug, Clone)]
pub struct DepNode {
    pub name: String,
    pub version: String,
    pub is_workspace_member: bool,
    /// Number of unique transitive dependencies (including self).
    pub transitive_weight: usize,
}

/// A fat node: a non-workspace dependency with large transitive weight.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FatNode {
    pub id: PackageId,
    pub name: String,
    pub version: String,
    pub transitive_weight: usize,
}

/// A candidate pair for heuristic scanning: an intermediate crate I depends
/// on a fat dependency F.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct IntermediateEdge {
    pub intermediate_id: PackageId,
    pub intermediate_name: String,
    pub intermediate_version: String,
    pub fat_id: PackageId,
    pub fat_name: String,
    pub fat_version: String,
    pub fat_transitive_weight: usize,
}

/// The full dependency graph.
pub struct DepGraph {
    pub nodes: HashMap<PackageId, DepNode>,
    /// forward: package -> its dependencies
    pub forward: HashMap<PackageId, Vec<PackageId>>,
    /// reverse: package -> packages that depend on it
    pub reverse: HashMap<PackageId, Vec<PackageId>>,
    pub workspace_members: HashSet<PackageId>,
}

impl DepGraph {
    /// Build the dependency graph from cargo_metadata output.
    pub fn from_metadata(metadata: &Metadata) -> Result<Self, TriageError> {
        let resolve = metadata.resolve.as_ref().ok_or(TriageError::NoResolveGraph)?;

        let workspace_members: HashSet<PackageId> =
            metadata.workspace_members.iter().cloned().collect();

        let pkg_map: HashMap<&PackageId, &cargo_metadata::Package> =
            metadata.packages.iter().map(|p| (&p.id, p)).collect();

        let mut nodes = HashMap::new();
        let mut forward: HashMap<PackageId, Vec<PackageId>> = HashMap::new();
        let mut reverse: HashMap<PackageId, Vec<PackageId>> = HashMap::new();

        for node in &resolve.nodes {
            let pkg = match pkg_map.get(&node.id) {
                Some(p) => p,
                None => continue,
            };

            nodes.insert(
                node.id.clone(),
                DepNode {
                    name: pkg.name.clone(),
                    version: pkg.version.to_string(),

                    is_workspace_member: workspace_members.contains(&node.id),
                    transitive_weight: 0,
                },
            );

            let deps: Vec<PackageId> = node
                .deps
                .iter()
                .filter(|d| {
                    d.dep_kinds.iter().any(|dk| {
                        matches!(dk.kind, DependencyKind::Normal | DependencyKind::Build)
                    })
                })
                .map(|d| d.pkg.clone())
                .collect();

            for dep_id in &deps {
                reverse
                    .entry(dep_id.clone())
                    .or_default()
                    .push(node.id.clone());
            }

            forward.insert(node.id.clone(), deps);
        }

        let mut graph = Self {
            nodes,
            forward,
            reverse,
            workspace_members,
        };
        graph.compute_transitive_weights();
        Ok(graph)
    }

    /// Compute W_transitive for every node via memoized DFS.
    fn compute_transitive_weights(&mut self) {
        let ids: Vec<PackageId> = self.nodes.keys().cloned().collect();
        let mut cache: HashMap<PackageId, usize> = HashMap::new();

        for id in &ids {
            Self::transitive_size(id, &self.forward, &mut cache);
        }

        for (id, weight) in &cache {
            if let Some(node) = self.nodes.get_mut(id) {
                node.transitive_weight = *weight;
            }
        }
    }

    fn transitive_size(
        id: &PackageId,
        forward: &HashMap<PackageId, Vec<PackageId>>,
        cache: &mut HashMap<PackageId, usize>,
    ) -> usize {
        if let Some(&cached) = cache.get(id) {
            return cached;
        }

        // BFS to find all unique transitive deps.
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(id.clone());
        visited.insert(id.clone());

        while let Some(current) = queue.pop_front() {
            if let Some(deps) = forward.get(&current) {
                for dep in deps {
                    if visited.insert(dep.clone()) {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        let size = visited.len(); // includes self
        cache.insert(id.clone(), size);
        size
    }

    /// Find all non-workspace nodes with W_transitive > threshold.
    pub fn fat_nodes(&self, threshold: usize) -> Vec<FatNode> {
        self.nodes
            .iter()
            .filter(|(id, node)| {
                !node.is_workspace_member
                    && node.transitive_weight > threshold
                    && !self.workspace_members.contains(*id)
            })
            .map(|(id, node)| FatNode {
                id: id.clone(),
                name: node.name.clone(),
                version: node.version.clone(),
                transitive_weight: node.transitive_weight,
            })
            .collect()
    }

    /// For each fat node F, find intermediate crates I such that:
    /// - I depends on F directly
    /// - I is not a workspace member
    /// - I is reachable from a workspace member
    pub fn intermediate_edges(&self, fat_nodes: &[FatNode]) -> Vec<IntermediateEdge> {
        let fat_ids: HashSet<&PackageId> = fat_nodes.iter().map(|f| &f.id).collect();
        let mut edges = Vec::new();

        // For each non-workspace node, check if any of its direct deps is a fat node.
        for (id, deps) in &self.forward {
            let node = match self.nodes.get(id) {
                Some(n) => n,
                None => continue,
            };

            // Skip workspace members — we want upstream targets.
            if node.is_workspace_member {
                continue;
            }

            // Check if this node is reachable from any workspace member
            // (i.e., it's actually in our dep tree, not an orphan).
            if !self.is_reachable_from_workspace(id) {
                continue;
            }

            for dep_id in deps {
                if fat_ids.contains(dep_id) {
                    if let Some(fat_node) = self.nodes.get(dep_id) {
                        edges.push(IntermediateEdge {
                            intermediate_id: id.clone(),
                            intermediate_name: node.name.clone(),
                            intermediate_version: node.version.clone(),
                            fat_id: dep_id.clone(),
                            fat_name: fat_node.name.clone(),
                            fat_version: fat_node.version.clone(),
                            fat_transitive_weight: fat_node.transitive_weight,
                        });
                    }
                }
            }
        }

        // Also check workspace members' direct deps on fat nodes.
        for ws_id in &self.workspace_members {
            if let Some(deps) = self.forward.get(ws_id) {
                for dep_id in deps {
                    if fat_ids.contains(dep_id) {
                        if let (Some(ws_node), Some(fat_node)) =
                            (self.nodes.get(ws_id), self.nodes.get(dep_id))
                        {
                            edges.push(IntermediateEdge {
                                intermediate_id: ws_id.clone(),
                                intermediate_name: ws_node.name.clone(),
                                intermediate_version: ws_node.version.clone(),
                                fat_id: dep_id.clone(),
                                fat_name: fat_node.name.clone(),
                                fat_version: fat_node.version.clone(),
                                fat_transitive_weight: fat_node.transitive_weight,
                            });
                        }
                    }
                }
            }
        }

        edges
    }

    fn is_reachable_from_workspace(&self, target: &PackageId) -> bool {
        // Walk reverse edges from target to see if we reach a workspace member.
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(target.clone());
        visited.insert(target.clone());

        while let Some(current) = queue.pop_front() {
            if self.workspace_members.contains(&current) {
                return true;
            }
            if let Some(parents) = self.reverse.get(&current) {
                for parent in parents {
                    if visited.insert(parent.clone()) {
                        queue.push_back(parent.clone());
                    }
                }
            }
        }
        false
    }

    pub fn total_dependency_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|(_, n)| !n.is_workspace_member)
            .count()
    }
}
