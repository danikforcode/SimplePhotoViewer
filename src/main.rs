// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
#![windows_subsystem = "windows"]

mod clip;
mod codec;
mod collage;
mod doc;
mod files;
mod gui;
mod image_buf;
mod merge;
mod pdf_read;
mod pdf_write;
mod print_dlg;
mod settings;
mod theme;
mod ui;
mod util;

fn main() {
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("{info}");
        if let Some(dir) = util::app_dir() {
            let _ = std::fs::write(dir.join("error.log"), &msg);
        }
        ui::message(None, &format!("Критическая ошибка:\n{msg}"), true);
    }));
    codec::init();
    let args: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(std::path::PathBuf::from).collect();
    ui::run(args);
}
