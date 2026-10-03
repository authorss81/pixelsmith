//! Shared harness for the fuzz targets.
//!
//! Three things live here, and every target is built on all three:
//!
//! 1. **A counting global allocator.** "The decoder respects `Limits`" is a claim
//!    about memory, so it has to be checked against memory. [`measure_peak`]
//!    samples the live-heap high-water mark across a call and the targets assert
//!    on the number that comes out. Asserting instead that `input.len() * 4` was
//!    never exceeded would prove only that the input is short, which is not the
//!    property under test: a decompression bomb is small and explodes.
//! 2. **A deliberately tight `Limits` profile.** Production allows 128 MP, so a
//!    single fuzz iteration could legitimately allocate 512 MB before the budget
//!    said no. The fuzz profile turns the same arithmetic at magnitudes a fuzzer
//!    can actually reach, which is where the boundary bugs live.
//! 3. **Container headers the fuzzer gets to write.** [`seed_magic`] puts a real
//!    magic number and a plausible header in front of the fuzzer's bytes, so a
//!    target reaches the decoder's dimension parsing on almost every iteration
//!    instead of bouncing off a two-byte signature check.
//!
//! Nothing here is used by the shipped engine. It is a test crate.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use arbitrary::Arbitrary as _;
use pixelsmith_core::{Error, Limits, OutputFormat};

// Thread-local heap accounting, rather than global, because the counters answer a
// question about *one* call: "how much heap did this decode hold at once". A
// global counter would fold in whatever libFuzzer itself allocates on the same
// thread between iterations, and the number would drift with corpus size instead
// of with the decoder.
thread_local! {
    /// Bytes currently live on this thread.
    static LIVE: Cell<usize> = const { Cell::new(0) };
    /// High-water mark of [`LIVE`]. Monotonic for the life of the thread.
    static PEAK: Cell<usize> = const { Cell::new(0) };
}

/// Global allocator that records how much is live and what the peak was.
///
/// Every `#[global_allocator]` in a process wins or the link fails, so this is
/// declared once, in this crate, and every fuzz binary gets it by linking
/// `px_fuzz`. It is a wrapper, not a replacement: allocations still go to
/// `System`, so AddressSanitizer still sees and instruments every one of them.
pub struct CountingAllocator;

impl CountingAllocator {
    #[inline]
    fn record_alloc(size: usize) {
        LIVE.with(|live| {
            let now = live.get().saturating_add(size);
            live.set(now);
            PEAK.with(|peak| {
                if now > peak.get() {
                    peak.set(now);
                }
            });
        });
    }

    #[inline]
    fn record_dealloc(size: usize) {
        LIVE.with(|live| live.set(live.get().saturating_sub(size)));
    }
}

// SAFETY: every method forwards to `System` with the layout it was given, so the
// contract is upheld exactly. The counters are `Cell<usize>` behind
// `thread_local`, which needs no allocation and no `Sync` beyond what the macro
// already guarantees, and a torn counter would only affect a fuzz measurement,
// never memory safety.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            Self::record_alloc(layout.size());
        }
        ptr
    }

    // `alloc_zeroed` is left to the default implementation, which routes through
    // `alloc` and so is counted once, at the right size.

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        Self::record_dealloc(layout.size());
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let grown = unsafe { System.realloc(ptr, layout, new_size) };
        if !grown.is_null() {
            Self::record_dealloc(layout.size());
            Self::record_alloc(new_size);
        }
        grown
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Peak heap usage of one call, in bytes.
pub struct HeapGuard {
    /// Live bytes at entry. Everything above this was allocated by the measured
    /// call, so it is the only part attributable to the code under test.
    baseline_live: usize,
}

impl HeapGuard {
    /// Start measuring. Free the previous guard (or drop it) before starting a
    /// new one, or the baseline is wrong and every later reading is inflated by
    /// whatever the earlier measurement left behind.
    pub fn start() -> Self {
        let baseline_live = LIVE.with(Cell::get);
        Self { baseline_live }
    }

    /// Bytes held simultaneously by the measured call, peak over its lifetime.
    pub fn peak_bytes(&self) -> usize {
        PEAK.with(|peak| peak.get().saturating_sub(self.baseline_live))
    }
}

/// Run `f` and report both its result and the peak heap it held.
///
/// The reading is taken before `f`'s value is dropped, so the measurement covers
/// everything the call did, not just the part that survives it.
pub fn measure_peak<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let guard = HeapGuard::start();
    let value = f();
    let peak = guard.peak_bytes();
    (value, peak)
}

/// Slack for decoder scratch space on top of the pixel buffer.
///
/// A decoder is allowed to hold a row buffer, a Huffman table or a partially
/// built frame while the output is being allocated, so the ceiling cannot be
/// exactly `w * h * 4`. This is generous on purpose: a ceiling that fires on
/// legitimate decoding is a harness that gets switched off, which is worse than
/// no ceiling at all.
pub const DECODER_SLACK: usize = 8 * 1024 * 1024;

/// Absolute ceiling no target may cross, whatever the limits say.
///
/// A decoder that ignores `Limits` entirely still cannot get away with more than
/// this, so a bomb discovered here fails the target instead of the runner. It
/// has to be high enough that a legitimate in-budget decode cannot reach it,
/// because the point is to catch "no limit was applied", not to bound a budget
/// that is already checked more tightly.
pub const HARD_CAP: usize = 256 * 1024 * 1024;

/// The memory ceiling for one call against one limits profile.
#[derive(Debug, Clone, Copy)]
pub struct HeapBudget {
    /// Limit-derived ceiling: four bytes per pixel plus decoder slack.
    pub ceiling: usize,
    /// Never-exceeded backstop, see [`HARD_CAP`].
    pub hard_cap: usize,
    /// The budget this ceiling came from, for the failure message.
    pub max_pixels: u64,
    /// What was being measured, so a crash names the path rather than a number.
    pub what: &'static str,
}

/// Derive the ceiling for a call made under `limits`.
pub fn heap_budget(limits: &Limits, what: &'static str) -> HeapBudget {
    HeapBudget {
        ceiling: limits.max_pixels as usize * 4 + DECODER_SLACK,
        hard_cap: HARD_CAP,
        max_pixels: limits.max_pixels,
        what,
    }
}

impl HeapBudget {
    /// A budget for code that must not allocate pixels at all — header parsing.
    ///
    /// `validate_bytes` reads a header and is supposed to stop there; if a
    /// regression moved a full decode inside it, the measured peak jumps by the
    /// size of the image and this is the assertion that notices.
    pub fn header_only(input_bytes: usize, what: &'static str) -> HeapBudget {
        HeapBudget {
            ceiling: input_bytes * 4 + 1024 * 1024,
            hard_cap: HARD_CAP,
            max_pixels: 0,
            what,
        }
    }

    /// Fail the iteration if `peak` is over budget.
    ///
    /// A panic here is a libFuzzer crash, which means the reproducer is written
    /// to `fuzz/artifacts/` and reruns on its own. That is the intended outcome:
    /// a target that quietly kept allocating past its limit while calling itself
    /// a pass would be the worst possible signal.
    pub fn assert_within(&self, peak: usize) {
        assert!(
            peak <= self.hard_cap,
            "{}: {peak} bytes of heap exceeded the {} byte hard cap; \
             something allocated without consulting Limits at all",
            self.what, self.hard_cap
        );
        assert!(
            peak <= self.ceiling,
            "{}: {peak} bytes of heap exceeded the {} byte ceiling for a \
             max_pixels of {} ({} bytes of RGBA plus {} bytes of decoder slack)",
            self.what,
            self.ceiling,
            self.max_pixels,
            self.max_pixels.saturating_mul(4),
            DECODER_SLACK
        );
    }
}

/// The limits profile a fuzz iteration runs under.
///
/// Four profiles rather than one, because the interesting bugs in a limit check
/// are the ones one order of magnitude either side of it: a decoder that reads
/// `width - 1` instead of `width`, or that multiplies before it compares. A
/// single profile tests one point on that curve.
///
/// `max_dimension` is kept at or below the square root of `max_pixels` on
/// purpose. `image` checks the per-side limits when the decoder is constructed
/// but the allocation limit only inside `read_image`, and the harness allocates
/// the buffer itself — so a decoder that honoured one limit and not the other
/// would have to be reported as a finding, and it must not be. With these numbers
/// a decoder that respects either limit alone stays inside the ceiling.
pub fn fuzz_limits(profile: u8) -> Limits {
    let (pixels, dimension, bytes) = match profile % 4 {
        0 => (4_000_000u64, 2000u32, 2 * 1024 * 1024usize),
        1 => (250_000, 500, 512 * 1024),
        2 => (16_000_000, 4000, 8 * 1024 * 1024),
        _ => (1_000_000, 1000, 1024 * 1024),
    };
    Limits { max_input_bytes: bytes, max_pixels: pixels, max_dimension: dimension }
}

/// Failures that mean "this file was too big", as opposed to "these bytes are not
/// a picture we can handle".
///
/// The distinction is the whole point of the cross-check in `decode_bounded` and
/// `validate_bytes`: if `validate_bytes` accepted a file and `decode_bounded`
/// then refused it *for being too large*, one of the two disagree with the
/// configured budget, which is a hole in hard rule 4 rather than a codec saying
/// no to garbage.
pub fn is_resource_error(error: &Error) -> bool {
    matches!(
        error,
        Error::InputTooLarge { .. }
            | Error::PixelBudgetExceeded { .. }
            | Error::SuspiciousDimensions { .. }
            | Error::ZeroDimension
    )
}

/// The header a decoder needs before it will look at a single pixel.
///
/// The fuzzer supplies the tail; the header is built here so the dimension,
/// chunk-count and frame-count fields are always *present and parsed*. Without
/// this, a JPEG target spends its whole budget being told "not a JPEG" by the
/// two-byte SOI check and never reaches the code that matters.
pub fn seed_magic(format: OutputFormat, tail: &[u8]) -> Vec<u8> {
    match format {
        OutputFormat::Jpeg => jpeg_header(tail),
        OutputFormat::Png => png_header(tail),
        OutputFormat::WebP => webp_header(tail),
        OutputFormat::Gif => gif_header(tail),
        OutputFormat::Tiff => tiff_header(tail),
        OutputFormat::Bmp => bmp_header(tail),
        OutputFormat::Ico => ico_header(tail),
        // Recognised on input, no decoder wired up in this build. There is no
        // target for it, because there is nothing to reach.
        OutputFormat::Avif => b"".to_vec(),
    }
}

/// `FF D8` then an APP0/JFIF segment, then whatever the fuzzer wrote.
///
/// The APP0 length field is recomputed from the tail so the segment is
/// self-consistent: a JFIF length that disagrees with the payload is a different
/// fuzz target, and it is reached anyway from `decode_bounded`, whose input is
/// used verbatim.
fn jpeg_header(tail: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(tail.len() + 18);
    out.extend_from_slice(&[0xFF, 0xD8]);
    out.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
    out.extend_from_slice(b"JFIF\0");
    out.extend_from_slice(&[0x01, 0x02, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00]);
    out.extend_from_slice(tail);
    out
}

/// The eight-byte signature, then a well-formed IHDR chunk whose 13 payload
/// bytes — width, height, bit depth, colour type, and the rest — are the
/// fuzzer's.
///
/// A PNG bomb *is* a 13-byte payload, so giving the fuzzer control of exactly
/// those bytes is what makes it able to ask for 60000x60000 without having to
/// discover the whole PNG container first.
fn png_header(tail: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(tail.len() + 33);
    out.extend_from_slice(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
    out.extend_from_slice(&13u32.to_be_bytes());
    out.extend_from_slice(b"IHDR");
    for i in 0..13 {
        out.push(tail.get(i).copied().unwrap_or(0));
    }
    out.extend_from_slice(&tail[13.min(tail.len())..]);
    out
}

/// `RIFF .... WEBP` then a chunk header the fuzzer picks.
///
/// The chunk tag is what separates VP8, VP8L and VP8X, and those three decode
/// through entirely different code paths — including the extended format with
/// its own 24-bit canvas. Letting the fuzzer choose the tag means all three are
/// one byte apart.
fn webp_header(tail: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(tail.len() + 20);
    let payload_len = (tail.len() + 8) as u32;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&payload_len.to_le_bytes());
    out.extend_from_slice(b"WEBP");
    let tag: &[u8] = match tail.first().copied().unwrap_or(0) % 3 {
        0 => b"VP8 ",
        1 => b"VP8L",
        _ => b"VP8X",
    };
    out.extend_from_slice(tag);
    out.extend_from_slice(&(tail.len() as u32).to_le_bytes());
    out.extend_from_slice(&tail[1.min(tail.len())..]);
    out
}

/// `GIF89a` plus a logical screen descriptor the fuzzer fills in.
///
/// The screen width and height are the first two little-endian shorts in the
/// descriptor, and they are the whole of a GIF's memory bomb: the decoder
/// allocates the canvas before it looks at a single frame.
fn gif_header(tail: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(tail.len() + 13);
    out.extend_from_slice(b"GIF89a");
    for i in 0..7 {
        out.push(tail.get(i).copied().unwrap_or(0));
    }
    out.extend_from_slice(&tail[7.min(tail.len())..]);
    out
}

/// A TIFF byte-order mark, then an IFD offset the fuzzer controls.
///
/// Both endiannesses are reachable, because a little-endian reader handed a
/// big-endian file, and vice versa, is a classic way to read an entry count of
/// 65535 where the file meant 6.
fn tiff_header(tail: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(tail.len() + 8);
    let big_endian = tail.first().is_some_and(|b| b % 2 == 0);
    if big_endian {
        out.extend_from_slice(b"MM\0*");
        for i in 0..4 {
            out.push(tail.get(i).copied().unwrap_or(0));
        }
    } else {
        out.extend_from_slice(b"II*\0");
        for i in 0..4 {
            out.push(tail.get(i).copied().unwrap_or(0));
        }
    }
    out.extend_from_slice(&tail[4.min(tail.len())..]);
    out
}

/// `BM`, a file size, and a complete 40-byte `BITMAPINFOHEADER`.
///
/// A BMP's dimensions are a signed 32-bit pair in the DIB header, so they can be
/// negative — which is why this is hand-built rather than left to a library:
/// the fuzzer gets to set the width and height fields directly, including the
/// sign bit.
fn bmp_header(tail: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(tail.len() + 54);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&0u32.to_le_bytes()); // file size, recomputed by nobody
    out.extend_from_slice(&[0u8; 4]); // reserved
    out.extend_from_slice(&54u32.to_le_bytes()); // pixel data offset
    out.extend_from_slice(&40u32.to_le_bytes()); // DIB header size
    for i in 0..16 {
        out.push(tail.get(i).copied().unwrap_or(0));
    }
    out.extend_from_slice(&[1u8, 0, 24, 0]); // planes, 24 bpp, no compression
    out.extend_from_slice(&tail[16.min(tail.len())..]);
    out
}

/// An `ICONDIR` whose entry count and entries are the fuzzer's.
///
/// The count is a `u16`, so 65535 entries fit in two bytes and the directory
/// that claims them is ~1024 bytes of attacker-chosen offsets and sizes. That is
/// the ICO memory bomb, and it needs a hand-built header to be reachable.
fn ico_header(tail: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(tail.len() + 10);
    out.extend_from_slice(&[0x00, 0x00, 0x01, 0x00]);
    let count = u16::from(tail.first().copied().unwrap_or(1));
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&tail[1.min(tail.len())..]);
    out
}

/// Fuzzer-controlled parameters, read off the front of the input.
///
/// `arbitrary` is used to widen what each iteration explores rather than to
/// replace libFuzzer's byte stream: a limits profile, which container header to
/// lay down, and a truncation choice. Unstructured reads from the front and
/// leaves the rest for the image, so one input carries both the configuration and
/// the payload.
#[derive(Debug, arbitrary::Arbitrary)]
pub struct Case {
    /// Which `Limits` profile this iteration runs under.
    pub profile: u8,
    /// Whether to prepend a real container header.
    pub with_header: bool,
    /// Whether to truncate the payload, exercising the short-file paths.
    pub truncate: bool,
    /// Which format's header to lay down.
    pub format: u8,
}

/// The formats a header can be built for, in a fixed order so a given input
/// always produces the same case.
pub const HEADER_FORMATS: [OutputFormat; 7] = [
    OutputFormat::Jpeg,
    OutputFormat::Png,
    OutputFormat::WebP,
    OutputFormat::Gif,
    OutputFormat::Tiff,
    OutputFormat::Bmp,
    OutputFormat::Ico,
];

impl Case {
    /// Read the configuration from the head of `data`.
    ///
    /// Returns `None` when the input is too short to configure anything, which
    /// is not a finding: it is an input that carries no case.
    pub fn from_bytes(data: &[u8]) -> Option<(Case, &[u8])> {
        let mut u = arbitrary::Unstructured::new(data);
        let case = Case::arbitrary(&mut u).ok()?;
        let tail = u.take_rest();
        Some((case, tail))
    }

    /// The format this case lays a header down for.
    pub fn format(&self) -> OutputFormat {
        HEADER_FORMATS[usize::from(self.format) % HEADER_FORMATS.len()]
    }

    /// The payload this case wants to feed to the engine.
    pub fn payload(&self, format: OutputFormat, tail: &[u8]) -> Vec<u8> {
        let body = if self.with_header {
            seed_magic(format, tail)
        } else {
            tail.to_vec()
        };
        if !self.truncate || body.len() < 2 {
            return body;
        }
        // Keep at least one byte: a zero-length input is rejected before any
        // parsing, which is a test the non-fuzzered unit tests already cover and
        // would waste the iteration.
        body[..1 + tail.len() % (body.len() - 1)].to_vec()
    }
}

/// Wrap bytes in a reader for `image` decoders.
pub fn cursor(data: &[u8]) -> std::io::Cursor<&[u8]> {
    std::io::Cursor::new(data)
}

/// The decoder-side limits for a profile, mirroring
/// `pixelsmith_core::Limits::apply_to_decoder`.
///
/// Duplicated rather than called because `apply_to_decoder` takes an
/// `ImageReader` and the per-decoder targets build their decoder directly. The
/// numbers must match: a fuzz target that fuzzed a *looser* decoder than the
/// engine uses would test a configuration that cannot happen in the product.
pub fn decoder_limits(limits: &Limits) -> image::Limits {
    let mut out = image::Limits::default();
    out.max_image_width = Some(limits.max_dimension);
    out.max_image_height = Some(limits.max_dimension);
    out.max_alloc = Some(limits.max_pixels.saturating_mul(4));
    out
}

/// Assert that a decoded image is within the profile it was decoded under.
///
/// This is the assertion that catches a decoder ignoring its limits: `image`
/// would hand back a perfectly valid 30000x30000 `DynamicImage` if nothing
/// stopped it, and only a check against the same numbers the engine uses turns
/// that into a failing iteration.
pub fn assert_within_limits(img: &image::DynamicImage, limits: &Limits) {
    if let Err(e) = limits.check_decoded(img) {
        panic!(
            "decoded {}x{} outside the profile it was given: {e}",
            img.width(),
            img.height()
        );
    }
    assert!(img.width() > 0 && img.height() > 0, "decoder returned a zero-sized image");
}

/// Decode through `read_image` into a buffer sized from the decoder's own claim.
///
/// `image::ImageDecoder::read_image` documents a panic when the buffer length
/// does not equal `total_bytes()`. That is the only panic on the decode path a
/// caller can walk into, so the harness walks into it on purpose: the buffer is
/// sized from the decoder's own numbers, and the arithmetic is checked before a
/// single byte is allocated.
///
/// `Err` means the decoder refused this profile or refused this file, which is an
/// ordinary outcome — the interesting case is a file that decodes at all.
pub fn decode_into_exact_buffer<D: image::ImageDecoder>(
    mut decoder: D,
    limits: &Limits,
    what: &'static str,
) -> Result<image::DynamicImage, String> {
    // Refuses when the header's dimensions are over the per-side limit. The
    // engine applies the same limits, so this is where a bomb header stops.
    decoder.set_limits(decoder_limits(limits)).map_err(|e| e.to_string())?;

    let (w, h) = decoder.dimensions();
    if w == 0 || h == 0 {
        return Err(format!("{what}: header declares {w}x{h}"));
    }
    let channels = decoder.color_type().bytes_per_pixel();
    let claimed = decoder.total_bytes();
    let by_dimension = u64::from(w) * u64::from(h) * u64::from(channels);
    // A decoder whose own arithmetic disagrees with its header is the shape of
    // bug that becomes a heap overflow in whatever reads the buffer next.
    assert_eq!(
        claimed, by_dimension,
        "{what}: decoder wants {claimed} bytes, but {w}x{h} at {channels} \
         bytes per pixel is {by_dimension}"
    );
    assert!(
        claimed <= limits.max_pixels.saturating_mul(4),
        "{what}: {w}x{h} is inside the {}-pixel per-side limit but wants \
         {claimed} bytes of buffer, over the {} pixel budget",
        limits.max_dimension,
        limits.max_pixels
    );

    let mut buffer = vec![0u8; claimed as usize];
    decoder.read_image(&mut buffer).map_err(|e| e.to_string())?;

    // `from_raw` re-checks the length against the dimensions and returns `None`
    // on a mismatch, so this also proves the buffer was the size it claimed.
    let image = match channels {
        1 => image::GrayImage::from_raw(w, h, buffer).map(image::DynamicImage::ImageLuma8),
        3 => image::RgbImage::from_raw(w, h, buffer).map(image::DynamicImage::ImageRgb8),
        4 => image::RgbaImage::from_raw(w, h, buffer).map(image::DynamicImage::ImageRgba8),
        other => panic!("{what}: {other} bytes per pixel is not a colour type"),
    };
    image.ok_or_else(|| format!("{what}: {claimed} bytes did not make a {w}x{h} image"))
}

/// Run the engine's own bounded decode and require it to agree with the raw
/// decoder above.
///
/// The two are given the same limits. If the raw decoder read the file and the
/// engine then called it too large, the engine's limits are not the ones it
/// claims to apply — which is the failure mode hard rule 4 is about, and which
/// only shows up when both paths are run on the same bytes.
pub fn assert_engine_agrees(raw_decoded: bool, engine: Result<image::DynamicImage, Error>) {
    if !raw_decoded {
        return;
    }
    if let Err(e) = engine {
        assert!(
            !is_resource_error(&e),
            "the decoder read this file under the same limits and decode_bounded \
             called it too large: {e}"
        );
    }
}

/// If a format's decoder accepted these bytes, the sniffer must call them that.
///
/// Both are signature checks over the same bytes, so a disagreement means a
/// decoder has been pointed at the wrong parser and something decoded anyway.
/// Only asserted for the decoders whose constructor validates the full
/// signature: WebP and ICO constructors are looser than `guess_format`, and
/// asserting there would be asserting a rule those decoders do not follow.
pub fn assert_sniffed(bytes: &[u8], expected: OutputFormat, raw_decoded: bool) {
    if !raw_decoded {
        return;
    }
    match pixelsmith_core::detect_format(bytes) {
        Ok(sniffed) => assert_eq!(
            sniffed, expected,
            "the {expected:?} decoder read bytes the sniffer calls {sniffed:?}"
        ),
        Err(e) => panic!("the {expected:?} decoder read bytes the sniffer rejects: {e}"),
    }
}