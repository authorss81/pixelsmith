//! TEMPORARY measurement harness. Deleted before the phase is committed.
#![cfg(feature = "simd")]

use image::DynamicImage;
use image::imageops::FilterType;
use pixelsmith_core::resize::{resample_reference, try_resample_simd};

fn fixture(w: u32, h: u32) -> image::RgbaImage {
    image::RgbaImage::from_fn(w, h, |x, y| {
        let fx = f64::from(x) / f64::from(w.max(1)) * 6.0;
        let fy = f64::from(y) / f64::from(h.max(1)) * 4.0;
        let wave = fx.sin() * 80.0 + fy.cos() * 55.0;
        let (r, g, b) = if x < w / 4 && (x / 2 + y / 2) % 2 == 0 {
            (250u8, 8, 8)
        } else {
            (
                (128.0 + wave).clamp(0.0, 255.0) as u8,
                (100.0 + wave * 0.6).clamp(0.0, 255.0) as u8,
                (150.0 - wave * 0.5).clamp(0.0, 255.0) as u8,
            )
        };
        let alpha = if y * 2 >= h {
            (x.min(w.saturating_sub(1)) * 255 / w.max(1)) as u8
        } else {
            255
        };
        image::Rgba([r, g, b, alpha])
    })
}

fn stats(a: &image::RgbaImage, b: &image::RgbaImage) -> (u32, f64, u64) {
    let mut max = 0u32;
    let mut total = 0.0f64;
    let mut n = 0.0f64;
    let mut over = 0u64;
    for (x, y, pa) in a.enumerate_pixels() {
        let pb = b.get_pixel(x, y);
        for (va, vb) in pa.0.iter().zip(pb.0.iter()) {
            let d = u32::from(*va).abs_diff(u32::from(*vb));
            if d > max {
                max = d;
            }
            if d > 12 {
                over += 1;
            }
            total += f64::from(d);
            n += 1.0;
        }
    }
    (max, total / n, over)
}

const SIZE_PAIRS: [((u32, u32), (u32, u32)); 8] = [
    ((64, 48), (32, 24)),
    ((64, 48), (1, 1)),
    ((300, 200), (100, 67)),
    ((300, 200), (450, 300)),
    ((97, 61), (400, 251)),
    ((512, 384), (17, 13)),
    ((256, 256), (255, 255)),
    ((64, 64), (64, 1)),
];

#[test]
fn measure_matrix() {
    for filter in [
        FilterType::Triangle,
        FilterType::CatmullRom,
        FilterType::Gaussian,
        FilterType::Lanczos3,
    ] {
        for ((sw, sh), (dw, dh)) in SIZE_PAIRS {
            let src = DynamicImage::ImageRgba8(fixture(sw, sh));
            let want = resample_reference(&src, dw, dh, filter).to_rgba8();
            let got = try_resample_simd(&src, dw, dh, filter).unwrap().to_rgba8();
            let (max, mean, over) = stats(&want, &got);
            println!("MATRIX {filter:?} {sw}x{sh}->{dw}x{dh} max={max} mean={mean:.4} over12={over}");
        }
    }
}

#[test]
fn measure_extreme() {
    let src = DynamicImage::ImageRgba8(fixture(10_000, 200));
    for filter in [FilterType::Triangle, FilterType::Lanczos3] {
        let want = resample_reference(&src, 50, 2, filter).to_rgba8();
        let got = try_resample_simd(&src, 50, 2, filter).unwrap().to_rgba8();
        let (max, mean, over) = stats(&want, &got);
        println!("EXTREME {filter:?} 10000x200->50x2 max={max} mean={mean:.4} over12={over}");
    }
}

#[test]
fn measure_nearest_disagreement() {
    let src = DynamicImage::ImageRgba8(fixture(300, 200));
    // Ask the SIMD crate for Nearest directly, bypassing this crate's refusal.
    let got = fir_nearest(&src, 450, 300);
    let want = resample_reference(&src, 450, 300, FilterType::Nearest).to_rgba8();
    let mut differing = 0u64;
    let mut total = 0u64;
    let mut max = 0u32;
    for (x, y, pa) in want.enumerate_pixels() {
        let pb = got.get_pixel(x, y);
        total += 1;
        let differs = pa.0 != pb.0;
        if differs {
            differing += 1;
        }
        for (va, vb) in pa.0.iter().zip(pb.0.iter()) {
            max = max.max(u32::from(*va).abs_diff(u32::from(*vb)));
        }
        let _ = (x, y);
    }
    println!(
        "NEAREST 300x200->450x300 differing={differing}/{total} ({:.1}%) maxchan={max}",
        100.0 * differing as f64 / total as f64
    );

    // Row-selection ramp, as the test uses.
    let ramp = || image::RgbaImage::from_fn(4, 4, |_, y| image::Rgba([(y as u8) * 60, 0, 0, 255]));
    let r = resample_reference(
        &DynamicImage::ImageRgba8(ramp()),
        6,
        6,
        FilterType::Nearest,
    )
    .to_rgba8();
    let s = fir_nearest(&DynamicImage::ImageRgba8(ramp()), 6, 6);
    let image_rows: Vec<u32> = (0..6)
        .map(|y| u32::from(r.get_pixel(0, y).0[0]) / 60)
        .collect();
    let fir_rows: Vec<u32> = (0..6).map(|y| u32::from(s.get_pixel(0, y).0[0]) / 60).collect();
    println!("NEAREST ramp image={image_rows:?} fir={fir_rows:?}");
}

fn fir_nearest(src: &DynamicImage, w: u32, h: u32) -> image::RgbaImage {
    use fast_image_resize::images::{Image as FirImage, ImageRef};
    use fast_image_resize::{PixelType, ResizeAlg, ResizeOptions, Resizer};
    let s = src.to_rgba8();
    let sv = ImageRef::new(s.width(), s.height(), s.as_raw(), PixelType::U8x4).unwrap();
    let mut d = FirImage::new(w, h, PixelType::U8x4);
    Resizer::new()
        .resize(
            &sv,
            &mut d,
            &ResizeOptions::new()
                .resize_alg(ResizeAlg::Nearest)
                .use_alpha(false),
        )
        .unwrap();
    image::RgbaImage::from_raw(w, h, d.into_vec()).unwrap()
}

/// Timing across the four realistic cases the phase prompt names.
#[test]
fn measure_timing() {
    let cases: [(&str, u32, u32, u32, u32, FilterType); 4] = [
        ("24MP -> 1920 wide", 6000, 4000, 1920, 1280, FilterType::Lanczos3),
        ("24MP -> 400 wide", 6000, 4000, 400, 267, FilterType::Triangle),
        ("4000 -> thumbnail", 4000, 3000, 400, 300, FilterType::Triangle),
        ("upscale 800 -> 4000", 800, 600, 4000, 3000, FilterType::Lanczos3),
    ];
    for (name, sw, sh, dw, dh, filter) in cases {
        let src = DynamicImage::ImageRgba8(fixture(sw, sh));
        // Warm up both, then take the best of five: the best is the number
        // least contaminated by another process on the runner.
        let _ = resample_reference(&src, dw, dh, filter);
        let _ = try_resample_simd(&src, dw, dh, filter).unwrap();

        let mut ref_best = f64::MAX;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            let out = resample_reference(&src, dw, dh, filter);
            let e = t.elapsed().as_secs_f64() * 1000.0;
            ref_best = ref_best.min(e);
            std::hint::black_box(out);
        }
        let mut simd_best = f64::MAX;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            let out = try_resample_simd(&src, dw, dh, filter).unwrap();
            let e = t.elapsed().as_secs_f64() * 1000.0;
            simd_best = simd_best.min(e);
            std::hint::black_box(out);
        }
        println!(
            "TIMING {name} {sw}x{sh}->{dw}x{dh} {filter:?} reference={ref_best:.1}ms simd={simd_best:.1}ms ratio={:.2}x",
            ref_best / simd_best
        );
    }
}
/// Where does the difference live: hard edges or everywhere?
#[test]
fn measure_localisation() {
    // Smooth-only fixture: same wave, no checkerboard.
    let smooth = |w: u32, h: u32| {
        image::RgbaImage::from_fn(w, h, |x, y| {
            let fx = f64::from(x) / f64::from(w.max(1)) * 6.0;
            let fy = f64::from(y) / f64::from(h.max(1)) * 4.0;
            let wave = fx.sin() * 80.0 + fy.cos() * 55.0;
            image::Rgba([
                (128.0 + wave).clamp(0.0, 255.0) as u8,
                (100.0 + wave * 0.6).clamp(0.0, 255.0) as u8,
                (150.0 - wave * 0.5).clamp(0.0, 255.0) as u8,
                255,
            ])
        })
    };
    for filter in [FilterType::CatmullRom, FilterType::Lanczos3] {
        let src = DynamicImage::ImageRgba8(smooth(300, 200));
        let want = resample_reference(&src, 450, 300, filter).to_rgba8();
        let got = try_resample_simd(&src, 450, 300, filter).unwrap().to_rgba8();
        let (max, mean, _) = stats(&want, &got);
        println!("SMOOTH {filter:?} 300x200->450x300 max={max} mean={mean:.4}");
    }

    // Checkerboard-only fixture: pure high frequency.
    let checker = |w: u32, h: u32| {
        image::RgbaImage::from_fn(w, h, |x, y| {
            if (x / 2 + y / 2) % 2 == 0 {
                image::Rgba([250, 8, 8, 255])
            } else {
                image::Rgba([10, 240, 10, 255])
            }
        })
    };
    for filter in [FilterType::CatmullRom, FilterType::Lanczos3] {
        let src = DynamicImage::ImageRgba8(checker(300, 200));
        let want = resample_reference(&src, 450, 300, filter).to_rgba8();
        let got = try_resample_simd(&src, 450, 300, filter).unwrap().to_rgba8();
        let (max, mean, _) = stats(&want, &got);
        println!("CHECKER {filter:?} 300x200->450x300 max={max} mean={mean:.4}");
    }

    // Where in the mixed fixture: inside the checkerboard quarter, or outside?
    let src = DynamicImage::ImageRgba8(fixture(300, 200));
    for filter in [FilterType::CatmullRom, FilterType::Lanczos3] {
        let want = resample_reference(&src, 450, 300, filter).to_rgba8();
        let got = try_resample_simd(&src, 450, 300, filter).unwrap().to_rgba8();
        let (mut in_box, mut out_box) = (0u32, 0u32);
        for (x, y, pa) in want.enumerate_pixels() {
            let pb = got.get_pixel(x, y);
            let mut d = 0u32;
            for (va, vb) in pa.0.iter().zip(pb.0.iter()) {
                d = d.max(u32::from(*va).abs_diff(u32::from(*vb)));
            }
            if x < 450 / 4 && y < 300 {
                in_box = in_box.max(d);
            } else {
                out_box = out_box.max(d);
            }
        }
        println!("LOCAL {filter:?} checker-quarter max={in_box} rest max={out_box}");
    }
}

/// Recover each crate's effective 1-D kernel by resampling a delta, so the
/// difference can be attributed to coefficient quantisation or to a different
/// kernel shape.
#[test]
fn measure_effective_kernel() {
    for filter in [FilterType::CatmullRom, FilterType::Lanczos3] {
        // 1-D probe: a single lit pixel in the middle of a dark row, 2x upscale.
        let n = 16u32;
        let src = DynamicImage::ImageRgba8(image::RgbaImage::from_fn(n, 1, |x, _| {
            if x == 8 {
                image::Rgba([255, 255, 255, 255])
            } else {
                image::Rgba([0, 0, 0, 255])
            }
        }));
        let want = resample_reference(&src, n * 2, 1, filter).to_rgba8();
        let got = try_resample_simd(&src, n * 2, 1, filter).unwrap().to_rgba8();
        let wr: Vec<i64> = (0..n * 2).map(|x| i64::from(want.get_pixel(x, 0).0[0])).collect();
        let gr: Vec<i64> = (0..n * 2).map(|x| i64::from(got.get_pixel(x, 0).0[0])).collect();
        println!("KERNEL1D {filter:?} image={wr:?}");
        println!("KERNEL1D {filter:?} simd ={gr:?}");

        // 2-D probe on a delta in a dark field, 1.5x upscale.
        let src = DynamicImage::ImageRgba8(image::RgbaImage::from_fn(16, 16, |x, y| {
            if x == 8 && y == 8 {
                image::Rgba([255, 255, 255, 255])
            } else {
                image::Rgba([0, 0, 0, 255])
            }
        }));
        let want = resample_reference(&src, 24, 24, filter).to_rgba8();
        let got = try_resample_simd(&src, 24, 24, filter).unwrap().to_rgba8();
        let mid = 12u32;
        let wr: Vec<i64> = (mid - 6..mid + 6)
            .map(|x| i64::from(want.get_pixel(x, mid).0[0]))
            .collect();
        let gr: Vec<i64> = (mid - 6..mid + 6)
            .map(|x| i64::from(got.get_pixel(x, mid).0[0]))
            .collect();
        println!("KERNEL2D {filter:?} image={wr:?} sum={}", wr.iter().sum::<i64>());
        println!("KERNEL2D {filter:?} simd ={gr:?} sum={}", gr.iter().sum::<i64>());
    }
}

/// Attribute the worst-case difference: which pass, and what the values are.
#[test]
fn measure_worst_pixel() {
    let checker = |w: u32, h: u32| {
        image::RgbaImage::from_fn(w, h, |x, y| {
            if (x / 2 + y / 2) % 2 == 0 {
                image::Rgba([250, 8, 8, 255])
            } else {
                image::Rgba([10, 240, 10, 255])
            }
        })
    };
    let src = DynamicImage::ImageRgba8(checker(300, 200));
    let filter = FilterType::Lanczos3;
    let want = resample_reference(&src, 450, 300, filter).to_rgba8();
    let got = try_resample_simd(&src, 450, 300, filter).unwrap().to_rgba8();

    let mut worst = (0u32, 0u32, 0usize, 0u8, 0u8);
    let mut hist = [0u32; 256];
    for (x, y, pa) in want.enumerate_pixels() {
        let pb = got.get_pixel(x, y);
        for (c, (va, vb)) in pa.0.iter().zip(pb.0.iter()).enumerate() {
            let d = u32::from(*va).abs_diff(u32::from(*vb));
            hist[d as usize] += 1;
            if d > worst.3.into() {
                worst = (x, y, c, *va, *vb);
            }
        }
    }
    println!(
        "WORST at ({},{}) ch{} image={} simd={}",
        worst.0, worst.1, worst.2, worst.3, worst.4
    );
    let over8: u32 = hist[9..].iter().sum();
    let over16: u32 = hist[17..].iter().sum();
    let over24: u32 = hist[25..].iter().sum();
    println!(
        "HIST total={} >8={} >16={} >24={} max_nonzero_bucket={}",
        hist.iter().sum::<u32>(),
        over8,
        over16,
        over24,
        hist.iter().rposition(|&c| c > 0).unwrap_or(0)
    );

    // Horizontal-only and vertical-only passes, to attribute the error.
    let h_want = resample_reference(&src, 450, 200, filter).to_rgba8();
    let h_got = try_resample_simd(&src, 450, 200, filter).unwrap().to_rgba8();
    let (hmax, hmean, _) = stats(&h_want, &h_got);
    println!("PASS horizontal-only max={hmax} mean={hmean:.4}");
    let v_want = resample_reference(&src, 300, 300, filter).to_rgba8();
    let v_got = try_resample_simd(&src, 300, 300, filter).unwrap().to_rgba8();
    let (vmax, vmean, _) = stats(&v_want, &v_got);
    println!("PASS vertical-only max={vmax} mean={vmean:.4}");

    // 2-D vs the composition of the two one-axis passes on the reference: is the
    // error in the horizontal pass carried through, or created by the second?
    println!(
        "2D mean={:.4} vs sqrt(h^2+v^2)={:.4}",
        stats(&want, &got).1,
        (hmean * hmean + vmean * vmean).sqrt()
    );
}

/// Two numbers that decide the tolerance: what a hard-edge fixture costs, and
/// what a genuinely different kernel costs on the same fixture.
#[test]
fn measure_contrast() {
    let checker = |w: u32, h: u32| {
        image::RgbaImage::from_fn(w, h, |x, y| {
            if (x / 2 + y / 2) % 2 == 0 {
                image::Rgba([250, 8, 8, 255])
            } else {
                image::Rgba([10, 240, 10, 255])
            }
        })
    };
    let smooth = |w: u32, h: u32| {
        image::RgbaImage::from_fn(w, h, |x, y| {
            let fx = f64::from(x) / f64::from(w.max(1)) * 6.0;
            let fy = f64::from(y) / f64::from(h.max(1)) * 4.0;
            let wave = fx.sin() * 80.0 + fy.cos() * 55.0;
            image::Rgba([
                (128.0 + wave).clamp(0.0, 255.0) as u8,
                (100.0 + wave * 0.6).clamp(0.0, 255.0) as u8,
                (150.0 - wave * 0.5).clamp(0.0, 255.0) as u8,
                255,
            ])
        })
    };
    // A picture with real edges, not a synthetic Nyquist square wave: a hard
    // step, a diagonal, text-like blocks and a gradient.
    let picture = |w: u32, h: u32| {
        image::RgbaImage::from_fn(w, h, |x, y| {
            let fx = f64::from(x) / f64::from(w.max(1));
            let fy = f64::from(y) / f64::from(h.max(1));
            let wave = fx.sin() * 90.0 + fy.cos() * 60.0;
            let base = image::Rgba([
                (120.0 + wave).clamp(0.0, 255.0) as u8,
                (140.0 + wave * 0.5).clamp(0.0, 255.0) as u8,
                (160.0 - wave * 0.4).clamp(0.0, 255.0) as u8,
                255,
            ]);
            // A vertical hard edge at 40% width, and a text-like block pattern
            // over the left half.
            if x as f64 / f64::from(w) > 0.4 {
                let g = (255.0 * fx) as u8;
                return image::Rgba([g, g / 2, 40, 255]);
            }
            if (x / 7 + y / 11) % 3 == 0 {
                return image::Rgba([250, 250, 250, 255]);
            }
            base
        })
    };

    for (name, make) in [
        ("smooth", smooth as fn(u32, u32) -> image::RgbaImage),
        ("picture", picture as fn(u32, u32) -> image::RgbaImage),
        ("checker", checker as fn(u32, u32) -> image::RgbaImage),
    ] {
        let src = DynamicImage::ImageRgba8(make(300, 200));
        // SIMD vs reference, per filter.
        for filter in [
            FilterType::Triangle,
            FilterType::Gaussian,
            FilterType::CatmullRom,
            FilterType::Lanczos3,
        ] {
            let want = resample_reference(&src, 450, 300, filter).to_rgba8();
            let got = try_resample_simd(&src, 450, 300, filter).unwrap().to_rgba8();
            let (max, mean, _) = stats(&want, &got);
            println!("KERNELPAIR {name} {filter:?} simd-vs-ref max={max} mean={mean:.4}");
        }
        // Contrast: reference Lanczos3 against reference CatmullRom, i.e. two
        // kernels that are both defensible and both shipped as choices.
        let a = resample_reference(&src, 450, 300, FilterType::Lanczos3).to_rgba8();
        let b = resample_reference(&src, 450, 300, FilterType::CatmullRom).to_rgba8();
        let c = resample_reference(&src, 450, 300, FilterType::Triangle).to_rgba8();
        let (max, mean, _) = stats(&a, &b);
        println!("CONTRAST {name} lanczos-vs-catmull max={max} mean={mean:.4}");
        let (max, mean, _) = stats(&a, &c);
        println!("CONTRAST {name} lanczos-vs-triangle max={max} mean={mean:.4}");
    }

    // And the smooth picture across the whole matrix, to size the tight bound.
    for filter in [
        FilterType::Triangle,
        FilterType::Gaussian,
        FilterType::CatmullRom,
        FilterType::Lanczos3,
    ] {
        for ((sw, sh), (dw, dh)) in SIZE_PAIRS {
            let src = DynamicImage::ImageRgba8(picture(sw, sh));
            let want = resample_reference(&src, dw, dh, filter).to_rgba8();
            let got = try_resample_simd(&src, dw, dh, filter).unwrap().to_rgba8();
            let (max, mean, _) = stats(&want, &got);
            if max > 2 {
                println!("PICTURE {filter:?} {sw}x{sh}->{dw}x{dh} max={max} mean={mean:.4}");
            }
        }
    }
    println!("PICTURE done");
}

/// Prove the mechanism: the two crates do the same maths, but
/// `fast_image_resize` stores the intermediate of its separable pass as u8
/// (and clips it to gamut there), while `image` keeps f32 all the way to the
/// end. Reproduce the 2-D difference by making the reference round its own
/// intermediate.
#[test]
fn measure_intermediate_quantisation() {
    let checker = |w: u32, h: u32| {
        image::RgbaImage::from_fn(w, h, |x, y| {
            if (x / 2 + y / 2) % 2 == 0 {
                image::Rgba([250, 8, 8, 255])
            } else {
                image::Rgba([10, 240, 10, 255])
            }
        })
    };
    let src = DynamicImage::ImageRgba8(checker(300, 200));
    for filter in [FilterType::CatmullRom, FilterType::Lanczos3] {
        let reference_2d = resample_reference(&src, 450, 300, filter).to_rgba8();
        let simd_2d = try_resample_simd(&src, 450, 300, filter).unwrap().to_rgba8();

        // Step 1: the horizontal pass alone, as u8. Measured bit-identical to
        // the SIMD crate's own single-axis pass, so this is the shared
        // intermediate.
        let horiz = resample_reference(&src, 450, 200, filter).to_rgba8();
        let simd_horiz = try_resample_simd(&src, 450, 200, filter).unwrap().to_rgba8();
        println!(
            "PROBE {filter:?} horiz simd-vs-ref max={}",
            stats(&horiz, &simd_horiz).0
        );

        // Step 2: the reference's vertical pass on that u8 intermediate, which
        // is what `fast_image_resize` does.
        let two_step = resample_reference(
            &DynamicImage::ImageRgba8(horiz.clone()),
            450,
            300,
            filter,
        )
        .to_rgba8();

        let (direct, d_mean, _) = stats(&reference_2d, &simd_2d);
        let (quantised, q_mean, _) = stats(&reference_2d, &two_step);
        println!(
            "PROBE {filter:?} simd-vs-direct2d max={direct} mean={d_mean:.4}; \
             u8-intermediate-vs-direct2d max={quantised} mean={q_mean:.4}"
        );
        let (reproduced, r_mean, _) = stats(&simd_2d, &two_step);
        println!(
            "PROBE {filter:?} simd-vs-u8-intermediate max={reproduced} mean={r_mean:.4}"
        );
    }
}

/// The definitive attribution: `fast_image_resize` does the *vertical* pass
/// first into a u8 intermediate and then the horizontal pass. Emulate exactly
/// that with the reference kernel and the output should match bit for bit.
#[test]
fn measure_pass_order() {
    let checker = |w: u32, h: u32| {
        image::RgbaImage::from_fn(w, h, |x, y| {
            if (x / 2 + y / 2) % 2 == 0 {
                image::Rgba([250, 8, 8, 255])
            } else {
                image::Rgba([10, 240, 10, 255])
            }
        })
    };
    let src = DynamicImage::ImageRgba8(checker(300, 200));
    for filter in [FilterType::Triangle, FilterType::CatmullRom, FilterType::Lanczos3] {
        let simd = try_resample_simd(&src, 450, 300, filter).unwrap().to_rgba8();
        let direct = resample_reference(&src, 450, 300, filter).to_rgba8();

        // fir's order: vertical into u8, then horizontal.
        let vert_first = resample_reference(&src, 300, 300, filter).to_rgba8();
        let emulated = resample_reference(
            &DynamicImage::ImageRgba8(vert_first),
            450,
            300,
            filter,
        )
        .to_rgba8();

        let horiz_first = resample_reference(&src, 450, 200, filter).to_rgba8();
        let emulated_h = resample_reference(
            &DynamicImage::ImageRgba8(horiz_first),
            450,
            300,
            filter,
        )
        .to_rgba8();

        let (m1, mean1, _) = stats(&simd, &emulated);
        let (m2, mean2, _) = stats(&simd, &emulated_h);
        let (m3, mean3, _) = stats(&simd, &direct);
        println!(
            "ORDER {filter:?} simd-vs-vertical-first-u8 max={m1} mean={mean1:.4}; \
             simd-vs-horizontal-first-u8 max={m2} mean={mean2:.4}; \
             simd-vs-direct-2d max={m3} mean={mean3:.4}"
        );
    }
}

/// One-axis-only resizes have no intermediate buffer at all, so any difference
/// there cannot be intermediate quantisation. Investigate.
#[test]
fn measure_one_axis_only() {
    let src = DynamicImage::ImageRgba8(fixture(64, 64));
    for filter in [
        FilterType::Triangle,
        FilterType::Gaussian,
        FilterType::CatmullRom,
        FilterType::Lanczos3,
    ] {
        for (dw, dh) in [(64u32, 1u32), (64, 2), (64, 4), (64, 8), (64, 16), (1, 64)] {
            let want = resample_reference(&src, dw, dh, filter).to_rgba8();
            let got = try_resample_simd(&src, dw, dh, filter).unwrap().to_rgba8();
            let (max, mean, _) = stats(&want, &got);
            if max > 0 {
                println!("ONEAxis {filter:?} 64x64->{dw}x{dh} max={max} mean={mean:.4}");
            }
        }
    }
    println!("ONEAxis done");
}

/// Is the one-axis Gaussian difference at the edges (window handling) or in
/// the interior (arithmetic)?
#[test]
fn measure_gaussian_edges() {
    let src = DynamicImage::ImageRgba8(fixture(64, 64));
    let filter = FilterType::Gaussian;
    let want = resample_reference(&src, 64, 1, filter).to_rgba8();
    let got = try_resample_simd(&src, 64, 1, filter).unwrap().to_rgba8();
    let mut diffs: Vec<u32> = Vec::new();
    for x in 0..64u32 {
        let mut d = 0u32;
        for c in 0..4 {
            d = d.max(
                u32::from(want.get_pixel(x, 0).0[c])
                    .abs_diff(u32::from(got.get_pixel(x, 0).0[c])),
            );
        }
        diffs.push(d);
    }
    println!("GAUSSIAN-ROW {diffs:?}");
    let mut interior = 0u32;
    for &d in &diffs[8..56] {
        interior = interior.max(d);
    }
    let mut edge = 0u32;
    for &d in diffs.iter().take(8).chain(diffs.iter().skip(56)) {
        edge = edge.max(d);
    }
    println!("GAUSSIAN-INTERIOR max={interior}");
    println!("GAUSSIAN-EDGE max={edge}");

    // Sum of a constant field should be preserved by either. Check: does a flat
    // grey survive both?
    let flat = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(64, 64, image::Rgba([137, 137, 137, 255])));
    let w = resample_reference(&flat, 64, 1, filter).to_rgba8();
    let g = try_resample_simd(&flat, 64, 1, filter).unwrap().to_rgba8();
    println!(
        "GAUSSIAN-FLAT image={:?} simd={:?}",
        w.get_pixel(32, 0).0,
        g.get_pixel(32, 0).0
    );
}

/// The crate's SIMD entry point takes a `&DynamicImage` and normalises to RGBA8
/// inside. On a 24 MP image that is a 96 MB allocation and copy, so it belongs
/// in the benchmark rather than being quietly excluded from it.
#[test]
fn measure_normalisation_cost() {
    let src = DynamicImage::ImageRgba8(fixture(6000, 4000));
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t = std::time::Instant::now();
        let rgba = src.to_rgba8();
        let e = t.elapsed().as_secs_f64() * 1000.0;
        best = best.min(e);
        std::hint::black_box(rgba);
    }
    println!("NORMALISE to_rgba8 on 6000x4000 = {best:.1}ms (96 MB of RGBA8)");

    // And what it costs as a share of the SIMD resize that follows.
    let img = DynamicImage::ImageRgba8(fixture(6000, 4000));
    let _ = try_resample_simd(&img, 400, 267, FilterType::Triangle).unwrap();
    let mut simd_total = f64::MAX;
    for _ in 0..5 {
        let t = std::time::Instant::now();
        let o = try_resample_simd(&img, 400, 267, FilterType::Triangle).unwrap();
        simd_total = simd_total.min(t.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(o);
    }
    println!(
        "NORMALISE share of simd 24MP->400: {:.1}ms of {simd_total:.1}ms = {:.0}%",
        best,
        best / simd_total * 100.0
    );

    // The same resize with the RGBA8 already extracted, for the comparison.
    let rgba = img.to_rgba8();
    let s = fast_image_resize::images::ImageRef::new(
        rgba.width(),
        rgba.height(),
        rgba.as_raw(),
        fast_image_resize::PixelType::U8x4,
    )
    .unwrap();
    let mut d = fast_image_resize::images::Image::new(400, 267, fast_image_resize::PixelType::U8x4);
    let opts = fast_image_resize::ResizeOptions::new()
        .resize_alg(fast_image_resize::ResizeAlg::Convolution(
            fast_image_resize::FilterType::Bilinear,
        ))
        .use_alpha(false);
    let mut kernel_only = f64::MAX;
    for _ in 0..5 {
        let t = std::time::Instant::now();
        fast_image_resize::Resizer::new().resize(&s, &mut d, &opts).unwrap();
        kernel_only = kernel_only.min(t.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(d.buffer());
    }
    println!("NORMALISE kernel only (no conversion) = {kernel_only:.1}ms");
}

/// Is the SIMD kernel bit-identical to "the reference kernel with its
/// intermediate rounded to u8"? If so, that is a far stronger assertion than
/// any tolerance: it proves the two are the same resampler and pins the whole
/// difference to one quantisation step.
#[test]
fn measure_exact_equivalence() {
    let filters = [
        FilterType::Triangle,
        FilterType::Gaussian,
        FilterType::CatmullRom,
        FilterType::Lanczos3,
    ];
    let mut mismatches = 0;
    for filter in filters {
        for ((sw, sh), (dw, dh)) in SIZE_PAIRS {
            let src = DynamicImage::ImageRgba8(fixture(sw, sh));
            let simd = try_resample_simd(&src, dw, dh, filter).unwrap().to_rgba8();
            // fir's order: vertical first (into u8), then horizontal.
            let step1 = resample_reference(&src, sw, dh, filter).to_rgba8();
            let emulated = resample_reference(
                &DynamicImage::ImageRgba8(step1),
                dw,
                dh,
                filter,
            )
            .to_rgba8();
            let (max, mean, _) = stats(&simd, &emulated);
            if max != 0 {
                mismatches += 1;
                println!(
                    "EXACT-MISMATCH {filter:?} {sw}x{sh}->{dw}x{dh} max={max} mean={mean:.4}"
                );
            }
        }
    }
    println!("EXACT mismatched_cases={mismatches} of {}", filters.len() * SIZE_PAIRS.len());
}

/// Localise the Gaussian residual: one axis at a time on the mixed fixture.
#[test]
fn measure_gaussian_localise() {
    let src = DynamicImage::ImageRgba8(fixture(64, 48));
    let filter = FilterType::Gaussian;
    for (dw, dh, label) in [
        (64u32, 24u32, "vertical only"),
        (32, 48, "horizontal only"),
        (32, 24, "both"),
    ] {
        let r = resample_reference(&src, dw, dh, filter).to_rgba8();
        let s = try_resample_simd(&src, dw, dh, filter).unwrap().to_rgba8();
        let (max, mean, _) = stats(&r, &s);
        println!("GAUSSLOC {label} {dw}x{dh} simd-vs-ref max={max} mean={mean:.4}");
    }
    // And the same case on a flat field, where a coefficient difference shows
    // as a DC offset rather than as ringing.
    let flat = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        64,
        48,
        image::Rgba([137, 90, 200, 255]),
    ));
    let r = resample_reference(&flat, 32, 24, filter).to_rgba8();
    let s = try_resample_simd(&flat, 32, 24, filter).unwrap().to_rgba8();
    let (max, mean, _) = stats(&r, &s);
    println!("GAUSSLOC flat 32x24 simd-vs-ref max={max} mean={mean:.4}");
    // Impulse: reveals the effective kernel directly.
    let imp = DynamicImage::ImageRgba8(image::RgbaImage::from_fn(64, 48, |x, y| {
        if x == 32 && y == 24 {
            image::Rgba([255, 255, 255, 255])
        } else {
            image::Rgba([0, 0, 0, 255])
        }
    }));
    let r = resample_reference(&imp, 32, 24, filter).to_rgba8();
    let s = try_resample_simd(&imp, 32, 24, filter).unwrap().to_rgba8();
    let mut row_r = String::new();
    let mut row_s = String::new();
    for x in 8..24 {
        row_r.push_str(&format!("{} ", r.get_pixel(x, 12).0[0]));
        row_s.push_str(&format!("{} ", s.get_pixel(x, 12).0[0]));
    }
    println!("GAUSSLOC impulse row image: {row_r}");
    println!("GAUSSLOC impulse row simd : {row_s}");
    let mut sum_r = 0i64;
    let mut sum_s = 0i64;
    for x in 0..32 {
        sum_r += i64::from(r.get_pixel(x, 12).0[0]);
        sum_s += i64::from(s.get_pixel(x, 12).0[0]);
    }
    println!("GAUSSLOC impulse row sums image={sum_r} simd={sum_s}");
}

/// Are the one-axis residuals confined to the boundary rows/columns?
#[test]
fn measure_gaussian_boundary() {
    let src = DynamicImage::ImageRgba8(fixture(64, 48));
    let filter = FilterType::Gaussian;
    let r = resample_reference(&src, 64, 24, filter).to_rgba8();
    let s = try_resample_simd(&src, 64, 24, filter).unwrap().to_rgba8();
    for y in 0..24u32 {
        let mut worst = 0u32;
        let mut at = 0u32;
        for x in 0..64u32 {
            let d = (0..4)
                .map(|c| u32::from(r.get_pixel(x, y).0[c]).abs_diff(u32::from(s.get_pixel(x, y).0[c])))
                .max()
                .unwrap();
            if d > worst {
                worst = d;
                at = x;
            }
        }
        println!("GAUSSROW y={y} worst={worst} at_x={at}");
    }
}

/// The contrast figure on the crate's own fixture: what two *defensible*
/// reference kernels cost, against what the SIMD kernel costs.
#[test]
fn measure_fixture_contrast() {
    let src = DynamicImage::ImageRgba8(fixture(300, 200));
    let l = resample_reference(&src, 450, 300, FilterType::Lanczos3).to_rgba8();
    for other in [
        FilterType::CatmullRom,
        FilterType::Gaussian,
        FilterType::Triangle,
    ] {
        let o = resample_reference(&src, 450, 300, other).to_rgba8();
        let (max, mean, _) = stats(&l, &o);
        println!("FIXCONTRAST lanczos-vs-{other:?} max={max} mean={mean:.4}");
    }
    for f in [
        FilterType::Triangle,
        FilterType::Gaussian,
        FilterType::CatmullRom,
        FilterType::Lanczos3,
    ] {
        let s = try_resample_simd(&src, 450, 300, f).unwrap().to_rgba8();
        let r = resample_reference(&src, 450, 300, f).to_rgba8();
        let (max, mean, _) = stats(&r, &s);
        println!("FIXSIMD {f:?} max={max} mean={mean:.4}");
    }
}
