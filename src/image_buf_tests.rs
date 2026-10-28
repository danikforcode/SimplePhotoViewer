// Тесты чистой логики (cargo test). Эталон для adjust() — PIL.ImageEnhance
// из исходной Python-версии: данные в image_buf_pil_data.rs.
use super::*;

include!("image_buf_pil_data.rs");

#[test]
fn adjust_matches_pil() {
    let src = Img { w: W, h: H, px: SRC.to_vec() };
    for (k, exp) in CASES {
        let got = src.adjust(k.0, k.1, k.2);
        let maxdiff = got.px.iter().zip(exp.iter()).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(maxdiff == 0, "{:?}: расхождение с PIL {}", k, maxdiff);
    }
}

#[test]
fn geometry() {
    // 3x2: пиксель (x,y) хранит значение 10*y+x в канале B
    let mut px = vec![255u8; 3 * 2 * 4];
    for y in 0..2 {
        for x in 0..3 {
            px[(y * 3 + x) * 4] = (10 * y + x) as u8;
        }
    }
    let im = Img { w: 3, h: 2, px };
    let b = |i: &Img| -> Vec<u8> { i.px.chunks(4).map(|p| p[0]).collect() };
    assert_eq!(b(&im.rotate_cw()), vec![10, 0, 11, 1, 12, 2]);
    assert_eq!(b(&im.rotate_ccw()), vec![2, 12, 1, 11, 0, 10]);
    assert_eq!(b(&im.flip_h()), vec![2, 1, 0, 12, 11, 10]);
    assert_eq!(b(&im.flip_v()), vec![10, 11, 12, 0, 1, 2]);
    assert_eq!(b(&im.crop(1, 0, 2, 2)), vec![1, 2, 11, 12]);
    assert_eq!(b(&im.clone().apply_exif(6)), b(&im.rotate_cw()));
    assert_eq!(b(&im.clone().apply_exif(8)), b(&im.rotate_ccw()));
    assert_eq!(b(&im.clone().apply_exif(3)), vec![12, 11, 10, 2, 1, 0]);
}

#[test]
fn compose() {
    let mut c = Img::filled(4, 3, [1, 2, 3]);
    let d = Img::filled(2, 2, [9, 9, 9]);
    c.draw(&d, 3, 2); // выходит за край: остаётся 1×1
    assert_eq!(&c.px[(2 * 4 + 3) * 4..(2 * 4 + 3) * 4 + 4], &[9, 9, 9, 255]);
    assert_eq!(&c.px[(2 * 4 + 2) * 4..(2 * 4 + 2) * 4 + 4], &[1, 2, 3, 255]);
    c.draw(&d, -1, -1);
    assert_eq!(&c.px[0..4], &[9, 9, 9, 255]);
    assert_eq!(&c.px[4..8], &[1, 2, 3, 255]);
    let wide = Img::filled(100, 50, [0, 0, 0]);
    let sq = wide.crop_to_aspect(1.0);
    assert_eq!((sq.w, sq.h), (50, 50));
    let tall = wide.crop_to_aspect(0.5);
    assert_eq!((tall.w, tall.h), (25, 50));
}
