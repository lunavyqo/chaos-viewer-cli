//! Squarified treemap layout (same math as chaos-viewer / sm64ds-decomp).
//!
//! Modules are laid out first by total byte mass, then functions fill each
//! module's inner rectangle. The TUI paints the result into character cells.

use crate::schema::ChaosFunction;

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

/// Function rects only, sorted reading-order (top→bottom, left→right) for nav.
pub fn navigable_functions(rects: &[LayoutRect]) -> Vec<usize> {
    let mut idx: Vec<usize> = rects
        .iter()
        .enumerate()
        .filter(|(_, r)| !r.is_module)
        .map(|(i, _)| i)
        .collect();
    idx.sort_by(|&a, &b| {
        let (ax, ay) = rects[a].center();
        let (bx, by) = rects[b].center();
        ay.partial_cmp(&by)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| ax.partial_cmp(&bx).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| rects[a].id.cmp(&rects[b].id))
    });
    idx
}

/// Move selection to the nearest navigable neighbour in a cardinal direction.
///
/// `dir`: (-1,0) left, (1,0) right, (0,-1) up, (0,1) down.
pub fn step_spatial(
    rects: &[LayoutRect],
    nav: &[usize],
    current_nav_i: usize,
    dir_x: i8,
    dir_y: i8,
) -> usize {
    if nav.is_empty() {
        return 0;
    }
    let cur = nav[current_nav_i.min(nav.len() - 1)];
    let (cx, cy) = rects[cur].center();
    let mut best: Option<(usize, f64)> = None;

    for (ni, &ri) in nav.iter().enumerate() {
        if ni == current_nav_i {
            continue;
        }
        let (x, y) = rects[ri].center();
        let dx = x - cx;
        let dy = y - cy;
        // Must be primarily in the requested direction.
        let ok = if dir_x < 0 {
            dx < -0.01 && dx.abs() >= dy.abs() * 0.5
        } else if dir_x > 0 {
            dx > 0.01 && dx.abs() >= dy.abs() * 0.5
        } else if dir_y < 0 {
            dy < -0.01 && dy.abs() >= dx.abs() * 0.5
        } else if dir_y > 0 {
            dy > 0.01 && dy.abs() >= dx.abs() * 0.5
        } else {
            false
        };
        if !ok {
            continue;
        }
        let dist = dx * dx + dy * dy;
        if best.map(|(_, d)| dist < d).unwrap_or(true) {
            best = Some((ni, dist));
        }
    }

    best.map(|(ni, _)| ni).unwrap_or(current_nav_i)
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
    fn spatial_step_moves_right() {
        let rects = vec![
            LayoutRect {
                id: "L".into(),
                name: "L".into(),
                module: "m".into(),
                matched: false,
                is_module: false,
                module_label: None,
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            },
            LayoutRect {
                id: "R".into(),
                name: "R".into(),
                module: "m".into(),
                matched: true,
                is_module: false,
                module_label: None,
                x: 20.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            },
        ];
        let nav = navigable_functions(&rects);
        assert_eq!(nav, vec![0, 1]);
        let next = step_spatial(&rects, &nav, 0, 1, 0);
        assert_eq!(next, 1);
        let back = step_spatial(&rects, &nav, 1, -1, 0);
        assert_eq!(back, 0);
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
