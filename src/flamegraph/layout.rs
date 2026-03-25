use super::DepTreeData;
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

pub(super) const CHART_WIDTH: f64 = 1200.0;
pub(super) const ROW_HEIGHT: f64 = 18.0;
pub(super) const ROW_GAP: f64 = 1.0;
pub(super) const ROW_TOTAL: f64 = ROW_HEIGHT + ROW_GAP;
pub(super) const MIN_RECT_WIDTH: f64 = 2.0;
pub(super) const MAX_DEPTH: usize = 40;
pub(super) const CHAR_WIDTH: f64 = 6.5;
pub(super) const TEXT_PAD: f64 = 4.0;
pub(super) const HEADER_HEIGHT: f64 = 40.0;
pub(super) const FOOTER_HEIGHT: f64 = 30.0;
/// Safety limit on total frames to keep SVG size reasonable.
pub(super) const MAX_FRAMES: usize = 8000;

// ---------------------------------------------------------------------------
// Internal layout representation.
// ---------------------------------------------------------------------------

pub(super) struct LayoutRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub name: String,
    pub version: String,
    pub weight: usize,
    pub depth: usize,
    pub is_shared: bool,
    pub is_workspace: bool,
    /// Name of the parent node (empty for roots).
    pub parent_name: String,
    pub ancestor_count: usize,
    /// Number of children that were too small to render (collapsed).
    pub collapsed_children: usize,
}

// ---------------------------------------------------------------------------
// Layout algorithm.
//
// Handles the DAG (shared deps with multiple ancestors) using the same
// strategy as flamegraphs handle merged call stacks:
//
// - Each node is expanded under EVERY parent that depends on it (not just
//   the first one encountered).  This lets the user see the full "blame
//   path" from every angle.
// - To prevent exponential blowup on diamond dependencies, we use:
//   (a) per-path cycle detection (a node is only skipped if it's an
//       ancestor of itself on the CURRENT path — true cycles are rare in
//       cargo but possible with dev-deps in metadata);
//   (b) a global frame budget (MAX_FRAMES) — once hit, remaining children
//       are collapsed into an "[N more]" placeholder.
//   (c) minimum pixel-width cutoff — narrow slices are collapsed.
// - Nodes with ancestor_count > 1 are marked `is_shared` and rendered in a
//   distinct colour so the user can immediately spot diamond deps.
// ---------------------------------------------------------------------------

pub(super) fn compute_ancestor_counts(tree: &DepTreeData) -> Vec<usize> {
    let mut counts = vec![0usize; tree.nodes.len()];
    for node in &tree.nodes {
        for &child_idx in &node.children {
            counts[child_idx] += 1;
        }
    }
    counts
}

pub(super) fn layout(tree: &DepTreeData) -> Vec<LayoutRect> {
    let mut rects = Vec::new();
    let ancestor_counts = compute_ancestor_counts(tree);

    let total_weight: f64 = tree
        .root_indices
        .iter()
        .map(|&idx| tree.nodes[idx].transitive_weight as f64)
        .sum();

    if total_weight == 0.0 {
        return rects;
    }

    let mut x = 0.0;
    let mut path = HashSet::new();
    for &root_idx in &tree.root_indices {
        let w = (tree.nodes[root_idx].transitive_weight as f64 / total_weight) * CHART_WIDTH;
        layout_node(
            tree,
            root_idx,
            x,
            0,
            w,
            &mut rects,
            &mut path,
            &ancestor_counts,
            "",
        );
        x += w;
    }

    rects
}

fn layout_node(
    tree: &DepTreeData,
    node_idx: usize,
    x: f64,
    depth: usize,
    width: f64,
    rects: &mut Vec<LayoutRect>,
    path: &mut HashSet<usize>,
    ancestor_counts: &[usize],
    parent_name: &str,
) {
    if width < MIN_RECT_WIDTH || depth > MAX_DEPTH || rects.len() >= MAX_FRAMES {
        return;
    }

    // Cycle detection on the current path only (not global).
    if !path.insert(node_idx) {
        return;
    }

    let node = &tree.nodes[node_idx];
    let is_shared = ancestor_counts[node_idx] > 1;

    let rect_idx = rects.len();
    rects.push(LayoutRect {
        x,
        y: depth as f64 * ROW_TOTAL + HEADER_HEIGHT,
        w: width,
        name: node.name.clone(),
        version: node.version.clone(),
        weight: node.transitive_weight,
        depth,
        is_shared,
        is_workspace: node.is_workspace,
        parent_name: parent_name.to_string(),
        ancestor_count: ancestor_counts[node_idx],
        collapsed_children: 0,
    });

    if node.children.is_empty() {
        path.remove(&node_idx);
        return;
    }

    let child_total: f64 = node
        .children
        .iter()
        .map(|&c| tree.nodes[c].transitive_weight as f64)
        .sum();
    if child_total == 0.0 {
        path.remove(&node_idx);
        return;
    }

    let mut cx = x;
    let mut collapsed = 0usize;
    for &child_idx in &node.children {
        if rects.len() >= MAX_FRAMES {
            collapsed += 1;
            continue;
        }
        let cw = (tree.nodes[child_idx].transitive_weight as f64 / child_total) * width;
        if cw < MIN_RECT_WIDTH {
            collapsed += 1;
            continue;
        }
        layout_node(
            tree,
            child_idx,
            cx,
            depth + 1,
            cw,
            rects,
            path,
            ancestor_counts,
            &node.name,
        );
        cx += cw;
    }

    if collapsed > 0 {
        rects[rect_idx].collapsed_children = collapsed;
    }

    path.remove(&node_idx);
}
