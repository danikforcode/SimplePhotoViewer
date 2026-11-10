// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Правка фото: выделение (рамка, овал, лассо, волшебная палочка), настройки
//! с ползунками (яркость, экспозиция, контраст, светлые/тени, насыщенность,
//! теплота, оттенок, резкость, размытие, виньетка) и разовые эффекты.
//! Всё, что зависит от размера (радиус размытия, клетка пикселизации),
//! задано относительно длинной стороны — поэтому уменьшенный предпросмотр
//! выглядит так же, как итог в полном размере. Чистая логика, без Win32.

use crate::image_buf::Img;

// ------------------------------------------------------------- выделение ---

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SelOp {
    Replace,
    Add,
    Sub,
    Intersect,
}

/// Маска выделения: 0 — не выделено, 255 — выделено, между — мягкий край.
#[derive(Clone)]
pub struct Sel {
    pub w: u32,
    pub h: u32,
    pub mask: Vec<u8>,
}

impl Sel {
    pub fn empty(w: u32, h: u32) -> Sel {
        Sel { w, h, mask: vec![0; w as usize * h as usize] }
    }

    pub fn all(w: u32, h: u32) -> Sel {
        Sel { w, h, mask: vec![255; w as usize * h as usize] }
    }

    /// Прямоугольник (координаты в точках фото, углы в любом порядке).
    pub fn rect(w: u32, h: u32, a: (f64, f64), b: (f64, f64)) -> Sel {
        let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
        let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
        let mut s = Sel::empty(w, h);
        let cov = |p: f64, lo: f64, hi: f64| ((p + 1.0).min(hi) - p.max(lo)).clamp(0.0, 1.0);
        let (xa, xb) = (x0.floor().max(0.0) as u32, (x1.ceil() as u32).min(w));
        let (ya, yb) = (y0.floor().max(0.0) as u32, (y1.ceil() as u32).min(h));
        for y in ya..yb {
            let cy = cov(y as f64, y0, y1);
            for x in xa..xb {
                let c = cov(x as f64, x0, x1) * cy;
                s.mask[(y * w + x) as usize] = (c * 255.0).round() as u8;
            }
        }
        s
    }

    /// Овал, вписанный в прямоугольник a–b (сглаженный край).
    pub fn ellipse(w: u32, h: u32, a: (f64, f64), b: (f64, f64)) -> Sel {
        let (cx, cy) = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        let (rx, ry) = (((a.0 - b.0) / 2.0).abs().max(0.5), ((a.1 - b.1) / 2.0).abs().max(0.5));
        let mut s = Sel::empty(w, h);
        let (xa, xb) = ((cx - rx - 1.0).floor().max(0.0) as u32, ((cx + rx + 1.0).ceil() as u32).min(w));
        let (ya, yb) = ((cy - ry - 1.0).floor().max(0.0) as u32, ((cy + ry + 1.0).ceil() as u32).min(h));
        let rmin = rx.min(ry);
        for y in ya..yb {
            for x in xa..xb {
                let (dx, dy) = ((x as f64 + 0.5 - cx) / rx, (y as f64 + 0.5 - cy) / ry);
                let d = (dx * dx + dy * dy).sqrt();
                // расстояние до края в точках ≈ (1 − d)·r
                let c = ((1.0 - d) * rmin + 0.5).clamp(0.0, 1.0);
                s.mask[(y * w + x) as usize] = (c * 255.0).round() as u8;
            }
        }
        s
    }

    /// Лассо: многоугольник по точкам (чётно-нечётное правило, 4 подстроки на точку).
    pub fn polygon(w: u32, h: u32, pts: &[(f64, f64)]) -> Sel {
        let mut s = Sel::empty(w, h);
        if pts.len() < 3 {
            return s;
        }
        const SUB: usize = 4;
        let ymin = pts.iter().map(|p| p.1).fold(f64::MAX, f64::min).floor().max(0.0) as u32;
        let ymax = (pts.iter().map(|p| p.1).fold(f64::MIN, f64::max).ceil() as u32).min(h);
        let mut acc = vec![0f32; w as usize];
        let mut xs: Vec<f64> = vec![];
        for y in ymin..ymax {
            acc.iter_mut().for_each(|a| *a = 0.0);
            for k in 0..SUB {
                let sy = y as f64 + (k as f64 + 0.5) / SUB as f64;
                xs.clear();
                for i in 0..pts.len() {
                    let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                    if (a.1 <= sy && b.1 > sy) || (b.1 <= sy && a.1 > sy) {
                        xs.push(a.0 + (sy - a.1) / (b.1 - a.1) * (b.0 - a.0));
                    }
                }
                xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
                for pair in xs.chunks_exact(2) {
                    let (l, r) = (pair[0].max(0.0), pair[1].min(w as f64));
                    if r <= l {
                        continue;
                    }
                    let (li, ri) = (l.floor() as usize, (r.ceil() as usize).min(w as usize));
                    for x in li..ri {
                        let c = ((x as f64 + 1.0).min(r) - (x as f64).max(l)).clamp(0.0, 1.0);
                        acc[x] += c as f32 / SUB as f32;
                    }
                }
            }
            let row = (y * w) as usize;
            for x in 0..w as usize {
                s.mask[row + x] = (acc[x].min(1.0) * 255.0).round() as u8;
            }
        }
        s
    }

    /// Волшебная палочка: области, похожие по цвету на точку (x, y).
    /// tol — допуск 0–100; contiguous — только связная область.
    pub fn wand(img: &Img, x: u32, y: u32, tol: f64, contiguous: bool) -> Sel {
        let (w, h) = (img.w, img.h);
        let mut s = Sel::empty(w, h);
        if x >= w || y >= h {
            return s;
        }
        let o = ((y * w + x) * 4) as usize;
        let seed = [img.px[o] as i32, img.px[o + 1] as i32, img.px[o + 2] as i32];
        let lim = (tol.clamp(0.0, 100.0) * 2.55).round() as i32;
        let near = |i: usize| -> bool {
            let p = &img.px[i * 4..i * 4 + 3];
            (p[0] as i32 - seed[0]).abs().max((p[1] as i32 - seed[1]).abs()).max((p[2] as i32 - seed[2]).abs()) <= lim
        };
        if !contiguous {
            for i in 0..(w * h) as usize {
                if near(i) {
                    s.mask[i] = 255;
                }
            }
            return s;
        }
        // заливка построчно (стек отрезков)
        let mut stack = vec![(x, y)];
        while let Some((sx, sy)) = stack.pop() {
            let row = (sy * w) as usize;
            if s.mask[row + sx as usize] != 0 || !near(row + sx as usize) {
                continue;
            }
            let mut l = sx;
            while l > 0 && s.mask[row + l as usize - 1] == 0 && near(row + l as usize - 1) {
                l -= 1;
            }
            let mut r = sx;
            while r + 1 < w && s.mask[row + r as usize + 1] == 0 && near(row + r as usize + 1) {
                r += 1;
            }
            for xx in l..=r {
                s.mask[row + xx as usize] = 255;
            }
            for ny in [sy.wrapping_sub(1), sy + 1] {
                if ny >= h {
                    continue;
                }
                let nrow = (ny * w) as usize;
                let mut xx = l;
                while xx <= r {
                    let i = nrow + xx as usize;
                    if s.mask[i] == 0 && near(i) {
                        stack.push((xx, ny));
                        // пропустить весь подходящий отрезок — его зальёт одна точка
                        while xx <= r && s.mask[nrow + xx as usize] == 0 && near(nrow + xx as usize) {
                            xx += 1;
                        }
                    }
                    xx += 1;
                }
            }
        }
        s
    }

    pub fn combine(&mut self, o: &Sel, op: SelOp) {
        if self.w != o.w || self.h != o.h {
            return;
        }
        for (a, &b) in self.mask.iter_mut().zip(&o.mask) {
            *a = match op {
                SelOp::Replace => b,
                SelOp::Add => (*a).max(b),
                SelOp::Sub => (*a).min(255 - b),
                SelOp::Intersect => (*a).min(b),
            };
        }
    }

    pub fn invert(&mut self) {
        self.mask.iter_mut().for_each(|m| *m = 255 - *m);
    }

    pub fn is_empty(&self) -> bool {
        !self.mask.iter().any(|&m| m > 0)
    }

    /// Рамка вокруг выделенного: (x, y, w, h).
    pub fn bbox(&self) -> Option<(u32, u32, u32, u32)> {
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for y in 0..self.h {
            let row = &self.mask[(y * self.w) as usize..((y + 1) * self.w) as usize];
            if let Some(l) = row.iter().position(|&m| m > 127) {
                let r = row.iter().rposition(|&m| m > 127).unwrap();
                x0 = x0.min(l as u32);
                x1 = x1.max(r as u32 + 1);
                y0 = y0.min(y);
                y1 = y + 1;
            }
        }
        (x1 > x0 && y1 > y0).then(|| (x0, y0, x1 - x0, y1 - y0))
    }

    /// Смягчить край на r точек.
    pub fn feather(&mut self, r: usize) {
        if r == 0 {
            return;
        }
        let mut img = Img { w: self.w, h: self.h, px: self.mask.iter().flat_map(|&m| [m, m, m, 255]).collect() };
        img = box_blur(&img, r, 2);
        for (m, p) in self.mask.iter_mut().zip(img.px.chunks_exact(4)) {
            *m = p[0];
        }
    }

    /// Маска другого размера (для уменьшенного предпросмотра).
    pub fn scaled(&self, nw: u32, nh: u32) -> Sel {
        let mut s = Sel::empty(nw, nh);
        for y in 0..nh {
            let sy = ((y as u64 * self.h as u64) / nh.max(1) as u64) as u32;
            for x in 0..nw {
                let sx = ((x as u64 * self.w as u64) / nw.max(1) as u64) as u32;
                s.mask[(y * nw + x) as usize] = self.mask[(sy * self.w + sx) as usize];
            }
        }
        s
    }

    /// Подсветка для экрана: снаружи — затемнение, по краю — линия цвета accent (BGR).
    /// Картинка с готовой (умноженной) альфой для AlphaBlend.
    pub fn overlay(&self, nw: u32, nh: u32, accent: [u8; 3]) -> Img {
        let s = self.scaled(nw, nh);
        let mut px = vec![0u8; (nw * nh * 4) as usize];
        let at = |x: i64, y: i64| -> u8 {
            if x < 0 || y < 0 || x >= nw as i64 || y >= nh as i64 { 0 } else { s.mask[(y as u32 * nw + x as u32) as usize] }
        };
        for y in 0..nh as i64 {
            for x in 0..nw as i64 {
                let m = at(x, y);
                let o = ((y as u32 * nw + x as u32) * 4) as usize;
                let inside = m >= 128;
                let edge = inside && (at(x - 1, y) < 128 || at(x + 1, y) < 128 || at(x, y - 1) < 128 || at(x, y + 1) < 128);
                if edge {
                    // пунктир: через 4 точки — светлая, иначе цвет акцента
                    let light = ((x + y) / 4) % 2 == 0;
                    let c = if light { [255, 255, 255] } else { accent };
                    px[o] = c[0];
                    px[o + 1] = c[1];
                    px[o + 2] = c[2];
                    px[o + 3] = 255;
                } else {
                    let a = ((255 - m as u32) * 110 / 255) as u8;
                    px[o + 3] = a; // чёрный с альфой: цвет 0 — уже «умножен»
                }
            }
        }
        Img { w: nw, h: nh, px }
    }
}

/// Итог = исходник, где маска 0; обработанное, где 255; смесь между.
pub fn blend_mask(orig: &Img, proc_: &Img, sel: &Sel) -> Img {
    let mut o = orig.clone();
    if sel.w != orig.w || sel.h != orig.h || proc_.w != orig.w || proc_.h != orig.h {
        return proc_.clone();
    }
    for (i, &m) in sel.mask.iter().enumerate() {
        if m == 0 {
            continue;
        }
        let k = i * 4;
        if m == 255 {
            o.px[k..k + 3].copy_from_slice(&proc_.px[k..k + 3]);
        } else {
            let a = m as u32;
            for c in 0..3 {
                o.px[k + c] = ((orig.px[k + c] as u32 * (255 - a) + proc_.px[k + c] as u32 * a + 127) / 255) as u8;
            }
        }
    }
    o
}

// ---------------------------------------------------------- размытие ---

/// Сколько потоков брать для обработки картинки.
fn threads(px: usize) -> usize {
    if px < 400_000 {
        return 1; // мелочь — быстрее без потоков
    }
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 16)
}

/// Обработать буфер BGRA полосами строк параллельно: f(первая строка полосы, полоса).
pub fn par_rows(px: &mut [u8], w: usize, f: impl Fn(usize, &mut [u8]) + Sync) {
    let h = if w == 0 { 0 } else { px.len() / (w * 4) };
    let n = threads(w * h).min(h.max(1));
    if n <= 1 {
        f(0, px);
        return;
    }
    let band = h.div_ceil(n);
    std::thread::scope(|sc| {
        for (i, chunk) in px.chunks_mut(band * w * 4).enumerate() {
            let f = &f;
            sc.spawn(move || f(i * band, chunk));
        }
    });
}

/// Размытие: `passes` проходов скользящего среднего радиуса r (3 прохода ≈ по Гауссу).
/// Оба прохода идут по строкам (по памяти подряд) и делятся между ядрами.
pub fn box_blur(img: &Img, r: usize, passes: usize) -> Img {
    if r == 0 || img.w == 0 || img.h == 0 {
        return img.clone();
    }
    let (w, h) = (img.w as usize, img.h as usize);
    let mut a = img.px.clone();
    let mut b = vec![0u8; a.len()];
    for _ in 0..passes {
        let src = &a;
        par_rows(&mut b, w, |y0, band| blur_h(src, band, w, y0, r));
        let src = &b;
        par_rows(&mut a, w, |y0, band| blur_v(src, band, w, h, y0, r));
    }
    Img { w: img.w, h: img.h, px: a }
}

/// По горизонтали: каждая строка полосы отдельно, три канала сразу.
fn blur_h(src: &[u8], dst: &mut [u8], w: usize, y0: usize, r: usize) {
    let win = (2 * r + 1) as u32;
    let rows = dst.len() / (w * 4);
    let last = w as isize - 1;
    for ry in 0..rows {
        let row = &src[(y0 + ry) * w * 4..(y0 + ry + 1) * w * 4];
        let out = &mut dst[ry * w * 4..(ry + 1) * w * 4];
        let px = |i: isize, c: usize| row[i.clamp(0, last) as usize * 4 + c] as u32;
        let mut s = [0u32; 3];
        for i in -(r as isize)..=r as isize {
            for c in 0..3 {
                s[c] += px(i, c);
            }
        }
        for x in 0..w {
            let o = x * 4;
            for c in 0..3 {
                out[o + c] = ((s[c] + win / 2) / win) as u8;
            }
            out[o + 3] = 255;
            let (add, sub) = (x as isize + r as isize + 1, x as isize - r as isize);
            for c in 0..3 {
                s[c] = s[c] + px(add, c) - px(sub, c);
            }
        }
    }
}

/// По вертикали: суммы по столбцам на всю ширину, скользим вниз по строкам полосы.
fn blur_v(src: &[u8], dst: &mut [u8], w: usize, h: usize, y0: usize, r: usize) {
    let win = (2 * r + 1) as u32;
    let rows = dst.len() / (w * 4);
    let last = h as isize - 1;
    let line = |y: isize| {
        let y = y.clamp(0, last) as usize;
        &src[y * w * 4..(y + 1) * w * 4]
    };
    let mut s = vec![0u32; w * 3];
    for y in y0 as isize - r as isize..=y0 as isize + r as isize {
        let l = line(y);
        for x in 0..w {
            for c in 0..3 {
                s[x * 3 + c] += l[x * 4 + c] as u32;
            }
        }
    }
    for ry in 0..rows {
        let y = (y0 + ry) as isize;
        let out = &mut dst[ry * w * 4..(ry + 1) * w * 4];
        for x in 0..w {
            for c in 0..3 {
                out[x * 4 + c] = ((s[x * 3 + c] + win / 2) / win) as u8;
            }
            out[x * 4 + 3] = 255;
        }
        if ry + 1 < rows {
            let (add, sub) = (line(y + r as isize + 1), line(y - r as isize));
            for x in 0..w {
                for c in 0..3 {
                    let k = x * 3 + c;
                    s[k] = s[k] + add[x * 4 + c] as u32 - sub[x * 4 + c] as u32;
                }
            }
        }
    }
}

// ---------------------------------------------------------- настройки ---

/// Ползунки «Настройки». Все от −100 до 100 (резкость и размытие — 0…100). 0 — без изменений.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Adjust {
    pub bright: f64,
    pub exposure: f64,
    pub contrast: f64,
    pub highlights: f64,
    pub shadows: f64,
    pub saturation: f64,
    pub warmth: f64,
    pub tint: f64,
    pub sharpen: f64,
    pub blur: f64,
    pub vignette: f64,
}

/// Имена и пределы ползунков — для окна настроек.
pub const ADJ_FIELDS: [(&str, f64, f64); 11] = [
    ("Яркость", -100.0, 100.0),
    ("Экспозиция", -100.0, 100.0),
    ("Контраст", -100.0, 100.0),
    ("Светлые", -100.0, 100.0),
    ("Тени", -100.0, 100.0),
    ("Насыщенность", -100.0, 100.0),
    ("Теплота", -100.0, 100.0),
    ("Оттенок", -100.0, 100.0),
    ("Резкость", 0.0, 100.0),
    ("Размытие", 0.0, 100.0),
    ("Виньетка", -100.0, 100.0),
];

impl Adjust {
    pub fn get(&self, i: usize) -> f64 {
        [self.bright, self.exposure, self.contrast, self.highlights, self.shadows, self.saturation, self.warmth, self.tint, self.sharpen, self.blur, self.vignette][i]
    }
    pub fn set(&mut self, i: usize, v: f64) {
        let f = match i {
            0 => &mut self.bright,
            1 => &mut self.exposure,
            2 => &mut self.contrast,
            3 => &mut self.highlights,
            4 => &mut self.shadows,
            5 => &mut self.saturation,
            6 => &mut self.warmth,
            7 => &mut self.tint,
            8 => &mut self.sharpen,
            9 => &mut self.blur,
            _ => &mut self.vignette,
        };
        *f = v.clamp(ADJ_FIELDS[i].1, ADJ_FIELDS[i].2);
    }
    pub fn is_identity(&self) -> bool {
        *self == Adjust::default()
    }
}

fn long_side(img: &Img) -> f64 {
    img.w.max(img.h) as f64
}

fn smooth(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn apply_adjust(img: &Img, a: &Adjust) -> Img {
    if a.is_identity() {
        return img.clone();
    }
    let ls = long_side(img);
    let mut o = if a.blur > 0.0 {
        let r = (a.blur / 100.0 * ls * 0.012).round().max(1.0) as usize;
        box_blur(img, r, 3)
    } else {
        img.clone()
    };
    let exp = 2f64.powf(a.exposure / 100.0 * 1.5) as f32;
    let br = (a.bright / 100.0 * 70.0) as f32;
    let ct = (1.0 + a.contrast / 100.0).max(0.0) as f32;
    let sat = (1.0 + a.saturation / 100.0).max(0.0) as f32;
    let warm = (a.warmth / 100.0 * 30.0) as f32;
    let tint = (a.tint / 100.0 * 25.0) as f32;
    let (hl, sh) = (a.highlights / 100.0, a.shadows / 100.0);
    let tone = !(hl == 0.0 && sh == 0.0);
    // светлые/тени: таблица по яркости
    let mut lut = [0f32; 256];
    if tone {
        for (l, v) in lut.iter_mut().enumerate() {
            let x = l as f64 / 255.0;
            *v = ((hl * smooth(0.45, 1.0, x) + sh * (1.0 - smooth(0.0, 0.55, x))) * 0.35 * 255.0) as f32;
        }
    }
    // всё, кроме насыщенности, — линейно по каналу: v = x·A + d·B + C;
    // d («светлые/тени») зависит только от исходной яркости — таблица по ней
    let (ka, kb) = (exp * ct, ct);
    let base = (br - 128.0) * ct + 128.0;
    let cc = [base - warm, base - tint, base + warm]; // B, G, R
    let mut dl = [0f32; 256];
    if tone {
        for (l, v) in dl.iter_mut().enumerate() {
            let lx = ((l as f32 * exp + br).clamp(0.0, 255.0)) as usize;
            *v = lut[lx];
        }
    }
    let w = o.w as usize;
    par_rows(&mut o.px, w, |_, band| {
        for p in band.chunks_exact_mut(4) {
            let d = if tone { dl[((77 * p[2] as u32 + 150 * p[1] as u32 + 29 * p[0] as u32) >> 8) as usize] * kb } else { 0.0 };
            let mut b = p[0] as f32 * ka + d + cc[0];
            let mut g = p[1] as f32 * ka + d + cc[1];
            let mut r = p[2] as f32 * ka + d + cc[2];
            if sat != 1.0 {
                let y = 0.299 * r + 0.587 * g + 0.114 * b;
                b = y + (b - y) * sat;
                g = y + (g - y) * sat;
                r = y + (r - y) * sat;
            }
            p[0] = (b.clamp(0.0, 255.0) + 0.5) as u8;
            p[1] = (g.clamp(0.0, 255.0) + 0.5) as u8;
            p[2] = (r.clamp(0.0, 255.0) + 0.5) as u8;
        }
    });
    if a.sharpen > 0.0 {
        o = sharpen(&o, a.sharpen / 100.0 * 1.6, ((ls / 1000.0).round() as usize).max(1));
    }
    if a.vignette != 0.0 {
        vignette(&mut o, a.vignette / 100.0);
    }
    o
}

/// Нерезкое маскирование: x + k·(x − размытое).
pub fn sharpen(img: &Img, k: f64, r: usize) -> Img {
    let bl = box_blur(img, r, 2);
    let mut o = img.clone();
    let w = o.w as usize;
    // целочисленно: k в 1/256
    let ki = (k * 256.0).round() as i32;
    par_rows(&mut o.px, w, |y0, band| {
        let q = &bl.px[y0 * w * 4..y0 * w * 4 + band.len()];
        for (p, q) in band.chunks_exact_mut(4).zip(q.chunks_exact(4)) {
            for c in 0..3 {
                let x = p[c] as i32;
                p[c] = (x + ((ki * (x - q[c] as i32) + 128) >> 8)).clamp(0, 255) as u8;
            }
        }
    });
    o
}

/// v > 0 — темнее к краям, v < 0 — светлее.
fn vignette(img: &mut Img, v: f64) {
    let (w, h) = (img.w as usize, img.h as f32);
    let (cx, cy) = (w as f32 / 2.0, h / 2.0);
    let norm2 = (cx * cx + cy * cy).max(1.0);
    let dx2: Vec<f32> = (0..w).map(|x| (x as f32 + 0.5 - cx).powi(2)).collect();
    let (dark, amt) = (v > 0.0, v.abs() as f32 * 0.85);
    par_rows(&mut img.px, w, |y0, band| {
        for (ry, row) in band.chunks_exact_mut(w * 4).enumerate() {
            let dy2 = ((y0 + ry) as f32 + 0.5 - cy).powi(2);
            // в середине (d < 0.35) ничего не меняется — пропускаем
            if dy2 / norm2 < 0.35 * 0.35 && dx2[0] / norm2 + dy2 / norm2 < 0.35 * 0.35 {
                continue;
            }
            for (x, p) in row.chunks_exact_mut(4).enumerate() {
                let d2 = (dx2[x] + dy2) / norm2;
                if d2 <= 0.1225 {
                    continue;
                }
                let tt = ((d2.sqrt() - 0.35) / 0.65).clamp(0.0, 1.0);
                let t = tt * tt * (3.0 - 2.0 * tt) * amt;
                for c in 0..3 {
                    let q = p[c] as f32;
                    p[c] = (if dark { q * (1.0 - t) } else { q + (255.0 - q) * t } + 0.5) as u8;
                }
            }
        }
    });
}

// --------------------------------------------------------- эффекты ---

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fx {
    Gray,
    Sepia,
    Invert,
    /// Крупные клетки — «замазать» номер или лицо.
    Pixelate,
    Blur,
    Sharpen,
}

pub fn apply_fx(img: &Img, fx: Fx) -> Img {
    let mut o = img.clone();
    match fx {
        Fx::Gray => {
            let w = o.w as usize;
            par_rows(&mut o.px, w, |_, band| {
                for p in band.chunks_exact_mut(4) {
                    let y = ((77 * p[2] as u32 + 150 * p[1] as u32 + 29 * p[0] as u32 + 128) >> 8) as u8;
                    p[0] = y;
                    p[1] = y;
                    p[2] = y;
                }
            });
        }
        Fx::Sepia => {
            let w = o.w as usize;
            par_rows(&mut o.px, w, |_, band| {
                for p in band.chunks_exact_mut(4) {
                    let (b, g, r) = (p[0] as u32, p[1] as u32, p[2] as u32);
                    p[2] = ((101 * r + 197 * g + 48 * b) >> 8).min(255) as u8;
                    p[1] = ((89 * r + 176 * g + 43 * b) >> 8).min(255) as u8;
                    p[0] = ((70 * r + 137 * g + 34 * b) >> 8).min(255) as u8;
                }
            });
        }
        Fx::Invert => {
            for p in o.px.chunks_exact_mut(4) {
                p[0] = 255 - p[0];
                p[1] = 255 - p[1];
                p[2] = 255 - p[2];
            }
        }
        Fx::Pixelate => {
            let cell = ((long_side(img) / 60.0).round() as u32).max(2);
            let (w, h) = (img.w, img.h);
            let mut by = 0;
            while by < h {
                let mut bx = 0;
                while bx < w {
                    let (ex, ey) = ((bx + cell).min(w), (by + cell).min(h));
                    let mut s = [0u64; 3];
                    for y in by..ey {
                        for x in bx..ex {
                            let k = ((y * w + x) * 4) as usize;
                            for c in 0..3 {
                                s[c] += img.px[k + c] as u64;
                            }
                        }
                    }
                    let n = ((ex - bx) * (ey - by)) as u64;
                    for y in by..ey {
                        for x in bx..ex {
                            let k = ((y * w + x) * 4) as usize;
                            for c in 0..3 {
                                o.px[k + c] = ((s[c] + n / 2) / n) as u8;
                            }
                        }
                    }
                    bx += cell;
                }
                by += cell;
            }
        }
        Fx::Blur => {
            let r = ((long_side(img) * 0.006).round() as usize).max(1);
            o = box_blur(img, r, 3);
        }
        Fx::Sharpen => {
            o = sharpen(img, 0.8, ((long_side(img) / 1000.0).round() as usize).max(1));
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(s: &Sel) -> f64 {
        s.mask.iter().map(|&m| m as f64 / 255.0).sum()
    }

    #[test]
    fn selections() {
        let r = Sel::rect(100, 100, (10.0, 20.0), (60.0, 40.0));
        assert!((area(&r) - 1000.0).abs() < 1.0);
        assert_eq!(r.bbox(), Some((10, 20, 50, 20)));
        let e = Sel::ellipse(200, 200, (0.0, 0.0), (200.0, 200.0));
        assert!((area(&e) - std::f64::consts::PI * 100.0 * 100.0).abs() < 200.0);
        let t = Sel::polygon(100, 100, &[(0.0, 0.0), (100.0, 0.0), (0.0, 100.0)]);
        assert!((area(&t) - 5000.0).abs() < 60.0, "{}", area(&t));
        let mut c = r.clone();
        c.invert();
        assert!((area(&c) - 9000.0).abs() < 1.0);
        c.combine(&r, SelOp::Add);
        assert!((area(&c) - 10000.0).abs() < 1.0);
        c.combine(&r, SelOp::Sub);
        assert!((area(&c) - 9000.0).abs() < 1.0);
        assert!(Sel::empty(5, 5).is_empty());
    }

    #[test]
    fn magic_wand() {
        // слева красное, справа синее, посередине белая полоса
        let mut img = Img::filled(30, 10, [0, 0, 255]);
        for y in 0..10 {
            for x in 0..30 {
                let k = ((y * 30 + x) * 4) as usize;
                if x >= 20 {
                    img.px[k..k + 3].copy_from_slice(&[255, 0, 0]);
                } else if x >= 10 {
                    img.px[k..k + 3].copy_from_slice(&[255, 255, 255]);
                }
            }
        }
        let s = Sel::wand(&img, 2, 2, 10.0, true);
        assert_eq!(area(&s) as u32, 100);
        assert_eq!(s.bbox(), Some((0, 0, 10, 10)));
        let all_blue = Sel::wand(&img, 25, 5, 10.0, false);
        assert_eq!(area(&all_blue) as u32, 100);
    }

    #[test]
    fn adjustments_and_fx() {
        let mut img = Img::filled(40, 30, [30, 120, 200]);
        img.px[0] = 0;
        assert_eq!(apply_adjust(&img, &Adjust::default()).px, img.px);
        let inv = apply_fx(&apply_fx(&img, Fx::Invert), Fx::Invert);
        assert_eq!(inv.px, img.px);
        let flat = Img::filled(50, 50, [10, 20, 30]);
        assert_eq!(box_blur(&flat, 3, 3).px, flat.px, "размытие однотонного — то же");
        let mut a = Adjust::default();
        a.set(0, 50.0);
        let br = apply_adjust(&flat, &a);
        assert!(br.px[0] > 10 && br.px[2] > 30);
        a = Adjust::default();
        a.set(6, 100.0);
        let warm = apply_adjust(&flat, &a);
        assert!(warm.px[2] > 30 && warm.px[0] < 10 || warm.px[0] == 0);
        let g = apply_fx(&img, Fx::Gray);
        assert!(g.px.chunks_exact(4).all(|p| p[0] == p[1] && p[1] == p[2]));
        // смешивание по маске: левая половина — обработанная
        let sel = Sel::rect(40, 30, (0.0, 0.0), (20.0, 30.0));
        let mix = blend_mask(&img, &g, &sel);
        assert_eq!(&mix.px[4..8], &g.px[4..8]);
        let k = (5 * 40 + 30) * 4;
        assert_eq!(&mix.px[k..k + 4], &img.px[k..k + 4]);
        let ov = sel.overlay(40, 30, [200, 100, 0]);
        assert_eq!(ov.px[k + 3], 110, "снаружи — затемнение");
    }

    fn noise(w: u32, h: u32) -> Img {
        let mut img = Img::filled(w, h, [0, 0, 0]);
        let mut r = 12345u32;
        for p in img.px.chunks_exact_mut(4) {
            for c in 0..3 {
                r ^= r << 13;
                r ^= r >> 17;
                r ^= r << 5;
                p[c] = (r >> 7) as u8;
            }
        }
        img
    }

    #[test]
    fn fast_blur_matches_naive() {
        // > 400 000 точек — работает в несколько потоков
        let img = noise(700, 610);
        let (w, h, r) = (700isize, 610isize, 3isize);
        let fast = box_blur(&img, r as usize, 1);
        let at = |x: isize, y: isize, c: usize| img.px[((y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) * 4) as usize + c] as u32;
        let win = (2 * r + 1) as u32;
        for &(x, y) in &[(0isize, 0isize), (5, 300), (699, 609), (350, 0), (123, 457), (698, 1)] {
            for c in 0..3 {
                // сначала по горизонтали (с округлением), потом по вертикали
                let hrow = |yy: isize| ((-r..=r).map(|d| at(x + d, yy, c)).sum::<u32>() + win / 2) / win;
                let v = ((-r..=r).map(|d| hrow((y + d).clamp(0, h - 1))).sum::<u32>() + win / 2) / win;
                let got = fast.px[((y * w + x) * 4) as usize + c] as u32;
                assert_eq!(got, v, "({x},{y}) канал {c}");
            }
        }
    }

    #[test]
    fn fast_adjust_matches_formula() {
        let img = noise(64, 48);
        let a = Adjust { bright: 20.0, exposure: 10.0, contrast: 30.0, saturation: -40.0, warmth: 15.0, tint: -10.0, ..Default::default() };
        let out = apply_adjust(&img, &a);
        let exp = 2f32.powf(0.1 * 1.5);
        let (br, ct, sat, warm, tint) = (14.0f32, 1.3f32, 0.6f32, 4.5f32, -2.5f32);
        for (p, q) in img.px.chunks_exact(4).zip(out.px.chunks_exact(4)) {
            let f = |x: u8| ((x as f32 * exp + br) - 128.0) * ct + 128.0;
            let (mut b, mut g, mut r) = (f(p[0]) - warm, f(p[1]) - tint, f(p[2]) + warm);
            let y = 0.299 * r + 0.587 * g + 0.114 * b;
            b = y + (b - y) * sat;
            g = y + (g - y) * sat;
            r = y + (r - y) * sat;
            for (c, v) in [b, g, r].into_iter().enumerate() {
                assert!((v.clamp(0.0, 255.0).round() as i32 - q[c] as i32).abs() <= 1, "{v} vs {}", q[c]);
            }
        }
    }
}
