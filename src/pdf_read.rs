// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Просмотр PDF встроенным в Windows 10/11 модулем Windows.Data.Pdf
//! (тот же, что использовал старый Edge). В exe это не добавляет ни одного
//! килобайта движка: страница рисуется системой в картинку, дальше — как фото.
//!
//! Работа идёт в отдельном фоновом потоке (MTA): так асинхронные вызовы WinRT
//! можно спокойно дожидаться, а открытый документ кешируется между страницами.

use crate::image_buf::Img;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};
use windows::core::HSTRING;
use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use windows::Storage::StorageFile;
use windows::Storage::Streams::{DataReader, InMemoryRandomAccessStream};
use windows::Win32::Graphics::Imaging::GUID_ContainerFormatBmp;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

/// Сведения о PDF: размеры страниц в пунктах (1/72 дюйма), с учётом поворота.
#[derive(Clone, Debug)]
pub struct PdfInfo {
    pub pages: Vec<(f64, f64)>,
}

/// Открытые документы (последний — в конце). Несколько: соседние файлы
/// для «подглядывания» не должны выталкивать открытую книгу.
struct Cache {
    docs: Vec<(PathBuf, PdfDocument)>,
}

type Job = Box<dyn FnOnce(&mut Cache) + Send>;

fn worker() -> &'static Mutex<Sender<Job>> {
    static W: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();
    W.get_or_init(|| {
        let (tx, rx) = channel::<Job>();
        std::thread::spawn(move || {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            let mut c = Cache { docs: vec![] };
            while let Ok(job) = rx.recv() {
                job(&mut c);
            }
        });
        Mutex::new(tx)
    })
}

fn call<T: Send + 'static>(f: impl FnOnce(&mut Cache) -> Result<T, String> + Send + 'static) -> Result<T, String> {
    let (rtx, rrx) = channel();
    let job: Job = Box::new(move |c| {
        let _ = rtx.send(f(c));
    });
    worker().lock().map_err(|_| "поток PDF недоступен".to_string())?.send(job).map_err(|e| e.to_string())?;
    rrx.recv().map_err(|e| e.to_string())?
}

fn err(e: windows::core::Error) -> String {
    // класс WinRT не найден: система без Windows.Data.Pdf (Windows 7/8, урезанные сборки)
    if matches!(e.code().0 as u32, 0x8004_0111 | 0x8004_0154 | 0x8007_007E) {
        return "Просмотр PDF недоступен: нужен компонент Windows 10/11 «Windows.Data.Pdf».\n\
                Склеивать и сохранять такие PDF всё равно можно."
            .into();
    }
    let m = e.message().trim().to_string();
    if m.is_empty() { format!("{e}") } else { m }
}

fn open_doc<'a>(c: &'a mut Cache, path: &Path) -> Result<&'a PdfDocument, String> {
    if let Some(i) = c.docs.iter().position(|d| d.0 == path) {
        let d = c.docs.remove(i);
        c.docs.push(d);
    } else {
        let full = std::path::absolute(path).map_err(|e| e.to_string())?;
        let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(full.as_os_str()))
            .and_then(|op| op.get())
            .map_err(err)?;
        let doc = PdfDocument::LoadFromFileAsync(&file).and_then(|op| op.get()).map_err(|e| {
            let m = err(e);
            if m.to_lowercase().contains("password") || m.contains("парол") {
                "PDF защищён паролем — такие файлы пока не поддерживаются".to_string()
            } else {
                m
            }
        })?;
        if c.docs.len() >= 3 {
            c.docs.remove(0);
        }
        c.docs.push((path.to_path_buf(), doc));
    }
    Ok(&c.docs.last().unwrap().1)
}

/// Открыть PDF: количество и размеры страниц.
pub fn info(path: &Path) -> Result<PdfInfo, String> {
    let path = path.to_path_buf();
    call(move |c| {
        let doc = open_doc(c, &path)?;
        let n = doc.PageCount().map_err(err)?;
        let mut pages = Vec::with_capacity(n as usize);
        for i in 0..n {
            let p = doc.GetPage(i).map_err(err)?;
            let s = p.Size().map_err(err)?; // в DIP (1/96 дюйма)
            pages.push((s.Width as f64 * 72.0 / 96.0, s.Height as f64 * 72.0 / 96.0));
        }
        if pages.is_empty() {
            return Err("В PDF нет страниц".into());
        }
        Ok(PdfInfo { pages })
    })
}

/// Нарисовать страницу шириной `width` пикселей (высота — по пропорциям).
pub fn render(path: &Path, index: u32, width: u32) -> Result<Img, String> {
    let path = path.to_path_buf();
    if let Some(ms) = fake_delay() {
        return call(move |_| Ok(fake_page(index, width, ms)));
    }
    let bytes = call(move |c| {
        let doc = open_doc(c, &path)?;
        let page = doc.GetPage(index).map_err(err)?;
        let run = |bmp: bool| -> windows::core::Result<Vec<u8>> {
            let stream = InMemoryRandomAccessStream::new()?;
            let opts = PdfPageRenderOptions::new()?;
            opts.SetDestinationWidth(width.max(16))?;
            if bmp {
                // BMP быстрее PNG: не тратим время на сжатие
                opts.SetBitmapEncoderId(GUID_ContainerFormatBmp)?;
            }
            page.RenderWithOptionsToStreamAsync(&stream, &opts)?.get()?;
            let size = stream.Size()? as u32;
            let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
            reader.LoadAsync(size)?.get()?;
            let mut buf = vec![0u8; size as usize];
            reader.ReadBytes(&mut buf)?;
            Ok(buf)
        };
        run(true).or_else(|_| run(false)).map_err(err)
    })?;
    crate::codec::load_bytes(&bytes).map_err(err)
}

/// Забыть открытый документ (например, перед перезаписью файла).
pub fn forget() {
    let _ = call(|c| {
        c.docs.clear();
        Ok(())
    });
}

/// Для замеров без Windows.Data.Pdf (Wine): SPV_FAKE_PDF=<мс на 1000 точек ширины>
/// — вместо отрисовки серая «страница» с полосами и задержкой, как у настоящей.
fn fake_delay() -> Option<u64> {
    static D: OnceLock<Option<u64>> = OnceLock::new();
    *D.get_or_init(|| std::env::var("SPV_FAKE_PDF").ok().and_then(|v| v.parse().ok()))
}

fn fake_page(index: u32, width: u32, ms: u64) -> Img {
    let w = width.max(16);
    let h = (w as f64 * 1.414) as u32;
    std::thread::sleep(std::time::Duration::from_millis(ms * w as u64 * h as u64 / 1_414_000));
    let mut img = Img::filled(w, h, [255, 255, 255]);
    // строки «текста» и номер страницы полосками сверху
    let line = (h / 60).max(1);
    for y in 0..h {
        let row = y / line;
        let ink = row > 6 && row % 2 == 0 && row < 56;
        let head = row < 5 && (y * 8 / h.max(1)) == 0;
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let m = x > w / 10 && x < w * 9 / 10;
            if (ink && m && (x / (line * 3) + row) % 7 != 0) || (head && x > w / 10 && x < w / 10 + (index % 20 + 1) * w / 30) {
                img.px[i] = 60;
                img.px[i + 1] = 60;
                img.px[i + 2] = 60;
            }
        }
    }
    img
}
