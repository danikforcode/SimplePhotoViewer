// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Таблицы: XLSX/XLSM/XLSB/XLS/ODS (calamine) и CSV/TSV (свой разбор).
//! Только просмотр: сортировка по столбцу меняет порядок на экране, файл не трогаем.

use crate::gui::{self, Kind, Mouse, Ui, W};
use crate::util::rgb;
use std::path::Path;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::*;

// ------------------------------------------------------------- данные ---
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Empty,
    Num(f64),
    /// Число из CSV: храним и исходную запись, чтобы показать как в файле.
    NumS(f64, String),
    Text(String),
    Bool(bool),
    Date(f64),
    Err(String),
}

impl Cell {
    pub fn text(&self) -> String {
        match self {
            Cell::Empty => String::new(),
            Cell::Num(v) => fmt_num(*v),
            Cell::NumS(_, s) | Cell::Text(s) | Cell::Err(s) => s.clone(),
            Cell::Bool(b) => if *b { "ИСТИНА".into() } else { "ЛОЖЬ".into() },
            Cell::Date(v) => fmt_date(*v),
        }
    }
    fn num(&self) -> Option<f64> {
        match self {
            Cell::Num(v) | Cell::NumS(v, _) | Cell::Date(v) => Some(*v),
            Cell::Bool(b) => Some(*b as u8 as f64),
            _ => None,
        }
    }
    pub fn is_numeric(&self) -> bool {
        self.num().is_some()
    }
}

pub fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{:.10}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s.len() > 16 { format!("{v:.6e}") } else { s }
}

/// Excel-дата (дни с 30.12.1899) → «ГГГГ-ММ-ДД[ ЧЧ:ММ]».
pub fn fmt_date(v: f64) -> String {
    let days = v.floor() as i64;
    let z = days - 25569 + 719468; // в дни от 01.03.0000 (алгоритм Хиннанта)
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let secs = ((v - v.floor()) * 86400.0).round() as i64;
    if secs == 0 {
        format!("{y:04}-{m:02}-{d:02}")
    } else {
        format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", secs / 3600, secs / 60 % 60)
    }
}

pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<Cell>>,
    pub cols: usize,
}

fn csv_split(text: &str, delim: char) -> Vec<Vec<String>> {
    let mut rows = vec![];
    let mut row = vec![];
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    let mut at_start = true;
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    cell.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                cell.push(c);
            }
            continue;
        }
        match c {
            '"' if at_start => {
                quoted = true;
                at_start = false;
            }
            c if c == delim => {
                row.push(std::mem::take(&mut cell));
                at_start = true;
            }
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
                at_start = true;
            }
            c => {
                cell.push(c);
                at_start = false;
            }
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

/// Разделитель CSV: тот, что встречается одинаково часто в первых строках.
pub fn detect_delim(text: &str) -> char {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).take(20).collect();
    let mut best = (',', 0usize);
    for d in [';', ',', '\t', '|'] {
        let counts: Vec<usize> = lines.iter().map(|l| l.matches(d).count()).collect();
        let min = counts.iter().copied().min().unwrap_or(0);
        if min > best.1 {
            best = (d, min);
        }
    }
    best.0
}

pub fn parse_csv(text: &str, delim: Option<char>) -> Sheet {
    let d = delim.unwrap_or_else(|| detect_delim(text));
    let mut rows: Vec<Vec<Cell>> = csv_split(text, d)
        .into_iter()
        .map(|r| {
            r.into_iter()
                .map(|s| {
                    let t = s.trim();
                    if t.is_empty() {
                        return Cell::Empty;
                    }
                    // «1 234,5» и «1234.5» — числа; при разделителе «;» запятая — десятичная
                    let norm: String = t.chars().filter(|c| !matches!(c, ' ' | '\u{a0}')).collect();
                    let norm = if d != ',' { norm.replacen(',', ".", 1) } else { norm };
                    match norm.parse::<f64>() {
                        Ok(v) if v.is_finite() && !t.starts_with('+') && !(t.len() > 1 && t.starts_with('0') && !t.starts_with("0.") && !t.starts_with("0,")) => Cell::NumS(v, s),
                        _ => Cell::Text(s),
                    }
                })
                .collect()
        })
        .collect();
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    for r in rows.iter_mut() {
        r.resize(cols, Cell::Empty);
    }
    Sheet { name: "Лист1".into(), rows, cols }
}

pub fn load(path: &Path) -> Result<Vec<Sheet>, String> {
    let ext = crate::util::ext_lower(path);
    if ext == "csv" || ext == "tsv" {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let text = crate::textenc::decode(&bytes).0;
        let mut s = parse_csv(&text, if ext == "tsv" { Some('\t') } else { None });
        s.name = crate::util::file_name(path);
        return Ok(vec![s]);
    }
    use calamine::{open_workbook_auto, Data, Reader};
    let mut wb = open_workbook_auto(path).map_err(|e| e.to_string())?;
    let mut out = vec![];
    for name in wb.sheet_names().to_owned() {
        let Ok(range) = wb.worksheet_range(&name) else { continue };
        // calamine отдаёт диапазон от первой заполненной ячейки — восстановим отступ сверху/слева
        let (r0, c0) = range.start().map(|(r, c)| (r as usize, c as usize)).unwrap_or((0, 0));
        let mut rows: Vec<Vec<Cell>> = vec![vec![]; r0];
        for row in range.rows() {
            let mut v: Vec<Cell> = vec![Cell::Empty; c0];
            for c in row {
                v.push(match c {
                    Data::Empty => Cell::Empty,
                    Data::Int(i) => Cell::Num(*i as f64),
                    Data::Float(f) => Cell::Num(*f),
                    Data::String(s) => Cell::Text(s.clone()),
                    Data::Bool(b) => Cell::Bool(*b),
                    Data::DateTime(d) => Cell::Date(d.as_f64()),
                    Data::DateTimeIso(s) | Data::DurationIso(s) => Cell::Text(s.clone()),
                    Data::Error(e) => Cell::Err(format!("{e}")),
                });
            }
            rows.push(v);
        }
        let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        for r in rows.iter_mut() {
            r.resize(cols, Cell::Empty);
        }
        out.push(Sheet { name, rows, cols });
    }
    if out.is_empty() {
        return Err("в книге нет листов".into());
    }
    Ok(out)
}

/// Порядок строк при сортировке по столбцу. Пустые — всегда в конце,
/// числа и даты — раньше текста, текст — без учёта регистра.
pub fn sorted_order(rows: &[Vec<Cell>], skip: usize, col: usize, asc: bool) -> Vec<usize> {
    let mut idx: Vec<usize> = (skip..rows.len()).collect();
    let key = |i: usize| -> (u8, f64, String) {
        match rows[i].get(col).unwrap_or(&Cell::Empty) {
            Cell::Empty => (3, 0.0, String::new()),
            c => match c.num() {
                Some(v) => (0, v, String::new()),
                None => (if matches!(c, Cell::Err(_)) { 2 } else { 1 }, 0.0, c.text().to_lowercase()),
            },
        }
    };
    let keys: Vec<(u8, f64, String)> = (0..rows.len()).map(key).collect();
    idx.sort_by(|&a, &b| {
        let (ka, kb) = (&keys[a], &keys[b]);
        if ka.0 == 3 || kb.0 == 3 {
            return ka.0.cmp(&kb.0); // пустые в конце независимо от направления
        }
        let o = ka.0.cmp(&kb.0).then(ka.1.total_cmp(&kb.1)).then_with(|| ka.2.cmp(&kb.2));
        if asc { o } else { o.reverse() }
    });
    let mut v: Vec<usize> = (0..skip).collect();
    v.extend(idx);
    v
}

pub fn col_name(mut c: usize) -> String {
    let mut s = String::new();
    loop {
        s.insert(0, (b'A' + (c % 26) as u8) as char);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    s
}

// -------------------------------------------------------------- вид ---
pub const ID_SHEET0: u32 = 900;

pub struct TableView {
    pub sheets: Vec<Sheet>,
    pub sheet: usize,
    /// Первая строка — заголовок (закреплена и не сортируется).
    pub header: bool,
    pub sort: Option<(usize, bool)>,
    order: Vec<usize>,
    col_w: Vec<i32>,
    pub zoom: f64,
    sx: f64,
    sy: f64,
    pub sel: Option<(usize, usize)>, // (строка на экране, столбец)
    drag: Option<(i32, i32, f64, f64, bool)>,
    resize: Option<(usize, i32, i32)>,
    rect: RECT,
    measured_for: Option<(usize, u32)>,
}

impl TableView {
    pub fn new(sheets: Vec<Sheet>) -> TableView {
        let mut t = TableView {
            sheets, sheet: 0, header: false, sort: None, order: vec![], col_w: vec![], zoom: 1.0, sx: 0.0, sy: 0.0,
            sel: None, drag: None, resize: None, rect: RECT::default(), measured_for: None,
        };
        t.select_sheet(0);
        t
    }

    fn sh(&self) -> &Sheet {
        &self.sheets[self.sheet]
    }

    /// Первая строка похожа на заголовок: вся текстовая, а ниже есть числа.
    fn guess_header(s: &Sheet) -> bool {
        let Some(first) = s.rows.first() else { return false };
        let filled: Vec<&Cell> = first.iter().filter(|c| **c != Cell::Empty).collect();
        if filled.is_empty() || filled.iter().any(|c| c.is_numeric()) {
            return false;
        }
        s.rows.iter().skip(1).take(50).any(|r| r.iter().any(|c| c.is_numeric())) || s.rows.len() > 1 && filled.len() == s.cols
    }

    pub fn select_sheet(&mut self, i: usize) {
        self.sheet = i.min(self.sheets.len() - 1);
        self.header = Self::guess_header(self.sh());
        self.sort = None;
        self.sx = 0.0;
        self.sy = 0.0;
        self.sel = None;
        self.measured_for = None;
        self.resort();
    }

    fn skip(&self) -> usize {
        if self.header { 1.min(self.sh().rows.len()) } else { 0 }
    }

    pub fn resort(&mut self) {
        let n = self.sh().rows.len();
        self.order = match self.sort {
            Some((c, asc)) => sorted_order(&self.sh().rows, self.skip(), c, asc),
            None => (0..n).collect(),
        };
    }

    pub fn toggle_header(&mut self) {
        self.header = !self.header;
        self.resort();
    }

    pub fn reset_sort(&mut self) {
        self.sort = None;
        self.resort();
    }

    /// Щелчок по заголовку столбца: ↑ → ↓ → без сортировки.
    pub fn click_sort(&mut self, c: usize) {
        self.sort = match self.sort {
            Some((cc, true)) if cc == c => Some((c, false)),
            Some((cc, false)) if cc == c => None,
            _ => Some((c, true)),
        };
        self.resort();
    }

    fn measure(&mut self, ui: &Ui) {
        let key = (self.sheet, ui.dpi);
        if self.measured_for == Some(key) {
            return;
        }
        self.measured_for = Some(key);
        let s = &self.sheets[self.sheet];
        let mut w = vec![ui.s(56); s.cols];
        for row in s.rows.iter().take(300) {
            for (c, cell) in row.iter().enumerate() {
                let t = cell.text();
                if !t.is_empty() {
                    let tw = ui.tw(ui.f.text, &t.chars().take(80).collect::<String>()) + ui.s(20);
                    w[c] = w[c].max(tw);
                }
            }
        }
        // ширины храним «при 100%» — масштабирование отдельно
        self.col_w = w.into_iter().map(|x| (x.min(ui.s(420)) * 96 / ui.dpi as i32).max(40)).collect();
    }

    fn row_h(&self, ui: &Ui) -> f64 {
        ui.s(26) as f64 * self.zoom
    }
    fn hdr_h(&self, ui: &Ui) -> f64 {
        ui.s(26) as f64 * self.zoom
    }
    fn num_w(&self, ui: &Ui) -> f64 {
        let digits = self.sh().rows.len().max(1).to_string().len() as i32;
        ui.s(22 + 8 * digits) as f64 * self.zoom
    }
    fn cw(&self, ui: &Ui, c: usize) -> f64 {
        ui.s(*self.col_w.get(c).unwrap_or(&80)) as f64 * self.zoom
    }
    fn col_x(&self, ui: &Ui, c: usize) -> f64 {
        (0..c).map(|i| self.cw(ui, i)).sum()
    }
    fn total_w(&self, ui: &Ui) -> f64 {
        self.col_x(ui, self.sh().cols)
    }

    /// Высота, отведённая под ярлыки листов внизу.
    fn tabs_h(&self, ui: &Ui) -> i32 {
        if self.sheets.len() > 1 { ui.s(40) } else { 0 }
    }

    fn clamp_scroll(&mut self, ui: &Ui) {
        let rc = self.rect;
        let view_w = (rc.right - rc.left) as f64 - self.num_w(ui);
        let frozen = if self.header { 1.0 } else { 0.0 };
        let view_h = (rc.bottom - rc.top - self.tabs_h(ui)) as f64 - self.hdr_h(ui) - frozen * self.row_h(ui);
        let rows = (self.order.len() as f64 - frozen).max(0.0);
        self.sx = self.sx.clamp(0.0, (self.total_w(ui) - view_w + ui.s(8) as f64).max(0.0));
        self.sy = self.sy.clamp(0.0, (rows * self.row_h(ui) - view_h + ui.s(8) as f64).max(0.0));
    }

    /// Элементы поверх таблицы: ярлыки листов.
    pub fn widgets(&mut self, ui: &Ui, rc: RECT) -> Vec<W> {
        self.rect = rc;
        self.measure(ui);
        self.clamp_scroll(ui);
        let mut v = vec![];
        if self.sheets.len() > 1 {
            let y = rc.bottom - ui.s(36);
            let mut x = rc.left + ui.s(8);
            for (i, s) in self.sheets.iter().enumerate() {
                let w = (ui.tw(ui.f.text, &s.name) + ui.s(24)).min(ui.s(200));
                if x + w > rc.right - ui.s(8) {
                    break;
                }
                v.push(W::new(Kind::Seg, ID_SHEET0 + i as u32, gui::r(x, y, w, ui.s(30))).t(s.name.clone()).on(i == self.sheet));
                x += w + ui.s(4);
            }
        }
        v
    }

    pub fn paint(&mut self, ui: &Ui, hdc: HDC) {
        let t = &ui.t;
        let rc = RECT { bottom: self.rect.bottom - self.tabs_h(ui), ..self.rect };
        gui::fill(hdc, &self.rect, t.canvas);
        gui::fill(hdc, &rc, t.card);
        let s = &self.sheets[self.sheet];
        let (rh, hh, nw) = (self.row_h(ui), self.hdr_h(ui), self.num_w(ui));
        let fsz = (ui.s(12) as f64 * self.zoom).round() as i32;
        let face = crate::util::wide("Segoe UI");
        let mk = |weight: i32| unsafe {
            CreateFontW(-fsz.max(6), 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET.0 as u32, 0, 0, CLEARTYPE_QUALITY.0 as u32, 0, windows::core::PCWSTR(face.as_ptr()))
        };
        let (font, bold) = (mk(400), mk(600));
        let grid = gui::blend(t.border, t.card, 0.7);
        let head_bg = gui::blend(t.bg, t.card, 0.6);
        let frozen = if self.header && !s.rows.is_empty() { 1 } else { 0 };
        let top = rc.top as f64 + hh + frozen as f64 * rh;
        unsafe {
            let _ = IntersectClipRect(hdc, rc.left, rc.top, rc.right, rc.bottom);
        }
        // видимые столбцы
        let x0 = rc.left as f64 + nw - self.sx;
        let mut cols = vec![];
        let mut x = x0;
        for c in 0..s.cols {
            let w = self.cw(ui, c);
            if x + w >= rc.left as f64 + nw && x <= rc.right as f64 {
                cols.push((c, x, w));
            }
            x += w;
            if x > rc.right as f64 {
                break;
            }
        }
        let draw_row = |hdc: HDC, ri: usize, y: f64, b: bool, bg: Option<windows::Win32::Foundation::COLORREF>| {
            let row = &s.rows[ri];
            if let Some(bg) = bg {
                gui::fill(hdc, &RECT { left: rc.left + nw as i32, top: y as i32, right: rc.right, bottom: (y + rh) as i32 }, bg);
            }
            for &(c, x, w) in &cols {
                let cell = &row[c];
                let txt = cell.text();
                if !txt.is_empty() {
                    let r = RECT { left: (x + ui.s(6) as f64) as i32, top: y as i32, right: (x + w - ui.s(6) as f64) as i32, bottom: (y + rh) as i32 };
                    let color = match cell {
                        Cell::Err(_) => rgb(200, 60, 60),
                        _ => t.text,
                    };
                    let fl = if cell.is_numeric() && !b { gui::RIGHT } else { gui::LEFT };
                    gui::text(hdc, &txt.replace(['\n', '\r'], " "), &r, if b { bold } else { font }, color, fl);
                }
                gui::line(hdc, (x + w) as i32 - 1, y as i32, (x + w) as i32 - 1, (y + rh) as i32, grid, 1, PS_SOLID);
            }
            gui::line(hdc, rc.left, (y + rh) as i32 - 1, rc.right, (y + rh) as i32 - 1, grid, 1, PS_SOLID);
        };
        // строки данных
        let first = ((self.sy / rh).floor() as usize).min(self.order.len());
        let mut y = top - (self.sy - first as f64 * rh);
        let mut k = first + frozen;
        while k < self.order.len() && y < rc.bottom as f64 {
            let ri = self.order[k];
            let selected = self.sel.map(|(r, _)| r == k).unwrap_or(false);
            draw_row(hdc, ri, y, false, if selected { Some(gui::blend(t.sel, t.card, 0.6)) } else { None });
            if let Some((r, c)) = self.sel {
                if r == k {
                    if let Some(&(_, cx, cw)) = cols.iter().find(|(cc, _, _)| *cc == c) {
                        let rr = RECT { left: cx as i32, top: y as i32, right: (cx + cw) as i32, bottom: (y + rh) as i32 };
                        gui::frame(hdc, &rr, t.accent, PS_SOLID);
                        gui::frame(hdc, &RECT { left: rr.left + 1, top: rr.top + 1, right: rr.right - 1, bottom: rr.bottom - 1 }, t.accent, PS_SOLID);
                    }
                }
            }
            // номер строки
            let nr = RECT { left: rc.left, top: y as i32, right: rc.left + nw as i32, bottom: (y + rh) as i32 };
            gui::fill(hdc, &nr, head_bg);
            gui::text(hdc, &(ri + 1).to_string(), &RECT { right: nr.right - ui.s(6), ..nr }, font, t.dim, gui::RIGHT);
            y += rh;
            k += 1;
        }
        // закреплённая строка-заголовок
        if frozen == 1 {
            let y = rc.top as f64 + hh;
            draw_row(hdc, self.order[0], y, true, Some(head_bg));
            let nr = RECT { left: rc.left, top: y as i32, right: rc.left + nw as i32, bottom: (y + rh) as i32 };
            gui::fill(hdc, &nr, head_bg);
            gui::text(hdc, "1", &RECT { right: nr.right - ui.s(6), ..nr }, font, t.dim, gui::RIGHT);
            gui::line(hdc, rc.left, (y + rh) as i32, rc.right, (y + rh) as i32, t.border, 2, PS_SOLID);
        }
        // буквы столбцов + стрелка сортировки
        gui::fill(hdc, &RECT { left: rc.left, top: rc.top, right: rc.right, bottom: rc.top + hh as i32 }, head_bg);
        for &(c, x, w) in &cols {
            let r = RECT { left: x as i32, top: rc.top, right: (x + w) as i32, bottom: rc.top + hh as i32 };
            let mut label = col_name(c);
            let mut color = t.dim;
            if let Some((sc, asc)) = self.sort {
                if sc == c {
                    label = format!("{label} {}", if asc { "▲" } else { "▼" });
                    color = t.accent;
                }
            }
            gui::text(hdc, &label, &r, bold, color, gui::CENTER);
            gui::line(hdc, r.right - 1, r.top + ui.s(4), r.right - 1, r.bottom - ui.s(4), t.border, 1, PS_SOLID);
        }
        gui::line(hdc, rc.left, rc.top + hh as i32 - 1, rc.right, rc.top + hh as i32 - 1, t.border, 1, PS_SOLID);
        gui::fill(hdc, &RECT { left: rc.left, top: rc.top, right: rc.left + nw as i32, bottom: rc.top + hh as i32 }, head_bg);
        gui::line(hdc, rc.left + nw as i32 - 1, rc.top, rc.left + nw as i32 - 1, rc.bottom, t.border, 1, PS_SOLID);
        if s.rows.is_empty() {
            gui::text(hdc, "Лист пуст", &rc, ui.f.text, t.dim, gui::CENTER);
        }
        unsafe {
            let _ = SelectClipRgn(hdc, None);
            let _ = DeleteObject(font);
            let _ = DeleteObject(bold);
        }
    }

    /// Попадание: (область, строка на экране, столбец). Область: 0 — ячейки, 1 — буквы, 2 — закреплённая строка.
    fn hit(&self, ui: &Ui, x: i32, y: i32) -> Option<(u8, usize, usize)> {
        let rc = RECT { bottom: self.rect.bottom - self.tabs_h(ui), ..self.rect };
        if !gui::inside(&rc, x, y) {
            return None;
        }
        let (rh, hh, nw) = (self.row_h(ui), self.hdr_h(ui), self.num_w(ui));
        let fx = x as f64 - rc.left as f64 - nw + self.sx;
        if fx < 0.0 {
            return None;
        }
        let mut acc = 0.0;
        let mut col = None;
        for c in 0..self.sh().cols {
            acc += self.cw(ui, c);
            if fx < acc {
                col = Some(c);
                break;
            }
        }
        let col = col?;
        let fy = y as f64 - rc.top as f64;
        if fy < hh {
            return Some((1, 0, col));
        }
        let frozen = if self.header && !self.sh().rows.is_empty() { 1 } else { 0 };
        if frozen == 1 && fy < hh + rh {
            return Some((2, 0, col));
        }
        let k = ((fy - hh - frozen as f64 * rh + self.sy) / rh).floor() as usize + frozen;
        (k < self.order.len()).then_some((0, k, col))
    }

    /// Граница столбца под курсором в строке букв (для изменения ширины).
    fn border_at(&self, ui: &Ui, x: i32, y: i32) -> Option<usize> {
        let rc = self.rect;
        if y < rc.top || y > rc.top + self.hdr_h(ui) as i32 {
            return None;
        }
        let mut edge = rc.left as f64 + self.num_w(ui) - self.sx;
        for c in 0..self.sh().cols {
            edge += self.cw(ui, c);
            if (x as f64 - edge).abs() <= ui.s(4) as f64 {
                return Some(c);
            }
        }
        None
    }

    pub fn cursor(&self, ui: &Ui, x: i32, y: i32) -> Option<windows::core::PCWSTR> {
        if self.resize.is_some() || self.border_at(ui, x, y).is_some() {
            return Some(windows::Win32::UI::WindowsAndMessaging::IDC_SIZEWE);
        }
        None
    }

    pub fn mouse(&mut self, ui: &Ui, m: Mouse, x: i32, y: i32, ctrl: bool, shift: bool) -> bool {
        match m {
            Mouse::Down => {
                if let Some(c) = self.border_at(ui, x, y) {
                    self.resize = Some((c, x, self.col_w[c]));
                    return true;
                }
                if gui::inside(&self.rect, x, y) {
                    self.drag = Some((x, y, self.sx, self.sy, false));
                }
                false
            }
            Mouse::Move => {
                if let Some((c, x0, w0)) = self.resize {
                    let d = ((x - x0) as f64 / self.zoom) as i32 * 96 / ui.dpi as i32;
                    self.col_w[c] = (w0 + d).clamp(24, 2000);
                    return true;
                }
                if let Some((x0, y0, sx, sy, moved)) = self.drag {
                    let (dx, dy) = (x - x0, y - y0);
                    if moved || dx.abs() + dy.abs() > ui.s(4) {
                        self.sx = sx - dx as f64;
                        self.sy = sy - dy as f64;
                        self.clamp_scroll(ui);
                        self.drag = Some((x0, y0, sx, sy, true));
                        return true;
                    }
                }
                false
            }
            Mouse::Up => {
                if self.resize.take().is_some() {
                    return true;
                }
                let click = matches!(self.drag.take(), Some((_, _, _, _, false)));
                if click {
                    match self.hit(ui, x, y) {
                        Some((1, _, c)) | Some((2, _, c)) => self.click_sort(c),
                        Some((0, r, c)) => self.sel = Some((r, c)),
                        _ => {}
                    }
                    return true;
                }
                false
            }
            Mouse::Dbl => {
                if let Some(c) = self.border_at(ui, x, y) {
                    // двойной щелчок по границе — ширина по содержимому
                    self.measured_for = None;
                    let keep: Vec<i32> = self.col_w.clone();
                    self.measure(ui);
                    let auto = self.col_w[c];
                    self.col_w = keep;
                    self.col_w[c] = auto;
                    return true;
                }
                false
            }
            Mouse::Wheel(d) => {
                if !gui::inside(&self.rect, x, y) {
                    return false;
                }
                if ctrl {
                    self.zoom = (self.zoom * if d > 0 { 1.1 } else { 1.0 / 1.1 }).clamp(0.5, 3.0);
                } else if shift {
                    self.sx -= d as f64 / 120.0 * self.cw(ui, 0) * 1.5;
                } else {
                    self.sy -= d as f64 / 120.0 * self.row_h(ui) * 3.0;
                }
                self.clamp_scroll(ui);
                true
            }
            _ => false,
        }
    }

    /// Клавиши: стрелки, Page Up/Down, Home/End. true — обработано.
    pub fn key(&mut self, ui: &Ui, vk: u16, ctrl: bool) -> bool {
        let n = self.order.len();
        let cols = self.sh().cols;
        if n == 0 || cols == 0 {
            return false;
        }
        let frozen = if self.header { 1 } else { 0 };
        let (mut r, mut c) = self.sel.unwrap_or((frozen.min(n - 1), 0));
        let page = (((self.rect.bottom - self.rect.top) as f64 / self.row_h(ui)) as usize).saturating_sub(3).max(1);
        match vk {
            0x26 => r = r.saturating_sub(1).max(frozen.min(n - 1)),
            0x28 => r = (r + 1).min(n - 1),
            0x25 => c = c.saturating_sub(1),
            0x27 => c = (c + 1).min(cols - 1),
            0x21 => r = r.saturating_sub(page).max(frozen.min(n - 1)),
            0x22 => r = (r + page).min(n - 1),
            0x24 => {
                if ctrl {
                    r = frozen.min(n - 1)
                }
                c = 0
            }
            0x23 => {
                if ctrl {
                    r = n - 1
                }
                c = cols - 1
            }
            _ => return false,
        }
        self.sel = Some((r, c));
        // прокрутить к выделению
        let rh = self.row_h(ui);
        let view_h = (self.rect.bottom - self.rect.top - self.tabs_h(ui)) as f64 - self.hdr_h(ui) - frozen as f64 * rh;
        let ry = (r - frozen.min(r)) as f64 * rh;
        if ry < self.sy {
            self.sy = ry;
        } else if ry + rh > self.sy + view_h {
            self.sy = ry + rh - view_h;
        }
        let cx = self.col_x(ui, c);
        let view_w = (self.rect.right - self.rect.left) as f64 - self.num_w(ui);
        if cx < self.sx {
            self.sx = cx;
        } else if cx + self.cw(ui, c) > self.sx + view_w {
            self.sx = cx + self.cw(ui, c) - view_w;
        }
        self.clamp_scroll(ui);
        true
    }

    /// Текст выделенной ячейки (или всей строки, если row=true) для буфера обмена.
    pub fn selection_text(&self, row: bool) -> Option<String> {
        let (r, c) = self.sel?;
        let cells = &self.sh().rows[*self.order.get(r)?];
        Some(if row { cells.iter().map(|x| x.text()).collect::<Vec<_>>().join("\t") } else { cells.get(c)?.text() })
    }

    pub fn info(&self) -> String {
        let s = self.sh();
        let mut v = vec![format!("{} × {}", s.rows.len(), s.cols)];
        if self.sheets.len() > 1 {
            v.push(format!("лист {} / {}", self.sheet + 1, self.sheets.len()));
        }
        if let Some((c, asc)) = self.sort {
            v.push(format!("сортировка: {} {}", col_name(c), if asc { "▲" } else { "▼" }));
        }
        if let Some((r, c)) = self.sel {
            if let Some(&ri) = self.order.get(r) {
                v.push(format!("{}{}", col_name(c), ri + 1));
            }
        }
        v.join("   ·   ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_parse() {
        let s = parse_csv("Имя;Сумма;Дата\r\n\"Иванов; И.\";1 234,5;2024\r\nПётр;\"7\";\n", None);
        assert_eq!(s.cols, 3);
        assert_eq!(s.rows.len(), 3);
        assert_eq!(s.rows[1][0], Cell::Text("Иванов; И.".into()));
        assert_eq!(s.rows[1][1], Cell::NumS(1234.5, "1 234,5".into()));
        assert_eq!(s.rows[2][2], Cell::Empty);
        let s = parse_csv("a,b\n\"x \"\"q\"\"\",007\n", None);
        assert_eq!(s.rows[1][0], Cell::Text("x \"q\"".into()));
        assert_eq!(s.rows[1][1], Cell::Text("007".into()), "ведущие нули — это не число");
        assert_eq!(detect_delim("a\tb\tc\n1\t2\t3"), '\t');
    }

    #[test]
    fn sorting() {
        let rows = vec![
            vec![Cell::Text("h".into())],
            vec![Cell::Num(10.0)],
            vec![Cell::Empty],
            vec![Cell::Text("б".into())],
            vec![Cell::Num(2.0)],
            vec![Cell::Text("А".into())],
        ];
        assert_eq!(sorted_order(&rows, 1, 0, true), vec![0, 4, 1, 5, 3, 2]);
        assert_eq!(sorted_order(&rows, 1, 0, false), vec![0, 3, 5, 1, 4, 2], "пустые — всё равно в конце");
        assert_eq!(sorted_order(&rows, 0, 0, true)[..2], [4, 1]);
    }

    #[test]
    fn formats() {
        assert_eq!(fmt_num(3.0), "3");
        assert_eq!(fmt_num(0.1 + 0.2), "0.3");
        assert_eq!(fmt_date(45292.0), "2024-01-01");
        assert_eq!(fmt_date(45292.5), "2024-01-01 12:00");
        assert_eq!(col_name(0), "A");
        assert_eq!(col_name(25), "Z");
        assert_eq!(col_name(26), "AA");
        assert_eq!(col_name(701), "ZZ");
        assert_eq!(col_name(702), "AAA");
    }
}
