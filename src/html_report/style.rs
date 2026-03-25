/// Returns the CSS content for the HTML report (everything inside `<style>...</style>`).
pub(super) fn css() -> &'static str {
    r##"* { box-sizing: border-box; margin: 0; padding: 0; }
body {
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", sans-serif;
  background: #f5f5f5; color: #333; line-height: 1.5;
}
.header {
  background: #fff; border-bottom: 1px solid #ddd; padding: 16px 24px;
}
.header h1 { font-size: 20px; margin-bottom: 4px; }
.header .stats { font-size: 13px; color: #666; }
.header .stats span { margin-right: 16px; }
.header .stats span[title] { cursor: help; border-bottom: 1px dashed #aaa; }
.tabs {
  display: flex; background: #fff; border-bottom: 2px solid #ddd;
  padding: 0 24px; gap: 0;
}
.tab-btn {
  padding: 10px 20px; border: none; background: none; cursor: pointer;
  font-size: 14px; font-weight: 500; color: #666;
  border-bottom: 2px solid transparent; margin-bottom: -2px;
  transition: color 0.15s, border-color 0.15s;
}
.tab-btn:hover { color: #333; }
.tab-btn.active { color: #0066cc; border-bottom-color: #0066cc; }
.tab-content { display: none; }
.tab-content.active { display: block; }

/* Flamegraph tab */
#tab-flamegraph { background: #fff; }
#tab-flamegraph svg { display: block; width: 100%; height: auto; }

/* Targets tab */
#tab-targets { padding: 24px; }
.action-summary {
  background: #fff; border: 1px solid #ddd; border-radius: 6px;
  padding: 16px 20px; margin-bottom: 20px;
}
.action-summary h3 { font-size: 14px; margin-bottom: 8px; color: #444; }
.action-summary ul { list-style: none; padding: 0; }
.action-summary li { font-size: 13px; padding: 3px 0; }
.cargo-diff {
  background: #1e1e1e; border-radius: 4px; padding: 8px 12px;
  margin: 6px 0 10px 20px; font-family: "Consolas", "Fira Code", monospace;
  font-size: 12px; line-height: 1.6; overflow-x: auto;
  display: none;
}
.show-diff-btn {
  display: inline-block; font-size: 11px; color: #0066cc;
  border: 1px solid #0066cc; border-radius: 3px; padding: 1px 6px;
  margin-left: 6px; cursor: pointer; vertical-align: middle;
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
}
.cargo-diff .diff-file { color: #888; }
.cargo-diff .diff-rm { color: #f44; }
.cargo-diff .diff-add { color: #4c4; }
.cargo-diff .diff-comment { color: #888; font-style: italic; }
.targets-table {
  width: 100%; border-collapse: collapse; background: #fff;
  border: 1px solid #ddd; border-radius: 6px; overflow: hidden;
  font-size: 13px;
}
.targets-table th {
  background: #f8f8f8; text-align: left; padding: 10px 12px;
  border-bottom: 2px solid #ddd; font-weight: 600; font-size: 12px;
  text-transform: uppercase; color: #555; white-space: nowrap;
}
.targets-table th[title] {
  cursor: help; border-bottom: 1px dashed #999;
}
.targets-table td {
  padding: 8px 12px; border-bottom: 1px solid #eee;
  vertical-align: top;
}
.targets-table tr:hover { background: #f9f9f9; }
.targets-table tr.expandable { cursor: pointer; }
.detail-row { display: none; }
.detail-row.open { display: table-row; }
.detail-row td {
  background: #fafafa; padding: 12px 20px;
  border-bottom: 1px solid #ddd;
}
.detail-box {
  font-family: "Consolas", monospace; font-size: 12px; line-height: 1.6;
}
.detail-box .label { color: #888; }
.badge {
  display: inline-block; padding: 1px 6px; border-radius: 3px;
  font-size: 11px; font-weight: 600;
}
.badge-high { background: #e8f5e9; color: #2e7d32; }
.badge-medium { background: #fff3e0; color: #e65100; }
.badge-low { background: #fce4ec; color: #c62828; }
.badge-noise { background: #f3e5f5; color: #6a1b9a; }
.badge-flag {
  background: #e3f2fd; color: #1565c0; margin-right: 4px;
}
.ref-file { color: #0066cc; }
.ref-line { color: #888; margin-left: 16px; }

/* JSON tab */
#tab-json { padding: 24px; }
.json-container {
  position: relative; background: #1e1e1e; border-radius: 6px;
  overflow: hidden;
}
.json-container pre {
  padding: 20px; overflow-x: auto; color: #d4d4d4;
  font-family: "Consolas", "Fira Code", monospace; font-size: 12px;
  line-height: 1.5; margin: 0;
}
.copy-btn {
  position: absolute; top: 8px; right: 8px; padding: 6px 14px;
  background: #333; color: #ccc; border: 1px solid #555;
  border-radius: 4px; cursor: pointer; font-size: 12px;
}
.copy-btn:hover { background: #444; }"##
}

/// Returns the JavaScript content for the HTML report (everything inside `<script>...</script>`).
///
/// Note: the JS uses literal braces, so the caller must NOT pass this through `format!`.
/// Instead it should be written directly via `write!` or string concatenation.
pub(super) fn js() -> &'static str {
    r##"function showTab(name) {
  document.querySelectorAll('.tab-content').forEach(function(el) {
    el.classList.remove('active');
  });
  document.querySelectorAll('.tab-btn').forEach(function(btn) {
    btn.classList.remove('active');
  });
  document.getElementById('tab-' + name).classList.add('active');
  // Find the button whose onclick contains the tab name.
  document.querySelectorAll('.tab-btn').forEach(function(btn) {
    if (btn.getAttribute('onclick').indexOf(name) !== -1) {
      btn.classList.add('active');
    }
  });
}
function toggleDetail(n) {
  var row = document.getElementById('detail-' + n);
  if (row) row.classList.toggle('open');
}
function toggleDiff(li) {
  var diff = li.querySelector('.cargo-diff');
  var btn = li.querySelector('.show-diff-btn');
  if (diff) {
    var show = diff.style.display !== 'block';
    diff.style.display = show ? 'block' : 'none';
    if (btn) btn.textContent = show ? 'hide diff' : 'show diff';
  }
}
function copyJson() {
  var text = document.querySelector('#tab-json pre code').textContent;
  navigator.clipboard.writeText(text).then(function() {
    var btn = document.querySelector('.copy-btn');
    btn.textContent = 'Copied!';
    setTimeout(function() { btn.textContent = 'Copy'; }, 1500);
  });
}"##
}
