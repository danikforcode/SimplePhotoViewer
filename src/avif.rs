// SimplePhotoViewer (Rust/Win32) — © 2026 danikforcode
// Лицензия MIT (см. LICENSE). Распространяется «как есть», без гарантий.
//! AVIF: контейнер HEIF (heif.rs) + сжатие AV1 (rav1d — перенос dav1d на Rust).
//! Поддержано: 8/10/12 бит, 4:0:0/4:2:0/4:2:2/4:4:4, сетки плиток,
//! прозрачность, поворот/отражение/обрезка (irot/imir/clap), матрицы BT.601/709/2020.

use crate::heif::{self, Heif, Prop};
use crate::image_buf::Img;
use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
use rav1d::include::dav1d::headers::{DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I420, DAV1D_PIXEL_LAYOUT_I422};
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::*;
use std::ptr::NonNull;

/// Одна расшифрованная плоскость Y/U/V в «как есть» значениях (0..2^bpc-1).
pub struct Planes {
    pub w: usize,
    pub h: usize,
    pub bpc: u32,
    /// 0 — 4:0:0, 1 — 4:2:0, 2 — 4:2:2, 3 — 4:4:4
    pub layout: u8,
    pub y: Vec<u16>,
    pub u: Vec<u16>,
    pub v: Vec<u16>,
    pub cw: usize,
    pub ch: usize,
    /// Матрица из потока (cicp) и полный диапазон.
    pub mtrx: u32,
    pub full: bool,
}

/// Расшифровать поток AV1 (OBU) одной картинки.
pub fn decode_av1(obus: &[u8]) -> Result<Planes, String> {
    unsafe {
        let mut s = std::mem::MaybeUninit::<Dav1dSettings>::uninit();
        dav1d_default_settings(NonNull::new(s.as_mut_ptr()).unwrap());
        let mut s = s.assume_init();
        s.n_threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(8) as i32;
        s.max_frame_delay = 1;
        let mut ctx: Option<Dav1dContext> = None;
        if dav1d_open(NonNull::new(&mut ctx), NonNull::new(&mut s)).0 != 0 {
            return Err("AV1: не удалось запустить декодер".into());
        }
        let res = (|| {
            let mut data: Dav1dData = std::mem::zeroed();
            let p = dav1d_data_create(NonNull::new(&mut data), obus.len());
            if p.is_null() {
                return Err("AV1: не хватает памяти".to_string());
            }
            std::ptr::copy_nonoverlapping(obus.as_ptr(), p, obus.len());
            let mut pic: Dav1dPicture = std::mem::zeroed();
            let mut sent = false;
            for _ in 0..64 {
                if !sent {
                    let r = dav1d_send_data(ctx, NonNull::new(&mut data)).0;
                    if r == 0 || data.sz == 0 {
                        sent = true;
                    } else if r != -11 {
                        dav1d_data_unref(NonNull::new(&mut data));
                        return Err(format!("AV1: ошибка данных ({r})"));
                    }
                }
                let r = dav1d_get_picture(ctx, NonNull::new(&mut pic)).0;
                if r == 0 {
                    break;
                }
                if r != -11 {
                    dav1d_data_unref(NonNull::new(&mut data));
                    return Err(format!("AV1: ошибка расшифровки ({r})"));
                }
                if sent {
                    // всё отдали, а картинки нет — сбросить очередь
                    let r = dav1d_get_picture(ctx, NonNull::new(&mut pic)).0;
                    if r == 0 {
                        break;
                    }
                    dav1d_data_unref(NonNull::new(&mut data));
                    return Err("AV1: в данных нет картинки".into());
                }
            }
            if data.sz > 0 {
                dav1d_data_unref(NonNull::new(&mut data));
            }
            let out = planes(&pic);
            dav1d_picture_unref(NonNull::new(&mut pic));
            out
        })();
        dav1d_close(NonNull::new(&mut ctx));
        res
    }
}

unsafe fn planes(pic: &Dav1dPicture) -> Result<Planes, String> {
    let (w, h) = (pic.p.w as usize, pic.p.h as usize);
    if w == 0 || h == 0 || w * h > 268_000_000 {
        return Err("AV1: пустая или слишком большая картинка".into());
    }
    let bpc = pic.p.bpc as u32;
    let layout = match pic.p.layout {
        DAV1D_PIXEL_LAYOUT_I400 => 0,
        DAV1D_PIXEL_LAYOUT_I420 => 1,
        DAV1D_PIXEL_LAYOUT_I422 => 2,
        _ => 3,
    };
    let (cw, ch) = match layout {
        0 => (0, 0),
        1 => (w.div_ceil(2), h.div_ceil(2)),
        2 => (w.div_ceil(2), h),
        _ => (w, h),
    };
    let read = |plane: usize, pw: usize, ph: usize, stride: isize| -> Vec<u16> {
        let Some(base) = pic.data[plane] else { return vec![] };
        let base = base.as_ptr() as *const u8;
        let mut v = Vec::with_capacity(pw * ph);
        for y in 0..ph {
            let row = base.offset(y as isize * stride);
            if bpc > 8 {
                let r = std::slice::from_raw_parts(row as *const u16, pw);
                v.extend_from_slice(r);
            } else {
                let r = std::slice::from_raw_parts(row, pw);
                v.extend(r.iter().map(|&x| x as u16));
            }
        }
        v
    };
    let y = read(0, w, h, pic.stride[0]);
    let (u, v) = if layout == 0 { (vec![], vec![]) } else { (read(1, cw, ch, pic.stride[1]), read(2, cw, ch, pic.stride[1])) };
    let (mtrx, full) = match pic.seq_hdr {
        Some(sh) => {
            let sh = sh.as_ref();
            (sh.mtrx as u32, sh.color_range != 0)
        }
        None => (2, false),
    };
    Ok(Planes { w, h, bpc, layout, y, u, v, cw, ch, mtrx, full })
}

/// Коэффициенты (Kr, Kb) по матрице cicp.
fn kr_kb(m: u32) -> (f32, f32) {
    match m {
        1 => (0.2126, 0.0722),            // BT.709
        9 | 10 => (0.2627, 0.0593),       // BT.2020
        4 => (0.30, 0.11),                // FCC
        7 => (0.212, 0.087),              // SMPTE 240M
        _ => (0.299, 0.114),              // BT.601 (5, 6 и «не указано»)
    }
}

/// YUV → RGBA8 (альфа — из отдельной картинки или 255).
pub fn to_rgba(p: &Planes, matrix: Option<(u16, bool)>, alpha: Option<&Planes>) -> Vec<u8> {
    let (m, full) = match matrix {
        Some((m, f)) if m != 2 => (m as u32, f),
        Some((_, f)) => (p.mtrx, f),
        None => (p.mtrx, p.full),
    };
    let maxv = ((1u32 << p.bpc) - 1) as f32;
    let (ys, yo, cs) = if full { (1.0 / maxv, 0.0, 1.0 / maxv) } else {
        let k = (1u32 << (p.bpc - 8)) as f32;
        (1.0 / (219.0 * k), 16.0 * k, 1.0 / (224.0 * k))
    };
    let half = (1u32 << (p.bpc - 1)) as f32;
    let (kr, kb) = kr_kb(m);
    let kg = 1.0 - kr - kb;
    let identity = m == 0;
    let mut out = vec![0u8; p.w * p.h * 4];
    let (sx, sy) = match p.layout {
        1 => (1, 1),
        2 => (1, 0),
        _ => (0, 0),
    };
    let alpha_max = alpha.map(|a| ((1u32 << a.bpc) - 1) as f32).unwrap_or(1.0);
    for y in 0..p.h {
        for x in 0..p.w {
            let yy = (p.y[y * p.w + x] as f32 - yo) * ys;
            let (r, g, b) = if p.layout == 0 {
                (yy, yy, yy)
            } else {
                let ci = (y >> sy).min(p.ch - 1) * p.cw + (x >> sx).min(p.cw - 1);
                let cb = (p.u[ci] as f32 - half) * cs;
                let cr = (p.v[ci] as f32 - half) * cs;
                if identity {
                    // GBR: Y — зелёный, U — синий, V — красный
                    let s = if full { 1.0 / maxv } else { ys };
                    ((p.v[ci] as f32 - yo) * s, yy, (p.u[ci] as f32 - yo) * s)
                } else {
                    let r = yy + 2.0 * (1.0 - kr) * cr;
                    let b = yy + 2.0 * (1.0 - kb) * cb;
                    let g = (yy - kr * r - kb * b) / kg;
                    (r, g, b)
                }
            };
            let o = (y * p.w + x) * 4;
            let c = |v: f32| (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            out[o] = c(r);
            out[o + 1] = c(g);
            out[o + 2] = c(b);
            out[o + 3] = match alpha {
                Some(a) if a.w == p.w && a.h == p.h => {
                    let v = a.y[y * p.w + x] as f32;
                    let v = if a.full { v / alpha_max } else { (v - 16.0 * (1u32 << (a.bpc - 8)) as f32) / (219.0 * (1u32 << (a.bpc - 8)) as f32) };
                    c(v)
                }
                _ => 255,
            };
        }
    }
    out
}

/// Картинка id (одиночная или сетка) → RGBA8 + размеры.
fn item_rgba(h: &Heif, file: &[u8], id: u32, depth: u32) -> Result<(u32, u32, Vec<u8>), String> {
    let it = h.item(id).ok_or("нет картинки")?;
    let nclx = h.props_of(id).iter().find_map(|p| match p {
        Prop::Nclx { matrix, full } => Some((*matrix, *full)),
        _ => None,
    });
    match &it.kind {
        b"av01" => {
            let p = decode_av1(&h.data(file, id)?)?;
            let a = match h.alpha_for(id) {
                Some(aid) => decode_av1(&h.data(file, aid)?).ok(),
                None => None,
            };
            Ok((p.w as u32, p.h as u32, to_rgba(&p, nclx, a.as_ref())))
        }
        b"grid" if depth == 0 => {
            let (rows, cols, ow, oh) = heif::grid(&h.data(file, id)?).ok_or("AVIF: повреждённая сетка")?;
            let tiles = h.refs_from(b"dimg", id);
            if tiles.len() != (rows * cols) as usize {
                return Err("AVIF: в сетке не хватает плиток".into());
            }
            if ow as u64 * oh as u64 > 268_000_000 {
                return Err("AVIF: слишком большая картинка".into());
            }
            // плитки расшифровываем параллельно
            let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 8);
            let next = std::sync::atomic::AtomicUsize::new(0);
            let results: Vec<std::sync::Mutex<Option<Result<(u32, u32, Vec<u8>), String>>>> = tiles.iter().map(|_| std::sync::Mutex::new(None)).collect();
            std::thread::scope(|sc| {
                for _ in 0..n.min(tiles.len()) {
                    sc.spawn(|| loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= tiles.len() {
                            break;
                        }
                        let r = item_rgba(h, file, tiles[i], depth + 1);
                        *results[i].lock().unwrap() = Some(r);
                    });
                }
            });
            let mut out = vec![255u8; ow as usize * oh as usize * 4];
            let mut tw = 0;
            let mut th = 0;
            for (i, r) in results.into_iter().enumerate() {
                let (w, hh, px) = r.into_inner().unwrap().unwrap_or_else(|| Err("AVIF: плитка не расшифрована".into()))?;
                if i == 0 {
                    tw = w;
                    th = hh;
                }
                let (gx, gy) = ((i as u32 % cols) * tw, (i as u32 / cols) * th);
                for y in 0..hh.min(oh.saturating_sub(gy)) {
                    let cw = w.min(ow.saturating_sub(gx)) as usize;
                    let src = &px[(y * w) as usize * 4..][..cw * 4];
                    let dst = ((gy + y) * ow + gx) as usize * 4;
                    out[dst..dst + cw * 4].copy_from_slice(src);
                }
            }
            Ok((ow, oh, out))
        }
        k => Err(format!("AVIF: неподдерживаемый вид картинки «{}»", String::from_utf8_lossy(k))),
    }
}

/// Повороты, отражения и обрезка — в порядке свойств.
fn transform(h: &Heif, id: u32, mut img: Img) -> Img {
    for p in h.props_of(id) {
        img = match p {
            Prop::Clap { w, h: ch, dx, dy } => {
                let (cw, chh) = (w.round().max(1.0) as u32, ch.round().max(1.0) as u32);
                let x = ((img.w as f64 - 1.0) / 2.0 + dx - (*w - 1.0) / 2.0).round().max(0.0) as u32;
                let y = ((img.h as f64 - 1.0) / 2.0 + dy - (*ch - 1.0) / 2.0).round().max(0.0) as u32;
                if x + cw <= img.w && y + chh <= img.h && (cw, chh) != (img.w, img.h) {
                    img.crop(x, y, cw, chh)
                } else {
                    img
                }
            }
            // irot — против часовой стрелки
            Prop::Irot(1) => img.rotate_ccw(),
            Prop::Irot(2) => img.rotate_cw().rotate_cw(),
            Prop::Irot(3) => img.rotate_cw(),
            Prop::Imir(0) => img.flip_h(),
            Prop::Imir(_) => img.flip_v(),
            _ => img,
        };
    }
    img
}

pub fn decode(file: &[u8]) -> Result<Img, String> {
    let h = heif::parse(file)?;
    let id = h.primary;
    let (w, hh, rgba) = item_rgba(&h, file, id, 0)?;
    let img = crate::extcodec::from_rgba(w, hh, &rgba, 4);
    Ok(transform(&h, id, img))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_sample_avif() {
        // 8 бит 4:2:0 и 10 бит; эталон — середина картинки «примерно как у libavif»
        for name in ["fox8.avif", "fox10.avif", "fox444.avif"] {
            let p = format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
            let Ok(b) = std::fs::read(&p) else { continue };
            let img = decode(&b).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!((img.w, img.h), (1204, 800), "{name}");
            let px = &img.px[((400 * 1204 + 602) * 4) as usize..][..3];
            assert!(px.iter().any(|&v| v > 20), "{name}: чёрная картинка {px:?}");
        }
    }
}
