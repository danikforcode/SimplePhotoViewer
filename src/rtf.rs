// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Документы → RTF. RTF затем раскладывает по страницам системный RichEdit
//! (тот же движок, что в WordPad): шрифты, абзацы, таблицы, картинки — без
//! собственного «движка вёрстки» в exe.
//!
//! Поддержано: DOCX (Word), ODT (LibreOffice/OpenOffice), Markdown, RTF как есть.

use crate::xmltree::{self, Node};
use std::collections::HashMap;
use std::io::Read;

/// Лист и поля в твипах (1/1440 дюйма).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageSetup {
    pub w: i32,
    pub h: i32,
    pub margin: (i32, i32, i32, i32), // слева, сверху, справа, снизу
}

impl Default for PageSetup {
    fn default() -> Self {
        // A4, поля 2 см
        PageSetup { w: 11906, h: 16838, margin: (1134, 1134, 1134, 1134) }
    }
}

// ------------------------------------------------------------ сборщик RTF ---
/// Свойства текста.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct Ch {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub size: Option<u32>, // полупункты
    pub color: Option<(u8, u8, u8)>,
    pub back: Option<(u8, u8, u8)>,
    pub font: Option<String>,
    pub sup: bool,
    pub sub: bool,
}

/// Свойства абзаца.
#[derive(Clone, Default, Debug)]
pub struct Para {
    pub align: u8, // 0 слева, 1 центр, 2 справа, 3 по ширине
    pub left: i32,
    pub first: i32,
    pub before: i32,
    pub after: i32,
    pub in_table: bool,
    pub shade: Option<(u8, u8, u8)>,
}

pub struct Rtf {
    fonts: Vec<String>,
    colors: Vec<(u8, u8, u8)>,
    body: String,
    base_size: u32,
}

impl Rtf {
    pub fn new() -> Rtf {
        Rtf { fonts: vec!["Calibri".into(), "Consolas".into()], colors: vec![], body: String::new(), base_size: 22 }
    }
    pub fn set_base_size(&mut self, hp: u32) {
        self.base_size = hp.clamp(12, 72);
    }
    fn font(&mut self, name: &str) -> usize {
        if let Some(i) = self.fonts.iter().position(|f| f.eq_ignore_ascii_case(name)) {
            return i;
        }
        self.fonts.push(name.to_string());
        self.fonts.len() - 1
    }
    fn color(&mut self, c: (u8, u8, u8)) -> usize {
        if let Some(i) = self.colors.iter().position(|x| *x == c) {
            return i + 1;
        }
        self.colors.push(c);
        self.colors.len()
    }
    pub fn raw(&mut self, s: &str) {
        self.body.push_str(s);
    }
    /// Начать абзац.
    pub fn para(&mut self, p: &Para) {
        self.body.push_str("\\pard\\plain");
        if p.in_table {
            self.body.push_str("\\intbl");
        }
        self.body.push_str(match p.align {
            1 => "\\qc",
            2 => "\\qr",
            3 => "\\qj",
            _ => "\\ql",
        });
        if p.left != 0 {
            self.body.push_str(&format!("\\li{}", p.left));
        }
        if p.first != 0 {
            self.body.push_str(&format!("\\fi{}", p.first));
        }
        self.body.push_str(&format!("\\sb{}\\sa{}", p.before.max(0), p.after.max(0)));
        if let Some(c) = p.shade {
            let i = self.color(c);
            self.body.push_str(&format!("\\cbpat{i}"));
        }
        self.body.push_str(&format!("\\f0\\fs{} ", self.base_size));
    }
    pub fn end_para(&mut self) {
        self.body.push_str("\\par\n");
    }
    /// Текст с оформлением.
    pub fn text(&mut self, s: &str, c: &Ch) {
        if s.is_empty() {
            return;
        }
        let mut pre = String::from("{");
        if c.bold {
            pre += "\\b";
        }
        if c.italic {
            pre += "\\i";
        }
        if c.underline {
            pre += "\\ul";
        }
        if c.strike {
            pre += "\\strike";
        }
        if c.sup {
            pre += "\\super";
        }
        if c.sub {
            pre += "\\sub";
        }
        pre += &format!("\\fs{}", c.size.unwrap_or(self.base_size));
        if let Some(col) = c.color {
            let i = self.color(col);
            pre += &format!("\\cf{i}");
        }
        if let Some(col) = c.back {
            let i = self.color(col);
            pre += &format!("\\highlight{i}");
        }
        if let Some(f) = &c.font {
            let i = self.font(f);
            pre += &format!("\\f{i}");
        }
        pre.push(' ');
        self.body.push_str(&pre);
        escape_into(&mut self.body, s);
        self.body.push('}');
    }
    pub fn line_break(&mut self) {
        self.body.push_str("\\line ");
    }
    pub fn page_break(&mut self) {
        self.body.push_str("\\page ");
    }
    /// Картинка PNG/JPEG; размер в твипах.
    pub fn picture(&mut self, data: &[u8], w_tw: i32, h_tw: i32) {
        let kind = if data.starts_with(&[0x89, b'P', b'N', b'G']) {
            "\\pngblip"
        } else if data.starts_with(&[0xFF, 0xD8]) {
            "\\jpegblip"
        } else {
            return;
        };
        self.body.push_str(&format!("{{\\pict{kind}\\picwgoal{}\\pichgoal{}\n", w_tw.max(1), h_tw.max(1)));
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for (i, b) in data.iter().enumerate() {
            self.body.push(HEX[(b >> 4) as usize] as char);
            self.body.push(HEX[(b & 15) as usize] as char);
            if i % 64 == 63 {
                self.body.push('\n');
            }
        }
        self.body.push('}');
    }
    /// Строка таблицы: правые границы ячеек (твипы от левого поля).
    pub fn row_start(&mut self, rights: &[i32]) {
        self.body.push_str("\\trowd\\trgaph80\\trleft0");
        for r in rights {
            self.body.push_str(&format!(
                "\\clbrdrt\\brdrs\\brdrw8\\clbrdrl\\brdrs\\brdrw8\\clbrdrb\\brdrs\\brdrw8\\clbrdrr\\brdrs\\brdrw8\\cellx{r}"
            ));
        }
        self.body.push('\n');
    }
    pub fn cell_end(&mut self) {
        self.body.push_str("\\cell ");
    }
    pub fn row_end(&mut self) {
        self.body.push_str("\\row\n");
    }
    pub fn finish(self) -> String {
        let mut s = String::from("{\\rtf1\\ansi\\ansicpg1252\\deff0\\uc1{\\fonttbl");
        for (i, f) in self.fonts.iter().enumerate() {
            s += &format!("{{\\f{i}\\fnil\\fcharset0 ");
            escape_into(&mut s, f);
            s += ";}";
        }
        s += "}{\\colortbl;";
        for (r, g, b) in &self.colors {
            s += &format!("\\red{r}\\green{g}\\blue{b};");
        }
        s += "}\n\\viewkind4\n";
        s += &self.body;
        s += "}";
        s
    }
}

/// Экранирование текста для RTF (всё не-ASCII — через \uN?).
pub fn escape_into(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\t' => out.push_str("\\tab "),
            '\n' => out.push_str("\\line "),
            '\r' => {}
            '\u{a0}' => out.push_str("\\~"),
            c if (c as u32) < 0x80 && !c.is_control() => out.push(c),
            c => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{}?", *u as i16));
                }
            }
        }
    }
}

fn hex_color(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.trim_start_matches('#');
    if s.len() != 6 || s.eq_ignore_ascii_case("auto") {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some(((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// Длина ODF («2.5cm», «1in», «12pt», «20mm») → твипы.
fn odf_len(s: &str) -> Option<i32> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len()));
    let v: f64 = num.parse().ok()?;
    let k = match unit {
        "cm" => 1440.0 / 2.54,
        "mm" => 144.0 / 2.54,
        "in" => 1440.0,
        "pt" => 20.0,
        "pc" => 240.0,
        "px" => 15.0,
        _ => return None,
    };
    Some((v * k).round() as i32)
}

/// Картинку любого формата (EMF/GIF/BMP/TIFF…) привести к PNG/JPEG для RTF.
pub type ImageFixer<'a> = &'a dyn Fn(&[u8]) -> Option<Vec<u8>>;

fn fit_picture(rtf: &mut Rtf, data: &[u8], w: i32, h: i32, max_w: i32, fix: ImageFixer) {
    let ok = data.starts_with(&[0x89, b'P', b'N', b'G']) || data.starts_with(&[0xFF, 0xD8]);
    let owned;
    let data = if ok {
        data
    } else {
        match fix(data) {
            Some(d) => {
                owned = d;
                &owned
            }
            None => return,
        }
    };
    let (mut w, mut h) = (w.max(1), h.max(1));
    if w > max_w {
        h = (h as i64 * max_w as i64 / w as i64) as i32;
        w = max_w;
    }
    rtf.picture(data, w, h);
}

// ---------------------------------------------------------------- ZIP ---
fn zip_text(z: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>, name: &str) -> Option<String> {
    let mut f = z.by_name(name).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    Some(s)
}

fn zip_bytes(z: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>, name: &str) -> Option<Vec<u8>> {
    let mut f = z.by_name(name).ok()?;
    let mut v = vec![];
    f.read_to_end(&mut v).ok()?;
    Some(v)
}

// ---------------------------------------------------------------- DOCX ---
#[derive(Clone, Default)]
struct Style {
    name: String,
    based_on: Option<String>,
    ch: ChPatch,
    align: Option<u8>,
    outline: Option<u32>,
}

/// Частичные свойства текста (из стиля или прямого оформления).
#[derive(Clone, Default)]
struct ChPatch {
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    size: Option<u32>,
    color: Option<(u8, u8, u8)>,
    back: Option<(u8, u8, u8)>,
    font: Option<String>,
    valign: Option<String>,
}

impl ChPatch {
    fn apply(&self, c: &mut Ch) {
        if let Some(v) = self.bold {
            c.bold = v;
        }
        if let Some(v) = self.italic {
            c.italic = v;
        }
        if let Some(v) = self.underline {
            c.underline = v;
        }
        if let Some(v) = self.strike {
            c.strike = v;
        }
        if self.size.is_some() {
            c.size = self.size;
        }
        if self.color.is_some() {
            c.color = self.color;
        }
        if self.back.is_some() {
            c.back = self.back;
        }
        if self.font.is_some() {
            c.font = self.font.clone();
        }
        if let Some(v) = &self.valign {
            c.sup = v == "superscript";
            c.sub = v == "subscript";
        }
    }
}

fn on(n: &Node) -> bool {
    !matches!(n.attr("val"), Some("0") | Some("false") | Some("off") | Some("none"))
}

fn docx_rpr(rpr: &Node) -> ChPatch {
    let mut p = ChPatch::default();
    for c in &rpr.children {
        match c.name.as_str() {
            "b" => p.bold = Some(on(c)),
            "i" => p.italic = Some(on(c)),
            "u" => p.underline = Some(on(c)),
            "strike" | "dstrike" => p.strike = Some(on(c)),
            "sz" => p.size = c.attr("val").and_then(|v| v.parse().ok()),
            "color" => p.color = c.attr("val").and_then(hex_color),
            "highlight" => {
                p.back = match c.attr("val") {
                    Some("yellow") => Some((255, 255, 0)),
                    Some("green") => Some((0, 255, 0)),
                    Some("cyan") => Some((0, 255, 255)),
                    Some("magenta") => Some((255, 0, 255)),
                    Some("red") => Some((255, 0, 0)),
                    Some("lightGray") => Some((211, 211, 211)),
                    _ => None,
                }
            }
            "shd" => {
                if let Some(f) = c.attr("fill").and_then(hex_color) {
                    p.back = Some(f);
                }
            }
            "rFonts" => p.font = c.attr("ascii").or(c.attr("hAnsi")).or(c.attr("cs")).map(|s| s.to_string()),
            "vertAlign" => p.valign = c.attr("val").map(|s| s.to_string()),
            _ => {}
        }
    }
    p
}

fn docx_align(v: &str) -> u8 {
    match v {
        "center" => 1,
        "right" | "end" => 2,
        "both" | "distribute" | "justify" => 3,
        _ => 0,
    }
}

struct Docx<'a> {
    rtf: Rtf,
    styles: HashMap<String, Style>,
    default_ch: ChPatch,
    rels: HashMap<String, String>,
    zip: zip::ZipArchive<std::io::Cursor<Vec<u8>>>,
    num_fmt: HashMap<(String, u32), String>,
    counters: HashMap<(String, u32), u32>,
    text_w: i32,
    fix: ImageFixer<'a>,
}

impl Docx<'_> {
    /// Цепочка стилей (basedOn) от базового к конкретному.
    fn style_chain(&self, id: &str) -> Vec<Style> {
        let mut v = vec![];
        let mut cur = Some(id.to_string());
        let mut guard = 0;
        while let (Some(c), true) = (cur, guard < 16) {
            guard += 1;
            match self.styles.get(&c) {
                Some(s) => {
                    v.push(s.clone());
                    cur = s.based_on.clone();
                }
                None => break,
            }
        }
        v.reverse();
        v
    }

    fn paragraph(&mut self, p: &Node, in_table: bool) {
        let ppr = p.child("pPr");
        let style_id = ppr.and_then(|x| x.child("pStyle")).and_then(|x| x.attr("val")).unwrap_or("Normal").to_string();
        let chain = self.style_chain(&style_id);
        let mut base = Ch::default();
        self.default_ch.apply(&mut base);
        let mut para = Para { in_table, after: if in_table { 0 } else { 120 }, ..Default::default() };
        let mut heading: Option<u32> = None;
        for s in &chain {
            s.ch.apply(&mut base);
            if let Some(a) = s.align {
                para.align = a;
            }
            let lname = s.name.to_lowercase();
            if s.outline.is_some() {
                heading = s.outline;
            } else if let Some(n) = lname.strip_prefix("heading ").or_else(|| lname.strip_prefix("заголовок ")) {
                heading = n.trim().parse::<u32>().ok().map(|n| n.saturating_sub(1));
            } else if lname == "title" || lname == "название" {
                heading = Some(0);
                if base.size.is_none() {
                    base.size = Some(56);
                }
            }
        }
        if let Some(h) = heading {
            base.bold = true;
            if base.size.is_none() {
                base.size = Some([40, 32, 28, 26, 24, 24][h.min(5) as usize]);
            }
            para.before = 240;
        }
        let mut prefix = String::new();
        if let Some(ppr) = ppr {
            if let Some(jc) = ppr.child("jc").and_then(|j| j.attr("val")) {
                para.align = docx_align(jc);
            }
            if let Some(sp) = ppr.child("spacing") {
                if let Some(b) = sp.attr("before").and_then(|v| v.parse().ok()) {
                    para.before = b;
                }
                if let Some(a) = sp.attr("after").and_then(|v| v.parse().ok()) {
                    para.after = a;
                }
            }
            if let Some(ind) = ppr.child("ind") {
                para.left = ind.attr("left").or(ind.attr("start")).and_then(|v| v.parse().ok()).unwrap_or(0);
                para.first = ind.attr("firstLine").and_then(|v| v.parse().ok()).unwrap_or(0)
                    - ind.attr("hanging").and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
            }
            if let Some(num) = ppr.child("numPr") {
                let id = num.child("numId").and_then(|n| n.attr("val")).unwrap_or("0").to_string();
                let lvl: u32 = num.child("ilvl").and_then(|n| n.attr("val")).and_then(|v| v.parse().ok()).unwrap_or(0);
                if id != "0" {
                    let fmt = self.num_fmt.get(&(id.clone(), lvl)).cloned().unwrap_or_else(|| "bullet".into());
                    // сбросить счётчики вложенных уровней
                    self.counters.retain(|(i, l), _| !(i == &id && *l > lvl));
                    let n = self.counters.entry((id, lvl)).or_insert(0);
                    *n += 1;
                    prefix = match fmt.as_str() {
                        "bullet" => "•\t".into(),
                        "lowerLetter" => format!("{}.\t", (b'a' + ((*n - 1) % 26) as u8) as char),
                        "upperLetter" => format!("{}.\t", (b'A' + ((*n - 1) % 26) as u8) as char),
                        "lowerRoman" => format!("{}.\t", roman(*n).to_lowercase()),
                        "upperRoman" => format!("{}.\t", roman(*n)),
                        _ => format!("{n}.\t"),
                    };
                    if para.left == 0 {
                        para.left = 360 * (lvl as i32 + 1);
                        para.first = -360;
                    }
                }
            }
            if let Some(sh) = ppr.child("shd").and_then(|s| s.attr("fill")).and_then(hex_color) {
                para.shade = Some(sh);
            }
        }
        self.rtf.para(&para);
        if !prefix.is_empty() {
            self.rtf.text(&prefix, &base);
        }
        self.runs(p, &base);
        self.rtf.end_para();
    }

    fn runs(&mut self, parent: &Node, base: &Ch) {
        for c in &parent.children {
            match c.name.as_str() {
                "r" => self.run(c, base),
                "hyperlink" | "smartTag" | "ins" | "fldSimple" | "sdt" | "sdtContent" | "customXml" => {
                    let mut ch = base.clone();
                    if c.name == "hyperlink" {
                        ch.color = Some((5, 99, 193));
                        ch.underline = true;
                    }
                    self.runs(c, &ch)
                }
                _ => {}
            }
        }
    }

    fn run(&mut self, r: &Node, base: &Ch) {
        let mut ch = base.clone();
        if let Some(rpr) = r.child("rPr") {
            if let Some(sid) = rpr.child("rStyle").and_then(|s| s.attr("val")) {
                for s in self.style_chain(sid) {
                    s.ch.apply(&mut ch);
                }
            }
            docx_rpr(rpr).apply(&mut ch);
        }
        for c in &r.children {
            match c.name.as_str() {
                "t" | "delText" => {
                    if c.name == "t" {
                        self.rtf.text(&c.all_text(), &ch)
                    }
                }
                "tab" => self.rtf.text("\t", &ch),
                "br" => {
                    if c.attr("type") == Some("page") {
                        self.rtf.page_break()
                    } else {
                        self.rtf.line_break()
                    }
                }
                "cr" => self.rtf.line_break(),
                "noBreakHyphen" => self.rtf.text("-", &ch),
                "sym" => {
                    if let Some(v) = c.attr("char").and_then(|v| u32::from_str_radix(v, 16).ok()).and_then(char::from_u32) {
                        self.rtf.text(&v.to_string(), &ch)
                    }
                }
                "drawing" | "pict" | "object" => self.image(c),
                "AlternateContent" => {
                    if let Some(f) = c.child("Fallback").or_else(|| c.child("Choice")) {
                        self.run(f, base)
                    }
                }
                _ => {}
            }
        }
    }

    fn image(&mut self, d: &Node) {
        let Some(blip) = d.find("blip").and_then(|b| b.attr("embed")).or_else(|| d.find("imagedata").and_then(|b| b.attr("id"))) else {
            return;
        };
        let Some(target) = self.rels.get(blip).cloned() else { return };
        let path = if let Some(t) = target.strip_prefix('/') { t.to_string() } else { format!("word/{target}") };
        let Some(data) = zip_bytes(&mut self.zip, &path) else { return };
        // размер: wp:extent (EMU) → твипы
        let (w, h) = d
            .find("extent")
            .and_then(|e| Some((e.attr("cx")?.parse::<i64>().ok()? / 635, e.attr("cy")?.parse::<i64>().ok()? / 635)))
            .map(|(w, h)| (w as i32, h as i32))
            .unwrap_or((4000, 3000));
        fit_picture(&mut self.rtf, &data, w, h, self.text_w, self.fix);
    }

    fn table(&mut self, t: &Node) {
        let grid: Vec<i32> = t
            .child("tblGrid")
            .map(|g| g.kids("gridCol").filter_map(|c| c.attr("w").and_then(|v| v.parse().ok())).collect())
            .unwrap_or_default();
        for tr in t.kids("tr") {
            let cells: Vec<&Node> = tr.kids("tc").collect();
            if cells.is_empty() {
                continue;
            }
            // правые границы: из сетки с учётом объединений, иначе поровну
            let mut rights = vec![];
            let mut col = 0usize;
            let mut x = 0;
            for tc in &cells {
                let span: usize = tc.child("tcPr").and_then(|p| p.child("gridSpan")).and_then(|g| g.attr("val")).and_then(|v| v.parse().ok()).unwrap_or(1);
                let w: i32 = if grid.len() >= col + span {
                    grid[col..col + span].iter().sum()
                } else {
                    self.text_w / cells.len() as i32
                };
                col += span;
                x += w.max(200);
                rights.push(x.min(self.text_w.max(x)));
            }
            // таблица шире листа — сжимаем пропорционально
            if x > self.text_w && x > 0 {
                for r in rights.iter_mut() {
                    *r = (*r as i64 * self.text_w as i64 / x as i64) as i32;
                }
            }
            self.rtf.row_start(&rights);
            for tc in &cells {
                let mut first = true;
                for c in &tc.children {
                    match c.name.as_str() {
                        "p" => {
                            if !first {
                                self.rtf.raw("\\par ");
                            }
                            self.paragraph_inline(c);
                            first = false;
                        }
                        "tbl" => {
                            // вложенная таблица — просто текстом
                            for p in collect(c, "p") {
                                if !first {
                                    self.rtf.raw("\\par ");
                                }
                                self.paragraph_inline(p);
                                first = false;
                            }
                        }
                        _ => {}
                    }
                }
                self.rtf.cell_end();
            }
            self.rtf.row_end();
        }
        self.rtf.raw("\\pard\\plain\\par\n");
    }

    /// Абзац внутри ячейки: без \\par в конце (его ставит \\cell).
    fn paragraph_inline(&mut self, p: &Node) {
        self.paragraph(p, true);
        if self.rtf.body.ends_with("\\par\n") {
            let l = self.rtf.body.len();
            self.rtf.body.truncate(l - 5);
        }
    }

    fn body(&mut self, b: &Node) {
        for c in &b.children {
            match c.name.as_str() {
                "p" => self.paragraph(c, false),
                "tbl" => self.table(c),
                "sdt" => {
                    if let Some(sc) = c.child("sdtContent") {
                        self.body(sc)
                    }
                }
                _ => {}
            }
        }
    }
}

fn collect<'a>(n: &'a Node, name: &str) -> Vec<&'a Node> {
    let mut v = vec![];
    fn go<'a>(n: &'a Node, name: &str, v: &mut Vec<&'a Node>) {
        for c in &n.children {
            if c.name == name {
                v.push(c);
            } else {
                go(c, name, v);
            }
        }
    }
    go(n, name, &mut v);
    v
}

fn roman(mut n: u32) -> String {
    let t = [(1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"), (50, "L"), (40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I")];
    let mut s = String::new();
    for (v, r) in t {
        while n >= v {
            s += r;
            n -= v;
        }
    }
    s
}

pub fn docx_to_rtf(bytes: Vec<u8>, fix: ImageFixer) -> Result<(String, PageSetup), String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("не DOCX: {e}"))?;
    let doc = zip_text(&mut zip, "word/document.xml").ok_or("в архиве нет word/document.xml")?;
    let doc = xmltree::parse(&doc)?;
    // связи (картинки)
    let mut rels = HashMap::new();
    if let Some(r) = zip_text(&mut zip, "word/_rels/document.xml.rels").and_then(|s| xmltree::parse(&s).ok()) {
        if let Some(root) = r.child("Relationships") {
            for rel in root.kids("Relationship") {
                if let (Some(id), Some(t)) = (rel.attr("Id"), rel.attr("Target")) {
                    rels.insert(id.to_string(), t.to_string());
                }
            }
        }
    }
    // стили
    let mut styles = HashMap::new();
    let mut default_ch = ChPatch::default();
    if let Some(st) = zip_text(&mut zip, "word/styles.xml").and_then(|s| xmltree::parse(&s).ok()) {
        if let Some(root) = st.child("styles") {
            if let Some(rpr) = root.child("docDefaults").and_then(|d| d.find("rPr")) {
                default_ch = docx_rpr(rpr);
            }
            for s in root.kids("style") {
                let Some(id) = s.attr("styleId") else { continue };
                let style = Style {
                    name: s.child("name").and_then(|n| n.attr("val")).unwrap_or("").to_string(),
                    based_on: s.child("basedOn").and_then(|n| n.attr("val")).map(|v| v.to_string()),
                    ch: s.child("rPr").map(docx_rpr).unwrap_or_default(),
                    align: s.child("pPr").and_then(|p| p.child("jc")).and_then(|j| j.attr("val")).map(docx_align),
                    outline: s.child("pPr").and_then(|p| p.child("outlineLvl")).and_then(|o| o.attr("val")).and_then(|v| v.parse().ok()),
                };
                styles.insert(id.to_string(), style);
            }
        }
    }
    // нумерация списков
    let mut num_fmt = HashMap::new();
    if let Some(nm) = zip_text(&mut zip, "word/numbering.xml").and_then(|s| xmltree::parse(&s).ok()) {
        if let Some(root) = nm.child("numbering") {
            let mut abs: HashMap<String, Vec<(u32, String)>> = HashMap::new();
            for a in root.kids("abstractNum") {
                let id = a.attr("abstractNumId").unwrap_or("").to_string();
                let lv = a
                    .kids("lvl")
                    .map(|l| {
                        (l.attr("ilvl").and_then(|v| v.parse().ok()).unwrap_or(0), l.child("numFmt").and_then(|f| f.attr("val")).unwrap_or("decimal").to_string())
                    })
                    .collect();
                abs.insert(id, lv);
            }
            for n in root.kids("num") {
                let (Some(nid), Some(aid)) = (n.attr("numId"), n.child("abstractNumId").and_then(|a| a.attr("val"))) else { continue };
                for (l, f) in abs.get(aid).cloned().unwrap_or_default() {
                    num_fmt.insert((nid.to_string(), l), f);
                }
            }
        }
    }
    let body = doc.child("document").and_then(|d| d.child("body")).ok_or("пустой документ")?;
    // лист
    let mut page = PageSetup::default();
    if let Some(sect) = body.child("sectPr") {
        if let Some(sz) = sect.child("pgSz") {
            page.w = sz.attr("w").and_then(|v| v.parse().ok()).unwrap_or(page.w);
            page.h = sz.attr("h").and_then(|v| v.parse().ok()).unwrap_or(page.h);
        }
        if let Some(m) = sect.child("pgMar") {
            let g = |k: &str, d: i32| m.attr(k).and_then(|v| v.parse::<i32>().ok()).unwrap_or(d).max(0);
            page.margin = (g("left", 1134), g("top", 1134), g("right", 1134), g("bottom", 1134));
        }
    }
    let text_w = (page.w - page.margin.0 - page.margin.2).max(1440);
    let mut rtf = Rtf::new();
    if let Some(s) = default_ch.size {
        rtf.set_base_size(s);
    }
    if let Some(f) = &default_ch.font {
        rtf.fonts[0] = f.clone();
    }
    let mut d = Docx { rtf, styles, default_ch, rels, zip, num_fmt, counters: HashMap::new(), text_w, fix };
    d.body(body);
    Ok((d.rtf.finish(), page))
}

// ----------------------------------------------------------------- ODT ---
struct Odt<'a> {
    rtf: Rtf,
    styles: HashMap<String, (ChPatch, Option<u8>)>,
    zip: zip::ZipArchive<std::io::Cursor<Vec<u8>>>,
    text_w: i32,
    fix: ImageFixer<'a>,
}

fn odt_style(s: &Node) -> (ChPatch, Option<u8>) {
    let mut p = ChPatch::default();
    let mut align = None;
    if let Some(t) = s.child("text-properties") {
        if let Some(w) = t.attr("font-weight") {
            p.bold = Some(w == "bold" || w.parse::<u32>().map(|n| n >= 600).unwrap_or(false));
        }
        if let Some(st) = t.attr("font-style") {
            p.italic = Some(st == "italic" || st == "oblique");
        }
        if let Some(u) = t.attr("text-underline-style") {
            p.underline = Some(u != "none");
        }
        if let Some(l) = t.attr("text-line-through-style") {
            p.strike = Some(l != "none");
        }
        if let Some(sz) = t.attr("font-size").and_then(odf_len) {
            p.size = Some((sz / 10).max(8) as u32);
        }
        p.color = t.attr("color").and_then(hex_color);
        if let Some(bg) = t.attr("background-color").and_then(hex_color) {
            p.back = Some(bg);
        }
        p.font = t.attr("font-name").map(|s| s.to_string());
        if let Some(pos) = t.attr("text-position") {
            p.valign = Some(if pos.starts_with("super") { "superscript".into() } else if pos.starts_with("sub") { "subscript".into() } else { String::new() });
        }
    }
    if let Some(pp) = s.child("paragraph-properties") {
        align = pp.attr("text-align").map(|a| match a {
            "center" => 1,
            "end" | "right" => 2,
            "justify" => 3,
            _ => 0,
        });
    }
    (p, align)
}

impl Odt<'_> {
    fn block(&mut self, n: &Node, list_level: i32, in_table: bool) {
        for c in &n.children {
            match c.name.as_str() {
                "h" | "p" => {
                    let mut ch = Ch::default();
                    let mut para = Para { in_table, after: if in_table { 0 } else { 100 }, ..Default::default() };
                    if let Some((patch, al)) = c.attr("style-name").and_then(|s| self.styles.get(s)).cloned() {
                        patch.apply(&mut ch);
                        if let Some(a) = al {
                            para.align = a;
                        }
                    }
                    if c.name == "h" {
                        let lvl: usize = c.attr("outline-level").and_then(|v| v.parse().ok()).unwrap_or(1);
                        ch.bold = true;
                        ch.size = ch.size.or(Some([40, 32, 28, 26, 24, 24][(lvl.max(1) - 1).min(5)]));
                        para.before = 240;
                    }
                    if list_level > 0 {
                        para.left = 360 * list_level;
                        para.first = -360;
                    }
                    if in_table {
                        // внутри ячейки абзацы разделяем \par, но без последнего
                        self.rtf.para(&para);
                    } else {
                        self.rtf.para(&para);
                    }
                    if list_level > 0 {
                        self.rtf.text("•\t", &ch);
                    }
                    self.inline(c, &ch);
                    if !in_table {
                        self.rtf.end_para();
                    } else {
                        self.rtf.raw("\\par ");
                    }
                }
                "list" => {
                    for item in c.kids("list-item") {
                        self.block(item, list_level + 1, in_table);
                    }
                }
                "table" => self.table(c),
                "section" | "list-header" | "list-item" => self.block(c, list_level, in_table),
                _ => {}
            }
        }
    }

    fn inline(&mut self, n: &Node, base: &Ch) {
        for c in &n.children {
            if c.is_text() {
                self.rtf.text(&c.text, base);
                continue;
            }
            match c.name.as_str() {
                "span" | "a" => {
                    let mut ch = base.clone();
                    if let Some((patch, _)) = c.attr("style-name").and_then(|s| self.styles.get(s)).cloned() {
                        patch.apply(&mut ch);
                    }
                    if c.name == "a" {
                        ch.color = Some((5, 99, 193));
                        ch.underline = true;
                    }
                    self.inline(c, &ch);
                }
                "s" => {
                    let k: usize = c.attr("c").and_then(|v| v.parse().ok()).unwrap_or(1);
                    self.rtf.text(&" ".repeat(k), base);
                }
                "tab" => self.rtf.text("\t", base),
                "line-break" => self.rtf.line_break(),
                "frame" => {
                    if let Some(img) = c.child("image").and_then(|i| i.attr("href")) {
                        let w = c.attr("width").and_then(odf_len).unwrap_or(4000);
                        let h = c.attr("height").and_then(odf_len).unwrap_or(3000);
                        if let Some(data) = zip_bytes(&mut self.zip, img) {
                            fit_picture(&mut self.rtf, &data, w, h, self.text_w, self.fix);
                        }
                    }
                }
                _ => self.inline(c, base),
            }
        }
    }

    fn table(&mut self, t: &Node) {
        let rows: Vec<&Node> = collect(t, "table-row");
        for r in rows {
            let cells: Vec<&Node> = r.kids("table-cell").collect();
            if cells.is_empty() {
                continue;
            }
            let w = self.text_w / cells.len() as i32;
            let rights: Vec<i32> = (1..=cells.len() as i32).map(|i| i * w).collect();
            self.rtf.row_start(&rights);
            for c in cells {
                let before = self.rtf.body.len();
                self.block(c, 0, true);
                // убрать последний \par внутри ячейки
                if self.rtf.body.len() > before && self.rtf.body.ends_with("\\par ") {
                    let l = self.rtf.body.len();
                    self.rtf.body.truncate(l - 5);
                }
                if self.rtf.body.len() == before {
                    self.rtf.raw("\\pard\\plain\\intbl ");
                }
                self.rtf.cell_end();
            }
            self.rtf.row_end();
        }
        self.rtf.raw("\\pard\\plain\\par\n");
    }
}

pub fn odt_to_rtf(bytes: Vec<u8>, fix: ImageFixer) -> Result<(String, PageSetup), String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("не ODT: {e}"))?;
    let content = zip_text(&mut zip, "content.xml").ok_or("в архиве нет content.xml")?;
    let content = xmltree::parse(&content)?;
    let root = content.child("document-content").ok_or("не ODT")?;
    let mut styles = HashMap::new();
    let mut page = PageSetup::default();
    let style_doc = zip_text(&mut zip, "styles.xml").and_then(|s| xmltree::parse(&s).ok());
    for src in [style_doc.as_ref().and_then(|d| d.child("document-styles")), Some(root)].into_iter().flatten() {
        for sect in ["styles", "automatic-styles"] {
            if let Some(ss) = src.child(sect) {
                for s in ss.kids("style") {
                    if let Some(n) = s.attr("name") {
                        styles.insert(n.to_string(), odt_style(s));
                    }
                }
                if let Some(pl) = ss.child("page-layout").and_then(|p| p.child("page-layout-properties")) {
                    page.w = pl.attr("page-width").and_then(odf_len).unwrap_or(page.w);
                    page.h = pl.attr("page-height").and_then(odf_len).unwrap_or(page.h);
                    let g = |k: &str| pl.attr(k).and_then(odf_len).unwrap_or(1134).max(0);
                    page.margin = (g("margin-left"), g("margin-top"), g("margin-right"), g("margin-bottom"));
                }
            }
        }
    }
    let text = root.child("body").and_then(|b| b.child("text")).ok_or("пустой документ")?;
    let text_w = (page.w - page.margin.0 - page.margin.2).max(1440);
    let mut o = Odt { rtf: Rtf::new(), styles, zip, text_w, fix };
    o.block(text, 0, false);
    Ok((o.rtf.finish(), page))
}

// ------------------------------------------------------------ Markdown ---
pub fn markdown_to_rtf(src: &str, base_dir: Option<&std::path::Path>, fix: ImageFixer) -> String {
    use pulldown_cmark::{Event as E, HeadingLevel, Options, Parser, Tag, TagEnd};
    let mut rtf = Rtf::new();
    let text_w = PageSetup::default().w - 2 * 1134;
    let mut ch_stack: Vec<Ch> = vec![Ch::default()];
    let mut list_stack: Vec<Option<u64>> = vec![];
    let mut quote = 0;
    let mut in_code_block = false;
    let mut para_open = false;
    let mut table_row: Vec<String> = vec![];
    let mut table_rows: Vec<(bool, Vec<String>)> = vec![];
    let mut cell = String::new();
    let mut in_table = false;
    let mut in_head = false;
    let cur = |s: &Vec<Ch>| s.last().cloned().unwrap_or_default();
    let open_para = |rtf: &mut Rtf, quote: i32, lists: usize, before: i32, shade: Option<(u8, u8, u8)>| {
        let p = Para { left: 360 * (quote + lists as i32), before, after: 120, shade, ..Default::default() };
        rtf.para(&p);
    };
    for ev in Parser::new_ext(src, Options::all()) {
        match ev {
            E::Start(Tag::Heading { level, .. }) => {
                let size = match level {
                    HeadingLevel::H1 => 40,
                    HeadingLevel::H2 => 32,
                    HeadingLevel::H3 => 28,
                    _ => 24,
                };
                open_para(&mut rtf, quote, list_stack.len(), 240, None);
                para_open = true;
                ch_stack.push(Ch { bold: true, size: Some(size), ..cur(&ch_stack) });
            }
            E::End(TagEnd::Heading(_)) => {
                ch_stack.pop();
                rtf.end_para();
                para_open = false;
            }
            E::Start(Tag::Paragraph) => {
                if !in_table && !para_open {
                    open_para(&mut rtf, quote, list_stack.len(), 0, None);
                    para_open = true;
                }
            }
            E::End(TagEnd::Paragraph) => {
                if para_open && !in_table {
                    rtf.end_para();
                    para_open = false;
                }
            }
            E::Start(Tag::BlockQuote(_)) => {
                quote += 1;
                ch_stack.push(Ch { color: Some((110, 110, 110)), italic: true, ..cur(&ch_stack) });
            }
            E::End(TagEnd::BlockQuote(_)) => {
                quote -= 1;
                ch_stack.pop();
            }
            E::Start(Tag::CodeBlock(_)) => {
                in_code_block = true;
                open_para(&mut rtf, quote, list_stack.len(), 60, Some((240, 242, 245)));
                para_open = true;
                ch_stack.push(Ch { font: Some("Consolas".into()), size: Some(19), ..cur(&ch_stack) });
            }
            E::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                ch_stack.pop();
                rtf.end_para();
                para_open = false;
            }
            E::Start(Tag::List(start)) => list_stack.push(start),
            E::End(TagEnd::List(_)) => {
                list_stack.pop();
            }
            E::Start(Tag::Item) => {
                let lvl = list_stack.len();
                let p = Para { left: 360 * (quote + lvl as i32), first: -360, after: 60, ..Default::default() };
                rtf.para(&p);
                para_open = true;
                let marker = match list_stack.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}.\t");
                        *n += 1;
                        m
                    }
                    _ => "•\t".to_string(),
                };
                rtf.text(&marker, &cur(&ch_stack));
            }
            E::End(TagEnd::Item) => {
                if para_open {
                    rtf.end_para();
                    para_open = false;
                }
            }
            E::TaskListMarker(done) => rtf.text(if done { "☑ " } else { "☐ " }, &cur(&ch_stack)),
            E::Start(Tag::Emphasis) => ch_stack.push(Ch { italic: true, ..cur(&ch_stack) }),
            E::Start(Tag::Strong) => ch_stack.push(Ch { bold: true, ..cur(&ch_stack) }),
            E::Start(Tag::Strikethrough) => ch_stack.push(Ch { strike: true, ..cur(&ch_stack) }),
            E::Start(Tag::Link { .. }) => ch_stack.push(Ch { underline: true, color: Some((5, 99, 193)), ..cur(&ch_stack) }),
            E::End(TagEnd::Emphasis) | E::End(TagEnd::Strong) | E::End(TagEnd::Strikethrough) | E::End(TagEnd::Link) => {
                ch_stack.pop();
            }
            E::Start(Tag::Image { dest_url, .. }) => {
                let p = base_dir.map(|d| d.join(dest_url.as_ref())).unwrap_or_else(|| dest_url.to_string().into());
                if let Ok(data) = std::fs::read(&p) {
                    if !para_open {
                        open_para(&mut rtf, quote, list_stack.len(), 0, None);
                        para_open = true;
                    }
                    let (w, h) = image_size(&data).map(|(w, h)| (w as i32 * 15, h as i32 * 15)).unwrap_or((6000, 4000));
                    fit_picture(&mut rtf, &data, w, h, text_w, fix);
                }
                ch_stack.push(Ch { color: Some((140, 140, 140)), ..cur(&ch_stack) });
            }
            E::End(TagEnd::Image) => {
                ch_stack.pop();
            }
            E::Start(Tag::Table(_)) => {
                in_table = true;
                table_rows.clear();
            }
            E::Start(Tag::TableHead) => in_head = true,
            E::End(TagEnd::TableHead) => {
                table_rows.push((true, std::mem::take(&mut table_row)));
                in_head = false;
            }
            E::Start(Tag::TableRow) => table_row.clear(),
            E::End(TagEnd::TableRow) => table_rows.push((in_head, std::mem::take(&mut table_row))),
            E::Start(Tag::TableCell) => cell.clear(),
            E::End(TagEnd::TableCell) => table_row.push(std::mem::take(&mut cell)),
            E::End(TagEnd::Table) => {
                in_table = false;
                let cols = table_rows.iter().map(|r| r.1.len()).max().unwrap_or(1).max(1);
                let w = text_w / cols as i32;
                let rights: Vec<i32> = (1..=cols as i32).map(|i| i * w).collect();
                for (head, row) in &table_rows {
                    rtf.row_start(&rights);
                    for k in 0..cols {
                        rtf.raw("\\pard\\plain\\intbl ");
                        let ch = Ch { bold: *head, ..Default::default() };
                        rtf.text(row.get(k).map(|s| s.as_str()).unwrap_or(""), &ch);
                        rtf.cell_end();
                    }
                    rtf.row_end();
                }
                rtf.raw("\\pard\\plain\\par\n");
            }
            E::Text(t) => {
                if in_table {
                    cell.push_str(&t);
                } else {
                    if !para_open {
                        open_para(&mut rtf, quote, list_stack.len(), 0, None);
                        para_open = true;
                    }
                    let s = if in_code_block { t.trim_end_matches('\n').to_string() } else { t.to_string() };
                    rtf.text(&s, &cur(&ch_stack));
                }
            }
            E::Code(t) => {
                if in_table {
                    cell.push_str(&t);
                } else {
                    let ch = Ch { font: Some("Consolas".into()), back: Some((238, 240, 243)), ..cur(&ch_stack) };
                    rtf.text(&t, &ch);
                }
            }
            E::SoftBreak => {
                if in_table {
                    cell.push(' ')
                } else {
                    rtf.text(" ", &cur(&ch_stack))
                }
            }
            E::HardBreak => rtf.line_break(),
            E::Rule => {
                rtf.raw("\\pard\\plain\\brdrb\\brdrs\\brdrw10\\brsp20\\sa120\\par\n");
            }
            _ => {}
        }
    }
    rtf.finish()
}

/// Размер PNG/JPEG по заголовку (для картинок Markdown).
fn image_size(d: &[u8]) -> Option<(u32, u32)> {
    if d.starts_with(&[0x89, b'P', b'N', b'G']) && d.len() > 24 {
        return Some((u32::from_be_bytes(d[16..20].try_into().ok()?), u32::from_be_bytes(d[20..24].try_into().ok()?)));
    }
    if d.starts_with(&[0xFF, 0xD8]) {
        let mut i = 2;
        while i + 9 < d.len() {
            if d[i] != 0xFF {
                return None;
            }
            let m = d[i + 1];
            let len = u16::from_be_bytes([d[i + 2], d[i + 3]]) as usize;
            if (0xC0..=0xCF).contains(&m) && m != 0xC4 && m != 0xC8 && m != 0xCC {
                return Some((u16::from_be_bytes([d[i + 7], d[i + 8]]) as u32, u16::from_be_bytes([d[i + 5], d[i + 6]]) as u32));
            }
            i += 2 + len;
        }
    }
    None
}

/// Простой текст → RTF (для предпросмотра и печати .txt как документа).
pub fn plain_to_rtf(s: &str) -> String {
    let mut rtf = Rtf::new();
    rtf.fonts[0] = "Consolas".into();
    rtf.set_base_size(20);
    for line in s.lines() {
        rtf.para(&Para::default());
        rtf.text(line, &Ch::default());
        rtf.end_para();
    }
    rtf.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn no_fix(_: &[u8]) -> Option<Vec<u8>> {
        None
    }

    fn make_zip(files: &[(&str, &str)]) -> Vec<u8> {
        // zip без сжатия собираем вручную (в exe нет записи ZIP)
        let mut out = vec![];
        let mut central = vec![];
        for (name, data) in files {
            let crc = crc32(data.as_bytes());
            let off = out.len() as u32;
            let mut h = vec![];
            h.extend_from_slice(&0x04034b50u32.to_le_bytes());
            h.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            h.extend_from_slice(&crc.to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            out.write_all(&h).unwrap();
            out.write_all(name.as_bytes()).unwrap();
            out.write_all(data.as_bytes()).unwrap();
            let mut c = vec![];
            c.extend_from_slice(&0x02014b50u32.to_le_bytes());
            c.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            c.extend_from_slice(&crc.to_le_bytes());
            c.extend_from_slice(&(data.len() as u32).to_le_bytes());
            c.extend_from_slice(&(data.len() as u32).to_le_bytes());
            c.extend_from_slice(&(name.len() as u16).to_le_bytes());
            c.extend_from_slice(&[0; 12]);
            c.extend_from_slice(&off.to_le_bytes());
            c.extend_from_slice(name.as_bytes());
            central.extend_from_slice(&c);
        }
        let cd_off = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&0x06054b50u32.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_off.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    fn crc32(d: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for b in d {
            c ^= *b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
            }
        }
        !c
    }

    #[test]
    fn escape() {
        let mut s = String::new();
        escape_into(&mut s, "a{b}\\ я\t😀");
        assert_eq!(s, "a\\{b\\}\\\\ \\u1103?\\tab \\u-10179?\\u-8704?");
    }

    #[test]
    fn docx_basic() {
        let doc = r#"<w:document xmlns:w="w"><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Заголовок</w:t></w:r></w:p>
<w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:rPr><w:b/><w:color w:val="FF0000"/></w:rPr><w:t>жирный</w:t></w:r><w:r><w:t xml:space="preserve"> и обычный</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>пункт</w:t></w:r></w:p>
<w:tbl><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="3000"/></w:tblGrid>
<w:tr><w:tc><w:p><w:r><w:t>A1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B1</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
<w:sectPr><w:pgSz w:w="16838" w:h="11906"/></w:sectPr></w:body></w:document>"#;
        let styles = r#"<w:styles xmlns:w="w"><w:style w:styleId="Heading1"><w:name w:val="heading 1"/></w:style></w:styles>"#;
        let num = r#"<w:numbering xmlns:w="w"><w:abstractNum w:abstractNumId="7"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="7"/></w:num></w:numbering>"#;
        let z = make_zip(&[("word/document.xml", doc), ("word/styles.xml", styles), ("word/numbering.xml", num)]);
        let (rtf, page) = docx_to_rtf(z, &no_fix).unwrap();
        assert_eq!(page.w, 16838, "альбомный лист");
        assert!(rtf.starts_with("{\\rtf1"));
        assert!(rtf.contains("\\b\\fs40"), "заголовок 1 — крупный жирный");
        assert!(rtf.contains("\\qc"));
        assert!(rtf.contains("\\red255\\green0\\blue0"));
        assert!(rtf.contains("1.\\tab"), "нумерованный список");
        assert!(rtf.contains("\\cellx2000\\") || rtf.contains("\\cellx2000"));
        assert!(rtf.contains("\\cellx5000"));
        assert!(rtf.contains("\\row"));
        // кириллица — через \\u
        assert!(rtf.contains("\\u1047?"), "«З» из «Заголовок»");
        assert_eq!(rtf.matches('{').count() - rtf.matches("\\{").count(), rtf.matches('}').count() - rtf.matches("\\}").count());
    }

    #[test]
    fn odt_basic() {
        let content = r#"<office:document-content xmlns:office="o" xmlns:text="t" xmlns:style="s" xmlns:fo="f" xmlns:table="tb">
<office:automatic-styles><style:style style:name="T1" style:family="text"><style:text-properties fo:font-weight="bold"/></style:style>
<style:style style:name="P1" style:family="paragraph"><style:paragraph-properties fo:text-align="center"/></style:style></office:automatic-styles>
<office:body><office:text><text:h text:outline-level="1">Глава</text:h>
<text:p text:style-name="P1">Текст <text:span text:style-name="T1">жирно</text:span><text:s text:c="2"/>и всё</text:p>
<text:list><text:list-item><text:p>пункт</text:p></text:list-item></text:list>
<table:table><table:table-row><table:table-cell><text:p>1</text:p></table:table-cell><table:table-cell><text:p>2</text:p></table:table-cell></table:table-row></table:table>
</office:text></office:body></office:document-content>"#;
        let z = make_zip(&[("content.xml", content)]);
        let (rtf, _) = odt_to_rtf(z, &no_fix).unwrap();
        assert!(rtf.contains("\\b\\fs40"));
        assert!(rtf.contains("\\qc"));
        assert!(rtf.contains("{\\b\\fs22 "), "жирный фрагмент");
        assert!(rtf.contains("  "), "text:s → пробелы");
        assert!(rtf.contains("\\u8226?\\tab"), "маркер списка");
        assert_eq!(rtf.matches("\\cell ").count(), 2);
    }

    #[test]
    fn markdown_basic() {
        let md = "# Заголовок\n\nТекст **жирный** и *курсив* `код`\n\n- раз\n- два\n\n1. один\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n```\nfn main() {}\n```\n";
        let rtf = markdown_to_rtf(md, None, &no_fix);
        assert!(rtf.contains("\\b\\fs40"));
        assert!(rtf.contains("{\\b\\fs22 "));
        assert!(rtf.contains("{\\i\\fs22 "));
        assert!(rtf.contains("\\u8226?\\tab"));
        assert!(rtf.contains("1.\\tab"));
        assert_eq!(rtf.matches("\\row").count(), 2);
        assert!(rtf.contains("fn main() \\{\\}"));
    }

    #[test]
    fn odf_lengths() {
        assert_eq!(odf_len("2.54cm"), Some(1440));
        assert_eq!(odf_len("12pt"), Some(240));
        assert_eq!(odf_len("1in"), Some(1440));
        assert_eq!(odf_len("abc"), None);
    }
}
