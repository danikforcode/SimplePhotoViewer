// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Окно «Объединить в PDF»: список фото и PDF по порядку → один PDF.
//! Страницы PDF переносятся без потери качества; фото — страницами.

use crate::gui::{self, Ev, Kind, Panel, Ui, W};
use crate::util::{file_name, is_pdf};
use crate::{doc, files, settings};
use std::path::PathBuf;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::HDC;

pub enum Outcome {
    None,
    Open(Vec<PathBuf>),
    Saved(PathBuf),
}

const ID_LIST: u32 = 1;
const ID_ADD: u32 = 2;
const ID_DEL: u32 = 3;
const ID_UP: u32 = 4;
const ID_DOWN: u32 = 5;
const ID_CLEAR: u32 = 6;
const ID_PHOTO0: u32 = 10; // 10, 11
const ID_M_DEC: u32 = 20;
const ID_M_INC: u32 = 21;
const ID_OPEN: u32 = 30;
const ID_SAVE: u32 = 31;
const ID_CANCEL: u32 = 32;

struct Merge {
    items: Vec<(PathBuf, String)>,
    sel: Option<usize>,
    scroll: usize,
    photo: usize,
    margin: i32,
    out: Outcome,
}

fn describe(p: &PathBuf) -> String {
    let dir = p.parent().map(|d| d.to_string_lossy().to_string()).filter(|d| !d.is_empty()).map(|d| format!(" · {d}")).unwrap_or_default();
    if is_pdf(p) {
        let n = crate::pdf_read::info(p).map(|i| i.pages.len()).or_else(|_| crate::pdf_write::page_count(p).map(|n| n as usize));
        match n {
            Ok(n) => format!("PDF · страниц: {n}{dir}"),
            Err(_) => format!("PDF · не читается{dir}"),
        }
    } else {
        format!("Фото{dir}")
    }
}

impl Merge {
    fn add(&mut self, paths: Vec<PathBuf>) {
        let at = self.sel.map(|s| s + 1).unwrap_or(self.items.len());
        let mut k = at;
        for p in paths.into_iter().filter(|p| files::is_supported(p)) {
            let p = std::path::absolute(&p).unwrap_or(p);
            let d = describe(&p);
            self.items.insert(k, (p, d));
            k += 1;
        }
        if k > at {
            self.sel = Some(k - 1);
        }
    }

    fn save(&mut self, hwnd: HWND) -> bool {
        if self.items.is_empty() {
            return true;
        }
        let first = &self.items[0].0;
        let dir = first.parent().map(|d| d.to_path_buf());
        let name = if self.items.len() == 1 {
            format!("{}.pdf", first.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default())
        } else {
            "Объединённый.pdf".to_string()
        };
        let Some(out) = files::save_dialog(hwnd, &name, dir.as_deref(), true, true) else { return true };
        settings::set_i("pdf_photo", self.photo as i32);
        settings::set_i("pdf_margin", self.margin);
        let paths: Vec<PathBuf> = self.items.iter().map(|(p, _)| p.clone()).collect();
        let (pages, errs) = doc::pages_from_all(&paths);
        if !errs.is_empty() {
            crate::ui::message(Some(hwnd), &format!("Не удалось прочитать:\n\n{}", errs.join("\n\n")), true);
            return true;
        }
        match doc::save_pdf(&pages, &out) {
            Ok(()) => {
                self.out = Outcome::Saved(out);
                false
            }
            Err(e) => {
                crate::ui::message(Some(hwnd), &format!("Не удалось сохранить PDF:\n{e}"), true);
                true
            }
        }
    }
}

impl Panel for Merge {
    fn title(&self) -> String {
        "Объединить в PDF".into()
    }

    fn init(&mut self, hwnd: HWND) {
        unsafe { windows::Win32::UI::Shell::DragAcceptFiles(hwnd, true) };
    }

    fn min_size(&self) -> (i32, i32) {
        (560, 440)
    }

    fn layout(&mut self, ui: &Ui, w: i32, h: i32) -> Vec<W> {
        let m = ui.s(16);
        let rw = ui.s(170);
        let mut v = vec![];
        v.push(W::new(Kind::Group, 0, gui::r(m, m, w - 2 * m, ui.s(22))).t("Файлы по порядку — так же пойдут страницы"));
        let ly = m + ui.s(28);
        let bottom_h = ui.s(170);
        let lh = (h - ly - bottom_h).max(ui.s(100));
        let lw = w - 2 * m - rw - ui.s(12);
        let mut list = W::new(Kind::List, ID_LIST, gui::r(m, ly, lw, lh)).t("Добавьте фото или PDF (или перетащите в окно)");
        list.items = self.items.iter().map(|(p, d)| (file_name(p), d.clone())).collect();
        list.sel = self.sel;
        gui::list_fix_scroll(ui, &list, &mut self.scroll);
        list.scroll = self.scroll;
        v.push(list);

        let bx = m + lw + ui.s(12);
        let bh = ui.s(34);
        let has = self.sel.is_some();
        let btns = [
            (ID_ADD, gui::IC_ADD, "Добавить…", true),
            (ID_DEL, gui::IC_REMOVE, "Убрать", has),
            (ID_UP, gui::IC_UP, "Выше", has && self.sel != Some(0)),
            (ID_DOWN, gui::IC_DOWN, "Ниже", has && self.sel.map(|s| s + 1 < self.items.len()).unwrap_or(false)),
            (ID_CLEAR, gui::IC_DELETE, "Очистить", !self.items.is_empty()),
        ];
        for (k, (id, ic, t, en)) in btns.iter().enumerate() {
            v.push(W::new(Kind::Button, *id, gui::r(bx, ly + k as i32 * (bh + ui.s(8)), rw, bh)).i(*ic).t(*t).en(*en));
        }

        let y = ly + lh + ui.s(16);
        v.push(W::new(Kind::Group, 0, gui::r(m, y, ui.s(300), ui.s(22))).t("Фото на странице PDF"));
        let segw = ui.s(300);
        v.extend(gui::segments(ui, m, y + ui.s(26), segw, ID_PHOTO0, &["Лист A4 с полями", "По размеру фото"], self.photo));
        if self.photo == 0 {
            let x2 = m + segw + ui.s(16);
            v.push(W::new(Kind::Label, 0, gui::r(x2, y + ui.s(26), ui.s(70), ui.s(32))).t("Поля:"));
            v.extend(gui::stepper(ui, x2 + ui.s(56), y + ui.s(26), ui.s(150), format!("{} мм", self.margin), ID_M_DEC, ID_M_INC, "Поля вокруг фото"));
        }
        let hint = "Страницы PDF сохраняются без потери качества. «Открыть» — чтобы удалить, повернуть или переставить отдельные страницы.";
        v.push(W::new(Kind::Label, 0, gui::r(m, y + ui.s(66), w - 2 * m, ui.s(22))).t(hint).color(ui.t.dim));

        let by = h - m - ui.s(36);
        let ok = !self.items.is_empty();
        let mut x = w - m;
        for (id, t, primary, en, bw) in [
            (ID_SAVE, "Сохранить PDF…", true, ok, 160),
            (ID_OPEN, "Открыть для правки", false, ok, 170),
            (ID_CANCEL, "Отмена", false, true, 100),
        ] {
            let bw = ui.s(bw);
            x -= bw;
            v.push(W::new(Kind::Button, id, gui::r(x, by, bw, ui.s(36))).t(t).on(primary).en(en));
            x -= ui.s(8);
        }
        v
    }

    fn paint(&mut self, ui: &Ui, hdc: HDC, w: i32, h: i32) {
        gui::fill(hdc, &gui::r(0, 0, w, h), ui.t.bg);
    }

    fn event(&mut self, hwnd: HWND, ev: Ev) -> bool {
        match ev {
            Ev::Item(ID_LIST, k) => self.sel = Some(k),
            Ev::Dbl(ID_LIST, k) => self.sel = Some(k),
            Ev::Scroll(ID_LIST, d) => self.scroll = (self.scroll as i32 + d).max(0) as usize,
            Ev::Click(id) => match id {
                ID_ADD => {
                    let p = files::open_dialog(hwnd, files::Pick::Any, true);
                    self.add(p);
                }
                ID_DEL => {
                    if let Some(s) = self.sel {
                        self.items.remove(s);
                        self.sel = if self.items.is_empty() { None } else { Some(s.min(self.items.len() - 1)) };
                    }
                }
                ID_UP => {
                    if let Some(s) = self.sel.filter(|&s| s > 0) {
                        self.items.swap(s, s - 1);
                        self.sel = Some(s - 1);
                    }
                }
                ID_DOWN => {
                    if let Some(s) = self.sel.filter(|&s| s + 1 < self.items.len()) {
                        self.items.swap(s, s + 1);
                        self.sel = Some(s + 1);
                    }
                }
                ID_CLEAR => {
                    self.items.clear();
                    self.sel = None;
                }
                ID_PHOTO0 | 11 => self.photo = (id - ID_PHOTO0) as usize,
                ID_M_DEC => self.margin = (self.margin - 1).max(0),
                ID_M_INC => self.margin = (self.margin + 1).min(40),
                ID_SAVE => return self.save(hwnd),
                ID_OPEN => {
                    settings::set_i("pdf_photo", self.photo as i32);
                    settings::set_i("pdf_margin", self.margin);
                    self.out = Outcome::Open(self.items.iter().map(|(p, _)| p.clone()).collect());
                    return false;
                }
                ID_CANCEL => return false,
                _ => {}
            },
            _ => {}
        }
        true
    }

    fn key(&mut self, hwnd: HWND, vk: u16, ctrl: bool, _shift: bool) -> Option<bool> {
        let n = self.items.len();
        match vk {
            0x2E => Some(self.event(hwnd, Ev::Click(ID_DEL))),
            0x26 if ctrl => Some(self.event(hwnd, Ev::Click(ID_UP))),
            0x28 if ctrl => Some(self.event(hwnd, Ev::Click(ID_DOWN))),
            0x26 if n > 0 => {
                self.sel = Some(self.sel.map(|s| s.saturating_sub(1)).unwrap_or(0));
                Some(true)
            }
            0x28 if n > 0 => {
                self.sel = Some(self.sel.map(|s| (s + 1).min(n - 1)).unwrap_or(0));
                Some(true)
            }
            _ => None,
        }
    }

    fn drop_files(&mut self, _hwnd: HWND, files: Vec<PathBuf>) {
        self.add(files);
    }
}

pub fn run(owner: HWND, init: Vec<PathBuf>) -> Outcome {
    let mut m = Merge {
        items: vec![],
        sel: None,
        scroll: 0,
        photo: (settings::get_i("pdf_photo", 0) == 1) as usize,
        margin: settings::get_i("pdf_margin", 10),
        out: Outcome::None,
    };
    m.add(init);
    gui::run_modal(owner, m, (720, 560)).out
}
