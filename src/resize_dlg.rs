// SimplePhotoViewer (Rust/Win32) — © 2026 danikforcode
// Лицензия MIT (см. LICENSE). Распространяется «как есть», без гарантий.
//! «Размер и формат» — как «Изменить размер» в «Фотографиях» Windows:
//! размер в точках или процентах (с замком пропорций), тип файла, качество,
//! разрешение для печати. «Сохранить копию» не трогает открытое фото,
//! «Применить» меняет размер открытого фото (можно отменить).
//! Здесь же — маленькое окно «Свой размер листа» для печати.

use crate::fields::{Fields, EN_CHANGE};
use crate::gui::{self, Ev, Kind, Panel, Ui, W};
use crate::image_buf::Img;
use crate::{codec, files, settings, theme};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::HDC;
use windows::Win32::UI::WindowsAndMessaging::{WM_COMMAND, WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC};

const ID_W: u32 = 1;
const ID_H: u32 = 2;
const ID_UNIT0: u32 = 10;
const ID_LOCK: u32 = 12;
const ID_PRESET0: u32 = 20;
const ID_TYPE0: u32 = 40;
const ID_Q: u32 = 60;
const ID_DPI0: u32 = 70;
const ID_SAVE: u32 = 80;
const ID_APPLY: u32 = 81;
const ID_CANCEL: u32 = 82;

/// Тип файла: подпись, расширение, есть ли «качество».
const TYPES: [(&str, &str, bool); 8] = [
    ("JPEG", "jpg", true),
    ("PNG", "png", false),
    ("BMP", "bmp", false),
    ("TIFF", "tif", false),
    ("GIF", "gif", false),
    ("WebP", "webp", false),
    ("JPEG XR", "jxr", true),
    ("HEIC", "heic", true),
];
const PRESETS: [(&str, f64); 6] = [("Исходный", 0.0), ("75%", -0.75), ("50%", -0.5), ("25%", -0.25), ("1920", 1920.0), ("1280", 1280.0)];
const DPIS: [u32; 5] = [72, 96, 150, 300, 600];

pub enum Outcome {
    None,
    Apply(Img),
    Saved(PathBuf),
}

struct Dlg {
    hwnd: HWND,
    img: Img,
    name: String,
    dir: Option<PathBuf>,
    fields: Option<Fields>,
    percent: bool,
    lock: bool,
    w: u32,
    h: u32,
    ty: usize,
    quality: i32,
    dpi: usize,
    out: Outcome,
    err: String,
}

impl Dlg {
    fn aspect(&self) -> f64 {
        self.img.w as f64 / self.img.h.max(1) as f64
    }

    /// Показать текущий размер в полях.
    fn sync_fields(&self) {
        let Some(f) = &self.fields else { return };
        if self.percent {
            f.set(0, &fmt(self.w as f64 / self.img.w as f64 * 100.0));
            f.set(1, &fmt(self.h as f64 / self.img.h as f64 * 100.0));
        } else {
            f.set(0, &self.w.to_string());
            f.set(1, &self.h.to_string());
        }
    }

    /// Пользователь поменял поле i.
    fn field_changed(&mut self, i: usize) {
        let Some(f) = &self.fields else { return };
        let Some(v) = f.num(i) else { return };
        if v <= 0.0 {
            return;
        }
        let (iw, ih) = (self.img.w as f64, self.img.h as f64);
        let px = if self.percent { v / 100.0 * if i == 0 { iw } else { ih } } else { v };
        let px = px.round().clamp(1.0, 30000.0);
        if i == 0 {
            self.w = px as u32;
            if self.lock {
                self.h = (px / self.aspect()).round().max(1.0) as u32;
            }
        } else {
            self.h = px as u32;
            if self.lock {
                self.w = (px * self.aspect()).round().max(1.0) as u32;
            }
        }
        // второе поле — обновить, первое оставить как вводят
        let other = 1 - i;
        if self.lock {
            let s = if self.percent {
                fmt(if other == 0 { self.w as f64 / iw } else { self.h as f64 / ih } * 100.0)
            } else if other == 0 {
                self.w.to_string()
            } else {
                self.h.to_string()
            };
            f.set(other, &s);
        }
    }

    fn result(&self) -> Result<Img, String> {
        if (self.w, self.h) == (self.img.w, self.img.h) {
            return Ok(self.img.clone());
        }
        codec::resize(&self.img, self.w, self.h).map_err(|e| e.message().to_string())
    }

    fn save(&mut self, hwnd: HWND) -> bool {
        let (label, ext, _) = TYPES[self.ty];
        let stem = Path::new(&self.name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "фото".into());
        let name = format!("{stem} {}×{}.{ext}", self.w, self.h);
        let exts = [ext];
        let Some(path) = files::save_as(hwnd, &name, self.dir.as_deref(), &[(label, &exts)], 0) else { return false };
        let img = match self.result() {
            Ok(i) => i,
            Err(e) => {
                self.err = e;
                return false;
            }
        };
        let q = if TYPES[self.ty].2 { self.quality } else { 92 };
        match codec::save_full(&img, &path, q, DPIS[self.dpi] as f64) {
            Ok(()) => {
                settings::set_i("rs_type", self.ty as i32);
                settings::set_i("rs_q", self.quality);
                settings::set_i("rs_dpi", self.dpi as i32);
                self.out = Outcome::Saved(path);
                true
            }
            Err(e) => {
                self.err = if ext == "heic" {
                    "Не удалось сохранить HEIC: нужно расширение «HEVC» из Microsoft Store. Выберите JPEG или PNG.".into()
                } else {
                    format!("Не удалось сохранить: {}", e.message())
                };
                false
            }
        }
    }
}

fn fmt(v: f64) -> String {
    let s = format!("{v:.1}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

impl Panel for Dlg {
    fn title(&self) -> String {
        "Размер и формат".into()
    }

    fn init(&mut self, hwnd: HWND) {
        self.hwnd = hwnd;
        self.fields = Some(Fields::new(hwnd, &[ID_W, ID_H], true));
        self.sync_fields();
    }

    fn min_size(&self) -> (i32, i32) {
        (520, 600)
    }

    fn layout(&mut self, ui: &Ui, w: i32, h: i32) -> Vec<W> {
        let m = ui.s(18);
        let cw = w - 2 * m;
        let mut v = vec![];
        let mut y = m;
        let gh = ui.s(24);
        let row = ui.s(32);
        let step = ui.s(16);
        let group = |v: &mut Vec<W>, y: &mut i32, t: &str| {
            v.push(W::new(Kind::Group, 0, gui::r(m, *y, cw, gh)).t(t));
            *y += gh + ui.s(4);
        };
        group(&mut v, &mut y, &format!("Размер (сейчас {} × {} точек)", self.img.w, self.img.h));
        v.extend(gui::segments(ui, m, y, ui.s(220), ID_UNIT0, &["Точки", "Проценты"], self.percent as usize));
        v.push(W::new(Kind::Check, ID_LOCK, gui::r(m + ui.s(236), y, cw - ui.s(236), row)).t("Сохранять пропорции").on(self.lock));
        y += row + ui.s(10);
        let unit = if self.percent { "%" } else { "точек" };
        let fw = ui.s(110);
        v.push(W::new(Kind::Label, 0, gui::r(m, y, ui.s(70), row)).t("Ширина"));
        if let Some(f) = &self.fields {
            v.push(f.place(ui, 0, gui::r(m + ui.s(70), y, fw, row), true));
        }
        v.push(W::new(Kind::Label, 0, gui::r(m + ui.s(70) + fw + ui.s(6), y, ui.s(50), row)).t(unit).color(ui.t.dim));
        let x2 = m + cw / 2 + ui.s(10);
        v.push(W::new(Kind::Label, 0, gui::r(x2, y, ui.s(70), row)).t("Высота"));
        if let Some(f) = &self.fields {
            v.push(f.place(ui, 1, gui::r(x2 + ui.s(70), y, fw, row), true));
        }
        v.push(W::new(Kind::Label, 0, gui::r(x2 + ui.s(70) + fw + ui.s(6), y, ui.s(50), row)).t(unit).color(ui.t.dim));
        y += row + ui.s(10);
        let names: Vec<&str> = PRESETS.iter().map(|p| p.0).collect();
        v.extend(gui::segments(ui, m, y, cw, ID_PRESET0, &names, usize::MAX));
        y += row + ui.s(6);
        let mp = self.w as f64 * self.h as f64 / 1e6;
        v.push(W::new(Kind::Label, 0, gui::r(m, y, cw, ui.s(22))).t(format!("Получится: {} × {} точек  ·  {:.1} Мп", self.w, self.h, mp)).color(ui.t.dim));
        y += ui.s(22) + step;

        group(&mut v, &mut y, "Тип файла");
        let tn: Vec<&str> = TYPES.iter().map(|t| t.0).collect();
        let sel = self.ty;
        v.extend(gui::segments(ui, m, y, cw, ID_TYPE0, &tn[..4], sel));
        y += row + ui.s(4);
        v.extend(gui::segments(ui, m, y, cw, ID_TYPE0 + 4, &tn[4..], sel.wrapping_sub(4)));
        y += row + step;

        let has_q = TYPES[self.ty].2;
        group(&mut v, &mut y, &if has_q { format!("Качество: {}", self.quality) } else { "Качество: без потерь".into() });
        v.push(W::new(Kind::Slider, ID_Q, gui::r(m, y, cw, row)).v((self.quality - 10) as f64 / 90.0).en(has_q).tip("Меньше — файл легче, больше — чётче. Двойной щелчок — 92"));
        y += row + step;

        group(&mut v, &mut y, "Разрешение для печати (точек на дюйм)");
        let dn: Vec<String> = DPIS.iter().map(|d| d.to_string()).collect();
        let dn: Vec<&str> = dn.iter().map(|s| s.as_str()).collect();
        v.extend(gui::segments(ui, m, y, cw, ID_DPI0, &dn, self.dpi));
        y += row + ui.s(6);
        let d = DPIS[self.dpi] as f64;
        v.push(
            W::new(Kind::Label, 0, gui::r(m, y, cw, ui.s(22)))
                .t(format!("На бумаге: {:.1} × {:.1} см", self.w as f64 / d * 2.54, self.h as f64 / d * 2.54))
                .color(ui.t.dim),
        );
        y += ui.s(22);
        if !self.err.is_empty() {
            v.push(W::new(Kind::Label, 0, gui::r(m, y + ui.s(6), cw, ui.s(40))).t(self.err.clone()).color(crate::util::rgb(220, 60, 60)));
        }

        let by = h - m - ui.s(36);
        let mut x = w - m;
        for (id, t, primary, bw) in [(ID_SAVE, "Сохранить копию…", true, 190), (ID_APPLY, "Применить к фото", false, 170), (ID_CANCEL, "Отмена", false, 100)] {
            let bw = ui.s(bw);
            x -= bw;
            let tip = match id {
                ID_SAVE => "Записать новый файл; открытое фото не меняется",
                ID_APPLY => "Изменить размер открытого фото (Ctrl+Z — отменить)",
                _ => "",
            };
            v.push(W::new(Kind::Button, id, gui::r(x, by, bw, ui.s(36))).t(t).on(primary).tip(tip));
            x -= ui.s(8);
        }
        v
    }

    fn paint(&mut self, ui: &Ui, hdc: HDC, w: i32, h: i32) {
        gui::fill(hdc, &gui::r(0, 0, w, h), ui.t.bg);
    }

    fn event(&mut self, hwnd: HWND, ev: Ev) -> bool {
        self.err.clear();
        match ev {
            Ev::Slide(ID_Q, v) => self.quality = (10.0 + v * 90.0).round() as i32,
            Ev::Click(id) => match id {
                ID_Q => self.quality = 92,
                ID_CANCEL => return false,
                ID_APPLY => match self.result() {
                    Ok(i) => {
                        self.out = Outcome::Apply(i);
                        return false;
                    }
                    Err(e) => self.err = e,
                },
                ID_SAVE => {
                    if self.save(hwnd) {
                        return false;
                    }
                }
                ID_LOCK => {
                    self.lock = !self.lock;
                    if self.lock {
                        self.h = (self.w as f64 / self.aspect()).round().max(1.0) as u32;
                        self.sync_fields();
                    }
                }
                i if i == ID_UNIT0 || i == ID_UNIT0 + 1 => {
                    self.percent = i == ID_UNIT0 + 1;
                    self.sync_fields();
                }
                i if (ID_PRESET0..ID_PRESET0 + PRESETS.len() as u32).contains(&i) => {
                    let p = PRESETS[(i - ID_PRESET0) as usize].1;
                    let (iw, ih) = (self.img.w as f64, self.img.h as f64);
                    let k = if p == 0.0 { 1.0 } else if p < 0.0 { -p } else { p / iw.max(ih) };
                    self.w = (iw * k).round().max(1.0) as u32;
                    self.h = (ih * k).round().max(1.0) as u32;
                    self.lock = true;
                    self.sync_fields();
                }
                i if (ID_TYPE0..ID_TYPE0 + 7).contains(&i) => {
                    self.ty = (i - ID_TYPE0) as usize;
                }
                i if (ID_DPI0..ID_DPI0 + DPIS.len() as u32).contains(&i) => self.dpi = (i - ID_DPI0) as usize,
                _ => {}
            },
            _ => {}
        }
        true
    }

    fn key(&mut self, hwnd: HWND, vk: u16, _ctrl: bool, _shift: bool) -> Option<bool> {
        match vk {
            0x0D => Some(self.event(hwnd, Ev::Click(ID_SAVE))),
            0x1B => Some(false),
            _ => None,
        }
    }

    fn raw(&mut self, _hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_COMMAND => {
                let code = (wp.0 >> 16) as u32 & 0xFFFF;
                let id = wp.0 as u32 & 0xFFFF;
                if code == EN_CHANGE && (id == ID_W || id == ID_H) {
                    // только если правит человек (поле в фокусе)
                    let focused = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() };
                    if let Some(f) = &self.fields {
                        if f.boxes.get(if id == ID_W { 0 } else { 1 }) == Some(&focused) {
                            self.field_changed(if id == ID_W { 0 } else { 1 });
                            gui::invalidate(self.hwnd);
                        }
                    }
                    return Some(LRESULT(0));
                }
                None
            }
            WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
                let t = theme::cur();
                self.fields.as_mut()?.ctl_color(&t, HDC(wp.0 as *mut _), HWND(lp.0 as *mut _))
            }
            _ => None,
        }
    }
}

pub fn run(owner: HWND, img: Img, name: &str, dir: Option<PathBuf>) -> Outcome {
    let ty = settings::get_i("rs_type", 0).clamp(0, TYPES.len() as i32 - 1) as usize;
    let d = Dlg {
        hwnd: owner,
        w: img.w,
        h: img.h,
        img,
        name: name.to_string(),
        dir,
        fields: None,
        percent: false,
        lock: true,
        ty,
        quality: settings::get_i("rs_q", 92).clamp(10, 100),
        dpi: settings::get_i("rs_dpi", 1).clamp(0, DPIS.len() as i32 - 1) as usize,
        out: Outcome::None,
        err: String::new(),
    };
    gui::run_modal(owner, d, (560, 640)).out
}

// ------------------------------------------------ свой размер листа ---

struct SizeDlg {
    fields: Option<Fields>,
    init: (f64, f64),
    out: Option<(f64, f64)>,
    err: bool,
}

const ID_OK: u32 = 90;

impl Panel for SizeDlg {
    fn title(&self) -> String {
        "Свой размер листа".into()
    }
    fn init(&mut self, hwnd: HWND) {
        let f = Fields::new(hwnd, &[ID_W, ID_H], true);
        f.set(0, &fmt(self.init.0));
        f.set(1, &fmt(self.init.1));
        f.focus(0);
        self.fields = Some(f);
    }
    fn min_size(&self) -> (i32, i32) {
        (380, 230)
    }
    fn layout(&mut self, ui: &Ui, w: i32, h: i32) -> Vec<W> {
        let m = ui.s(18);
        let row = ui.s(32);
        let mut v = vec![W::new(Kind::Group, 0, gui::r(m, m, w - 2 * m, ui.s(24))).t("Ширина × высота, мм (например, 16:9 — 338,7 × 190,5)")];
        let y = m + ui.s(34);
        let fw = ui.s(110);
        if let Some(f) = &self.fields {
            v.push(f.place(ui, 0, gui::r(m, y, fw, row), true));
            v.push(W::new(Kind::Label, 0, gui::r(m + fw, y, ui.s(30), row)).t("×").align(1));
            v.push(f.place(ui, 1, gui::r(m + fw + ui.s(30), y, fw, row), true));
        }
        v.push(W::new(Kind::Label, 0, gui::r(m + 2 * fw + ui.s(40), y, ui.s(60), row)).t("мм").color(ui.t.dim));
        if self.err {
            v.push(W::new(Kind::Label, 0, gui::r(m, y + row + ui.s(6), w - 2 * m, ui.s(22))).t("От 20 до 2000 мм").color(crate::util::rgb(220, 60, 60)));
        }
        let by = h - m - ui.s(36);
        v.push(W::new(Kind::Button, ID_OK, gui::r(w - m - ui.s(120), by, ui.s(120), ui.s(36))).t("Готово").on(true));
        v.push(W::new(Kind::Button, ID_CANCEL, gui::r(w - m - ui.s(230), by, ui.s(100), ui.s(36))).t("Отмена"));
        v
    }
    fn paint(&mut self, ui: &Ui, hdc: HDC, w: i32, h: i32) {
        gui::fill(hdc, &gui::r(0, 0, w, h), ui.t.bg);
    }
    fn event(&mut self, _hwnd: HWND, ev: Ev) -> bool {
        match ev {
            Ev::Click(ID_CANCEL) => false,
            Ev::Click(ID_OK) => {
                let f = self.fields.as_ref().unwrap();
                match (f.num(0), f.num(1)) {
                    (Some(a), Some(b)) if (20.0..=2000.0).contains(&a) && (20.0..=2000.0).contains(&b) => {
                        self.out = Some((a, b));
                        false
                    }
                    _ => {
                        self.err = true;
                        true
                    }
                }
            }
            _ => true,
        }
    }
    fn key(&mut self, hwnd: HWND, vk: u16, _c: bool, _s: bool) -> Option<bool> {
        match vk {
            0x0D => Some(self.event(hwnd, Ev::Click(ID_OK))),
            0x1B => Some(false),
            _ => None,
        }
    }
    fn raw(&mut self, _hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        if msg == WM_CTLCOLOREDIT || msg == WM_CTLCOLORSTATIC {
            let t = theme::cur();
            return self.fields.as_mut()?.ctl_color(&t, HDC(wp.0 as *mut _), HWND(lp.0 as *mut _));
        }
        None
    }
}

/// Спросить размер листа в мм.
pub fn ask_size(owner: HWND, init: (f64, f64)) -> Option<(f64, f64)> {
    gui::run_modal(owner, SizeDlg { fields: None, init, out: None, err: false }, (420, 250)).out
}
