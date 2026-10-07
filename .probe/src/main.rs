use image::imageops::FilterType;
use std::time::Instant;

fn fixture(w: u32, h: u32) -> image::RgbaImage {
    image::RgbaImage::from_fn(w, h, |x, y| {
        let fx = f64::from(x) / f64::from(w.max(1)) * 6.0;
        let fy = f64::from(y) / f64::from(h.max(1)) * 4.0;
        let wave = fx.sin() * 80.0 + fy.cos() * 55.0;
        let (r, g, b) = if x < w / 4 && (x / 2 + y / 2) % 2 == 0 { (250u8, 8, 8) } else {
            ((128.0 + wave).clamp(0.0,255.0) as u8,
             (100.0 + wave*0.6).clamp(0.0,255.0) as u8,
             (150.0 - wave*0.5).clamp(0.0,255.0) as u8)
        };
        let alpha = if y * 2 >= h { (x.min(w.saturating_sub(1)) * 255 / w.max(1)) as u8 } else { 255 };
        image::Rgba([r, g, b, alpha])
    })
}

fn alg(f: FilterType) -> fast_image_resize::ResizeAlg {
    use fast_image_resize::FilterType as F;
    match f {
        FilterType::Triangle => fast_image_resize::ResizeAlg::Convolution(F::Bilinear),
        FilterType::CatmullRom => fast_image_resize::ResizeAlg::Convolution(F::CatmullRom),
        FilterType::Gaussian => fast_image_resize::ResizeAlg::Convolution(F::Gaussian),
        FilterType::Lanczos3 => fast_image_resize::ResizeAlg::Convolution(F::Lanczos3),
        FilterType::Nearest => fast_image_resize::ResizeAlg::Nearest,
    }
}

fn u8_path(src: &image::RgbaImage, w: u32, h: u32, f: FilterType) -> image::RgbaImage {
    let s = fast_image_resize::images::ImageRef::new(src.width(), src.height(), src.as_raw(), fast_image_resize::PixelType::U8x4).unwrap();
    let mut d = fast_image_resize::images::Image::new(w, h, fast_image_resize::PixelType::U8x4);
    fast_image_resize::Resizer::new().resize(&s, &mut d, &fast_image_resize::ResizeOptions::new().resize_alg(alg(f)).use_alpha(false)).unwrap();
    image::RgbaImage::from_raw(w, h, d.into_vec()).unwrap()
}

fn f32_path(src: &image::RgbaImage, w: u32, h: u32, f: FilterType) -> image::RgbaImage {
    let mut inp: Vec<u8> = Vec::with_capacity(src.as_raw().len() * 4);
    for v in src.as_raw() { inp.extend_from_slice(&(*v as f32).to_le_bytes()); }
    let s = fast_image_resize::images::ImageRef::new(src.width(), src.height(), &inp, fast_image_resize::PixelType::F32x4).unwrap();
    let mut outbuf = vec![0u8; (w as usize)*(h as usize)*16];
    let mut d = fast_image_resize::images::Image::from_slice_u8(w, h, &mut outbuf, fast_image_resize::PixelType::F32x4).unwrap();
    fast_image_resize::Resizer::new().resize(&s, &mut d, &fast_image_resize::ResizeOptions::new().resize_alg(alg(f)).use_alpha(false)).unwrap();
    let out = d.into_vec();
    image::RgbaImage::from_fn(w, h, |x, y| {
        let i = ((y as usize) * (w as usize) + x as usize) * 16;
        let mut p = [0f32; 4];
        for (c, slot) in p.iter_mut().enumerate() {
            *slot = f32::from_le_bytes([out[i+c*4], out[i+c*4+1], out[i+c*4+2], out[i+c*4+3]]);
        }
        image::Rgba([p[0].round().clamp(0.0,255.0) as u8, p[1].round().clamp(0.0,255.0) as u8, p[2].round().clamp(0.0,255.0) as u8, p[3].round().clamp(0.0,255.0) as u8])
    })
}

fn stats(a: &image::RgbaImage, b: &image::RgbaImage) -> (u32, f64) {
    let mut max = 0; let mut t = 0.0; let mut n = 0.0;
    for (x, y, pa) in a.enumerate_pixels() {
        let pb = b.get_pixel(x, y);
        for (va, vb) in pa.0.iter().zip(pb.0.iter()) {
            let d = u32::from(*va).abs_diff(u32::from(*vb));
            if d > max { max = d }
            t += d as f64; n += 1.0;
        }
    }
    (max, t / n)
}

fn bench(label: &str, src: &image::RgbaImage, w: u32, h: u32, f: FilterType) {
    let _ = u8_path(src, w, h, f);
    let _ = f32_path(src, w, h, f);
    let _ = image::imageops::resize(src, w, h, f);

    let mut ref_b = f64::MAX; let mut u8_b = f64::MAX; let mut f32_b = f64::MAX;
    for _ in 0..5 {
        let t = Instant::now(); let o = image::imageops::resize(src, w, h, f);
        ref_b = ref_b.min(t.elapsed().as_secs_f64()*1000.0); std::hint::black_box(o);
        let t = Instant::now(); let o = u8_path(src, w, h, f);
        u8_b = u8_b.min(t.elapsed().as_secs_f64()*1000.0); std::hint::black_box(o);
        let t = Instant::now(); let o = f32_path(src, w, h, f);
        f32_b = f32_b.min(t.elapsed().as_secs_f64()*1000.0); std::hint::black_box(o);
    }
    println!("BENCH {label} {w}x{h} {f:?} ref={ref_b:.1}ms u8={u8_b:.1}ms f32={f32_b:.1}ms  u8speedup={:.2}x f32speedup={:.2}x", ref_b/u8_b, ref_b/f32_b);
}

fn main() {
    for f in [FilterType::Triangle, FilterType::CatmullRom, FilterType::Gaussian, FilterType::Lanczos3] {
        let src = fixture(300, 200);
        let r = image::imageops::resize(&src, 450, 300, f);
        let (m1, n1) = stats(&r, &u8_path(&src, 450, 300, f));
        let (m2, n2) = stats(&r, &f32_path(&src, 450, 300, f));
        println!("ACC {f:?} mixed 300x200->450x300  u8: max={m1} mean={n1:.4}   f32: max={m2} mean={n2:.4}");
    }
    let checker = image::RgbaImage::from_fn(300, 200, |x, y| if (x/2 + y/2) % 2 == 0 { image::Rgba([250,8,8,255]) } else { image::Rgba([10,240,10,255]) });
    for f in [FilterType::CatmullRom, FilterType::Lanczos3] {
        let r = image::imageops::resize(&checker, 450, 300, f);
        let (m1, n1) = stats(&r, &u8_path(&checker, 450, 300, f));
        let (m2, n2) = stats(&r, &f32_path(&checker, 450, 300, f));
        println!("ACC {f:?} checker 300x200->450x300  u8: max={m1} mean={n1:.4}   f32: max={m2} mean={n2:.4}");
    }

    bench("24MP->1920", &fixture(6000,4000), 1920, 1280, FilterType::Lanczos3);
    bench("24MP->400", &fixture(6000,4000), 400, 267, FilterType::Triangle);
    bench("4000->thumb", &fixture(4000,3000), 400, 300, FilterType::Triangle);
    bench("800->4000", &fixture(800,600), 4000, 3000, FilterType::Lanczos3);
}
