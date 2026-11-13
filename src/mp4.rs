// SimplePhotoViewer (Rust/Win32) — © 2026 danikforcode
// Лицензия MIT (см. LICENSE). Распространяется «как есть», без гарантий.
//! Разбор MP4/MOV (ISO BMFF) для своего проигрывателя HEVC: дорожка видео,
//! её кадры (где лежат, сколько весят, когда показывать, какие — опорные),
//! параметры HEVC (hvcC: VPS/SPS/PPS) и поворот из матрицы tkhd.

use crate::image_buf::Img;

#[derive(Clone, Debug)]
pub struct Sample {
    pub offset: u64,
    pub size: u32,
    /// Время показа, с.
    pub pts: f64,
    pub sync: bool,
}

#[derive(Debug, Default)]
pub struct Track {
    pub codec: [u8; 4],
    pub width: u32,
    pub height: u32,
    /// Поворот при показе по часовой стрелке: 0, 90, 180, 270.
    pub rotation: u32,
    pub timescale: u32,
    pub duration: f64,
    /// Параметры декодера (VPS, SPS, PPS — без стартовых кодов).
    pub params: Vec<Vec<u8>>,
    /// Длина поля «размер NAL» в кадре (обычно 4).
    pub nal_len: usize,
    pub samples: Vec<Sample>,
    pub fps: f64,
}

#[derive(Debug, Default)]
pub struct Movie {
    pub video: Option<Track>,
    pub has_audio: bool,
    pub duration: f64,
}

struct R<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> R<'a> {
    fn new(b: &'a [u8]) -> Self {
        R { b, p: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.p..self.p.checked_add(n)?)?;
        self.p += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        let s = self.take(2)?;
        Some(u16::from_be_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        let s = self.take(4)?;
        Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_be_bytes(self.take(8)?.try_into().ok()?))
    }
    fn skip(&mut self, n: usize) -> Option<()> {
        self.take(n).map(|_| ())
    }
    fn boxed(&mut self) -> Option<([u8; 4], &'a [u8])> {
        if self.b.len().saturating_sub(self.p) < 8 {
            return None;
        }
        let start = self.p;
        let size = self.u32()? as u64;
        let kind: [u8; 4] = self.take(4)?.try_into().ok()?;
        let size = match size {
            1 => self.u64()?,
            0 => (self.b.len() - start) as u64,
            s => s,
        };
        let body = size.checked_sub((self.p - start) as u64)? as usize;
        Some((kind, self.take(body)?))
    }
}

fn find<'a>(b: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    let mut r = R::new(b);
    while let Some((k, body)) = r.boxed() {
        if &k == kind {
            return Some(body);
        }
    }
    None
}

/// Найти бокс moov в файле, не читая весь файл (moov бывает и в конце).
pub fn read_moov(f: &mut std::fs::File) -> std::io::Result<Vec<u8>> {
    let len = f.metadata()?.len();
    read_moov_from(f, len)
}

/// То же для любого источника с перемоткой (поток Проводника).
pub fn read_moov_from<F: std::io::Read + std::io::Seek>(f: &mut F, len: u64) -> std::io::Result<Vec<u8>> {
    use std::io::SeekFrom;
    let mut pos = 0u64;
    while pos + 8 <= len {
        f.seek(SeekFrom::Start(pos))?;
        let mut h = [0u8; 16];
        f.read_exact(&mut h[..8])?;
        let mut size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        let kind = [h[4], h[5], h[6], h[7]];
        let mut hdr = 8;
        if size == 1 {
            f.read_exact(&mut h[8..16])?;
            size = u64::from_be_bytes(h[8..16].try_into().unwrap());
            hdr = 16;
        } else if size == 0 {
            size = len - pos;
        }
        if size < hdr {
            break;
        }
        if &kind == b"moov" {
            if size > 512 << 20 {
                break;
            }
            let mut body = vec![0u8; (size - hdr) as usize];
            f.read_exact(&mut body)?;
            return Ok(body);
        }
        pos += size;
    }
    Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "в файле нет описания (moov)"))
}

pub fn parse(moov: &[u8]) -> Option<Movie> {
    let mut m = Movie::default();
    let mut r = R::new(moov);
    while let Some((k, b)) = r.boxed() {
        match &k {
            b"mvhd" => {
                let mut h = R::new(b);
                let v = h.u8()?;
                h.skip(3)?;
                let (ts, dur) = if v == 1 {
                    h.skip(16)?;
                    (h.u32()?, h.u64()?)
                } else {
                    h.skip(8)?;
                    (h.u32()?, h.u32()? as u64)
                };
                if ts > 0 {
                    m.duration = dur as f64 / ts as f64;
                }
            }
            b"trak" => {
                let mdia = find(b, b"mdia")?;
                let hdlr = find(mdia, b"hdlr")?;
                let kind = hdlr.get(8..12)?;
                if kind == b"soun" {
                    m.has_audio = true;
                } else if kind == b"vide" && m.video.is_none() {
                    if let Some(t) = video_track(b, mdia) {
                        m.video = Some(t);
                    }
                }
            }
            _ => {}
        }
    }
    if m.duration == 0.0 {
        m.duration = m.video.as_ref().map(|v| v.duration).unwrap_or(0.0);
    }
    Some(m)
}

fn video_track(trak: &[u8], mdia: &[u8]) -> Option<Track> {
    let mut t = Track { nal_len: 4, ..Default::default() };
    // поворот: матрица в tkhd
    if let Some(tkhd) = find(trak, b"tkhd") {
        let mut h = R::new(tkhd);
        let v = h.u8()?;
        h.skip(3)?;
        h.skip(if v == 1 { 32 } else { 20 })?;
        h.skip(16)?; // reserved, layer, group, volume, reserved
        let a = h.u32()? as i32;
        let b = h.u32()? as i32;
        let deg = (b as f64).atan2(a as f64).to_degrees().round() as i32;
        t.rotation = deg.rem_euclid(360) as u32 / 90 * 90;
    }
    let mdhd = find(mdia, b"mdhd")?;
    let mut h = R::new(mdhd);
    let v = h.u8()?;
    h.skip(3)?;
    let (ts, dur) = if v == 1 {
        h.skip(16)?;
        (h.u32()?, h.u64()?)
    } else {
        h.skip(8)?;
        (h.u32()?, h.u32()? as u64)
    };
    t.timescale = ts.max(1);
    t.duration = dur as f64 / t.timescale as f64;
    let stbl = find(find(mdia, b"minf")?, b"stbl")?;

    // stsd: кодек, размер, hvcC
    let stsd = find(stbl, b"stsd")?;
    let mut s = R::new(stsd);
    s.skip(8)?;
    let (k, e) = s.boxed()?;
    t.codec = k;
    let mut er = R::new(e);
    er.skip(24)?;
    t.width = er.u16()? as u32;
    t.height = er.u16()? as u32;
    er.skip(50)?;
    while let Some((bk, bb)) = er.boxed() {
        if &bk == b"hvcC" {
            parse_hvcc(bb, &mut t)?;
        }
    }

    // таблицы кадров
    let mut durs = vec![]; // (count, delta)
    let mut ctts = vec![];
    let mut sizes = vec![];
    let mut stsc = vec![];
    let mut chunks = vec![];
    let mut sync: Option<Vec<u32>> = None;
    let mut r = R::new(stbl);
    while let Some((k, b)) = r.boxed() {
        let mut x = R::new(b);
        let v = x.u8().unwrap_or(0);
        x.skip(3);
        match &k {
            b"stts" => {
                for _ in 0..x.u32()? {
                    durs.push((x.u32()?, x.u32()?));
                }
            }
            b"ctts" => {
                for _ in 0..x.u32()? {
                    let n = x.u32()?;
                    let o = x.u32()?;
                    ctts.push((n, if v == 1 { o as i32 as i64 } else { o as i64 }));
                }
            }
            b"stsz" => {
                let fixed = x.u32()?;
                let n = x.u32()?;
                if fixed != 0 {
                    sizes = vec![fixed; n as usize];
                } else {
                    for _ in 0..n {
                        sizes.push(x.u32()?);
                    }
                }
            }
            b"stsc" => {
                for _ in 0..x.u32()? {
                    stsc.push((x.u32()?, x.u32()?, x.u32()?));
                }
            }
            b"stco" => {
                for _ in 0..x.u32()? {
                    chunks.push(x.u32()? as u64);
                }
            }
            b"co64" => {
                for _ in 0..x.u32()? {
                    chunks.push(x.u64()?);
                }
            }
            b"stss" => {
                let mut v = vec![];
                for _ in 0..x.u32()? {
                    v.push(x.u32()?);
                }
                sync = Some(v);
            }
            _ => {}
        }
    }
    // смещение каждого кадра: кадры лежат кусками (chunk) подряд
    let n = sizes.len();
    let mut offsets = Vec::with_capacity(n);
    let mut si = 0usize;
    for (ci, &co) in chunks.iter().enumerate() {
        let c1 = ci as u32 + 1;
        let per = stsc.iter().rev().find(|e| e.0 <= c1).map(|e| e.1).unwrap_or(1);
        let mut off = co;
        for _ in 0..per {
            if si >= n {
                break;
            }
            offsets.push(off);
            off += sizes[si] as u64;
            si += 1;
        }
    }
    let mut dts = 0i64;
    let mut di = durs.iter().flat_map(|&(c, d)| std::iter::repeat_n(d as i64, c as usize));
    let mut ci = ctts.iter().flat_map(|&(c, o)| std::iter::repeat_n(o, c as usize));
    for i in 0..offsets.len() {
        let off = ci.next().unwrap_or(0);
        let pts = (dts + off) as f64 / t.timescale as f64;
        let is_sync = sync.as_ref().map(|s| s.binary_search(&(i as u32 + 1)).is_ok()).unwrap_or(true);
        t.samples.push(Sample { offset: offsets[i], size: sizes[i], pts, sync: is_sync });
        dts += di.next().unwrap_or(0);
    }
    // время первого кадра — ноль (сдвиг из-за ctts)
    if let Some(min) = t.samples.iter().map(|s| s.pts).reduce(f64::min) {
        for s in &mut t.samples {
            s.pts -= min;
        }
    }
    if t.duration > 0.0 {
        t.fps = t.samples.len() as f64 / t.duration;
    }
    Some(t)
}

fn parse_hvcc(b: &[u8], t: &mut Track) -> Option<()> {
    let mut r = R::new(b);
    r.skip(21)?;
    t.nal_len = (r.u8()? & 3) as usize + 1;
    let arrays = r.u8()?;
    for _ in 0..arrays {
        let _ty = r.u8()?;
        let n = r.u16()?;
        for _ in 0..n {
            let len = r.u16()? as usize;
            t.params.push(r.take(len)?.to_vec());
        }
    }
    Some(())
}

/// Кадр (длина + NAL, длина + NAL…) → отдельные NAL.
pub fn split_nals(data: &[u8], len_size: usize) -> Vec<&[u8]> {
    let mut v = vec![];
    let mut p = 0;
    while p + len_size <= data.len() {
        let mut n = 0usize;
        for i in 0..len_size {
            n = (n << 8) | data[p + i] as usize;
        }
        p += len_size;
        if n == 0 || p + n > data.len() {
            break;
        }
        v.push(&data[p..p + n]);
        p += n;
    }
    v
}

/// YUV (8/10 бит, 4:2:0/4:2:2) → BGRA сразу нужного размера (вписать в max_w × max_h,
/// не крупнее кадра) и с поворотом: потом кадр выводится на экран 1:1, без растяжения.
pub fn to_bgra(fr: &rusty_h265::Frame, rot: u32, max_w: u32, max_h: u32) -> Img {
    let p = &fr.picture;
    let (cx, cy, cw, ch) = p.crop;
    let (sw, sh_) = if rot % 180 == 90 { (ch, cw) } else { (cw, ch) };
    let k = (max_w as f64 / sw as f64).min(max_h as f64 / sh_ as f64).min(1.0);
    let (ow, oh) = (((sw as f64 * k).round() as usize).max(1), ((sh_ as f64 * k).round() as usize).max(1));
    let mut img = Img { w: ow as u32, h: oh as u32, px: vec![255u8; ow * oh * 4] };
    let shift = (p.bit_depth_luma as u32).saturating_sub(8);
    let (xs, ys) = match p.chroma_format_idc {
        1 => (1, 1),
        2 => (1, 0),
        _ => (0, 0),
    };
    let mono = p.chroma_format_idc == 0;
    // какие столбцы/строки выходного кадра брать из исходного (ближайший)
    let mx: Vec<usize> = (0..ow).map(|o| ((o as f64 + 0.5) * sw as f64 / ow as f64) as usize).map(|v| v.min(sw - 1)).collect();
    let my: Vec<usize> = (0..oh).map(|o| ((o as f64 + 0.5) * sh_ as f64 / oh as f64) as usize).map(|v| v.min(sh_ - 1)).collect();
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(8);
    let per = oh.div_ceil(n).max(1);
    let (mx, my) = (&mx, &my);
    std::thread::scope(|scope| {
        for (bi, band) in img.px.chunks_mut(per * ow * 4).enumerate() {
            scope.spawn(move || {
                for (dy, row) in band.chunks_exact_mut(ow * 4).enumerate() {
                    let ry = my[bi * per + dy];
                    if rot == 0 {
                        // частый случай — без поворота: строки источника берём один раз
                        let y = ry + cy;
                        let yrow = &p.planes[0].data[y * p.planes[0].stride..];
                        let (urow, vrow): (&[u16], &[u16]) = if mono {
                            (&[], &[])
                        } else {
                            let uy = y >> ys;
                            (&p.planes[1].data[uy * p.planes[1].stride..], &p.planes[2].data[uy * p.planes[2].stride..])
                        };
                        for (px, &rx) in row.chunks_exact_mut(4).zip(mx.iter()) {
                            let x = rx + cx;
                            let yy = (yrow[x] >> shift) as i32;
                            let (u, v) = if mono { (128, 128) } else { ((urow[x >> xs] >> shift) as i32, (vrow[x >> xs] >> shift) as i32) };
                            let c = (yy - 16) * 298;
                            let (d, e) = (u - 128, v - 128);
                            px[2] = ((c + 459 * e + 128) >> 8).clamp(0, 255) as u8;
                            px[1] = ((c - 55 * d - 136 * e + 128) >> 8).clamp(0, 255) as u8;
                            px[0] = ((c + 541 * d + 128) >> 8).clamp(0, 255) as u8;
                        }
                        continue;
                    }
                    for (ox, px) in row.chunks_exact_mut(4).enumerate() {
                        let rx = mx[ox];
                        // точка исходного кадра для точки (rx, ry) повёрнутого
                        let (x, y) = match rot {
                            90 => (ry, ch - 1 - rx),
                            180 => (cw - 1 - rx, ch - 1 - ry),
                            270 => (cw - 1 - ry, rx),
                            _ => (rx, ry),
                        };
                        let (x, y) = (x + cx, y + cy);
                        let yy = (p.planes[0].data[y * p.planes[0].stride + x] >> shift) as i32;
                        let (u, v) = if mono {
                            (128, 128)
                        } else {
                            let (ux, uy) = (x >> xs, y >> ys);
                            ((p.planes[1].data[uy * p.planes[1].stride + ux] >> shift) as i32, (p.planes[2].data[uy * p.planes[2].stride + ux] >> shift) as i32)
                        };
                        // BT.709, ограниченный диапазон (так пишут почти все камеры и телефоны)
                        let c = (yy - 16) * 298;
                        let (d, e) = (u - 128, v - 128);
                        px[2] = ((c + 459 * e + 128) >> 8).clamp(0, 255) as u8;
                        px[1] = ((c - 55 * d - 136 * e + 128) >> 8).clamp(0, 255) as u8;
                        px[0] = ((c + 541 * d + 128) >> 8).clamp(0, 255) as u8;
                    }
                }
            });
        }
    });
    img
}

/// Кадр HEVC-видео для миниатюры: ближайший опорный кадр к 10% длины.
pub fn hevc_thumb<R: std::io::Read + std::io::Seek>(f: &mut R, moov: &[u8], max: u32) -> Option<Img> {
    use std::io::SeekFrom;
    let m = parse(moov)?;
    let t = m.video?;
    if &t.codec != b"hvc1" && &t.codec != b"hev1" {
        return None;
    }
    let at = t.duration * 0.1;
    let idx = t.samples.iter().enumerate().filter(|(_, s)| s.sync && s.pts <= at + 0.01).map(|(i, _)| i).last().unwrap_or(0);
    let mut dec = rusty_h265::Decoder::new();
    for p in &t.params {
        let _ = dec.push_nal(p, None);
    }
    // опорный кадр и пара следующих (декодер отдаёт картинку, когда начинается следующая)
    for s in t.samples.iter().skip(idx).take(3) {
        let mut buf = vec![0u8; s.size as usize];
        f.seek(SeekFrom::Start(s.offset)).ok()?;
        f.read_exact(&mut buf).ok()?;
        for nal in split_nals(&buf, t.nal_len) {
            let _ = dec.push_nal(nal, None);
        }
        if let Ok(fr) = dec.next_frame() {
            return Some(to_bgra(&fr, t.rotation, max, max));
        }
    }
    dec.flush();
    dec.next_frame().ok().map(|fr| to_bgra(&fr, t.rotation, max, max))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hevc_mp4() {
        let p = format!("{}/tests/data/hevc_small.mp4", env!("CARGO_MANIFEST_DIR"));
        let Ok(mut f) = std::fs::File::open(&p) else { return };
        let moov = read_moov(&mut f).unwrap();
        let m = parse(&moov).unwrap();
        let v = m.video.unwrap();
        assert_eq!(&v.codec, b"hvc1");
        assert_eq!((v.width, v.height), (320, 180));
        assert!(v.params.len() >= 3, "VPS/SPS/PPS");
        assert_eq!(v.samples.len(), 30);
        assert!(v.samples[0].sync && v.samples[0].pts.abs() < 1e-9);
        assert!(m.has_audio);
        assert_eq!(v.rotation, 90);
    }

    #[test]
    fn splits_nals() {
        let d = [0, 0, 0, 2, 0x40, 1, 0, 0, 0, 1, 0x42];
        assert_eq!(split_nals(&d, 4), vec![&[0x40, 1][..], &[0x42][..]]);
    }
}
