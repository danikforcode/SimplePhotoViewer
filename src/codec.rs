// SimplePhotoViewer (Rust/Win32) — © 2026 danikforcode
// Лицензия MIT (см. LICENSE). Распространяется «как есть», без гарантий.
//! Чтение/запись/масштабирование через WIC — кодеки, которые уже есть в Windows.
//! Поэтому в exe нет ни одного декодера: JPEG, PNG, BMP, GIF, TIFF «бесплатно»;
//! HEIC, AVIF, WebP и RAW открываются и без расширений Microsoft — своими декодерами (extcodec.rs).

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

pub(crate) use crate::extcodec::jpeg_orientation;

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
    let wic = unsafe {
        (|| {
            let f = factory()?;
            let p = crate::util::wpath(path);
            let dec = f.CreateDecoderFromFilename(PCWSTR(p.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)?;
            decode(&f, &dec, jpeg_orientation(&file_head(path)))
        })()
    };
    // RAW: системный TIFF-декодер «открывает» многие RAW, но отдаёт крошечную
    // миниатюру из первой папки; тогда лучше встроенный полноразмерный JPEG
    if crate::kinds::is_raw_ext(&ext) {
        if let Ok(bytes) = std::fs::read(path) {
            let best = crate::extcodec::raw_jpegs(&bytes).iter().map(|j| j.2 as u64 * j.3 as u64).max().unwrap_or(0);
            let wic_area = wic.as_ref().map(|i| i.w as u64 * i.h as u64).unwrap_or(0);
            if best > 0 && wic_area * 10 < best * 8 {
                if let Some(Ok(img)) = crate::extcodec::decode(&bytes, &ext) {
                    return Ok(img);
                }
            }
        }
    }
    match wic {
        Ok(img) => Ok(img),
        Err(e) => {
            // в системе нет кодека (HEIC без HEVC, AVIF, WebP, RAW) — свои декодеры
            let head = file_head(path);
            if crate::extcodec::sniff(&head, &ext).is_none() {
                return Err(e);
            }
            let bytes = std::fs::read(path).map_err(|e| Error::new(E_FAIL, e.to_string()))?;
            match crate::extcodec::decode(&bytes, &ext) {
                Some(r) => r.map_err(|m| Error::new(E_FAIL, m)),
                None => Err(e),
            }
        }
    }
}

/// Декодировать картинку из памяти (PNG/BMP/JPEG… и свои форматы, если WIC не смог).
pub fn load_bytes(bytes: &[u8]) -> Result<Img> {
    match load_bytes_wic(bytes) {
        Ok(i) => Ok(i),
        Err(e) => match crate::extcodec::decode(bytes, "") {
            Some(r) => r.map_err(|m| Error::new(E_FAIL, m)),
            None => Err(e),
        },
    }
}

/// Только системный WIC (без своих декодеров).
pub fn load_bytes_wic(bytes: &[u8]) -> Result<Img> {
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

thread_local! {
    /// Разрешение (точек на дюйм), записываемое в файл; 0 — не трогать.
    static SAVE_DPI: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
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
        let dpi = SAVE_DPI.with(|d| d.get());
        if dpi > 0.0 {
            let _ = frame.SetResolution(dpi, dpi);
        }
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
        // реальный размер данных — конец потока (не текущая позиция: TIFF и другие
        // кодировщики под конец возвращаются в начало файла дописать заголовок)
        let mut len = 0u64;
        stream.Seek(0, windows::Win32::System::Com::STREAM_SEEK_END, Some(&mut len))?;
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
/// Сохранить с качеством и разрешением (DPI) — для окна «Размер и формат».
pub fn save_full(img: &Img, path: &Path, quality: i32, dpi: f64) -> Result<()> {
    SAVE_DPI.with(|d| d.set(dpi));
    let r = save_q(img, path, quality);
    SAVE_DPI.with(|d| d.set(0.0));
    r
}

/// WebP без потерь (свой кодировщик: в Windows записи WebP нет).
pub fn encode_webp(img: &Img) -> Result<Vec<u8>> {
    let rgb: Vec<u8> = img.px.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0]]).collect();
    let mut out = vec![];
    image_webp::WebPEncoder::new(&mut out).encode(&rgb, img.w, img.h, image_webp::ColorType::Rgb8).map_err(|e| Error::new(E_FAIL, format!("WebP: {e}")))?;
    Ok(out)
}

pub fn save_q(img: &Img, path: &Path, quality: i32) -> Result<()> {
    let ext = crate::util::ext_lower(path);
    let data = match ext.as_str() {
        "ico" => encode_ico(img)?,
        "webp" => encode_webp(img)?,
        _ => encode_mem(img, &ext, quality)?,
    };
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

/// Быстрая уменьшенная копия файла, вписанная в max×max: для HEIC —
/// встроенная миниатюра, для JPEG и др. — уменьшение прямо при чтении
/// (WIC сжимает JPEG ещё на этапе DCT, это в разы быстрее полного декода).
/// Если не вышло — обычная загрузка и уменьшение.
pub fn load_small(path: &Path, max: u32) -> Result<Img> {
    let ext = crate::util::ext_lower(path);
    let head = file_head(path);
    let raw = crate::kinds::is_raw_ext(&ext);
    match crate::extcodec::sniff(&head, &ext) {
        Some(crate::extcodec::Sniff::Heic) => {
            if let Ok(bytes) = std::fs::read(path) {
                if let Some(t) = crate::extcodec::heic_thumb(&bytes, max * 3 / 4) {
                    return Ok(thumb(&t, max));
                }
            }
        }
        None if !raw => {
            let r = unsafe {
                (|| -> Result<Img> {
                    let f = factory()?;
                    let p = crate::util::wpath(path);
                    let dec = f.CreateDecoderFromFilename(PCWSTR(p.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)?;
                    let frame = dec.GetFrame(0)?;
                    let orient = jpeg_orientation(&head).unwrap_or_else(|| read_orientation(&frame));
                    let (mut w, mut h) = (0u32, 0u32);
                    frame.GetSize(&mut w, &mut h)?;
                    if w == 0 || h == 0 {
                        return Err(Error::from(E_FAIL));
                    }
                    let k = (max as f64 / w as f64).min(max as f64 / h as f64);
                    let src: IWICBitmapSource = if k < 1.0 {
                        let (nw, nh) = (((w as f64 * k).round() as u32).max(1), ((h as f64 * k).round() as u32).max(1));
                        let sc = f.CreateBitmapScaler()?;
                        sc.Initialize(&frame, nw, nh, WICBitmapInterpolationModeFant)?;
                        sc.cast()?
                    } else {
                        frame.cast()?
                    };
                    let conv = f.CreateFormatConverter()?;
                    conv.Initialize(&src, &GUID_WICPixelFormat32bppBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
                    conv.GetSize(&mut w, &mut h)?;
                    let mut px = vec![0u8; w as usize * h as usize * 4];
                    conv.CopyPixels(std::ptr::null(), w * 4, &mut px)?;
                    let mut img = Img { w, h, px };
                    img.flatten_white();
                    Ok(img.apply_exif(orient))
                })()
            };
            if r.is_ok() {
                return r;
            }
        }
        _ => {}
    }
    load(path).map(|i| thumb(&i, max))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Каждый формат: записать в память → прочитать → тот же размер и цвет.
    /// (Был сбой: TIFF обрезался — длину брали по позиции потока, а не по его концу.)
    #[test]
    fn encode_decode_roundtrip() {
        init();
        let mut img = Img::filled(173, 91, [20, 140, 230]);
        img.px[0..4].copy_from_slice(&[0, 0, 0, 255]);
        for ext in ["jpg", "png", "bmp", "tif", "gif"] {
            let data = encode_mem(&img, ext, 95).unwrap_or_else(|e| panic!("{ext}: {e}"));
            let back = load_bytes(&data).unwrap_or_else(|e| panic!("{ext}: {e}"));
            assert_eq!((back.w, back.h), (173, 91), "{ext}");
            let k = ((50 * 173 + 80) * 4) as usize;
            for c in 0..3 {
                assert!((back.px[k + c] as i32 - img.px[k + c] as i32).abs() <= 12, "{ext}: {:?}", &back.px[k..k + 3]);
            }
        }
    }
}
