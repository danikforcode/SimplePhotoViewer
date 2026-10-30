// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! «Хвостики»: превью соседних файлов по краям окна. Превью готовятся в
//! фоновом потоке: фото и PDF — картинка, видео — кадр, аудио — волна,
//! текст и код — первые строки, таблица — первые ячейки.

use crate::gui::{self, Ui};
use crate::image_buf::Img;
use crate::kinds::{self, Kind};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, Sender};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

pub const WM_PEEK: u32 = 0x8000 + 22;

pub enum Peek {
    Img(Img),
    Lines(Vec<String>),
    Grid(Vec<Vec<String>>),
    Wave(Vec<u8>),
    /// Превью нет — рисуем значок типа.
    Icon,
}

pub struct Peeker {
    tx: Sender<PathBuf>,
    rx: Receiver<(PathBuf, Peek)>,
    cache: HashMap<PathBuf, Rc<Peek>>,
    pending: HashSet<PathBuf>,
}

impl Peeker {
    pub fn new(hwnd: HWND) -> Peeker {
        let (tx, wrx) = channel::<PathBuf>();
        let (wtx, rx) = channel();
        let h = hwnd.0 as isize;
        std::thread::spawn(move || {
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
            }
            while let Ok(p) = wrx.recv() {
                // берём самый свежий запрос — старые уже не нужны
                let mut todo = vec![p];
                while let Ok(q) = wrx.try_recv() {
                    todo.push(q);
                }
                for p in todo.into_iter().rev() {
                    let pk = std::panic::catch_unwind(|| make(&p)).unwrap_or(Peek::Icon);
                    if wtx.send((p, pk)).is_err() {
                        return;
                    }
                    unsafe {
                        let _ = PostMessageW(HWND(h as *mut _), WM_PEEK, WPARAM(0), LPARAM(0));
                    }
                }
            }
        });
        Peeker { tx, rx, cache: HashMap::new(), pending: HashSet::new() }
    }

    pub fn get(&mut self, p: &Path) -> Option<Rc<Peek>> {
        if let Some(v) = self.cache.get(p) {
            return Some(v.clone());
        }
        if self.pending.insert(p.to_path_buf()) {
            let _ = self.tx.send(p.to_path_buf());
        }
        None
    }

    /// Забрать готовые превью. true — что-то пришло.
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        while let Ok((p, pk)) = self.rx.try_recv() {
            self.pending.remove(&p);
            if self.cache.len() > 60 {
                self.cache.clear();
            }
            self.cache.insert(p, Rc::new(pk));
            any = true;
        }
        any
    }

    pub fn forget(&mut self, p: &Path) {
        self.cache.remove(p);
    }
}

fn text_lines(bytes: &[u8], n: usize) -> Vec<String> {
    let cut = &bytes[..bytes.len().min(16384)];
    let (s, _) = crate::textenc::decode(cut);
    s.lines().take(n).map(|l| l.replace('\t', "    ").chars().take(100).collect()).collect()
}

fn read_head(p: &Path, n: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).ok()?;
    let mut b = vec![0u8; n];
    let k = f.read(&mut b).ok()?;
    b.truncate(k);
    Some(b)
}

fn make(p: &Path) -> Peek {
    let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    match kinds::kind_of(p) {
        Some(Kind::Image) => crate::codec::load(p).map(|i| Peek::Img(crate::codec::thumb(&i, 360))).unwrap_or(Peek::Icon),
        Some(Kind::Pdf) => crate::pdf_read::render(p, 0, 300).map(Peek::Img).unwrap_or(Peek::Icon),
        Some(Kind::Video) => crate::media::video_thumb(p, 360).map(Peek::Img).unwrap_or(Peek::Icon),
        Some(Kind::Audio) if size < 80_000_000 => crate::media::audio_peaks(p, 160).map(Peek::Wave).unwrap_or(Peek::Icon),
        Some(Kind::Text) => read_head(p, 16384).map(|b| Peek::Lines(text_lines(&b, 48))).unwrap_or(Peek::Icon),
        Some(Kind::Doc) => {
            let e = crate::util::ext_lower(p);
            if e == "md" || e == "markdown" {
                read_head(p, 16384).map(|b| Peek::Lines(text_lines(&b, 48))).unwrap_or(Peek::Icon)
            } else {
                Peek::Icon
            }
        }
        Some(Kind::Table) if size < 30_000_000 => match crate::table::load(p) {
            Ok(sheets) if !sheets.is_empty() => {
                let s = &sheets[0];
                Peek::Grid(s.rows.iter().take(40).map(|r| r.iter().take(10).map(|c| c.text().chars().take(24).collect()).collect()).collect())
            }
            _ => Peek::Icon,
        },
        _ => Peek::Icon,
    }
}

pub fn kind_icon(k: Option<Kind>) -> u32 {
    match k {
        Some(Kind::Image) => gui::IC_PICTURE,
        Some(Kind::Pdf) | Some(Kind::Doc) => gui::IC_DOC,
        Some(Kind::Table) => gui::IC_GRID,
        Some(Kind::Text) => gui::IC_DOC,
        Some(Kind::Audio) => 0xE8D6, // Audio (нота)
        Some(Kind::Video) => 0xE714, // Video
        None => gui::IC_DOC,
    }
}

/// Соотношение сторон карточки превью (ширина / высота).
pub fn aspect(pk: Option<&Peek>, k: Option<Kind>) -> f64 {
    match pk {
        Some(Peek::Img(i)) => (i.w as f64 / i.h.max(1) as f64).clamp(0.3, 3.0),
        Some(Peek::Grid(_)) => 1.35,
        Some(Peek::Wave(_)) => 1.6,
        _ => match k {
            Some(Kind::Video) => 16.0 / 9.0,
            Some(Kind::Audio) => 1.6,
            Some(Kind::Table) => 1.35,
            _ => 0.72,
        },
    }
}

/// Нарисовать карточку превью в прямоугольнике card.
pub fn draw(ui: &Ui, hdc: HDC, pk: Option<&Peek>, k: Option<Kind>, card: &RECT) {
    let t = &ui.t;
    let paper = if t.dark { crate::util::rgb(38, 38, 40) } else { crate::util::rgb(255, 255, 255) };
    match pk {
        Some(Peek::Img(img)) => {
            gui::fill(hdc, &RECT { left: card.left + 2, top: card.top + 2, right: card.right + 2, bottom: card.bottom + 2 }, gui::blend(t.canvas, crate::util::rgb(0, 0, 0), 0.7));
            gui::blit(hdc, img, card, true);
        }
        Some(Peek::Lines(lines)) => {
            gui::round(hdc, card, ui.s(6), Some(paper), Some(t.border));
            let lh = ui.s(9).max(6);
            let font = small_font(lh);
            let mut y = card.top + ui.s(8);
            for l in lines {
                if y + lh > card.bottom - ui.s(4) {
                    break;
                }
                let indent = l.chars().take_while(|c| *c == ' ').count() as i32;
                let rc = RECT { left: card.left + ui.s(8) + indent * lh / 3, top: y, right: card.right - ui.s(4), bottom: y + lh + 2 };
                gui::text(hdc, l.trim_start(), &rc, font, t.dim, gui::LEFT);
                y += lh + 1;
            }
            unsafe {
                let _ = DeleteObject(font);
            }
        }
        Some(Peek::Grid(rows)) => {
            gui::fill(hdc, card, paper);
            let rh = ui.s(14);
            let cw = ui.s(52);
            let font = small_font(ui.s(9).max(6));
            let grid = gui::blend(t.border, paper, 0.8);
            let head = gui::blend(t.bg, paper, 0.5);
            gui::fill(hdc, &RECT { bottom: card.top + rh, ..*card }, head);
            let mut y = card.top;
            for r in rows {
                if y > card.bottom {
                    break;
                }
                let mut x = card.left;
                for c in r {
                    if x > card.right {
                        break;
                    }
                    let rc = RECT { left: x + 3, top: y, right: (x + cw - 2).min(card.right), bottom: (y + rh).min(card.bottom) };
                    gui::text(hdc, c, &rc, font, t.dim, gui::LEFT);
                    x += cw;
                    gui::line(hdc, x, card.top, x, card.bottom, grid, 1, PS_SOLID);
                }
                y += rh;
                gui::line(hdc, card.left, y, card.right, y, grid, 1, PS_SOLID);
            }
            gui::frame(hdc, card, t.border, PS_SOLID);
            unsafe {
                let _ = DeleteObject(font);
            }
        }
        Some(Peek::Wave(w)) => {
            gui::round(hdc, card, ui.s(8), Some(paper), Some(t.border));
            let inner = RECT { left: card.left + ui.s(8), right: card.right - ui.s(8), top: card.top + ui.s(10), bottom: card.bottom - ui.s(10) };
            crate::media::draw_wave(hdc, Some(w), &inner, 0.0, t.accent, gui::blend(t.accent, paper, 0.55));
        }
        _ => {
            gui::round(hdc, card, ui.s(8), Some(paper), Some(t.border));
            let ic = kind_icon(k);
            gui::text(hdc, &char::from_u32(ic).map(|c| c.to_string()).unwrap_or_default(), card, ui.f.icon_big, t.dim, gui::CENTER);
            if matches!(k, Some(Kind::Doc) | Some(Kind::Pdf)) {
                // строки «текста» — силуэт страницы
                let mut y = card.top + ui.s(14);
                let mut i = 0;
                while y < card.top + (card.bottom - card.top) / 2 - ui.s(18) {
                    let w = (card.right - card.left - ui.s(24)) * if i % 4 == 3 { 3 } else { 5 } / 5;
                    gui::fill(hdc, &gui::r(card.left + ui.s(12), y, w, ui.s(3)), gui::blend(t.dim, paper, 0.35));
                    y += ui.s(9);
                    i += 1;
                }
            }
        }
    }
}

fn small_font(px: i32) -> HFONT {
    let face = crate::util::wide("Consolas");
    unsafe {
        CreateFontW(-px, 0, 0, 0, 400, 0, 0, 0, DEFAULT_CHARSET.0 as u32, 0, 0, CLEARTYPE_QUALITY.0 as u32, 0, windows::core::PCWSTR(face.as_ptr()))
    }
}
