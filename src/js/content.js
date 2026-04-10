// ---------------------------------------------------------------------------
// depflame — Generate all HTML report content from JSON data.
// Reads window.__DEPFLAME_REPORT__ and populates the page.
// ---------------------------------------------------------------------------

// Shared icon SVGs used across content.js and features.js.
var DepflameIcons = {
  box: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"></path><polyline points="3.27 6.96 12 12.01 20.73 6.96"></polyline><line x1="12" y1="22.08" x2="12" y2="12"></line></svg>',
  externalLink: '<svg viewBox="0 0 12 12"><path d="M3.5 1H1v10h10V8.5M7 1h4v4M11 1L5.5 6.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>'
};

var DepflameContent = (function() {
  'use strict';

  function esc(s) {
    return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;')
      .replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }

  function crateLink(name) {
    var e = esc(name);
    return '<a href="https://crates.io/crates/' + e + '" target="_blank" '
      + 'class="crate-link">' + e + '</a>';
  }

  function formatTimestamp(raw) {
    var m = String(raw).match(/^epoch:(\d+)$/);
    if (!m) return esc(raw);
    var d = new Date(parseInt(m[1], 10) * 1000);
    return d.toLocaleString(undefined, {
      year: 'numeric', month: 'short', day: 'numeric',
      hour: '2-digit', minute: '2-digit'
    });
  }

  // SVG icon helpers (inline feather-style icons).
  var icons = {
    layers: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="12 2 2 7 12 12 22 7 12 2"></polygon><polyline points="2 17 12 22 22 17"></polyline><polyline points="2 12 12 17 22 12"></polyline></svg>',
    database: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><ellipse cx="12" cy="5" rx="9" ry="3"></ellipse><path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3"></path><path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"></path></svg>',
    ghost: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 10h.01"></path><path d="M15 10h.01"></path><path d="M12 2a8 8 0 0 0-8 8v12l3-3 2.5 2.5L12 19l2.5 2.5L17 19l3 3V10a8 8 0 0 0-8-8z"></path></svg>',
    box: DepflameIcons.box,
    target: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"></circle><circle cx="12" cy="12" r="6"></circle><circle cx="12" cy="12" r="2"></circle></svg>',
    search: '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"></circle><line x1="21" y1="21" x2="16.65" y2="16.65"></line></svg>'
  };

  function statItem(icon, text, title) {
    return '<div class="stat-item" title="' + esc(title) + '">'
      + icon + '<span>' + text + '</span></div>';
  }

  // -------------------------------------------------------------------------
  // Header + stats.
  // -------------------------------------------------------------------------

  function buildHeader(r) {
    var stats = '';
    stats += statItem(icons.layers, r.total_dependencies + ' Total Deps',
      'Total crate dependencies in the full cross-platform resolve graph.');
    if (r.platform_dependencies != null) {
      stats += statItem(icons.database, r.platform_dependencies + ' Platform Deps',
        'Dependencies actually compiled for your current platform/target.');
    }
    stats += statItem(icons.ghost, r.phantom_dependencies + ' Phantom Deps',
      'Dependencies not compiled on your current platform.');
    stats += statItem(icons.box, r.heavy_nodes_found + ' Heavy Crates',
      'Crates with a high transitive dependency count.');
    stats += statItem(icons.target, r.targets.length + ' Targets',
      'Potential optimization targets found.');

    return '<div class="header">'
      + '<div class="header-top"><h1>depflame &mdash; Dependency Analysis Report</h1></div>'
      + '<div class="stats">' + stats + '</div>'
      + '<div class="meta">v' + esc(r.tool_version) + ' &middot; ' + formatTimestamp(r.timestamp) + '</div>'
      + '</div>';
  }

  // -------------------------------------------------------------------------
  // Tabs.
  // -------------------------------------------------------------------------

  function buildTabs(nTargets) {
    return '<div class="tabs">'
      + '<button class="tab-btn active" onclick="showTab(\'flamegraph\')">Flamegraph</button>'
      + '<button class="tab-btn" onclick="showTab(\'table\')">Table</button>'
      + '<button class="tab-btn" onclick="showTab(\'targets\')">Suggestions (' + nTargets + ')</button>'
      + '<button class="tab-btn" onclick="showTab(\'json\')">Raw JSON</button>'
      + '</div>';
  }

  // -------------------------------------------------------------------------
  // Flamegraph tab with bottom bar.
  // -------------------------------------------------------------------------

  function buildFlamegraphTab() {
    return '<div id="tab-flamegraph" class="tab-content active">'
      + buildToolbar()
      + '<div id="depflame-summary-bar" class="depflame-summary-bar"></div>'
      + '<div class="flamegraph-main">'
      +   '<div class="flamegraph-wrap"><div id="flamegraph-container"></div></div>'
      +   '<div class="feature-sidebar" id="feature-sidebar">'
      +     '<div class="feature-sidebar-header">'
      +       '<select id="feature-crate-select" onchange="DepflameFeatures.selectCrate(this.value)"></select>'
      +     '</div>'
      +     '<div class="feature-sidebar-body" id="feature-sidebar-body"></div>'
      +     '<div class="feature-sidebar-footer" id="feature-sidebar-footer"></div>'
      +   '</div>'
      + '</div>'
      + '</div>';
  }

  function buildToolbar() {
    return '<div class="bottom-bar">'
      + '<div class="controls-left">'
      +   '<div class="search-box">'
      +     icons.search
      +     '<input type="text" placeholder="Search crates..." id="flame-search-input">'
      +   '</div>'
      +   '<button class="control-btn" onclick="Depflame.resetZoom()">Reset Zoom</button>'
      +   '<button class="control-btn" id="reverse-btn" onclick="Depflame.toggleReverse()">Reverse</button>'
      +   '<label class="ancestor-filter-label" title="Hide dependencies with more than N unique ancestors (hard to remove).">'
      +     'Max ancestors: <input type="number" id="ancestor-filter-input" min="0" value="" placeholder="off"'
      +     ' style="width:50px" onchange="Depflame.setAncestorFilter(this.value)">'
      +   '</label>'
      +   '<span id="search-matches"></span>'
      + '</div>'
      + '<div class="legend-right">'
      +   '<div class="legend-item"><div class="legend-color legend-workspace"></div> Workspace</div>'
      +   '<div class="legend-item"><div class="legend-color legend-leaf"></div> Leaf</div>'
      +   '<div class="legend-item"><div class="legend-color legend-normal"></div> Normal</div>'
      +   '<div class="legend-item"><div class="legend-color legend-shared"></div> Shared</div>'
      +   '<div class="legend-item"><div class="legend-color legend-unused"></div> Unused</div>'
      + '</div>'
      + '</div>';
  }

  // -------------------------------------------------------------------------
  // Table tab: direct dep summary.
  // -------------------------------------------------------------------------

  // Current sort state for the dep summary table.
  var depSortKey = 'unique_transitive_deps';
  var depSortAsc = false; // default: descending by unique deps

  function buildTableTab(r) {
    var summary = r.direct_dep_summary || [];
    if (summary.length === 0) {
      return '<p class="text-light">No direct dependency data available.</p>';
    }

    var html = '<div class="action-summary">'
      + '<h3>Direct dependencies by unique transitive dep count</h3>'
      + '<p class="text-section-desc">'
      + 'Each row shows a direct dependency of your workspace. '
      + '<em>Unique Deps</em> = transitive deps that vanish if removed. '
      + '<em>Total Deps</em> includes shared ones. '
      + 'Click a column header to sort.</p>'
      + '<table class="targets-table dep-summary-table" id="dep-summary-table"><thead><tr>'
      + '<th class="sortable" data-sort-key="#">#</th>'
      + '<th class="sortable" data-sort-key="dep_name">Dependency</th>'
      + '<th class="sortable" data-sort-key="dep_version">Version</th>'
      + '<th class="sortable sort-active sort-desc" data-sort-key="unique_transitive_deps"'
      + ' title="Transitive deps unique to this edge.">Unique Deps</th>'
      + '<th class="sortable" data-sort-key="total_transitive_deps"'
      + ' title="Total transitive deps.">Total Deps</th>'
      + '<th class="sortable" data-sort-key="unique_ancestors"'
      + ' title="Number of unique ancestor packages that transitively depend on this crate. High = hard to remove.">Ancestors</th>'
      + '</tr></thead><tbody id="dep-summary-tbody">';

    html += renderDepSummaryRows(summary);
    html += '</tbody></table></div>';
    return html;
  }

  function renderDepSummaryRows(summary) {
    var maxUnique = 1;
    for (var i = 0; i < summary.length; i++) {
      if (summary[i].unique_transitive_deps > maxUnique) maxUnique = summary[i].unique_transitive_deps;
    }
    var html = '';
    for (var i = 0; i < summary.length; i++) {
      var e = summary[i];
      var barW = maxUnique > 0 ? Math.round(e.unique_transitive_deps / maxUnique * 100) : 0;
      html += '<tr><td>' + (i + 1) + '</td>'
        + '<td><code>' + crateLink(e.dep_name) + '</code></td>'
        + '<td>' + esc(e.dep_version) + '</td>'
        + '<td><div class="dep-summary-cell">'
        + '<div class="unique-bar" style="width:' + barW + '%"></div>'
        + '<span>' + e.unique_transitive_deps + '</span></div></td>'
        + '<td>' + e.total_transitive_deps + '</td>'
        + '<td>' + (e.unique_ancestors || 0) + '</td></tr>';
    }
    return html;
  }

  function sortDepSummary(key) {
    var report = window.__DEPFLAME_REPORT__;
    if (!report) return;
    var summary = report.direct_dep_summary;
    if (!summary || summary.length === 0) return;

    // Toggle direction if clicking same column, otherwise default desc for numbers, asc for strings.
    if (key === depSortKey) {
      depSortAsc = !depSortAsc;
    } else {
      depSortKey = key;
      depSortAsc = (key === 'dep_name' || key === 'dep_version');
    }

    // Sort a copy with stable indices for the '#' column.
    var indexed = summary.map(function(e, i) { return { entry: e, origIdx: i }; });

    if (key === '#') {
      // Sort by original index (restore default order from analysis).
      indexed.sort(function(a, b) {
        return depSortAsc ? a.origIdx - b.origIdx : b.origIdx - a.origIdx;
      });
    } else {
      indexed.sort(function(a, b) {
        var va = a.entry[key], vb = b.entry[key];
        if (va == null) va = 0;
        if (vb == null) vb = 0;
        var cmp;
        if (typeof va === 'string') {
          cmp = va.localeCompare(vb);
        } else {
          cmp = va - vb;
        }
        return depSortAsc ? cmp : -cmp;
      });
    }

    var sorted = indexed.map(function(item) { return item.entry; });

    // Update header classes.
    var table = document.getElementById('dep-summary-table');
    if (table) {
      var ths = table.querySelectorAll('th.sortable');
      for (var i = 0; i < ths.length; i++) {
        ths[i].classList.remove('sort-active', 'sort-asc', 'sort-desc');
        if (ths[i].getAttribute('data-sort-key') === key) {
          ths[i].classList.add('sort-active', depSortAsc ? 'sort-asc' : 'sort-desc');
        }
      }
    }

    // Re-render tbody.
    var tbody = document.getElementById('dep-summary-tbody');
    if (tbody) {
      tbody.innerHTML = renderDepSummaryRows(sorted);
    }
  }

  // -------------------------------------------------------------------------
  // Suggestions tab.
  // -------------------------------------------------------------------------

  function suggestionType(t) {
    if (typeof t.suggestion === 'string') return t.suggestion;
    return Object.keys(t.suggestion)[0];
  }

  function suggestionDetail(t) {
    if (typeof t.suggestion === 'string') return {};
    var key = Object.keys(t.suggestion)[0];
    return t.suggestion[key] || {};
  }

  function suggestionDisplay(t) {
    var stype = suggestionType(t);
    switch (stype) {
      case 'Remove': return 'Remove';
      case 'FeatureGate': return 'Feature Gate';
      case 'AlreadyGated': return 'Already Gated';
      case 'MoveToDevDeps': return 'Move to Dev';
      case 'ReplaceWithStd': return 'Replace with Std';
      case 'ReplaceWithLighter': return 'Replace';
      case 'RequiredBySibling': return 'Required by Sibling';
      case 'InlineUpstream': return 'Inline';
      default: return stype;
    }
  }

  function categorize(t) {
    var stype = suggestionType(t);
    if (stype === 'Remove' && t.intermediate_is_workspace_member) return 0;
    if (stype === 'AlreadyGated') return 1;
    if (!t.intermediate_is_workspace_member) return 3;
    if (stype === 'RequiredBySibling') return 4;
    return 2;
  }

  var sectionDefs = [
    { title: 'Remove unused dependencies',
      desc: 'These dependencies are in your <code>Cargo.toml</code> but no references were found in your source code.' },
    { title: 'Disable unnecessary features',
      desc: 'These dependencies are already <em>optional</em> upstream but pulled in by a feature you have enabled.' },
    { title: 'Make dependencies optional',
      desc: 'These dependencies could be made optional by adding <code>optional = true</code> in your <code>Cargo.toml</code>.' },
    { title: 'Proposals for upstream libraries',
      desc: 'These changes would need to happen in an external library\'s repository (issue or PR).' },
    { title: 'Not actionable',
      desc: 'These dependencies can\'t be easily removed because they\'re required by sibling deps.' }
  ];

  function formatActionLine(t) {
    var prefix = '(-' + t.w_unique + ' deps)';
    var confClass = 'badge-' + t.confidence.toLowerCase();
    var confBadge = ' <span class="badge ' + confClass + '">' + esc(t.confidence) + '</span>';
    var heavy = crateLink(t.heavy_dependency.name);
    var inter = crateLink(t.intermediate.name);
    var stype = suggestionType(t);
    var detail = suggestionDetail(t);

    switch (stype) {
      case 'Remove':
        return confBadge + ' ' + prefix + ' Remove ' + heavy + ' from ' + inter + ' &mdash; it appears unused';
      case 'FeatureGate':
        if (t.intermediate_is_workspace_member) {
          return confBadge + ' ' + prefix + ' Make ' + heavy + ' optional in ' + inter + ' &mdash; put it behind a feature flag';
        }
        return confBadge + ' ' + prefix + ' Propose making ' + heavy + ' optional in ' + inter;
      case 'AlreadyGated':
        var featHint = '';
        var ef = detail.enabling_features || [];
        if (ef.length > 0) {
          var feats = ef.map(function(f) { return '<code>' + esc(f) + '</code>'; }).join(', ');
          if (detail.recommended_defaults != null) {
            featHint = ' &mdash; default feature(s) ' + feats + ' pull it in; disable defaults and keep only what you need';
          } else {
            featHint = ' &mdash; enabled by feature(s) ' + feats;
          }
        }
        return confBadge + ' ' + prefix + ' ' + heavy + ' is already optional in ' + inter
          + ' (' + esc(detail.detail || 'already optional') + ')' + featHint;
      case 'ReplaceWithStd':
        return confBadge + ' ' + prefix + ' Replace ' + heavy + ' with <code>' + esc(detail.suggestion || '') + '</code> in ' + inter;
      case 'ReplaceWithLighter':
        return confBadge + ' ' + prefix + ' Switch from ' + heavy + ' to <code>' + esc(detail.alternative || '') + '</code> in ' + inter;
      case 'RequiredBySibling':
        return confBadge + ' ' + prefix + ' ' + heavy + ' can\'t be removed &mdash; required by ' + crateLink(detail.sibling || '');
      case 'MoveToDevDeps':
        return confBadge + ' ' + prefix + ' Move ' + heavy + ' to <code>[dev-dependencies]</code> in ' + inter;
      case 'InlineUpstream':
        return confBadge + ' ' + prefix + ' Copy code from ' + heavy + ' into ' + inter
          + ' &mdash; only ' + (detail.api_items_used || '?') + ' API items used';
      default:
        return confBadge + ' ' + prefix + ' ' + stype + ' ' + heavy + ' in ' + inter;
    }
  }

  function buildCargoDiff(t) {
    var dep = esc(t.heavy_dependency.name);
    var depVer = esc(t.heavy_dependency.version);
    var inter = esc(t.intermediate.name);
    var toml = t.intermediate_is_workspace_member ? 'Cargo.toml' : inter + '/Cargo.toml';
    var stype = suggestionType(t);
    var detail = suggestionDetail(t);

    var lines = [];
    function file(s) { lines.push('<div class="diff-file">' + s + '</div>'); }
    function rm(s)   { lines.push('<div class="diff-rm">' + s + '</div>'); }
    function add(s)  { lines.push('<div class="diff-add">' + s + '</div>'); }
    function cmt(s)  { lines.push('<div class="diff-comment">' + s + '</div>'); }

    switch (stype) {
      case 'Remove':
        file('# ' + toml);
        rm('- ' + dep + ' = "' + depVer + '"');
        break;
      case 'FeatureGate':
        var feat = 'use-' + dep;
        file('# ' + toml + ' &mdash; [dependencies]');
        rm('- ' + dep + ' = "' + depVer + '"');
        add('+ ' + dep + ' = { version = "' + depVer + '", optional = true }');
        cmt('');
        cmt('# add a feature flag so users can opt in:');
        file('# ' + toml + ' &mdash; [features]');
        add('+ ' + feat + ' = ["dep:' + dep + '"]');
        break;
      case 'AlreadyGated':
        var ef = (detail.enabling_features || []).map(function(f) { return esc(f); });
        var rd = detail.recommended_defaults;
        file('# Cargo.toml');
        if (ef.length === 0) {
          cmt('# check your [' + inter + '] dep &mdash; a feature is pulling in ' + dep);
          rm('- ' + inter + ' = { version = "...", features = ["..."] }');
          add('+ ' + inter + ' = { version = "..." }');
        } else if (rd != null) {
          var bad = ef.map(function(f) { return '"' + f + '"'; }).join(', ');
          cmt('# default feature(s) ' + bad + ' of ' + inter + ' pull in ' + dep);
          rm('- ' + inter + ' = "..."');
          if (rd.length === 0) {
            add('+ ' + inter + ' = { version = "...", default-features = false }');
          } else {
            var keep = rd.map(function(f) { return '"' + esc(f) + '"'; }).join(', ');
            add('+ ' + inter + ' = { version = "...", default-features = false, features = [' + keep + '] }');
          }
        } else {
          var feats = ef.map(function(f) { return '"' + f + '"'; }).join(', ');
          cmt('# feature(s) ' + feats + ' of ' + inter + ' pull in ' + dep);
          rm('- ' + inter + ' = { version = "...", features = [' + feats + '] }');
          add('+ ' + inter + ' = { version = "..." }');
        }
        break;
      case 'ReplaceWithStd':
        file('# ' + toml);
        rm('- ' + dep + ' = "' + depVer + '"');
        cmt('# replace usage with ' + esc(detail.suggestion || ''));
        break;
      case 'ReplaceWithLighter':
        file('# ' + toml);
        rm('- ' + dep + ' = "' + depVer + '"');
        add('+ ' + esc(detail.alternative || '???') + ' = "..."');
        break;
      case 'InlineUpstream':
        file('# ' + toml);
        rm('- ' + dep + ' = "' + depVer + '"');
        cmt('# copy the items you use directly into your code');
        break;
      case 'MoveToDevDeps':
        file('# ' + toml + ' &mdash; move from [dependencies] to [dev-dependencies]');
        rm('- ' + dep + ' = "' + depVer + '"  # under [dependencies]');
        add('+ ' + dep + ' = "' + depVer + '"  # under [dev-dependencies]');
        break;
      default:
        return '';
    }
    if (stype === 'RequiredBySibling') return '';
    return '<div class="cargo-diff">' + lines.join('\n') + '</div>';
  }

  function buildSuggestionsTab(r) {
    var targets = r.targets || [];
    // Filter out noise-confidence targets and sort by confidence (High first).
    var confOrder = { 'High': 0, 'Medium': 1, 'Low': 2 };
    var actionableTargets = [];
    for (var i = 0; i < targets.length; i++) {
      if (targets[i].confidence !== 'Noise') {
        actionableTargets.push({ target: targets[i], originalIdx: i });
      }
    }
    actionableTargets.sort(function(a, b) {
      var ca = confOrder[a.target.confidence] != null ? confOrder[a.target.confidence] : 9;
      var cb = confOrder[b.target.confidence] != null ? confOrder[b.target.confidence] : 9;
      if (ca !== cb) return ca - cb;
      return b.target.w_unique - a.target.w_unique;
    });

    if (actionableTargets.length === 0) {
      return '<div class="action-summary"><p class="text-light">No actionable suggestions found.</p></div>';
    }

    var html = '<div class="disclaimer">'
      + '<strong>\u26a0 Use your judgement.</strong> '
      + 'These suggestions are based on automated analysis. They may be wrong or impractical. '
      + 'Before acting, make sure you understand why the dependency exists.</div>';

    // Categorize.
    var sections = [];
    for (var s = 0; s < sectionDefs.length; s++) {
      sections.push({ title: sectionDefs[s].title, desc: sectionDefs[s].desc, items: [] });
    }
    for (var i = 0; i < actionableTargets.length; i++) {
      var t = actionableTargets[i].target;
      var cat = categorize(t);
      var isLocal = t.intermediate_is_workspace_member
        || suggestionType(t) === 'AlreadyGated'
        || suggestionType(t) === 'RequiredBySibling';
      var badge = isLocal ? '' : ' <span title="Requires PR to upstream" class="upstream-badge">\ud83d\udce4</span>';
      sections[cat].items.push({ idx: actionableTargets[i].originalIdx, badge: badge, isLocal: isLocal });
    }

    var anyRendered = false;
    for (var s = 0; s < sections.length; s++) {
      if (sections[s].items.length === 0) continue;
      anyRendered = true;
      html += '<div class="action-summary"><h3>' + sections[s].title + '</h3>'
        + '<p class="text-section-desc">' + sections[s].desc + '</p><ul>';
      for (var j = 0; j < sections[s].items.length; j++) {
        var item = sections[s].items[j];
        var t = targets[item.idx];
        var line = formatActionLine(t);
        var diff = item.isLocal ? buildCargoDiff(t) : '';
        if (diff) {
          html += '<li>' + line + item.badge
            + ' <span class="show-diff-btn" onclick="toggleDiff(this.parentElement)">show diff</span>'
            + diff + '</li>';
        } else {
          html += '<li>' + line + item.badge + '</li>';
        }
      }
      html += '</ul></div>';
    }
    if (!anyRendered) {
      html += '<div class="action-summary"><p class="text-light">No actionable suggestions found.</p></div>';
    }

    // Detail table.
    html += '<div class="detail-section">'
      + '<p class="text-section-title"><strong>Detailed breakdown</strong></p>'
      + '<p class="text-section-desc">Click any row to see source references.</p></div>';

    html += '<table class="targets-table"><thead><tr>'
      + '<th>#</th>'
      + '<th title="Crate where the change would happen.">Upstream Crate</th>'
      + '<th title="The heavy dependency being pulled in.">Heavy Dep</th>'
      + '<th title="Deps saved if this edge were cut.">Deps Saved</th>'
      + '<th title="Total transitive deps of the heavy dep.">Total Deps</th>'
      + '<th title="Source references found.">Code Refs</th>'
      + '<th title="W_transitive / C_ref.">Score</th>'
      + '<th title="Confidence level.">Confidence</th>'
      + '<th title="Suggested action.">Suggested Action</th>'
      + '</tr></thead><tbody>';

    for (var i = 0; i < targets.length; i++) {
      var t = targets[i];
      if (t.confidence === 'Noise') continue;
      var idx = i + 1;
      var confClass = 'badge-' + t.confidence.toLowerCase();
      var hurrs = t.hurrs == null ? '\u221e' : t.hurrs.toFixed(1);
      var isLocal = t.intermediate_is_workspace_member
        || suggestionType(t) === 'AlreadyGated'
        || suggestionType(t) === 'RequiredBySibling';
      var upBadge = isLocal ? '' : ' <span title="Requires upstream PR">\ud83d\udce4</span>';

      html += '<tr class="expandable" onclick="toggleDetail(' + idx + ')">'
        + '<td>' + idx + '</td>'
        + '<td><code>' + crateLink(t.intermediate.name) + '</code>' + upBadge + '</td>'
        + '<td><code>' + crateLink(t.heavy_dependency.name) + '</code></td>'
        + '<td>' + t.w_unique + '</td><td>' + t.w_transitive + '</td><td>' + t.c_ref + '</td>'
        + '<td>' + hurrs + '</td>'
        + '<td><span class="badge ' + confClass + '">' + esc(t.confidence) + '</span></td>'
        + '<td>' + esc(suggestionDisplay(t)) + '</td></tr>';

      // Detail row.
      var flags = [];
      if (t.phantom) flags.push('PHANTOM');
      if (t.intermediate_is_workspace_member) flags.push('YOUR-CRATE');
      if (t.edge_meta.build_only) flags.push('BUILD-ONLY');
      if (t.edge_meta.already_optional) flags.push('ALREADY-OPTIONAL');
      if (t.edge_meta.platform_conditional) flags.push('PLATFORM-CONDITIONAL');
      if (t.has_re_export_all) flags.push('RE-EXPORTS-ALL');

      var flagsHtml = flags.length === 0
        ? '<span class="text-disabled">none</span>'
        : flags.map(function(f) { return '<span class="badge badge-flag">' + f + '</span>'; }).join(' ');

      var chainHtml = (t.dep_chain || []).map(crateLink).join(' &rarr; ');

      var refsHtml = '';
      var matches = t.scan_result.file_matches || [];
      for (var m = 0; m < matches.length; m++) {
        var fm = matches[m];
        var dp = displayPath(fm.path);
        var gen = fm.in_generated_file ? ' <span class="text-generated">[generated]</span>' : '';
        refsHtml += '<div class="ref-file">' + esc(dp) + ':' + fm.line_number + gen + '</div>'
          + '<div class="ref-line"><code>' + esc(fm.line_content) + '</code></div>';
      }
      if (!refsHtml) refsHtml = '<span class="text-disabled">no references found</span>';

      html += '<tr class="detail-row" id="detail-' + idx + '">'
        + '<td colspan="9"><div class="detail-box">'
        + '<div><span class="label">Edge:</span> ' + crateLink(t.intermediate.name) + ' v' + esc(t.intermediate.version)
        + ' &rarr; ' + crateLink(t.heavy_dependency.name) + ' v' + esc(t.heavy_dependency.version) + '</div>'
        + '<div><span class="label">Flags:</span> ' + flagsHtml + '</div>'
        + '<div><span class="label">Chain:</span> ' + chainHtml + '</div>'
        + '<div class="detail-refs-header"><span class="label">References (' + t.scan_result.ref_count + '):</span></div>'
        + '<div class="detail-refs-body">' + refsHtml + '</div>'
        + '</div></td></tr>';
    }

    html += '</tbody></table>';
    return html;
  }

  // -------------------------------------------------------------------------
  // JSON tab.
  // -------------------------------------------------------------------------

  function buildJsonTab(r) {
    var json = JSON.stringify(r, null, 2);
    return '<div class="json-container">'
      + '<button class="copy-btn" onclick="copyJson()">Copy</button>'
      + '<pre><code>' + esc(json) + '</code></pre></div>';
  }

  // -------------------------------------------------------------------------
  // Helpers.
  // -------------------------------------------------------------------------

  function displayPath(path) {
    var m = path.match(/\.cargo\/registry\/src\/[^/]+\/(.+)/);
    if (m) return m[1];
    return path.replace(/^\/home\/[^/]+\//, '~/');
  }

  // -------------------------------------------------------------------------
  // Main: build and inject all content.
  // -------------------------------------------------------------------------

  function init() {
    var report = window.__DEPFLAME_REPORT__;
    if (!report) {
      document.getElementById('app').innerHTML = '<p>No report data found.</p>';
      return;
    }

    var app = document.getElementById('app');
    if (!app) return;

    var nActionable = 0;
    for (var i = 0; i < report.targets.length; i++) {
      if (report.targets[i].confidence !== 'Noise') nActionable++;
    }
    var html = buildHeader(report);
    html += buildTabs(nActionable);

    // Tab contents.
    html += buildFlamegraphTab();
    html += '<div id="tab-table" class="tab-content">' + buildTableTab(report) + '</div>';
    html += '<div id="tab-targets" class="tab-content">' + buildSuggestionsTab(report) + '</div>';
    html += '<div id="tab-json" class="tab-content">' + buildJsonTab(report) + '</div>';

    app.innerHTML = html;

    // Wire up search input.
    var searchInput = document.getElementById('flame-search-input');
    if (searchInput) {
      searchInput.addEventListener('input', function() {
        var q = this.value;
        if (!q) { Depflame.clearSearch(); return; }
        Depflame.searchByQuery(q);
      });
      searchInput.addEventListener('keydown', function(e) {
        if (e.key === 'Escape') { this.value = ''; Depflame.clearSearch(); }
      });
    }

    // Wire up sortable table headers.
    var sortHeaders = document.querySelectorAll('#dep-summary-table th.sortable');
    for (var i = 0; i < sortHeaders.length; i++) {
      sortHeaders[i].addEventListener('click', function() {
        sortDepSummary(this.getAttribute('data-sort-key'));
      });
    }

    // Initialize flamegraph — filter to only nodes whose features are
    // actually enabled (the all-features tree includes optional deps that
    // may not be active in the user's current build).
    var data = report.dep_tree;
    if (data) {
      window.__DEPFLAME_DATA__ = data;
      window.__DEPFLAME_TOTAL_DEPS__ = report.total_dependencies;
      window.__DEPFLAME_UNUSED_EDGES__ = report.unused_edges || [];
      try {
        var initial = DepflameFeatures.recomputeActiveGraph(data);
        for (var i = 0; i < data.nodes.length; i++) {
          data.nodes[i].transitive_weight = initial.weights[i] || 0;
        }
        Depflame.render(
          document.getElementById('flamegraph-container'),
          data,
          report.unused_edges || [],
          initial.activeNodes,
          initial.activeEdges
        );
        // Count active non-workspace deps for the summary bar.
        var initActiveDeps = 0;
        for (var idx in initial.activeNodes) {
          if (!data.nodes[parseInt(idx, 10)].is_workspace) initActiveDeps++;
        }
        // Initialize summary bar and feature sidebar.
        DepflameFeatures.initBaseline(data, initActiveDeps);
        DepflameFeatures.populateDropdown(data);
      } catch (e) {
        console.error('Flamegraph render error:', e);
        document.getElementById('flamegraph-container').innerHTML =
          '<p class="flamegraph-error">Failed to render flamegraph: ' + esc(e.message) + '</p>';
      }
    } else {
      document.getElementById('flamegraph-container').innerHTML =
        '<p>No dependency tree data available.</p>';
    }
  }

  return { init: init };
})();
