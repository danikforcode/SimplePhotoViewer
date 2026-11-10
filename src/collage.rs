// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Шаблоны коллажа: ячейки для n фото (сетка, крупное слева/сверху, в ряд,
//! в столбик). Само окно — «Холст» (canvas.rs): там фото раскладываются
//! шаблоном и дальше двигаются свободно.

use crate::image_buf::Img;
use std::path::PathBuf;
use std::rc::Rc;

// ------------------------------------------------------------ чистая логика ---
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layout {
    Grid,
    BigLeft,
    BigTop,
    Row,
    Column,
}

pub const LAYOUTS: [(Layout, &str); 5] = [
    (Layout::Grid, "Сетка"),
    (Layout::BigLeft, "Крупное слева"),
    (Layout::BigTop, "Крупное сверху"),
    (Layout::Row, "В ряд"),
    (Layout::Column, "В столбик"),
];

pub type Cell = (f64, f64, f64, f64);

fn grid(area: Cell, n: usize, cols: usize, gap: f64) -> Vec<Cell> {
    if n == 0 {
        return vec![];
    }
    let cols = cols.clamp(1, n);
    let rows = n.div_ceil(cols);
    let cw = (area.2 - gap * (cols - 1) as f64) / cols as f64;
    let ch = (area.3 - gap * (rows - 1) as f64) / rows as f64;
    (0..n)
        .map(|i| {
            let (r, c) = (i / cols, i % cols);
            let in_row = (n - r * cols).min(cols);
            let shift = (cols - in_row) as f64 * (cw + gap) / 2.0; // неполный ряд — по центру
            (area.0 + shift + c as f64 * (cw + gap), area.1 + r as f64 * (ch + gap), cw, ch)
        })
        .collect()
}

/// Число столбцов сетки: ячейки ближе к пропорциям фото, меньше пустых мест.
fn best_cols(n: usize, w: f64, h: f64, aspect: f64) -> usize {
    (1..=n)
        .map(|cols| {
            let rows = n.div_ceil(cols);
            let cell = (w / cols as f64) / (h / rows as f64);
            let score = (cell / aspect).ln().abs() + 0.35 * (rows * cols - n) as f64;
            (cols, score)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|c| c.0)
        .unwrap_or(1)
}

/// Ячейки для n фото на холсте w×h. aspects — пропорции фото (ш/в).
pub fn cells(layout: Layout, aspects: &[f64], w: f64, h: f64, gap: f64) -> Vec<Cell> {
    let n = aspects.len();
    if n == 0 {
        return vec![];
    }
    let area = (gap, gap, (w - 2.0 * gap).max(1.0), (h - 2.0 * gap).max(1.0));
    let mean = (aspects.iter().map(|a| a.ln()).sum::<f64>() / n as f64).exp();
    if n == 1 {
        return vec![area];
    }
    match layout {
        Layout::Grid => grid(area, n, best_cols(n, area.2, area.3, mean), gap),
        Layout::Row => {
            let total = area.2 - gap * (n - 1) as f64;
            let sum: f64 = aspects.iter().sum();
            let mut x = area.0;
            aspects
                .iter()
                .map(|a| {
                    let cw = total * a / sum;
                    let c = (x, area.1, cw, area.3);
                    x += cw + gap;
                    c
                })
                .collect()
        }
        Layout::Column => {
            let total = area.3 - gap * (n - 1) as f64;
            let sum: f64 = aspects.iter().map(|a| 1.0 / a).sum();
            let mut y = area.1;
            aspects
                .iter()
                .map(|a| {
                    let ch = total * (1.0 / a) / sum;
                    let c = (area.0, y, area.2, ch);
                    y += ch + gap;
                    c
                })
                .collect()
        }
        Layout::BigLeft => {
            let bw = (area.2 - gap) * 0.62;
            let mut v = vec![(area.0, area.1, bw, area.3)];
            let rest = (area.0 + bw + gap, area.1, area.2 - bw - gap, area.3);
            v.extend(grid(rest, n - 1, if n - 1 <= 3 { 1 } else { 2 }, gap));
            v
        }
        Layout::BigTop => {
            let bh = (area.3 - gap) * 0.62;
            let mut v = vec![(area.0, area.1, area.2, bh)];
            let rest = (area.0, area.1 + bh + gap, area.2, area.3 - bh - gap);
            let m = n - 1;
            v.extend(grid(rest, m, if m <= 4 { m } else { m.div_ceil(2) }, gap));
            v
        }
    }
}

// ----------------------------------------------------------- элемент ---
/// Фото для холста/коллажа: из файла или из памяти.
pub struct Item {
    pub path: Option<PathBuf>,
    pub img: Option<Rc<Img>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inside(c: &Cell, w: f64, h: f64) -> bool {
        c.0 >= -1e-6 && c.1 >= -1e-6 && c.0 + c.2 <= w + 1e-6 && c.1 + c.3 <= h + 1e-6 && c.2 > 0.0 && c.3 > 0.0
    }

    fn no_overlap(cs: &[Cell]) -> bool {
        for i in 0..cs.len() {
            for j in i + 1..cs.len() {
                let (a, b) = (cs[i], cs[j]);
                let ox = a.0 < b.0 + b.2 - 1e-6 && b.0 < a.0 + a.2 - 1e-6;
                let oy = a.1 < b.1 + b.3 - 1e-6 && b.1 < a.1 + a.3 - 1e-6;
                if ox && oy {
                    return false;
                }
            }
        }
        true
    }

    #[test]
    fn all_layouts_fit() {
        for (l, _) in LAYOUTS {
            for n in 1..=10 {
                let asp: Vec<f64> = (0..n).map(|i| if i % 2 == 0 { 1.5 } else { 0.75 }).collect();
                let cs = cells(l, &asp, 3000.0, 2000.0, 20.0);
                assert_eq!(cs.len(), n, "{l:?} {n}");
                assert!(cs.iter().all(|c| inside(c, 3000.0, 2000.0)), "{l:?} {n}: {cs:?}");
                assert!(no_overlap(&cs), "{l:?} {n}: {cs:?}");
            }
        }
    }

    #[test]
    fn grid_shape() {
        // 4 фото 4:3 на холсте 4:3 → 2×2
        let cs = cells(Layout::Grid, &[4.0 / 3.0; 4], 4000.0, 3000.0, 0.0);
        assert!((cs[0].2 - 2000.0).abs() < 1e-6 && (cs[0].3 - 1500.0).abs() < 1e-6);
        // 3 фото: неполный ряд по центру
        let cs = cells(Layout::Grid, &[1.0; 3], 2000.0, 2000.0, 0.0);
        assert_eq!(cs.len(), 3);
        assert!((cs[2].0 - 500.0).abs() < 1e-6, "{cs:?}");
    }

    #[test]
    fn row_keeps_aspect() {
        let asp = [1.5, 0.75, 1.0];
        let h = 1000.0;
        let w: f64 = asp.iter().sum::<f64>() * h;
        let cs = cells(Layout::Row, &asp, w, h, 0.0);
        for (c, a) in cs.iter().zip(asp) {
            assert!((c.2 / c.3 - a).abs() < 0.01, "{c:?} {a}");
        }
    }
}
