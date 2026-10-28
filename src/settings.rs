// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Настройки: простой файл «ключ=значение» в %APPDATA%\SimplePhotoViewer\settings.ini.

use std::cell::RefCell;
use std::collections::BTreeMap;

thread_local! { static S: RefCell<Option<BTreeMap<String, String>>> = const { RefCell::new(None) }; }

fn with<R>(f: impl FnOnce(&mut BTreeMap<String, String>) -> R) -> R {
    S.with(|c| {
        let mut b = c.borrow_mut();
        let m = b.get_or_insert_with(load);
        f(m)
    })
}

fn load() -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    if let Some(d) = crate::util::app_dir() {
        if let Ok(s) = std::fs::read_to_string(d.join("settings.ini")) {
            for line in s.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    m.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
        }
    }
    m
}

pub fn save() {
    let text = with(|m| m.iter().map(|(k, v)| format!("{k}={v}\n")).collect::<String>());
    if let Some(d) = crate::util::app_dir() {
        let _ = std::fs::write(d.join("settings.ini"), text);
    }
}

pub fn get_i(key: &str, def: i32) -> i32 {
    with(|m| m.get(key).and_then(|v| v.parse().ok()).unwrap_or(def))
}

pub fn set_i(key: &str, v: i32) {
    with(|m| m.insert(key.to_string(), v.to_string()));
}

pub fn get_s(key: &str) -> Option<String> {
    with(|m| m.get(key).cloned())
}

pub fn set_s(key: &str, v: &str) {
    with(|m| m.insert(key.to_string(), v.replace(['\r', '\n'], " ")));
}
