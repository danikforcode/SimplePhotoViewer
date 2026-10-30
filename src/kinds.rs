// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Типы файлов: по расширению решаем, каким просмотрщиком открыть файл.

use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Image,
    Pdf,
    Doc,
    Table,
    Text,
    Audio,
    Video,
}

/// Фото: системные кодеки WIC (часть — через расширения Microsoft) + свои декодеры.
pub const IMAGE: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "bmp", "dib", "gif", "tif", "tiff", "ico", "cur", "jxr", "wdp", "hdp",
    "heic", "heif", "hif", "avif", "webp", "dds",
    // RAW камер (расширение «Raw Image Extension»)
    "dng", "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "orf", "rw2", "raf", "pef", "srw", "erf", "kdc", "mrw", "3fr", "x3f",
    // свои декодеры
    "tga", "pnm", "ppm", "pgm", "pbm", "qoi",
];
pub const DOC: &[&str] = &["docx", "docm", "dotx", "odt", "rtf", "md", "markdown"];
pub const TABLE: &[&str] = &["xlsx", "xlsm", "xlsb", "xls", "xla", "ods", "csv", "tsv"];
pub const AUDIO: &[&str] = &["mp3", "wav", "wave", "m4a", "aac", "adts", "flac", "wma", "ogg", "oga", "opus", "amr", "aif", "aiff", "aifc", "ac3", "ec3", "mka"];
pub const VIDEO: &[&str] = &["mp4", "m4v", "mov", "avi", "wmv", "asf", "mkv", "webm", "3gp", "3g2", "ts", "mts", "m2ts", "mpg", "mpeg", "vob"];
pub const TEXT: &[&str] = &[
    "txt", "log", "nfo", "ini", "cfg", "conf", "config", "env", "properties", "reg", "json", "jsonc", "json5", "yaml", "yml", "toml",
    "xml", "xaml", "csproj", "vcxproj", "props", "targets", "plist", "svg", "html", "htm", "xhtml", "css", "scss", "less",
    "js", "mjs", "cjs", "jsx", "ts.txt", "tsx", "vue", "py", "pyw", "pyi", "rs", "c", "h", "cpp", "hpp", "cc", "cxx", "hxx", "cs",
    "java", "kt", "kts", "go", "rb", "php", "pl", "lua", "sql", "sh", "bash", "zsh", "ps1", "psm1", "psd1", "bat", "cmd",
    "gradle", "cmake", "swift", "dart", "r", "m", "asm", "s", "srt", "vtt", "gitignore", "gitattributes", "editorconfig", "lock",
];
/// Файлы без расширения, которые тоже текст.
const TEXT_NAMES: &[&str] = &["makefile", "dockerfile", "license", "readme", "changelog", "cmakelists.txt"];

pub fn ext(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

pub fn kind_of(p: &Path) -> Option<Kind> {
    let e = ext(p);
    let e = e.as_str();
    // .ts — это и видео (MPEG-TS), и TypeScript: смотрим на первый байт (0x47 у видеопотока)
    if e == "ts" {
        return Some(if is_mpeg_ts(p) { Kind::Video } else { Kind::Text });
    }
    if IMAGE.contains(&e) {
        Some(Kind::Image)
    } else if e == "pdf" {
        Some(Kind::Pdf)
    } else if DOC.contains(&e) {
        Some(Kind::Doc)
    } else if TABLE.contains(&e) {
        Some(Kind::Table)
    } else if AUDIO.contains(&e) {
        Some(Kind::Audio)
    } else if VIDEO.contains(&e) {
        Some(Kind::Video)
    } else if TEXT.contains(&e) {
        Some(Kind::Text)
    } else {
        let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        TEXT_NAMES.contains(&name.as_str()).then_some(Kind::Text)
    }
}

fn is_mpeg_ts(p: &Path) -> bool {
    use std::io::Read;
    let mut b = [0u8; 189];
    match std::fs::File::open(p).and_then(|mut f| f.read(&mut b)) {
        Ok(n) if n >= 189 => b[0] == 0x47 && b[188] == 0x47,
        Ok(n) if n >= 1 => b[0] == 0x47,
        _ => false,
    }
}

pub fn is_supported(p: &Path) -> bool {
    kind_of(p).is_some()
}

/// Можно ли этот тип склеивать/печатать как страницы.
pub fn is_paged(k: Kind) -> bool {
    matches!(k, Kind::Image | Kind::Pdf | Kind::Doc)
}

/// Фильтр для диалога открытия: «Все поддерживаемые» + по группам.
pub fn open_filter() -> String {
    let pat = |list: &[&str]| list.iter().filter(|e| !e.contains('.')).map(|e| format!("*.{e}")).collect::<Vec<_>>().join(";");
    let all: Vec<&str> = [IMAGE, &["pdf"], DOC, TABLE, TEXT, AUDIO, VIDEO].concat();
    format!(
        "Все поддерживаемые\0{};*.ts\0Фото\0{}\0PDF и документы\0*.pdf;{}\0Таблицы\0{}\0Аудио\0{}\0Видео\0{};*.ts\0Текст и код\0{}\0Все файлы\0*.*\0\0",
        pat(&all), pat(IMAGE), pat(DOC), pat(TABLE), pat(AUDIO), pat(VIDEO), pat(TEXT)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    #[test]
    fn kinds() {
        let k = |s: &str| kind_of(&PathBuf::from(s));
        assert_eq!(k("a.JPG"), Some(Kind::Image));
        assert_eq!(k("a.cr3"), Some(Kind::Image));
        assert_eq!(k("a.docx"), Some(Kind::Doc));
        assert_eq!(k("a.xlsx"), Some(Kind::Table));
        assert_eq!(k("a.mkv"), Some(Kind::Video));
        assert_eq!(k("a.flac"), Some(Kind::Audio));
        assert_eq!(k("a.rs"), Some(Kind::Text));
        assert_eq!(k("Makefile"), Some(Kind::Text));
        assert_eq!(k("a.exe"), None);
        assert!(open_filter().ends_with("\0\0"));
    }
}
