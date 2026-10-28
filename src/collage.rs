// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Коллаж: несколько фото → одно изображение. Раскладки: сетка, крупное фото
//! слева/сверху, в ряд, в столбик. Формат холста, отступы, фон, «заполнить/вписать».

use crate::codec;
use crate::gui::{self, Ev, Kind, Mouse, Panel, Ui, W};
use crate::image_buf::Img;
use crate::util::{file_name, rgb};
use crate::{files, settings};
use std::path::PathBuf;
use std::rc::Rc;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{HDC, PS_SOLID};

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

/// Холсты: название, ширина, высота (0×0 — по фото). A4 и 10×15 — для печати 300 dpi.
pub const CANVAS: [(&str, u32, u32); 7] = [
    ("Авто (по фото)", 0, 0),
    ("A4 книжный", 2480, 3508),
    ("A4 альбомный", 3508, 2480),
    ("10×15 альбомный", 1800, 1200),
    ("10×15 книжный", 1200, 1800),
    ("Квадрат", 3000, 3000),
    ("16:9", 3840, 2160),
];

/// Фон (RGB).
pub const BG: [(&str, [u8; 3]); 5] = [
    ("Белый", [255, 255, 255]),
    ("Светло-серый", [232, 232, 232]),
    ("Кремовый", [245, 238, 224]),
    ("Тёмно-серый", [48, 48, 48]),
    ("Чёрный", [0, 0, 0]),
];

pub type Cell = (f64, f64, f64, f64);

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Cfg {
    pub layout: Layout,
    pub canvas: usize,
    pub gap: i32, // 0..=10 → доля короткой стороны (gap/200)
    pub fill: bool,
    pub bg: usize,
}

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

/// Размер холста в точках.
pub fn canvas_size(cfg: &Cfg, aspects: &[f64]) -> (u32, u32) {
    let (_, w, h) = CANVAS[cfg.canvas.min(CANVAS.len() - 1)];
    if w > 0 {
        return (w, h);
    }
    let base = 2400.0;
    let (w, h) = match cfg.layout {
        Layout::Row if !aspects.is_empty() => (aspects.iter().sum::<f64>() * base, base),
        Layout::Column if !aspects.is_empty() => (base, aspects.iter().map(|a| 1.0 / a).sum::<f64>() * base),
        _ if aspects.len() == 1 => (base * aspects[0], base),
        _ => (3200.0, 2400.0),
    };
    let k = (12000.0 / w.max(h)).min(1.0);
    (((w * k).round() as u32).max(16), ((h * k).round() as u32).max(16))
}

pub fn gap_px(cfg: &Cfg, w: u32, h: u32) -> f64 {
    w.min(h) as f64 * cfg.gap as f64 / 200.0
}

/// Положить фото в ячейку (заполнить с обрезкой или вписать).
fn place(canvas: &mut Img, img: &Img, c: Cell, fill: bool) {
    let (x0, y0) = (c.0.round() as i32, c.1.round() as i32);
    let (x1, y1) = ((c.0 + c.2).round() as i32, (c.1 + c.3).round() as i32);
    let (cw, ch) = ((x1 - x0).max(1) as u32, (y1 - y0).max(1) as u32);
    if fill {
        let part = img.crop_to_aspect(cw as f64 / ch as f64);
        if let Ok(r) = codec::resize(&part, cw, ch) {
            canvas.draw(&r, x0, y0);
        }
    } else {
        let s = (cw as f64 / img.w as f64).min(ch as f64 / img.h as f64);
        let (w, h) = (((img.w as f64 * s).round() as u32).max(1), ((img.h as f64 * s).round() as u32).max(1));
        if let Ok(r) = codec::resize(img, w, h) {
            canvas.draw(&r, x0 + (cw as i32 - w as i32) / 2, y0 + (ch as i32 - h as i32) / 2);
        }
    }
}

/// Собрать коллаж w×h.
pub fn compose(imgs: &[&Img], cfg: &Cfg, w: u32, h: u32) -> (Img, Vec<Cell>) {
    let [r, g, b] = BG[cfg.bg.min(BG.len() - 1)].1;
    let mut canvas = Img::filled(w, h, [b, g, r]);
    let aspects: Vec<f64> = imgs.iter().map(|i| i.w as f64 / i.h as f64).collect();
    let cs = cells(cfg.layout, &aspects, w as f64, h as f64, gap_px(cfg, w, h));
    for (img, c) in imgs.iter().zip(cs.iter()) {
        place(&mut canvas, img, *c, cfg.fill);
    }
    (canvas, cs)
}

// ------------------------------------------------------------------ окно ---
pub struct Item {
    pub label: String,
    pub path: Option<PathBuf>,
    pub img: Option<Rc<Img>>,
}

struct Entry {
    item: Item,
    thumb: Rc<Img>,
}

const ID_LIST: u32 = 1;
const ID_ADD: u32 = 2;
const ID_UP: u32 = 3;
const ID_DOWN: u32 = 4;
const ID_DEL: u32 = 5;
const ID_LAYOUT0: u32 = 10;
const ID_CANVAS_PREV: u32 = 20;
const ID_CANVAS_NEXT: u32 = 21;
const ID_GAP_DEC: u32 = 22;
const ID_GAP_INC: u32 = 23;
const ID_FILL: u32 = 24;
const ID_FIT: u32 = 25;
const ID_BG0: u32 = 30;
const ID_OK: u32 = 40;
const ID_SAVE: u32 = 41;
const ID_CANCEL: u32 = 42;

struct Collage {
    entries: Vec<Entry>,
    sel: Option<usize>,
    scroll: usize,
    cfg: Cfg,
    preview: Option<(Cfg, Vec<usize>, i32, i32, Img, Vec<Cell>)>, // ключ кеша + картинка + ячейки
    prect: RECT,
    hover_cell: Option<usize>,
    result: Option<Img>,
}

fn load_entry(item: Item) -> Result<Entry, String> {
    let thumb = match (&item.img, &item.path) {
        (Some(i), _) => codec::thumb(i, 900),
        (None, Some(p)) => codec::thumb(&codec::load(p).map_err(|e| format!("{}: {}", file_name(p), e.message()))?, 900),
        _ => return Err("пустой элемент".into()),
    };
    Ok(Entry { item, thumb: Rc::new(thumb) })
}

impl Collage {
    fn add_items(&mut self, items: Vec<Item>, hwnd: HWND) {
        let mut errs = vec![];
        let at = self.sel.map(|s| s + 1).unwrap_or(self.entries.len());
        let mut k = at;
        for it in items {
            match load_entry(it) {
                Ok(e) => {
                    self.entries.insert(k, e);
                    k += 1;
                }
                Err(e) => errs.push(e),
            }
        }
        if k > at {
            self.sel = Some(k - 1);
        }
        if !errs.is_empty() {
            crate::ui::message(Some(hwnd), &format!("Не удалось открыть:\n\n{}", errs.join("\n")), true);
        }
    }

    fn aspects(&self) -> Vec<f64> {
        self.entries.iter().map(|e| e.thumb.w as f64 / e.thumb.h as f64).collect()
    }

    /// Итоговый коллаж в полном разрешении.
    fn build(&self) -> Result<Img, String> {
        let mut full: Vec<Rc<Img>> = vec![];
        for e in &self.entries {
            full.push(match (&e.item.img, &e.item.path) {
                (Some(i), _) => i.clone(),
                (None, Some(p)) => Rc::new(codec::load(p).map_err(|er| format!("{}: {}", file_name(p), er.message()))?),
                _ => e.thumb.clone(),
            });
        }
        let (w, h) = canvas_size(&self.cfg, &self.aspects());
        let refs: Vec<&Img> = full.iter().map(|r| &**r).collect();
        Ok(compose(&refs, &self.cfg, w, h).0)
    }

    fn save_settings(&self) {
        settings::set_i("collage_layout", LAYOUTS.iter().position(|l| l.0 == self.cfg.layout).unwrap_or(0) as i32);
        settings::set_i("collage_canvas", self.cfg.canvas as i32);
        settings::set_i("collage_gap", self.cfg.gap);
        settings::set_i("collage_fill", self.cfg.fill as i32);
        settings::set_i("collage_bg", self.cfg.bg as i32);
    }
}

impl Panel for Collage {
    fn title(&self) -> String {
        "Коллаж".into()
    }

    fn init(&mut self, hwnd: HWND) {
        unsafe { windows::Win32::UI::Shell::DragAcceptFiles(hwnd, true) };
    }

    fn min_size(&self) -> (i32, i32) {
        (820, 640)
    }

    fn layout(&mut self, ui: &Ui, w: i32, h: i32) -> Vec<W> {
        let m = ui.s(16);
        let lw = ui.s(330);
        let mut v = vec![];
        let mut y = m;
        v.push(W::new(Kind::Group, 0, gui::r(m, y, lw, ui.s(22))).t(format!("Фото: {}", self.entries.len())));
        y += ui.s(26);
        let mut list = W::new(Kind::List, ID_LIST, gui::r(m, y, lw, ui.s(170))).t("Добавьте фото");
        list.items = self.entries.iter().map(|e| (e.item.label.clone(), format!("{} × {}", e.thumb.w, e.thumb.h))).collect();
        // размеры показываем у оригинала, если он в памяти
        for (i, e) in self.entries.iter().enumerate() {
            if let Some(img) = &e.item.img {
                list.items[i].1 = format!("{} × {}", img.w, img.h);
            } else {
                list.items[i].1.clear();
            }
        }
        list.sel = self.sel;
        gui::list_fix_scroll(ui, &list, &mut self.scroll);
        list.scroll = self.scroll;
        v.push(list);
        y += ui.s(178);
        let has = self.sel.is_some();
        let n = self.entries.len();
        v.push(W::new(Kind::Button, ID_ADD, gui::r(m, y, ui.s(150), ui.s(34))).i(gui::IC_ADD).t("Добавить…"));
        let tb = ui.s(34);
        let mut x = m + lw - 3 * tb - 2 * ui.s(6);
        for (id, ic, tip, en) in [
            (ID_UP, gui::IC_UP, "Выше", has && self.sel != Some(0)),
            (ID_DOWN, gui::IC_DOWN, "Ниже", has && self.sel.map(|s| s + 1 < n).unwrap_or(false)),
            (ID_DEL, gui::IC_REMOVE, "Убрать", has),
        ] {
            v.push(W::new(Kind::Button, id, gui::r(x, y, tb, tb)).i(ic).tip(tip).en(en));
            x += tb + ui.s(6);
        }
        y += ui.s(50);

        v.push(W::new(Kind::Group, 0, gui::r(m, y, lw, ui.s(22))).t("Раскладка"));
        y += ui.s(26);
        let cur = LAYOUTS.iter().position(|l| l.0 == self.cfg.layout).unwrap_or(0);
        let names: Vec<&str> = LAYOUTS.iter().map(|l| l.1).collect();
        let mut s1 = gui::segments(ui, m, y, lw, ID_LAYOUT0, &names[..3], cur);
        let mut s2 = gui::segments(ui, m, y + ui.s(38), lw * 2 / 3 - ui.s(2), ID_LAYOUT0 + 3, &names[3..], cur.wrapping_sub(3));
        v.append(&mut s1);
        v.append(&mut s2);
        y += ui.s(84);

        v.push(W::new(Kind::Group, 0, gui::r(m, y, lw, ui.s(22))).t("Формат холста"));
        y += ui.s(26);
        let (cname, _, _) = CANVAS[self.cfg.canvas];
        let (cw, chh) = canvas_size(&self.cfg, &self.aspects());
        v.extend(gui::stepper(ui, m, y, lw, cname.to_string(), ID_CANVAS_PREV, ID_CANVAS_NEXT, "Колесо мыши — листать форматы"));
        // вместо «−/+» — стрелки
        let k = v.len();
        v[k - 3].text = "‹".into();
        v[k - 1].text = "›".into();
        y += ui.s(46);

        let half = (lw - ui.s(12)) / 2;
        v.push(W::new(Kind::Group, 0, gui::r(m, y, half, ui.s(22))).t("Отступы"));
        v.push(W::new(Kind::Group, 0, gui::r(m + half + ui.s(12), y, half, ui.s(22))).t("Фото в ячейке"));
        y += ui.s(26);
        let gtxt = if self.cfg.gap == 0 { "нет".to_string() } else { format!("{}", self.cfg.gap) };
        v.extend(gui::stepper(ui, m, y, half, gtxt, ID_GAP_DEC, ID_GAP_INC, "Ширина отступов между фото"));
        v.extend(gui::segments(ui, m + half + ui.s(12), y, half, ID_FILL, &["Заполнить", "Вписать"], if self.cfg.fill { 0 } else { 1 }));
        y += ui.s(46);

        v.push(W::new(Kind::Group, 0, gui::r(m, y, lw, ui.s(22))).t("Фон"));
        y += ui.s(26);
        let sw = ui.s(40);
        for (i, (name, c)) in BG.iter().enumerate() {
            v.push(W::new(Kind::Swatch, ID_BG0 + i as u32, gui::r(m + i as i32 * (sw + ui.s(8)), y, sw, sw)).color(rgb(c[0], c[1], c[2])).on(i == self.cfg.bg).tip(*name));
        }

        // предпросмотр
        let px = m + lw + ui.s(20);
        self.prect = gui::r(px, m, w - px - m, h - m - ui.s(64) - m);
        // кнопки
        let by = h - m - ui.s(36);
        v.push(W::new(Kind::Label, 0, gui::r(px, by, ui.s(300), ui.s(36))).t(format!("Итог: {cw} × {chh} точек")).color(ui.t.dim));
        let ok = !self.entries.is_empty();
        let mut x = w - m;
        for (id, t, primary, en, bw) in [
            (ID_OK, "Готово", true, ok, 120),
            (ID_SAVE, "Сохранить как…", false, ok, 160),
            (ID_CANCEL, "Отмена", false, true, 100),
        ] {
            let bw = ui.s(bw);
            x -= bw;
            v.push(W::new(Kind::Button, id, gui::r(x, by, bw, ui.s(36))).t(t).on(primary).en(en));
            x -= ui.s(8);
        }
        v
    }

    fn paint(&mut self, ui: &Ui, hdc: HDC, w: i32, h: i32) {
        let t = &ui.t;
        gui::fill(hdc, &gui::r(0, 0, w, h), t.bg);
        let pr = self.prect;
        gui::round(hdc, &pr, ui.s(10), Some(t.canvas), None);
        if self.entries.is_empty() {
            gui::text(hdc, "Добавьте фото слева\nили перетащите их в окно", &RECT { top: (pr.top + pr.bottom) / 2 - ui.s(20), ..pr }, ui.f.text, t.dim, gui::WRAP);
            return;
        }
        let aspects = self.aspects();
        let (cw, ch) = canvas_size(&self.cfg, &aspects);
        let pad = ui.s(20);
        let (aw, ah) = ((pr.right - pr.left - 2 * pad).max(10) as f64, (pr.bottom - pr.top - 2 * pad).max(10) as f64);
        let k = (aw / cw as f64).min(ah / ch as f64);
        let (pw, ph) = (((cw as f64 * k).round() as i32).max(1), ((ch as f64 * k).round() as i32).max(1));
        let key: Vec<usize> = self.entries.iter().map(|e| Rc::as_ptr(&e.thumb) as usize).collect();
        let fresh = matches!(&self.preview, Some((c, kk, w0, h0, _, _)) if *c == self.cfg && *kk == key && *w0 == pw && *h0 == ph);
        if !fresh {
            // предпросмотр собираем из миниатюр в масштабе экрана; отступы — как в итоге
            let refs: Vec<&Img> = self.entries.iter().map(|e| &*e.thumb).collect();
            let mut c2 = self.cfg;
            c2.canvas = self.cfg.canvas;
            let (img, mut cells) = compose_scaled(&refs, &c2, pw as u32, ph as u32, cw, ch);
            for c in cells.iter_mut() {
                *c = (c.0, c.1, c.2, c.3);
            }
            self.preview = Some((self.cfg, key, pw, ph, img, cells));
        }
        let (_, _, _, _, img, cells) = self.preview.as_ref().unwrap();
        let x0 = (pr.left + pr.right - pw) / 2;
        let y0 = (pr.top + pr.bottom - ph) / 2;
        let shadow = gui::r(x0 + ui.s(3), y0 + ui.s(3), pw, ph);
        gui::fill(hdc, &shadow, gui::blend(t.canvas, rgb(0, 0, 0), 0.7));
        gui::blit(hdc, img, &gui::r(x0, y0, pw, ph), false);
        let cell_rc = |c: &Cell| RECT {
            left: x0 + c.0.round() as i32,
            top: y0 + c.1.round() as i32,
            right: x0 + (c.0 + c.2).round() as i32,
            bottom: y0 + (c.1 + c.3).round() as i32,
        };
        if let Some(s) = self.sel.and_then(|s| cells.get(s)) {
            let rc = cell_rc(s);
            for d in 0..ui.s(3) {
                gui::frame(hdc, &RECT { left: rc.left + d, top: rc.top + d, right: rc.right - d, bottom: rc.bottom - d }, t.accent, PS_SOLID);
            }
        }
        if let Some(hc) = self.hover_cell.filter(|&c| Some(c) != self.sel).and_then(|c| cells.get(c)) {
            gui::frame(hdc, &cell_rc(hc), rgb(255, 255, 255), PS_SOLID);
        }
    }

    fn mouse(&mut self, _hwnd: HWND, m: Mouse, x: i32, y: i32) -> bool {
        let Some((_, _, pw, ph, _, cells)) = &self.preview else { return false };
        let pr = self.prect;
        let x0 = (pr.left + pr.right - pw) / 2;
        let y0 = (pr.top + pr.bottom - ph) / 2;
        let (fx, fy) = ((x - x0) as f64, (y - y0) as f64);
        let hit = cells.iter().position(|c| fx >= c.0 && fx < c.0 + c.2 && fy >= c.1 && fy < c.1 + c.3);
        match m {
            Mouse::Move => {
                let ch = hit != self.hover_cell;
                self.hover_cell = hit;
                ch
            }
            Mouse::Down => {
                if hit.is_some() {
                    self.sel = hit;
                }
                true
            }
            _ => false,
        }
    }

    fn event(&mut self, hwnd: HWND, ev: Ev) -> bool {
        let n = self.entries.len();
        match ev {
            Ev::Item(ID_LIST, k) | Ev::Dbl(ID_LIST, k) => self.sel = Some(k),
            Ev::Scroll(ID_LIST, d) => self.scroll = (self.scroll as i32 + d).max(0) as usize,
            Ev::Click(id) => match id {
                ID_ADD => {
                    let p = files::open_dialog(hwnd, files::Pick::Images, true);
                    let items = p.into_iter().map(|p| Item { label: file_name(&p), path: Some(p), img: None }).collect();
                    self.add_items(items, hwnd);
                }
                ID_UP => {
                    if let Some(s) = self.sel.filter(|&s| s > 0) {
                        self.entries.swap(s, s - 1);
                        self.sel = Some(s - 1);
                    }
                }
                ID_DOWN => {
                    if let Some(s) = self.sel.filter(|&s| s + 1 < n) {
                        self.entries.swap(s, s + 1);
                        self.sel = Some(s + 1);
                    }
                }
                ID_DEL => {
                    if let Some(s) = self.sel {
                        self.entries.remove(s);
                        self.sel = if self.entries.is_empty() { None } else { Some(s.min(self.entries.len() - 1)) };
                    }
                }
                i if (ID_LAYOUT0..ID_LAYOUT0 + LAYOUTS.len() as u32).contains(&i) => self.cfg.layout = LAYOUTS[(i - ID_LAYOUT0) as usize].0,
                ID_CANVAS_PREV => self.cfg.canvas = (self.cfg.canvas + CANVAS.len() - 1) % CANVAS.len(),
                ID_CANVAS_NEXT => self.cfg.canvas = (self.cfg.canvas + 1) % CANVAS.len(),
                ID_GAP_DEC => self.cfg.gap = (self.cfg.gap - 1).max(0),
                ID_GAP_INC => self.cfg.gap = (self.cfg.gap + 1).min(10),
                ID_FILL => self.cfg.fill = true,
                ID_FIT => self.cfg.fill = false,
                i if (ID_BG0..ID_BG0 + BG.len() as u32).contains(&i) => self.cfg.bg = (i - ID_BG0) as usize,
                ID_OK | ID_SAVE => {
                    self.save_settings();
                    let img = match self.build() {
                        Ok(i) => i,
                        Err(e) => {
                            crate::ui::message(Some(hwnd), &format!("Не удалось собрать коллаж:\n{e}"), true);
                            return true;
                        }
                    };
                    if id == ID_SAVE {
                        let dir = self.entries.iter().find_map(|e| e.item.path.as_ref().and_then(|p| p.parent()).map(|d| d.to_path_buf()));
                        let Some(p) = files::save_dialog(hwnd, "Коллаж.jpg", dir.as_deref(), false, false) else { return true };
                        let res = if crate::util::is_pdf(&p) {
                            crate::doc::save_pdf(&[crate::doc::Page::mem("Коллаж", img.clone())], &p)
                        } else {
                            codec::save(&img, &p).map_err(|e| e.message().to_string())
                        };
                        if let Err(e) = res {
                            crate::ui::message(Some(hwnd), &format!("Не удалось сохранить:\n{e}"), true);
                            return true;
                        }
                    }
                    self.result = Some(img);
                    return false;
                }
                ID_CANCEL => return false,
                _ => {}
            },
            _ => {}
        }
        true
    }

    fn key(&mut self, hwnd: HWND, vk: u16, _ctrl: bool, _shift: bool) -> Option<bool> {
        match vk {
            0x2E => Some(self.event(hwnd, Ev::Click(ID_DEL))),
            0x0D if !self.entries.is_empty() => Some(self.event(hwnd, Ev::Click(ID_OK))),
            _ => None,
        }
    }

    fn drop_files(&mut self, hwnd: HWND, files: Vec<PathBuf>) {
        let items = files
            .into_iter()
            .filter(|p| files::is_image(p))
            .map(|p| Item { label: file_name(&p), path: Some(p), img: None })
            .collect();
        self.add_items(items, hwnd);
    }
}

/// Предпросмотр: тот же коллаж в уменьшенном масштабе (отступы считаются от полного размера).
fn compose_scaled(imgs: &[&Img], cfg: &Cfg, pw: u32, ph: u32, fw: u32, fh: u32) -> (Img, Vec<Cell>) {
    let [r, g, b] = BG[cfg.bg.min(BG.len() - 1)].1;
    let mut canvas = Img::filled(pw, ph, [b, g, r]);
    let k = pw as f64 / fw as f64;
    let aspects: Vec<f64> = imgs.iter().map(|i| i.w as f64 / i.h as f64).collect();
    let cs: Vec<Cell> = cells(cfg.layout, &aspects, fw as f64, fh as f64, gap_px(cfg, fw, fh))
        .into_iter()
        .map(|c| (c.0 * k, c.1 * k, c.2 * k, c.3 * k))
        .collect();
    let _ = ph;
    for (img, c) in imgs.iter().zip(cs.iter()) {
        place(&mut canvas, img, *c, cfg.fill);
    }
    (canvas, cs)
}

pub fn run(owner: HWND, items: Vec<Item>) -> Option<Img> {
    let li = settings::get_i("collage_layout", 0).clamp(0, LAYOUTS.len() as i32 - 1) as usize;
    let mut c = Collage {
        entries: vec![],
        sel: None,
        scroll: 0,
        cfg: Cfg {
            layout: LAYOUTS[li].0,
            canvas: settings::get_i("collage_canvas", 0).clamp(0, CANVAS.len() as i32 - 1) as usize,
            gap: settings::get_i("collage_gap", 2).clamp(0, 10),
            fill: settings::get_i("collage_fill", 1) == 1,
            bg: settings::get_i("collage_bg", 0).clamp(0, BG.len() as i32 - 1) as usize,
        },
        preview: None,
        prect: RECT::default(),
        hover_cell: None,
        result: None,
    };
    c.add_items(items, owner);
    c.sel = if c.entries.is_empty() { None } else { Some(0) };
    gui::run_modal(owner, c, (1000, 700)).result
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
    fn row_keeps_aspect_on_auto() {
        let asp = [1.5, 0.75, 1.0];
        let cfg = Cfg { layout: Layout::Row, canvas: 0, gap: 0, fill: true, bg: 0 };
        let (w, h) = canvas_size(&cfg, &asp);
        let cs = cells(Layout::Row, &asp, w as f64, h as f64, 0.0);
        for (c, a) in cs.iter().zip(asp) {
            assert!((c.2 / c.3 - a).abs() < 0.01, "{c:?} {a}");
        }
    }
}
