//! Squarified treemap layout (same math as chaos-viewer / sm64ds-decomp).
//!
//! Modules are laid out first by total byte mass, then functions fill each
//! module's inner rectangle. The TUI paints via a **braille** raster (2×4
//! sub-pixels per character cell) so dense maps keep borders and tiny
//! functions still light at least one dot.

use crate::schema::ChaosFunction;

/// Braille sub-pixel grid: 2 wide × 4 tall per terminal character.
pub const BRAILLE_W: usize = 2;
pub const BRAILLE_H: usize = 4;

/// Input leaf for layout (function or synthetic).
#[derive(Debug, Clone)]
pub struct TreemapLeaf {
    pub id: String,
    pub module: String,
    pub name: String,
    pub size: u64,
    pub matched: bool,
}

impl From<&ChaosFunction> for TreemapLeaf {
    fn from(f: &ChaosFunction) -> Self {
        Self {
            id: f.id.clone(),
            module: f.module.clone(),
            name: f.name.clone(),
            size: f.size,
            matched: f.matched,
        }
    }
}

/// One laid-out rectangle in continuous canvas coordinates (origin top-left).
#[derive(Debug, Clone)]
pub struct LayoutRect {
    pub id: String,
    pub name: String,
    pub module: String,
    pub matched: bool,
    pub is_module: bool,
    /// Label for module chrome, e.g. `"arm9 42.1%"`.
    pub module_label: Option<String>,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl LayoutRect {
    /// Integer cell bounds covering this rect inside a `cols`×`rows` canvas.
    /// Returns `None` if the rect collapses to zero cells.
    pub fn cell_bounds(&self, cols: u16, rows: u16) -> Option<(u16, u16, u16, u16)> {
        if cols == 0 || rows == 0 || self.w <= 0.0 || self.h <= 0.0 {
            return None;
        }
        let x0 = self.x.floor().max(0.0) as i32;
        let y0 = self.y.floor().max(0.0) as i32;
        let x1 = (self.x + self.w).ceil() as i32;
        let y1 = (self.y + self.h).ceil() as i32;
        let x0 = x0.clamp(0, cols as i32) as u16;
        let y0 = y0.clamp(0, rows as i32) as u16;
        let x1 = x1.clamp(0, cols as i32) as u16;
        let y1 = y1.clamp(0, rows as i32) as u16;
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some((x0, y0, x1 - x0, y1 - y0))
    }

    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w * 0.5, self.y + self.h * 0.5)
    }
}

const PAD: f64 = 0.35;
const LABEL_H: f64 = 1.0;
const INNER: f64 = 0.15;

/// Build a two-level treemap: modules, then functions inside each module.
///
/// When `module_filter` is `Some(name)`, only that module is laid out (full canvas).
pub fn layout_treemap(
    functions: &[TreemapLeaf],
    width: f64,
    height: f64,
    module_filter: Option<&str>,
) -> Vec<LayoutRect> {
    if width <= 0.0 || height <= 0.0 || functions.is_empty() {
        return Vec::new();
    }

    let mut by_mod: Vec<(String, Vec<&TreemapLeaf>)> = Vec::new();
    for f in functions {
        if let Some(want) = module_filter {
            if f.module != want {
                continue;
            }
        }
        if let Some((_, recs)) = by_mod.iter_mut().find(|(m, _)| m == &f.module) {
            recs.push(f);
        } else {
            by_mod.push((f.module.clone(), vec![f]));
        }
    }

    // Sort modules by descending mass for stable, readable layout.
    by_mod.sort_by(|(a, ra), (b, rb)| {
        let ba: u64 = ra.iter().map(|f| f.size.max(1)).sum();
        let bb: u64 = rb.iter().map(|f| f.size.max(1)).sum();
        bb.cmp(&ba).then_with(|| a.cmp(b))
    });

    struct ModPack<'a> {
        label: String,
        recs: Vec<&'a TreemapLeaf>,
        bytes: u64,
        done_bytes: u64,
    }

    let mods: Vec<ModPack<'_>> = by_mod
        .into_iter()
        .map(|(label, recs)| {
            let bytes: u64 = recs.iter().map(|f| f.size.max(1)).sum();
            let done_bytes: u64 = recs
                .iter()
                .filter(|f| f.matched)
                .map(|f| f.size.max(1))
                .sum();
            ModPack {
                label,
                recs,
                bytes,
                done_bytes,
            }
        })
        .filter(|m| m.bytes > 0)
        .collect();

    if mods.is_empty() {
        return Vec::new();
    }

    // Single-module zoom: skip outer module chrome, fill the canvas with functions.
    if mods.len() == 1 && module_filter.is_some() {
        let m = &mods[0];
        return layout_functions_in(&m.recs, 0.0, 0.0, width, height);
    }

    let mod_items: Vec<ValueItem<usize>> = mods
        .iter()
        .enumerate()
        .map(|(i, m)| ValueItem {
            value: m.bytes as f64,
            payload: i,
        })
        .collect();
    let mod_boxes = squarify(&mod_items, 0.0, 0.0, width, height);

    let mut out = Vec::new();
    for b in mod_boxes {
        let m = &mods[b.item.payload];
        let bx = b.x + PAD * 0.5;
        let by = b.y + PAD * 0.5;
        let bw = b.w - PAD;
        let bh = b.h - PAD;
        if bw < 0.5 || bh < 0.5 {
            continue;
        }

        let pct = if m.bytes > 0 {
            (m.done_bytes as f64 / m.bytes as f64) * 100.0
        } else {
            0.0
        };

        out.push(LayoutRect {
            id: format!("mod:{}", m.label),
            name: m.label.clone(),
            module: m.label.clone(),
            matched: false,
            is_module: true,
            module_label: Some(format!("{} {:.1}%", m.label, pct)),
            x: bx,
            y: by,
            w: bw,
            h: bh,
        });

        let show_label = bh > LABEL_H + 0.8 && bw > 4.0;
        let inner_y = by + if show_label { LABEL_H } else { 0.0 } + INNER;
        let inner_h = bh - if show_label { LABEL_H } else { 0.0 } - INNER * 2.0;
        let inner_x = bx + INNER;
        let inner_w = bw - INNER * 2.0;
        if inner_w < 0.4 || inner_h < 0.4 {
            continue;
        }

        out.extend(layout_functions_in(
            &m.recs, inner_x, inner_y, inner_w, inner_h,
        ));
    }

    out
}

fn layout_functions_in(recs: &[&TreemapLeaf], x: f64, y: f64, w: f64, h: f64) -> Vec<LayoutRect> {
    let items: Vec<ValueItem<usize>> = recs
        .iter()
        .enumerate()
        .map(|(i, r)| ValueItem {
            value: r.size.max(1) as f64,
            payload: i,
        })
        .collect();
    let boxes = squarify(&items, x, y, w, h);
    let mut out = Vec::with_capacity(boxes.len());
    for b in boxes {
        if b.w < 0.05 || b.h < 0.05 {
            continue;
        }
        let r = recs[b.item.payload];
        out.push(LayoutRect {
            id: r.id.clone(),
            name: r.name.clone(),
            module: r.module.clone(),
            matched: r.matched,
            is_module: false,
            module_label: None,
            x: b.x,
            y: b.y,
            w: b.w,
            h: b.h,
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Squarify (Bruls / sm64ds-decomp / chaos-viewer)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct ValueItem<T> {
    value: f64,
    payload: T,
}

struct SquarifyBox<T> {
    item: ValueItem<T>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[derive(Clone)]
struct Scaled<T> {
    it: ValueItem<T>,
    area: f64,
}

/// Squarified treemap: items with positive `value`, axis-aligned packing.
fn squarify<T: Clone>(
    items: &[ValueItem<T>],
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> Vec<SquarifyBox<T>> {
    let filtered: Vec<&ValueItem<T>> = items.iter().filter(|it| it.value > 0.0).collect();
    if filtered.is_empty() || w <= 0.0 || h <= 0.0 {
        return Vec::new();
    }
    let total: f64 = filtered.iter().map(|it| it.value).sum();
    if total <= 0.0 {
        return Vec::new();
    }

    let scale = (w * h) / total;
    let scaled: Vec<Scaled<T>> = filtered
        .into_iter()
        .map(|it| Scaled {
            it: it.clone(),
            area: it.value * scale,
        })
        .collect();

    let mut out = Vec::new();
    let mut rx = x;
    let mut ry = y;
    let mut rw = w;
    let mut rh = h;
    let mut i = 0usize;
    let n = scaled.len();

    while i < n {
        let short = rw.min(rh);
        let mut row = vec![Scaled {
            it: scaled[i].it.clone(),
            area: scaled[i].area,
        }];
        i += 1;

        while i < n {
            let mut trial = row.clone();
            trial.push(Scaled {
                it: scaled[i].it.clone(),
                area: scaled[i].area,
            });
            if worst(&trial, short) <= worst(&row, short) {
                row = trial;
                i += 1;
            } else {
                break;
            }
        }

        out.extend(layout_row(&row, rx, ry, rw, rh));

        let row_sum: f64 = row.iter().map(|r| r.area).sum();
        if rw <= rh {
            let dh = if rw > 0.0 { row_sum / rw } else { 0.0 };
            ry += dh;
            rh -= dh;
        } else {
            let dw = if rh > 0.0 { row_sum / rh } else { 0.0 };
            rx += dw;
            rw -= dw;
        }
    }
    out
}

fn worst<T>(row: &[Scaled<T>], short: f64) -> f64 {
    let s: f64 = row.iter().map(|r| r.area).sum();
    if s <= 0.0 || short <= 0.0 {
        return f64::INFINITY;
    }
    let mx = row.iter().map(|r| r.area).fold(f64::NEG_INFINITY, f64::max);
    let mn = row.iter().map(|r| r.area).fold(f64::INFINITY, f64::min);
    ((short * short * mx) / (s * s)).max((s * s) / (short * short * mn))
}

fn layout_row<T: Clone>(
    row: &[Scaled<T>],
    rx: f64,
    ry: f64,
    rw: f64,
    rh: f64,
) -> Vec<SquarifyBox<T>> {
    let s: f64 = row.iter().map(|r| r.area).sum();
    if s <= 0.0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(row.len());
    if rw <= rh {
        let dh = s / rw;
        let mut cx = rx;
        for r in row {
            let cw = if dh > 0.0 { r.area / dh } else { 0.0 };
            out.push(SquarifyBox {
                item: r.it.clone(),
                x: cx,
                y: ry,
                w: cw,
                h: dh,
            });
            cx += cw;
        }
    } else {
        let dw = s / rh;
        let mut cy = ry;
        for r in row {
            let ch = if dw > 0.0 { r.area / dw } else { 0.0 };
            out.push(SquarifyBox {
                item: r.it.clone(),
                x: rx,
                y: cy,
                w: dw,
                h: ch,
            });
            cy += ch;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Braille raster (higher effective resolution in the terminal)
// ---------------------------------------------------------------------------

/// One painted function in the raster (only functions that own ≥1 sub-pixel).
#[derive(Debug, Clone)]
pub struct RasterFn {
    pub id: String,
    pub name: String,
    pub module: String,
    pub matched: bool,
    /// Centroid in sub-pixel coordinates (for spatial nav).
    pub cx: f64,
    pub cy: f64,
    pub pixel_count: u32,
}

/// One terminal character cell after braille encoding.
#[derive(Debug, Clone)]
pub struct BrailleCell {
    /// Unicode braille glyph (U+2800 … U+28FF).
    pub ch: char,
    /// Primary function index into [`HeatmapFrame::functions`] for colouring.
    /// `None` = empty / border gap.
    pub color_fn: Option<usize>,
    /// True if the currently selected function owns any dot in this cell.
    pub has_selected: bool,
}

/// Ready-to-paint heatmap at character resolution.
#[derive(Debug, Clone)]
pub struct HeatmapFrame {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<BrailleCell>,
    pub functions: Vec<RasterFn>,
    /// Indices into `functions`, reading order (top→bottom, left→right).
    pub nav: Vec<usize>,
    /// Module label overlays in character cells: (col, row, text).
    pub module_labels: Vec<(u16, u16, String)>,
}

impl HeatmapFrame {
    pub fn cell(&self, col: u16, row: u16) -> Option<&BrailleCell> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        self.cells
            .get(row as usize * self.cols as usize + col as usize)
    }
}

/// Build a braille heatmap for the terminal map pane.
///
/// Layout runs in **sub-pixel** space (`cols*2` × `rows*4`). Function rects are
/// inset by ~½px so empty dots form thin borders. Any function with a layout
/// box but zero floor-pixels still gets a single forced dot so it can light up
/// and be selected.
pub fn build_heatmap_frame(
    leaves: &[TreemapLeaf],
    cols: u16,
    rows: u16,
    module_filter: Option<&str>,
    selected_id: Option<&str>,
) -> HeatmapFrame {
    let cols = cols.max(1);
    let rows = rows.max(1);
    let px_w = cols as usize * BRAILLE_W;
    let px_h = rows as usize * BRAILLE_H;

    let rects = layout_treemap(leaves, px_w as f64, px_h as f64, module_filter);

    // owner[y*px_w + x] = Some(fn_index) into `fn_meta` as we build it
    let mut owners: Vec<Option<usize>> = vec![None; px_w * px_h];
    let mut fn_meta: Vec<RasterFn> = Vec::new();
    // id -> index in fn_meta
    let mut id_to_fn: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    let mut module_labels = Vec::new();
    for r in &rects {
        if r.is_module {
            if let Some(label) = &r.module_label {
                // Character-cell position of module top-left.
                let cc = (r.x / BRAILLE_W as f64).floor().max(0.0) as u16;
                let cr = (r.y / BRAILLE_H as f64).floor().max(0.0) as u16;
                if cc < cols && cr < rows {
                    module_labels.push((cc, cr, label.clone()));
                }
            }
            continue;
        }

        let fi = *id_to_fn.entry(r.id.clone()).or_insert_with(|| {
            let i = fn_meta.len();
            fn_meta.push(RasterFn {
                id: r.id.clone(),
                name: r.name.clone(),
                module: r.module.clone(),
                matched: r.matched,
                cx: r.center().0,
                cy: r.center().1,
                pixel_count: 0,
            });
            i
        });

        // Inset for borders: shrink by 0.5px each side when large enough.
        let inset = if r.w >= 2.0 && r.h >= 2.0 { 0.5 } else { 0.0 };
        let x0 = (r.x + inset).floor() as i32;
        let y0 = (r.y + inset).floor() as i32;
        let x1 = (r.x + r.w - inset).ceil() as i32;
        let y1 = (r.y + r.h - inset).ceil() as i32;

        let mut painted = 0u32;
        for py in y0.max(0)..y1.min(px_h as i32) {
            for px in x0.max(0)..x1.min(px_w as i32) {
                let idx = py as usize * px_w + px as usize;
                // Last writer wins on ties; borders stay empty when inset applied.
                owners[idx] = Some(fi);
                painted += 1;
            }
        }

        // Tiny functions: guarantee at least one lit sub-pixel at the centre.
        if painted == 0 {
            let cx = r.center().0.round() as i32;
            let cy = r.center().1.round() as i32;
            let cx = cx.clamp(0, px_w as i32 - 1) as usize;
            let cy = cy.clamp(0, px_h as i32 - 1) as usize;
            owners[cy * px_w + cx] = Some(fi);
            painted = 1;
        }

        fn_meta[fi].pixel_count = painted;
        // Recompute centroid from painted pixels later if needed; seed with layout.
        let _ = painted;
    }

    // Accurate centroids from painted pixels (stable spatial nav).
    let mut sum_x = vec![0.0f64; fn_meta.len()];
    let mut sum_y = vec![0.0f64; fn_meta.len()];
    let mut counts = vec![0u32; fn_meta.len()];
    for py in 0..px_h {
        for px in 0..px_w {
            if let Some(fi) = owners[py * px_w + px] {
                sum_x[fi] += px as f64 + 0.5;
                sum_y[fi] += py as f64 + 0.5;
                counts[fi] += 1;
            }
        }
    }
    for (i, f) in fn_meta.iter_mut().enumerate() {
        if counts[i] > 0 {
            f.cx = sum_x[i] / counts[i] as f64;
            f.cy = sum_y[i] / counts[i] as f64;
            f.pixel_count = counts[i];
        }
    }

    // Encode braille cells.
    // Dot bit map for (dx, dy) in 0..2 × 0..4:
    const DOT_BITS: [[u8; BRAILLE_H]; BRAILLE_W] = [
        [0x01, 0x02, 0x04, 0x40], // left column: dots 1,2,3,7
        [0x08, 0x10, 0x20, 0x80], // right column: dots 4,5,6,8
    ];

    let mut cells = Vec::with_capacity(cols as usize * rows as usize);
    for row in 0..rows as usize {
        for col in 0..cols as usize {
            let mut mask: u8 = 0;
            let mut votes: Vec<(usize, u32)> = Vec::new();
            let mut has_selected = false;

            for (dx, col_bits) in DOT_BITS.iter().enumerate() {
                for (dy, &bit) in col_bits.iter().enumerate() {
                    let px = col * BRAILLE_W + dx;
                    let py = row * BRAILLE_H + dy;
                    if let Some(fi) = owners[py * px_w + px] {
                        mask |= bit;
                        if let Some((_, c)) = votes.iter_mut().find(|(f, _)| *f == fi) {
                            *c += 1;
                        } else {
                            votes.push((fi, 1));
                        }
                        if selected_id == Some(fn_meta[fi].id.as_str()) {
                            has_selected = true;
                        }
                    }
                }
            }

            // Prefer selected function as colour owner when present, else majority.
            let color_fn = if has_selected {
                selected_id.and_then(|sid| fn_meta.iter().position(|f| f.id == sid))
            } else {
                votes.into_iter().max_by_key(|(_, c)| *c).map(|(fi, _)| fi)
            };

            let ch = char::from_u32(0x2800 + mask as u32).unwrap_or('\u{2800}');
            cells.push(BrailleCell {
                ch,
                color_fn,
                has_selected,
            });
        }
    }

    // Nav: only functions that actually painted, reading order by centroid.
    let mut nav: Vec<usize> = (0..fn_meta.len())
        .filter(|&i| fn_meta[i].pixel_count > 0)
        .collect();
    nav.sort_by(|&a, &b| {
        fn_meta[a]
            .cy
            .partial_cmp(&fn_meta[b].cy)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                fn_meta[a]
                    .cx
                    .partial_cmp(&fn_meta[b].cx)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| fn_meta[a].id.cmp(&fn_meta[b].id))
    });

    HeatmapFrame {
        cols,
        rows,
        cells,
        functions: fn_meta,
        nav,
        module_labels,
    }
}

/// Sequential step along reading-order nav (stable, no jumps).
pub fn step_sequential(nav_len: usize, current: usize, delta: isize) -> usize {
    if nav_len == 0 {
        return 0;
    }
    let n = nav_len as isize;
    let i = current.min(nav_len - 1) as isize + delta;
    (((i % n) + n) % n) as usize
}

/// Spatial step using painted centroids among `nav` (indices into functions).
///
/// Prefers the nearest neighbour whose vector is mostly aligned with `dir`.
pub fn step_spatial_centroids(
    functions: &[RasterFn],
    nav: &[usize],
    current_nav_i: usize,
    dir_x: i8,
    dir_y: i8,
) -> usize {
    if nav.is_empty() {
        return 0;
    }
    let cur_i = current_nav_i.min(nav.len() - 1);
    let cur = &functions[nav[cur_i]];
    let (cx, cy) = (cur.cx, cur.cy);

    let mut best: Option<(usize, f64)> = None;
    for (ni, &fi) in nav.iter().enumerate() {
        if ni == cur_i {
            continue;
        }
        let f = &functions[fi];
        let dx = f.cx - cx;
        let dy = f.cy - cy;
        // Require movement into the requested half-plane; mild cone (not 45° hard).
        let ok = if dir_x < 0 {
            dx < -0.25
        } else if dir_x > 0 {
            dx > 0.25
        } else if dir_y < 0 {
            dy < -0.25
        } else if dir_y > 0 {
            dy > 0.25
        } else {
            false
        };
        if !ok {
            continue;
        }
        // Score: primarily distance along the direction axis, then total distance.
        let along = if dir_x != 0 { dx.abs() } else { dy.abs() };
        let across = if dir_x != 0 { dy.abs() } else { dx.abs() };
        let score = along + across * 0.35;
        if best.map(|(_, s)| score < s).unwrap_or(true) {
            best = Some((ni, score));
        }
    }
    best.map(|(ni, _)| ni).unwrap_or(cur_i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(id: &str, module: &str, size: u64, matched: bool) -> TreemapLeaf {
        TreemapLeaf {
            id: id.into(),
            module: module.into(),
            name: id.into(),
            size,
            matched,
        }
    }

    #[test]
    fn squarify_covers_canvas_area() {
        let items = vec![
            ValueItem {
                value: 100.0,
                payload: 0,
            },
            ValueItem {
                value: 50.0,
                payload: 1,
            },
            ValueItem {
                value: 25.0,
                payload: 2,
            },
        ];
        let boxes = squarify(&items, 0.0, 0.0, 100.0, 50.0);
        assert_eq!(boxes.len(), 3);
        let area: f64 = boxes.iter().map(|b| b.w * b.h).sum();
        assert!((area - 5000.0).abs() < 1e-6, "area={area}");
    }

    #[test]
    fn layout_two_modules() {
        let fns = vec![
            leaf("a:1", "arm9", 100, true),
            leaf("a:2", "arm9", 50, false),
            leaf("o:1", "ov0", 200, false),
            leaf("o:2", "ov0", 10, true),
        ];
        let rects = layout_treemap(&fns, 80.0, 40.0, None);
        let modules = rects.iter().filter(|r| r.is_module).count();
        let funcs = rects.iter().filter(|r| !r.is_module).count();
        assert_eq!(modules, 2);
        assert_eq!(funcs, 4);
        assert!(rects.iter().any(|r| r.id == "a:1" && r.matched));
        assert!(rects
            .iter()
            .any(|r| r.is_module && r.module_label.as_deref().unwrap_or("").starts_with("ov0")));
    }

    #[test]
    fn module_filter_fills_canvas() {
        let fns = vec![
            leaf("a:1", "arm9", 100, true),
            leaf("a:2", "arm9", 50, false),
            leaf("o:1", "ov0", 200, false),
        ];
        let rects = layout_treemap(&fns, 40.0, 20.0, Some("arm9"));
        assert!(rects.iter().all(|r| !r.is_module || r.module == "arm9"));
        assert_eq!(rects.iter().filter(|r| !r.is_module).count(), 2);
    }

    #[test]
    fn braille_frame_paints_and_navigates() {
        let fns = vec![
            leaf("L", "m", 100, false),
            leaf("R", "m", 100, true),
            leaf("tiny", "m", 1, false),
        ];
        let frame = build_heatmap_frame(&fns, 40, 12, None, Some("tiny"));
        assert!(!frame.functions.is_empty());
        assert_eq!(frame.nav.len(), frame.functions.len());
        // Tiny still gets at least one pixel.
        let tiny = frame.functions.iter().find(|f| f.id == "tiny").unwrap();
        assert!(tiny.pixel_count >= 1);
        // Some braille cells are non-empty.
        assert!(frame.cells.iter().any(|c| c.ch != '\u{2800}'));
        // Selected marks cells.
        assert!(frame.cells.iter().any(|c| c.has_selected));

        let right = step_spatial_centroids(&frame.functions, &frame.nav, 0, 1, 0);
        assert!(right < frame.nav.len());
        let next = step_sequential(frame.nav.len(), 0, 1);
        assert_eq!(next, 1);
    }

    #[test]
    fn cell_bounds_clamps() {
        let r = LayoutRect {
            id: "x".into(),
            name: "x".into(),
            module: "m".into(),
            matched: false,
            is_module: false,
            module_label: None,
            x: 1.2,
            y: 2.8,
            w: 3.1,
            h: 1.1,
        };
        let (x, y, w, h) = r.cell_bounds(80, 40).unwrap();
        assert_eq!((x, y), (1, 2));
        assert!(w >= 3 && h >= 1);
    }
}
