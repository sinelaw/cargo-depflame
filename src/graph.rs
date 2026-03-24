use anyhow::{anyhow, Result};
use cargo_metadata::{DependencyKind, Metadata, PackageId};
use std::collections::{HashMap, HashSet, VecDeque};

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

/// A heavy node: a non-workspace dependency with large transitive weight.
#[derive(Debug, Clone)]
pub struct HeavyNode {
    pub id: PackageId,
    pub name: String,
    pub version: String,
    pub transitive_weight: usize,
}

/// A candidate pair for heuristic scanning: an intermediate crate I depends
/// on a heavy dependency F.
#[derive(Debug, Clone)]
pub struct IntermediateEdge {
    pub intermediate_id: PackageId,
    pub intermediate_name: String,
    pub intermediate_version: String,
    pub heavy_id: PackageId,
    pub heavy_name: String,
    pub heavy_version: String,
    pub heavy_transitive_weight: usize,
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
    pub fn from_metadata(metadata: &Metadata) -> Result<Self> {
        let resolve = metadata
            .resolve
            .as_ref()
            .ok_or_else(|| anyhow!("no dependency resolution graph found"))?;

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
                    matches!(dk.kind, DependencyKind::Normal | DependencyKind::Build)
                        && dk.target.is_some()
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

    /// Compute the "unique subtree weight" for an edge (intermediate -> heavy):
    /// How many transitive deps of the heavy dep would be removed from the entire workspace
    /// if this single edge were cut?
    pub fn unique_subtree_weight(
        &self,
        intermediate_id: &PackageId,
        heavy_id: &PackageId,
    ) -> usize {
        let heavy_set = match self.nodes.get(heavy_id) {
            Some(n) => &n.transitive_set,
            None => return 0,
        };

        // Build the set of all deps reachable from workspace WITHOUT
        // traversing the (intermediate -> heavy) edge.
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
                    if &current == intermediate_id && dep == heavy_id {
                        continue;
                    }
                    if reachable_without.insert(dep.clone()) {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        // Count how many of heavy's transitive deps are NOT reachable without this edge.
        heavy_set
            .iter()
            .filter(|dep| !reachable_without.contains(*dep))
            .count()
    }

    /// Compute shortest dependency chains from workspace members to ALL nodes
    /// via a single BFS. Returns a map from PackageId to the chain (as crate names).
    pub fn all_dependency_chains(&self) -> HashMap<PackageId, Vec<String>> {
        let mut parent: HashMap<PackageId, PackageId> = HashMap::new();
        let mut queue = VecDeque::new();
        let mut visited = HashSet::new();

        for ws_id in &self.workspace_members {
            if visited.insert(ws_id.clone()) {
                queue.push_back(ws_id.clone());
            }
        }

        while let Some(current) = queue.pop_front() {
            if let Some(deps) = self.forward.get(&current) {
                for dep in deps {
                    if visited.insert(dep.clone()) {
                        parent.insert(dep.clone(), current.clone());
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        // Build chains for all reachable nodes.
        let mut chains = HashMap::new();
        for id in &visited {
            let mut path = vec![id.clone()];
            let mut cur = id.clone();
            while let Some(p) = parent.get(&cur) {
                path.push(p.clone());
                cur = p.clone();
            }
            path.reverse();

            let chain: Vec<String> = path
                .iter()
                .filter_map(|pid| self.nodes.get(pid).map(|n| n.name.clone()))
                .collect();
            chains.insert(id.clone(), chain);
        }

        chains
    }

    /// Check if any sibling dependency of `intermediate_id` transitively
    /// depends on `heavy_id`. If so, the heavy dep is required even if intermediate
    /// doesn't reference it in source code.
    /// Returns the name of the sibling that requires it, if any.
    pub fn sibling_requires(
        &self,
        intermediate_id: &PackageId,
        heavy_id: &PackageId,
    ) -> Option<String> {
        let siblings = self.forward.get(intermediate_id)?;

        for sibling_id in siblings {
            if sibling_id == heavy_id {
                continue;
            }
            if let Some(sibling_node) = self.nodes.get(sibling_id) {
                if sibling_node.transitive_set.contains(heavy_id) {
                    return Some(sibling_node.name.clone());
                }
            }
        }
        None
    }

    /// Get the number of *direct* dependencies a node has (excluding itself).
    pub fn direct_dep_count(&self, id: &PackageId) -> usize {
        self.forward.get(id).map(|deps| deps.len()).unwrap_or(0)
    }

    /// Check if a workspace member is a standalone integration crate:
    /// no other workspace member depends on it. This means it's already
    /// effectively opt-in — users only get it if they explicitly add it.
    pub fn is_standalone_workspace_member(&self, id: &PackageId) -> bool {
        if !self.workspace_members.contains(id) {
            return false;
        }
        // Check if any other workspace member depends on this crate.
        match self.reverse.get(id) {
            None => true, // No reverse deps at all.
            Some(dependents) => {
                // If the only dependents are non-workspace members, it's standalone.
                !dependents
                    .iter()
                    .any(|dep_id| self.workspace_members.contains(dep_id))
            }
        }
    }

    /// Find all non-workspace nodes with W_transitive > threshold.
    pub fn heavy_nodes(&self, threshold: usize) -> Vec<HeavyNode> {
        self.nodes
            .iter()
            .filter(|(_, node)| !node.is_workspace_member && node.transitive_weight > threshold)
            .map(|(id, node)| HeavyNode {
                id: id.clone(),
                name: node.name.clone(),
                version: node.version.clone(),
                transitive_weight: node.transitive_weight,
            })
            .collect()
    }

    /// For each heavy node F, find crates I such that:
    /// - I depends on F directly
    /// - I is reachable from a workspace member (including workspace members themselves)
    pub fn intermediate_edges(&self, heavy_nodes: &[HeavyNode]) -> Vec<IntermediateEdge> {
        let heavy_ids: HashSet<&PackageId> = heavy_nodes.iter().map(|f| &f.id).collect();
        let mut edges = Vec::new();

        let default_meta = EdgeMeta {
            build_only: false,
            already_optional: false,
            platform_conditional: false,
        };

        for (id, deps) in &self.forward {
            let node = match self.nodes.get(id) {
                Some(n) => n,
                None => continue,
            };

            // Workspace members are trivially reachable; others need a check.
            if !node.is_workspace_member && !self.is_reachable_from_workspace(id) {
                continue;
            }

            for dep_id in deps {
                if heavy_ids.contains(dep_id) {
                    if let Some(heavy_node) = self.nodes.get(dep_id) {
                        let meta = self
                            .edge_meta
                            .get(&(id.clone(), dep_id.clone()))
                            .cloned()
                            .unwrap_or_else(|| default_meta.clone());

                        edges.push(IntermediateEdge {
                            intermediate_id: id.clone(),
                            intermediate_name: node.name.clone(),
                            intermediate_version: node.version.clone(),
                            heavy_id: dep_id.clone(),
                            heavy_name: heavy_node.name.clone(),
                            heavy_version: heavy_node.version.clone(),
                            heavy_transitive_weight: heavy_node.transitive_weight,
                            edge_meta: meta,
                        });
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

#[cfg(test)]
mod tests {
    use super::*;
    use cargo_metadata::PackageId;

    /// Create a PackageId from a short name.
    fn pid(name: &str) -> PackageId {
        PackageId {
            repr: format!("{name} 1.0.0 (path+file:///test/{name})"),
        }
    }

    /// Build a DepGraph from an adjacency list and a list of workspace member names.
    /// Each edge is (from_name, to_name). All referenced names become nodes.
    /// Workspace members get `is_workspace_member = true`.
    fn test_graph(edges: &[(&str, &str)], workspace: &[&str]) -> DepGraph {
        let ws_set: HashSet<String> = workspace.iter().map(|s| s.to_string()).collect();

        // Collect all node names.
        let mut names: HashSet<String> = HashSet::new();
        for (from, to) in edges {
            names.insert(from.to_string());
            names.insert(to.to_string());
        }
        for w in workspace {
            names.insert(w.to_string());
        }

        // Build nodes.
        let mut nodes = HashMap::new();
        for name in &names {
            let id = pid(name);
            nodes.insert(
                id,
                DepNode {
                    name: name.clone(),
                    version: "1.0.0".to_string(),
                    is_workspace_member: ws_set.contains(name.as_str()),
                    transitive_weight: 0,
                    transitive_set: HashSet::new(),
                },
            );
        }

        // Build forward and reverse adjacency.
        let mut forward: HashMap<PackageId, Vec<PackageId>> = HashMap::new();
        let mut reverse: HashMap<PackageId, Vec<PackageId>> = HashMap::new();
        let mut edge_meta_map: HashMap<(PackageId, PackageId), EdgeMeta> = HashMap::new();

        // Initialize forward for all nodes (even leaves).
        for name in &names {
            forward.entry(pid(name)).or_default();
        }

        for (from, to) in edges {
            let from_id = pid(from);
            let to_id = pid(to);
            forward
                .entry(from_id.clone())
                .or_default()
                .push(to_id.clone());
            reverse
                .entry(to_id.clone())
                .or_default()
                .push(from_id.clone());
            edge_meta_map.insert(
                (from_id, to_id),
                EdgeMeta {
                    build_only: false,
                    already_optional: false,
                    platform_conditional: false,
                },
            );
        }

        let workspace_members: HashSet<PackageId> = workspace.iter().map(|w| pid(w)).collect();

        let mut graph = DepGraph {
            nodes,
            forward,
            reverse,
            workspace_members,
            edge_meta: edge_meta_map,
        };
        graph.compute_transitive_weights();
        graph
    }

    // ---------------------------------------------------------------
    // transitive_weight / transitive_set
    // ---------------------------------------------------------------

    #[test]
    fn transitive_weight_single_node() {
        let g = test_graph(&[], &["A"]);
        assert_eq!(g.nodes[&pid("A")].transitive_weight, 1); // includes self
    }

    #[test]
    fn transitive_weight_chain() {
        // A -> B -> C
        let g = test_graph(&[("A", "B"), ("B", "C")], &["A"]);
        assert_eq!(g.nodes[&pid("A")].transitive_weight, 3);
        assert_eq!(g.nodes[&pid("B")].transitive_weight, 2);
        assert_eq!(g.nodes[&pid("C")].transitive_weight, 1);
    }

    #[test]
    fn transitive_weight_diamond() {
        // A -> B -> D, A -> C -> D
        let g = test_graph(&[("A", "B"), ("A", "C"), ("B", "D"), ("C", "D")], &["A"]);
        // A reaches {A, B, C, D} = 4
        assert_eq!(g.nodes[&pid("A")].transitive_weight, 4);
        // B reaches {B, D} = 2
        assert_eq!(g.nodes[&pid("B")].transitive_weight, 2);
        // D is a leaf = 1
        assert_eq!(g.nodes[&pid("D")].transitive_weight, 1);
    }

    #[test]
    fn transitive_set_contains_self() {
        let g = test_graph(&[("A", "B")], &["A"]);
        assert!(g.nodes[&pid("A")].transitive_set.contains(&pid("A")));
        assert!(g.nodes[&pid("A")].transitive_set.contains(&pid("B")));
    }

    // ---------------------------------------------------------------
    // unique_subtree_weight
    // ---------------------------------------------------------------

    #[test]
    fn unique_subtree_simple_chain() {
        // WS -> A -> B -> C
        // Cutting WS->A should remove A, B, C (weight = 3)
        let g = test_graph(&[("WS", "A"), ("A", "B"), ("B", "C")], &["WS"]);
        assert_eq!(g.unique_subtree_weight(&pid("WS"), &pid("A")), 3);
    }

    #[test]
    fn unique_subtree_diamond_shared_dep() {
        // WS -> A -> D, WS -> B -> D
        // Cutting WS->A: D is still reachable via B, so only A is unique.
        let g = test_graph(&[("WS", "A"), ("WS", "B"), ("A", "D"), ("B", "D")], &["WS"]);
        assert_eq!(g.unique_subtree_weight(&pid("WS"), &pid("A")), 1);
    }

    #[test]
    fn unique_subtree_diamond_with_unique_tail() {
        // WS -> A -> B -> D, WS -> C -> D
        // A's subtree is {A, B, D}. D still reachable via C.
        // Cutting WS->A removes A and B (2).
        let g = test_graph(
            &[("WS", "A"), ("WS", "C"), ("A", "B"), ("B", "D"), ("C", "D")],
            &["WS"],
        );
        assert_eq!(g.unique_subtree_weight(&pid("WS"), &pid("A")), 2);
    }

    #[test]
    fn unique_subtree_no_shared_deps() {
        // WS -> A -> B, WS -> C -> D
        // Cutting WS->A removes A and B (2).
        let g = test_graph(&[("WS", "A"), ("WS", "C"), ("A", "B"), ("C", "D")], &["WS"]);
        assert_eq!(g.unique_subtree_weight(&pid("WS"), &pid("A")), 2);
    }

    #[test]
    fn unique_subtree_complex_diamond() {
        // WS -> I -> F -> G -> H, WS -> J -> G
        // Cutting I->F: F's subtree = {F, G, H}.
        // Without I->F edge: G is reachable via J, H is reachable via J->G->H.
        // So only F is unique.
        let g = test_graph(
            &[
                ("WS", "I"),
                ("WS", "J"),
                ("I", "F"),
                ("F", "G"),
                ("G", "H"),
                ("J", "G"),
            ],
            &["WS"],
        );
        assert_eq!(g.unique_subtree_weight(&pid("I"), &pid("F")), 1);
    }

    #[test]
    fn unique_subtree_nonexistent_heavy() {
        let g = test_graph(&[("WS", "A")], &["WS"]);
        assert_eq!(g.unique_subtree_weight(&pid("WS"), &pid("nonexistent")), 0);
    }

    // ---------------------------------------------------------------
    // all_dependency_chains
    // ---------------------------------------------------------------

    #[test]
    fn dependency_chains_simple() {
        // WS -> A -> B
        let g = test_graph(&[("WS", "A"), ("A", "B")], &["WS"]);
        let chains = g.all_dependency_chains();

        // WS is a root, its chain should be just ["WS"]
        assert_eq!(chains[&pid("WS")], vec!["WS".to_string()]);

        // B should have chain WS -> A -> B
        let b_chain = &chains[&pid("B")];
        assert_eq!(b_chain.len(), 3);
        assert_eq!(b_chain[0], "WS");
        assert_eq!(b_chain[b_chain.len() - 1], "B");
    }

    #[test]
    fn dependency_chains_multiple_workspace_members() {
        // WS1 -> A, WS2 -> A -> B
        let g = test_graph(&[("WS1", "A"), ("WS2", "A"), ("A", "B")], &["WS1", "WS2"]);
        let chains = g.all_dependency_chains();

        // A should be reachable from some workspace member, chain length 2
        let a_chain = &chains[&pid("A")];
        assert_eq!(a_chain.len(), 2);
    }

    // ---------------------------------------------------------------
    // sibling_requires
    // ---------------------------------------------------------------

    #[test]
    fn sibling_requires_found() {
        // I -> S -> F, I -> F
        // S is a sibling of F under I, and S transitively needs F.
        let g = test_graph(&[("WS", "I"), ("I", "S"), ("I", "F"), ("S", "F")], &["WS"]);
        let result = g.sibling_requires(&pid("I"), &pid("F"));
        assert_eq!(result, Some("S".to_string()));
    }

    #[test]
    fn sibling_requires_not_found() {
        // I -> S, I -> F (S does not depend on F)
        let g = test_graph(&[("WS", "I"), ("I", "S"), ("I", "F")], &["WS"]);
        let result = g.sibling_requires(&pid("I"), &pid("F"));
        assert_eq!(result, None);
    }

    #[test]
    fn sibling_requires_transitive() {
        // I -> S -> X -> F, I -> F
        // S transitively depends on F through X.
        let g = test_graph(
            &[("WS", "I"), ("I", "S"), ("I", "F"), ("S", "X"), ("X", "F")],
            &["WS"],
        );
        let result = g.sibling_requires(&pid("I"), &pid("F"));
        assert_eq!(result, Some("S".to_string()));
    }

    // ---------------------------------------------------------------
    // heavy_nodes
    // ---------------------------------------------------------------

    #[test]
    fn heavy_nodes_threshold() {
        // WS -> A -> B -> C, WS -> D
        // A has transitive_weight=3, D has 1, B has 2, C has 1
        let g = test_graph(&[("WS", "A"), ("A", "B"), ("B", "C"), ("WS", "D")], &["WS"]);
        // threshold=1: nodes with weight > 1 that are NOT workspace members
        let heavy = g.heavy_nodes(1);
        let names: HashSet<String> = heavy.iter().map(|h| h.name.clone()).collect();
        assert!(names.contains("A")); // weight 3
        assert!(names.contains("B")); // weight 2
        assert!(!names.contains("C")); // weight 1, not > 1
        assert!(!names.contains("D")); // weight 1, not > 1
        assert!(!names.contains("WS")); // workspace member excluded
    }

    #[test]
    fn heavy_nodes_excludes_workspace() {
        // Even if a workspace member has high weight, it should be excluded.
        let g = test_graph(&[("WS", "A"), ("A", "B")], &["WS"]);
        let heavy = g.heavy_nodes(0);
        let names: HashSet<String> = heavy.iter().map(|h| h.name.clone()).collect();
        assert!(!names.contains("WS"));
        assert!(names.contains("A")); // weight 2 > 0
        assert!(names.contains("B")); // weight 1 > 0
    }

    // ---------------------------------------------------------------
    // intermediate_edges
    // ---------------------------------------------------------------

    #[test]
    fn intermediate_edges_basic() {
        // WS -> I -> F, F has high weight
        // I should appear as an intermediate to F.
        let g = test_graph(&[("WS", "I"), ("I", "F"), ("F", "X"), ("F", "Y")], &["WS"]);
        let heavy = g.heavy_nodes(1); // F has weight 3 (F, X, Y)
        let edges = g.intermediate_edges(&heavy);

        assert!(
            edges
                .iter()
                .any(|e| e.intermediate_name == "I" && e.heavy_name == "F"),
            "expected I->F intermediate edge"
        );
    }

    #[test]
    fn intermediate_edges_workspace_member_as_intermediate() {
        // WS -> F directly, F is heavy
        let g = test_graph(&[("WS", "F"), ("F", "X"), ("F", "Y")], &["WS"]);
        let heavy = g.heavy_nodes(1);
        let edges = g.intermediate_edges(&heavy);

        assert!(
            edges
                .iter()
                .any(|e| e.intermediate_name == "WS" && e.heavy_name == "F"),
            "workspace member can be an intermediate"
        );
    }

    #[test]
    fn intermediate_edges_unreachable_excluded() {
        // Isolated node Z -> F. Z is not reachable from any workspace member.
        // WS -> A (separate component)
        let g = test_graph(&[("WS", "A"), ("Z", "F"), ("F", "X"), ("F", "Y")], &["WS"]);
        let heavy = g.heavy_nodes(1);
        let edges = g.intermediate_edges(&heavy);

        assert!(
            !edges.iter().any(|e| e.intermediate_name == "Z"),
            "unreachable node Z should not appear"
        );
    }

    // ---------------------------------------------------------------
    // is_standalone_workspace_member
    // ---------------------------------------------------------------

    #[test]
    fn standalone_workspace_no_reverse_deps() {
        let g = test_graph(&[("WS", "A")], &["WS"]);
        assert!(g.is_standalone_workspace_member(&pid("WS")));
    }

    #[test]
    fn standalone_workspace_depended_by_another_ws() {
        // WS1 -> WS2 -> A. WS2 is depended on by WS1 (another workspace member).
        let g = test_graph(&[("WS1", "WS2"), ("WS2", "A")], &["WS1", "WS2"]);
        assert!(!g.is_standalone_workspace_member(&pid("WS2")));
        assert!(g.is_standalone_workspace_member(&pid("WS1")));
    }

    #[test]
    fn standalone_non_workspace_returns_false() {
        let g = test_graph(&[("WS", "A")], &["WS"]);
        assert!(!g.is_standalone_workspace_member(&pid("A")));
    }

    #[test]
    fn standalone_only_non_workspace_dependents() {
        // WS -> A, B -> WS (B is NOT a workspace member)
        // WS has a reverse dep from B, but B is not a workspace member, so standalone.
        let g = test_graph(&[("WS", "A"), ("B", "WS")], &["WS"]);
        assert!(g.is_standalone_workspace_member(&pid("WS")));
    }

    // ---------------------------------------------------------------
    // total_dependency_count
    // ---------------------------------------------------------------

    #[test]
    fn total_dependency_count_basic() {
        let g = test_graph(&[("WS", "A"), ("A", "B"), ("B", "C")], &["WS"]);
        // Non-workspace nodes: A, B, C = 3
        assert_eq!(g.total_dependency_count(), 3);
    }

    #[test]
    fn total_dependency_count_multiple_workspace() {
        let g = test_graph(&[("WS1", "A"), ("WS2", "B")], &["WS1", "WS2"]);
        // Non-workspace: A, B = 2
        assert_eq!(g.total_dependency_count(), 2);
    }

    #[test]
    fn total_dependency_count_all_workspace() {
        let g = test_graph(&[("WS1", "WS2")], &["WS1", "WS2"]);
        assert_eq!(g.total_dependency_count(), 0);
    }

    // ---------------------------------------------------------------
    // direct_dep_count
    // ---------------------------------------------------------------

    #[test]
    fn direct_dep_count_basic() {
        let g = test_graph(&[("A", "B"), ("A", "C"), ("B", "D")], &["A"]);
        assert_eq!(g.direct_dep_count(&pid("A")), 2);
        assert_eq!(g.direct_dep_count(&pid("B")), 1);
        assert_eq!(g.direct_dep_count(&pid("D")), 0);
    }
}
