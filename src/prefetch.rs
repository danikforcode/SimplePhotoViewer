// SimplePhotoViewer (Rust/Win32) — © 2026 danikforcode
// Лицензия MIT (см. LICENSE). Распространяется «как есть», без гарантий.
//! Предзагрузка соседних фото: пока смотрите одно, следующее и предыдущее
//! уже расшифровываются в фоне — переход ← → берёт готовую картинку.
//! Хранится 3 последних (предыдущее, текущее, следующее), не больше ~200 Мп.

use crate::image_buf::Img;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::SystemTime;

/// Метка файла: изменился — кэш не годится.
type Stamp = (u64, Option<SystemTime>);

fn stamp(p: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.len(), m.modified().ok()))
}

struct State {
    cache: Vec<(PathBuf, Stamp, Arc<Img>)>,
    queue: VecDeque<PathBuf>,
    busy: Option<PathBuf>,
}

const KEEP: usize = 3;
const MAX_PX: u64 = 200_000_000;
/// Слишком большие (панорамы на сотни мегапикселей) не держим.
const MAX_ONE: u64 = 80_000_000;

fn shared() -> &'static (Mutex<State>, Condvar) {
    static S: OnceLock<(Mutex<State>, Condvar)> = OnceLock::new();
    S.get_or_init(|| {
        std::thread::spawn(worker);
        (Mutex::new(State { cache: vec![], queue: VecDeque::new(), busy: None }), Condvar::new())
    })
}

fn worker() {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
    loop {
        let (m, cv) = shared();
        let p = {
            let mut g = m.lock().unwrap();
            while g.queue.is_empty() {
                g = cv.wait(g).unwrap();
            }
            let p = g.queue.pop_front().unwrap();
            g.busy = Some(p.clone());
            p
        };
        let st = stamp(&p);
        let img = crate::codec::load(&p).ok();
        let mut g = m.lock().unwrap();
        g.busy = None;
        if let (Some(img), Some(st)) = (img, st) {
            let px = img.w as u64 * img.h as u64;
            if px <= MAX_ONE {
                put(&mut g, p, st, Arc::new(img));
            }
        }
        cv.notify_all();
    }
}

fn put(g: &mut State, p: PathBuf, st: Stamp, img: Arc<Img>) {
    g.cache.retain(|c| c.0 != p);
    g.cache.push((p, st, img));
    while g.cache.len() > KEEP || g.cache.iter().map(|c| c.2.w as u64 * c.2.h as u64).sum::<u64>() > MAX_PX {
        if g.cache.len() <= 1 {
            break;
        }
        g.cache.remove(0);
    }
}

/// Попросить расшифровать (первые — важнее). Уже готовые и текущий в работе пропускаются.
pub fn want(paths: &[PathBuf]) {
    let (m, cv) = shared();
    let mut g = m.lock().unwrap();
    g.queue.clear();
    for p in paths {
        let ready = g.cache.iter().any(|c| &c.0 == p && stamp(p).as_ref() == Some(&c.1));
        if !ready && g.busy.as_ref() != Some(p) && crate::kinds::kind_of(p) == Some(crate::kinds::Kind::Image) {
            g.queue.push_back(p.clone());
        }
    }
    cv.notify_all();
}

/// Готовая картинка. Если она как раз расшифровывается — подождать (это быстрее, чем начинать заново).
pub fn take(p: &Path) -> Option<Img> {
    let (m, cv) = shared();
    let st = stamp(p)?;
    let mut g = m.lock().unwrap();
    while g.busy.as_deref() == Some(p) {
        g = cv.wait(g).unwrap();
    }
    let i = g.cache.iter().position(|c| c.0 == p)?;
    if g.cache[i].1 != st {
        g.cache.remove(i);
        return None;
    }
    // в конец — «свежее» (останется при вытеснении)
    let e = g.cache.remove(i);
    let img = (*e.2).clone();
    g.cache.push(e);
    Some(img)
}

/// Уже готово или как раз расшифровывается (тогда take() дождётся).
pub fn ready_or_busy(p: &Path) -> bool {
    let (m, _) = shared();
    let st = stamp(p);
    let g = m.lock().unwrap();
    g.busy.as_deref() == Some(p) || g.cache.iter().any(|c| c.0 == p && Some(&c.1) == st.as_ref())
}

/// Положить уже расшифрованное (открыли сами) — чтобы «назад» было мгновенным.
pub fn keep(p: &Path, img: &Img) {
    let Some(st) = stamp(p) else { return };
    if img.w as u64 * img.h as u64 > MAX_ONE {
        return;
    }
    let (m, _) = shared();
    let mut g = m.lock().unwrap();
    if g.cache.iter().any(|c| c.0 == p && c.1 == st) {
        return;
    }
    put(&mut g, p.to_path_buf(), st, Arc::new(img.clone()));
}
