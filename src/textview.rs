// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Текст и код: редактор уровня «Блокнота» на системном RichEdit.
//! Отмена/повтор, поиск и замена, перенос строк, масштаб, подсветка
//! синтаксиса, сохранение в исходной кодировке и с исходными переводами строк.

use crate::gui::{self, Kind, Ui, W};
use crate::highlight::{self, Lang, Tok};
use crate::textenc::{self, Enc, Eol};
use crate::theme::Theme;
use crate::util::{rgb, wide};
use std::path::{Path, PathBuf};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryW};
use windows::Win32::UI::Controls::RichEdit::*;
use windows::Win32::UI::Controls::SetWindowTheme;
use windows::Win32::UI::Controls::Dialogs::{FR_DOWN, FR_MATCHCASE};
use windows::Win32::UI::WindowsAndMessaging::*;

// базовые сообщения поля ввода (общие для EDIT и RichEdit)
const EM_SETSEL: u32 = 0x00B1;
const EM_SCROLLCARET: u32 = 0x00B7;
const EM_GETLINECOUNT: u32 = 0x00BA;
const EM_GETMODIFY: u32 = 0x00B8;
const EM_SETMODIFY: u32 = 0x00B9;
const EM_LINEINDEX: u32 = 0x00BB;
const EM_REPLACESEL: u32 = 0x00C2;
const EM_UNDO: u32 = 0x00C7;
const EM_CANUNDO: u32 = 0x00C6;
const EM_EMPTYUNDOBUFFER: u32 = 0x00CD;
const EM_GETFIRSTVISIBLELINE: u32 = 0x00CE;
const EM_SETMARGINS: u32 = 0x00D3;
const EC_LEFTMARGIN: u32 = 1;
const EC_RIGHTMARGIN: u32 = 2;
const ES_MULTILINE: i32 = 0x0004;
const ES_AUTOVSCROLL: i32 = 0x0040;
const ES_AUTOHSCROLL: i32 = 0x0080;
const ES_NOHIDESEL: i32 = 0x0100;
const ES_WANTRETURN: i32 = 0x1000;
const ES_SAVESEL: i32 = 0x8000;

pub const ID_EDIT: u32 = 1001;
pub const ID_FIND_BOX: u32 = 1002;
pub const ID_REPL_BOX: u32 = 1003;
pub const ID_FIND_NEXT: u32 = 1010;
pub const ID_FIND_PREV: u32 = 1011;
pub const ID_REPLACE: u32 = 1012;
pub const ID_REPLACE_ALL: u32 = 1013;
pub const ID_FIND_CASE: u32 = 1014;
pub const ID_FIND_CLOSE: u32 = 1015;

/// Больше этого — без подсветки (чтобы не тормозить).
const HL_LIMIT: usize = 3_000_000;

pub struct TextView {
    pub path: PathBuf,
    pub enc: Enc,
    pub eol: Eol,
    pub lang: Lang,
    re: HWND,
    find: HWND,
    repl: HWND,
    pub find_open: bool,
    pub replace_open: bool,
    pub match_case: bool,
    pub wrap: bool,
    big: bool,
    hl_due: bool,
    last_first_line: i32,
    brush: HBRUSH,
    font: HFONT,
    font_dpi: u32,
    theme_dark: Option<bool>,
    doc: Option<ITextDocument>,
}

impl Drop for TextView {
    fn drop(&mut self) {
        unsafe {
            self.doc = None;
            let _ = DestroyWindow(self.re);
            let _ = DestroyWindow(self.find);
            let _ = DestroyWindow(self.repl);
            if !self.brush.is_invalid() {
                let _ = DeleteObject(self.brush);
            }
            if !self.font.is_invalid() {
                let _ = DeleteObject(self.font);
            }
        }
    }
}

struct InBuf<'a> {
    data: &'a [u8],
    pos: usize,
}

unsafe extern "system" fn stream_in(cookie: usize, buf: *mut u8, cb: i32, pcb: *mut i32) -> u32 {
    let b = &mut *(cookie as *mut InBuf);
    let n = (cb as usize).min(b.data.len() - b.pos);
    std::ptr::copy_nonoverlapping(b.data.as_ptr().add(b.pos), buf, n);
    b.pos += n;
    *pcb = n as i32;
    0
}

unsafe extern "system" fn stream_out(cookie: usize, buf: *mut u8, cb: i32, pcb: *mut i32) -> u32 {
    let v = &mut *(cookie as *mut Vec<u8>);
    v.extend_from_slice(std::slice::from_raw_parts(buf, cb as usize));
    *pcb = cb;
    0
}

fn colorref_i32(c: COLORREF) -> i32 {
    c.0 as i32
}

/// Цвета подсветки под тему.
fn tok_color(t: Tok, dark: bool) -> COLORREF {
    if dark {
        match t {
            Tok::Comment => rgb(106, 153, 85),
            Tok::Str => rgb(206, 145, 120),
            Tok::Num => rgb(181, 206, 168),
            Tok::Kw => rgb(86, 156, 214),
            Tok::Type => rgb(78, 201, 176),
            Tok::Tag => rgb(86, 156, 214),
            Tok::Attr => rgb(156, 220, 254),
            Tok::Key => rgb(156, 220, 254),
        }
    } else {
        match t {
            Tok::Comment => rgb(0, 128, 0),
            Tok::Str => rgb(163, 21, 21),
            Tok::Num => rgb(9, 134, 88),
            Tok::Kw => rgb(0, 0, 255),
            Tok::Type => rgb(38, 127, 153),
            Tok::Tag => rgb(128, 0, 0),
            Tok::Attr => rgb(229, 20, 0),
            Tok::Key => rgb(4, 81, 165),
        }
    }
}

impl TextView {
    pub fn open(parent: HWND, path: &Path) -> std::result::Result<TextView, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        if bytes.len() > 200_000_000 {
            return Err("Файл слишком большой для редактора (больше 200 МБ)".into());
        }
        let (text, enc) = textenc::decode(&bytes);
        let eol = textenc::detect_eol(&text);
        let ext = crate::util::ext_lower(path);
        let name = crate::util::file_name(path).to_lowercase();
        let lang = highlight::lang_for(&ext, &name);
        unsafe {
            if LoadLibraryW(w!("Msftedit.dll")).is_err() {
                return Err("Не найден системный компонент RichEdit (Msftedit.dll)".into());
            }
            let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
            let style = WS_CHILD | WS_VSCROLL | WS_HSCROLL | WS_TABSTOP
                | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_AUTOHSCROLL | ES_NOHIDESEL | ES_WANTRETURN | ES_SAVESEL) as u32);
            let re = CreateWindowExW(WINDOW_EX_STYLE(0), w!("RICHEDIT50W"), w!(""), style, 0, 0, 10, 10, parent, HMENU(ID_EDIT as usize as *mut _), hinst, None)
                .map_err(|e| e.to_string())?;
            SendMessageW(re, EM_SETTEXTMODE, WPARAM((TM_RICHTEXT.0 | TM_MULTILEVELUNDO.0) as usize), LPARAM(0));
            SendMessageW(re, EM_EXLIMITTEXT, WPARAM(0), LPARAM(0x7FFF_FFFF));
            SendMessageW(re, EM_SETMARGINS, WPARAM((EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize), LPARAM(((10 << 16) | 10) as isize));
            let edit = |id: u32| {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0), w!("EDIT"), w!(""),
                    WS_CHILD | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                    0, 0, 10, 10, parent, HMENU(id as usize as *mut _), hinst, None,
                )
                .unwrap_or_default()
            };
            let find = edit(ID_FIND_BOX);
            let repl = edit(ID_REPL_BOX);
            let mut tv = TextView {
                path: path.to_path_buf(), enc, eol, lang, re, find, repl, find_open: false, replace_open: false,
                match_case: false, wrap: crate::settings::get_i("text_wrap", 0) == 1, big: text.len() > HL_LIMIT,
                hl_due: true, last_first_line: -1, brush: HBRUSH::default(), font: HFONT::default(), font_dpi: 0,
                theme_dark: None, doc: None,
            };
            // TOM — для подсветки без засорения истории отмены
            let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
            SendMessageW(re, EM_GETOLEINTERFACE, WPARAM(0), LPARAM(&mut p as *mut _ as isize));
            if !p.is_null() {
                let ole = IRichEditOle::from_raw(p);
                tv.doc = ole.cast::<ITextDocument>().ok();
            }
            tv.set_text(&text);
            tv.apply_wrap();
            SendMessageW(re, EM_SETEVENTMASK, WPARAM(0), LPARAM((ENM_CHANGE | ENM_SELCHANGE | ENM_SCROLL) as isize));
            SendMessageW(re, EM_SETMODIFY, WPARAM(0), LPARAM(0));
            let _ = ShowWindow(re, SW_SHOW);
            Ok(tv)
        }
    }

    fn set_text(&mut self, text: &str) {
        let u: Vec<u16> = text.encode_utf16().collect();
        let bytes: Vec<u8> = u.iter().flat_map(|c| c.to_le_bytes()).collect();
        let mut ib = InBuf { data: &bytes, pos: 0 };
        let mut es = EDITSTREAM { dwCookie: &mut ib as *mut _ as usize, dwError: 0, pfnCallback: Some(stream_in) };
        unsafe {
            SendMessageW(self.re, EM_STREAMIN, WPARAM((SF_TEXT | SF_UNICODE) as usize), LPARAM(&mut es as *mut _ as isize));
            SendMessageW(self.re, EM_EMPTYUNDOBUFFER, WPARAM(0), LPARAM(0));
            SendMessageW(self.re, EM_SETSEL, WPARAM(0), LPARAM(0));
        }
    }

    /// Текст в том виде, как записать в файл (с переводами строк файла).
    pub fn text_for_save(&self) -> String {
        let mut out: Vec<u8> = vec![];
        let mut es = EDITSTREAM { dwCookie: &mut out as *mut _ as usize, dwError: 0, pfnCallback: Some(stream_out) };
        unsafe {
            SendMessageW(self.re, EM_STREAMOUT, WPARAM((SF_TEXT | SF_UNICODE) as usize), LPARAM(&mut es as *mut _ as isize));
        }
        let u: Vec<u16> = out.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        textenc::normalize_eol(&String::from_utf16_lossy(&u), self.eol)
    }

    /// Текст как его видит RichEdit (переводы строк — '\r'), UTF-16.
    fn text_u16(&self) -> Vec<u16> {
        unsafe {
            let g = GETTEXTLENGTHEX { flags: GTL_NUMCHARS | GTL_PRECISE, codepage: 1200 };
            let n = SendMessageW(self.re, EM_GETTEXTLENGTHEX, WPARAM(&g as *const _ as usize), LPARAM(0)).0.max(0) as usize;
            let mut buf = vec![0u16; n + 1];
            let gt = GETTEXTEX { cb: ((n + 1) * 2) as u32, flags: GT_DEFAULT, codepage: 1200, ..Default::default() };
            let got = SendMessageW(self.re, EM_GETTEXTEX, WPARAM(&gt as *const _ as usize), LPARAM(buf.as_mut_ptr() as isize)).0.max(0) as usize;
            buf.truncate(got.min(n));
            buf
        }
    }

    pub fn dirty(&self) -> bool {
        unsafe { SendMessageW(self.re, EM_GETMODIFY, WPARAM(0), LPARAM(0)).0 != 0 }
    }

    pub fn save(&mut self, to: Option<&Path>) -> std::result::Result<PathBuf, String> {
        let path = to.map(|p| p.to_path_buf()).unwrap_or_else(|| self.path.clone());
        let text = self.text_for_save();
        let mut enc = self.enc;
        if enc == Enc::Cp1251 && textenc::decode(&textenc::encode(&text, enc)).0 != text {
            // в 1251 не все символы помещаются — без потерь только UTF-8
            enc = Enc::Utf8;
        }
        let bytes = textenc::encode(&text, enc);
        let tmp = path.with_extension("spv-tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            e.to_string()
        })?;
        self.enc = enc;
        unsafe {
            SendMessageW(self.re, EM_SETMODIFY, WPARAM(0), LPARAM(0));
        }
        if to.is_some() {
            self.path = path.clone();
            let ext = crate::util::ext_lower(&path);
            self.lang = highlight::lang_for(&ext, &crate::util::file_name(&path).to_lowercase());
            self.hl_due = true;
        }
        Ok(path)
    }

    pub fn focus(&self) {
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(self.re);
        }
    }

    /// Курсор сейчас в редакторе (режим правки), а не в окне программы.
    pub fn has_focus(&self) -> bool {
        unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() == self.re }
    }

    /// Прокрутка из режима просмотра (клавиши без курсора в тексте).
    pub fn scroll_key(&self, vk: u16) {
        let code = match vk {
            0x21 => SB_PAGEUP,
            0x22 => SB_PAGEDOWN,
            0x26 => SB_LINEUP,
            0x28 => SB_LINEDOWN,
            0x24 => SB_TOP,
            _ => SB_BOTTOM,
        };
        unsafe {
            for _ in 0..if matches!(vk, 0x26 | 0x28) { 3 } else { 1 } {
                SendMessageW(self.re, WM_VSCROLL, WPARAM(code.0 as usize), LPARAM(0));
            }
        }
    }

    /// Начать правку: курсор в текст; символ, нажатый в режиме просмотра, не теряется.
    pub fn start_edit(&self, owner: HWND) {
        self.focus();
        unsafe {
            let mut m = MSG::default();
            if PeekMessageW(&mut m, owner, WM_CHAR, WM_CHAR, PM_REMOVE).as_bool() {
                SendMessageW(self.re, WM_CHAR, m.wParam, m.lParam);
            }
        }
    }

    pub fn show(&self, on: bool) {
        unsafe {
            let _ = ShowWindow(self.re, if on { SW_SHOW } else { SW_HIDE });
            let _ = ShowWindow(self.find, if on && self.find_open { SW_SHOW } else { SW_HIDE });
            let _ = ShowWindow(self.repl, if on && self.find_open && self.replace_open { SW_SHOW } else { SW_HIDE });
        }
    }

    fn apply_wrap(&self) {
        unsafe {
            SendMessageW(self.re, EM_SETTARGETDEVICE, WPARAM(0), LPARAM(if self.wrap { 0 } else { 1 }));
        }
    }

    pub fn toggle_wrap(&mut self) {
        self.wrap = !self.wrap;
        crate::settings::set_i("text_wrap", self.wrap as i32);
        self.apply_wrap();
    }

    /// Тема: фон, цвет текста, полосы прокрутки, шрифт полей поиска.
    fn apply_theme(&mut self, t: &Theme, dpi: u32) {
        if self.theme_dark == Some(t.dark) && self.font_dpi == dpi {
            return;
        }
        self.theme_dark = Some(t.dark);
        unsafe {
            // оформление — не правка: ни «изменён», ни шаг отмены
            let modified = SendMessageW(self.re, EM_GETMODIFY, WPARAM(0), LPARAM(0)).0 != 0;
            if let Some(d) = &self.doc {
                let _ = d.Undo(tomSuspend.0);
            }
            SendMessageW(self.re, EM_SETBKGNDCOLOR, WPARAM(0), LPARAM(t.card.0 as isize));
            let mut cf = CHARFORMAT2W::default();
            cf.Base.cbSize = std::mem::size_of::<CHARFORMAT2W>() as u32;
            cf.Base.dwMask = CFM_FACE | CFM_SIZE | CFM_COLOR | CFM_CHARSET;
            cf.Base.yHeight = 200; // 10 пт
            cf.Base.crTextColor = t.text;
            cf.Base.bCharSet = DEFAULT_CHARSET;
            for (i, c) in "Consolas".encode_utf16().enumerate() {
                cf.Base.szFaceName[i] = c;
            }
            SendMessageW(self.re, EM_SETCHARFORMAT, WPARAM(SCF_ALL as usize), LPARAM(&cf as *const _ as isize));
            SendMessageW(self.re, EM_SETCHARFORMAT, WPARAM(SCF_DEFAULT as usize), LPARAM(&cf as *const _ as isize));
            // табуляция — 4 знака Consolas (около 0,55 em)
            let mut pf = PARAFORMAT::default();
            pf.cbSize = std::mem::size_of::<PARAFORMAT>() as u32;
            pf.dwMask = PFM_TABSTOPS;
            pf.cTabCount = 32;
            for i in 0..32 {
                pf.rgxTabs[i] = ((i as u32) + 1) * 440;
            }
            let mut sel = CHARRANGE::default();
            SendMessageW(self.re, EM_EXGETSEL, WPARAM(0), LPARAM(&mut sel as *mut _ as isize));
            let ev = SendMessageW(self.re, EM_SETEVENTMASK, WPARAM(0), LPARAM(0));
            SendMessageW(self.re, EM_SETSEL, WPARAM(0), LPARAM(-1));
            SendMessageW(self.re, EM_SETPARAFORMAT, WPARAM(0), LPARAM(&pf as *const _ as isize));
            SendMessageW(self.re, EM_EXSETSEL, WPARAM(0), LPARAM(&sel as *const _ as isize));
            SendMessageW(self.re, EM_SETEVENTMASK, WPARAM(0), LPARAM(ev.0));
            if let Some(d) = &self.doc {
                let _ = d.Undo(tomResume.0);
            }
            SendMessageW(self.re, EM_SETMODIFY, WPARAM(modified as usize), LPARAM(0));
            let theme = if t.dark { w!("DarkMode_Explorer") } else { w!("Explorer") };
            let _ = SetWindowTheme(self.re, theme, PCWSTR::null());
            if !self.brush.is_invalid() {
                let _ = DeleteObject(self.brush);
            }
            self.brush = CreateSolidBrush(t.card);
            if self.font_dpi != dpi {
                if !self.font.is_invalid() {
                    let _ = DeleteObject(self.font);
                }
                let face = wide("Segoe UI");
                self.font = CreateFontW(-(12 * dpi as i32 / 96), 0, 0, 0, 400, 0, 0, 0, DEFAULT_CHARSET.0 as u32, 0, 0, CLEARTYPE_QUALITY.0 as u32, 0, PCWSTR(face.as_ptr()));
                self.font_dpi = dpi;
                SendMessageW(self.find, WM_SETFONT, WPARAM(self.font.0 as usize), LPARAM(1));
                SendMessageW(self.repl, WM_SETFONT, WPARAM(self.font.0 as usize), LPARAM(1));
            }
        }
        self.hl_due = true;
    }

    /// Раскраска видимой части (±40 строк). Изменения цвета не попадают в отмену.
    pub fn rehighlight(&mut self, t: &Theme) {
        self.hl_due = false;
        let Some(doc) = self.doc.clone() else { return };
        let text = self.text_u16();
        if self.big || text.len() > HL_LIMIT {
            self.big = true;
            return;
        }
        unsafe {
            let first = SendMessageW(self.re, EM_GETFIRSTVISIBLELINE, WPARAM(0), LPARAM(0)).0 as i32;
            self.last_first_line = first;
            let mut rc = RECT::default();
            let _ = GetClientRect(self.re, &mut rc);
            let lines_visible = (rc.bottom / 12).max(20);
            let from_line = (first - 40).max(0);
            let to_line = first + lines_visible + 40;
            let a = SendMessageW(self.re, EM_LINEINDEX, WPARAM(from_line as usize), LPARAM(0)).0.max(0) as usize;
            let b = SendMessageW(self.re, EM_LINEINDEX, WPARAM(to_line as usize), LPARAM(0)).0;
            let b = if b < 0 { text.len() } else { (b as usize).min(text.len()) };
            // раскраска не должна делать файл «изменённым»
            let modified = SendMessageW(self.re, EM_GETMODIFY, WPARAM(0), LPARAM(0)).0 != 0;
            let _ = doc.Undo(tomSuspend.0);
            let _ = doc.Freeze();
            if let Ok(r) = doc.Range(a as i32, b as i32) {
                if let Ok(f) = r.GetFont() {
                    let _ = f.SetForeColor(colorref_i32(t.text));
                }
            }
            if self.lang != Lang::Plain {
                for s in highlight::highlight(&text, self.lang) {
                    let (s0, s1) = (s.start, s.start + s.len);
                    if s1 <= a || s0 >= b {
                        continue;
                    }
                    if let Ok(r) = doc.Range(s0.max(a) as i32, s1.min(b) as i32) {
                        if let Ok(f) = r.GetFont() {
                            let _ = f.SetForeColor(colorref_i32(tok_color(s.tok, t.dark)));
                        }
                    }
                }
            }
            let _ = doc.Unfreeze();
            let _ = doc.Undo(tomResume.0);
            SendMessageW(self.re, EM_SETMODIFY, WPARAM(modified as usize), LPARAM(0));
        }
    }

    /// Раз в ~200 мс: перекрасить, если текст менялся или прокрутили.
    pub fn tick(&mut self, t: &Theme) {
        let first = unsafe { SendMessageW(self.re, EM_GETFIRSTVISIBLELINE, WPARAM(0), LPARAM(0)).0 as i32 };
        if self.hl_due || first != self.last_first_line {
            self.rehighlight(t);
        }
    }

    pub fn mark_changed(&mut self) {
        self.hl_due = true;
    }

    /// Разместить редактор и поля поиска; вернуть кнопки панели поиска.
    pub fn layout(&mut self, ui: &Ui, rc: RECT) -> Vec<W> {
        self.apply_theme(&ui.t, ui.dpi);
        let mut v = vec![];
        let bar = if self.find_open { ui.s(if self.replace_open { 88 } else { 48 }) } else { 0 };
        unsafe {
            let _ = MoveWindow(self.re, rc.left, rc.top + bar, rc.right - rc.left, (rc.bottom - rc.top - bar).max(1), true);
        }
        if self.find_open {
            let m = ui.s(10);
            let bh = ui.s(30);
            let ew = ((rc.right - rc.left) / 3).clamp(ui.s(160), ui.s(360));
            let y = rc.top + ui.s(9);
            unsafe {
                let _ = MoveWindow(self.find, rc.left + m + ui.s(6), y + ui.s(6), ew - ui.s(12), bh - ui.s(10), true);
                let _ = ShowWindow(self.find, SW_SHOW);
                if self.replace_open {
                    let _ = MoveWindow(self.repl, rc.left + m + ui.s(6), y + bh + ui.s(10) + ui.s(6), ew - ui.s(12), bh - ui.s(10), true);
                    let _ = ShowWindow(self.repl, SW_SHOW);
                } else {
                    let _ = ShowWindow(self.repl, SW_HIDE);
                }
            }
            let mut x = rc.left + m + ew + ui.s(8);
            for (id, icon, tip) in [(ID_FIND_PREV, gui::IC_UP, "Найти назад (Shift+F3)"), (ID_FIND_NEXT, gui::IC_DOWN, "Найти далее (F3 или Enter)")] {
                v.push(W::new(Kind::Button, id, gui::r(x, y, ui.s(36), bh)).i(icon).tip(tip));
                x += ui.s(42);
            }
            let cw = ui.tw(ui.f.text, "Аа") + ui.s(20);
            v.push(W::new(Kind::Seg, ID_FIND_CASE, gui::r(x, y, cw, bh)).t("Аа").tip("Учитывать регистр").on(self.match_case));
            x += cw + ui.s(6);
            if !self.replace_open {
                let rw = ui.tw(ui.f.text, "Заменить…") + ui.s(24);
                v.push(W::new(Kind::Button, ID_REPLACE, gui::r(x, y, rw, bh)).t("Заменить…").tip("Показать замену (Ctrl+H)"));
            }
            v.push(W::new(Kind::Tool, ID_FIND_CLOSE, gui::r(rc.right - m - ui.s(32), y, ui.s(32), bh)).i(gui::IC_CLOSE).tip("Закрыть поиск (Esc)"));
            if self.replace_open {
                let y2 = y + bh + ui.s(10);
                let mut x2 = rc.left + m + ew + ui.s(8);
                for (id, t) in [(ID_REPLACE, "Заменить"), (ID_REPLACE_ALL, "Заменить все")] {
                    let w = ui.tw(ui.f.text, t) + ui.s(24);
                    v.push(W::new(Kind::Button, id, gui::r(x2, y2, w, bh)).t(t));
                    x2 += w + ui.s(6);
                }
            }
        } else {
            unsafe {
                let _ = ShowWindow(self.find, SW_HIDE);
                let _ = ShowWindow(self.repl, SW_HIDE);
            }
        }
        v
    }

    /// Фон панели поиска и рамки полей ввода.
    pub fn paint(&self, ui: &Ui, hdc: HDC, rc: RECT) {
        if !self.find_open {
            return;
        }
        let bar = ui.s(if self.replace_open { 88 } else { 48 });
        gui::fill(hdc, &RECT { bottom: rc.top + bar, ..rc }, ui.t.bg);
        gui::line(hdc, rc.left, rc.top + bar - 1, rc.right, rc.top + bar - 1, ui.t.border, 1, PS_SOLID);
        let m = ui.s(10);
        let ew = ((rc.right - rc.left) / 3).clamp(ui.s(160), ui.s(360));
        let bh = ui.s(30);
        let y = rc.top + ui.s(9);
        gui::round(hdc, &gui::r(rc.left + m, y, ew, bh), ui.s(8), Some(ui.t.card), Some(ui.t.border));
        if self.replace_open {
            gui::round(hdc, &gui::r(rc.left + m, y + bh + ui.s(10), ew, bh), ui.s(8), Some(ui.t.card), Some(ui.t.border));
        }
    }

    pub fn open_find(&mut self, replace: bool) {
        self.find_open = true;
        self.replace_open = replace || self.replace_open && self.find_open;
        // подставить выделенное слово
        unsafe {
            let mut sel = CHARRANGE::default();
            SendMessageW(self.re, EM_EXGETSEL, WPARAM(0), LPARAM(&mut sel as *mut _ as isize));
            if sel.cpMax > sel.cpMin && sel.cpMax - sel.cpMin < 200 {
                let mut buf = vec![0u16; (sel.cpMax - sel.cpMin + 1) as usize];
                SendMessageW(self.re, EM_GETSELTEXT, WPARAM(0), LPARAM(buf.as_mut_ptr() as isize));
                let s = crate::util::from_wide(&buf);
                if !s.contains('\r') {
                    let w = wide(&s);
                    let _ = SetWindowTextW(self.find, PCWSTR(w.as_ptr()));
                }
            }
            let _ = ShowWindow(self.find, SW_SHOW);
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(self.find);
            SendMessageW(self.find, EM_SETSEL, WPARAM(0), LPARAM(-1));
        }
    }

    pub fn close_find(&mut self) {
        self.find_open = false;
        self.replace_open = false;
        self.focus();
    }

    fn query(&self, h: HWND) -> String {
        unsafe {
            let n = GetWindowTextLengthW(h);
            let mut buf = vec![0u16; n as usize + 1];
            GetWindowTextW(h, &mut buf);
            crate::util::from_wide(&buf)
        }
    }

    /// Найти следующее/предыдущее вхождение. false — не найдено.
    pub fn find_next(&mut self, forward: bool) -> bool {
        let q = self.query(self.find);
        if q.is_empty() {
            return false;
        }
        let w = wide(&q);
        unsafe {
            let mut sel = CHARRANGE::default();
            SendMessageW(self.re, EM_EXGETSEL, WPARAM(0), LPARAM(&mut sel as *mut _ as isize));
            let mut flags = if self.match_case { FR_MATCHCASE.0 } else { 0 };
            if forward {
                flags |= FR_DOWN.0;
            }
            let search = |from: i32, to: i32| -> i32 {
                let mut ft = FINDTEXTEXW { chrg: CHARRANGE { cpMin: from, cpMax: to }, lpstrText: PCWSTR(w.as_ptr()), chrgText: CHARRANGE::default() };
                let r = SendMessageW(self.re, EM_FINDTEXTEXW, WPARAM(flags as usize), LPARAM(&mut ft as *mut _ as isize)).0 as i32;
                if r >= 0 {
                    SendMessageW(self.re, EM_EXSETSEL, WPARAM(0), LPARAM(&ft.chrgText as *const _ as isize));
                    SendMessageW(self.re, EM_SCROLLCARET, WPARAM(0), LPARAM(0));
                }
                r
            };
            let found = if forward { search(sel.cpMax, -1) } else { search(sel.cpMin, 0) };
            if found >= 0 {
                return true;
            }
            // по кругу
            let again = if forward { search(0, -1) } else { search(i32::MAX, 0) };
            again >= 0
        }
    }

    fn selection_matches(&self, q: &str) -> bool {
        unsafe {
            let mut sel = CHARRANGE::default();
            SendMessageW(self.re, EM_EXGETSEL, WPARAM(0), LPARAM(&mut sel as *mut _ as isize));
            if sel.cpMax - sel.cpMin != q.encode_utf16().count() as i32 {
                return false;
            }
            let mut buf = vec![0u16; (sel.cpMax - sel.cpMin + 1) as usize];
            SendMessageW(self.re, EM_GETSELTEXT, WPARAM(0), LPARAM(buf.as_mut_ptr() as isize));
            let s = crate::util::from_wide(&buf);
            if self.match_case { s == q } else { s.to_lowercase() == q.to_lowercase() }
        }
    }

    pub fn replace_one(&mut self) -> bool {
        let q = self.query(self.find);
        if q.is_empty() {
            return false;
        }
        if self.selection_matches(&q) {
            let r = wide(&self.query(self.repl));
            unsafe {
                SendMessageW(self.re, EM_REPLACESEL, WPARAM(1), LPARAM(r.as_ptr() as isize));
            }
        }
        self.find_next(true)
    }

    pub fn replace_all(&mut self) -> usize {
        let q = self.query(self.find);
        if q.is_empty() {
            return 0;
        }
        let r = wide(&self.query(self.repl));
        let w = wide(&q);
        let mut n = 0;
        unsafe {
            let flags = FR_DOWN.0 | if self.match_case { FR_MATCHCASE.0 } else { 0 };
            let mut from = 0;
            loop {
                let mut ft = FINDTEXTEXW { chrg: CHARRANGE { cpMin: from, cpMax: -1 }, lpstrText: PCWSTR(w.as_ptr()), chrgText: CHARRANGE::default() };
                if (SendMessageW(self.re, EM_FINDTEXTEXW, WPARAM(flags as usize), LPARAM(&mut ft as *mut _ as isize)).0 as i32) < 0 {
                    break;
                }
                SendMessageW(self.re, EM_EXSETSEL, WPARAM(0), LPARAM(&ft.chrgText as *const _ as isize));
                SendMessageW(self.re, EM_REPLACESEL, WPARAM(1), LPARAM(r.as_ptr() as isize));
                from = ft.chrgText.cpMin + (r.len() as i32 - 1);
                n += 1;
                if n > 1_000_000 {
                    break;
                }
            }
        }
        self.hl_due = true;
        n
    }

    pub fn undo(&self) {
        unsafe {
            SendMessageW(self.re, EM_UNDO, WPARAM(0), LPARAM(0));
        }
    }
    pub fn redo(&self) {
        unsafe {
            SendMessageW(self.re, EM_REDO, WPARAM(0), LPARAM(0));
        }
    }
    pub fn can_undo(&self) -> bool {
        unsafe { SendMessageW(self.re, EM_CANUNDO, WPARAM(0), LPARAM(0)).0 != 0 }
    }

    pub fn zoom(&self, delta: i32) {
        unsafe {
            let (mut num, mut den) = (0i32, 0i32);
            SendMessageW(self.re, EM_GETZOOM, WPARAM(&mut num as *mut _ as usize), LPARAM(&mut den as *mut _ as isize));
            let cur = if num == 0 || den == 0 { 100 } else { num * 100 / den };
            let next = if delta == 0 { 100 } else { (cur + delta * 10).clamp(30, 500) };
            SendMessageW(self.re, EM_SETZOOM, WPARAM(next as usize), LPARAM(100));
        }
    }

    pub fn zoom_pct(&self) -> i32 {
        unsafe {
            let (mut num, mut den) = (0i32, 0i32);
            SendMessageW(self.re, EM_GETZOOM, WPARAM(&mut num as *mut _ as usize), LPARAM(&mut den as *mut _ as isize));
            if num == 0 || den == 0 { 100 } else { num * 100 / den }
        }
    }

    /// Строка/столбец курсора, всего строк.
    pub fn info(&self) -> String {
        unsafe {
            let mut sel = CHARRANGE::default();
            SendMessageW(self.re, EM_EXGETSEL, WPARAM(0), LPARAM(&mut sel as *mut _ as isize));
            let line = SendMessageW(self.re, EM_EXLINEFROMCHAR, WPARAM(0), LPARAM(sel.cpMax as isize)).0;
            let start = SendMessageW(self.re, EM_LINEINDEX, WPARAM(line as usize), LPARAM(0)).0;
            let total = SendMessageW(self.re, EM_GETLINECOUNT, WPARAM(0), LPARAM(0)).0;
            let mut v = vec![
                format!("Стр {}, стлб {}", line + 1, sel.cpMax as isize - start + 1),
                format!("{total} строк"),
                format!("{} · {}", self.enc.label(), self.eol.label()),
            ];
            if self.zoom_pct() != 100 {
                v.push(format!("{}%", self.zoom_pct()));
            }
            if self.big {
                v.push("без подсветки".into());
            }
            v.join("   ·   ")
        }
    }

    /// Цвета полей поиска (WM_CTLCOLOREDIT).
    pub fn ctl_color(&self, t: &Theme, hdc: HDC) -> LRESULT {
        unsafe {
            SetTextColor(hdc, t.text);
            SetBkColor(hdc, t.card);
        }
        LRESULT(self.brush.0 as isize)
    }

    pub fn is_find_box(&self, h: HWND) -> bool {
        h == self.find || h == self.repl
    }
}
