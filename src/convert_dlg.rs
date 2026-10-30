// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Окно пакетной конвертации: список файлов, во что превращать каждый тип,
//! куда класть результат. Показываются только настройки для типов,
//! которые есть в списке, — без лишних кнопок.

use crate::convert::{self, Msg, Settings, AUDIO_TARGETS, PHOTO_TARGETS, VIDEO_TARGETS};
use crate::files;
use crate::gui::{self, Ev, Kind as WK, Panel, Ui, W};
use crate::kinds::{self, Kind};
use crate::settings;
use crate::util::file_name;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::HDC;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

const ID_LIST: u32 = 1;
const ID_ADD: u32 = 2;
const ID_DEL: u32 = 3;
const ID_CLEAR: u32 = 4;
const ID_GO: u32 = 5;
const ID_STOP: u32 = 6;
const ID_CLOSE: u32 = 7;
const ID_FOLDER: u32 = 8;
const ID_SHOW: u32 = 9;
const ID_PHOTO0: u32 = 100;
const ID_PMAX0: u32 = 120;
const ID_AUDIO0: u32 = 140;
const ID_VIDEO0: u32 = 160;
const ID_VH0: u32 = 180;
const ID_Q0: u32 = 200;
const ID_DEST0: u32 = 220;
const TIMER: usize = 0x5301;

const PMAX: [(u32, &str); 5] = [(0, "Как есть"), (3840, "3840"), (2560, "2560"), (1920, "1920"), (1280, "1280")];
const VH: [(u32, &str); 4] = [(0, "Как есть"), (1080, "1080p"), (720, "720p"), (480, "480p")];

#[derive(Clone)]
enum St {
    Wait,
    Run(f64),
    Ok(PathBuf),
    Err(String),
}

struct Item {
    path: PathBuf,
    kind: Kind,
    st: St,
}

struct Dlg {
    hwnd: HWND,
    items: Vec<Item>,
    sel: Option<usize>,
    scroll: usize,
    s: Settings,
    dest: Option<PathBuf>,
    rx: Option<Receiver<Msg>>,
    stop: Arc<AtomicBool>,
    /// Индексы в items для текущего запуска.
    run_map: Vec<usize>,
    last_out: Option<PathBuf>,
}

fn convertible(k: Kind) -> bool {
    matches!(k, Kind::Image | Kind::Audio | Kind::Video)
}

impl Dlg {
    fn add(&mut self, paths: Vec<PathBuf>) -> usize {
        let mut skipped = 0;
        for p in paths {
            if p.is_dir() {
                if let Ok(rd) = std::fs::read_dir(&p) {
                    let mut v: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file()).collect();
                    v.sort();
                    skipped += self.add(v);
                }
                continue;
            }
            match kinds::kind_of(&p) {
                Some(k) if convertible(k) && k != Kind::Pdf => {
                    if !self.items.iter().any(|i| i.path == p) {
                        self.items.push(Item { path: p, kind: k, st: St::Wait });
                    }
                }
                _ => skipped += 1,
            }
        }
        skipped
    }

    fn running(&self) -> bool {
        self.rx.is_some()
    }

    fn has(&self, k: Kind) -> bool {
        self.items.iter().any(|i| i.kind == k)
    }

    fn sub(&self, i: &Item) -> String {
        let ext = convert::target_ext(i.kind, &self.s);
        match &i.st {
            St::Wait => {
                let kind = match i.kind {
                    Kind::Image => "фото",
                    Kind::Audio => "аудио",
                    _ => "видео",
                };
                let same = i.path.extension().map(|e| e.to_string_lossy().eq_ignore_ascii_case(ext)).unwrap_or(false);
                if same && i.kind != Kind::Image {
                    format!("{kind} · уже .{ext} — будет пересжато")
                } else {
                    format!("{kind} → .{ext}")
                }
            }
            St::Run(f) => format!("{:.0}%  …", f * 100.0),
            St::Ok(p) => format!("Готово: {}", file_name(p)),
            St::Err(e) => format!("Ошибка: {e}"),
        }
    }

    fn start(&mut self, hwnd: HWND) {
        let mut jobs = vec![];
        self.run_map.clear();
        for (k, i) in self.items.iter_mut().enumerate() {
            if !matches!(i.st, St::Ok(_)) {
                i.st = St::Wait;
                jobs.push((i.path.clone(), i.kind));
                self.run_map.push(k);
            }
        }
        if jobs.is_empty() {
            return;
        }
        self.save_settings();
        let (tx, rx) = channel();
        self.stop = Arc::new(AtomicBool::new(false));
        convert::start(jobs, self.s, self.dest.clone(), tx, self.stop.clone());
        self.rx = Some(rx);
        unsafe {
            SetTimer(hwnd, TIMER, 120, None);
        }
    }

    fn poll(&mut self, hwnd: HWND) -> bool {
        let Some(rx) = &self.rx else { return false };
        let mut changed = false;
        let mut finished = false;
        while let Ok(m) = rx.try_recv() {
            changed = true;
            match m {
                Msg::Progress(j, f) => {
                    if let Some(&k) = self.run_map.get(j) {
                        self.items[k].st = St::Run(f);
                    }
                }
                Msg::Done(j, p) => {
                    if let Some(&k) = self.run_map.get(j) {
                        self.items[k].st = St::Ok(p.clone());
                    }
                    self.last_out = Some(p);
                }
                Msg::Error(j, e) => {
                    if let Some(&k) = self.run_map.get(j) {
                        self.items[k].st = St::Err(e);
                    }
                }
                Msg::Finished => finished = true,
            }
        }
        if finished {
            self.rx = None;
            for i in &mut self.items {
                if matches!(i.st, St::Run(_)) {
                    i.st = St::Err("остановлено".into());
                }
            }
            unsafe {
                let _ = KillTimer(hwnd, TIMER);
            }
        }
        changed
    }

    fn save_settings(&self) {
        settings::set_i("cv_photo", self.s.photo as i32);
        settings::set_i("cv_pmax", self.s.photo_max as i32);
        settings::set_i("cv_audio", self.s.audio as i32);
        settings::set_i("cv_video", self.s.video as i32);
        settings::set_i("cv_vh", self.s.video_h as i32);
        settings::set_i("cv_q", self.s.quality as i32);
    }

    fn summary(&self) -> String {
        let n = self.items.len();
        let ok = self.items.iter().filter(|i| matches!(i.st, St::Ok(_))).count();
        let bad = self.items.iter().filter(|i| matches!(i.st, St::Err(_))).count();
        if self.running() {
            let done = ok + bad;
            let cur = self.items.iter().find_map(|i| if let St::Run(f) = i.st { Some(f) } else { None }).unwrap_or(0.0);
            let total = if n > 0 { (done as f64 + cur) / n as f64 * 100.0 } else { 0.0 };
            format!("Конвертация: {} из {} · {:.0}%", (done + 1).min(n), n, total)
        } else if ok + bad > 0 {
            if bad > 0 { format!("Готово: {ok}, с ошибкой: {bad}") } else { format!("Готово: {ok} из {n}") }
        } else {
            format!("Файлов: {n}")
        }
    }
}

impl Panel for Dlg {
    fn title(&self) -> String {
        "Конвертация файлов".into()
    }

    fn init(&mut self, hwnd: HWND) {
        self.hwnd = hwnd;
        unsafe { windows::Win32::UI::Shell::DragAcceptFiles(hwnd, true) };
    }

    fn min_size(&self) -> (i32, i32) {
        (760, 520)
    }

    fn layout(&mut self, ui: &Ui, w: i32, h: i32) -> Vec<W> {
        let m = ui.s(16);
        let pw = ui.s(330);
        let rx = w - m - pw;
        let mut v = vec![];
        let busy = self.running();
        let gh = ui.s(22);
        let row = ui.s(32);
        let step = ui.s(14);

        // ---- список слева
        v.push(W::new(WK::Group, 0, gui::r(m, m, rx - 2 * m, gh)).t("Файлы (можно перетащить в окно — и папки)"));
        let ly = m + gh + ui.s(6);
        let bh = ui.s(34);
        let lh = (h - ly - m - bh - ui.s(52)).max(ui.s(120));
        let lw = rx - 2 * m;
        let mut list = W::new(WK::List, ID_LIST, gui::r(m, ly, lw, lh)).t("Добавьте фото, аудио или видео");
        list.items = self.items.iter().map(|i| (file_name(&i.path), self.sub(i))).collect();
        list.sel = self.sel;
        gui::list_fix_scroll(ui, &list, &mut self.scroll);
        list.scroll = self.scroll;
        v.push(list);
        let by = ly + lh + ui.s(8);
        let mut bx = m;
        for (id, ic, t, en, bw) in [
            (ID_ADD, gui::IC_ADD, "Добавить…", !busy, 130),
            (ID_DEL, gui::IC_REMOVE, "Убрать", !busy && self.sel.is_some(), 110),
            (ID_CLEAR, gui::IC_DELETE, "Очистить", !busy && !self.items.is_empty(), 120),
        ] {
            v.push(W::new(WK::Button, id, gui::r(bx, by, ui.s(bw), bh)).i(ic).t(t).en(en));
            bx += ui.s(bw) + ui.s(8);
        }
        v.push(W::new(WK::Label, 0, gui::r(m, by + bh + ui.s(8), lw, ui.s(22))).t(self.summary()).color(ui.t.dim));

        // ---- настройки справа: только для тех типов, что есть в списке
        let mut y = m;
        let group = |v: &mut Vec<W>, y: &mut i32, t: &str| {
            v.push(W::new(WK::Group, 0, gui::r(rx, *y, pw, gh)).t(t));
            *y += gh + ui.s(4);
        };
        let any = !self.items.is_empty();
        if self.has(Kind::Image) || !any {
            group(&mut v, &mut y, "Фото → формат");
            let names: Vec<&str> = PHOTO_TARGETS.iter().map(|t| t.1).collect();
            v.extend(gui::segments(ui, rx, y, pw, ID_PHOTO0, &names[..4], self.s.photo));
            y += row + ui.s(4);
            v.extend(gui::segments(ui, rx, y, pw, ID_PHOTO0 + 4, &names[4..], self.s.photo.wrapping_sub(4)));
            y += row + ui.s(6);
            v.push(W::new(WK::Label, 0, gui::r(rx, y, pw, ui.s(20))).t("Длинная сторона, пикс.").color(ui.t.dim));
            y += ui.s(22);
            let pm: Vec<&str> = PMAX.iter().map(|p| p.1).collect();
            let cur = PMAX.iter().position(|p| p.0 == self.s.photo_max).unwrap_or(0);
            v.extend(gui::segments(ui, rx, y, pw, ID_PMAX0, &pm, cur));
            y += row + step;
        }
        if self.has(Kind::Audio) {
            group(&mut v, &mut y, "Аудио → формат");
            let names: Vec<&str> = AUDIO_TARGETS.iter().map(|t| t.1).collect();
            v.extend(gui::segments(ui, rx, y, pw, ID_AUDIO0, &names, self.s.audio));
            y += row + step;
        }
        if self.has(Kind::Video) {
            group(&mut v, &mut y, "Видео → формат");
            let names: Vec<&str> = VIDEO_TARGETS.iter().map(|t| t.1).collect();
            v.extend(gui::segments(ui, rx, y, pw, ID_VIDEO0, &names[..2], self.s.video));
            y += row + ui.s(4);
            v.extend(gui::segments(ui, rx, y, pw, ID_VIDEO0 + 2, &names[2..], self.s.video.wrapping_sub(2)));
            y += row + ui.s(6);
            if self.s.video < 2 {
                v.push(W::new(WK::Label, 0, gui::r(rx, y, pw, ui.s(20))).t("Высота кадра (не больше исходной)").color(ui.t.dim));
                y += ui.s(22);
                let vh: Vec<&str> = VH.iter().map(|p| p.1).collect();
                let cur = VH.iter().position(|p| p.0 == self.s.video_h).unwrap_or(0);
                v.extend(gui::segments(ui, rx, y, pw, ID_VH0, &vh, cur));
                y += row;
            }
            y += step;
        }
        let lossy = (self.has(Kind::Image) && matches!(PHOTO_TARGETS[self.s.photo].0, "jpg" | "jxr" | "pdf"))
            || (self.has(Kind::Audio) && matches!(AUDIO_TARGETS[self.s.audio].0, "mp3" | "m4a" | "wma"))
            || self.has(Kind::Video);
        if lossy {
            group(&mut v, &mut y, "Качество");
            v.extend(gui::segments(ui, rx, y, pw, ID_Q0, &["Высокое", "Среднее", "Меньше файл"], self.s.quality));
            y += row + step;
        }
        group(&mut v, &mut y, "Куда сохранить");
        v.extend(gui::segments(ui, rx, y, pw, ID_DEST0, &["Рядом с исходными", "В папку…"], self.dest.is_some() as usize));
        y += row + ui.s(4);
        if let Some(d) = &self.dest {
            v.push(W::new(WK::Button, ID_FOLDER, gui::r(rx, y, pw, ui.s(30))).t(d.to_string_lossy()).tip("Сменить папку"));
        } else {
            v.push(W::new(WK::Label, 0, gui::r(rx, y, pw, ui.s(30))).t("Существующие файлы не перезаписываются").color(ui.t.dim));
        }

        // ---- кнопки
        let by = h - m - ui.s(36);
        let mut x = w - m;
        let pending = self.items.iter().any(|i| !matches!(i.st, St::Ok(_)));
        let buttons: Vec<(u32, &str, bool, bool, i32)> = if busy {
            vec![(ID_STOP, "Остановить", true, true, 150)]
        } else {
            let mut b = vec![(ID_GO, "Конвертировать", true, pending, 170), (ID_CLOSE, "Закрыть", false, true, 100)];
            if self.last_out.is_some() {
                b.push((ID_SHOW, "Показать в папке", false, true, 160));
            }
            b
        };
        for (id, t, primary, en, bw) in buttons {
            let bw = ui.s(bw);
            x -= bw;
            v.push(W::new(WK::Button, id, gui::r(x, by, bw, ui.s(36))).t(t).on(primary).en(en));
            x -= ui.s(8);
        }
        v
    }

    fn paint(&mut self, ui: &Ui, hdc: HDC, w: i32, h: i32) {
        gui::fill(hdc, &gui::r(0, 0, w, h), ui.t.bg);
    }

    fn event(&mut self, hwnd: HWND, ev: Ev) -> bool {
        let busy = self.running();
        match ev {
            Ev::Item(ID_LIST, k) => self.sel = Some(k),
            Ev::Dbl(ID_LIST, k) => {
                self.sel = Some(k);
                if let Some(St::Ok(p)) = self.items.get(k).map(|i| i.st.clone()) {
                    show_in_folder(&p);
                }
            }
            Ev::Scroll(ID_LIST, d) => self.scroll = (self.scroll as i32 + d).max(0) as usize,
            Ev::Click(id) => match id {
                ID_ADD if !busy => {
                    let p = files::open_dialog(hwnd, files::Pick::Any, true);
                    self.add(p);
                }
                ID_DEL if !busy => {
                    if let Some(s) = self.sel {
                        if s < self.items.len() {
                            self.items.remove(s);
                        }
                        self.sel = if self.items.is_empty() { None } else { Some(s.min(self.items.len() - 1)) };
                    }
                }
                ID_CLEAR if !busy => {
                    self.items.clear();
                    self.sel = None;
                    self.last_out = None;
                }
                ID_GO if !busy => self.start(hwnd),
                ID_STOP => self.stop.store(true, Ordering::Relaxed),
                ID_CLOSE => return false,
                ID_SHOW => {
                    if let Some(p) = &self.last_out {
                        show_in_folder(p);
                    }
                }
                ID_FOLDER => {
                    if let Some(d) = files::pick_folder(hwnd, self.dest.as_deref()) {
                        settings::set_s("cv_dest", &d.to_string_lossy());
                        self.dest = Some(d);
                    }
                }
                id if (ID_DEST0..ID_DEST0 + 2).contains(&id) => {
                    if id == ID_DEST0 {
                        self.dest = None;
                    } else if self.dest.is_none() {
                        let start = settings::get_s("cv_dest").map(PathBuf::from).filter(|p| p.is_dir());
                        if let Some(d) = files::pick_folder(hwnd, start.as_deref()) {
                            settings::set_s("cv_dest", &d.to_string_lossy());
                            self.dest = Some(d);
                        }
                    }
                }
                id if busy && id >= ID_PHOTO0 => {}
                id if (ID_PHOTO0..ID_PHOTO0 + PHOTO_TARGETS.len() as u32).contains(&id) => self.s.photo = (id - ID_PHOTO0) as usize,
                id if (ID_PMAX0..ID_PMAX0 + PMAX.len() as u32).contains(&id) => self.s.photo_max = PMAX[(id - ID_PMAX0) as usize].0,
                id if (ID_AUDIO0..ID_AUDIO0 + AUDIO_TARGETS.len() as u32).contains(&id) => self.s.audio = (id - ID_AUDIO0) as usize,
                id if (ID_VIDEO0..ID_VIDEO0 + VIDEO_TARGETS.len() as u32).contains(&id) => self.s.video = (id - ID_VIDEO0) as usize,
                id if (ID_VH0..ID_VH0 + VH.len() as u32).contains(&id) => self.s.video_h = VH[(id - ID_VH0) as usize].0,
                id if (ID_Q0..ID_Q0 + 3).contains(&id) => self.s.quality = (id - ID_Q0) as usize,
                _ => {}
            },
            _ => {}
        }
        // смена настроек — заново для уже готовых
        if !busy {
            if let Ev::Click(id) = ev {
                if id >= ID_PHOTO0 && id < ID_DEST0 {
                    for i in &mut self.items {
                        if matches!(i.st, St::Ok(_) | St::Err(_)) {
                            i.st = St::Wait;
                        }
                    }
                }
            }
        }
        true
    }

    fn key(&mut self, hwnd: HWND, vk: u16, _ctrl: bool, _shift: bool) -> Option<bool> {
        match vk {
            0x2E if !self.running() => Some(self.event(hwnd, Ev::Click(ID_DEL))),
            0x0D if !self.running() => Some(self.event(hwnd, Ev::Click(ID_GO))),
            _ => None,
        }
    }

    fn timer(&mut self, hwnd: HWND, id: usize) -> bool {
        if id == TIMER && self.poll(hwnd) {
            gui::invalidate(hwnd);
        }
        true
    }

    fn drop_files(&mut self, _hwnd: HWND, files: Vec<PathBuf>) {
        if !self.running() {
            self.add(files);
        }
    }

    fn close(&mut self, _hwnd: HWND) -> bool {
        if self.running() {
            self.stop.store(true, Ordering::Relaxed);
        }
        true
    }
}

fn show_in_folder(p: &std::path::Path) {
    let arg = crate::util::wide(&format!("/select,\"{}\"", p.to_string_lossy()));
    let exe = crate::util::wide("explorer.exe");
    unsafe {
        windows::Win32::UI::Shell::ShellExecuteW(
            None,
            windows::core::PCWSTR::null(),
            windows::core::PCWSTR(exe.as_ptr()),
            windows::core::PCWSTR(arg.as_ptr()),
            windows::core::PCWSTR::null(),
            windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        );
    }
}

/// Открыть окно конвертации с начальными файлами.
pub fn run(owner: HWND, init: Vec<PathBuf>) {
    let g = |k: &str, d: i32, n: usize| settings::get_i(k, d).clamp(0, n as i32 - 1) as usize;
    let mut d = Dlg {
        hwnd: owner,
        items: vec![],
        sel: None,
        scroll: 0,
        s: Settings {
            photo: g("cv_photo", 0, PHOTO_TARGETS.len()),
            photo_max: settings::get_i("cv_pmax", 0).max(0) as u32,
            audio: g("cv_audio", 0, AUDIO_TARGETS.len()),
            video: g("cv_video", 0, VIDEO_TARGETS.len()),
            video_h: settings::get_i("cv_vh", 0).max(0) as u32,
            quality: g("cv_q", 0, 3),
        },
        dest: None,
        rx: None,
        stop: Arc::new(AtomicBool::new(false)),
        run_map: vec![],
        last_out: None,
    };
    d.add(init);
    let mut d = gui::run_modal(owner, d, (900, 620));
    // окно закрыто во время работы: дождаться остановки потока
    while d.running() {
        d.stop.store(true, Ordering::Relaxed);
        if let Some(rx) = &d.rx {
            match rx.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(Msg::Finished) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => d.rx = None,
                _ => {}
            }
        }
    }
}
