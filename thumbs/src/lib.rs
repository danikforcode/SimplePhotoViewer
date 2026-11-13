// SimplePhotoViewer (Rust/Win32) — © 2026 danikforcode
// Лицензия MIT (см. LICENSE). Распространяется «как есть», без гарантий.
//! Миниатюры в Проводнике (IThumbnailProvider), когда SimplePhotoViewer —
//! программа по умолчанию. Сначала — системный обработчик миниатюр для этого
//! типа файла (тот, что был бы без нас: фото, видео, PDF, Office…), а если его
//! нет или он не справился — свои: WIC, HEIC/AVIF/WebP/RAW (те же декодеры,
//! что в программе) и первая страница PDF.
//! DLL кладёт и регистрирует сама программа (assoc.rs), только для текущего
//! пользователя: права администратора не нужны.

#![allow(clippy::missing_safety_doc)]

#[path = "../../src/avif.rs"]
mod avif;
#[path = "../../src/extcodec.rs"]
mod extcodec;
#[path = "../../src/heif.rs"]
mod heif;
#[path = "../../src/image_buf.rs"]
mod image_buf;
#[path = "../../src/mp4.rs"]
mod mp4;

use image_buf::Img;
use std::cell::RefCell;
use std::ffi::c_void;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::PropertiesSystem::*;
use windows::Win32::UI::Shell::*;

/// Наш класс миниатюр (тот же номер — в assoc.rs программы).
pub const CLSID_SPV_THUMBS: GUID = GUID::from_u128(0x6b1c4e2a_3f8d_4a57_9e21_5c7d0b8a4f13);
const THUMB_IID: &str = "{e357fccd-a995-4576-b01f-234630154e96}";

// ------------------------------------------------- то, что ждёт extcodec ---
mod codec {
    use crate::image_buf::Img;
    use windows::core::*;
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::*;

    pub use crate::extcodec::jpeg_orientation;

    pub fn factory() -> Result<IWICImagingFactory> {
        unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
    }

    /// Расшифровать через WIC; max > 0 — сразу уменьшить (JPEG уменьшается при чтении — быстро).
    pub fn wic(bytes: &[u8], max: u32) -> Result<Img> {
        unsafe {
            let f = factory()?;
            let s = f.CreateStream()?;
            s.InitializeFromMemory(bytes)?;
            let dec = f.CreateDecoderFromStream(&s, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
            let frame = dec.GetFrame(0)?;
            let (mut w, mut h) = (0u32, 0u32);
            frame.GetSize(&mut w, &mut h)?;
            if w == 0 || h == 0 {
                return Err(Error::from(windows::Win32::Foundation::E_FAIL));
            }
            let src: IWICBitmapSource = if max > 0 && w.max(h) > max {
                let k = max as f64 / w.max(h) as f64;
                let sc = f.CreateBitmapScaler()?;
                sc.Initialize(&frame, ((w as f64 * k).round() as u32).max(1), ((h as f64 * k).round() as u32).max(1), WICBitmapInterpolationModeFant)?;
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
            Ok(match jpeg_orientation(bytes) {
                Some(o) => img.apply_exif(o),
                None => img,
            })
        }
    }

    pub fn load_bytes_wic(bytes: &[u8]) -> Result<Img> {
        wic(bytes, 0)
    }
}

mod kinds {
    pub fn is_raw_ext(ext: &str) -> bool {
        crate::extcodec::is_raw_ext(ext)
    }
}

// ------------------------------------------------------------ DLL ---
static mut COUNT: i32 = 0;

#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if unsafe { COUNT } <= 0 {
        S_OK
    } else {
        S_FALSE
    }
}

#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(rclsid: *const GUID, riid: *const GUID, ppv: *mut *mut c_void) -> HRESULT {
    if ppv.is_null() || rclsid.is_null() {
        return E_POINTER;
    }
    *ppv = std::ptr::null_mut();
    if *rclsid != CLSID_SPV_THUMBS {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let f: IClassFactory = Factory.into();
    f.query(riid, ppv)
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Option<&IUnknown>, riid: *const GUID, ppv: *mut *mut c_void) -> Result<()> {
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let p: IThumbnailProvider = Provider { st: RefCell::new(St::default()) }.into();
        unsafe { p.query(riid, ppv).ok() }
    }
    fn LockServer(&self, lock: BOOL) -> Result<()> {
        unsafe {
            COUNT += if lock.as_bool() { 1 } else { -1 };
        }
        Ok(())
    }
}

#[derive(Default)]
struct St {
    stream: Option<IStream>,
    item: Option<IShellItem>,
    path: Option<String>,
}

#[implement(IThumbnailProvider, IInitializeWithStream, IInitializeWithItem)]
struct Provider {
    st: RefCell<St>,
}

impl IInitializeWithStream_Impl for Provider_Impl {
    fn Initialize(&self, stream: Option<&IStream>, _mode: u32) -> Result<()> {
        let s = stream.ok_or(Error::from(E_INVALIDARG))?.clone();
        // имя файла (для расширения) — из описания потока
        let name = unsafe {
            let mut st = STATSTG::default();
            if s.Stat(&mut st, STATFLAG_DEFAULT).is_ok() && !st.pwcsName.is_null() {
                let n = st.pwcsName.to_string().ok();
                CoTaskMemFree(Some(st.pwcsName.0 as *const c_void));
                n
            } else {
                None
            }
        };
        let mut g = self.st.borrow_mut();
        g.stream = Some(s);
        g.path = name;
        Ok(())
    }
}

impl IInitializeWithItem_Impl for Provider_Impl {
    fn Initialize(&self, item: Option<&IShellItem>, _mode: u32) -> Result<()> {
        let it = item.ok_or(Error::from(E_INVALIDARG))?.clone();
        let path = unsafe {
            it.GetDisplayName(SIGDN_FILESYSPATH).ok().and_then(|p| {
                let s = p.to_string().ok();
                CoTaskMemFree(Some(p.0 as *const c_void));
                s
            })
        };
        let mut g = self.st.borrow_mut();
        g.item = Some(it);
        g.path = path;
        Ok(())
    }
}

impl IThumbnailProvider_Impl for Provider_Impl {
    fn GetThumbnail(&self, cx: u32, phbmp: *mut HBITMAP, palpha: *mut WTS_ALPHATYPE) -> Result<()> {
        if phbmp.is_null() || palpha.is_null() {
            return Err(E_POINTER.into());
        }
        let g = self.st.borrow();
        let ext = g.path.as_deref().and_then(|p| p.rsplit_once('.')).map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        // 1) системный обработчик для этого типа — как было бы без нас
        if !ext.is_empty() {
            if let Some(clsid) = system_handler(&ext) {
                if let Ok((b, a)) = delegate(&clsid, &g, cx) {
                    unsafe {
                        *phbmp = b;
                        *palpha = a;
                    }
                    return Ok(());
                }
            }
        }
        // 2) своими силами: видео HEVC (без кодека в системе) — кадр из файла, не читая его целиком
        if matches!(ext.as_str(), "mp4" | "m4v" | "mov" | "3gp" | "3g2") {
            if let Some(s) = &g.stream {
                let mut r = StreamReader(s.clone());
                let len = unsafe {
                    let mut st = STATSTG::default();
                    s.Stat(&mut st, STATFLAG_NONAME).map(|_| st.cbSize).unwrap_or(0)
                };
                let img = mp4::read_moov_from(&mut r, len).ok().and_then(|m| mp4::hevc_thumb(&mut r, &m, cx)).ok_or(Error::from(E_FAIL))?;
                let b = to_hbitmap(&img)?;
                unsafe {
                    *phbmp = b;
                    *palpha = WTSAT_RGB;
                }
                return Ok(());
            }
            return Err(E_FAIL.into());
        }
        let bytes = read_all(&g)?;
        let img = own(&bytes, &ext, cx).ok_or(Error::from(E_FAIL))?;
        let b = to_hbitmap(&img)?;
        unsafe {
            *phbmp = b;
            *palpha = WTSAT_RGB;
        }
        Ok(())
    }
}

// --------------------------------------------------- системный обработчик ---
fn reg_str(root: HKEY, path: &str, name: Option<&str>) -> Option<String> {
    unsafe {
        let p = HSTRING::from(path);
        let n = name.map(HSTRING::from);
        let mut buf = vec![0u16; 512];
        let mut sz = (buf.len() * 2) as u32;
        let r = RegGetValueW(root, &p, n.as_ref().map(|h| PCWSTR(h.as_ptr())).unwrap_or(PCWSTR::null()), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut c_void), Some(&mut sz));
        if r.is_err() {
            return None;
        }
        let len = (sz as usize / 2).saturating_sub(1);
        let s = String::from_utf16_lossy(&buf[..len.min(buf.len())]);
        (!s.is_empty()).then_some(s)
    }
}

fn reg_values(root: HKEY, path: &str) -> Vec<String> {
    let mut out = vec![];
    unsafe {
        let mut k = HKEY::default();
        if RegOpenKeyExW(root, &HSTRING::from(path), 0, KEY_READ, &mut k).is_err() {
            return out;
        }
        for i in 0..64 {
            let mut name = vec![0u16; 256];
            let mut n = name.len() as u32;
            if RegEnumValueW(k, i, PWSTR(name.as_mut_ptr()), &mut n, None, None, None, None).is_err() {
                break;
            }
            out.push(String::from_utf16_lossy(&name[..n as usize]));
        }
        let _ = RegCloseKey(k);
    }
    out
}

/// Обработчик миниатюр, который был бы у этого расширения без нас.
fn system_handler(ext: &str) -> Option<GUID> {
    let ours = format!("{CLSID_SPV_THUMBS:?}").to_ascii_lowercase();
    let mut places = vec![format!("SystemFileAssociations\\.{ext}"), format!(".{ext}")];
    // «родной» тип файла (из HKLM — наш тип записан только в HKCU)
    let lm = |p: &str, n: Option<&str>| reg_str(HKEY_LOCAL_MACHINE, &format!("SOFTWARE\\Classes\\{p}"), n);
    if let Some(pid) = lm(&format!(".{ext}"), None) {
        places.push(pid);
    }
    for pid in reg_values(HKEY_LOCAL_MACHINE, &format!("SOFTWARE\\Classes\\.{ext}\\OpenWithProgids")) {
        if !pid.starts_with("SimplePhotoViewer") {
            places.push(pid);
        }
    }
    if let Some(pt) = lm(&format!(".{ext}"), Some("PerceivedType")) {
        places.push(format!("SystemFileAssociations\\{pt}"));
    }
    for p in places {
        for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
            let base = format!("SOFTWARE\\Classes\\{p}\\ShellEx\\{THUMB_IID}");
            if let Some(v) = reg_str(root, &base, None) {
                let v = v.trim().to_ascii_lowercase();
                if v.contains(ours.trim_matches(|c| c == '{' || c == '}')) {
                    continue;
                }
                if let Some(g) = parse_guid(&v) {
                    return Some(g);
                }
            }
        }
    }
    None
}

/// «{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}» → GUID (без паники на мусоре).
fn parse_guid(s: &str) -> Option<GUID> {
    let hex: String = s.chars().filter(|c| *c != '{' && *c != '}' && *c != '-').collect();
    if hex.len() != 32 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u128::from_str_radix(&hex, 16).ok().map(GUID::from_u128)
}

/// Передать работу системному обработчику.
fn delegate(clsid: &GUID, st: &St, cx: u32) -> Result<(HBITMAP, WTS_ALPHATYPE)> {
    unsafe {
        let h: IThumbnailProvider = CoCreateInstance(clsid, None, CLSCTX_INPROC_SERVER)?;
        let mut done = false;
        if let Some(s) = &st.stream {
            if let Ok(i) = h.cast::<IInitializeWithStream>() {
                let _ = s.Seek(0, STREAM_SEEK_SET, None);
                done = i.Initialize(s, STGM_READ.0).is_ok();
            }
        }
        if !done {
            if let (Some(it), Ok(i)) = (&st.item, h.cast::<IInitializeWithItem>()) {
                done = i.Initialize(it, STGM_READ.0).is_ok();
            }
        }
        if !done {
            if let (Some(p), Ok(i)) = (&st.path, h.cast::<IInitializeWithFile>()) {
                done = i.Initialize(&HSTRING::from(p.as_str()), STGM_READ.0).is_ok();
            }
        }
        if !done {
            if let (Some(p), Ok(i)) = (&st.path, h.cast::<IInitializeWithStream>()) {
                if let Ok(s) = SHCreateStreamOnFileEx(&HSTRING::from(p.as_str()), STGM_READ.0 | STGM_SHARE_DENY_NONE.0, 0, false, None) {
                    done = i.Initialize(&s, STGM_READ.0).is_ok();
                }
            }
        }
        if !done {
            return Err(E_FAIL.into());
        }
        let mut b = HBITMAP::default();
        let mut a = WTSAT_UNKNOWN;
        h.GetThumbnail(cx, &mut b, &mut a)?;
        if b.is_invalid() {
            return Err(E_FAIL.into());
        }
        Ok((b, a))
    }
}

// ------------------------------------------------------------ свои ---
/// IStream как std::io::Read + Seek.
struct StreamReader(IStream);

impl std::io::Read for StreamReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut n = 0u32;
        let hr = unsafe { self.0.Read(buf.as_mut_ptr() as *mut c_void, buf.len() as u32, Some(&mut n)) };
        if hr.is_err() {
            return Err(std::io::Error::other("IStream::Read"));
        }
        Ok(n as usize)
    }
}

impl std::io::Seek for StreamReader {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        let (off, how) = match pos {
            std::io::SeekFrom::Start(o) => (o as i64, STREAM_SEEK_SET),
            std::io::SeekFrom::Current(o) => (o, STREAM_SEEK_CUR),
            std::io::SeekFrom::End(o) => (o, STREAM_SEEK_END),
        };
        let mut np = 0u64;
        unsafe { self.0.Seek(off, how, Some(&mut np)) }.map_err(|_| std::io::Error::other("IStream::Seek"))?;
        Ok(np)
    }
}

fn read_all(st: &St) -> Result<Vec<u8>> {
    const MAX: usize = 512 << 20;
    if let Some(s) = &st.stream {
        unsafe {
            let _ = s.Seek(0, STREAM_SEEK_SET, None);
            let mut out = Vec::new();
            let mut buf = vec![0u8; 1 << 20];
            loop {
                let mut n = 0u32;
                let hr = s.Read(buf.as_mut_ptr() as *mut c_void, buf.len() as u32, Some(&mut n));
                if n > 0 {
                    out.extend_from_slice(&buf[..n as usize]);
                }
                if hr.is_err() || n == 0 || out.len() > MAX {
                    break;
                }
            }
            return Ok(out);
        }
    }
    if let Some(p) = &st.path {
        return std::fs::read(p).map_err(|_| E_FAIL.into());
    }
    Err(E_FAIL.into())
}

/// Уменьшить до cx по длинной стороне (WIC, качественно).
fn fit(img: Img, cx: u32) -> Img {
    if img.w.max(img.h) <= cx {
        return img;
    }
    unsafe {
        let r = (|| -> Result<Img> {
            let f = codec::factory()?;
            let bmp = f.CreateBitmapFromMemory(img.w, img.h, &GUID_WICPixelFormat32bppBGRA, img.w * 4, &img.px)?;
            let k = cx as f64 / img.w.max(img.h) as f64;
            let (w, h) = (((img.w as f64 * k).round() as u32).max(1), ((img.h as f64 * k).round() as u32).max(1));
            let sc = f.CreateBitmapScaler()?;
            sc.Initialize(&bmp, w, h, WICBitmapInterpolationModeFant)?;
            let mut px = vec![0u8; w as usize * h as usize * 4];
            sc.CopyPixels(std::ptr::null(), w * 4, &mut px)?;
            Ok(Img { w, h, px })
        })();
        r.unwrap_or(img)
    }
}

fn own(bytes: &[u8], ext: &str, cx: u32) -> Option<Img> {
    if bytes.starts_with(b"%PDF") || ext == "pdf" {
        return pdf_first_page(bytes, cx);
    }
    // RAW: встроенный JPEG крупнее, чем то, что отдаёт системный TIFF-декодер
    if extcodec::is_raw_ext(ext) {
        if let Some(Ok(i)) = extcodec::decode(bytes, ext) {
            return Some(fit(i, cx));
        }
    }
    // HEIC: встроенной миниатюры хватает для значков Проводника (до ~256)
    if matches!(extcodec::sniff(bytes, ext), Some(extcodec::Sniff::Heic)) {
        if let Some(i) = extcodec::heic_thumb(bytes, cx) {
            return Some(fit(i, cx));
        }
    }
    if let Ok(i) = codec::wic(bytes, cx) {
        return Some(i);
    }
    match extcodec::decode(bytes, ext) {
        Some(Ok(i)) => Some(fit(i, cx)),
        _ => None,
    }
}

/// Первая страница PDF (Windows.Data.Pdf, в отдельном потоке MTA — асинхронные вызовы WinRT).
fn pdf_first_page(bytes: &[u8], cx: u32) -> Option<Img> {
    let data = bytes.to_vec();
    std::thread::spawn(move || -> Option<Img> {
        use windows::Data::Pdf::*;
        use windows::Storage::Streams::*;
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let r = (|| -> Result<Vec<u8>> {
            let mem = InMemoryRandomAccessStream::new()?;
            let w = DataWriter::CreateDataWriter(&mem)?;
            w.WriteBytes(&data)?;
            w.StoreAsync()?.get()?;
            w.FlushAsync()?.get()?;
            w.DetachStream()?;
            mem.Seek(0)?;
            let doc = PdfDocument::LoadFromStreamAsync(&mem)?.get()?;
            let page = doc.GetPage(0)?;
            let sz = page.Size()?;
            let out = InMemoryRandomAccessStream::new()?;
            let opts = PdfPageRenderOptions::new()?;
            if sz.Width >= sz.Height {
                opts.SetDestinationWidth(cx.max(16))?;
            } else {
                opts.SetDestinationHeight(cx.max(16))?;
            }
            opts.SetBitmapEncoderId(GUID_ContainerFormatBmp)?;
            page.RenderWithOptionsToStreamAsync(&out, &opts)?.get()?;
            let n = out.Size()? as u32;
            let rd = DataReader::CreateDataReader(&out.GetInputStreamAt(0)?)?;
            rd.LoadAsync(n)?.get()?;
            let mut buf = vec![0u8; n as usize];
            rd.ReadBytes(&mut buf)?;
            Ok(buf)
        })();
        r.ok().and_then(|b| codec::wic(&b, cx).ok())
    })
    .join()
    .ok()
    .flatten()
}

fn to_hbitmap(img: &Img) -> Result<HBITMAP> {
    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: img.w as i32,
                biHeight: -(img.h as i32),
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let b = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
        if bits.is_null() {
            return Err(E_OUTOFMEMORY.into());
        }
        std::ptr::copy_nonoverlapping(img.px.as_ptr(), bits as *mut u8, img.px.len());
        Ok(b)
    }
}
