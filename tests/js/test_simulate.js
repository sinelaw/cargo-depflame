// Tests for simulate.js — "what if I dropped this direct dependency?".
// Assumes: DepflameSimulate/DepflameContent are loaded, `report` and test
// helpers are in scope.

// The sample tree:
//   my-app (ws) -> heavy-framework, serde, unused-dep, http-client,
//                  test-helpers, remote-lib (optional, off by default)
//   my-lib (ws) -> regex, tiny-helper, once_cell
//   heavy-framework -> tokio, heavy-sub-a, heavy-sub-b
//   http-client     -> tokio          (tokio is shared)
//   serde           -> serde_derive
//   unused-dep      -> dead-dep
function nodeIndex(name) {
  var nodes = report.dep_tree.nodes;
  for (var i = 0; i < nodes.length; i++) {
    if (nodes[i].name === name) return i;
  }
  return -1;
}

function freshSim() {
  window.__DEPFLAME_DATA__ = null;
  window.__DEPFLAME_REPORT__ = report;
  DepflameSimulate.init();
}

test('only direct deps of workspace members are removable', function() {
  freshSim();
  assert(DepflameSimulate.indexFor('heavy-framework', '2.0.0') >= 0,
    'heavy-framework is a direct dep of my-app');
  assert(DepflameSimulate.indexFor('regex', '1.10.0') >= 0,
    'regex is a direct dep of my-lib');
  assertEquals(DepflameSimulate.indexFor('tokio', '1.35.0'), -1,
    'tokio is only transitive, so it cannot be removed');
  assertEquals(DepflameSimulate.indexFor('serde_derive', '1.0.200'), -1,
    'serde_derive is only transitive');
  assertEquals(DepflameSimulate.indexFor('no-such-crate', '1.0.0'), -1,
    'unknown crates are not removable');
});

test('baseline unique counts split a shared transitive dep', function() {
  freshSim();
  var base = DepflameSimulate.baseline();
  // tokio is pulled in by both heavy-framework and http-client, so neither
  // owns it: heavy-framework keeps itself + its two private subs.
  assertEquals(base.rows[nodeIndex('heavy-framework')].unique, 3);
  assertEquals(base.rows[nodeIndex('http-client')].unique, 1);
  assertEquals(base.rows[nodeIndex('heavy-framework')].owners, 1);
});

test('removing one owner makes the shared dep unique to the other', function() {
  freshSim();
  var hf = nodeIndex('heavy-framework');
  DepflameSimulate.toggle(nodeIndex('http-client'));

  var sim = DepflameSimulate.current();
  // tokio now belongs to heavy-framework alone: 3 -> 4.
  assertEquals(sim.rows[hf].unique, 4);
  assert(sim.rows[nodeIndex('http-client')].removed, 'http-client is marked removed');
  // tokio survives — it is still pulled in by heavy-framework.
  assertEquals(sim.totalDeps, DepflameSimulate.baseline().totalDeps - 1);

  DepflameSimulate.clear();
  assertEquals(DepflameSimulate.current().rows[hf].unique, 3, 'clearing restores the baseline');
});

test('removing both owners drops the shared dep too', function() {
  freshSim();
  var base = DepflameSimulate.baseline();
  DepflameSimulate.toggle(nodeIndex('heavy-framework'));
  DepflameSimulate.toggle(nodeIndex('http-client'));

  var sim = DepflameSimulate.current();
  assertEquals(sim.removedCount, 2);
  // heavy-framework + 2 private subs + http-client + tokio = 5 crates gone.
  assertEquals(base.totalDeps - sim.totalDeps, 5);
  assertEquals(sim.rows[nodeIndex('heavy-framework')].stillPulledBy.length, 0);
  DepflameSimulate.clear();
});

test('a removed dep that is still pulled in transitively is reported', function() {
  // my-app -> a, b; a -> b. Removing b as a direct dep changes nothing:
  // it is still pulled in by a.
  var saved = window.__DEPFLAME_DATA__;
  window.__DEPFLAME_DATA__ = {
    root_indices: [0],
    edges: [],
    nodes: [
      { name: 'my-app', version: '0.1.0', is_workspace: true, children: [1, 2], transitive_weight: 3, unique_ancestors: 0 },
      { name: 'a', version: '1.0.0', is_workspace: false, children: [2], transitive_weight: 2, unique_ancestors: 1 },
      { name: 'b', version: '1.0.0', is_workspace: false, children: [], transitive_weight: 1, unique_ancestors: 2 }
    ]
  };
  DepflameSimulate.init();

  var base = DepflameSimulate.baseline();
  assertEquals(base.rows[1].unique, 1, 'a owns only itself: b is shared with the direct edge');
  assertEquals(base.rows[2].owners, 2, 'b is pulled in by itself and by a');

  DepflameSimulate.toggle(2);
  var sim = DepflameSimulate.current();
  assert(sim.rows[2].removed, 'b is marked removed');
  assertEquals(sim.rows[2].stillPulledBy.join(','), 'a', 'b survives through a');
  assertEquals(sim.totalDeps, base.totalDeps, 'nothing is actually saved');
  assertEquals(sim.rows[1].unique, 2, 'a now uniquely owns b');

  DepflameSimulate.clear();
  window.__DEPFLAME_DATA__ = saved;
});

test('table tab renders a checkbox per row', function() {
  freshSim();
  elements['app'] = new MockElement('div');
  DepflameContent.init();
  var html = elements['app']._innerHTML;
  var table = html.substring(html.indexOf('id="dep-summary-table"'), html.indexOf('id="tab-sharing"'));
  assertContains(table, 'class="sim-box"');
  assertContains(table, 'DepflameContent.toggleRemoval(');
  assertContains(table, '>Remove<');
  assert(table.indexOf('disabled') === -1,
    'every sample row is a direct dep, so no checkbox is disabled');
});

test('a row that is not a direct dep gets a disabled checkbox', function() {
  // tokio appears in the tree only as a transitive dep of heavy-framework and
  // http-client, so a summary row for it must not be removable.
  var r = JSON.parse(JSON.stringify(report));
  r.direct_dep_summary.push({
    workspace_member: 'my-app',
    dep_name: 'tokio',
    dep_version: '1.35.0',
    unique_transitive_deps: 0,
    total_transitive_deps: 19,
    unique_ancestors: 2,
    owner_count: 2
  });
  window.__DEPFLAME_DATA__ = null;
  window.__DEPFLAME_REPORT__ = r;
  elements['app'] = new MockElement('div');
  DepflameContent.init();
  var html = elements['app']._innerHTML;
  var row = html.substring(html.indexOf('>tokio<') - 400, html.indexOf('>tokio<'));
  assertContains(row, 'disabled');
  assertContains(row, 'Not a direct dependency of a workspace member');

  window.__DEPFLAME_REPORT__ = report;
});

test('toggling a row through the UI re-renders the table with new numbers', function() {
  freshSim();
  elements['app'] = new MockElement('div');
  DepflameContent.init();

  var tbody = new MockElement('tbody');
  elements['dep-summary-tbody'] = tbody;
  elements['sim-status'] = new MockElement('span');
  elements['sim-reset'] = new MockElement('button');

  // The sample's direct_dep_summary has heavy-framework at 18 unique deps
  // (server value); the tree-derived baseline is 3.
  DepflameContent.toggleRemoval(nodeIndex('regex'));
  assertContains(tbody._innerHTML, 'row-removed');
  assertContains(elements['sim-status'].textContent, '1 removed');
  assertEquals(elements['sim-reset'].style.visibility, 'visible');

  DepflameContent.resetRemovals();
  assert(tbody._innerHTML.indexOf('row-removed') === -1, 'reset clears the removed rows');
  assertEquals(elements['sim-status'].textContent, '');
  assertEquals(elements['sim-reset'].style.visibility, 'hidden',
    'the button keeps its slot in the layout so the row never jumps');
});

test('removed rows show a dash instead of a unique count', function() {
  freshSim();
  elements['app'] = new MockElement('div');
  DepflameContent.init();
  var tbody = new MockElement('tbody');
  elements['dep-summary-tbody'] = tbody;
  elements['sim-status'] = new MockElement('span');
  elements['sim-reset'] = new MockElement('button');

  DepflameContent.toggleRemoval(nodeIndex('regex'));
  assertContains(tbody._innerHTML, 'sim-gone');
  assertContains(tbody._innerHTML, 'gone');
  DepflameContent.resetRemovals();
});

// ---------------------------------------------------------------------------
// Removals reach the flamegraph: the same cut edges drive both views.
// ---------------------------------------------------------------------------

test('cutEdges lists the workspace edges into removed deps', function() {
  freshSim();
  var hf = nodeIndex('heavy-framework');
  var myApp = nodeIndex('my-app');
  assertEquals(Object.keys(DepflameSimulate.cutEdges()).length, 0);

  DepflameSimulate.toggle(hf);
  var cuts = DepflameSimulate.cutEdges();
  assertEquals(Object.keys(cuts).join(','), myApp + ':' + hf);
  DepflameSimulate.clear();
});

test('the flamegraph graph drops a removed dep and its private subtree', function() {
  freshSim();
  var tree = report.dep_tree;
  var hf = nodeIndex('heavy-framework');
  var before = DepflameFeatures.recomputeActiveGraph(tree);
  assert(before.activeNodes[hf], 'heavy-framework starts active');
  assert(before.activeNodes[nodeIndex('heavy-sub-a')], 'its subs start active');

  DepflameSimulate.toggle(hf);
  var after = DepflameFeatures.recomputeActiveGraph(tree, DepflameSimulate.cutEdges());
  assert(!after.activeNodes[hf], 'the removed dep leaves the flamegraph');
  assert(!after.activeNodes[nodeIndex('heavy-sub-a')], 'so does its private subtree');
  assert(after.activeNodes[nodeIndex('tokio')],
    'tokio stays: http-client still pulls it in');

  // Weights shrink accordingly: my-app no longer counts the removed subtree.
  assert(after.weights[nodeIndex('my-app')] < before.weights[nodeIndex('my-app')],
    'the workspace crate gets lighter');

  DepflameSimulate.clear();
});

// ---------------------------------------------------------------------------
// Workspace feature picker.
// ---------------------------------------------------------------------------

test('the table tab renders a feature picker for workspace crates', function() {
  freshSim();
  elements['app'] = new MockElement('div');
  DepflameContent.init();
  var html = elements['app']._innerHTML;
  var table = html.substring(html.indexOf('id="tab-table"'),
                             html.indexOf('id="dep-summary-table"'));
  assertContains(table, 'Workspace features');
  // my-app declares "default" and "remote"; my-lib declares none.
  assertContains(table, 'DepflameContent.toggleWorkspaceFeature(');
  assertContains(table, 'remote</label>');
  assertContains(table, 'default</label>');
  assertContains(table, 'Restore defaults');
});

test('enabling a workspace feature pulls its optional dep into the graph', function() {
  freshSim();
  var tree = report.dep_tree;
  var myApp = nodeIndex('my-app');
  var remoteLib = nodeIndex('remote-lib');
  assert(remoteLib >= 0, 'the sample has a feature-gated remote-lib');
  assert(!DepflameFeatures.recomputeActiveGraph(tree).activeNodes[remoteLib],
    'remote-lib is off by default');
  assertEquals(DepflameSimulate.indexFor('remote-lib', null), -1,
    'and so it is not a removable direct dep');

  DepflameContent.toggleWorkspaceFeature(myApp, 'remote', true);
  assert(DepflameFeatures.recomputeActiveGraph(tree).activeNodes[remoteLib],
    'enabling "remote" activates it');
  assert(DepflameSimulate.indexFor('remote-lib', null) >= 0,
    'it becomes a removable direct dep');

  DepflameContent.resetWorkspaceFeatures();
  assert(!DepflameFeatures.recomputeActiveGraph(tree).activeNodes[remoteLib],
    'restoring defaults switches it back off');
});

test('disabling a workspace feature marks the affected row as off', function() {
  freshSim();
  elements['app'] = new MockElement('div');
  DepflameContent.init();
  var tbody = new MockElement('tbody');
  elements['dep-summary-tbody'] = tbody;
  elements['sim-status'] = new MockElement('span');
  elements['sim-reset'] = new MockElement('button');
  elements['feature-picker-body'] = new MockElement('div');
  elements['feature-picker-status'] = new MockElement('span');

  // "default" on my-app activates heavy-framework; without it the row is off.
  DepflameContent.toggleWorkspaceFeature(nodeIndex('my-app'), 'default', false);
  assertContains(tbody._innerHTML, 'row-inactive');
  assertContains(tbody._innerHTML, 'off in the current feature set');
  assertContains(elements['feature-picker-status'].textContent, 'customized');

  DepflameContent.resetWorkspaceFeatures();
  assert(tbody._innerHTML.indexOf('row-inactive') === -1,
    'restoring defaults brings the row back');
  assertEquals(elements['feature-picker-status'].textContent, '');
});

test('the flamegraph "Reset all" clears removals as well as features', function() {
  freshSim();
  elements['app'] = new MockElement('div');
  DepflameContent.init();
  elements['dep-summary-tbody'] = new MockElement('tbody');
  elements['sim-status'] = new MockElement('span');
  elements['sim-reset'] = new MockElement('button');
  elements['feature-picker-body'] = new MockElement('div');
  elements['feature-picker-status'] = new MockElement('span');

  DepflameContent.toggleWorkspaceFeature(nodeIndex('my-app'), 'remote', true);
  DepflameContent.toggleRemoval(nodeIndex('regex'));
  assert(DepflameSimulate.hasRemovals(), 'a removal is active');
  assert(DepflameFeatures.hasFeatureOverrides(), 'a feature override is active');

  DepflameFeatures.resetAll();
  assert(!DepflameSimulate.hasRemovals(), 'reset all clears removals');
  assert(!DepflameFeatures.hasFeatureOverrides(), 'reset all clears feature overrides');
  assertEquals(elements['sim-status'].textContent, '', 'and the table status clears');
});

test('the table Reset clears removals but keeps the feature selection', function() {
  freshSim();
  elements['app'] = new MockElement('div');
  DepflameContent.init();
  elements['dep-summary-tbody'] = new MockElement('tbody');
  elements['sim-status'] = new MockElement('span');
  elements['sim-reset'] = new MockElement('button');
  elements['feature-picker-body'] = new MockElement('div');
  elements['feature-picker-status'] = new MockElement('span');

  DepflameContent.toggleWorkspaceFeature(nodeIndex('my-app'), 'remote', true);
  DepflameContent.toggleRemoval(nodeIndex('regex'));

  DepflameContent.resetRemovals();
  assert(!DepflameSimulate.hasRemovals(), 'removals are cleared');
  assert(DepflameFeatures.hasFeatureOverrides(), 'the feature selection survives');

  DepflameFeatures.resetAll();
});

// ---------------------------------------------------------------------------
// Feature toggles recompute the table, not just the flamegraph.
// ---------------------------------------------------------------------------

function tableHarness() {
  freshSim();
  elements['app'] = new MockElement('div');
  DepflameContent.init();
  var tbody = new MockElement('tbody');
  elements['dep-summary-tbody'] = tbody;
  elements['sim-status'] = new MockElement('span');
  elements['sim-reset'] = new MockElement('button');
  elements['feature-picker-body'] = new MockElement('div');
  elements['feature-picker-status'] = new MockElement('span');
  DepflameContent.refreshDepSummary();
  return tbody;
}

test('enabling a workspace feature recomputes the table with no removal', function() {
  var tbody = tableHarness();
  assertEquals(elements['sim-status'].textContent, '', 'nothing toggled yet');
  var before = tbody._innerHTML;
  assert(before.indexOf('remote-lib') === -1, 'remote-lib starts out of the graph');

  DepflameContent.toggleWorkspaceFeature(nodeIndex('my-app'), 'remote', true);
  var after = tbody._innerHTML;
  assert(after !== before, 'the table re-renders on a feature change');
  assertContains(after, 'remote-lib');
  assertContains(after, 'sim-added');
  // remote-lib brings in two crates of its own: 13 -> 16.
  assertContains(elements['sim-status'].textContent, '13 \u2192 16 crates (+3)');

  DepflameContent.resetWorkspaceFeatures();
  assert(tbody._innerHTML.indexOf('remote-lib') === -1, 'and it leaves again');
  assertEquals(elements['sim-status'].textContent, '');
});

test('only crates a feature brings in get a row, not every unlisted direct dep', function() {
  var tbody = tableHarness();
  // unused-dep and http-client are direct deps in the tree but absent from the
  // sample summary; a feature toggle must not conjure rows for them.
  DepflameContent.toggleWorkspaceFeature(nodeIndex('my-app'), 'remote', true);
  var html = tbody._innerHTML;
  assertContains(html, 'remote-lib');
  assert(html.indexOf('unused-dep') === -1, 'unused-dep was already there, so no new row');
  assert(html.indexOf('http-client') === -1, 'nor http-client');
  DepflameContent.resetWorkspaceFeatures();
});

test('restoring feature defaults puts the analysis numbers back', function() {
  var tbody = tableHarness();
  var before = tbody._innerHTML;
  assertContains(before, '>18<');  // heavy-framework unique deps, from the report

  DepflameContent.toggleWorkspaceFeature(nodeIndex('my-app'), 'default', false);
  assert(tbody._innerHTML.indexOf('>18<') === -1, 'live numbers replace the report ones');

  DepflameContent.toggleWorkspaceFeature(nodeIndex('my-app'), 'default', true);
  assertContains(tbody._innerHTML, '>18<', 'and the report numbers come back');
  assertEquals(elements['sim-status'].textContent, '');
});

test('a feature toggle and a removal compose in the status line', function() {
  var tbody = tableHarness();
  DepflameContent.toggleWorkspaceFeature(nodeIndex('my-app'), 'remote', true);
  DepflameContent.toggleRemoval(nodeIndex('regex'));

  var status = elements['sim-status'].textContent;
  assertContains(status, '1 removed');
  // remote adds 3, dropping regex removes 1: 13 -> 15.
  assertContains(status, '13 \u2192 15 crates (+2)');

  DepflameFeatures.resetAll();
  assertEquals(elements['sim-status'].textContent, '');
});

// ---------------------------------------------------------------------------
// Row order only changes when you ask for it.
// ---------------------------------------------------------------------------

function rowOrder(tbody) {
  return tbody._innerHTML.split('<tr').slice(1).map(function(r) {
    var m = r.match(/crates\/([a-z0-9_-]+)"/);
    return m ? m[1] : '?';
  }).join(' ');
}

test('ticking a Remove box does not reorder the table', function() {
  var tbody = tableHarness();
  var before = rowOrder(tbody);

  DepflameContent.toggleRemoval(nodeIndex('heavy-framework'));
  assertEquals(rowOrder(tbody), before, 'rows stay put when a box is ticked');
  assertContains(tbody._innerHTML, 'row-removed');

  DepflameContent.toggleRemoval(nodeIndex('serde'));
  assertEquals(rowOrder(tbody), before, 'and still with a second removal');

  DepflameContent.resetRemovals();
  assertEquals(rowOrder(tbody), before, 'and when they are cleared again');
});

test('clicking a column header re-sorts using the simulated values', function() {
  var tbody = tableHarness();
  DepflameContent.toggleRemoval(nodeIndex('heavy-framework'));
  var frozen = rowOrder(tbody);

  // Re-sorting by unique deps sinks the removed row, which no longer has one.
  DepflameContent.sortDepSummary('unique_transitive_deps');
  DepflameContent.sortDepSummary('unique_transitive_deps'); // back to descending
  var resorted = rowOrder(tbody);
  assert(resorted !== frozen, 'an explicit sort does reorder');
  assertEquals(resorted.split(' ').pop(), 'heavy-framework',
    'the removed row sorts last on unique deps');

  DepflameContent.resetRemovals();
});

test('a feature toggle re-sorts, so an added crate lands in order', function() {
  var tbody = tableHarness();
  DepflameContent.toggleWorkspaceFeature(nodeIndex('my-app'), 'remote', true);
  assertContains(rowOrder(tbody), 'remote-lib');
  DepflameContent.resetWorkspaceFeatures();
});
