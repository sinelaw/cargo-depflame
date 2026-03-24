// ---------------------------------------------------------------------------
// depflame — Feature toggle UI and dependency graph recomputation.
// ---------------------------------------------------------------------------

var DepflameFeatures = (function() {
  'use strict';

  // The user's feature overrides: Map<nodeIndex, Set<featureName>>.
  // If a node index is present, those are the features to use instead of defaults.
  var featureOverrides = {};

  // Snapshot of the original transitive_weight values (before any recomputation).
  var originalWeights = null;
  var originalTotalDeps = 0;

  // -------------------------------------------------------------------------
  // Feature resolution engine.
  // -------------------------------------------------------------------------

  // Build a lookup: edge key "from:to" -> edge info.
  function buildEdgeMap(treeData) {
    var map = {};
    if (!treeData.edges) return map;
    for (var i = 0; i < treeData.edges.length; i++) {
      var e = treeData.edges[i];
      map[e.from + ':' + e.to] = e;
    }
    return map;
  }

  // Resolve features within a single node: if feature A activates sub-feature B
  // (an entry that doesn't contain '/' or 'dep:'), recursively include B.
  function resolveNodeFeatures(node, initialFeatures) {
    var enabled = {};
    for (var i = 0; i < initialFeatures.length; i++) {
      enabled[initialFeatures[i]] = true;
    }

    var changed = true;
    var iterations = 0;
    while (changed && iterations < 100) {
      changed = false;
      iterations++;
      var feats = Object.keys(enabled);
      for (var i = 0; i < feats.length; i++) {
        var activates = node.available_features[feats[i]];
        if (!activates) continue;
        for (var j = 0; j < activates.length; j++) {
          var entry = activates[j];
          // Sub-feature: doesn't start with "dep:" and doesn't contain "/"
          if (entry.indexOf('/') === -1 && entry.indexOf('dep:') !== 0) {
            if (!enabled[entry]) {
              enabled[entry] = true;
              changed = true;
            }
          }
        }
      }
    }
    return enabled;
  }

  // Determine which optional deps a node's enabled features activate.
  // Returns a set of child node names that are activated.
  function activatedOptionalDeps(node, resolvedFeatures) {
    var activated = {};
    var feats = Object.keys(resolvedFeatures);
    for (var i = 0; i < feats.length; i++) {
      var activates = node.available_features[feats[i]];
      if (!activates) continue;
      for (var j = 0; j < activates.length; j++) {
        var entry = activates[j];
        if (entry.indexOf('dep:') === 0) {
          activated[entry.substring(4)] = true;
        } else if (entry.indexOf('/') !== -1) {
          // "child/feature" -> activates child
          activated[entry.split('/')[0]] = true;
        }
      }
    }
    // Also: the feature name itself might match a dep name (implicit dep: syntax).
    for (var i = 0; i < feats.length; i++) {
      var feat = feats[i];
      // Check if this feature name matches any child node name.
      var children = node.children || [];
      for (var c = 0; c < children.length; c++) {
        // We'll check against node names in the caller.
      }
    }
    return activated;
  }

  // Recompute which nodes are active given feature overrides.
  // Returns { activeNodes: {idx: true}, weights: {idx: weight} }.
  function recomputeActiveGraph(treeData) {
    var edgeMap = buildEdgeMap(treeData);
    var nodes = treeData.nodes;
    var n = nodes.length;

    // Step 1: For each node, determine effective enabled features.
    var effectiveFeatures = new Array(n);
    for (var i = 0; i < n; i++) {
      var initial = featureOverrides[i] !== undefined
        ? featureOverrides[i]
        : nodes[i].enabled_features || [];
      effectiveFeatures[i] = resolveNodeFeatures(nodes[i], initial);
    }

    // Step 2: Determine which edges are active.
    // An edge from->to is active if:
    //   (a) the edge is NOT optional, OR
    //   (b) the edge IS optional AND the gating feature is enabled on the parent,
    //       OR the parent's resolved features activate "dep:child_name".
    var activeEdges = {};
    for (var i = 0; i < n; i++) {
      var children = nodes[i].children || [];
      var activatedDeps = activatedOptionalDeps(nodes[i], effectiveFeatures[i]);

      for (var c = 0; c < children.length; c++) {
        var ci = children[c];
        var key = i + ':' + ci;
        var edge = edgeMap[key];

        if (!edge || !edge.is_optional) {
          // Non-optional edge: always active.
          activeEdges[key] = true;
        } else {
          // Optional edge: active if the resolved features activate this dep.
          // Check all activation paths:
          //   1. Explicit: a resolved feature activates "dep:child_name" or "child_name/feat"
          //   2. Implicit: a resolved feature name matches the child dep name
          //      (Cargo's implicit optional dep features)
          var childName = nodes[ci].name;
          var active = activatedDeps[childName] || effectiveFeatures[i][childName];
          if (active) {
            activeEdges[key] = true;
          }
        }
      }
    }

    // Step 3: BFS from roots through active edges only.
    var activeNodes = {};
    var queue = [];
    for (var i = 0; i < treeData.root_indices.length; i++) {
      var ri = treeData.root_indices[i];
      activeNodes[ri] = true;
      queue.push(ri);
    }
    while (queue.length > 0) {
      var idx = queue.shift();
      var children = nodes[idx].children || [];
      for (var c = 0; c < children.length; c++) {
        var ci = children[c];
        var key = idx + ':' + ci;
        if (activeEdges[key] && !activeNodes[ci]) {
          activeNodes[ci] = true;
          queue.push(ci);
        }
      }
    }

    // Step 4: Recompute transitive weights for active nodes.
    var weights = {};
    var weightCache = {};

    function computeWeight(idx) {
      if (weightCache[idx] !== undefined) return weightCache[idx];
      if (!activeNodes[idx]) { weightCache[idx] = 0; return 0; }

      var visited = {};
      var stack = [idx];
      var count = 0;
      while (stack.length > 0) {
        var cur = stack.pop();
        if (visited[cur]) continue;
        visited[cur] = true;
        if (!activeNodes[cur]) continue;
        count++;
        var children = nodes[cur].children || [];
        for (var c = 0; c < children.length; c++) {
          var ci = children[c];
          if (!visited[ci] && activeNodes[ci] && activeEdges[cur + ':' + ci]) {
            stack.push(ci);
          }
        }
      }
      weightCache[idx] = count;
      return count;
    }

    for (var idx in activeNodes) {
      weights[idx] = computeWeight(parseInt(idx, 10));
    }

    return { activeNodes: activeNodes, weights: weights, activeEdges: activeEdges };
  }

  // Apply recomputed weights to the tree data (mutates transitive_weight).
  // Returns the data needed for re-rendering.
  function applyRecomputation(treeData) {
    var result = recomputeActiveGraph(treeData);

    // Update transitive weights.
    for (var i = 0; i < treeData.nodes.length; i++) {
      if (result.weights[i] !== undefined) {
        treeData.nodes[i].transitive_weight = result.weights[i];
      } else {
        treeData.nodes[i].transitive_weight = 0;
      }
    }

    // Count active non-workspace nodes.
    var activeDeps = 0;
    for (var idx in result.activeNodes) {
      var i = parseInt(idx, 10);
      if (!treeData.nodes[i].is_workspace) activeDeps++;
    }

    return { activeNodes: result.activeNodes, activeEdges: result.activeEdges, activeDeps: activeDeps };
  }

  // Reset to original weights.
  function resetWeights(treeData) {
    if (!originalWeights) return;
    for (var i = 0; i < treeData.nodes.length; i++) {
      if (originalWeights[i] !== undefined) {
        treeData.nodes[i].transitive_weight = originalWeights[i];
      }
    }
  }

  // -------------------------------------------------------------------------
  // Feature sidebar UI.
  // -------------------------------------------------------------------------

  var currentPanelNode = -1;

  function buildNodeNameSet(treeData) {
    var names = {};
    for (var i = 0; i < treeData.nodes.length; i++) {
      names[treeData.nodes[i].name] = true;
    }
    return names;
  }

  // Build reverse dep map: nodeIdx -> [parent nodeIdx, ...].
  // Cached to avoid recomputing on every selectCrate call.
  var reverseDepCache = null;
  var reverseDepTreeId = null;

  function getReverseDepMap(treeData) {
    // Simple cache: recompute only when treeData changes.
    if (reverseDepCache && reverseDepTreeId === treeData) return reverseDepCache;
    var rev = {};
    for (var i = 0; i < treeData.nodes.length; i++) {
      var children = treeData.nodes[i].children || [];
      for (var c = 0; c < children.length; c++) {
        var ci = children[c];
        if (!rev[ci]) rev[ci] = [];
        rev[ci].push(i);
      }
    }
    reverseDepCache = rev;
    reverseDepTreeId = treeData;
    return rev;
  }

  var PARENT_CAP = 10;

  function buildParentsHtml(treeData, nodeIdx) {
    var revMap = getReverseDepMap(treeData);
    var parentIndices = revMap[nodeIdx] || [];
    if (parentIndices.length === 0) return '';

    // Deduplicate and sort by name.
    var seen = {};
    var parents = [];
    for (var i = 0; i < parentIndices.length; i++) {
      var pi = parentIndices[i];
      if (seen[pi]) continue;
      seen[pi] = true;
      parents.push({ idx: pi, name: treeData.nodes[pi].name });
    }
    parents.sort(function(a, b) { return a.name < b.name ? -1 : 1; });

    var html = '<div class="feature-parents">'
      + '<div class="feature-parents-label">Depended on by (' + parents.length + ')</div>';

    var showCount = Math.min(parents.length, PARENT_CAP);
    for (var i = 0; i < showCount; i++) {
      html += '<span class="feature-parent-link" data-node-idx="' + parents[i].idx + '">'
        + escHtml(parents[i].name) + '</span>';
      if (i < showCount - 1) html += ', ';
    }

    if (parents.length > PARENT_CAP) {
      var remaining = parents.length - PARENT_CAP;
      html += '<span class="feature-parents-more" onclick="this.parentNode.querySelector(\'.feature-parents-overflow\').style.display=\'inline\';this.style.display=\'none\'"> and '
        + remaining + ' more...</span>';
      html += '<span class="feature-parents-overflow">';
      for (var i = PARENT_CAP; i < parents.length; i++) {
        html += ', <span class="feature-parent-link" data-node-idx="' + parents[i].idx + '">'
          + escHtml(parents[i].name) + '</span>';
      }
      html += '</span>';
    }

    html += '<div class="feature-parents-actions">'
      + '<button class="feature-panel-btn feature-reverse-link" data-node-idx="' + nodeIdx + '">View reverse deps</button>'
      + '</div>';

    html += '</div>';
    return html;
  }

  function wireParentLinks(container) {
    var parentLinks = container.querySelectorAll('.feature-parent-link');
    for (var i = 0; i < parentLinks.length; i++) {
      parentLinks[i].addEventListener('click', function(evt) {
        var targetIdx = parseInt(evt.target.getAttribute('data-node-idx'), 10);
        if (!isNaN(targetIdx)) {
          var select = document.getElementById('feature-crate-select');
          if (select) select.value = targetIdx;
          selectCrate(targetIdx);
        }
      });
    }
    var reverseLink = container.querySelector('.feature-reverse-link');
    if (reverseLink) {
      reverseLink.addEventListener('click', function(evt) {
        var targetIdx = parseInt(evt.target.getAttribute('data-node-idx'), 10);
        if (!isNaN(targetIdx)) {
          Depflame.zoomReverse(targetIdx);
        }
      });
    }
  }

  function featureAffectsGraph(activates, nodeNames, availableFeatures) {
    if (!activates) return false;
    for (var i = 0; i < activates.length; i++) {
      var a = activates[i];
      if (a.indexOf('dep:') === 0) {
        if (nodeNames[a.substring(4)]) return true;
      } else if (a.indexOf('/') !== -1) {
        if (nodeNames[a.split('/')[0]]) return true;
      } else {
        if (nodeNames[a]) return true;
        if (availableFeatures && availableFeatures[a]) {
          if (featureAffectsGraph(availableFeatures[a], nodeNames, availableFeatures)) return true;
        }
      }
    }
    return false;
  }

  function isImplicitDepFeature(fname, activates) {
    return activates && activates.length === 1 && activates[0] === 'dep:' + fname;
  }

  // Populate the crate dropdown with all nodes, grouped by type.
  function populateDropdown(treeData) {
    var select = document.getElementById('feature-crate-select');
    if (!select) return;
    select.innerHTML = '';

    var wsNodes = [];
    var otherNodes = [];
    for (var i = 0; i < treeData.nodes.length; i++) {
      var n = treeData.nodes[i];
      var entry = { idx: i, name: n.name, version: n.version };
      if (n.is_workspace) {
        wsNodes.push(entry);
      } else {
        otherNodes.push(entry);
      }
    }
    wsNodes.sort(function(a, b) { return a.name < b.name ? -1 : 1; });
    otherNodes.sort(function(a, b) { return a.name < b.name ? -1 : 1; });

    if (wsNodes.length > 0) {
      var grp = document.createElement('optgroup');
      grp.label = 'Workspace';
      for (var i = 0; i < wsNodes.length; i++) {
        var opt = document.createElement('option');
        opt.value = wsNodes[i].idx;
        opt.textContent = wsNodes[i].name + ' v' + wsNodes[i].version;
        grp.appendChild(opt);
      }
      select.appendChild(grp);
    }

    if (otherNodes.length > 0) {
      var grp = document.createElement('optgroup');
      grp.label = 'Dependencies';
      for (var i = 0; i < otherNodes.length; i++) {
        var opt = document.createElement('option');
        opt.value = otherNodes[i].idx;
        opt.textContent = otherNodes[i].name + ' v' + otherNodes[i].version;
        grp.appendChild(opt);
      }
      select.appendChild(grp);
    }

    // Select the first workspace node by default.
    if (wsNodes.length > 0) {
      select.value = wsNodes[0].idx;
      selectCrate(wsNodes[0].idx);
    } else if (otherNodes.length > 0) {
      select.value = otherNodes[0].idx;
      selectCrate(otherNodes[0].idx);
    }
  }

  // Called when a crate is selected in the dropdown.
  function selectCrate(nodeIdx) {
    nodeIdx = parseInt(nodeIdx, 10);
    var treeData = Depflame.getCurrentTreeData();
    if (!treeData || isNaN(nodeIdx)) return;

    currentPanelNode = nodeIdx;
    var node = treeData.nodes[nodeIdx];
    if (!node) return;

    var body = document.getElementById('feature-sidebar-body');
    var footer = document.getElementById('feature-sidebar-footer');
    if (!body) return;

    var features = node.available_features || {};
    var featureNames = Object.keys(features).sort();

    var currentEnabled = {};
    var enabledList = featureOverrides[nodeIdx] !== undefined
      ? featureOverrides[nodeIdx]
      : (node.enabled_features || []);
    for (var i = 0; i < enabledList.length; i++) {
      currentEnabled[enabledList[i]] = true;
    }

    var nodeNames = buildNodeNameSet(treeData);

    if (featureNames.length === 0) {
      var noFeatHtml = '';
      if (!node.is_workspace) {
        noFeatHtml += '<div class="feature-crate-link">'
          + '<a href="https://crates.io/crates/' + escHtml(node.name) + '" target="_blank" rel="noopener" class="crate-link">'
          + '<span class="crate-icon">' + DepflameIcons.box + '</span>'
          + escHtml(node.name) + ' v' + escHtml(node.version)
          + ' <span class="external-link-icon">' + DepflameIcons.externalLink + '</span>'
          + '</a></div>';
      }
      noFeatHtml += buildParentsHtml(treeData, nodeIdx);
      noFeatHtml += '<p class="feature-empty">No features.</p>';
      body.innerHTML = noFeatHtml;
      if (footer) footer.innerHTML = '';
      wireParentLinks(body);
      return;
    }

    var graphFeatures = [];
    var implicitFeatures = [];
    var flagFeatures = [];
    for (var i = 0; i < featureNames.length; i++) {
      var fname = featureNames[i];
      if (fname === 'default') continue;
      if (isImplicitDepFeature(fname, features[fname])) {
        implicitFeatures.push(fname);
      } else if (featureAffectsGraph(features[fname], nodeNames, features)) {
        graphFeatures.push(fname);
      } else {
        flagFeatures.push(fname);
      }
    }

    // Compute dep deltas: for each feature, how many unique deps does
    // toggling it add or remove?
    var featureDeltas = computeFeatureDeltas(
      treeData, nodeIdx, enabledList, featureNames
    );

    var html = '';

    // Crate link with crates.io icon and external link arrow.
    if (!node.is_workspace) {
      html += '<div class="feature-crate-link">'
        + '<a href="https://crates.io/crates/' + escHtml(node.name) + '" target="_blank" rel="noopener" class="crate-link">'
        + '<span class="crate-icon">' + DepflameIcons.box + '</span>'
        + escHtml(node.name) + ' v' + escHtml(node.version)
        + ' <span class="external-link-icon">' + DepflameIcons.externalLink + '</span>'
        + '</a></div>';
    }

    // Show reverse dependencies (parents) before features.
    html += buildParentsHtml(treeData, nodeIdx);

    if (features['default'] !== undefined) {
      html += featureCheckbox('default', currentEnabled['default'], features['default'], nodeNames, featureDeltas['default']);
      html += '<hr class="feature-separator">';
    }

    for (var i = 0; i < graphFeatures.length; i++) {
      var gf = graphFeatures[i];
      html += featureCheckbox(gf, !!currentEnabled[gf], features[gf], nodeNames, featureDeltas[gf]);
    }

    if (implicitFeatures.length > 0) {
      html += '<details class="feature-collapsed-section"><summary>Optional deps (' + implicitFeatures.length + ')</summary>';
      for (var i = 0; i < implicitFeatures.length; i++) {
        var imf = implicitFeatures[i];
        html += featureCheckbox(imf, !!currentEnabled[imf], features[imf], nodeNames, featureDeltas[imf]);
      }
      html += '</details>';
    }

    if (flagFeatures.length > 0) {
      html += '<details class="feature-collapsed-section"><summary>Flags (' + flagFeatures.length + ')</summary>';
      for (var i = 0; i < flagFeatures.length; i++) {
        var ff = flagFeatures[i];
        html += featureCheckbox(ff, !!currentEnabled[ff], features[ff], nodeNames, featureDeltas[ff]);
      }
      html += '</details>';
    }

    body.innerHTML = html;

    if (footer) {
      footer.innerHTML = '<button class="feature-panel-btn feature-panel-btn-reset" onclick="DepflameFeatures.resetNode()">Reset</button>'
        + '<button class="feature-panel-btn feature-panel-btn-reset" onclick="DepflameFeatures.resetAll()">Reset all</button>'
        + '<span id="feature-delta" class="feature-delta"></span>';
    }

    // Wire up immediate-apply on every checkbox.
    var checkboxes = body.querySelectorAll('input[type="checkbox"]');
    for (var i = 0; i < checkboxes.length; i++) {
      checkboxes[i].addEventListener('change', applyFromCheckboxes);
    }

    wireParentLinks(body);
  }

  // Called by flamegraph zoom to auto-select the zoomed crate.
  // Accepts a name (string) or a node index (number).
  function onZoom(nameOrIdx) {
    var treeData = Depflame.getCurrentTreeData();
    if (!treeData) return;

    var nodeIdx;
    if (typeof nameOrIdx === 'number') {
      nodeIdx = nameOrIdx;
    } else {
      nodeIdx = -1;
      for (var i = 0; i < treeData.nodes.length; i++) {
        if (treeData.nodes[i].name === nameOrIdx) { nodeIdx = i; break; }
      }
      if (nodeIdx < 0) return;
    }

    var select = document.getElementById('feature-crate-select');
    if (select) select.value = nodeIdx;
    selectCrate(nodeIdx);
  }

  function openPanel(treeData, nodeIdx) {
    onZoom(nodeIdx);
  }


  function closePanel() { /* no-op for sidebar */ }

  // Compute the dep count delta for each feature on a given node.
  // Returns { featureName: { unique: N, total: N }, ... }.
  // unique = deps exclusively brought/removed by this feature alone.
  // total  = all deps added/removed (including shared ones).
  // Sign: positive = enabling adds deps; negative = disabling removes deps.
  // For each feature, compute:
  //   total:  deps introduced by enabling this feature (from empty baseline).
  //   unique: deps that ONLY this feature introduces (no other feature covers them).
  // Both are fixed properties, independent of current checkbox state.
  function computeFeatureDeltas(treeData, nodeIdx, currentEnabledList, featureNames) {
    var deltas = {};

    function getActiveSet(overrides) {
      var saved = featureOverrides[nodeIdx];
      featureOverrides[nodeIdx] = overrides;
      var result = recomputeActiveGraph(treeData);
      if (saved !== undefined) {
        featureOverrides[nodeIdx] = saved;
      } else {
        delete featureOverrides[nodeIdx];
      }
      return result.activeNodes;
    }

    function countNonWs(active) {
      var c = 0;
      for (var idx in active) {
        if (!treeData.nodes[parseInt(idx, 10)].is_workspace) c++;
      }
      return c;
    }

    var emptyActive = getActiveSet([]);
    var emptyCount = countNonWs(emptyActive);

    // Per-feature: which non-workspace nodes does it introduce?
    var featureNodes = {};
    for (var i = 0; i < featureNames.length; i++) {
      var fname = featureNames[i];
      var withFeature = getActiveSet([fname]);
      var introduced = {};
      var total = 0;
      for (var idx in withFeature) {
        if (!emptyActive[idx] && !treeData.nodes[parseInt(idx, 10)].is_workspace) {
          introduced[idx] = true;
          total++;
        }
      }
      featureNodes[fname] = introduced;
      deltas[fname] = { total: total, unique: 0 };
    }

    // Count how many features introduce each node.
    var nodeRefCount = {};
    for (var i = 0; i < featureNames.length; i++) {
      var introduced = featureNodes[featureNames[i]];
      for (var idx in introduced) {
        nodeRefCount[idx] = (nodeRefCount[idx] || 0) + 1;
      }
    }

    // Unique = nodes introduced by exactly one feature.
    for (var i = 0; i < featureNames.length; i++) {
      var fname = featureNames[i];
      var introduced = featureNodes[fname];
      var unique = 0;
      for (var idx in introduced) {
        if (nodeRefCount[idx] === 1) unique++;
      }
      deltas[fname].unique = unique;
    }

    return deltas;
  }

  function featureCheckbox(name, checked, activates, nodeNames, deltaInfo) {
    var id = 'feat-' + name;
    var html = '<label class="feature-label">'
      + '<input type="checkbox" id="' + id + '" data-feature="' + escHtml(name) + '"'
      + (checked ? ' checked' : '') + '> '
      + '<span class="feature-name">' + escHtml(name) + '</span>';

    // Show dep count: total deps this feature introduces, and how many are unique to it.
    if (deltaInfo && deltaInfo.total > 0) {
      html += ' <span class="feature-dep-delta">'
        + '(' + deltaInfo.total + ' total / ' + deltaInfo.unique + ' unique)</span>';
    }

    if (activates && activates.length > 0) {
      var deps = [];
      var subfeats = [];
      for (var i = 0; i < activates.length; i++) {
        var a = activates[i];
        if (a.indexOf('dep:') === 0) {
          deps.push(a.substring(4));
        } else if (a.indexOf('/') !== -1) {
          deps.push(a);
        } else {
          subfeats.push(a);
        }
      }
      var parts = [];
      if (deps.length > 0) parts.push('enables: ' + deps.join(', '));
      if (subfeats.length > 0) parts.push('includes: ' + subfeats.join(', '));
      if (parts.length > 0) {
        html += '<span class="feature-activates">' + escHtml(parts.join(' | ')) + '</span>';
      }
    }

    html += '</label>';
    return html;
  }

  function getCheckedFeatures() {
    var body = document.getElementById('feature-sidebar-body');
    if (!body) return [];
    var checkboxes = body.querySelectorAll('input[type="checkbox"]');
    var feats = [];
    for (var i = 0; i < checkboxes.length; i++) {
      if (checkboxes[i].checked) {
        feats.push(checkboxes[i].getAttribute('data-feature'));
      }
    }
    return feats;
  }

  function applyFromCheckboxes(evt) {
    if (currentPanelNode < 0) return;
    var treeData = Depflame.getCurrentTreeData();
    if (!treeData) return;

    var node = treeData.nodes[currentPanelNode];
    var changedFeature = evt && evt.target ? evt.target.getAttribute('data-feature') : null;
    var defaultCheckbox = document.getElementById('feat-default');

    if (changedFeature === 'default') {
      // "default" was toggled directly.
      if (defaultCheckbox && defaultCheckbox.checked) {
        // Checking default: restore to the node's original enabled_features.
        featureOverrides[currentPanelNode] = (node.enabled_features || []).slice();
      } else {
        // Unchecking default: disable all features (--no-default-features).
        featureOverrides[currentPanelNode] = [];
      }
      // Refresh the sidebar to update all checkboxes to match.
      var result = applyRecomputation(treeData);
      Depflame.rerender(result.activeNodes, result.activeEdges);
      updateSummaryBar(treeData, result.activeDeps);
      selectCrate(currentPanelNode);
      return;
    }

    // A non-default feature was toggled.
    var checked = getCheckedFeatures();

    // If "default" is currently checked and user changed something else,
    // uncheck "default" (user is customizing).
    if (defaultCheckbox && defaultCheckbox.checked) {
      // Remove "default" from the list — the user is now in custom mode.
      checked = checked.filter(function(f) { return f !== 'default'; });
      defaultCheckbox.checked = false;
    }

    featureOverrides[currentPanelNode] = checked;
    var result = applyRecomputation(treeData);
    Depflame.rerender(result.activeNodes, result.activeEdges);
    updateSummaryBar(treeData, result.activeDeps);
  }

  function applyChanges() {
    applyFromCheckboxes();
  }

  function resetNode() {
    if (currentPanelNode < 0) return;
    var treeData = Depflame.getCurrentTreeData();
    if (!treeData) return;

    delete featureOverrides[currentPanelNode];

    if (Object.keys(featureOverrides).length === 0) {
      resetAll();
      return;
    }

    var result = applyRecomputation(treeData);
    Depflame.rerender(result.activeNodes, result.activeEdges);
    updateSummaryBar(treeData, result.activeDeps);
    selectCrate(currentPanelNode); // refresh checkboxes
  }

  function resetAll() {
    var treeData = Depflame.getCurrentTreeData();
    if (!treeData) return;

    featureOverrides = {};
    resetWeights(treeData);
    Depflame.rerender(null);
    removeSummaryBar();
    selectCrate(currentPanelNode); // refresh checkboxes
  }


  // -------------------------------------------------------------------------
  // Summary bar.
  // -------------------------------------------------------------------------

  // Called once at init to snapshot the baseline weights and dep count.
  function initBaseline(treeData, activeDeps) {
    originalTotalDeps = activeDeps;
    originalWeights = {};
    for (var i = 0; i < treeData.nodes.length; i++) {
      originalWeights[i] = treeData.nodes[i].transitive_weight;
    }
    var bar = document.getElementById('depflame-summary-bar');
    if (bar) bar.innerHTML = '<span>Deps: <b>' + activeDeps + '</b></span>';
  }

  function updateSummaryBar(treeData, activeDeps) {
    var bar = document.getElementById('depflame-summary-bar');
    if (!bar) return;

    var orig = originalTotalDeps || activeDeps;
    var diff = activeDeps - orig;
    var diffStr = diff < 0 ? (diff + ' deps') : (diff > 0 ? '+' + diff + ' deps' : '');
    var diffCls = diff < 0 ? 'summary-delta-negative' : (diff > 0 ? 'summary-delta-positive' : '');

    var html = '<span>Deps: <b>' + activeDeps + '</b></span>';
    if (diff !== 0) {
      html += '<span class="' + diffCls + '">' + diffStr + '</span>'
        + '<button class="feature-panel-btn feature-panel-btn-reset feature-panel-btn-sm" onclick="DepflameFeatures.resetAll()">Reset all</button>';
    }
    bar.innerHTML = html;
  }

  function removeSummaryBar() {
    var bar = document.getElementById('depflame-summary-bar');
    if (!bar) return;
    var count = originalTotalDeps || 0;
    bar.innerHTML = '<span>Deps: <b>' + count + '</b></span>';
  }

  // -------------------------------------------------------------------------
  // Helpers.
  // -------------------------------------------------------------------------

  function escHtml(s) {
    return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;')
      .replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }

  // -------------------------------------------------------------------------
  // Public API.
  // -------------------------------------------------------------------------

  return {
    initBaseline: initBaseline,
    populateDropdown: populateDropdown,
    selectCrate: selectCrate,
    onZoom: onZoom,
    openPanel: openPanel,
    closePanel: closePanel,
    applyChanges: applyChanges,
    resetNode: resetNode,
    resetAll: resetAll,
    recomputeActiveGraph: recomputeActiveGraph
  };
})();
