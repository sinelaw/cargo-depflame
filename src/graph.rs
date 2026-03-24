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
    /// Set of all transitive dependency PackageIds (including self).
    pub transitive_set: HashSet<PackageId>,
}

/// Metadata about an edge: how the dependency is declared.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EdgeMeta {
    /// Is this a build-only dependency?
    pub build_only: bool,
    /// Is this dependency declared as optional?
    pub already_optional: bool,
    /// Is this dependency platform-conditional?
    pub platform_conditional: bool,
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
    pub edge_meta: EdgeMeta,
}

/// The full dependency graph.
pub struct DepGraph {
    pub nodes: HashMap<PackageId, DepNode>,
    /// forward: package -> its dependencies
    pub forward: HashMap<PackageId, Vec<PackageId>>,
    /// reverse: package -> packages that depend on it
    pub reverse: HashMap<PackageId, Vec<PackageId>>,
    pub workspace_members: HashSet<PackageId>,
    /// Per-edge metadata: (from, to) -> EdgeMeta
    pub edge_meta: HashMap<(PackageId, PackageId), EdgeMeta>,
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
        let mut edge_meta_map: HashMap<(PackageId, PackageId), EdgeMeta> = HashMap::new();

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
                    transitive_set: HashSet::new(),
                },
            );

            let mut deps = Vec::new();

            for dep_info in &node.deps {
                let has_normal = dep_info
                    .dep_kinds
                    .iter()
                    .any(|dk| dk.kind == DependencyKind::Normal);
                let has_build = dep_info
                    .dep_kinds
                    .iter()
                    .any(|dk| dk.kind == DependencyKind::Build);

                if !has_normal && !has_build {
                    continue;
                }

                let build_only = has_build && !has_normal;

                // Check if any dep_kind has a target (platform-conditional).
                let platform_conditional = dep_info.dep_kinds.iter().all(|dk| {
                    matches!(
                        dk.kind,
                        DependencyKind::Normal | DependencyKind::Build
                    ) && dk.target.is_some()
                });

                deps.push(dep_info.pkg.clone());

                edge_meta_map.insert(
                    (node.id.clone(), dep_info.pkg.clone()),
                    EdgeMeta {
                        build_only,
                        already_optional: false, // Will be enriched later from Cargo.toml
                        platform_conditional,
                    },
                );
            }

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
            edge_meta: edge_meta_map,
        };
        graph.compute_transitive_weights();
        Ok(graph)
    }

    /// Enrich edge metadata with optional/platform info from Cargo.toml parsing.
    pub fn enrich_edge_meta(&mut self, from_id: &PackageId, to_package_name: &str, meta: &crate::cargo_toml::CrateDepInfo) {
        if let Some(edge) = self.forward.get(from_id) {
            for to_id in edge {
                if let Some(to_node) = self.nodes.get(to_id) {
                    if to_node.name == to_package_name {
                        if let Some(em) = self.edge_meta.get_mut(&(from_id.clone(), to_id.clone())) {
                            if meta.is_optional(to_package_name) {
                                em.already_optional = true;
                            }
                            if meta.is_platform_conditional(to_package_name) {
                                em.platform_conditional = true;
                            }
                            if meta.is_build_dep(to_package_name) {
                                em.build_only = true;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Compute W_transitive for every node via BFS, storing the full set.
    fn compute_transitive_weights(&mut self) {
        let ids: Vec<PackageId> = self.nodes.keys().cloned().collect();
        let mut cache: HashMap<PackageId, HashSet<PackageId>> = HashMap::new();

        for id in &ids {
            Self::transitive_set(id, &self.forward, &mut cache);
        }

        for (id, set) in &cache {
            if let Some(node) = self.nodes.get_mut(id) {
                node.transitive_weight = set.len();
                node.transitive_set = set.clone();
            }
        }
    }

    fn transitive_set(
        id: &PackageId,
        forward: &HashMap<PackageId, Vec<PackageId>>,
        cache: &mut HashMap<PackageId, HashSet<PackageId>>,
    ) -> HashSet<PackageId> {
        if let Some(cached) = cache.get(id) {
            return cached.clone();
        }

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

        cache.insert(id.clone(), visited.clone());
        visited
    }

    /// Compute the "unique subtree weight" for an edge (intermediate -> fat):
    /// How many transitive deps of `fat` would be removed from the entire workspace
    /// if this single edge were cut?
    pub fn unique_subtree_weight(
        &self,
        intermediate_id: &PackageId,
        fat_id: &PackageId,
    ) -> usize {
        let fat_set = match self.nodes.get(fat_id) {
            Some(n) => &n.transitive_set,
            None => return 0,
        };

        // Build the set of all deps reachable from workspace WITHOUT
        // traversing the (intermediate -> fat) edge.
        let mut reachable_without = HashSet::new();
        let mut queue = VecDeque::new();

        for ws_id in &self.workspace_members {
            if reachable_without.insert(ws_id.clone()) {
                queue.push_back(ws_id.clone());
            }
        }

        while let Some(current) = queue.pop_front() {
            if let Some(deps) = self.forward.get(&current) {
                for dep in deps {
                    // Skip the specific edge we're "cutting".
                    if &current == intermediate_id && dep == fat_id {
                        continue;
                    }
                    if reachable_without.insert(dep.clone()) {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        // Count how many of fat's transitive deps are NOT reachable without this edge.
        fat_set
            .iter()
            .filter(|dep| !reachable_without.contains(*dep))
            .count()
    }

    /// Find the shortest dependency chain from any workspace member to a target node.
    pub fn dependency_chain(&self, target: &PackageId) -> Vec<String> {
        // BFS from workspace members.
        let mut parent: HashMap<PackageId, PackageId> = HashMap::new();
        let mut queue = VecDeque::new();
        let mut visited = HashSet::new();

        for ws_id in &self.workspace_members {
            if visited.insert(ws_id.clone()) {
                queue.push_back(ws_id.clone());
            }
        }

        let mut found = false;
        while let Some(current) = queue.pop_front() {
            if &current == target {
                found = true;
                break;
            }
            if let Some(deps) = self.forward.get(&current) {
                for dep in deps {
                    if visited.insert(dep.clone()) {
                        parent.insert(dep.clone(), current.clone());
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        if !found {
            return Vec::new();
        }

        // Reconstruct path.
        let mut path = vec![target.clone()];
        let mut cur = target.clone();
        while let Some(p) = parent.get(&cur) {
            path.push(p.clone());
            cur = p.clone();
        }
        path.reverse();

        path.iter()
            .filter_map(|id| self.nodes.get(id).map(|n| n.name.clone()))
            .collect()
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

        let default_meta = EdgeMeta {
            build_only: false,
            already_optional: false,
            platform_conditional: false,
        };

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

            // Check if this node is reachable from any workspace member.
            if !self.is_reachable_from_workspace(id) {
                continue;
            }

            for dep_id in deps {
                if fat_ids.contains(dep_id) {
                    if let Some(fat_node) = self.nodes.get(dep_id) {
                        let meta = self
                            .edge_meta
                            .get(&(id.clone(), dep_id.clone()))
                            .cloned()
                            .unwrap_or_else(|| default_meta.clone());

                        edges.push(IntermediateEdge {
                            intermediate_id: id.clone(),
                            intermediate_name: node.name.clone(),
                            intermediate_version: node.version.clone(),
                            fat_id: dep_id.clone(),
                            fat_name: fat_node.name.clone(),
                            fat_version: fat_node.version.clone(),
                            fat_transitive_weight: fat_node.transitive_weight,
                            edge_meta: meta,
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
                            let meta = self
                                .edge_meta
                                .get(&(ws_id.clone(), dep_id.clone()))
                                .cloned()
                                .unwrap_or_else(|| default_meta.clone());

                            edges.push(IntermediateEdge {
                                intermediate_id: ws_id.clone(),
                                intermediate_name: ws_node.name.clone(),
                                intermediate_version: ws_node.version.clone(),
                                fat_id: dep_id.clone(),
                                fat_name: fat_node.name.clone(),
                                fat_version: fat_node.version.clone(),
                                fat_transitive_weight: fat_node.transitive_weight,
                                edge_meta: meta,
                            });
                        }
                    }
                }
            }
        }

        edges
    }

    fn is_reachable_from_workspace(&self, target: &PackageId) -> bool {
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
