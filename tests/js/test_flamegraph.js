// Tests for flamegraph.js — layout algorithm and rendering helpers.
// Assumes: Depflame is loaded, `report` and test helpers are in scope.

var tree = report.dep_tree;

test('computeAncestorCounts returns correct counts', function() {
  var counts = Depflame.computeAncestorCounts(tree);
  // tokio (index 7) has 2 parents: heavy-framework (2) and http-client (8)
  assertEquals(counts[7], 2, 'tokio should have 2 parents');
  // my-app (index 0) is a root — 0 parents
  assertEquals(counts[0], 0, 'my-app should have 0 parents');
  // serde_derive (index 13) has 1 parent: serde (3)
  assertEquals(counts[13], 1, 'serde_derive should have 1 parent');
});

test('layoutTree produces rects for all visible nodes', function() {
  var rects = Depflame.layoutTree(tree, null);
  assert(rects.length > 0, 'should produce some rects');
  // Should have at least the 2 root nodes.
  var rootNames = rects.filter(function(r) { return r.depth === 0; }).map(function(r) { return r.name; });
  assert(rootNames.indexOf('my-app') !== -1, 'should have my-app root');
  assert(rootNames.indexOf('my-lib') !== -1, 'should have my-lib root');
});

test('layoutTree respects workspace flag', function() {
  var rects = Depflame.layoutTree(tree, null);
  var myApp = rects.find(function(r) { return r.name === 'my-app'; });
  assert(myApp, 'my-app should be in rects');
  assertEquals(myApp.isWorkspace, true, 'my-app should be workspace');
  var serde = rects.find(function(r) { return r.name === 'serde'; });
  assert(serde, 'serde should be in rects');
  assertEquals(serde.isWorkspace, false, 'serde should not be workspace');
});

test('layoutTree marks shared nodes', function() {
  var rects = Depflame.layoutTree(tree, null);
  // tokio appears under both heavy-framework and http-client — should be shared.
  var tokioRects = rects.filter(function(r) { return r.name === 'tokio'; });
  assert(tokioRects.length >= 1, 'tokio should appear in rects');
  assert(tokioRects.some(function(r) { return r.isShared; }), 'tokio should be marked as shared');
});

test('layoutTree with activeNodes filters inactive nodes', function() {
  // Deactivate unused-dep (index 4) and its child dead-dep (index 14).
  var active = {};
  for (var i = 0; i < tree.nodes.length; i++) {
    if (i !== 4 && i !== 14) active[i] = true;
  }
  var rects = Depflame.layoutTree(tree, active);
  var hasUnused = rects.some(function(r) { return r.name === 'unused-dep'; });
  assertEquals(hasUnused, false, 'unused-dep should not appear when inactive');
  var hasDeadDep = rects.some(function(r) { return r.name === 'dead-dep'; });
  assertEquals(hasDeadDep, false, 'dead-dep should not appear when inactive');
});

test('layoutTree handles empty tree', function() {
  var emptyTree = { nodes: [], root_indices: [] };
  var rects = Depflame.layoutTree(emptyTree, null);
  assertEquals(rects.length, 0, 'empty tree should produce 0 rects');
});

test('layoutTree rects have valid coordinates', function() {
  var rects = Depflame.layoutTree(tree, null);
  for (var i = 0; i < rects.length; i++) {
    var r = rects[i];
    assert(r.x >= 0, 'x should be >= 0 for ' + r.name);
    assert(r.w > 0, 'width should be > 0 for ' + r.name);
    assert(r.x + r.w <= 1201, 'rect should not exceed chart width for ' + r.name);
    assert(r.y >= 0, 'y should be >= 0 for ' + r.name);
    assert(r.depth >= 0, 'depth should be >= 0 for ' + r.name);
  }
});

test('render does not throw', function() {
  var container = new MockElement('div');
  // Should not throw even with mock DOM.
  Depflame.render(container, tree, report.unused_edges || []);
  assert(container._children.length > 0 || container._innerHTML !== '',
    'container should have content after render');
});
