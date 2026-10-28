// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Главное окно: шапка с вкладками (лента), холст, нижняя панель быстрых
//! инструментов. Документ — список страниц (фото и/или страницы PDF).

use crate::doc::{self, Page, Src};
use crate::gui::{self, Ev, Mouse, Panel, Ui, W};
use crate::image_buf::Img;
use crate::theme::{self, Mode};
use crate::util::{file_name, is_pdf};
use crate::{clip, codec, files, settings};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Shell::DragAcceptFiles;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const APP_NAME: &str = "SimplePhotoViewer";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const HISTORY: usize = 10;
const TIMER_HQ: usize = 1;
const TIMER_STATUS: usize = 2;
const TIMER_START: usize = 3;

// ---- команды
const ID_OPEN: u32 = 101;
const ID_SAVE: u32 = 102;
const ID_EXIT: u32 = 103;
const ID_SAVE_PDF: u32 = 104;
const ID_COPY: u32 = 105;
const ID_PASTE: u32 = 106;
const ID_UNDO: u32 = 110;
const ID_ROT_L: u32 = 111;
const ID_ROT_R: u32 = 112;
const ID_FLIP: u32 = 113;
const ID_CROP: u32 = 114;
const ID_FLIP_V: u32 = 115;
const ID_BR_UP: u32 = 120;
const ID_BR_DN: u32 = 121;
const ID_CO_UP: u32 = 122;
const ID_CO_DN: u32 = 123;
const ID_SA_UP: u32 = 124;
const ID_SA_DN: u32 = 125;
const ID_GRAY: u32 = 126;
const ID_RS_50: u32 = 130;
const ID_RS_1920: u32 = 131;
const ID_PREV: u32 = 140;
const ID_NEXT: u32 = 141;
const ID_FIT: u32 = 142;
const ID_100: u32 = 143;
const ID_ZIN: u32 = 144;
const ID_ZOUT: u32 = 145;
const ID_FULL: u32 = 146;
const ID_PRINT: u32 = 150;
const ID_KEYS: u32 = 160;
const ID_ABOUT: u32 = 161;
const ID_INSERT: u32 = 170;
const ID_DEL_PAGE: u32 = 171;
const ID_MOVE_L: u32 = 172;
const ID_MOVE_R: u32 = 173;
const ID_COLLAGE: u32 = 180;
const ID_MERGE: u32 = 181;
const ID_THEME_SYS: u32 = 190;
const ID_THEME_LIGHT: u32 = 191;
const ID_THEME_DARK: u32 = 192;
const ID_THEME_CYCLE: u32 = 193;
const ID_TAB0: u32 = 200;

const TABS: [&str; 5] = ["Главная", "Правка", "Страницы", "Создать", "Вид"];

enum R {
    B(u32, u32, &'static str, &'static str),
    S,
}

fn ribbon(tab: usize) -> Vec<R> {
    use crate::gui::*;
    use R::*;
    match tab {
        0 => vec![
            B(ID_OPEN, IC_OPEN, "Открыть", "Открыть фото или PDF (Ctrl+O)"),
            B(ID_SAVE, IC_SAVE, "Сохранить", "Сохранить как… (Ctrl+S)"),
            B(ID_SAVE_PDF, IC_DOC, "В PDF", "Сохранить все страницы в PDF (Ctrl+Shift+S)"),
            B(ID_PRINT, IC_PRINT, "Печать", "Печать с предпросмотром (Ctrl+P)"),
            S,
            B(ID_UNDO, IC_UNDO, "Отменить", "Отменить (Ctrl+Z)"),
            B(ID_ROT_L, IC_ROT_L, "Влево", "Повернуть влево (Ctrl+L)"),
            B(ID_ROT_R, IC_ROT_R, "Вправо", "Повернуть вправо (Ctrl+R)"),
            B(ID_FLIP, IC_FLIP, "Отразить", "Отразить по горизонтали (Ctrl+H)"),
            B(ID_CROP, IC_CROP, "Обрезать", "Обрезка: выделите область, Enter — применить (Ctrl+K)"),
            S,
            B(ID_COPY, IC_COPY, "Копировать", "Копировать картинку (Ctrl+C)"),
            B(ID_PASTE, IC_PASTE, "Вставить", "Вставить картинку или файлы из буфера (Ctrl+V)"),
        ],
        1 => vec![
            B(ID_BR_UP, IC_SUN, "Ярче", "Яркость +10%"),
            B(ID_BR_DN, IC_SUN, "Темнее", "Яркость −10%"),
            S,
            B(ID_CO_UP, IC_CONTRAST, "Контраст +", "Контраст +10%"),
            B(ID_CO_DN, IC_CONTRAST, "Контраст −", "Контраст −10%"),
            S,
            B(ID_SA_UP, IC_COLOR, "Насыщ. +", "Насыщенность +20%"),
            B(ID_SA_DN, IC_COLOR, "Насыщ. −", "Насыщенность −20%"),
            B(ID_GRAY, IC_COLOR, "Ч/Б", "Сделать чёрно-белым"),
            S,
            B(ID_RS_50, IC_PICTURE, "Размер ½", "Уменьшить вдвое"),
            B(ID_RS_1920, IC_FULLSCREEN, "1920 px", "1920 точек по ширине"),
            S,
            B(ID_FLIP_V, IC_FLIP, "Отразить ↕", "Отразить по вертикали"),
            B(ID_UNDO, IC_UNDO, "Отменить", "Отменить (Ctrl+Z)"),
        ],
        2 => vec![
            B(ID_INSERT, IC_ADD, "Вставить", "Вставить фото или страницы PDF после текущей (Ctrl+I)"),
            B(ID_DEL_PAGE, IC_DELETE, "Удалить", "Удалить текущую страницу (Delete)"),
            B(ID_MOVE_L, IC_BACK, "Раньше", "Переместить страницу назад"),
            B(ID_MOVE_R, IC_FORWARD, "Позже", "Переместить страницу вперёд"),
            S,
            B(ID_ROT_L, IC_ROT_L, "Влево", "Повернуть страницу влево"),
            B(ID_ROT_R, IC_ROT_R, "Вправо", "Повернуть страницу вправо"),
            S,
            B(ID_SAVE_PDF, IC_DOC, "Сохранить PDF", "Сохранить все страницы в PDF (Ctrl+Shift+S)"),
            B(ID_MERGE, IC_MERGE, "Объединить", "Объединить файлы в один PDF (Ctrl+M)"),
        ],
        3 => vec![
            B(ID_COLLAGE, IC_GRID, "Коллаж", "Склеить несколько фото в одно (Ctrl+G)"),
            B(ID_MERGE, IC_MERGE, "Объединить в PDF", "Фото и PDF → один PDF (Ctrl+M)"),
            S,
            B(ID_PRINT, IC_PRINT, "Печать", "Печать с предпросмотром (Ctrl+P)"),
        ],
        _ => vec![
            B(ID_PREV, IC_PREV, "Назад", "Предыдущее фото / страница (←)"),
            B(ID_NEXT, IC_NEXT, "Вперёд", "Следующее фото / страница (→)"),
            S,
            B(ID_ZOUT, IC_ZOOM_OUT, "Меньше", "Уменьшить (Ctrl+−)"),
            B(ID_FIT, IC_FIT, "Вписать", "Вписать в окно (Ctrl+0)"),
            B(ID_100, IC_PICTURE, "100%", "Реальный размер (Ctrl+1)"),
            B(ID_ZIN, IC_ZOOM_IN, "Больше", "Увеличить (Ctrl++)"),
            B(ID_FULL, IC_FULLSCREEN, "Весь экран", "Во весь экран (F11)"),
            S,
            B(ID_THEME_SYS, IC_MONITOR, "Как в системе", "Тема как в Windows"),
            B(ID_THEME_LIGHT, IC_SUN, "Светлая", "Светлая тема"),
            B(ID_THEME_DARK, IC_MOON, "Тёмная", "Тёмная тема"),
            S,
            B(ID_KEYS, IC_KEYBOARD, "Клавиши", "Горячие клавиши (F1)"),
            B(ID_ABOUT, IC_INFO, "О программе", "О программе"),
        ],
    }
}

// ---------------------------------------------------------------- состояние ---
pub struct App {
    hwnd: HWND,
    start: Vec<PathBuf>,
    pages: Vec<Page>,
    cur: usize,
    doc_name: String,
    doc_path: Option<PathBuf>,
    files: Vec<PathBuf>,
    idx: isize,
    history: Vec<(Vec<Page>, usize)>,
    dirty: bool,
    view: Option<Rc<Img>>,
    view_err: Option<String>,
    zoom: f64, // относительно «вписать в окно»
    off: (f64, f64),
    canvas: RECT,
    crop_mode: bool,
    crop: Option<(f64, f64, f64, f64)>,
    drag: Option<(f64, f64)>,
    hq: bool,
    tab: usize,
    full: Option<(WINDOWPLACEMENT, isize)>,
    status: String,
}

impl App {
    fn new(start: Vec<PathBuf>) -> App {
        App {
            hwnd: HWND::default(), start, pages: vec![], cur: 0, doc_name: String::new(), doc_path: None,
            files: vec![], idx: -1, history: vec![], dirty: false, view: None, view_err: None,
            zoom: 1.0, off: (0.0, 0.0), canvas: RECT::default(), crop_mode: false, crop: None, drag: None,
            hq: true, tab: settings::get_i("tab", 0).clamp(0, 4) as usize, full: None, status: String::new(),
        }
    }

    fn cw(&self) -> f64 {
        (self.canvas.right - self.canvas.left).max(1) as f64
    }
    fn ch(&self) -> f64 {
        (self.canvas.bottom - self.canvas.top).max(1) as f64
    }
    fn fit(&self) -> f64 {
        match &self.view {
            Some(i) => {
                let pad = if self.full.is_some() { 0.0 } else { 16.0 };
                let f = ((self.cw() - pad) / i.w as f64).min((self.ch() - pad) / i.h as f64).max(0.001);
                // документы вписываем всегда, мелкие фото не растягиваем
                if self.pages.get(self.cur).map(|p| p.is_pdf()).unwrap_or(false) { f } else { f.min(1.0) }
            }
            None => 1.0,
        }
    }
    fn scale(&self) -> f64 {
        self.fit() * self.zoom
    }
    /// Положение картинки в координатах холста.
    fn dest(&self) -> (f64, f64, f64, f64) {
        let i = self.view.as_ref().unwrap();
        let s = self.scale();
        let (w, h) = (i.w as f64 * s, i.h as f64 * s);
        ((self.cw() - w) / 2.0 + self.off.0, (self.ch() - h) / 2.0 + self.off.1, w, h)
    }
    fn reset_view(&mut self) {
        self.zoom = 1.0;
        self.off = (0.0, 0.0);
    }
    fn actual_size(&mut self) {
        let f = self.fit();
        self.zoom = if f > 0.0 { 1.0 / f } else { 1.0 };
        self.off = (0.0, 0.0);
    }
    /// Масштаб относительно точки (mx, my) холста — она остаётся на месте.
    fn zoom_at(&mut self, k: f64, mx: f64, my: f64) {
        if self.view.is_none() {
            return;
        }
        let (dx, dy, _, _) = self.dest();
        let s = self.scale();
        let (u, v) = ((mx - dx) / s, (my - dy) / s);
        self.zoom = (self.zoom * k).clamp(0.05, 40.0);
        let s2 = self.scale();
        let i = self.view.as_ref().unwrap();
        let (w2, h2) = (i.w as f64 * s2, i.h as f64 * s2);
        self.off = (mx - u * s2 - (self.cw() - w2) / 2.0, my - v * s2 - (self.ch() - h2) / 2.0);
        if self.zoom <= 1.0 + 1e-9 {
            self.off = (0.0, 0.0);
        }
    }
    fn zoom_center(&mut self, k: f64) {
        let (cx, cy) = (self.cw() / 2.0, self.ch() / 2.0);
        self.zoom_at(k, cx, cy);
    }

    fn crop_box(&self) -> Option<(u32, u32, u32, u32)> {
        let img = self.view.as_ref()?;
        let (a, b, c, d) = self.crop?;
        let (l, r) = (a.min(c), a.max(c));
        let (t, bt) = (b.min(d), b.max(d));
        let (dx, dy, _, _) = self.dest();
        let s = self.scale();
        let x0 = ((l - dx) / s).max(0.0);
        let y0 = ((t - dy) / s).max(0.0);
        let x1 = ((r - dx) / s).min(img.w as f64);
        let y1 = ((bt - dy) / s).min(img.h as f64);
        if x1 - x0 < 2.0 || y1 - y0 < 2.0 {
            return None;
        }
        Some((x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
    }

    fn title(&self) -> String {
        if self.pages.is_empty() {
            return format!("{APP_NAME} {VERSION}");
        }
        let star = if self.dirty { "*" } else { "" };
        let hint = if self.crop_mode { " — ОБРЕЗКА: выделите область, Enter — применить, Esc — отмена" } else { "" };
        format!("{star}{} — {APP_NAME}{hint}", self.doc_name)
    }

    fn info(&self) -> String {
        let mut parts = vec![];
        if let Some(v) = &self.view {
            parts.push(format!("{} × {} px", v.w, v.h));
            parts.push(format!("{}%", (self.scale() * 100.0).round() as i64));
        }
        if self.pages.len() > 1 {
            parts.push(format!("стр. {} / {}", self.cur + 1, self.pages.len()));
        } else if self.idx >= 0 && self.files.len() > 1 {
            parts.push(format!("файл {} / {}", self.idx + 1, self.files.len()));
        }
        parts.join("   ·   ")
    }

    fn set_status(&mut self, s: impl Into<String>) {
        self.status = s.into();
        unsafe {
            SetTimer(self.hwnd, TIMER_STATUS, 4000, None);
        }
    }

    fn msg(&self, text: &str, error: bool) {
        message(Some(self.hwnd), text, error);
    }

    // ------------------------------------------------------------ документ ---
    /// Показать текущую страницу (загрузить/нарисовать при необходимости).
    fn refresh_view(&mut self) {
        self.crop = None;
        self.crop_mode = false;
        self.reset_view();
        self.view = None;
        self.view_err = None;
        let Some(p) = self.pages.get(self.cur) else { return };
        match doc::render(p, doc::VIEW_DPI) {
            Ok(i) => self.view = Some(i),
            Err(e) => self.view_err = Some(e),
        }
    }

    fn confirm_discard(&mut self) -> bool {
        if !self.dirty {
            return true;
        }
        let r = unsafe {
            MessageBoxW(self.hwnd, w!("Есть несохранённые изменения. Сохранить?"), &HSTRING::from(APP_NAME), MB_YESNOCANCEL | MB_ICONQUESTION)
        };
        match r {
            IDYES => self.save_as(false),
            IDNO => true,
            _ => false,
        }
    }

    fn set_doc(&mut self, pages: Vec<Page>, name: String, path: Option<PathBuf>, dirty: bool) {
        self.pages = pages;
        self.cur = 0;
        self.doc_name = name;
        self.history.clear();
        self.dirty = dirty;
        match &path {
            Some(p) => {
                let (list, idx) = files::siblings(p);
                self.files = list;
                self.idx = idx;
            }
            None => {
                self.files.clear();
                self.idx = -1;
            }
        }
        self.doc_path = path;
        self.refresh_view();
    }

    /// Открыть файлы (без вопроса о несохранённом).
    fn load_paths(&mut self, paths: &[PathBuf]) {
        let paths: Vec<PathBuf> = paths.iter().filter(|p| p.is_file()).map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone())).collect();
        if paths.is_empty() {
            return;
        }
        let (pages, errs) = doc::pages_from_all(&paths);
        if !errs.is_empty() {
            self.msg(&format!("Не удалось открыть:\n\n{}", errs.join("\n\n")), true);
        }
        if pages.is_empty() {
            return;
        }
        if paths.len() == 1 {
            let n = pages.len();
            let name = if n > 1 { format!("{} ({} стр.)", file_name(&paths[0]), n) } else { file_name(&paths[0]) };
            self.set_doc(pages, name, Some(paths[0].clone()), false);
        } else {
            let name = format!("Документ ({} стр.)", pages.len());
            self.set_doc(pages, name, None, false);
        }
        if let Some(d) = paths[0].parent() {
            settings::set_s("last_dir", &d.to_string_lossy());
        }
    }

    fn open(&mut self) {
        if !self.confirm_discard() {
            return;
        }
        let p = files::open_dialog(self.hwnd, files::Pick::Any, true);
        self.load_paths(&p);
    }

    fn default_dir(&self) -> Option<PathBuf> {
        self.doc_path
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .or_else(|| self.pages.iter().find_map(|p| p.path().and_then(|x| x.parent()).map(|d| d.to_path_buf())))
            .or_else(|| settings::get_s("last_dir").map(PathBuf::from))
    }

    fn stem(&self) -> String {
        let p = self.doc_path.clone().or_else(|| self.pages.get(self.cur).and_then(|p| p.path().map(|x| x.to_path_buf())));
        p.and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_else(|| if self.doc_name.is_empty() { "документ".into() } else { self.doc_name.clone() })
    }

    /// «Сохранить как». pdf — сразу предложить PDF.
    fn save_as(&mut self, pdf: bool) -> bool {
        if self.pages.is_empty() {
            return false;
        }
        let multi = self.pages.len() > 1 || self.pages.iter().any(|p| p.is_pdf());
        let prefer_pdf = pdf || multi;
        let name = format!("{}{}", self.stem(), if prefer_pdf { ".pdf" } else { "_edit.jpg" });
        let dir = self.default_dir();
        let Some(path) = files::save_dialog(self.hwnd, &name, dir.as_deref(), prefer_pdf, pdf) else { return false };
        if is_pdf(&path) {
            let res = doc::save_pdf(&self.pages, &path);
            match res {
                Ok(()) => {
                    self.dirty = false;
                    self.set_status(format!("Сохранено: {}", file_name(&path)));
                    // если перезаписали исходный PDF — откроем заново (номера страниц изменились)
                    let reopen = self.pages.iter().any(|p| matches!(&p.src, Src::Pdf { path: sp, .. } if same_file(sp, &path)));
                    if reopen {
                        let cur = self.cur;
                        if let Ok(pages) = doc::pages_from(&path) {
                            let n = pages.len();
                            self.set_doc(pages, format!("{} ({} стр.)", file_name(&path), n), Some(path.clone()), false);
                            self.cur = cur.min(n.saturating_sub(1));
                            self.refresh_view();
                        }
                    }
                    true
                }
                Err(e) => {
                    self.msg(&format!("Не удалось сохранить PDF:\n{e}"), true);
                    false
                }
            }
        } else {
            let Some(img) = self.view.clone() else { return false };
            match codec::save(&img, &path) {
                Ok(()) => {
                    // для одиночного фото сохранение «закрывает» правки
                    if self.pages.len() == 1 {
                        self.dirty = false;
                    }
                    self.set_status(format!("Сохранено: {}", file_name(&path)));
                    true
                }
                Err(e) => {
                    self.msg(&format!("Не удалось сохранить файл:\n{}", e.message()), true);
                    false
                }
            }
        }
    }

    fn step(&mut self, d: isize) {
        if self.pages.len() > 1 {
            let n = self.cur as isize + d;
            if n >= 0 && (n as usize) < self.pages.len() {
                self.cur = n as usize;
                self.refresh_view();
            }
            return;
        }
        let n = self.files.len() as isize;
        if n < 2 || self.idx < 0 || !self.confirm_discard() {
            return;
        }
        let p = self.files[(self.idx + d).rem_euclid(n) as usize].clone();
        self.load_paths(&[p]);
    }

    fn push_history(&mut self) {
        self.history.push((self.pages.clone(), self.cur));
        if self.history.len() > HISTORY {
            self.history.remove(0);
        }
        self.dirty = true;
    }

    fn undo(&mut self) {
        if let Some((pages, cur)) = self.history.pop() {
            self.pages = pages;
            self.cur = cur.min(self.pages.len().saturating_sub(1));
            self.dirty = !self.history.is_empty();
            self.refresh_view();
        }
    }

    /// Правка картинки текущей страницы. k_px — во сколько раз меняется число точек по ширине.
    fn edit(&mut self, f: impl FnOnce(&Img) -> Option<Img>) {
        let Some(v) = self.view.clone() else { return };
        let Some(new) = f(&v) else { return };
        self.push_history();
        let p = &mut self.pages[self.cur];
        // сколько пунктов листа приходится на точку — чтобы страница PDF не поменяла размер
        let ppx = p.ppx.or_else(|| p.size_pt().map(|s| s.0 / v.w as f64));
        let ppx = ppx.map(|k| if new.w != v.w && new.h != v.h && (new.w as f64 / v.w as f64 - new.h as f64 / v.h as f64).abs() < 0.01 {
            // масштабирование: лист тот же, точек меньше/больше
            k * v.w as f64 / new.w as f64
        } else {
            k
        });
        p.img = Some(Rc::new(new));
        p.rot = 0;
        p.ppx = ppx;
        self.view = p.img.clone();
        self.crop = None;
        self.crop_mode = false;
        self.reset_view();
    }

    fn rotate(&mut self, cw: bool) {
        if self.pages.get(self.cur).map(|p| p.is_vector()).unwrap_or(false) {
            // страница PDF остаётся векторной: запоминаем поворот
            self.push_history();
            let p = &mut self.pages[self.cur];
            p.rot = (p.rot + if cw { 1 } else { 3 }) % 4;
            if let Some(v) = self.view.take() {
                self.view = Some(Rc::new(if cw { v.rotate_cw() } else { v.rotate_ccw() }));
            }
            self.crop_mode = false;
            self.reset_view();
            self.set_status(format!("Страница {} повёрнута", self.cur + 1));
        } else {
            self.edit(|i| Some(if cw { i.rotate_cw() } else { i.rotate_ccw() }));
        }
    }

    fn insert_pages(&mut self, pages: Vec<Page>) {
        if pages.is_empty() {
            return;
        }
        if self.pages.is_empty() {
            let n = pages.len();
            let name = if n == 1 { pages[0].label() } else { format!("Документ ({n} стр.)") };
            self.set_doc(pages, name, None, true);
            return;
        }
        self.push_history();
        let at = self.cur + 1;
        let n = pages.len();
        for (k, p) in pages.into_iter().enumerate() {
            self.pages.insert(at + k, p);
        }
        self.cur = at;
        if self.doc_path.is_some() && !self.doc_name.contains("стр.") || self.pages.len() > 1 {
            self.doc_name = format!("{} ({} стр.)", self.doc_path.as_ref().map(|p| file_name(p)).unwrap_or_else(|| "Документ".into()), self.pages.len());
        }
        self.refresh_view();
        self.set_status(format!("Вставлено страниц: {n}"));
    }

    fn insert_dialog(&mut self) {
        let p = files::open_dialog(self.hwnd, files::Pick::Any, true);
        if p.is_empty() {
            return;
        }
        let (pages, errs) = doc::pages_from_all(&p);
        if !errs.is_empty() {
            self.msg(&format!("Не удалось добавить:\n\n{}", errs.join("\n\n")), true);
        }
        self.insert_pages(pages);
    }

    fn paste(&mut self) {
        if let Some(img) = clip::image(self.hwnd) {
            let n = self.pages.iter().filter(|p| matches!(p.src, Src::Mem(_))).count() + 1;
            self.insert_pages(vec![Page::mem(&format!("Из буфера {n}"), img)]);
            return;
        }
        let fl: Vec<PathBuf> = clip::files(self.hwnd).into_iter().filter(|p| files::is_supported(p)).collect();
        if fl.is_empty() {
            self.set_status("В буфере обмена нет картинки или файлов");
            return;
        }
        let (pages, errs) = doc::pages_from_all(&fl);
        if !errs.is_empty() {
            self.msg(&format!("Не удалось добавить:\n\n{}", errs.join("\n\n")), true);
        }
        self.insert_pages(pages);
    }

    fn delete_page(&mut self) {
        if self.pages.len() < 2 {
            self.set_status("В документе одна страница — удалять нечего");
            return;
        }
        self.push_history();
        self.pages.remove(self.cur);
        self.cur = self.cur.min(self.pages.len() - 1);
        self.refresh_view();
    }

    fn move_page(&mut self, d: isize) {
        let n = self.cur as isize + d;
        if self.pages.len() < 2 || n < 0 || n as usize >= self.pages.len() {
            return;
        }
        self.push_history();
        self.pages.swap(self.cur, n as usize);
        self.cur = n as usize;
        self.set_status(format!("Страница перемещена на место {}", self.cur + 1));
    }

    fn toggle_crop(&mut self) {
        if self.view.is_none() {
            return;
        }
        if self.crop_mode && self.crop.is_some() {
            return self.apply_crop();
        }
        self.crop_mode = !self.crop_mode;
        self.crop = None;
    }

    fn apply_crop(&mut self) {
        match if self.crop_mode { self.crop_box() } else { None } {
            Some((x, y, w, h)) => self.edit(|i| Some(i.crop(x, y, w, h))),
            None => {
                self.crop_mode = false;
                self.crop = None;
            }
        }
    }

    fn print(&mut self) {
        if self.pages.is_empty() {
            return;
        }
        let name = self.stem();
        if let Some(s) = crate::print_dlg::run(self.hwnd, self.pages.clone(), self.cur, self.view.clone(), &name) {
            self.set_status(s);
        }
    }

    fn collage(&mut self) {
        let items: Vec<crate::collage::Item> = self
            .pages
            .iter()
            .filter(|p| !p.is_pdf() || p.img.is_some())
            .map(|p| crate::collage::Item { label: p.label(), path: p.path().map(|x| x.to_path_buf()), img: p.img.clone() })
            .collect();
        if let Some(img) = crate::collage::run(self.hwnd, items) {
            if !self.confirm_discard() {
                return;
            }
            self.set_doc(vec![Page::mem("Коллаж", img)], "Коллаж".into(), None, true);
        }
    }

    fn merge(&mut self) {
        let init: Vec<PathBuf> = match &self.doc_path {
            Some(p) if self.pages.len() == 1 || is_pdf(p) => vec![p.clone()],
            _ => {
                let mut v: Vec<PathBuf> = vec![];
                for p in &self.pages {
                    if let Some(x) = p.path() {
                        if !v.iter().any(|y| y == x) {
                            v.push(x.to_path_buf());
                        }
                    }
                }
                v
            }
        };
        match crate::merge::run(self.hwnd, init) {
            crate::merge::Outcome::Saved(p) => self.set_status(format!("Сохранено: {}", file_name(&p))),
            crate::merge::Outcome::Open(paths) => {
                if self.confirm_discard() {
                    let (pages, errs) = doc::pages_from_all(&paths);
                    if !errs.is_empty() {
                        self.msg(&format!("Не удалось открыть:\n\n{}", errs.join("\n\n")), true);
                    }
                    if !pages.is_empty() {
                        let n = pages.len();
                        self.set_doc(pages, format!("Документ ({n} стр.)"), None, true);
                    }
                }
            }
            crate::merge::Outcome::None => {}
        }
    }

    fn toggle_full(&mut self) {
        let hwnd = self.hwnd;
        unsafe {
            match self.full.take() {
                Some((wp, style)) => {
                    SetWindowLongPtrW(hwnd, GWL_STYLE, style);
                    let _ = SetWindowPlacement(hwnd, &wp);
                    let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED);
                }
                None => {
                    let mut wp = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
                    let _ = GetWindowPlacement(hwnd, &mut wp);
                    let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
                    let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
                    let _ = GetMonitorInfoW(mon, &mut mi);
                    SetWindowLongPtrW(hwnd, GWL_STYLE, style & !(WS_OVERLAPPEDWINDOW.0 as isize));
                    let r = mi.rcMonitor;
                    let _ = SetWindowPos(hwnd, HWND_TOP, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_FRAMECHANGED);
                    self.full = Some((wp, style));
                }
            }
        }
        self.reset_view();
    }

    fn set_theme(&mut self, m: Mode) {
        theme::set_mode(m);
        gui::retheme_all();
        let name = match m { Mode::System => "как в системе", Mode::Light => "светлая", Mode::Dark => "тёмная" };
        self.set_status(format!("Тема: {name}"));
    }

    fn command(&mut self, id: u32) -> bool {
        match id {
            ID_OPEN => self.open(),
            ID_SAVE => {
                self.save_as(false);
            }
            ID_SAVE_PDF => {
                self.save_as(true);
            }
            ID_EXIT => unsafe {
                let _ = PostMessageW(self.hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            ID_COPY => {
                if let Some(v) = &self.view {
                    if clip::set_image(self.hwnd, v) {
                        self.set_status("Картинка скопирована");
                    }
                }
            }
            ID_PASTE => self.paste(),
            ID_UNDO => self.undo(),
            ID_ROT_L => self.rotate(false),
            ID_ROT_R => self.rotate(true),
            ID_FLIP => self.edit(|i| Some(i.flip_h())),
            ID_FLIP_V => self.edit(|i| Some(i.flip_v())),
            ID_CROP => self.toggle_crop(),
            ID_BR_UP => self.edit(|i| Some(i.adjust(1.1, 1.0, 1.0))),
            ID_BR_DN => self.edit(|i| Some(i.adjust(0.9, 1.0, 1.0))),
            ID_CO_UP => self.edit(|i| Some(i.adjust(1.0, 1.1, 1.0))),
            ID_CO_DN => self.edit(|i| Some(i.adjust(1.0, 0.9, 1.0))),
            ID_SA_UP => self.edit(|i| Some(i.adjust(1.0, 1.0, 1.2))),
            ID_SA_DN => self.edit(|i| Some(i.adjust(1.0, 1.0, 0.8))),
            ID_GRAY => self.edit(|i| Some(i.gray())),
            ID_RS_50 => self.edit(|i| codec::resize(i, (i.w / 2).max(1), (i.h / 2).max(1)).ok()),
            ID_RS_1920 => self.edit(|i| {
                if i.w == 1920 {
                    return None;
                }
                let nh = ((i.h as u64 * 1920) / i.w as u64).max(1) as u32;
                codec::resize(i, 1920, nh).ok()
            }),
            ID_PREV => self.step(-1),
            ID_NEXT => self.step(1),
            ID_FIT => self.reset_view(),
            ID_100 => self.actual_size(),
            ID_ZIN => self.zoom_center(1.25),
            ID_ZOUT => self.zoom_center(0.8),
            ID_FULL => self.toggle_full(),
            ID_PRINT => self.print(),
            ID_INSERT => {
                if self.pages.is_empty() {
                    self.open()
                } else {
                    self.insert_dialog()
                }
            }
            ID_DEL_PAGE => self.delete_page(),
            ID_MOVE_L => self.move_page(-1),
            ID_MOVE_R => self.move_page(1),
            ID_COLLAGE => self.collage(),
            ID_MERGE => self.merge(),
            ID_THEME_SYS => self.set_theme(Mode::System),
            ID_THEME_LIGHT => self.set_theme(Mode::Light),
            ID_THEME_DARK => self.set_theme(Mode::Dark),
            ID_THEME_CYCLE => {
                let next = if theme::cur().dark { Mode::Light } else { Mode::Dark };
                self.set_theme(next)
            }
            ID_KEYS => self.msg(KEYS, false),
            ID_ABOUT => self.msg(
                &format!(
                    "{APP_NAME} {VERSION}\n\nЛёгкий просмотрщик фото и PDF: правки, коллаж, склейка PDF, печать.\n\
                     © 2026 Даниил Кочубейник\nЛицензия: GNU GPL v3 или новее. Гарантий нет.\n\n\
                     Используется lopdf (MIT) для записи PDF."
                ),
                false,
            ),
            t if (ID_TAB0..ID_TAB0 + TABS.len() as u32).contains(&t) => {
                self.tab = (t - ID_TAB0) as usize;
                settings::set_i("tab", self.tab as i32);
            }
            _ => {}
        }
        true
    }
}

const KEYS: &str = "Ctrl+O — открыть (можно несколько файлов)\nCtrl+S — сохранить как\nCtrl+Shift+S — сохранить в PDF\n\
Ctrl+P — печать\nCtrl+C / Ctrl+V — копировать / вставить\n\n\
← / → — предыдущее / следующее фото или страница\nPage Up / Page Down, Home / End — страницы\n\
Ctrl+I — вставить фото/страницы\nDelete — удалить страницу\n\n\
Ctrl+L / Ctrl+R — повернуть влево / вправо\nCtrl+H — отразить\nCtrl+K — обрезка (Enter — применить, Esc — отмена)\n\
Ctrl+Z — отменить\n\nCtrl+G — коллаж\nCtrl+M — объединить в PDF\n\n\
Колесо мыши — масштаб, перетаскивание — сдвиг\nДвойной клик, Ctrl+0 — вписать; Ctrl+1 — 100%\nF11 — во весь экран";

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

// ------------------------------------------------------------------ панель ---
impl Panel for App {
    fn title(&self) -> String {
        App::title(self)
    }

    fn init(&mut self, hwnd: HWND) {
        self.hwnd = hwnd;
        unsafe {
            DragAcceptFiles(hwnd, true);
            SetTimer(hwnd, TIMER_START, 1, None);
        }
    }

    fn min_size(&self) -> (i32, i32) {
        (640, 420)
    }

    fn layout(&mut self, ui: &Ui, w: i32, h: i32) -> Vec<W> {
        let mut v = vec![];
        if self.full.is_some() {
            self.canvas = gui::r(0, 0, w, h);
            return v;
        }
        let tab_h = ui.s(38);
        let rib_h = ui.s(78);
        let bot_h = ui.s(50);
        let top = tab_h + rib_h + 1;
        self.canvas = RECT { left: 0, top, right: w, bottom: (h - bot_h).max(top + 1) };
        let has = self.view.is_some();
        let multi = self.pages.len() > 1;
        let can_rot = has || self.pages.get(self.cur).map(|p| p.is_vector()).unwrap_or(false);

        // вкладки
        let mut x = ui.s(8);
        for (i, t) in TABS.iter().enumerate() {
            let tw = ui.tw(ui.f.bold, t) + ui.s(28);
            v.push(W::new(Kind::Tab, ID_TAB0 + i as u32, gui::r(x, ui.s(4), tw, tab_h - ui.s(6))).t(*t).on(i == self.tab));
            x += tw + ui.s(2);
        }
        let b = ui.s(34);
        let mode_icon = match theme::mode() {
            Mode::System => gui::IC_MONITOR,
            Mode::Light => gui::IC_SUN,
            Mode::Dark => gui::IC_MOON,
        };
        v.push(W::new(Kind::Tool, ID_KEYS, gui::r(w - ui.s(8) - b, ui.s(3), b, b)).i(gui::IC_HELP).tip("Горячие клавиши (F1)"));
        v.push(W::new(Kind::Tool, ID_THEME_CYCLE, gui::r(w - ui.s(12) - 2 * b, ui.s(3), b, b)).i(mode_icon).tip("Сменить тему: светлая / тёмная"));

        // лента
        let mut x = ui.s(8);
        let y = tab_h + ui.s(4);
        let bh = rib_h - ui.s(8);
        for it in ribbon(self.tab) {
            match it {
                R::S => {
                    v.push(W::new(Kind::Sep, 0, gui::r(x + ui.s(3), y + ui.s(8), ui.s(2), bh - ui.s(16))));
                    x += ui.s(8);
                }
                R::B(id, icon, label, tip) => {
                    let bw = (ui.tw(ui.f.small, label) + ui.s(16)).max(ui.s(58));
                    let enabled = match id {
                        ID_OPEN | ID_KEYS | ID_ABOUT | ID_COLLAGE | ID_MERGE | ID_PASTE | ID_INSERT | ID_FULL => true,
                        ID_THEME_SYS | ID_THEME_LIGHT | ID_THEME_DARK => true,
                        ID_UNDO => !self.history.is_empty(),
                        ID_DEL_PAGE | ID_MOVE_L | ID_MOVE_R => multi,
                        ID_PREV | ID_NEXT => multi || self.files.len() > 1,
                        ID_SAVE_PDF | ID_PRINT => !self.pages.is_empty(),
                        ID_ROT_L | ID_ROT_R => can_rot,
                        _ => has,
                    };
                    let on = match id {
                        ID_THEME_SYS => theme::mode() == Mode::System,
                        ID_THEME_LIGHT => theme::mode() == Mode::Light,
                        ID_THEME_DARK => theme::mode() == Mode::Dark,
                        ID_CROP => self.crop_mode,
                        ID_FULL => self.full.is_some(),
                        _ => false,
                    };
                    v.push(W::new(Kind::Ribbon, id, gui::r(x, y, bw, bh)).i(icon).t(label).tip(tip).en(enabled).on(on));
                    x += bw + ui.s(2);
                }
            }
        }

        // нижняя панель: ◀ ▶ | − вставка вписать 100% + | ↺ ↻ | обрезка | отмена | печать
        use crate::gui::*;
        let by = h - bot_h + ui.s(7);
        let tb = ui.s(36);
        let gap = ui.s(2);
        let sepw = ui.s(14);
        let items: Vec<Option<(u32, u32, &str, &str, bool)>> = vec![
            Some((ID_PREV, IC_PREV, "", "Предыдущее фото / страница (←)", multi || self.files.len() > 1)),
            Some((ID_NEXT, IC_NEXT, "", "Следующее фото / страница (→)", multi || self.files.len() > 1)),
            None,
            Some((ID_ZOUT, IC_ZOOM_OUT, "", "Уменьшить (Ctrl+−)", has)),
            Some((ID_INSERT, IC_ADD, "", "Вставить фото или страницу (Ctrl+I)", true)),
            Some((ID_FIT, IC_FIT, "", "Вписать в окно — сброс масштаба (Ctrl+0)", has)),
            Some((ID_100, 0, "1:1", "Реальный размер, 100% (Ctrl+1)", has)),
            Some((ID_ZIN, IC_ZOOM_IN, "", "Увеличить (Ctrl++)", has)),
            None,
            Some((ID_ROT_L, IC_ROT_L, "", "Повернуть влево (Ctrl+L)", can_rot)),
            Some((ID_ROT_R, IC_ROT_R, "", "Повернуть вправо (Ctrl+R)", can_rot)),
            None,
            Some((ID_CROP, IC_CROP, "", "Обрезать (Ctrl+K)", has)),
            None,
            Some((ID_UNDO, IC_UNDO, "", "Отменить (Ctrl+Z)", !self.history.is_empty())),
            None,
            Some((ID_PRINT, IC_PRINT, "", "Печать (Ctrl+P)", !self.pages.is_empty())),
        ];
        let total: i32 = items.iter().map(|i| if i.is_some() { tb + gap } else { sepw }).sum();
        let info = self.info();
        let info_w = ui.tw(ui.f.text, &info) + ui.s(16);
        // по центру, но не наезжая на сведения справа
        let mut x = ((w - total) / 2).min(w - total - info_w - ui.s(8)).max(ui.s(8));
        let status_w = x - ui.s(16);
        for it in items {
            match it {
                Some((id, icon, txt, tip, en)) => {
                    let on = id == ID_CROP && self.crop_mode;
                    v.push(W::new(Kind::Tool, id, gui::r(x, by, tb, tb)).i(icon).t(txt).tip(tip).en(en).on(on));
                    x += tb + gap;
                }
                None => {
                    v.push(W::new(Kind::Sep, 0, gui::r(x + sepw / 2 - 1, by + ui.s(8), ui.s(2), tb - ui.s(16))));
                    x += sepw;
                }
            }
        }
        let rx = x + ui.s(8);
        v.push(W::new(Kind::Label, 0, gui::r(rx, by, (w - ui.s(14) - rx).max(0), tb)).t(info).align(2).color(ui.t.dim));
        if !self.status.is_empty() && status_w > ui.s(80) {
            v.push(W::new(Kind::Label, 0, gui::r(ui.s(14), by, status_w, tb)).t(self.status.clone()).color(ui.t.accent));
        }

        // пустое окно — кнопка «Открыть»
        if self.pages.is_empty() {
            let bw = ui.s(200);
            let cx = (self.canvas.left + self.canvas.right) / 2;
            let cy = (self.canvas.top + self.canvas.bottom) / 2;
            v.push(W::new(Kind::Button, ID_OPEN, gui::r(cx - bw / 2, cy + ui.s(10), bw, ui.s(38))).t("Открыть файл…").i(IC_OPEN).on(true));
            v.push(W::new(Kind::Button, ID_PASTE, gui::r(cx - bw / 2, cy + ui.s(56), bw, ui.s(34))).t("Вставить из буфера").i(IC_PASTE));
        }
        v
    }

    fn paint(&mut self, ui: &Ui, hdc: HDC, w: i32, h: i32) {
        let t = &ui.t;
        let c = self.canvas;
        if self.full.is_none() {
            gui::fill(hdc, &gui::r(0, 0, w, c.top), t.bg);
            gui::line(hdc, 0, c.top - 1, w, c.top - 1, t.border, 1, PS_SOLID);
            gui::fill(hdc, &gui::r(0, c.bottom, w, h - c.bottom), t.bg);
            gui::line(hdc, 0, c.bottom, w, c.bottom, t.border, 1, PS_SOLID);
        }
        gui::fill(hdc, &c, if self.full.is_some() { crate::util::rgb(0, 0, 0) } else { t.canvas });
        unsafe {
            let _ = IntersectClipRect(hdc, c.left, c.top, c.right, c.bottom);
        }
        match &self.view {
            None => {
                let cy = (c.top + c.bottom) / 2;
                if self.pages.is_empty() {
                    let ir = gui::r(c.left, cy - ui.s(110), c.right - c.left, ui.s(60));
                    gui::text(hdc, &char::from_u32(gui::IC_PICTURE).unwrap().to_string(), &ir, ui.f.icon_big, t.dim, gui::CENTER);
                    let tr = gui::r(c.left, cy - ui.s(52), c.right - c.left, ui.s(26));
                    gui::text(hdc, "Откройте фото или PDF", &tr, ui.f.big, t.text, gui::CENTER);
                    let tr2 = gui::r(c.left, cy - ui.s(26), c.right - c.left, ui.s(24));
                    gui::text(hdc, "или перетащите файлы в окно", &tr2, ui.f.text, t.dim, gui::CENTER);
                } else if let Some(e) = &self.view_err {
                    let tr = gui::r(c.left + ui.s(40), cy - ui.s(60), c.right - c.left - ui.s(80), ui.s(140));
                    gui::text(hdc, &format!("Не удалось показать страницу\n\n{e}"), &tr, ui.f.text, t.dim, gui::WRAP);
                }
            }
            Some(img) => {
                let s = self.scale();
                let (dx, dy, dw, dh) = self.dest();
                let (cw, ch) = (self.cw(), self.ch());
                let (vx0, vy0) = (dx.max(0.0), dy.max(0.0));
                let (vx1, vy1) = ((dx + dw).min(cw), (dy + dh).min(ch));
                if vx1 > vx0 && vy1 > vy0 {
                    let sx0 = ((vx0 - dx) / s).floor().max(0.0) as i32;
                    let sy0 = ((vy0 - dy) / s).floor().max(0.0) as i32;
                    let sx1 = (((vx1 - dx) / s).ceil() as i32).min(img.w as i32);
                    let sy1 = (((vy1 - dy) / s).ceil() as i32).min(img.h as i32);
                    let (sw, sh) = (sx1 - sx0, sy1 - sy0);
                    if sw > 0 && sh > 0 {
                        let tx = c.left + (dx + sx0 as f64 * s).round() as i32;
                        let ty = c.top + (dy + sy0 as f64 * s).round() as i32;
                        let tw = ((sw as f64 * s).round() as i32).max(1);
                        let th = ((sh as f64 * s).round() as i32).max(1);
                        // лёгкая тень под страницей
                        if self.full.is_none() {
                            let sh_rc = gui::r(c.left + dx.round() as i32 + 2, c.top + dy.round() as i32 + 2, dw.round() as i32, dh.round() as i32);
                            gui::fill(hdc, &sh_rc, gui::blend(t.canvas, crate::util::rgb(0, 0, 0), if t.dark { 0.6 } else { 0.85 }));
                        }
                        unsafe {
                            let info = crate::util::bmi(img);
                            SetStretchBltMode(hdc, if self.hq && s < 1.0 { HALFTONE } else { COLORONCOLOR });
                            let _ = SetBrushOrgEx(hdc, 0, 0, None);
                            // строки sy0..sy1: для top-down DIB указываем смещение снизу
                            StretchDIBits(hdc, tx, ty, tw, th, sx0, img.h as i32 - sy1, sw, sh,
                                Some(img.px.as_ptr() as *const _), &info, DIB_RGB_COLORS, SRCCOPY);
                        }
                    }
                }
                if let (true, Some((a, b, cc, d))) = (self.crop_mode, self.crop) {
                    let rc = RECT {
                        left: c.left + a.min(cc) as i32,
                        top: c.top + b.min(d) as i32,
                        right: c.left + a.max(cc) as i32,
                        bottom: c.top + b.max(d) as i32,
                    };
                    gui::frame(hdc, &rc, crate::util::rgb(255, 255, 255), PS_SOLID);
                    gui::frame(hdc, &rc, t.accent, PS_DASH);
                }
                if self.crop_mode && self.crop.is_none() {
                    let tr = gui::r(c.left, c.top + ui.s(10), c.right - c.left, ui.s(30));
                    let tw = ui.tw(ui.f.text, CROP_HINT) + ui.s(24);
                    let bx = gui::r((c.left + c.right - tw) / 2, tr.top, tw, tr.bottom - tr.top);
                    gui::round(hdc, &bx, ui.s(8), Some(t.tip_bg), None);
                    gui::text(hdc, CROP_HINT, &bx, ui.f.text, t.tip_text, gui::CENTER);
                }
            }
        }
        unsafe {
            let _ = SelectClipRgn(hdc, None);
        }
    }

    fn event(&mut self, _hwnd: HWND, ev: Ev) -> bool {
        if let Ev::Click(id) = ev {
            self.command(id);
        }
        true
    }

    fn key(&mut self, _hwnd: HWND, vk: u16, ctrl: bool, shift: bool) -> Option<bool> {
        let id = if ctrl {
            match vk {
                0x4F => ID_OPEN,
                0x53 if shift => ID_SAVE_PDF,
                0x53 => ID_SAVE,
                0x50 => ID_PRINT,
                0x5A => ID_UNDO,
                0x4C => ID_ROT_L,
                0x52 => ID_ROT_R,
                0x48 => ID_FLIP,
                0x4B => ID_CROP,
                0x51 => ID_EXIT,
                0x43 => ID_COPY,
                0x56 => ID_PASTE,
                0x49 => ID_INSERT,
                0x47 => ID_COLLAGE,
                0x4D => ID_MERGE,
                0x30 | 0x60 => ID_FIT,
                0x31 | 0x61 => ID_100,
                0xBB | 0x6B => ID_ZIN,
                0xBD | 0x6D => ID_ZOUT,
                0x09 => {
                    // Ctrl+Tab — следующая вкладка
                    self.tab = (self.tab + if shift { TABS.len() - 1 } else { 1 }) % TABS.len();
                    return Some(true);
                }
                _ => 0,
            }
        } else {
            match vk {
                0x25 | 0x21 => ID_PREV, // ← PageUp
                0x27 | 0x22 => ID_NEXT, // → PageDown
                0x24 if self.pages.len() > 1 => {
                    self.cur = 0;
                    self.refresh_view();
                    return Some(true);
                }
                0x23 if self.pages.len() > 1 => {
                    self.cur = self.pages.len() - 1;
                    self.refresh_view();
                    return Some(true);
                }
                0x2E => ID_DEL_PAGE,
                0x70 => ID_KEYS,
                0x7A => ID_FULL,
                0x0D => {
                    self.apply_crop();
                    return Some(true);
                }
                0x1B => {
                    if self.crop_mode {
                        self.crop_mode = false;
                        self.crop = None;
                    } else if self.full.is_some() {
                        self.toggle_full();
                    }
                    return Some(true);
                }
                _ => 0,
            }
        };
        if id != 0 {
            self.command(id);
        }
        Some(true)
    }

    fn cursor(&self, x: i32, y: i32) -> Option<PCWSTR> {
        if self.view.is_some() && gui::inside(&self.canvas, x, y) {
            if self.crop_mode {
                return Some(IDC_CROSS);
            }
            if self.drag.is_some() {
                return Some(IDC_SIZEALL);
            }
        }
        None
    }

    fn mouse(&mut self, hwnd: HWND, m: Mouse, x: i32, y: i32) -> bool {
        let (cx, cy) = ((x - self.canvas.left) as f64, (y - self.canvas.top) as f64);
        let in_canvas = gui::inside(&self.canvas, x, y);
        match m {
            Mouse::Down => {
                if self.view.is_none() || !in_canvas {
                    return false;
                }
                self.drag = Some((cx, cy));
                if self.crop_mode {
                    self.crop = Some((cx, cy, cx, cy));
                }
                true
            }
            Mouse::Move => {
                let Some((px, py)) = self.drag else { return false };
                if self.crop_mode {
                    let (cw, ch) = (self.cw(), self.ch());
                    if let Some(c) = &mut self.crop {
                        c.2 = cx.clamp(0.0, cw);
                        c.3 = cy.clamp(0.0, ch);
                    }
                } else {
                    self.off.0 += cx - px;
                    self.off.1 += cy - py;
                    self.drag = Some((cx, cy));
                    self.lazy_quality(hwnd);
                }
                true
            }
            Mouse::Up => {
                self.drag = None;
                false
            }
            Mouse::Dbl => {
                if in_canvas && !self.crop_mode {
                    self.reset_view();
                    true
                } else {
                    false
                }
            }
            Mouse::Wheel(d) => {
                if self.view.is_some() && in_canvas {
                    self.lazy_quality(hwnd);
                    self.zoom_at(if d > 0 { 1.15 } else { 1.0 / 1.15 }, cx, cy);
                    true
                } else {
                    false
                }
            }
            Mouse::RDown => false,
        }
    }

    fn timer(&mut self, hwnd: HWND, id: usize) -> bool {
        unsafe {
            let _ = KillTimer(hwnd, id);
        }
        match id {
            TIMER_HQ => self.hq = true,
            TIMER_STATUS => self.status.clear(),
            TIMER_START => {
                let s = std::mem::take(&mut self.start);
                self.load_paths(&s);
            }
            _ => {}
        }
        true
    }

    fn drop_files(&mut self, _hwnd: HWND, files: Vec<PathBuf>) {
        let files: Vec<PathBuf> = files.into_iter().filter(|p| files::is_supported(p)).collect();
        if files.is_empty() {
            return;
        }
        // Shift при перетаскивании — добавить к документу, иначе — открыть
        let shift = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x10) } < 0;
        if shift && !self.pages.is_empty() {
            let (pages, errs) = doc::pages_from_all(&files);
            if !errs.is_empty() {
                self.msg(&format!("Не удалось добавить:\n\n{}", errs.join("\n\n")), true);
            }
            self.insert_pages(pages);
        } else if self.confirm_discard() {
            self.load_paths(&files);
        }
    }

    fn close(&mut self, hwnd: HWND) -> bool {
        if !self.confirm_discard() {
            return false;
        }
        if self.full.is_some() {
            self.toggle_full();
        }
        save_placement(hwnd);
        settings::save();
        true
    }
}

const CROP_HINT: &str = "Выделите область мышью · Enter — применить · Esc — отмена";

impl App {
    fn lazy_quality(&mut self, hwnd: HWND) {
        self.hq = false;
        unsafe {
            SetTimer(hwnd, TIMER_HQ, 160, None);
        }
    }
}

// ------------------------------------------------------------------ сервис ---
pub fn message(hwnd: Option<HWND>, text: &str, error: bool) -> MESSAGEBOX_RESULT {
    unsafe {
        MessageBoxW(
            hwnd.unwrap_or_default(),
            &HSTRING::from(text),
            &HSTRING::from(APP_NAME),
            if error { MB_OK | MB_ICONERROR } else { MB_OK | MB_ICONINFORMATION },
        )
    }
}

fn save_placement(hwnd: HWND) {
    unsafe {
        let mut wp = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
        if GetWindowPlacement(hwnd, &mut wp).is_ok() {
            let r = wp.rcNormalPosition;
            settings::set_s("window", &format!("{},{},{},{}", r.left, r.top, r.right, r.bottom));
            settings::set_i("maximized", (wp.showCmd == SW_SHOWMAXIMIZED.0 as u32) as i32);
        }
    }
}

fn saved_placement() -> Option<RECT> {
    let s = settings::get_s("window")?;
    let v: Vec<i32> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    if v.len() != 4 || v[2] - v[0] < 300 || v[3] - v[1] < 200 {
        return None;
    }
    let rc = RECT { left: v[0], top: v[1], right: v[2], bottom: v[3] };
    // окно должно быть хотя бы частично на каком-то мониторе
    let on_screen = unsafe { !MonitorFromRect(&rc, MONITOR_DEFAULTTONULL).is_invalid() };
    on_screen.then_some(rc)
}

pub fn run(args: Vec<PathBuf>) {
    theme::init();
    let app: Rc<RefCell<dyn Panel>> = Rc::new(RefCell::new(App::new(args)));
    let hwnd = gui::create(None, app, (1120, 760), saved_placement());
    unsafe {
        let _ = ShowWindow(hwnd, if settings::get_i("maximized", 0) == 1 { SW_SHOWMAXIMIZED } else { SW_SHOW });
    }
    gui::message_loop();
    settings::save();
}
