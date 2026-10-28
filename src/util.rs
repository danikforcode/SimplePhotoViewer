// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Мелкие помощники для Win32.

use crate::image_buf::Img;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Gdi::{BITMAPINFO, BITMAPINFOHEADER, BI_RGB, RGBQUAD};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Путь → UTF-16 с нулём на конце.
pub fn wpath(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

/// Буфер UTF-16 (до первого нуля) → String.
pub fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

pub const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

/// Описание буфера Img для GDI: top-down 32-битный BGRA.
pub fn bmi(img: &Img) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: img.w as i32,
            biHeight: -(img.h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        bmiColors: [RGBQUAD::default(); 1],
    }
}

/// %APPDATA%\SimplePhotoViewer (создаётся при необходимости).
pub fn app_dir() -> Option<PathBuf> {
    let d = Path::new(&std::env::var_os("APPDATA")?).join("SimplePhotoViewer");
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

pub fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

pub fn ext_lower(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

pub fn is_pdf(p: &Path) -> bool {
    ext_lower(p) == "pdf"
}
