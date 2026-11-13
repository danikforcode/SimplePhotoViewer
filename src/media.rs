// SimplePhotoViewer (Rust/Win32) — © 2026 danikforcode
// Лицензия MIT (см. LICENSE). Распространяется «как есть», без гарантий.
//! Аудио и видео: системный Media Engine (кодеки Windows). Кадры видео
//! передаются через Direct3D 11 и рисуются на общем холсте; для аудио —
//! волна звука. Плюс миниатюры (первый кадр, волна) для «хвостиков».

use crate::gui::{self, Mouse, Ui};
use crate::image_buf::Img;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D10::ID3D10Multithread;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Сообщение главному окну о событии Media Engine.
pub const WM_MEDIA: u32 = 0x8000 + 21; // WM_APP + 21

#[windows::core::implement(IMFMediaEngineNotify)]
struct Notify(isize);

impl IMFMediaEngineNotify_Impl for Notify_Impl {
    fn EventNotify(&self, event: u32, param1: usize, _param2: u32) -> Result<()> {
        unsafe {
            let _ = PostMessageW(HWND(self.0 as *mut _), WM_MEDIA, WPARAM(event as usize), LPARAM(param1 as isize));
        }
        Ok(())
    }
}

pub fn startup() {
    thread_local! { static OK: () = unsafe { let _ = MFStartup(MF_VERSION, MFSTARTUP_FULL); }; }
    OK.with(|_| {});
}

struct D3d {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    _mgr: IMFDXGIDeviceManager,
}

fn create_d3d() -> Option<(D3d, IMFDXGIDeviceManager)> {
    unsafe {
        let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_9_3];
        let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT;
        let mut dev = None;
        let mut ctx = None;
        let mut ok = false;
        for (driver, fl) in [(D3D_DRIVER_TYPE_HARDWARE, flags), (D3D_DRIVER_TYPE_HARDWARE, D3D11_CREATE_DEVICE_BGRA_SUPPORT), (D3D_DRIVER_TYPE_WARP, D3D11_CREATE_DEVICE_BGRA_SUPPORT)] {
            if D3D11CreateDevice(None, driver, HMODULE::default(), fl, Some(&levels), D3D11_SDK_VERSION, Some(&mut dev), None, Some(&mut ctx)).is_ok() {
                ok = true;
                break;
            }
        }
        if !ok {
            return None;
        }
        let (device, ctx): (ID3D11Device, ID3D11DeviceContext) = (dev?, ctx?);
        if let Ok(mt) = device.cast::<ID3D10Multithread>() {
            let _ = mt.SetMultithreadProtected(true);
        }
        let mut token = 0u32;
        let mut mgr = None;
        MFCreateDXGIDeviceManager(&mut token, &mut mgr).ok()?;
        let mgr = mgr?;
        mgr.ResetDevice(&device, token).ok()?;
        Some((D3d { device, ctx, _mgr: mgr.clone() }, mgr))
    }
}

// ------------------------------------------------- вывод видео (GPU) ---
// Кадр идёт из Media Engine прямо в цепочку буферов DXGI окна-«экрана»:
// без копии в память и без растягивания процессором. Отдельный поток
// ждёт обновления монитора (VBlank) и показывает новый кадр — ровно,
// без рывков таймера и без перерисовки всего окна программы.

struct Shared {
    stop: AtomicBool,
    /// Размер окна-экрана (точки); меняется из окна программы.
    size: Mutex<(u32, u32)>,
    /// Показать кадр заново (сдвинули/изменили размер/пауза).
    redraw: AtomicBool,
}

struct Agile<T>(T);
unsafe impl<T> Send for Agile<T> {}

struct VideoOut {
    hwnd: HWND,
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
    rect: RECT,
}

unsafe extern "system" fn video_wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        // мышь «проходит насквозь» — её обрабатывает окно программы
        WM_NCHITTEST => LRESULT(-1), // HTTRANSPARENT
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let _ = BeginPaint(h, &mut ps);
            let _ = EndPaint(h, &ps);
            let p = GetWindowLongPtrW(h, GWLP_USERDATA) as *const Shared;
            if !p.is_null() {
                (*p).redraw.store(true, Ordering::Relaxed);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

fn video_class() -> PCWSTR {
    static ONCE: std::sync::Once = std::sync::Once::new();
    let name = w!("SPV_Video");
    ONCE.call_once(|| unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(video_wndproc),
            hInstance: windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default().into(),
            lpszClassName: name,
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            ..Default::default()
        };
        RegisterClassW(&wc);
    });
    name
}

fn create_swap(d: &D3d, hwnd: HWND) -> Option<IDXGISwapChain1> {
    unsafe {
        let dxgi: IDXGIDevice = d.device.cast().ok()?;
        let adapter = dxgi.GetAdapter().ok()?;
        let factory: IDXGIFactory2 = adapter.GetParent().ok()?;
        for (effect, count) in [(DXGI_SWAP_EFFECT_FLIP_DISCARD, 2), (DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, 2), (DXGI_SWAP_EFFECT_DISCARD, 1)] {
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: 64,
                Height: 64,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: count,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: effect,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                ..Default::default()
            };
            if let Ok(sc) = factory.CreateSwapChainForHwnd(&d.device, hwnd, &desc, None, None) {
                let _ = factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);
                return Some(sc);
            }
        }
        None
    }
}

/// Поток показа кадров.
fn render_loop(engine: IMFMediaEngine, swap: IDXGISwapChain1, sh: Arc<Shared>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let output = swap.GetContainingOutput().ok();
        let mut last = i64::MIN;
        let mut cur = (64u32, 64u32);
        let black = MFARGB { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbAlpha: 255 };
        while !sh.stop.load(Ordering::Relaxed) {
            let want = *sh.size.lock().unwrap();
            let mut force = sh.redraw.swap(false, Ordering::Relaxed);
            if want != cur && want.0 > 0 && want.1 > 0 {
                if swap.ResizeBuffers(0, want.0, want.1, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0)).is_ok() {
                    cur = want;
                    force = true;
                }
            }
            let fresh = matches!(engine.OnVideoStreamTick(), Ok(p) if p != last && { last = p; true });
            if fresh || force {
                let (mut vw, mut vh) = (0u32, 0u32);
                let _ = engine.GetNativeVideoSize(Some(&mut vw), Some(&mut vh));
                if vw > 0 && vh > 0 {
                    if let Ok(bb) = swap.GetBuffer::<ID3D11Texture2D>(0) {
                        // вписать с сохранением пропорций (поля — чёрные)
                        let (cw, ch) = (cur.0 as f64, cur.1 as f64);
                        let k = (cw / vw as f64).min(ch / vh as f64);
                        let (w, h) = ((vw as f64 * k).round(), (vh as f64 * k).round());
                        let dst = RECT { left: ((cw - w) / 2.0) as i32, top: ((ch - h) / 2.0) as i32, right: ((cw + w) / 2.0) as i32, bottom: ((ch + h) / 2.0) as i32 };
                        if engine.TransferVideoFrame(&bb, None, &dst, Some(&black)).is_ok() {
                            let _ = swap.Present(0, DXGI_PRESENT(0));
                        }
                    }
                }
            }
            // ждать следующего обновления экрана — ровный темп без лишней нагрузки
            match &output {
                Some(o) if o.WaitForVBlank().is_ok() => {}
                _ => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        }
        CoUninitialize();
    }
}

pub struct MediaView {
    pub path: PathBuf,
    engine: Option<IMFMediaEngine>,
    d3d: Option<D3d>,
    out: Option<VideoOut>,
    /// Размер кадра видео (0 — ещё неизвестен).
    pub native: (u32, u32),
    pub is_video: bool,
    pub ready: bool,
    pub error: Option<String>,
    pub duration: f64,
    pub pos: f64,
    pub playing: bool,
    pub volume: f64,
    pub muted: bool,
    pub looped: bool,
    pub rate: f64,
    wave: Option<Vec<u8>>,
    wave_rx: Option<Receiver<Vec<u8>>>,
    pub meta: String,
    seek_rect: RECT,
    seeking: bool,
    hover_x: Option<i32>,
    bar_px: i32,
    /// Свой проигрыватель HEVC (когда в системе нет кодека).
    soft: Option<crate::softvideo::SoftPlayer>,
}

impl Drop for MediaView {
    fn drop(&mut self) {
        if let Some(mut o) = self.out.take() {
            o.shared.stop.store(true, Ordering::Relaxed);
            if let Some(t) = o.thread.take() {
                let _ = t.join();
            }
            unsafe {
                SetWindowLongPtrW(o.hwnd, GWLP_USERDATA, 0);
                let _ = DestroyWindow(o.hwnd);
            }
        }
        if let Some(e) = self.engine.take() {
            unsafe {
                let _ = e.Shutdown();
            }
        }
    }
}

pub fn fmt_time(s: f64) -> String {
    if !s.is_finite() || s < 0.0 {
        return "--:--".into();
    }
    let t = s.round() as u64;
    if t >= 3600 { format!("{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60) } else { format!("{}:{:02}", t / 60, t % 60) }
}

impl MediaView {
    pub fn open(hwnd: HWND, path: &Path, is_video: bool) -> MediaView {
        startup();
        let mut v = MediaView {
            path: path.to_path_buf(), engine: None, d3d: None, out: None, native: (0, 0), is_video, ready: false, error: None,
            duration: 0.0, pos: 0.0, playing: false, volume: crate::settings::get_i("volume", 80) as f64 / 100.0,
            muted: false, looped: false, rate: 1.0, wave: None, wave_rx: None, meta: String::new(),
            seek_rect: RECT::default(), seeking: false, hover_x: None, bar_px: 0, soft: None,
        };
        v.meta = describe(path);
        // HEVC без системного кодека — свой проигрыватель
        if is_video && crate::softvideo::needed(path) {
            match crate::softvideo::SoftPlayer::open(hwnd, path, v.volume) {
                Ok(p) => {
                    v.duration = p.duration;
                    v.native = p.native;
                    v.ready = true;
                    v.playing = true;
                    v.meta = format!("{}  ·  HEVC — встроенный декодер", v.meta);
                    v.soft = Some(p);
                    return v;
                }
                Err(e) => v.error = Some(format!("HEVC: {e}")),
            }
        }
        if let Err(e) = v.create(hwnd) {
            v.error = Some(e);
        }
        if !is_video {
            let (tx, rx) = channel();
            let p = path.to_path_buf();
            std::thread::spawn(move || {
                if let Some(w) = audio_peaks(&p, 1200) {
                    let _ = tx.send(w);
                }
            });
            v.wave_rx = Some(rx);
        }
        v
    }

    fn create(&mut self, hwnd: HWND) -> std::result::Result<(), String> {
        unsafe {
            let factory: IMFMediaEngineClassFactory =
                CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER).map_err(|e| format!("Media Engine недоступен: {}", e.message()))?;
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 3).map_err(|e| e.message().to_string())?;
            let attrs = attrs.unwrap();
            let notify: IMFMediaEngineNotify = Notify(hwnd.0 as isize).into();
            attrs.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify).map_err(|e| e.message().to_string())?;
            attrs.SetUINT32(&MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32).map_err(|e| e.message().to_string())?;
            if self.is_video {
                if let Some((d3d, mgr)) = create_d3d() {
                    let _ = attrs.SetUnknown(&MF_MEDIA_ENGINE_DXGI_MANAGER, &mgr);
                    self.d3d = Some(d3d);
                }
            }
            let engine = factory.CreateInstance(0, &attrs).map_err(|e| e.message().to_string())?;
            let _ = engine.SetVolume(self.volume);
            let url = BSTR::from(self.path.to_string_lossy().as_ref());
            engine.SetSource(&url).map_err(|e| e.message().to_string())?;
            let _ = engine.SetAutoPlay(true);
            // окно-«экран» и поток показа
            if let Some(d) = &self.d3d {
                let child = CreateWindowExW(
                    WINDOW_EX_STYLE(0), video_class(), w!(""), WS_CHILD | WS_CLIPSIBLINGS, 0, 0, 64, 64, hwnd, None,
                    windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default(), None,
                );
                if let Ok(child) = child {
                    if let Some(swap) = create_swap(d, child) {
                        let shared = Arc::new(Shared { stop: AtomicBool::new(false), size: Mutex::new((64, 64)), redraw: AtomicBool::new(true) });
                        SetWindowLongPtrW(child, GWLP_USERDATA, Arc::as_ptr(&shared) as isize);
                        let (e2, s2, sh2) = (Agile(engine.clone()), Agile(swap), shared.clone());
                        let thread = std::thread::spawn(move || {
                            let (e, s) = (e2, s2);
                            render_loop(e.0, s.0, sh2);
                        });
                        self.out = Some(VideoOut { hwnd: child, shared, thread: Some(thread), rect: RECT::default() });
                    } else {
                        let _ = DestroyWindow(child);
                    }
                }
            }
            self.engine = Some(engine);
        }
        Ok(())
    }

    /// Разместить окно-экран видео (rc — весь холст; снизу остаётся полоса перемотки).
    pub fn place(&mut self, ui: &Ui, rc: RECT) {
        self.bar_px = ui.s(44);
        if let Some(p) = &mut self.soft {
            let area = RECT { bottom: (rc.bottom - self.bar_px).max(rc.top + 1), ..rc };
            p.place(area, self.error.is_none());
            return;
        }
        let Some(o) = &mut self.out else { return };
        let area = RECT { bottom: (rc.bottom - self.bar_px).max(rc.top + 1), ..rc };
        let show = self.is_video && self.error.is_none();
        unsafe {
            if area != o.rect {
                o.rect = area;
                let (w, h) = ((area.right - area.left).max(1), (area.bottom - area.top).max(1));
                let _ = MoveWindow(o.hwnd, area.left, area.top, w, h, false);
                *o.shared.size.lock().unwrap() = (w as u32, h as u32);
                o.shared.redraw.store(true, Ordering::Relaxed);
            }
            let vis = IsWindowVisible(o.hwnd).as_bool();
            if vis != show {
                let _ = ShowWindow(o.hwnd, if show { SW_SHOWNA } else { SW_HIDE });
            }
        }
    }

    /// Событие от Media Engine (через WM_MEDIA).
    pub fn event(&mut self, ev: u32) {
        let Some(e) = &self.engine else { return };
        unsafe {
            match MF_MEDIA_ENGINE_EVENT(ev as i32) {
                MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA | MF_MEDIA_ENGINE_EVENT_DURATIONCHANGE => {
                    self.duration = e.GetDuration();
                    self.is_video = e.HasVideo().as_bool();
                    self.ready = true;
                }
                MF_MEDIA_ENGINE_EVENT_PLAYING | MF_MEDIA_ENGINE_EVENT_PLAY => self.playing = true,
                MF_MEDIA_ENGINE_EVENT_PAUSE | MF_MEDIA_ENGINE_EVENT_ENDED => self.playing = false,
                MF_MEDIA_ENGINE_EVENT_SEEKED => {
                    if let Some(o) = &self.out {
                        o.shared.redraw.store(true, Ordering::Relaxed);
                    }
                }
                MF_MEDIA_ENGINE_EVENT_ERROR => {
                    let code = e.GetError().map(|er| er.GetErrorCode()).unwrap_or(0);
                    self.error = Some(match code {
                        4 => "Формат или кодек не поддерживается этой Windows (может помочь расширение из Microsoft Store: HEVC, VP9, AV1, Web Media)".into(),
                        3 => "Файл повреждён или не читается".into(),
                        _ => format!("Не удалось воспроизвести (код {code})"),
                    });
                    self.playing = false;
                }
                _ => {}
            }
        }
    }

    /// Раз в ~40 мс: позиция, состояние, волна. Кадры показывает свой поток.
    /// true — перерисовать полосу перемотки (только если она заметно сдвинулась).
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.wave_rx {
            if let Ok(w) = rx.try_recv() {
                self.wave = Some(w);
                self.wave_rx = None;
                changed = true;
            }
        }
        if let Some(sp) = &mut self.soft {
            if sp.ended() && self.looped {
                sp.seek(0.0);
                sp.set_playing(true);
            }
            let p = sp.pos();
            let track = (self.seek_rect.right - self.seek_rect.left).max(100) as f64;
            let px = if self.duration > 0.0 { (p - self.pos).abs() / self.duration * track } else { 0.0 };
            if px >= 1.0 || p.floor() != self.pos.floor() {
                self.pos = p;
                changed = true;
            }
            if sp.playing() != self.playing {
                self.playing = sp.playing();
                changed = true;
            }
            if let Some(e) = sp.error() {
                if self.error.is_none() {
                    self.error = Some(e);
                    changed = true;
                }
            }
            return changed;
        }
        let Some(e) = self.engine.clone() else { return changed };
        unsafe {
            let p = e.GetCurrentTime();
            // сдвиг хотя бы на точку полосы или смена секунды на часах
            let track = (self.seek_rect.right - self.seek_rect.left).max(100) as f64;
            let px = if self.duration > 0.0 { (p - self.pos).abs() / self.duration * track } else { 0.0 };
            if px >= 1.0 || p.floor() != self.pos.floor() {
                self.pos = p;
                changed = true;
            }
            let playing = !e.IsPaused().as_bool() && !e.IsEnded().as_bool();
            if playing != self.playing {
                self.playing = playing;
                changed = true;
            }
            if self.is_video && self.native == (0, 0) {
                let (mut w, mut h) = (0u32, 0u32);
                if e.GetNativeVideoSize(Some(&mut w), Some(&mut h)).is_ok() && w > 0 {
                    self.native = (w, h);
                    changed = true;
                }
            }
        }
        changed
    }

    pub fn toggle_play(&mut self) {
        if let Some(p) = &mut self.soft {
            let on = !p.playing();
            p.set_playing(on);
            self.playing = on;
            return;
        }
        let Some(e) = &self.engine else { return };
        unsafe {
            if e.IsPaused().as_bool() || e.IsEnded().as_bool() {
                if e.IsEnded().as_bool() {
                    let _ = e.SetCurrentTime(0.0);
                }
                let _ = e.Play();
                self.playing = true;
            } else {
                let _ = e.Pause();
                self.playing = false;
            }
        }
    }

    pub fn pause(&mut self) {
        if let Some(p) = &mut self.soft {
            p.set_playing(false);
            self.playing = false;
        }
        if let Some(e) = &self.engine {
            unsafe {
                let _ = e.Pause();
            }
            self.playing = false;
        }
    }

    pub fn seek(&mut self, t: f64) {
        if let Some(p) = &mut self.soft {
            let t = t.clamp(0.0, self.duration.max(0.0));
            p.seek(t);
            self.pos = t;
            return;
        }
        if let Some(e) = &self.engine {
            let t = t.clamp(0.0, self.duration.max(0.0));
            unsafe {
                let _ = e.SetCurrentTime(t);
            }
            self.pos = t;
        }
    }

    pub fn step(&mut self, secs: f64) {
        self.seek(self.pos + secs);
    }

    pub fn set_volume(&mut self, v: f64) {
        self.volume = v.clamp(0.0, 1.0);
        crate::settings::set_i("volume", (self.volume * 100.0).round() as i32);
        if let Some(p) = &self.soft {
            p.set_volume(if self.muted { 0.0 } else { self.volume });
        }
        if let Some(e) = &self.engine {
            unsafe {
                let _ = e.SetVolume(self.volume);
            }
        }
    }

    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        if let Some(p) = &self.soft {
            p.set_volume(if self.muted { 0.0 } else { self.volume });
        }
        if let Some(e) = &self.engine {
            unsafe {
                let _ = e.SetMuted(self.muted);
            }
        }
    }

    pub fn toggle_loop(&mut self) {
        self.looped = !self.looped;
        if let Some(e) = &self.engine {
            unsafe {
                let _ = e.SetLoop(self.looped);
            }
        }
    }

    pub fn cycle_rate(&mut self) {
        const R: [f64; 6] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];
        if self.soft.is_some() {
            return; // свой проигрыватель — только обычная скорость
        }
        let i = R.iter().position(|r| (*r - self.rate).abs() < 0.01).unwrap_or(2);
        self.rate = R[(i + 1) % R.len()];
        if let Some(e) = &self.engine {
            unsafe {
                let _ = e.SetDefaultPlaybackRate(self.rate);
                let _ = e.SetPlaybackRate(self.rate);
            }
        }
    }

    pub fn info(&self) -> String {
        let mut v = vec![format!("{} / {}", fmt_time(self.pos), fmt_time(self.duration))];
        if self.native.0 > 0 {
            v.push(format!("{} × {}", self.native.0, self.native.1));
        }
        if (self.rate - 1.0).abs() > 0.01 {
            v.push(format!("×{}", self.rate));
        }
        if let Some(p) = &self.soft {
            let (dec, drop) = p.stats();
            if drop > 0 && dec > 0 {
                v.push(format!("пропущено кадров: {} из {}", drop, dec));
            }
            if p.fps > 0.0 {
                v.push(format!("{:.0} к/с", p.fps));
            }
            v.push(if p.gpu() { "HEVC · Direct3D".into() } else { "HEVC".into() });
        }
        v.join("   ·   ")
    }

    /// Рисование: кадр или волна + полоса перемотки внизу холста.
    pub fn paint(&mut self, ui: &Ui, hdc: HDC, rc: RECT, hq: bool) {
        let t = &ui.t;
        let bar_h = ui.s(44);
        let area = RECT { bottom: rc.bottom - bar_h, ..rc };
        if self.is_video {
            // сам кадр рисует окно-экран поверх (GPU); здесь — только фон
            gui::fill(hdc, &rc, crate::util::rgb(0, 0, 0));
            let _ = hq;
        } else {
            gui::fill(hdc, &rc, t.canvas);
            // название и сведения
            let name = crate::util::file_name(&self.path);
            let cy = (area.top + area.bottom) / 2;
            gui::text(hdc, &name, &gui::r(area.left + ui.s(20), cy - ui.s(120), area.right - area.left - ui.s(40), ui.s(30)), ui.f.big, t.text, gui::CENTER);
            gui::text(hdc, &self.meta, &gui::r(area.left + ui.s(20), cy - ui.s(88), area.right - area.left - ui.s(40), ui.s(24)), ui.f.text, t.dim, gui::CENTER);
            // волна
            let wr = RECT { left: area.left + ui.s(40), right: area.right - ui.s(40), top: cy - ui.s(50), bottom: cy + ui.s(70) };
            draw_wave(hdc, self.wave.as_deref(), &wr, if self.duration > 0.0 { self.pos / self.duration } else { 0.0 }, t.accent, gui::blend(t.dim, t.canvas, 0.5));
            if self.wave.is_none() && self.error.is_none() {
                gui::text(hdc, "…", &wr, ui.f.big, t.dim, gui::CENTER);
            }
        }
        if let Some(e) = &self.error {
            let er = gui::r(area.left + ui.s(30), (area.top + area.bottom) / 2 + ui.s(80), area.right - area.left - ui.s(60), ui.s(80));
            gui::text(hdc, e, &er, ui.f.text, if self.is_video { crate::util::rgb(230, 230, 230) } else { t.dim }, gui::WRAP);
        }
        // полоса перемотки
        let sb = RECT { left: rc.left + ui.s(16), right: rc.right - ui.s(16), top: rc.bottom - bar_h + ui.s(14), bottom: rc.bottom - bar_h + ui.s(30) };
        self.seek_rect = sb;
        let track_y = (sb.top + sb.bottom) / 2;
        let frac = if self.duration > 0.0 { (self.pos / self.duration).clamp(0.0, 1.0) } else { 0.0 };
        let x = sb.left + ((sb.right - sb.left) as f64 * frac) as i32;
        let (bg, fg) = if self.is_video { (crate::util::rgb(90, 90, 90), crate::util::rgb(240, 240, 240)) } else { (t.border, t.accent) };
        gui::round(hdc, &gui::r(sb.left, track_y - ui.s(2), sb.right - sb.left, ui.s(4)), ui.s(4), Some(bg), None);
        gui::round(hdc, &gui::r(sb.left, track_y - ui.s(2), x - sb.left, ui.s(4)), ui.s(4), Some(fg), None);
        let kr = ui.s(7);
        gui::round(hdc, &gui::r(x - kr, track_y - kr, 2 * kr, 2 * kr), 2 * kr, Some(fg), None);
        let lc = if self.is_video { crate::util::rgb(220, 220, 220) } else { t.dim };
        gui::text(hdc, &fmt_time(self.pos), &gui::r(sb.left, sb.bottom, ui.s(80), ui.s(16)), ui.f.small, lc, gui::LEFT);
        gui::text(hdc, &fmt_time(self.duration), &gui::r(sb.right - ui.s(80), sb.bottom, ui.s(80), ui.s(16)), ui.f.small, lc, gui::RIGHT);
        if let Some(hx) = self.hover_x {
            if self.duration > 0.0 {
                let f = ((hx - sb.left) as f64 / (sb.right - sb.left).max(1) as f64).clamp(0.0, 1.0);
                let s = fmt_time(f * self.duration);
                let tw = ui.tw(ui.f.small, &s) + ui.s(12);
                let b = gui::r((hx - tw / 2).clamp(sb.left + ui.s(60), sb.right - tw - ui.s(60)), sb.bottom - ui.s(1), tw, ui.s(18));
                gui::round(hdc, &b, ui.s(6), Some(t.tip_bg), None);
                gui::text(hdc, &s, &b, ui.f.small, t.tip_text, gui::CENTER);
            }
        }
    }

    pub fn mouse(&mut self, m: Mouse, x: i32, y: i32) -> bool {
        let sb = self.seek_rect;
        let near = x >= sb.left - 8 && x <= sb.right + 8 && y >= sb.top - 10 && y <= sb.bottom + 10;
        let to_t = |x: i32| ((x - sb.left) as f64 / (sb.right - sb.left).max(1) as f64).clamp(0.0, 1.0) * self.duration;
        match m {
            Mouse::Down if near => {
                self.seeking = true;
                self.seek(to_t(x));
                true
            }
            Mouse::Move => {
                let old = self.hover_x;
                self.hover_x = if near || self.seeking { Some(x.clamp(sb.left, sb.right)) } else { None };
                if self.seeking {
                    self.seek(to_t(x));
                    return true;
                }
                old != self.hover_x
            }
            Mouse::Up => {
                let was = self.seeking;
                self.seeking = false;
                was
            }
            Mouse::Wheel(d) => {
                // колесо — громкость
                self.set_volume(self.volume + if d > 0 { 0.05 } else { -0.05 });
                true
            }
            Mouse::Dbl if !near => {
                self.toggle_play();
                true
            }
            _ => false,
        }
    }

    /// Текущий кадр (для «Кадр → фото»): по запросу, в полном размере.
    pub fn snapshot(&self) -> Option<Img> {
        if let Some(p) = &self.soft {
            return p.snapshot();
        }
        let (d, e) = (self.d3d.as_ref()?, self.engine.as_ref()?);
        let (w, h) = self.native;
        if w == 0 || h == 0 {
            return None;
        }
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC {
                Width: w, Height: h, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32, CPUAccessFlags: 0, MiscFlags: 0,
            };
            let mut target = None;
            d.device.CreateTexture2D(&desc, None, Some(&mut target)).ok()?;
            let target = target?;
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            d.device.CreateTexture2D(&desc, None, Some(&mut staging)).ok()?;
            let staging = staging?;
            let dst = RECT { left: 0, top: 0, right: w as i32, bottom: h as i32 };
            e.TransferVideoFrame(&target, None, &dst, None).ok()?;
            d.ctx.CopyResource(&staging, &target);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            d.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m)).ok()?;
            let mut px = vec![0u8; (w * h * 4) as usize];
            for y in 0..h as usize {
                let src = std::slice::from_raw_parts((m.pData as *const u8).add(y * m.RowPitch as usize), w as usize * 4);
                px[y * w as usize * 4..(y + 1) * w as usize * 4].copy_from_slice(src);
            }
            d.ctx.Unmap(&staging, 0);
            for p in px.chunks_exact_mut(4) {
                p[3] = 255;
            }
            Some(Img { w, h, px })
        }
    }
}

/// Волна: столбики от центра, сыгранная часть — акцентом.
pub fn draw_wave(hdc: HDC, wave: Option<&[u8]>, r: &RECT, played: f64, accent: COLORREF, rest: COLORREF) {
    let Some(w) = wave else { return };
    if w.is_empty() {
        return;
    }
    let width = (r.right - r.left).max(1);
    let mid = (r.top + r.bottom) / 2;
    let half = (r.bottom - r.top) / 2;
    let step = 3;
    let cut = r.left + (width as f64 * played) as i32;
    let mut x = r.left;
    while x < r.right {
        let i = ((x - r.left) as usize * w.len() / width as usize).min(w.len() - 1);
        let h = (w[i] as i32 * half / 255).max(1);
        let c = if x < cut { accent } else { rest };
        gui::fill(hdc, &RECT { left: x, top: mid - h, right: x + 2, bottom: mid + h }, c);
        x += step;
    }
}

/// Короткое описание: формат, длительность, разрешение (через Source Reader).
fn describe(path: &Path) -> String {
    unsafe {
        startup();
        let url = crate::util::wpath(path);
        let Ok(r) = MFCreateSourceReaderFromURL(PCWSTR(url.as_ptr()), None) else { return String::new() };
        let mut parts = vec![];
        let dur = crate::convert::duration(&r);
        if dur > 0 {
            parts.push(fmt_time(dur as f64 / 1e7));
        }
        if let Ok(t) = r.GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, 0) {
            if let Ok(fs) = t.GetUINT64(&MF_MT_FRAME_SIZE) {
                parts.push(format!("{} × {}", fs >> 32, fs & 0xFFFF_FFFF));
            }
            if let Ok(s) = t.GetGUID(&MF_MT_SUBTYPE) {
                parts.push(codec_name(&s));
            }
        }
        if let Ok(t) = r.GetNativeMediaType(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32, 0) {
            if let Ok(s) = t.GetGUID(&MF_MT_SUBTYPE) {
                parts.push(codec_name(&s));
            }
            if let Ok(rate) = t.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND) {
                parts.push(format!("{:.1} кГц", rate as f64 / 1000.0));
            }
            if let Ok(ch) = t.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS) {
                parts.push(if ch == 1 { "моно".into() } else if ch == 2 { "стерео".into() } else { format!("{ch} кан.") });
            }
        }
        parts.retain(|p| !p.is_empty());
        parts.join("  ·  ")
    }
}

fn codec_name(g: &GUID) -> String {
    let known: [(GUID, &str); 16] = [
        (MFVideoFormat_H264, "H.264"), (MFVideoFormat_HEVC, "H.265"), (MFVideoFormat_VP90, "VP9"), (MFVideoFormat_AV1, "AV1"),
        (MFVideoFormat_MP4V, "MPEG-4"), (MFVideoFormat_MPEG2, "MPEG-2"), (MFVideoFormat_WMV3, "WMV"), (MFVideoFormat_MJPG, "MJPEG"),
        (MFAudioFormat_AAC, "AAC"), (MFAudioFormat_MP3, "MP3"), (MFAudioFormat_FLAC, "FLAC"), (MFAudioFormat_PCM, "PCM"),
        (MFAudioFormat_WMAudioV9, "WMA"), (MFAudioFormat_Opus, "Opus"), (MFAudioFormat_ALAC, "ALAC"), (MFAudioFormat_Dolby_AC3, "AC-3"),
    ];
    known.iter().find(|(k, _)| k == g).map(|(_, n)| n.to_string()).unwrap_or_default()
}

/// Волна звука: n значений 0–255 (пики по отрезкам). Для фонового потока.
pub fn audio_peaks(path: &Path, n: usize) -> Option<Vec<u8>> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_FULL);
        let res = (|| -> Option<Vec<u8>> {
            let url = crate::util::wpath(path);
            let r = MFCreateSourceReaderFromURL(PCWSTR(url.as_ptr()), None).ok()?;
            let first_audio = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
            r.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false).ok()?;
            r.SetStreamSelection(first_audio, true).ok()?;
            let t = MFCreateMediaType().ok()?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).ok()?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).ok()?;
            t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).ok()?;
            r.SetCurrentMediaType(first_audio, None, &t).ok()?;
            let cur = r.GetCurrentMediaType(first_audio).ok()?;
            let rate = cur.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).ok()? as f64;
            let ch = cur.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).ok()?.max(1) as usize;
            let dur = crate::convert::duration(&r) as f64 / 1e7;
            let total = (rate * dur).max(1.0);
            let mut peaks = vec![0u16; n];
            let mut frame = 0f64;
            loop {
                let mut flags = 0u32;
                let mut sample: Option<IMFSample> = None;
                if r.ReadSample(first_audio, 0, None, Some(&mut flags), None, Some(&mut sample)).is_err() {
                    break;
                }
                if let Some(s) = sample {
                    let buf = s.ConvertToContiguousBuffer().ok()?;
                    let mut p: *mut u8 = std::ptr::null_mut();
                    let mut len = 0u32;
                    buf.Lock(&mut p, None, Some(&mut len)).ok()?;
                    let data = std::slice::from_raw_parts(p as *const i16, len as usize / 2);
                    for fr in data.chunks_exact(ch) {
                        let i = ((frame / total) * n as f64) as usize;
                        if i < n {
                            let v = fr.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
                            peaks[i] = peaks[i].max(v);
                        }
                        frame += 1.0;
                    }
                    let _ = buf.Unlock();
                }
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
            }
            let max = (*peaks.iter().max()?).max(1) as u32;
            Some(peaks.iter().map(|&p| (p as u32 * 255 / max) as u8).collect())
        })();
        let _ = MFShutdown();
        CoUninitialize();
        res
    }
}

/// Кадр видео (около 10% длительности) для миниатюры. Для фонового потока.
pub fn video_thumb(path: &Path, max: u32) -> Option<Img> {
    // HEVC без системного кодека — свой декодер
    if crate::softvideo::needed(path) {
        let mut f = std::fs::File::open(path).ok()?;
        let moov = crate::mp4::read_moov(&mut f).ok()?;
        return crate::mp4::hevc_thumb(&mut f, &moov, max);
    }
    video_thumb_mf(path, max)
}

fn video_thumb_mf(path: &Path, max: u32) -> Option<Img> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_FULL);
        let res = (|| -> Option<Img> {
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 1).ok()?;
            let attrs = attrs?;
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1).ok()?;
            let url = crate::util::wpath(path);
            let r = MFCreateSourceReaderFromURL(PCWSTR(url.as_ptr()), &attrs).ok()?;
            let fv = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
            r.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false).ok()?;
            r.SetStreamSelection(fv, true).ok()?;
            let t = MFCreateMediaType().ok()?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).ok()?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).ok()?;
            r.SetCurrentMediaType(fv, None, &t).ok()?;
            let cur = r.GetCurrentMediaType(fv).ok()?;
            let fs = cur.GetUINT64(&MF_MT_FRAME_SIZE).ok()?;
            let (w, h) = ((fs >> 32) as u32, (fs & 0xFFFF_FFFF) as u32);
            let stride = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or(w as i32 * 4);
            let dur = crate::convert::duration(&r);
            if dur > 0 {
                let pos = PROPVARIANT::from((dur / 10) as i64);
                let _ = r.SetCurrentPosition(&GUID::zeroed(), &pos);
            }
            for _ in 0..30 {
                let mut flags = 0u32;
                let mut sample: Option<IMFSample> = None;
                r.ReadSample(fv, 0, None, Some(&mut flags), None, Some(&mut sample)).ok()?;
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
                let Some(s) = sample else { continue };
                let buf = s.ConvertToContiguousBuffer().ok()?;
                let mut p: *mut u8 = std::ptr::null_mut();
                let mut len = 0u32;
                buf.Lock(&mut p, None, Some(&mut len)).ok()?;
                let mut px = vec![0u8; (w * h * 4) as usize];
                let abs = stride.unsigned_abs() as usize;
                for y in 0..h as usize {
                    let sy = if stride < 0 { h as usize - 1 - y } else { y };
                    if (sy + 1) * abs <= len as usize {
                        let src = std::slice::from_raw_parts(p.add(sy * abs), w as usize * 4);
                        px[y * w as usize * 4..(y + 1) * w as usize * 4].copy_from_slice(src);
                    }
                }
                let _ = buf.Unlock();
                for q in px.chunks_exact_mut(4) {
                    q[3] = 255;
                }
                let img = Img { w, h, px };
                return Some(crate::codec::thumb(&img, max));
            }
            None
        })();
        let _ = MFShutdown();
        CoUninitialize();
        res
    }
}
