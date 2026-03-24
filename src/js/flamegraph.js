// ---------------------------------------------------------------------------
// depflame — Client-side flamegraph layout + rendering.
// Ported from Rust: flamegraph/layout.rs + flamegraph/render.rs
// ---------------------------------------------------------------------------

var Depflame = (function() {
  'use strict';

  // Layout constants.
  var CHART_WIDTH  = 1200;
  var ROW_HEIGHT   = 18;
  var ROW_GAP      = 1;
  var ROW_TOTAL    = ROW_HEIGHT + ROW_GAP;
  var MIN_RECT_WIDTH = 2;
  var MAX_DEPTH    = 40;
  var CHAR_WIDTH   = 6.5;
  var TEXT_PAD      = 4;
  var HEADER_HEIGHT = 4;   // minimal top padding (no in-SVG title/legend)
  var FOOTER_HEIGHT = 4;
  var MAX_FRAMES   = 8000;

  // Zoom state.
  var zoomStack = [];
  var reverseMode = false;

  // Reference to the current container and data for re-renders.
  var currentContainer = null;
  var currentTreeData  = null;
  var currentUnusedEdges = null;
  var currentActiveNodes = null;
  var currentActiveEdges = null;

  // -------------------------------------------------------------------------
  // Layout algorithm (port of layout.rs).
  // -------------------------------------------------------------------------

  function computeAncestorCounts(tree) {
    var counts = new Array(tree.nodes.length).fill(0);
    for (var i = 0; i < tree.nodes.length; i++) {
      var children = tree.nodes[i].children;
      for (var j = 0; j < children.length; j++) {
        counts[children[j]]++;
      }
    }
    return counts;
  }

  function layoutTree(tree, activeNodes, activeEdges) {
    var ancestorCounts = computeAncestorCounts(tree);
    var totalWeight = 0;
    for (var i = 0; i < tree.root_indices.length; i++) {
      totalWeight += tree.nodes[tree.root_indices[i]].transitive_weight;
    }
    if (totalWeight === 0) return [];

    var rects = [];
    var path = {};
    var pathStack = [];  // stack of nodeIdx for building unique tree-path IDs

    function layoutNode(nodeIdx, x, depth, width, parentName) {
      if (width < MIN_RECT_WIDTH || depth > MAX_DEPTH || rects.length >= MAX_FRAMES) return;
      if (path[nodeIdx]) return;
      path[nodeIdx] = true;
      pathStack.push(nodeIdx);

      if (activeNodes && !activeNodes[nodeIdx]) {
        pathStack.pop();
        delete path[nodeIdx];
        return;
      }

      var node = tree.nodes[nodeIdx];
      var isShared = ancestorCounts[nodeIdx] > 1;
      var rectIdx = rects.length;

      // Build a unique tree-path ID: "root>parent>...>nodeIdx"
      var treePath = pathStack.join('>');

      rects.push({
        x: x,
        y: depth * ROW_TOTAL + HEADER_HEIGHT,
        w: width,
        name: node.name,
        version: node.version,
        weight: node.transitive_weight,
        depth: depth,
        isShared: isShared,
        isWorkspace: node.is_workspace,
        parentName: parentName || '',
        ancestorCount: ancestorCounts[nodeIdx],
        collapsedChildren: 0,
        nodeIdx: nodeIdx,
        treePath: treePath
      });

      if (!node.children || node.children.length === 0) {
        pathStack.pop();
        delete path[nodeIdx];
        return;
      }

      var childTotal = 0;
      for (var i = 0; i < node.children.length; i++) {
        var ci = node.children[i];
        if (activeNodes && !activeNodes[ci]) continue;
        if (activeEdges && !activeEdges[nodeIdx + ':' + ci]) continue;
        childTotal += tree.nodes[ci].transitive_weight;
      }
      if (childTotal === 0) {
        pathStack.pop();
        delete path[nodeIdx];
        return;
      }

      var cx = x;
      var collapsed = 0;
      for (var i = 0; i < node.children.length; i++) {
        if (rects.length >= MAX_FRAMES) { collapsed++; continue; }
        var ci = node.children[i];
        if (activeNodes && !activeNodes[ci]) continue;
        if (activeEdges && !activeEdges[nodeIdx + ':' + ci]) continue;
        var cw = (tree.nodes[ci].transitive_weight / childTotal) * width;
        if (cw < MIN_RECT_WIDTH) { collapsed++; continue; }
        layoutNode(ci, cx, depth + 1, cw, node.name);
        cx += cw;
      }

      if (collapsed > 0) {
        rects[rectIdx].collapsedChildren = collapsed;
      }

      pathStack.pop();
      delete path[nodeIdx];
    }

    var x = 0;
    for (var i = 0; i < tree.root_indices.length; i++) {
      var ri = tree.root_indices[i];
      if (activeNodes && !activeNodes[ri]) continue;
      var w = (tree.nodes[ri].transitive_weight / totalWeight) * CHART_WIDTH;
      layoutNode(ri, x, 0, w, '');
      x += w;
    }

    return rects;
  }

  // -------------------------------------------------------------------------
  // Zoomed layout: re-layout a subtree with full CHART_WIDTH.
  // -------------------------------------------------------------------------

  // Find the path from any root to targetIdx, returning an array of nodeIdx
  // from root to target (inclusive), or null if not reachable.
  function findPathToNode(tree, targetIdx, activeNodes) {
    var roots = tree.root_indices;
    for (var ri = 0; ri < roots.length; ri++) {
      var rootIdx = roots[ri];
      if (activeNodes && !activeNodes[rootIdx]) continue;
      var result = dfsPath(tree, rootIdx, targetIdx, activeNodes, {});
      if (result) return result;
    }
    return null;
  }

  function dfsPath(tree, current, target, activeNodes, visited) {
    if (visited[current]) return null;
    visited[current] = true;
    if (current === target) { visited[current] = false; return [current]; }
    var children = tree.nodes[current].children;
    for (var i = 0; i < children.length; i++) {
      var ci = children[i];
      if (activeNodes && !activeNodes[ci]) continue;
      var sub = dfsPath(tree, ci, target, activeNodes, visited);
      if (sub) { visited[current] = false; return [current].concat(sub); }
    }
    visited[current] = false;
    return null;
  }

  // Layout a subtree rooted at zoomNodeIdx with full CHART_WIDTH,
  // plus ancestor nodes shown as full-width bars above.
  function layoutZoomed(tree, zoomNodeIdx, activeNodes, activeEdges) {
    var ancestorCounts = computeAncestorCounts(tree);
    var ancestorPath = findPathToNode(tree, zoomNodeIdx, activeNodes) || [zoomNodeIdx];
    var rects = [];
    var pathStack = [];

    // Render ancestors as full-width bars at depths 0..n-1.
    for (var d = 0; d < ancestorPath.length; d++) {
      var ni = ancestorPath[d];
      var node = tree.nodes[ni];
      pathStack.push(ni);
      rects.push({
        x: 0,
        y: d * ROW_TOTAL + HEADER_HEIGHT,
        w: CHART_WIDTH,
        name: node.name,
        version: node.version,
        weight: node.transitive_weight,
        depth: d,
        isShared: ancestorCounts[ni] > 1,
        isWorkspace: node.is_workspace,
        parentName: d > 0 ? tree.nodes[ancestorPath[d - 1]].name : '',
        ancestorCount: ancestorCounts[ni],
        collapsedChildren: 0,
        nodeIdx: ni,
        treePath: pathStack.join('>')
      });
    }

    // Now lay out the zoomed node's children at depth = ancestorPath.length,
    // using the full CHART_WIDTH.
    var zoomDepthOffset = ancestorPath.length;
    var path = {};
    // Mark all ancestors as visited to prevent cycles back up.
    for (var i = 0; i < ancestorPath.length; i++) path[ancestorPath[i]] = true;

    var zoomNode = tree.nodes[zoomNodeIdx];
    if (zoomNode.children && zoomNode.children.length > 0) {
      var childTotal = 0;
      for (var i = 0; i < zoomNode.children.length; i++) {
        var ci = zoomNode.children[i];
        if (activeNodes && !activeNodes[ci]) continue;
        if (activeEdges && !activeEdges[zoomNodeIdx + ':' + ci]) continue;
        childTotal += tree.nodes[ci].transitive_weight;
      }

      if (childTotal > 0) {
        var cx = 0;
        var collapsed = 0;
        for (var i = 0; i < zoomNode.children.length; i++) {
          if (rects.length >= MAX_FRAMES) { collapsed++; continue; }
          var ci = zoomNode.children[i];
          if (activeNodes && !activeNodes[ci]) continue;
          if (activeEdges && !activeEdges[zoomNodeIdx + ':' + ci]) continue;
          var cw = (tree.nodes[ci].transitive_weight / childTotal) * CHART_WIDTH;
          if (cw < MIN_RECT_WIDTH) { collapsed++; continue; }
          layoutSubNode(tree, ci, cx, zoomDepthOffset, cw,
            zoomNode.name, activeNodes, activeEdges, ancestorCounts, path, pathStack, rects);
          cx += cw;
        }
        // Update the zoom node's collapsed count.
        if (collapsed > 0) {
          rects[ancestorPath.length - 1].collapsedChildren = collapsed;
        }
      }
    }

    return rects;
  }

  // Recursive layout helper for zoomed subtrees — same logic as layoutNode
  // but operates on a separate path/pathStack context.
  function layoutSubNode(tree, nodeIdx, x, depth, width, parentName,
                         activeNodes, activeEdges, ancestorCounts, path, pathStack, rects) {
    if (width < MIN_RECT_WIDTH || depth > MAX_DEPTH || rects.length >= MAX_FRAMES) return;
    if (path[nodeIdx]) return;
    path[nodeIdx] = true;
    pathStack.push(nodeIdx);

    if (activeNodes && !activeNodes[nodeIdx]) {
      pathStack.pop();
      delete path[nodeIdx];
      return;
    }

    var node = tree.nodes[nodeIdx];
    var rectIdx = rects.length;
    var treePath = pathStack.join('>');

    rects.push({
      x: x,
      y: depth * ROW_TOTAL + HEADER_HEIGHT,
      w: width,
      name: node.name,
      version: node.version,
      weight: node.transitive_weight,
      depth: depth,
      isShared: ancestorCounts[nodeIdx] > 1,
      isWorkspace: node.is_workspace,
      parentName: parentName || '',
      ancestorCount: ancestorCounts[nodeIdx],
      collapsedChildren: 0,
      nodeIdx: nodeIdx,
      treePath: treePath
    });

    if (!node.children || node.children.length === 0) {
      pathStack.pop();
      delete path[nodeIdx];
      return;
    }

    var childTotal = 0;
    for (var i = 0; i < node.children.length; i++) {
      var ci = node.children[i];
      if (activeNodes && !activeNodes[ci]) continue;
      if (activeEdges && !activeEdges[nodeIdx + ':' + ci]) continue;
      childTotal += tree.nodes[ci].transitive_weight;
    }
    if (childTotal === 0) {
      pathStack.pop();
      delete path[nodeIdx];
      return;
    }

    var cx = x;
    var collapsed = 0;
    for (var i = 0; i < node.children.length; i++) {
      if (rects.length >= MAX_FRAMES) { collapsed++; continue; }
      var ci = node.children[i];
      if (activeNodes && !activeNodes[ci]) continue;
      if (activeEdges && !activeEdges[nodeIdx + ':' + ci]) continue;
      var cw = (tree.nodes[ci].transitive_weight / childTotal) * width;
      if (cw < MIN_RECT_WIDTH) { collapsed++; continue; }
      layoutSubNode(tree, ci, cx, depth + 1, cw, node.name,
                    activeNodes, activeEdges, ancestorCounts, path, pathStack, rects);
      cx += cw;
    }

    if (collapsed > 0) {
      rects[rectIdx].collapsedChildren = collapsed;
    }

    pathStack.pop();
    delete path[nodeIdx];
  }

  // -------------------------------------------------------------------------
  // Reverse flamegraph layout: show who depends on a given node.
  // The target node is at depth 0, its parents at depth 1, grandparents
  // at depth 2, etc.
  // -------------------------------------------------------------------------

  function buildReverseMap(tree) {
    var rev = {};
    for (var i = 0; i < tree.nodes.length; i++) {
      var children = tree.nodes[i].children || [];
      for (var c = 0; c < children.length; c++) {
        var ci = children[c];
        if (!rev[ci]) rev[ci] = [];
        rev[ci].push(i);
      }
    }
    return rev;
  }

  function layoutReverse(tree, targetIdx, activeNodes, activeEdges) {
    var ancestorCounts = computeAncestorCounts(tree);
    var revMap = buildReverseMap(tree);

    var rects = [];
    var path = {};
    var pathStack = [];

    // Target node at depth 0.
    var targetNode = tree.nodes[targetIdx];
    pathStack.push(targetIdx);
    rects.push({
      x: 0,
      y: HEADER_HEIGHT,
      w: CHART_WIDTH,
      name: targetNode.name,
      version: targetNode.version,
      weight: targetNode.transitive_weight,
      depth: 0,
      isShared: ancestorCounts[targetIdx] > 1,
      isWorkspace: targetNode.is_workspace,
      parentName: '',
      ancestorCount: ancestorCounts[targetIdx],
      collapsedChildren: 0,
      nodeIdx: targetIdx,
      treePath: pathStack.join('>')
    });
    path[targetIdx] = true;

    // Recursively lay out parents (reverse edges).
    function layoutParent(nodeIdx, x, depth, width, childName) {
      if (width < MIN_RECT_WIDTH || depth > MAX_DEPTH || rects.length >= MAX_FRAMES) return;
      if (path[nodeIdx]) return;
      path[nodeIdx] = true;
      pathStack.push(nodeIdx);

      if (activeNodes && !activeNodes[nodeIdx]) {
        pathStack.pop();
        delete path[nodeIdx];
        return;
      }

      var node = tree.nodes[nodeIdx];
      rects.push({
        x: x,
        y: depth * ROW_TOTAL + HEADER_HEIGHT,
        w: width,
        name: node.name,
        version: node.version,
        weight: node.transitive_weight,
        depth: depth,
        isShared: ancestorCounts[nodeIdx] > 1,
        isWorkspace: node.is_workspace,
        parentName: childName || '',
        ancestorCount: ancestorCounts[nodeIdx],
        collapsedChildren: 0,
        nodeIdx: nodeIdx,
        treePath: pathStack.join('>')
      });

      // Get this node's parents (reverse deps).
      var parents = revMap[nodeIdx] || [];
      var activeParents = [];
      for (var i = 0; i < parents.length; i++) {
        var pi = parents[i];
        if (path[pi]) continue;
        if (activeNodes && !activeNodes[pi]) continue;
        if (activeEdges && !activeEdges[pi + ':' + nodeIdx]) continue;
        activeParents.push(pi);
      }

      if (activeParents.length === 0) {
        pathStack.pop();
        delete path[nodeIdx];
        return;
      }

      var parentTotal = 0;
      for (var i = 0; i < activeParents.length; i++) {
        parentTotal += tree.nodes[activeParents[i]].transitive_weight;
      }
      if (parentTotal === 0) {
        pathStack.pop();
        delete path[nodeIdx];
        return;
      }

      var cx = x;
      for (var i = 0; i < activeParents.length; i++) {
        if (rects.length >= MAX_FRAMES) break;
        var pi = activeParents[i];
        var pw = (tree.nodes[pi].transitive_weight / parentTotal) * width;
        if (pw < MIN_RECT_WIDTH) continue;
        layoutParent(pi, cx, depth + 1, pw, node.name);
        cx += pw;
      }

      pathStack.pop();
      delete path[nodeIdx];
    }

    // Lay out direct parents of the target at depth 1.
    var targetParents = revMap[targetIdx] || [];
    var activeTargetParents = [];
    for (var i = 0; i < targetParents.length; i++) {
      var pi = targetParents[i];
      if (activeNodes && !activeNodes[pi]) continue;
      if (activeEdges && !activeEdges[pi + ':' + targetIdx]) continue;
      activeTargetParents.push(pi);
    }

    if (activeTargetParents.length > 0) {
      var parentTotal = 0;
      for (var i = 0; i < activeTargetParents.length; i++) {
        parentTotal += tree.nodes[activeTargetParents[i]].transitive_weight;
      }
      var cx = 0;
      for (var i = 0; i < activeTargetParents.length; i++) {
        if (rects.length >= MAX_FRAMES) break;
        var pi = activeTargetParents[i];
        var pw = (tree.nodes[pi].transitive_weight / parentTotal) * CHART_WIDTH;
        if (pw < MIN_RECT_WIDTH) continue;
        layoutParent(pi, cx, 1, pw, targetNode.name);
        cx += pw;
      }
    }

    return rects;
  }

  // -------------------------------------------------------------------------
  // Colour scheme (port of render.rs).
  // -------------------------------------------------------------------------

  function rectFill(r, maxWeight, isUnused) {
    if (isUnused) return 'rgb(220,20,80)';
    if (r.isWorkspace) return 'rgb(70,130,180)';

    var ratio = maxWeight > 1
      ? Math.log(r.weight) / Math.log(maxWeight)
      : 0;
    ratio = Math.max(0, Math.min(1, ratio));

    if (r.isShared) {
      var hue = 280 - 40 * ratio;
      var sat = 45 + 20 * ratio;
      var lit = 65 - 10 * ratio;
      return 'hsl(' + hue.toFixed(0) + ',' + sat.toFixed(0) + '%,' + lit.toFixed(0) + '%)';
    }

    var hue = 120 - 90 * ratio;
    var sat = 55 + 20 * ratio;
    var lit = 58 - 8 * ratio;
    return 'hsl(' + hue.toFixed(0) + ',' + sat.toFixed(0) + '%,' + lit.toFixed(0) + '%)';
  }

  function textColor(r) {
    return r.isWorkspace ? '#fff' : '#000';
  }

  function fitLabel(name, weight, availWidth) {
    var full = name + ' (' + weight + ')';
    if (full.length * CHAR_WIDTH + TEXT_PAD * 2 <= availWidth) return full;
    if (name.length * CHAR_WIDTH + TEXT_PAD * 2 <= availWidth) return name;
    var maxChars = Math.floor((availWidth - TEXT_PAD * 2) / CHAR_WIDTH);
    if (maxChars > 2) return name.substr(0, maxChars - 2) + '..';
    return '';
  }

  function tooltipText(r) {
    var s = r.name + ' v' + r.version + '\n'
      + r.weight + ' transitive dep' + (r.weight === 1 ? '' : 's') + '\n'
      + 'depth ' + r.depth;
    if (r.isShared) s += '\n[shared: ' + r.ancestorCount + ' parents in dep graph]';
    if (r.collapsedChildren > 0) s += '\n[' + r.collapsedChildren + ' children too small to show]';
    return s;
  }

  // -------------------------------------------------------------------------
  // SVG rendering — frames only (no title/legend/controls; those are in HTML).
  // -------------------------------------------------------------------------

  var SVG_NS = 'http://www.w3.org/2000/svg';

  function createSvgElement(tag, attrs) {
    var el = document.createElementNS(SVG_NS, tag);
    for (var k in attrs) {
      if (attrs.hasOwnProperty(k)) el.setAttribute(k, attrs[k]);
    }
    return el;
  }

  // Transition duration for animated updates (ms).
  var TRANSITION_MS = 300;
  var TRANSITION_CSS = TRANSITION_MS + 'ms ease-in-out';

  function buildUnusedSet(unusedEdges) {
    var s = {};
    if (unusedEdges) {
      for (var i = 0; i < unusedEdges.length; i++) {
        s[unusedEdges[i][0] + ':' + unusedEdges[i][1]] = true;
      }
    }
    return s;
  }

  function computeMaxWeight(tree) {
    var mw = 1;
    for (var i = 0; i < tree.nodes.length; i++) {
      if (!tree.nodes[i].is_workspace && tree.nodes[i].transitive_weight > mw) {
        mw = tree.nodes[i].transitive_weight;
      }
    }
    return mw;
  }

  function svgStyleText() {
    return [
      '.frame { cursor: pointer; }',
      '.frame:hover rect { stroke: #222; stroke-width: 1.5; }',
      '.frame:hover rect.shared { stroke: #fff; stroke-width: 1.5; }',
      '.frame text { pointer-events: none; }',
      'rect.shared { stroke-dasharray: 4,2; stroke: rgba(100,70,130,0.5); stroke-width: 0.5; }',
      'rect.normal { stroke: rgba(0,0,0,0.12); stroke-width: 0.5; }',
      'rect.unused { stroke: rgba(220,20,80,0.8); stroke-width: 1.5; }',
      'rect.workspace { stroke: rgba(0,0,0,0.3); stroke-width: 1; }',
      '.highlight rect { stroke: #000 !important; stroke-width: 2 !important; }',
      '.ghost rect { opacity: 0.25; }',
      '.ghost text { opacity: 0.35; }',
      // Transition rules for animated layout changes.
      '.frame rect { transition: x ' + TRANSITION_CSS + ', y ' + TRANSITION_CSS
        + ', width ' + TRANSITION_CSS + ', fill ' + TRANSITION_CSS
        + ', opacity ' + TRANSITION_CSS + '; }',
      '.frame text { transition: transform ' + TRANSITION_CSS
        + ', opacity ' + TRANSITION_CSS + '; }',
      '.frame { transition: opacity ' + TRANSITION_CSS + '; }',
      '.frame.exiting { opacity: 0; pointer-events: none; }',
      '.frame.entering { opacity: 0; }',
    ].join('\n');
  }

  function createFrame(r, fill, tc, cls, label, tip) {
    var g = createSvgElement('g', {
      'class': 'frame',
      'data-x': r.x, 'data-w': r.w, 'data-d': r.depth,
      'data-name': r.name, 'data-weight': r.weight,
      'data-node-idx': r.nodeIdx,
      'data-tree-path': r.treePath
    });

    var titleEl = createSvgElement('title', {});
    titleEl.textContent = tip;
    g.appendChild(titleEl);

    g.appendChild(createSvgElement('rect', {
      x: r.x, y: r.y, width: r.w, height: ROW_HEIGHT,
      rx: '2', fill: fill, 'class': cls
    }));

    var text = createSvgElement('text', {
      fill: tc,
      transform: 'translate(' + (r.x + TEXT_PAD) + ',' + (r.y + 13) + ')'
    });
    text.textContent = label;
    g.appendChild(text);

    g.addEventListener('click', onFrameClick);
    g.addEventListener('mouseover', hlOn);
    g.addEventListener('mouseout', hlOff);

    return g;
  }

  function updateFrame(g, r, fill, tc, cls, label, tip) {
    g.setAttribute('data-x', r.x);
    g.setAttribute('data-w', r.w);
    g.setAttribute('data-d', r.depth);
    g.setAttribute('data-name', r.name);
    g.setAttribute('data-weight', r.weight);
    g.setAttribute('data-tree-path', r.treePath);

    g.querySelector('title').textContent = tip;

    var rect = g.querySelector('rect');
    rect.setAttribute('x', r.x);
    rect.setAttribute('y', r.y);
    rect.setAttribute('width', r.w);
    rect.setAttribute('fill', fill);
    rect.setAttribute('class', cls);

    var text = g.querySelector('text');
    text.setAttribute('transform', 'translate(' + (r.x + TEXT_PAD) + ',' + (r.y + 13) + ')');
    text.setAttribute('fill', tc);
    text.textContent = label;

    // Remove exiting class if present (element is being kept).
    g.classList.remove('exiting');
    g.style.display = '';
  }

  function renderSvg(container, rects, tree, unusedEdges) {
    // Empty state — full replace.
    if (!rects || rects.length === 0) {
      container.innerHTML = '<p style="color:#888;padding:24px">No dependency tree data to display.</p>';
      return;
    }

    var unusedSet = buildUnusedSet(unusedEdges);
    var maxWeight = computeMaxWeight(tree);

    var maxDepth = 0;
    for (var i = 0; i < rects.length; i++) {
      if (rects[i].depth > maxDepth) maxDepth = rects[i].depth;
    }
    var svgHeight = (maxDepth + 1) * ROW_TOTAL + HEADER_HEIGHT + FOOTER_HEIGHT;

    // Check if we can reconcile against existing SVG.
    var svg = container.querySelector('svg');
    var framesG = svg ? svg.querySelector('#frames') : null;
    var canReconcile = !!(svg && framesG);

    if (!canReconcile) {
      // First render — build from scratch.
      container.innerHTML = '';
      svg = createSvgElement('svg', {
        'xmlns': SVG_NS,
        'viewBox': '0 0 ' + CHART_WIDTH + ' ' + svgHeight,
        'width': '100%',
        'font-family': 'Consolas,monospace',
        'font-size': '11'
      });

      var style = createSvgElement('style', {});
      style.textContent = svgStyleText();
      svg.appendChild(style);

      framesG = createSvgElement('g', { id: 'frames' });
      for (var i = 0; i < rects.length; i++) {
        var r = rects[i];
        var isUnused = r.parentName && unusedSet[r.parentName + ':' + r.name];
        var fill = rectFill(r, maxWeight, isUnused);
        var tc = isUnused ? '#fff' : textColor(r);
        var cls = isUnused ? 'unused'
          : r.isWorkspace ? 'workspace'
          : r.isShared ? 'shared' : 'normal';
        framesG.appendChild(createFrame(r, fill, tc, cls,
          fitLabel(r.name, r.weight, r.w), tooltipText(r)));
      }

      svg.appendChild(framesG);
      container.appendChild(svg);
      return;
    }

    // --- Reconcile existing frames ---
    svg.setAttribute('viewBox', '0 0 ' + CHART_WIDTH + ' ' + svgHeight);

    // Build a map of treePath → existing <g> element.
    var existingMap = {};
    var oldFrames = framesG.querySelectorAll(':scope > .frame');
    for (var i = 0; i < oldFrames.length; i++) {
      var tp = oldFrames[i].getAttribute('data-tree-path');
      if (tp) existingMap[tp] = oldFrames[i];
    }

    // Build a set of treePath values present in the new layout.
    var newPathSet = {};
    for (var i = 0; i < rects.length; i++) {
      newPathSet[rects[i].treePath] = true;
    }

    // Mark frames that are leaving (fade out, then remove after transition).
    for (var tp in existingMap) {
      if (!newPathSet[tp]) {
        var g = existingMap[tp];
        g.classList.add('exiting');
        // Remove after transition ends.
        (function(el) {
          setTimeout(function() { if (el.parentNode) el.parentNode.removeChild(el); }, TRANSITION_MS);
        })(g);
      }
    }

    // Update or create frames.
    for (var i = 0; i < rects.length; i++) {
      var r = rects[i];
      var isUnused = r.parentName && unusedSet[r.parentName + ':' + r.name];
      var fill = rectFill(r, maxWeight, isUnused);
      var tc = isUnused ? '#fff' : textColor(r);
      var cls = isUnused ? 'unused'
        : r.isWorkspace ? 'workspace'
        : r.isShared ? 'shared' : 'normal';
      var label = fitLabel(r.name, r.weight, r.w);
      var tip = tooltipText(r);

      var existing = existingMap[r.treePath];
      if (existing) {
        // Update in place — CSS transitions animate the changes.
        updateFrame(existing, r, fill, tc, cls, label, tip);
      } else {
        // New frame — fade in.
        var g = createFrame(r, fill, tc, cls, label, tip);
        g.classList.add('entering');
        framesG.appendChild(g);
        // Trigger reflow then remove the entering class to start fade-in.
        (function(el) {
          requestAnimationFrame(function() {
            requestAnimationFrame(function() { el.classList.remove('entering'); });
          });
        })(g);
      }
    }
  }

  // -------------------------------------------------------------------------
  // Interaction: zoom (re-layout based).
  // -------------------------------------------------------------------------

  function onFrameClick(evt) {
    var g = evt.currentTarget;
    zoom(g);
  }

  function zoom(g) {
    var nodeIdx = parseInt(g.getAttribute('data-node-idx'), 10);
    var ow = parseFloat(g.getAttribute('data-w'));
    var name = g.getAttribute('data-name');

    // If the clicked node is already full-width, zoom out.
    if (ow >= CHART_WIDTH * 0.99) {
      if (zoomStack.length > 0) {
        var prev = zoomStack.pop();
        applyZoomLayout(prev[2]);  // restore previous zoom target (null = full tree)
        if (typeof DepflameFeatures !== 'undefined') {
          if (zoomStack.length > 0) {
            DepflameFeatures.onZoom(zoomStack[zoomStack.length - 1][0]);
          } else {
            DepflameFeatures.onZoom(nodeIdx);
          }
        }
      }
      return;
    }

    // Push previous zoom target onto stack so we can restore it on zoom-out.
    var prevZoomTarget = zoomStack.length > 0
      ? zoomStack[zoomStack.length - 1][0]
      : null;
    zoomStack.push([nodeIdx, name, prevZoomTarget]);

    applyZoomLayout(nodeIdx);

    if (typeof DepflameFeatures !== 'undefined') {
      DepflameFeatures.onZoom(nodeIdx);
    }
  }

  // Re-layout and render for the given zoom target.
  // zoomNodeIdx: node to zoom into, or null for full tree.
  function applyZoomLayout(zoomNodeIdx) {
    if (!currentContainer || !currentTreeData) return;
    var rects;
    if (reverseMode && zoomNodeIdx != null) {
      rects = layoutReverse(currentTreeData, zoomNodeIdx, currentActiveNodes, currentActiveEdges);
    } else if (zoomNodeIdx != null) {
      rects = layoutZoomed(currentTreeData, zoomNodeIdx, currentActiveNodes, currentActiveEdges);
    } else {
      rects = layoutTree(currentTreeData, currentActiveNodes, currentActiveEdges);
    }
    renderSvg(currentContainer, rects, currentTreeData, currentUnusedEdges);
  }

  function resetZoom() {
    zoomStack = [];
    reverseMode = false;
    updateReverseButton();
    applyZoomLayout(null);
  }

  function toggleReverse() {
    reverseMode = !reverseMode;
    updateReverseButton();
    if (zoomStack.length > 0) {
      var currentZoomTarget = zoomStack[zoomStack.length - 1][0];
      applyZoomLayout(currentZoomTarget);
    } else {
      reverseMode = false;
      updateReverseButton();
    }
  }

  // Zoom into a node by index and enable reverse mode.
  function zoomReverse(nodeIdx) {
    // Push zoom entry if not already zoomed on this node.
    var prevZoomTarget = zoomStack.length > 0
      ? zoomStack[zoomStack.length - 1][0]
      : null;
    if (prevZoomTarget !== nodeIdx) {
      zoomStack.push([nodeIdx, currentTreeData.nodes[nodeIdx].name, prevZoomTarget]);
    }
    reverseMode = true;
    updateReverseButton();
    applyZoomLayout(nodeIdx);
  }

  function updateReverseButton() {
    var btn = document.getElementById('reverse-btn');
    if (btn) {
      if (reverseMode) {
        btn.classList.add('active');
      } else {
        btn.classList.remove('active');
      }
    }
  }

  // -------------------------------------------------------------------------
  // Interaction: hover highlight.
  // -------------------------------------------------------------------------

  function hlOn(evt) {
    var name = evt.currentTarget.getAttribute('data-name');
    var all = document.querySelectorAll('.frame[data-name="' + name + '"]');
    for (var i = 0; i < all.length; i++) all[i].classList.add('highlight');
  }

  function hlOff(evt) {
    var name = evt.currentTarget.getAttribute('data-name');
    var all = document.querySelectorAll('.frame[data-name="' + name + '"]');
    for (var i = 0; i < all.length; i++) all[i].classList.remove('highlight');
  }

  // -------------------------------------------------------------------------
  // Interaction: search (driven from HTML input in bottom bar).
  // -------------------------------------------------------------------------

  function searchByQuery(q) {
    if (!q) { clearSearch(); return; }
    var re;
    try { re = new RegExp(q, 'i'); } catch(e) { return; }
    var frames = document.querySelectorAll('.frame');
    var count = 0;
    for (var i = 0; i < frames.length; i++) {
      var name = frames[i].getAttribute('data-name');
      if (re.test(name)) {
        frames[i].classList.add('highlight');
        count++;
      } else {
        frames[i].classList.remove('highlight');
      }
    }
    var el = document.getElementById('search-matches');
    if (el) el.textContent = count + ' matches';
  }

  function clearSearch() {
    var frames = document.querySelectorAll('.frame');
    for (var i = 0; i < frames.length; i++) frames[i].classList.remove('highlight');
    var el = document.getElementById('search-matches');
    if (el) el.textContent = '';
    var input = document.getElementById('flame-search-input');
    if (input) input.value = '';
  }

  // -------------------------------------------------------------------------
  // Public API.
  // -------------------------------------------------------------------------

  function render(container, treeData, unusedEdges, activeNodes, activeEdges) {
    currentContainer = container;
    currentTreeData  = treeData;
    currentUnusedEdges = unusedEdges;
    currentActiveNodes = activeNodes || null;
    currentActiveEdges = activeEdges || null;

    zoomStack = [];

    var rects = layoutTree(treeData, currentActiveNodes, currentActiveEdges);
    renderSvg(container, rects, treeData, unusedEdges);
  }

  function rerender(activeNodes, activeEdges) {
    if (!currentContainer || !currentTreeData) return;
    currentActiveNodes = activeNodes || null;
    currentActiveEdges = activeEdges || null;

    // If zoomed in, re-layout at the current zoom level with new active nodes.
    if (zoomStack.length > 0) {
      var currentZoomTarget = zoomStack[zoomStack.length - 1][0]; // [nodeIdx, name, prevTarget]
      // The zoom target may no longer be active — fall back to full tree.
      if (currentZoomTarget != null && currentActiveNodes && !currentActiveNodes[currentZoomTarget]) {
        zoomStack = [];
        var rects = layoutTree(currentTreeData, currentActiveNodes, currentActiveEdges);
        renderSvg(currentContainer, rects, currentTreeData, currentUnusedEdges);
      } else {
        applyZoomLayout(currentZoomTarget);
      }
    } else {
      var rects = layoutTree(currentTreeData, currentActiveNodes, currentActiveEdges);
      renderSvg(currentContainer, rects, currentTreeData, currentUnusedEdges);
    }
  }

  return {
    render: render,
    rerender: rerender,
    resetZoom: resetZoom,
    toggleReverse: toggleReverse,
    zoomReverse: zoomReverse,
    searchByQuery: searchByQuery,
    clearSearch: clearSearch,
    CHART_WIDTH: CHART_WIDTH,
    layoutTree: layoutTree,
    computeAncestorCounts: computeAncestorCounts,
    getCurrentTreeData: function() { return currentTreeData; }
  };
})();
