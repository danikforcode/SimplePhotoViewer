// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Поля ввода (системный EDIT) для «самонарисованных» окон: рамку рисует
//! само окно (элемент Value), а EDIT без рамки лежит внутри.

use crate::gui::{Ui, W};
use crate::theme::Theme;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const EN_CHANGE: u32 = 0x0300;

pub struct Fields {
    pub boxes: Vec<HWND>,
    pub ids: Vec<u32>,
    brush: HBRUSH,
    brush_color: u32,
}

impl Fields {
    pub fn new(parent: HWND, ids: &[u32], numeric: bool) -> Fields {
        let mut boxes = vec![];
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap_or_default();
            for &id in ids {
                let mut style = WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(0x0080 /* ES_AUTOHSCROLL */);
                if numeric {
                    style |= WINDOW_STYLE(0x0001 /* ES_CENTER */);
                }
                let h = CreateWindowExW(WINDOW_EX_STYLE(0), w!("EDIT"), w!(""), style, 0, 0, 200, 24, parent, HMENU(id as usize as *mut _), hinst, None)
                    .unwrap_or_default();
                boxes.push(h);
            }
        }
        Fields { boxes, ids: ids.to_vec(), brush: HBRUSH::default(), brush_color: u32::MAX }
    }

    /// Разместить поле i в рамке rc; возвращает виджет-рамку.
    pub fn place(&self, ui: &Ui, i: usize, rc: RECT, enabled: bool) -> W {
        let h = self.boxes[i];
        let pad = ui.s(6);
        let fh = ui.s(20);
        unsafe {
            let _ = MoveWindow(h, rc.left + pad, (rc.top + rc.bottom - fh) / 2, rc.right - rc.left - 2 * pad, fh, true);
            SendMessageW(h, WM_SETFONT, WPARAM(ui.f.text.0 as usize), LPARAM(1));
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(h, enabled);
        }
        W::new(crate::gui::Kind::Value, 0, rc).en(enabled)
    }

    pub fn get(&self, i: usize) -> String {
        unsafe {
            let n = GetWindowTextLengthW(self.boxes[i]);
            let mut buf = vec![0u16; n as usize + 1];
            let k = GetWindowTextW(self.boxes[i], &mut buf);
            String::from_utf16_lossy(&buf[..k as usize])
        }
    }

    pub fn set(&self, i: usize, s: &str) {
        if self.get(i) == s {
            return;
        }
        unsafe {
            let _ = SetWindowTextW(self.boxes[i], &HSTRING::from(s));
        }
    }

    /// Число из поля (запятая тоже годится).
    pub fn num(&self, i: usize) -> Option<f64> {
        self.get(i).trim().replace(',', ".").replace(' ', "").parse::<f64>().ok().filter(|v| v.is_finite())
    }

    pub fn index_of(&self, id: u32) -> Option<usize> {
        self.ids.iter().position(|&x| x == id)
    }

    pub fn focus(&self, i: usize) {
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(self.boxes[i]);
            SendMessageW(self.boxes[i], 0x00B1 /* EM_SETSEL */, WPARAM(0), LPARAM(-1));
        }
    }

    /// Цвета поля под тему (ответ на WM_CTLCOLOREDIT).
    pub fn ctl_color(&mut self, t: &Theme, hdc: HDC, h: HWND) -> Option<LRESULT> {
        if !self.boxes.contains(&h) {
            return None;
        }
        unsafe {
            if self.brush_color != t.card.0 {
                if !self.brush.is_invalid() {
                    let _ = DeleteObject(self.brush);
                }
                self.brush = CreateSolidBrush(t.card);
                self.brush_color = t.card.0;
            }
            SetTextColor(hdc, t.text);
            SetBkColor(hdc, t.card);
        }
        Some(LRESULT(self.brush.0 as isize))
    }
}

impl Drop for Fields {
    fn drop(&mut self) {
        unsafe {
            if !self.brush.is_invalid() {
                let _ = DeleteObject(self.brush);
            }
        }
    }
}
