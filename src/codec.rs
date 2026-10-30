// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Чтение/запись/масштабирование через WIC — кодеки, которые уже есть в Windows.
//! Поэтому в exe нет ни одного декодера: JPEG, PNG, BMP, GIF, TIFF «бесплатно»;
//! HEIC/WebP откроются, если в системе стоят соответствующие расширения.

use crate::image_buf::Img;
use std::path::Path;
use windows::core::*;
use windows::Win32::Foundation::{E_FAIL, GENERIC_READ};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::StructuredStorage::{CreateStreamOnHGlobal, GetHGlobalFromStream};
use windows::Win32::System::Com::*;
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};


pub fn init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

fn factory() -> Result<IWICImagingFactory> {
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
}

fn read_orientation(frame: &IWICBitmapFrameDecode) -> u16 {
    unsafe {
        let Ok(q) = frame.GetMetadataQueryReader() else { return 1 };
        for name in [w!("/app1/ifd/{ushort=274}"), w!("/ifd/{ushort=274}")] {
            let mut pv = PROPVARIANT::default();
            if q.GetMetadataByName(name, &mut pv).is_ok() {
                if let Ok(v) = u16::try_from(&pv) {
                    if (1..=8).contains(&v) {
                        return v;
                    }
                }
            }
        }
        1
    }
}

/// Ориентация из EXIF JPEG напрямую (без WIC): надёжнее и не зависит от версии системы.
fn jpeg_orientation(b: &[u8]) -> Option<u16> {
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

fn tiff_orientation(t: &[u8]) -> Option<u16> {
    let le = match t.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let r16 = |o: usize| -> Option<u16> {
        let s = t.get(o..o + 2)?;
        Some(if le { u16::from_le_bytes([s[0], s[1]]) } else { u16::from_be_bytes([s[0], s[1]]) })
    };
    let r32 = |o: usize| -> Option<u32> {
        let s = t.get(o..o + 4)?;
        Some(if le { u32::from_le_bytes([s[0], s[1], s[2], s[3]]) } else { u32::from_be_bytes([s[0], s[1], s[2], s[3]]) })
    };
    let ifd = r32(4)? as usize;
    let n = r16(ifd)? as usize;
    for k in 0..n {
        let e = ifd + 2 + k * 12;
        if r16(e)? == 0x0112 {
            let v = r16(e + 8)?;
            return (1..=8).contains(&v).then_some(v);
        }
    }
    None
}

fn file_head(path: &Path) -> Vec<u8> {
    use std::io::Read;
    let mut buf = vec![0u8; 65536];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut buf)).unwrap_or(0);
    buf.truncate(n);
    buf
}

fn decode(f: &IWICImagingFactory, dec: &IWICBitmapDecoder, exif: Option<u16>) -> Result<Img> {
    unsafe {
        let frame = dec.GetFrame(0)?;
        let orient = exif.unwrap_or_else(|| read_orientation(&frame));
        let conv = f.CreateFormatConverter()?;
        conv.Initialize(&frame, &GUID_WICPixelFormat32bppBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
        let (mut w, mut h) = (0u32, 0u32);
        conv.GetSize(&mut w, &mut h)?;
        if w == 0 || h == 0 || (w as u64 * h as u64) > 400_000_000 {
            return Err(Error::new(E_FAIL, "Слишком большое или пустое изображение"));
        }
        let mut px = vec![0u8; w as usize * h as usize * 4];
        conv.CopyPixels(std::ptr::null(), w * 4, &mut px)?;
        let mut img = Img { w, h, px };
        img.flatten_white();
        Ok(img.apply_exif(orient))
    }
}

pub fn load(path: &Path) -> Result<Img> {
    let ext = crate::util::ext_lower(path);
    if matches!(ext.as_str(), "tga" | "pnm" | "ppm" | "pgm" | "pbm" | "qoi") {
        let bytes = std::fs::read(path).map_err(|e| Error::new(E_FAIL, e.to_string()))?;
        return match crate::imgfmt::decode_own(&ext, &bytes) {
            Some(r) => r.map_err(|e| Error::new(E_FAIL, e)),
            None => Err(Error::from(E_FAIL)),
        };
    }
    unsafe {
        let f = factory()?;
        let p = crate::util::wpath(path);
        let dec = f.CreateDecoderFromFilename(PCWSTR(p.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)?;
        decode(&f, &dec, jpeg_orientation(&file_head(path)))
    }
}

/// Декодировать картинку из памяти (PNG/BMP/JPEG…).
pub fn load_bytes(bytes: &[u8]) -> Result<Img> {
    unsafe {
        let f = factory()?;
        let s = f.CreateStream()?;
        s.InitializeFromMemory(bytes)?;
        let dec = f.CreateDecoderFromStream(&s, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
        decode(&f, &dec, jpeg_orientation(bytes))
    }
}

fn container_for(ext: &str) -> GUID {
    match ext {
        "png" => GUID_ContainerFormatPng,
        "bmp" => GUID_ContainerFormatBmp,
        "tif" | "tiff" => GUID_ContainerFormatTiff,
        "gif" => GUID_ContainerFormatGif,
        "jxr" | "wdp" | "hdp" => GUID_ContainerFormatWmp,
        "heic" | "heif" => GUID_ContainerFormatHeif,
        _ => GUID_ContainerFormatJpeg,
    }
}

/// Качество JPEG по умолчанию (0..100).
pub fn jpeg_quality() -> i32 {
    crate::settings::get_i("jpeg_quality", 92).clamp(10, 100)
}

fn encode_to(f: &IWICImagingFactory, img: &Img, stream: &IStream, container: &GUID, quality: i32) -> Result<()> {
    unsafe {
        let enc = f.CreateEncoder(container, std::ptr::null())?;
        enc.Initialize(stream, WICBitmapEncoderNoCache)?;
        let mut frame: Option<IWICBitmapFrameEncode> = None;
        let mut bag: Option<windows::Win32::System::Com::StructuredStorage::IPropertyBag2> = None;
        enc.CreateNewFrame(&mut frame, &mut bag)?;
        let frame = frame.ok_or_else(|| Error::from(E_FAIL))?;
        // качество сжатия: JPEG, JPEG XR, HEIF понимают «ImageQuality» (0.0–1.0)
        if let (Some(b), true) = (&bag, *container != GUID_ContainerFormatPng && *container != GUID_ContainerFormatBmp && *container != GUID_ContainerFormatGif && *container != GUID_ContainerFormatTiff) {
            let mut name = crate::util::wide("ImageQuality");
            let pb = windows::Win32::System::Com::StructuredStorage::PROPBAG2 {
                pstrName: windows::core::PWSTR(name.as_mut_ptr()),
                ..Default::default()
            };
            let v = windows::core::VARIANT::from(quality as f32 / 100.0);
            let _ = b.Write(1, &pb, &v);
        }
        frame.Initialize(bag.as_ref())?;
        frame.SetSize(img.w, img.h)?;
        let bmp = f.CreateBitmapFromMemory(img.w, img.h, &GUID_WICPixelFormat32bppBGRA, img.w * 4, &img.px)?;
        if *container == GUID_ContainerFormatGif {
            // GIF — 256 цветов: строим палитру по картинке и переводим в индексы
            let pal = f.CreatePalette()?;
            pal.InitializeFromBitmap(&bmp, 256, false)?;
            let conv = f.CreateFormatConverter()?;
            conv.Initialize(&bmp, &GUID_WICPixelFormat8bppIndexed, WICBitmapDitherTypeErrorDiffusion, &pal, 0.0, WICBitmapPaletteTypeCustom)?;
            let mut pf = GUID_WICPixelFormat8bppIndexed;
            frame.SetPixelFormat(&mut pf)?;
            frame.SetPalette(&pal)?;
            frame.WriteSource(&conv, std::ptr::null())?;
        } else {
            let mut pf = GUID_WICPixelFormat24bppBGR;
            frame.SetPixelFormat(&mut pf)?;
            frame.WriteSource(&bmp, std::ptr::null())?;
        }
        frame.Commit()?;
        enc.Commit()?;
        Ok(())
    }
}

/// Закодировать в память (JPEG, PNG и т.д.).
pub fn encode_mem(img: &Img, ext: &str, quality: i32) -> Result<Vec<u8>> {
    unsafe {
        let f = factory()?;
        let stream = CreateStreamOnHGlobal(None, true)?;
        encode_to(&f, img, &stream, &container_for(ext), quality)?;
        let hg = GetHGlobalFromStream(&stream)?;
        // реальный размер данных — позиция потока после записи
        let mut len = 0u64;
        stream.Seek(0, windows::Win32::System::Com::STREAM_SEEK_CUR, Some(&mut len))?;
        let len = len as usize;
        let n = GlobalSize(hg).min(len.max(1));
        let p = GlobalLock(hg) as *const u8;
        if p.is_null() {
            return Err(Error::from(E_FAIL));
        }
        let v = std::slice::from_raw_parts(p, n).to_vec();
        let _ = GlobalUnlock(hg);
        Ok(v)
    }
}

/// JPEG в память (для вставки страниц-картинок в PDF).
pub fn encode_jpeg(img: &Img) -> Result<Vec<u8>> {
    encode_mem(img, "jpg", jpeg_quality())
}

/// Иконка .ico: набор размеров до 256 (картинка вписывается в квадрат).
fn encode_ico(img: &Img) -> Result<Vec<u8>> {
    let side = img.w.max(img.h);
    let mut sq = Img::filled(side, side, [255, 255, 255]);
    sq.draw(img, ((side - img.w) / 2) as i32, ((side - img.h) / 2) as i32);
    let mut parts = vec![];
    for s in [16u32, 24, 32, 48, 64, 128, 256] {
        if s > side && !parts.is_empty() {
            break;
        }
        let r = resize(&sq, s, s)?;
        parts.push((s, encode_mem(&r, "png", 100)?));
    }
    Ok(crate::imgfmt::build_ico(&parts))
}

pub fn save(img: &Img, path: &Path) -> Result<()> {
    save_q(img, path, jpeg_quality())
}

/// Сохранить с заданным качеством (для JPEG/JPEG XR/HEIF).
pub fn save_q(img: &Img, path: &Path, quality: i32) -> Result<()> {
    let ext = crate::util::ext_lower(path);
    let data = if ext == "ico" { encode_ico(img)? } else { encode_mem(img, &ext, quality)? };
    // пишем во временный файл и переименовываем: исходник не испортится при сбое
    let tmp = path.with_extension("spv-tmp");
    std::fs::write(&tmp, &data).map_err(|e| Error::new(E_FAIL, e.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        Error::new(E_FAIL, e.to_string())
    })
}

/// Качественное масштабирование (Fant) — для «Размер», печати, коллажа.
pub fn resize(img: &Img, nw: u32, nh: u32) -> Result<Img> {
    let (nw, nh) = (nw.max(1), nh.max(1));
    if nw == img.w && nh == img.h {
        return Ok(img.clone());
    }
    unsafe {
        let f = factory()?;
        let bmp = f.CreateBitmapFromMemory(img.w, img.h, &GUID_WICPixelFormat32bppBGRA, img.w * 4, &img.px)?;
        let sc = f.CreateBitmapScaler()?;
        let mode = if nw > img.w { WICBitmapInterpolationModeHighQualityCubic } else { WICBitmapInterpolationModeFant };
        if sc.Initialize(&bmp, nw, nh, mode).is_err() {
            sc.Initialize(&bmp, nw, nh, WICBitmapInterpolationModeFant)?;
        }
        let mut px = vec![0u8; nw as usize * nh as usize * 4];
        sc.CopyPixels(std::ptr::null(), nw * 4, &mut px)?;
        Ok(Img { w: nw, h: nh, px })
    }
}

/// Уменьшенная копия, вписанная в max×max (не увеличивает).
pub fn thumb(img: &Img, max: u32) -> Img {
    let k = (max as f64 / img.w as f64).min(max as f64 / img.h as f64);
    if k >= 1.0 {
        return img.clone();
    }
    let (w, h) = (((img.w as f64 * k).round() as u32).max(1), ((img.h as f64 * k).round() as u32).max(1));
    resize(img, w, h).unwrap_or_else(|_| img.clone())
}
