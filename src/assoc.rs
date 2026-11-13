// SimplePhotoViewer (Rust/Win32) — © 2026 danikforcode
// Лицензия MIT (см. LICENSE). Распространяется «как есть», без гарантий.
//! Связь с Проводником (только для текущего пользователя, без прав администратора):
//! свои типы файлов «SimplePhotoViewer.<расширение>», регистрация в «Приложениях
//! по умолчанию» и DLL миниатюр (thumbs/), чтобы при нашей программе по умолчанию
//! в Проводнике оставались миниатюры, как с «Фотографиями» Windows.
//! Значок типа и окно предпросмотра берутся у прежнего (системного) типа файла.
//! Выбрать программу по умолчанию Windows разрешает только самому пользователю —
//! поэтому в конце открываются «Параметры → Приложения по умолчанию».

use crate::gui::{self, Ev, Kind, Panel, Ui, W};
use crate::kinds;
use std::path::{Path, PathBuf};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::HDC;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use std::result::Result;

/// Класс миниатюр из DLL (тот же номер — в thumbs/src/lib.rs).
const CLSID: &str = "{6B1C4E2A-3F8D-4A57-9E21-5C7D0B8A4F13}";
const THUMB_IID: &str = "{e357fccd-a995-4576-b01f-234630154e96}";
const PREVIEW_IID: &str = "{8895b1c6-b41f-4c1c-a562-0d564250836f}";
const APP_KEY: &str = "Software\\danikforcode\\SimplePhotoViewer";
const REG_NAME: &str = "SimplePhotoViewer";

/// DLL миниатюр (сжатая deflate).
#[cfg(has_thumbs)]
const DLL: &[u8] = include_bytes!(env!("SPV_THUMBS_DLL"));
#[cfg(not(has_thumbs))]
const DLL: &[u8] = &[];

fn dll_bytes() -> Result<Vec<u8>, String> {
    let mut out = vec![];
    std::io::Read::read_to_end(&mut flate2::read::DeflateDecoder::new(DLL), &mut out).map_err(|e| format!("DLL миниатюр повреждена: {e}"))?;
    Ok(out)
}

/// Есть ли в этой сборке DLL миниатюр.
pub fn has_thumbs() -> bool {
    !DLL.is_empty()
}

/// Расширения, которые предлагаем Windows (тексты — только частые).
fn exts() -> Vec<(&'static str, &'static str)> {
    let mut v = vec![];
    v.extend(kinds::IMAGE.iter().map(|e| (*e, "Фото")));
    v.push(("pdf", "Документ PDF"));
    v.extend(kinds::DOC.iter().map(|e| (*e, "Документ")));
    v.extend(kinds::TABLE.iter().map(|e| (*e, "Таблица")));
    v.extend(["txt", "log", "json", "xml", "yaml", "yml", "ini", "csv"].iter().filter(|e| !kinds::TABLE.contains(e)).map(|e| (*e, "Текст")));
    v.extend(kinds::AUDIO.iter().map(|e| (*e, "Аудио")));
    v.extend(kinds::VIDEO.iter().map(|e| (*e, "Видео")));
    v
}

// ------------------------------------------------------------ реестр ---
fn key(path: &str) -> Option<HKEY> {
    unsafe {
        let mut k = HKEY::default();
        RegCreateKeyExW(HKEY_CURRENT_USER, &HSTRING::from(path), 0, None, REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut k, None).ok().ok()?;
        Some(k)
    }
}

fn set(path: &str, name: Option<&str>, val: &str) -> bool {
    let Some(k) = key(path) else { return false };
    unsafe {
        let w: Vec<u16> = val.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = std::slice::from_raw_parts(w.as_ptr() as *const u8, w.len() * 2);
        let n = name.map(HSTRING::from);
        let r = RegSetValueExW(k, n.as_ref().map(|h| PCWSTR(h.as_ptr())).unwrap_or(PCWSTR::null()), 0, REG_SZ, Some(bytes));
        let _ = RegCloseKey(k);
        r.is_ok()
    }
}

fn get(root: HKEY, path: &str, name: Option<&str>) -> Option<String> {
    unsafe {
        let p = HSTRING::from(path);
        let n = name.map(HSTRING::from);
        let mut buf = vec![0u16; 1024];
        let mut sz = (buf.len() * 2) as u32;
        RegGetValueW(root, &p, n.as_ref().map(|h| PCWSTR(h.as_ptr())).unwrap_or(PCWSTR::null()), RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut sz)).ok().ok()?;
        let s = String::from_utf16_lossy(&buf[..(sz as usize / 2).saturating_sub(1).min(buf.len())]);
        (!s.is_empty()).then_some(s)
    }
}

fn values(root: HKEY, path: &str) -> Vec<String> {
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

fn delete_tree(path: &str) {
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(path));
        let _ = RegDeleteKeyW(HKEY_CURRENT_USER, &HSTRING::from(path));
    }
}

fn delete_value(path: &str, name: &str) {
    unsafe {
        let mut k = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(path), 0, KEY_WRITE, &mut k).is_ok() {
            let _ = RegDeleteValueW(k, &HSTRING::from(name));
            let _ = RegCloseKey(k);
        }
    }
}

/// «Родной» тип файла (системный, из HKLM).
fn native_progid(ext: &str) -> Option<String> {
    get(HKEY_LOCAL_MACHINE, &format!("SOFTWARE\\Classes\\.{ext}"), None).filter(|p| !p.starts_with(REG_NAME))
}

/// Обработчик (миниатюр, предпросмотра…) для расширения — без учёта нашего.
fn system_handler(ext: &str, iid: &str) -> Option<String> {
    let mut places = vec![format!("SystemFileAssociations\\.{ext}"), format!(".{ext}")];
    if let Some(p) = native_progid(ext) {
        places.push(p);
    }
    places.extend(values(HKEY_LOCAL_MACHINE, &format!("SOFTWARE\\Classes\\.{ext}\\OpenWithProgids")).into_iter().filter(|p| !p.starts_with(REG_NAME)));
    if let Some(pt) = get(HKEY_LOCAL_MACHINE, &format!("SOFTWARE\\Classes\\.{ext}"), Some("PerceivedType")) {
        places.push(format!("SystemFileAssociations\\{pt}"));
    }
    for p in places {
        if let Some(v) = get(HKEY_LOCAL_MACHINE, &format!("SOFTWARE\\Classes\\{p}\\ShellEx\\{iid}"), None) {
            if !v.eq_ignore_ascii_case(CLSID) {
                return Some(v);
            }
        }
    }
    None
}

fn exe_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("simple_photo_viewer.exe"))
}

/// Папка для DLL: %LOCALAPPDATA%\SimplePhotoViewer\Explorer
fn dll_dir() -> Option<PathBuf> {
    let d = Path::new(&std::env::var_os("LOCALAPPDATA")?).join("SimplePhotoViewer").join("Explorer");
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

/// Метка сборки DLL: другая сборка → другой файл (старый мог быть занят Проводником).
fn dll_tag() -> String {
    let h = DLL.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3));
    format!("{}-{:08x}", env!("CARGO_PKG_VERSION"), h as u32)
}

fn write_dll() -> Result<PathBuf, String> {
    let dir = dll_dir().ok_or("нет папки %LOCALAPPDATA%")?;
    let p = dir.join(format!("spv_thumbs-{}.dll", dll_tag()));
    let bytes = dll_bytes()?;
    if std::fs::metadata(&p).map(|m| m.len() as usize != bytes.len()).unwrap_or(true) {
        std::fs::write(&p, &bytes).map_err(|e| format!("не удалось записать {}: {e}", p.display()))?;
    }
    // старые сборки — убрать (занятые Проводником останутся до следующего раза)
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let q = e.path();
            if q != p && q.extension().map(|x| x == "dll").unwrap_or(false) {
                let _ = std::fs::remove_file(&q);
            }
        }
    }
    Ok(p)
}

/// Проверка DLL без Проводника: --thumb файл… → файл.thumb.png (256 точек).
pub fn test_thumbs(files: &[PathBuf]) {
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::Com::*;
    use windows::Win32::System::LibraryLoader::*;
    use windows::Win32::UI::Shell::PropertiesSystem::IInitializeWithStream;
    let dll = match write_dll() {
        Ok(p) => p,
        Err(e) => return println!("DLL: {e}"),
    };
    unsafe {
        let Ok(m) = LoadLibraryW(&HSTRING::from(dll.as_os_str())) else { return println!("DLL не загрузилась: {}", dll.display()) };
        type GetCls = unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut core::ffi::c_void) -> HRESULT;
        let Some(f) = GetProcAddress(m, s!("DllGetClassObject")) else { return println!("нет DllGetClassObject") };
        let f: GetCls = std::mem::transmute(f);
        let clsid = GUID::from_u128(0x6b1c4e2a_3f8d_4a57_9e21_5c7d0b8a4f13);
        for file in files {
            let t = std::time::Instant::now();
            let r = (|| -> windows::core::Result<crate::image_buf::Img> {
                let mut cf: *mut core::ffi::c_void = std::ptr::null_mut();
                f(&clsid, &IClassFactory::IID, &mut cf).ok()?;
                let cf = IClassFactory::from_raw(cf);
                let tp: IThumbnailProvider = cf.CreateInstance(None)?;
                let st = SHCreateStreamOnFileEx(&HSTRING::from(file.as_os_str()), STGM_READ.0, 0, false, None)?;
                tp.cast::<IInitializeWithStream>()?.Initialize(&st, STGM_READ.0)?;
                let mut b = HBITMAP::default();
                let mut a = WTSAT_UNKNOWN;
                tp.GetThumbnail(256, &mut b, &mut a)?;
                let mut bm = BITMAP::default();
                GetObjectW(b, std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _));
                let (w, h) = (bm.bmWidth as u32, bm.bmHeight.unsigned_abs());
                let mut bi = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: 40, biWidth: w as i32, biHeight: -(h as i32), biPlanes: 1, biBitCount: 32, ..Default::default() }, ..Default::default() };
                let mut px = vec![0u8; (w * h * 4) as usize];
                let dc = GetDC(None);
                GetDIBits(dc, b, 0, h, Some(px.as_mut_ptr() as *mut _), &mut bi, DIB_RGB_COLORS);
                ReleaseDC(None, dc);
                let _ = DeleteObject(b);
                for p in px.chunks_mut(4) {
                    p[3] = 255;
                }
                Ok(crate::image_buf::Img { w, h, px })
            })();
            match r {
                Ok(img) => {
                    let out = file.with_extension(format!("{}.thumb.png", crate::util::ext_lower(file)));
                    let _ = crate::codec::save(&img, &out);
                    println!("{}: {}×{} за {:.0} мс → {}", file.display(), img.w, img.h, t.elapsed().as_secs_f64() * 1000.0, out.display());
                }
                Err(e) => println!("{}: ошибка {:?}", file.display(), e.code()),
            }
        }
    }
}

// --------------------------------------------------------- установка ---
/// Зарегистрировать типы файлов и миниатюры (без открытия «Параметров»).
pub fn install() -> Result<usize, String> {
    let exe = exe_path();
    let exe_s = exe.display().to_string();
    let exe_name = exe.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let open = format!("\"{exe_s}\" \"%1\"");
    let icon = format!("\"{exe_s}\",0");
    let classes = "Software\\Classes";

    // DLL миниатюр
    let thumbs = if has_thumbs() {
        let dll = write_dll()?;
        let ck = format!("{classes}\\CLSID\\{CLSID}");
        set(&ck, None, "SimplePhotoViewer — миниатюры");
        set(&format!("{ck}\\InprocServer32"), None, &dll.display().to_string());
        set(&format!("{ck}\\InprocServer32"), Some("ThreadingModel"), "Apartment");
        true
    } else {
        false
    };

    let caps = format!("{APP_KEY}\\Capabilities");
    set(&caps, Some("ApplicationName"), "SimplePhotoViewer");
    set(&caps, Some("ApplicationDescription"), "Лёгкий просмотрщик фото, PDF, документов, таблиц, текстов, аудио и видео");
    set(&caps, Some("ApplicationIcon"), &icon);
    set("Software\\RegisteredApplications", Some(REG_NAME), &caps);
    set(APP_KEY, Some("Build"), &dll_tag());
    set(APP_KEY, Some("Exe"), &exe_s);

    // «Открыть с помощью» → наш exe (и миниатюры, если выбран именно он)
    let app = format!("{classes}\\Applications\\{exe_name}");
    set(&app, Some("FriendlyAppName"), "SimplePhotoViewer");
    set(&format!("{app}\\DefaultIcon"), None, &icon);
    set(&format!("{app}\\shell\\open\\command"), None, &open);
    if thumbs {
        set(&format!("{app}\\ShellEx\\{THUMB_IID}"), None, CLSID);
    }

    let mut n = 0;
    for (ext, kind) in exts() {
        let pid = format!("{REG_NAME}.{ext}");
        let pk = format!("{classes}\\{pid}");
        set(&pk, None, &format!("{kind} {}", ext.to_uppercase()));
        // значок — как у прежнего типа файла (привычный вид), иначе — наш
        let native = native_progid(ext);
        let ico = native
            .as_ref()
            .and_then(|p| get(HKEY_LOCAL_MACHINE, &format!("SOFTWARE\\Classes\\{p}\\DefaultIcon"), None))
            .unwrap_or_else(|| icon.clone());
        set(&format!("{pk}\\DefaultIcon"), None, &ico);
        set(&format!("{pk}\\shell\\open\\command"), None, &open);
        if thumbs {
            set(&format!("{pk}\\ShellEx\\{THUMB_IID}"), None, CLSID);
        }
        if let Some(pv) = system_handler(ext, PREVIEW_IID) {
            set(&format!("{pk}\\ShellEx\\{PREVIEW_IID}"), None, &pv);
        }
        if kind == "Видео" {
            // «плёнка» по краям миниатюры, как у системных видео
            set_dword(&pk, "Treatment", 3);
        }
        set(&format!("{classes}\\.{ext}\\OpenWithProgids"), Some(&pid), "");
        set(&format!("{app}\\SupportedTypes"), Some(&format!(".{ext}")), "");
        set(&format!("{caps}\\FileAssociations"), Some(&format!(".{ext}")), &pid);
        n += 1;
    }
    notify();
    Ok(n)
}

fn set_dword(path: &str, name: &str, v: u32) {
    if let Some(k) = key(path) {
        unsafe {
            let _ = RegSetValueExW(k, &HSTRING::from(name), 0, REG_DWORD, Some(&v.to_le_bytes()));
            let _ = RegCloseKey(k);
        }
    }
}

/// Убрать всё, что записали.
pub fn uninstall() {
    let classes = "Software\\Classes";
    let exe_name = exe_path().file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    for (ext, _) in exts() {
        let pid = format!("{REG_NAME}.{ext}");
        delete_tree(&format!("{classes}\\{pid}"));
        delete_value(&format!("{classes}\\.{ext}\\OpenWithProgids"), &pid);
    }
    delete_tree(&format!("{classes}\\Applications\\{exe_name}"));
    delete_tree(&format!("{classes}\\CLSID\\{CLSID}"));
    delete_value("Software\\RegisteredApplications", REG_NAME);
    delete_tree(APP_KEY);
    notify();
    if let Some(d) = dll_dir() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

fn notify() {
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
}

/// Зарегистрирована ли программа.
pub fn installed() -> bool {
    get(HKEY_CURRENT_USER, APP_KEY, Some("Build")).is_some()
}

/// При запуске: если программу перенесли или обновили — обновить записи (тихо, в фоне).
pub fn refresh_if_needed() {
    if !installed() {
        return;
    }
    let same_exe = get(HKEY_CURRENT_USER, APP_KEY, Some("Exe")).map(|e| Path::new(&e) == exe_path()).unwrap_or(false);
    let same_build = get(HKEY_CURRENT_USER, APP_KEY, Some("Build")).map(|b| b == dll_tag()).unwrap_or(false);
    if !same_exe || !same_build {
        std::thread::spawn(|| {
            let _ = install();
        });
    }
}

/// Открыть «Параметры → Приложения по умолчанию» на нашей программе.
pub fn open_settings() {
    unsafe {
        let r = ShellExecuteW(None, w!("open"), &HSTRING::from(format!("ms-settings:defaultapps?registeredAppUser={REG_NAME}")), None, None, SW_SHOWNORMAL);
        if r.0 as isize <= 32 {
            let _ = ShellExecuteW(None, w!("open"), w!("ms-settings:defaultapps"), None, None, SW_SHOWNORMAL);
        }
    }
}

// ------------------------------------------------------------ окно ---
const ID_DEFAULT: u32 = 1;
const ID_THUMBS: u32 = 2;
const ID_REMOVE: u32 = 3;
const ID_CLOSE: u32 = 4;

struct Dlg {
    status: String,
}

const ABOUT: &str = "SimplePhotoViewer может открывать фото, PDF, документы, таблицы, тексты, аудио и видео по двойному щелчку. \
Миниатюры в Проводнике сохранятся: для каждого типа используется системная миниатюра, а где её нет (HEIC без кодека HEVC, AVIF, RAW, WebP) — своя.\n\n\
Всё записывается только для вас (без прав администратора). Выбрать программу по умолчанию Windows разрешает только вручную — откроются «Параметры», где нужно нажать «Установить по умолчанию».";

impl Panel for Dlg {
    fn title(&self) -> String {
        "Программа по умолчанию".into()
    }
    fn resizable(&self) -> bool {
        false
    }
    fn min_size(&self) -> (i32, i32) {
        (560, 340)
    }
    fn layout(&mut self, ui: &Ui, w: i32, h: i32) -> Vec<W> {
        let m = ui.s(24);
        let by = h - m - ui.s(36);
        let mut x = w - m;
        let mut v = vec![];
        let on = installed();
        for (id, t, bw, primary, en) in [
            (ID_CLOSE, "Закрыть", 100, false, true),
            (ID_REMOVE, "Убрать из системы", 170, false, on),
            (ID_THUMBS, if on { "Обновить" } else { "Только миниатюры" }, 160, false, true),
            (ID_DEFAULT, "Сделать по умолчанию…", 200, true, true),
        ] {
            let bw = ui.s(bw);
            x -= bw;
            v.push(W::new(Kind::Button, id, gui::r(x, by, bw, ui.s(36))).t(t).on(primary).en(en));
            x -= ui.s(8);
        }
        v
    }
    fn paint(&mut self, ui: &Ui, hdc: HDC, w: i32, h: i32) {
        let t = &ui.t;
        gui::fill(hdc, &gui::r(0, 0, w, h), t.bg);
        let m = ui.s(24);
        let cw = w - 2 * m;
        gui::text(hdc, "Связь с Проводником", &gui::r(m, m, cw, ui.s(30)), ui.f.big, t.text, gui::LEFT);
        let th = ui.th(ui.f.body, ABOUT, cw);
        gui::text(hdc, ABOUT, &gui::r(m, m + ui.s(40), cw, th + ui.s(4)), ui.f.body, t.text, gui::PARA);
        let state = if installed() { "Сейчас: подключено." } else { "Сейчас: не подключено." };
        let extra = if has_thumbs() { "" } else { " В этой сборке нет DLL миниатюр." };
        let s = if self.status.is_empty() { format!("{state}{extra}") } else { self.status.clone() };
        gui::text(hdc, &s, &gui::r(m, m + ui.s(48) + th, cw, ui.s(40)), ui.f.text, t.dim, gui::PARA);
    }
    fn event(&mut self, _hwnd: HWND, ev: Ev) -> bool {
        match ev {
            Ev::Click(ID_CLOSE) => false,
            Ev::Click(ID_DEFAULT) | Ev::Click(ID_THUMBS) => {
                let id = if let Ev::Click(i) = ev { i } else { 0 };
                match install() {
                    Ok(n) => {
                        self.status = format!("Готово: {n} типов файлов. Если миниатюры появились не сразу — перезапустите Проводник или откройте папку заново.");
                        if id == ID_DEFAULT {
                            open_settings();
                        }
                    }
                    Err(e) => self.status = format!("Не получилось: {e}"),
                }
                true
            }
            Ev::Click(ID_REMOVE) => {
                uninstall();
                self.status = "Записи убраны. Типы файлов вернутся к прежним программам.".into();
                true
            }
            _ => true,
        }
    }
    fn key(&mut self, _hwnd: HWND, vk: u16, _c: bool, _s: bool) -> Option<bool> {
        (vk == 0x1B).then_some(false)
    }
}

pub fn dialog(owner: HWND) {
    gui::run_modal(owner, Dlg { status: String::new() }, (720, 400));
}
