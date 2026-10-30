// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Диалоги открытия/сохранения и список соседних файлов папки.

use crate::util::{from_wide, wide, wpath};
use std::path::{Path, PathBuf};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Controls::Dialogs::*;

pub fn is_image(p: &Path) -> bool {
    crate::kinds::kind_of(p) == Some(crate::kinds::Kind::Image)
}

pub fn is_supported(p: &Path) -> bool {
    crate::kinds::is_supported(p)
}

/// Все поддерживаемые файлы в папке, по алфавиту, + индекс текущего.
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
    let imgs: Vec<String> = crate::kinds::IMAGE.iter().map(|e| format!("*.{e}")).collect();
    let imgs = imgs.join(";");
    let filter_s = match pick {
        Pick::Any => crate::kinds::open_filter(),
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

/// Общий диалог «Сохранить как». filters — (подпись, расширения), idx — выбранный с нуля.
/// Если имя без известного расширения, добавляется первое расширение выбранного фильтра.
pub fn save_as(hwnd: HWND, default_name: &str, initial_dir: Option<&Path>, filters: &[(&str, &[&str])], idx: usize) -> Option<PathBuf> {
    let mut fs = String::new();
    for (label, exts) in filters {
        let pat: Vec<String> = exts.iter().map(|e| format!("*.{e}")).collect();
        fs += &format!("{label}\0{}\0", pat.join(";"));
    }
    fs.push('\0');
    let filter: Vec<u16> = fs.encode_utf16().collect();
    let mut buf = vec![0u16; 32768];
    for (i, c) in default_name.encode_utf16().take(32000).enumerate() {
        buf[i] = c;
    }
    let idx = idx.min(filters.len().saturating_sub(1));
    let defext = wide(filters.get(idx).and_then(|f| f.1.first()).copied().unwrap_or(""));
    let dir = initial_dir.map(wpath);
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        nFilterIndex: idx as u32 + 1,
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
    let chosen = filters.get((ofn.nFilterIndex.max(1) - 1) as usize).copied().unwrap_or(filters[0]);
    let e = crate::util::ext_lower(&p);
    let known = filters.iter().any(|f| f.1.contains(&e.as_str()));
    if !known {
        if let Some(first) = chosen.1.first() {
            let name = format!("{}.{first}", crate::util::file_name(&p));
            p.set_file_name(name);
        }
    }
    Some(p)
}

/// Форматы фото + PDF для «Сохранить как».
pub const PHOTO_FILTERS: &[(&str, &[&str])] = &[
    ("JPEG", &["jpg", "jpeg"]),
    ("PNG", &["png"]),
    ("BMP", &["bmp"]),
    ("TIFF", &["tif", "tiff"]),
    ("GIF", &["gif"]),
    ("JPEG XR", &["jxr"]),
    ("HEIC (нужно расширение HEIF)", &["heic"]),
    ("Иконка ICO", &["ico"]),
    ("PDF", &["pdf"]),
];

/// Диалог «Сохранить как» для фото/документа. prefer_pdf — выбрать PDF по умолчанию; only_pdf — только PDF.
pub fn save_dialog(hwnd: HWND, default_name: &str, initial_dir: Option<&Path>, prefer_pdf: bool, only_pdf: bool) -> Option<PathBuf> {
    if only_pdf {
        return save_as(hwnd, default_name, initial_dir, &[("PDF", &["pdf"])], 0);
    }
    let idx = if prefer_pdf { PHOTO_FILTERS.len() - 1 } else { 0 };
    save_as(hwnd, default_name, initial_dir, PHOTO_FILTERS, idx)
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

/// Выбор папки (системный диалог).
pub fn pick_folder(hwnd: HWND, initial: Option<&Path>) -> Option<PathBuf> {
    use windows::Win32::System::Com::*;
    use windows::Win32::UI::Shell::*;
    unsafe {
        let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let o = d.GetOptions().ok()?;
        d.SetOptions(o | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
        if let Some(p) = initial {
            let w = wpath(p);
            if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(w.as_ptr()), None) {
                let _ = d.SetFolder(&item);
            }
        }
        d.Show(hwnd).ok()?;
        let item = d.GetResult().ok()?;
        let s = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = s.to_string().unwrap_or_default();
        CoTaskMemFree(Some(s.0 as *const _));
        Some(PathBuf::from(path))
    }
}
