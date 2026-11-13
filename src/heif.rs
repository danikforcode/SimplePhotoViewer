// SimplePhotoViewer (Rust/Win32) — © 2026 danikforcode
// Лицензия MIT (см. LICENSE). Распространяется «как есть», без гарантий.
//! Разбор контейнера HEIF (ISO/IEC 23008-12) для AVIF: главная картинка,
//! её данные (iloc/idat), свойства (av1C, ispe, irot, imir, clap, colr),
//! сетка плиток (grid) и прозрачность (auxl + auxC). Само сжатие AV1
//! расшифровывает rav1d (avif.rs). HEIC разбирает heic-decoder целиком.

#[derive(Clone, Debug, Default)]
pub struct Item {
    pub id: u32,
    pub kind: [u8; 4],
    /// Номера свойств (с 1) по порядку из ipma.
    pub props: Vec<u16>,
}

#[derive(Clone, Debug)]
pub enum Prop {
    Av1C(Vec<u8>),
    Ispe(u32, u32),
    /// Поворот против часовой стрелки на 90° × n.
    Irot(u8),
    /// 0 — отразить слева направо, 1 — сверху вниз.
    Imir(u8),
    /// Обрезка: ширина, высота, сдвиг центра (в точках, дробно).
    Clap { w: f64, h: f64, dx: f64, dy: f64 },
    /// cicp: первичные, перенос, матрица, полный диапазон.
    Nclx { matrix: u16, full: bool },
    AuxC(String),
    Other,
}

#[derive(Debug, Default)]
pub struct Heif {
    pub brands: Vec<[u8; 4]>,
    pub primary: u32,
    /// Где в файле лежит номер главной картинки (смещение, байт: 2 или 4).
    pub pitm_at: Option<(usize, usize)>,
    pub items: Vec<Item>,
    pub props: Vec<Prop>,
    /// (тип ссылки, откуда, куда…)
    pub refs: Vec<([u8; 4], u32, Vec<u32>)>,
    /// item → список (метод, смещение, длина)
    locs: Vec<(u32, u8, Vec<(u64, u64)>)>,
    idat: Vec<u8>,
}

struct R<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> R<'a> {
    fn new(b: &'a [u8]) -> R<'a> {
        R { b, p: 0 }
    }
    fn left(&self) -> usize {
        self.b.len().saturating_sub(self.p)
    }
    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.p..self.p.checked_add(n)?)?;
        self.p += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.bytes(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        let s = self.bytes(2)?;
        Some(u16::from_be_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        let s = self.bytes(4)?;
        Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u64(&mut self) -> Option<u64> {
        let s = self.bytes(8)?;
        Some(u64::from_be_bytes(s.try_into().ok()?))
    }
    /// Число из n байт (0, 4 или 8 — как в iloc).
    fn un(&mut self, n: u8) -> Option<u64> {
        match n {
            0 => Some(0),
            1 => self.u8().map(|v| v as u64),
            2 => self.u16().map(|v| v as u64),
            4 => self.u32().map(|v| v as u64),
            8 => self.u64(),
            _ => None,
        }
    }
    fn cstr(&mut self) -> String {
        let start = self.p;
        while self.p < self.b.len() && self.b[self.p] != 0 {
            self.p += 1;
        }
        let s = String::from_utf8_lossy(&self.b[start..self.p]).into_owned();
        self.p = (self.p + 1).min(self.b.len());
        s
    }
    /// Следующий бокс: (тип, тело).
    fn boxed(&mut self) -> Option<([u8; 4], &'a [u8])> {
        if self.left() < 8 {
            return None;
        }
        let start = self.p;
        let size = self.u32()? as u64;
        let kind: [u8; 4] = self.bytes(4)?.try_into().ok()?;
        let size = match size {
            1 => self.u64()?,
            0 => (self.b.len() - start) as u64,
            s => s,
        };
        let hdr = (self.p - start) as u64;
        let body = size.checked_sub(hdr)? as usize;
        let b = self.bytes(body)?;
        Some((kind, b))
    }
}

/// «Полный» бокс: версия и флаги.
fn full(r: &mut R) -> Option<(u8, u32)> {
    let v = r.u32()?;
    Some(((v >> 24) as u8, v & 0xFF_FFFF))
}

pub fn parse(file: &[u8]) -> Result<Heif, String> {
    let bad = || "повреждённый файл HEIF".to_string();
    let mut h = Heif::default();
    let mut r = R::new(file);
    let mut meta = None;
    while let Some((k, b)) = r.boxed() {
        match &k {
            b"ftyp" => {
                let mut f = R::new(b);
                if let Some(major) = f.bytes(4) {
                    h.brands.push(major.try_into().unwrap());
                }
                let _ = f.u32();
                while let Some(c) = f.bytes(4) {
                    h.brands.push(c.try_into().unwrap());
                }
            }
            b"meta" => meta = Some(b),
            _ => {}
        }
    }
    let meta = meta.ok_or("в файле нет картинки (meta)")?;
    let mut m = R::new(meta);
    full(&mut m).ok_or_else(bad)?;
    while let Some((k, b)) = m.boxed() {
        let mut r = R::new(b);
        match &k {
            b"pitm" => {
                let (v, _) = full(&mut r).ok_or_else(bad)?;
                h.primary = if v == 0 { r.u16().ok_or_else(bad)? as u32 } else { r.u32().ok_or_else(bad)? };
                h.pitm_at = Some((b.as_ptr() as usize - file.as_ptr() as usize + 4, if v == 0 { 2 } else { 4 }));
            }
            b"iinf" => {
                let (v, _) = full(&mut r).ok_or_else(bad)?;
                let n = if v == 0 { r.u16().ok_or_else(bad)? as u32 } else { r.u32().ok_or_else(bad)? };
                for _ in 0..n {
                    let Some((k, b)) = r.boxed() else { break };
                    if &k != b"infe" {
                        continue;
                    }
                    let mut e = R::new(b);
                    let (v, _) = full(&mut e).ok_or_else(bad)?;
                    if v < 2 {
                        continue;
                    }
                    let id = if v == 2 { e.u16().ok_or_else(bad)? as u32 } else { e.u32().ok_or_else(bad)? };
                    let _prot = e.u16();
                    let kind: [u8; 4] = e.bytes(4).ok_or_else(bad)?.try_into().unwrap();
                    h.items.push(Item { id, kind, props: vec![] });
                }
            }
            b"iloc" => {
                let (v, _) = full(&mut r).ok_or_else(bad)?;
                let a = r.u8().ok_or_else(bad)?;
                let b2 = r.u8().ok_or_else(bad)?;
                let (off_sz, len_sz, base_sz) = (a >> 4, a & 15, b2 >> 4);
                let idx_sz = if v >= 1 { b2 & 15 } else { 0 };
                let n = if v < 2 { r.u16().ok_or_else(bad)? as u32 } else { r.u32().ok_or_else(bad)? };
                for _ in 0..n {
                    let id = if v < 2 { r.u16().ok_or_else(bad)? as u32 } else { r.u32().ok_or_else(bad)? };
                    let method = if v >= 1 { (r.u16().ok_or_else(bad)? & 15) as u8 } else { 0 };
                    let _dref = r.u16().ok_or_else(bad)?;
                    let base = r.un(base_sz).ok_or_else(bad)?;
                    let ec = r.u16().ok_or_else(bad)?;
                    let mut ex = vec![];
                    for _ in 0..ec {
                        if idx_sz > 0 {
                            r.un(idx_sz).ok_or_else(bad)?;
                        }
                        let o = r.un(off_sz).ok_or_else(bad)?;
                        let l = r.un(len_sz).ok_or_else(bad)?;
                        ex.push((base + o, l));
                    }
                    h.locs.push((id, method, ex));
                }
            }
            b"idat" => h.idat = b.to_vec(),
            b"iref" => {
                let (v, _) = full(&mut r).ok_or_else(bad)?;
                while let Some((k, b)) = r.boxed() {
                    let mut e = R::new(b);
                    let rd = |e: &mut R| if v == 0 { e.u16().map(|x| x as u32) } else { e.u32() };
                    let from = rd(&mut e).ok_or_else(bad)?;
                    let n = e.u16().ok_or_else(bad)?;
                    let mut to = vec![];
                    for _ in 0..n {
                        to.push(rd(&mut e).ok_or_else(bad)?);
                    }
                    h.refs.push((k, from, to));
                }
            }
            b"iprp" => {
                while let Some((k, b)) = r.boxed() {
                    let mut e = R::new(b);
                    match &k {
                        b"ipco" => {
                            while let Some((pk, pb)) = e.boxed() {
                                h.props.push(prop(&pk, pb));
                            }
                        }
                        b"ipma" => {
                            let (v, fl) = full(&mut e).ok_or_else(bad)?;
                            let n = e.u32().ok_or_else(bad)?;
                            for _ in 0..n {
                                let id = if v < 1 { e.u16().ok_or_else(bad)? as u32 } else { e.u32().ok_or_else(bad)? };
                                let c = e.u8().ok_or_else(bad)?;
                                let mut ps = vec![];
                                for _ in 0..c {
                                    let p = if fl & 1 != 0 { e.u16().ok_or_else(bad)? & 0x7FFF } else { (e.u8().ok_or_else(bad)? & 0x7F) as u16 };
                                    ps.push(p);
                                }
                                if let Some(it) = h.items.iter_mut().find(|i| i.id == id) {
                                    it.props.extend(ps);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if h.items.is_empty() {
        return Err("в файле нет картинки".into());
    }
    if h.primary == 0 {
        h.primary = h.items[0].id;
    }
    Ok(h)
}

fn prop(k: &[u8; 4], b: &[u8]) -> Prop {
    let mut r = R::new(b);
    let p = (|| -> Option<Prop> {
        Some(match k {
            b"av1C" => Prop::Av1C(b.to_vec()),
            b"ispe" => {
                full(&mut r)?;
                Prop::Ispe(r.u32()?, r.u32()?)
            }
            b"irot" => Prop::Irot(r.u8()? & 3),
            b"imir" => Prop::Imir(r.u8()? & 1),
            b"clap" => {
                let mut f = || -> Option<f64> {
                    let n = r.u32()? as i32 as f64;
                    let d = r.u32()?.max(1) as f64;
                    Some(n / d)
                };
                let (w, h, dx, dy) = (f()?, f()?, f()?, f()?);
                Prop::Clap { w, h, dx, dy }
            }
            b"colr" => {
                if r.bytes(4)? != b"nclx" {
                    return Some(Prop::Other);
                }
                let _pri = r.u16()?;
                let _trc = r.u16()?;
                let matrix = r.u16()?;
                let full = r.u8()? & 0x80 != 0;
                Prop::Nclx { matrix, full }
            }
            b"auxC" => {
                full(&mut r)?;
                Prop::AuxC(r.cstr())
            }
            _ => Prop::Other,
        })
    })();
    p.unwrap_or(Prop::Other)
}

impl Heif {
    pub fn has_brand(&self, b: &[u8; 4]) -> bool {
        self.brands.iter().any(|x| x == b)
    }
    pub fn item(&self, id: u32) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }
    /// Свойства картинки по порядку.
    pub fn props_of(&self, id: u32) -> Vec<&Prop> {
        self.item(id).map(|i| i.props.iter().filter_map(|&p| self.props.get(p as usize - 1)).collect()).unwrap_or_default()
    }
    /// Данные картинки (склеенные отрезки).
    pub fn data<'a>(&'a self, file: &'a [u8], id: u32) -> Result<Vec<u8>, String> {
        let (_, method, ex) = self.locs.iter().find(|l| l.0 == id).ok_or("нет данных картинки")?;
        let src: &[u8] = match method {
            0 => file,
            1 => &self.idat,
            _ => return Err("неподдерживаемый способ хранения картинки".into()),
        };
        let mut out = vec![];
        for &(o, l) in ex {
            let (o, l) = (o as usize, if l == 0 { src.len().saturating_sub(o as usize) as u64 } else { l } as usize);
            out.extend_from_slice(src.get(o..o.checked_add(l).ok_or("повреждённый файл")?).ok_or("файл обрезан")?);
        }
        Ok(out)
    }
    /// Ссылки типа kind из картинки from.
    pub fn refs_from(&self, kind: &[u8; 4], from: u32) -> Vec<u32> {
        self.refs.iter().filter(|r| &r.0 == kind && r.1 == from).flat_map(|r| r.2.clone()).collect()
    }
    /// Картинка-прозрачность для id (auxl с auxC «…alpha»).
    pub fn alpha_for(&self, id: u32) -> Option<u32> {
        self.refs.iter().filter(|r| &r.0 == b"auxl" && r.2.contains(&id)).map(|r| r.1).find(|&a| {
            self.props_of(a).iter().any(|p| matches!(p, Prop::AuxC(s) if s.contains("alpha")))
        })
    }
}

/// Сетка плиток: (строк, столбцов, ширина, высота).
pub fn grid(data: &[u8]) -> Option<(u32, u32, u32, u32)> {
    let mut r = R::new(data);
    let _v = r.u8()?;
    let fl = r.u8()?;
    let rows = r.u8()? as u32 + 1;
    let cols = r.u8()? as u32 + 1;
    let (w, h) = if fl & 1 == 0 { (r.u16()? as u32, r.u16()? as u32) } else { (r.u32()?, r.u32()?) };
    Some((rows, cols, w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn parses_minimal_avif() {
        // одна картинка av01 с данными в idat, ispe и irot
        let mut ipco = bx(b"ispe", &[0, 0, 0, 0, 0, 0, 0, 64, 0, 0, 0, 32]);
        ipco.extend(bx(b"irot", &[1]));
        let ipma = [0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 2, 0x81, 2];
        let mut iprp = bx(b"ipco", &ipco);
        iprp.extend(bx(b"ipma", &ipma));
        let infe = bx(b"infe", &[2, 0, 0, 0, 0, 1, 0, 0, b'a', b'v', b'0', b'1', 0]);
        let mut iinf = vec![0, 0, 0, 0, 0, 1];
        iinf.extend(infe);
        // iloc v1: offset 4, length 4, base 0; item 1, метод 1 (idat), 1 отрезок
        let iloc = [1, 0, 0, 0, 0x44, 0x00, 0, 1, 0, 1, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 3];
        let mut meta = vec![0, 0, 0, 0];
        meta.extend(bx(b"pitm", &[0, 0, 0, 0, 0, 1]));
        meta.extend(bx(b"iinf", &iinf));
        meta.extend(bx(b"iloc", &iloc));
        meta.extend(bx(b"idat", &[9, 1, 2, 3, 9]));
        meta.extend(bx(b"iprp", &iprp));
        let mut file = bx(b"ftyp", b"avif\0\0\0\0mif1");
        file.extend(bx(b"meta", &meta));
        let h = parse(&file).unwrap();
        assert!(h.has_brand(b"avif"));
        assert_eq!(h.primary, 1);
        assert_eq!(&h.item(1).unwrap().kind, b"av01");
        assert_eq!(h.data(&file, 1).unwrap(), vec![1, 2, 3]);
        let p = h.props_of(1);
        assert!(matches!(p[0], Prop::Ispe(64, 32)) && matches!(p[1], Prop::Irot(1)));
        assert_eq!(grid(&[0, 0, 1, 2, 0, 100, 0, 50]), Some((2, 3, 100, 50)));
    }
}
