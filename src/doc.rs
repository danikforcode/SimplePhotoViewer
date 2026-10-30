// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Документ = список страниц. Страница — фото из файла, страница PDF или
//! картинка в памяти (вставка из буфера, коллаж, результат правки).
//!
//! Страницы PDF, которые только поворачивали, остаются «векторными»: при
//! сохранении в PDF они переносятся без потери качества. Любая другая правка
//! (обрезка, цвет, размер) превращает страницу в картинку.

use crate::image_buf::Img;
use crate::pdf_write::{self, OutPage, PhotoPage, Placed};
use crate::util::{file_name, is_pdf};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Разрешение, с которым PDF рисуется для экрана и правок.
pub const VIEW_DPI: f64 = 200.0;
/// Разрешение для печати страниц PDF.
pub const PRINT_DPI: f64 = 300.0;
const MAX_SIDE: f64 = 6000.0;

#[derive(Clone, Debug)]
pub enum Src {
    File(PathBuf),
    /// size — в пунктах, как его отдаёт Windows (с учётом /Rotate файла).
    Pdf { path: PathBuf, index: u32, size: (f64, f64) },
    Mem(String),
    /// Страница документа (DOCX/ODT/RTF/Markdown), разложенного RichEdit.
    Rich { path: PathBuf, doc: Rc<crate::richdoc::RichDoc>, index: u32 },
}

#[derive(Clone)]
pub struct Page {
    pub src: Src,
    /// Доп. поворот векторной страницы PDF, четверти по часовой.
    pub rot: u8,
    /// Картинка после правок (или содержимое Mem-страницы).
    pub img: Option<Rc<Img>>,
    /// Пунктов на пиксель для растровой страницы, полученной из PDF (размер листа).
    pub ppx: Option<f64>,
}

impl Page {
    pub fn file(p: PathBuf) -> Page {
        Page { src: Src::File(p), rot: 0, img: None, ppx: None }
    }
    pub fn mem(name: &str, img: Img) -> Page {
        Page { src: Src::Mem(name.to_string()), rot: 0, img: Some(Rc::new(img)), ppx: None }
    }
    pub fn is_vector(&self) -> bool {
        matches!(self.src, Src::Pdf { .. }) && self.img.is_none()
    }
    pub fn is_pdf(&self) -> bool {
        matches!(self.src, Src::Pdf { .. })
    }
    /// Страница документа (PDF или DOCX/ODT/…): вписывается в окно всегда, поворачивается без потерь.
    pub fn is_docpage(&self) -> bool {
        matches!(self.src, Src::Pdf { .. } | Src::Rich { .. })
    }
    pub fn path(&self) -> Option<&Path> {
        match &self.src {
            Src::File(p) | Src::Pdf { path: p, .. } | Src::Rich { path: p, .. } => Some(p),
            Src::Mem(_) => None,
        }
    }
    /// Подпись для списков.
    pub fn label(&self) -> String {
        let star = if self.img.is_some() && !matches!(self.src, Src::Mem(_)) { " *" } else { "" };
        match &self.src {
            Src::File(p) => format!("{}{star}", file_name(p)),
            Src::Pdf { path, index, .. } => format!("{} — стр. {}{star}", file_name(path), index + 1),
            Src::Mem(n) => n.clone(),
            Src::Rich { path, index, .. } => format!("{} — стр. {}{star}", file_name(path), index + 1),
        }
    }
    /// Размер листа в пунктах (для PDF-страниц), с учётом поворотов и правок.
    pub fn size_pt(&self) -> Option<(f64, f64)> {
        if let (Some(img), Some(k)) = (&self.img, self.ppx) {
            return Some((img.w as f64 * k, img.h as f64 * k));
        }
        match &self.src {
            Src::Pdf { size, .. } if self.img.is_none() => Some(if self.rot % 2 == 1 { (size.1, size.0) } else { *size }),
            Src::Rich { doc, .. } if self.img.is_none() => {
                let s = doc.size_pt();
                Some(if self.rot % 2 == 1 { (s.1, s.0) } else { s })
            }
            _ => None,
        }
    }
}

pub fn rot_q(img: Img, q: u8) -> Img {
    match q % 4 {
        1 => img.rotate_cw(),
        2 => img.rotate_cw().rotate_cw(),
        3 => img.rotate_ccw(),
        _ => img,
    }
}

/// Страницы из файла: фото — одна, PDF — все страницы.
pub fn pages_from(path: &Path) -> Result<Vec<Page>, String> {
    if is_pdf(path) {
        // основной путь — Windows.Data.Pdf; если недоступен, страницы узнаём через lopdf
        // (тогда показать их не выйдет, но склеить/сохранить — можно)
        let sizes = match crate::pdf_read::info(path) {
            Ok(info) => info.pages,
            Err(e) => pdf_write::page_sizes(path).map_err(|_| format!("{}:\n{e}", file_name(path)))?,
        };
        Ok(sizes
            .iter()
            .enumerate()
            .map(|(i, s)| Page { src: Src::Pdf { path: path.to_path_buf(), index: i as u32, size: *s }, rot: 0, img: None, ppx: None })
            .collect())
    } else if crate::kinds::kind_of(path) == Some(crate::kinds::Kind::Doc) {
        let doc = Rc::new(crate::richdoc::RichDoc::open(path).map_err(|e| format!("{}:\n{e}", file_name(path)))?);
        Ok((0..doc.pages() as u32).map(|i| Page { src: Src::Rich { path: path.to_path_buf(), doc: doc.clone(), index: i }, rot: 0, img: None, ppx: None }).collect())
    } else if crate::files::is_image(path) {
        Ok(vec![Page::file(path.to_path_buf())])
    } else {
        Err(format!("{}:\nформат не поддерживается", file_name(path)))
    }
}

/// Страницы из нескольких файлов; ошибки собираются отдельно.
pub fn pages_from_all(paths: &[PathBuf]) -> (Vec<Page>, Vec<String>) {
    let mut pages = vec![];
    let mut errs = vec![];
    for p in paths {
        match pages_from(p) {
            Ok(v) => pages.extend(v),
            Err(e) => errs.push(e),
        }
    }
    (pages, errs)
}

/// Нарисовать страницу. dpi — для PDF.
pub fn render(page: &Page, dpi: f64) -> Result<Rc<Img>, String> {
    if let Some(i) = &page.img {
        return Ok(i.clone());
    }
    let img = match &page.src {
        Src::File(p) => crate::codec::load(p).map_err(|e| format!("{}:\n{}", file_name(p), e.message()))?,
        Src::Pdf { path, index, size } => {
            let k = (dpi / 72.0).min(MAX_SIDE / size.0.max(size.1).max(1.0));
            let w = (size.0 * k).round().max(16.0) as u32;
            crate::pdf_read::render(path, *index, w).map_err(|e| format!("{}, стр. {}:\n{e}", file_name(path), index + 1))?
        }
        Src::Mem(_) => return Err("пустая страница".into()),
        Src::Rich { doc, index, .. } => {
            let i = doc.render(*index, dpi.min(300.0))?;
            if page.rot % 4 == 0 {
                return Ok(i);
            }
            (*i).clone()
        }
    };
    Ok(Rc::new(rot_q(img, page.rot)))
}

/// Подготовить страницы к записи в PDF.
pub fn to_out(pages: &[Page], photo: PhotoPage, margin_mm: f64) -> Result<Vec<OutPage>, String> {
    // какие PDF lopdf умеет открыть (защищённые паролем/битые — растрируем)
    let mut ok: HashMap<PathBuf, bool> = HashMap::new();
    let mut out = Vec::with_capacity(pages.len());
    for pg in pages {
        if let (true, Src::Pdf { path, index, .. }) = (pg.is_vector(), &pg.src) {
            let good = *ok.entry(path.clone()).or_insert_with(|| pdf_write::page_count(path).is_ok());
            if good {
                out.push(OutPage::Vector { path: path.clone(), index: *index, rot: pg.rot });
                continue;
            }
        }
        let img = render(pg, VIEW_DPI)?;
        let jpeg = crate::codec::encode_jpeg(&img).map_err(|e| e.message().to_string())?;
        let px = (img.w, img.h);
        let size = pg.size_pt().or_else(|| {
            // растр из PDF без ppx (защищённый файл): лист по исходному размеру
            if let Src::Pdf { size, .. } = &pg.src {
                Some(if pg.rot % 2 == 1 { (size.1, size.0) } else { *size })
            } else {
                None
            }
        });
        let (page, place) = match size {
            Some(s) => (s, (0.0, 0.0, s.0, s.1)),
            None => pdf_write::photo_layout(px, photo, margin_mm),
        };
        out.push(OutPage::Raster { page, items: vec![Placed { jpeg, px, place, angle: 0.0 }] });
    }
    Ok(out)
}

/// Сохранить страницы в PDF с текущими настройками «фото на странице».
pub fn save_pdf(pages: &[Page], path: &Path) -> Result<(), String> {
    let photo = if crate::settings::get_i("pdf_photo", 0) == 1 { PhotoPage::Fit } else { PhotoPage::A4 };
    let margin = crate::settings::get_i("pdf_margin", 10) as f64;
    let out = to_out(pages, photo, margin)?;
    // файл мог быть открыт системным просмотрщиком PDF — отпустим его
    crate::pdf_read::forget();
    pdf_write::write(&out, path)
}
