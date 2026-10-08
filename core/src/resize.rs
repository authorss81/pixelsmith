//! The single resampling pass, and the two kernels that can perform it.
//!
//! Hard rule 5 is the reason this is one module with one entry point: exactly
//! one resampling pass ever touches the pixels. `pipeline::resize_to` decides
//! the size, the filter and the crop, then calls [`resample`] once. Nothing here
//! resamples twice, and nothing else in the engine reaches past that one call.
//!
//! Two kernels exist because one is fast and one is trusted:
//!
//! * [`resample_reference`] is `image`'s pure-Rust implementation. It is the
//!   default, it is never deleted, and it is the correctness oracle.
//! * [`simd::try_resample`] is `fast_image_resize`, behind the `simd` feature.
//!   It is opt-in because a second kernel is a permanent maintenance cost, and
//!   the measurement is in docs/BENCHMARKS.md.
//!
//! The SIMD path falls back to the reference whenever it cannot serve a
//! request, including when it returns an error. That is deliberate: an opt-in
//! optimisation must not be able to fail a resize the default build performs.
//! It cannot add a resampling pass either — the fallback *is* the pass.
//!
//! All three functions are public, and that is for the benchmark rather than
//! for callers: `docs/BENCHMARKS.md` is only meaningful if the numbers came
//! from the same entry points production uses, and `resample_reference` in
//! particular has to be callable without the `simd` feature for the two columns
//! to be produced by one build.

use image::DynamicImage;
use image::imageops::FilterType;

/// Perform the one resampling pass, through whichever kernel this build has.
///
/// This is the function `pipeline::resize_to` calls, and the only resampling
/// call in the crate. It is public so a benchmark or a caller that wants to
/// measure this build's kernel has the same entry point production uses.
pub fn resample(img: &DynamicImage, width: u32, height: u32, filter: FilterType) -> DynamicImage {
    #[cfg(feature = "simd")]
    if let Some(out) = simd::try_resample(img, width, height, filter) {
        return out;
    }
    resample_reference(img, width, height, filter)
}

/// The reference kernel, and the oracle every other kernel is checked against.
///
/// `imageops::resize` is generic over the concrete buffer, which is why this
/// normalises the result back into a `DynamicImage` — and why it has always
/// produced **RGBA8**, whatever the source buffer was: the generic parameter
/// here is `DynamicImage`, and `GenericImageView for DynamicImage` fixes
/// `Pixel = Rgba<u8>`. `reference_output_is_always_rgba8` pins that, because
/// the SIMD kernel below relies on it: normalising to RGBA8 there costs nothing
/// that is not already being paid here.
pub fn resample_reference(
    img: &DynamicImage,
    width: u32,
    height: u32,
    filter: FilterType,
) -> DynamicImage {
    DynamicImage::from(image::imageops::resize(img, width, height, filter))
}

/// The SIMD kernel, behind the `simd` feature.
///
/// The module exists so the whole second kernel is one `#[cfg]`, and so
/// [`crate::resize::resample`] reads as "try the SIMD kernel, else the reference"
/// rather than as a maze of feature-gated free functions.
#[cfg(feature = "simd")]
pub mod simd {
    use super::*;
    use fast_image_resize::images::{Image as FirImage, ImageRef};
    use fast_image_resize::{PixelType, ResizeAlg, ResizeOptions, Resizer};

    /// The SIMD kernel, or `None` to mean "use the reference kernel instead".
    ///
    /// `None` covers a filter this crate deliberately does not take (`Nearest`;
    /// see `algorithm_for` below) and an error out of the resizer. There is no
    /// `unwrap` on either: the buffers are built from widths and heights this
    /// crate has already resolved, but they still come from a caller, and hard
    /// rule 3 is about callers.
    ///
    /// Public because [`super::resample`] falls back on `None` and a benchmark
    /// has to be able to time the SIMD kernel on its own — otherwise the two
    /// columns in docs/BENCHMARKS.md would be "SIMD" and "SIMD, sometimes",
    /// which is not a comparison.
    pub fn try_resample(
        img: &DynamicImage,
        width: u32,
        height: u32,
        filter: FilterType,
    ) -> Option<DynamicImage> {
        let algorithm = algorithm_for(filter)?;
        resize_alg(img, width, height, algorithm)
    }

    /// Run one algorithm, with no filter mapping in the way.
    ///
    /// Split out from [`try_resample`] so the one test that needs the SIMD crate
    /// to answer for a filter `try_resample` refuses can ask for it without
    /// reintroducing the mapping. `pub(super)` rather than private because that
    /// test lives in a sibling module; it is not part of the crate's API.
    pub(super) fn resize_alg(
        img: &DynamicImage,
        width: u32,
        height: u32,
        algorithm: ResizeAlg,
    ) -> Option<DynamicImage> {
        // Straight alpha, to match the reference. `image`'s kernel filters the
        // four channels independently and never premultiplies; this crate
        // defaults to premultiplying. Premultiplied alpha is the better answer
        // for a transparent PNG, but changing how transparency is resampled is a
        // decision rather than an optimisation, and it would make this flag a
        // behaviour change instead of a speed one. It belongs in the
        // colour-management phase, applied to both kernels.
        let options = ResizeOptions::new().resize_alg(algorithm).use_alpha(false);
        // Normalise to RGBA8 once, here, rather than in a match over buffer
        // types per call. See `resample_reference`: the reference kernel already
        // returns RGBA8 for every input type, so this is not a narrowing of what
        // the pipeline can handle — it is the same conversion, done once and in
        // one place instead of per branch.
        let src = img.to_rgba8();
        let src_view =
            ImageRef::new(src.width(), src.height(), src.as_raw(), PixelType::U8x4).ok()?;
        let mut dst = FirImage::new(width, height, PixelType::U8x4);
        Resizer::new().resize(&src_view, &mut dst, &options).ok()?;
        let buffer = dst.into_vec();
        Some(DynamicImage::ImageRgba8(image::RgbaImage::from_raw(
            width, height, buffer,
        )?))
    }

    /// `image`'s five filters onto this crate's kernels.
    ///
    /// `image::imageops::FilterType` is not `#[non_exhaustive]`, so this match is
    /// exhaustive and adding a variant upstream is a compile error here rather
    /// than a filter that silently keeps using the reference.
    ///
    /// **`Nearest` returns `None`, and it is not because the kernel is missing.**
    /// Both crates implement nearest-neighbour; they disagree about *which*
    /// source pixel a destination pixel takes when the destination sample lands
    /// exactly halfway between two source pixels, and there is no tolerance that
    /// covers that, because the difference is a whole source pixel rather than a
    /// rounding step.
    ///
    /// `image` computes the source position directly as `(y + 0.5) * ratio`
    /// (`imageops::sample::vertical_sample`). `fast_image_resize` walks the rows
    /// by accumulating `y += step` (`images::typed_image::iter_rows_with_step`)
    /// and truncating, so accumulated `f64` error decides the tie instead of the
    /// exact value. Measured on a 4x4 row-ramp upscaled to 6x6, `image` picks
    /// source rows `[0, 1, 1, 2, 3, 3]` and `fast_image_resize` picks
    /// `[0, 1, 1, 2, 2, 3]` — the row where `(3 + 0.5) * (4/6)` is exactly
    /// `3.0` and the accumulated `0.3333… + 3 × 0.6666…` is `2.9999999999999996`.
    ///
    /// That matters more than a blur would. `ResampleFilter::Nearest` is
    /// documented for pixel art and icons, where a pixel that moves is a visible
    /// defect rather than a soft edge. Routing it to the reference keeps this
    /// flag a speed change: the four convolution filters get the SIMD kernel,
    /// and pixel art gets exactly the pixels it got before. Reinstating it needs
    /// a `fast_image_resize` fix, not a tolerance — and
    /// `tests::simd_agreement::the_two_nearest_implementations_really_do_disagree`
    /// fails the moment that changes, which is how it would be noticed.
    fn algorithm_for(filter: FilterType) -> Option<ResizeAlg> {
        use fast_image_resize::FilterType as FirFilter;
        let algorithm = match filter {
            FilterType::Nearest => return None,
            FilterType::Triangle => ResizeAlg::Convolution(FirFilter::Bilinear),
            FilterType::CatmullRom => ResizeAlg::Convolution(FirFilter::CatmullRom),
            FilterType::Gaussian => ResizeAlg::Convolution(FirFilter::Gaussian),
            FilterType::Lanczos3 => ResizeAlg::Convolution(FirFilter::Lanczos3),
        };
        Some(algorithm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// Every filter `image` offers. The cross-check matrix below is over this
    /// list, so a filter added upstream is covered or fails to compile.
    const FILTERS: [FilterType; 5] = [
        FilterType::Nearest,
        FilterType::Triangle,
        FilterType::CatmullRom,
        FilterType::Gaussian,
        FilterType::Lanczos3,
    ];

    /// A fixture built to make disagreement visible rather than to flatter a
    /// kernel.
    ///
    /// Smooth gradients are where any two resamplers agree, because there is
    /// nothing in them for a filter to get wrong, so most of this is a smooth
    /// wave (a photograph-ish picture, where a difference means one of the two
    /// kernels is broken). The hard 2px checkerboard in the top-left quarter is
    /// the opposite case: high-frequency content, where a kernel that weights
    /// the wrong samples shows up immediately. The bottom half ramps alpha from
    /// 0 to 255, which is where premultiplying alpha and filtering it straight
    /// visibly part company — the test that would catch that setting being
    /// flipped.
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
            Rgba([r, g, b, alpha])
        })
    }

    /// Largest absolute per-channel difference, the mean of them, and where the
    /// worst one was. Both are reported so a failure says which of the two
    /// "off by one rounding step" or "off by a whole kernel" it is.
    ///
    /// This and the two functions below it live in `simd_agreement` rather than
    /// here, and used to be here. They exist to compare the two kernels, so
    /// without the `simd` feature nothing calls them, and a default build reported
    /// three `never used` warnings for them — invisible for as long as the gate
    /// ran clippy with `--all-features` and nothing else. Which is the same class
    /// of defect as audit finding 2, one level down: a `cfg` on one side of a
    /// reference and not the other. `scripts/feature-matrix.sh` now compiles
    /// every configuration, so the next one of these is caught at the gate.
    #[cfg(feature = "simd")]
    #[derive(Debug)]
    struct Difference {
        max: u32,
        mean: f64,
        at: (u32, u32, usize),
    }

    #[cfg(feature = "simd")]
    fn difference(a: &image::RgbaImage, b: &image::RgbaImage) -> Difference {
        assert_eq!(a.dimensions(), b.dimensions());
        let mut max = 0u32;
        let mut total = 0.0f64;
        let mut count = 0.0f64;
        let mut at = (0u32, 0u32, 0usize);
        for (x, y, pa) in a.enumerate_pixels() {
            let pb = b.get_pixel(x, y);
            for (channel, (va, vb)) in pa.0.iter().zip(pb.0.iter()).enumerate() {
                let d = u32::from(*va).abs_diff(u32::from(*vb));
                if d > max {
                    max = d;
                    at = (x, y, channel);
                }
                total += f64::from(d);
                count += 1.0;
            }
        }
        Difference {
            max,
            mean: total / count,
            at,
        }
    }

    #[cfg(feature = "simd")]
    fn reference(
        img: &image::RgbaImage,
        width: u32,
        height: u32,
        filter: FilterType,
    ) -> image::RgbaImage {
        let src = DynamicImage::ImageRgba8(img.clone());
        resample_reference(&src, width, height, filter).to_rgba8()
    }

    #[test]
    fn reference_output_is_always_rgba8() {
        // The claim the SIMD path's single normalisation rests on. Every buffer
        // type `DynamicImage` can hold, resized through the reference kernel:
        // RGBA8 out, every time, because `GenericImageView for DynamicImage`
        // fixes `Pixel = Rgba<u8>`. If this ever stops being true, the SIMD path
        // is narrowing what the pipeline can produce and this test is the thing
        // that says so.
        let luma = image::DynamicImage::ImageLuma8(image::GrayImage::from_fn(9, 7, |x, _| {
            image::Luma([(x * 28) as u8])
        }));
        let luma_a =
            image::DynamicImage::ImageLumaA8(image::GrayAlphaImage::from_fn(9, 7, |x, _| {
                image::LumaA([(x * 28) as u8, 255])
            }));
        let rgb = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(9, 7, |x, y| {
            image::Rgb([(x * 28) as u8, (y * 36) as u8, 7])
        }));
        let rgba = image::DynamicImage::ImageRgba8(fixture(9, 7));
        // `image::Rgb16Image` and `Rgba16Image` are `pub(crate)` in image 0.25,
        // so the buffer is spelled out. Not worth a `to_rgb16()` round trip
        // either: what is under test is which buffer type `resample_reference`
        // is handed, not how the fixture was built.
        let rgb16 = image::DynamicImage::ImageRgb16(image::ImageBuffer::from_fn(9, 7, |x, y| {
            image::Rgb([(x * 3000) as u16, (y * 7000) as u16, 7])
        }));
        let rgba16 = image::DynamicImage::ImageRgba16(image::ImageBuffer::from_fn(9, 7, |x, y| {
            image::Rgba([(x * 3000) as u16, (y * 7000) as u16, 7, 65535])
        }));
        let rgba32f =
            image::DynamicImage::ImageRgba32F(image::Rgba32FImage::from_fn(9, 7, |x, y| {
                image::Rgba([x as f32 / 8.0, y as f32 / 6.0, 0.5, 1.0])
            }));

        for src in [luma, luma_a, rgb, rgba, rgb16, rgba16, rgba32f] {
            for filter in FILTERS {
                let out = resample_reference(&src, 5, 4, filter);
                assert!(
                    matches!(out, DynamicImage::ImageRgba8(_)),
                    "reference kernel returned {:?} for filter {filter:?}",
                    out.color()
                );
            }
        }
    }

    #[cfg(feature = "simd")]
    mod simd_agreement {
        use super::*;

        /// Per-filter tolerances for the matrix below: the largest per-channel
        /// difference permitted, and the largest mean per-channel difference
        /// permitted. Both are measured, not chosen, and both matter.
        ///
        /// **Why the arithmetic differs at all.** The two kernels run the same
        /// maths to different precision. `image` accumulates in `f32` and rounds
        /// once at the end — `sample::horizontal_sample` builds an
        /// `Rgba32FImage`, accumulates, and clips through `FloatNearest`, which
        /// is a `round()`. `fast_image_resize` instead quantises each
        /// convolution coefficient to an `i16` scaled by `1 << precision`, where
        /// `precision` is the smallest value in `4..=22` that keeps the largest
        /// coefficient inside an `i16` (`convolution::optimisations::Normalizer16`),
        /// accumulates in `i32`, and clips through a 1280-entry lookup table with
        /// an arithmetic right shift. So the SIMD kernel truncates rather than
        /// rounds, and its weights carry 4 to 22 fractional bits depending on the
        /// filter. Neither is "wrong"; they are two correctly-rounded resamplers
        /// of the same kernel, and the difference is arithmetic rather than a
        /// disagreement about which source pixels contribute.
        ///
        /// **Why the difference tracks the filter's negative lobes.** Both
        /// effects above are worth a fraction of one output step, so a smooth
        /// area barely moves. What makes it visible is ringing: a kernel with
        /// negative lobes turns a hard edge into an overshoot and an undershoot,
        /// and the overshoot is where the truncation bias and the coefficient
        /// quantisation have the most leverage. That is why `Triangle` — a tent
        /// function with no negative part — comes out at a difference of 1, and
        /// `Lanczos3`, which has four of them, comes out at 29. A single flat
        /// tolerance would have to be set at the Lanczos3 figure and would then
        /// assert almost nothing about `Triangle`.
        ///
        /// **The measurements**, on the fixture in this module over the four
        /// filters and the eight size pairs in `SIZE_PAIRS` (`maximum over
        /// pairs`, then the worst filter):
        ///
        /// | filter        | worst per-channel | worst mean |
        /// | ------------- | ----------------- | ---------- |
        /// | `Triangle`    | 1                 | 0.12       |
        /// | `Gaussian`    | 7                 | 0.15       |
        /// | `CatmullRom`  | 15                | 0.25       |
        /// | `Lanczos3`    | 29                | 0.48       |
        ///
        /// Identical in debug and in release, which is what integer accumulation
        /// buys: `fast_image_resize`'s i32 path cannot be reassociated by the
        /// optimiser, and `image`'s f32 accumulation does not move when this
        /// matrix is measured either way.
        ///
        /// **Why the mean bound is asserted as well as the maximum**, because the
        /// maximum on its own is a weak detector and pretending otherwise would be
        /// the dishonest choice. The nearest *wrong* kernel on this fixture is
        /// `Triangle` against `Gaussian`, and it differs by a maximum of **31** —
        /// against a correct-kernel maximum of 29 for `Lanczos3`. A per-channel
        /// maximum therefore cannot separate "same kernel, different arithmetic"
        /// from "adjacent filter" for the two ringing kernels, and no tolerance
        /// can fix that: it is a property of the fixture, which puts a 2px
        /// checkerboard in its top-left quarter specifically to make disagreement
        /// visible. The mean separates cleanly, because the wrong kernel lifts the
        /// whole picture rather than a handful of ringing pixels: no wrong-kernel
        /// pairing anywhere in the square stays under **1.17**, and most are above 3.
        ///
        /// So each filter gets both bounds, chosen to sit between its own
        /// measurement and the wrong-kernel floor. The two floor columns are the
        /// *minimum* over the three wrong kernels each filter could be confused
        /// with, measured by `a_wrong_kernel_always_lands_outside_the_tolerance`
        /// below:
        ///
        /// | filter       | per-channel max | worst mean | nearest wrong max | nearest wrong mean |
        /// | ------------ | --------------- | ---------- | ----------------- | ------------------ |
        /// | `Triangle`   | 1 → **2**       | 0.12 → **0.4** | 31              | 1.44               |
        /// | `Gaussian`   | 7 → **10**      | 0.15 → **0.4** | 31              | 1.45               |
        /// | `CatmullRom` | 15 → **20**     | 0.25 → **0.6** | 41              | 1.22               |
        /// | `Lanczos3`   | 29 → **36**     | 0.48 → **0.8** | 47              | 1.17               |
        ///
        /// Every maximum bound sits below its floor — a sixth of it for
        /// `Triangle`, three quarters for `Lanczos3` — and every mean bound
        /// below half of it. The mean column is the one that would catch a mistake in
        /// `simd::algorithm_for` on its own; the maximum column is the one that
        /// catches a rounding change in either crate without failing the build.
        const TOLERANCES: [((FilterType, u32), u32); 4] = [
            // (filter, per-channel maximum allowed, mean allowed x10)
            ((FilterType::Triangle, 2), 4), // a mean of 0.4
            ((FilterType::Gaussian, 10), 4),
            ((FilterType::CatmullRom, 20), 6),
            ((FilterType::Lanczos3, 36), 8),
        ];

        /// The tolerance pair for one filter. `mean_tenths` is tenths of an LSB,
        /// so the table above stays in integers and the comparison is exact
        /// enough not to depend on how `f64` rounds.
        fn tolerance_for(filter: FilterType) -> (u32, u32) {
            let (key, mean_tenths) = TOLERANCES
                .iter()
                .find(|((f, _), _)| *f == filter)
                .expect("every filter the SIMD kernel takes has a tolerance");
            (key.1, *mean_tenths)
        }

        /// Six size pairs at minimum; eight here, chosen to cover the cases
        /// where resamplers are most likely to part company rather than to pad
        /// the count: a 1-pixel target, two exact integer downscales, a
        /// non-integer ratio, a sub-pixel shrink (the worst case for filter
        /// alignment — every destination sample lands between the same two
        /// source samples), a large upscale, and a one-axis-only resize.
        const SIZE_PAIRS: [((u32, u32), (u32, u32)); 8] = [
            ((64, 48), (32, 24)),     // exact 2:1
            ((64, 48), (1, 1)),       // 1-pixel target
            ((300, 200), (100, 67)),  // 3:1, non-integer
            ((300, 200), (450, 300)), // 1.5:1 upscale
            ((97, 61), (400, 251)),   // upscale from awkward source dims
            ((512, 384), (17, 13)),   // ~30:1 downscale
            ((256, 256), (255, 255)), // sub-pixel shrink
            ((64, 64), (64, 1)),      // one axis only
        ];

        /// The filters the SIMD kernel takes. `Nearest` is deliberately absent; see
        /// `simd::algorithm_for`, where the measurement is.
        const SIMD_FILTERS: [FilterType; 4] = [
            FilterType::Triangle,
            FilterType::CatmullRom,
            FilterType::Gaussian,
            FilterType::Lanczos3,
        ];

        #[test]
        fn every_convolution_filter_reaches_the_simd_kernel() {
            // The cross-check below compares two kernels, which is only a
            // two-kernel comparison if the second one actually ran. `resample`
            // falls back to the reference silently, so a filter that failed to
            // map would make the matrix compare the reference against itself and
            // pass with a difference of exactly zero. Asserting reachability
            // separately is what stops that being a green test of nothing.
            let src = DynamicImage::ImageRgba8(fixture(40, 30));
            for filter in SIMD_FILTERS {
                assert!(
                    simd::try_resample(&src, 17, 11, filter).is_some(),
                    "{filter:?} did not reach the SIMD kernel, so the cross-check \
                     would be comparing the reference against itself"
                );
            }
        }

        #[test]
        fn nearest_is_refused_by_the_simd_kernel_not_approximated() {
            // `algorithm_for` returns `None` for `Nearest`, which means
            // `resample` runs the reference. Asserted rather than assumed
            // because the alternative failure is silent and invisible: a
            // tolerance-based matrix would either pass Nearest by widening the
            // tolerance past 255 or fail it, and in both cases the picture the
            // user gets of their pixel art would have changed without anything
            // saying so. This test is the thing that says so.
            assert!(
                simd::try_resample(
                    &DynamicImage::ImageRgba8(fixture(40, 30)),
                    60,
                    45,
                    FilterType::Nearest
                )
                .is_none(),
                "Nearest now reaches the SIMD kernel, which changes which source \\
                 pixel a pixel-art export takes; read simd::algorithm_for before \\
                 removing this"
            );

            // And the observable consequence, which is the thing that matters:
            // an export of pixel art is byte-identical to the reference, so
            // turning this feature on cannot move a pixel.
            let src = fixture(64, 48);
            let reference = reference(&src, 96, 72, FilterType::Nearest);
            let shipped = resample(&DynamicImage::ImageRgba8(src), 96, 72, FilterType::Nearest);
            assert_eq!(
                reference.as_raw(),
                shipped.to_rgba8().as_raw(),
                "a Nearest export no longer matches the reference kernel"
            );
        }

        #[test]
        fn the_two_nearest_implementations_really_do_disagree() {
            // The measurement `algorithm_for`'s comment rests on, kept as a
            // test so it cannot rot. This is the reason Nearest is refused, so
            // it belongs next to the refusal rather than only in prose: if a
            // future `fast_image_resize` fixes the row-walk, this test fails and
            // says the refusal can be reconsidered.
            //
            // Built on `resample_reference` plus `simd::resize_alg` directly,
            // because `try_resample` cannot express "give me the SIMD answer" now
            // that Nearest is refused — which is the point being measured.
            // A ramp whose value identifies the source row, so the chosen row is
            // readable straight off the output.
            let ramp = || image::RgbaImage::from_fn(4, 4, |_, y| Rgba([(y as u8) * 60, 0, 0, 255]));

            let image_rows: Vec<u32> = {
                let out = reference(&ramp(), 6, 6, FilterType::Nearest);
                (0..6)
                    .map(|y| u32::from(out.get_pixel(0, y).0[0]) / 60)
                    .collect()
            };
            let fir_rows: Vec<u32> = {
                // Asked of the SIMD crate directly, because `try_resample` now
                // refuses Nearest — which is the point being measured.
                let got = simd::resize_alg(
                    &DynamicImage::ImageRgba8(ramp()),
                    6,
                    6,
                    fast_image_resize::ResizeAlg::Nearest,
                )
                .expect("Nearest is still offered by the SIMD crate itself")
                .to_rgba8();
                (0..6)
                    .map(|y| u32::from(got.get_pixel(0, y).0[0]) / 60)
                    .collect()
            };

            assert_eq!(
                image_rows,
                vec![0, 1, 1, 2, 3, 3],
                "`image`'s nearest row selection changed; re-measure before \
                 trusting the comment on algorithm_for"
            );
            assert_eq!(
                fir_rows,
                vec![0, 1, 1, 2, 2, 3],
                "`fast_image_resize`'s nearest row selection changed, so the \
                 disagreement this documents is gone and Nearest can be \
                 reinstated"
            );
            assert_ne!(
                image_rows, fir_rows,
                "the two now agree; revisit the refusal"
            );
        }

        #[test]
        fn the_two_kernels_are_actually_different_implementations() {
            // The mirror of the test above, from the other side: the matrix is
            // only meaningful if `resample` is really taking the SIMD branch.
            // If the feature were wired up but never dispatched, the two would
            // be byte-identical everywhere and every tolerance above would be
            // vacuous. Nearest is excluded because two nearest-neighbour
            // samplers genuinely agree on which source pixel each destination
            // pixel takes, so identical output is the correct answer there.
            let src = DynamicImage::ImageRgba8(fixture(300, 200));
            for filter in [
                FilterType::Triangle,
                FilterType::CatmullRom,
                FilterType::Gaussian,
                FilterType::Lanczos3,
            ] {
                let reference = resample_reference(&src, 100, 67, filter).to_rgba8();
                let simd = resample(&src, 100, 67, filter).to_rgba8();
                assert_ne!(
                    reference.as_raw(),
                    simd.as_raw(),
                    "{filter:?} produced byte-identical output, so this is not \
                     two kernels"
                );
            }
        }

        #[test]
        fn the_simd_kernel_normalises_to_rgba8_like_the_reference() {
            // The colour-type claim from `resample_reference`, asserted on the
            // SIMD side for the same reason: one normalisation, one place, and
            // a conversion that cannot have changed what the pipeline produces.
            let rgb = DynamicImage::ImageRgb8(image::RgbImage::from_fn(40, 30, |x, y| {
                image::Rgb([(x * 6) as u8, (y * 8) as u8, 9])
            }));
            let rgba16 = DynamicImage::ImageRgba16(image::ImageBuffer::from_fn(40, 30, |x, y| {
                image::Rgba([(x * 1600) as u16, (y * 2000) as u16, 7, 65535])
            }));
            for src in [rgb, rgba16] {
                for filter in FILTERS {
                    // All five here, `Nearest` included: this asserts the
                    // *output colour type*, which is a property of the RGBA8
                    // normalisation rather than of which kernel ran.
                    let out = resample(&src, 17, 11, filter);
                    assert!(
                        matches!(out, DynamicImage::ImageRgba8(_)),
                        "SIMD kernel returned {:?} for filter {filter:?}",
                        out.color()
                    );
                }
            }
        }

        #[test]
        fn the_two_kernels_agree_across_every_convolution_filter_and_size() {
            for filter in SIMD_FILTERS {
                let (max_tolerance, mean_tenths) = tolerance_for(filter);
                for ((sw, sh), (dw, dh)) in SIZE_PAIRS {
                    let src = fixture(sw, sh);
                    let want = reference(&src, dw, dh, filter);
                    let got = resample(&DynamicImage::ImageRgba8(src), dw, dh, filter).to_rgba8();
                    assert_eq!(
                        got.dimensions(),
                        (dw, dh),
                        "{filter:?} {sw}x{sh} -> {dw}x{dh}: wrong size"
                    );
                    let diff = difference(&want, &got);
                    let mean_tenths_allowed = f64::from(mean_tenths) / 10.0;
                    assert!(
                        diff.max <= max_tolerance,
                        "{filter:?} {sw}x{sh} -> {dw}x{dh}: worst per-channel \
                         difference {} (mean {:.3}) at {:?}, tolerance {max_tolerance}",
                        diff.max,
                        diff.mean,
                        diff.at
                    );
                    // The aggregate bound is the one that would catch a filter
                    // mapped onto the wrong kernel; see `TOLERANCES` for why
                    // the per-channel maximum cannot do that job on its own.
                    assert!(
                        diff.mean <= mean_tenths_allowed,
                        "{filter:?} {sw}x{sh} -> {dw}x{dh}: mean per-channel \
                         difference {:.3} exceeds {:.1}, which is above this \
                         fixture's wrong-kernel floor of ~1.2 — so this looks \
                         like a different kernel rather than different rounding; \
                         worst single pixel was {} at {:?}",
                        diff.mean,
                        mean_tenths_allowed,
                        diff.max,
                        diff.at
                    );
                }
            }
        }

        /// A 10000 -> 50 downscale, on a tolerance of its own.
        ///
        /// Extreme decimation is where two resamplers are most likely to
        /// disagree for a structural reason rather than an arithmetic one, so
        /// it is not folded into the matrix above. Both kernels scale their
        /// support by the reduction ratio — `image` via `sratio`
        /// (`sample.rs`), `fast_image_resize` via `adaptive_kernel_size` — so
        /// both are proper area averages rather than point samples, and both
        /// get the same `Triangle`/bilinear filter from the engine's
        /// extreme-reduction fallback in `pipeline::resize_to`. That the
        /// fallback picks the *same* filter for both is the thing being
        /// asserted: a kernel with its own decimation would silently disagree
        /// here and nowhere else.
        #[test]
        fn an_extreme_reduction_agrees_on_its_own_tolerance() {
            const WIDE: u32 = 10_000;
            const NARROW: u32 = 50;

            // Half the fixture is opaque and half ramps alpha, so the tolerance
            // below is measuring the kernel rather than the alpha setting.
            let src = fixture(WIDE, 200);

            for filter in [FilterType::Triangle, FilterType::Lanczos3] {
                let (max_tolerance, mean_tenths) = tolerance_for(filter);
                let mean_tenths_allowed = f64::from(mean_tenths) / 10.0;
                let want = reference(&src, NARROW, 2, filter);
                let got =
                    resample(&DynamicImage::ImageRgba8(src.clone()), NARROW, 2, filter).to_rgba8();
                assert_eq!(got.dimensions(), (NARROW, 2));
                let diff = difference(&want, &got);
                assert!(
                    diff.max <= max_tolerance && diff.mean <= mean_tenths_allowed,
                    "{filter:?} {WIDE}x200 -> {NARROW}x2: worst per-channel \
                     difference {} at {:?} and mean {:.3}; tolerances \
                     {max_tolerance} and {mean_tenths_allowed:.1}",
                    diff.max,
                    diff.at,
                    diff.mean
                );
            }
        }

        /// The claim `TOLERANCES` rests on, kept as a test so it cannot rot.
        ///
        /// The tolerance table says "a per-channel maximum of 31 is what the
        /// nearest wrong kernel produces on this fixture". That sentence is what
        /// justifies a per-filter table at all, and it is the sentence most
        /// likely to become wrong the next time either crate is upgraded — not
        /// by changing the tolerance, which nobody would notice, but by quietly
        /// widening the gap the other way. So it is measured in the suite.
        ///
        /// Every ordered pair of the four convolution filters, on the worst size
        /// pairs from `SIZE_PAIRS`, and each is asserted to exceed the
        /// corresponding correct-kernel tolerance. If a future `image` or
        /// `fast_image_resize` release makes two filters converge — or diverge
        /// further — this fails and the table is re-measured rather than
        /// inherited.
        #[test]
        fn a_wrong_kernel_always_lands_outside_the_tolerance() {
            const PROBES: [((u32, u32), (u32, u32)); 5] = [
                ((300, 200), (450, 300)),
                ((97, 61), (400, 251)),
                ((256, 256), (255, 255)),
                ((64, 64), (64, 1)),
                ((512, 384), (17, 13)),
            ];

            for intended in SIMD_FILTERS {
                let (tolerance, _) = tolerance_for(intended);
                for wrong in SIMD_FILTERS {
                    if intended == wrong {
                        continue;
                    }
                    let mut worst = 0u32;
                    let mut worst_mean = 0.0f64;
                    for ((sw, sh), (dw, dh)) in PROBES {
                        let src = DynamicImage::ImageRgba8(fixture(sw, sh));
                        let want = resample_reference(&src, dw, dh, intended).to_rgba8();
                        // Asked of `try_resample` with the *wrong* filter, which
                        // is the mistake this test is about: a filter mapped onto
                        // the wrong kernel inside `simd::algorithm_for`.
                        let got = simd::try_resample(&src, dw, dh, wrong)
                            .expect("every convolution filter reaches the SIMD kernel")
                            .to_rgba8();
                        let diff = difference(&want, &got);
                        worst = worst.max(diff.max);
                        worst_mean = worst_mean.max(diff.mean);
                    }
                    assert!(
                        worst > tolerance,
                        "reference-{intended:?} against SIMD-{wrong:?} differs by \
                         only {worst} at worst, inside the {tolerance} tolerance \
                         for {intended:?}; the tolerance table no longer separates \
                         this filter from the one it would be confused with"
                    );
                    assert!(
                        worst_mean > f64::from(tolerance_for(intended).1) / 10.0,
                        "reference-{intended:?} against SIMD-{wrong:?} has a mean \
                         difference of only {worst_mean:.3}, inside the tolerance \
                         for {intended:?}"
                    );
                }
            }
        }
    }
}
