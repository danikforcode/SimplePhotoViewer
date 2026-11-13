// SimplePhotoViewer (Rust/Win32) — © 2026 danikforcode
// Лицензия MIT (см. LICENSE). Распространяется «как есть», без гарантий.
//! Редактор фото в главном окне: выделение, панель «Настройка» с ползунками
//! и живым просмотром, эффекты, рисование, обрезка с пропорциями.
//! Это часть модуля ui (видит поля App).

use super::*;
use crate::draw::{self, Shape, Tool};
use crate::edit::{self, Adjust, Fx, Sel, SelOp, ADJ_FIELDS};

// ---- команды редактора
pub(super) const ID_T_RECT: u32 = 400;
pub(super) const ID_T_ELL: u32 = 401;
pub(super) const ID_T_LASSO: u32 = 402;
pub(super) const ID_T_WAND: u32 = 403;
pub(super) const ID_SEL_ALL: u32 = 404;
pub(super) const ID_SEL_NONE: u32 = 405;
pub(super) const ID_SEL_INV: u32 = 406;
pub(super) const ID_ADJ: u32 = 407;
pub(super) const ID_FX0: u32 = 410;
pub(super) const ID_RESIZE: u32 = 416;
const ID_SELMODE0: u32 = 420;
const ID_WAND_TOL: u32 = 423;
const ID_WAND_CONT: u32 = 424;
pub(super) const ID_D0: u32 = 430;
const ID_COL0: u32 = 440;
const ID_COL_MORE: u32 = 450;
const ID_WIDTH0: u32 = 451;
const ID_ALPHA: u32 = 455;
const ID_FILL: u32 = 456;
const ID_D_UNDO: u32 = 461;
const ID_D_APPLY: u32 = 462;
const ID_D_CANCEL: u32 = 463;
const ID_ADJ_S0: u32 = 470;
const ID_ADJ_APPLY: u32 = 485;
const ID_ADJ_RESET: u32 = 486;
const ID_ADJ_CANCEL: u32 = 487;
const ID_CROP_A0: u32 = 490;
const ID_CROP_FLIP: u32 = 497;
const ID_CROP_OK: u32 = 498;
const ID_CROP_CANCEL: u32 = 499;
const ID_TEXT_BOX: u32 = 1101;
pub(super) const WM_TEXT_COMMIT: u32 = 0x8000 + 24;

pub(super) const FX: [(Fx, &str, u32, &str); 6] = [
    (Fx::Gray, "Ч/Б", gui::IC_COLOR, "Чёрно-белое"),
    (Fx::Sepia, "Сепия", 0xE91B, "Тёплый «старинный» тон"),
    (Fx::Invert, "Негатив", gui::IC_CONTRAST, "Обратить цвета"),
    (Fx::Blur, "Размытие", 0xEA3B, "Размыть (фон, лишние детали)"),
    (Fx::Sharpen, "Резкость", 0xE71C, "Сделать чётче"),
    (Fx::Pixelate, "Пиксели", 0xECA5, "Крупные клетки — скрыть номер, лицо, текст"),
];
const ASPECTS: [(&str, f64); 7] = [("Свободно", 0.0), ("Исходные", -1.0), ("1:1", 1.0), ("4:3", 4.0 / 3.0), ("3:2", 1.5), ("16:9", 16.0 / 9.0), ("A4", 297.0 / 210.0)];
/// Толщина линии и размер текста — в точках экрана при «вписать».
const WIDTHS: [f32; 4] = [2.0, 4.0, 8.0, 16.0];
const TEXT_SIZES: [f32; 4] = [16.0, 24.0, 36.0, 56.0];

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum SelShape {
    Rect,
    Ellipse,
    Lasso,
    Wand,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum ETool {
    None,
    Sel(SelShape),
    Draw(Tool),
}

pub(super) struct TextEdit {
    hwnd: HWND,
    at: (f32, f32),
    size: f32,
    font: HFONT,
    brush: HBRUSH,
}

pub(super) struct Editor {
    pub tool: ETool,
    sel_mode: usize,
    pub sel: Option<Sel>,
    sel_drag: Option<Vec<(f64, f64)>>,
    overlay: Option<((u32, u32), Rc<Img>)>,
    wand_tol: f64,
    wand_contig: bool,
    pub shapes: Vec<Shape>,
    hist: Vec<Vec<Shape>>,
    cur: Option<Shape>,
    color: u32,
    width_i: usize,
    alpha: f64,
    fill: bool,
    text: Option<TextEdit>,
    pub adj_open: bool,
    adj: Adjust,
    proxy: Option<(Rc<Img>, Option<Sel>)>,
    pub preview: Option<Rc<Img>>,
    crop_aspect: usize,
    crop_flip: bool,
    crop_move: Option<(f64, f64)>,
}

impl Editor {
    pub fn new() -> Editor {
        Editor {
            tool: ETool::None,
            sel_mode: 0,
            sel: None,
            sel_drag: None,
            overlay: None,
            wand_tol: settings::get_i("wand_tol", 25).clamp(0, 100) as f64,
            wand_contig: settings::get_i("wand_contig", 1) == 1,
            shapes: vec![],
            hist: vec![],
            cur: None,
            color: draw::COLORS[settings::get_i("pen_color", 0).clamp(0, draw::COLORS.len() as i32 - 1) as usize],
            width_i: settings::get_i("pen_width", 1).clamp(0, 3) as usize,
            alpha: 1.0,
            fill: false,
            text: None,
            adj_open: false,
            adj: Adjust::default(),
            proxy: None,
            preview: None,
            crop_aspect: 0,
            crop_flip: false,
            crop_move: None,
        }
    }

    fn sel_op(&self) -> SelOp {
        let shift = key_down(0x10);
        let alt = key_down(0x12);
        match (shift, alt, self.sel_mode) {
            (true, _, _) | (_, _, 1) => SelOp::Add,
            (_, true, _) | (_, _, 2) => SelOp::Sub,
            _ => SelOp::Replace,
        }
    }
}

fn rgb_to_colorref(c: u32) -> COLORREF {
    crate::util::rgb(((c >> 16) & 255) as u8, ((c >> 8) & 255) as u8, (c & 255) as u8)
}

/// Картинку с готовой альфой — поверх (AlphaBlend), растянув в rc.
fn alpha_blit(hdc: HDC, img: &Img, rc: &RECT) {
    unsafe {
        let info = crate::util::bmi(img);
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let Ok(bmp) = CreateDIBSection(hdc, &info, DIB_RGB_COLORS, &mut bits, None, 0) else { return };
        if bits.is_null() {
            let _ = DeleteObject(bmp);
            return;
        }
        std::ptr::copy_nonoverlapping(img.px.as_ptr(), bits as *mut u8, img.px.len());
        let mdc = CreateCompatibleDC(hdc);
        let old = SelectObject(mdc, bmp);
        let bf = BLENDFUNCTION { BlendOp: 0, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: 1 };
        let _ = AlphaBlend(hdc, rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top, mdc, 0, 0, img.w as i32, img.h as i32, bf);
        SelectObject(mdc, old);
        let _ = DeleteDC(mdc);
        let _ = DeleteObject(bmp);
    }
}

impl App {
    // ------------------------------------------------------- состояние ---
    /// Можно ли править: открыта одна картинка (страница) в постраничном виде.
    pub(super) fn can_edit(&self) -> bool {
        self.viewer_is_none() && self.view.is_some() && !self.scrolling()
    }

    pub(super) fn tool_bar_h(&self, ui: &Ui) -> i32 {
        if self.can_edit() && (self.ed.tool != ETool::None || self.crop_mode) { ui.s(46) } else { 0 }
    }

    pub(super) fn adj_panel_w(&self, ui: &Ui) -> i32 {
        if self.can_edit() && self.ed.adj_open { ui.s(320) } else { 0 }
    }

    /// Точка холста → точка фото.
    fn img_pt(&self, cx: f64, cy: f64) -> (f64, f64) {
        let (dx, dy, _, _) = self.dest();
        let s = self.scale().max(1e-9);
        ((cx - dx) / s, (cy - dy) / s)
    }

    pub(super) fn set_tool(&mut self, t: ETool) {
        self.ensure_full_view();
        self.commit_text();
        if !matches!(t, ETool::Draw(_)) {
            self.commit_shapes();
        }
        if t != ETool::None {
            if !self.work && self.can_work() {
                self.set_work(true);
            }
            self.crop_mode = false;
            self.crop = None;
        }
        self.ed.tool = t;
        self.ed.sel_drag = None;
        self.ed.cur = None;
        match t {
            ETool::Draw(Tool::Text) => self.set_status("Щёлкните на фото, где написать · Esc — готово"),
            ETool::Draw(Tool::Eraser) => self.set_status("Щёлкните или проведите по нарисованному, чтобы стереть"),
            ETool::Draw(_) => self.set_status("Рисуйте мышью · Ctrl+Z — отменить штрих · «Готово» — впечатать в фото"),
            ETool::Sel(SelShape::Wand) => self.set_status("Щёлкните по области похожего цвета · Shift — добавить, Alt — вычесть"),
            ETool::Sel(_) => self.set_status("Обведите область · Shift — добавить, Alt — вычесть · эффекты лягут только на неё"),
            ETool::None => {}
        }
    }

    pub(super) fn has_shapes(&self) -> bool {
        !self.ed.shapes.is_empty() || self.ed.text.is_some() || !self.ed.hist.is_empty()
    }

    pub(super) fn undo_shape(&mut self) {
        if self.ed.text.is_some() {
            self.cancel_text();
        } else if let Some(h) = self.ed.hist.pop() {
            self.ed.shapes = h;
        } else {
            self.ed.shapes.pop();
        }
    }

    pub(super) fn crop_start(&mut self) {
        self.crop_default();
        self.set_status("Обрезка: рамка тянется мышью, внутри — двигается · Enter — обрезать, Esc — отмена");
    }

    pub(super) fn clear_sel(&mut self) {
        self.ed.sel = None;
        self.ed.overlay = None;
        self.ed.proxy = None;
    }

    /// Всё, что «висит» (текст, рисунок, ползунки), — применить к фото.
    pub(super) fn commit_pending(&mut self) {
        self.commit_text();
        self.commit_shapes();
        if self.ed.adj_open {
            self.apply_adjust();
        }
    }

    /// Сброс состояния редактора при смене страницы/файла (несохранённое уже применено).
    pub(super) fn reset_editor(&mut self) {
        self.cancel_text();
        self.ed.shapes.clear();
        self.ed.hist.clear();
        self.ed.cur = None;
        self.ed.adj_open = false;
        self.ed.adj = Adjust::default();
        self.ed.preview = None;
        self.ed.tool = ETool::None;
        self.clear_sel();
    }

    /// Эффект с учётом выделения.
    fn edit_sel(&mut self, f: impl Fn(&Img) -> Img) {
        let sel = self.ed.sel.clone();
        self.edit(move |i| {
            let p = f(i);
            Some(match &sel {
                Some(s) if s.w == i.w && s.h == i.h => edit::blend_mask(i, &p, s),
                _ => p,
            })
        });
        self.ed.proxy = None;
    }

    // ---------------------------------------------------- «Настройка» ---
    fn open_adjust(&mut self) {
        if self.ed.adj_open {
            self.cancel_adjust();
            return;
        }
        self.commit_text();
        self.commit_shapes();
        if !self.work && self.can_work() {
            self.set_work(true);
        }
        self.ed.adj_open = true;
        self.ed.adj = Adjust::default();
        self.ed.preview = None;
        self.ed.proxy = None;
    }

    fn update_preview(&mut self) {
        let Some(v) = self.view.clone() else { return };
        if self.ed.adj.is_identity() {
            self.ed.preview = None;
            return;
        }
        if self.ed.proxy.is_none() {
            let small = codec::thumb(&v, 1400);
            let sel = self.ed.sel.as_ref().map(|s| s.scaled(small.w, small.h));
            self.ed.proxy = Some((Rc::new(small), sel));
        }
        let (p, s) = self.ed.proxy.clone().unwrap();
        let out = edit::apply_adjust(&p, &self.ed.adj);
        let out = match &s {
            Some(s) => edit::blend_mask(&p, &out, s),
            None => out,
        };
        self.ed.preview = Some(Rc::new(out));
    }

    fn apply_adjust(&mut self) {
        let a = self.ed.adj;
        self.ed.adj_open = false;
        self.ed.preview = None;
        self.ed.adj = Adjust::default();
        if !a.is_identity() {
            self.edit_sel(move |i| edit::apply_adjust(i, &a));
            self.set_status("Настройки применены (Ctrl+Z — отменить)");
        }
    }

    fn cancel_adjust(&mut self) {
        self.ed.adj_open = false;
        self.ed.preview = None;
        self.ed.adj = Adjust::default();
    }

    // ------------------------------------------------------- рисование ---
    fn pen_width(&self) -> f32 {
        let fit = self.fit().max(1e-6) as f32;
        let w = if self.ed.tool == ETool::Draw(Tool::Text) { TEXT_SIZES[self.ed.width_i] } else { WIDTHS[self.ed.width_i] };
        w * self.dpi as f32 / 96.0 / fit
    }

    fn commit_shapes(&mut self) {
        if let Some(c) = self.ed.cur.take() {
            if !c.is_trivial() {
                self.ed.shapes.push(c);
            }
        }
        if self.ed.shapes.is_empty() {
            return;
        }
        let shapes = std::mem::take(&mut self.ed.shapes);
        self.ed.hist.clear();
        self.edit(move |i| Some(draw::flatten(i, &shapes)));
        self.set_status("Рисунок впечатан в фото (Ctrl+Z — отменить)");
    }

    fn push_shape_hist(&mut self) {
        self.ed.hist.push(self.ed.shapes.clone());
        if self.ed.hist.len() > 50 {
            self.ed.hist.remove(0);
        }
    }

    fn start_text(&mut self, at: (f64, f64), sx: i32, sy: i32) {
        self.commit_text();
        let size = self.pen_width();
        let px = (size as f64 * self.scale()).round().max(8.0) as i32;
        unsafe {
            let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default();
            let style = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(0x0004 | 0x0040 | 0x0080); // MULTILINE | AUTOVSCROLL | AUTOHSCROLL
            let w = (self.canvas.right - sx - 8).clamp(160, (px * 14).max(240));
            let Ok(h) = CreateWindowExW(WINDOW_EX_STYLE(0), w!("EDIT"), w!(""), style, sx, sy, w, px * 2 + px / 2, self.hwnd, HMENU(ID_TEXT_BOX as usize as *mut _), hinst, None) else { return };
            let face = crate::util::wide("Segoe UI");
            let font = CreateFontW(-px, 0, 0, 0, 700, 0, 0, 0, DEFAULT_CHARSET.0 as u32, 0, 0, CLEARTYPE_QUALITY.0 as u32, 0, PCWSTR(face.as_ptr()));
            SendMessageW(h, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
            let light = self.ed.color == 0xFFFFFF || self.ed.color == 0xFDD835;
            let brush = CreateSolidBrush(if light { crate::util::rgb(60, 60, 60) } else { crate::util::rgb(255, 255, 255) });
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(h);
            self.ed.text = Some(TextEdit { hwnd: h, at: (at.0 as f32, at.1 as f32), size, font, brush });
        }
    }

    fn cancel_text(&mut self) {
        if let Some(t) = self.ed.text.take() {
            unsafe {
                let _ = DestroyWindow(t.hwnd);
                let _ = DeleteObject(t.font);
                let _ = DeleteObject(t.brush);
            }
        }
    }

    pub(super) fn commit_text(&mut self) {
        let Some(t) = self.ed.text.as_ref() else { return };
        let text = unsafe {
            let n = GetWindowTextLengthW(t.hwnd);
            let mut buf = vec![0u16; n as usize + 1];
            let k = GetWindowTextW(t.hwnd, &mut buf);
            String::from_utf16_lossy(&buf[..k as usize]).replace("\r\n", "\n")
        };
        let (at, size) = (t.at, t.size);
        self.cancel_text();
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(self.hwnd);
        }
        if text.trim().is_empty() {
            return;
        }
        self.push_shape_hist();
        let mut s = Shape::new(Tool::Text, at, self.ed.color, size, (self.ed.alpha * 255.0) as u8, false);
        s.text = text;
        self.ed.shapes.push(s);
    }

    pub(super) fn text_ctl_color(&self, hdc: HDC, h: HWND) -> Option<LRESULT> {
        let t = self.ed.text.as_ref().filter(|t| t.hwnd == h)?;
        unsafe {
            SetTextColor(hdc, rgb_to_colorref(self.ed.color));
            let light = self.ed.color == 0xFFFFFF || self.ed.color == 0xFDD835;
            SetBkColor(hdc, if light { crate::util::rgb(60, 60, 60) } else { crate::util::rgb(255, 255, 255) });
        }
        Some(LRESULT(t.brush.0 as isize))
    }

    pub(super) fn is_text_box(&self, h: HWND) -> bool {
        self.ed.text.as_ref().map(|t| t.hwnd == h).unwrap_or(false)
    }

    fn pick_color(&mut self) {
        if let Some(c) = crate::dialogs::pick_color(self.hwnd, self.ed.color) {
            self.ed.color = c;
        }
    }

    // ------------------------------------------------------- обрезка ---
    fn crop_ratio(&self) -> Option<f64> {
        let a = ASPECTS[self.ed.crop_aspect].1;
        let a = if a < 0.0 { self.view.as_ref().map(|v| v.w as f64 / v.h.max(1) as f64)? } else { a };
        if a == 0.0 {
            return None;
        }
        // для фото «стоя» и по кнопке ↻ — книжная рамка
        let portrait = self.view.as_ref().map(|v| v.h > v.w).unwrap_or(false) && ASPECTS[self.ed.crop_aspect].1 > 0.0;
        Some(if portrait != self.ed.crop_flip { 1.0 / a } else { a })
    }

    /// Рамка обрезки по пропорции, вписанная в фото (по центру).
    fn crop_default(&mut self) {
        let Some(r) = self.crop_ratio() else {
            self.crop = None;
            return;
        };
        let (dx, dy, dw, dh) = self.dest();
        let (w, h) = if dw / dh > r { (dh * r, dh) } else { (dw, dw / r) };
        let (w, h) = (w * 0.9, h * 0.9);
        let (cx, cy) = (dx + dw / 2.0, dy + dh / 2.0);
        self.crop = Some((cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0));
    }

    // ------------------------------------------------- команды ---
    /// Команды редактора. true — обработано.
    pub(super) fn editor_command(&mut self, id: u32) -> bool {
        let sel_tools = [(ID_T_RECT, SelShape::Rect), (ID_T_ELL, SelShape::Ellipse), (ID_T_LASSO, SelShape::Lasso), (ID_T_WAND, SelShape::Wand)];
        if let Some((_, s)) = sel_tools.iter().find(|(i, _)| *i == id) {
            let t = ETool::Sel(*s);
            self.set_tool(if self.ed.tool == t { ETool::None } else { t });
            return true;
        }
        if (ID_D0..ID_D0 + draw::TOOLS.len() as u32).contains(&id) {
            let t = ETool::Draw(draw::TOOLS[(id - ID_D0) as usize].0);
            self.set_tool(t);
            return true;
        }
        if (ID_FX0..ID_FX0 + FX.len() as u32).contains(&id) {
            self.commit_pending();
            let fx = FX[(id - ID_FX0) as usize].0;
            self.edit_sel(move |i| edit::apply_fx(i, fx));
            let what = if self.ed.sel.is_some() { "к выделенному" } else { "ко всему фото" };
            self.set_status(format!("«{}» — {what}", FX[(id - ID_FX0) as usize].1));
            return true;
        }
        if (ID_ADJ_S0..ID_ADJ_S0 + ADJ_FIELDS.len() as u32).contains(&id) {
            // двойной щелчок по ползунку — ноль
            self.ed.adj.set((id - ID_ADJ_S0) as usize, 0.0);
            self.update_preview();
            return true;
        }
        if (ID_COL0..ID_COL0 + draw::COLORS.len() as u32).contains(&id) {
            let k = (id - ID_COL0) as usize;
            self.ed.color = draw::COLORS[k];
            settings::set_i("pen_color", k as i32);
            return true;
        }
        if (ID_WIDTH0..ID_WIDTH0 + 4).contains(&id) {
            self.ed.width_i = (id - ID_WIDTH0) as usize;
            settings::set_i("pen_width", self.ed.width_i as i32);
            return true;
        }
        if (ID_SELMODE0..ID_SELMODE0 + 3).contains(&id) {
            self.ed.sel_mode = (id - ID_SELMODE0) as usize;
            return true;
        }
        if (ID_CROP_A0..ID_CROP_A0 + ASPECTS.len() as u32).contains(&id) {
            self.ed.crop_aspect = (id - ID_CROP_A0) as usize;
            self.crop_default();
            return true;
        }
        match id {
            ID_SEL_ALL => {
                if let Some(v) = &self.view {
                    self.ed.sel = Some(Sel::all(v.w, v.h));
                    self.ed.overlay = None;
                    self.ed.proxy = None;
                }
            }
            ID_SEL_NONE => self.clear_sel(),
            ID_SEL_INV => {
                let v = self.view.clone();
                match (&mut self.ed.sel, v) {
                    (Some(s), _) => s.invert(),
                    (None, Some(v)) => self.ed.sel = Some(Sel::empty(v.w, v.h)),
                    _ => {}
                }
                if let Some(s) = &self.ed.sel {
                    if s.is_empty() {
                        self.ed.sel = None;
                    }
                }
                self.ed.overlay = None;
                self.ed.proxy = None;
            }
            ID_ADJ => self.open_adjust(),
            ID_ADJ_APPLY => self.apply_adjust(),
            ID_ADJ_CANCEL => self.cancel_adjust(),
            ID_ADJ_RESET => {
                self.ed.adj = Adjust::default();
                self.ed.preview = None;
            }
            ID_WAND_TOL => {
                self.ed.wand_tol = 25.0;
            }
            ID_WAND_CONT => {
                self.ed.wand_contig = !self.ed.wand_contig;
                settings::set_i("wand_contig", self.ed.wand_contig as i32);
            }
            ID_COL_MORE => self.pick_color(),
            ID_ALPHA => self.ed.alpha = 1.0,
            ID_FILL => self.ed.fill = !self.ed.fill,
            ID_D_UNDO => {
                if let Some(h) = self.ed.hist.pop() {
                    self.ed.shapes = h;
                }
            }
            ID_D_APPLY => {
                self.commit_text();
                self.commit_shapes();
                self.set_tool(ETool::None);
            }
            ID_D_CANCEL => {
                self.cancel_text();
                self.ed.shapes.clear();
                self.ed.hist.clear();
                self.ed.cur = None;
                self.set_tool(ETool::None);
            }
            ID_CROP_FLIP => {
                self.ed.crop_flip = !self.ed.crop_flip;
                self.crop_default();
            }
            ID_CROP_OK => self.apply_crop(),
            ID_CROP_CANCEL => {
                self.crop_mode = false;
                self.crop = None;
            }
            ID_RESIZE => self.resize_dialog(),
            _ => return false,
        }
        true
    }

    /// Ползунки (тянут).
    pub(super) fn editor_slide(&mut self, id: u32, v: f64) -> bool {
        if (ID_ADJ_S0..ID_ADJ_S0 + ADJ_FIELDS.len() as u32).contains(&id) {
            let k = (id - ID_ADJ_S0) as usize;
            let (_, lo, hi) = ADJ_FIELDS[k];
            self.ed.adj.set(k, (lo + v * (hi - lo)).round());
            self.update_preview();
            return true;
        }
        match id {
            ID_WAND_TOL => {
                self.ed.wand_tol = (v * 100.0).round();
                settings::set_i("wand_tol", self.ed.wand_tol as i32);
                true
            }
            ID_ALPHA => {
                self.ed.alpha = (0.1 + v * 0.9).clamp(0.1, 1.0);
                true
            }
            _ => false,
        }
    }

    pub(super) fn resize_dialog(&mut self) {
        self.commit_pending();
        if self.scrolling() {
            self.set_scroll(false);
        }
        let Some(img) = self.view.clone() else { return };
        let name = self.stem();
        let dir = self.default_dir();
        match crate::resize_dlg::run(self.hwnd, (*img).clone(), &name, dir) {
            crate::resize_dlg::Outcome::Apply(new) => {
                let (w, h) = (new.w, new.h);
                self.edit(move |_| Some(new));
                self.clear_sel();
                self.set_status(format!("Новый размер: {w} × {h}"));
            }
            crate::resize_dlg::Outcome::Saved(p) => {
                self.set_status(format!("Сохранено: {}", file_name(&p)));
                let dp = self.doc_path.clone();
                if dp.is_some() {
                    self.set_siblings(dp.as_deref());
                }
            }
            crate::resize_dlg::Outcome::None => {}
        }
    }

    // ------------------------------------------------------- клавиши ---
    pub(super) fn editor_key(&mut self, vk: u16, ctrl: bool) -> Option<bool> {
        if !self.can_edit() {
            return None;
        }
        if self.ed.text.is_some() {
            if vk == 0x1B {
                self.commit_text();
                return Some(true);
            }
            if vk == 0x0D && ctrl {
                self.commit_text();
                return Some(true);
            }
        }
        match (vk, ctrl) {
            (0x1B, _) => {
                if self.crop_mode {
                    self.crop_mode = false;
                    self.crop = None;
                } else if self.ed.cur.is_some() || self.ed.sel_drag.is_some() {
                    self.ed.cur = None;
                    self.ed.sel_drag = None;
                } else if self.ed.tool != ETool::None {
                    let draw = matches!(self.ed.tool, ETool::Draw(_));
                    self.set_tool(ETool::None);
                    if draw {
                        self.set_status("Рисунок впечатан в фото (Ctrl+Z — отменить)");
                    }
                } else if self.ed.adj_open {
                    self.cancel_adjust();
                } else if self.ed.sel.is_some() {
                    self.clear_sel();
                } else {
                    return None;
                }
                Some(true)
            }
            (0x0D, false) if self.ed.adj_open => {
                self.apply_adjust();
                Some(true)
            }
            (0x5A, true) if self.has_shapes() => {
                self.undo_shape();
                Some(true)
            }
            (0x41, true) => {
                self.editor_command(ID_SEL_ALL);
                Some(true)
            }
            (0x44, true) => {
                self.clear_sel();
                Some(true)
            }
            (0x2E, false) if self.ed.sel.is_some() => {
                // Delete при выделении — залить белым
                let sel = self.ed.sel.clone();
                if let Some(s) = sel {
                    self.edit(move |i| Some(edit::blend_mask(i, &Img::filled(i.w, i.h, [255, 255, 255]), &s)));
                }
                Some(true)
            }
            _ => None,
        }
    }

    // --------------------------------------------------------- мышь ---
    pub(super) fn editor_mouse(&mut self, m: Mouse, cx: f64, cy: f64, in_canvas: bool) -> Option<bool> {
        if !self.can_edit() {
            return None;
        }
        // обрезка: рамка с пропорцией, тянуть внутри — двигать
        if self.crop_mode {
            let ratio = self.crop_ratio();
            match m {
                Mouse::Down if in_canvas => {
                    if let Some((a, b, c, d)) = self.crop {
                        let (l, r, t, bt) = (a.min(c), a.max(c), b.min(d), b.max(d));
                        if cx > l && cx < r && cy > t && cy < bt {
                            self.ed.crop_move = Some((cx, cy));
                            return Some(true);
                        }
                    }
                    self.crop = Some((cx, cy, cx, cy));
                    self.drag = Some((cx, cy));
                    return Some(true);
                }
                Mouse::Move => {
                    if let Some((px, py)) = self.ed.crop_move {
                        if let Some(c) = &mut self.crop {
                            let (dx, dy) = (cx - px, cy - py);
                            c.0 += dx;
                            c.2 += dx;
                            c.1 += dy;
                            c.3 += dy;
                        }
                        self.ed.crop_move = Some((cx, cy));
                        return Some(true);
                    }
                    if self.drag.is_some() {
                        let (cw, ch) = (self.cw(), self.ch());
                        if let Some(c) = &mut self.crop {
                            let (x, y) = (cx.clamp(0.0, cw), cy.clamp(0.0, ch));
                            let (mut w, mut h) = (x - c.0, y - c.1);
                            if let Some(r) = ratio {
                                let sg = |v: f64| if v < 0.0 { -1.0 } else { 1.0 };
                                if w.abs() / r > h.abs() {
                                    h = sg(h) * w.abs() / r;
                                } else {
                                    w = sg(w) * h.abs() * r;
                                }
                            }
                            let (x, y) = (c.0 + w, c.1 + h);
                            c.2 = x;
                            c.3 = y;
                        }
                        return Some(true);
                    }
                    return Some(false);
                }
                Mouse::Up => {
                    self.drag = None;
                    self.ed.crop_move = None;
                    return Some(true);
                }
                Mouse::Dbl if in_canvas => {
                    self.apply_crop();
                    return Some(true);
                }
                _ => return None,
            }
        }
        let tool = self.ed.tool;
        if tool == ETool::None {
            return None;
        }
        let p = self.img_pt(cx, cy);
        let (iw, ih) = self.view.as_ref().map(|v| (v.w, v.h)).unwrap_or((1, 1));
        match tool {
            ETool::Sel(shape) => match m {
                Mouse::Down if in_canvas => {
                    if shape == SelShape::Wand {
                        let v = self.view.clone()?;
                        let (x, y) = (p.0.floor(), p.1.floor());
                        if x >= 0.0 && y >= 0.0 && (x as u32) < iw && (y as u32) < ih {
                            let s = Sel::wand(&v, x as u32, y as u32, self.ed.wand_tol, self.ed.wand_contig);
                            self.combine_sel(s);
                        }
                        return Some(true);
                    }
                    self.ed.sel_drag = Some(vec![p, p]);
                    Some(true)
                }
                Mouse::Move => {
                    let sc = self.scale();
                    let first_sel = self.ed.sel.is_none();
                    let Some(d) = &mut self.ed.sel_drag else { return Some(false) };
                    if shape == SelShape::Lasso {
                        let last = *d.last().unwrap();
                        if (last.0 - p.0).abs() + (last.1 - p.1).abs() > 1.5 / sc {
                            d.push(p);
                        }
                    } else {
                        let shift = key_down(0x10) && first_sel;
                        let a = d[0];
                        d[1] = if shift {
                            let m2 = (p.0 - a.0).abs().max((p.1 - a.1).abs());
                            (a.0 + m2 * (p.0 - a.0).signum(), a.1 + m2 * (p.1 - a.1).signum())
                        } else {
                            p
                        };
                    }
                    Some(true)
                }
                Mouse::Up => {
                    let Some(d) = self.ed.sel_drag.take() else { return Some(false) };
                    let tiny = d.len() <= 2 && (d[0].0 - d[d.len() - 1].0).abs() * self.scale() < 3.0 && (d[0].1 - d[d.len() - 1].1).abs() * self.scale() < 3.0;
                    if tiny {
                        if self.ed.sel_op() == SelOp::Replace {
                            self.clear_sel();
                        }
                        return Some(true);
                    }
                    let s = match shape {
                        SelShape::Rect => Sel::rect(iw, ih, d[0], d[1]),
                        SelShape::Ellipse => Sel::ellipse(iw, ih, d[0], d[1]),
                        _ => Sel::polygon(iw, ih, &d),
                    };
                    self.combine_sel(s);
                    Some(true)
                }
                _ => None,
            },
            ETool::Draw(t) => match m {
                Mouse::Down if in_canvas => {
                    let pf = (p.0 as f32, p.1 as f32);
                    match t {
                        Tool::Text => {
                            if self.ed.text.is_some() {
                                self.commit_text();
                            } else {
                                let sx = self.canvas.left + cx.round() as i32;
                                let sy = self.canvas.top + cy.round() as i32;
                                self.start_text(p, sx, sy);
                            }
                        }
                        Tool::Eraser => {
                            self.push_shape_hist();
                            self.erase_at(pf);
                            self.ed.cur = Some(Shape::new(Tool::Eraser, pf, 0, 0.0, 0, false));
                        }
                        _ => {
                            let a = (self.ed.alpha * 255.0).round() as u8;
                            self.ed.cur = Some(Shape::new(t, pf, self.ed.color, self.pen_width(), a, self.ed.fill));
                        }
                    }
                    Some(true)
                }
                Mouse::Move => {
                    let pf = (p.0 as f32, p.1 as f32);
                    let shift = key_down(0x10);
                    match &mut self.ed.cur {
                        Some(c) if c.tool == Tool::Eraser => {
                            self.erase_at(pf);
                            Some(true)
                        }
                        Some(c) => {
                            c.extend(pf, shift);
                            Some(true)
                        }
                        None => Some(false),
                    }
                }
                Mouse::Up => {
                    if let Some(c) = self.ed.cur.take() {
                        if c.tool != Tool::Eraser && !c.is_trivial() {
                            self.push_shape_hist();
                            self.ed.shapes.push(c);
                        } else if c.tool == Tool::Eraser && self.ed.hist.last().map(|h| h.len()) == Some(self.ed.shapes.len()) {
                            // ничего не стёрли — шаг отмены не нужен
                            self.ed.hist.pop();
                        }
                    }
                    Some(true)
                }
                Mouse::Dbl => Some(true),
                _ => None,
            },
            ETool::None => None,
        }
    }

    fn erase_at(&mut self, p: (f32, f32)) {
        let r = (8.0 * self.dpi as f64 / 96.0 / self.scale()) as f32;
        self.ed.shapes.retain(|s| !s.hit(p, r));
    }

    fn combine_sel(&mut self, s: Sel) {
        let op = self.ed.sel_op();
        match (&mut self.ed.sel, op) {
            (Some(cur), SelOp::Add | SelOp::Sub | SelOp::Intersect) => cur.combine(&s, op),
            _ => self.ed.sel = if op == SelOp::Sub { None } else { Some(s) },
        }
        if self.ed.sel.as_ref().map(|s| s.is_empty()).unwrap_or(false) {
            self.ed.sel = None;
        }
        self.ed.overlay = None;
        self.ed.proxy = None;
        if self.ed.adj_open {
            self.update_preview();
        }
    }

    pub(super) fn editor_cursor(&self, x: i32, y: i32) -> Option<PCWSTR> {
        if !self.can_edit() || !gui::inside(&self.canvas, x, y) {
            return None;
        }
        match self.ed.tool {
            ETool::Sel(_) => Some(IDC_CROSS),
            ETool::Draw(Tool::Text) => Some(IDC_IBEAM),
            ETool::Draw(_) => Some(IDC_CROSS),
            ETool::None if self.crop_mode => {
                if let Some((a, b, c, d)) = self.crop {
                    let (cx, cy) = ((x - self.canvas.left) as f64, (y - self.canvas.top) as f64);
                    if cx > a.min(c) && cx < a.max(c) && cy > b.min(d) && cy < b.max(d) {
                        return Some(IDC_SIZEALL);
                    }
                }
                Some(IDC_CROSS)
            }
            ETool::None => None,
        }
    }

    // ----------------------------------------------------- отрисовка ---
    /// Поверх фото: подсветка выделения, рисунок, текущая фигура.
    pub(super) fn paint_editor(&mut self, ui: &Ui, hdc: HDC) {
        if !self.can_edit() {
            return;
        }
        let c = self.canvas;
        let (dx, dy, dw, dh) = self.dest();
        let s = self.scale();
        let dest = gui::r(c.left + dx.round() as i32, c.top + dy.round() as i32, dw.round() as i32, dh.round() as i32);
        // выделение
        if let Some(sel) = &self.ed.sel {
            let want = ((dw.round() as u32).clamp(1, 2400.min(sel.w.max(1) * 4)), (dh.round() as u32).clamp(1, 2400.min(sel.h.max(1) * 4)));
            let ok = self.ed.overlay.as_ref().map(|(sz, _)| {
                let (a, b) = (sz.0 as f64, want.0 as f64);
                (a - b).abs() / b < 0.08
            });
            if ok != Some(true) {
                let t = ui.t;
                let acc = [((t.accent.0 >> 16) & 255) as u8, ((t.accent.0 >> 8) & 255) as u8, (t.accent.0 & 255) as u8];
                self.ed.overlay = Some((want, Rc::new(sel.overlay(want.0, want.1, acc))));
            }
            if let Some((_, ov)) = &self.ed.overlay {
                alpha_blit(hdc, ov, &dest);
            }
        }
        // фигуры
        let ox = c.left as f64 + dx;
        let oy = c.top as f64 + dy;
        let mut all: Vec<Shape> = self.ed.shapes.clone();
        if let Some(cur) = &self.ed.cur {
            if cur.tool != Tool::Eraser {
                all.push(cur.clone());
            }
        }
        draw::paint(hdc, &all, ox, oy, s);
        // выделение в процессе
        if let Some(d) = &self.ed.sel_drag {
            let to = |p: (f64, f64)| POINT { x: (ox + p.0 * s).round() as i32, y: (oy + p.1 * s).round() as i32 };
            unsafe {
                let pen = CreatePen(PS_DOT, 1, ui.t.accent);
                let oldp = SelectObject(hdc, pen);
                let oldb = SelectObject(hdc, GetStockObject(NULL_BRUSH));
                SetBkColor(hdc, crate::util::rgb(255, 255, 255));
                match self.ed.tool {
                    ETool::Sel(SelShape::Rect) => {
                        let (a, b) = (to(d[0]), to(d[1]));
                        let _ = Rectangle(hdc, a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y));
                    }
                    ETool::Sel(SelShape::Ellipse) => {
                        let (a, b) = (to(d[0]), to(d[1]));
                        let _ = Ellipse(hdc, a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y));
                    }
                    _ => {
                        let pts: Vec<POINT> = d.iter().map(|p| to(*p)).collect();
                        let _ = Polyline(hdc, &pts);
                    }
                }
                SelectObject(hdc, oldb);
                SelectObject(hdc, oldp);
                let _ = DeleteObject(pen);
            }
        }
    }

    /// Фон полосы инструментов и панели «Настройка».
    pub(super) fn paint_editor_back(&self, ui: &Ui, hdc: HDC) {
        let bh = self.tool_bar_h(ui);
        if bh > 0 {
            let r = gui::r(self.area.left, self.canvas.top - bh, self.area.right - self.area.left, bh);
            gui::fill(hdc, &r, ui.t.bg);
            gui::line(hdc, r.left, r.bottom - 1, r.right, r.bottom - 1, ui.t.border, 1, PS_SOLID);
        }
        let pw = self.adj_panel_w(ui);
        if pw > 0 {
            let r = RECT { left: self.canvas.right, top: self.canvas.top, right: self.area.right, bottom: self.area.bottom };
            gui::fill(hdc, &r, ui.t.bg);
            gui::line(hdc, r.left, r.top, r.left, r.bottom, ui.t.border, 1, PS_SOLID);
        }
    }

    // ---------------------------------------------------- элементы ---
    pub(super) fn layout_editor(&mut self, ui: &Ui, v: &mut Vec<W>) {
        use crate::gui::Kind;
        let bh = self.tool_bar_h(ui);
        if bh > 0 {
            let y = self.canvas.top - bh + ui.s(6);
            let h = bh - ui.s(12);
            let mut x = self.area.left + ui.s(10);
            let right = self.area.right - ui.s(10);
            let gap = ui.s(6);
            let sep = |v: &mut Vec<W>, x: &mut i32| {
                v.push(W::new(Kind::Sep, 0, gui::r(*x + ui.s(4), y + ui.s(4), ui.s(2), h - ui.s(8))));
                *x += ui.s(12);
            };
            if self.crop_mode {
                let names: Vec<&str> = ASPECTS.iter().map(|a| a.0).collect();
                let sw = ui.s(78) * names.len() as i32;
                v.extend(gui::segments(ui, x, y, sw, ID_CROP_A0, &names, self.ed.crop_aspect));
                x += sw + gap;
                v.push(W::new(Kind::Tool, ID_CROP_FLIP, gui::r(x, y, h, h)).i(0xE7AD).tip("Повернуть рамку: альбомная ↔ книжная").en(ASPECTS[self.ed.crop_aspect].1 != 0.0).on(self.ed.crop_flip));
                let bw = ui.s(110);
                v.push(W::new(Kind::Button, ID_CROP_OK, gui::r(right - bw, y, bw, h)).t("Обрезать").on(true).en(self.crop.is_some()).tip("Применить (Enter или двойной щелчок по рамке)"));
                v.push(W::new(Kind::Button, ID_CROP_CANCEL, gui::r(right - 2 * bw - gap, y, bw, h)).t("Отмена").tip("Esc"));
            } else if let ETool::Sel(shape) = self.ed.tool {
                let sw = ui.s(300);
                v.extend(gui::segments(ui, x, y, sw, ID_SELMODE0, &["Новое", "Добавить", "Вычесть"], self.ed.sel_mode));
                x += sw + gap;
                if shape == SelShape::Wand {
                    sep(v, &mut x);
                    v.push(W::new(Kind::Label, 0, gui::r(x, y, ui.s(110), h)).t(format!("Допуск: {}", self.ed.wand_tol)));
                    x += ui.s(110);
                    v.push(W::new(Kind::Slider, ID_WAND_TOL, gui::r(x, y, ui.s(150), h)).v(self.ed.wand_tol / 100.0).tip("Насколько похожими должны быть цвета"));
                    x += ui.s(150) + gap;
                    v.push(W::new(Kind::Check, ID_WAND_CONT, gui::r(x, y, ui.s(120), h)).t("Смежные").on(self.ed.wand_contig).tip("Только связная область (иначе — все такие цвета на фото)"));
                    x += ui.s(120);
                }
                let _ = x;
                let bw = ui.s(150);
                v.push(W::new(Kind::Button, ID_SEL_NONE, gui::r(right - bw, y, bw, h)).t("Снять выделение").en(self.ed.sel.is_some()).tip("Ctrl+D"));
            } else if let ETool::Draw(t) = self.ed.tool {
                for (k, &c) in draw::COLORS.iter().enumerate() {
                    v.push(W::new(Kind::Swatch, ID_COL0 + k as u32, gui::r(x, y, h, h)).color(rgb_to_colorref(c)).on(self.ed.color == c));
                    x += h + ui.s(2);
                }
                let custom = !draw::COLORS.contains(&self.ed.color);
                v.push(W::new(Kind::Tool, ID_COL_MORE, gui::r(x, y, h, h)).i(0xE790).tip("Другой цвет…").on(custom));
                x += h + gap;
                sep(v, &mut x);
                let wn: [&str; 4] = if t == Tool::Text { ["A", "A+", "A++", "A+++"] } else { ["S", "M", "L", "XL"] };
                let sw = ui.s(180);
                v.extend(gui::segments(ui, x, y, sw, ID_WIDTH0, &wn, self.ed.width_i));
                x += sw + gap;
                if matches!(t, Tool::Pen | Tool::Line | Tool::Arrow | Tool::Rect | Tool::Ellipse | Tool::Text) {
                    sep(v, &mut x);
                    v.push(W::new(Kind::Label, 0, gui::r(x, y, ui.s(48), h)).t(format!("{}%", (self.ed.alpha * 100.0).round())).tip("Непрозрачность"));
                    x += ui.s(48);
                    v.push(W::new(Kind::Slider, ID_ALPHA, gui::r(x, y, ui.s(110), h)).v((self.ed.alpha - 0.1) / 0.9).tip("Непрозрачность (двойной щелчок — 100%)"));
                    x += ui.s(110) + gap;
                }
                if matches!(t, Tool::Rect | Tool::Ellipse) {
                    v.push(W::new(Kind::Check, ID_FILL, gui::r(x, y, ui.s(100), h)).t("Заливка").on(self.ed.fill));
                }
                let bw = ui.s(100);
                v.push(W::new(Kind::Button, ID_D_APPLY, gui::r(right - bw, y, bw, h)).t("Готово").on(true).tip("Впечатать рисунок в фото"));
                v.push(W::new(Kind::Button, ID_D_CANCEL, gui::r(right - 2 * bw - gap, y, bw, h)).t("Стереть всё").en(!self.ed.shapes.is_empty() || self.ed.text.is_some()).tip("Убрать всё нарисованное сейчас"));
                v.push(W::new(Kind::Tool, ID_D_UNDO, gui::r(right - 2 * bw - 2 * gap - h, y, h, h)).i(gui::IC_UNDO).en(!self.ed.hist.is_empty()).tip("Отменить штрих (Ctrl+Z)"));
            }
        }
        let pw = self.adj_panel_w(ui);
        if pw > 0 {
            let x0 = self.canvas.right + ui.s(14);
            let w = self.area.right - x0 - ui.s(14);
            let mut y = self.canvas.top + ui.s(12);
            v.push(W::new(Kind::Group, 0, gui::r(x0, y, w, ui.s(22))).t("Настройка"));
            y += ui.s(22);
            let to = if self.ed.sel.is_some() { "К выделенному (остальное не меняется)" } else { "Ко всему фото" };
            v.push(W::new(Kind::Label, 0, gui::r(x0, y, w, ui.s(20))).t(to).color(ui.t.dim));
            y += ui.s(26);
            let avail = self.area.bottom - y - ui.s(60);
            let rh = (avail / ADJ_FIELDS.len() as i32).clamp(ui.s(26), ui.s(38));
            for (k, (name, lo, hi)) in ADJ_FIELDS.iter().enumerate() {
                let val = self.ed.adj.get(k);
                let lw = ui.s(104);
                let vw = ui.s(40);
                let on = val != 0.0;
                v.push(W::new(Kind::Label, 0, gui::r(x0, y, lw, rh)).t(*name).on(on));
                let sl = W::new(Kind::Slider, ID_ADJ_S0 + k as u32, gui::r(x0 + lw, y, w - lw - vw, rh)).v((val - lo) / (hi - lo)).tip("Двойной щелчок — ноль");
                v.push(if *lo < 0.0 { sl.align(1) } else { sl });
                v.push(W::new(Kind::Label, 0, gui::r(x0 + w - vw, y, vw, rh)).t(format!("{val:.0}")).align(2).color(if on { ui.t.accent } else { ui.t.dim }));
                y += rh;
            }
            let by = self.area.bottom - ui.s(50);
            let bw = (w - 2 * ui.s(8)) / 3;
            v.push(W::new(Kind::Button, ID_ADJ_APPLY, gui::r(x0 + 2 * (bw + ui.s(8)), by, bw, ui.s(36))).t("Применить").on(true).tip("Enter"));
            v.push(W::new(Kind::Button, ID_ADJ_RESET, gui::r(x0 + bw + ui.s(8), by, bw, ui.s(36))).t("Сбросить").en(!self.ed.adj.is_identity()));
            v.push(W::new(Kind::Button, ID_ADJ_CANCEL, gui::r(x0, by, bw, ui.s(36))).t("Отмена").tip("Esc"));
        }
    }
}
