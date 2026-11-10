// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Замеры и нагрузочная проверка без окна:
//!   SimplePhotoViewer.exe --bench  [отчёт.txt]  — сколько длятся тяжёлые операции;
//!   SimplePhotoViewer.exe --stress [циклов] [отчёт.txt] — тысячи циклов
//!     «открыть → править → сохранить → закрыть»; следим, чтобы память
//!     и объекты GDI/USER не росли (утечки).
//! Отчёт пишется в файл (по умолчанию рядом с временными файлами) и в консоль,
//! если программа запущена из неё.

use crate::collage::Layout;
use crate::draw::{Shape, Tool};
use crate::edit::{self, Adjust, Fx, Sel};
use crate::image_buf::Img;
use crate::{codec, doc, highlight, table, textenc};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

struct Report {
    text: String,
}

impl Report {
    fn line(&mut self, s: impl AsRef<str>) {
        let s = s.as_ref();
        println!("{s}");
        let _ = writeln!(self.text, "{s}");
    }
}

/// Подключиться к консоли, из которой запустили (у оконной программы своей нет).
fn attach_console() {
    unsafe {
        let _ = windows::Win32::System::Console::AttachConsole(windows::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
}

/// Память процесса (МБ: занято своё, рабочий набор) и объекты GDI / USER.
pub fn resources() -> (f64, f64, u32, u32) {
    unsafe {
        use windows::Win32::System::ProcessStatus::*;
        use windows::Win32::System::Threading::*;
        let h = GetCurrentProcess();
        let mut c = PROCESS_MEMORY_COUNTERS_EX::default();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let _ = K32GetProcessMemoryInfo(h, &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS, c.cb);
        let gdi = GetGuiResources(h, GR_GDIOBJECTS);
        let user = GetGuiResources(h, GR_USEROBJECTS);
        // не все системы (Wine) считают «свою» память — тогда берём рабочий набор
        let own = if c.PrivateUsage > 0 { c.PrivateUsage } else { c.WorkingSetSize };
        (own as f64 / 1048576.0, c.WorkingSetSize as f64 / 1048576.0, gdi, user)
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Повторить f n раз: (лучшее, медиана) в мс.
fn time<T>(n: usize, mut f: impl FnMut() -> T) -> (f64, f64) {
    let mut v: Vec<f64> = (0..n.max(1))
        .map(|_| {
            let t = Instant::now();
            std::hint::black_box(f());
            ms(t.elapsed())
        })
        .collect();
    v.sort_by(|a, b| a.total_cmp(b));
    (v[0], v[v.len() / 2])
}

/// Фото-подобная картинка: плавные градиенты + шум (чтобы JPEG не был «пустым»).
pub fn synth(w: u32, h: u32, seed: u32) -> Img {
    let mut img = Img::filled(w, h, [0, 0, 0]);
    let mut r = seed.wrapping_mul(2654435761).wrapping_add(1);
    for y in 0..h {
        for x in 0..w {
            r ^= r << 13;
            r ^= r >> 17;
            r ^= r << 5;
            let n = (r & 15) as i32 - 8;
            let i = ((y * w + x) * 4) as usize;
            let fx = x as f32 / w as f32;
            let fy = y as f32 / h as f32;
            let v = |a: f32| (a * 255.0) as i32 + n;
            img.px[i] = v(fx).clamp(0, 255) as u8;
            img.px[i + 1] = v(fy).clamp(0, 255) as u8;
            img.px[i + 2] = v(((fx * 9.0 + seed as f32).sin() * 0.5 + 0.5) * (1.0 - fy)).clamp(0, 255) as u8;
            img.px[i + 3] = 255;
        }
    }
    img
}

fn tmp_dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("spv-bench-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&d);
    d
}

fn shapes(n: usize, w: f32, h: f32) -> Vec<Shape> {
    let tools = [Tool::Pen, Tool::Line, Tool::Arrow, Tool::Rect, Tool::Ellipse, Tool::Marker, Tool::Text];
    (0..n)
        .map(|i| {
            let t = tools[i % tools.len()];
            let (x, y) = ((i as f32 * 97.0) % w, (i as f32 * 61.0) % h);
            let mut s = Shape::new(t, (x, y), 0xE53935, 6.0, 255, i % 3 == 0);
            if t == Tool::Text {
                s.text = format!("Надпись {i}");
                s.width = 48.0;
            } else if t == Tool::Pen || t == Tool::Marker {
                for k in 1..40 {
                    s.extend((x + k as f32 * 7.0, y + ((k * 13) % 50) as f32), false);
                }
            } else {
                s.extend((x + 300.0, y + 200.0), false);
            }
            s
        })
        .collect()
}

fn csv_text(rows: usize) -> String {
    let mut s = String::from("id;name;date;sum;city\n");
    for i in 0..rows {
        let _ = writeln!(s, "{i};Имя {};{:02}.{:02}.20{:02};{},{:02};Город {}", i * 7919 % 100003, i % 28 + 1, i % 12 + 1, i % 30, (i * 37) % 100000, i % 100, i % 50);
    }
    s
}

fn code_text(bytes: usize) -> String {
    let unit = "fn main() {\n    // комментарий: проверка подсветки\n    let s = \"строка с \\\"кавычками\\\"\";\n    for i in 0..10 { println!(\"{} {}\", i, s); }\n    let x = 0x1F + 42 * 3.5e2;\n}\n\n";
    unit.repeat(bytes / unit.len() + 1)
}

pub fn bench(out: Option<PathBuf>) {
    attach_console();
    let mut r = Report { text: String::new() };
    let (p0, w0, g0, u0) = resources();
    r.line(format!("SimplePhotoViewer {} — замеры (мс: лучшее / медиана)", env!("CARGO_PKG_VERSION")));
    r.line(format!("Память в начале: {p0:.0} МБ своей, {w0:.0} МБ рабочий набор; GDI {g0}, USER {u0}"));
    r.line(format!("Потоков процессора: {}", std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)));
    r.line("");
    let dir = tmp_dir();
    let row = |r: &mut Report, name: &str, (best, med): (f64, f64)| r.line(format!("{name:<46} {best:>9.1} / {med:>9.1}"));

    // ---- фото 24 Мп
    let t = Instant::now();
    let big = synth(6000, 4000, 1);
    r.line(format!("(подготовка 24 Мп: {:.0} мс)", ms(t.elapsed())));
    let jpg = dir.join("big.jpg");
    let png = dir.join("big.png");
    row(&mut r, "JPEG 24 Мп: сохранить", time(3, || codec::save(&big, &jpg).unwrap()));
    row(&mut r, "PNG 24 Мп: сохранить", time(1, || codec::save(&big, &png).unwrap()));
    row(&mut r, "JPEG 24 Мп: открыть", time(5, || codec::load(&jpg).unwrap()));
    row(&mut r, "PNG 24 Мп: открыть", time(3, || codec::load(&png).unwrap()));
    row(&mut r, "Миниатюра 24 Мп → 1600", time(5, || codec::thumb(&big, 1600)));
    row(&mut r, "Размер 24 Мп → 1920×1280 (WIC)", time(5, || codec::resize(&big, 1920, 1280).unwrap()));
    row(&mut r, "Поворот 90° 24 Мп", time(5, || big.rotate_cw()));
    row(&mut r, "Обрезка 24 Мп → 4000×3000", time(5, || big.crop(1000, 500, 4000, 3000)));
    let mut adj = Adjust::default();
    adj.bright = 20.0;
    adj.contrast = 15.0;
    adj.saturation = 25.0;
    adj.warmth = 10.0;
    adj.vignette = 30.0;
    row(&mut r, "Настройки (5 ползунков) 24 Мп", time(3, || edit::apply_adjust(&big, &adj)));
    let proxy = codec::thumb(&big, 1400);
    row(&mut r, "Настройки на экранной копии 1400 (живой показ)", time(10, || edit::apply_adjust(&proxy, &adj)));
    row(&mut r, "Эффект «Сепия» 24 Мп", time(3, || edit::apply_fx(&big, Fx::Sepia)));
    row(&mut r, "Эффект «Размытие» 24 Мп", time(2, || edit::apply_fx(&big, Fx::Blur)));
    row(&mut r, "Эффект «Резкость» 24 Мп", time(2, || edit::apply_fx(&big, Fx::Sharpen)));
    row(&mut r, "Эффект «Пикселизация» 24 Мп", time(3, || edit::apply_fx(&big, Fx::Pixelate)));
    row(&mut r, "Волшебная палочка 24 Мп (связная)", time(3, || Sel::wand(&big, 3000, 2000, 30.0, true)));
    row(&mut r, "Волшебная палочка 24 Мп (по всему фото)", time(3, || Sel::wand(&big, 3000, 2000, 30.0, false)));
    let sel = Sel::ellipse(6000, 4000, (1000.0, 800.0), (5000.0, 3200.0));
    let fx = edit::apply_fx(&big, Fx::Gray);
    row(&mut r, "Эффект только в выделении 24 Мп", time(3, || edit::blend_mask(&big, &fx, &sel)));
    let sh = shapes(200, 6000.0, 4000.0);
    row(&mut r, "Рисунок: 200 фигур → в фото 24 Мп (GDI+)", time(3, || crate::draw::flatten(&big, &sh)));

    // ---- холст / коллаж
    let photos: Vec<Rc<Img>> = (0..8).map(|i| Rc::new(synth(3000, 2000 + (i % 2) * 1000, i + 2))).collect();
    let ink = shapes(50, 3840.0, 2160.0);
    row(&mut r, "Коллаж 8 фото по 6–9 Мп → 3840×2160 + 50 фигур", time(3, || crate::canvas::compose(&photos, (3840, 2160), Layout::Grid, 2, ink.clone()).unwrap()));
    row(&mut r, "Коллаж «крупное слева» → A4 300 dpi", time(3, || crate::canvas::compose(&photos[..5], (3508, 2480), Layout::BigLeft, 3, vec![]).unwrap()));

    // ---- PDF
    let pages: Vec<doc::Page> = (0..30).map(|i| doc::Page::mem("стр", synth(2480, 3508, 100 + i))).collect();
    let pdf = dir.join("book.pdf");
    row(&mut r, "PDF: 30 страниц A4 300 dpi из фото — собрать", time(1, || doc::save_pdf(&pages, &pdf).unwrap()));
    row(&mut r, "PDF: прочитать размеры страниц (lopdf)", time(5, || crate::pdf_write::page_sizes(&pdf).unwrap()));
    let merged = dir.join("merged.pdf");
    let vec_pages: Vec<doc::Page> = doc::pages_from(&pdf).unwrap_or_default();
    if !vec_pages.is_empty() {
        let twice: Vec<doc::Page> = vec_pages.iter().chain(vec_pages.iter()).cloned().collect();
        row(&mut r, "PDF: склеить 2 × 30 страниц (без перекодирования)", time(2, || doc::save_pdf(&twice, &merged).unwrap()));
    }
    match crate::pdf_read::render(&pdf, 0, 1654) {
        Ok(_) => row(&mut r, "PDF: нарисовать страницу A4, 200 dpi (Windows.Data.Pdf)", time(5, || crate::pdf_read::render(&pdf, 3, 1654).unwrap())),
        Err(e) => r.line(format!("{:<46} {}", "PDF: отрисовка страницы", e.lines().next().unwrap_or(""))),
    }

    // ---- таблицы, текст, код
    let csv = csv_text(100_000);
    r.line(format!("(таблица: 100 000 строк, {:.1} МБ)", csv.len() as f64 / 1048576.0));
    row(&mut r, "CSV 100 000 строк: разобрать", time(3, || table::parse_csv(&csv, None)));
    let sheet = table::parse_csv(&csv, None);
    row(&mut r, "Сортировка 100 000 строк по числу", time(5, || table::sorted_order(&sheet.rows, 1, 3, true)));
    row(&mut r, "Сортировка 100 000 строк по тексту", time(5, || table::sorted_order(&sheet.rows, 1, 1, false)));
    let code = code_text(3 * 1048576);
    let wide: Vec<u16> = code.encode_utf16().collect();
    row(&mut r, "Подсветка кода 3 МБ (Rust)", time(3, || highlight::highlight(&wide, highlight::Lang::Rust)));
    let bytes = textenc::encode(&code, textenc::Enc::Utf8);
    row(&mut r, "Определить кодировку и прочитать 3 МБ", time(5, || textenc::decode(&bytes)));
    let cp = textenc::encode(&code, textenc::Enc::Cp1251);
    row(&mut r, "То же, Windows-1251", time(5, || textenc::decode(&cp)));

    drop(big);
    drop(photos);
    drop(pages);
    let (p1, w1, g1, u1) = resources();
    r.line("");
    r.line(format!("Память в конце: {p1:.0} МБ своей, {w1:.0} МБ рабочий набор; GDI {g1}, USER {u1}"));
    let _ = std::fs::remove_dir_all(&dir);
    save_report(&r, out, "spv-bench.txt");
}

/// Нагрузочная проверка: циклы правки и сохранения, следим за ростом ресурсов.
pub fn stress(cycles: usize, out: Option<PathBuf>) {
    attach_console();
    let mut r = Report { text: String::new() };
    let dir = tmp_dir();
    r.line(format!("SimplePhotoViewer {} — нагрузочная проверка, циклов: {cycles}", env!("CARGO_PKG_VERSION")));
    let start = Instant::now();
    // разогрев: первые вызовы WIC/GDI+ создают кэши — их не считаем утечкой
    let _ = stress_cycle(&dir, 0);
    let base = resources();
    r.line(format!("После разогрева: {:.0} МБ своей памяти, GDI {}, USER {}", base.0, base.2, base.3));
    let mut peak = base.0;
    let mut errors = 0usize;
    let step = (cycles / 10).max(1);
    for i in 1..=cycles {
        if let Err(e) = stress_cycle(&dir, i) {
            errors += 1;
            if errors <= 5 {
                r.line(format!("  ошибка в цикле {i}: {e}"));
            }
        }
        let now = resources();
        peak = peak.max(now.0);
        if i % step == 0 || i == cycles {
            r.line(format!(
                "  цикл {i:>6}: {:>6.0} МБ своей (пик {:.0}), GDI {:>4}, USER {:>4}, прошло {:.1} с",
                now.0,
                peak,
                now.2,
                now.3,
                start.elapsed().as_secs_f64()
            ));
        }
    }
    let end = resources();
    r.line("");
    let dm = end.0 - base.0;
    let dg = end.2 as i64 - base.2 as i64;
    let du = end.3 as i64 - base.3 as i64;
    r.line(format!("Итог: ошибок {errors}; рост памяти {dm:+.1} МБ, GDI {dg:+}, USER {du:+}; {:.1} с", start.elapsed().as_secs_f64()));
    let ok = errors == 0 && dm < 64.0 && dg.abs() <= 4 && du.abs() <= 4;
    r.line(if ok { "Утечек не найдено." } else { "ВНИМАНИЕ: ресурсы растут или были ошибки — см. выше." });
    let _ = std::fs::remove_dir_all(&dir);
    save_report(&r, out, "spv-stress.txt");
    if !ok {
        std::process::exit(1);
    }
}

/// Один цикл «как пользователь»: открыть фото, поправить, порисовать,
/// сохранить в разные форматы, открыть снова, коллаж, PDF, таблица, текст.
fn stress_cycle(dir: &Path, i: usize) -> Result<(), String> {
    let w = 900 + (i % 7) as u32 * 113;
    let h = 700 + (i % 5) as u32 * 97;
    let img = synth(w, h, i as u32);
    let exts = ["jpg", "png", "bmp", "tif", "gif", "webp"];
    let ext = exts[i % exts.len()];
    let p = dir.join(format!("s{}.{ext}", i % 3));
    let mut a = Adjust::default();
    a.bright = (i % 40) as f64 - 20.0;
    a.sharpen = 30.0;
    let edited = edit::apply_adjust(&img, &a);
    let sel = Sel::rect(w, h, (10.0, 10.0), (w as f64 / 2.0, h as f64 / 2.0));
    let fx = edit::apply_fx(&edited, [Fx::Gray, Fx::Sepia, Fx::Invert, Fx::Pixelate, Fx::Blur, Fx::Sharpen][i % 6]);
    let mixed = edit::blend_mask(&edited, &fx, &sel);
    let drawn = crate::draw::flatten(&mixed, &shapes(12, w as f32, h as f32));
    let cropped = drawn.crop(5, 5, w - 10, h - 10).rotate_cw();
    // webp пишет не каждая система — тогда это не ошибка программы
    match codec::save(&cropped, &p) {
        Ok(()) => {
            let back = codec::load(&p).map_err(|e| format!("открыть {ext}: {}", e.message()))?;
            if (back.w, back.h) != (cropped.w, cropped.h) {
                return Err(format!("{ext}: размер {}×{} вместо {}×{}", back.w, back.h, cropped.w, cropped.h));
            }
        }
        Err(e) if ext == "webp" || ext == "gif" => {
            let _ = e;
        }
        Err(e) => return Err(format!("сохранить {ext}: {}", e.message())),
    }
    let _ = codec::thumb(&cropped, 300);
    let _ = codec::resize(&cropped, 320, 240).map_err(|e| e.message().to_string())?;
    if i % 5 == 0 {
        let photos: Vec<Rc<Img>> = (0..4).map(|k| Rc::new(synth(500 + k * 40, 400, (i + k as usize) as u32))).collect();
        let c = crate::canvas::compose(&photos, (1200, 800), [Layout::Grid, Layout::BigLeft, Layout::BigTop, Layout::Row, Layout::Column][i / 5 % 5], 2, shapes(5, 1200.0, 800.0))?;
        if (c.w, c.h) != (1200, 800) {
            return Err("коллаж: не тот размер".into());
        }
    }
    if i % 10 == 0 {
        let pdf = dir.join("s.pdf");
        let pages = vec![doc::Page::mem("a", img.clone()), doc::Page::mem("b", cropped)];
        doc::save_pdf(&pages, &pdf)?;
        let n = crate::pdf_write::page_sizes(&pdf)?.len();
        if n != 2 {
            return Err(format!("PDF: {n} страниц вместо 2"));
        }
        let csv = csv_text(2000);
        let s = table::parse_csv(&csv, None);
        let _ = table::sorted_order(&s.rows, 1, 3, i % 20 == 0);
        let code: Vec<u16> = code_text(20_000).encode_utf16().collect();
        let _ = highlight::highlight(&code, highlight::Lang::Rust);
    }
    Ok(())
}

fn save_report(r: &Report, out: Option<PathBuf>, default: &str) {
    let path = out.unwrap_or_else(|| std::env::temp_dir().join(default));
    match std::fs::write(&path, r.text.as_bytes()) {
        Ok(()) => println!("Отчёт: {}", path.display()),
        Err(e) => println!("Не удалось записать отчёт {}: {e}", path.display()),
    }
}
