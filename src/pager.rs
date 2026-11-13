// SimplePhotoViewer (Rust/Win32) — © 2026 danikforcode
// Лицензия MIT (см. LICENSE). Распространяется «как есть», без гарантий.
//! Фоновая отрисовка страниц (PDF и фото-страницы), чтобы быстрая прокрутка
//! и листание не останавливали окно. Очередь «последний запрос — первым»:
//! при быстрой прокрутке рисуются только те страницы, что видны сейчас,
//! а промелькнувшие отбрасываются. Готовое приходит сообщением WM_PAGE.

use crate::image_buf::Img;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

pub const WM_PAGE: u32 = 0x8000 + 25;
/// Сколько запросов держать: больше — уже не видно.
const QUEUE_MAX: usize = 16;

/// Что нарисовать (только данные, которые можно передать в другой поток).
#[derive(Clone, PartialEq, Debug)]
pub enum Job {
    Pdf { path: PathBuf, index: u32, width: u32 },
    File { path: PathBuf, max: u32 },
}

#[derive(Clone)]
struct Req {
    gen: u64,
    page: usize,
    key: u32,
    rot: u8,
    job: Job,
}

pub struct Done {
    pub gen: u64,
    pub page: usize,
    /// Качество (для PDF — точек по ширине, для фото — длинная сторона; 0 — полное).
    pub key: u32,
    pub img: Result<Img, String>,
}

struct Queue {
    q: VecDeque<Req>,
    /// Что рисуется прямо сейчас (чтобы не ставить второй раз).
    busy: Vec<(u64, usize, u32)>,
    stop: bool,
}

pub struct Pager {
    shared: Arc<(Mutex<Queue>, Condvar)>,
    rx: Receiver<Done>,
    pub gen: u64,
}

impl Pager {
    pub fn new(hwnd: HWND) -> Pager {
        let shared = Arc::new((Mutex::new(Queue { q: VecDeque::new(), busy: vec![], stop: false }), Condvar::new()));
        let (tx, rx) = channel::<Done>();
        let h = hwnd.0 as isize;
        // два потока: PDF рисует системный модуль (свой поток внутри), фото — WIC
        for _ in 0..2 {
            let sh = shared.clone();
            let tx: Sender<Done> = tx.clone();
            std::thread::spawn(move || {
                unsafe {
                    let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
                }
                loop {
                    let req = {
                        let (m, cv) = &*sh;
                        let mut g = m.lock().unwrap();
                        while g.q.is_empty() && !g.stop {
                            g = cv.wait(g).unwrap();
                        }
                        if g.stop {
                            return;
                        }
                        let r = g.q.pop_back().unwrap(); // самый свежий
                        g.busy.push((r.gen, r.page, r.key));
                        r
                    };
                    let img = match &req.job {
                        Job::Pdf { path, index, width } => crate::pdf_read::render(path, *index, *width),
                        Job::File { path, max } => crate::codec::load(path)
                            .map(|i| if *max > 0 && i.w.max(i.h) > *max { crate::codec::thumb(&i, *max) } else { i })
                            .map_err(|e| e.message().to_string()),
                    }
                    .map(|i| crate::doc::rot_q(i, req.rot));
                    sh.0.lock().unwrap().busy.retain(|b| *b != (req.gen, req.page, req.key));
                    if tx.send(Done { gen: req.gen, page: req.page, key: req.key, img }).is_err() {
                        return;
                    }
                    unsafe {
                        let _ = PostMessageW(HWND(h as *mut _), WM_PAGE, WPARAM(0), LPARAM(0));
                    }
                }
            });
        }
        Pager { shared, rx, gen: 1 }
    }

    /// Попросить страницу. Повторный запрос того же поднимает его в начало очереди.
    pub fn want(&self, page: usize, key: u32, rot: u8, job: Job) {
        let (m, cv) = &*self.shared;
        let mut g = m.lock().unwrap();
        if g.busy.contains(&(self.gen, page, key)) {
            return;
        }
        g.q.retain(|r| !(r.page == page && r.key == key && r.gen == self.gen));
        g.q.push_back(Req { gen: self.gen, page, key, rot, job });
        // старые (давно промелькнувшие) — выбросить
        while g.q.len() > QUEUE_MAX {
            g.q.pop_front();
        }
        cv.notify_one();
    }

    /// Документ сменился: всё, что в очереди, больше не нужно.
    pub fn reset(&mut self) {
        self.gen += 1;
        let (m, _) = &*self.shared;
        m.lock().unwrap().q.clear();
    }

    pub fn poll(&self) -> Vec<Done> {
        let mut v = vec![];
        while let Ok(d) = self.rx.try_recv() {
            if d.gen == self.gen {
                v.push(d);
            }
        }
        v
    }
}

impl Drop for Pager {
    fn drop(&mut self) {
        let (m, cv) = &*self.shared;
        m.lock().unwrap().stop = true;
        cv.notify_all();
    }
}
