// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Буфер обмена: картинка (CF_DIB) и скопированные в Проводнике файлы (CF_HDROP).

use crate::image_buf::Img;
use std::path::PathBuf;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

const CF_DIB: u32 = 8;
const CF_HDROP: u32 = 15;

struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

fn open(hwnd: HWND) -> Option<Guard> {
    unsafe { OpenClipboard(hwnd).ok().map(|_| Guard) }
}

/// Пути файлов, скопированных в Проводнике.
pub fn files(hwnd: HWND) -> Vec<PathBuf> {
    let mut out = vec![];
    unsafe {
        if IsClipboardFormatAvailable(CF_HDROP).is_err() {
            return out;
        }
        let Some(_g) = open(hwnd) else { return out };
        let Ok(h) = GetClipboardData(CF_HDROP) else { return out };
        let hdrop = HDROP(h.0);
        let n = DragQueryFileW(hdrop, 0xFFFF_FFFF, None);
        for i in 0..n {
            let len = DragQueryFileW(hdrop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            DragQueryFileW(hdrop, i, Some(&mut buf));
            out.push(PathBuf::from(String::from_utf16_lossy(&buf[..len])));
        }
    }
    out
}

/// Картинка из буфера (24/32 бита).
pub fn image(hwnd: HWND) -> Option<Img> {
    unsafe {
        IsClipboardFormatAvailable(CF_DIB).ok()?;
        let _g = open(hwnd)?;
        let h = GetClipboardData(CF_DIB).ok()?;
        let hg = HGLOBAL(h.0);
        let size = GlobalSize(hg);
        let p = GlobalLock(hg) as *const u8;
        if p.is_null() {
            return None;
        }
        let data = std::slice::from_raw_parts(p, size).to_vec();
        let _ = GlobalUnlock(hg);
        parse_dib(&data)
    }
}

/// Разбор DIB: BITMAPINFOHEADER (+ маски BI_BITFIELDS) и пиксели.
pub fn parse_dib(d: &[u8]) -> Option<Img> {
    let u32_at = |o: usize| -> Option<u32> { Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?)) };
    let u16_at = |o: usize| -> Option<u16> { Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?)) };
    let hsize = u32_at(0)? as usize;
    let w = u32_at(4)? as i32;
    let hraw = u32_at(8)? as i32;
    let bpp = u16_at(14)?;
    let comp = u32_at(16)?;
    let clr_used = u32_at(32)? as usize;
    if w <= 0 || hraw == 0 || !(bpp == 24 || bpp == 32) || !(comp == 0 || comp == 3) {
        return None;
    }
    let (h, top_down) = (hraw.unsigned_abs(), hraw < 0);
    let w = w as u32;
    let mut off = hsize + clr_used * 4;
    if comp == 3 && hsize == 40 {
        off += 12;
    }
    let stride = ((w as usize * bpp as usize + 31) / 32) * 4;
    if d.len() < off + stride * h as usize {
        return None;
    }
    let mut px = vec![0u8; w as usize * h as usize * 4];
    for y in 0..h as usize {
        let sy = if top_down { y } else { h as usize - 1 - y };
        let row = &d[off + sy * stride..];
        for x in 0..w as usize {
            let s = x * (bpp as usize / 8);
            let o = (y * w as usize + x) * 4;
            px[o..o + 3].copy_from_slice(&row[s..s + 3]);
            px[o + 3] = 255;
        }
    }
    Some(Img { w, h, px })
}

/// Положить картинку в буфер (24 бита, снизу вверх — понимают все программы).
pub fn set_image(hwnd: HWND, img: &Img) -> bool {
    let stride = ((img.w as usize * 3 + 3) / 4) * 4;
    let size = 40 + stride * img.h as usize;
    unsafe {
        let Ok(hg) = GlobalAlloc(GMEM_MOVEABLE, size) else { return false };
        let p = GlobalLock(hg) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(hg);
            return false;
        }
        let d = std::slice::from_raw_parts_mut(p, size);
        d.fill(0);
        d[0..4].copy_from_slice(&40u32.to_le_bytes());
        d[4..8].copy_from_slice(&(img.w as i32).to_le_bytes());
        d[8..12].copy_from_slice(&(img.h as i32).to_le_bytes());
        d[12..14].copy_from_slice(&1u16.to_le_bytes());
        d[14..16].copy_from_slice(&24u16.to_le_bytes());
        d[20..24].copy_from_slice(&((stride * img.h as usize) as u32).to_le_bytes());
        for y in 0..img.h as usize {
            let dy = img.h as usize - 1 - y;
            for x in 0..img.w as usize {
                let s = (y * img.w as usize + x) * 4;
                let o = 40 + dy * stride + x * 3;
                d[o..o + 3].copy_from_slice(&img.px[s..s + 3]);
            }
        }
        let _ = GlobalUnlock(hg);
        let Some(_g) = open(hwnd) else {
            let _ = GlobalFree(hg);
            return false;
        };
        let _ = EmptyClipboard();
        if SetClipboardData(CF_DIB, HANDLE(hg.0)).is_err() {
            let _ = GlobalFree(hg);
            return false;
        }
        true
    }
}

/// Положить текст в буфер.
pub fn set_text(hwnd: HWND, text: &str) -> bool {
    let w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let size = w.len() * 2;
    unsafe {
        let Ok(hg) = GlobalAlloc(GMEM_MOVEABLE, size) else { return false };
        let p = GlobalLock(hg) as *mut u16;
        if p.is_null() {
            let _ = GlobalFree(hg);
            return false;
        }
        std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
        let _ = GlobalUnlock(hg);
        let Some(_g) = open(hwnd) else {
            let _ = GlobalFree(hg);
            return false;
        };
        let _ = EmptyClipboard();
        if SetClipboardData(13, HANDLE(hg.0)).is_err() {
            let _ = GlobalFree(hg);
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(w: i32, h: i32, bpp: u16) -> Vec<u8> {
        let mut d = vec![0u8; 40];
        d[0..4].copy_from_slice(&40u32.to_le_bytes());
        d[4..8].copy_from_slice(&w.to_le_bytes());
        d[8..12].copy_from_slice(&h.to_le_bytes());
        d[12..14].copy_from_slice(&1u16.to_le_bytes());
        d[14..16].copy_from_slice(&bpp.to_le_bytes());
        d
    }

    #[test]
    fn dib_24_bottom_up() {
        // 2×2, строки по 8 байт (6 + 2 выравнивания); снизу вверх
        let mut d = header(2, 2, 24);
        d.extend_from_slice(&[1, 1, 1, 2, 2, 2, 0, 0]); // нижняя строка
        d.extend_from_slice(&[3, 3, 3, 4, 4, 4, 0, 0]); // верхняя строка
        let img = parse_dib(&d).unwrap();
        assert_eq!((img.w, img.h), (2, 2));
        assert_eq!(&img.px[0..8], &[3, 3, 3, 255, 4, 4, 4, 255]);
        assert_eq!(&img.px[8..16], &[1, 1, 1, 255, 2, 2, 2, 255]);
    }

    #[test]
    fn dib_32_top_down_bitfields() {
        let mut d = header(1, -2, 32);
        d[16..20].copy_from_slice(&3u32.to_le_bytes()); // BI_BITFIELDS
        d.extend_from_slice(&[0; 12]); // маски
        d.extend_from_slice(&[9, 8, 7, 0, 6, 5, 4, 0]);
        let img = parse_dib(&d).unwrap();
        assert_eq!(&img.px, &[9, 8, 7, 255, 6, 5, 4, 255]);
        assert!(parse_dib(&header(1, 1, 8)).is_none());
    }
}
