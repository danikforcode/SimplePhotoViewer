// SimplePhotoViewer (Rust/Win32) — © 2026 danikforcode
// Лицензия MIT (см. LICENSE). Распространяется «как есть», без гарантий.
//! Свои декодеры — когда в Windows нет нужного кодека (а скачать его из
//! Microsoft Store нельзя или не хочется): HEIC/HEIF (HEVC), AVIF (AV1),
//! WebP и RAW камер (встроенный в снимок полноразмерный JPEG).
//! Сначала всегда пробуется системный WIC — он быстрее и точнее; сюда
//! попадаем, только если он не справился.

use crate::image_buf::Img;

/// Что это за файл (по первым байтам, не по расширению).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Sniff {
    Heic,
    Avif,
    WebP,
    Raw,
}

pub fn sniff(b: &[u8], ext: &str) -> Option<Sniff> {
    if b.len() >= 12 && &b[4..8] == b"ftyp" {
        let brands: Vec<&[u8]> = b[8..(8 + 64).min(b.len())].chunks(4).collect();
        let has = |x: &[u8]| brands.contains(&x);
        if has(b"avif") || has(b"avis") {
            return Some(Sniff::Avif);
        }
        if has(b"heic") || has(b"heix") || has(b"hevc") || has(b"hevx") || has(b"heim") || has(b"heis") {
            return Some(Sniff::Heic);
        }
        if has(b"crx ") {
            return Some(Sniff::Raw);
        }
        if has(b"mif1") || has(b"msf1") || has(b"miaf") {
            // общий бренд HEIF: смотрим, чем сжата картинка
            return Some(if contains(&b[..b.len().min(65536)], b"av01") { Sniff::Avif } else { Sniff::Heic });
        }
    }
    if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        return Some(Sniff::WebP);
    }
    if is_raw_ext(ext) || b.starts_with(b"FUJIFILMCCD-RAW") || b.starts_with(b"IIRO") || b.starts_with(b"IIRS") || b.starts_with(b"MMOR") {
        return Some(Sniff::Raw);
    }
    None
}

fn contains(h: &[u8], n: &[u8]) -> bool {
    h.windows(n.len()).any(|w| w == n)
}

/// Расшифровать своими силами. None — формат не наш (пусть остаётся ошибка WIC).
pub fn decode(bytes: &[u8], ext: &str) -> Option<Result<Img, String>> {
    let kind = sniff(bytes, ext)?;
    Some(match kind {
        Sniff::Heic => heic(bytes),
        Sniff::Avif => crate::avif::decode(bytes),
        Sniff::WebP => webp(bytes),
        Sniff::Raw => raw_preview(bytes),
    })
}

/// RGBA → наш BGRA на белом фоне.
pub fn from_rgba(w: u32, h: u32, rgba: &[u8], channels: usize) -> Img {
    let mut px = vec![0u8; w as usize * h as usize * 4];
    for (d, s) in px.chunks_exact_mut(4).zip(rgba.chunks_exact(channels)) {
        d[0] = s[2];
        d[1] = s[1];
        d[2] = s[0];
        d[3] = if channels == 4 { s[3] } else { 255 };
    }
    let mut img = Img { w, h, px };
    img.flatten_white();
    img
}

// ---------------------------------------------------------------- HEIC ---
fn heic(bytes: &[u8]) -> Result<Img, String> {
    let d = heic_decoder::decode(bytes).map_err(|e| format!("HEIC: {e}"))?;
    let has_alpha = d.alpha.is_some();
    if has_alpha {
        let rgba = d.to_rgba8().map_err(|e| format!("HEIC: {e}"))?;
        Ok(from_rgba(rgba.width as u32, rgba.height as u32, &rgba.data, 4))
    } else {
        let rgb = d.to_rgb8().map_err(|e| format!("HEIC: {e}"))?;
        Ok(from_rgba(rgb.width as u32, rgb.height as u32, &rgb.data, 3))
    }
}

/// Встроенная в HEIC миниатюра (iPhone кладёт ~320×240), если её длинная
/// сторона не меньше `min_long`. Расшифровывается в десятки раз быстрее
/// полного снимка: подменяем в копии файла номер главной картинки (pitm)
/// на номер миниатюры и отдаём её тому же декодеру.
pub fn heic_thumb(bytes: &[u8], min_long: u32) -> Option<Img> {
    let h = crate::heif::parse(bytes).ok()?;
    let (at, n) = h.pitm_at?;
    let mut best: Option<(u32, u32)> = None;
    for (kind, from, to) in &h.refs {
        if kind != b"thmb" || !to.contains(&h.primary) {
            continue;
        }
        let it = h.item(*from)?;
        if &it.kind != b"hvc1" {
            continue;
        }
        let side = h.props_of(*from).iter().find_map(|p| match p {
            crate::heif::Prop::Ispe(w, hh) => Some((*w).max(*hh)),
            _ => None,
        })?;
        if side >= min_long && best.map_or(true, |b| side > b.1) {
            best = Some((*from, side));
        }
    }
    let (id, _) = best?;
    if n == 2 && id > 0xFFFF {
        return None;
    }
    let mut b = bytes.to_vec();
    match n {
        2 => b.get_mut(at..at + 2)?.copy_from_slice(&(id as u16).to_be_bytes()),
        _ => b.get_mut(at..at + 4)?.copy_from_slice(&id.to_be_bytes()),
    }
    heic(&b).ok()
}

// ---------------------------------------------------------------- WebP ---
fn webp(bytes: &[u8]) -> Result<Img, String> {
    let mut d = image_webp::WebPDecoder::new(std::io::Cursor::new(bytes)).map_err(|e| format!("WebP: {e}"))?;
    let (w, h) = d.dimensions();
    if w as u64 * h as u64 > 400_000_000 {
        return Err("WebP: слишком большое изображение".into());
    }
    let ch = if d.has_alpha() { 4 } else { 3 };
    let mut buf = vec![0u8; w as usize * h as usize * ch];
    // анимация — первый кадр
    if d.is_animated() {
        d.read_frame(&mut buf).map_err(|e| format!("WebP: {e}"))?;
    } else {
        d.read_image(&mut buf).map_err(|e| format!("WebP: {e}"))?;
    }
    Ok(from_rgba(w, h, &buf, ch))
}

// ----------------------------------------------------------------- RAW ---
/// Встроенные в RAW снимки JPEG: (смещение, длина, ширина, высота).
pub fn raw_jpegs(b: &[u8]) -> Vec<(usize, usize, u32, u32)> {
    let mut out = vec![];
    let mut i = 0;
    while i + 4 < b.len() {
        // FF D8 FF — начало JPEG
        let Some(p) = memchr3(b, i) else { break };
        if let Some((len, w, h)) = jpeg_extent(&b[p..]) {
            out.push((p, len, w, h));
            i = p + len.max(4);
        } else {
            i = p + 3;
        }
    }
    out
}

fn memchr3(b: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 2 < b.len() {
        match b[i..].iter().position(|&x| x == 0xFF) {
            Some(k) => {
                let p = i + k;
                if p + 2 < b.len() && b[p + 1] == 0xD8 && b[p + 2] == 0xFF {
                    return Some(p);
                }
                i = p + 1;
            }
            None => return None,
        }
    }
    None
}

/// Пройти маркеры JPEG: обычный (не «без потерь») кадр с размерами → (длина до FFD9, ш, в).
fn jpeg_extent(b: &[u8]) -> Option<(usize, u32, u32)> {
    let mut p = 2;
    let mut size = None;
    loop {
        if p + 4 > b.len() || b[p] != 0xFF {
            return None;
        }
        let m = b[p + 1];
        if m == 0xFF {
            p += 1;
            continue;
        }
        let len = u16::from_be_bytes([b[p + 2], b[p + 3]]) as usize;
        if len < 2 {
            return None;
        }
        match m {
            // SOF0/1/2 — обычный и прогрессивный JPEG; SOF3 (без потерь, сырые данные) — не наш
            0xC0..=0xC2 => {
                let s = b.get(p + 4..p + 9)?;
                let h = u16::from_be_bytes([s[1], s[2]]) as u32;
                let w = u16::from_be_bytes([s[3], s[4]]) as u32;
                size = Some((w, h));
            }
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return None,
            0xDA => {
                // данные до FFD9 (внутри FF всегда экранирован 00 или RST)
                let (w, h) = size?;
                let mut q = p + 2 + len;
                while q + 1 < b.len() {
                    if b[q] == 0xFF && b[q + 1] == 0xD9 {
                        return (w >= 160 && h >= 120).then_some((q + 2, w, h));
                    }
                    q += 1;
                }
                return None;
            }
            _ => {}
        }
        p += 2 + len;
    }
}

/// RAW камер (для них — встроенный JPEG, если системного кодека нет).
pub fn is_raw_ext(ext: &str) -> bool {
    matches!(ext, "dng" | "cr2" | "cr3" | "crw" | "nef" | "nrw" | "arw" | "srf" | "sr2" | "orf" | "rw2" | "raf" | "pef" | "srw" | "erf" | "kdc" | "mrw" | "3fr" | "x3f")
}

/// Ориентация из EXIF JPEG напрямую (без WIC): надёжнее и не зависит от версии системы.
pub fn jpeg_orientation(b: &[u8]) -> Option<u16> {
    if b.len() < 4 || b[0] != 0xFF || b[1] != 0xD8 {
        return None;
    }
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xFF {
            return None;
        }
        let marker = b[i + 1];
        if marker == 0xD9 || marker == 0xDA {
            return None;
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        if marker == 0xE1 && i + 10 <= b.len() && &b[i + 4..i + 10] == b"Exif\0\0" {
            let end = (i + 2 + len).min(b.len());
            return tiff_orientation(&b[i + 10..end]);
        }
        i += 2 + len;
    }
    None
}

/// Поворот из первой папки TIFF (EXIF, CR2, NEF, ARW, DNG, ORF, RW2, PEF…).
pub fn tiff_orientation(b: &[u8]) -> Option<u16> {
    let le = match b.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let r16 = |o: usize| b.get(o..o + 2).map(|s| if le { u16::from_le_bytes([s[0], s[1]]) } else { u16::from_be_bytes([s[0], s[1]]) });
    let r32 = |o: usize| b.get(o..o + 4).map(|s| if le { u32::from_le_bytes([s[0], s[1], s[2], s[3]]) } else { u32::from_be_bytes([s[0], s[1], s[2], s[3]]) });
    let ifd = r32(4)? as usize;
    let n = r16(ifd)? as usize;
    for k in 0..n.min(400) {
        let e = ifd + 2 + k * 12;
        if r16(e)? == 0x0112 {
            let v = r16(e + 8)?;
            return (1..=8).contains(&v).then_some(v);
        }
    }
    None
}

fn raw_preview(b: &[u8]) -> Result<Img, String> {
    let mut list = raw_jpegs(b);
    if list.is_empty() {
        return Err("в снимке RAW нет встроенного JPEG; для полной обработки нужно расширение «Raw Image Extension»".into());
    }
    // самый крупный, а при ошибке — следующий
    list.sort_by_key(|j| std::cmp::Reverse(j.2 as u64 * j.3 as u64));
    let orient = tiff_orientation(b);
    let mut last = String::new();
    for (o, l, _, _) in list {
        let jpg = &b[o..o + l];
        let has_own = jpeg_orientation(jpg).is_some();
        match crate::codec::load_bytes_wic(jpg) {
            Ok(img) => {
                return Ok(match (has_own, orient) {
                    (false, Some(v)) => img.apply_exif(v),
                    _ => img,
                });
            }
            Err(e) => last = e.message().to_string(),
        }
    }
    Err(format!("RAW: встроенный снимок не открылся: {last}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_formats() {
        let avif = b"\0\0\0\x1cftypavif\0\0\0\0avifmif1miaf";
        assert_eq!(sniff(avif, ""), Some(Sniff::Avif));
        let heic = b"\0\0\0\x18ftypheic\0\0\0\0mif1heic";
        assert_eq!(sniff(heic, ""), Some(Sniff::Heic));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 ", ""), Some(Sniff::WebP));
        assert_eq!(sniff(b"II*\0\x08\0\0\0", "cr2"), Some(Sniff::Raw));
        assert_eq!(sniff(b"II*\0\x08\0\0\0", "tif"), None);
    }

    #[test]
    fn finds_embedded_jpeg() {
        // настоящий JPEG 200×150 внутри «RAW»-мусора
        crate::codec::init();
        let img = Img::filled(200, 150, [10, 200, 30]);
        let jpg = crate::codec::encode_mem(&img, "jpg", 90).unwrap();
        let mut raw = b"II*\0\x08\0\0\0junk\xFF\xD8\xFF\xC3lossless".to_vec();
        raw.extend_from_slice(&jpg);
        raw.extend_from_slice(&[0xFF, 0xD9, 1, 2, 3]);
        let found = raw_jpegs(&raw);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!((found[0].2, found[0].3), (200, 150));
        let back = raw_preview(&raw).unwrap();
        assert_eq!((back.w, back.h), (200, 150));
    }

    #[test]
    fn heic_embedded_thumb() {
        let b = std::fs::read(format!("{}/tests/data/miaf003.heic", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let full = decode(&b, "heic").unwrap().unwrap();
        let t = heic_thumb(&b, 16).expect("миниатюра");
        assert!(t.w < full.w && t.h < full.h, "{}x{} vs {}x{}", t.w, t.h, full.w, full.h);
        assert!(heic_thumb(&b, 100_000).is_none());
    }

    #[test]
    fn decodes_sample_heic() {
        let b = std::fs::read(format!("{}/tests/data/miaf003.heic", env!("CARGO_MANIFEST_DIR"))).unwrap();
        assert_eq!(sniff(&b, ""), Some(Sniff::Heic));
        let img = decode(&b, "heic").unwrap().unwrap();
        assert!(img.w > 100 && img.h > 100, "{}×{}", img.w, img.h);
        assert!(img.px.chunks(4).any(|p| p[0] != p[1]), "цветная картинка");
    }

    #[test]
    fn webp_lossless_roundtrip() {
        // 2×1 WebP без потерь: синий и красный (собран image-webp)
        let mut enc = vec![];
        image_webp::WebPEncoder::new(&mut enc).encode(&[0, 0, 255, 255, 0, 0], 2, 1, image_webp::ColorType::Rgb8).unwrap();
        let img = webp(&enc).unwrap();
        assert_eq!((img.w, img.h), (2, 1));
        assert_eq!(&img.px[0..3], &[255, 0, 0]); // BGRA: синий
        assert_eq!(&img.px[4..7], &[0, 0, 255]);
    }
}
