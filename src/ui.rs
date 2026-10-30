// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Главное окно: шапка с вкладками (лента), холст, нижняя панель быстрых
//! инструментов. Что показывается на холсте, зависит от типа файла:
//! страницы (фото, PDF, документы), таблица, текст/код или аудио/видео.
//! Лента и нижняя панель подстраиваются под тип файла и ширину окна.
//! По краям холста — «хвостики» соседних файлов (или страниц).

use crate::doc::{self, Page, Src};
use crate::gui::{self, Ev, Mouse, Panel, Ui, W};
use crate::image_buf::Img;
use crate::kinds::{self, Kind as FK};
use crate::media::{MediaView, WM_MEDIA};
use crate::peek::{self, Peeker, WM_PEEK};
use crate::table::TableView;
use crate::textview::{self, TextView};
use crate::theme::{self, Mode};
use crate::util::{file_name, is_pdf};
use crate::{clip, codec, files, settings};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
use windows::Win32::UI::Shell::DragAcceptFiles;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const APP_NAME: &str = "SimplePhotoViewer";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const HISTORY: usize = 10;
const TIMER_HQ: usize = 1;
const TIMER_STATUS: usize = 2;
const TIMER_START: usize = 3;
const TIMER_TEXT: usize = 4;
const TIMER_MEDIA: usize = 5;
const TIMER_ANIM: usize = 6;
const WM_TEXT_REFRESH: u32 = 0x8000 + 23;
const ANIM_MS: f64 = 220.0;

// ---- команды
const ID_OPEN: u32 = 101;
const ID_SAVE: u32 = 102;
const ID_EXIT: u32 = 103;
const ID_SAVE_PDF: u32 = 104;
const ID_COPY: u32 = 105;
const ID_PASTE: u32 = 106;
const ID_SAVE_AS: u32 = 107;
const ID_CLOSE_FILE: u32 = 108;
const ID_UNDO: u32 = 110;
const ID_ROT_L: u32 = 111;
const ID_ROT_R: u32 = 112;
const ID_FLIP: u32 = 113;
const ID_CROP: u32 = 114;
const ID_FLIP_V: u32 = 115;
const ID_REDO: u32 = 116;
const ID_FIND: u32 = 117;
const ID_REPLACE_UI: u32 = 118;
const ID_WRAP: u32 = 119;
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
const ID_BOOK: u32 = 147;
const ID_SCROLL: u32 = 148;
const ID_PRINT: u32 = 150;
const ID_KEYS: u32 = 160;
const ID_ABOUT: u32 = 161;
const ID_INSERT: u32 = 170;
const ID_DEL_PAGE: u32 = 171;
const ID_MOVE_L: u32 = 172;
const ID_MOVE_R: u32 = 173;
const ID_COLLAGE: u32 = 180;
const ID_MERGE: u32 = 181;
const ID_CONVERT: u32 = 182;
const ID_THEME_SYS: u32 = 190;
const ID_THEME_LIGHT: u32 = 191;
const ID_THEME_DARK: u32 = 192;
const ID_THEME_CYCLE: u32 = 193;
const ID_MORE: u32 = 199;
const ID_WORK: u32 = 198;
const ID_TAB0: u32 = 200;
const ID_PLAY: u32 = 300;
const ID_BACK10: u32 = 301;
const ID_FWD10: u32 = 302;
const ID_MUTE: u32 = 303;
const ID_LOOP: u32 = 304;
const ID_SPEED: u32 = 305;
const ID_FRAME: u32 = 306;
const ID_HEADER: u32 = 310;
const ID_UNSORT: u32 = 311;
const ID_COPY_ROW: u32 = 312;

const TABS: [&str; 5] = ["Главная", "Правка", "Страницы", "Создать", "Вид"];

/// Что сейчас на холсте.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode2 {
    Empty,
    Pages,
    Table,
    Text,
    Media,
}

enum Viewer {
    None,
    Table(TableView),
    Text(TextView),
    Media(MediaView),
}

/// Сосед текущего элемента: страница этого документа или файл папки.
#[derive(Clone)]
enum Nb {
    Page(usize),
    File(PathBuf),
}

struct Anim {
    old: Rc<Img>,
    rect: (f64, f64, f64, f64),
    dir: f64,
    t0: Instant,
}

/// Кнопка ленты: id, значок, подпись, подсказка.
type Rb = (u32, u32, &'static str, &'static str);

// ---------------------------------------------------------------- состояние ---
pub struct App {
    hwnd: HWND,
    dpi: u32,
    start: Vec<PathBuf>,
    pages: Vec<Page>,
    cur: usize,
    doc_name: String,
    doc_path: Option<PathBuf>,
    kind: Option<FK>,
    viewer: Viewer,
    files: Vec<PathBuf>,
    idx: isize,
    history: Vec<(Vec<Page>, usize)>,
    dirty: bool,
    view: Option<Rc<Img>>,
    view_err: Option<String>,
    zoom: f64, // относительно «вписать в окно»
    off: (f64, f64),
    /// Весь холст (с «хвостиками»).
    area: RECT,
    /// Где показывается содержимое.
    canvas: RECT,
    crop_mode: bool,
    crop: Option<(f64, f64, f64, f64)>,
    drag: Option<(f64, f64)>,
    hq: bool,
    tab: usize,
    full: Option<(WINDOWPLACEMENT, isize)>,
    status: String,
    // режим «прокрутка» для многостраничных документов
    scroll_mode: bool,
    /// Режим работы с файлом (иначе — обзор: стрелки листают файлы).
    work: bool,
    sy: f64,
    page_cache: HashMap<usize, (f64, Rc<Img>)>,
    page_thumbs: HashMap<usize, Rc<Img>>,
    // хвостики
    peeker: Option<Peeker>,
    peek_l: RECT,
    peek_r: RECT,
    peek_hover: i8,
    anim: Option<Anim>,
    overflow: Vec<(u32, String)>,
}

fn key_down(vk: i32) -> bool {
    unsafe { GetKeyState(vk) < 0 }
}

impl App {
    fn new(start: Vec<PathBuf>) -> App {
        App {
            hwnd: HWND::default(), dpi: 96, start, pages: vec![], cur: 0, doc_name: String::new(), doc_path: None, kind: None,
            viewer: Viewer::None, files: vec![], idx: -1, history: vec![], dirty: false, view: None, view_err: None,
            zoom: 1.0, off: (0.0, 0.0), area: RECT::default(), canvas: RECT::default(), crop_mode: false, crop: None, drag: None,
            hq: true, tab: settings::get_i("tab", 0).clamp(0, 4) as usize, full: None, status: String::new(),
            scroll_mode: settings::get_i("doc_scroll", 0) == 1, work: false, sy: 0.0, page_cache: HashMap::new(), page_thumbs: HashMap::new(),
            peeker: None, peek_l: RECT::default(), peek_r: RECT::default(), peek_hover: 0, anim: None, overflow: vec![],
        }
    }

    fn mode(&self) -> Mode2 {
        match &self.viewer {
            Viewer::Table(_) => Mode2::Table,
            Viewer::Text(_) => Mode2::Text,
            Viewer::Media(_) => Mode2::Media,
            Viewer::None if !self.pages.is_empty() => Mode2::Pages,
            Viewer::None => Mode2::Empty,
        }
    }

    /// Прокрутка страниц включена и имеет смысл (страниц больше одной).
    fn scrolling(&self) -> bool {
        self.scroll_mode && self.pages.len() > 1 && self.viewer_is_none()
    }

    fn viewer_is_none(&self) -> bool {
        matches!(self.viewer, Viewer::None)
    }

    fn s(&self, v: i32) -> i32 {
        v * self.dpi as i32 / 96
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
                if self.pages.get(self.cur).map(|p| p.is_docpage()).unwrap_or(false) { f } else { f.min(1.0) }
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
        if self.scrolling() {
            // ширина страницы = её размер в пунктах при текущем DPI экрана
            let wpt = self.pages.get(self.cur).and_then(|p| p.size_pt()).map(|s| s.0).unwrap_or(595.0);
            self.zoom = (wpt / 72.0 * self.dpi as f64 / self.sm_base()).clamp(0.2, 8.0);
            return;
        }
        let f = self.fit();
        self.zoom = if f > 0.0 { 1.0 / f } else { 1.0 };
        self.off = (0.0, 0.0);
    }
    /// Масштаб относительно точки (mx, my) холста — она остаётся на месте.
    fn zoom_at(&mut self, k: f64, mx: f64, my: f64) {
        if self.scrolling() {
            let z1 = self.zoom;
            self.zoom = (self.zoom * k).clamp(0.2, 8.0);
            let r = self.zoom / z1;
            self.sy = (self.sy + my) * r - my;
            let cx = mx - self.cw() / 2.0;
            self.off.0 = if self.zoom <= 1.0 + 1e-9 { 0.0 } else { (self.off.0 - cx) * r + cx };
            return;
        }
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
        match &mut self.viewer {
            Viewer::Table(t) => {
                t.zoom = (t.zoom * k).clamp(0.5, 3.0);
                return;
            }
            Viewer::Text(t) => {
                t.zoom(if k > 1.0 { 1 } else { -1 });
                return;
            }
            _ => {}
        }
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

    fn text_dirty(&self) -> bool {
        matches!(&self.viewer, Viewer::Text(t) if t.dirty())
    }

    fn title(&self) -> String {
        if self.mode() == Mode2::Empty && self.doc_path.is_none() {
            return format!("{APP_NAME} {VERSION}");
        }
        let star = if self.dirty || self.text_dirty() { "*" } else { "" };
        let hint = if self.crop_mode { " — ОБРЕЗКА: выделите область, Enter — применить, Esc — отмена" } else { "" };
        format!("{star}{} — {APP_NAME}{hint}", self.doc_name)
    }

    fn info(&self) -> String {
        let mut parts = vec![];
        match &self.viewer {
            Viewer::Table(t) => parts.push(t.info()),
            Viewer::Text(t) => {
                parts.push(t.info());
            }
            Viewer::Media(m) => parts.push(m.info()),
            Viewer::None => {
                if self.scrolling() {
                    parts.push(format!("{}%", (self.zoom * 100.0).round() as i64));
                } else if let Some(v) = &self.view {
                    parts.push(format!("{} × {} px", v.w, v.h));
                    parts.push(format!("{}%", (self.scale() * 100.0).round() as i64));
                }
                if self.pages.len() > 1 {
                    parts.push(format!("стр. {} / {}", self.cur + 1, self.pages.len()));
                }
            }
        }
        if (self.pages.len() <= 1) && self.idx >= 0 && self.files.len() > 1 {
            parts.push(format!("файл {} / {}", self.idx + 1, self.files.len()));
        }
        parts.retain(|p| !p.is_empty());
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
    fn pages_changed(&mut self) {
        self.page_cache.clear();
        self.page_thumbs.clear();
    }

    /// Показать текущую страницу (загрузить/нарисовать при необходимости).
    fn refresh_view(&mut self) {
        self.crop = None;
        self.crop_mode = false;
        self.reset_view();
        self.view = None;
        self.view_err = None;
        if self.scrolling() {
            return;
        }
        let Some(p) = self.pages.get(self.cur) else { return };
        match doc::render(p, doc::VIEW_DPI) {
            Ok(i) => self.view = Some(i),
            Err(e) => self.view_err = Some(e),
        }
    }

    fn confirm_discard(&mut self) -> bool {
        if self.text_dirty() {
            let r = unsafe {
                MessageBoxW(self.hwnd, w!("Текст изменён. Сохранить изменения?"), &HSTRING::from(APP_NAME), MB_YESNOCANCEL | MB_ICONQUESTION)
            };
            return match r {
                IDYES => self.save_text(false),
                IDNO => true,
                _ => false,
            };
        }
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

    /// Закрыть текущее (просмотрщик, страницы) — без вопросов.
    fn clear_all(&mut self) {
        if let Viewer::Media(m) = &mut self.viewer {
            m.pause();
        }
        self.viewer = Viewer::None;
        self.work = false;
        unsafe {
            let _ = KillTimer(self.hwnd, TIMER_TEXT);
            let _ = KillTimer(self.hwnd, TIMER_MEDIA);
        }
        self.pages.clear();
        self.cur = 0;
        self.history.clear();
        self.dirty = false;
        self.view = None;
        self.view_err = None;
        self.kind = None;
        self.crop_mode = false;
        self.crop = None;
        self.sy = 0.0;
        self.pages_changed();
        self.reset_view();
    }

    fn set_siblings(&mut self, path: Option<&Path>) {
        match path {
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
    }

    fn set_doc(&mut self, pages: Vec<Page>, name: String, path: Option<PathBuf>, dirty: bool) {
        self.clear_all();
        self.pages = pages;
        self.doc_name = name;
        self.dirty = dirty;
        self.set_siblings(path.as_deref());
        self.kind = path.as_deref().and_then(kinds::kind_of).or(Some(FK::Image));
        self.doc_path = path;
        self.refresh_view();
    }

    /// Открыть файл, который показывается не страницами: таблица, текст, медиа.
    fn open_single(&mut self, path: &Path, k: FK) {
        self.clear_all();
        self.set_siblings(Some(path));
        self.doc_name = file_name(path);
        self.doc_path = Some(path.to_path_buf());
        self.kind = Some(k);
        let r: std::result::Result<Viewer, String> = match k {
            FK::Table => crate::table::load(path).and_then(|s| {
                if s.is_empty() { Err("В файле нет листов с данными".into()) } else { Ok(Viewer::Table(TableView::new(s))) }
            }),
            FK::Text => TextView::open(self.hwnd, path).map(Viewer::Text),
            FK::Audio | FK::Video => Ok(Viewer::Media(MediaView::open(self.hwnd, path, k == FK::Video))),
            _ => Err("формат не поддерживается".into()),
        };
        match r {
            Ok(v) => {
                self.viewer = v;
                unsafe {
                    match &self.viewer {
                        Viewer::Text(_) => {
                            SetTimer(self.hwnd, TIMER_TEXT, 200, None);
                            // открываем в режиме просмотра: стрелки листают файлы;
                            // щелчок по тексту или любая буква — правка
                            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(self.hwnd);
                        }
                        Viewer::Media(_) => {
                            SetTimer(self.hwnd, TIMER_MEDIA, 15, None);
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => self.view_err = Some(format!("{}:\n{e}", file_name(path))),
        }
    }

    /// Открыть файлы (без вопроса о несохранённом).
    fn load_paths(&mut self, paths: &[PathBuf]) {
        let paths: Vec<PathBuf> = paths.iter().filter(|p| p.is_file()).map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone())).collect();
        if paths.is_empty() {
            return;
        }
        if let Some(d) = paths[0].parent() {
            settings::set_s("last_dir", &d.to_string_lossy());
        }
        // таблица/текст/медиа — открывается по одному
        if let Some(k) = kinds::kind_of(&paths[0]) {
            if !kinds::is_paged(k) {
                if paths.len() > 1 {
                    self.set_status("Таблицы, тексты и медиа открываются по одному — показан первый файл");
                }
                self.open_single(&paths[0], k);
                return;
            }
        }
        let paged: Vec<PathBuf> = paths.iter().filter(|p| kinds::kind_of(p).map(kinds::is_paged).unwrap_or(false)).cloned().collect();
        let (pages, errs) = doc::pages_from_all(&paged);
        if pages.is_empty() {
            if paths.len() == 1 {
                // показываем ошибку прямо на холсте — листать соседей можно дальше
                self.clear_all();
                self.set_siblings(Some(&paths[0]));
                self.doc_name = file_name(&paths[0]);
                self.doc_path = Some(paths[0].clone());
                self.kind = kinds::kind_of(&paths[0]);
                self.view_err = Some(errs.join("\n\n"));
            } else if !errs.is_empty() {
                self.msg(&format!("Не удалось открыть:\n\n{}", errs.join("\n\n")), true);
            }
            return;
        }
        if !errs.is_empty() {
            self.msg(&format!("Не удалось открыть:\n\n{}", errs.join("\n\n")), true);
        }
        if paged.len() == 1 {
            let n = pages.len();
            let name = if n > 1 { format!("{} ({} стр.)", file_name(&paged[0]), n) } else { file_name(&paged[0]) };
            self.set_doc(pages, name, Some(paged[0].clone()), false);
        } else {
            let name = format!("Документ ({} стр.)", pages.len());
            self.set_doc(pages, name, None, false);
        }
    }

    fn open(&mut self) {
        if !self.confirm_discard() {
            return;
        }
        let p = files::open_dialog(self.hwnd, files::Pick::Any, true);
        self.load_paths(&p);
    }

    fn close_file(&mut self) {
        if !self.confirm_discard() {
            return;
        }
        self.clear_all();
        self.doc_path = None;
        self.doc_name.clear();
        self.set_siblings(None);
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

    /// Сохранить текст: на место (Ctrl+S) или под другим именем.
    fn save_text(&mut self, ask: bool) -> bool {
        let dir = self.default_dir();
        let Viewer::Text(t) = &mut self.viewer else { return false };
        let target = if ask {
            let name = file_name(&t.path);
            let ext = crate::util::ext_lower(&t.path);
            let exts: Vec<&str> = if ext.is_empty() { vec!["txt"] } else { vec![ext.as_str()] };
            match files::save_as(self.hwnd, &name, dir.as_deref(), &[("Текст", &exts), ("Все файлы", &["*"])], 0) {
                Some(p) => Some(p),
                None => return false,
            }
        } else {
            None
        };
        match t.save(target.as_deref()) {
            Ok(p) => {
                self.doc_name = file_name(&p);
                if target.is_some() {
                    self.doc_path = Some(p.clone());
                    self.set_siblings(Some(&p));
                }
                if let Some(pk) = &mut self.peeker {
                    pk.forget(&p);
                }
                self.set_status(format!("Сохранено: {}", file_name(&p)));
                true
            }
            Err(e) => {
                self.msg(&format!("Не удалось сохранить:\n{e}"), true);
                false
            }
        }
    }

    /// «Сохранить как». pdf — сразу предложить PDF.
    fn save_as(&mut self, pdf: bool) -> bool {
        if self.mode() == Mode2::Text {
            return self.save_text(true);
        }
        if self.pages.is_empty() {
            return false;
        }
        let multi = self.pages.len() > 1 || self.pages.iter().any(|p| p.is_docpage());
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
            if self.view.is_none() {
                if let Some(p) = self.pages.get(self.cur) {
                    self.view = doc::render(p, doc::VIEW_DPI).ok();
                }
            }
            let Some(img) = self.view.clone() else { return false };
            match codec::save(&img, &path) {
                Ok(()) => {
                    // для одиночного фото сохранение «закрывает» правки
                    if self.pages.len() == 1 {
                        self.dirty = false;
                    }
                    if let Some(pk) = &mut self.peeker {
                        pk.forget(&path);
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

    // ------------------------------------------------------ листание, соседи ---
    /// Несколько страниц без «своего» файла в папке (склейка, несколько фото):
    /// тогда стрелки в обзоре листают страницы.
    fn pages_are_files(&self) -> bool {
        self.viewer_is_none() && self.pages.len() > 1 && (self.doc_path.is_none() || self.idx < 0)
    }

    /// Можно ли «войти» в файл: документ, текст, таблица.
    fn can_work(&self) -> bool {
        match self.mode() {
            Mode2::Text | Mode2::Table => true,
            Mode2::Pages => self.view_err.is_none() && !self.pages_are_files()
                && (self.pages.len() > 1 || matches!(self.kind, Some(FK::Pdf | FK::Doc))),
            _ => false,
        }
    }

    /// Подпись кнопки входа в режим работы — по типу файла.
    fn work_label(&self) -> (&'static str, u32, &'static str) {
        if self.work {
            return ("К файлам", gui::IC_BACK, "Вернуться к листанию файлов (Esc)");
        }
        match self.mode() {
            Mode2::Text => ("Править", 0xE70F, "Править текст: курсор, ввод, поиск (Enter или щелчок по тексту)"),
            Mode2::Table => ("Работать", gui::IC_GRID, "Ходить по ячейкам, сортировать, копировать (Enter или щелчок по таблице)"),
            _ if matches!(self.kind, Some(FK::Pdf | FK::Doc)) => ("Читать", gui::IC_BOOK, "Читать документ: стрелки листают страницы (Enter или двойной щелчок)"),
            _ => ("Страницы", gui::IC_BOOK, "Листать страницы этого документа (Enter)"),
        }
    }

    fn set_work(&mut self, on: bool) {
        if on && !self.can_work() || self.work == on {
            return;
        }
        self.work = on;
        self.peek_hover = 0;
        self.anim = None;
        if let Viewer::Text(t) = &self.viewer {
            if on {
                t.focus();
            } else {
                unsafe {
                    let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(self.hwnd);
                }
            }
        }
        if on {
            let hint = match self.mode() {
                Mode2::Text => "Правка · Esc — к файлам",
                Mode2::Table => "Работа с таблицей: стрелки — по ячейкам · Esc — к файлам",
                _ => "Чтение: ← → — страницы · Esc — к файлам",
            };
            self.set_status(hint);
        } else {
            self.set_status("Обзор: ← → — соседние файлы");
        }
    }

    fn neighbor(&self, d: isize) -> Option<Nb> {
        // страницы — в режиме работы (книга) или если документ не из папки
        if self.viewer_is_none() && self.pages.len() > 1 && !self.scrolling() && (self.work || self.pages_are_files()) {
            let n = self.cur as isize + d;
            if n >= 0 && (n as usize) < self.pages.len() {
                return Some(Nb::Page(n as usize));
            }
        }
        // в режиме работы из файла никуда не уходим
        if self.work || self.idx < 0 {
            return None;
        }
        let j = self.idx + d;
        (j >= 0 && (j as usize) < self.files.len()).then(|| Nb::File(self.files[j as usize].clone()))
    }

    fn start_anim(&mut self, old: Option<(Rc<Img>, (f64, f64, f64, f64))>, dir: f64) {
        if let (Some((img, rect)), true) = (old, self.view.is_some() && !self.scrolling() && self.full.is_none()) {
            self.anim = Some(Anim { old: img, rect, dir, t0: Instant::now() });
            unsafe {
                SetTimer(self.hwnd, TIMER_ANIM, 15, None);
            }
        }
    }

    fn snapshot(&self) -> Option<(Rc<Img>, (f64, f64, f64, f64))> {
        if self.scrolling() || !self.viewer_is_none() {
            return None;
        }
        self.view.clone().map(|v| (v, self.dest()))
    }

    /// ← → : в обзоре — соседний файл; в работе — страница документа.
    fn step(&mut self, d: isize) {
        if self.viewer_is_none() && self.pages.len() > 1 && (self.work || self.pages_are_files()) {
            let n = self.cur as isize + d;
            if n >= 0 && (n as usize) < self.pages.len() {
                if self.scrolling() {
                    self.scroll_to_page(n as usize);
                } else {
                    let old = self.snapshot();
                    self.cur = n as usize;
                    self.refresh_view();
                    self.start_anim(old, d as f64);
                }
            }
            return;
        }
        if self.work {
            return;
        }
        self.step_file(d);
    }

    /// Соседний файл папки (из любого режима: Alt+← →, Ctrl+PgUp/PgDn, хвостики).
    fn step_file(&mut self, d: isize) {
        let n = self.files.len() as isize;
        let j = self.idx + d;
        if self.idx < 0 || j < 0 || j >= n || !self.confirm_discard() {
            return;
        }
        let old = self.snapshot();
        let p = self.files[j as usize].clone();
        self.load_paths(&[p]);
        self.start_anim(old, d as f64);
    }

    fn go_home(&mut self, end: bool) {
        if self.pages.len() < 2 {
            return;
        }
        if self.scrolling() {
            self.sy = if end { f64::MAX } else { 0.0 };
            return;
        }
        self.cur = if end { self.pages.len() - 1 } else { 0 };
        self.refresh_view();
    }

    // ------------------------------------------------------------ правка ---
    fn push_history(&mut self) {
        self.history.push((self.pages.clone(), self.cur));
        if self.history.len() > HISTORY {
            self.history.remove(0);
        }
        self.dirty = true;
        self.pages_changed();
    }

    fn undo(&mut self) {
        if let Viewer::Text(t) = &self.viewer {
            t.undo();
            return;
        }
        if let Some((pages, cur)) = self.history.pop() {
            self.pages = pages;
            self.cur = cur.min(self.pages.len().saturating_sub(1));
            self.dirty = !self.history.is_empty();
            self.pages_changed();
            self.refresh_view();
        }
    }

    /// Правка картинки текущей страницы.
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
        if self.pages.get(self.cur).map(|p| p.is_docpage() && p.img.is_none()).unwrap_or(false) {
            // страница PDF/документа остаётся векторной: запоминаем поворот
            self.push_history();
            let p = &mut self.pages[self.cur];
            p.rot = (p.rot + if cw { 1 } else { 3 }) % 4;
            if let Some(v) = self.view.take() {
                self.view = Some(Rc::new(if cw { v.rotate_cw() } else { v.rotate_ccw() }));
            }
            self.crop_mode = false;
            if !self.scrolling() {
                self.reset_view();
            }
            self.set_status(format!("Страница {} повёрнута", self.cur + 1));
        } else {
            if self.scrolling() {
                self.set_scroll(false);
            }
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
        if self.scrolling() {
            self.scroll_to_page(at);
        }
        self.set_status(format!("Вставлено страниц: {n}"));
    }

    fn insert_dialog(&mut self) {
        let p = files::open_dialog(self.hwnd, files::Pick::Any, true);
        if p.is_empty() {
            return;
        }
        let p: Vec<PathBuf> = p.into_iter().filter(|x| kinds::kind_of(x).map(kinds::is_paged).unwrap_or(false)).collect();
        let (pages, errs) = doc::pages_from_all(&p);
        if !errs.is_empty() {
            self.msg(&format!("Не удалось добавить:\n\n{}", errs.join("\n\n")), true);
        }
        self.insert_pages(pages);
    }

    fn paste(&mut self) {
        if !matches!(self.mode(), Mode2::Pages | Mode2::Empty) {
            return;
        }
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
        if self.pages.is_empty() {
            self.load_paths(&fl);
            return;
        }
        let fl: Vec<PathBuf> = fl.into_iter().filter(|x| kinds::kind_of(x).map(kinds::is_paged).unwrap_or(false)).collect();
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
            .filter(|p| !p.is_docpage() || p.img.is_some())
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
            Some(p) if self.mode() != Mode2::Pages => {
                if kinds::kind_of(p).map(kinds::is_paged).unwrap_or(false) { vec![p.clone()] } else { vec![] }
            }
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

    fn convert(&mut self) {
        let mut init: Vec<PathBuf> = vec![];
        if let Some(p) = &self.doc_path {
            if matches!(kinds::kind_of(p), Some(FK::Image | FK::Audio | FK::Video)) {
                init.push(p.clone());
            }
        }
        if self.doc_path.is_none() {
            for p in &self.pages {
                if let (Src::File(x), None) = (&p.src, &p.img) {
                    if !init.contains(x) {
                        init.push(x.clone());
                    }
                }
            }
        }
        if let Viewer::Media(m) = &mut self.viewer {
            m.pause();
        }
        crate::convert_dlg::run(self.hwnd, init);
        // новые файлы могли появиться в папке
        let p = self.doc_path.clone();
        if p.is_some() {
            self.set_siblings(p.as_deref());
        }
    }

    fn save_frame(&mut self) {
        let Viewer::Media(m) = &mut self.viewer else { return };
        m.pause();
        let Some(img) = m.snapshot() else {
            self.set_status("Кадр ещё не готов");
            return;
        };
        let t = crate::media::fmt_time(m.pos).replace(':', "-");
        let name = format!("{} {}.jpg", self.stem(), t);
        let dir = self.default_dir();
        let Some(path) = files::save_dialog(self.hwnd, &name, dir.as_deref(), false, false) else { return };
        let res = if is_pdf(&path) {
            doc::save_pdf(&[Page::mem("Кадр", img)], &path)
        } else {
            codec::save(&img, &path).map_err(|e| e.message().to_string())
        };
        match res {
            Ok(()) => {
                self.set_status(format!("Кадр сохранён: {}", file_name(&path)));
                let p = self.doc_path.clone();
                self.set_siblings(p.as_deref());
            }
            Err(e) => self.msg(&format!("Не удалось сохранить кадр:\n{e}"), true),
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
        if !self.scrolling() {
            self.reset_view();
        }
    }

    fn set_theme(&mut self, m: Mode) {
        theme::set_mode(m);
        gui::retheme_all();
        let name = match m { Mode::System => "как в системе", Mode::Light => "светлая", Mode::Dark => "тёмная" };
        self.set_status(format!("Тема: {name}"));
    }

    fn set_scroll(&mut self, on: bool) {
        if self.scroll_mode == on {
            return;
        }
        let cur = self.cur;
        self.scroll_mode = on;
        settings::set_i("doc_scroll", on as i32);
        self.anim = None;
        self.reset_view();
        self.refresh_view();
        if on {
            self.scroll_to_page(cur);
        }
    }

    // -------------------------------------------------- режим «прокрутка» ---
    fn sm_pad(&self) -> f64 {
        self.s(20) as f64
    }
    fn sm_gap(&self) -> f64 {
        self.s(14) as f64
    }
    /// Ширина страницы при масштабе 1 — «по ширине окна», но не шире разумного.
    fn sm_base(&self) -> f64 {
        (self.cw() - 2.0 * self.sm_pad()).clamp(60.0, self.s(980) as f64)
    }
    /// Высота / ширина страницы.
    fn page_aspect(&self, i: usize) -> f64 {
        let p = &self.pages[i];
        if let Some((w, h)) = p.size_pt() {
            return h / w.max(1.0);
        }
        let img = p.img.clone().or_else(|| self.page_cache.get(&i).map(|c| c.1.clone())).or_else(|| self.page_thumbs.get(&i).cloned());
        match img {
            Some(im) => im.h as f64 / im.w.max(1) as f64,
            None => 0.75,
        }
    }
    /// Верх и высота каждой страницы (в координатах документа), общая высота.
    fn sm_layout(&self) -> (Vec<(f64, f64)>, f64, f64) {
        let w = self.sm_base() * self.zoom;
        let (pad, gap) = (self.sm_pad(), self.sm_gap());
        let mut y = pad;
        let mut v = Vec::with_capacity(self.pages.len());
        for i in 0..self.pages.len() {
            let h = w * self.page_aspect(i);
            v.push((y, h));
            y += h + gap;
        }
        (v, y - gap + pad, w)
    }
    fn sm_clamp(&mut self, total: f64, w: f64) {
        self.sy = self.sy.clamp(0.0, (total - self.ch()).max(0.0));
        let maxx = ((w - self.cw()) / 2.0 + self.sm_pad()).max(0.0);
        self.off.0 = self.off.0.clamp(-maxx, maxx);
    }
    fn scroll_to_page(&mut self, i: usize) {
        let (v, _, _) = self.sm_layout();
        if let Some((y, _)) = v.get(i) {
            self.sy = y - self.sm_pad() / 2.0;
            self.cur = i;
        }
    }
    /// Картинка страницы i шириной не меньше px точек (из кэша или заново).
    fn page_img(&mut self, i: usize, px: f64) -> std::result::Result<Rc<Img>, String> {
        let p = &self.pages[i];
        let dpi = match p.size_pt() {
            Some((w, _)) => {
                let d = (px / (w / 72.0)).clamp(24.0, 300.0);
                (d / 24.0).ceil() * 24.0
            }
            None => 0.0,
        };
        if let Some((d, img)) = self.page_cache.get(&i) {
            if *d >= dpi {
                return Ok(img.clone());
            }
        }
        let img = doc::render(p, dpi.max(24.0))?;
        if self.page_cache.len() > 14 {
            let cur = self.cur as isize;
            let far: Vec<usize> = {
                let mut k: Vec<usize> = self.page_cache.keys().copied().collect();
                k.sort_by_key(|x| std::cmp::Reverse((*x as isize - cur).abs()));
                k.into_iter().take(self.page_cache.len() - 10).collect()
            };
            for f in far {
                self.page_cache.remove(&f);
            }
        }
        self.page_cache.insert(i, (dpi, img.clone()));
        Ok(img)
    }

    fn paint_scroll(&mut self, ui: &Ui, hdc: HDC) {
        let t = ui.t;
        let c = self.canvas;
        let (lay, total, w) = self.sm_layout();
        self.sm_clamp(total, w);
        let (cw, ch) = (self.cw(), self.ch());
        let x = c.left as f64 + (cw - w) / 2.0 + self.off.0;
        // текущая — та, что на середине окна
        let mid = self.sy + ch / 2.0;
        if let Some(i) = lay.iter().position(|(y, h)| mid < y + h + self.sm_gap()) {
            self.cur = i;
        }
        for i in 0..lay.len() {
            let (y, h) = lay[i];
            let top = c.top as f64 + y - self.sy;
            if top > c.bottom as f64 || top + h < c.top as f64 {
                continue;
            }
            let rc = gui::r(x.round() as i32, top.round() as i32, w.round() as i32, h.round().max(1.0) as i32);
            gui::fill(hdc, &gui::r(rc.left + 2, rc.top + 2, rc.right - rc.left, rc.bottom - rc.top), gui::blend(t.canvas, crate::util::rgb(0, 0, 0), if t.dark { 0.6 } else { 0.85 }));
            match self.page_img(i, w) {
                Ok(img) => gui::blit(hdc, &img, &rc, self.hq),
                Err(e) => {
                    gui::fill(hdc, &rc, t.card);
                    gui::text(hdc, &e, &rc, ui.f.text, t.dim, gui::WRAP);
                }
            }
        }
        // полоса прокрутки (индикатор)
        if total > ch {
            let track = ch - ui.s(16) as f64;
            let th = (track * ch / total).max(ui.s(30) as f64);
            let ty = c.top as f64 + ui.s(8) as f64 + (track - th) * self.sy / (total - ch);
            gui::round(hdc, &gui::r(c.right - ui.s(8), ty as i32, ui.s(4), th as i32), ui.s(4), Some(t.dim), None);
        }
    }

    // ------------------------------------------------------------ хвостики ---
    fn page_thumb(&mut self, i: usize) -> Option<Rc<peek::Peek>> {
        let p = self.pages.get(i)?.clone();
        if let Some(img) = &p.img {
            return Some(Rc::new(peek::Peek::Img(codec::thumb(img, 360))));
        }
        if let Src::File(path) = &p.src {
            return self.peeker.as_mut()?.get(path);
        }
        if let Some(t) = self.page_thumbs.get(&i) {
            return Some(Rc::new(peek::Peek::Img((**t).clone())));
        }
        let dpi = p.size_pt().map(|s| (300.0 / (s.0 / 72.0)).clamp(12.0, 96.0)).unwrap_or(30.0);
        let img = doc::render(&p, dpi).ok()?;
        let th = Rc::new(codec::thumb(&img, 360));
        self.page_thumbs.insert(i, th.clone());
        Some(Rc::new(peek::Peek::Img((*th).clone())))
    }

    fn paint_peeks(&mut self, ui: &Ui, hdc: HDC) {
        let t = ui.t;
        for (d, strip) in [(-1isize, self.peek_l), (1, self.peek_r)] {
            if strip.right <= strip.left {
                continue;
            }
            let Some(nb) = self.neighbor(d) else { continue };
            let hover = self.peek_hover == d as i8;
            let wide = hover && self.mode() != Mode2::Text;
            let ext = if wide { ui.s(150) } else { strip.right - strip.left };
            let s = if d < 0 { RECT { right: strip.left + ext, ..strip } } else { RECT { left: strip.right - ext, ..strip } };
            let (pk, kind, name) = match &nb {
                Nb::Page(i) => (self.page_thumb(*i), Some(FK::Pdf), format!("Стр. {}", i + 1)),
                Nb::File(p) => (self.peeker.as_mut().and_then(|pk| pk.get(p)), kinds::kind_of(p), file_name(p)),
            };
            let area_h = (self.area.bottom - self.area.top) as f64;
            let ch = (area_h * 0.62).min(ui.s(520) as f64);
            let mut cw = ch * peek::aspect(pk.as_deref(), kind);
            let mut chh = ch;
            if cw > ui.s(460) as f64 {
                chh = chh * ui.s(460) as f64 / cw;
                cw = ui.s(460) as f64;
            }
            let cy = (s.top + s.bottom) / 2;
            let gap = ui.s(6);
            let card = if d < 0 {
                gui::r(s.right - gap - cw as i32, cy - chh as i32 / 2, cw as i32, chh as i32)
            } else {
                gui::r(s.left + gap, cy - chh as i32 / 2, cw as i32, chh as i32)
            };
            unsafe {
                let saved = SaveDC(hdc);
                let _ = IntersectClipRect(hdc, s.left, s.top, s.right, s.bottom);
                gui::fill(hdc, &s, if wide { gui::blend(t.canvas, t.bg, 0.5) } else { t.canvas });
                peek::draw(ui, hdc, pk.as_deref(), kind, &card);
                if wide {
                    let lr = gui::r(s.left + ui.s(8), card.bottom + ui.s(8), s.right - s.left - ui.s(16), ui.s(36));
                    gui::text(hdc, &name, &lr, ui.f.small, t.text, gui::WRAP);
                }
                // стрелка-подсказка
                if hover {
                    let ic = if d < 0 { gui::IC_PREV } else { gui::IC_NEXT };
                    let b = ui.s(30);
                    let br = if d < 0 { gui::r(s.left + ui.s(4), cy - b / 2, b, b) } else { gui::r(s.right - ui.s(4) - b, cy - b / 2, b, b) };
                    gui::round(hdc, &br, b, Some(t.tip_bg), None);
                    gui::glyph(ui, hdc, ic, &br, false, t.tip_text);
                }
                let _ = RestoreDC(hdc, saved);
            }
        }
    }

    fn peek_at(&self, x: i32, y: i32) -> i8 {
        let wide = self.mode() != Mode2::Text;
        let grow = if wide { self.s(150) } else { 0 };
        let l = if self.peek_hover == -1 && wide { RECT { right: self.peek_l.left + grow, ..self.peek_l } } else { self.peek_l };
        let r = if self.peek_hover == 1 && wide { RECT { left: self.peek_r.right - grow, ..self.peek_r } } else { self.peek_r };
        if l.right > l.left && gui::inside(&l, x, y) && self.neighbor(-1).is_some() {
            -1
        } else if r.right > r.left && gui::inside(&r, x, y) && self.neighbor(1).is_some() {
            1
        } else {
            0
        }
    }

    // ------------------------------------------------------------ команды ---
    fn command(&mut self, id: u32) -> bool {
        // правки картинки — только в постраничном виде
        if self.scrolling() && matches!(id, ID_FLIP | ID_FLIP_V | ID_CROP | ID_BR_UP | ID_BR_DN | ID_CO_UP | ID_CO_DN | ID_SA_UP | ID_SA_DN | ID_GRAY | ID_RS_50 | ID_RS_1920 | ID_COPY) {
            self.set_scroll(false);
        }
        match id {
            ID_OPEN => self.open(),
            ID_SAVE => {
                if self.mode() == Mode2::Text {
                    self.save_text(false);
                } else {
                    self.save_as(false);
                }
            }
            ID_SAVE_AS => {
                self.save_as(false);
            }
            ID_SAVE_PDF => {
                self.save_as(true);
            }
            ID_CLOSE_FILE => self.close_file(),
            ID_EXIT => unsafe {
                let _ = PostMessageW(self.hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            ID_COPY => match &self.viewer {
                Viewer::Table(t) => {
                    if let Some(s) = t.selection_text(false) {
                        if clip::set_text(self.hwnd, &s) {
                            self.set_status("Ячейка скопирована");
                        }
                    }
                }
                Viewer::Media(m) => {
                    if let Some(f) = m.snapshot() {
                        if clip::set_image(self.hwnd, &f) {
                            self.set_status("Кадр скопирован");
                        }
                    }
                }
                _ => {
                    if let Some(v) = &self.view {
                        if clip::set_image(self.hwnd, v) {
                            self.set_status("Картинка скопирована");
                        }
                    }
                }
            },
            ID_COPY_ROW => {
                if let Viewer::Table(t) = &self.viewer {
                    if let Some(s) = t.selection_text(true) {
                        if clip::set_text(self.hwnd, &s) {
                            self.set_status("Строка скопирована (через табуляцию — вставляется в Excel)");
                        }
                    }
                }
            }
            ID_PASTE => self.paste(),
            ID_UNDO => self.undo(),
            ID_REDO => {
                if let Viewer::Text(t) = &self.viewer {
                    t.redo();
                }
            }
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
            ID_WORK => {
                let on = !self.work;
                self.set_work(on);
            }
            ID_FIT => match &mut self.viewer {
                Viewer::Table(t) => t.zoom = 1.0,
                Viewer::Text(t) => t.zoom(0),
                _ => self.reset_view(),
            },
            ID_100 => self.actual_size(),
            ID_ZIN => self.zoom_center(1.25),
            ID_ZOUT => self.zoom_center(0.8),
            ID_FULL => self.toggle_full(),
            ID_BOOK => self.set_scroll(false),
            ID_SCROLL => self.set_scroll(true),
            ID_PRINT => self.print(),
            ID_INSERT => {
                if self.mode() != Mode2::Pages {
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
            ID_CONVERT => self.convert(),
            ID_FIND => {
                if let Viewer::Text(t) = &mut self.viewer {
                    t.open_find(false);
                }
            }
            ID_REPLACE_UI => {
                if let Viewer::Text(t) = &mut self.viewer {
                    t.open_find(true);
                }
            }
            ID_WRAP => {
                if let Viewer::Text(t) = &mut self.viewer {
                    t.toggle_wrap();
                }
            }
            textview::ID_FIND_NEXT | textview::ID_FIND_PREV => {
                if let Viewer::Text(t) = &mut self.viewer {
                    if !t.find_next(id == textview::ID_FIND_NEXT) {
                        self.set_status("Не найдено");
                    }
                }
            }
            textview::ID_REPLACE => {
                if let Viewer::Text(t) = &mut self.viewer {
                    if !t.replace_open {
                        t.open_find(true);
                    } else if !t.replace_one() {
                        self.set_status("Больше не найдено");
                    }
                }
            }
            textview::ID_REPLACE_ALL => {
                if let Viewer::Text(t) = &mut self.viewer {
                    let n = t.replace_all();
                    self.set_status(format!("Заменено: {n}"));
                }
            }
            textview::ID_FIND_CASE => {
                if let Viewer::Text(t) = &mut self.viewer {
                    t.match_case = !t.match_case;
                }
            }
            textview::ID_FIND_CLOSE => {
                if let Viewer::Text(t) = &mut self.viewer {
                    t.close_find();
                }
            }
            ID_PLAY | ID_BACK10 | ID_FWD10 | ID_MUTE | ID_LOOP | ID_SPEED => {
                if let Viewer::Media(m) = &mut self.viewer {
                    match id {
                        ID_PLAY => m.toggle_play(),
                        ID_BACK10 => m.step(-10.0),
                        ID_FWD10 => m.step(10.0),
                        ID_MUTE => m.toggle_mute(),
                        ID_LOOP => m.toggle_loop(),
                        _ => m.cycle_rate(),
                    }
                }
            }
            ID_FRAME => self.save_frame(),
            ID_HEADER => {
                if let Viewer::Table(t) = &mut self.viewer {
                    t.toggle_header();
                }
            }
            ID_UNSORT => {
                if let Viewer::Table(t) = &mut self.viewer {
                    t.reset_sort();
                }
            }
            ID_MORE => {
                let items: Vec<(u32, String, bool)> = self.overflow.iter().map(|(i, s)| (*i, s.clone(), false)).collect();
                let (x, y) = gui::mouse_pos(self.hwnd);
                let r = gui::popup_menu(self.hwnd, x, y, &items);
                if r != 0 {
                    return self.command(r);
                }
            }
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
                    "{APP_NAME} {VERSION}\n\nЛёгкий просмотрщик: фото, PDF, документы, таблицы, тексты и код, аудио и видео.\n\
                     Правка фото, коллаж, склейка PDF, печать, конвертация.\n\
                     © 2026 Даниил Кочубейник\nЛицензия: GNU GPL v3 или новее. Гарантий нет.\n\n\
                     Используются lopdf, calamine, zip, quick-xml, pulldown-cmark (MIT/Apache).\n\
                     Кодеки и PDF — системные компоненты Windows."
                ),
                false,
            ),
            t if (ID_TAB0..ID_TAB0 + TABS.len() as u32).contains(&t) => {
                self.tab = (t - ID_TAB0) as usize;
                settings::set_i("tab", self.tab as i32);
            }
            s if (crate::table::ID_SHEET0..crate::table::ID_SHEET0 + 64).contains(&s) => {
                if let Viewer::Table(t) = &mut self.viewer {
                    t.select_sheet((s - crate::table::ID_SHEET0) as usize);
                }
            }
            _ => {}
        }
        true
    }

    // --------------------------------------------- лента и панель по типу ---
    /// Вкладки, которые есть смысл показывать для текущего файла.
    fn tabs(&self) -> Vec<usize> {
        match self.mode() {
            Mode2::Pages => {
                let mut v = vec![0];
                if self.pages.iter().any(|p| !p.is_docpage() || p.img.is_some()) || self.pages.len() == 1 {
                    v.push(1);
                }
                v.extend([2, 3, 4]);
                v
            }
            Mode2::Empty => vec![0, 3, 4],
            _ => vec![0, 3, 4],
        }
    }

    fn ribbon(&self, tab: usize) -> Vec<Option<Rb>> {
        let mut v = self.ribbon_base(tab);
        if tab == 0 && (self.can_work() || self.work) {
            let (l, ic, tip) = self.work_label();
            v.insert(0, None);
            v.insert(0, Some((ID_WORK, ic, l, tip)));
        }
        v
    }

    fn ribbon_base(&self, tab: usize) -> Vec<Option<Rb>> {
        use crate::gui::*;
        let m = self.mode();
        let s = None;
        let b = |id, ic, l, t| Some((id, ic, l, t));
        match (tab, m) {
            (0, Mode2::Table) => vec![
                b(ID_OPEN, IC_OPEN, "Открыть", "Открыть файл (Ctrl+O)"),
                s,
                b(ID_HEADER, IC_TABLE_HEAD, "Заголовок", "Первая строка — заголовок: закреплена и не сортируется"),
                b(ID_UNSORT, IC_SORT, "Без сортировки", "Вернуть исходный порядок строк (щелчок по заголовку столбца — сортировка)"),
                s,
                b(ID_COPY, IC_COPY, "Копировать", "Копировать ячейку (Ctrl+C)"),
                b(ID_COPY_ROW, IC_COPY, "Строку", "Копировать строку целиком"),
                s,
                b(ID_CLOSE_FILE, IC_CLOSE, "Закрыть", "Закрыть файл (Ctrl+W)"),
            ],
            (0, Mode2::Text) => vec![
                b(ID_OPEN, IC_OPEN, "Открыть", "Открыть файл (Ctrl+O)"),
                b(ID_SAVE, IC_SAVE, "Сохранить", "Сохранить (Ctrl+S)"),
                b(ID_SAVE_AS, IC_SAVE, "Сохранить как", "Сохранить под другим именем (Ctrl+Shift+S)"),
                s,
                b(ID_UNDO, IC_UNDO, "Отменить", "Отменить (Ctrl+Z)"),
                b(ID_REDO, IC_REDO, "Вернуть", "Вернуть (Ctrl+Y)"),
                s,
                b(ID_FIND, IC_FIND, "Найти", "Найти (Ctrl+F, далее — F3)"),
                b(ID_REPLACE_UI, IC_FIND, "Заменить", "Найти и заменить (Ctrl+H)"),
                b(ID_WRAP, IC_WRAP, "Перенос строк", "Переносить длинные строки"),
                s,
                b(ID_CLOSE_FILE, IC_CLOSE, "Закрыть", "Закрыть файл (Ctrl+W)"),
            ],
            (0, Mode2::Media) => {
                let mut v = vec![
                    b(ID_OPEN, IC_OPEN, "Открыть", "Открыть файл (Ctrl+O)"),
                    s,
                    b(ID_PLAY, IC_PLAY, "Пуск / пауза", "Воспроизвести или остановить (пробел)"),
                    b(ID_BACK10, IC_BACK, "−10 с", "Назад на 10 секунд (←: 5 с)"),
                    b(ID_FWD10, IC_FORWARD, "+10 с", "Вперёд на 10 секунд (→: 5 с)"),
                    s,
                    b(ID_MUTE, IC_VOLUME, "Звук", "Выключить / включить звук (M). Колесо мыши — громкость"),
                    b(ID_LOOP, IC_REPEAT, "Повтор", "Повторять по кругу"),
                    b(ID_SPEED, IC_SPEED, "Скорость", "Скорость: 0,5× … 2×"),
                    s,
                ];
                if self.kind == Some(FK::Video) {
                    v.push(b(ID_FRAME, IC_CAMERA, "Кадр", "Сохранить текущий кадр как фото"));
                }
                v.push(b(ID_CONVERT, IC_CONVERT, "Конвертировать", "Конвертировать в другой формат (можно несколько файлов)"));
                v.push(s);
                v.push(b(ID_CLOSE_FILE, IC_CLOSE, "Закрыть", "Закрыть файл (Ctrl+W)"));
                v
            }
            (0, _) => vec![
                b(ID_OPEN, IC_OPEN, "Открыть", "Открыть файл (Ctrl+O)"),
                b(ID_SAVE, IC_SAVE, "Сохранить", "Сохранить как… (Ctrl+S)"),
                b(ID_SAVE_PDF, IC_DOC, "В PDF", "Сохранить все страницы в PDF (Ctrl+Shift+S)"),
                b(ID_PRINT, IC_PRINT, "Печать", "Печать с предпросмотром (Ctrl+P)"),
                s,
                b(ID_UNDO, IC_UNDO, "Отменить", "Отменить (Ctrl+Z)"),
                b(ID_ROT_L, IC_ROT_L, "Влево", "Повернуть влево (Ctrl+L)"),
                b(ID_ROT_R, IC_ROT_R, "Вправо", "Повернуть вправо (Ctrl+R)"),
                b(ID_FLIP, IC_FLIP, "Отразить", "Отразить по горизонтали (Ctrl+H)"),
                b(ID_CROP, IC_CROP, "Обрезать", "Обрезка: выделите область, Enter — применить (Ctrl+K)"),
                s,
                b(ID_COPY, IC_COPY, "Копировать", "Копировать картинку (Ctrl+C)"),
                b(ID_PASTE, IC_PASTE, "Вставить", "Вставить картинку или файлы из буфера (Ctrl+V)"),
            ],
            (1, _) => vec![
                b(ID_BR_UP, IC_SUN, "Ярче", "Яркость +10%"),
                b(ID_BR_DN, IC_SUN, "Темнее", "Яркость −10%"),
                s,
                b(ID_CO_UP, IC_CONTRAST, "Контраст +", "Контраст +10%"),
                b(ID_CO_DN, IC_CONTRAST, "Контраст −", "Контраст −10%"),
                s,
                b(ID_SA_UP, IC_COLOR, "Насыщ. +", "Насыщенность +20%"),
                b(ID_SA_DN, IC_COLOR, "Насыщ. −", "Насыщенность −20%"),
                b(ID_GRAY, IC_COLOR, "Ч/Б", "Сделать чёрно-белым"),
                s,
                b(ID_RS_50, IC_PICTURE, "Размер ½", "Уменьшить вдвое"),
                b(ID_RS_1920, IC_FULLSCREEN, "1920 px", "1920 точек по ширине"),
                s,
                b(ID_FLIP_V, IC_FLIP, "Отразить ↕", "Отразить по вертикали"),
                b(ID_UNDO, IC_UNDO, "Отменить", "Отменить (Ctrl+Z)"),
            ],
            (2, _) => vec![
                b(ID_INSERT, IC_ADD, "Вставить", "Вставить фото или страницы PDF после текущей (Ctrl+I)"),
                b(ID_DEL_PAGE, IC_DELETE, "Удалить", "Удалить текущую страницу (Delete)"),
                b(ID_MOVE_L, IC_BACK, "Раньше", "Переместить страницу назад"),
                b(ID_MOVE_R, IC_FORWARD, "Позже", "Переместить страницу вперёд"),
                s,
                b(ID_ROT_L, IC_ROT_L, "Влево", "Повернуть страницу влево"),
                b(ID_ROT_R, IC_ROT_R, "Вправо", "Повернуть страницу вправо"),
                s,
                b(ID_SAVE_PDF, IC_DOC, "Сохранить PDF", "Сохранить все страницы в PDF (Ctrl+Shift+S)"),
                b(ID_MERGE, IC_MERGE, "Объединить", "Объединить файлы в один PDF (Ctrl+M)"),
            ],
            (3, _) => vec![
                b(ID_COLLAGE, IC_GRID, "Коллаж", "Склеить несколько фото в одно (Ctrl+G)"),
                b(ID_MERGE, IC_MERGE, "Объединить в PDF", "Фото, PDF и документы → один PDF (Ctrl+M)"),
                b(ID_CONVERT, IC_CONVERT, "Конвертировать", "Фото, аудио, видео → другой формат; сразу много файлов (Ctrl+E)"),
                s,
                b(ID_PRINT, IC_PRINT, "Печать", "Печать с предпросмотром (Ctrl+P)"),
            ],
            _ => {
                let mut v = vec![
                    b(ID_PREV, IC_PREV, "Назад", "Предыдущий файл / страница"),
                    b(ID_NEXT, IC_NEXT, "Вперёд", "Следующий файл / страница"),
                    s,
                ];
                if m != Mode2::Media {
                    v.extend([
                        b(ID_ZOUT, IC_ZOOM_OUT, "Меньше", "Уменьшить (Ctrl+−)"),
                        b(ID_FIT, IC_FIT, "Вписать", "Вписать / обычный размер (Ctrl+0)"),
                    ]);
                    if m == Mode2::Pages {
                        v.push(b(ID_100, IC_PICTURE, "100%", "Реальный размер (Ctrl+1)"));
                    }
                    v.push(b(ID_ZIN, IC_ZOOM_IN, "Больше", "Увеличить (Ctrl++)"));
                }
                v.push(b(ID_FULL, IC_FULLSCREEN, "Весь экран", "Во весь экран (F11)"));
                if m == Mode2::Pages && self.pages.len() > 1 {
                    v.extend([s, b(ID_BOOK, IC_BOOK, "Книга", "По одной странице, листать ← →"), b(ID_SCROLL, IC_SCROLL, "Прокрутка", "Все страницы лентой, листать колесом")]);
                }
                if m == Mode2::Text {
                    v.extend([s, b(ID_WRAP, IC_WRAP, "Перенос строк", "Переносить длинные строки")]);
                }
                v.extend([
                    s,
                    b(ID_THEME_SYS, IC_MONITOR, "Как в системе", "Тема как в Windows"),
                    b(ID_THEME_LIGHT, IC_SUN, "Светлая", "Светлая тема"),
                    b(ID_THEME_DARK, IC_MOON, "Тёмная", "Тёмная тема"),
                    s,
                    b(ID_KEYS, IC_KEYBOARD, "Клавиши", "Горячие клавиши (F1)"),
                    b(ID_ABOUT, IC_INFO, "О программе", "О программе"),
                ]);
                v
            }
        }
    }

    fn enabled(&self, id: u32) -> bool {
        let has = self.view.is_some() || (self.scrolling() && !self.pages.is_empty());
        let multi = self.pages.len() > 1;
        let can_rot = has || self.pages.get(self.cur).map(|p| p.is_docpage() && p.img.is_none()).unwrap_or(false);
        match id {
            ID_OPEN | ID_KEYS | ID_ABOUT | ID_COLLAGE | ID_MERGE | ID_CONVERT | ID_INSERT | ID_FULL | ID_MORE | ID_WORK => true,
            ID_THEME_SYS | ID_THEME_LIGHT | ID_THEME_DARK | ID_BOOK | ID_SCROLL | ID_CLOSE_FILE => true,
            ID_PASTE => matches!(self.mode(), Mode2::Pages | Mode2::Empty),
            ID_UNDO => match &self.viewer {
                Viewer::Text(t) => t.can_undo(),
                _ => !self.history.is_empty(),
            },
            ID_DEL_PAGE | ID_MOVE_L | ID_MOVE_R => multi,
            ID_PREV => self.neighbor(-1).is_some() || (self.scrolling() && self.cur > 0),
            ID_NEXT => self.neighbor(1).is_some() || (self.scrolling() && self.cur + 1 < self.pages.len()),
            ID_SAVE_PDF | ID_PRINT => !self.pages.is_empty(),
            ID_SAVE => !self.pages.is_empty() || self.mode() == Mode2::Text,
            ID_ROT_L | ID_ROT_R => can_rot,
            ID_UNSORT => matches!(&self.viewer, Viewer::Table(t) if t.sort.is_some()),
            ID_COPY | ID_COPY_ROW => match &self.viewer {
                Viewer::Table(t) => t.sel.is_some(),
                Viewer::Media(m) => m.frame.is_some(),
                _ => has,
            },
            ID_FRAME => matches!(&self.viewer, Viewer::Media(m) if m.frame.is_some()),
            ID_ZIN | ID_ZOUT | ID_FIT | ID_100 => has || matches!(self.mode(), Mode2::Table | Mode2::Text),
            _ => has || !self.viewer_is_none(),
        }
    }

    fn is_on(&self, id: u32) -> bool {
        match id {
            ID_THEME_SYS => theme::mode() == Mode::System,
            ID_THEME_LIGHT => theme::mode() == Mode::Light,
            ID_THEME_DARK => theme::mode() == Mode::Dark,
            ID_CROP => self.crop_mode,
            ID_FULL => self.full.is_some(),
            ID_WORK => self.work,
            ID_BOOK => !self.scroll_mode,
            ID_SCROLL => self.scroll_mode,
            ID_HEADER => matches!(&self.viewer, Viewer::Table(t) if t.header),
            ID_WRAP => matches!(&self.viewer, Viewer::Text(t) if t.wrap),
            ID_MUTE => matches!(&self.viewer, Viewer::Media(m) if m.muted),
            ID_LOOP => matches!(&self.viewer, Viewer::Media(m) if m.looped),
            ID_SPEED => matches!(&self.viewer, Viewer::Media(m) if (m.rate - 1.0).abs() > 0.01),
            _ => false,
        }
    }

    /// Значок, который зависит от состояния (пуск/пауза, звук).
    fn live_icon(&self, id: u32, icon: u32) -> u32 {
        match (id, &self.viewer) {
            (ID_PLAY, Viewer::Media(m)) => if m.playing { gui::IC_PAUSE } else { gui::IC_PLAY },
            (ID_MUTE, Viewer::Media(m)) => if m.muted { gui::IC_MUTE } else { gui::IC_VOLUME },
            _ => icon,
        }
    }

    /// Нижняя панель: (id, значок, текст, подсказка, приоритет). None — разделитель.
    /// Чем больше приоритет, тем раньше кнопка прячется в узком окне.
    fn bottom_items(&self) -> Vec<Option<(u32, u32, String, &'static str, u8)>> {
        use crate::gui::*;
        let b = |id: u32, ic: u32, t: &str, tip: &'static str, p: u8| Some((id, ic, t.to_string(), tip, p));
        let nav = [b(ID_PREV, IC_PREV, "", "Предыдущий файл / страница", 0), b(ID_NEXT, IC_NEXT, "", "Следующий файл / страница", 0), None];
        let mut v: Vec<Option<(u32, u32, String, &'static str, u8)>> =
            if self.work && !self.viewer_is_none() { vec![] } else { nav.to_vec() };
        match self.mode() {
            Mode2::Table => v.extend([
                b(ID_ZOUT, IC_ZOOM_OUT, "", "Мельче (Ctrl+−)", 2),
                b(ID_FIT, 0, "100%", "Обычный размер (Ctrl+0)", 3),
                b(ID_ZIN, IC_ZOOM_IN, "", "Крупнее (Ctrl++)", 2),
                None,
                b(ID_HEADER, IC_TABLE_HEAD, "", "Первая строка — заголовок", 1),
                b(ID_UNSORT, IC_SORT, "", "Сбросить сортировку", 1),
                None,
                b(ID_COPY, IC_COPY, "", "Копировать ячейку (Ctrl+C)", 2),
            ]),
            Mode2::Text => v.extend([
                b(ID_ZOUT, IC_ZOOM_OUT, "", "Мельче (Ctrl+−)", 3),
                b(ID_ZIN, IC_ZOOM_IN, "", "Крупнее (Ctrl++)", 3),
                None,
                b(ID_UNDO, IC_UNDO, "", "Отменить (Ctrl+Z)", 1),
                b(ID_REDO, IC_REDO, "", "Вернуть (Ctrl+Y)", 2),
                None,
                b(ID_FIND, IC_FIND, "", "Найти (Ctrl+F)", 1),
                b(ID_WRAP, IC_WRAP, "", "Перенос строк", 2),
                None,
                b(ID_SAVE, IC_SAVE, "", "Сохранить (Ctrl+S)", 0),
            ]),
            Mode2::Media => {
                let rate = match &self.viewer {
                    Viewer::Media(m) => format!("{}×", crate::table::fmt_num(m.rate)),
                    _ => "1×".into(),
                };
                v.extend([
                    b(ID_BACK10, IC_BACK, "", "Назад 10 с", 2),
                    b(ID_PLAY, IC_PLAY, "", "Пуск / пауза (пробел)", 0),
                    b(ID_FWD10, IC_FORWARD, "", "Вперёд 10 с", 2),
                    None,
                    b(ID_MUTE, IC_VOLUME, "", "Звук вкл/выкл (M), колесо — громкость", 1),
                    b(ID_LOOP, IC_REPEAT, "", "Повтор", 3),
                    b(ID_SPEED, 0, &rate, "Скорость воспроизведения", 3),
                    None,
                ]);
                if self.kind == Some(FK::Video) {
                    v.push(b(ID_FRAME, IC_CAMERA, "", "Сохранить кадр как фото", 2));
                }
                v.push(b(ID_CONVERT, IC_CONVERT, "", "Конвертировать", 2));
            }
            _ => {
                v.extend([
                    b(ID_ZOUT, IC_ZOOM_OUT, "", "Уменьшить (Ctrl+−)", 2),
                    b(ID_INSERT, IC_ADD, "", "Вставить фото или страницу (Ctrl+I)", 3),
                    b(ID_FIT, IC_FIT, "", "Вписать в окно — сброс масштаба (Ctrl+0)", 1),
                    b(ID_100, 0, "1:1", "Реальный размер, 100% (Ctrl+1)", 3),
                    b(ID_ZIN, IC_ZOOM_IN, "", "Увеличить (Ctrl++)", 2),
                    None,
                    b(ID_ROT_L, IC_ROT_L, "", "Повернуть влево (Ctrl+L)", 2),
                    b(ID_ROT_R, IC_ROT_R, "", "Повернуть вправо (Ctrl+R)", 1),
                    None,
                    b(ID_CROP, IC_CROP, "", "Обрезать (Ctrl+K)", 3),
                    None,
                    b(ID_UNDO, IC_UNDO, "", "Отменить (Ctrl+Z)", 2),
                    None,
                    b(ID_PRINT, IC_PRINT, "", "Печать (Ctrl+P)", 2),
                ]);
                if self.pages.len() > 1 {
                    let (id, ic, tip) = if self.scroll_mode {
                        (ID_BOOK, IC_BOOK, "Режим «Книга»: по одной странице")
                    } else {
                        (ID_SCROLL, IC_SCROLL, "Режим «Прокрутка»: все страницы лентой")
                    };
                    v.extend([None, b(id, ic, "", tip, 1)]);
                }
            }
        }
        v
    }
}

const KEYS: &str = "ДВА РЕЖИМА\n\
Обзор — ← → листают файлы папки, по краям видны соседи.\n\
Работа — Enter или кнопка «Читать / Править / Работать» (у документов, текстов, таблиц):\n\
   документ — ← → страницы; текст — правка; таблица — ходить по ячейкам.\n\
   Esc или «К файлам» — обратно в обзор. Alt+← → и Ctrl+PgUp/PgDn — соседний файл всегда.\n\n\
Ctrl+O — открыть (можно несколько файлов)\nCtrl+S — сохранить\nCtrl+Shift+S — сохранить в PDF / как…\n\
Ctrl+P — печать\nCtrl+C / Ctrl+V — копировать / вставить\nCtrl+W — закрыть файл\nCtrl+E — конвертация\n\n\
Home / End — первая / последняя страница (в работе)\nCtrl+I — вставить фото/страницы\nDelete — удалить страницу\n\
Ctrl+L / Ctrl+R — повернуть\nCtrl+H — отразить (в тексте — замена)\nCtrl+K — обрезка (Enter — применить, Esc — отмена)\n\
Ctrl+Z — отменить\nCtrl+G — коллаж\nCtrl+M — объединить в PDF\n\n\
Текст: Ctrl+F — найти, F3 — далее, Ctrl+H — заменить\n\
Медиа: пробел — пуск/пауза, Shift+← → — 5 с, ↑ ↓ — громкость, M — звук\n\
Таблица: щелчок по заголовку — сортировка ▲ ▼ (файл не меняется)\n\n\
Колесо мыши — масштаб (в прокрутке и таблице — листать; Ctrl+колесо — масштаб)\nДвойной клик — «Читать»; в работе — вписать\nF11 — во весь экран";

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
        self.dpi = gui::dpi_of(hwnd);
        self.peeker = Some(Peeker::new(hwnd));
        unsafe {
            DragAcceptFiles(hwnd, true);
            SetTimer(hwnd, TIMER_START, 1, None);
        }
    }

    fn min_size(&self) -> (i32, i32) {
        (560, 400)
    }

    fn layout(&mut self, ui: &Ui, w: i32, h: i32) -> Vec<W> {
        use crate::gui::Kind;
        self.dpi = ui.dpi;
        let mut v = vec![];
        let mode = self.mode();
        if self.full.is_some() {
            self.area = gui::r(0, 0, w, h);
            self.canvas = self.area;
            self.peek_l = RECT::default();
            self.peek_r = RECT::default();
            self.layout_viewer(ui, &mut v);
            return v;
        }
        let tab_h = ui.s(38);
        let rib_h = ui.s(78);
        let bot_h = ui.s(50);
        let top = tab_h + rib_h + 1;
        self.area = RECT { left: 0, top, right: w, bottom: (h - bot_h).max(top + 1) };
        // хвостики: полоски по краям, если есть сосед хотя бы с одной стороны
        let has_nb = self.neighbor(-1).is_some() || self.neighbor(1).is_some();
        let pw = if has_nb && w > ui.s(520) { ui.s(if mode == Mode2::Text { 22 } else { 34 }) } else { 0 };
        self.peek_l = RECT { right: self.area.left + pw, ..self.area };
        self.peek_r = RECT { left: self.area.right - pw, ..self.area };
        self.canvas = RECT { left: self.area.left + pw, right: self.area.right - pw, ..self.area };
        if self.work {
            self.canvas.top += ui.s(3); // место под полосу «режим работы»
        }

        // вкладки — только нужные этому типу файла
        let tabs = self.tabs();
        let tab = if tabs.contains(&self.tab) { self.tab } else { 0 };
        let mut x = ui.s(8);
        for i in tabs {
            let t = TABS[i];
            let tw = ui.tw(ui.f.bold, t) + ui.s(28);
            v.push(W::new(Kind::Tab, ID_TAB0 + i as u32, gui::r(x, ui.s(4), tw, tab_h - ui.s(6))).t(t).on(i == tab));
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

        // лента: подписи → только значки → «ещё…»
        let items = self.ribbon(tab);
        let y = tab_h + ui.s(4);
        let bh = rib_h - ui.s(8);
        let avail = w - ui.s(16);
        let sep_w = ui.s(8);
        let full_w = |it: &Rb| (ui.tw(ui.f.small, it.2) + ui.s(16)).max(ui.s(58)) + ui.s(2);
        let total: i32 = items.iter().map(|i| i.as_ref().map(full_w).unwrap_or(sep_w)).sum();
        let compact = total > avail;
        let cbw = ui.s(44);
        let item_w = |it: &Rb| if compact { cbw + ui.s(2) } else { full_w(it) };
        let ctotal: i32 = items.iter().map(|i| i.as_ref().map(item_w).unwrap_or(sep_w)).sum();
        let mut limit = avail;
        if ctotal > avail {
            limit = avail - cbw - ui.s(4);
        }
        self.overflow.clear();
        let mut x = ui.s(8);
        let mut used = 0;
        let mut cut = false;
        let mut last_sep = false;
        for it in &items {
            match it {
                None => {
                    if cut || used == 0 || last_sep {
                        continue;
                    }
                    if used + sep_w > limit {
                        cut = true;
                        continue;
                    }
                    v.push(W::new(Kind::Sep, 0, gui::r(x + ui.s(3), y + ui.s(8), ui.s(2), bh - ui.s(16))));
                    x += sep_w;
                    used += sep_w;
                    last_sep = true;
                }
                Some(rb) => {
                    let iw = item_w(rb);
                    if cut || used + iw > limit {
                        cut = true;
                        if self.enabled(rb.0) {
                            self.overflow.push((rb.0, rb.2.to_string()));
                        }
                        continue;
                    }
                    let icon = self.live_icon(rb.0, rb.1);
                    let wd = if compact {
                        W::new(Kind::Tool, rb.0, gui::r(x, y + (bh - cbw) / 2, cbw, cbw)).i(icon).tip(format!("{} — {}", rb.2, rb.3))
                    } else {
                        W::new(Kind::Ribbon, rb.0, gui::r(x, y, iw - ui.s(2), bh)).i(icon).t(rb.2).tip(rb.3)
                    };
                    v.push(wd.en(self.enabled(rb.0)).on(self.is_on(rb.0)));
                    x += iw;
                    used += iw;
                    last_sep = false;
                }
            }
        }
        if !self.overflow.is_empty() {
            v.push(W::new(Kind::Tool, ID_MORE, gui::r(x + ui.s(4), y + (bh - cbw) / 2, cbw, cbw)).i(gui::IC_MORE).tip("Ещё команды"));
        }

        // нижняя панель — свой набор для каждого типа; в узком окне лишнее прячется
        let by = h - bot_h + ui.s(7);
        let tb = ui.s(36);
        let gap = ui.s(2);
        let sepw = ui.s(14);
        let info = self.info();
        let info_w = (ui.tw(ui.f.text, &info) + ui.s(16)).min(w / 2);
        let mut items = self.bottom_items();
        let width = |it: &Vec<Option<(u32, u32, String, &'static str, u8)>>| -> i32 { it.iter().map(|i| if i.is_some() { tb + gap } else { sepw }).sum() };
        // кнопка «Читать / Править / Работать» ↔ «К файлам» — слева
        let work_btn = if self.can_work() || self.work {
            let (l, ic, tip) = self.work_label();
            let bw = ui.tw(ui.f.text, l) + ui.s(46);
            Some((l, ic, tip, bw))
        } else {
            None
        };
        let lead = work_btn.map(|b| b.3 + ui.s(22)).unwrap_or(0);
        let room = w - info_w - ui.s(24) - lead;
        while width(&items) > room {
            let worst = items.iter().enumerate().filter_map(|(k, i)| i.as_ref().map(|x| (k, x.4))).filter(|x| x.1 > 0).max_by_key(|x| (x.1, x.0));
            match worst {
                Some((k, _)) => {
                    items.remove(k);
                }
                None => break,
            }
            // убрать двойные и крайние разделители
            let mut cleaned: Vec<Option<(u32, u32, String, &'static str, u8)>> = vec![];
            for i in items.drain(..) {
                if i.is_none() && (cleaned.is_empty() || cleaned.last().map(|l| l.is_none()).unwrap_or(false)) {
                    continue;
                }
                cleaned.push(i);
            }
            while cleaned.last().map(|l| l.is_none()).unwrap_or(false) {
                cleaned.pop();
            }
            items = cleaned;
        }
        let total = width(&items);
        // по центру, но не наезжая на сведения справа
        let mut x = ((w - total) / 2).min(w - total - info_w - ui.s(8)).max(ui.s(8) + lead);
        if let Some((l, ic, tip, bw)) = work_btn {
            v.push(W::new(Kind::Button, ID_WORK, gui::r(ui.s(12), by, bw, tb)).t(l).i(ic).tip(tip).on(!self.work));
        }
        let status_x = ui.s(14) + lead;
        let status_w = x - ui.s(16) - lead;
        for it in items {
            match it {
                Some((id, icon, txt, tip, _)) => {
                    let icon = self.live_icon(id, icon);
                    v.push(W::new(Kind::Tool, id, gui::r(x, by, tb, tb)).i(icon).t(txt).tip(tip).en(self.enabled(id)).on(self.is_on(id)));
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
        let left_text = if self.peek_hover != 0 {
            self.neighbor(self.peek_hover as isize).map(|nb| match nb {
                Nb::Page(i) => format!("{} стр. {}", if self.peek_hover < 0 { "←" } else { "→" }, i + 1),
                Nb::File(p) => format!("{} {}", if self.peek_hover < 0 { "←" } else { "→" }, file_name(&p)),
            })
        } else {
            None
        };
        if let Some(lt) = left_text.or_else(|| (!self.status.is_empty()).then(|| self.status.clone())) {
            if status_w > ui.s(80) {
                v.push(W::new(Kind::Label, 0, gui::r(status_x, by, status_w, tb)).t(lt).color(ui.t.accent));
            }
        }

        // пустое окно — кнопка «Открыть»
        if mode == Mode2::Empty && self.view_err.is_none() {
            let bw = ui.s(200);
            let cx = (self.canvas.left + self.canvas.right) / 2;
            let cy = (self.canvas.top + self.canvas.bottom) / 2;
            v.push(W::new(Kind::Button, ID_OPEN, gui::r(cx - bw / 2, cy + ui.s(10), bw, ui.s(38))).t("Открыть файл…").i(gui::IC_OPEN).on(true));
            v.push(W::new(Kind::Button, ID_PASTE, gui::r(cx - bw / 2, cy + ui.s(56), bw, ui.s(34))).t("Вставить из буфера").i(gui::IC_PASTE));
        }
        self.layout_viewer(ui, &mut v);
        v
    }

    fn paint(&mut self, ui: &Ui, hdc: HDC, w: i32, h: i32) {
        let t = &ui.t;
        let a = self.area;
        let c = self.canvas;
        if self.full.is_none() {
            gui::fill(hdc, &gui::r(0, 0, w, a.top), t.bg);
            gui::line(hdc, 0, a.top - 1, w, a.top - 1, t.border, 1, PS_SOLID);
            gui::fill(hdc, &gui::r(0, a.bottom, w, h - a.bottom), t.bg);
            gui::line(hdc, 0, a.bottom, w, a.bottom, t.border, 1, PS_SOLID);
        }
        let black = self.full.is_some() || (self.mode() == Mode2::Media && self.kind == Some(FK::Video));
        gui::fill(hdc, &a, if self.full.is_some() { crate::util::rgb(0, 0, 0) } else { t.canvas });
        if black {
            gui::fill(hdc, &c, crate::util::rgb(0, 0, 0));
        }
        if self.full.is_none() {
            self.paint_peeks(ui, hdc);
        }
        if self.work && self.full.is_none() {
            // режим работы — тонкая полоса акцентного цвета над холстом
            gui::fill(hdc, &gui::r(a.left, a.top, a.right - a.left, ui.s(3)), t.accent);
        }
        unsafe {
            let _ = IntersectClipRect(hdc, c.left, c.top, c.right, c.bottom);
        }
        let hq = self.hq;
        match &mut self.viewer {
            Viewer::Table(tv) => tv.paint(ui, hdc),
            Viewer::Text(tv) => tv.paint(ui, hdc, c),
            Viewer::Media(m) => m.paint(ui, hdc, c, hq),
            Viewer::None => {}
        }
        if self.viewer_is_none() {
            if self.scrolling() {
                self.paint_scroll(ui, hdc);
            } else {
                self.paint_pages(ui, hdc);
            }
        }
        // хвостик при наведении — поверх содержимого (кроме текста: там редактор)
        if self.peek_hover != 0 && self.full.is_none() && self.mode() != Mode2::Text {
            unsafe {
                let _ = SelectClipRgn(hdc, None);
            }
            self.paint_peeks(ui, hdc);
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
        let mode = self.mode();
        // общие для всех режимов
        if ctrl {
            let id = match vk {
                0x4F => ID_OPEN,
                0x53 if shift => if mode == Mode2::Text { ID_SAVE_AS } else { ID_SAVE_PDF },
                0x53 => ID_SAVE,
                0x50 => ID_PRINT,
                0x51 => ID_EXIT,
                0x57 => ID_CLOSE_FILE,
                0x45 => ID_CONVERT,
                0x47 => ID_COLLAGE,
                0x4D => ID_MERGE,
                0x21 => ID_PREV,
                0x22 => ID_NEXT,
                0x09 => {
                    // Ctrl+Tab — следующая вкладка
                    let tabs = self.tabs();
                    let k = tabs.iter().position(|t| *t == self.tab).unwrap_or(0);
                    let n = tabs.len();
                    self.tab = tabs[(k + if shift { n - 1 } else { 1 }) % n];
                    return Some(true);
                }
                _ => 0,
            };
            if id != 0 {
                if id == ID_PREV || id == ID_NEXT {
                    // соседний файл из любого режима
                    self.step_file(if id == ID_PREV { -1 } else { 1 });
                    return Some(true);
                }
                self.command(id);
                return Some(true);
            }
        }
        if vk == 0x70 {
            self.command(ID_KEYS);
            return Some(true);
        }
        if vk == 0x7A {
            self.command(ID_FULL);
            return Some(true);
        }
        // Alt+← / Alt+→ — соседний файл в любом режиме (даже из редактора)
        if key_down(0x12) && (vk == 0x25 || vk == 0x27) {
            self.step_file(if vk == 0x25 { -1 } else { 1 });
            return Some(true);
        }
        // Enter — «войти» в файл (документ, текст, таблица)
        if vk == 0x0D && !ctrl && !self.work && !self.crop_mode && self.can_work() {
            self.set_work(true);
            return Some(true);
        }
        let work = self.work;
        let ui = Ui::new(self.dpi);
        match &mut self.viewer {
            Viewer::Text(t) => {
                if !work {
                    // обзор: стрелки — файлы, остальное листает текст; буква — сразу правка
                    match (vk, ctrl) {
                        (0x25 | 0x27, false) => self.step_file(if vk == 0x25 { -1 } else { 1 }),
                        (0x21 | 0x22 | 0x26 | 0x28 | 0x24 | 0x23, false) => t.scroll_key(vk),
                        (0x46 | 0x48, true) => {
                            let rep = vk == 0x48;
                            self.set_work(true);
                            if let Viewer::Text(t) = &mut self.viewer {
                                t.open_find(rep);
                            }
                        }
                        (0x1B, _) => {
                            if self.full.is_some() {
                                self.toggle_full();
                            }
                        }
                        (0x08..=0x14 | 0x5B..=0x5D | 0x70..=0x87, _) | (_, true) => {}
                        _ => {
                            self.set_work(true);
                            if let Viewer::Text(t) = &self.viewer {
                                t.start_edit(self.hwnd);
                            }
                        }
                    }
                    return Some(true);
                }
                match (vk, ctrl) {
                    (0x46, true) => t.open_find(false),
                    (0x48, true) => t.open_find(true),
                    (0x72, _) | (0x0D, _) => {
                        if t.find_open {
                            if !t.find_next(!shift) {
                                self.set_status("Не найдено");
                            }
                        } else {
                            t.focus();
                        }
                    }
                    (0x1B, _) => {
                        if t.find_open {
                            t.close_find();
                        } else {
                            self.set_work(false);
                        }
                    }
                    (_, false) if !t.has_focus() && !matches!(vk, 0x08..=0x14 | 0x5B..=0x5D | 0x70..=0x87) => {
                        // курсор ушёл из текста (щелчок по панели) — вернуть
                        t.start_edit(self.hwnd);
                    }
                    _ => {}
                }
                return Some(true);
            }
            Viewer::Table(t) => {
                if ctrl && vk == 0x43 {
                    self.command(ID_COPY);
                    return Some(true);
                }
                if ctrl && matches!(vk, 0xBB | 0x6B | 0xBD | 0x6D | 0x30 | 0x60) {
                    let id = match vk { 0xBB | 0x6B => ID_ZIN, 0xBD | 0x6D => ID_ZOUT, _ => ID_FIT };
                    self.command(id);
                    return Some(true);
                }
                if vk == 0x1B {
                    if work {
                        self.set_work(false);
                    } else if self.full.is_some() {
                        self.toggle_full();
                    }
                    return Some(true);
                }
                if !work && (vk == 0x25 || vk == 0x27) {
                    self.step_file(if vk == 0x25 { -1 } else { 1 });
                    return Some(true);
                }
                t.key(&ui, vk, ctrl);
                return Some(true);
            }
            Viewer::Media(m) => {
                match vk {
                    0x20 => m.toggle_play(),
                    0x25 | 0x27 if shift => m.step(if vk == 0x25 { -5.0 } else { 5.0 }),
                    0x25 | 0x21 => self.step_file(-1),
                    0x27 | 0x22 => self.step_file(1),
                    0x26 => m.set_volume(m.volume + 0.05),
                    0x28 => m.set_volume(m.volume - 0.05),
                    0x4D if !ctrl => m.toggle_mute(),
                    0x24 => m.seek(0.0),
                    0x43 if ctrl => {
                        self.command(ID_COPY);
                    }
                    0x1B => {
                        if self.full.is_some() {
                            self.toggle_full();
                        }
                    }
                    _ => {}
                }
                return Some(true);
            }
            Viewer::None => {}
        }
        let scrolling = self.scrolling();
        let id = if ctrl {
            match vk {
                0x5A => ID_UNDO,
                0x4C => ID_ROT_L,
                0x52 => ID_ROT_R,
                0x48 => ID_FLIP,
                0x4B => ID_CROP,
                0x43 => ID_COPY,
                0x56 => ID_PASTE,
                0x49 => ID_INSERT,
                0x30 | 0x60 => ID_FIT,
                0x31 | 0x61 => ID_100,
                0xBB | 0x6B => ID_ZIN,
                0xBD | 0x6D => ID_ZOUT,
                _ => 0,
            }
        } else {
            match vk {
                0x25 => ID_PREV,
                0x27 => ID_NEXT,
                0x21 | 0x22 if scrolling && self.work => {
                    let d = self.ch() * 0.9;
                    self.sy += if vk == 0x21 { -d } else { d };
                    return Some(true);
                }
                0x26 | 0x28 if scrolling && self.work => {
                    let d = self.s(60) as f64;
                    self.sy += if vk == 0x26 { -d } else { d };
                    return Some(true);
                }
                0x21 => ID_PREV,
                0x22 => ID_NEXT,
                0x24 | 0x23 if self.work || self.pages_are_files() => {
                    self.go_home(vk == 0x23);
                    return Some(true);
                }
                0x2E => ID_DEL_PAGE,
                0x0D => {
                    self.apply_crop();
                    return Some(true);
                }
                0x1B => {
                    if self.crop_mode {
                        self.crop_mode = false;
                        self.crop = None;
                    } else if self.work {
                        self.set_work(false);
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
        if self.peek_at(x, y) != 0 {
            return Some(IDC_HAND);
        }
        if let Viewer::Table(t) = &self.viewer {
            return t.cursor(&Ui::new(self.dpi), x, y);
        }
        if (self.view.is_some() || self.scrolling()) && gui::inside(&self.canvas, x, y) {
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
        // хвостики
        let ph = self.peek_at(x, y);
        let hover_changed = ph != self.peek_hover && self.drag.is_none();
        if hover_changed {
            self.peek_hover = ph;
        }
        if ph != 0 && self.drag.is_none() {
            match m {
                Mouse::Down | Mouse::Dbl => {
                    self.peek_hover = 0;
                    match self.neighbor(ph as isize) {
                        Some(Nb::File(_)) => self.step_file(ph as isize),
                        _ => self.step(ph as isize),
                    }
                    return true;
                }
                Mouse::Wheel(_) => return hover_changed,
                _ => return hover_changed || m == Mouse::Move,
            }
        }
        let (cx, cy) = ((x - self.canvas.left) as f64, (y - self.canvas.top) as f64);
        let in_canvas = gui::inside(&self.canvas, x, y);
        let ctrl = key_down(0x11);
        let shift = key_down(0x10);
        let dpi = self.dpi;
        match &mut self.viewer {
            Viewer::Table(_) if !self.work && m == Mouse::Down && in_canvas => {
                // щелчок по таблице — начать работу с ней
                self.set_work(true);
                if let Viewer::Table(t) = &mut self.viewer {
                    t.mouse(&Ui::new(dpi), m, x, y, ctrl, shift);
                }
                return true;
            }
            Viewer::Table(t) => return t.mouse(&Ui::new(dpi), m, x, y, ctrl, shift) || hover_changed,
            Viewer::Media(md) => {
                if m == Mouse::Dbl && in_canvas && self.kind == Some(FK::Video) {
                    self.toggle_full();
                    return true;
                }
                return md.mouse(m, x, y) || hover_changed;
            }
            Viewer::Text(_) => return hover_changed,
            Viewer::None => {}
        }
        let has = self.view.is_some() || self.scrolling();
        match m {
            Mouse::Down => {
                if !has || !in_canvas {
                    return hover_changed;
                }
                self.drag = Some((cx, cy));
                if self.crop_mode {
                    self.crop = Some((cx, cy, cx, cy));
                }
                true
            }
            Mouse::Move => {
                let Some((px, py)) = self.drag else { return hover_changed };
                if self.crop_mode {
                    let (cw, ch) = (self.cw(), self.ch());
                    if let Some(c) = &mut self.crop {
                        c.2 = cx.clamp(0.0, cw);
                        c.3 = cy.clamp(0.0, ch);
                    }
                } else if self.scrolling() {
                    self.sy -= cy - py;
                    self.off.0 += cx - px;
                    self.drag = Some((cx, cy));
                    self.lazy_quality(hwnd);
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
                    if !self.work && self.can_work() {
                        self.drag = None;
                        self.set_work(true);
                    } else {
                        self.reset_view();
                    }
                    true
                } else {
                    false
                }
            }
            Mouse::Wheel(d) => {
                if !has || !in_canvas {
                    return false;
                }
                self.lazy_quality(hwnd);
                if self.scrolling() && !ctrl {
                    self.sy -= d as f64 / 120.0 * self.s(110) as f64;
                } else {
                    self.zoom_at(if d > 0 { 1.15 } else { 1.0 / 1.15 }, cx, cy);
                }
                true
            }
            Mouse::RDown => false,
        }
    }

    fn timer(&mut self, hwnd: HWND, id: usize) -> bool {
        match id {
            TIMER_MEDIA => {
                return match &mut self.viewer {
                    Viewer::Media(m) => m.tick(),
                    _ => {
                        unsafe {
                            let _ = KillTimer(hwnd, id);
                        }
                        false
                    }
                };
            }
            TIMER_TEXT => {
                let t = theme::cur();
                if let Viewer::Text(tv) = &mut self.viewer {
                    tv.tick(&t);
                }
                return false;
            }
            TIMER_ANIM => {
                let done = self.anim.as_ref().map(|a| a.t0.elapsed().as_secs_f64() * 1000.0 >= ANIM_MS).unwrap_or(true);
                if done {
                    self.anim = None;
                    unsafe {
                        let _ = KillTimer(hwnd, id);
                    }
                }
                return true;
            }
            _ => {}
        }
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
        let shift = key_down(0x10);
        if shift && self.mode() == Mode2::Pages {
            let fl: Vec<PathBuf> = files.into_iter().filter(|x| kinds::kind_of(x).map(kinds::is_paged).unwrap_or(false)).collect();
            let (pages, errs) = doc::pages_from_all(&fl);
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
        self.viewer = Viewer::None;
        save_placement(hwnd);
        settings::save();
        true
    }

    fn raw(&mut self, _hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_MEDIA => {
                if let Viewer::Media(m) = &mut self.viewer {
                    m.event(wp.0 as u32);
                }
                Some(LRESULT(0))
            }
            WM_PEEK => {
                if let Some(p) = &mut self.peeker {
                    p.poll();
                }
                Some(LRESULT(0))
            }
            WM_TEXT_REFRESH => Some(LRESULT(0)),
            WM_COMMAND => {
                let code = (wp.0 >> 16) as u32 & 0xFFFF;
                let id = wp.0 as u32 & 0xFFFF;
                if id == textview::ID_EDIT && code == 0x0100 && !self.work && matches!(self.viewer, Viewer::Text(_)) {
                    // щелчок по тексту в обзоре — начать правку
                    self.set_work(true);
                }
                if let Viewer::Text(t) = &mut self.viewer {
                    if id == textview::ID_EDIT && (code == 0x0100 || code == 0x0200) {
                        // EN_SETFOCUS / EN_KILLFOCUS
                        unsafe {
                            let _ = PostMessageW(self.hwnd, WM_TEXT_REFRESH, WPARAM(0), LPARAM(0));
                        }
                        return Some(LRESULT(0));
                    }
                    if id == textview::ID_EDIT && code == 0x0300 {
                        // EN_CHANGE
                        t.mark_changed();
                        unsafe {
                            let _ = PostMessageW(self.hwnd, WM_TEXT_REFRESH, WPARAM(0), LPARAM(0));
                        }
                        return Some(LRESULT(0));
                    }
                }
                None
            }
            WM_NOTIFY => {
                let nm = unsafe { &*(lp.0 as *const windows::Win32::UI::Controls::NMHDR) };
                if nm.idFrom == textview::ID_EDIT as usize && nm.code == 0x0702 {
                    // EN_SELCHANGE — обновить «Стр, стлб»
                    unsafe {
                        let _ = PostMessageW(self.hwnd, WM_TEXT_REFRESH, WPARAM(0), LPARAM(0));
                    }
                    return Some(LRESULT(0));
                }
                None
            }
            WM_CTLCOLOREDIT => {
                let t = theme::cur();
                if let Viewer::Text(tv) = &self.viewer {
                    if tv.is_find_box(HWND(lp.0 as *mut _)) {
                        return Some(tv.ctl_color(&t, HDC(wp.0 as *mut _)));
                    }
                }
                None
            }
            _ => None,
        }
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

    /// Элементы просмотрщика (ярлыки листов, панель поиска) и размещение редактора.
    fn layout_viewer(&mut self, ui: &Ui, v: &mut Vec<W>) {
        let c = self.canvas;
        match &mut self.viewer {
            Viewer::Table(t) => v.extend(t.widgets(ui, c)),
            Viewer::Text(t) => v.extend(t.layout(ui, c)),
            _ => {}
        }
    }

    /// Постраничный вид («Книга»): картинка текущей страницы + анимация листания.
    fn paint_pages(&mut self, ui: &Ui, hdc: HDC) {
        let t = &ui.t;
        let c = self.canvas;
        let Some(img) = self.view.clone() else {
            let cy = (c.top + c.bottom) / 2;
            if let Some(e) = &self.view_err {
                let ir = gui::r(c.left, cy - ui.s(120), c.right - c.left, ui.s(50));
                let ic = peek::kind_icon(self.kind);
                gui::text(hdc, &char::from_u32(ic).map(|c| c.to_string()).unwrap_or_default(), &ir, ui.f.icon_big, t.dim, gui::CENTER);
                let tr = gui::r(c.left + ui.s(40), cy - ui.s(60), c.right - c.left - ui.s(80), ui.s(160));
                gui::text(hdc, &format!("Не удалось открыть\n\n{e}"), &tr, ui.f.text, t.dim, gui::WRAP);
            } else if self.mode() == Mode2::Empty {
                let ir = gui::r(c.left, cy - ui.s(110), c.right - c.left, ui.s(60));
                gui::text(hdc, &char::from_u32(gui::IC_PICTURE).unwrap().to_string(), &ir, ui.f.icon_big, t.dim, gui::CENTER);
                let tr = gui::r(c.left, cy - ui.s(52), c.right - c.left, ui.s(26));
                gui::text(hdc, "Откройте фото, PDF, документ, таблицу, текст или медиа", &tr, ui.f.big, t.text, gui::CENTER);
                let tr2 = gui::r(c.left, cy - ui.s(26), c.right - c.left, ui.s(24));
                gui::text(hdc, "или перетащите файлы в окно", &tr2, ui.f.text, t.dim, gui::CENTER);
            }
            return;
        };
        let (dx, dy, dw, dh) = self.dest();
        // анимация листания: старая уезжает, новая приезжает
        let mut shift = 0.0;
        if let Some(a) = &self.anim {
            let p = (a.t0.elapsed().as_secs_f64() * 1000.0 / ANIM_MS).clamp(0.0, 1.0);
            let e = 1.0 - (1.0 - p).powi(3);
            let span = self.cw() * 0.6;
            shift = a.dir * (1.0 - e) * span;
            let (ox, oy, ow, oh) = a.rect;
            let orc = gui::r(c.left + (ox - a.dir * e * span).round() as i32, c.top + oy.round() as i32, ow.round() as i32, oh.round() as i32);
            gui::blit(hdc, &a.old, &orc, false);
        }
        if shift != 0.0 {
            let rc = gui::r(c.left + (dx + shift).round() as i32, c.top + dy.round() as i32, dw.round() as i32, dh.round() as i32);
            gui::fill(hdc, &gui::r(rc.left + 2, rc.top + 2, rc.right - rc.left, rc.bottom - rc.top), gui::blend(t.canvas, crate::util::rgb(0, 0, 0), if t.dark { 0.6 } else { 0.85 }));
            gui::blit(hdc, &img, &rc, false);
            return;
        }
        let s = self.scale();
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
                    let info = crate::util::bmi(&img);
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
    crate::media::startup();
    let app: Rc<RefCell<dyn Panel>> = Rc::new(RefCell::new(App::new(args)));
    let hwnd = gui::create(None, app, (1120, 760), saved_placement());
    unsafe {
        let _ = ShowWindow(hwnd, if settings::get_i("maximized", 0) == 1 { SW_SHOWMAXIMIZED } else { SW_SHOW });
    }
    gui::message_loop();
    settings::save();
}
