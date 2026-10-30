// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Кодировки текстовых файлов: UTF-8 (с BOM и без), UTF-16 LE/BE, Windows-1251.
//! Определяем при открытии и сохраняем обратно в той же кодировке и с теми же
//! переводами строк. Чистая логика, покрыта тестами.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Enc {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Cp1251,
}

impl Enc {
    pub fn label(self) -> &'static str {
        match self {
            Enc::Utf8 => "UTF-8",
            Enc::Utf8Bom => "UTF-8 BOM",
            Enc::Utf16Le => "UTF-16 LE",
            Enc::Utf16Be => "UTF-16 BE",
            Enc::Cp1251 => "Windows-1251",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Eol {
    CrLf,
    Lf,
    Cr,
}

impl Eol {
    pub fn label(self) -> &'static str {
        match self {
            Eol::CrLf => "CRLF",
            Eol::Lf => "LF",
            Eol::Cr => "CR",
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Eol::CrLf => "\r\n",
            Eol::Lf => "\n",
            Eol::Cr => "\r",
        }
    }
}

const CP1251: [u16; 128] = [0x0402, 0x0403, 0x201A, 0x0453, 0x201E, 0x2026, 0x2020, 0x2021, 0x20AC, 0x2030, 0x0409, 0x2039, 0x040A, 0x040C, 0x040B, 0x040F, 0x0452, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x003F, 0x2122, 0x0459, 0x203A, 0x045A, 0x045C, 0x045B, 0x045F, 0x00A0, 0x040E, 0x045E, 0x0408, 0x00A4, 0x0490, 0x00A6, 0x00A7, 0x0401, 0x00A9, 0x0404, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x0407, 0x00B0, 0x00B1, 0x0406, 0x0456, 0x0491, 0x00B5, 0x00B6, 0x00B7, 0x0451, 0x2116, 0x0454, 0x00BB, 0x0458, 0x0405, 0x0455, 0x0457, 0x0410, 0x0411, 0x0412, 0x0413, 0x0414, 0x0415, 0x0416, 0x0417, 0x0418, 0x0419, 0x041A, 0x041B, 0x041C, 0x041D, 0x041E, 0x041F, 0x0420, 0x0421, 0x0422, 0x0423, 0x0424, 0x0425, 0x0426, 0x0427, 0x0428, 0x0429, 0x042A, 0x042B, 0x042C, 0x042D, 0x042E, 0x042F, 0x0430, 0x0431, 0x0432, 0x0433, 0x0434, 0x0435, 0x0436, 0x0437, 0x0438, 0x0439, 0x043A, 0x043B, 0x043C, 0x043D, 0x043E, 0x043F, 0x0440, 0x0441, 0x0442, 0x0443, 0x0444, 0x0445, 0x0446, 0x0447, 0x0448, 0x0449, 0x044A, 0x044B, 0x044C, 0x044D, 0x044E, 0x044F];

fn utf16(b: &[u8], le: bool) -> String {
    let u: Vec<u16> = b.chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
    String::from_utf16_lossy(&u)
}

/// Байты → текст + найденная кодировка. Переводы строк не трогаем.
pub fn decode(b: &[u8]) -> (String, Enc) {
    if let Some(r) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return (String::from_utf8_lossy(r).into_owned(), Enc::Utf8Bom);
    }
    if let Some(r) = b.strip_prefix(&[0xFF, 0xFE]) {
        return (utf16(r, true), Enc::Utf16Le);
    }
    if let Some(r) = b.strip_prefix(&[0xFE, 0xFF]) {
        return (utf16(r, false), Enc::Utf16Be);
    }
    if let Ok(s) = std::str::from_utf8(b) {
        return (s.to_string(), Enc::Utf8);
    }
    // UTF-16 без BOM: много нулей на нечётных (LE) или чётных (BE) местах
    let sample = &b[..b.len().min(4096)];
    let zeros_odd = sample.iter().skip(1).step_by(2).filter(|&&x| x == 0).count();
    let zeros_even = sample.iter().step_by(2).filter(|&&x| x == 0).count();
    let half = sample.len() / 2;
    if half > 4 && zeros_odd * 3 > half * 2 {
        return (utf16(b, true), Enc::Utf16Le);
    }
    if half > 4 && zeros_even * 3 > half * 2 {
        return (utf16(b, false), Enc::Utf16Be);
    }
    let s = b.iter().map(|&c| if c < 128 { c as char } else { char::from_u32(CP1251[(c - 128) as usize] as u32).unwrap_or('?') }).collect();
    (s, Enc::Cp1251)
}

/// Какие переводы строк в тексте (по большинству).
pub fn detect_eol(s: &str) -> Eol {
    let crlf = s.matches("\r\n").count();
    let lf = s.matches('\n').count() - crlf;
    let cr = s.matches('\r').count() - crlf;
    if crlf >= lf && crlf >= cr && crlf > 0 {
        Eol::CrLf
    } else if cr > lf {
        Eol::Cr
    } else if lf > 0 {
        Eol::Lf
    } else {
        Eol::CrLf
    }
}

/// Привести все переводы строк к одному виду.
pub fn normalize_eol(s: &str, eol: Eol) -> String {
    let unified = s.replace("\r\n", "\n").replace('\r', "\n");
    if eol == Eol::Lf { unified } else { unified.replace('\n', eol.as_str()) }
}

/// Текст → байты в нужной кодировке. Символы, которых нет в 1251, станут «?».
pub fn encode(s: &str, enc: Enc) -> Vec<u8> {
    match enc {
        Enc::Utf8 => s.as_bytes().to_vec(),
        Enc::Utf8Bom => [&[0xEF, 0xBB, 0xBF][..], s.as_bytes()].concat(),
        Enc::Utf16Le => [0xFF, 0xFE].into_iter().chain(s.encode_utf16().flat_map(|u| u.to_le_bytes())).collect(),
        Enc::Utf16Be => [0xFE, 0xFF].into_iter().chain(s.encode_utf16().flat_map(|u| u.to_be_bytes())).collect(),
        Enc::Cp1251 => s
            .chars()
            .map(|c| {
                if (c as u32) < 128 {
                    c as u8
                } else {
                    CP1251.iter().position(|&u| u as u32 == c as u32).map(|i| i as u8 + 128).unwrap_or(b'?')
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all() {
        let text = "Привет, мир! ёЁ №\r\nline 2";
        for e in [Enc::Utf8, Enc::Utf8Bom, Enc::Utf16Le, Enc::Utf16Be, Enc::Cp1251] {
            let b = encode(text, e);
            let (s, got) = decode(&b);
            assert_eq!(s, text, "{e:?}");
            assert_eq!(got, e, "{e:?}");
        }
    }

    #[test]
    fn eol() {
        assert_eq!(detect_eol("a\r\nb\r\nc\n"), Eol::CrLf);
        assert_eq!(detect_eol("a\nb\nc"), Eol::Lf);
        assert_eq!(detect_eol("a\rb"), Eol::Cr);
        assert_eq!(normalize_eol("a\r\nb\nc\rd", Eol::CrLf), "a\r\nb\r\nc\r\nd");
        assert_eq!(normalize_eol("a\r\nb", Eol::Lf), "a\nb");
    }
}
