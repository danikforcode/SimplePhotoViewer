// SimplePhotoViewer (Rust/Win32) — © 2026 danikforcode
// Лицензия MIT (см. LICENSE). Распространяется «как есть», без гарантий.
#![windows_subsystem = "windows"]

mod clip;
mod codec;
mod bench;
mod assoc;
mod avif;
mod canvas;
mod collage;
mod convert;
mod convert_dlg;
mod doc;
mod dialogs;
mod draw;
mod extcodec;
mod heif;
mod edit;
mod fields;
mod files;
mod gui;
mod highlight;
mod image_buf;
mod table;
mod textview;
mod resize_dlg;
mod richdoc;
mod rtf;
mod textenc;
mod xmltree;
mod imgfmt;
mod kinds;
mod media;
mod mp4;
mod merge;
mod pager;
mod peek;
mod pdf_read;
mod pdf_write;
mod prefetch;
mod print_dlg;
mod settings;
mod gpuvideo;
mod softvideo;
mod theme;
mod ui;
mod util;

fn main() {
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("{info}");
        if let Some(dir) = util::app_dir() {
            let _ = std::fs::write(dir.join("error.log"), &msg);
        }
        // при сбое — системное окно: своё рисование может быть уже неисправно
        unsafe {
            use windows::core::HSTRING;
            use windows::Win32::UI::WindowsAndMessaging::*;
            MessageBoxW(None, &HSTRING::from(format!("Критическая ошибка:\n{msg}")), &HSTRING::from("SimplePhotoViewer"), MB_OK | MB_ICONERROR);
        }
    }));
    codec::init();
    let args: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(std::path::PathBuf::from).collect();
    // замеры и нагрузочная проверка — без окна
    match args.first().and_then(|a| a.to_str()) {
        Some("--bench") => return bench::bench(args.get(1).cloned()),
        Some("--probe") => return bench::probe(&args[1..]),
        Some("--thumb") => return assoc::test_thumbs(&args[1..]),
        Some("--stress") => {
            let n = args.get(1).and_then(|a| a.to_str()).and_then(|s| s.parse().ok());
            let out = if n.is_some() { args.get(2) } else { args.get(1) };
            return bench::stress(n.unwrap_or(1000), out.cloned());
        }
        _ => {}
    }
    ui::run(args);
    gui::trace::report();
}
