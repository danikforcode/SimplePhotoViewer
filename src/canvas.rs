// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! «Холст» — как лист в Paint, только с фото-объектами: чистый холст нужного
//! формата, на нём фото (двигать, крутить, менять размер, обрезать рамкой,
//! порядок слоёв) и рисунок поверх (карандаш, фигуры, текст).
//! Коллаж — это тот же холст, где фото разложены шаблоном; после раскладки
//! каждое фото по-прежнему можно двигать и крутить («и так и сяк»).
//! Всё рисует системный GDI+ — одинаково на экране и в итоговой картинке.

use crate::collage::{self, Item, Layout, LAYOUTS};
use crate::draw::{self, Shape, Tool, COLORS, TOOLS};
use crate::gui::{self, Ev, Kind, Mouse, Panel, Ui, W};
use crate::image_buf::Img;
use crate::util::{file_name, rgb};
use crate::{codec, files, settings};
use std::path::PathBuf;
use std::rc::Rc;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::GdiPlus::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::*;
// явные имена важнее «звёздочек»: в GDI+ есть свой Ok, в windows::core — свой Result
use std::result::Result;
use std::result::Result::Ok;

// --------------------------------------------------------- форматы и цвета ---
/// Форматы холста: название, ширина, высота (точки; бумага — 300 dpi).
pub const FORMATS: [(&str, u32, u32); 10] = [
    ("16:9 (3840 × 2160)", 3840, 2160),
    ("Full HD (1920 × 1080)", 1920, 1080),
    ("4:3 (3200 × 2400)", 3200, 2400),
    ("Квадрат (3000 × 3000)", 3000, 3000),
    ("A4 альбомный", 3508, 2480),
    ("A4 книжный", 2480, 3508),
    ("10×15 альбомный", 1800, 1200),
    ("10×15 книжный", 1200, 1800),
    ("Сторис 9:16 (1080 × 1920)", 1080, 1920),
    ("Обложка (1500 × 500)", 1500, 500),
];

/// Фон холста (RGB).
pub const BACKS: [(&str, u32); 8] = [
    ("Белый", 0xFFFFFF),
    ("Светло-серый", 0xE8E8E8),
    ("Кремовый", 0xF5EEE0),
    ("Пудровый", 0xF6E3E1),
    ("Мятный", 0xE1F2EA),
    ("Небесный", 0xE0ECF8),
    ("Тёмно-серый", 0x303030),
    ("Чёрный", 0x000000),
];

/// Толщина линий — доля длинной стороны холста.
const WIDTHS: [f64; 5] = [0.0015, 0.003, 0.006, 0.012, 0.024];
/// Размер текста — доля высоты холста.
const TEXT_SIZES: [f64; 5] = [0.025, 0.04, 0.06, 0.09, 0.14];
/// Миниатюра фото для экрана (длинная сторона).
const PREVIEW: u32 = 1600;

// ------------------------------------------------------------ объекты ---
#[derive(Clone)]
struct Photo {
    path: Option<PathBuf>,
    /// Оригинал, если он в памяти (страница, вставка); иначе грузится при сборке.
    full: Option<Rc<Img>>,
    prev: Rc<Img>,
    /// Размер оригинала.
    src: (u32, u32),
    cx: f64,
    cy: f64,
    w: f64,
    h: f64,
    angle: f64,
    /// Сдвиг фото внутри рамки (−1…1 по каждой оси), если рамка обрезает фото.
    pan: (f64, f64),
}

impl Photo {
    fn aspect(&self) -> f64 {
        self.src.0 as f64 / self.src.1.max(1) as f64
    }
    /// Видимая часть фото (u0, v0, ширина, высота) в долях: рамка «заполняет» без искажений.
    fn crop(&self) -> (f64, f64, f64, f64) {
        let (sa, fa) = (self.aspect(), self.w / self.h.max(1e-9));
        let (cw, ch) = if fa >= sa { (1.0, sa / fa) } else { (fa / sa, 1.0) };
        let u0 = (1.0 - cw) / 2.0 * (1.0 + self.pan.0.clamp(-1.0, 1.0));
        let v0 = (1.0 - ch) / 2.0 * (1.0 + self.pan.1.clamp(-1.0, 1.0));
        (u0, v0, cw, ch)
    }
    fn local(&self, x: f64, y: f64) -> (f64, f64) {
        let a = (-self.angle).to_radians();
        let (dx, dy) = (x - self.cx, y - self.cy);
        (dx * a.cos() - dy * a.sin(), dx * a.sin() + dy * a.cos())
    }
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
    /// Вписать фото целиком в прямоугольник (x, y, w, h).
    fn fit_into(&mut self, c: (f64, f64, f64, f64)) {
        let a = self.aspect();
        let (w, h) = if c.2 / c.3.max(1e-9) > a { (c.3 * a, c.3) } else { (c.2, c.2 / a) };
        self.cx = c.0 + c.2 / 2.0;
        self.cy = c.1 + c.3 / 2.0;
        self.w = w;
        self.h = h;
        self.pan = (0.0, 0.0);
    }
}

#[derive(Clone)]
enum Obj {
    Photo(Photo),
    Ink(Shape),
}

// ---------------------------------------------------------- состояние ---
#[derive(Clone, Copy, PartialEq)]
enum Handle {
    Corner(usize),
    Side(usize), // 0 — верх, 1 — право, 2 — низ, 3 — лево
    Rotate,
}

enum Drag {
    None,
    Move { start: (f64, f64), orig: Photo },
    Pan { start: (f64, f64), orig: Photo },
    Handle { h: Handle, orig: Photo },
    Ink,
    Erase,
}

struct TextBox {
    hwnd: HWND,
    at: (f32, f32),
    size: f32,
    font: HFONT,
}

const ID_FMT_PREV: u32 = 1;
const ID_FMT_NEXT: u32 = 2;
const ID_FMT_OWN: u32 = 3;
const ID_BACK0: u32 = 10;
const ID_TOOL0: u32 = 30; // 30 — «Выбор», дальше TOOLS
const ID_COLOR0: u32 = 50;
const ID_W_DEC: u32 = 70;
const ID_W_INC: u32 = 71;
const ID_FILL: u32 = 72;
const ID_ADD: u32 = 80;
const ID_LAYOUT0: u32 = 90;
const ID_GAP_DEC: u32 = 100;
const ID_GAP_INC: u32 = 101;
const ID_MODE_FILL: u32 = 102;
const ID_MODE_FIT: u32 = 103;
const ID_FRONT: u32 = 110;
const ID_BACKWARD: u32 = 111;
const ID_ROT90: u32 = 112;
const ID_STRAIGHT: u32 = 113;
const ID_DUP: u32 = 114;
const ID_DEL: u32 = 115;
const ID_UNDO: u32 = 120;
const ID_OK: u32 = 130;
const ID_SAVE: u32 = 131;
const ID_CANCEL: u32 = 132;
const ID_TEXT: u32 = 1201;

struct Canvas {
    size: (u32, u32),
    fmt: Option<usize>,
    back: u32,
    objs: Vec<Obj>,
    sel: Option<usize>,
    undo: Vec<(Vec<Obj>, (u32, u32), u32)>,
    /// None — «Выбор», иначе инструмент рисования.
    tool: Option<Tool>,
    color: u32,
    width_i: usize,
    fill: bool,
    gap: i32,
    fit_mode: bool,
    layout: Option<Layout>,
    drag: Drag,
    cur: Option<Shape>,
    text: Option<TextBox>,
    view: RECT,
    hover: Option<Handle>,
    hwnd: HWND,
    changed: bool,
    result: Option<Img>,
    status: String,
}

fn ctrl() -> bool {
    unsafe { GetKeyState(0x11) < 0 }
}
fn shift() -> bool {
    unsafe { GetKeyState(0x10) < 0 }
}

fn load_photo(it: Item) -> Result<Photo, String> {
    let (full, prev, src) = match (&it.img, &it.path) {
        (Some(i), _) => (Some(i.clone()), Rc::new(codec::thumb(i, PREVIEW)), (i.w, i.h)),
        (None, Some(p)) => {
            let i = codec::load(p).map_err(|e| format!("{}: {}", file_name(p), e.message()))?;
            (None, Rc::new(codec::thumb(&i, PREVIEW)), (i.w, i.h))
        }
        _ => return Err("пустой элемент".into()),
    };
    Ok(Photo { path: it.path, full, prev, src, cx: 0.0, cy: 0.0, w: 1.0, h: 1.0, angle: 0.0, pan: (0.0, 0.0) })
}

impl Canvas {
    fn k(&self) -> f64 {
        let (vw, vh) = ((self.view.right - self.view.left) as f64, (self.view.bottom - self.view.top) as f64);
        (vw / self.size.0 as f64).min(vh / self.size.1 as f64).max(1e-6)
    }
    /// Холст на экране.
    fn sheet(&self) -> RECT {
        let k = self.k();
        let (w, h) = ((self.size.0 as f64 * k).round() as i32, (self.size.1 as f64 * k).round() as i32);
        let x = (self.view.left + self.view.right - w) / 2;
        let y = (self.view.top + self.view.bottom - h) / 2;
        gui::r(x, y, w, h)
    }
    fn to_canvas(&self, x: i32, y: i32) -> (f64, f64) {
        let s = self.sheet();
        let k = self.k();
        ((x - s.left) as f64 / k, (y - s.top) as f64 / k)
    }
    fn to_screen(&self, p: (f64, f64)) -> (i32, i32) {
        let s = self.sheet();
        let k = self.k();
        (s.left + (p.0 * k).round() as i32, s.top + (p.1 * k).round() as i32)
    }

    fn photo(&self, i: usize) -> Option<&Photo> {
        match self.objs.get(i) {
            Some(Obj::Photo(p)) => Some(p),
            _ => None,
        }
    }
    fn photo_mut(&mut self, i: usize) -> Option<&mut Photo> {
        match self.objs.get_mut(i) {
            Some(Obj::Photo(p)) => Some(p),
            _ => None,
        }
    }
    fn sel_photo(&self) -> Option<&Photo> {
        self.sel.and_then(|i| self.photo(i))
    }
    fn photo_count(&self) -> usize {
        self.objs.iter().filter(|o| matches!(o, Obj::Photo(_))).count()
    }

    fn push_undo(&mut self) {
        self.undo.push((self.objs.clone(), self.size, self.back));
        if self.undo.len() > 60 {
            self.undo.remove(0);
        }
        self.changed = true;
    }
    fn do_undo(&mut self) {
        if let Some((o, s, b)) = self.undo.pop() {
            self.objs = o;
            self.size = s;
            self.back = b;
            self.fmt = FORMATS.iter().position(|f| (f.1, f.2) == s);
            self.sel = self.sel.filter(|&i| self.photo(i).is_some());
        }
    }

    fn line_width(&self) -> f32 {
        (WIDTHS[self.width_i] * self.size.0.max(self.size.1) as f64).max(1.0) as f32
    }
    fn text_size(&self) -> f32 {
        (TEXT_SIZES[self.width_i] * self.size.1 as f64).max(8.0) as f32
    }

    // ---------------------------------------------------- фото и шаблоны ---
    fn add_items(&mut self, items: Vec<Item>) {
        let mut errs = vec![];
        let mut added = vec![];
        for it in items {
            match load_photo(it) {
                Ok(p) => added.push(p),
                Err(e) => errs.push(e),
            }
        }
        if !added.is_empty() {
            self.push_undo();
            let n = added.len();
            let (cw, ch) = (self.size.0 as f64, self.size.1 as f64);
            for (k, mut p) in added.into_iter().enumerate() {
                // новое фото — по центру, «лесенкой», на треть холста
                let off = (k as f64 - (n as f64 - 1.0) / 2.0) * cw.min(ch) * 0.06;
                p.fit_into((cw * 0.3 + off, ch * 0.3 + off, cw * 0.4, ch * 0.4));
                self.objs.push(Obj::Photo(p));
            }
            self.sel = Some(self.objs.len() - 1);
            self.tool = None;
        }
        if !errs.is_empty() {
            crate::ui::message(Some(self.hwnd), &format!("Не удалось открыть:\n\n{}", errs.join("\n")), true);
        }
    }

    /// Разложить все фото шаблоном (дальше их можно двигать как угодно).
    fn apply_layout(&mut self, l: Layout) {
        let idx: Vec<usize> = (0..self.objs.len()).filter(|&i| self.photo(i).is_some()).collect();
        if idx.is_empty() {
            self.layout = Some(l);
            return;
        }
        self.push_undo();
        self.layout = Some(l);
        let aspects: Vec<f64> = idx.iter().map(|&i| self.photo(i).unwrap().aspect()).collect();
        let (w, h) = (self.size.0 as f64, self.size.1 as f64);
        let gap = w.min(h) * self.gap as f64 / 200.0;
        let cells = collage::cells(l, &aspects, w, h, gap);
        let fit = self.fit_mode;
        for (&i, c) in idx.iter().zip(cells.iter()) {
            let p = self.photo_mut(i).unwrap();
            p.angle = 0.0;
            if fit {
                p.fit_into(*c);
            } else {
                p.cx = c.0 + c.2 / 2.0;
                p.cy = c.1 + c.3 / 2.0;
                p.w = c.2;
                p.h = c.3;
                p.pan = (0.0, 0.0);
            }
        }
    }

    fn set_size(&mut self, s: (u32, u32)) {
        if s == self.size {
            return;
        }
        self.push_undo();
        // всё на холсте — пропорционально новому размеру
        let (kx, ky) = (s.0 as f64 / self.size.0 as f64, s.1 as f64 / self.size.1 as f64);
        let k = kx.min(ky);
        for o in &mut self.objs {
            match o {
                Obj::Photo(p) => {
                    p.cx *= kx;
                    p.cy *= ky;
                    p.w *= k;
                    p.h *= k;
                }
                Obj::Ink(sh) => {
                    for pt in &mut sh.pts {
                        pt.0 *= kx as f32;
                        pt.1 *= ky as f32;
                    }
                    sh.width *= k as f32;
                }
            }
        }
        self.size = s;
        self.fmt = FORMATS.iter().position(|f| (f.1, f.2) == s);
    }

    // ------------------------------------------------------------ текст ---
    fn start_text(&mut self, at: (f64, f64), sx: i32, sy: i32) {
        self.commit_text();
        let size = self.text_size();
        let px = (size as f64 * self.k()).round().max(8.0) as i32;
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap_or_default();
            let style = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(0x0004 | 0x0040 | 0x0080);
            let s = self.sheet();
            let w = (s.right - sx - 4).clamp(120, (px * 14).max(220));
            let Ok(h) = CreateWindowExW(WINDOW_EX_STYLE(0), w!("EDIT"), w!(""), style, sx, sy, w, px * 2 + px / 2, self.hwnd, HMENU(ID_TEXT as usize as *mut _), hinst, None) else { return };
            let face = crate::util::wide("Segoe UI");
            let font = CreateFontW(-px, 0, 0, 0, 700, 0, 0, 0, DEFAULT_CHARSET.0 as u32, 0, 0, CLEARTYPE_QUALITY.0 as u32, 0, PCWSTR(face.as_ptr()));
            SendMessageW(h, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
            let _ = SetFocus(h);
            self.text = Some(TextBox { hwnd: h, at: (at.0 as f32, at.1 as f32), size, font });
        }
    }

    fn commit_text(&mut self) {
        let Some(t) = self.text.take() else { return };
        let text = unsafe {
            let n = GetWindowTextLengthW(t.hwnd);
            let mut buf = vec![0u16; n as usize + 1];
            let k = GetWindowTextW(t.hwnd, &mut buf);
            let _ = DestroyWindow(t.hwnd);
            let _ = DeleteObject(t.font);
            let _ = SetFocus(self.hwnd);
            String::from_utf16_lossy(&buf[..k as usize]).replace("\r\n", "\n")
        };
        if text.trim().is_empty() {
            return;
        }
        self.push_undo();
        let mut s = Shape::new(Tool::Text, t.at, self.color, t.size, 255, false);
        s.text = text;
        self.objs.push(Obj::Ink(s));
    }

    // ------------------------------------------------------------ сборка ---
    /// Итоговая картинка в полном размере холста.
    fn build(&self) -> Result<Img, String> {
        let b = self.back;
        let mut out = Img::filled(self.size.0, self.size.1, [(b & 0xFF) as u8, (b >> 8 & 0xFF) as u8, (b >> 16) as u8]);
        if !draw::startup() {
            return Err("GDI+ недоступен".into());
        }
        unsafe {
            let mut bmp = std::ptr::null_mut();
            if GdipCreateBitmapFromScan0(out.w as i32, out.h as i32, out.w as i32 * 4, 0x0002_2009, Some(out.px.as_mut_ptr()), &mut bmp) != Status(0) {
                return Err("не хватает памяти для холста".into());
            }
            let mut g = std::ptr::null_mut();
            if GdipGetImageGraphicsContext(bmp as *mut GpImage, &mut g) != Status(0) {
                let _ = GdipDisposeImage(bmp as *mut GpImage);
                return Err("не хватает памяти для холста".into());
            }
            let _ = GdipSetInterpolationMode(g, InterpolationModeHighQualityBicubic);
            let _ = GdipSetPixelOffsetMode(g, PixelOffsetModeHalf);
            let _ = GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
            let mut err = None;
            for o in &self.objs {
                match o {
                    Obj::Photo(p) => {
                        // оригинал: из памяти или с диска; заранее уменьшить до нужного (WIC — качественно)
                        let full = match (&p.full, &p.path) {
                            (Some(f), _) => f.clone(),
                            (None, Some(path)) => match codec::load(path) {
                                Ok(i) => Rc::new(i),
                                Err(e) => {
                                    err = Some(format!("{}: {}", file_name(path), e.message()));
                                    p.prev.clone()
                                }
                            },
                            _ => p.prev.clone(),
                        };
                        let (_, _, cw, _) = p.crop();
                        let need_w = p.w / cw.max(1e-6);
                        let img = if (full.w as f64) > need_w * 1.4 && need_w >= 1.0 {
                            let nh = need_w / p.aspect();
                            Rc::new(codec::resize(&full, need_w.round().max(1.0) as u32, nh.round().max(1.0) as u32).unwrap_or_else(|_| (*full).clone()))
                        } else {
                            full
                        };
                        draw_photo(g, p, &img, (0.0, 0.0), 1.0);
                    }
                    Obj::Ink(s) => draw::render(g, std::slice::from_ref(s)),
                }
            }
            let _ = GdipDeleteGraphics(g);
            let _ = GdipDisposeImage(bmp as *mut GpImage);
            if let Some(e) = err {
                return Err(e);
            }
        }
        for p in out.px.chunks_exact_mut(4) {
            p[3] = 255;
        }
        Ok(out)
    }

    // ------------------------------------------------------------ мышь ---
    fn handle_at(&self, x: i32, y: i32, ui_r: i32) -> Option<Handle> {
        let p = self.sel_photo()?;
        let d = |q: (f64, f64)| {
            let s = self.to_screen(q);
            (((s.0 - x).pow(2) + (s.1 - y).pow(2)) as f64).sqrt() <= ui_r as f64
        };
        if d(self.rot_handle(p)) {
            return Some(Handle::Rotate);
        }
        let c = p.corners();
        for (i, q) in c.iter().enumerate() {
            if d(*q) {
                return Some(Handle::Corner(i));
            }
        }
        let (w, h) = (p.w / 2.0, p.h / 2.0);
        let sides = [p.world(0.0, -h), p.world(w, 0.0), p.world(0.0, h), p.world(-w, 0.0)];
        for (i, q) in sides.iter().enumerate() {
            if d(*q) {
                return Some(Handle::Side(i));
            }
        }
        None
    }
    fn rot_handle(&self, p: &Photo) -> (f64, f64) {
        p.world(0.0, -p.h / 2.0 - 30.0 / self.k())
    }

    fn drag_handle(&mut self, h: Handle, orig: &Photo, c: (f64, f64)) {
        let snap = shift();
        let Some(i) = self.sel else { return };
        let k = self.k();
        let Some(p) = self.photo_mut(i) else { return };
        match h {
            Handle::Rotate => {
                let mut a = (c.1 - orig.cy).atan2(c.0 - orig.cx).to_degrees() + 90.0;
                if snap {
                    a = (a / 15.0).round() * 15.0;
                } else {
                    // «примагнитить» к прямым углам
                    let q = (a / 90.0).round() * 90.0;
                    if (a - q).abs() < 3.0 {
                        a = q;
                    }
                }
                p.angle = (a + 540.0).rem_euclid(360.0) - 180.0;
            }
            Handle::Corner(_) => {
                // пропорционально, от центра
                let d0 = ((orig.w / 2.0).powi(2) + (orig.h / 2.0).powi(2)).sqrt();
                let d1 = ((c.0 - orig.cx).powi(2) + (c.1 - orig.cy).powi(2)).sqrt();
                let s = (d1 / d0.max(1e-6)).max(12.0 / k / orig.w.min(orig.h).max(1e-6));
                p.w = orig.w * s;
                p.h = orig.h * s;
            }
            Handle::Side(side) => {
                // край рамки — обрезка (фото не растягивается)
                let (u, v) = orig.local(c.0, c.1);
                let min = 12.0 / k;
                if side % 2 == 1 {
                    p.w = (2.0 * u.abs()).max(min);
                } else {
                    p.h = (2.0 * v.abs()).max(min);
                }
            }
        }
    }

    fn hit_photo(&self, c: (f64, f64)) -> Option<usize> {
        (0..self.objs.len()).rev().find(|&i| self.photo(i).map(|p| p.contains(c.0, c.1)).unwrap_or(false))
    }

    fn erase_at(&mut self, c: (f64, f64)) -> bool {
        let r = (6.0 / self.k()) as f32;
        let before = self.objs.len();
        self.objs.retain(|o| !matches!(o, Obj::Ink(s) if s.hit((c.0 as f32, c.1 as f32), r)));
        if let Some(s) = self.sel {
            if s >= self.objs.len() {
                self.sel = None;
            }
        }
        before != self.objs.len()
    }

    fn reorder(&mut self, front: bool) {
        let Some(i) = self.sel else { return };
        let j = if front { i + 1 } else { i.wrapping_sub(1) };
        if j >= self.objs.len() {
            return;
        }
        self.push_undo();
        self.objs.swap(i, j);
        self.sel = Some(j);
    }
}

/// Нарисовать фото с поворотом и обрезкой рамкой: три угла назначения — GDI+ сам повернёт.
/// off, k — перевод точек холста в точки устройства (на экране; при сборке — 0 и 1).
unsafe fn draw_photo(g: *mut GpGraphics, p: &Photo, img: &Img, off: (f64, f64), k: f64) {
    let mut bmp = std::ptr::null_mut();
    if GdipCreateBitmapFromScan0(img.w as i32, img.h as i32, img.w as i32 * 4, 0x0002_2009, Some(img.px.as_ptr() as *mut u8), &mut bmp) != Status(0) {
        return;
    }
    let (u0, v0, cw, ch) = p.crop();
    let (w, h) = (p.w / 2.0, p.h / 2.0);
    let pt = |q: (f64, f64)| PointF { X: (off.0 + q.0 * k) as f32, Y: (off.1 + q.1 * k) as f32 };
    let pts = [pt(p.world(-w, -h)), pt(p.world(w, -h)), pt(p.world(-w, h))];
    let mut attr = std::ptr::null_mut();
    let _ = GdipCreateImageAttributes(&mut attr);
    // без тёмной каймы по краям при сглаживании
    let _ = GdipSetImageAttributesWrapMode(attr, WrapModeTileFlipXY, 0, FALSE);
    let (iw, ih) = (img.w as f64, img.h as f64);
    let _ = GdipDrawImagePointsRect(g, bmp as *mut GpImage, pts.as_ptr(), 3, (u0 * iw) as f32, (v0 * ih) as f32, (cw * iw) as f32, (ch * ih) as f32, UnitPixel, attr, 0, std::ptr::null_mut());
    let _ = GdipDisposeImageAttributes(attr);
    let _ = GdipDisposeImage(bmp as *mut GpImage);
}

impl Panel for Canvas {
    fn title(&self) -> String {
        "Холст".into()
    }

    fn init(&mut self, hwnd: HWND) {
        self.hwnd = hwnd;
        unsafe { windows::Win32::UI::Shell::DragAcceptFiles(hwnd, true) };
    }

    fn min_size(&self) -> (i32, i32) {
        (940, 700)
    }

    fn layout(&mut self, ui: &Ui, w: i32, h: i32) -> Vec<W> {
        let m = ui.s(14);
        let pw = ui.s(312);
        let px = w - m - pw;
        let mut v = vec![];
        let mut y = m;
        let grp = |v: &mut Vec<W>, y: &mut i32, t: &str| {
            v.push(W::new(Kind::Group, 0, gui::r(px, *y, pw, ui.s(20))).t(t));
            *y += ui.s(24);
        };
        // холст
        grp(&mut v, &mut y, "Холст");
        let name = match self.fmt {
            Some(i) => FORMATS[i].0.to_string(),
            None => format!("Свой ({} × {})", self.size.0, self.size.1),
        };
        let sw = pw - ui.s(96);
        v.extend(gui::stepper(ui, px, y, sw, name, ID_FMT_PREV, ID_FMT_NEXT, "Формат холста (колесо мыши — листать)"));
        let k = v.len();
        v[k - 3].text = "‹".into();
        v[k - 1].text = "›".into();
        v.push(W::new(Kind::Button, ID_FMT_OWN, gui::r(px + sw + ui.s(6), y, ui.s(90), ui.s(34))).t("Свой…").tip("Свой размер в точках"));
        y += ui.s(40);
        let sz = ui.s(26);
        for (i, (n, c)) in BACKS.iter().enumerate() {
            let c = *c;
            v.push(W::new(Kind::Swatch, ID_BACK0 + i as u32, gui::r(px + i as i32 * (sz + ui.s(6)), y, sz, sz)).color(rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)).on(self.back == c).tip(format!("Фон: {n}")));
        }
        y += sz + ui.s(12);

        // инструменты
        grp(&mut v, &mut y, "Инструмент");
        let names = ["Выбор", "Карандаш", "Маркер", "Ластик", "Линия", "Стрелка", "Рамка", "Овал", "Текст"];
        let cur = match self.tool {
            None => 0,
            Some(t) => 1 + TOOLS.iter().position(|x| x.0 == t).unwrap_or(0),
        };
        for row in 0..3 {
            let base = row * 3;
            let mut seg = gui::segments(ui, px, y, pw, ID_TOOL0 + base as u32, &names[base..base + 3], cur.wrapping_sub(base));
            for (j, s) in seg.iter_mut().enumerate() {
                s.tip = if base + j == 0 { "Двигать, крутить, менять размер фото".into() } else { TOOLS[base + j - 1].2.into() };
            }
            v.extend(seg);
            y += ui.s(36);
        }
        y += ui.s(4);
        let cs = ui.s(24);
        let gapc = (pw - cs * COLORS.len() as i32) / (COLORS.len() as i32 - 1);
        for (i, c) in COLORS.iter().enumerate() {
            let c = *c;
            v.push(W::new(Kind::Swatch, ID_COLOR0 + i as u32, gui::r(px + i as i32 * (cs + gapc), y, cs, cs)).color(rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)).on(self.color == c).en(self.tool.is_some()));
        }
        y += cs + ui.s(8);
        let half = (pw - ui.s(8)) / 2;
        let wt = if self.tool == Some(Tool::Text) { format!("Текст {}", self.width_i + 1) } else { format!("Толщина {}", self.width_i + 1) };
        let mut st = gui::stepper(ui, px, y, half, wt, ID_W_DEC, ID_W_INC, "Толщина линии / размер текста");
        for s in st.iter_mut() {
            s.enabled = self.tool.is_some();
        }
        v.extend(st);
        v.push(W::new(Kind::Check, ID_FILL, gui::r(px + half + ui.s(8), y, half, ui.s(34))).t("Заливка").on(self.fill).en(matches!(self.tool, Some(Tool::Rect | Tool::Ellipse))).tip("Рамку и овал — залить цветом"));
        y += ui.s(44);

        // фото и шаблоны
        grp(&mut v, &mut y, &format!("Фото: {}", self.photo_count()));
        v.push(W::new(Kind::Button, ID_ADD, gui::r(px, y, pw, ui.s(34))).i(gui::IC_ADD).t("Добавить фото…").tip("Или перетащите файлы в окно"));
        y += ui.s(42);
        let lnames: Vec<&str> = LAYOUTS.iter().map(|l| l.1).collect();
        let lcur = self.layout.and_then(|l| LAYOUTS.iter().position(|x| x.0 == l)).unwrap_or(usize::MAX);
        let mut s1 = gui::segments(ui, px, y, pw, ID_LAYOUT0, &lnames[..3], lcur);
        let mut s2 = gui::segments(ui, px, y + ui.s(36), pw * 2 / 3, ID_LAYOUT0 + 3, &lnames[3..], lcur.wrapping_sub(3));
        let n = self.photo_count();
        for s in s1.iter_mut().chain(s2.iter_mut()) {
            s.enabled = n > 0;
            s.tip = "Разложить все фото шаблоном (потом их можно двигать)".into();
        }
        v.append(&mut s1);
        v.append(&mut s2);
        y += ui.s(78);
        let gtxt = if self.gap == 0 { "Отступ: нет".to_string() } else { format!("Отступ {}", self.gap) };
        v.extend(gui::stepper(ui, px, y, half, gtxt, ID_GAP_DEC, ID_GAP_INC, "Отступы между фото в шаблоне"));
        v.extend(gui::segments(ui, px + half + ui.s(8), y, half, ID_MODE_FILL, &["Заполнить", "Вписать"], if self.fit_mode { 1 } else { 0 }));
        y += ui.s(46);

        // выбранное фото
        if self.sel_photo().is_some() {
            grp(&mut v, &mut y, "Выбранное фото");
            let tb = ui.s(34);
            let gp = (pw - 6 * tb) / 5;
            for (j, (id, ic, tip)) in [
                (ID_FRONT, gui::IC_UP, "Выше других (Page Up)"),
                (ID_BACKWARD, gui::IC_DOWN, "Ниже других (Page Down)"),
                (ID_ROT90, gui::IC_ROT_R, "Повернуть на 90°"),
                (ID_STRAIGHT, gui::IC_RESET, "Ровно: без наклона, без обрезки"),
                (ID_DUP, gui::IC_COPY, "Копия (Ctrl+D)"),
                (ID_DEL, gui::IC_DELETE, "Убрать с холста (Delete)"),
            ]
            .into_iter()
            .enumerate()
            {
                v.push(W::new(Kind::Button, id, gui::r(px + j as i32 * (tb + gp), y, tb, tb)).i(ic).tip(tip));
            }
        }

        // холст на экране
        self.view = gui::r(m + ui.s(10), m + ui.s(10), px - 2 * m - ui.s(20), h - 2 * m - ui.s(56) - ui.s(20));
        // низ
        let by = h - m - ui.s(36);
        v.push(W::new(Kind::Button, ID_UNDO, gui::r(m, by, ui.s(36), ui.s(36))).i(gui::IC_UNDO).tip("Отменить (Ctrl+Z)").en(!self.undo.is_empty()));
        let hint = if !self.status.is_empty() {
            self.status.clone()
        } else {
            match self.tool {
                None if self.sel_photo().is_some() => "Тяните фото; углы — размер, края — обрезка, кружок — поворот (Shift — по 15°), Ctrl — сдвиг внутри рамки".into(),
                None if self.photo_count() > 0 => format!("Холст {} × {} точек · щёлкните фото — двигать, крутить, обрезать; шаблон справа — разложить заново", self.size.0, self.size.1),
                None => format!("Холст {} × {} точек — добавьте фото или выберите инструмент", self.size.0, self.size.1),
                Some(Tool::Eraser) => "Ластик стирает нарисованное целиком (фото не трогает)".into(),
                Some(Tool::Text) => "Щёлкните, где написать; Esc или щелчок мимо — готово".into(),
                Some(_) => "Рисуйте поверх; Shift — ровно (45°, квадрат, круг)".into(),
            }
        };
        let mut x = w - m;
        let ok = !self.objs.is_empty();
        let mut btns = vec![];
        for (id, t, primary, en, bw) in [(ID_OK, "Готово", true, ok, 110), (ID_SAVE, "Сохранить как…", false, ok, 150), (ID_CANCEL, "Отмена", false, true, 96)] {
            let bw = ui.s(bw);
            x -= bw;
            btns.push(W::new(Kind::Button, id, gui::r(x, by, bw, ui.s(36))).t(t).on(primary).en(en));
            x -= ui.s(8);
        }
        v.push(W::new(Kind::Label, 0, gui::r(m + ui.s(46), by, x - m - ui.s(54), ui.s(36))).t(hint).color(ui.t.dim));
        v.extend(btns);
        v
    }

    fn paint(&mut self, ui: &Ui, hdc: HDC, w: i32, h: i32) {
        let t = &ui.t;
        gui::fill(hdc, &gui::r(0, 0, w, h), t.bg);
        let vr = RECT { left: self.view.left - ui.s(10), top: self.view.top - ui.s(10), right: self.view.right + ui.s(10), bottom: self.view.bottom + ui.s(10) };
        gui::round(hdc, &vr, ui.s(10), Some(t.canvas), None);
        let s = self.sheet();
        gui::fill(hdc, &RECT { left: s.left + ui.s(3), top: s.top + ui.s(3), right: s.right + ui.s(3), bottom: s.bottom + ui.s(3) }, gui::blend(t.canvas, rgb(0, 0, 0), 0.7));
        let b = self.back;
        gui::fill(hdc, &s, rgb((b >> 16) as u8, (b >> 8) as u8, b as u8));
        let k = self.k();
        if draw::startup() {
            unsafe {
                let mut g = std::ptr::null_mut();
                if GdipCreateFromHDC(hdc, &mut g) == Status(0) {
                    let _ = GdipSetClipRectI(g, s.left, s.top, s.right - s.left, s.bottom - s.top, CombineModeReplace);
                    // в точки экрана пересчитываем сами (а не преобразованием GDI+):
                    // толщина линий тогда одинакова в любой реализации GDI+
                    let off = (s.left as f64, s.top as f64);
                    let scr = |sh: &Shape| {
                        let mut c = sh.clone();
                        for p in &mut c.pts {
                            *p = ((off.0 + p.0 as f64 * k) as f32, (off.1 + p.1 as f64 * k) as f32);
                        }
                        c.width = (c.width as f64 * k) as f32;
                        c
                    };
                    let moving = !matches!(self.drag, Drag::None);
                    let _ = GdipSetInterpolationMode(g, if moving { InterpolationModeBilinear } else { InterpolationModeHighQualityBilinear });
                    let _ = GdipSetPixelOffsetMode(g, PixelOffsetModeHalf);
                    for o in &self.objs {
                        match o {
                            Obj::Photo(p) => draw_photo(g, p, &p.prev, off, k),
                            Obj::Ink(sh) => draw::render(g, &[scr(sh)]),
                        }
                    }
                    if let Some(c) = &self.cur {
                        draw::render(g, &[scr(c)]);
                    }
                    let _ = GdipDeleteGraphics(g);
                }
            }
        }
        if self.objs.is_empty() && self.tool.is_none() {
            let c = if self.back < 0x808080 { rgb(200, 200, 200) } else { rgb(120, 120, 120) };
            gui::text(hdc, "Пустой холст\nДобавьте фото (или перетащите их сюда), выберите шаблон —\nили просто рисуйте", &RECT { top: (s.top + s.bottom) / 2 - ui.s(30), ..s }, ui.f.text, c, gui::WRAP);
        }
        // рамка выбранного фото и «ручки»
        if let (Some(p), None) = (self.sel_photo(), self.tool) {
            let c = p.corners();
            let pts: Vec<POINT> = c.iter().chain(std::iter::once(&c[0])).map(|q| {
                let s = self.to_screen(*q);
                POINT { x: s.0, y: s.1 }
            }).collect();
            unsafe {
                let pen = CreatePen(PS_SOLID, ui.s(2), t.accent);
                let old = SelectObject(hdc, pen);
                let _ = Polyline(hdc, &pts);
                let top = self.to_screen(p.world(0.0, -p.h / 2.0));
                let rh = self.to_screen(self.rot_handle(p));
                let _ = MoveToEx(hdc, top.0, top.1, None);
                let _ = LineTo(hdc, rh.0, rh.1);
                SelectObject(hdc, old);
                let _ = DeleteObject(pen);
            }
            let hs = ui.s(5);
            let (w2, h2) = (p.w / 2.0, p.h / 2.0);
            let sides = [p.world(0.0, -h2), p.world(w2, 0.0), p.world(0.0, h2), p.world(-w2, 0.0)];
            for q in c.iter().chain(sides.iter()) {
                let s = self.to_screen(*q);
                let rc = gui::r(s.0 - hs, s.1 - hs, 2 * hs, 2 * hs);
                gui::fill(hdc, &rc, rgb(255, 255, 255));
                gui::frame(hdc, &rc, t.accent, PS_SOLID);
            }
            let rh = self.to_screen(self.rot_handle(p));
            gui::round(hdc, &gui::r(rh.0 - hs - 1, rh.1 - hs - 1, 2 * hs + 2, 2 * hs + 2), hs + 1, Some(rgb(255, 255, 255)), Some(t.accent));
        }
    }

    fn cursor(&self, x: i32, y: i32) -> Option<PCWSTR> {
        let s = self.sheet();
        let inside = x >= s.left && x < s.right && y >= s.top && y < s.bottom;
        match self.tool {
            Some(Tool::Text) if inside => Some(IDC_IBEAM),
            Some(_) if inside => Some(IDC_CROSS),
            None => match self.hover {
                Some(Handle::Rotate) => Some(IDC_HAND),
                Some(Handle::Corner(i)) => Some(if i % 2 == 0 { IDC_SIZENWSE } else { IDC_SIZENESW }),
                Some(Handle::Side(i)) => Some(if i % 2 == 0 { IDC_SIZENS } else { IDC_SIZEWE }),
                None if self.hit_photo(self.to_canvas(x, y)).is_some() => Some(IDC_SIZEALL),
                None => None,
            },
            _ => None,
        }
    }

    fn mouse(&mut self, hwnd: HWND, m: Mouse, x: i32, y: i32) -> bool {
        let c = self.to_canvas(x, y);
        let s = self.sheet();
        let inside = x >= s.left && x < s.right && y >= s.top && y < s.bottom;
        let grab = 9;
        match m {
            Mouse::Down => {
                self.status.clear();
                if self.text.is_some() {
                    self.commit_text();
                    return true;
                }
                match self.tool {
                    None => {
                        let r = (grab as f64 * (self.view.right - self.view.left).max(1) as f64 / 900.0).clamp(7.0, 14.0) as i32;
                        if let Some(h) = self.handle_at(x, y, r) {
                            let orig = self.sel_photo().unwrap().clone();
                            self.push_undo();
                            self.drag = Drag::Handle { h, orig };
                            return true;
                        }
                        self.sel = self.hit_photo(c);
                        if let Some(p) = self.sel_photo().cloned() {
                            self.push_undo();
                            self.drag = if ctrl() { Drag::Pan { start: c, orig: p } } else { Drag::Move { start: c, orig: p } };
                        }
                        true
                    }
                    Some(Tool::Text) if inside => {
                        self.start_text(c, x, y);
                        true
                    }
                    Some(Tool::Eraser) if inside => {
                        self.push_undo();
                        if !self.erase_at(c) {
                            self.undo.pop();
                        }
                        self.drag = Drag::Erase;
                        true
                    }
                    Some(t) if inside => {
                        self.cur = Some(Shape::new(t, (c.0 as f32, c.1 as f32), self.color, self.line_width(), 255, self.fill));
                        self.drag = Drag::Ink;
                        true
                    }
                    _ => false,
                }
            }
            Mouse::Move => match std::mem::replace(&mut self.drag, Drag::None) {
                Drag::None => {
                    if self.tool.is_none() {
                        let h = self.handle_at(x, y, 9);
                        let ch = h != self.hover;
                        self.hover = h;
                        return ch;
                    }
                    false
                }
                Drag::Move { start, orig } => {
                    if let Some(p) = self.sel.and_then(|i| self.photo_mut(i)) {
                        p.cx = orig.cx + c.0 - start.0;
                        p.cy = orig.cy + c.1 - start.1;
                    }
                    self.drag = Drag::Move { start, orig };
                    true
                }
                Drag::Pan { start, orig } => {
                    // сдвиг фото внутри рамки: в долях свободного места
                    let (_, _, cw, ch) = orig.crop();
                    let (du, dv) = {
                        let (a, b) = orig.local(c.0, c.1);
                        let (a0, b0) = orig.local(start.0, start.1);
                        (a - a0, b - b0)
                    };
                    if let Some(p) = self.sel.and_then(|i| self.photo_mut(i)) {
                        let fw = orig.w / cw.max(1e-6) - orig.w; // невидимая часть по ширине, в точках холста
                        let fh = orig.h / ch.max(1e-6) - orig.h;
                        p.pan.0 = if fw > 0.5 { (orig.pan.0 - 2.0 * du / fw).clamp(-1.0, 1.0) } else { 0.0 };
                        p.pan.1 = if fh > 0.5 { (orig.pan.1 - 2.0 * dv / fh).clamp(-1.0, 1.0) } else { 0.0 };
                    }
                    self.drag = Drag::Pan { start, orig };
                    true
                }
                Drag::Handle { h, orig } => {
                    self.drag_handle(h, &orig, c);
                    self.drag = Drag::Handle { h, orig };
                    true
                }
                Drag::Ink => {
                    if let Some(cur) = &mut self.cur {
                        cur.extend((c.0 as f32, c.1 as f32), shift());
                    }
                    self.drag = Drag::Ink;
                    true
                }
                Drag::Erase => {
                    self.erase_at(c);
                    self.drag = Drag::Erase;
                    true
                }
            },
            Mouse::Up => {
                let d = std::mem::replace(&mut self.drag, Drag::None);
                match d {
                    Drag::Ink => {
                        if let Some(cur) = self.cur.take() {
                            if !cur.is_trivial() {
                                self.push_undo();
                                self.objs.push(Obj::Ink(cur));
                            }
                        }
                    }
                    Drag::Move { orig, .. } | Drag::Pan { orig, .. } | Drag::Handle { orig, .. } => {
                        // щелчок без движения — не шаг отмены
                        let same = self.sel_photo().map(|p| p.cx == orig.cx && p.cy == orig.cy && p.w == orig.w && p.h == orig.h && p.angle == orig.angle && p.pan == orig.pan).unwrap_or(true);
                        if same {
                            self.undo.pop();
                        }
                    }
                    _ => {}
                }
                let _ = hwnd;
                true
            }
            Mouse::Wheel(dz) => {
                // колесо над выбранным фото — размер
                if self.tool.is_none() {
                    if let Some(i) = self.sel.filter(|&i| self.photo(i).map(|p| p.contains(c.0, c.1)).unwrap_or(false)) {
                        self.push_undo();
                        let f = if dz > 0 { 1.05 } else { 1.0 / 1.05 };
                        let p = self.photo_mut(i).unwrap();
                        p.w *= f;
                        p.h *= f;
                        return true;
                    }
                }
                false
            }
            Mouse::Dbl => {
                // двойной щелчок по фото — «ровно»
                if self.tool.is_none() && self.sel.is_some() {
                    return self.event(hwnd, Ev::Click(ID_STRAIGHT));
                }
                false
            }
            _ => false,
        }
    }

    fn event(&mut self, hwnd: HWND, ev: Ev) -> bool {
        let Ev::Click(id) = ev else { return true };
        if id != ID_UNDO {
            self.status.clear();
        }
        self.commit_text();
        match id {
            ID_FMT_PREV | ID_FMT_NEXT => {
                let n = FORMATS.len();
                let i = self.fmt.unwrap_or(n - 1);
                let j = if id == ID_FMT_NEXT { (i + 1) % n } else { (i + n - 1) % n };
                self.set_size((FORMATS[j].1, FORMATS[j].2));
                self.relayout();
            }
            ID_FMT_OWN => {
                if let Some((w, h)) = crate::resize_dlg::ask_size(hwnd, (self.size.0 as f64, self.size.1 as f64)) {
                    let s = ((w.round() as u32).clamp(16, 12000), (h.round() as u32).clamp(16, 12000));
                    self.set_size(s);
                    self.relayout();
                }
            }
            i if (ID_BACK0..ID_BACK0 + BACKS.len() as u32).contains(&i) => {
                let c = BACKS[(i - ID_BACK0) as usize].1;
                if c != self.back {
                    self.push_undo();
                    self.back = c;
                }
            }
            i if (ID_TOOL0..ID_TOOL0 + 9).contains(&i) => {
                let k = (i - ID_TOOL0) as usize;
                self.tool = if k == 0 { None } else { Some(TOOLS[k - 1].0) };
                self.hover = None;
            }
            i if (ID_COLOR0..ID_COLOR0 + COLORS.len() as u32).contains(&i) => self.color = COLORS[(i - ID_COLOR0) as usize],
            ID_W_DEC => self.width_i = self.width_i.saturating_sub(1),
            ID_W_INC => self.width_i = (self.width_i + 1).min(WIDTHS.len() - 1),
            ID_FILL => self.fill = !self.fill,
            ID_ADD => {
                let p = files::open_dialog(hwnd, files::Pick::Images, true);
                let items = p.into_iter().map(|p| Item { path: Some(p), img: None }).collect();
                self.add_items(items);
            }
            i if (ID_LAYOUT0..ID_LAYOUT0 + LAYOUTS.len() as u32).contains(&i) => {
                self.apply_layout(LAYOUTS[(i - ID_LAYOUT0) as usize].0);
                self.tool = None;
            }
            ID_GAP_DEC | ID_GAP_INC => {
                self.gap = if id == ID_GAP_INC { (self.gap + 1).min(10) } else { (self.gap - 1).max(0) };
                self.relayout();
            }
            ID_MODE_FILL | ID_MODE_FIT => {
                self.fit_mode = id == ID_MODE_FIT;
                self.relayout();
            }
            ID_FRONT => self.reorder(true),
            ID_BACKWARD => self.reorder(false),
            ID_ROT90 => {
                if let Some(i) = self.sel {
                    self.push_undo();
                    let p = self.photo_mut(i).unwrap();
                    p.angle = ((p.angle / 90.0).round() * 90.0 + 90.0 + 540.0).rem_euclid(360.0) - 180.0;
                }
            }
            ID_STRAIGHT => {
                if let Some(i) = self.sel {
                    self.push_undo();
                    let p = self.photo_mut(i).unwrap();
                    // ровно и целиком: та же площадь, пропорции фото
                    let area = p.w * p.h;
                    let a = p.aspect();
                    p.angle = 0.0;
                    p.pan = (0.0, 0.0);
                    p.h = (area / a).sqrt();
                    p.w = p.h * a;
                }
            }
            ID_DUP => {
                if let Some(mut p) = self.sel_photo().cloned() {
                    self.push_undo();
                    let d = self.size.0.min(self.size.1) as f64 * 0.03;
                    p.cx += d;
                    p.cy += d;
                    self.objs.push(Obj::Photo(p));
                    self.sel = Some(self.objs.len() - 1);
                }
            }
            ID_DEL => {
                if let Some(i) = self.sel.take() {
                    self.push_undo();
                    self.objs.remove(i);
                }
            }
            ID_UNDO => self.do_undo(),
            ID_OK | ID_SAVE => {
                self.save_settings();
                let img = match self.build() {
                    Ok(i) => i,
                    Err(e) => {
                        crate::ui::message(Some(hwnd), &format!("Не удалось собрать холст:\n{e}"), true);
                        return true;
                    }
                };
                if id == ID_SAVE {
                    let dir = self.objs.iter().find_map(|o| match o {
                        Obj::Photo(p) => p.path.as_ref().and_then(|x| x.parent()).map(|d| d.to_path_buf()),
                        _ => None,
                    });
                    let Some(p) = files::save_dialog(hwnd, "Холст.jpg", dir.as_deref(), false, false) else { return true };
                    let res = if crate::util::is_pdf(&p) {
                        crate::doc::save_pdf(&[crate::doc::Page::mem("Холст", img.clone())], &p)
                    } else {
                        codec::save(&img, &p).map_err(|e| e.message().to_string())
                    };
                    if let Err(e) = res {
                        crate::ui::message(Some(hwnd), &format!("Не удалось сохранить:\n{e}"), true);
                        return true;
                    }
                }
                self.result = Some(img);
                self.changed = false;
                return false;
            }
            ID_CANCEL => {
                return !self.close(hwnd);
            }
            _ => {}
        }
        true
    }

    fn key(&mut self, hwnd: HWND, vk: u16, ctrl: bool, shift: bool) -> Option<bool> {
        if self.text.is_some() {
            if vk == 0x1B {
                self.commit_text();
                return Some(true);
            }
            return None;
        }
        let nudge = |me: &mut Canvas, dx: f64, dy: f64| {
            if let Some(i) = me.sel {
                let step = if shift { 10.0 } else { 1.0 } / me.k();
                me.push_undo();
                let p = me.photo_mut(i).unwrap();
                p.cx += dx * step;
                p.cy += dy * step;
            }
        };
        match vk {
            0x5A if ctrl => self.do_undo(),
            0x44 if ctrl => return Some(self.event(hwnd, Ev::Click(ID_DUP))),
            0x2E | 0x08 if self.sel.is_some() => return Some(self.event(hwnd, Ev::Click(ID_DEL))),
            0x21 => self.reorder(true),
            0x22 => self.reorder(false),
            0x25 => nudge(self, -1.0, 0.0),
            0x27 => nudge(self, 1.0, 0.0),
            0x26 => nudge(self, 0.0, -1.0),
            0x28 => nudge(self, 0.0, 1.0),
            0x1B if self.tool.is_some() => self.tool = None,
            0x1B if self.sel.is_some() => self.sel = None,
            0x0D if !self.objs.is_empty() => return Some(self.event(hwnd, Ev::Click(ID_OK))),
            0x56 if ctrl => {
                // вставить картинку из буфера
                if let Some(img) = crate::clip::image(hwnd) {
                    self.add_items(vec![Item { path: None, img: Some(Rc::new(img)) }]);
                }
            }
            _ => return None,
        }
        Some(true)
    }

    fn drop_files(&mut self, _hwnd: HWND, files: Vec<PathBuf>) {
        let items = files.into_iter().filter(|p| files::is_image(p)).map(|p| Item { path: Some(p), img: None }).collect();
        self.add_items(items);
    }

    fn close(&mut self, hwnd: HWND) -> bool {
        self.commit_text();
        if !self.changed || self.objs.is_empty() {
            return true;
        }
        let r = unsafe { MessageBoxW(hwnd, w!("Закрыть холст? Сделанное не сохранится."), w!("Холст"), MB_YESNO | MB_ICONQUESTION) };
        r == IDYES
    }

    fn raw(&mut self, hwnd: HWND, msg: u32, wp: WPARAM, _lp: LPARAM) -> Option<LRESULT> {
        if msg == WM_COMMAND && (wp.0 & 0xFFFF) as u32 == ID_TEXT && (wp.0 >> 16) & 0xFFFF == 0x0200 {
            // поле текста потеряло фокус — «впечатать» надпись (после выхода из уведомления)
            unsafe {
                let _ = PostMessageW(hwnd, WM_APP + 7, WPARAM(0), LPARAM(0));
            }
            return Some(LRESULT(0));
        }
        if msg == WM_APP + 7 {
            self.commit_text();
            unsafe {
                let _ = windows::Win32::Graphics::Gdi::InvalidateRect(hwnd, None, false);
            }
            return Some(LRESULT(0));
        }
        if msg == WM_CTLCOLOREDIT {
            if let Some(t) = &self.text {
                if t.hwnd.0 as isize == _lp.0 {
                    unsafe {
                        let hdc = HDC(wp.0 as *mut _);
                        let c = self.color;
                        SetTextColor(hdc, rgb((c >> 16) as u8, (c >> 8) as u8, c as u8));
                        let light = c == 0xFFFFFF || c == 0xFDD835;
                        SetBkColor(hdc, if light { rgb(60, 60, 60) } else { rgb(255, 255, 255) });
                        return Some(LRESULT(GetStockObject(if light { DKGRAY_BRUSH } else { WHITE_BRUSH }).0 as isize));
                    }
                }
            }
        }
        None
    }
}

impl Canvas {
    /// Если фото разложены шаблоном — пересобрать под новые настройки.
    fn relayout(&mut self) {
        if let Some(l) = self.layout {
            if self.photo_count() > 0 {
                // одно действие — один шаг отмены
                let before = self.undo.len();
                self.apply_layout(l);
                if self.undo.len() > before + 1 {
                    let keep = self.undo.remove(before);
                    self.undo.truncate(before);
                    self.undo.push(keep);
                }
            }
        }
    }

    fn save_settings(&self) {
        settings::set_i("canvas_fmt", self.fmt.map(|i| i as i32).unwrap_or(-1));
        settings::set_i("canvas_back", self.back as i32);
        settings::set_i("collage_gap", self.gap);
        settings::set_i("collage_fill", (!self.fit_mode) as i32);
        if let Some(l) = self.layout {
            settings::set_i("collage_layout", LAYOUTS.iter().position(|x| x.0 == l).unwrap_or(0) as i32);
        }
    }
}

impl Canvas {
    fn blank(size: (u32, u32)) -> Canvas {
        Canvas {
            size,
            fmt: FORMATS.iter().position(|f| (f.1, f.2) == size),
            back: 0xFFFFFF,
            objs: vec![],
            sel: None,
            undo: vec![],
            tool: None,
            color: COLORS[0],
            width_i: 1,
            fill: false,
            gap: 2,
            fit_mode: false,
            layout: None,
            drag: Drag::None,
            cur: None,
            text: None,
            view: RECT::default(),
            hover: None,
            hwnd: HWND::default(),
            changed: false,
            result: None,
            status: String::new(),
        }
    }
}

/// Собрать коллаж без окна: фото по шаблону и рисунок поверх (замеры, проверки).
pub fn compose(imgs: &[Rc<Img>], size: (u32, u32), layout: Layout, gap: i32, ink: Vec<Shape>) -> Result<Img, String> {
    let mut c = Canvas::blank(size);
    c.gap = gap;
    c.add_items(imgs.iter().map(|i| Item { path: None, img: Some(i.clone()) }).collect());
    c.apply_layout(layout);
    c.objs.extend(ink.into_iter().map(Obj::Ink));
    c.build()
}

/// Открыть холст. items — фото, которые сразу положить; layout — сразу разложить
/// шаблоном (коллаж). Возвращает собранную картинку.
pub fn run(owner: HWND, items: Vec<Item>, collage_mode: bool) -> Option<Img> {
    let fmt = settings::get_i("canvas_fmt", 0);
    let fmt = if (0..FORMATS.len() as i32).contains(&fmt) { fmt as usize } else { 0 };
    let back = settings::get_i("canvas_back", 0xFFFFFF) as u32 & 0xFF_FFFF;
    let li = settings::get_i("collage_layout", 0).clamp(0, LAYOUTS.len() as i32 - 1) as usize;
    let mut c = Canvas::blank((FORMATS[fmt].1, FORMATS[fmt].2));
    c.fmt = Some(fmt);
    c.back = if BACKS.iter().any(|b| b.1 == back) { back } else { 0xFFFFFF };
    c.gap = settings::get_i("collage_gap", 2).clamp(0, 10);
    c.fit_mode = settings::get_i("collage_fill", 1) == 0;
    c.hwnd = owner;
    c.add_items(items);
    if collage_mode && c.photo_count() > 0 {
        c.apply_layout(LAYOUTS[li].0);
        c.sel = None;
    }
    c.undo.clear();
    c.changed = c.photo_count() > 0;
    gui::run_modal(owner, c, (1200, 780)).result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(w: u32, h: u32) -> Photo {
        let img = Rc::new(Img::filled(w, h, [10, 20, 30]));
        load_photo(Item { path: None, img: Some(img) }).unwrap()
    }

    #[test]
    fn crop_keeps_aspect() {
        let mut p = photo(400, 200); // 2:1
        p.w = 100.0;
        p.h = 100.0; // квадратная рамка — видна середина
        let (u0, v0, cw, ch) = p.crop();
        assert!((cw - 0.5).abs() < 1e-9 && (ch - 1.0).abs() < 1e-9 && (u0 - 0.25).abs() < 1e-9 && v0.abs() < 1e-9);
        p.pan = (1.0, 0.0);
        assert!((p.crop().0 - 0.5).abs() < 1e-9, "сдвиг до правого края");
        p.w = 200.0;
        p.h = 100.0;
        assert_eq!(p.crop(), (0.0, 0.0, 1.0, 1.0));
    }

    #[test]
    fn rotate_hit_and_world() {
        let mut p = photo(100, 50);
        p.cx = 100.0;
        p.cy = 100.0;
        p.w = 100.0;
        p.h = 50.0;
        p.angle = 90.0;
        assert!(p.contains(100.0, 145.0) && !p.contains(145.0, 100.0));
        let (x, y) = p.world(50.0, 0.0);
        assert!((x - 100.0).abs() < 1e-9 && (y - 150.0).abs() < 1e-9);
    }

    #[test]
    fn layout_fills_canvas_and_resize_scales() {
        let mut c = Canvas::blank((3000, 2000));
        c.objs = (0..5).map(|i| Obj::Photo(photo(300 + i * 50, 200))).collect();
        c.view = gui::r(0, 0, 300, 200);
        c.apply_layout(Layout::Grid);
        let area: f64 = (0..5).map(|i| c.photo(i).unwrap()).map(|p| p.w * p.h).sum();
        assert!(area > 3000.0 * 2000.0 * 0.6, "сетка занимает холст: {area}");
        for i in 0..5 {
            let p = c.photo(i).unwrap();
            assert!(p.cx - p.w / 2.0 >= -0.5 && p.cx + p.w / 2.0 <= 3000.5);
        }
        assert_eq!(c.undo.len(), 1);
        c.set_size((1500, 1000));
        let p = c.photo(0).unwrap();
        assert!(p.cx + p.w / 2.0 <= 1500.5);
        c.do_undo();
        assert_eq!(c.size, (3000, 2000));
        // сборка: фон и фото на месте
        let img = c.build().unwrap();
        assert_eq!((img.w, img.h), (3000, 2000));
        let p = c.photo(0).unwrap();
        let k = ((p.cy as usize) * 3000 + p.cx as usize) * 4;
        assert_eq!(&img.px[k..k + 3], &[10, 20, 30]);
    }

    #[test]
    fn screen_mapping_roundtrip() {
        let mut c = Canvas::blank((3000, 2000));
        c.view = gui::r(10, 20, 600, 400);
        let s = c.to_screen((1500.0, 1000.0));
        let b = c.to_canvas(s.0, s.1);
        assert!((b.0 - 1500.0).abs() < 6.0 && (b.1 - 1000.0).abs() < 6.0);
    }
}

