// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Свои маленькие декодеры форматов, которых нет в Windows: TGA, PNM (PPM/PGM/PBM),
//! QOI. И запись ICO (иконки) — в Windows есть только чтение.
//! Чистая логика без Win32, покрыта тестами.

use crate::image_buf::Img;

fn rgba_to_img(w: u32, h: u32, mut px: Vec<u8>) -> Img {
    // px — BGRA; заливаем прозрачность белым, как и для остальных форматов
    let mut img = Img { w, h, px: std::mem::take(&mut px) };
    img.flatten_white();
    img
}

fn check_size(w: u64, h: u64) -> Result<(), String> {
    if w == 0 || h == 0 || w * h > 200_000_000 {
        Err("неверный размер изображения".into())
    } else {
        Ok(())
    }
}

// --------------------------------------------------------------------- TGA ---
/// TGA: цвет 15/16/24/32 бит, градации серого, палитра; без сжатия и RLE.
pub fn decode_tga(b: &[u8]) -> Result<Img, String> {
    if b.len() < 18 {
        return Err("файл TGA повреждён".into());
    }
    let id_len = b[0] as usize;
    let cmap_type = b[1];
    let itype = b[2];
    let cm_first = u16::from_le_bytes([b[3], b[4]]) as usize;
    let cm_len = u16::from_le_bytes([b[5], b[6]]) as usize;
    let cm_bits = b[7] as usize;
    let w = u16::from_le_bytes([b[12], b[13]]) as u32;
    let h = u16::from_le_bytes([b[14], b[15]]) as u32;
    let bpp = b[16] as usize;
    let desc = b[17];
    check_size(w as u64, h as u64)?;
    let rle = itype >= 9;
    let base = itype & 7;
    if !matches!(base, 1 | 2 | 3) {
        return Err(format!("тип TGA {itype} не поддерживается"));
    }
    let mut pos = 18 + id_len;
    let mut cmap: Vec<[u8; 4]> = vec![];
    if cmap_type == 1 {
        let eb = cm_bits.div_ceil(8);
        for i in 0..cm_len {
            let o = pos + i * eb;
            let e = b.get(o..o + eb).ok_or("палитра TGA повреждена")?;
            cmap.push(color(e, cm_bits));
        }
        pos += cm_len * eb;
    }
    let pb = bpp.div_ceil(8);
    if pb == 0 || pb > 4 {
        return Err("глубина цвета TGA не поддерживается".into());
    }
    let n = (w * h) as usize;
    let mut raw: Vec<[u8; 4]> = Vec::with_capacity(n);
    let pixel = |e: &[u8]| -> Result<[u8; 4], String> {
        Ok(match base {
            1 => {
                let idx = if pb == 1 { e[0] as usize } else { u16::from_le_bytes([e[0], e[1]]) as usize };
                *cmap.get(idx.wrapping_sub(cm_first)).ok_or("индекс палитры TGA вне диапазона")?
            }
            3 => [e[0], e[0], e[0], if pb >= 2 { e[1] } else { 255 }],
            _ => color(e, bpp),
        })
    };
    if rle {
        while raw.len() < n {
            let hdr = *b.get(pos).ok_or("данные TGA обрываются")?;
            pos += 1;
            let cnt = (hdr & 0x7F) as usize + 1;
            if hdr & 0x80 != 0 {
                let p = pixel(b.get(pos..pos + pb).ok_or("данные TGA обрываются")?)?;
                pos += pb;
                for _ in 0..cnt.min(n - raw.len()) {
                    raw.push(p);
                }
            } else {
                for _ in 0..cnt.min(n - raw.len()) {
                    raw.push(pixel(b.get(pos..pos + pb).ok_or("данные TGA обрываются")?)?);
                    pos += pb;
                }
            }
        }
    } else {
        for i in 0..n {
            let o = pos + i * pb;
            raw.push(pixel(b.get(o..o + pb).ok_or("данные TGA обрываются")?)?);
        }
    }
    // у 32-битных TGA альфа часто нулевая «по ошибке» — тогда её игнорируем
    let alpha_bits = desc & 0x0F;
    let all_zero_alpha = raw.iter().all(|p| p[3] == 0);
    let top = desc & 0x20 != 0;
    let right = desc & 0x10 != 0;
    let mut px = vec![0u8; n * 4];
    for y in 0..h as usize {
        let sy = if top { y } else { h as usize - 1 - y };
        for x in 0..w as usize {
            let sx = if right { w as usize - 1 - x } else { x };
            let mut p = raw[sy * w as usize + sx];
            if alpha_bits == 0 || all_zero_alpha {
                p[3] = 255;
            }
            px[(y * w as usize + x) * 4..][..4].copy_from_slice(&p);
        }
    }
    Ok(rgba_to_img(w, h, px))
}

/// Цвет TGA (порядок B,G,R[,A]) → BGRA.
fn color(e: &[u8], bits: usize) -> [u8; 4] {
    match bits {
        15 | 16 => {
            let v = u16::from_le_bytes([e[0], e[1]]);
            let c = |s: u16| (((v >> s) & 31) as u32 * 255 / 31) as u8;
            [c(0), c(5), c(10), 255]
        }
        24 => [e[0], e[1], e[2], 255],
        32 => [e[0], e[1], e[2], e[3]],
        _ => [e[0], e[0], e[0], 255],
    }
}

// --------------------------------------------------------------------- PNM ---
/// PBM/PGM/PPM (P1–P6), 8 и 16 бит.
pub fn decode_pnm(b: &[u8]) -> Result<Img, String> {
    if b.len() < 2 || b[0] != b'P' || !(b'1'..=b'6').contains(&b[1]) {
        return Err("это не файл PNM".into());
    }
    let kind = b[1] - b'0';
    let mut pos = 2;
    let next = |pos: &mut usize| -> Result<u32, String> {
        loop {
            while *pos < b.len() && b[*pos].is_ascii_whitespace() {
                *pos += 1;
            }
            if *pos < b.len() && b[*pos] == b'#' {
                while *pos < b.len() && b[*pos] != b'\n' {
                    *pos += 1;
                }
                continue;
            }
            break;
        }
        let s = *pos;
        while *pos < b.len() && b[*pos].is_ascii_digit() {
            *pos += 1;
        }
        std::str::from_utf8(&b[s..*pos]).ok().and_then(|t| t.parse().ok()).ok_or_else(|| "заголовок PNM повреждён".to_string())
    };
    let w = next(&mut pos)?;
    let h = next(&mut pos)?;
    check_size(w as u64, h as u64)?;
    let maxv = if kind == 1 || kind == 4 { 1 } else { next(&mut pos)?.max(1) };
    let n = (w * h) as usize;
    let mut px = vec![255u8; n * 4];
    let scale = |v: u32| (v.min(maxv) * 255 / maxv) as u8;
    let mut set = |i: usize, r: u8, g: u8, bl: u8| {
        px[i * 4] = bl;
        px[i * 4 + 1] = g;
        px[i * 4 + 2] = r;
    };
    match kind {
        1 | 2 | 3 => {
            let ch = if kind == 3 { 3 } else { 1 };
            for i in 0..n {
                let mut v = [0u32; 3];
                for c in v.iter_mut().take(ch) {
                    if kind == 1 {
                        // в P1 цифры могут идти без пробелов
                        while pos < b.len() && !matches!(b[pos], b'0' | b'1') {
                            pos += 1;
                        }
                        *c = if pos < b.len() && b[pos] == b'1' { 0 } else { 1 };
                        pos += 1;
                    } else {
                        *c = next(&mut pos)?;
                    }
                }
                let (r, g, bl) = if ch == 3 { (scale(v[0]), scale(v[1]), scale(v[2])) } else { let g = scale(v[0]); (g, g, g) };
                set(i, r, g, bl);
            }
        }
        4 => {
            pos += 1;
            let stride = (w as usize).div_ceil(8);
            for y in 0..h as usize {
                for x in 0..w as usize {
                    let byte = *b.get(pos + y * stride + x / 8).ok_or("данные PBM обрываются")?;
                    let v = if byte & (0x80 >> (x % 8)) != 0 { 0 } else { 255 };
                    set(y * w as usize + x, v, v, v);
                }
            }
        }
        _ => {
            pos += 1;
            let ch = if kind == 6 { 3 } else { 1 };
            let bps = if maxv > 255 { 2 } else { 1 };
            let need = n * ch * bps;
            let d = b.get(pos..pos + need).ok_or("данные PNM обрываются")?;
            for i in 0..n {
                let get = |c: usize| -> u32 {
                    let o = (i * ch + c) * bps;
                    if bps == 2 { u16::from_be_bytes([d[o], d[o + 1]]) as u32 } else { d[o] as u32 }
                };
                let (r, g, bl) = if ch == 3 { (scale(get(0)), scale(get(1)), scale(get(2))) } else { let g = scale(get(0)); (g, g, g) };
                set(i, r, g, bl);
            }
        }
    }
    Ok(Img { w, h, px })
}

// --------------------------------------------------------------------- QOI ---
pub fn decode_qoi(b: &[u8]) -> Result<Img, String> {
    if b.len() < 22 || &b[0..4] != b"qoif" {
        return Err("это не файл QOI".into());
    }
    let w = u32::from_be_bytes([b[4], b[5], b[6], b[7]]);
    let h = u32::from_be_bytes([b[8], b[9], b[10], b[11]]);
    check_size(w as u64, h as u64)?;
    let n = (w as usize) * (h as usize);
    let mut px = Vec::with_capacity(n * 4);
    let mut index = [[0u8; 4]; 64];
    let mut p = [0u8, 0, 0, 255]; // RGBA
    let mut pos = 14;
    let end = b.len().saturating_sub(8);
    let mut run = 0u32;
    while px.len() < n * 4 {
        if run > 0 {
            run -= 1;
        } else if pos < end {
            let t = b[pos];
            pos += 1;
            if t == 0xFE {
                p[0] = b[pos];
                p[1] = b[pos + 1];
                p[2] = b[pos + 2];
                pos += 3;
            } else if t == 0xFF {
                p.copy_from_slice(&b[pos..pos + 4]);
                pos += 4;
            } else {
                match t >> 6 {
                    0 => p = index[t as usize & 63],
                    1 => {
                        p[0] = p[0].wrapping_add(((t >> 4) & 3).wrapping_sub(2));
                        p[1] = p[1].wrapping_add(((t >> 2) & 3).wrapping_sub(2));
                        p[2] = p[2].wrapping_add((t & 3).wrapping_sub(2));
                    }
                    2 => {
                        let b2 = b[pos];
                        pos += 1;
                        let vg = (t & 63).wrapping_sub(32);
                        p[0] = p[0].wrapping_add(vg.wrapping_sub(8).wrapping_add((b2 >> 4) & 15));
                        p[1] = p[1].wrapping_add(vg);
                        p[2] = p[2].wrapping_add(vg.wrapping_sub(8).wrapping_add(b2 & 15));
                    }
                    _ => run = (t & 63) as u32,
                }
            }
            let h = (p[0] as usize * 3 + p[1] as usize * 5 + p[2] as usize * 7 + p[3] as usize * 11) % 64;
            index[h] = p;
        } else {
            return Err("данные QOI обрываются".into());
        }
        px.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
    }
    Ok(rgba_to_img(w, h, px))
}

/// Свой декодер по расширению (None — пусть читает WIC).
pub fn decode_own(ext: &str, bytes: &[u8]) -> Option<Result<Img, String>> {
    match ext {
        "tga" => Some(decode_tga(bytes)),
        "pnm" | "ppm" | "pgm" | "pbm" => Some(decode_pnm(bytes)),
        "qoi" => Some(decode_qoi(bytes)),
        _ => None,
    }
}

// --------------------------------------------------------------------- ICO ---
/// Собрать .ico из готовых PNG (каждый — квадрат size×size, size ≤ 256).
pub fn build_ico(pngs: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = vec![0, 0, 1, 0];
    out.extend_from_slice(&(pngs.len() as u16).to_le_bytes());
    let mut off = 6 + 16 * pngs.len() as u32;
    for (size, data) in pngs {
        let s = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[s, s, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&off.to_le_bytes());
        off += data.len() as u32;
    }
    for (_, data) in pngs {
        out.extend_from_slice(data);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tga_uncompressed_bottom_up() {
        // 2×1, 24 бит, снизу вверх (одна строка — без разницы), пиксели: синий, красный
        let mut b = vec![0u8; 18];
        b[2] = 2;
        b[12] = 2;
        b[14] = 1;
        b[16] = 24;
        b.extend_from_slice(&[255, 0, 0, 0, 0, 255]);
        let img = decode_tga(&b).unwrap();
        assert_eq!(&img.px, &[255, 0, 0, 255, 0, 0, 255, 255]);
    }

    #[test]
    fn tga_rle_32_top_left() {
        let mut b = vec![0u8; 18];
        b[2] = 10;
        b[12] = 3;
        b[14] = 1;
        b[16] = 32;
        b[17] = 0x28; // сверху вниз, 8 бит альфы
        b.extend_from_slice(&[0x82, 1, 2, 3, 255]); // повтор 3 раза
        let img = decode_tga(&b).unwrap();
        assert_eq!(img.px, [1, 2, 3, 255].repeat(3));
    }

    #[test]
    fn pnm_variants() {
        let p3 = b"P3\n# comment\n2 1\n255\n255 0 0  0 0 255\n";
        let img = decode_pnm(p3).unwrap();
        assert_eq!(&img.px, &[0, 0, 255, 255, 255, 0, 0, 255]);
        let p1 = b"P1 3 1 101";
        let img = decode_pnm(p1).unwrap();
        assert_eq!(&img.px, &[0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255]);
        let mut p5 = b"P5 2 1 255\n".to_vec();
        p5.extend_from_slice(&[10, 200]);
        let img = decode_pnm(&p5).unwrap();
        assert_eq!(&img.px, &[10, 10, 10, 255, 200, 200, 200, 255]);
    }

    #[test]
    fn qoi_basic() {
        // 2×1: QOI_OP_RGB красный, затем QOI_OP_RUN длиной 1
        let mut b = b"qoif".to_vec();
        b.extend_from_slice(&2u32.to_be_bytes());
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(&[4, 0]);
        b.extend_from_slice(&[0xFE, 255, 0, 0, 0xC0]);
        b.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
        let img = decode_qoi(&b).unwrap();
        assert_eq!(&img.px, &[0, 0, 255, 255, 0, 0, 255, 255]);
    }

    #[test]
    fn ico_layout() {
        let ico = build_ico(&[(16, vec![1, 2, 3]), (256, vec![4])]);
        assert_eq!(&ico[0..6], &[0, 0, 1, 0, 2, 0]);
        assert_eq!(ico[6], 16);
        assert_eq!(ico[22], 0); // 256 записывается как 0
        assert_eq!(u32::from_le_bytes(ico[18..22].try_into().unwrap()), 38);
        assert_eq!(&ico[38..], &[1, 2, 3, 4]);
    }
}
