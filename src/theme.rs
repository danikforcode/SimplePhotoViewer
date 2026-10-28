// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Светлая / тёмная тема. Весь интерфейс рисуется самостоятельно, поэтому
//! цвета берутся отсюда. Заголовок окна затемняется через DWM (Windows 10 1809+).

use crate::util::rgb;
use std::cell::Cell;
use windows::core::w;
use windows::Win32::Foundation::{BOOL, COLORREF, HWND};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy)]
pub struct Theme {
    pub dark: bool,
    pub bg: COLORREF,      // фон панелей (шапка, нижняя панель, диалоги)
    pub canvas: COLORREF,  // фон под изображением
    pub card: COLORREF,    // поля ввода, списки
    pub border: COLORREF,
    pub text: COLORREF,
    pub dim: COLORREF,     // второстепенный текст, неактивные кнопки
    pub hover: COLORREF,
    pub press: COLORREF,
    pub sel: COLORREF,     // выделенный элемент / включённый переключатель
    pub accent: COLORREF,
    pub on_accent: COLORREF,
    pub tip_bg: COLORREF,
    pub tip_text: COLORREF,
}

const LIGHT: Theme = Theme {
    dark: false,
    bg: rgb(249, 249, 249),
    canvas: rgb(232, 232, 232),
    card: rgb(255, 255, 255),
    border: rgb(222, 222, 222),
    text: rgb(26, 26, 26),
    dim: rgb(112, 112, 112),
    hover: rgb(234, 234, 234),
    press: rgb(219, 219, 219),
    sel: rgb(221, 234, 248),
    accent: rgb(0, 103, 192),
    on_accent: rgb(255, 255, 255),
    tip_bg: rgb(50, 50, 50),
    tip_text: rgb(245, 245, 245),
};

const DARK: Theme = Theme {
    dark: true,
    bg: rgb(39, 39, 39),
    canvas: rgb(25, 25, 25),
    card: rgb(48, 48, 48),
    border: rgb(62, 62, 62),
    text: rgb(240, 240, 240),
    dim: rgb(160, 160, 160),
    hover: rgb(58, 58, 58),
    press: rgb(70, 70, 70),
    sel: rgb(33, 64, 94),
    accent: rgb(96, 205, 255),
    on_accent: rgb(0, 0, 0),
    tip_bg: rgb(236, 236, 236),
    tip_text: rgb(20, 20, 20),
};

thread_local! {
    static MODE: Cell<Mode> = const { Cell::new(Mode::System) };
    static CUR: Cell<Theme> = const { Cell::new(LIGHT) };
}

/// Системная настройка «Режим приложений: тёмный».
pub fn system_dark() -> bool {
    let mut v: u32 = 1;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    r.is_ok() && v == 0
}

pub fn init() {
    let m = match crate::settings::get_i("theme", 0) {
        1 => Mode::Light,
        2 => Mode::Dark,
        _ => Mode::System,
    };
    set_mode(m);
}

pub fn mode() -> Mode {
    MODE.with(|m| m.get())
}

pub fn set_mode(m: Mode) {
    MODE.with(|c| c.set(m));
    crate::settings::set_i("theme", match m { Mode::System => 0, Mode::Light => 1, Mode::Dark => 2 });
    refresh();
}

/// Пересчитать тему (например, после смены системной). true — тема поменялась.
pub fn refresh() -> bool {
    let dark = match mode() {
        Mode::System => system_dark(),
        Mode::Light => false,
        Mode::Dark => true,
    };
    let old = cur().dark;
    CUR.with(|c| c.set(if dark { DARK } else { LIGHT }));
    old != dark
}

pub fn cur() -> Theme {
    CUR.with(|c| c.get())
}

/// Тёмный заголовок окна (атрибут 20; на ранних сборках Windows 10 — 19).
pub fn apply_titlebar(hwnd: HWND) {
    let v = BOOL::from(cur().dark);
    unsafe {
        let p = &v as *const BOOL as *const _;
        if DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(20), p, 4).is_err() {
            let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(19), p, 4);
        }
    }
}
