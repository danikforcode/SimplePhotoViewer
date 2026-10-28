// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Изображение в памяти: BGRA, 8 бит на канал, альфа всегда 255 (прозрачность
//! при загрузке заливается белым). Здесь только чистая логика — без Win32.

#[derive(Clone)]
pub struct Img {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl Img {
    pub fn rotate_cw(&self) -> Img {
        let (w, h) = (self.w as usize, self.h as usize);
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let s = (y * w + x) * 4;
                let d = (x * h + (h - 1 - y)) * 4;
                out[d..d + 4].copy_from_slice(&self.px[s..s + 4]);
            }
        }
        Img { w: self.h, h: self.w, px: out }
    }

    pub fn rotate_ccw(&self) -> Img {
        let (w, h) = (self.w as usize, self.h as usize);
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let s = (y * w + x) * 4;
                let d = ((w - 1 - x) * h + y) * 4;
                out[d..d + 4].copy_from_slice(&self.px[s..s + 4]);
            }
        }
        Img { w: self.h, h: self.w, px: out }
    }

    pub fn flip_h(&self) -> Img {
        let (w, h) = (self.w as usize, self.h as usize);
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let s = (y * w + x) * 4;
                let d = (y * w + (w - 1 - x)) * 4;
                out[d..d + 4].copy_from_slice(&self.px[s..s + 4]);
            }
        }
        Img { w: self.w, h: self.h, px: out }
    }

    pub fn flip_v(&self) -> Img {
        let (w, h) = (self.w as usize, self.h as usize);
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            let s = y * w * 4;
            let d = (h - 1 - y) * w * 4;
            out[d..d + w * 4].copy_from_slice(&self.px[s..s + w * 4]);
        }
        Img { w: self.w, h: self.h, px: out }
    }

    pub fn crop(&self, x: u32, y: u32, cw: u32, ch: u32) -> Img {
        let x = x.min(self.w.saturating_sub(1));
        let y = y.min(self.h.saturating_sub(1));
        let cw = cw.min(self.w - x).max(1);
        let ch = ch.min(self.h - y).max(1);
        let mut out = Vec::with_capacity((cw * ch * 4) as usize);
        for row in y..y + ch {
            let s = ((row * self.w + x) * 4) as usize;
            out.extend_from_slice(&self.px[s..s + (cw * 4) as usize]);
        }
        Img { w: cw, h: ch, px: out }
    }

    /// EXIF-ориентация 1..8 → нормальный вид.
    pub fn apply_exif(self, o: u16) -> Img {
        match o {
            2 => self.flip_h(),
            3 => self.rotate_cw().rotate_cw(),
            4 => self.flip_v(),
            5 => self.rotate_cw().flip_h(),
            6 => self.rotate_cw(),
            7 => self.rotate_ccw().flip_h(),
            8 => self.rotate_ccw(),
            _ => self,
        }
    }

    /// Прозрачность → заливка белым (вход: BGRA с неумноженным альфа-каналом).
    pub fn flatten_white(&mut self) {
        for p in self.px.chunks_exact_mut(4) {
            let a = p[3] as u32;
            if a != 255 {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
                }
                p[3] = 255;
            }
        }
    }

    /// Яркость / контраст / насыщенность — как PIL.ImageEnhance в исходной версии:
    /// 1.0 = без изменений. Порядок тот же: яркость → контраст → насыщенность.
    /// Каждый этап — «смесь» с вырожденным изображением (чёрное / серое среднее /
    /// чёрно-белое) с усечением до 8 бит, ровно как Image.blend в PIL.
    pub fn adjust(&self, b: f32, c: f32, s: f32) -> Img {
        fn blend(d: f32, x: u8, a: f32) -> u8 {
            let t = d + a * (x as f32 - d);
            if t <= 0.0 {
                0
            } else if t >= 255.0 {
                255
            } else {
                t as u8
            }
        }
        // «L» из PIL: фиксированная арифметика с округлением
        fn luma(p: &[u8]) -> u8 {
            ((p[2] as u32 * 19595 + p[1] as u32 * 38470 + p[0] as u32 * 7471 + 0x8000) >> 16) as u8
        }
        let mut o = self.clone();
        let n = (o.w as u64 * o.h as u64).max(1);
        let mut sum: u64 = 0;
        for p in o.px.chunks_exact_mut(4) {
            for ch in &mut p[..3] {
                *ch = blend(0.0, *ch, b);
            }
            sum += luma(p) as u64;
        }
        let mean = (sum as f64 / n as f64 + 0.5).floor() as f32;
        for p in o.px.chunks_exact_mut(4) {
            for ch in &mut p[..3] {
                *ch = blend(mean, *ch, c);
            }
            let gray = luma(p) as f32;
            for ch in &mut p[..3] {
                *ch = blend(gray, *ch, s);
            }
        }
        o
    }
}

impl Img {
    /// Однотонное изображение (цвет в BGR).
    pub fn filled(w: u32, h: u32, bgr: [u8; 3]) -> Img {
        let px = [bgr[0], bgr[1], bgr[2], 255].repeat(w as usize * h as usize);
        Img { w, h, px }
    }

    /// Вклеить `src` в точку (x, y); выходящее за край отбрасывается.
    pub fn draw(&mut self, src: &Img, x: i32, y: i32) {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + src.w as i32).min(self.w as i32);
        let y1 = (y + src.h as i32).min(self.h as i32);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let n = (x1 - x0) as usize * 4;
        for yy in y0..y1 {
            let s = (((yy - y) as usize) * src.w as usize + (x0 - x) as usize) * 4;
            let d = ((yy as usize) * self.w as usize + x0 as usize) * 4;
            self.px[d..d + n].copy_from_slice(&src.px[s..s + n]);
        }
    }

    /// Оттенки серого (как «Насыщенность 0» в PIL).
    pub fn gray(&self) -> Img {
        self.adjust(1.0, 1.0, 0.0)
    }

    /// Вырезать центральную часть с заданным соотношением сторон (для «заполнить»).
    pub fn crop_to_aspect(&self, aspect: f64) -> Img {
        let cur = self.w as f64 / self.h as f64;
        if (cur - aspect).abs() < 1e-3 {
            return self.clone();
        }
        if cur > aspect {
            let nw = ((self.h as f64 * aspect).round() as u32).clamp(1, self.w);
            self.crop((self.w - nw) / 2, 0, nw, self.h)
        } else {
            let nh = ((self.w as f64 / aspect).round() as u32).clamp(1, self.h);
            self.crop(0, (self.h - nh) / 2, self.w, nh)
        }
    }
}

#[cfg(test)]
#[path = "image_buf_tests.rs"]
mod tests;
