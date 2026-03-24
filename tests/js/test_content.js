// Tests for content.js — HTML generation from JSON report data.
// Assumes: DepflameContent is loaded, `report` and test helpers are in scope.

// Access internals by re-extracting from the IIFE. We test via the init() path
// by calling init and inspecting the resulting HTML, plus testing the public API.

test('esc() escapes HTML special characters', function() {
  // DepflameContent is an IIFE so esc isn't directly accessible.
  // We test it indirectly through content that uses it.
  // Build a report with special chars in a field.
  var r = JSON.parse(JSON.stringify(report));
  r.tool_version = '<script>alert("xss")</script>';
  elements['app'] = new MockElement('div');
  window.__DEPFLAME_REPORT__ = r;
  DepflameContent.init();
  var html = elements['app']._innerHTML;
  assert(html.indexOf('<script>alert') === -1, 'should not contain raw <script> tag');
  assertContains(html, '&lt;script&gt;', 'should contain escaped version');
  // Restore.
  window.__DEPFLAME_REPORT__ = report;
});

test('init() produces header with correct dep counts', function() {
  elements['app'] = new MockElement('div');
  window.__DEPFLAME_REPORT__ = report;
  DepflameContent.init();
  var html = elements['app']._innerHTML;
  assertContains(html, '42 Total Deps');
  assertContains(html, '38 Platform Deps');
  assertContains(html, '4 Phantom Deps');
  assertContains(html, '6 Heavy Crates');
  assertContains(html, '7 Targets');
});

test('init() produces all four tabs', function() {
  elements['app'] = new MockElement('div');
  DepflameContent.init();
  var html = elements['app']._innerHTML;
  assertContains(html, 'id="tab-flamegraph"');
  assertContains(html, 'id="tab-table"');
  assertContains(html, 'id="tab-targets"');
  assertContains(html, 'id="tab-json"');
});

test('table tab contains direct dep summary rows', function() {
  elements['app'] = new MockElement('div');
  DepflameContent.init();
  var html = elements['app']._innerHTML;
  assertContains(html, 'heavy-framework');
  assertContains(html, 'serde');
  assertContains(html, 'regex');
  // Check unique dep counts appear.
  assertContains(html, '>18<');  // heavy-framework unique deps
  assertContains(html, '>25<');  // heavy-framework total deps
});

test('suggestions tab contains category sections for non-noise targets', function() {
  elements['app'] = new MockElement('div');
  DepflameContent.init();
  var html = elements['app']._innerHTML;
  assertContains(html, 'Remove unused dependencies');
  assertContains(html, 'Disable unnecessary features');
  assertContains(html, 'Make dependencies optional');
});

test('suggestions tab contains disclaimer banner', function() {
  elements['app'] = new MockElement('div');
  DepflameContent.init();
  var html = elements['app']._innerHTML;
  assertContains(html, 'Use your judgement');
});

test('suggestions tab has detail table with non-noise targets', function() {
  elements['app'] = new MockElement('div');
  DepflameContent.init();
  var html = elements['app']._innerHTML;
  // 6 non-noise targets (RequiredBySibling is Noise and filtered out).
  assertContains(html, 'detail-1');
  // Check suggestion displays for non-noise targets.
  assertContains(html, 'Remove');
  assertContains(html, 'Feature Gate');
  assertContains(html, 'Already Gated');
  assertContains(html, 'Replace with Std');
  assertContains(html, 'Inline');
  assertContains(html, 'Move to Dev');
  // Noise targets should not appear.
  var tableStart = html.indexOf('targets-table');
  var tableHtml = html.substring(tableStart);
  assert(tableHtml.indexOf('Required by Sibling') === -1,
    'Noise-confidence targets should be filtered from the detail table');
});

test('AlreadyGated target shows enabling features and diff', function() {
  elements['app'] = new MockElement('div');
  DepflameContent.init();
  var html = elements['app']._innerHTML;
  // The AlreadyGated target for serde -> serde_derive has enabling_features: ["derive"]
  assertContains(html, 'derive');
  assertContains(html, 'default-features = false');
});

test('FeatureGate target shows cargo diff with optional = true', function() {
  elements['app'] = new MockElement('div');
  DepflameContent.init();
  var html = elements['app']._innerHTML;
  assertContains(html, 'optional = true');
});

test('RequiredBySibling target mentions the sibling', function() {
  elements['app'] = new MockElement('div');
  DepflameContent.init();
  var html = elements['app']._innerHTML;
  assertContains(html, 'hyper');
});

test('JSON tab contains report data', function() {
  elements['app'] = new MockElement('div');
  DepflameContent.init();
  var html = elements['app']._innerHTML;
  assertContains(html, 'copyJson()');
  assertContains(html, '0.1.0-sample');
});

test('timestamp is formatted from epoch', function() {
  elements['app'] = new MockElement('div');
  DepflameContent.init();
  var html = elements['app']._innerHTML;
  // The epoch:1700000000 should be converted to a date string.
  // Since we're in Node, Date.toLocaleString() will produce something like "11/14/2023".
  // Just check that the raw epoch string is NOT in the header (it should be formatted).
  var headerEnd = html.indexOf('id="tab-flamegraph"');
  var header = html.substring(0, headerEnd);
  assert(header.indexOf('epoch:1700000000') === -1,
    'header should not contain raw epoch string — should be formatted');
});

test('crate links point to crates.io', function() {
  elements['app'] = new MockElement('div');
  DepflameContent.init();
  var html = elements['app']._innerHTML;
  assertContains(html, 'https://crates.io/crates/serde');
  // tokio only appears in RequiredBySibling which is Noise-confidence
  // and filtered out of the suggestions tab, but still present in the
  // flamegraph/table tabs or JSON.
});
