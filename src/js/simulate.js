// ---------------------------------------------------------------------------
// depflame — removal simulator.
//
// Lets the Table tab mark direct dependencies of workspace members as
// "removed", then recomputes what the remaining direct deps would be worth:
// their unique transitive dep count, how many top-level deps still pull each
// crate in, and how many crates are left in the graph.
//
// Everything is computed from the dep tree under the *current* feature
// selection (the same active graph the flamegraph renders), so toggling a
// feature and simulating a removal compose.
//
// The key identity: a crate is unique to direct dep D exactly when D is its
// only owner. So one BFS per remaining direct dep yields both the owner counts
// and the unique counts.
// ---------------------------------------------------------------------------

var DepflameSimulate = (function() {
  'use strict';

  // Node indices marked as removed, keyed by index.
  var removed = {};
  // Result of a removal-free run, for "was N" annotations.
  var baseline = null;

  function treeData() {
    if (window.__DEPFLAME_DATA__) return window.__DEPFLAME_DATA__;
    var r = window.__DEPFLAME_REPORT__;
    return (r && r.dep_tree) || null;
  }

  // Active nodes/edges under the current feature selection. Falls back to
  // "every edge is active" if the feature engine isn't loaded.
  function activeGraph(tree) {
    if (typeof DepflameFeatures !== 'undefined' && DepflameFeatures.recomputeActiveGraph) {
      // No cuts and no weights: the simulator filters the root set itself, and
      // only needs to know which edges are live.
      return DepflameFeatures.recomputeActiveGraph(tree, null, true);
    }
    var activeEdges = {};
    for (var i = 0; i < tree.nodes.length; i++) {
      var children = tree.nodes[i].children || [];
      for (var c = 0; c < children.length; c++) activeEdges[i + ':' + children[c]] = true;
    }
    return { activeEdges: activeEdges, activeNodes: null };
  }

  // Non-workspace crates that a workspace member depends on directly. These
  // are the only rows the simulator will let you remove.
  function directDeps(tree, activeEdges) {
    var nodes = tree.nodes;
    var seen = {};
    var out = [];
    for (var i = 0; i < nodes.length; i++) {
      if (!nodes[i].is_workspace) continue;
      var children = nodes[i].children || [];
      for (var c = 0; c < children.length; c++) {
        var ci = children[c];
        if (nodes[ci].is_workspace) continue;
        if (!activeEdges[i + ':' + ci]) continue;
        if (seen[ci]) continue;
        seen[ci] = true;
        out.push(ci);
      }
    }
    return out;
  }

  // Everything reachable from `start` over active edges, including itself.
  // Removals never cut an edge inside a subtree — only the workspace edge
  // above it — so subtrees are independent of what's marked removed.
  function subtreeOf(tree, activeEdges, start) {
    var nodes = tree.nodes;
    var visited = {};
    var queue = [start];
    visited[start] = true;
    while (queue.length > 0) {
      var cur = queue.shift();
      var children = nodes[cur].children || [];
      for (var c = 0; c < children.length; c++) {
        var ci = children[c];
        if (!visited[ci] && activeEdges[cur + ':' + ci]) {
          visited[ci] = true;
          queue.push(ci);
        }
      }
    }
    return visited;
  }

  // Run the simulation for a given set of removed indices.
  //
  // Returns:
  //   rows      idx -> { unique, owners, subtree, removed, stillPulledBy }
  //   totalDeps non-workspace crates left in the graph
  //   direct    every removable node index, in table order
  function run(removedSet) {
    var tree = treeData();
    if (!tree || !tree.nodes || tree.nodes.length === 0) {
      return { rows: {}, totalDeps: 0, direct: [], removedCount: 0 };
    }

    var graph = activeGraph(tree);
    var activeEdges = graph.activeEdges;
    var direct = directDeps(tree, activeEdges);

    var subtrees = {};
    var owners = {};
    var live = [];
    for (var i = 0; i < direct.length; i++) {
      var idx = direct[i];
      subtrees[idx] = subtreeOf(tree, activeEdges, idx);
      if (removedSet[idx]) continue;
      live.push(idx);
      for (var key in subtrees[idx]) {
        if (!owners[key]) owners[key] = [];
        owners[key].push(idx);
      }
    }

    var rows = {};
    for (var i = 0; i < direct.length; i++) {
      var idx = direct[i];
      if (removedSet[idx]) {
        // A removed dep can survive: another direct dep may still pull it in.
        var pulledBy = (owners[idx] || []).map(function(o) { return tree.nodes[o].name; });
        rows[idx] = {
          removed: true,
          stillPulledBy: pulledBy,
          unique: 0,
          owners: pulledBy.length,
          subtree: Object.keys(subtrees[idx]).length - 1
        };
        continue;
      }
      var unique = 0;
      for (var key in subtrees[idx]) {
        if (owners[key] && owners[key].length === 1) unique++;
      }
      rows[idx] = {
        removed: false,
        stillPulledBy: [],
        unique: unique,
        owners: (owners[idx] || []).length,
        subtree: Object.keys(subtrees[idx]).length - 1
      };
    }

    var totalDeps = 0;
    for (var key in owners) {
      if (!tree.nodes[key].is_workspace) totalDeps++;
    }

    return {
      rows: rows,
      totalDeps: totalDeps,
      direct: direct,
      removedCount: live.length < direct.length ? direct.length - live.length : 0
    };
  }

  // Recompute the removal-free baseline. Call after anything that changes the
  // active graph (feature toggles), not just at startup.
  function refreshBaseline() {
    baseline = run({});
    return baseline;
  }

  function init() {
    removed = {};
    return refreshBaseline();
  }

  function getBaseline() {
    if (!baseline) refreshBaseline();
    return baseline;
  }

  function current() {
    return run(removed);
  }

  // Node index of a direct dep by name+version, or -1 if this crate isn't a
  // direct dependency of a workspace member (and so can't be removed).
  function indexFor(name, version) {
    var tree = treeData();
    if (!tree) return -1;
    var base = getBaseline();
    for (var i = 0; i < base.direct.length; i++) {
      var node = tree.nodes[base.direct[i]];
      if (node.name === name && (!version || node.version === version)) {
        return base.direct[i];
      }
    }
    return -1;
  }

  // Edges to cut so the rest of the report (flamegraph, summary bar) sees the
  // same graph the table does: every workspace edge into a removed dep.
  function cutEdges() {
    var tree = treeData();
    var cuts = {};
    if (!tree) return cuts;
    var nodes = tree.nodes;
    for (var i = 0; i < nodes.length; i++) {
      if (!nodes[i].is_workspace) continue;
      var children = nodes[i].children || [];
      for (var c = 0; c < children.length; c++) {
        if (removed[children[c]]) cuts[i + ':' + children[c]] = true;
      }
    }
    return cuts;
  }

  function toggle(idx) {
    idx = parseInt(idx, 10);
    if (removed[idx]) delete removed[idx];
    else removed[idx] = true;
    return !!removed[idx];
  }

  function isRemoved(idx) {
    return !!removed[parseInt(idx, 10)];
  }

  function removedIndices() {
    return Object.keys(removed).map(Number);
  }

  function hasRemovals() {
    return removedIndices().length > 0;
  }

  function clear() {
    removed = {};
  }

  return {
    init: init,
    refreshBaseline: refreshBaseline,
    baseline: getBaseline,
    current: current,
    indexFor: indexFor,
    cutEdges: cutEdges,
    toggle: toggle,
    isRemoved: isRemoved,
    removedIndices: removedIndices,
    hasRemovals: hasRemovals,
    clear: clear
  };
})();
