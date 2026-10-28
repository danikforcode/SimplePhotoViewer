// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Мини-набор «самонарисованных» элементов интерфейса на GDI.
//!
//! Почему не стандартные кнопки Windows: они не умеют тёмную тему. Здесь всё
//! рисуется само, поэтому светлая/тёмная тема работает везде одинаково.
//!
//! Устройство: каждое окно — `Panel` (состояние + логика). Окно-«хозяин» (`Host`)
//! при каждой перерисовке спрашивает у панели список элементов (`layout`),
//! рисует их, а клики/колесо/клавиши превращает в события (`Ev`) для панели.
//! Если панель занята (например, открыт диалог выбора файла), окно показывает
//! последний нарисованный кадр — поэтому из обработчиков можно вызывать
//! модальные окна без опасений.

use crate::theme::{self, Theme};
use crate::util::{rgb, wide};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{DragFinish, DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::*;

// ------------------------------------------------------------------ иконки ---
// Глифы шрифта «Segoe MDL2 Assets» (есть в Windows 10 и 11): ноль байт в exe,
// чёткие на любом масштабе и сами перекрашиваются под тему.
// Флаг SYM — символ берётся из «Segoe UI Symbol».
pub const SYM: u32 = 0x10_0000;
pub const IC_OPEN: u32 = 0xE8E5;
pub const IC_SAVE: u32 = 0xE74E;
pub const IC_PRINT: u32 = 0xE749;
pub const IC_UNDO: u32 = 0xE7A7;
pub const IC_ROT_L: u32 = SYM | 0x21BA; // ↺
pub const IC_ROT_R: u32 = SYM | 0x21BB; // ↻
pub const IC_FLIP: u32 = 0xE8AB;
pub const IC_CROP: u32 = 0xE7A8;
pub const IC_PREV: u32 = 0xE76B;
pub const IC_NEXT: u32 = 0xE76C;
pub const IC_ZOOM_IN: u32 = 0xE8A3;
pub const IC_ZOOM_OUT: u32 = 0xE71F;
pub const IC_FIT: u32 = 0xE73F; // BackToWindow — «вписать»
pub const IC_ADD: u32 = 0xE710;
pub const IC_REMOVE: u32 = 0xE738;
pub const IC_DELETE: u32 = 0xE74D;
pub const IC_UP: u32 = 0xE74A;
pub const IC_DOWN: u32 = 0xE74B;
pub const IC_BACK: u32 = 0xE72B;
pub const IC_FORWARD: u32 = 0xE72A;
pub const IC_SUN: u32 = 0xE706;
pub const IC_MOON: u32 = 0xE708;
pub const IC_MONITOR: u32 = 0xE7F4;
pub const IC_HELP: u32 = 0xE897;
pub const IC_INFO: u32 = 0xE946;
pub const IC_KEYBOARD: u32 = 0xE765;
pub const IC_FULLSCREEN: u32 = 0xE740;
pub const IC_DOC: u32 = 0xE8A5;
pub const IC_MERGE: u32 = 0xE8F1;
pub const IC_GRID: u32 = 0xE8A9;
pub const IC_COLOR: u32 = 0xE790;
pub const IC_CONTRAST: u32 = SYM | 0x25D0; // ◐
pub const IC_SETTINGS: u32 = 0xE713;
pub const IC_COPY: u32 = 0xE8C8;
pub const IC_PASTE: u32 = 0xE77F;
pub const IC_CHECK: u32 = 0xE73E;
pub const IC_PICTURE: u32 = 0xE8B9;
pub const IC_RESET: u32 = 0xE72C;

// ------------------------------------------------------------------ шрифты ---
pub struct Fonts {
    pub text: HFONT,
    pub small: HFONT,
    pub bold: HFONT,
    pub big: HFONT,
    pub icon: HFONT,
    pub icon_big: HFONT,
    pub sym: HFONT,
    pub sym_big: HFONT,
}

fn mkfont(px: i32, weight: i32, face: &str) -> HFONT {
    let f = wide(face);
    unsafe {
        CreateFontW(-px, 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32, CLEARTYPE_QUALITY.0 as u32, (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32, PCWSTR(f.as_ptr()))
    }
}

thread_local! { static FONTS: RefCell<Vec<(u32, Rc<Fonts>)>> = const { RefCell::new(Vec::new()) }; }

fn fonts(dpi: u32) -> Rc<Fonts> {
    FONTS.with(|c| {
        let mut v = c.borrow_mut();
        if let Some((_, f)) = v.iter().find(|(d, _)| *d == dpi) {
            return f.clone();
        }
        let s = |x: i32| x * dpi as i32 / 96;
        let f = Rc::new(Fonts {
            text: mkfont(s(12), 400, "Segoe UI"),
            small: mkfont(s(11), 400, "Segoe UI"),
            bold: mkfont(s(12), 600, "Segoe UI"),
            big: mkfont(s(15), 600, "Segoe UI"),
            icon: mkfont(s(16), 400, "Segoe MDL2 Assets"),
            icon_big: mkfont(s(20), 400, "Segoe MDL2 Assets"),
            sym: mkfont(s(17), 400, "Segoe UI Symbol"),
            sym_big: mkfont(s(21), 400, "Segoe UI Symbol"),
        });
        v.push((dpi, f.clone()));
        f
    })
}

// ---------------------------------------------------------------------- Ui ---
/// Контекст отрисовки/раскладки: масштаб (DPI), тема, шрифты.
pub struct Ui {
    pub dpi: u32,
    pub t: Theme,
    pub f: Rc<Fonts>,
    mdc: HDC,
}

impl Ui {
    pub fn new(dpi: u32) -> Ui {
        let mdc = unsafe { CreateCompatibleDC(None) };
        Ui { dpi, t: theme::cur(), f: fonts(dpi), mdc }
    }
    /// Пиксели «при 100%» → пиксели экрана.
    pub fn s(&self, v: i32) -> i32 {
        v * self.dpi as i32 / 96
    }
    /// Ширина текста в пикселях.
    pub fn tw(&self, font: HFONT, s: &str) -> i32 {
        let w: Vec<u16> = s.encode_utf16().collect();
        let mut sz = SIZE::default();
        unsafe {
            let old = SelectObject(self.mdc, font);
            let _ = GetTextExtentPoint32W(self.mdc, &w, &mut sz);
            SelectObject(self.mdc, old);
        }
        sz.cx
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteDC(self.mdc);
        }
    }
}

// ------------------------------------------------------------ примитивы ---
pub fn r(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT { left: x, top: y, right: x + w, bottom: y + h }
}

pub fn inside(rc: &RECT, x: i32, y: i32) -> bool {
    x >= rc.left && x < rc.right && y >= rc.top && y < rc.bottom
}

pub fn fill(hdc: HDC, rc: &RECT, c: COLORREF) {
    unsafe {
        let b = CreateSolidBrush(c);
        FillRect(hdc, rc, b);
        let _ = DeleteObject(b);
    }
}

/// Скруглённый прямоугольник: заливка и/или рамка.
pub fn round(hdc: HDC, rc: &RECT, rad: i32, fillc: Option<COLORREF>, line: Option<COLORREF>) {
    unsafe {
        let pen = match line {
            Some(c) => CreatePen(PS_SOLID, 1, c),
            None => HPEN(GetStockObject(NULL_PEN).0),
        };
        let br = match fillc {
            Some(c) => CreateSolidBrush(c),
            None => HBRUSH(GetStockObject(NULL_BRUSH).0),
        };
        let op = SelectObject(hdc, pen);
        let ob = SelectObject(hdc, br);
        let extra = if line.is_none() { 1 } else { 0 };
        let _ = RoundRect(hdc, rc.left, rc.top, rc.right + extra, rc.bottom + extra, rad, rad);
        SelectObject(hdc, op);
        SelectObject(hdc, ob);
        if line.is_some() {
            let _ = DeleteObject(pen);
        }
        if fillc.is_some() {
            let _ = DeleteObject(br);
        }
    }
}

pub fn line(hdc: HDC, x0: i32, y0: i32, x1: i32, y1: i32, c: COLORREF, width: i32, style: PEN_STYLE) {
    unsafe {
        let pen = CreatePen(style, width, c);
        let op = SelectObject(hdc, pen);
        let _ = MoveToEx(hdc, x0, y0, None);
        let _ = LineTo(hdc, x1, y1);
        SelectObject(hdc, op);
        let _ = DeleteObject(pen);
    }
}

pub fn frame(hdc: HDC, rc: &RECT, c: COLORREF, style: PEN_STYLE) {
    unsafe {
        let pen = CreatePen(style, 1, c);
        let op = SelectObject(hdc, pen);
        let ob = SelectObject(hdc, GetStockObject(NULL_BRUSH));
        let _ = Rectangle(hdc, rc.left, rc.top, rc.right, rc.bottom);
        SelectObject(hdc, op);
        SelectObject(hdc, ob);
        let _ = DeleteObject(pen);
    }
}

pub const LEFT: u32 = (DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0 | DT_NOPREFIX.0) as u32;
pub const CENTER: u32 = (DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0 | DT_NOPREFIX.0) as u32;
pub const RIGHT: u32 = (DT_RIGHT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0 | DT_NOPREFIX.0) as u32;
pub const WRAP: u32 = (DT_CENTER.0 | DT_WORDBREAK.0 | DT_NOPREFIX.0) as u32;

pub fn text(hdc: HDC, s: &str, rc: &RECT, font: HFONT, c: COLORREF, flags: u32) {
    if s.is_empty() {
        return;
    }
    let mut w: Vec<u16> = s.encode_utf16().collect();
    let mut rc = *rc;
    unsafe {
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, c);
        let of = SelectObject(hdc, font);
        DrawTextW(hdc, &mut w, &mut rc, DRAW_TEXT_FORMAT(flags));
        SelectObject(hdc, of);
    }
}

pub fn glyph(ui: &Ui, hdc: HDC, icon: u32, rc: &RECT, big: bool, c: COLORREF) {
    let (font, code) = if icon & SYM != 0 {
        (if big { ui.f.sym_big } else { ui.f.sym }, icon & !SYM)
    } else {
        (if big { ui.f.icon_big } else { ui.f.icon }, icon)
    };
    let s = char::from_u32(code).map(|c| c.to_string()).unwrap_or_default();
    text(hdc, &s, rc, font, c, CENTER);
}

/// Нарисовать изображение (BGRA) в прямоугольник. hq — качественное сглаживание.
pub fn blit(hdc: HDC, img: &crate::image_buf::Img, dst: &RECT, hq: bool) {
    unsafe {
        let info = crate::util::bmi(img);
        SetStretchBltMode(hdc, if hq { HALFTONE } else { COLORONCOLOR });
        let _ = SetBrushOrgEx(hdc, 0, 0, None);
        StretchDIBits(hdc, dst.left, dst.top, dst.right - dst.left, dst.bottom - dst.top, 0, 0, img.w as i32, img.h as i32,
            Some(img.px.as_ptr() as *const _), &info, DIB_RGB_COLORS, SRCCOPY);
    }
}

// ---------------------------------------------------------------- элементы ---
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Tab,    // вкладка шапки (on = активна)
    Ribbon, // большая кнопка ленты: значок над подписью
    Tool,   // кнопка-значок (нижняя панель); text — если значка нет
    Button, // обычная кнопка с рамкой; on = основная (акцентная)
    Seg,    // вариант выбора; on = выбран
    Check,  // флажок
    Label,  // текст; align 0/1/2 = слева/по центру/справа
    Value,  // значение «счётчика» (колесо меняет)
    Sep,    // разделитель
    Group,  // заголовок раздела
    List,   // список
    Swatch, // образец цвета (кнопка)
}

#[derive(Clone)]
pub struct W {
    pub id: u32,
    pub kind: Kind,
    pub rc: RECT,
    pub text: String,
    pub icon: u32,
    pub tip: String,
    pub on: bool,
    pub enabled: bool,
    pub wheel: Option<(u32, u32)>,
    pub align: u8,
    pub items: Vec<(String, String)>,
    pub sel: Option<usize>,
    pub scroll: usize,
    pub color: Option<COLORREF>,
}

impl W {
    pub fn new(kind: Kind, id: u32, rc: RECT) -> W {
        W {
            id, kind, rc, text: String::new(), icon: 0, tip: String::new(), on: false, enabled: true,
            wheel: None, align: 0, items: vec![], sel: None, scroll: 0, color: None,
        }
    }
    pub fn t(mut self, s: impl Into<String>) -> W {
        self.text = s.into();
        self
    }
    pub fn i(mut self, icon: u32) -> W {
        self.icon = icon;
        self
    }
    pub fn tip(mut self, s: impl Into<String>) -> W {
        self.tip = s.into();
        self
    }
    pub fn on(mut self, b: bool) -> W {
        self.on = b;
        self
    }
    pub fn en(mut self, b: bool) -> W {
        self.enabled = b;
        self
    }
    pub fn wheel(mut self, up: u32, down: u32) -> W {
        self.wheel = Some((up, down));
        self
    }
    pub fn align(mut self, a: u8) -> W {
        self.align = a;
        self
    }
    pub fn color(mut self, c: COLORREF) -> W {
        self.color = Some(c);
        self
    }
    fn clickable(&self) -> bool {
        self.enabled && matches!(self.kind, Kind::Tab | Kind::Ribbon | Kind::Tool | Kind::Button | Kind::Seg | Kind::Check | Kind::List | Kind::Swatch)
    }
}

/// Высота строки списка.
pub fn list_row_h(ui: &Ui, w: &W) -> i32 {
    if w.items.iter().any(|(_, sub)| !sub.is_empty()) { ui.s(40) } else { ui.s(28) }
}

/// Сколько строк списка помещается.
pub fn list_visible(ui: &Ui, w: &W) -> usize {
    (((w.rc.bottom - w.rc.top - 4) / list_row_h(ui, w)).max(1)) as usize
}

/// Поправить прокрутку так, чтобы выделенная строка была видна.
pub fn list_fix_scroll(ui: &Ui, w: &W, scroll: &mut usize) {
    let vis = list_visible(ui, w);
    let max = w.items.len().saturating_sub(vis);
    if let Some(s) = w.sel {
        if s < *scroll {
            *scroll = s;
        } else if s >= *scroll + vis {
            *scroll = s + 1 - vis;
        }
    }
    *scroll = (*scroll).min(max);
}

fn draw_w(ui: &Ui, hdc: HDC, w: &W, hover: bool, press: bool, hover_item: Option<usize>) {
    let t = &ui.t;
    let rc = w.rc;
    let fg = if w.enabled { t.text } else { t.dim };
    let state_bg = |on: bool| -> Option<COLORREF> {
        if !w.enabled {
            None
        } else if press {
            Some(t.press)
        } else if hover {
            Some(t.hover)
        } else if on {
            Some(t.sel)
        } else {
            None
        }
    };
    let rad = ui.s(8);
    match w.kind {
        Kind::Tab => {
            if let Some(c) = state_bg(false) {
                round(hdc, &rc, rad, Some(c), None);
            }
            let font = if w.on { ui.f.bold } else { ui.f.text };
            text(hdc, &w.text, &rc, font, if w.on { t.text } else { t.dim }, CENTER);
            if w.on {
                let tw = ui.tw(font, &w.text).min(rc.right - rc.left);
                let cx = (rc.left + rc.right) / 2;
                let bar = r(cx - tw / 2, rc.bottom - ui.s(3), tw, ui.s(3));
                round(hdc, &bar, ui.s(3), Some(t.accent), None);
            }
        }
        Kind::Ribbon => {
            if let Some(c) = state_bg(w.on) {
                round(hdc, &rc, rad, Some(c), None);
            }
            let ih = ui.s(30);
            let ir = RECT { top: rc.top + ui.s(4), bottom: rc.top + ui.s(4) + ih, ..rc };
            glyph(ui, hdc, w.icon, &ir, true, if w.on && w.enabled { t.accent } else { fg });
            let tr = RECT { left: rc.left + ui.s(2), right: rc.right - ui.s(2), top: ir.bottom, bottom: rc.bottom - ui.s(3) };
            text(hdc, &w.text, &tr, ui.f.small, fg, CENTER);
        }
        Kind::Tool => {
            if let Some(c) = state_bg(w.on) {
                round(hdc, &rc, rad, Some(c), None);
            }
            if w.icon != 0 {
                glyph(ui, hdc, w.icon, &rc, false, if w.on && w.enabled { t.accent } else { fg });
            } else {
                text(hdc, &w.text, &rc, ui.f.bold, fg, CENTER);
            }
        }
        Kind::Button => {
            let (bg, fgc, border) = if w.on && w.enabled {
                let bg = if press { t.press } else if hover { blend(t.accent, t.bg, 0.85) } else { t.accent };
                (bg, if press { t.text } else { t.on_accent }, None)
            } else {
                (state_bg(false).unwrap_or(t.card), fg, Some(t.border))
            };
            round(hdc, &rc, rad, Some(bg), border);
            if w.icon != 0 && !w.text.is_empty() {
                let iw = ui.s(20);
                let tw = ui.tw(ui.f.text, &w.text);
                let total = iw + ui.s(6) + tw;
                let x0 = (rc.left + rc.right - total) / 2;
                glyph(ui, hdc, w.icon, &RECT { left: x0, right: x0 + iw, ..rc }, false, fgc);
                text(hdc, &w.text, &RECT { left: x0 + iw + ui.s(6), right: rc.right - ui.s(4), ..rc }, ui.f.text, fgc, LEFT);
            } else if w.icon != 0 {
                glyph(ui, hdc, w.icon, &rc, false, fgc);
            } else {
                text(hdc, &w.text, &rc, ui.f.text, fgc, CENTER);
            }
        }
        Kind::Seg => {
            let (bg, fgc, border) = if w.on {
                (if hover { blend(t.accent, t.bg, 0.85) } else { t.accent }, t.on_accent, None)
            } else {
                (state_bg(false).unwrap_or(t.card), fg, Some(t.border))
            };
            round(hdc, &rc, rad, Some(bg), border);
            text(hdc, &w.text, &RECT { left: rc.left + ui.s(4), right: rc.right - ui.s(4), ..rc }, ui.f.text, fgc, CENTER);
        }
        Kind::Check => {
            if let Some(c) = state_bg(false) {
                round(hdc, &rc, rad, Some(c), None);
            }
            let b = ui.s(18);
            let bx = r(rc.left + ui.s(4), (rc.top + rc.bottom - b) / 2, b, b);
            if w.on {
                round(hdc, &bx, ui.s(5), Some(t.accent), None);
                glyph(ui, hdc, IC_CHECK, &bx, false, t.on_accent);
            } else {
                round(hdc, &bx, ui.s(5), Some(t.card), Some(t.dim));
            }
            text(hdc, &w.text, &RECT { left: bx.right + ui.s(8), ..rc }, ui.f.text, fg, LEFT);
        }
        Kind::Label => {
            let fl = match w.align { 1 => CENTER, 2 => RIGHT, _ => LEFT };
            text(hdc, &w.text, &rc, if w.on { ui.f.bold } else { ui.f.text }, w.color.unwrap_or(fg), fl);
        }
        Kind::Value => {
            round(hdc, &rc, rad, Some(t.card), Some(if hover { t.dim } else { t.border }));
            text(hdc, &w.text, &rc, ui.f.text, fg, CENTER);
        }
        Kind::Sep => {
            if rc.right - rc.left <= rc.bottom - rc.top {
                let x = (rc.left + rc.right) / 2;
                line(hdc, x, rc.top, x, rc.bottom, t.border, 1, PS_SOLID);
            } else {
                let y = (rc.top + rc.bottom) / 2;
                line(hdc, rc.left, y, rc.right, y, t.border, 1, PS_SOLID);
            }
        }
        Kind::Group => {
            text(hdc, &w.text, &rc, ui.f.bold, t.dim, LEFT);
        }
        Kind::Swatch => {
            let c = w.color.unwrap_or(t.card);
            round(hdc, &rc, rad, Some(if hover { t.hover } else { t.card }), Some(if w.on { t.accent } else { t.border }));
            let inner = RECT { left: rc.left + ui.s(5), top: rc.top + ui.s(5), right: rc.right - ui.s(5), bottom: rc.bottom - ui.s(5) };
            round(hdc, &inner, ui.s(5), Some(c), Some(t.border));
            if w.on {
                let ring = RECT { left: rc.left - 1, top: rc.top - 1, right: rc.right + 1, bottom: rc.bottom + 1 };
                round(hdc, &ring, rad, None, Some(t.accent));
            }
        }
        Kind::List => {
            round(hdc, &rc, rad, Some(t.card), Some(t.border));
            let rh = list_row_h(ui, w);
            let vis = list_visible(ui, w);
            let mut y = rc.top + 2;
            if w.items.is_empty() {
                text(hdc, &w.text, &rc, ui.f.text, t.dim, CENTER);
            }
            for (k, (main, sub)) in w.items.iter().enumerate().skip(w.scroll).take(vis) {
                let row = r(rc.left + 3, y, rc.right - rc.left - 6, rh);
                if w.sel == Some(k) {
                    round(hdc, &row, ui.s(6), Some(t.sel), None);
                    let bar = r(row.left + 1, row.top + rh / 4, ui.s(3), rh / 2);
                    round(hdc, &bar, ui.s(3), Some(t.accent), None);
                } else if hover_item == Some(k) {
                    round(hdc, &row, ui.s(6), Some(t.hover), None);
                }
                let tx = RECT { left: row.left + ui.s(12), right: row.right - ui.s(8), ..row };
                if sub.is_empty() {
                    text(hdc, main, &tx, ui.f.text, t.text, LEFT);
                } else {
                    let half = (tx.top + tx.bottom) / 2;
                    text(hdc, main, &RECT { bottom: half + ui.s(2), ..tx }, ui.f.text, t.text, LEFT);
                    text(hdc, sub, &RECT { top: half, ..tx }, ui.f.small, t.dim, LEFT);
                }
                y += rh;
            }
            // полоска прокрутки
            let n = w.items.len();
            if n > vis {
                let track = (rc.bottom - rc.top - 8) as f64;
                let th = (track * vis as f64 / n as f64).max(ui.s(16) as f64);
                let ty = rc.top as f64 + 4.0 + (track - th) * w.scroll as f64 / (n - vis) as f64;
                let bar = r(rc.right - ui.s(6), ty as i32, ui.s(3), th as i32);
                round(hdc, &bar, ui.s(3), Some(t.dim), None);
            }
        }
    }
}

/// Смешать два цвета: k=1 → a, k=0 → b.
pub fn blend(a: COLORREF, b: COLORREF, k: f64) -> COLORREF {
    let ch = |c: COLORREF, s: u32| ((c.0 >> s) & 0xFF) as f64;
    let m = |s| (ch(a, s) * k + ch(b, s) * (1.0 - k)).round() as u8;
    rgb(m(0), m(8), m(16))
}

// ------------------------------------------------------------------ панель ---
#[derive(Clone, Copy, Debug)]
pub enum Ev {
    Click(u32),
    Item(u32, usize),
    Dbl(u32, usize),
    Scroll(u32, i32),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mouse {
    Down,
    Up,
    Move,
    Dbl,
    Wheel(i32),
    RDown,
}

pub trait Panel {
    fn title(&self) -> String;
    /// Вызывается один раз после создания окна.
    fn init(&mut self, _hwnd: HWND) {}
    /// Список элементов для текущего размера окна.
    fn layout(&mut self, ui: &Ui, w: i32, h: i32) -> Vec<W>;
    /// Нарисовать всё, кроме элементов (фон, предпросмотр). Вызывается после layout.
    fn paint(&mut self, ui: &Ui, hdc: HDC, w: i32, h: i32) {
        fill(hdc, &r(0, 0, w, h), ui.t.bg);
    }
    /// Событие от элемента. false — закрыть окно.
    fn event(&mut self, hwnd: HWND, ev: Ev) -> bool;
    /// Клавиша. None — не обработана (Esc тогда закрывает диалог).
    fn key(&mut self, _hwnd: HWND, _vk: u16, _ctrl: bool, _shift: bool) -> Option<bool> {
        None
    }
    /// Мышь вне элементов. true — перерисовать.
    fn mouse(&mut self, _hwnd: HWND, _m: Mouse, _x: i32, _y: i32) -> bool {
        false
    }
    fn cursor(&self, _x: i32, _y: i32) -> Option<PCWSTR> {
        None
    }
    fn timer(&mut self, _hwnd: HWND, _id: usize) -> bool {
        false
    }
    fn drop_files(&mut self, _hwnd: HWND, _files: Vec<PathBuf>) {}
    /// Можно ли закрыть окно (вопрос о несохранённом и т.п.).
    fn close(&mut self, _hwnd: HWND) -> bool {
        true
    }
    /// Минимальный размер клиентской области (в «пикселях 100%»).
    fn min_size(&self) -> (i32, i32) {
        (480, 360)
    }
}

// -------------------------------------------------------------------- окно ---
const TIP_TIMER: usize = 0x7001;
const WM_MOUSELEAVE: u32 = 0x02A3;

struct St {
    ws: Vec<W>,
    mouse: (i32, i32),
    hover: Option<usize>,
    hover_item: Option<usize>,
    press: Option<(u32, Kind)>,
    back: HBITMAP,
    bw: i32,
    bh: i32,
    tip: bool,
    tracking: bool,
    title: String,
}

struct Host {
    panel: Rc<RefCell<dyn Panel>>,
    st: RefCell<St>,
    owner: HWND,
}

fn host<'a>(hwnd: HWND) -> Option<&'a Host> {
    unsafe {
        let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Host;
        if p.is_null() { None } else { Some(&*p) }
    }
}

pub fn dpi_of(hwnd: HWND) -> u32 {
    let d = unsafe { GetDpiForWindow(hwnd) };
    if d == 0 { 96 } else { d }
}

pub fn invalidate(hwnd: HWND) {
    unsafe {
        let _ = InvalidateRect(hwnd, None, false);
    }
}

fn hit(ws: &[W], x: i32, y: i32) -> Option<usize> {
    ws.iter().rposition(|w| inside(&w.rc, x, y) && (w.clickable() || w.wheel.is_some() || !w.tip.is_empty()))
}

fn list_item(ui_dpi: u32, w: &W, y: i32) -> Option<usize> {
    let ui = Ui::new(ui_dpi);
    let rh = list_row_h(&ui, w);
    let k = w.scroll + ((y - w.rc.top - 2).max(0) / rh) as usize;
    (k < w.items.len() && k < w.scroll + list_visible(&ui, w)).then_some(k)
}

fn paint(hwnd: HWND, h: &Host) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let (w, ht) = (rc.right.max(1), rc.bottom.max(1));
        let mem = CreateCompatibleDC(hdc);
        {
            let mut st = h.st.borrow_mut();
            if st.back.is_invalid() || st.bw != w || st.bh != ht {
                if !st.back.is_invalid() {
                    let _ = DeleteObject(st.back);
                }
                st.back = CreateCompatibleBitmap(hdc, w, ht);
                (st.bw, st.bh) = (w, ht);
                // нечего показывать из старого кадра — зальём фоном
                let o = SelectObject(mem, st.back);
                fill(mem, &rc, theme::cur().bg);
                SelectObject(mem, o);
            }
            let old = SelectObject(mem, st.back);
            if let Ok(mut p) = h.panel.try_borrow_mut() {
                let ui = Ui::new(dpi_of(hwnd));
                st.ws = p.layout(&ui, w, ht);
                p.paint(&ui, mem, w, ht);
                drop(p);
                let (mx, my) = st.mouse;
                st.hover = hit(&st.ws, mx, my);
                st.hover_item = st.hover.and_then(|i| {
                    let wd = &st.ws[i];
                    (wd.kind == Kind::List).then(|| list_item(ui.dpi, wd, my)).flatten()
                });
                for (i, wd) in st.ws.iter().enumerate() {
                    let hov = st.hover == Some(i) && wd.clickable();
                    let pr = hov && st.press.map(|(id, k)| id == wd.id && k == wd.kind).unwrap_or(false) && wd.kind != Kind::List;
                    draw_w(&ui, mem, wd, hov, pr, if hov { st.hover_item } else { None });
                }
                // подсказка
                if st.tip && st.press.is_none() {
                    if let Some(wd) = st.hover.map(|i| &st.ws[i]).filter(|wd| !wd.tip.is_empty()) {
                        let tw = ui.tw(ui.f.text, &wd.tip) + ui.s(16);
                        let th = ui.s(26);
                        let mut x = mx.min(w - tw - ui.s(4)).max(ui.s(4));
                        let mut y = wd.rc.bottom + ui.s(6);
                        if y + th > ht {
                            y = wd.rc.top - th - ui.s(6);
                        }
                        if x < 0 {
                            x = 0;
                        }
                        let b = r(x, y, tw, th);
                        round(mem, &b, ui.s(6), Some(ui.t.tip_bg), None);
                        text(mem, &wd.tip, &b, ui.f.text, ui.t.tip_text, CENTER);
                    }
                }
            }
            let _ = BitBlt(hdc, 0, 0, w, ht, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
        }
        let _ = DeleteDC(mem);
        let _ = EndPaint(hwnd, &ps);
    }
}

/// Передать событие панели; после — обновить заголовок и перерисовать.
fn dispatch(hwnd: HWND, h: &Host, f: impl FnOnce(&mut dyn Panel) -> bool) {
    let res = match h.panel.try_borrow_mut() {
        Ok(mut p) => {
            let keep = f(&mut *p);
            let t = p.title();
            Some((keep, t))
        }
        Err(_) => None,
    };
    if let Some((keep, t)) = res {
        let changed = {
            let mut st = h.st.borrow_mut();
            let c = st.title != t;
            if c {
                st.title = t.clone();
            }
            c
        };
        if changed {
            unsafe {
                let _ = SetWindowTextW(hwnd, &HSTRING::from(t));
            }
        }
        if !keep {
            unsafe {
                let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
    }
    invalidate(hwnd);
}

fn xy(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32)
}

fn set_hover(hwnd: HWND, h: &Host, x: i32, y: i32) {
    let (changed, has_tip) = {
        let mut st = h.st.borrow_mut();
        st.mouse = (x, y);
        let nh = hit(&st.ws, x, y);
        let ni = nh.and_then(|i| {
            let wd = &st.ws[i];
            (wd.kind == Kind::List).then(|| list_item(dpi_of(hwnd), wd, y)).flatten()
        });
        let changed = nh != st.hover || ni != st.hover_item;
        if nh != st.hover {
            st.tip = false;
        }
        st.hover = nh;
        st.hover_item = ni;
        (changed, nh.map(|i| !st.ws[i].tip.is_empty()).unwrap_or(false))
    };
    if changed {
        unsafe {
            let _ = KillTimer(hwnd, TIP_TIMER);
            if has_tip {
                SetTimer(hwnd, TIP_TIMER, 550, None);
            }
        }
        invalidate(hwnd);
    }
}

pub fn retheme_all() {
    unsafe extern "system" fn cb(hwnd: HWND, _: LPARAM) -> BOOL {
        if host(hwnd).is_some() {
            theme::apply_titlebar(hwnd);
            invalidate(hwnd);
        }
        TRUE
    }
    unsafe {
        let _ = EnumThreadWindows(GetCurrentThreadId(), Some(cb), LPARAM(0));
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let cs = &*(lp.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let Some(h) = host(hwnd) else { return DefWindowProcW(hwnd, msg, wp, lp) };
        match msg {
            WM_CREATE => {
                theme::apply_titlebar(hwnd);
                if let Ok(mut p) = h.panel.try_borrow_mut() {
                    p.init(hwnd);
                }
                LRESULT(0)
            }
            WM_PAINT => {
                paint(hwnd, h);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SIZE => {
                invalidate(hwnd);
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                if let Ok(p) = h.panel.try_borrow() {
                    let (mw, mh) = p.min_size();
                    let d = dpi_of(hwnd) as i32;
                    let mi = &mut *(lp.0 as *mut MINMAXINFO);
                    mi.ptMinTrackSize = POINT { x: mw * d / 96 + 16, y: mh * d / 96 + 39 };
                }
                LRESULT(0)
            }
            WM_DPICHANGED => {
                let rc = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(hwnd, None, rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top, SWP_NOZORDER | SWP_NOACTIVATE);
                invalidate(hwnd);
                LRESULT(0)
            }
            WM_SETTINGCHANGE => {
                if theme::mode() == theme::Mode::System && theme::refresh() {
                    retheme_all();
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_SETCURSOR => {
                if (lp.0 & 0xFFFF) as u32 == HTCLIENT {
                    let mut pt = POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    let _ = ScreenToClient(hwnd, &mut pt);
                    let over_widget = h.st.borrow().ws.iter().any(|w| inside(&w.rc, pt.x, pt.y) && w.clickable());
                    let cur = if over_widget { None } else { h.panel.try_borrow().ok().and_then(|p| p.cursor(pt.x, pt.y)) };
                    SetCursor(LoadCursorW(None, cur.unwrap_or(IDC_ARROW)).unwrap_or_default());
                    return LRESULT(1);
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_MOUSEMOVE => {
                let (x, y) = xy(lp);
                if !h.st.borrow().tracking {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut tme);
                    h.st.borrow_mut().tracking = true;
                }
                set_hover(hwnd, h, x, y);
                let pressing = h.st.borrow().press.is_some();
                if !pressing {
                    let over = h.st.borrow().hover.is_some();
                    if !over || GetCapture() == hwnd {
                        let redraw = h.panel.try_borrow_mut().map(|mut p| p.mouse(hwnd, Mouse::Move, x, y)).unwrap_or(false);
                        if redraw {
                            invalidate(hwnd);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                {
                    let mut st = h.st.borrow_mut();
                    st.tracking = false;
                }
                if GetCapture() != hwnd {
                    set_hover(hwnd, h, -10000, -10000);
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN => {
                let (x, y) = xy(lp);
                set_hover(hwnd, h, x, y);
                let target = {
                    let st = h.st.borrow();
                    st.hover.map(|i| st.ws[i].clone()).filter(|w| w.clickable())
                };
                if msg == WM_RBUTTONDOWN {
                    if target.is_none() {
                        dispatch(hwnd, h, |p| {
                            p.mouse(hwnd, Mouse::RDown, x, y);
                            true
                        });
                    }
                    return LRESULT(0);
                }
                match target {
                    Some(w) => {
                        let _ = KillTimer(hwnd, TIP_TIMER);
                        h.st.borrow_mut().tip = false;
                        if w.kind == Kind::List {
                            if let Some(k) = list_item(dpi_of(hwnd), &w, y) {
                                let ev = if msg == WM_LBUTTONDBLCLK { Ev::Dbl(w.id, k) } else { Ev::Item(w.id, k) };
                                dispatch(hwnd, h, |p| p.event(hwnd, ev));
                            }
                        } else if msg == WM_LBUTTONDBLCLK {
                            // быстрый второй щелчок по кнопке — ещё одно нажатие
                            dispatch(hwnd, h, |p| p.event(hwnd, Ev::Click(w.id)));
                        } else {
                            h.st.borrow_mut().press = Some((w.id, w.kind));
                            SetCapture(hwnd);
                            invalidate(hwnd);
                        }
                    }
                    None => {
                        SetCapture(hwnd);
                        let m = if msg == WM_LBUTTONDBLCLK { Mouse::Dbl } else { Mouse::Down };
                        dispatch(hwnd, h, |p| {
                            p.mouse(hwnd, m, x, y);
                            true
                        });
                    }
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let (x, y) = xy(lp);
                let _ = ReleaseCapture();
                let pressed = h.st.borrow_mut().press.take();
                set_hover(hwnd, h, x, y);
                match pressed {
                    Some((id, kind)) => {
                        let same = {
                            let st = h.st.borrow();
                            st.hover.map(|i| st.ws[i].id == id && st.ws[i].kind == kind).unwrap_or(false)
                        };
                        if same {
                            dispatch(hwnd, h, |p| p.event(hwnd, Ev::Click(id)));
                        }
                        invalidate(hwnd);
                    }
                    None => dispatch(hwnd, h, |p| {
                        p.mouse(hwnd, Mouse::Up, x, y);
                        true
                    }),
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) & 0xFFFF) as i16 as i32;
                let mut pt = POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 };
                let _ = ScreenToClient(hwnd, &mut pt);
                let target = {
                    let st = h.st.borrow();
                    st.ws.iter().rev().find(|w| inside(&w.rc, pt.x, pt.y) && w.enabled && (w.wheel.is_some() || w.kind == Kind::List)).cloned()
                };
                match target {
                    Some(w) if w.kind == Kind::List => {
                        let d = if delta > 0 { -3 } else { 3 };
                        dispatch(hwnd, h, |p| p.event(hwnd, Ev::Scroll(w.id, d)));
                    }
                    Some(w) => {
                        let (up, down) = w.wheel.unwrap();
                        dispatch(hwnd, h, |p| p.event(hwnd, Ev::Click(if delta > 0 { up } else { down })));
                    }
                    None => dispatch(hwnd, h, |p| {
                        p.mouse(hwnd, Mouse::Wheel(delta), pt.x, pt.y);
                        true
                    }),
                }
                set_hover(hwnd, h, pt.x, pt.y);
                LRESULT(0)
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let vk = wp.0 as u16;
                if msg == WM_SYSKEYDOWN && vk != VK_F10.0 {
                    return DefWindowProcW(hwnd, msg, wp, lp);
                }
                let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
                let mut handled = None;
                dispatch(hwnd, h, |p| match p.key(hwnd, vk, ctrl, shift) {
                    Some(k) => {
                        handled = Some(k);
                        k
                    }
                    None => true,
                });
                if handled.is_none() && vk == VK_ESCAPE.0 && !h.owner.is_invalid() {
                    let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                }
                LRESULT(0)
            }
            WM_TIMER => {
                if wp.0 == TIP_TIMER {
                    let _ = KillTimer(hwnd, TIP_TIMER);
                    h.st.borrow_mut().tip = true;
                    invalidate(hwnd);
                } else {
                    let id = wp.0;
                    dispatch(hwnd, h, |p| {
                        p.timer(hwnd, id);
                        true
                    });
                }
                LRESULT(0)
            }
            WM_DROPFILES => {
                let hdrop = HDROP(wp.0 as *mut _);
                let n = DragQueryFileW(hdrop, 0xFFFF_FFFF, None);
                let mut files = vec![];
                for i in 0..n {
                    let len = DragQueryFileW(hdrop, i, None) as usize;
                    let mut buf = vec![0u16; len + 1];
                    DragQueryFileW(hdrop, i, Some(&mut buf));
                    files.push(PathBuf::from(String::from_utf16_lossy(&buf[..len])));
                }
                DragFinish(hdrop);
                let _ = SetForegroundWindow(hwnd);
                dispatch(hwnd, h, |p| {
                    p.drop_files(hwnd, files);
                    true
                });
                LRESULT(0)
            }
            WM_CLOSE => {
                let ok = match h.panel.try_borrow_mut() {
                    Ok(mut p) => p.close(hwnd),
                    Err(_) => false,
                };
                if ok {
                    if !h.owner.is_invalid() {
                        let _ = EnableWindow(h.owner, true);
                        let _ = SetForegroundWindow(h.owner);
                    }
                    let _ = DestroyWindow(hwnd);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                if h.owner.is_invalid() {
                    PostQuitMessage(0);
                }
                LRESULT(0)
            }
            WM_NCDESTROY => {
                let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Host;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                if !p.is_null() {
                    let b = Box::from_raw(p);
                    let st = b.st.borrow();
                    if !st.back.is_invalid() {
                        let _ = DeleteObject(st.back);
                    }
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

const CLASS: PCWSTR = w!("SimplePhotoViewer.Host");

fn register() {
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: LoadIconW(hinst, PCWSTR(1 as *const u16)).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
}

/// Создать окно-хозяина для панели. size — клиентская область в «пикселях 100%».
/// place — сохранённое положение главного окна (x, y, w, h в пикселях экрана).
pub fn create(owner: Option<HWND>, panel: Rc<RefCell<dyn Panel>>, size: (i32, i32), place: Option<RECT>) -> HWND {
    register();
    unsafe {
        let owner_h = owner.unwrap_or_default();
        let dpi = if owner.is_some() { dpi_of(owner_h) } else { GetDpiForSystem().max(96) };
        let style = WS_OVERLAPPEDWINDOW;
        let ex = WINDOW_EX_STYLE(0);
        let mut rc = r(0, 0, size.0 * dpi as i32 / 96, size.1 * dpi as i32 / 96);
        let _ = AdjustWindowRectExForDpi(&mut rc, style, false, ex, dpi);
        let (mut ww, mut wh) = (rc.right - rc.left, rc.bottom - rc.top);
        let (mut x, mut y) = (CW_USEDEFAULT, CW_USEDEFAULT);
        if let Some(p) = place {
            (x, y, ww, wh) = (p.left, p.top, p.right - p.left, p.bottom - p.top);
        } else if owner.is_some() {
            let mut orc = RECT::default();
            let _ = GetWindowRect(owner_h, &mut orc);
            x = (orc.left + (orc.right - orc.left - ww) / 2).max(0);
            y = (orc.top + (orc.bottom - orc.top - wh) / 2).max(0);
        }
        let title = panel.borrow().title();
        let host = Box::new(Host {
            panel,
            st: RefCell::new(St {
                ws: vec![], mouse: (-10000, -10000), hover: None, hover_item: None, press: None,
                back: HBITMAP::default(), bw: 0, bh: 0, tip: false, tracking: false, title: title.clone(),
            }),
            owner: owner_h,
        });
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        CreateWindowExW(
            ex, CLASS, &HSTRING::from(title), style, x, y, ww, wh,
            owner_h, None, hinst, Some(Box::into_raw(host) as *const _),
        )
        .unwrap_or_default()
    }
}

/// Показать панель модальным окном поверх `owner` и вернуть её после закрытия.
pub fn run_modal<P: Panel + 'static>(owner: HWND, p: P, size: (i32, i32)) -> P {
    let rc = Rc::new(RefCell::new(p));
    let dynp: Rc<RefCell<dyn Panel>> = rc.clone();
    let hwnd = create(Some(owner), dynp, size, None);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = EnableWindow(owner, false);
        let mut msg = MSG::default();
        while IsWindow(hwnd).as_bool() {
            let r = GetMessageW(&mut msg, None, 0, 0).0;
            if r == 0 {
                PostQuitMessage(msg.wParam.0 as i32);
                break;
            }
            if r == -1 {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = EnableWindow(owner, true);
    }
    match Rc::try_unwrap(rc) {
        Ok(c) => c.into_inner(),
        Err(_) => panic!("окно ещё живо"),
    }
}

/// Главный цикл сообщений.
pub fn message_loop() {
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

// ------------------------------------------------- помощники раскладки ---
/// Счётчик «− значение +» в строке: возвращает 3 элемента.
pub fn stepper(ui: &Ui, x: i32, y: i32, w: i32, val: String, dec: u32, inc: u32, tip: &str) -> [W; 3] {
    let h = ui.s(32);
    let b = ui.s(32);
    [
        W::new(Kind::Button, dec, r(x, y, b, h)).t("−"),
        W::new(Kind::Value, 0, r(x + b + ui.s(4), y, w - 2 * b - ui.s(8), h)).t(val).wheel(inc, dec).tip(tip),
        W::new(Kind::Button, inc, r(x + w - b, y, b, h)).t("+"),
    ]
}

/// Ряд вариантов выбора равной ширины.
pub fn segments(ui: &Ui, x: i32, y: i32, w: i32, base_id: u32, labels: &[&str], cur: usize) -> Vec<W> {
    let n = labels.len().max(1) as i32;
    let gap = ui.s(4);
    let bw = (w - gap * (n - 1)) / n;
    labels
        .iter()
        .enumerate()
        .map(|(i, l)| W::new(Kind::Seg, base_id + i as u32, r(x + i as i32 * (bw + gap), y, bw, ui.s(32))).t(*l).on(i == cur))
        .collect()
}
