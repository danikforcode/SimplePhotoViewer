// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Документ (DOCX/ODT/RTF/Markdown) по страницам через системный RichEdit
//! (msftedit.dll — движок WordPad). RichEdit сам раскладывает текст, таблицы
//! и картинки на листы (EM_FORMATRANGE), а мы рисуем каждый лист в картинку.

use crate::image_buf::Img;
use crate::rtf::{self, PageSetup};
use std::cell::RefCell;
use std::path::Path;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryW};
use windows::Win32::UI::Controls::RichEdit::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub struct RichDoc {
    re: HWND,
    pub page: PageSetup,
    /// Начало каждой страницы (номер символа); последний элемент — конец текста.
    breaks: Vec<i32>,
    cache: RefCell<Vec<(u32, i32, std::rc::Rc<Img>)>>, // (страница, dpi, картинка)
    emf: RefCell<Vec<Option<HENHMETAFILE>>>,
}

impl std::fmt::Debug for RichDoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RichDoc({} стр.)", self.pages())
    }
}

impl Drop for RichDoc {
    fn drop(&mut self) {
        unsafe {
            for m in self.emf.borrow().iter().flatten() {
                let _ = DeleteEnhMetaFile(*m);
            }
            let _ = DestroyWindow(self.re);
        }
    }
}

fn ensure_lib() -> bool {
    thread_local! { static OK: bool = unsafe { LoadLibraryW(w!("Msftedit.dll")).is_ok() }; }
    OK.with(|v| *v)
}

/// Картинку любого формата (EMF, GIF, BMP, TIFF…) → PNG для RTF.
fn fix_image(data: &[u8]) -> Option<Vec<u8>> {
    let img = crate::codec::load_bytes(data).ok()?;
    let img = crate::codec::thumb(&img, 2400);
    crate::codec::encode_mem(&img, "png", 100).ok()
}

/// Прочитать документ и превратить в RTF + параметры листа.
pub fn to_rtf(path: &Path) -> std::result::Result<(String, PageSetup), String> {
    let ext = crate::util::ext_lower(path);
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    match ext.as_str() {
        "docx" | "docm" | "dotx" => rtf::docx_to_rtf(bytes, &fix_image),
        "odt" => rtf::odt_to_rtf(bytes, &fix_image),
        "rtf" => Ok((String::from_utf8_lossy(&bytes).into_owned(), PageSetup::default())),
        "md" | "markdown" => {
            let text = crate::textenc::decode(&bytes).0;
            Ok((rtf::markdown_to_rtf(&text, path.parent(), &fix_image), PageSetup::default()))
        }
        _ => {
            let text = crate::textenc::decode(&bytes).0;
            Ok((rtf::plain_to_rtf(&text), PageSetup::default()))
        }
    }
}

impl RichDoc {
    pub fn open(path: &Path) -> std::result::Result<RichDoc, String> {
        let (rtf, page) = to_rtf(path)?;
        Self::from_rtf(&rtf, page)
    }

    pub fn from_rtf(rtf: &str, page: PageSetup) -> std::result::Result<RichDoc, String> {
        if !ensure_lib() {
            return Err("Не найден системный компонент RichEdit (Msftedit.dll)".into());
        }
        unsafe {
            let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
            let re = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("RICHEDIT50W"),
                w!(""),
                WS_POPUP | WINDOW_STYLE(ES_MULTILINE as u32),
                0, 0, 800, 600,
                None, None, hinst, None,
            )
            .map_err(|e| format!("RichEdit: {e}"))?;
            SendMessageW(re, EM_EXLIMITTEXT, WPARAM(0), LPARAM(0x7FFF_FFFF));
            // RTF в RichEdit: через EM_SETTEXTEX (весь не-ASCII уже экранирован)
            let mut bytes = rtf.as_bytes().to_vec();
            bytes.push(0);
            let st = SETTEXTEX { flags: 0, codepage: 0 };
            SendMessageW(re, EM_SETTEXTEX, WPARAM(&st as *const _ as usize), LPARAM(bytes.as_ptr() as isize));
            let mut doc = RichDoc { re, page, breaks: vec![], cache: RefCell::new(vec![]), emf: RefCell::new(vec![]) };
            doc.paginate();
            if std::env::var_os("SPV_DEBUG").is_some() {
                eprintln!("richdoc: rtf {} байт, текст {} симв., страниц {} {:?}", rtf.len(), doc.text_len(), doc.pages(), doc.breaks);
            }
            Ok(doc)
        }
    }

    fn text_len(&self) -> i32 {
        unsafe {
            let g = GETTEXTLENGTHEX { flags: GTL_NUMCHARS | GTL_PRECISE, codepage: 1200 };
            SendMessageW(self.re, EM_GETTEXTLENGTHEX, WPARAM(&g as *const _ as usize), LPARAM(0)).0 as i32
        }
    }

    fn rects(&self) -> (RECT, RECT) {
        let p = &self.page;
        let page = RECT { left: 0, top: 0, right: p.w, bottom: p.h };
        let rc = RECT { left: p.margin.0, top: p.margin.1, right: p.w - p.margin.2, bottom: p.h - p.margin.3 };
        (page, rc)
    }

    fn paginate(&mut self) {
        let len = self.text_len();
        self.breaks = vec![0];
        unsafe {
            let dc = CreateCompatibleDC(None);
            let (page, rc) = self.rects();
            let mut cp = 0;
            let mut guard = 0;
            while cp < len && guard < 100_000 {
                guard += 1;
                let mut fr = FORMATRANGE { hdc: dc, hdcTarget: dc, rc, rcPage: page, chrg: CHARRANGE { cpMin: cp, cpMax: -1 } };
                let next = SendMessageW(self.re, EM_FORMATRANGE, WPARAM(0), LPARAM(&mut fr as *mut _ as isize)).0 as i32;
                if next <= cp {
                    break; // защита от зацикливания (огромная картинка и т.п.)
                }
                cp = next;
                self.breaks.push(cp.min(len));
            }
            SendMessageW(self.re, EM_FORMATRANGE, WPARAM(0), LPARAM(0));
            let _ = DeleteDC(dc);
        }
        if self.breaks.len() == 1 {
            self.breaks.push(len);
        }
    }

    pub fn pages(&self) -> usize {
        self.breaks.len().saturating_sub(1).max(1)
    }

    /// Размер листа в пунктах.
    pub fn size_pt(&self) -> (f64, f64) {
        (self.page.w as f64 / 20.0, self.page.h as f64 / 20.0)
    }

    /// Векторная запись страницы (EMF): RichEdit рисует один раз, дальше
    /// метафайл масштабируется без потери чёткости.
    fn metafile(&self, index: usize) -> Option<HENHMETAFILE> {
        if let Some(Some(m)) = self.emf.borrow().get(index) {
            return Some(*m);
        }
        let (from, to) = (*self.breaks.get(index)?, *self.breaks.get(index + 1).unwrap_or(&-1));
        unsafe {
            let screen = CreateCompatibleDC(None);
            // Рамка метафайла — в сотых долях мм, но пересчитывать её надо так же,
            // как это делает GDI: через физический размер экрана (HORZSIZE/HORZRES),
            // а RichEdit рисует в точках по LOGPIXELS. При масштабе Windows 125–150%
            // эти величины расходятся, и «честные» мм листа обрезали страницу справа
            // и снизу. Поэтому: лист в точках устройства → те же точки в мм GDI.
            let g = |i| GetDeviceCaps(screen, i).max(1) as i64;
            let (lx, ly) = (g(LOGPIXELSX), g(LOGPIXELSY));
            let (hs, hr, vs, vr) = (g(HORZSIZE), g(HORZRES), g(VERTSIZE), g(VERTRES));
            let pw = self.page.w as i64 * lx / 1440;
            let ph = self.page.h as i64 * ly / 1440;
            let frame = RECT { left: 0, top: 0, right: (pw * hs * 100 / hr) as i32, bottom: (ph * vs * 100 / vr) as i32 };
            let emf_dc = CreateEnhMetaFileW(screen, PCWSTR::null(), Some(&frame), PCWSTR::null());
            if emf_dc.is_invalid() {
                let _ = DeleteDC(screen);
                return None;
            }
            let (page, rc) = self.rects();
            let mut fr = FORMATRANGE { hdc: emf_dc, hdcTarget: screen, rc, rcPage: page, chrg: CHARRANGE { cpMin: from, cpMax: to } };
            let r = SendMessageW(self.re, EM_FORMATRANGE, WPARAM(1), LPARAM(&mut fr as *mut _ as isize));
            SendMessageW(self.re, EM_FORMATRANGE, WPARAM(0), LPARAM(0));
            if std::env::var_os("SPV_DEBUG").is_some() {
                eprintln!("richdoc: стр.{index} {from}..{to} → {}", r.0);
            }
            let m = CloseEnhMetaFile(emf_dc);
            let _ = DeleteDC(screen);
            if m.is_invalid() {
                return None;
            }
            let mut v = self.emf.borrow_mut();
            if v.len() <= index {
                v.resize(index + 1, None);
            }
            v[index] = Some(m);
            Some(m)
        }
    }

    /// Нарисовать страницу при заданном разрешении (точек на дюйм).
    pub fn render(&self, index: u32, dpi: f64) -> std::result::Result<std::rc::Rc<Img>, String> {
        let dpi_key = dpi.round() as i32;
        if let Some((_, _, img)) = self.cache.borrow().iter().find(|(i, d, _)| *i == index && *d == dpi_key) {
            return Ok(img.clone());
        }
        let m = self.metafile(index as usize).ok_or("не удалось разметить страницу")?;
        let w = ((self.page.w as f64 * dpi / 1440.0).round() as i32).max(1);
        let h = ((self.page.h as f64 * dpi / 1440.0).round() as i32).max(1);
        unsafe {
            let mem = CreateCompatibleDC(None);
            let bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let dib = CreateDIBSection(mem, &bi, DIB_RGB_COLORS, &mut bits, None, 0).map_err(|e| e.to_string())?;
            let old = SelectObject(mem, dib);
            let white = CreateSolidBrush(COLORREF(0x00FF_FFFF));
            let full = RECT { left: 0, top: 0, right: w, bottom: h };
            FillRect(mem, &full, white);
            let _ = DeleteObject(white);
            SetStretchBltMode(mem, HALFTONE);
            let _ = PlayEnhMetaFile(mem, m, &full);
            let _ = GdiFlush();
            let n = (w * h * 4) as usize;
            let mut px = std::slice::from_raw_parts(bits as *const u8, n).to_vec();
            for p in px.chunks_exact_mut(4) {
                p[3] = 255;
            }
            SelectObject(mem, old);
            let _ = DeleteObject(dib);
            let _ = DeleteDC(mem);
            let img = std::rc::Rc::new(Img { w: w as u32, h: h as u32, px });
            let mut c = self.cache.borrow_mut();
            c.push((index, dpi_key, img.clone()));
            if c.len() > 12 {
                c.remove(0);
            }
            Ok(img)
        }
    }
}

/// Простой «рисунок» RTF для проверки: один абзац текста.
#[allow(dead_code)]
pub fn sample() -> String {
    let mut r = rtf::Rtf::new();
    r.para(&rtf::Para::default());
    r.text("Пример", &rtf::Ch::default());
    r.end_para();
    r.finish()
}
