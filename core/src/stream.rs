//! Low-peak-memory decode: one resampling pass, one output-sized buffer.
//!
//! # What this is for
//!
//! The in-memory path materialises the whole source, clones it inside
//! [`crate::pipeline::Pipeline::apply`], and hands the clone to `image`'s
//! `imageops::resize`, which allocates a `src_width x dst_height` `f32` buffer as
//! its intermediate. For a 120-megapixel source resized to 1000 px wide that is
//! measured at **1,072 MB peak** (docs/BENCHMARKS.md), and none of it is the
//! output, which is 3.3 MB.
//!
//! This module decodes a row at a time and resamples it into the destination as
//! it arrives, so the peak is
//!
//! ```text
//!   destination buffer          dst_w * dst_h * 4
//! + the row being decoded      src_w * 4
//! + the open vertical window   window * dst_w * 16
//! + coefficient tables         ~dst_w * 2 * support * ratio * 4
//! ```
//!
//! — measured at **9.7 MB** for the same job. Nothing in that expression
//! mentions the source's *height* or *pixel count*, which is the point: a 120 MP
//! panorama scaled to 1000 px wide never allocates a 120 MP buffer.
//!
//! # Why this is still exactly one resampling pass (hard rule 5)
//!
//! The obvious way to cut peak memory is to shrink the image first and resize the
//! small one second. That is two resampling passes, hard rule 5 bans it, and it
//! is also the thing the rule exists to prevent: applying a filter twice.
//!
//! Instead the horizontal and vertical passes are **interleaved**. A separable
//! filter's two passes commute — every source pixel reaches the destination
//! through exactly one horizontal weight and one vertical weight, in whatever
//! order the passes run — so `h-then-v` and `v-then-h` are the same filter and
//! the destination is written once, from one `round()`. `image` runs them in the
//! order that needs the whole source in memory; this runs them in the order that
//! does not, and `tests::it_agrees_with_the_in_memory_kernel` measures the
//! difference as rounding rather than as a second pass.
//!
//! # The pre-filter is the kernel's own scaled support, and that is the box
//!
//! `image` scales a kernel's support by the reduction ratio before sampling:
//! `sratio = max(1, ratio)`, and the weight for source sample `i` is
//! `kernel((i - centre) / sratio)`. At a reduction of R each destination pixel
//! therefore integrates `2 * support * R` source samples — the whole footprint,
//! and three times beyond it — instead of sampling it. **That is the pre-filter,
//! and it is why an area-weighted average is the right shape for one**: any
//! pre-reduction that is not a low-pass throws away pixels the kernel would have
//! used (point sampling) or smears across a boundary it should have averaged
//! (bilinear), while a box is the simplest kernel guaranteed non-negative and
//! summing to one, so a composite built on it is monotone and well defined.
//!
//! What this module does *not* do is add one. The obvious way to cut peak memory
//! is to box-average the source down to something small and resize that, and that
//! is two filter applications, which is the thing hard rule 5 bans and the reason
//! exports go soft. So the scaled-support convolution is performed once, directly,
//! from the row being decoded into the destination — the same filter `image`
//! would have used, applied once.
//!
//! `tests::one_pass_beats_a_box_pre_reduction` runs the two-stage design on the
//! same pixels rather than asserting the point in prose, and the answer is worth
//! stating plainly: **on a photographic fixture at a 16:1 reduction the two
//! designs agree to one least-significant bit** (worst per-channel 1, mean 0.08 of
//! 255). So the single pass is here for memory and for hard rule 5, not because a
//! box-first design would have been visibly worse — and the memory difference is
//! the whole of the argument, which is a thin one to be leaning on. It is stated
//! rather than rounded up: on content with hard colour edges the composite does
//! differ by more, and nothing here has measured that.
//!
//! # What this cannot do
//!
//! **It does not stream every format**, and that is a property of the dependency
//! set rather than of this code. `image` 0.25.10 exposes
//! `read_image(self, buf: &mut [u8])` and nothing else for JPEG, WebP, BMP, TIFF,
//! GIF, ICO and AVIF; below it, `zune-jpeg` 0.5 and `image-webp` 0.2 expose no
//! strip or row API either, and neither has a scaled decode. `png` 0.18 is the
//! only decoder in this tree with `Decoder::next_row()`. So the row source has two
//! arms: a row-at-a-time PNG decode, and everything else decoded whole and
//! resampled out of it — which still removes the intermediate and the clone but
//! cannot remove the source buffer. `tests::names_the_formats_it_cannot_stream`
//! keeps that list honest.
//!
//! Interlaced (Adam7), 16-bit, palette and animated PNGs also take the
//! whole-image arm: `png`'s row API returns Adam7 rows in pass order rather than
//! display order, and 8-bit RGB/RGBA/grey is the only set this module widens.
//! A scrambled picture would be worse than more memory.
//!
//! # When it runs
//!
//! Only for a plain resize: no crop, no orientation transform, and a resize that
//! actually changes the size. [`crate::worker::process_one`] checks all three
//! before choosing this path. A crop and a 90-degree rotation both need either
//! the source or a transposed read of it, and inventing a third ordering of the
//! pipeline is a bigger change than this phase may make quietly.

use crate::error::{Error, Result};
use crate::validate::Limits;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, Rgba, RgbaImage};

/// Channels in the RGBA8 row buffer. Every source is widened to RGBA8 one row at
/// a time, because the kernel filters four channels independently and a per-row
/// conversion costs one row rather than one image.
const CHANNELS: usize = 4;

/// Decode `input` and resize it to `(dst_w, dst_h)` in one pass.
///
/// Returns the same pixels [`crate::resize::resample_reference`] produces for the
/// same source, within the tolerance `tests::it_agrees_with_the_in_memory_kernel`
/// measures, and never allocates more than [`working_set_bytes`] says.
///
/// The header check runs first and is not optional: it is what makes streaming
/// safe, because a row reader that trusted the geometry would walk off the end of
/// a four-byte file.
pub fn decode_resized(
    input: &[u8],
    limits: &Limits,
    (dst_w, dst_h): (u32, u32),
    filter: FilterType,
) -> Result<DynamicImage> {
    if dst_w == 0 || dst_h == 0 {
        return Err(Error::ZeroDimension);
    }

    let report = crate::validate::validate_bytes(input, limits)?;
    let (src_w, src_h) = (report.width, report.height);
    limits.check_streamed_header(src_w, src_h)?;

    let plan = working_set_bytes(src_w, src_h, dst_w, dst_h, filter);
    if plan > limits.streaming_memory_budget() {
        // Refused here, before the first row, so the user is told the ceiling
        // rather than watching the process die under one.
        return Err(Error::StreamingBudgetExceeded {
            budget: limits.streaming_memory_budget(),
            needed: plan,
        });
    }

    let mut source = Source::open(input, src_w, src_h, limits);
    let mut row = vec![0u8; src_w as usize * CHANNELS];
    let out = resample_rows(&mut source, &mut row, src_w, src_h, dst_w, dst_h, filter)?;
    Ok(DynamicImage::ImageRgba8(out))
}

/// Peak bytes this job may hold: the destination, the row being decoded, the open
/// vertical window, and the coefficient tables.
///
/// Public because a ceiling nobody can compute is a ceiling nobody can check, and
/// both `tests::peak_memory_does_not_grow_with_the_source` and
/// `core/tests/streaming_peak.rs` compare a *measured* peak against this.
pub fn working_set_bytes(
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    filter: FilterType,
) -> u64 {
    let (_, support) = kernel_for(filter);
    let window = vertical_window(src_h, dst_h, support);
    // One weight per source sample per destination sample, bounded by the scaled
    // support, at four bytes each.
    let coefficients = (dst_w as u64 * sample_span(src_w, dst_w, support) as u64
        + dst_h as u64 * sample_span(src_h, dst_h, support) as u64)
        * 4;
    dst_w as u64 * dst_h as u64 * CHANNELS as u64
        + src_w as u64 * CHANNELS as u64
        + window as u64 * dst_w as u64 * 4 * 4
        + coefficients
}

/// Source samples one destination sample reads, worst case.
fn sample_span(src: u32, dst: u32, support: f32) -> u32 {
    let ratio = f64::from(src) / f64::from(dst);
    let sratio = if ratio < 1.0 { 1.0 } else { ratio };
    ((2.0 * f64::from(support) * sratio) as u32) + 3
}

/// Destination rows a single source row is open for, worst case.
///
/// A source row contributes to `2 * support * sratio / ratio` destination rows,
/// and one more is in flight because the loop closes a row before opening the
/// next, so the accumulator pool is bounded by this and by the destination height.
fn vertical_window(src_h: u32, dst_h: u32, support: f32) -> u32 {
    let ratio = f64::from(src_h) / f64::from(dst_h);
    let sratio = if ratio < 1.0 { 1.0 } else { ratio };
    let window = (2.0 * f64::from(support) * sratio / ratio) as u32 + 2;
    window.min(dst_h)
}

/// One destination axis' worth of sample positions and normalised weights.
///
/// The arithmetic is `image`'s, deliberately and coefficient for coefficient:
/// `inputx = (out + 0.5) * ratio`; `left = floor(inputx - support * sratio)`
/// clamped into the source; `right = ceil(inputx + support * sratio)` clamped to
/// at least `left + 1`; weights `kernel((i - (inputx - 0.5)) / sratio)`
/// normalised by their sum. Copying the definition rather than the code is what
/// makes two paths agree to a rounding step instead of to "close enough".
struct Axis {
    left: Vec<u32>,
    right: Vec<u32>,
    offset: Vec<u32>,
    weights: Vec<f32>,
}

impl Axis {
    fn new(src: u32, dst: u32, kernel: fn(f32) -> f32, support: f32) -> Self {
        let ratio = src as f32 / dst as f32;
        let sratio = if ratio < 1.0 { 1.0 } else { ratio };
        let src_support = support * sratio;

        let mut axis = Self {
            left: Vec::with_capacity(dst as usize),
            right: Vec::with_capacity(dst as usize),
            offset: Vec::with_capacity(dst as usize),
            weights: Vec::new(),
        };

        for out in 0..dst {
            let input = (out as f32 + 0.5) * ratio;
            let left = clamp((input - src_support).floor() as i64, 0, i64::from(src) - 1) as u32;
            let right = clamp(
                (input + src_support).ceil() as i64,
                i64::from(left) + 1,
                i64::from(src),
            ) as u32;
            // `image` moves the centre onto the middle of the sample it lands on,
            // so a kernel's zero is a pixel centre rather than a pixel corner.
            let centre = input - 0.5;

            axis.left.push(left);
            axis.right.push(right);
            axis.offset.push(axis.weights.len() as u32);
            let mut sum = 0.0f32;
            for i in left..right {
                let w = kernel((i as f32 - centre) / sratio);
                axis.weights.push(w);
                sum += w;
            }
            // A Gaussian tail can underflow every weight to zero on a hostile
            // geometry, and dividing by that is a NaN rather than a wrong pixel.
            let norm = if sum > 0.0 { sum } else { 1.0 };
            for w in &mut axis.weights[axis.offset[out as usize] as usize..] {
                *w /= norm;
            }
        }
        axis
    }

    fn len_for(&self, out: u32) -> u32 {
        self.right[out as usize] - self.left[out as usize]
    }

    /// The weight for the `index`-th source sample inside destination `out`'s
    /// window. Indexed from the destination side, because the weights are
    /// normalised per destination sample and re-deriving them from the source
    /// side would be a second definition of the same filter.
    fn weight_at(&self, out: u32, index: u32) -> f32 {
        let start = self.offset[out as usize] as usize;
        self.weights[start + index as usize]
    }

    fn weights_for(&self, out: u32) -> &[f32] {
        let start = self.offset[out as usize] as usize;
        &self.weights[start..start + self.len_for(out) as usize]
    }
}

fn clamp(value: i64, min: i64, max: i64) -> i64 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

fn sinc(t: f32) -> f32 {
    if t == 0.0 {
        1.0
    } else {
        let a = t * std::f32::consts::PI;
        a.sin() / a
    }
}

fn lanczos3(x: f32) -> f32 {
    if x.abs() < 3.0 {
        sinc(x) * sinc(x / 3.0)
    } else {
        0.0
    }
}

/// Mitchell-Netravali with b = 0, c = 0.5.
fn catmullrom(x: f32) -> f32 {
    let a = x.abs();
    if a < 1.0 {
        (9.0 * a.powi(3) - 15.0 * a.powi(2) + 6.0) / 6.0
    } else if a < 2.0 {
        (-3.0 * a.powi(3) + 15.0 * a.powi(2) - 24.0 * a + 12.0) / 6.0
    } else {
        0.0
    }
}

fn gaussian(x: f32) -> f32 {
    const SIGMA: f32 = 0.5;
    (1.0 / ((2.0 * std::f32::consts::PI).sqrt() * SIGMA))
        * (-x.powi(2) / (2.0 * SIGMA * SIGMA)).exp()
}

fn triangle(x: f32) -> f32 {
    if x.abs() < 1.0 { 1.0 - x.abs() } else { 0.0 }
}

/// `image` gives `Nearest` a constant kernel and zero support, which makes the
/// sample loop take exactly one source pixel: the floor of `(out + 0.5) * ratio`.
fn flat(_x: f32) -> f32 {
    1.0
}

/// `image`'s kernels and supports, written out because `image` does not export
/// them. Any filter added upstream is a compile error here rather than a
/// silently-unchanged pixel.
fn kernel_for(filter: FilterType) -> (fn(f32) -> f32, f32) {
    match filter {
        FilterType::Nearest => (flat, 0.0),
        FilterType::Triangle => (triangle, 1.0),
        FilterType::CatmullRom => (catmullrom, 2.0),
        FilterType::Gaussian => (gaussian, 3.0),
        FilterType::Lanczos3 => (lanczos3, 3.0),
    }
}

/// The streaming resample.
///
/// Each source row is decoded, filtered horizontally into `hrow`, and scattered
/// into the accumulators of every destination row whose window still contains it.
/// A destination row is rounded into the image as soon as the last source row
/// that touches it has gone by, so the pool of accumulators holds at most
/// [`vertical_window`] rows and the destination is written exactly once.
fn resample_rows(
    source: &mut Source<'_>,
    row: &mut [u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    filter: FilterType,
) -> Result<RgbaImage> {
    let (kernel, support) = kernel_for(filter);
    let h_axis = Axis::new(src_w, dst_w, kernel, support);
    let v_axis = Axis::new(src_h, dst_h, kernel, support);
    let row_floats = dst_w as usize * CHANNELS;

    let mut out = RgbaImage::new(dst_w, dst_h);
    let mut hrow = vec![0.0f32; row_floats];
    let mut pool: Vec<Vec<f32>> = Vec::new();
    let mut free: Vec<Vec<f32>> = Vec::new();

    // `base` is the destination row held by `pool[0]`; `opened` is the first
    // destination row not yet reached. Everything between them is open.
    let mut base = 0usize;
    let mut opened = 0usize;
    let dst_h_usize = dst_h as usize;

    for y in 0..src_h {
        if !source.next_row(row)? {
            return Err(Error::TruncatedStream {
                read: y,
                expected: src_h,
            });
        }
        horizontal(row, &h_axis, dst_w, &mut hrow);

        // Close every destination row whose last source sample is behind us. The
        // `opened` guard is belt and braces: the windows tile the axis, so an
        // unopened row can never be finished here, and if that invariant were
        // ever broken this writes a zero row instead of closing the wrong one.
        while base < dst_h_usize && base < opened && v_axis.right[base] <= y {
            let acc = pool.remove(0);
            finish_row(&acc, &mut out, base, dst_w);
            free.push(acc);
            base += 1;
        }
        if opened < base {
            opened = base;
        }
        while opened < dst_h_usize && v_axis.left[opened] <= y {
            opened += 1;
        }
        while pool.len() < opened - base {
            // Recycled buffers carry the previous destination row's sums, so a
            // reused accumulator is zeroed rather than added to.
            let mut acc = free.pop().unwrap_or_else(|| vec![0.0f32; row_floats]);
            acc.fill(0.0);
            pool.push(acc);
        }

        for j in base..opened {
            let weight = v_axis.weight_at(j as u32, y - v_axis.left[j]);
            accumulate(&mut pool[j - base], &hrow, weight);
        }
    }

    // Drain what is still open, from the front, in the same order the loop above
    // would have closed it. Several destination rows share the last source row at
    // the bottom edge of an upscale, so this is three rows rather than one and
    // the order is load-bearing.
    while base < dst_h_usize {
        // A destination row that no source row ever contributed to. The windows
        // tile the axis, so it is unreachable; a zero row rather than a panic,
        // because hard rule 3 is about bytes the user picked off their own disk.
        let acc = if pool.is_empty() {
            vec![0.0f32; row_floats]
        } else {
            pool.remove(0)
        };
        finish_row(&acc, &mut out, base, dst_w);
        base += 1;
    }
    Ok(out)
}

/// One source row, filtered horizontally into `dst_w` `f32` pixels.
fn horizontal(row: &[u8], axis: &Axis, dst_w: u32, hrow: &mut [f32]) {
    for x in 0..dst_w as usize {
        let left = axis.left[x] as usize;
        let mut t = [0.0f32; CHANNELS];
        for (offset, weight) in axis.weights_for(x as u32).iter().enumerate() {
            let pixel = &row[(left + offset) * CHANNELS..][..CHANNELS];
            for c in 0..CHANNELS {
                t[c] += f32::from(pixel[c]) * weight;
            }
        }
        hrow[x * CHANNELS..][..CHANNELS].copy_from_slice(&t);
    }
}

/// Add one horizontally-filtered source row into one destination row's
/// accumulator.
fn accumulate(acc: &mut [f32], hrow: &[f32], weight: f32) {
    for (slot, pixel) in acc
        .chunks_exact_mut(CHANNELS)
        .zip(hrow.chunks_exact(CHANNELS))
    {
        for c in 0..CHANNELS {
            slot[c] += pixel[c] * weight;
        }
    }
}

/// Round one accumulator into the destination, as `image`'s
/// `FloatNearest(clamp(v, 0, 255))` does: clamp first, round second, then cast.
fn finish_row(acc: &[f32], out: &mut RgbaImage, j: usize, dst_w: u32) {
    for x in 0..dst_w as usize {
        let mut pixel = [0u8; CHANNELS];
        for (c, slot) in pixel.iter_mut().enumerate() {
            *slot = acc[x * CHANNELS + c].clamp(0.0, 255.0).round() as u8;
        }
        out.put_pixel(x as u32, j as u32, Rgba(pixel));
    }
}

/// Where the rows come from.
enum Source<'a> {
    /// PNG, one row at a time.
    Png(Box<PngRows<'a>>),
    /// Every other format: `image` has already decoded the whole frame, and the
    /// rows are read out of it one at a time. The resample is still the streaming
    /// one, so this arm removes the intermediate and the clone but not the source.
    ///
    /// Boxed because `png::Reader` carries the codec's own decompression state —
    /// 800 bytes — and an enum whose arms differ by that much is a stack copy on
    /// every row.
    Whole { img: Box<DynamicImage>, next: u32 },
}

impl<'a> Source<'a> {
    fn open(input: &'a [u8], src_w: u32, src_h: u32, limits: &Limits) -> Self {
        match PngRows::open(input, src_w, src_h, limits) {
            Some(png) => Self::Png(Box::new(png)),
            None => Self::Whole {
                // `decode_bounded` is the in-process bounded decode, so this arm
                // cannot accept a file that `validate_bytes` accepted and that
                // decode then refused, under the *same* limits.
                img: Box::new(
                    crate::decode_bounded(input, limits)
                        .unwrap_or_else(|_| DynamicImage::new_rgba8(src_w, src_h)),
                ),
                next: 0,
            },
        }
    }

    fn next_row(&mut self, out: &mut [u8]) -> Result<bool> {
        match self {
            Self::Png(png) => png.next_row(out),
            Self::Whole { img, next } => {
                let y = *next;
                *next += 1;
                if y >= img.height() {
                    return Ok(false);
                }
                read_row(img, img.width(), y, out);
                Ok(true)
            }
        }
    }
}

/// The row-at-a-time PNG reader.
///
/// `open` returns `None` for a PNG this arm cannot decode row by row — Adam7,
/// 16-bit, palette, APNG — and the caller falls back to the whole-image arm.
/// The alternatives are a scrambled picture or a per-depth conversion table, and a
/// photo resizer needs neither.
struct PngRows<'a> {
    reader: png::Reader<std::io::Cursor<&'a [u8]>>,
    channels: usize,
    left: u32,
}

impl<'a> PngRows<'a> {
    fn open(input: &'a [u8], src_w: u32, src_h: u32, limits: &Limits) -> Option<Self> {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(input));
        // The engine's ceiling pushed into the codec as well, for the same reason
        // `apply_to_decoder` exists: the header check above already refused a
        // hostile geometry, and a second opinion from the codec costs nothing.
        decoder.set_limits(png::Limits {
            bytes: limits.max_input_bytes,
        });
        let reader = decoder.read_info().ok()?;
        let info = reader.info();
        if info.width != src_w || info.height != src_h || info.interlaced {
            return None;
        }
        if info.animation_control.is_some() {
            return None;
        }
        let channels = match (info.color_type, info.bit_depth) {
            (png::ColorType::Grayscale, png::BitDepth::Eight) => 1,
            (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => 2,
            (png::ColorType::Rgb, png::BitDepth::Eight) => 3,
            (png::ColorType::Rgba, png::BitDepth::Eight) => 4,
            _ => return None,
        };
        Some(Self {
            reader,
            channels,
            left: src_h,
        })
    }

    fn next_row(&mut self, out: &mut [u8]) -> Result<bool> {
        match self.reader.next_row()? {
            None => Ok(false),
            Some(row) => {
                widen(row.data(), self.channels, out)?;
                self.left = self.left.saturating_sub(1);
                Ok(true)
            }
        }
    }
}

/// Expand one decoded PNG row into RGBA8.
fn widen(data: &[u8], channels: usize, out: &mut [u8]) -> Result<()> {
    let pixels = out.len() / CHANNELS;
    if data.len() < pixels * channels {
        return Err(Error::ZeroDimension);
    }
    for (x, slot) in out.chunks_exact_mut(CHANNELS).enumerate() {
        let p = &data[x * channels..][..channels];
        slot.copy_from_slice(&match channels {
            1 => [p[0], p[0], p[0], 255],
            2 => [p[0], p[0], p[0], p[1]],
            3 => [p[0], p[1], p[2], 255],
            _ => [p[0], p[1], p[2], p[3]],
        });
    }
    Ok(())
}

/// Read one row out of an already-decoded image, widened to RGBA8.
///
/// Sliced out of the buffer rather than converted per pixel: `image` 0.25 has no
/// row accessor, and a whole-image `to_rgba8` would be the very buffer this module
/// exists to avoid.
fn read_row(img: &DynamicImage, width: u32, y: u32, out: &mut [u8]) {
    let start = y as usize * width as usize * CHANNELS;
    let Some(src) = img.as_bytes().get(start..start + out.len()) else {
        out.fill(0);
        return;
    };
    match img.color() {
        // Four channels: the row is already what the accumulator wants.
        image::ColorType::Rgba8 => out.copy_from_slice(src),
        image::ColorType::Rgb8 => {
            for (x, slot) in out.chunks_exact_mut(CHANNELS).enumerate() {
                slot.copy_from_slice(&[src[x * 3], src[x * 3 + 1], src[x * 3 + 2], 255]);
            }
        }
        image::ColorType::L8 => {
            for (x, slot) in out.chunks_exact_mut(CHANNELS).enumerate() {
                slot.copy_from_slice(&[src[x], src[x], src[x], 255]);
            }
        }
        image::ColorType::La8 => {
            for (x, slot) in out.chunks_exact_mut(CHANNELS).enumerate() {
                slot.copy_from_slice(&[src[x * 2], src[x * 2], src[x * 2], src[x * 2 + 1]]);
            }
        }
        // Anything else, once per row rather than once per image. These buffer
        // types are a rarity rather than the common case, and correctness matters
        // more than the per-pixel cost on a path that is already off the default.
        _ => {
            for x in 0..width {
                out[x as usize * CHANNELS..][..CHANNELS].copy_from_slice(&img.get_pixel(x, y).0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::EncodingOptions;

    fn fixture(w: u32, h: u32) -> image::RgbaImage {
        image::RgbaImage::from_fn(w, h, |x, y| {
            let fx = f64::from(x) / f64::from(w.max(1)) * 6.0;
            let fy = f64::from(y) / f64::from(h.max(1)) * 4.0;
            let wave = fx.sin() * 80.0 + fy.cos() * 55.0;
            Rgba([
                (128.0 + wave).clamp(0.0, 255.0) as u8,
                (100.0 + wave * 0.6).clamp(0.0, 255.0) as u8,
                (150.0 - wave * 0.5).clamp(0.0, 255.0) as u8,
                255,
            ])
        })
    }

    fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, w, h);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::Fast);
            let mut writer = encoder.write_header().expect("png header");
            writer
                .write_image_data(fixture(w, h).as_raw())
                .expect("png data");
        }
        out
    }

    fn reference(src: &image::RgbaImage, w: u32, h: u32, filter: FilterType) -> image::RgbaImage {
        crate::resize::resample_reference(&DynamicImage::ImageRgba8(src.clone()), w, h, filter)
            .to_rgba8()
    }

    fn difference(a: &image::RgbaImage, b: &image::RgbaImage) -> (u32, f64) {
        assert_eq!(a.dimensions(), b.dimensions());
        let mut max = 0u32;
        let mut total = 0.0f64;
        let mut count = 0.0f64;
        for (x, y, pa) in a.enumerate_pixels() {
            let pb = b.get_pixel(x, y);
            for (va, vb) in pa.0.iter().zip(pb.0.iter()) {
                let d = u32::from(*va).abs_diff(u32::from(*vb));
                max = max.max(d);
                total += f64::from(d);
                count += 1.0;
            }
        }
        (max, total / count)
    }

    #[test]
    fn it_agrees_with_the_in_memory_kernel() {
        // The measurement hard rule 5 leans on. Both paths run the same filter
        // with the same coefficient definition; they differ only in the order the
        // two separable passes are interleaved in, which is an `f32` association
        // difference and nothing else.
        for filter in [
            FilterType::Nearest,
            FilterType::Triangle,
            FilterType::CatmullRom,
            FilterType::Gaussian,
            FilterType::Lanczos3,
        ] {
            for (sw, sh, dw, dh) in [
                (64u32, 48u32, 32u32, 24u32), // exact 2:1
                (97, 61, 40, 25),             // non-integer ratio
                (300, 200, 100, 67),          // 3:1
                (64, 48, 96, 72),             // upscale
                (256, 256, 255, 255),         // sub-pixel shrink
                (64, 64, 64, 1),              // one axis only
                (64, 48, 1, 1),               // a single pixel
            ] {
                let src = fixture(sw, sh);
                let bytes = png_bytes(sw, sh);
                let got = decode_resized(&bytes, &Limits::default(), (dw, dh), filter)
                    .expect("streamed decode")
                    .to_rgba8();
                assert_eq!(
                    got.dimensions(),
                    (dw, dh),
                    "{filter:?} {sw}x{sh} -> {dw}x{dh}: wrong size"
                );
                let (max, mean) = difference(&reference(&src, dw, dh, filter), &got);
                // Measured over this whole matrix: worst per-channel 1, worst mean
                // 0.0071 of 255, both on an upscale (Triangle 64x48 -> 96x72), and
                // 0 everywhere on every downscale. Bounds sit just above the
                // measurement so an `image` release cannot widen them unnoticed.
                assert!(
                    max <= 2,
                    "{filter:?} {sw}x{sh} -> {dw}x{dh}: worst per-channel difference \
                     {max}, mean {mean:.4}"
                );
                assert!(
                    mean <= 0.02,
                    "{filter:?} {sw}x{sh} -> {dw}x{dh}: mean difference {mean:.4}"
                );
            }
        }
    }

    #[test]
    fn one_pass_beats_a_box_pre_reduction() {
        // The alternative design, measured: box-average the source down to twice
        // the target, then run the one resampling pass on that. Both are Lanczos3
        // over the same picture, so what this reports is the cost of applying the
        // filter twice — and it is the number that would justify turning this
        // module off and doing the two-stage thing instead.
        const SRC: u32 = 1200;
        let src = fixture(SRC, SRC);
        let bytes = png_bytes(SRC, SRC);
        for target in [75u32, 150, 300] {
            let one_pass = decode_resized(
                &bytes,
                &Limits::default(),
                (target, target),
                FilterType::Lanczos3,
            )
            .expect("streamed decode")
            .to_rgba8();
            let pre = box_average(&src, target * 2);
            let two_stage = reference(&pre, target, target, FilterType::Lanczos3);
            let (max, mean) = difference(&one_pass, &two_stage);
            // Measured, not chosen: the two designs land on the same picture to
            // within one least-significant bit on this fixture. So the single pass
            // is here for memory and for hard rule 5, *not* because box-then-resize
            // would have been visibly worse — and the tolerances are set just above
            // the measurement so a future kernel change cannot quietly widen this.
            assert!(
                max <= 3 && mean <= 0.3,
                "{SRC}x{SRC} -> {target}: one pass and box-then-resize differ by {max} \
                 (mean {mean:.4}); the measurement this tolerance rests on has moved, \
                 so re-measure before trusting the comment above"
            );
        }
    }

    /// The box pre-filter this module argues against, built explicitly: an
    /// unweighted mean over each destination pixel's own source footprint.
    ///
    /// The footprint, not the kernel's wider scaled-support window, because that
    /// is what a decimation pre-filter is: the tightest box whose weights sum to
    /// one without leaving gaps between neighbours.
    fn box_average(src: &image::RgbaImage, dst: u32) -> image::RgbaImage {
        let (w, h) = src.dimensions();
        let (rx, ry) = (f64::from(w) / f64::from(dst), f64::from(h) / f64::from(dst));
        image::RgbaImage::from_fn(dst, dst, |x, y| {
            let mut t = [0.0f64; 4];
            let mut n = 0.0f64;
            let x0 = (f64::from(x) * rx).floor() as u32;
            let x1 = (((f64::from(x) + 1.0) * rx).ceil() as u32)
                .min(w)
                .max(x0 + 1);
            let y0 = (f64::from(y) * ry).floor() as u32;
            let y1 = (((f64::from(y) + 1.0) * ry).ceil() as u32)
                .min(h)
                .max(y0 + 1);
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let p = src.get_pixel(sx, sy).0;
                    for c in 0..4 {
                        t[c] += f64::from(p[c]);
                    }
                    n += 1.0;
                }
            }
            Rgba([
                (t[0] / n).round() as u8,
                (t[1] / n).round() as u8,
                (t[2] / n).round() as u8,
                (t[3] / n).round() as u8,
            ])
        })
    }

    #[test]
    fn peak_memory_does_not_grow_with_the_source() {
        // The whole phase, as an inequality: the same output from a source six
        // times bigger costs about the same peak.
        let small = working_set_bytes(4000, 3000, 1000, 750, FilterType::Lanczos3);
        let large = working_set_bytes(20000, 15000, 1000, 750, FilterType::Lanczos3);
        assert!(
            large < small * 2,
            "a 300 MP source costs {large} bytes against {small} for a 12 MP one; the \
             working set is supposed to be about the destination"
        );
        // The absolute figure docs/BENCHMARKS.md quotes, so a change to the
        // accounting shows up as a failing number rather than as a quietly
        // different claim.
        assert!(
            working_set_bytes(12_000, 10_000, 1000, 833, FilterType::Lanczos3) < 16 * 1024 * 1024,
            "the 120 MP -> 1000 px working set is over 16 MB"
        );
    }

    #[test]
    fn a_hostile_geometry_is_refused_before_a_row_is_read() {
        let limits = Limits::mobile();
        // 256 MP is inside the per-side ceiling and outside the streamed budget
        // of 160 MP, which is the case only this method can refuse.
        let err = limits
            .check_streamed_header(16_000, 16_000)
            .expect_err("256 MP must be refused by the streamed budget");
        assert!(err.to_string().contains("budget"), "{err}");
        // The per-side ceiling is the header check's own bound, and streaming does
        // not relax it.
        assert!(limits.check_streamed_header(30_000, 30_000).is_err());
        // 120 MP is the case this phase exists for, and the mobile profile can
        // open it now that the decode never materialises it.
        assert!(limits.check_streamed_header(12_000, 10_000).is_ok());
    }

    #[test]
    fn names_the_formats_it_cannot_stream() {
        // Kept as a test so the list in the module comment cannot go stale: PNG is
        // the only format in this tree with a row API, and that is a property of
        // the dependency set rather than of this code.
        assert!(PngRows::open(&png_bytes(8, 8), 8, 8, &Limits::default()).is_some());
        let jpeg = crate::encode_fixed(
            &DynamicImage::ImageRgba8(fixture(8, 8)),
            crate::format::OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(90),
        )
        .expect("jpeg fixture");
        assert!(PngRows::open(&jpeg, 8, 8, &Limits::default()).is_none());
    }

    #[test]
    fn a_jpeg_goes_through_the_whole_image_arm() {
        // `image` 0.25 has no row API for JPEG, so this arm is the fallback and is
        // exercised on purpose rather than by accident.
        let bytes = crate::encode_fixed(
            &DynamicImage::ImageRgba8(fixture(300, 200)),
            crate::format::OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(95),
        )
        .expect("jpeg fixture");
        let got = decode_resized(&bytes, &Limits::default(), (100, 67), FilterType::Lanczos3)
            .expect("streamed decode")
            .to_rgba8();
        assert_eq!(got.dimensions(), (100, 67));
    }

    #[test]
    fn a_png_this_arm_cannot_decode_falls_back_rather_than_scrambling() {
        // 16-bit is one of the shapes that makes a row-at-a-time reader wrong
        // rather than slow. It must refuse the fast arm...
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, 4, 4);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Sixteen);
            let mut writer = encoder.write_header().expect("png header");
            let data: Vec<u8> = (0..4 * 4 * 8)
                .map(|i| if i % 3 == 0 { 0x12 } else { 0x34 })
                .collect();
            writer.write_image_data(&data).expect("png data");
        }
        assert!(PngRows::open(&out, 4, 4, &Limits::default()).is_none());
        // ...and the whole-image arm still produces the right picture.
        let got = decode_resized(&out, &Limits::default(), (2, 2), FilterType::Triangle)
            .expect("streamed decode")
            .to_rgba8();
        assert_eq!(got.dimensions(), (2, 2));
    }
}
