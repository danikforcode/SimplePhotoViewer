// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Рисование поверх фото: карандаш, маркер, линия, стрелка, прямоугольник,
//! овал, текст. Пока рисуешь — это отдельные фигуры (их можно отменять
//! и стирать ластиком целиком), при применении они «впечатываются» в фото.
//! Сглаживание и текст — системный GDI+ (gdiplus.dll, 0 байт в exe).

use crate::image_buf::Img;
use std::sync::OnceLock;
use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::HDC;
use windows::Win32::Graphics::GdiPlus::*;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tool {
    Pen,
    Marker,
    Eraser,
    Line,
    Arrow,
    Rect,
    Ellipse,
    Text,
}

pub const TOOLS: [(Tool, &str, &str); 8] = [
    (Tool::Pen, "Карандаш", "Рисовать от руки"),
    (Tool::Marker, "Маркер", "Полупрозрачный выделитель"),
    (Tool::Eraser, "Ластик", "Стереть нарисованное (щелчок или провести по штриху)"),
    (Tool::Line, "Линия", "Прямая линия (Shift — под углом 45°)"),
    (Tool::Arrow, "Стрелка", "Стрелка-указатель (Shift — под углом 45°)"),
    (Tool::Rect, "Рамка", "Прямоугольник (Shift — квадрат)"),
    (Tool::Ellipse, "Овал", "Овал (Shift — круг)"),
    (Tool::Text, "Текст", "Щёлкните, где написать; Enter — новая строка, Esc или щелчок мимо — готово"),
];

/// Палитра: RGB.
pub const COLORS: [u32; 10] = [0xE53935, 0xFB8C00, 0xFDD835, 0x43A047, 0x1E88E5, 0x8E24AA, 0x000000, 0x757575, 0xFFFFFF, 0xEC407A];

#[derive(Clone, Debug)]
pub struct Shape {
    pub tool: Tool,
    /// Точки в координатах фото (для фигур — два угла, для текста — левый верхний).
    pub pts: Vec<(f32, f32)>,
    pub color: u32,
    /// Толщина линии в точках фото; у текста — размер шрифта.
    pub width: f32,
    pub alpha: u8,
    pub fill: bool,
    pub text: String,
}

impl Shape {
    pub fn new(tool: Tool, p: (f32, f32), color: u32, width: f32, alpha: u8, fill: bool) -> Shape {
        let alpha = if tool == Tool::Marker { alpha.min(110) } else { alpha };
        let width = if tool == Tool::Marker { width * 3.0 } else { width };
        Shape { tool, pts: vec![p, p], color, width, alpha, fill, text: String::new() }
    }

    /// Продолжить фигуру до точки p (shift — ровно: 45° / квадрат / круг).
    pub fn extend(&mut self, p: (f32, f32), shift: bool) {
        match self.tool {
            Tool::Pen | Tool::Marker => {
                let last = *self.pts.last().unwrap();
                if (last.0 - p.0).abs() + (last.1 - p.1).abs() >= 0.7 {
                    self.pts.push(p);
                }
            }
            _ => {
                let a = self.pts[0];
                let mut b = p;
                if shift {
                    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                    match self.tool {
                        Tool::Line | Tool::Arrow => {
                            let ang = dy.atan2(dx);
                            let step = std::f32::consts::FRAC_PI_4;
                            let snapped = (ang / step).round() * step;
                            let len = (dx * dx + dy * dy).sqrt();
                            b = (a.0 + len * snapped.cos(), a.1 + len * snapped.sin());
                        }
                        _ => {
                            let m = dx.abs().max(dy.abs());
                            b = (a.0 + m * dx.signum(), a.1 + m * dy.signum());
                        }
                    }
                }
                self.pts[1] = b;
            }
        }
    }

    /// Пустяковая фигура (щелчок без движения) — не сохраняем.
    pub fn is_trivial(&self) -> bool {
        match self.tool {
            Tool::Text => self.text.trim().is_empty(),
            Tool::Pen | Tool::Marker => false, // точка — тоже рисунок
            _ => {
                let (a, b) = (self.pts[0], self.pts[1]);
                (a.0 - b.0).abs() < 1.0 && (a.1 - b.1).abs() < 1.0
            }
        }
    }

    /// Примерная рамка текста.
    fn text_box(&self) -> (f32, f32, f32, f32) {
        let lines: Vec<&str> = self.text.split('\n').collect();
        let cols = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as f32;
        let (x, y) = self.pts[0];
        (x, y, (cols * self.width * 0.6).max(self.width), lines.len() as f32 * self.width * 1.3)
    }

    /// Задевает ли точка p (радиус r) фигуру — для ластика.
    pub fn hit(&self, p: (f32, f32), r: f32) -> bool {
        let tol = self.width / 2.0 + r;
        let seg = |a: (f32, f32), b: (f32, f32)| dist_seg(p, a, b) <= tol;
        match self.tool {
            Tool::Pen | Tool::Marker => {
                if self.pts.len() == 1 {
                    return dist(p, self.pts[0]) <= tol;
                }
                self.pts.windows(2).any(|w| seg(w[0], w[1]))
            }
            Tool::Line | Tool::Arrow => seg(self.pts[0], self.pts[1]),
            Tool::Rect => {
                let (a, b) = (self.pts[0], self.pts[1]);
                let (x0, x1, y0, y1) = (a.0.min(b.0), a.0.max(b.0), a.1.min(b.1), a.1.max(b.1));
                if self.fill && p.0 >= x0 && p.0 <= x1 && p.1 >= y0 && p.1 <= y1 {
                    return true;
                }
                seg((x0, y0), (x1, y0)) || seg((x1, y0), (x1, y1)) || seg((x1, y1), (x0, y1)) || seg((x0, y1), (x0, y0))
            }
            Tool::Ellipse => {
                let (a, b) = (self.pts[0], self.pts[1]);
                let (cx, cy) = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
                let (rx, ry) = (((a.0 - b.0) / 2.0).abs().max(0.5), ((a.1 - b.1) / 2.0).abs().max(0.5));
                let d = (((p.0 - cx) / rx).powi(2) + ((p.1 - cy) / ry).powi(2)).sqrt();
                if self.fill && d <= 1.0 {
                    return true;
                }
                (d - 1.0).abs() * rx.min(ry) <= tol
            }
            Tool::Text => {
                let (x, y, w, h) = self.text_box();
                p.0 >= x - r && p.0 <= x + w + r && p.1 >= y - r && p.1 <= y + h + r
            }
            Tool::Eraser => false,
        }
    }
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn dist_seg(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    if l2 < 1e-6 {
        return dist(p, a);
    }
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0);
    dist(p, (a.0 + t * dx, a.1 + t * dy))
}

// ------------------------------------------------------------- GDI+ ---

pub(crate) fn startup() -> bool {
    static TOKEN: OnceLock<bool> = OnceLock::new();
    *TOKEN.get_or_init(|| unsafe {
        let mut token = 0usize;
        let input = GdiplusStartupInput { GdiplusVersion: 1, ..Default::default() };
        GdiplusStartup(&mut token, &input, std::ptr::null_mut()) == Ok
    })
}

fn argb(rgb: u32, a: u8) -> u32 {
    ((a as u32) << 24) | (rgb & 0xFF_FFFF)
}

/// Нарисовать фигуры на готовом «холсте» GDI+ (в координатах фото).
pub(crate) unsafe fn render(g: *mut GpGraphics, shapes: &[Shape]) {
    let _ = GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
    let _ = GdipSetPixelOffsetMode(g, PixelOffsetModeHalf);
    let _ = GdipSetTextRenderingHint(g, TextRenderingHintAntiAlias);
    for s in shapes {
        let col = argb(s.color, s.alpha);
        let pts: Vec<PointF> = s.pts.iter().map(|p| PointF { X: p.0, Y: p.1 }).collect();
        let mut pen = std::ptr::null_mut();
        let _ = GdipCreatePen1(col, s.width.max(0.5), UnitPixel, &mut pen);
        let _ = GdipSetPenLineCap197819(pen, LineCapRound, LineCapRound, DashCapRound);
        let _ = GdipSetPenLineJoin(pen, LineJoinRound);
        let mut brush = std::ptr::null_mut();
        let _ = GdipCreateSolidFill(col, &mut brush);
        let br = brush as *mut GpBrush;
        let (a, b) = (s.pts[0], *s.pts.last().unwrap());
        let (x0, y0, w, h) = (a.0.min(b.0), a.1.min(b.1), (a.0 - b.0).abs(), (a.1 - b.1).abs());
        match s.tool {
            Tool::Pen | Tool::Marker => {
                if pts.len() == 1 || (pts.len() == 2 && dist(s.pts[0], s.pts[1]) < 0.5) {
                    let r = s.width / 2.0;
                    let _ = GdipFillEllipse(g, br, a.0 - r, a.1 - r, 2.0 * r, 2.0 * r);
                } else {
                    let _ = GdipDrawLines(g, pen, pts.as_ptr(), pts.len() as i32);
                }
            }
            Tool::Line => {
                let _ = GdipDrawLine(g, pen, a.0, a.1, b.0, b.1);
            }
            Tool::Arrow => {
                let len = dist(a, b).max(0.01);
                let head = (s.width * 4.0).max(10.0).min(len * 0.6);
                let (ux, uy) = ((b.0 - a.0) / len, (b.1 - a.1) / len);
                // древко — до основания наконечника
                let base = (b.0 - ux * head * 0.8, b.1 - uy * head * 0.8);
                let _ = GdipDrawLine(g, pen, a.0, a.1, base.0, base.1);
                let half = head * 0.55;
                let tri = [
                    PointF { X: b.0, Y: b.1 },
                    PointF { X: b.0 - ux * head - uy * half, Y: b.1 - uy * head + ux * half },
                    PointF { X: b.0 - ux * head + uy * half, Y: b.1 - uy * head - ux * half },
                ];
                let _ = GdipFillPolygon(g, br, tri.as_ptr(), 3, FillModeAlternate);
            }
            Tool::Rect => {
                if s.fill {
                    let _ = GdipFillRectangle(g, br, x0, y0, w, h);
                } else {
                    let _ = GdipDrawRectangle(g, pen, x0, y0, w, h);
                }
            }
            Tool::Ellipse => {
                if s.fill {
                    let _ = GdipFillEllipse(g, br, x0, y0, w, h);
                } else {
                    let _ = GdipDrawEllipse(g, pen, x0, y0, w, h);
                }
            }
            Tool::Text => {
                let mut fam = std::ptr::null_mut();
                for name in ["Segoe UI", "Arial", "Tahoma", "Microsoft Sans Serif"] {
                    let face: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
                    if GdipCreateFontFamilyFromName(PCWSTR(face.as_ptr()), std::ptr::null_mut(), &mut fam) == Ok && !fam.is_null() {
                        break;
                    }
                    fam = std::ptr::null_mut();
                }
                let owned = !fam.is_null();
                if fam.is_null() {
                    let _ = GdipGetGenericFontFamilySansSerif(&mut fam);
                }
                let mut font = std::ptr::null_mut();
                let _ = GdipCreateFont(fam, s.width.max(4.0), FontStyleBold.0, UnitPixel, &mut font);
                let txt: Vec<u16> = s.text.encode_utf16().collect();
                // рамка «с запасом»: нулевая у некоторых реализаций GDI+ не рисует ничего
                let layout = RectF { X: a.0, Y: a.1, Width: 100_000.0, Height: 100_000.0 };
                let _ = GdipDrawString(g, PCWSTR(txt.as_ptr()), txt.len() as i32, font, &layout, std::ptr::null(), br);
                let _ = GdipDeleteFont(font);
                if owned {
                    let _ = GdipDeleteFontFamily(fam);
                }
            }
            Tool::Eraser => {}
        }
        let _ = GdipDeletePen(pen);
        let _ = GdipDeleteBrush(br);
    }
}

/// Нарисовать на экране: точка фото (x, y) → (dx + x·scale, dy + y·scale).
pub fn paint(hdc: HDC, shapes: &[Shape], dx: f64, dy: f64, scale: f64) {
    if shapes.is_empty() || !startup() {
        return;
    }
    unsafe {
        let mut g = std::ptr::null_mut();
        if GdipCreateFromHDC(hdc, &mut g) != Ok {
            return;
        }
        let _ = GdipTranslateWorldTransform(g, dx as f32, dy as f32, MatrixOrderPrepend);
        let _ = GdipScaleWorldTransform(g, scale as f32, scale as f32, MatrixOrderPrepend);
        render(g, shapes);
        let _ = GdipDeleteGraphics(g);
    }
}

/// «Впечатать» фигуры в фото.
pub fn flatten(img: &Img, shapes: &[Shape]) -> Img {
    let mut out = img.clone();
    if shapes.is_empty() || !startup() {
        return out;
    }
    unsafe {
        let mut bmp = std::ptr::null_mut();
        // 32bppRGB = 0x00022009: альфа не используется, наш порядок BGRA совпадает
        if GdipCreateBitmapFromScan0(out.w as i32, out.h as i32, out.w as i32 * 4, 0x0002_2009, Some(out.px.as_mut_ptr()), &mut bmp) != Ok {
            return out;
        }
        let mut g = std::ptr::null_mut();
        if GdipGetImageGraphicsContext(bmp as *mut GpImage, &mut g) == Ok {
            render(g, shapes);
            let _ = GdipDeleteGraphics(g);
        }
        let _ = GdipDisposeImage(bmp as *mut GpImage);
    }
    for p in out.px.chunks_exact_mut(4) {
        p[3] = 255;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_hit_and_snap() {
        let mut s = Shape::new(Tool::Line, (0.0, 0.0), 0, 4.0, 255, false);
        s.extend((100.0, 10.0), true);
        assert!((s.pts[1].1).abs() < 1e-3, "Shift — горизонталь");
        assert!(s.hit((50.0, 1.0), 2.0));
        assert!(!s.hit((50.0, 20.0), 2.0));
        let mut r = Shape::new(Tool::Rect, (10.0, 10.0), 0, 2.0, 255, false);
        r.extend((30.0, 50.0), true);
        assert_eq!(r.pts[1], (50.0, 50.0), "Shift — квадрат");
        assert!(r.hit((10.0, 30.0), 1.0) && !r.hit((30.0, 30.0), 1.0));
        r.fill = true;
        assert!(r.hit((30.0, 30.0), 1.0));
        let click = Shape::new(Tool::Rect, (5.0, 5.0), 0, 2.0, 255, false);
        assert!(click.is_trivial());
        let m = Shape::new(Tool::Marker, (0.0, 0.0), 0, 4.0, 255, false);
        assert!(m.alpha <= 110 && m.width == 12.0);
    }

    #[test]
    fn flatten_draws() {
        let img = Img::filled(60, 40, [255, 255, 255]);
        let mut s = Shape::new(Tool::Rect, (10.0, 10.0), 0xFF0000, 2.0, 255, true);
        s.extend((50.0, 30.0), false);
        let out = flatten(&img, &[s]);
        let k = ((20 * 60 + 30) * 4) as usize;
        // красный: B мало, R много
        assert!(out.px[k + 2] > 200 && out.px[k] < 60, "{:?}", &out.px[k..k + 4]);
        assert_eq!(&out.px[0..3], &[255, 255, 255]);
    }
}
