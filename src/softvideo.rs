// SimplePhotoViewer (Rust/Win32) — © 2026 danikforcode
// Лицензия MIT (см. LICENSE). Распространяется «как есть», без гарантий.
//! Свой проигрыватель видео H.265 (HEVC) — когда в Windows нет кодека HEVC
//! (он платный в Microsoft Store, а на многих компьютерах его не поставить).
//! Разбор MP4/MOV — mp4.rs, расшифровка — rusty_h265 (чистый Rust), звук —
//! Media Foundation (AAC в Windows есть всегда) → waveOut. Часы — по звуку,
//! а без звука — по системному таймеру. Кадры рисует свой поток в окно-экран.
//!
//! Потоки: «декодер» (кадры вперёд, с запасом), «показ» (вовремя выводит
//! кадр), «звук». Команды (пауза, перемотка, громкость) — через общее состояние.

use crate::image_buf::Img;
use crate::mp4;
use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::*};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::{MMTIME, TIME_SAMPLES};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Есть ли в системе декодер HEVC (Media Foundation).
pub fn system_has_hevc() -> bool {
    static R: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *R.get_or_init(|| unsafe {
        crate::media::startup();
        let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_HEVC };
        let mut acts: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut n = 0u32;
        let flags = MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER;
        let ok = MFTEnumEx(MFT_CATEGORY_VIDEO_DECODER, flags, Some(&input), None, &mut acts, &mut n).is_ok();
        if !acts.is_null() {
            for i in 0..n as usize {
                let _ = std::ptr::read(acts.add(i));
            }
            CoTaskMemFree(Some(acts as *const _));
        }
        ok && n > 0
    })
}

/// Нужен ли свой проигрыватель: видео HEVC, а системного кодека нет.
/// SPV_SOFT_HEVC=1 — всегда свой (для проверки).
pub fn needed(path: &Path) -> bool {
    let ext = crate::util::ext_lower(path);
    if !matches!(ext.as_str(), "mp4" | "m4v" | "mov" | "3gp" | "3g2") {
        return false;
    }
    let Some(codec) = probe_codec(path) else { return false };
    if &codec != b"hvc1" && &codec != b"hev1" {
        return false;
    }
    std::env::var("SPV_SOFT_HEVC").is_ok() || !system_has_hevc()
}

fn probe_codec(path: &Path) -> Option<[u8; 4]> {
    let mut f = std::fs::File::open(path).ok()?;
    let moov = mp4::read_moov(&mut f).ok()?;
    Some(mp4::parse(&moov)?.video?.codec)
}

// ------------------------------------------------------- общее состояние ---
struct Frame {
    pts: f64,
    fr: rusty_h265::Frame,
}

struct Shared {
    stop: AtomicBool,
    playing: AtomicBool,
    ended: AtomicBool,
    /// Декодер отдал все кадры (до следующей перемотки).
    eos: AtomicBool,
    /// Запрос перемотки (секунды × 1000), u64::MAX — нет.
    seek_to: AtomicU64,
    /// Счётчик перемоток: кадры старых «поколений» отбрасываются.
    gen: AtomicU32,
    queue: Mutex<VecDeque<(u32, Frame)>>,
    cv: Condvar,
    /// Последний показанный кадр (для снимка и перерисовки).
    shown: Mutex<Option<rusty_h265::Frame>>,
    /// Поворот кадра (градусы по часовой).
    rot: u32,
    /// Кадры рисует видеокарта (Direct3D 11), а не GDI.
    gpu: AtomicBool,
    redraw: AtomicBool,
    /// Область показа (размер окна-экрана).
    size: Mutex<(i32, i32)>,
    clock: Mutex<Clock>,
    volume: AtomicU32,
    error: Mutex<Option<String>>,
    /// Сколько кадров расшифровано / пропущено (для «Сведений»).
    decoded: AtomicU64,
    dropped: AtomicU64,
}

/// Часы проигрывания: позиция = base + (сейчас − start), пока идёт.
struct Clock {
    base: f64,
    start: Option<Instant>,
    /// Звук: сколько семплов отыграно с последней перемотки (если звук есть).
    audio: Option<(f64, Arc<AtomicU64>, f64)>, // (base, played_samples, rate)
}

impl Clock {
    fn now(&self) -> f64 {
        // звук на паузе не идёт — его счётчик сам «стоит»
        if let Some((b, s, rate)) = &self.audio {
            return b + s.load(Relaxed) as f64 / rate;
        }
        self.base + self.start.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0)
    }
    /// Перейти на системный таймер с текущей позиции (звук кончился или пропал).
    fn to_wall(&mut self) {
        let now = self.now();
        self.audio = None;
        self.base = now;
        if self.start.is_some() {
            self.start = Some(Instant::now());
        }
    }
}

pub struct SoftPlayer {
    sh: Arc<Shared>,
    child: HWND,
    threads: Vec<std::thread::JoinHandle<()>>,
    pub duration: f64,
    /// Размер кадра с учётом поворота.
    pub native: (u32, u32),
    pub fps: f64,
    rect: RECT,
}

unsafe extern "system" fn screen_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(-1),
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let _ = BeginPaint(h, &mut ps);
            let _ = EndPaint(h, &ps);
            let p = GetWindowLongPtrW(h, GWLP_USERDATA) as *const Shared;
            if !p.is_null() {
                (*p).redraw.store(true, Relaxed);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

fn screen_class() -> PCWSTR {
    static ONCE: std::sync::Once = std::sync::Once::new();
    let name = w!("SPV_SoftVideo");
    ONCE.call_once(|| unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(screen_proc),
            hInstance: windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default().into(),
            lpszClassName: name,
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            ..Default::default()
        };
        RegisterClassW(&wc);
    });
    name
}

impl SoftPlayer {
    pub fn open(parent: HWND, path: &Path, volume: f64) -> std::result::Result<SoftPlayer, String> {
        let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let moov = mp4::read_moov(&mut f).map_err(|e| e.to_string())?;
        let movie = mp4::parse(&moov).ok_or("не удалось разобрать MP4")?;
        let track = movie.video.ok_or("в файле нет видео")?;
        if track.samples.is_empty() || track.params.is_empty() {
            return Err("в видео нет кадров или параметров HEVC".into());
        }
        let native = if track.rotation % 180 == 90 { (track.height, track.width) } else { (track.width, track.height) };
        let sh = Arc::new(Shared {
            stop: AtomicBool::new(false),
            playing: AtomicBool::new(true),
            ended: AtomicBool::new(false),
            eos: AtomicBool::new(false),
            seek_to: AtomicU64::new(u64::MAX),
            gen: AtomicU32::new(0),
            queue: Mutex::new(VecDeque::new()),
            cv: Condvar::new(),
            shown: Mutex::new(None),
            rot: track.rotation,
            gpu: AtomicBool::new(false),
            redraw: AtomicBool::new(true),
            size: Mutex::new((64, 64)),
            clock: Mutex::new(Clock { base: 0.0, start: Some(Instant::now()), audio: None }),
            volume: AtomicU32::new((volume.clamp(0.0, 1.0) * 1000.0) as u32),
            error: Mutex::new(None),
            decoded: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        });
        let child = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                screen_class(),
                w!(""),
                WS_CHILD | WS_CLIPSIBLINGS,
                0,
                0,
                64,
                64,
                parent,
                None,
                windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default(),
                None,
            )
            .map_err(|e| e.message().to_string())?
        };
        unsafe {
            SetWindowLongPtrW(child, GWLP_USERDATA, Arc::as_ptr(&sh) as isize);
        }
        let duration = if movie.duration > 0.0 { movie.duration } else { track.duration };
        let fps = track.fps;
        let mut threads = vec![];
        let p: PathBuf = path.to_path_buf();
        // звук — первым: от него часы
        if movie.has_audio {
            let (s2, p2) = (sh.clone(), p.clone());
            threads.push(std::thread::spawn(move || audio_loop(s2, p2)));
        }
        let s2 = sh.clone();
        threads.push(std::thread::spawn(move || decode_loop(s2, f, track)));
        let (s2, h2) = (sh.clone(), child.0 as isize);
        threads.push(std::thread::spawn(move || present_loop(s2, HWND(h2 as *mut _))));
        Ok(SoftPlayer { sh, child, threads, duration, native, fps, rect: RECT::default() })
    }

    /// Разместить окно-экран (area — без полосы перемотки).
    pub fn place(&mut self, area: RECT, show: bool) {
        unsafe {
            if area != self.rect {
                self.rect = area;
                let (w, h) = ((area.right - area.left).max(1), (area.bottom - area.top).max(1));
                let _ = MoveWindow(self.child, area.left, area.top, w, h, false);
                *self.sh.size.lock().unwrap() = (w, h);
                self.sh.redraw.store(true, Relaxed);
            }
            if IsWindowVisible(self.child).as_bool() != show {
                let _ = ShowWindow(self.child, if show { SW_SHOWNA } else { SW_HIDE });
            }
        }
    }

    pub fn pos(&self) -> f64 {
        self.sh.clock.lock().unwrap().now().clamp(0.0, self.duration.max(0.0))
    }
    pub fn playing(&self) -> bool {
        self.sh.playing.load(Relaxed)
    }
    pub fn ended(&self) -> bool {
        self.sh.ended.load(Relaxed)
    }
    pub fn error(&self) -> Option<String> {
        self.sh.error.lock().unwrap().clone()
    }

    pub fn set_playing(&mut self, on: bool) {
        let mut c = self.sh.clock.lock().unwrap();
        if on && self.sh.ended.load(Relaxed) {
            drop(c);
            self.seek(0.0);
            c = self.sh.clock.lock().unwrap();
        }
        if on && c.start.is_none() {
            c.start = Some(Instant::now());
        } else if !on && c.start.is_some() {
            if c.audio.is_none() {
                c.base = c.now();
            }
            c.start = None;
        }
        self.sh.playing.store(on, Relaxed);
        self.sh.cv.notify_all();
    }

    pub fn seek(&mut self, t: f64) {
        let t = t.clamp(0.0, self.duration.max(0.0));
        {
            let mut c = self.sh.clock.lock().unwrap();
            c.base = t;
            c.audio = None; // звук заново выставит свою точку
            if c.start.is_some() {
                c.start = Some(Instant::now());
            }
        }
        self.sh.ended.store(false, Relaxed);
        self.sh.eos.store(false, Relaxed);
        self.sh.gen.fetch_add(1, Relaxed);
        self.sh.seek_to.store((t * 1000.0) as u64, Relaxed);
        self.sh.queue.lock().unwrap().clear();
        self.sh.cv.notify_all();
    }

    pub fn set_volume(&self, v: f64) {
        self.sh.volume.store((v.clamp(0.0, 1.0) * 1000.0) as u32, Relaxed);
    }

    /// Текущий кадр в полном размере.
    pub fn snapshot(&self) -> Option<Img> {
        let fr = self.sh.shown.lock().unwrap().clone()?;
        Some(to_bgra(&fr, self.sh.rot, u32::MAX, u32::MAX))
    }

    /// Рисует ли видеокарта.
    pub fn gpu(&self) -> bool {
        self.sh.gpu.load(Relaxed)
    }

    pub fn stats(&self) -> (u64, u64) {
        (self.sh.decoded.load(Relaxed), self.sh.dropped.load(Relaxed))
    }
}

impl Drop for SoftPlayer {
    fn drop(&mut self) {
        self.sh.stop.store(true, Relaxed);
        self.sh.cv.notify_all();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        unsafe {
            SetWindowLongPtrW(self.child, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.child);
        }
    }
}

// ------------------------------------------------------------ декодер ---
/// Сколько готовых кадров держать впереди (4K — меньше: кадр ~25 МБ).
fn ahead(w: u32, h: u32) -> usize {
    if w as u64 * h as u64 > 4_000_000 { 3 } else { 6 }
}

fn decode_loop(sh: Arc<Shared>, mut f: std::fs::File, t: mp4::Track) {
    let mut dec = rusty_h265::Decoder::new();
    let mut i = 0usize;
    let mut skip_before = -1.0f64;
    let mut gen = sh.gen.load(Relaxed);
    let push_params = |dec: &mut rusty_h265::Decoder| {
        for p in &t.params {
            let _ = dec.push_nal(p, None);
        }
    };
    push_params(&mut dec);
    let mut buf = vec![];
    let mut flushed = false;
    while !sh.stop.load(Relaxed) {
        // перемотка: с ближайшего опорного кадра до нужного времени
        let s = sh.seek_to.swap(u64::MAX, Relaxed);
        if s != u64::MAX {
            let target = s as f64 / 1000.0;
            gen = sh.gen.load(Relaxed);
            let k = t.samples.iter().rposition(|x| x.sync && x.pts <= target + 1e-3).unwrap_or(0);
            // ближайший опорный кадр в порядке хранения
            i = (0..=k).rev().find(|&j| t.samples[j].sync).unwrap_or(0);
            dec = rusty_h265::Decoder::new();
            push_params(&mut dec);
            skip_before = target - 0.5 / t.fps.max(1.0);
            flushed = false;
        }
        // очередь полна — ждём
        {
            let q = sh.queue.lock().unwrap();
            if q.len() >= ahead(t.width, t.height) {
                let _ = sh.cv.wait_timeout(q, Duration::from_millis(20));
                continue;
            }
        }
        if i < t.samples.len() {
            let smp = &t.samples[i];
            i += 1;
            buf.resize(smp.size as usize, 0);
            if f.seek(SeekFrom::Start(smp.offset)).is_err() || f.read_exact(&mut buf).is_err() {
                continue;
            }
            let pts = (smp.pts * 1e6) as i64;
            let mut first = true;
            for nal in mp4::split_nals(&buf, t.nal_len) {
                let _ = dec.push_nal(nal, if first { Some(pts) } else { None });
                first = false;
            }
        } else if !flushed {
            dec.flush();
            flushed = true;
        } else {
            if gen == sh.gen.load(Relaxed) {
                sh.eos.store(true, Relaxed);
            }
            // всё расшифровано — ждём перемотки или конца
            let q = sh.queue.lock().unwrap();
            let _ = sh.cv.wait_timeout(q, Duration::from_millis(30));
            continue;
        }
        while let Ok(fr) = dec.next_frame() {
            let pts = fr.pts.unwrap_or(0) as f64 / 1e6;
            sh.decoded.fetch_add(1, Relaxed);
            if pts < skip_before {
                continue;
            }
            // не успеваем: кадр уже в прошлом — не тратим время на перевод цветов
            let now = sh.clock.lock().unwrap().now();
            if sh.playing.load(Relaxed) && pts + 0.1 < now && sh.shown.lock().unwrap().is_some() {
                sh.dropped.fetch_add(1, Relaxed);
                continue;
            }
            // цвета переводит поток показа (или видеокарта) — декодер не ждёт
            let mut q = sh.queue.lock().unwrap();
            if gen == sh.gen.load(Relaxed) {
                q.push_back((gen, Frame { pts, fr }));
            }
        }
    }
}

pub use crate::mp4::to_bgra;

// ------------------------------------------------------------ показ ---
fn present_loop(sh: Arc<Shared>, hwnd: HWND) {
    let mut cur: Option<(f64, rusty_h265::Frame)> = None;
    // GDI: кадр, уже переведённый в цвета под размер окна
    let mut rgb: Option<((i32, i32), Img)> = None;
    let mut gpu = {
        let (w, h) = *sh.size.lock().unwrap();
        crate::gpuvideo::Gpu::new(hwnd, w, h)
    };
    sh.gpu.store(gpu.is_some(), Relaxed);
    let mut shown_gen = u32::MAX;
    while !sh.stop.load(Relaxed) {
        let now = sh.clock.lock().unwrap().now();
        let gen = sh.gen.load(Relaxed);
        let playing = sh.playing.load(Relaxed);
        let mut pick: Option<Frame> = None;
        {
            let mut q = sh.queue.lock().unwrap();
            q.retain(|(g, _)| *g == gen);
            if shown_gen != gen {
                // после открытия или перемотки — сразу первый кадр (и на паузе тоже)
                if let Some((_, f)) = q.pop_front() {
                    // без звука часы пускаем с первого кадра: пока декодер
                    // разгонялся, время не должно было уйти вперёд
                    let mut c = sh.clock.lock().unwrap();
                    if c.audio.is_none() && c.start.is_some() {
                        c.base = f.pts;
                        c.start = Some(Instant::now());
                    }
                    drop(c);
                    pick = Some(f);
                    shown_gen = gen;
                }
            } else if playing {
                // со звуком: последний кадр, которому уже пора, более ранние — пропущены
                // (картинка не отстаёт от звука); без звука — по одному кадру,
                // а часы подождут: видео идёт медленнее, но плавно
                let wall = sh.clock.lock().unwrap().audio.is_none();
                while q.front().map(|(_, f)| f.pts <= now + 0.004).unwrap_or(false) {
                    if pick.is_some() {
                        sh.dropped.fetch_add(1, Relaxed);
                    }
                    pick = q.pop_front().map(|x| x.1);
                    if wall {
                        break;
                    }
                }
                if let (true, Some(f)) = (wall, &pick) {
                    let mut c = sh.clock.lock().unwrap();
                    if c.audio.is_none() && c.start.is_some() && c.now() > f.pts + 0.02 {
                        c.base = f.pts;
                        c.start = Some(Instant::now());
                    }
                }
            }
            sh.cv.notify_all();
            // без звука и декодер не успевает (4K на слабом ПК): часы ждут кадры —
            // видео идёт медленнее, но без рывков и пропусков
            if playing && q.is_empty() && !sh.eos.load(Relaxed) && shown_gen == gen {
                if let Some((last, _)) = &cur {
                    let mut c = sh.clock.lock().unwrap();
                    if c.audio.is_none() && c.start.is_some() && c.now() > last + 0.06 {
                        c.base = last + 0.06;
                        c.start = Some(Instant::now());
                    }
                }
            }
            // конец: декодер всё отдал, очередь пуста, время ушло за последний кадр
            if playing && q.is_empty() && sh.eos.load(Relaxed) && cur.as_ref().map(|c| now > c.0 + 0.1).unwrap_or(true) {
                sh.ended.store(true, Relaxed);
                sh.playing.store(false, Relaxed);
            }
        }
        let fresh = pick.is_some();
        if let Some(f) = pick {
            *sh.shown.lock().unwrap() = Some(f.fr.clone());
            cur = Some((f.pts, f.fr));
            rgb = None;
            if let (Some(g), Some((_, fr))) = (&mut gpu, &cur) {
                if !g.load(fr) {
                    gpu = None;
                    sh.gpu.store(false, Relaxed);
                }
            }
        }
        if fresh || sh.redraw.swap(false, Relaxed) {
            if let Some((_, fr)) = &cur {
                let size = *sh.size.lock().unwrap();
                let ok = match &mut gpu {
                    Some(g) => g.draw(fr, sh.rot, size),
                    None => false,
                };
                if !ok {
                    if gpu.take().is_some() {
                        // видеокарта отказала (сброс драйвера и т. п.) — дальше через GDI
                        sh.gpu.store(false, Relaxed);
                    }
                    if rgb.as_ref().map(|r| r.0 != size).unwrap_or(true) {
                        rgb = Some((size, to_bgra(fr, sh.rot, size.0.max(1) as u32, size.1.max(1) as u32)));
                    }
                    if let Some((_, img)) = &rgb {
                        draw(hwnd, img, size);
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(3));
    }
}

/// Кадр в окно-экран: вписать по центру, поля — чёрные.
fn draw(hwnd: HWND, img: &Img, (cw, ch): (i32, i32)) {
    unsafe {
        let dc = GetDC(hwnd);
        if dc.is_invalid() {
            return;
        }
        let k = (cw as f64 / img.w as f64).min(ch as f64 / img.h as f64);
        let (w, h) = ((img.w as f64 * k).round() as i32, (img.h as f64 * k).round() as i32);
        let (x, y) = ((cw - w) / 2, (ch - h) / 2);
        let black = HBRUSH(GetStockObject(BLACK_BRUSH).0);
        for r in [RECT { left: 0, top: 0, right: cw, bottom: y }, RECT { left: 0, top: y + h, right: cw, bottom: ch }, RECT { left: 0, top: y, right: x, bottom: y + h }, RECT { left: x + w, top: y, right: cw, bottom: y + h }] {
            if r.right > r.left && r.bottom > r.top {
                FillRect(dc, &r, black);
            }
        }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER { biSize: 40, biWidth: img.w as i32, biHeight: -(img.h as i32), biPlanes: 1, biBitCount: 32, ..Default::default() },
            ..Default::default()
        };
        // кадр уже нужного размера (растяжение — только если окно выросло, до следующего кадра)
        SetStretchBltMode(dc, COLORONCOLOR);
        let _ = SetBrushOrgEx(dc, 0, 0, None);
        StretchDIBits(dc, x, y, w, h, 0, 0, img.w as i32, img.h as i32, Some(img.px.as_ptr() as *const _), &bmi, DIB_RGB_COLORS, SRCCOPY);
        ReleaseDC(hwnd, dc);
    }
}

// ------------------------------------------------------------ звук ---
fn audio_loop(sh: Arc<Shared>, path: PathBuf) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        crate::media::startup();
        let r = (|| -> Option<()> {
            let url = crate::util::wpath(&path);
            let rd = MFCreateSourceReaderFromURL(PCWSTR(url.as_ptr()), None).ok()?;
            let fa = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
            rd.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false).ok()?;
            rd.SetStreamSelection(fa, true).ok()?;
            let t = MFCreateMediaType().ok()?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).ok()?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).ok()?;
            t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).ok()?;
            rd.SetCurrentMediaType(fa, None, &t).ok()?;
            let cur = rd.GetCurrentMediaType(fa).ok()?;
            let rate = cur.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).ok()?;
            let ch = cur.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).ok()?.clamp(1, 8) as u16;
            let wf = WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_PCM as u16,
                nChannels: ch,
                nSamplesPerSec: rate,
                nAvgBytesPerSec: rate * ch as u32 * 2,
                nBlockAlign: ch * 2,
                wBitsPerSample: 16,
                cbSize: 0,
            };
            let mut wo = HWAVEOUT::default();
            if waveOutOpen(Some(&mut wo), WAVE_MAPPER, &wf, 0, 0, CALLBACK_NULL) != 0 {
                return None; // нет устройства звука — играем по таймеру
            }
            let played = Arc::new(AtomicU64::new(0));
            // буферы: 8 по ~100 мс
            const NB: usize = 8;
            let mut bufs: Vec<(Box<WAVEHDR>, Vec<u8>)> = (0..NB).map(|_| (Box::new(WAVEHDR::default()), vec![])).collect();
            let mut pending: Vec<u8> = vec![];
            let chunk = (rate as usize / 10) * ch as usize * 2;
            let mut gen = u32::MAX;
            let mut samples_base: u64 = 0;
            let mut eof = false;
            let mut paused = false;
            let mut last_vol = u32::MAX;
            let mut switched = false;
            while !sh.stop.load(Relaxed) {
                // громкость
                let v = sh.volume.load(Relaxed);
                if v != last_vol {
                    let lv = (v * 0xFFFF / 1000) as u32;
                    waveOutSetVolume(wo, lv | (lv << 16));
                    last_vol = v;
                }
                // перемотка (или старт)
                let g = sh.gen.load(Relaxed);
                if g != gen {
                    gen = g;
                    let target = sh.clock.lock().unwrap().now();
                    waveOutReset(wo);
                    for (h, _) in bufs.iter_mut() {
                        if h.dwFlags & WHDR_PREPARED != 0 {
                            waveOutUnprepareHeader(wo, &mut **h, std::mem::size_of::<WAVEHDR>() as u32);
                        }
                        h.dwFlags = 0;
                    }
                    pending.clear();
                    let pv = windows::core::PROPVARIANT::from((target * 1e7) as i64);
                    let _ = rd.SetCurrentPosition(&GUID::zeroed(), &pv);
                    eof = false;
                    switched = false;
                    // после сброса устройство снова «играет» — пауза выставится ниже заново
                    paused = false;
                    let mut mt = MMTIME { wType: TIME_SAMPLES, ..Default::default() };
                    waveOutGetPosition(wo, &mut mt, std::mem::size_of::<MMTIME>() as u32);
                    samples_base = mt.u.sample as u64;
                    played.store(0, Relaxed);
                    let mut c = sh.clock.lock().unwrap();
                    c.audio = Some((target, played.clone(), rate as f64));
                }
                // пауза
                let want_play = sh.playing.load(Relaxed);
                if !want_play && !paused {
                    waveOutPause(wo);
                    paused = true;
                } else if want_play && paused {
                    waveOutRestart(wo);
                    paused = false;
                }
                // позиция → часы
                let mut mt = MMTIME { wType: TIME_SAMPLES, ..Default::default() };
                if waveOutGetPosition(wo, &mut mt, std::mem::size_of::<MMTIME>() as u32) == 0 && mt.wType == TIME_SAMPLES {
                    played.store((mt.u.sample as u64).saturating_sub(samples_base), Relaxed);
                }
                // свободные буферы — наполнить
                let mut fed = false;
                for (h, data) in bufs.iter_mut() {
                    if h.dwFlags & WHDR_PREPARED != 0 && h.dwFlags & WHDR_DONE == 0 {
                        continue;
                    }
                    if h.dwFlags & WHDR_PREPARED != 0 {
                        waveOutUnprepareHeader(wo, &mut **h, std::mem::size_of::<WAVEHDR>() as u32);
                    }
                    while pending.len() < chunk && !eof {
                        let mut flags = 0u32;
                        let mut s: Option<IMFSample> = None;
                        if rd.ReadSample(fa, 0, None, Some(&mut flags), None, Some(&mut s)).is_err() {
                            eof = true;
                            break;
                        }
                        if let Some(s) = s {
                            if let Ok(b) = s.ConvertToContiguousBuffer() {
                                let mut p: *mut u8 = std::ptr::null_mut();
                                let mut len = 0u32;
                                if b.Lock(&mut p, None, Some(&mut len)).is_ok() {
                                    pending.extend_from_slice(std::slice::from_raw_parts(p, len as usize));
                                    let _ = b.Unlock();
                                }
                            }
                        }
                        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                            eof = true;
                        }
                    }
                    if pending.is_empty() {
                        break;
                    }
                    let n = pending.len().min(chunk);
                    *data = pending.drain(..n).collect();
                    **h = WAVEHDR { lpData: PSTR(data.as_mut_ptr()), dwBufferLength: data.len() as u32, ..Default::default() };
                    waveOutPrepareHeader(wo, &mut **h, std::mem::size_of::<WAVEHDR>() as u32);
                    waveOutWrite(wo, &mut **h, std::mem::size_of::<WAVEHDR>() as u32);
                    fed = true;
                }
                if !fed {
                    // звук кончился и доигран — дальше часы по таймеру (видео может быть длиннее)
                    let idle = bufs.iter().all(|(h, _)| h.dwFlags & WHDR_PREPARED == 0 || h.dwFlags & WHDR_DONE != 0);
                    if eof && pending.is_empty() && idle && !switched {
                        sh.clock.lock().unwrap().to_wall();
                        switched = true;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            waveOutReset(wo);
            for (h, _) in bufs.iter_mut() {
                if h.dwFlags & WHDR_PREPARED != 0 {
                    waveOutUnprepareHeader(wo, &mut **h, std::mem::size_of::<WAVEHDR>() as u32);
                }
            }
            waveOutClose(wo);
            Some(())
        })();
        let _ = r;
        CoUninitialize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_small_hevc_frames() {
        let p = format!("{}/tests/data/hevc_small.mp4", env!("CARGO_MANIFEST_DIR"));
        let Ok(mut f) = std::fs::File::open(&p) else { return };
        let moov = mp4::read_moov(&mut f).unwrap();
        let t = mp4::parse(&moov).unwrap().video.unwrap();
        let mut dec = rusty_h265::Decoder::new();
        for p in &t.params {
            dec.push_nal(p, None).unwrap();
        }
        let mut frames = vec![];
        for s in &t.samples {
            let mut buf = vec![0u8; s.size as usize];
            f.seek(SeekFrom::Start(s.offset)).unwrap();
            f.read_exact(&mut buf).unwrap();
            for (k, nal) in mp4::split_nals(&buf, t.nal_len).into_iter().enumerate() {
                let _ = dec.push_nal(nal, if k == 0 { Some((s.pts * 1e6) as i64) } else { None });
            }
            while let Ok(fr) = dec.next_frame() {
                frames.push(fr);
            }
        }
        dec.flush();
        while let Ok(fr) = dec.next_frame() {
            frames.push(fr);
        }
        assert_eq!(frames.len(), 30);
        // время показа растёт (B-кадры переставлены декодером)
        let pts: Vec<i64> = frames.iter().map(|f| f.pts.unwrap()).collect();
        assert!(pts.windows(2).all(|w| w[0] < w[1]), "{pts:?}");
        // поворот на 90°: кадр 320×180 → 180×320
        let img = to_bgra(&frames[0], t.rotation, 1000, 1000);
        assert_eq!((img.w, img.h), (180, 320));
        assert!(img.px.chunks(4).any(|p| p[0] != p[2]), "цветная картинка");
    }
}
