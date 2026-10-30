// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Сборка PDF: склейка страниц из разных PDF (без потери качества — страницы
//! переносятся как есть, текст остаётся текстом) и вставка фото страницами.
//! Библиотека lopdf (MIT). Чистая логика без Win32 — покрыта тестами.

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

pub const A4: (f64, f64) = (595.276, 841.89);
pub const MM: f64 = 72.0 / 25.4;

/// Страница будущего PDF.
pub enum OutPage {
    /// Страница существующего PDF (номер с нуля) + доп. поворот четвертями по часовой.
    Vector { path: PathBuf, index: u32, rot: u8 },
    /// Лист размером `page` (пункты) с картинками на нём.
    Raster { page: (f64, f64), items: Vec<Placed> },
}

/// Картинка (уже в JPEG) и её место на листе в пунктах: (x, y, w, h), y — снизу.
pub struct Placed {
    pub jpeg: Vec<u8>,
    pub px: (u32, u32),
    pub place: (f64, f64, f64, f64),
    /// Поворот вокруг центра, градусы по часовой (как видно на листе).
    pub angle: f64,
}

/// Как класть фото на страницу PDF.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PhotoPage {
    /// Лист A4, ориентация по снимку, фото вписано с полями.
    A4,
    /// Лист ровно по размеру фото (150 точек на дюйм).
    Fit,
}

/// Размер листа и место фото на нём.
pub fn photo_layout(px: (u32, u32), mode: PhotoPage, margin_mm: f64) -> ((f64, f64), (f64, f64, f64, f64)) {
    let (w, h) = (px.0.max(1) as f64, px.1.max(1) as f64);
    match mode {
        PhotoPage::Fit => {
            let k = 72.0 / 150.0;
            let page = (w * k, h * k);
            (page, (0.0, 0.0, page.0, page.1))
        }
        PhotoPage::A4 => {
            let page = if w > h { (A4.1, A4.0) } else { A4 };
            let m = (margin_mm * MM).min(page.0 / 3.0);
            let (aw, ah) = (page.0 - 2.0 * m, page.1 - 2.0 * m);
            let s = (aw / w).min(ah / h);
            let (dw, dh) = (w * s, h * s);
            (page, ((page.0 - dw) / 2.0, (page.1 - dh) / 2.0, dw, dh))
        }
    }
}

/// Унаследованный от родительских узлов атрибут страницы (Resources, MediaBox…).
fn inherited(doc: &Document, page: ObjectId, key: &[u8]) -> Option<Object> {
    let mut cur = doc.get_dictionary(page).ok()?.get(b"Parent").ok()?.as_reference().ok()?;
    for _ in 0..64 {
        let d = doc.get_dictionary(cur).ok()?;
        if let Ok(v) = d.get(key) {
            return Some(v.clone());
        }
        cur = d.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

fn fname(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

/// Число страниц PDF (через lopdf — запасной путь, если Windows.Data.Pdf недоступен).
pub fn page_count(path: &Path) -> Result<u32, String> {
    let d = Document::load(path).map_err(|e| format!("{}: {e}", fname(path)))?;
    Ok(d.get_pages().len() as u32)
}

/// Размеры страниц (пункты, с учётом /Rotate) через lopdf.
pub fn page_sizes(path: &Path) -> Result<Vec<(f64, f64)>, String> {
    let d = Document::load(path).map_err(|e| format!("{}: {e}", fname(path)))?;
    let mut v = vec![];
    for (_, id) in d.get_pages() {
        let get = |k: &[u8]| d.get_dictionary(id).ok().and_then(|p| p.get(k).ok().cloned()).or_else(|| inherited(&d, id, k));
        let bx = get(b"CropBox").or_else(|| get(b"MediaBox"));
        let (w, h) = bx
            .and_then(|o| o.as_array().ok().cloned())
            .and_then(|a| {
                let f: Vec<f64> = a.iter().filter_map(|x| x.as_float().ok().map(|v| v as f64)).collect();
                (f.len() == 4).then(|| ((f[2] - f[0]).abs(), (f[3] - f[1]).abs()))
            })
            .unwrap_or(A4);
        let rot = get(b"Rotate").and_then(|o| o.as_i64().ok()).unwrap_or(0).rem_euclid(360);
        v.push(if rot == 90 || rot == 270 { (h, w) } else { (w, h) });
    }
    if v.is_empty() {
        return Err("В PDF нет страниц".into());
    }
    Ok(v)
}

/// Собрать PDF из страниц и записать в `out` (через временный файл).
pub fn write(pages: &[OutPage], out: &Path) -> Result<(), String> {
    if pages.is_empty() {
        return Err("Нет страниц для сохранения".into());
    }
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let mut srcs: HashMap<PathBuf, BTreeMap<u32, ObjectId>> = HashMap::new();
    let mut kids: Vec<Object> = vec![];

    for pg in pages {
        match pg {
            OutPage::Vector { path, index, rot } => {
                if !srcs.contains_key(path) {
                    let mut src = Document::load(path).map_err(|e| format!("{}: {e}", fname(path)))?;
                    src.renumber_objects_with(doc.max_id + 1);
                    let map = src.get_pages();
                    doc.max_id = doc.max_id.max(src.max_id);
                    doc.objects.extend(src.objects);
                    srcs.insert(path.clone(), map);
                }
                let pid = *srcs[path]
                    .get(&(index + 1))
                    .ok_or_else(|| format!("{}: нет страницы {}", fname(path), index + 1))?;
                let mut d: Dictionary = doc.get_dictionary(pid).map_err(|e| e.to_string())?.clone();
                for key in [&b"Resources"[..], b"MediaBox", b"CropBox", b"Rotate"] {
                    if !d.has(key) {
                        if let Some(v) = inherited(&doc, pid, key) {
                            d.set(key.to_vec(), v);
                        }
                    }
                }
                if !d.has(b"MediaBox") {
                    d.set("MediaBox", vec![0.into(), 0.into(), (A4.0 as f32).into(), (A4.1 as f32).into()]);
                }
                if !d.has(b"Resources") {
                    d.set("Resources", Dictionary::new());
                }
                let base = d.get(b"Rotate").ok().and_then(|o| o.as_i64().ok()).unwrap_or(0);
                let r = (base + 90 * *rot as i64).rem_euclid(360) / 90 * 90;
                d.set("Rotate", r);
                d.set("Parent", pages_id);
                kids.push(doc.add_object(d).into());
            }
            OutPage::Raster { page, items } => {
                let mut xobj = Dictionary::new();
                let mut content = String::new();
                for (i, it) in items.iter().enumerate() {
                    let img = Stream::new(
                        dictionary! {
                            "Type" => "XObject",
                            "Subtype" => "Image",
                            "Width" => it.px.0 as i64,
                            "Height" => it.px.1 as i64,
                            "ColorSpace" => "DeviceRGB",
                            "BitsPerComponent" => 8,
                            "Filter" => "DCTDecode",
                        },
                        it.jpeg.clone(),
                    )
                    .with_compression(false);
                    let img_id = doc.add_object(img);
                    xobj.set(format!("Im{i}").into_bytes(), img_id);
                    let (x, y, w, h) = it.place;
                    if it.angle.abs() < 1e-6 {
                        content += &format!("q {w:.3} 0 0 {h:.3} {x:.3} {y:.3} cm /Im{i} Do Q\n");
                    } else {
                        // в PDF ось y вверх: поворот «по часовой на листе» = −angle
                        let a = (-it.angle).to_radians();
                        let (c, sn) = (a.cos(), a.sin());
                        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
                        content += &format!(
                            "q 1 0 0 1 {cx:.3} {cy:.3} cm {c:.6} {sn:.6} {:.6} {c:.6} 0 0 cm {w:.3} 0 0 {h:.3} {:.3} {:.3} cm /Im{i} Do Q\n",
                            -sn, -w / 2.0, -h / 2.0
                        );
                    }
                }
                let c_id = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
                let p = dictionary! {
                    "Type" => "Page",
                    "Parent" => pages_id,
                    "MediaBox" => vec![0.into(), 0.into(), (page.0 as f32).into(), (page.1 as f32).into()],
                    "Resources" => dictionary! { "XObject" => xobj },
                    "Contents" => c_id,
                };
                kids.push(doc.add_object(p).into());
            }
        }
    }

    let count = kids.len() as i64;
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }));
    let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    let info = doc.add_object(dictionary! {
        "Producer" => Object::string_literal(format!("SimplePhotoViewer {}", env!("CARGO_PKG_VERSION"))),
    });
    doc.trailer = Dictionary::new();
    doc.trailer.set("Root", cat);
    doc.trailer.set("Info", info);
    doc.prune_objects();
    doc.compress();

    let tmp = out.with_extension("spv-tmp");
    let res = (|| -> Result<(), String> {
        let f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut w = std::io::BufWriter::new(f);
        doc.save_to(&mut w).map_err(|e| e.to_string())?;
        use std::io::Write;
        w.flush().map_err(|e| e.to_string())?;
        drop(w);
        std::fs::rename(&tmp, out).map_err(|e| e.to_string())
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    // Минимальный корректный JPEG 1×1 (белый пиксель).
    const JPEG_1X1: &[u8] = &[
        0xFF, 0xD8, 0xFF, 0xDB, 0x00, 0x43, 0x00, 0x08, 0x06, 0x06, 0x07, 0x06, 0x05, 0x08, 0x07, 0x07, 0x07, 0x09, 0x09,
        0x08, 0x0A, 0x0C, 0x14, 0x0D, 0x0C, 0x0B, 0x0B, 0x0C, 0x19, 0x12, 0x13, 0x0F, 0x14, 0x1D, 0x1A, 0x1F, 0x1E, 0x1D,
        0x1A, 0x1C, 0x1C, 0x20, 0x24, 0x2E, 0x27, 0x20, 0x22, 0x2C, 0x23, 0x1C, 0x1C, 0x28, 0x37, 0x29, 0x2C, 0x30, 0x31,
        0x34, 0x34, 0x34, 0x1F, 0x27, 0x39, 0x3D, 0x38, 0x32, 0x3C, 0x2E, 0x33, 0x34, 0x32, 0xFF, 0xC0, 0x00, 0x0B, 0x08,
        0x00, 0x01, 0x00, 0x01, 0x01, 0x01, 0x11, 0x00, 0xFF, 0xC4, 0x00, 0x14, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x09, 0xFF, 0xC4, 0x00, 0x14, 0x10, 0x01, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF, 0xDA, 0x00, 0x08,
        0x01, 0x01, 0x00, 0x00, 0x3F, 0x00, 0x2A, 0x9F, 0xFF, 0xD9,
    ];

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("spv_test_{}_{name}", std::process::id()))
    }

    fn raster(mode: PhotoPage) -> OutPage {
        let (page, place) = photo_layout((1, 1), mode, 10.0);
        OutPage::Raster { page, items: vec![Placed { jpeg: JPEG_1X1.to_vec(), px: (1, 1), place, angle: 0.0 }] }
    }

    #[test]
    fn layout_a4() {
        let (page, (x, y, w, h)) = photo_layout((4000, 3000), PhotoPage::A4, 10.0);
        assert!(page.0 > page.1, "альбомный лист для широкого фото");
        assert!((w / h - 4.0 / 3.0).abs() < 1e-6);
        assert!(x >= 10.0 * MM - 1e-6 && y >= 10.0 * MM - 1e-6);
        assert!((x * 2.0 + w - page.0).abs() < 1e-6);
    }

    #[test]
    fn photos_then_merge() {
        let a = tmp("a.pdf");
        let b = tmp("b.pdf");
        let m = tmp("m.pdf");
        write(&[raster(PhotoPage::A4), raster(PhotoPage::Fit)], &a).unwrap();
        write(&[raster(PhotoPage::A4)], &b).unwrap();
        assert_eq!(page_count(&a).unwrap(), 2);
        // склейка: b целиком, затем 2-я и 1-я страницы a (вторая — с поворотом)
        write(
            &[
                OutPage::Vector { path: b.clone(), index: 0, rot: 0 },
                OutPage::Vector { path: a.clone(), index: 1, rot: 1 },
                OutPage::Vector { path: a.clone(), index: 0, rot: 3 },
                OutPage::Raster {
                    page: A4,
                    items: vec![
                        Placed { jpeg: JPEG_1X1.to_vec(), px: (1, 1), place: (10.0, 10.0, 100.0, 100.0), angle: 0.0 },
                        Placed { jpeg: JPEG_1X1.to_vec(), px: (1, 1), place: (200.0, 10.0, 100.0, 100.0), angle: 30.0 },
                    ],
                },
            ],
            &m,
        )
        .unwrap();
        let d = Document::load(&m).unwrap();
        let pages = d.get_pages();
        assert_eq!(pages.len(), 4);
        let sz = page_sizes(&m).unwrap();
        assert!((sz[1].0 - 0.48).abs() < 1e-3, "поворот 90° меняет местами стороны");
        let rot = |n: u32| d.get_dictionary(pages[&n]).unwrap().get(b"Rotate").unwrap().as_i64().unwrap();
        assert_eq!((rot(1), rot(2), rot(3)), (0, 90, 270));
        // Fit-страница a[1] имеет размер 1px*72/150
        let mb = d.get_dictionary(pages[&2]).unwrap().get(b"MediaBox").unwrap().as_array().unwrap().clone();
        assert!((mb[2].as_float().unwrap() - 0.48).abs() < 1e-3);
        // перезапись файла, который сам является источником
        write(&[OutPage::Vector { path: m.clone(), index: 3, rot: 0 }], &m).unwrap();
        assert_eq!(page_count(&m).unwrap(), 1);
        for p in [a, b, m] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn unicode_name() {
        let m = tmp("Коллаж (печать).pdf");
        write(&[raster(PhotoPage::A4)], &m).unwrap();
        assert_eq!(page_count(&m).unwrap(), 1);
        let _ = std::fs::remove_file(m);
    }

    #[test]
    fn bad_source() {
        let m = tmp("bad.pdf");
        let e = write(&[OutPage::Vector { path: tmp("нет.pdf"), index: 0, rot: 0 }], &m);
        assert!(e.is_err());
        assert!(!m.exists());
    }
}
