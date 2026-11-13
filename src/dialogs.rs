// SimplePhotoViewer (Rust/Win32) — © 2026 danikforcode
// Лицензия MIT (см. LICENSE). Распространяется «как есть», без гарантий.
//! Свои окна вместо системных (MessageBox, ChooseColor): в теме программы,
//! с тем же шрифтом и кнопками, что и всё остальное.
//!   ask      — вопрос с кнопками (Сохранить / Не сохранять / Отмена…);
//!   message  — сообщение или ошибка;
//!   keys     — горячие клавиши (таблица в две колонки);
//!   about    — «О программе»;
//!   pick_color — выбор цвета (поле тон/насыщенность, палитра, #RRGGBB).

use crate::fields::Fields;
use crate::gui::{self, Ev, Kind, Mouse, Panel, Ui, W};
use crate::theme;
use crate::util::rgb;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const APP: &str = "SimplePhotoViewer";
/// Номер сборки внутри версии (растёт с каждой выданной сборкой).
pub const BUILD: u32 = 1;

#[derive(Clone, Copy, PartialEq)]
pub enum Icon {
    None,
    Info,
    Warn,
    Error,
    Question,
}

const ID_BTN0: u32 = 10;

// ------------------------------------------------------------- вопрос ---
struct Ask {
    title: String,
    text: String,
    icon: Icon,
    buttons: Vec<String>,
    default: usize,
    out: Option<usize>,
}

/// Ширина текста сообщения (в «пикселях 100%»).
const TEXT_W: i32 = 400;

impl Ask {
    fn icon_glyph(&self) -> Option<(char, COLORREF)> {
        let t = theme::cur();
        match self.icon {
            Icon::None => None,
            Icon::Info => Some(('i', t.accent)),
            Icon::Question => Some(('?', t.accent)),
            Icon::Warn => Some(('!', rgb(230, 160, 20))),
            Icon::Error => Some(('×', rgb(220, 60, 60))),
        }
    }
}

impl Panel for Ask {
    fn title(&self) -> String {
        self.title.clone()
    }
    fn resizable(&self) -> bool {
        false
    }
    fn min_size(&self) -> (i32, i32) {
        (300, 120)
    }
    fn layout(&mut self, ui: &Ui, w: i32, h: i32) -> Vec<W> {
        let m = ui.s(22);
        let by = h - m - ui.s(34);
        let mut x = w - m;
        let mut v = vec![];
        // справа налево: последняя кнопка — крайняя справа
        for (i, b) in self.buttons.iter().enumerate().rev() {
            let bw = (ui.tw(ui.f.text, b) + ui.s(36)).max(ui.s(96));
            x -= bw;
            v.push(W::new(Kind::Button, ID_BTN0 + i as u32, gui::r(x, by, bw, ui.s(34))).t(b.clone()).on(i == self.default));
            x -= ui.s(8);
        }
        v
    }
    fn paint(&mut self, ui: &Ui, hdc: HDC, w: i32, h: i32) {
        let t = &ui.t;
        gui::fill(hdc, &gui::r(0, 0, w, h), t.bg);
        let m = ui.s(22);
        let mut x = m;
        if let Some((g, c)) = self.icon_glyph() {
            let d = ui.s(36);
            let rc = gui::r(m, m, d, d);
            gui::round(hdc, &rc, d, Some(c), None);
            gui::text(hdc, &g.to_string(), &rc, ui.f.big, rgb(255, 255, 255), gui::CENTER);
            x += d + ui.s(16);
        }
        // бар снизу под кнопки
        let bar = gui::r(0, h - m - ui.s(34) - ui.s(18), w, h);
        gui::fill(hdc, &bar, gui::blend(t.bg, t.canvas, 0.5));
        let tw = w - x - m;
        let th = ui.th(ui.f.body, &self.text, tw);
        let ty = if self.icon != Icon::None { m + (ui.s(36) - th).max(0) / 2 } else { m };
        gui::text(hdc, &self.text, &gui::r(x, ty.max(m), tw, th + ui.s(4)), ui.f.body, t.text, gui::PARA);
    }
    fn event(&mut self, _hwnd: HWND, ev: Ev) -> bool {
        if let Ev::Click(id) = ev {
            if (ID_BTN0..ID_BTN0 + self.buttons.len() as u32).contains(&id) {
                self.out = Some((id - ID_BTN0) as usize);
                return false;
            }
        }
        true
    }
    fn key(&mut self, hwnd: HWND, vk: u16, ctrl: bool, _s: bool) -> Option<bool> {
        match vk {
            0x0D => {
                self.out = Some(self.default);
                Some(false)
            }
            0x1B => Some(false),
            // Ctrl+C — текст сообщения в буфер (как у системного окна)
            0x43 if ctrl => {
                crate::clip::set_text(hwnd, &self.text);
                Some(true)
            }
            _ => None,
        }
    }
}

/// Спросить; вернуть номер нажатой кнопки (None — закрыли окно / Esc).
pub fn ask(owner: Option<HWND>, icon: Icon, title: &str, text: &str, buttons: &[&str], default: usize) -> Option<usize> {
    // размер под текст (в «пикселях 100%»)
    let ui = Ui::new(96);
    let icon_w = if icon == Icon::None { 0 } else { 52 };
    let long = text.chars().count() > 260;
    let tw = if long { TEXT_W + 160 } else { TEXT_W };
    let th = ui.th(ui.f.body, text, tw).min(560);
    let bw: i32 = buttons.iter().map(|b| (ui.tw(ui.f.text, b) + 36).max(96) + 8).sum::<i32>() + 44;
    let w = (44 + icon_w + tw).max(bw).max(340);
    let h = 22 + th.max(if icon == Icon::None { 0 } else { 36 }) + 30 + 34 + 22;
    let owner = owner.filter(|h| unsafe { IsWindow(*h).as_bool() }).unwrap_or_default();
    let p = Ask {
        title: if title.is_empty() { APP.into() } else { title.into() },
        text: text.into(),
        icon,
        buttons: buttons.iter().map(|s| s.to_string()).collect(),
        default,
        out: None,
    };
    gui::run_modal(owner, p, (w, h)).out
}

/// Сообщение с одной кнопкой «OK».
pub fn message(owner: Option<HWND>, text: &str, error: bool) {
    let _ = ask(owner, if error { Icon::Error } else { Icon::Info }, APP, text, &["OK"], 0);
}

/// «Да / Нет» → true, если «Да».
pub fn confirm(owner: Option<HWND>, title: &str, text: &str, yes: &str, no: &str) -> bool {
    ask(owner, Icon::Question, title, text, &[yes, no], 0) == Some(0)
}

/// «Сохранить / Не сохранять / Отмена»: Some(true) — сохранить, Some(false) — нет, None — отмена.
pub fn save_changes(owner: Option<HWND>, text: &str) -> Option<bool> {
    match ask(owner, Icon::Question, APP, text, &["Сохранить", "Не сохранять", "Отмена"], 0) {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

// ---------------------------------------------------- горячие клавиши ---
struct Keys {
    /// (заголовок раздела или пусто, клавиши, что делают)
    rows: Vec<(String, String, String)>,
}

fn parse_keys(text: &str) -> Vec<(String, String, String)> {
    let mut rows = vec![];
    for line in text.lines() {
        let l = line.trim_end();
        if l.trim().is_empty() {
            rows.push((String::new(), String::new(), String::new()));
            continue;
        }
        let up = l.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase());
        if up {
            rows.push((l.trim().to_string(), String::new(), String::new()));
        } else if let Some((k, d)) = l.split_once(" — ") {
            rows.push((String::new(), k.trim().to_string(), d.trim().to_string()));
        } else if let Some((k, d)) = l.split_once(": ") {
            rows.push((String::new(), k.trim().to_string(), d.trim().to_string()));
        } else {
            rows.push((String::new(), String::new(), l.trim().to_string()));
        }
    }
    rows
}

impl Keys {
    /// Разбить строки на две колонки по пустой строке ближе к середине.
    fn columns(&self) -> (usize, usize) {
        let n = self.rows.len();
        let mid = n / 2;
        let split = (0..n).filter(|&i| self.rows[i].1.is_empty() && self.rows[i].0.is_empty() && self.rows[i].2.is_empty()).min_by_key(|&i| (i as isize - mid as isize).abs()).unwrap_or(mid);
        (split, n)
    }
}

impl Panel for Keys {
    fn title(&self) -> String {
        "Горячие клавиши".into()
    }
    fn min_size(&self) -> (i32, i32) {
        (760, 520)
    }
    fn layout(&mut self, ui: &Ui, w: i32, h: i32) -> Vec<W> {
        let m = ui.s(20);
        vec![W::new(Kind::Button, ID_BTN0, gui::r(w - m - ui.s(110), h - m - ui.s(34), ui.s(110), ui.s(34))).t("Закрыть").on(true)]
    }
    fn paint(&mut self, ui: &Ui, hdc: HDC, w: i32, h: i32) {
        let t = &ui.t;
        gui::fill(hdc, &gui::r(0, 0, w, h), t.bg);
        let m = ui.s(20);
        let (split, n) = self.columns();
        let colw = (w - 3 * m) / 2;
        let kw = ui.s(165).min(colw / 3);
        for (c, range) in [(0, 0..split), (1, split + 1..n)] {
            let x = m + c * (colw + m);
            let mut y = m;
            for i in range {
                let (head, k, d) = &self.rows[i];
                if !head.is_empty() {
                    gui::text(hdc, head, &gui::r(x, y, colw, ui.s(24)), ui.f.bold, t.accent, gui::LEFT);
                    y += ui.s(26);
                } else if k.is_empty() && d.is_empty() {
                    y += ui.s(10);
                } else {
                    let dx = if k.is_empty() { x + kw + ui.s(10) } else { x + kw + ui.s(10) };
                    let dh = ui.th(ui.f.text, d, colw - kw - ui.s(10)).max(ui.s(18));
                    if !k.is_empty() {
                        let pill = gui::r(x, y, kw, ui.s(20));
                        gui::text(hdc, k, &pill, ui.f.bold, t.text, gui::LEFT);
                    }
                    gui::text(hdc, d, &gui::r(dx, y + ui.s(1), colw - kw - ui.s(10), dh + ui.s(4)), ui.f.text, t.dim, gui::PARA);
                    y += dh + ui.s(6);
                }
            }
        }
    }
    fn event(&mut self, _hwnd: HWND, ev: Ev) -> bool {
        !matches!(ev, Ev::Click(ID_BTN0))
    }
    fn key(&mut self, _hwnd: HWND, vk: u16, _c: bool, _s: bool) -> Option<bool> {
        matches!(vk, 0x0D | 0x1B).then_some(false)
    }
}

pub fn keys(owner: HWND, text: &str) {
    gui::run_modal(owner, Keys { rows: parse_keys(text) }, (980, 660));
}

// ---------------------------------------------------- о программе ---
struct About {
    icon: HICON,
}

const ID_LIC: u32 = 20;

const OFFLINE: &str = "Работает полностью без интернета: никакой телеметрии, сбора статистики, проверки обновлений, облачных функций и ИИ. Файлы и настройки хранятся только на этом компьютере.";
const WARRANTY: &str = "Программа распространяется по лицензии MIT «как есть», без каких-либо гарантий. Автор не несёт ответственности за ущерб, связанный с её использованием.";
const PARTS: &str = "Используются системные компоненты Windows (WIC, GDI+, Media Foundation, Windows.Data.Pdf) и открытые библиотеки с лицензиями MIT, Apache 2.0 и BSD — список в «Лицензии».";

impl Panel for About {
    fn title(&self) -> String {
        "О программе".into()
    }
    fn resizable(&self) -> bool {
        false
    }
    fn init(&mut self, _hwnd: HWND) {
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap_or_default();
            let sz = 256;
            self.icon = LoadImageW(hinst, PCWSTR(1 as *const u16), IMAGE_ICON, sz, sz, LR_DEFAULTCOLOR).map(|h| HICON(h.0)).unwrap_or_default();
        }
    }
    fn min_size(&self) -> (i32, i32) {
        (600, 460)
    }
    fn layout(&mut self, ui: &Ui, w: i32, h: i32) -> Vec<W> {
        let m = ui.s(28);
        let by = h - m - ui.s(36);
        vec![
            W::new(Kind::Button, ID_BTN0, gui::r(w - m - ui.s(130), by, ui.s(130), ui.s(36))).t("Закрыть"),
            W::new(Kind::Button, ID_LIC, gui::r(w - m - ui.s(130) - ui.s(10) - ui.s(130), by, ui.s(130), ui.s(36))).t("Лицензии"),
        ]
    }
    fn paint(&mut self, ui: &Ui, hdc: HDC, w: i32, h: i32) {
        let t = &ui.t;
        gui::fill(hdc, &gui::r(0, 0, w, h), t.bg);
        let m = ui.s(28);
        let cw = w - 2 * m;
        // полоса акцентного цвета сверху
        gui::fill(hdc, &gui::r(m, m, cw, ui.s(6)), t.accent);
        // значок и название
        let isz = ui.s(96);
        let iy = m + ui.s(30);
        if !self.icon.is_invalid() {
            unsafe {
                let _ = DrawIconEx(hdc, m, iy, self.icon, isz, isz, 0, None, DI_NORMAL);
            }
        }
        let tx = m + isz + ui.s(24);
        gui::text(hdc, APP, &gui::r(tx, iy + ui.s(12), w - tx - m, ui.s(40)), ui.f.title, t.text, gui::LEFT);
        let ver = format!("Версия {}, сборка {}", env!("CARGO_PKG_VERSION"), BUILD);
        gui::text(hdc, &ver, &gui::r(tx, iy + ui.s(54), w - tx - m, ui.s(24)), ui.f.body, t.dim, gui::LEFT);
        // текст
        let mut y = iy + isz + ui.s(22);
        for (s, c, gap) in [
            ("Разработчик: danikforcode", t.text, 4),
            ("© 2026 danikforcode. Лицензия MIT.", t.text, 18),
            (OFFLINE, t.text, 18),
            (WARRANTY, t.dim, 12),
            (PARTS, t.dim, 0),
        ] {
            let th = ui.th(ui.f.body, s, cw);
            gui::text(hdc, s, &gui::r(m, y, cw, th + ui.s(4)), ui.f.body, c, gui::PARA);
            y += th + ui.s(gap);
        }
    }
    fn event(&mut self, hwnd: HWND, ev: Ev) -> bool {
        match ev {
            Ev::Click(ID_BTN0) => false,
            Ev::Click(ID_LIC) => {
                licenses(hwnd);
                true
            }
            _ => true,
        }
    }
    fn key(&mut self, _hwnd: HWND, vk: u16, _c: bool, _s: bool) -> Option<bool> {
        matches!(vk, 0x0D | 0x1B).then_some(false)
    }
}

impl Drop for About {
    fn drop(&mut self) {
        if !self.icon.is_invalid() {
            unsafe {
                let _ = DestroyIcon(self.icon);
            }
        }
    }
}

pub fn about(owner: HWND) {
    // высота — под текст при ширине окна 680
    let ui = Ui::new(96);
    let cw = 680 - 56;
    let text_h: i32 = [OFFLINE, WARRANTY, PARTS].iter().map(|s| ui.th(ui.f.body, s, cw)).sum::<i32>() + 2 * 22 + 52;
    let h = 28 + 30 + 96 + 22 + text_h + 24 + 36 + 28;
    gui::run_modal(owner, About { icon: HICON::default() }, (680, h));
}

/// Сторонние компоненты и их лицензии (полные тексты — в файле THIRD-PARTY.md рядом с исходниками).
pub const THIRD_PARTY: &str = include_str!("../THIRD-PARTY.md");

struct Licenses {
    rows: Vec<(String, String)>,
    scroll: i32,
    content_h: i32,
}

const ID_LIST_SCROLL: u32 = 30;

impl Panel for Licenses {
    fn title(&self) -> String {
        "Лицензии компонентов".into()
    }
    fn min_size(&self) -> (i32, i32) {
        (560, 420)
    }
    fn layout(&mut self, ui: &Ui, w: i32, h: i32) -> Vec<W> {
        let m = ui.s(20);
        let _ = ID_LIST_SCROLL;
        vec![W::new(Kind::Button, ID_BTN0, gui::r(w - m - ui.s(110), h - m - ui.s(34), ui.s(110), ui.s(34))).t("Закрыть").on(true)]
    }
    fn paint(&mut self, ui: &Ui, hdc: HDC, w: i32, h: i32) {
        let t = &ui.t;
        gui::fill(hdc, &gui::r(0, 0, w, h), t.bg);
        let m = ui.s(20);
        let view = gui::r(m, m, w - 2 * m, h - 3 * m - ui.s(34));
        unsafe {
            let rg = CreateRectRgnIndirect(&view);
            SelectClipRgn(hdc, rg);
            let _ = DeleteObject(rg);
        }
        let mut y = view.top - self.scroll;
        let cw = view.right - view.left;
        for (name, body) in &self.rows {
            if !name.is_empty() {
                gui::text(hdc, name, &gui::r(view.left, y, cw, ui.s(24)), ui.f.bold, t.text, gui::LEFT);
                y += ui.s(26);
            }
            let bw = cw - ui.s(12);
            let bh = ui.th(ui.f.text, body, bw);
            gui::text(hdc, body, &gui::r(view.left, y, bw, bh + ui.s(4)), ui.f.text, t.dim, gui::PARA);
            y += bh + ui.s(14);
        }
        self.content_h = y + self.scroll - view.top;
        unsafe {
            SelectClipRgn(hdc, None);
        }
    }
    fn mouse(&mut self, _hwnd: HWND, m: Mouse, _x: i32, _y: i32) -> bool {
        if let Mouse::Wheel(d) = m {
            self.scroll = (self.scroll - d.signum() * 60).clamp(0, (self.content_h - 300).max(0));
            return true;
        }
        false
    }
    fn event(&mut self, _hwnd: HWND, ev: Ev) -> bool {
        !matches!(ev, Ev::Click(ID_BTN0))
    }
    fn key(&mut self, _hwnd: HWND, vk: u16, _c: bool, _s: bool) -> Option<bool> {
        match vk {
            0x0D | 0x1B => Some(false),
            0x22 | 0x28 => {
                self.scroll = (self.scroll + if vk == 0x22 { 300 } else { 40 }).min((self.content_h - 300).max(0));
                Some(true)
            }
            0x21 | 0x26 => {
                self.scroll = (self.scroll - if vk == 0x21 { 300 } else { 40 }).max(0);
                Some(true)
            }
            _ => None,
        }
    }
}

/// Разобрать THIRD-PARTY.md: «## Название» — заголовок, дальше — текст.
fn license_rows() -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = vec![];
    for line in THIRD_PARTY.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            rows.push((h.trim().to_string(), String::new()));
        } else if line.starts_with("# ") {
            continue;
        } else {
            if rows.is_empty() {
                rows.push((String::new(), String::new()));
            }
            let b = &mut rows.last_mut().unwrap().1;
            if line.trim().is_empty() {
                if !b.ends_with("\n\n") && !b.is_empty() {
                    b.push_str("\n\n");
                }
            } else {
                if !b.is_empty() && !b.ends_with('\n') {
                    b.push(' ');
                }
                b.push_str(line.trim());
            }
        }
    }
    for r in &mut rows {
        r.1 = r.1.trim().to_string();
    }
    rows
}

pub fn licenses(owner: HWND) {
    gui::run_modal(owner, Licenses { rows: license_rows(), scroll: 0, content_h: 0 }, (760, 620));
}

// ---------------------------------------------------- выбор цвета ---
const ID_OK: u32 = 40;
const ID_CANCEL: u32 = 41;
const ID_HEX: u32 = 1301;
const ID_SW0: u32 = 100;

/// Палитра: основные цвета и оттенки серого.
const PALETTE: [u32; 24] = [
    0x000000, 0x434343, 0x808080, 0xB7B7B7, 0xE0E0E0, 0xFFFFFF, //
    0xE53935, 0xFB8C00, 0xFDD835, 0x43A047, 0x1E88E5, 0x8E24AA, //
    0xB71C1C, 0xE65100, 0xF9A825, 0x1B5E20, 0x0D47A1, 0x4A148C, //
    0xF8BBD0, 0xFFE0B2, 0xFFF9C4, 0xC8E6C9, 0xBBDEFB, 0xE1BEE7,
];

struct ColorDlg {
    h: f64,
    s: f64,
    v: f64,
    old: u32,
    out: Option<u32>,
    sv_rc: RECT,
    hue_rc: RECT,
    drag: u8, // 1 — поле, 2 — тон
    fields: Option<Fields>,
    recent: Vec<u32>,
    sv_cache: Option<(i32, i32, i32, HBITMAP)>,
}

fn hsv_to_rgb(h: f64, s: f64, v: f64) -> u32 {
    let h = (h.rem_euclid(1.0)) * 6.0;
    let i = h.floor() as i32;
    let f = h - i as f64;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let (r, g, b) = match i % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    let c = |x: f64| (x * 255.0).round().clamp(0.0, 255.0) as u32;
    (c(r) << 16) | (c(g) << 8) | c(b)
}

fn rgb_to_hsv(c: u32) -> (f64, f64, f64) {
    let (r, g, b) = (((c >> 16) & 255) as f64 / 255.0, ((c >> 8) & 255) as f64 / 255.0, (c & 255) as f64 / 255.0);
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let d = mx - mn;
    let h = if d == 0.0 {
        0.0
    } else if mx == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if mx == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    (h, if mx == 0.0 { 0.0 } else { d / mx }, mx)
}

fn cref(c: u32) -> COLORREF {
    rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

pub fn parse_hex(s: &str) -> Option<u32> {
    let s = s.trim().trim_start_matches('#');
    let s = if s.len() == 3 { s.chars().flat_map(|c| [c, c]).collect::<String>() } else { s.to_string() };
    if s.len() != 6 {
        return None;
    }
    u32::from_str_radix(&s, 16).ok()
}

impl ColorDlg {
    fn cur(&self) -> u32 {
        hsv_to_rgb(self.h, self.s, self.v)
    }
    fn set(&mut self, c: u32) {
        let (h, s, v) = rgb_to_hsv(c);
        // у серых тон не определён — оставляем прежний
        if s > 0.0 {
            self.h = h;
        }
        self.s = s;
        self.v = v;
        self.sync_hex();
    }
    fn sync_hex(&self) {
        if let Some(f) = &self.fields {
            f.set(0, &format!("#{:06X}", self.cur()));
        }
    }
    fn pick(&mut self, x: i32, y: i32) {
        match self.drag {
            1 => {
                let r = self.sv_rc;
                self.s = ((x - r.left) as f64 / (r.right - r.left - 1).max(1) as f64).clamp(0.0, 1.0);
                self.v = 1.0 - ((y - r.top) as f64 / (r.bottom - r.top - 1).max(1) as f64).clamp(0.0, 1.0);
            }
            2 => {
                let r = self.hue_rc;
                self.h = ((y - r.top) as f64 / (r.bottom - r.top - 1).max(1) as f64).clamp(0.0, 0.9999);
            }
            _ => return,
        }
        self.sync_hex();
    }
    /// Поле «насыщенность × яркость» для текущего тона — картинкой (кэш по тону и размеру).
    fn sv_bitmap(&mut self, hdc: HDC) -> HBITMAP {
        let (w, h) = (self.sv_rc.right - self.sv_rc.left, self.sv_rc.bottom - self.sv_rc.top);
        let key = (self.h * 3600.0) as i32;
        if let Some((k, cw, ch, b)) = self.sv_cache {
            if k == key && cw == w && ch == h {
                return b;
            }
            unsafe {
                let _ = DeleteObject(b);
            }
        }
        let mut px = vec![0u8; (w * h * 4).max(4) as usize];
        for y in 0..h {
            let v = 1.0 - y as f64 / (h - 1).max(1) as f64;
            for x in 0..w {
                let s = x as f64 / (w - 1).max(1) as f64;
                let c = hsv_to_rgb(self.h, s, v);
                let i = ((y * w + x) * 4) as usize;
                px[i] = c as u8;
                px[i + 1] = (c >> 8) as u8;
                px[i + 2] = (c >> 16) as u8;
            }
        }
        let b = unsafe {
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, ..Default::default() },
                ..Default::default()
            };
            CreateDIBitmap(hdc, Some(&bmi.bmiHeader), CBM_INIT as u32, Some(px.as_ptr() as *const _), Some(&bmi), DIB_RGB_COLORS)
        };
        self.sv_cache = Some((key, w, h, b));
        b
    }
}

impl Drop for ColorDlg {
    fn drop(&mut self) {
        if let Some((_, _, _, b)) = self.sv_cache.take() {
            unsafe {
                let _ = DeleteObject(b);
            }
        }
    }
}

impl Panel for ColorDlg {
    fn title(&self) -> String {
        "Цвет".into()
    }
    fn resizable(&self) -> bool {
        false
    }
    fn init(&mut self, hwnd: HWND) {
        let f = Fields::new(hwnd, &[ID_HEX], true);
        self.fields = Some(f);
        self.sync_hex();
    }
    fn min_size(&self) -> (i32, i32) {
        (520, 380)
    }
    fn layout(&mut self, ui: &Ui, w: i32, h: i32) -> Vec<W> {
        let m = ui.s(20);
        let side = ui.s(240);
        self.sv_rc = gui::r(m, m, side, side);
        self.hue_rc = gui::r(m + side + ui.s(12), m, ui.s(22), side);
        let px = self.hue_rc.right + ui.s(20);
        let pw = w - px - m;
        let mut v = vec![];
        // палитра 6 × 4
        let sz = ((pw - 5 * ui.s(6)) / 6).min(ui.s(32));
        for (i, c) in PALETTE.iter().enumerate() {
            let (cx, cy) = ((i % 6) as i32, (i / 6) as i32);
            v.push(W::new(Kind::Swatch, ID_SW0 + i as u32, gui::r(px + cx * (sz + ui.s(6)), m + cy * (sz + ui.s(6)), sz, sz)).color(cref(*c)).on(*c == self.cur()));
        }
        let mut y = m + 4 * (sz + ui.s(6)) + ui.s(8);
        if !self.recent.is_empty() {
            v.push(W::new(Kind::Group, 0, gui::r(px, y, pw, ui.s(20))).t("Недавние"));
            y += ui.s(22);
            for (i, c) in self.recent.iter().take(6).enumerate() {
                v.push(W::new(Kind::Swatch, ID_SW0 + 50 + i as u32, gui::r(px + i as i32 * (sz + ui.s(6)), y, sz, sz)).color(cref(*c)).on(*c == self.cur()));
            }
            y += sz + ui.s(12);
        }
        if let Some(f) = &self.fields {
            v.push(f.place(ui, 0, gui::r(px, y, ui.s(120), ui.s(32)), true));
        }
        let by = h - m - ui.s(34);
        v.push(W::new(Kind::Button, ID_OK, gui::r(w - m - ui.s(110), by, ui.s(110), ui.s(34))).t("Готово").on(true));
        v.push(W::new(Kind::Button, ID_CANCEL, gui::r(w - m - ui.s(220), by, ui.s(100), ui.s(34))).t("Отмена"));
        v
    }
    fn paint(&mut self, ui: &Ui, hdc: HDC, w: i32, h: i32) {
        let t = &ui.t;
        gui::fill(hdc, &gui::r(0, 0, w, h), t.bg);
        // поле
        let b = self.sv_bitmap(hdc);
        unsafe {
            let mdc = CreateCompatibleDC(hdc);
            let old = SelectObject(mdc, b);
            let r = self.sv_rc;
            let _ = BitBlt(hdc, r.left, r.top, r.right - r.left, r.bottom - r.top, mdc, 0, 0, SRCCOPY);
            SelectObject(mdc, old);
            let _ = DeleteDC(mdc);
        }
        gui::frame(hdc, &self.sv_rc, t.border, PS_SOLID);
        // полоса тона
        let hr = self.hue_rc;
        for y in hr.top..hr.bottom {
            let c = hsv_to_rgb((y - hr.top) as f64 / (hr.bottom - hr.top) as f64, 1.0, 1.0);
            gui::fill(hdc, &RECT { left: hr.left, top: y, right: hr.right, bottom: y + 1 }, cref(c));
        }
        gui::frame(hdc, &hr, t.border, PS_SOLID);
        // метки
        let sx = self.sv_rc.left + (self.s * (self.sv_rc.right - self.sv_rc.left - 1) as f64) as i32;
        let sy = self.sv_rc.top + ((1.0 - self.v) * (self.sv_rc.bottom - self.sv_rc.top - 1) as f64) as i32;
        let d = ui.s(7);
        gui::round(hdc, &gui::r(sx - d, sy - d, 2 * d, 2 * d), 2 * d, None, Some(rgb(255, 255, 255)));
        gui::round(hdc, &gui::r(sx - d - 1, sy - d - 1, 2 * d + 2, 2 * d + 2), 2 * d + 2, None, Some(rgb(0, 0, 0)));
        let hy = hr.top + (self.h * (hr.bottom - hr.top) as f64) as i32;
        gui::frame(hdc, &RECT { left: hr.left - ui.s(3), top: hy - ui.s(3), right: hr.right + ui.s(3), bottom: hy + ui.s(3) }, t.text, PS_SOLID);
        // было → стало
        let py = self.sv_rc.bottom + ui.s(14);
        let pw = ui.s(60);
        gui::text(hdc, "Было", &gui::r(self.sv_rc.left, py, pw, ui.s(18)), ui.f.small, t.dim, gui::LEFT);
        gui::text(hdc, "Стало", &gui::r(self.sv_rc.left + pw + ui.s(8), py, pw, ui.s(18)), ui.f.small, t.dim, gui::LEFT);
        gui::round(hdc, &gui::r(self.sv_rc.left, py + ui.s(20), pw, ui.s(30)), ui.s(6), Some(cref(self.old)), Some(t.border));
        gui::round(hdc, &gui::r(self.sv_rc.left + pw + ui.s(8), py + ui.s(20), pw, ui.s(30)), ui.s(6), Some(cref(self.cur())), Some(t.border));
    }
    fn mouse(&mut self, _hwnd: HWND, m: Mouse, x: i32, y: i32) -> bool {
        match m {
            Mouse::Down => {
                self.drag = if gui::inside(&self.sv_rc, x, y) {
                    1
                } else if gui::inside(&RECT { left: self.hue_rc.left - 4, right: self.hue_rc.right + 4, ..self.hue_rc }, x, y) {
                    2
                } else {
                    0
                };
                self.pick(x, y);
                self.drag != 0
            }
            Mouse::Move if self.drag != 0 => {
                self.pick(x, y);
                true
            }
            Mouse::Up => {
                let was = self.drag != 0;
                self.drag = 0;
                was
            }
            Mouse::Dbl if gui::inside(&self.sv_rc, x, y) => {
                self.out = Some(self.cur());
                false
            }
            _ => false,
        }
    }
    fn event(&mut self, _hwnd: HWND, ev: Ev) -> bool {
        match ev {
            Ev::Click(ID_OK) => {
                if let Some(c) = self.fields.as_ref().and_then(|f| parse_hex(&f.get(0))) {
                    self.set(c);
                }
                self.out = Some(self.cur());
                false
            }
            Ev::Click(ID_CANCEL) => false,
            Ev::Click(i) if (ID_SW0..ID_SW0 + PALETTE.len() as u32).contains(&i) => {
                self.set(PALETTE[(i - ID_SW0) as usize]);
                true
            }
            Ev::Click(i) if (ID_SW0 + 50..ID_SW0 + 56).contains(&i) => {
                if let Some(c) = self.recent.get((i - ID_SW0 - 50) as usize).copied() {
                    self.set(c);
                }
                true
            }
            _ => true,
        }
    }
    fn key(&mut self, hwnd: HWND, vk: u16, _c: bool, _s: bool) -> Option<bool> {
        match vk {
            0x0D => Some(self.event(hwnd, Ev::Click(ID_OK))),
            0x1B => Some(false),
            _ => None,
        }
    }
    fn raw(&mut self, _hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        if msg == WM_COMMAND && (wp.0 & 0xFFFF) as u32 == ID_HEX && (wp.0 >> 16) as u32 == crate::fields::EN_CHANGE {
            // ввод #RRGGBB — сразу на поле
            if let Some(c) = self.fields.as_ref().and_then(|f| parse_hex(&f.get(0))) {
                if c != self.cur() {
                    let (h, s, v) = rgb_to_hsv(c);
                    if s > 0.0 {
                        self.h = h;
                    }
                    self.s = s;
                    self.v = v;
                    return Some(LRESULT(0));
                }
            }
            return None;
        }
        if msg == WM_CTLCOLOREDIT || msg == WM_CTLCOLORSTATIC {
            let t = theme::cur();
            return self.fields.as_mut()?.ctl_color(&t, HDC(wp.0 as *mut _), HWND(lp.0 as *mut _));
        }
        None
    }
}

/// Выбрать цвет (0xRRGGBB). Последние выбранные запоминаются.
pub fn pick_color(owner: HWND, init: u32) -> Option<u32> {
    let recent: Vec<u32> = crate::settings::get_s("recent_colors").unwrap_or_default().split(',').filter_map(parse_hex).collect();
    let (h, s, v) = rgb_to_hsv(init);
    let d = ColorDlg { h, s, v, old: init, out: None, sv_rc: RECT::default(), hue_rc: RECT::default(), drag: 0, fields: None, recent, sv_cache: None };
    let out = gui::run_modal(owner, d, (560, 400));
    if let Some(c) = out.out {
        let mut r: Vec<u32> = vec![c];
        r.extend(out.recent.iter().copied().filter(|&x| x != c));
        r.truncate(6);
        crate::settings::set_s("recent_colors", &r.iter().map(|c| format!("{c:06X}")).collect::<Vec<_>>().join(","));
    }
    out.out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_roundtrip() {
        for c in [0x000000, 0xFFFFFF, 0xE53935, 0x1E88E5, 0x43A047, 0x808080, 0x123456] {
            let (h, s, v) = rgb_to_hsv(c);
            assert_eq!(hsv_to_rgb(h, s, v), c, "{c:06X}");
        }
        assert_eq!(parse_hex("#fa0"), Some(0xFFAA00));
        assert_eq!(parse_hex(" 1e88e5 "), Some(0x1E88E5));
        assert_eq!(parse_hex("#12345"), None);
    }

    #[test]
    fn keys_table() {
        let rows = parse_keys("ДВА РЕЖИМА\nCtrl+O — открыть\n\nМедиа: пробел — пуск");
        assert_eq!(rows[0].0, "ДВА РЕЖИМА");
        assert_eq!((rows[1].1.as_str(), rows[1].2.as_str()), ("Ctrl+O", "открыть"));
        assert!(rows[2].0.is_empty() && rows[2].1.is_empty());
        assert_eq!(rows[3].1, "Медиа: пробел");
    }

    #[test]
    fn license_list_parsed() {
        let r = license_rows();
        assert!(r.iter().any(|x| x.0.contains("heic-decoder")), "{:?}", r.iter().map(|x| &x.0).collect::<Vec<_>>());
    }
}
