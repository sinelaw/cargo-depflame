// Tests for features.js — feature resolution and graph recomputation.
// Assumes: DepflameFeatures is loaded, `report` and test helpers are in scope.

var tree = JSON.parse(JSON.stringify(report.dep_tree)); // deep clone

test('recomputeActiveGraph with no overrides includes all nodes', function() {
  var result = DepflameFeatures.recomputeActiveGraph(tree);
  // All 15 nodes should be active (indices 0-14).
  var activeCount = Object.keys(result.activeNodes).length;
  assertEquals(activeCount, 15, 'all 15 nodes should be active with no overrides');
});

test('recomputeActiveGraph computes positive weights for active nodes', function() {
  var result = DepflameFeatures.recomputeActiveGraph(tree);
  // Root nodes should have positive weights.
  assert(result.weights[0] > 0, 'my-app should have positive weight');
  assert(result.weights[1] > 0, 'my-lib should have positive weight');
  // Leaf nodes should have weight 1 (just themselves).
  assertEquals(result.weights[6], 1, 'tiny-helper (leaf) should have weight 1');
  assertEquals(result.weights[10], 1, 'once_cell (leaf) should have weight 1');
});

test('disabling "derive" feature on serde removes serde_derive', function() {
  // serde (index 3) has feature "derive" that gates serde_derive (index 13).
  // By default, "derive" is enabled. Disable it.
  var treeCopy = JSON.parse(JSON.stringify(tree));

  // Override serde features: keep "default" and "std" but remove "derive".
  // We need to access the internal featureOverrides — use recomputeActiveGraph
  // with a modified tree where serde's enabled_features lacks "derive".
  treeCopy.nodes[3].enabled_features = ["default", "std"];

  var result = DepflameFeatures.recomputeActiveGraph(treeCopy);

  // serde_derive (index 13) should no longer be active.
  assertEquals(result.activeNodes[13], undefined,
    'serde_derive should be inactive when derive feature is disabled');

  // serde (index 3) should still be active.
  assert(result.activeNodes[3], 'serde should still be active');
});

test('disabling "async" feature on heavy-framework removes tokio from that path', function() {
  // heavy-framework (index 2) has "async" feature gating tokio (index 7).
  // But tokio is also reachable via http-client (index 8) with a non-optional edge.
  var treeCopy = JSON.parse(JSON.stringify(tree));

  // Remove "async" from heavy-framework's enabled features.
  treeCopy.nodes[2].enabled_features = ["default"];
  // Also remove "async" from default's activations to prevent re-enabling.
  treeCopy.nodes[2].available_features["default"] = [];

  var result = DepflameFeatures.recomputeActiveGraph(treeCopy);

  // tokio should STILL be active because http-client -> tokio is non-optional.
  assert(result.activeNodes[7], 'tokio should still be active via http-client');

  // heavy-framework should still be active.
  assert(result.activeNodes[2], 'heavy-framework should still be active');
});

test('removing all paths to tokio makes it inactive', function() {
  var treeCopy = JSON.parse(JSON.stringify(tree));

  // 1. Remove async feature from heavy-framework (removes heavy-framework -> tokio).
  treeCopy.nodes[2].enabled_features = ["default"];
  treeCopy.nodes[2].available_features["default"] = [];

  // 2. Make http-client -> tokio edge optional and ungated.
  for (var i = 0; i < treeCopy.edges.length; i++) {
    if (treeCopy.edges[i].from === 8 && treeCopy.edges[i].to === 7) {
      treeCopy.edges[i].is_optional = true;
      treeCopy.edges[i].gating_feature = "networking";
    }
  }
  // http-client has no "networking" feature enabled.
  treeCopy.nodes[8].enabled_features = [];

  var result = DepflameFeatures.recomputeActiveGraph(treeCopy);

  assertEquals(result.activeNodes[7], undefined,
    'tokio should be inactive when all paths are cut');
});

test('recomputeActiveGraph weight of my-app decreases when deps are removed', function() {
  var treeCopy = JSON.parse(JSON.stringify(tree));

  // Get baseline weight.
  var baseline = DepflameFeatures.recomputeActiveGraph(treeCopy);
  var baseWeight = baseline.weights[0]; // my-app

  // Now disable "derive" on serde to remove serde_derive.
  treeCopy.nodes[3].enabled_features = ["default", "std"];
  var modified = DepflameFeatures.recomputeActiveGraph(treeCopy);
  var newWeight = modified.weights[0];

  assert(newWeight < baseWeight,
    'my-app weight should decrease when serde_derive is removed: was ' + baseWeight + ', now ' + newWeight);
});

// -------------------------------------------------------------------------
// Transitive feature resolution within a node.
// -------------------------------------------------------------------------

test('transitive feature resolution: default -> async -> dep:tokio', function() {
  // heavy-framework (index 2) has: default -> [async], async -> [dep:tokio]
  // Enabling only "default" should transitively enable "async".
  var treeCopy = JSON.parse(JSON.stringify(tree));
  treeCopy.nodes[2].enabled_features = ["default"]; // only default, not async explicitly

  var result = DepflameFeatures.recomputeActiveGraph(treeCopy);

  // tokio should still be active because default -> async -> dep:tokio.
  assert(result.activeNodes[7], 'tokio should be active via transitive default -> async -> dep:tokio');
});

test('disabling default on heavy-framework breaks the async chain', function() {
  var treeCopy = JSON.parse(JSON.stringify(tree));
  // Give heavy-framework NO features — neither default nor async.
  treeCopy.nodes[2].enabled_features = [];

  var result = DepflameFeatures.recomputeActiveGraph(treeCopy);

  // The heavy-framework -> tokio edge is optional, gated by "async".
  // With no features enabled, async is not resolved, so tokio is not activated via this path.
  // But tokio is still reachable via http-client (non-optional edge).
  assert(result.activeNodes[7], 'tokio should still be active via http-client');

  // heavy-sub-a and heavy-sub-b are non-optional children of heavy-framework.
  assert(result.activeNodes[11], 'heavy-sub-a should be active (non-optional)');
  assert(result.activeNodes[12], 'heavy-sub-b should be active (non-optional)');
});

// -------------------------------------------------------------------------
// Cascading: disabling a feature on workspace root removes entire subtrees.
// -------------------------------------------------------------------------

test('disabling "default" on my-app removes heavy-framework and its subtree', function() {
  // my-app (index 0): default -> [heavy-framework] (optional edge gated by "default")
  var treeCopy = JSON.parse(JSON.stringify(tree));
  treeCopy.nodes[0].enabled_features = []; // disable all features

  var result = DepflameFeatures.recomputeActiveGraph(treeCopy);

  // heavy-framework (2) is optional, gated by "default" — should be gone.
  assertEquals(result.activeNodes[2], undefined, 'heavy-framework should be inactive');
  // heavy-sub-a (11) and heavy-sub-b (12) are children of heavy-framework — should be gone.
  assertEquals(result.activeNodes[11], undefined, 'heavy-sub-a should be inactive');
  assertEquals(result.activeNodes[12], undefined, 'heavy-sub-b should be inactive');

  // But tokio (7) is still reachable via http-client (8) -> tokio, non-optional.
  assert(result.activeNodes[7], 'tokio should still be active via http-client');

  // serde (3), unused-dep (4), http-client (8), test-helpers (9) are non-optional from my-app.
  assert(result.activeNodes[3], 'serde should still be active');
  assert(result.activeNodes[4], 'unused-dep should still be active');
  assert(result.activeNodes[8], 'http-client should still be active');
  assert(result.activeNodes[9], 'test-helpers should still be active');
});

test('cascading: removing heavy-framework reduces my-app weight by its exclusive deps', function() {
  var treeCopy = JSON.parse(JSON.stringify(tree));

  var baseline = DepflameFeatures.recomputeActiveGraph(treeCopy);
  var baseActive = Object.keys(baseline.activeNodes).length;

  // Disable "default" on my-app to remove heavy-framework subtree.
  treeCopy.nodes[0].enabled_features = [];
  var filtered = DepflameFeatures.recomputeActiveGraph(treeCopy);
  var filteredActive = Object.keys(filtered.activeNodes).length;

  // heavy-framework(2), heavy-sub-a(11), heavy-sub-b(12) should be removed = 3 nodes fewer.
  // tokio(7) stays via http-client.
  assertEquals(baseActive - filteredActive, 3,
    'should lose exactly 3 nodes (heavy-framework + 2 subs): was ' + baseActive + ' now ' + filteredActive);
});

// -------------------------------------------------------------------------
// Exact weight verification after filtering.
// -------------------------------------------------------------------------

test('exact weights: leaf nodes always have weight 1', function() {
  var result = DepflameFeatures.recomputeActiveGraph(tree);
  // All leaf nodes (no active children) should have weight 1.
  var leaves = [6, 9, 10, 11, 12, 13, 14]; // tiny-helper, test-helpers, once_cell, heavy-sub-a/b, serde_derive, dead-dep
  for (var i = 0; i < leaves.length; i++) {
    assertEquals(result.weights[leaves[i]], 1,
      tree.nodes[leaves[i]].name + ' (idx ' + leaves[i] + ') should have weight 1');
  }
});

test('exact weights: serde weight includes serde_derive when derive is enabled', function() {
  var result = DepflameFeatures.recomputeActiveGraph(tree);
  // serde(3) -> serde_derive(13). Weight should be 2 (serde + serde_derive).
  assertEquals(result.weights[3], 2, 'serde weight should be 2 (self + serde_derive)');
});

test('exact weights: serde weight is 1 when derive is disabled', function() {
  var treeCopy = JSON.parse(JSON.stringify(tree));
  treeCopy.nodes[3].enabled_features = ["default", "std"]; // no derive

  var result = DepflameFeatures.recomputeActiveGraph(treeCopy);
  assertEquals(result.weights[3], 1, 'serde weight should be 1 (only self, no serde_derive)');
});

test('exact weights: my-lib weight counts all its transitive deps', function() {
  var result = DepflameFeatures.recomputeActiveGraph(tree);
  // my-lib(1) -> regex(5), tiny-helper(6), once_cell(10). All are leaves.
  // Weight = 1 (self) + 1 + 1 + 1 = 4.
  assertEquals(result.weights[1], 4, 'my-lib weight should be 4 (self + 3 leaf deps)');
});

// -------------------------------------------------------------------------
// activeEdges tracking.
// -------------------------------------------------------------------------

test('activeEdges tracks which edges are live', function() {
  var result = DepflameFeatures.recomputeActiveGraph(tree);
  // Non-optional: my-app(0) -> serde(3) should be active.
  assert(result.activeEdges['0:3'], 'my-app -> serde edge should be active');
  // Optional + enabled: serde(3) -> serde_derive(13) should be active (derive is enabled).
  assert(result.activeEdges['3:13'], 'serde -> serde_derive edge should be active');
  // Optional + enabled: heavy-framework(2) -> tokio(7) should be active (async is enabled).
  assert(result.activeEdges['2:7'], 'heavy-framework -> tokio edge should be active');
});

test('activeEdges omits disabled optional edges', function() {
  var treeCopy = JSON.parse(JSON.stringify(tree));
  treeCopy.nodes[3].enabled_features = ["default", "std"]; // no derive

  var result = DepflameFeatures.recomputeActiveGraph(treeCopy);
  assertEquals(result.activeEdges['3:13'], undefined,
    'serde -> serde_derive edge should be inactive when derive is disabled');
  // Non-optional edges still active.
  assert(result.activeEdges['0:3'], 'my-app -> serde edge should still be active');
});

// -------------------------------------------------------------------------
// Flamegraph layout respects filtered active nodes.
// -------------------------------------------------------------------------

test('flamegraph layout excludes filtered-out nodes', function() {
  var treeCopy = JSON.parse(JSON.stringify(tree));
  treeCopy.nodes[0].enabled_features = []; // disable default on my-app

  var result = DepflameFeatures.recomputeActiveGraph(treeCopy);
  var rects = Depflame.layoutTree(treeCopy, result.activeNodes);

  var names = {};
  for (var i = 0; i < rects.length; i++) names[rects[i].name] = true;

  assertEquals(names['heavy-framework'], undefined, 'heavy-framework should not appear in layout');
  assertEquals(names['heavy-sub-a'], undefined, 'heavy-sub-a should not appear in layout');
  assertEquals(names['heavy-sub-b'], undefined, 'heavy-sub-b should not appear in layout');
  assert(names['serde'], 'serde should appear in layout');
  assert(names['my-app'], 'my-app should appear in layout');
  assert(names['tokio'], 'tokio should appear in layout (via http-client)');
});

test('flamegraph layout with all features disabled still shows non-optional deps', function() {
  var treeCopy = JSON.parse(JSON.stringify(tree));
  for (var i = 0; i < treeCopy.nodes.length; i++) {
    treeCopy.nodes[i].enabled_features = [];
  }

  var result = DepflameFeatures.recomputeActiveGraph(treeCopy);
  var rects = Depflame.layoutTree(treeCopy, result.activeNodes);

  var names = {};
  for (var i = 0; i < rects.length; i++) names[rects[i].name] = true;

  // All optional-only nodes should be gone.
  assertEquals(names['serde_derive'], undefined, 'serde_derive should not appear');
  assertEquals(names['heavy-framework'], undefined, 'heavy-framework should not appear');
  // Non-optional deps should remain.
  assert(names['unused-dep'], 'unused-dep should appear (non-optional)');
  assert(names['dead-dep'], 'dead-dep should appear (non-optional)');
  assert(names['regex'], 'regex should appear (non-optional from my-lib)');
});

// -------------------------------------------------------------------------
// Edge cases.
// -------------------------------------------------------------------------

test('node with no available_features is unaffected by feature filtering', function() {
  // tiny-helper (index 6) has no available_features.
  var treeCopy = JSON.parse(JSON.stringify(tree));
  var result = DepflameFeatures.recomputeActiveGraph(treeCopy);
  assert(result.activeNodes[6], 'tiny-helper should be active');
  assertEquals(result.weights[6], 1, 'tiny-helper weight should be 1');
});

test('tree with no edges array works', function() {
  var treeCopy = JSON.parse(JSON.stringify(tree));
  delete treeCopy.edges;

  // Without edge metadata, all edges are treated as non-optional.
  var result = DepflameFeatures.recomputeActiveGraph(treeCopy);
  assertEquals(Object.keys(result.activeNodes).length, treeCopy.nodes.length,
    'all nodes should be active when no edge metadata');
});

// -------------------------------------------------------------------------
// Full toggle cycle: disable → re-enable restores deps.
// -------------------------------------------------------------------------

// -------------------------------------------------------------------------
// Enabling a previously-disabled feature adds new deps to the graph.
// -------------------------------------------------------------------------

test('enabling "remote" on my-app activates remote-lib and its transitive deps', function() {
  var treeCopy = JSON.parse(JSON.stringify(tree));

  // Baseline: "remote" is NOT enabled, so remote-lib (15), remote-sub-a (16),
  // remote-sub-b (17) should be inactive.
  var base = DepflameFeatures.recomputeActiveGraph(treeCopy);
  assertEquals(base.activeNodes[15], undefined, 'remote-lib should be inactive at baseline');
  assertEquals(base.activeNodes[16], undefined, 'remote-sub-a should be inactive at baseline');
  assertEquals(base.activeNodes[17], undefined, 'remote-sub-b should be inactive at baseline');

  // Enable "remote" on my-app (index 0).
  treeCopy.nodes[0].enabled_features = ['default', 'remote'];
  var enabled = DepflameFeatures.recomputeActiveGraph(treeCopy);

  assert(enabled.activeNodes[15], 'remote-lib should be active after enabling remote');
  assert(enabled.activeNodes[16], 'remote-sub-a should be active (transitive dep of remote-lib)');
  assert(enabled.activeNodes[17], 'remote-sub-b should be active (transitive dep of remote-lib)');
});

test('enabling "remote" increases my-app weight by 3 (remote-lib + 2 subs)', function() {
  var treeCopy = JSON.parse(JSON.stringify(tree));

  var base = DepflameFeatures.recomputeActiveGraph(treeCopy);
  var baseWeight = base.weights[0];

  treeCopy.nodes[0].enabled_features = ['default', 'remote'];
  var enabled = DepflameFeatures.recomputeActiveGraph(treeCopy);
  var newWeight = enabled.weights[0];

  assertEquals(newWeight - baseWeight, 3,
    'my-app should gain 3 deps (remote-lib + 2 subs): was ' + baseWeight + ' now ' + newWeight);
});

test('enabling then disabling "remote" returns to baseline', function() {
  var treeCopy = JSON.parse(JSON.stringify(tree));

  var base = DepflameFeatures.recomputeActiveGraph(treeCopy);
  var baseCount = Object.keys(base.activeNodes).length;

  // Enable remote.
  treeCopy.nodes[0].enabled_features = ['default', 'remote'];
  var e = DepflameFeatures.recomputeActiveGraph(treeCopy);
  for (var i = 0; i < treeCopy.nodes.length; i++) {
    treeCopy.nodes[i].transitive_weight = e.weights[i] || 0;
  }

  // Disable remote again.
  treeCopy.nodes[0].enabled_features = ['default'];
  var d = DepflameFeatures.recomputeActiveGraph(treeCopy);
  var newCount = Object.keys(d.activeNodes).length;

  assertEquals(newCount, baseCount,
    'active count should return to baseline after toggle cycle');
});

test('flamegraph layout shows remote-lib after enabling remote', function() {
  var treeCopy = JSON.parse(JSON.stringify(tree));
  treeCopy.nodes[0].enabled_features = ['default', 'remote'];

  var result = DepflameFeatures.recomputeActiveGraph(treeCopy);
  for (var i = 0; i < treeCopy.nodes.length; i++) {
    treeCopy.nodes[i].transitive_weight = result.weights[i] || 0;
  }

  var rects = Depflame.layoutTree(treeCopy, result.activeNodes);
  var names = {};
  for (var i = 0; i < rects.length; i++) names[rects[i].name] = true;

  assert(names['remote-lib'], 'remote-lib should appear in layout');
  assert(names['remote-sub-a'], 'remote-sub-a should appear in layout');
  assert(names['remote-sub-b'], 'remote-sub-b should appear in layout');
});

test('disable then re-enable a feature restores the dep in activeNodes', function() {
  var treeCopy = JSON.parse(JSON.stringify(tree));

  // Baseline: derive enabled, serde_derive active.
  var base = DepflameFeatures.recomputeActiveGraph(treeCopy);
  assert(base.activeNodes[13], 'serde_derive should be active at baseline');
  var baseWeight3 = base.weights[3];

  // Disable derive → serde_derive removed.
  treeCopy.nodes[3].enabled_features = ['default', 'std'];
  var disabled = DepflameFeatures.recomputeActiveGraph(treeCopy);
  assertEquals(disabled.activeNodes[13], undefined, 'serde_derive should be gone after disabling derive');

  // Simulate applyRecomputation: update weights on the tree.
  for (var i = 0; i < treeCopy.nodes.length; i++) {
    treeCopy.nodes[i].transitive_weight = disabled.weights[i] || 0;
  }
  assertEquals(treeCopy.nodes[13].transitive_weight, 0, 'serde_derive weight should be 0');

  // Re-enable derive → serde_derive should come back.
  treeCopy.nodes[3].enabled_features = ['default', 'std', 'derive'];
  var reenabled = DepflameFeatures.recomputeActiveGraph(treeCopy);
  assert(reenabled.activeNodes[13], 'serde_derive should be active again after re-enabling derive');
  assert(reenabled.weights[13] > 0, 'serde_derive should have positive weight');

  // Apply weights again.
  for (var i = 0; i < treeCopy.nodes.length; i++) {
    treeCopy.nodes[i].transitive_weight = reenabled.weights[i] || 0;
  }

  // Verify layout includes serde_derive.
  var rects = Depflame.layoutTree(treeCopy, reenabled.activeNodes);
  var found = rects.some(function(r) { return r.name === 'serde_derive'; });
  assert(found, 'serde_derive should appear in layout after re-enabling');
});

test('disable then re-enable restores original active node count', function() {
  var treeCopy = JSON.parse(JSON.stringify(tree));

  var base = DepflameFeatures.recomputeActiveGraph(treeCopy);
  var baseCount = Object.keys(base.activeNodes).length;

  // Disable → update weights → re-enable → check count.
  treeCopy.nodes[3].enabled_features = ['default', 'std'];
  var d = DepflameFeatures.recomputeActiveGraph(treeCopy);
  for (var i = 0; i < treeCopy.nodes.length; i++) {
    treeCopy.nodes[i].transitive_weight = d.weights[i] || 0;
  }

  treeCopy.nodes[3].enabled_features = ['default', 'std', 'derive'];
  var r = DepflameFeatures.recomputeActiveGraph(treeCopy);
  var newCount = Object.keys(r.activeNodes).length;

  assertEquals(newCount, baseCount,
    'active node count should match baseline after toggle cycle: was ' + baseCount + ' now ' + newCount);
});

// -------------------------------------------------------------------------
// Edge gating: optional deps activated via feature resolution chains.
// -------------------------------------------------------------------------

// Test the "alacritty_terminal" pattern: an optional dep whose gating_feature
// is a parent feature (like "default") that activates a sub-feature (like "serde")
// which then activates "dep:serde". Disabling the sub-feature should disable the dep.
test('optional dep gated by sub-feature chain: default implies sub-feature', function() {
  // Model: parent has default->["async","myserde"], myserde->["dep:myserde-crate"].
  // When default is enabled, myserde is implicitly enabled (resolved), so the dep stays.
  // Only disabling default (or both default and myserde) removes the dep.
  var treeCopy = JSON.parse(JSON.stringify(tree));

  var newIdx = treeCopy.nodes.length;
  treeCopy.nodes.push({
    name: 'myserde-crate', version: '1.0.0', transitive_weight: 1,
    is_workspace: false, children: [], enabled_features: [], available_features: {}
  });

  treeCopy.nodes[2].available_features = {
    'default': ['async', 'myserde'],
    'async': ['dep:tokio'],
    'myserde': ['dep:myserde-crate']
  };
  treeCopy.nodes[2].enabled_features = ['default', 'async', 'myserde'];
  treeCopy.nodes[2].children.push(newIdx);

  treeCopy.edges.push({
    from: 2, to: newIdx, is_optional: true, gating_feature: 'default'
  });

  // Baseline: myserde-crate active (default resolves to myserde).
  var base = DepflameFeatures.recomputeActiveGraph(treeCopy);
  assert(base.activeNodes[newIdx], 'myserde-crate should be active at baseline');

  // "Disable myserde" but keep default — default still resolves to myserde,
  // so the dep stays active. This is correct Cargo behavior.
  treeCopy.nodes[2].enabled_features = ['default', 'async'];
  var result = DepflameFeatures.recomputeActiveGraph(treeCopy);
  assert(result.activeNodes[newIdx],
    'myserde-crate should still be active (default implies myserde)');

  // Only disabling default removes myserde from resolution.
  treeCopy.nodes[2].enabled_features = ['async'];
  var result2 = DepflameFeatures.recomputeActiveGraph(treeCopy);
  assertEquals(result2.activeNodes[newIdx], undefined,
    'myserde-crate should be inactive when default is disabled');
});

test('optional dep gated by sub-feature chain: disabling parent feature also removes dep', function() {
  var treeCopy = JSON.parse(JSON.stringify(tree));

  var newIdx = treeCopy.nodes.length;
  treeCopy.nodes.push({
    name: 'myserde-crate', version: '1.0.0', transitive_weight: 1,
    is_workspace: false, children: [], enabled_features: [], available_features: {}
  });

  treeCopy.nodes[2].available_features = {
    'default': ['async', 'myserde'],
    'async': ['dep:tokio'],
    'myserde': ['dep:myserde-crate']
  };
  treeCopy.nodes[2].enabled_features = ['default', 'async', 'myserde'];
  treeCopy.nodes[2].children.push(newIdx);

  treeCopy.edges.push({
    from: 2, to: newIdx, is_optional: true, gating_feature: 'default'
  });

  // Disable "default" (keep only async) — myserde is no longer resolved.
  treeCopy.nodes[2].enabled_features = ['async'];
  var result = DepflameFeatures.recomputeActiveGraph(treeCopy);
  assertEquals(result.activeNodes[newIdx], undefined,
    'myserde-crate should be inactive when default is disabled (breaks resolution chain)');
});

test('non-optional edges are always active regardless of features', function() {
  var treeCopy = JSON.parse(JSON.stringify(tree));

  // Clear ALL features from ALL nodes.
  for (var i = 0; i < treeCopy.nodes.length; i++) {
    treeCopy.nodes[i].enabled_features = [];
  }

  var result = DepflameFeatures.recomputeActiveGraph(treeCopy);

  // Non-optional edges should still be active.
  // my-app -> unused-dep (non-optional) should keep unused-dep active.
  assert(result.activeNodes[4], 'unused-dep should be active via non-optional edge');
  // unused-dep -> dead-dep (non-optional).
  assert(result.activeNodes[14], 'dead-dep should be active via non-optional edge');

  // Optional edges should be inactive:
  // serde -> serde_derive (optional, gated by "derive") — derive is not enabled.
  assertEquals(result.activeNodes[13], undefined,
    'serde_derive should be inactive when derive feature is disabled');
});
