use crate::graph::DepGraph;
use cargo_metadata::PackageId;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::Write;

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

const CHART_WIDTH: f64 = 1200.0;
const ROW_HEIGHT: f64 = 18.0;
const ROW_GAP: f64 = 1.0;
const ROW_TOTAL: f64 = ROW_HEIGHT + ROW_GAP;
const MIN_RECT_WIDTH: f64 = 2.0;
const MAX_DEPTH: usize = 40;
const CHAR_WIDTH: f64 = 6.5;
const TEXT_PAD: f64 = 4.0;
const HEADER_HEIGHT: f64 = 40.0;
const FOOTER_HEIGHT: f64 = 30.0;
/// Safety limit on total frames to keep SVG size reasonable.
const MAX_FRAMES: usize = 8000;

// ---------------------------------------------------------------------------
// Serializable dep-tree (embedded in JSON report so `report` subcommand can
// re-render as SVG without re-running analysis).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DepTreeData {
    pub nodes: Vec<DepTreeNode>,
    pub root_indices: Vec<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepTreeNode {
    pub name: String,
    pub version: String,
    /// Total transitive dep count (including self).
    pub transitive_weight: usize,
    pub is_workspace: bool,
    /// Indices into `DepTreeData::nodes`.
    pub children: Vec<usize>,
}

// ---------------------------------------------------------------------------
// Build the serializable tree from the live DepGraph.
// ---------------------------------------------------------------------------

pub fn build_dep_tree(graph: &DepGraph) -> DepTreeData {
    let mut nodes: Vec<DepTreeNode> = Vec::new();
    let mut id_to_idx: HashMap<PackageId, usize> = HashMap::new();

    // First pass: create all nodes.
    for (id, node) in &graph.nodes {
        let idx = nodes.len();
        id_to_idx.insert(id.clone(), idx);
        nodes.push(DepTreeNode {
            name: node.name.clone(),
            version: node.version.clone(),
            transitive_weight: node.transitive_weight,
            is_workspace: node.is_workspace_member,
            children: Vec::new(),
        });
    }

    // Second pass: wire up children (sorted by weight desc for determinism).
    for (id, deps) in &graph.forward {
        if let Some(&parent_idx) = id_to_idx.get(id) {
            let mut child_indices: Vec<usize> = deps
                .iter()
                .filter_map(|dep_id| id_to_idx.get(dep_id).copied())
                .collect();
            child_indices
                .sort_by(|&a, &b| nodes[b].transitive_weight.cmp(&nodes[a].transitive_weight));
            nodes[parent_idx].children = child_indices;
        }
    }

    let mut root_indices: Vec<usize> = graph
        .workspace_members
        .iter()
        .filter_map(|id| id_to_idx.get(id).copied())
        .collect();
    root_indices.sort_by(|&a, &b| nodes[b].transitive_weight.cmp(&nodes[a].transitive_weight));

    DepTreeData {
        nodes,
        root_indices,
    }
}

// ---------------------------------------------------------------------------
// Internal layout representation.
// ---------------------------------------------------------------------------

struct LayoutRect {
    x: f64,
    y: f64,
    w: f64,
    name: String,
    version: String,
    weight: usize,
    depth: usize,
    is_shared: bool,
    is_workspace: bool,
    ancestor_count: usize,
    /// Number of children that were too small to render (collapsed).
    collapsed_children: usize,
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

fn compute_ancestor_counts(tree: &DepTreeData) -> Vec<usize> {
    let mut counts = vec![0usize; tree.nodes.len()];
    for node in &tree.nodes {
        for &child_idx in &node.children {
            counts[child_idx] += 1;
        }
    }
    counts
}

fn layout(tree: &DepTreeData) -> Vec<LayoutRect> {
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
        );
        cx += cw;
    }

    if collapsed > 0 {
        rects[rect_idx].collapsed_children = collapsed;
    }

    path.remove(&node_idx);
}

// ---------------------------------------------------------------------------
// SVG rendering.
// ---------------------------------------------------------------------------

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Map transitive weight to a fill colour.
///   - workspace members → steel blue
///   - shared deps → colour based on weight but with purple tint
///   - leaf (weight=1) → cool green
///   - heavy → warm red/orange
fn rect_fill(r: &LayoutRect, max_weight: usize) -> String {
    if r.is_workspace {
        return "rgb(70,130,180)".to_string();
    }
    let ratio = if max_weight > 1 {
        (r.weight as f64).ln() / (max_weight as f64).ln()
    } else {
        0.0
    };
    let ratio = ratio.clamp(0.0, 1.0);

    if r.is_shared {
        // Purple-shifted heat gradient for shared deps:
        // cool violet → warm magenta
        let hue = 280.0 - 40.0 * ratio; // 280 (violet) → 240 (blue-purple)
        let sat = 45.0 + 20.0 * ratio;
        let lit = 65.0 - 10.0 * ratio;
        return format!("hsl({hue:.0},{sat:.0}%,{lit:.0}%)");
    }

    // Green → yellow → orange → red
    let hue = 120.0 * (1.0 - ratio);
    let sat = 65.0 + 15.0 * ratio;
    let lit = 55.0 - 10.0 * ratio;
    format!("hsl({hue:.0},{sat:.0}%,{lit:.0}%)")
}

fn text_color(r: &LayoutRect) -> &'static str {
    if r.is_workspace {
        "#fff"
    } else {
        "#000"
    }
}

fn fit_label(name: &str, weight: usize, avail_width: f64) -> String {
    let full = format!("{name} ({weight})");
    let full_w = full.len() as f64 * CHAR_WIDTH + TEXT_PAD * 2.0;
    if full_w <= avail_width {
        return full;
    }
    let name_w = name.len() as f64 * CHAR_WIDTH + TEXT_PAD * 2.0;
    if name_w <= avail_width {
        return name.to_string();
    }
    let max_chars = ((avail_width - TEXT_PAD * 2.0) / CHAR_WIDTH) as usize;
    if max_chars > 2 {
        let end = max_chars.min(name.len()).saturating_sub(2);
        // Don't split in the middle of a multi-byte char.
        let end = name.floor_char_boundary(end);
        format!("{}..", &name[..end])
    } else {
        String::new()
    }
}

fn tooltip(r: &LayoutRect) -> String {
    let shared_note = if r.is_shared {
        format!("\n[shared: {} parents in dep graph]", r.ancestor_count)
    } else {
        String::new()
    };
    let collapsed_note = if r.collapsed_children > 0 {
        format!("\n[{} children too small to show]", r.collapsed_children)
    } else {
        String::new()
    };
    format!(
        "{} v{}\n{} transitive dep{}\ndepth {}{}{}",
        r.name,
        r.version,
        r.weight,
        if r.weight == 1 { "" } else { "s" },
        r.depth,
        shared_note,
        collapsed_note,
    )
}

pub fn render_flamegraph(
    tree: &DepTreeData,
    total_deps: usize,
    writer: &mut dyn Write,
) -> anyhow::Result<()> {
    let rects = layout(tree);
    if rects.is_empty() {
        writeln!(writer, "<svg xmlns='http://www.w3.org/2000/svg'/>")?;
        return Ok(());
    }

    let max_depth = rects.iter().map(|r| r.depth).max().unwrap_or(0);
    let max_weight = tree
        .nodes
        .iter()
        .filter(|n| !n.is_workspace)
        .map(|n| n.transitive_weight)
        .max()
        .unwrap_or(1);
    let svg_height = (max_depth + 1) as f64 * ROW_TOTAL + HEADER_HEIGHT + FOOTER_HEIGHT;

    let shared_count = rects.iter().filter(|r| r.is_shared).count();
    let unique_nodes: HashSet<&str> = rects.iter().map(|r| r.name.as_str()).collect();

    let mut svg = String::with_capacity(128 * 1024);

    // -- SVG header + embedded styles + JS -----------------------------------
    svg.push_str(&format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {CHART_WIDTH} {svg_height}"
     width="{CHART_WIDTH}" height="{svg_height}"
     font-family="Consolas,monospace" font-size="11">
<style>
  .frame {{ cursor: pointer; }}
  .frame:hover rect {{ stroke: #222; stroke-width: 1.5; }}
  .frame:hover rect.shared {{ stroke: #fff; stroke-width: 1.5; }}
  .frame text {{ pointer-events: none; }}
  rect.shared {{ stroke-dasharray: 4,2; stroke: rgba(100,70,130,0.5); stroke-width: 0.5; }}
  rect.normal {{ stroke: rgba(0,0,0,0.12); stroke-width: 0.5; }}
  rect.workspace {{ stroke: rgba(0,0,0,0.3); stroke-width: 1; }}
  text.title {{ font-size: 15px; font-weight: bold; fill: #333; }}
  text.subtitle {{ font-size: 11px; fill: #888; }}
  text.legend {{ font-size: 10px; fill: #666; }}
  .legend-rect {{ stroke: #999; stroke-width: 0.5; }}
  /* Highlight all instances of the same dep on hover via JS */
  .highlight rect {{ stroke: #000 !important; stroke-width: 2 !important; }}
</style>
<script type="text/ecmascript"><![CDATA[
  // --- Click-to-zoom (flamegraph style) ---
  var zoomStack = [];
  var origView = [0, {cw}];

  function zoom(evt) {{
    var g = evt.currentTarget;
    var ox = parseFloat(g.getAttribute('data-x'));
    var ow = parseFloat(g.getAttribute('data-w'));
    if (ow >= {cw} * 0.99) {{
      if (zoomStack.length > 0) {{
        var prev = zoomStack.pop();
        applyZoom(prev[0], prev[1]);
      }}
      return;
    }}
    zoomStack.push(origView.slice());
    origView = [ox, ow];
    applyZoom(ox, ow);
  }}

  function applyZoom(ox, ow) {{
    var frames = document.querySelectorAll('.frame');
    var scale = {cw} / ow;
    for (var i = 0; i < frames.length; i++) {{
      var f = frames[i];
      var fx = parseFloat(f.getAttribute('data-x'));
      var fw = parseFloat(f.getAttribute('data-w'));
      var rect = f.querySelector('rect');
      var text = f.querySelector('text');
      var newX = (fx - ox) * scale;
      var newW = fw * scale;
      // Hide frames fully outside viewport.
      if (newX + newW < -1 || newX > {cw} + 1 || newW < 0.5) {{
        f.style.display = 'none';
      }} else {{
        f.style.display = '';
        rect.setAttribute('x', newX);
        rect.setAttribute('width', Math.max(newW, 0.5));
        text.setAttribute('x', newX + 3);
        var name = f.getAttribute('data-name');
        var weight = f.getAttribute('data-weight');
        text.textContent = fitLabel(name, weight, newW);
      }}
    }}
  }}

  function fitLabel(name, weight, w) {{
    var full = name + ' (' + weight + ')';
    if (full.length * 6.5 + 8 <= w) return full;
    if (name.length * 6.5 + 8 <= w) return name;
    var max = Math.floor((w - 8) / 6.5);
    if (max > 2) return name.substr(0, max - 2) + '..';
    return '';
  }}

  function resetZoom() {{
    zoomStack = [];
    origView = [0, {cw}];
    applyZoom(0, {cw});
  }}

  // --- Hover: highlight all instances of the same crate ---
  function hlOn(evt) {{
    var name = evt.currentTarget.getAttribute('data-name');
    var all = document.querySelectorAll('.frame[data-name="' + name + '"]');
    for (var i = 0; i < all.length; i++) all[i].classList.add('highlight');
  }}
  function hlOff(evt) {{
    var name = evt.currentTarget.getAttribute('data-name');
    var all = document.querySelectorAll('.frame[data-name="' + name + '"]');
    for (var i = 0; i < all.length; i++) all[i].classList.remove('highlight');
  }}

  // --- Search: highlight matching crates ---
  function search() {{
    var q = prompt('Search for crate name (regex):');
    if (!q) return;
    var re = new RegExp(q, 'i');
    var frames = document.querySelectorAll('.frame');
    var count = 0;
    for (var i = 0; i < frames.length; i++) {{
      var name = frames[i].getAttribute('data-name');
      if (re.test(name)) {{
        frames[i].classList.add('highlight');
        count++;
      }} else {{
        frames[i].classList.remove('highlight');
      }}
    }}
    document.getElementById('search-status').textContent = count + ' matches';
  }}
  function clearSearch() {{
    var frames = document.querySelectorAll('.frame');
    for (var i = 0; i < frames.length; i++) frames[i].classList.remove('highlight');
    document.getElementById('search-status').textContent = '';
  }}
]]></script>
"##,
        cw = CHART_WIDTH
    ));

    // -- Header --------------------------------------------------------------
    svg.push_str(&format!(
        r#"<text x="6" y="18" class="title">Dependency Tree — Icicle / Flamegraph</text>
<text x="6" y="33" class="subtitle">{total_deps} deps total, {unique} unique crates shown, {shared_count} shared (dashed border = multiple parents) | click to zoom</text>
"#,
        unique = unique_nodes.len(),
    ));

    // -- Legend + controls ----------------------------------------------------
    let lx = CHART_WIDTH - 480.0;
    svg.push_str(&format!(
        r#"<rect x="{lx0}" y="4" width="12" height="10" rx="2" fill="rgb(70,130,180)" class="legend-rect"/>
<text x="{lx1}" y="13" class="legend">workspace</text>
<rect x="{lx2}" y="4" width="12" height="10" rx="2" fill="hsl(110,65%,55%)" class="legend-rect"/>
<text x="{lx3}" y="13" class="legend">few deps</text>
<rect x="{lx4}" y="4" width="12" height="10" rx="2" fill="hsl(40,72%,50%)" class="legend-rect"/>
<text x="{lx5}" y="13" class="legend">moderate</text>
<rect x="{lx6}" y="4" width="12" height="10" rx="2" fill="hsl(5,78%,47%)" class="legend-rect"/>
<text x="{lx7}" y="13" class="legend">heavy</text>
<rect x="{lx8}" y="4" width="12" height="10" rx="2" fill="hsl(270,50%,65%)" class="legend-rect" stroke-dasharray="4,2"/>
<text x="{lx9}" y="13" class="legend">shared</text>
<text x="{sx}" y="33" class="legend" style="cursor:pointer;text-decoration:underline" onclick="search()">[search]</text>
<text x="{cx}" y="33" class="legend" style="cursor:pointer;text-decoration:underline" onclick="clearSearch()">[clear]</text>
<text x="{rx}" y="33" class="legend" style="cursor:pointer;text-decoration:underline" onclick="resetZoom()">[reset zoom]</text>
<text id="search-status" x="{ssx}" y="33" class="legend" fill="rgb(204,68,68)"></text>
"#,
        lx0 = lx,
        lx1 = lx + 15.0,
        lx2 = lx + 75.0,
        lx3 = lx + 90.0,
        lx4 = lx + 138.0,
        lx5 = lx + 153.0,
        lx6 = lx + 205.0,
        lx7 = lx + 220.0,
        lx8 = lx + 265.0,
        lx9 = lx + 280.0,
        sx = lx + 330.0,
        cx = lx + 378.0,
        rx = lx + 420.0,
        ssx = lx + 330.0 - 80.0,
    ));

    // -- Frames --------------------------------------------------------------
    svg.push_str("<g id=\"frames\">\n");

    for r in &rects {
        let fill = rect_fill(r, max_weight);
        let tc = text_color(r);
        let cls = if r.is_workspace {
            "workspace"
        } else if r.is_shared {
            "shared"
        } else {
            "normal"
        };
        let label = fit_label(&r.name, r.weight, r.w);
        let tip = xml_escape(&tooltip(r));
        let ename = xml_escape(&r.name);

        svg.push_str(&format!(
            r#"<g class="frame" data-x="{x}" data-w="{w}" data-d="{d}" data-name="{ename}" data-weight="{weight}" onclick="zoom(evt)" onmouseover="hlOn(evt)" onmouseout="hlOff(evt)">
<title>{tip}</title>
<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="2" fill="{fill}" class="{cls}"/>
<text x="{tx}" y="{ty}" fill="{tc}">{elabel}</text>
</g>
"#,
            x = r.x,
            y = r.y,
            w = r.w,
            h = ROW_HEIGHT,
            d = r.depth,
            weight = r.weight,
            tx = r.x + TEXT_PAD,
            ty = r.y + 13.0,
            elabel = xml_escape(&label),
        ));
    }

    svg.push_str("</g>\n</svg>\n");

    writer.write_all(svg.as_bytes())?;
    Ok(())
}
