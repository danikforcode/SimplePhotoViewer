// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Своё окно печати: предпросмотр листа, принтер, бумага, ориентация,
//! «вписать/заполнить», несколько фото на листе, поля, копии, правки
//! только для печати (поворот, отражение, яркость, контраст, Ч/Б)
//! и «Сохранить в PDF» ровно так, как на предпросмотре.

use crate::doc::{self, Page};
use crate::gui::{self, Ev, Kind, Mouse, Panel, Ui, W};
use crate::image_buf::Img;
use crate::pdf_write::{self, OutPage, Placed, MM};
use crate::util::{file_name, from_wide, rgb, wide};
use crate::{codec, files, settings};
use std::collections::HashMap;
use std::rc::Rc;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Printing::*;
use windows::Win32::Storage::Xps::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const NUPS: [usize; 5] = [1, 2, 4, 6, 9];
/// Бумага: название, код DMPAPER, размер в мм (книжная).
const PAPERS: [(&str, i16, (f64, f64)); 5] = [
    ("Принтер", 0, (210.0, 297.0)),
    ("A4", 9, (210.0, 297.0)),
    ("A5", 11, (148.0, 210.0)),
    ("A3", 8, (297.0, 420.0)),
    ("Letter", 1, (215.9, 279.4)),
];
const GAP_MM: f64 = 3.0;

const ID_PRINTER: u32 = 1;
const ID_PROPS: u32 = 2;
const ID_PAPER0: u32 = 10;
const ID_ORIENT0: u32 = 20;
const ID_NUP0: u32 = 30;
const ID_FIT: u32 = 40;
const ID_FILL: u32 = 41;
const ID_AUTOROT: u32 = 42;
const ID_M_DEC: u32 = 50;
const ID_M_INC: u32 = 51;
const ID_C_DEC: u32 = 52;
const ID_C_INC: u32 = 53;
const ID_ALL: u32 = 60;
const ID_CUR: u32 = 61;
const ID_E_ROTL: u32 = 70;
const ID_E_ROTR: u32 = 71;
const ID_E_FLIP: u32 = 72;
const ID_E_BDN: u32 = 73;
const ID_E_BUP: u32 = 74;
const ID_E_CDN: u32 = 75;
const ID_E_CUP: u32 = 76;
const ID_E_GRAY: u32 = 77;
const ID_E_RESET: u32 = 78;
const ID_S_PREV: u32 = 80;
const ID_S_NEXT: u32 = 81;
const ID_PDF: u32 = 90;
const ID_PRINT: u32 = 91;
const ID_CANCEL: u32 = 92;
const ID_O_ROTL: u32 = 100;
const ID_O_ROTR: u32 = 101;
const ID_O_SMALL: u32 = 102;
const ID_O_BIG: u32 = 103;
const ID_O_CENTER: u32 = 104;
const ID_O_RESET: u32 = 105;
const ID_O_RESET_ALL: u32 = 106;
const MENU_BASE: u32 = 1000;

#[derive(Clone, Copy, PartialEq, Default)]
struct Edits {
    rot: u8,
    flip: bool,
    bright: i32,
    contrast: i32,
    gray: bool,
}

impl Edits {
    fn apply(&self, img: &Img) -> Img {
        let mut im = doc::rot_q(img.clone(), self.rot);
        if self.flip {
            im = im.flip_h();
        }
        if self.bright != 0 || self.contrast != 0 || self.gray {
            im = im.adjust(1.0 + 0.1 * self.bright as f32, 1.0 + 0.1 * self.contrast as f32, if self.gray { 0.0 } else { 1.0 });
        }
        im
    }
}

/// Геометрия листа в мм: полный размер, смещение и размер области печати, dpi.
#[derive(Clone, Copy)]
struct Caps {
    phys: (f64, f64),
    off: (f64, f64),
    dpi: (f64, f64),
}

type MmRect = (f64, f64, f64, f64);

/// Ручная поправка одного фото на листе поверх шаблона: сдвиг (мм),
/// масштаб и поворот (градусы по часовой) вокруг центра.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Adj {
    dx: f64,
    dy: f64,
    scale: f64,
    angle: f64,
}

impl Default for Adj {
    fn default() -> Adj {
        Adj { dx: 0.0, dy: 0.0, scale: 1.0, angle: 0.0 }
    }
}

/// Фото на листе: центр, размер (до поворота) и угол, всё в мм.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Geom {
    cx: f64,
    cy: f64,
    w: f64,
    h: f64,
    angle: f64,
}

fn geom(r: MmRect, a: &Adj) -> Geom {
    Geom { cx: r.0 + r.2 / 2.0 + a.dx, cy: r.1 + r.3 / 2.0 + a.dy, w: r.2 * a.scale, h: r.3 * a.scale, angle: a.angle }
}

impl Geom {
    /// Точка листа (мм) → координаты фото относительно центра, без поворота.
    fn local(&self, x: f64, y: f64) -> (f64, f64) {
        let a = (-self.angle).to_radians();
        let (dx, dy) = (x - self.cx, y - self.cy);
        (dx * a.cos() - dy * a.sin(), dx * a.sin() + dy * a.cos())
    }
    /// Точка фото (относительно центра) → лист (мм).
    fn world(&self, u: f64, v: f64) -> (f64, f64) {
        let a = self.angle.to_radians();
        (self.cx + u * a.cos() - v * a.sin(), self.cy + u * a.sin() + v * a.cos())
    }
    fn contains(&self, x: f64, y: f64) -> bool {
        let (u, v) = self.local(x, y);
        u.abs() <= self.w / 2.0 && v.abs() <= self.h / 2.0
    }
    fn corners(&self) -> [(f64, f64); 4] {
        let (w, h) = (self.w / 2.0, self.h / 2.0);
        [self.world(-w, -h), self.world(w, -h), self.world(w, h), self.world(-w, h)]
    }
}

/// Нарисовать картинку с центром (cx, cy), размером w×h (до поворота) и углом — в точках устройства.
/// Повороты на 90° делаются без потерь, остаток — преобразованием GDI.
fn draw_obj(dc: HDC, im: &Img, cx: f64, cy: f64, w: f64, h: f64, angle: f64, hq: bool) {
    let qf = (angle / 90.0).round();
    let q = (qf as i64).rem_euclid(4) as u8;
    let rest = angle - qf * 90.0;
    let rotated;
    let im = if q != 0 {
        rotated = doc::rot_q(im.clone(), q);
        &rotated
    } else {
        im
    };
    let (w, h) = if q % 2 == 1 { (h, w) } else { (w, h) };
    unsafe {
        let info = crate::util::bmi(im);
        SetStretchBltMode(dc, if hq { HALFTONE } else { COLORONCOLOR });
        let _ = SetBrushOrgEx(dc, 0, 0, None);
        debug_assert!(rest.abs() < 0.01, "наклон — через compose()");
        StretchDIBits(dc, (cx - w / 2.0).round() as i32, (cy - h / 2.0).round() as i32, w.round().max(1.0) as i32, h.round().max(1.0) as i32,
            0, 0, im.w as i32, im.h as i32, Some(im.px.as_ptr() as *const _), &info, DIB_RGB_COLORS, SRCCOPY);
    }
}

/// Есть ли наклон не на прямой угол (тогда лист собирается своим кодом).
fn tilted(angle: f64) -> bool {
    (angle - (angle / 90.0).round() * 90.0).abs() >= 0.01
}

/// Собрать лист картинкой: белая бумага size (точки), k — точек на мм,
/// фото кладутся с поворотом на любой угол (билинейная выборка).
/// Свой код вместо поворота средствами GDI: так одинаково и на экране, и на
/// любом принтере (не все драйверы умеют повёрнутые картинки).
fn compose(size: (u32, u32), k: (f64, f64), objs: &[(&Img, Geom)]) -> Img {
    let mut dst = Img::filled(size.0.max(1), size.1.max(1), [255, 255, 255]);
    for (src, g) in objs {
        let (w, h) = (g.w * k.0, g.h * k.1);
        if w < 1.0 || h < 1.0 {
            continue;
        }
        // заранее уменьшить исходник до нужного размера (качественно, WIC)
        let small;
        let src: &Img = if (src.w as f64) > w * 1.5 {
            small = codec::resize(src, w.round().max(1.0) as u32, h.round().max(1.0) as u32).unwrap_or_else(|_| (*src).clone());
            &small
        } else {
            src
        };
        let (cx, cy) = (g.cx * k.0, g.cy * k.1);
        let a = g.angle.to_radians();
        let (ca, sa) = (a.cos(), a.sin());
        // габарит повёрнутого прямоугольника
        let ex = (w / 2.0 * ca).abs() + (h / 2.0 * sa).abs();
        let ey = (w / 2.0 * sa).abs() + (h / 2.0 * ca).abs();
        let x0 = (cx - ex).floor().max(0.0) as i64;
        let x1 = (cx + ex).ceil().min(dst.w as f64) as i64;
        let y0 = (cy - ey).floor().max(0.0) as i64;
        let y1 = (cy + ey).ceil().min(dst.h as f64) as i64;
        let (sw, sh) = (src.w as f64, src.h as f64);
        for y in y0..y1 {
            for x in x0..x1 {
                let (dx, dy) = (x as f64 + 0.5 - cx, y as f64 + 0.5 - cy);
                // в систему фото (поворот назад)
                let u = dx * ca + dy * sa;
                let v = -dx * sa + dy * ca;
                if u.abs() > w / 2.0 || v.abs() > h / 2.0 {
                    continue;
                }
                let fx = ((u + w / 2.0) / w * sw - 0.5).clamp(0.0, sw - 1.0);
                let fy = ((v + h / 2.0) / h * sh - 0.5).clamp(0.0, sh - 1.0);
                let (ix, iy) = (fx as usize, fy as usize);
                let (ix2, iy2) = ((ix + 1).min(src.w as usize - 1), (iy + 1).min(src.h as usize - 1));
                let (tx, ty) = (fx - ix as f64, fy - iy as f64);
                let o = (y as usize * dst.w as usize + x as usize) * 4;
                for c in 0..3 {
                    let p = |xx: usize, yy: usize| src.px[(yy * src.w as usize + xx) * 4 + c] as f64;
                    let top = p(ix, iy) * (1.0 - tx) + p(ix2, iy) * tx;
                    let bot = p(ix, iy2) * (1.0 - tx) + p(ix2, iy2) * tx;
                    dst.px[o + c] = (top * (1.0 - ty) + bot * ty).round() as u8;
                }
            }
        }
    }
    dst
}

#[derive(Clone, Copy, PartialEq)]
enum DragKind {
    Move,
    Scale,
    Rotate,
}

#[derive(Clone, Copy)]
struct Drag {
    kind: DragKind,
    slot: usize,
    from: (f64, f64),
    start: Adj,
    g: Geom,
}

/// Буфер DEVMODE с выравниванием (Vec<u64>).
#[derive(Clone)]
struct DevMode(Vec<u64>);

impl DevMode {
    fn ptr(&mut self) -> *mut DEVMODEW {
        self.0.as_mut_ptr() as *mut DEVMODEW
    }
}

struct PrintDlg {
    hwnd: HWND,
    pages: Vec<Page>,
    cur: usize,
    name: String,
    printers: Vec<String>,
    pi: Option<usize>,
    dm: Option<DevMode>,
    paper: usize,
    orient: usize,
    fill: bool,
    nup: usize,
    margin: i32,
    copies: i32,
    all: bool,
    autorot: bool,
    ed: Edits,
    sheet: usize,
    caps: HashMap<bool, Caps>,
    thumbs: HashMap<usize, Rc<Img>>,
    prepped: HashMap<usize, Rc<Img>>,
    prev: RECT,
    pbtn: RECT,
    result: Option<String>,
    /// Ручные поправки: (лист, место на листе) → сдвиг/масштаб/поворот.
    adj: HashMap<(usize, usize), Adj>,
    sel: Option<usize>,
    drag: Option<Drag>,
    hover: Option<DragKind>,
    /// Предпросмотр: левый верхний угол листа на экране и точек на мм.
    view: (f64, f64, f64),
}

// ------------------------------------------------------------ принтеры ---
fn printers() -> Vec<String> {
    let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
    let (mut need, mut ret) = (0u32, 0u32);
    unsafe {
        let _ = EnumPrintersW(flags, PCWSTR::null(), 4, None, &mut need, &mut ret);
        if need == 0 {
            return vec![];
        }
        let mut buf = vec![0u64; (need as usize).div_ceil(8)];
        let bytes = std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, need as usize);
        if EnumPrintersW(flags, PCWSTR::null(), 4, Some(bytes), &mut need, &mut ret).is_err() {
            return vec![];
        }
        let infos = buf.as_ptr() as *const PRINTER_INFO_4W;
        (0..ret as usize).filter_map(|i| (*infos.add(i)).pPrinterName.to_string().ok()).collect()
    }
}

fn default_printer() -> Option<String> {
    let mut buf = vec![0u16; 512];
    let mut n = buf.len() as u32;
    unsafe { GetDefaultPrinterW(PWSTR(buf.as_mut_ptr()), &mut n).as_bool().then(|| from_wide(&buf)) }
}

fn with_printer<R>(name: &str, f: impl FnOnce(HANDLE, PCWSTR) -> R) -> Option<R> {
    let w = wide(name);
    let mut h = HANDLE::default();
    unsafe {
        OpenPrinterW(PCWSTR(w.as_ptr()), &mut h, None).ok()?;
        let r = f(h, PCWSTR(w.as_ptr()));
        let _ = ClosePrinter(h);
        Some(r)
    }
}

/// Настройки принтера по умолчанию.
fn default_devmode(hwnd: HWND, name: &str) -> Option<DevMode> {
    with_printer(name, |h, pn| unsafe {
        let size = DocumentPropertiesW(hwnd, h, pn, None, None, 0);
        if size <= 0 {
            return None;
        }
        let mut dm = DevMode(vec![0u64; (size as usize).div_ceil(8)]);
        let r = DocumentPropertiesW(hwnd, h, pn, Some(dm.ptr()), None, DM_OUT_BUFFER.0);
        (r >= 0).then_some(dm)
    })
    .flatten()
}

/// Окно «Свойства принтера» самого драйвера.
fn properties(hwnd: HWND, name: &str, cur: &DevMode) -> Option<DevMode> {
    with_printer(name, |h, pn| unsafe {
        let size = DocumentPropertiesW(hwnd, h, pn, None, None, 0);
        if size <= 0 {
            return None;
        }
        let mut inp = cur.clone();
        let mut out = DevMode(vec![0u64; (size as usize).div_ceil(8)]);
        let r = DocumentPropertiesW(hwnd, h, pn, Some(out.ptr()), Some(inp.ptr()), (DM_IN_BUFFER | DM_OUT_BUFFER | DM_IN_PROMPT).0);
        (r == IDOK.0).then_some(out)
    })
    .flatten()
}

fn caps_of(dc: HDC) -> Caps {
    unsafe {
        let g = |i| GetDeviceCaps(dc, i) as f64;
        let (dx, dy) = (g(LOGPIXELSX).max(1.0), g(LOGPIXELSY).max(1.0));
        let mm = |v: f64, d: f64| v / d * 25.4;
        let (pw, ph) = (g(PHYSICALWIDTH), g(PHYSICALHEIGHT));
        let (pw, ph) = if pw > 0.0 && ph > 0.0 { (pw, ph) } else { (g(HORZRES), g(VERTRES)) };
        Caps { phys: (mm(pw, dx), mm(ph, dy)), off: (mm(g(PHYSICALOFFSETX), dx), mm(g(PHYSICALOFFSETY), dy)), dpi: (dx, dy) }
    }
}

// ------------------------------------------------------------------ логика ---
/// Ячейки листа (мм) для n фото: ряды/столбцы зависят от ориентации листа.
fn sheet_cells(phys: (f64, f64), margin: f64, n: usize) -> Vec<MmRect> {
    let land = phys.0 > phys.1;
    let (rows, cols) = match n {
        1 => (1, 1),
        2 => (2, 1),
        4 => (2, 2),
        6 => (3, 2),
        _ => (3, 3),
    };
    let (rows, cols) = if land { (cols, rows) } else { (rows, cols) };
    let gap = if n > 1 { GAP_MM } else { 0.0 };
    let m = margin.min(phys.0.min(phys.1) / 3.0);
    let (aw, ah) = (phys.0 - 2.0 * m, phys.1 - 2.0 * m);
    let cw = (aw - gap * (cols - 1) as f64) / cols as f64;
    let ch = (ah - gap * (rows - 1) as f64) / rows as f64;
    (0..n).map(|i| (m + (i % cols) as f64 * (cw + gap), m + (i / cols) as f64 * (ch + gap), cw, ch)).collect()
}

/// Подготовить картинку для ячейки: автоповорот + обрезка под ячейку (заполнить).
/// Возвращает картинку и прямоугольник на листе (мм).
fn fit_cell(img: &Img, cell: MmRect, fill: bool, autorot: bool) -> (Img, MmRect) {
    let mut im = img.clone();
    let cell_wide = cell.2 > cell.3;
    let img_wide = im.w > im.h;
    let squareish = (im.w as f64 / im.h as f64 - 1.0).abs() < 0.05 || (cell.2 / cell.3 - 1.0).abs() < 0.05;
    if autorot && !squareish && cell_wide != img_wide {
        im = im.rotate_cw();
    }
    if fill {
        (im.crop_to_aspect(cell.2 / cell.3), cell)
    } else {
        let s = (cell.2 / im.w as f64).min(cell.3 / im.h as f64);
        let (w, h) = (im.w as f64 * s, im.h as f64 * s);
        let r = (cell.0 + (cell.2 - w) / 2.0, cell.1 + (cell.3 - h) / 2.0, w, h);
        (im, r)
    }
}

/// Прямоугольник, который займёт фото с пропорцией aspect (ш/в) в ячейке — как в fit_cell.
fn fit_rect(aspect: f64, cell: MmRect, fill: bool, autorot: bool) -> MmRect {
    let cell_wide = cell.2 > cell.3;
    let img_wide = aspect > 1.0;
    let squareish = (aspect - 1.0).abs() < 0.05 || (cell.2 / cell.3 - 1.0).abs() < 0.05;
    let a = if autorot && !squareish && cell_wide != img_wide { 1.0 / aspect } else { aspect };
    if fill {
        cell
    } else {
        let (w, h) = if cell.2 / cell.3 > a { (cell.3 * a, cell.3) } else { (cell.2, cell.2 / a) };
        (cell.0 + (cell.2 - w) / 2.0, cell.1 + (cell.3 - h) / 2.0, w, h)
    }
}

/// Угол в диапазон (−180; 180].
fn norm_angle(a: f64) -> f64 {
    let mut a = a % 360.0;
    if a > 180.0 {
        a -= 360.0;
    }
    if a <= -180.0 {
        a += 360.0;
    }
    a
}

impl PrintDlg {
    fn adj_of(&self, sheet: usize, slot: usize) -> Adj {
        self.adj.get(&(sheet, slot)).copied().unwrap_or_default()
    }

    /// Места фото на текущем листе предпросмотра (мм): прямоугольник шаблона и итог с поправкой.
    fn sheet_geoms(&mut self) -> Vec<(usize, Geom)> {
        let sheets = self.sheets();
        let Some(sheet) = sheets.get(self.sheet).cloned() else { return vec![] };
        let land = self.sheet_land(&sheet);
        let c = self.caps_view(land);
        let cells = sheet_cells(c.phys, self.margin as f64, NUPS[self.nup]);
        let mut v = vec![];
        for (slot, (cell, &pi)) in cells.iter().zip(sheet.iter()).enumerate() {
            let a = self.aspect_after(pi);
            let r = fit_rect(a, *cell, self.fill, self.autorot);
            v.push((slot, geom(r, &self.adj_of(self.sheet, slot))));
        }
        v
    }

    /// Пропорции фото, как их положит fit_cell (с правками печати).
    fn aspect_after(&mut self, i: usize) -> f64 {
        self.aspect(i)
    }

    fn to_mm(&self, x: i32, y: i32) -> (f64, f64) {
        let (x0, y0, k) = self.view;
        ((x as f64 - x0) / k.max(1e-6), (y as f64 - y0) / k.max(1e-6))
    }

    /// Что под мышью: (место, что тащить).
    fn hit(&mut self, x: i32, y: i32) -> Option<(usize, DragKind, Geom)> {
        let (mx, my) = self.to_mm(x, y);
        let k = self.view.2.max(1e-6);
        let geoms = self.sheet_geoms();
        let near = 7.0 / k; // 7 точек экрана в мм
        if let Some(sel) = self.sel {
            if let Some((_, g)) = geoms.iter().find(|(s, _)| *s == sel) {
                let (u, v) = g.local(mx, my);
                let rot_v = -g.h / 2.0 - 24.0 / k;
                if (u.powi(2) + (v - rot_v).powi(2)).sqrt() <= near * 1.3 {
                    return Some((sel, DragKind::Rotate, *g));
                }
                if ((u.abs() - g.w / 2.0).abs() <= near) && ((v.abs() - g.h / 2.0).abs() <= near) {
                    return Some((sel, DragKind::Scale, *g));
                }
            }
        }
        // сверху то, что нарисовано позже; выбранное — в приоритете
        if let Some(sel) = self.sel {
            if let Some((_, g)) = geoms.iter().find(|(s, g)| *s == sel && g.contains(mx, my)) {
                return Some((sel, DragKind::Move, *g));
            }
        }
        geoms.iter().rev().find(|(_, g)| g.contains(mx, my)).map(|(s, g)| (*s, DragKind::Move, *g))
    }

    fn set_adj(&mut self, slot: usize, a: Adj) {
        if a == Adj::default() {
            self.adj.remove(&(self.sheet, slot));
        } else {
            self.adj.insert((self.sheet, slot), a);
        }
    }

    fn change_sel(&mut self, f: impl FnOnce(&mut Adj)) {
        if let Some(s) = self.sel {
            let mut a = self.adj_of(self.sheet, s);
            f(&mut a);
            a.scale = a.scale.clamp(0.1, 6.0);
            a.angle = norm_angle(a.angle);
            self.set_adj(s, a);
        }
    }

    fn printer(&self) -> Option<&str> {
        self.pi.and_then(|i| self.printers.get(i)).map(|s| s.as_str())
    }

    fn job_pages(&self) -> Vec<usize> {
        if self.all { (0..self.pages.len()).collect() } else { vec![self.cur] }
    }

    fn sheets(&self) -> Vec<Vec<usize>> {
        let jp = self.job_pages();
        let k = NUPS[self.nup];
        if jp.len() == 1 && k > 1 {
            vec![vec![jp[0]; k]] // одно фото — несколько копий на листе
        } else {
            jp.chunks(k).map(|c| c.to_vec()).collect()
        }
    }

    /// Пропорции страницы с учётом поворота из правок печати.
    fn aspect(&mut self, i: usize) -> f64 {
        let a = match self.pages[i].size_pt() {
            Some((w, h)) => w / h,
            None => self.thumb(i).map(|t| t.w as f64 / t.h as f64).unwrap_or(1.0),
        };
        if self.ed.rot % 2 == 1 { 1.0 / a } else { a }
    }

    fn sheet_land(&mut self, sheet: &[usize]) -> bool {
        match self.orient {
            1 => false,
            2 => true,
            _ => {
                let wide = sheet.first().map(|&i| self.aspect(i) > 1.0).unwrap_or(false);
                let k = NUPS[self.nup];
                if k == 2 || k == 6 { !wide } else { wide }
            }
        }
    }

    /// DEVMODE с нашими настройками (ориентация, бумага, цвет, 1 копия).
    fn build_dm(&self, land: bool) -> Option<DevMode> {
        let mut dm = match &self.dm {
            Some(d) => d.clone(),
            None => default_devmode(self.hwnd, self.printer()?)?,
        };
        unsafe {
            let p = dm.ptr();
            (*p).dmFields |= DM_ORIENTATION | DM_COPIES;
            (*p).Anonymous1.Anonymous1.dmOrientation = if land { DMORIENT_LANDSCAPE as i16 } else { DMORIENT_PORTRAIT as i16 };
            (*p).Anonymous1.Anonymous1.dmCopies = 1;
            if self.paper > 0 {
                (*p).dmFields |= DM_PAPERSIZE;
                (*p).Anonymous1.Anonymous1.dmPaperSize = PAPERS[self.paper].1;
            }
            if self.ed.gray {
                (*p).dmFields |= DM_COLOR;
                (*p).dmColor = DMCOLOR_MONOCHROME;
            }
        }
        Some(dm)
    }

    /// Размер бумаги без обращения к драйверу (для отрисовки): кеш или размер из списка.
    fn caps_view(&self, land: bool) -> Caps {
        self.caps.get(&land).copied().unwrap_or_else(|| {
            let (w, h) = PAPERS[self.paper].2;
            Caps { phys: if land { (h, w) } else { (w, h) }, off: (0.0, 0.0), dpi: (300.0, 300.0) }
        })
    }

    /// Спросить у драйвера принтера геометрию листа. Только из обработчиков
    /// событий — не из отрисовки: драйвер (особенно недоступного сетевого
    /// принтера) может надолго задуматься и прокрутить очередь сообщений.
    fn caps(&mut self, land: bool) -> Caps {
        if let Some(c) = self.caps.get(&land) {
            return *c;
        }
        let from_printer = self.printer().map(|s| s.to_string()).and_then(|name| {
            let mut dm = self.build_dm(land)?;
            let w = wide(&name);
            unsafe {
                let ic = CreateICW(PCWSTR::null(), PCWSTR(w.as_ptr()), PCWSTR::null(), Some(dm.ptr()));
                if ic.is_invalid() {
                    return None;
                }
                let c = caps_of(ic);
                let _ = DeleteDC(ic);
                (c.phys.0 > 10.0 && c.phys.1 > 10.0).then_some(c)
            }
        });
        let c = from_printer.unwrap_or_else(|| {
            let (w, h) = PAPERS[self.paper].2;
            Caps { phys: if land { (h, w) } else { (w, h) }, off: (0.0, 0.0), dpi: (300.0, 300.0) }
        });
        self.caps.insert(land, c);
        c
    }

    fn thumb(&mut self, i: usize) -> Option<Rc<Img>> {
        if let Some(t) = self.thumbs.get(&i) {
            return Some(t.clone());
        }
        let img = doc::render(&self.pages[i], 110.0).ok()?;
        let t = Rc::new(codec::thumb(&img, 1000));
        self.thumbs.insert(i, t.clone());
        Some(t)
    }

    fn prepped(&mut self, i: usize) -> Option<Rc<Img>> {
        if let Some(t) = self.prepped.get(&i) {
            return Some(t.clone());
        }
        let t = self.thumb(i)?;
        let p = Rc::new(self.ed.apply(&t));
        self.prepped.insert(i, p.clone());
        Some(p)
    }

    fn reset_caps(&mut self) {
        self.caps.clear();
        let _ = self.caps(false);
        let _ = self.caps(true);
    }

    fn select_printer(&mut self, i: usize) {
        self.pi = Some(i);
        self.dm = None;
        self.reset_caps();
        settings::set_s("printer", &self.printers[i]);
    }

    fn popup_printers(&mut self, hwnd: HWND, at: RECT) {
        if self.printers.is_empty() {
            return;
        }
        unsafe {
            let Ok(m) = CreatePopupMenu() else { return };
            for (i, p) in self.printers.iter().enumerate() {
                let flags = if Some(i) == self.pi { MF_STRING | MF_CHECKED } else { MF_STRING };
                let w = wide(p);
                let _ = AppendMenuW(m, flags, (MENU_BASE + i as u32) as usize, PCWSTR(w.as_ptr()));
            }
            let mut pt = POINT { x: at.left, y: at.bottom };
            let _ = ClientToScreen(hwnd, &mut pt);
            let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN, pt.x, pt.y, 0, hwnd, None).0 as u32;
            let _ = DestroyMenu(m);
            if cmd >= MENU_BASE && ((cmd - MENU_BASE) as usize) < self.printers.len() {
                self.select_printer((cmd - MENU_BASE) as usize);
            }
        }
    }

    fn save_settings(&self) {
        settings::set_i("print_paper", self.paper as i32);
        settings::set_i("print_orient", self.orient as i32);
        settings::set_i("print_fill", self.fill as i32);
        settings::set_i("print_nup", self.nup as i32);
        settings::set_i("print_margin", self.margin);
        settings::set_i("print_autorot", self.autorot as i32);
    }

    /// Полноразмерная картинка страницы для печати/PDF (с правками печати).
    fn full(&self, i: usize, cache: &mut HashMap<usize, Rc<Img>>) -> Result<Rc<Img>, String> {
        if let Some(c) = cache.get(&i) {
            return Ok(c.clone());
        }
        let img = doc::render(&self.pages[i], doc::PRINT_DPI)?;
        let p = Rc::new(self.ed.apply(&img));
        cache.clear(); // держим в памяти только текущую
        cache.insert(i, p.clone());
        Ok(p)
    }

    fn do_print(&mut self) -> Result<usize, String> {
        let name = self.printer().ok_or("Принтер не выбран")?.to_string();
        let sheets = self.sheets();
        let lands: Vec<bool> = sheets.iter().map(|s| self.sheet_land(s)).collect();
        let mut dm = self.build_dm(lands[0]).ok_or("Не удалось получить настройки принтера")?;
        let w = wide(&name);
        let doc_name = wide(&self.name);
        unsafe {
            let dc = CreateDCW(PCWSTR::null(), PCWSTR(w.as_ptr()), PCWSTR::null(), Some(dm.ptr()));
            if dc.is_invalid() {
                return Err("Принтер недоступен".into());
            }
            let di = DOCINFOW { cbSize: std::mem::size_of::<DOCINFOW>() as i32, lpszDocName: PCWSTR(doc_name.as_ptr()), ..Default::default() };
            if StartDocW(dc, &di) <= 0 {
                let _ = DeleteDC(dc);
                return Err("Не удалось начать печать".into());
            }
            let mut cache = HashMap::new();
            let mut cur_land = lands[0];
            let res = (|| -> Result<(), String> {
                for _ in 0..self.copies {
                    for (s, sheet) in sheets.iter().enumerate() {
                        if lands[s] != cur_land {
                            if let Some(mut d2) = self.build_dm(lands[s]) {
                                ResetDCW(dc, d2.ptr());
                            }
                            cur_land = lands[s];
                        }
                        if StartPage(dc) <= 0 {
                            return Err("Сбой при печати страницы".into());
                        }
                        let c = caps_of(dc);
                        let cells = sheet_cells(c.phys, self.margin as f64, NUPS[self.nup]);
                        let any_tilt = (0..sheet.len()).any(|slot| tilted(self.adj_of(s, slot).angle));
                        if any_tilt {
                            // лист целиком своей картинкой (до 300 точек на дюйм)
                            let d = (c.dpi.0.min(300.0), c.dpi.1.min(300.0));
                            let km = (d.0 / 25.4, d.1 / 25.4);
                            let size = ((c.phys.0 * km.0).round() as u32, (c.phys.1 * km.1).round() as u32);
                            let mut imgs = vec![];
                            for (slot, (cell, &pi)) in cells.iter().zip(sheet.iter()).enumerate() {
                                let img = self.full(pi, &mut cache)?;
                                let (im, r) = fit_cell(&img, *cell, self.fill, self.autorot);
                                imgs.push((im, geom(r, &self.adj_of(s, slot))));
                            }
                            let refs: Vec<(&Img, Geom)> = imgs.iter().map(|(i, g)| (i, *g)).collect();
                            let page = compose(size, km, &refs);
                            let info = crate::util::bmi(&page);
                            let to_dev = |x: f64, off: f64, dd: f64| ((x - off) / 25.4 * dd).round() as i32;
                            let (dx0, dy0) = (to_dev(0.0, c.off.0, c.dpi.0), to_dev(0.0, c.off.1, c.dpi.1));
                            let (dw, dh) = ((c.phys.0 / 25.4 * c.dpi.0).round() as i32, (c.phys.1 / 25.4 * c.dpi.1).round() as i32);
                            SetStretchBltMode(dc, HALFTONE);
                            let _ = SetBrushOrgEx(dc, 0, 0, None);
                            StretchDIBits(dc, dx0, dy0, dw, dh, 0, 0, page.w as i32, page.h as i32, Some(page.px.as_ptr() as *const _), &info, DIB_RGB_COLORS, SRCCOPY);
                            EndPage(dc);
                            continue;
                        }
                        for (slot, (cell, &pi)) in cells.iter().zip(sheet.iter()).enumerate() {
                            let img = self.full(pi, &mut cache)?;
                            let (im, r) = fit_cell(&img, *cell, self.fill, self.autorot);
                            let g = geom(r, &self.adj_of(s, slot));
                            let cx = (g.cx - c.off.0) / 25.4 * c.dpi.0;
                            let cy = (g.cy - c.off.1) / 25.4 * c.dpi.1;
                            let (dw, dh) = (g.w / 25.4 * c.dpi.0, g.h / 25.4 * c.dpi.1);
                            // не гоним в драйвер больше точек, чем нужно бумаге
                            let im = if im.w as f64 > dw * 1.05 {
                                codec::resize(&im, dw.round().max(1.0) as u32, dh.round().max(1.0) as u32).unwrap_or(im)
                            } else {
                                im
                            };
                            draw_obj(dc, &im, cx, cy, dw, dh, g.angle, true);
                        }
                        EndPage(dc);
                    }
                }
                Ok(())
            })();
            if res.is_err() {
                AbortDoc(dc);
            } else {
                EndDoc(dc);
            }
            let _ = DeleteDC(dc);
            res.map(|_| sheets.len() * self.copies as usize)
        }
    }

    fn do_pdf(&mut self, path: &std::path::Path) -> Result<(), String> {
        let sheets = self.sheets();
        let mut out = vec![];
        let mut cache = HashMap::new();
        for (si, sheet) in sheets.iter().enumerate() {
            let land = self.sheet_land(sheet);
            let c = self.caps(land);
            let cells = sheet_cells(c.phys, self.margin as f64, NUPS[self.nup]);
            let mut items = vec![];
            for (slot, (cell, &pi)) in cells.iter().zip(sheet.iter()).enumerate() {
                let img = self.full(pi, &mut cache)?;
                let (im, r) = fit_cell(&img, *cell, self.fill, self.autorot);
                let g = geom(r, &self.adj_of(si, slot));
                // 300 точек на дюйм достаточно для печати
                let (tw, th) = ((g.w / 25.4 * 300.0).round() as u32, (g.h / 25.4 * 300.0).round() as u32);
                let im = if im.w > tw.max(1) { codec::resize(&im, tw.max(1), th.max(1)).unwrap_or(im) } else { im };
                let jpeg = codec::encode_jpeg(&im).map_err(|e| e.message().to_string())?;
                let place = ((g.cx - g.w / 2.0) * MM, (c.phys.1 - g.cy - g.h / 2.0) * MM, g.w * MM, g.h * MM);
                items.push(Placed { jpeg, px: (im.w, im.h), place, angle: g.angle });
            }
            out.push(OutPage::Raster { page: (c.phys.0 * MM, c.phys.1 * MM), items });
        }
        pdf_write::write(&out, path)
    }
}

// ------------------------------------------------------------------ окно ---
impl Panel for PrintDlg {
    fn title(&self) -> String {
        format!("Печать — {}", self.name)
    }

    fn init(&mut self, hwnd: HWND) {
        self.hwnd = hwnd;
    }

    fn min_size(&self) -> (i32, i32) {
        (900, 660)
    }

    fn layout(&mut self, ui: &Ui, w: i32, h: i32) -> Vec<W> {
        let m = ui.s(16);
        let pw = ui.s(350);
        let x = w - m - pw;
        let mut v = vec![];
        let mut y = m;
        let gh = ui.s(22);
        let row = ui.s(32);
        let step = ui.s(12);
        let group = |v: &mut Vec<W>, y: &mut i32, t: &str| {
            v.push(W::new(Kind::Group, 0, gui::r(x, *y, pw, gh)).t(t));
            *y += gh + ui.s(4);
        };

        group(&mut v, &mut y, "Принтер");
        let pname = match self.printer() {
            Some(p) => format!("{p}  ▾"),
            None => "Принтеры не найдены".to_string(),
        };
        self.pbtn = gui::r(x, y, pw - ui.s(44), ui.s(34));
        v.push(W::new(Kind::Button, ID_PRINTER, self.pbtn).t(pname).en(!self.printers.is_empty()).tip("Выбрать принтер"));
        v.push(W::new(Kind::Button, ID_PROPS, gui::r(x + pw - ui.s(36), y, ui.s(36), ui.s(34))).i(gui::IC_SETTINGS).en(self.pi.is_some()).tip("Свойства принтера (качество, лоток…)"));
        y += ui.s(34) + step;

        group(&mut v, &mut y, "Бумага");
        let names: Vec<&str> = PAPERS.iter().map(|p| p.0).collect();
        v.extend(gui::segments(ui, x, y, pw, ID_PAPER0, &names, self.paper));
        y += row + step;

        group(&mut v, &mut y, "Ориентация");
        v.extend(gui::segments(ui, x, y, pw, ID_ORIENT0, &["Авто", "Книжная", "Альбомная"], self.orient));
        y += row + step;

        group(&mut v, &mut y, "Фото на листе");
        v.extend(gui::segments(ui, x, y, pw, ID_NUP0, &["1", "2", "4", "6", "9"], self.nup));
        y += row + step;

        group(&mut v, &mut y, "Размещение");
        let half = (pw - ui.s(10)) / 2;
        v.extend(gui::segments(ui, x, y, half, ID_FIT, &["Вписать", "Заполнить"], self.fill as usize));
        v.push(W::new(Kind::Check, ID_AUTOROT, gui::r(x + half + ui.s(10), y, half, row)).t("Автоповорот").on(self.autorot).tip("Поворачивать фото под форму ячейки"));
        y += row + step;

        v.push(W::new(Kind::Group, 0, gui::r(x, y, half, gh)).t("Поля, мм"));
        v.push(W::new(Kind::Group, 0, gui::r(x + half + ui.s(10), y, half, gh)).t("Копии"));
        y += gh + ui.s(4);
        v.extend(gui::stepper(ui, x, y, half, self.margin.to_string(), ID_M_DEC, ID_M_INC, "Поля по краям листа"));
        v.extend(gui::stepper(ui, x + half + ui.s(10), y, half, self.copies.to_string(), ID_C_DEC, ID_C_INC, "Сколько копий"));
        y += row + step;

        if self.pages.len() > 1 {
            group(&mut v, &mut y, "Страницы");
            let all = format!("Все ({})", self.pages.len());
            let curl = format!("Текущая ({})", self.cur + 1);
            v.extend(gui::segments(ui, x, y, pw, ID_ALL, &[all.as_str(), curl.as_str()], if self.all { 0 } else { 1 }));
            y += row + step;
        }

        group(&mut v, &mut y, "Правка для печати (файл не меняется)");
        let tb = ui.s(34);
        let gap = (pw - 9 * tb) / 8;
        let tools = [
            (ID_E_ROTL, gui::IC_ROT_L, "Повернуть влево", false),
            (ID_E_ROTR, gui::IC_ROT_R, "Повернуть вправо", false),
            (ID_E_FLIP, gui::IC_FLIP, "Отразить", self.ed.flip),
            (ID_E_BDN, gui::IC_SUN, "Темнее", false),
            (ID_E_BUP, gui::IC_SUN, "Ярче", false),
            (ID_E_CDN, gui::IC_CONTRAST, "Контраст −", false),
            (ID_E_CUP, gui::IC_CONTRAST, "Контраст +", false),
            (ID_E_GRAY, gui::IC_COLOR, "Чёрно-белая печать", self.ed.gray),
            (ID_E_RESET, gui::IC_RESET, "Сбросить правки", false),
        ];
        for (k, (id, ic, tip, on)) in tools.iter().enumerate() {
            v.push(W::new(Kind::Tool, *id, gui::r(x + k as i32 * (tb + gap), y, tb, tb)).i(*ic).tip(*tip).on(*on));
        }
        y += tb + ui.s(4);
        let sign = |n: i32| if n > 0 { format!("+{}%", n * 10) } else { format!("{}%", n * 10) };
        let desc = format!("Яркость {} · Контраст {}{}", sign(self.ed.bright), sign(self.ed.contrast), if self.ed.gray { " · Ч/Б" } else { "" });
        v.push(W::new(Kind::Label, 0, gui::r(x, y, pw, ui.s(22))).t(desc).color(ui.t.dim));

        // предпросмотр и листание
        self.prev = gui::r(m, m, x - 2 * m, h - m - ui.s(96) - m);
        let n = self.sheets().len();
        self.sheet = self.sheet.min(n.saturating_sub(1));
        let ny = self.prev.bottom + ui.s(8);
        // листы — слева в ряду кнопок
        let sy = h - m - ui.s(36) + (ui.s(36) - tb) / 2;
        v.push(W::new(Kind::Tool, ID_S_PREV, gui::r(m, sy, tb, tb)).i(gui::IC_PREV).en(self.sheet > 0).tip("Предыдущий лист"));
        v.push(W::new(Kind::Label, 0, gui::r(m + tb, sy, ui.s(112), tb)).t(format!("Лист {} из {}", self.sheet + 1, n.max(1))).align(1));
        v.push(W::new(Kind::Tool, ID_S_NEXT, gui::r(m + tb + ui.s(112), sy, tb, tb)).i(gui::IC_NEXT).en(self.sheet + 1 < n).tip("Следующий лист"));
        // выбранное фото — под предпросмотром
        let lx = self.prev.left;
        let avail = self.prev.right - lx;
        if let Some(sel) = self.sel {
            let a = self.adj_of(self.sheet, sel);
            let tools = [
                (ID_O_ROTL, gui::IC_ROT_L, "Повернуть фото на 90° влево"),
                (ID_O_ROTR, gui::IC_ROT_R, "Повернуть фото на 90° вправо"),
                (ID_O_SMALL, gui::IC_ZOOM_OUT, "Меньше (−, Ctrl+колесо)"),
                (ID_O_BIG, gui::IC_ZOOM_IN, "Больше (+, Ctrl+колесо)"),
                (ID_O_CENTER, gui::IC_FIT, "Вернуть на место (по центру ячейки)"),
                (ID_O_RESET, gui::IC_RESET, "Сбросить поворот, размер и место (Delete, двойной щелчок)"),
            ];
            let mut x = lx;
            for (id, ic, tip) in tools {
                v.push(W::new(Kind::Tool, id, gui::r(x, ny, tb, tb)).i(ic).tip(tip));
                x += tb + ui.s(2);
            }
            let d = format!("{:+.0}°  ·  {:.0}%", a.angle, a.scale * 100.0);
            if avail - (x - lx) > ui.s(90) {
                v.push(W::new(Kind::Label, 0, gui::r(x + ui.s(8), ny, avail - (x - lx) - ui.s(8), tb)).t(d).color(ui.t.dim));
            }
        } else if avail > ui.s(200) {
            let hint = if self.adj.is_empty() {
                "Щёлкните фото на листе: тянуть — двигать, уголки — размер, кружок — повернуть"
            } else {
                "Щёлкните фото, чтобы поправить его"
            };
            let mut x = lx;
            if !self.adj.is_empty() {
                v.push(W::new(Kind::Tool, ID_O_RESET_ALL, gui::r(x, ny, tb, tb)).i(gui::IC_RESET).tip("Сбросить ручные поправки всех фото"));
                x += tb + ui.s(8);
            }
            v.push(W::new(Kind::Label, 0, gui::r(x, ny, avail - (x - lx), tb)).t(hint).color(ui.t.dim));
        }

        // кнопки
        let by = h - m - ui.s(36);
        let mut bx = w - m;
        let sheets_total = n * self.copies as usize;
        let print_t = if sheets_total > 1 { format!("Печать ({sheets_total} л.)") } else { "Печать".to_string() };
        for (id, t, primary, en, bw) in [
            (ID_PRINT, print_t.as_str(), true, self.pi.is_some(), 150),
            (ID_CANCEL, "Отмена", false, true, 100),
            (ID_PDF, "Сохранить в PDF…", false, true, 170),
        ] {
            let bw = ui.s(bw);
            bx -= bw;
            v.push(W::new(Kind::Button, id, gui::r(bx, by, bw, ui.s(36))).t(t).on(primary).en(en));
            bx -= ui.s(8);
        }
        v
    }

    fn paint(&mut self, ui: &Ui, hdc: HDC, w: i32, h: i32) {
        let t = ui.t;
        gui::fill(hdc, &gui::r(0, 0, w, h), t.bg);
        let pr = self.prev;
        gui::round(hdc, &pr, ui.s(10), Some(t.canvas), None);
        let sheets = self.sheets();
        let Some(sheet) = sheets.get(self.sheet).cloned() else { return };
        let land = self.sheet_land(&sheet);
        let c = self.caps_view(land);
        let pad = ui.s(24) as f64;
        let k = ((pr.right - pr.left) as f64 - 2.0 * pad).max(10.0) / c.phys.0;
        let k = k.min(((pr.bottom - pr.top) as f64 - 2.0 * pad).max(10.0) / c.phys.1);
        let (pw, ph) = ((c.phys.0 * k).round() as i32, (c.phys.1 * k).round() as i32);
        let x0 = (pr.left + pr.right - pw) / 2;
        let y0 = (pr.top + pr.bottom - ph) / 2;
        gui::fill(hdc, &gui::r(x0 + ui.s(4), y0 + ui.s(4), pw, ph), gui::blend(t.canvas, rgb(0, 0, 0), 0.65));
        gui::fill(hdc, &gui::r(x0, y0, pw, ph), rgb(255, 255, 255));
        let to = |r: MmRect| RECT {
            left: x0 + (r.0 * k).round() as i32,
            top: y0 + (r.1 * k).round() as i32,
            right: x0 + ((r.0 + r.2) * k).round() as i32,
            bottom: y0 + ((r.1 + r.3) * k).round() as i32,
        };
        // поля (пунктиром)
        let m = (self.margin as f64).min(c.phys.0.min(c.phys.1) / 3.0);
        if m > 0.0 {
            gui::frame(hdc, &to((m, m, c.phys.0 - 2.0 * m, c.phys.1 - 2.0 * m)), rgb(200, 200, 200), PS_DOT);
        }
        self.view = (x0 as f64, y0 as f64, k);
        let cells = sheet_cells(c.phys, self.margin as f64, NUPS[self.nup]);
        let mut geoms = vec![];
        let mut objs: Vec<(Img, Geom)> = vec![];
        for (slot, (cell, &pi)) in cells.iter().zip(sheet.iter()).enumerate() {
            let Some(img) = self.prepped(pi) else {
                gui::text(hdc, "нет изображения", &to(*cell), ui.f.small, rgb(150, 150, 150), gui::CENTER);
                continue;
            };
            let (im, r) = fit_cell(&img, *cell, self.fill, self.autorot);
            let g = geom(r, &self.adj_of(self.sheet, slot));
            geoms.push((slot, g));
            objs.push((im, g));
        }
        if objs.iter().any(|(_, g)| tilted(g.angle)) {
            let refs: Vec<(&Img, Geom)> = objs.iter().map(|(i, g)| (i, *g)).collect();
            let sheet_img = compose((pw.max(1) as u32, ph.max(1) as u32), (k, k), &refs);
            gui::blit(hdc, &sheet_img, &gui::r(x0, y0, pw, ph), false);
            if m > 0.0 {
                gui::frame(hdc, &to((m, m, c.phys.0 - 2.0 * m, c.phys.1 - 2.0 * m)), rgb(200, 200, 200), PS_DOT);
            }
        } else {
            unsafe {
                let saved = SaveDC(hdc);
                let _ = IntersectClipRect(hdc, x0, y0, x0 + pw, y0 + ph);
                let hq = self.drag.is_none();
                for (im, g) in &objs {
                    draw_obj(hdc, im, x0 as f64 + g.cx * k, y0 as f64 + g.cy * k, g.w * k, g.h * k, g.angle, hq);
                }
                let _ = RestoreDC(hdc, saved);
            }
        }
        // выбранное фото: рамка, уголки (размер) и кружок (поворот)
        if let Some((_, g)) = geoms.iter().find(|(s, _)| Some(*s) == self.sel) {
            let p = |(x, y): (f64, f64)| ((x0 as f64 + x * k).round() as i32, (y0 as f64 + y * k).round() as i32);
            let cs = g.corners();
            for i in 0..4 {
                let (a, b) = (p(cs[i]), p(cs[(i + 1) % 4]));
                gui::line(hdc, a.0, a.1, b.0, b.1, t.accent, 2, PS_SOLID);
            }
            let hs = ui.s(5);
            for c in cs {
                let (x, y) = p(c);
                gui::fill(hdc, &gui::r(x - hs, y - hs, 2 * hs, 2 * hs), rgb(255, 255, 255));
                gui::frame(hdc, &gui::r(x - hs, y - hs, 2 * hs, 2 * hs), t.accent, PS_SOLID);
            }
            let top = p(g.world(0.0, -g.h / 2.0));
            let knob = p(g.world(0.0, -g.h / 2.0 - ui.s(24) as f64 / k));
            gui::line(hdc, top.0, top.1, knob.0, knob.1, t.accent, 1, PS_SOLID);
            let rr = ui.s(7);
            gui::round(hdc, &gui::r(knob.0 - rr, knob.1 - rr, 2 * rr, 2 * rr), 2 * rr, Some(rgb(255, 255, 255)), Some(t.accent));
        }
        // непечатаемая зона принтера — тонкая рамка
        if c.off.0 > 0.0 || c.off.1 > 0.0 {
            let pa = (c.off.0, c.off.1, c.phys.0 - 2.0 * c.off.0, c.phys.1 - 2.0 * c.off.1);
            gui::frame(hdc, &to(pa), rgb(235, 170, 170), PS_DOT);
        }
    }

    fn cursor(&self, x: i32, y: i32) -> Option<PCWSTR> {
        if !gui::inside(&self.prev, x, y) {
            return None;
        }
        if let Some(d) = self.drag {
            return Some(match d.kind {
                DragKind::Move => IDC_SIZEALL,
                DragKind::Scale => IDC_SIZENWSE,
                DragKind::Rotate => IDC_HAND,
            });
        }
        self.hover.map(|k| match k {
            DragKind::Move => IDC_SIZEALL,
            DragKind::Scale => IDC_SIZENWSE,
            DragKind::Rotate => IDC_HAND,
        })
    }

    fn mouse(&mut self, _hwnd: HWND, m: Mouse, x: i32, y: i32) -> bool {
        let shift = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x10) } < 0;
        let ctrl = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x11) } < 0;
        match m {
            Mouse::Down if gui::inside(&self.prev, x, y) => {
                match self.hit(x, y) {
                    Some((slot, kind, g)) => {
                        self.sel = Some(slot);
                        let start = self.adj_of(self.sheet, slot);
                        self.drag = Some(Drag { kind, slot, from: self.to_mm(x, y), start, g });
                    }
                    None => self.sel = None,
                }
                return true;
            }
            Mouse::Move => {
                let Some(d) = self.drag else {
                    let h = if gui::inside(&self.prev, x, y) { self.hit(x, y).map(|h| h.1) } else { None };
                    self.hover = h;
                    return false;
                };
                let (mx, my) = self.to_mm(x, y);
                let mut a = d.start;
                match d.kind {
                    DragKind::Move => {
                        a.dx = d.start.dx + mx - d.from.0;
                        a.dy = d.start.dy + my - d.from.1;
                        // Shift — только по горизонтали или вертикали
                        if shift {
                            if (mx - d.from.0).abs() > (my - d.from.1).abs() {
                                a.dy = d.start.dy;
                            } else {
                                a.dx = d.start.dx;
                            }
                        }
                    }
                    DragKind::Scale => {
                        let d0 = ((d.from.0 - d.g.cx).powi(2) + (d.from.1 - d.g.cy).powi(2)).sqrt().max(0.1);
                        let d1 = ((mx - d.g.cx).powi(2) + (my - d.g.cy).powi(2)).sqrt();
                        a.scale = (d.start.scale * d1 / d0).clamp(0.1, 6.0);
                    }
                    DragKind::Rotate => {
                        // угол от центра до мыши; «вверх» = 0°
                        let ang = (mx - d.g.cx).atan2(-(my - d.g.cy)).to_degrees();
                        let mut v = ang;
                        if shift {
                            v = (v / 15.0).round() * 15.0;
                        } else if ((v / 90.0).round() * 90.0 - v).abs() < 3.0 {
                            v = (v / 90.0).round() * 90.0; // прилипает к прямым углам
                        }
                        a.angle = norm_angle(v);
                    }
                }
                self.set_adj(d.slot, a);
                return true;
            }
            Mouse::Up => {
                if self.drag.take().is_some() {
                    return true;
                }
            }
            Mouse::Dbl if gui::inside(&self.prev, x, y) => {
                if let Some((slot, _, _)) = self.hit(x, y) {
                    self.sel = Some(slot);
                    self.set_adj(slot, Adj::default());
                    return true;
                }
            }
            Mouse::Wheel(d) if ctrl && self.sel.is_some() && gui::inside(&self.prev, x, y) => {
                let k = if d > 0 { 1.05 } else { 1.0 / 1.05 };
                self.change_sel(|a| a.scale *= k);
                return true;
            }
            _ => {}
        }
        if let Mouse::Wheel(d) = m {
            if gui::inside(&self.prev, x, y) {
                let n = self.sheets().len();
                if d < 0 && self.sheet + 1 < n {
                    self.sheet += 1;
                    self.sel = None;
                } else if d > 0 && self.sheet > 0 {
                    self.sheet -= 1;
                    self.sel = None;
                }
                return true;
            }
        }
        false
    }

    fn event(&mut self, hwnd: HWND, ev: Ev) -> bool {
        let Ev::Click(id) = ev else { return true };
        let edits_before = self.ed;
        match id {
            ID_PRINTER => self.popup_printers(hwnd, self.pbtn),
            ID_PROPS => {
                if let Some(name) = self.printer().map(|s| s.to_string()) {
                    let land = self.sheets().first().cloned().map(|s| self.sheet_land(&s)).unwrap_or(false);
                    if let Some(cur) = self.build_dm(land) {
                        if let Some(new) = properties(hwnd, &name, &cur) {
                            self.dm = Some(new);
                            // бумагу и ориентацию теперь задаёт драйвер
                            self.paper = 0;
                            self.reset_caps();
                        }
                    }
                }
            }
            i if (ID_PAPER0..ID_PAPER0 + PAPERS.len() as u32).contains(&i) => {
                self.paper = (i - ID_PAPER0) as usize;
                self.reset_caps();
            }
            i if (ID_ORIENT0..ID_ORIENT0 + 3).contains(&i) => self.orient = (i - ID_ORIENT0) as usize,
            i if (ID_NUP0..ID_NUP0 + NUPS.len() as u32).contains(&i) => {
                self.nup = (i - ID_NUP0) as usize;
                self.sheet = 0;
                self.adj.clear();
                self.sel = None;
            }
            ID_FIT => self.fill = false,
            ID_FILL => self.fill = true,
            ID_AUTOROT => self.autorot = !self.autorot,
            ID_M_DEC => self.margin = (self.margin - 1).max(0),
            ID_M_INC => self.margin = (self.margin + 1).min(50),
            ID_C_DEC => self.copies = (self.copies - 1).max(1),
            ID_C_INC => self.copies = (self.copies + 1).min(99),
            ID_ALL => {
                self.all = true;
                self.sheet = 0;
                self.adj.clear();
                self.sel = None;
            }
            ID_CUR => {
                self.all = false;
                self.sheet = 0;
                self.adj.clear();
                self.sel = None;
            }
            ID_O_ROTL => self.change_sel(|a| a.angle = ((a.angle / 90.0).round() - 1.0) * 90.0),
            ID_O_ROTR => self.change_sel(|a| a.angle = ((a.angle / 90.0).round() + 1.0) * 90.0),
            ID_O_SMALL => self.change_sel(|a| a.scale /= 1.1),
            ID_O_BIG => self.change_sel(|a| a.scale *= 1.1),
            ID_O_CENTER => self.change_sel(|a| {
                a.dx = 0.0;
                a.dy = 0.0;
            }),
            ID_O_RESET => self.change_sel(|a| *a = Adj::default()),
            ID_O_RESET_ALL => {
                self.adj.clear();
            }
            ID_E_ROTL => self.ed.rot = (self.ed.rot + 3) % 4,
            ID_E_ROTR => self.ed.rot = (self.ed.rot + 1) % 4,
            ID_E_FLIP => self.ed.flip = !self.ed.flip,
            ID_E_BDN => self.ed.bright = (self.ed.bright - 1).max(-8),
            ID_E_BUP => self.ed.bright = (self.ed.bright + 1).min(10),
            ID_E_CDN => self.ed.contrast = (self.ed.contrast - 1).max(-8),
            ID_E_CUP => self.ed.contrast = (self.ed.contrast + 1).min(10),
            ID_E_GRAY => {
                self.ed.gray = !self.ed.gray;
                self.reset_caps();
            }
            ID_E_RESET => self.ed = Edits::default(),
            ID_S_PREV => {
                self.sheet = self.sheet.saturating_sub(1);
                self.sel = None;
            }
            ID_S_NEXT => {
                self.sheet += 1;
                self.sel = None;
            }
            ID_PDF => {
                self.save_settings();
                let stem = self.name.clone();
                let dir = self.pages.iter().find_map(|p| p.path().and_then(|x| x.parent()).map(|d| d.to_path_buf()));
                if let Some(p) = files::save_dialog(hwnd, &format!("{stem} (печать).pdf"), dir.as_deref(), true, true) {
                    match self.do_pdf(&p) {
                        Ok(()) => {
                            self.result = Some(format!("Сохранено: {}", file_name(&p)));
                            return false;
                        }
                        Err(e) => {
                            crate::ui::message(Some(hwnd), &format!("Не удалось сохранить PDF:\n{e}"), true);
                        }
                    }
                }
            }
            ID_PRINT => {
                self.save_settings();
                let cursor = unsafe { SetCursor(LoadCursorW(None, IDC_WAIT).unwrap_or_default()) };
                let r = self.do_print();
                unsafe {
                    SetCursor(cursor);
                }
                match r {
                    Ok(n) => {
                        self.result = Some(format!("Отправлено на печать: листов — {n}"));
                        return false;
                    }
                    Err(e) => {
                        crate::ui::message(Some(hwnd), &format!("Печать не удалась:\n{e}"), true);
                    }
                }
            }
            ID_CANCEL => {
                self.save_settings();
                return false;
            }
            _ => {}
        }
        if self.ed != edits_before {
            self.prepped.clear();
        }
        true
    }

    fn key(&mut self, hwnd: HWND, vk: u16, _ctrl: bool, shift: bool) -> Option<bool> {
        if self.sel.is_some() {
            let st = if shift { 5.0 } else { 1.0 };
            match vk {
                0x25 => self.change_sel(|a| a.dx -= st),
                0x27 => self.change_sel(|a| a.dx += st),
                0x26 => self.change_sel(|a| a.dy -= st),
                0x28 => self.change_sel(|a| a.dy += st),
                0xBB | 0x6B => self.change_sel(|a| a.scale *= 1.05),
                0xBD | 0x6D => self.change_sel(|a| a.scale /= 1.05),
                0xDB => self.change_sel(|a| a.angle -= if shift { 15.0 } else { 1.0 }), // [
                0xDD => self.change_sel(|a| a.angle += if shift { 15.0 } else { 1.0 }), // ]
                0x2E | 0x08 => self.change_sel(|a| *a = Adj::default()),
                0x1B => self.sel = None,
                _ => return self.key_sheet(hwnd, vk),
            }
            return Some(true);
        }
        self.key_sheet(hwnd, vk)
    }
}

impl PrintDlg {
    fn key_sheet(&mut self, hwnd: HWND, vk: u16) -> Option<bool> {
        match vk {
            0x21 | 0x25 => Some(self.event(hwnd, Ev::Click(ID_S_PREV))),
            0x22 | 0x27 => {
                if self.sheet + 1 < self.sheets().len() {
                    self.sheet += 1;
                }
                Some(true)
            }
            0x0D => Some(self.event(hwnd, Ev::Click(ID_PRINT))),
            _ => None,
        }
    }
}

/// Открыть окно печати. Возвращает строку для строки состояния.
pub fn run(owner: HWND, pages: Vec<Page>, cur: usize, view: Option<Rc<Img>>, name: &str) -> Option<String> {
    let printers = printers();
    let want = settings::get_s("printer").filter(|p| printers.contains(p)).or_else(default_printer);
    let pi = want.and_then(|w| printers.iter().position(|p| *p == w)).or(if printers.is_empty() { None } else { Some(0) });
    let mut d = PrintDlg {
        hwnd: owner,
        cur: cur.min(pages.len().saturating_sub(1)),
        pages,
        name: name.to_string(),
        printers,
        pi,
        dm: None,
        paper: settings::get_i("print_paper", 0).clamp(0, PAPERS.len() as i32 - 1) as usize,
        orient: settings::get_i("print_orient", 0).clamp(0, 2) as usize,
        fill: settings::get_i("print_fill", 0) == 1,
        nup: settings::get_i("print_nup", 0).clamp(0, NUPS.len() as i32 - 1) as usize,
        margin: settings::get_i("print_margin", 5).clamp(0, 50),
        copies: 1,
        all: true,
        autorot: settings::get_i("print_autorot", 1) == 1,
        ed: Edits::default(),
        sheet: 0,
        caps: HashMap::new(),
        thumbs: HashMap::new(),
        prepped: HashMap::new(),
        prev: RECT::default(),
        pbtn: RECT::default(),
        result: None,
        adj: HashMap::new(),
        sel: None,
        drag: None,
        hover: None,
        view: (0.0, 0.0, 1.0),
    };
    if let Some(v) = view {
        d.thumbs.insert(d.cur, Rc::new(codec::thumb(&v, 1000)));
    }
    d.reset_caps();
    // начнём предпросмотр с листа, где текущая страница
    d.sheet = d.cur / NUPS[d.nup];
    gui::run_modal(owner, d, (1080, 760)).result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_geometry() {
        let g = geom((10.0, 20.0, 40.0, 20.0), &Adj { dx: 5.0, dy: 0.0, scale: 2.0, angle: 90.0 });
        assert_eq!((g.cx, g.cy, g.w, g.h), (35.0, 30.0, 80.0, 40.0));
        // повёрнутое на 90° фото 80×40 занимает по вертикали ±40 мм от центра
        assert!(g.contains(35.0, 30.0 + 39.0));
        assert!(!g.contains(35.0 + 39.0, 30.0));
        let (x, y) = g.world(40.0, 0.0);
        assert!((x - 35.0).abs() < 1e-9 && (y - 70.0).abs() < 1e-9);
        assert_eq!(norm_angle(270.0), -90.0);
        assert_eq!(norm_angle(-180.0), 180.0);
        let r = fit_rect(2.0, (0.0, 0.0, 100.0, 100.0), false, false);
        assert_eq!(r, (0.0, 25.0, 100.0, 50.0));
        let img = Img::filled(400, 200, [0, 0, 0]);
        assert_eq!(fit_cell(&img, (0.0, 0.0, 100.0, 100.0), false, false).1, r);
    }

    #[test]
    fn compose_rotated() {
        let black = Img::filled(40, 20, [0, 0, 0]);
        let g = Geom { cx: 50.0, cy: 50.0, w: 40.0, h: 10.0, angle: 90.0 + 0.5 };
        let sheet = compose((100, 100), (1.0, 1.0), &[(&black, g)]);
        let px = |x: usize, y: usize| sheet.px[(y * 100 + x) * 4];
        assert_eq!(px(50, 50), 0);
        assert_eq!(px(50, 32), 0, "почти вертикальная полоса — сверху от центра закрашено");
        assert_eq!(px(32, 50), 255, "а слева — нет");
        assert_eq!(px(1, 1), 255);
        assert!(tilted(30.0) && !tilted(-90.0) && !tilted(180.0));
    }

    #[test]
    fn cells_layout() {
        let a4 = (210.0, 297.0);
        let c = sheet_cells(a4, 5.0, 1);
        assert_eq!(c, vec![(5.0, 5.0, 200.0, 287.0)]);
        let c = sheet_cells(a4, 5.0, 2);
        assert_eq!(c.len(), 2);
        assert!(c[0].2 > c[0].3, "на книжном листе 2 фото — альбомные ячейки");
        let c = sheet_cells((297.0, 210.0), 5.0, 6);
        assert_eq!(c.len(), 6);
        assert!((c[2].0 + c[2].2 - 292.0).abs() < 1e-9);
    }

    #[test]
    fn fit_and_fill() {
        let img = Img::filled(400, 300, [0, 0, 0]);
        let (im, r) = fit_cell(&img, (0.0, 0.0, 100.0, 100.0), false, true);
        assert_eq!((im.w, im.h), (400, 300));
        assert!((r.2 - 100.0).abs() < 1e-9 && (r.3 - 75.0).abs() < 1e-9 && (r.1 - 12.5).abs() < 1e-9);
        // автоповорот: широкое фото в высокой ячейке
        let (im, _) = fit_cell(&img, (0.0, 0.0, 100.0, 200.0), true, true);
        assert_eq!((im.w, im.h), (200, 400));
        let (im, _) = fit_cell(&img, (0.0, 0.0, 100.0, 200.0), true, false);
        assert_eq!((im.w, im.h), (150, 300));
    }
}
