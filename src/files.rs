// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Диалоги открытия/сохранения и список соседних файлов папки.

use crate::util::{from_wide, wide, wpath};
use std::path::{Path, PathBuf};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Controls::Dialogs::*;

pub const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "bmp", "gif", "webp", "tif", "tiff", "heic", "heif", "jfif"];

pub fn is_image(p: &Path) -> bool {
    IMAGE_EXTS.contains(&crate::util::ext_lower(p).as_str())
}

pub fn is_supported(p: &Path) -> bool {
    is_image(p) || crate::util::is_pdf(p)
}

/// Все фото и PDF в папке файла, по алфавиту, + индекс текущего.
pub fn siblings(path: &Path) -> (Vec<PathBuf>, isize) {
    let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let dir = if dir.as_os_str().is_empty() { PathBuf::from(".") } else { dir };
    let mut list: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| is_supported(p)).collect())
        .unwrap_or_default();
    list.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default());
    let me = path.file_name().map(|n| n.to_string_lossy().to_lowercase());
    let idx = list
        .iter()
        .position(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()) == me)
        .map(|i| i as isize)
        .unwrap_or(-1);
    (list, idx)
}

/// Что показывать в диалоге открытия.
#[derive(Clone, Copy, PartialEq)]
pub enum Pick {
    Any,
    Images,
}

/// Диалог открытия; можно выбрать несколько файлов.
pub fn open_dialog(hwnd: HWND, pick: Pick, multi: bool) -> Vec<PathBuf> {
    let imgs: Vec<String> = IMAGE_EXTS.iter().map(|e| format!("*.{e}")).collect();
    let imgs = imgs.join(";");
    let filter_s = match pick {
        Pick::Any => format!("Фото и PDF\0{imgs};*.pdf\0Изображения\0{imgs}\0PDF\0*.pdf\0Все файлы\0*.*\0\0"),
        Pick::Images => format!("Изображения\0{imgs}\0Все файлы\0*.*\0\0"),
    };
    let filter: Vec<u16> = filter_s.encode_utf16().collect();
    let mut buf = vec![0u16; 65536];
    let dir = crate::settings::get_s("last_dir").map(|d| wide(&d));
    let mut flags = OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST;
    if multi {
        flags |= OFN_ALLOWMULTISELECT;
    }
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrInitialDir: dir.as_ref().map(|v| PCWSTR(v.as_ptr())).unwrap_or(PCWSTR::null()),
        Flags: flags,
        ..Default::default()
    };
    if !unsafe { GetOpenFileNameW(&mut ofn) }.as_bool() {
        return vec![];
    }
    let out = parse_multi(&buf);
    if let Some(d) = out.first().and_then(|p| p.parent()) {
        crate::settings::set_s("last_dir", &d.to_string_lossy());
    }
    out
}

/// Буфер множественного выбора: «папка\0файл1\0файл2\0\0» или «полный путь\0\0».
fn parse_multi(buf: &[u16]) -> Vec<PathBuf> {
    let mut parts = vec![];
    let mut start = 0;
    for i in 0..buf.len() {
        if buf[i] == 0 {
            if i == start {
                break;
            }
            parts.push(String::from_utf16_lossy(&buf[start..i]));
            start = i + 1;
        }
    }
    match parts.len() {
        0 => vec![],
        1 => vec![PathBuf::from(&parts[0])],
        _ => {
            let dir = PathBuf::from(&parts[0]);
            parts[1..].iter().map(|f| dir.join(f)).collect()
        }
    }
}

/// Форматы в диалоге сохранения (порядок = индекс фильтра − 1).
const SAVE_EXTS: [&str; 5] = ["jpg", "png", "bmp", "tif", "pdf"];

/// Диалог «Сохранить как». prefer_pdf — выбрать PDF по умолчанию; only_pdf — только PDF.
pub fn save_dialog(hwnd: HWND, default_name: &str, initial_dir: Option<&Path>, prefer_pdf: bool, only_pdf: bool) -> Option<PathBuf> {
    let filter_s = if only_pdf {
        "PDF\0*.pdf\0\0"
    } else {
        "JPEG\0*.jpg;*.jpeg\0PNG\0*.png\0BMP\0*.bmp\0TIFF\0*.tif;*.tiff\0PDF\0*.pdf\0\0"
    };
    let filter: Vec<u16> = filter_s.encode_utf16().collect();
    let mut buf = vec![0u16; 32768];
    for (i, c) in default_name.encode_utf16().take(32000).enumerate() {
        buf[i] = c;
    }
    let defext = wide(if prefer_pdf || only_pdf { "pdf" } else { "jpg" });
    let dir = initial_dir.map(wpath);
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        nFilterIndex: if only_pdf { 1 } else if prefer_pdf { 5 } else { 1 },
        lpstrFile: PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrDefExt: PCWSTR(defext.as_ptr()),
        lpstrInitialDir: dir.as_ref().map(|v| PCWSTR(v.as_ptr())).unwrap_or(PCWSTR::null()),
        Flags: OFN_EXPLORER | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST,
        ..Default::default()
    };
    if !unsafe { GetSaveFileNameW(&mut ofn) }.as_bool() {
        return None;
    }
    let mut p = PathBuf::from(from_wide(&buf));
    let known = ["jpg", "jpeg", "png", "bmp", "tif", "tiff", "pdf"];
    if !known.contains(&crate::util::ext_lower(&p).as_str()) {
        let e = if only_pdf { "pdf" } else { SAVE_EXTS[(ofn.nFilterIndex.max(1) as usize - 1).min(4)] };
        let name = format!("{}.{e}", crate::util::file_name(&p));
        p.set_file_name(name);
    }
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multi() {
        let b: Vec<u16> = "C:\\d\0a.jpg\0b.pdf\0\0".encode_utf16().collect();
        assert_eq!(parse_multi(&b), vec![PathBuf::from("C:\\d").join("a.jpg"), PathBuf::from("C:\\d").join("b.pdf")]);
        let b: Vec<u16> = "C:\\d\\a.jpg\0\0".encode_utf16().collect();
        assert_eq!(parse_multi(&b), vec![PathBuf::from("C:\\d\\a.jpg")]);
    }
}
