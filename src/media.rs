// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Аудио и видео: системный Media Engine (кодеки Windows). Кадры видео
//! передаются через Direct3D 11 и рисуются на общем холсте; для аудио —
//! волна звука. Плюс миниатюры (первый кадр, волна) для «хвостиков».

use crate::gui::{self, Mouse, Ui};
use crate::image_buf::Img;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D10::ID3D10Multithread;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

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
    tex: Option<(ID3D11Texture2D, ID3D11Texture2D, u32, u32)>,
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
        Some((D3d { device, ctx, _mgr: mgr.clone(), tex: None }, mgr))
    }
}

pub struct MediaView {
    pub path: PathBuf,
    engine: Option<IMFMediaEngine>,
    d3d: Option<D3d>,
    pub frame: Option<Rc<Img>>,
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
    last_pts: i64,
    wave: Option<Vec<u8>>,
    wave_rx: Option<Receiver<Vec<u8>>>,
    pub meta: String,
    seek_rect: RECT,
    seeking: bool,
    hover_x: Option<i32>,
}

impl Drop for MediaView {
    fn drop(&mut self) {
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
            path: path.to_path_buf(), engine: None, d3d: None, frame: None, is_video, ready: false, error: None,
            duration: 0.0, pos: 0.0, playing: false, volume: crate::settings::get_i("volume", 80) as f64 / 100.0,
            muted: false, looped: false, rate: 1.0, last_pts: -1, wave: None, wave_rx: None, meta: String::new(),
            seek_rect: RECT::default(), seeking: false, hover_x: None,
        };
        v.meta = describe(path);
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
            self.engine = Some(engine);
        }
        Ok(())
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

    /// Раз в кадр: новый кадр видео, позиция, волна. true — перерисовать.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.wave_rx {
            if let Ok(w) = rx.try_recv() {
                self.wave = Some(w);
                self.wave_rx = None;
                changed = true;
            }
        }
        let Some(e) = self.engine.clone() else { return changed };
        unsafe {
            let p = e.GetCurrentTime();
            if (p - self.pos).abs() > 0.03 {
                self.pos = p;
                changed = true;
            }
            let playing = !e.IsPaused().as_bool() && !e.IsEnded().as_bool();
            if playing != self.playing {
                self.playing = playing;
                changed = true;
            }
            if self.is_video {
                if let Ok(pts) = e.OnVideoStreamTick() {
                    if pts != self.last_pts && self.grab(&e) {
                        self.last_pts = pts;
                        changed = true;
                    }
                }
            }
        }
        changed
    }

    /// Забрать текущий кадр: Media Engine → текстура → копия в память.
    fn grab(&mut self, e: &IMFMediaEngine) -> bool {
        let Some(d) = &mut self.d3d else { return false };
        unsafe {
            let (mut w, mut h) = (0u32, 0u32);
            if e.GetNativeVideoSize(Some(&mut w), Some(&mut h)).is_err() || w == 0 || h == 0 {
                return false;
            }
            if d.tex.as_ref().map(|t| (t.2, t.3) != (w, h)).unwrap_or(true) {
                let mut desc = D3D11_TEXTURE2D_DESC {
                    Width: w, Height: h, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32, CPUAccessFlags: 0, MiscFlags: 0,
                };
                let mut target = None;
                if d.device.CreateTexture2D(&desc, None, Some(&mut target)).is_err() {
                    return false;
                }
                desc.Usage = D3D11_USAGE_STAGING;
                desc.BindFlags = 0;
                desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                let mut staging = None;
                if d.device.CreateTexture2D(&desc, None, Some(&mut staging)).is_err() {
                    return false;
                }
                d.tex = Some((target.unwrap(), staging.unwrap(), w, h));
            }
            let (target, staging, w, h) = d.tex.clone().unwrap();
            let dst = RECT { left: 0, top: 0, right: w as i32, bottom: h as i32 };
            if e.TransferVideoFrame(&target, None, &dst, None).is_err() {
                return false;
            }
            d.ctx.CopyResource(&staging, &target);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            if d.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m)).is_err() {
                return false;
            }
            let mut px = vec![0u8; (w * h * 4) as usize];
            for y in 0..h as usize {
                let src = std::slice::from_raw_parts((m.pData as *const u8).add(y * m.RowPitch as usize), w as usize * 4);
                px[y * w as usize * 4..(y + 1) * w as usize * 4].copy_from_slice(src);
            }
            d.ctx.Unmap(&staging, 0);
            for p in px.chunks_exact_mut(4) {
                p[3] = 255;
            }
            self.frame = Some(Rc::new(Img { w, h, px }));
            true
        }
    }

    pub fn toggle_play(&mut self) {
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
        if let Some(e) = &self.engine {
            unsafe {
                let _ = e.Pause();
            }
            self.playing = false;
        }
    }

    pub fn seek(&mut self, t: f64) {
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
        if let Some(e) = &self.engine {
            unsafe {
                let _ = e.SetVolume(self.volume);
            }
        }
    }

    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
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
        if let Some(f) = &self.frame {
            v.push(format!("{} × {}", f.w, f.h));
        }
        if (self.rate - 1.0).abs() > 0.01 {
            v.push(format!("×{}", self.rate));
        }
        v.join("   ·   ")
    }

    /// Рисование: кадр или волна + полоса перемотки внизу холста.
    pub fn paint(&mut self, ui: &Ui, hdc: HDC, rc: RECT, hq: bool) {
        let t = &ui.t;
        let bar_h = ui.s(44);
        let area = RECT { bottom: rc.bottom - bar_h, ..rc };
        if self.is_video {
            gui::fill(hdc, &rc, crate::util::rgb(0, 0, 0));
            if let Some(f) = &self.frame {
                let (aw, ah) = ((area.right - area.left) as f64, (area.bottom - area.top) as f64);
                let k = (aw / f.w as f64).min(ah / f.h as f64);
                let (w, h) = ((f.w as f64 * k) as i32, (f.h as f64 * k) as i32);
                let x = area.left + ((aw as i32 - w) / 2);
                let y = area.top + ((ah as i32 - h) / 2);
                gui::blit(hdc, f, &gui::r(x, y, w, h), hq || !self.playing);
            }
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
                let b = gui::r((hx - tw / 2).clamp(sb.left, sb.right - tw), sb.top - ui.s(24), tw, ui.s(20));
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

    /// Текущий кадр (для «Кадр → фото»).
    pub fn snapshot(&self) -> Option<Img> {
        self.frame.as_ref().map(|f| (**f).clone())
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
