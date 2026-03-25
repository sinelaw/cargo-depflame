use super::layout::{
    layout, LayoutRect, CHART_WIDTH, CHAR_WIDTH, ROW_HEIGHT, ROW_TOTAL, TEXT_PAD,
    HEADER_HEIGHT, FOOTER_HEIGHT,
};
use super::DepTreeData;
use std::collections::HashSet;
use std::io::Write;

// ---------------------------------------------------------------------------
// SVG rendering helpers.
// ---------------------------------------------------------------------------

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Map transitive weight to a fill colour.
///   - workspace members -> steel blue
///   - shared deps -> colour based on weight but with purple tint
///   - leaf (weight=1) -> cool green
///   - heavy -> warm red/orange
fn rect_fill(r: &LayoutRect, max_weight: usize, is_unused: bool) -> String {
    if is_unused {
        return "rgb(220,20,80)".to_string(); // bright magenta-red for unused deps
    }
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
        // cool violet -> warm magenta
        let hue = 280.0 - 40.0 * ratio; // 280 (violet) -> 240 (blue-purple)
        let sat = 45.0 + 20.0 * ratio;
        let lit = 65.0 - 10.0 * ratio;
        return format!("hsl({hue:.0},{sat:.0}%,{lit:.0}%)");
    }

    // Green -> yellow -> orange heat gradient based on transitive dep count.
    // Green = leaf (1 dep), orange = many transitive deps.
    let hue = 120.0 - 90.0 * ratio; // 120 (green) -> 30 (orange)
    let sat = 55.0 + 20.0 * ratio;
    let lit = 58.0 - 8.0 * ratio;
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

// ---------------------------------------------------------------------------
// Main SVG rendering.
// ---------------------------------------------------------------------------

pub(super) fn render_svg(
    tree: &DepTreeData,
    total_deps: usize,
    unused_edges: &[(String, String)],
    writer: &mut dyn Write,
) -> anyhow::Result<()> {
    let rects = layout(tree);
    if rects.is_empty() {
        writeln!(writer, "<svg xmlns='http://www.w3.org/2000/svg'/>")?;
        return Ok(());
    }

    let unused_edge_set: HashSet<(&str, &str)> = unused_edges
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();

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
    render_header(&mut svg, svg_height);

    // -- Title + subtitle ----------------------------------------------------
    svg.push_str(&format!(
        r#"<text x="6" y="18" class="title">depflame — Dependency Tree</text>
<text x="6" y="33" class="subtitle">{total_deps} deps total, {unique} unique crates shown, {shared_count} shared (dashed border = multiple parents) | click to zoom</text>
"#,
        unique = unique_nodes.len(),
    ));

    // -- Legend + controls ----------------------------------------------------
    render_legend(&mut svg);

    // -- Frames --------------------------------------------------------------
    render_frames(&mut svg, &rects, max_weight, &unused_edge_set);

    svg.push_str("</g>\n</svg>\n");

    writer.write_all(svg.as_bytes())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// SVG header with embedded CSS + JS.
// ---------------------------------------------------------------------------

fn render_header(svg: &mut String, svg_height: f64) {
    svg.push_str(&format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {CHART_WIDTH} {svg_height}"
     width="100%" font-family="Consolas,monospace" font-size="11">
<style>
  .frame {{ cursor: pointer; }}
  .frame:hover rect {{ stroke: #222; stroke-width: 1.5; }}
  .frame:hover rect.shared {{ stroke: #fff; stroke-width: 1.5; }}
  .frame text {{ pointer-events: none; }}
  rect.shared {{ stroke-dasharray: 4,2; stroke: rgba(100,70,130,0.5); stroke-width: 0.5; }}
  rect.normal {{ stroke: rgba(0,0,0,0.12); stroke-width: 0.5; }}
  rect.unused {{ stroke: rgba(220,20,80,0.8); stroke-width: 1.5; }}
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
}

// ---------------------------------------------------------------------------
// Legend + controls.
// ---------------------------------------------------------------------------

fn render_legend(svg: &mut String) {
    const LEGEND_FONT_WIDTH: f64 = 6.0;
    const LEGEND_SWATCH: f64 = 12.0;
    const LEGEND_GAP: f64 = 6.0;
    const SWATCH_TEXT_GAP: f64 = 3.0;

    struct LegendItem {
        label: &'static str,
        fill: &'static str,
        dash: bool,
    }
    let legend_items = [
        LegendItem { label: "workspace", fill: "rgb(70,130,180)", dash: false },
        LegendItem { label: "leaf (0 deps)", fill: "hsl(120,55%,58%)", dash: false },
        LegendItem { label: "some deps", fill: "hsl(75,65%,54%)", dash: false },
        LegendItem { label: "many deps", fill: "hsl(30,75%,50%)", dash: false },
        LegendItem { label: "shared", fill: "hsl(270,50%,65%)", dash: true },
        LegendItem { label: "unused", fill: "rgb(220,20,80)", dash: false },
    ];

    // Compute total legend width so we can right-align it.
    let legend_total_width: f64 = legend_items.iter().enumerate().map(|(i, item)| {
        let text_w = item.label.len() as f64 * LEGEND_FONT_WIDTH;
        let item_w = LEGEND_SWATCH + SWATCH_TEXT_GAP + text_w;
        item_w + if i > 0 { LEGEND_GAP } else { 0.0 }
    }).sum();

    let mut lx = CHART_WIDTH - legend_total_width - 6.0; // 6px right margin
    for item in &legend_items {
        let dash_attr = if item.dash { r#" stroke-dasharray="4,2""# } else { "" };
        let text_x = lx + LEGEND_SWATCH + SWATCH_TEXT_GAP;
        let text_w = item.label.len() as f64 * LEGEND_FONT_WIDTH;
        svg.push_str(&format!(
            r#"<rect x="{lx}" y="4" width="{LEGEND_SWATCH}" height="10" rx="2" fill="{fill}" class="legend-rect"{dash}/>
<text x="{tx}" y="13" class="legend">{label}</text>
"#,
            lx = lx,
            fill = item.fill,
            dash = dash_attr,
            tx = text_x,
            label = item.label,
        ));
        lx += LEGEND_SWATCH + SWATCH_TEXT_GAP + text_w + LEGEND_GAP;
    }

    // Controls row: right-align [search] [clear] [reset zoom]
    struct ControlItem {
        label: &'static str,
        onclick: &'static str,
    }
    let controls = [
        ControlItem { label: "[search]", onclick: "search()" },
        ControlItem { label: "[clear]", onclick: "clearSearch()" },
        ControlItem { label: "[reset zoom]", onclick: "resetZoom()" },
    ];

    let ctrl_total_width: f64 = controls.iter().enumerate().map(|(i, c)| {
        c.label.len() as f64 * LEGEND_FONT_WIDTH + if i > 0 { LEGEND_GAP } else { 0.0 }
    }).sum();

    let mut cx = CHART_WIDTH - ctrl_total_width - 6.0;
    for ctrl in &controls {
        let text_w = ctrl.label.len() as f64 * LEGEND_FONT_WIDTH;
        svg.push_str(&format!(
            r#"<text x="{x}" y="33" class="legend" style="cursor:pointer;text-decoration:underline" onclick="{onclick}">{label}</text>
"#,
            x = cx,
            onclick = ctrl.onclick,
            label = ctrl.label,
        ));
        cx += text_w + LEGEND_GAP;
    }

    // Search status text (to the left of controls)
    let ssx = CHART_WIDTH - ctrl_total_width - 6.0 - 80.0;
    svg.push_str(&format!(
        r#"<text id="search-status" x="{ssx}" y="33" class="legend" fill="rgb(204,68,68)"></text>
"#,
    ));
}

// ---------------------------------------------------------------------------
// Frame rendering.
// ---------------------------------------------------------------------------

fn render_frames(
    svg: &mut String,
    rects: &[LayoutRect],
    max_weight: usize,
    unused_edge_set: &HashSet<(&str, &str)>,
) {
    svg.push_str("<g id=\"frames\">\n");

    for r in rects {
        let is_unused = !r.parent_name.is_empty()
            && unused_edge_set.contains(&(r.parent_name.as_str(), r.name.as_str()));
        let fill = rect_fill(r, max_weight, is_unused);
        let tc = if is_unused { "#fff" } else { text_color(r) };
        let cls = if is_unused {
            "unused"
        } else if r.is_workspace {
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
}
