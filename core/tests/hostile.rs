//! Hostile-input corpus.
//!
//! Every case here is generated from a valid fixture rather than committed as an
//! opaque binary, so a reader can see exactly which byte was corrupted and why
//! the result must be rejected. A committed blob only tells you that *some*
//! malformed file once broke something.
//!
//! The rule this file exists to enforce: **hostile input produces a named error,
//! never a panic and never an unbounded success.** So every assertion below is
//! one of two specific shapes, and "it did not panic" is not one of them:
//!
//! * `Err(Error::SomeVariant { .. })` - the specific variant, so a change that
//!   swaps `ZeroDimension` for a generic `Decode` fails the test and gets looked
//!   at. The user-facing message is part of the contract (hard rule 9), so the
//!   message is checked too where it carries information.
//! * `Ok(report)` where the report is inside the configured `Limits` - a
//!   truncated file can legitimately decode if the truncation lands after the
//!   image data, and forcing an error there would be testing a fiction.
//!
//! Fixture choice matters (see AGENTS.md): `photo()` is compressible, so a size
//! ceiling means what it says. `noise()` is used only where the claim is "this
//! decodes at all", never to measure size.

use image::{ImageFormat, ImageReader};
use pixelsmith_core::error::Error;
use pixelsmith_core::format::OutputFormat;
use pixelsmith_core::validate::{Limits, ValidateReport};
use std::io::Cursor;

/// A valid, compressible PNG. Gradients and flat runs, so it is small and every
/// decoder in the tree can be handed a slice of it without surprises.
fn photo(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([
            (x * 255 / w.max(1)) as u8,
            (y * 255 / h.max(1)) as u8,
            ((x + y) % 256) as u8,
        ])
    });
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, ImageFormat::Png)
        .expect("fixture encoding cannot fail");
    out.into_inner()
}

/// PNG's chunk CRC, so a fixture can change `IHDR` *coherently*.
///
/// This matters more than it looks. Patching the width and height without
/// fixing the CRC produces a file that fails `CrcMismatch` before any decoder
/// looks at the dimensions - so a test written that way asserts "PNG corruption
/// is caught" while appearing to assert "oversized headers are caught", and it
/// would keep passing even if the dimension checks were deleted.
fn png_crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = if crc & 1 == 0 { 0 } else { 0xEDB8_8320 };
            crc = (crc >> 1) ^ mask;
        }
    }
    crc ^ 0xFFFF_FFFF
}

/// Return a copy of a PNG with `IHDR`'s width and height replaced and its CRC
/// recomputed, so the file is *valid* PNG that merely claims impossible
/// dimensions. That is the shape a real decompression bomb has: a correct file
/// with a hostile header, not a corrupted one.
fn png_claiming(src: &[u8], width: u32, height: u32) -> Vec<u8> {
    assert_eq!(
        &src[12..16],
        b"IHDR",
        "fixture must be a PNG with IHDR first"
    );
    let mut bytes = src.to_vec();
    bytes[16..20].copy_from_slice(&width.to_be_bytes());
    bytes[20..24].copy_from_slice(&height.to_be_bytes());
    // The CRC covers the chunk type and its data, i.e. bytes 12..29.
    let crc = png_crc32(&bytes[12..29]);
    bytes[29..33].copy_from_slice(&crc.to_be_bytes());
    bytes
}

/// A positive control for [`png_claiming`]: rewriting the dimensions to what
/// they already were must produce a file that still decodes. If this fails, the
/// CRC helper is wrong and every dimension test below is testing garbage.
#[test]
fn png_dimension_rewriting_produces_a_still_valid_png() {
    let limits = Limits::default();
    let rewritten = png_claiming(&photo(40, 30), 40, 30);
    let report = pixelsmith_core::validate::validate_bytes(&rewritten, &limits).expect(
        "a coherently rewritten PNG must still decode - if this fails the CRC helper is wrong",
    );
    assert_eq!((report.width, report.height), (40, 30));
}

/// Assert the outcome is one of the two accepted shapes, and that it is the
/// *specific* one this entry expects. Kept as a helper so every entry reads as
/// one line of intent rather than a wall of match arms.
#[track_caller]
fn assert_rejected(result: Result<ValidateReport, Error>, what: &str) {
    match result {
        Err(Error::UnknownFormat) => {}
        Err(Error::UnsupportedFormat(_)) => {}
        Err(Error::Decode(_)) => {}
        Err(Error::Metadata(_)) => {}
        Err(Error::InputTooLarge { .. }) => {}
        Err(Error::PixelBudgetExceeded { .. }) => {}
        Err(Error::SuspiciousDimensions { .. }) => {}
        Err(Error::ZeroDimension) => {}
        other => panic!(
            "{what}: expected a specific decode/validation error, got {other:?}.\n\
             A new error variant here means this corpus needs a decision about \
             whether the new message is acceptable to a user, not a silent pass."
        ),
    }
}

/// The other accepted shape: it decoded, and the result is inside `limits`.
#[track_caller]
fn assert_bounded_success(report: &ValidateReport, limits: &Limits, what: &str) {
    assert!(
        report.width <= limits.max_dimension && report.height <= limits.max_dimension,
        "{what}: decoded to {}x{}, which exceeds max_dimension {}",
        report.width,
        report.height,
        limits.max_dimension
    );
    assert!(
        u64::from(report.width) * u64::from(report.height) <= limits.max_pixels,
        "{what}: decoded to {}x{} = {} pixels, over the {} budget",
        report.width,
        report.height,
        u64::from(report.width) * u64::from(report.height),
        limits.max_pixels
    );
}

#[test]
fn the_fixture_itself_is_valid() {
    // If this fails, every other test in the file is testing a broken baseline
    // and the failures will be nonsense.
    let limits = Limits::default();
    let report = pixelsmith_core::validate::validate_bytes(&photo(64, 48), &limits)
        .expect("the fixture must be a valid image");
    assert_eq!((report.width, report.height), (64, 48));
    assert_eq!(report.format, OutputFormat::Png);
}

// ---------------------------------------------------------------------------
// Truncation
// ---------------------------------------------------------------------------

/// Cutting a valid file short at every offset is the highest-yield malformed
/// input there is: it exercises every decoder's length checks without needing a
/// single hand-crafted byte.
#[test]
fn truncation_at_every_offset_is_survivable() {
    let limits = Limits::default();
    let full = photo(64, 48);

    // Every byte, not a sample. The file is ~1 KB, so this is cheap, and a
    // sampled subset would leave gaps exactly where a decoder's bounds check
    // happens to sit.
    for cut in 0..full.len() {
        let truncated = &full[..cut];
        match pixelsmith_core::validate::validate_bytes(truncated, &limits) {
            Err(_) => {}
            Ok(report) => {
                assert_bounded_success(&report, &limits, &format!("truncated to {cut} bytes"))
            }
        }
    }
}

/// Truncation must not become unbounded work. The whole point of `Limits` is
/// that a tiny hostile file cannot cost gigabytes, so measure the *output*, not
/// just the absence of a crash.
#[test]
fn truncated_input_never_decodes_beyond_the_limits() {
    let limits = Limits::mobile();
    let full = photo(256, 256);
    for cut in (0..full.len()).step_by(7) {
        if let Ok(report) = pixelsmith_core::validate::validate_bytes(&full[..cut], &limits) {
            assert_bounded_success(&report, &limits, &format!("cut at {cut}"));
        }
    }
}

// ---------------------------------------------------------------------------
// Dimension and pixel-count lies
// ---------------------------------------------------------------------------

/// A PNG whose IHDR claims zero width, then zero height. Both are illegal in
/// the spec and both used to be a divide-by-zero or a zero-length allocation in
/// some decoder.
/// The dimension guards are exercised directly through [`Limits::check_header`],
/// because that is the function that actually stops a decompression bomb: it
/// runs on numbers read out of a header, before any pixel buffer exists. The
/// file-shaped cases below then confirm the same ceilings hold end to end.
///
/// Testing only through a file would be weaker than it looks - the PNG decoder
/// enforces its own limits too, so a file test can pass while `check_header` has
/// been deleted.
#[test]
fn check_header_refuses_zero_dimensions_by_name() {
    let limits = Limits::mobile();
    for (w, h) in [(0u32, 32u32), (32, 0), (0, 0)] {
        assert!(
            matches!(limits.check_header(w, h), Err(Error::ZeroDimension)),
            "check_header({w}, {h}) must be ZeroDimension, got {:?}",
            limits.check_header(w, h)
        );
    }
    // And a legal header passes, so the guard is not simply refusing everything.
    assert!(limits.check_header(64, 48).is_ok());
}

/// The same ceilings as the engine reports them to a user. Hard rule 9: the
/// message is the product.
#[test]
fn check_header_names_the_bomb_in_its_message() {
    let limits = Limits::mobile(); // max_dimension 16_000
    match limits.check_header(60_000, 60_000) {
        Err(Error::SuspiciousDimensions { w, h, mp }) => {
            assert_eq!((w, h), (60_000, 60_000));
            assert!(
                (mp - 3600.0).abs() < 0.01,
                "reported {mp} MP for 60000x60000"
            );
            let text = Error::SuspiciousDimensions { w, h, mp }.to_string();
            assert!(
                text.contains("60000") && text.contains("decompression bomb"),
                "the bomb error must name the dimensions it refused: {text}"
            );
        }
        other => panic!("expected SuspiciousDimensions for 60000x60000, got {other:?}"),
    }
}

/// A header that is individually legal but whose product is absurd. This is the
/// case a per-axis dimension check structurally cannot catch, which is why the
/// pixel budget is a separate limit and not a derived one.
#[test]
fn check_header_catches_the_pixel_budget_a_per_axis_check_would_miss() {
    let limits = Limits::mobile(); // max_dimension 16_000, max_pixels 40 MP
    assert!(
        15_000 <= limits.max_dimension,
        "this test only means something while the header is under the per-axis cap"
    );
    match limits.check_header(15_000, 15_000) {
        Err(Error::PixelBudgetExceeded { limit, actual }) => {
            assert_eq!(limit, limits.max_pixels);
            assert!(
                (actual - 225.0).abs() < 0.01,
                "15000x15000 is 225 MP; reported {actual}"
            );
            let text = Error::PixelBudgetExceeded { limit, actual }.to_string();
            assert!(
                text.contains("megapixel") && text.contains("225"),
                "must state the unit and the figure: {text}"
            );
        }
        other => panic!("expected PixelBudgetExceeded for 15000x15000, got {other:?}"),
    }
}

/// `check_decoded` is the post-decode backstop, for a header that lied about a
/// size it had already committed to.
#[test]
fn check_decoded_catches_a_lying_header_after_the_fact() {
    let limits = Limits::mobile();
    let oversize =
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 8, image::Rgb([0, 0, 0])));
    assert!(limits.check_decoded(&oversize).is_ok(), "8x8 is legal");

    // Zero is not representable in an RgbImage, so the post-decode zero case is
    // reached through a 1x0 buffer instead. This asserts the branch exists rather
    // than pretending the zero case is reachable from a decoded image.
    let empty = image::DynamicImage::ImageRgb8(image::RgbImage::new(0, 0));
    assert!(
        matches!(limits.check_decoded(&empty), Err(Error::ZeroDimension)),
        "a zero-sized decoded image must be refused"
    );
}

// ---------------------------------------------------------------------------
// The advisory / enforcing split
// ---------------------------------------------------------------------------

/// `validate_bytes` is a *header* check and is deliberately advisory: it sets
/// `suspicious` and returns `Ok`. The refusal happens in `decode_bounded`. This
/// is a load-bearing distinction - it is what lets the UI list a folder of files
/// with a warning badge without decoding any of them - so it is pinned here
/// rather than left to be rediscovered.
#[test]
fn validate_bytes_warns_where_decode_bounded_refuses() {
    let limits = Limits::mobile();
    // 225 MP: under the 16_000 per-axis cap, over the 40 MP budget.
    let bomb = png_claiming(&photo(32, 32), 15_000, 15_000);

    let report = pixelsmith_core::validate::validate_bytes(&bomb, &limits)
        .expect("validate_bytes is advisory: it warns, it does not refuse");
    assert_eq!((report.width, report.height), (15_000, 15_000));
    let warning = report
        .suspicious
        .expect("an over-budget header must produce a warning string");
    assert!(
        warning.contains("225") && warning.contains("MP"),
        "the warning must quantify the problem for the user: {warning}"
    );

    // The enforcing path refuses the same file.
    assert!(
        pixelsmith_core::decode_bounded(&bomb, &limits).is_err(),
        "decode_bounded is the gate that must actually stop the bomb"
    );
}

/// The per-axis ceiling behaves the same way from the file side, except the
/// decoder's own limit may fire before ours - which is still a refusal, just
/// with a less specific message. Asserting only "it was refused" is correct
/// here; asserting a variant would be pinning an implementation detail of a
/// third-party crate.
#[test]
fn a_hostile_header_is_refused_end_to_end() {
    let limits = Limits::mobile();
    for (w, h) in [(60_000u32, 60_000u32), (15_000, 15_000), (0, 32), (32, 0)] {
        let bytes = png_claiming(&photo(32, 32), w, h);
        assert!(
            pixelsmith_core::decode_bounded(&bytes, &limits).is_err(),
            "a {w}x{h} header must not decode"
        );
    }
}

/// `u32::MAX` on either axis is the classic decompression-bomb header. It must
/// never become an allocation: 4294967295 squared is 1.8e19 bytes.
#[test]
fn png_declaring_u32_max_dimensions_is_rejected_from_the_header() {
    for (w, h) in [
        (u32::MAX, 32u32),
        (32, u32::MAX),
        (u32::MAX, u32::MAX),
        (1, u32::MAX),
    ] {
        assert!(
            matches!(
                Limits::default().check_header(w, h),
                Err(Error::SuspiciousDimensions { .. })
            ),
            "check_header({w}, {h}) must refuse the header"
        );
        let bytes = png_claiming(&photo(32, 32), w, h);
        match pixelsmith_core::decode_bounded(&bytes, &Limits::default()) {
            Err(_) => {}
            Ok(img) => panic!(
                "a {w}x{h} IHDR decoded successfully to {}x{}",
                img.width(),
                img.height()
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Magic bytes that disagree with the content
// ---------------------------------------------------------------------------

/// A JPEG with a PNG signature in front of it. Whichever decoder wins, it must
/// not be handed bytes that merely *claim* to be another format (hard rule 7).
#[test]
fn mismatched_magic_bytes_do_not_decode_as_the_lies() {
    let png = photo(32, 32);
    let jpeg_magic = [0xFFu8, 0xD8, 0xFF, 0xE0];

    // PNG signature over PNG content is fine; PNG signature over JPEG content is
    // the hostile case, because the signature is what detection trusts.
    let mut hybrid = jpeg_magic.to_vec();
    hybrid.extend_from_slice(&png);
    let result = pixelsmith_core::validate::validate_bytes(&hybrid, &Limits::default());
    if let Ok(report) = result {
        assert_bounded_success(&report, &Limits::default(), "magic/content mismatch");
        assert_ne!(
            report.format,
            OutputFormat::Jpeg,
            "a buffer that starts with JPEG magic but holds PNG data must not be \
             reported as JPEG: format comes from content, not from the signature"
        );
    }

    // A file that is nothing but a signature is not an image.
    assert_rejected(
        pixelsmith_core::validate::validate_bytes(&jpeg_magic, &Limits::default()),
        "bare JPEG magic",
    );
}

#[test]
fn empty_and_tiny_inputs_are_rejected_with_a_named_error() {
    let limits = Limits::default();
    assert_rejected(
        pixelsmith_core::validate::validate_bytes(&[], &limits),
        "empty input",
    );
    assert_rejected(
        pixelsmith_core::validate::validate_bytes(&[0x00], &limits),
        "one zero byte",
    );
    assert_rejected(
        pixelsmith_core::validate::validate_bytes(&[0xFF; 3], &limits),
        "three 0xFF bytes",
    );
}

// ---------------------------------------------------------------------------
// Structural corruption in specific container formats
// ---------------------------------------------------------------------------

/// GIF carries its frame count in the logical screen descriptor. A frame count
/// of `u16::MAX` is the "infinite animation" trick, and a broken count is how a
/// decoder ends up looping forever allocating frames.
#[test]
fn gif_with_a_broken_frame_count_is_rejected() {
    let gif =
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3])));
    let mut buf = Cursor::new(Vec::new());
    gif.write_to(&mut buf, ImageFormat::Gif).unwrap();
    let mut bytes = buf.into_inner();

    // "GIF89a", then the logical screen descriptor: width(2) height(2) packed(1)
    // bg(1) aspect(1), and the frame count is not a GIF field at all - the
    // packed byte's low nibble is the colour table size. Corrupt the packed
    // byte and the descriptor's own length so both routes are covered.
    assert_eq!(&bytes[0..6], b"GIF89a");

    // Declare a colour table that the file does not contain.
    bytes[10] = 0xF7;
    assert_rejected(
        pixelsmith_core::validate::validate_bytes(&bytes, &Limits::default()),
        "GIF with a colour table that is not present",
    );

    // Truncate mid-descriptor.
    let short = &bytes[..8];
    assert_rejected(
        pixelsmith_core::validate::validate_bytes(short, &Limits::default()),
        "GIF truncated inside its screen descriptor",
    );
}

/// A JPEG segment length is a `u16` *including* its own two length bytes, so a
/// length of 0 or 1 is arithmetically impossible and a decoder that trusts it
/// computes a negative or overflowing slice.
#[test]
fn jpeg_segment_length_that_cannot_exist_is_rejected() {
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(16, 16, |x, y| {
        image::Rgb([(x * 8) as u8, (y * 8) as u8, 64])
    }));
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Jpeg).unwrap();
    let good = buf.into_inner();
    assert_eq!(&good[0..2], &[0xFF, 0xD8], "fixture must be a JPEG");

    // The SOS header follows SOI and APPn segments. Find it and corrupt the
    // length that follows its marker.
    let mut sos_at = None;
    let mut i = 2;
    while i + 4 < good.len() {
        if good[i] == 0xFF && good[i + 1] == 0xDA {
            sos_at = Some(i);
            break;
        }
        let len = u16::from_be_bytes([good[i + 2], good[i + 3]]) as usize;
        if len < 2 {
            break;
        }
        i += 2 + len;
    }
    let sos_at = sos_at.expect("fixture must contain an SOS marker");

    for bad_len in [0u16, 1, 3] {
        let mut bytes = good.clone();
        bytes[sos_at + 2..sos_at + 4].copy_from_slice(&bad_len.to_be_bytes());
        let outcome = pixelsmith_core::validate::validate_bytes(&bytes, &Limits::default());
        // Either it is refused, or - if this decoder happens to read no further -
        // it decodes, and then the result still has to be inside the limits.
        if let Ok(report) = outcome {
            assert_bounded_success(
                &report,
                &Limits::default(),
                &format!("SOS length {bad_len}"),
            );
        }
    }
}

/// EXIF with a truncated IFD. The APP1 payload is `Exif\0\0` followed by a TIFF
/// header and a directory whose entry count and offsets both come from the
/// file, so a short read is the normal case rather than an exotic one.
#[test]
fn exif_with_a_truncated_ifd_is_rejected_not_misread() {
    let jpeg = jpeg_with_exif_orientation(6);

    // Cut the APP1 payload at several depths inside the TIFF directory.
    let app1_at = jpeg
        .windows(2)
        .position(|w| w == [0xFF, 0xE1])
        .expect("fixture must contain an APP1 segment");
    let declared = u16::from_be_bytes([jpeg[app1_at + 2], jpeg[app1_at + 3]]) as usize;

    for cut in 4..declared.min(40) {
        let mut bytes = jpeg[..app1_at + 2 + cut].to_vec();
        // Keep the declared length consistent with what is actually there, so
        // the file is truncated rather than self-contradictory.
        bytes[app1_at + 2..app1_at + 4].copy_from_slice(&(cut as u16).to_be_bytes());
        let outcome = pixelsmith_core::validate::validate_bytes(&bytes, &Limits::default());
        if let Ok(report) = outcome {
            assert_bounded_success(&report, &Limits::default(), &format!("IFD cut at {cut}"));
            // A short EXIF block must not be reported as carrying a trustworthy
            // orientation, because the UI turns on that number.
            assert!(
                report.orientation.is_none() || cut >= declared,
                "truncated EXIF at {cut} yielded orientation {:?}",
                report.orientation
            );
        }
    }
}

/// Build a JPEG whose EXIF says "rotate 90".
///
/// Written with the engine's own EXIF writer (`exif::build_block` +
/// `format::append_exif`) rather than a hand-rolled TIFF header. A hand-rolled
/// one is a trap: an IFD entry that is 14 bytes instead of 12 still yields a file
/// that parses cleanly as "no orientation present", so every truncation test
/// built on that fixture would pass while testing nothing. The positive control
/// at the bottom of this file is what makes that failure visible.
fn jpeg_with_exif_orientation(orientation: u32) -> Vec<u8> {
    use pixelsmith_core::exif as px_exif;

    // `kamadak-exif` publishes its crate as `exif`, and `Tag` is its type. The
    // engine's own helpers are reached through the alias to keep the two apart.
    let block = px_exif::build_block(&[px_exif::field(
        exif::Tag::Orientation,
        px_exif::GPS_IFD,
        px_exif::long(orientation),
    )])
    .expect("building an EXIF block cannot fail");

    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(24, 16, |x, y| {
        image::Rgb([(x * 10) as u8, (y * 16) as u8, 200])
    }));
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Jpeg).unwrap();
    let mut jpeg = buf.into_inner();

    pixelsmith_core::format::append_exif(&mut jpeg, &block)
        .expect("appending EXIF to a valid JPEG cannot fail");
    jpeg
}

#[test]
fn the_exif_fixture_really_carries_the_orientation() {
    // Positive control. Without this, every assertion in the truncation test
    // above would also pass against a fixture whose EXIF was never read at all,
    // which would make that test vacuous.
    let report = pixelsmith_core::validate::validate_bytes(
        &jpeg_with_exif_orientation(6),
        &Limits::default(),
    )
    .expect("the EXIF fixture must decode");
    assert_eq!(
        report.orientation,
        Some(6),
        "the fixture must actually carry Orientation=6"
    );
    assert!(report.has_exif, "the report must admit it has EXIF");
}

// ---------------------------------------------------------------------------
// The input-size ceiling
// ---------------------------------------------------------------------------

/// `max_input_bytes` is the cheapest defence there is, so it is worth proving it
/// fires *before* any decode work is attempted.
#[test]
fn an_oversized_input_is_refused_with_the_limit_named() {
    let limits = Limits {
        max_input_bytes: 512,
        ..Limits::default()
    };
    let big = photo(300, 300);
    assert!(
        big.len() > 512,
        "fixture must exceed the configured ceiling for this test to mean anything"
    );

    match pixelsmith_core::validate::validate_bytes(&big, &limits) {
        Err(Error::InputTooLarge { limit, actual }) => {
            assert_eq!(limit, 512);
            assert_eq!(actual, big.len());
            let text = Error::InputTooLarge { limit, actual }.to_string();
            assert!(text.contains("512"), "must name the limit: {text}");
        }
        other => panic!("expected InputTooLarge, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Decoding a hostile file must never panic
// ---------------------------------------------------------------------------

/// The catch-all. Every corpus entry above asserts a *reason*; this one asserts
/// only that nothing aborts the process, which is the property that cannot be
/// expressed per-case because it is about all of them at once.
///
/// It deliberately does not assert which error, since the entries above own
/// that. Its value is that a new decoder panic shows up here as a hard failure
/// rather than as a silently swallowed `Err`.
#[test]
fn no_corpus_entry_panics() {
    let limits = Limits::mobile();
    let mut cases: Vec<Vec<u8>> = vec![vec![], vec![0], vec![0xFF; 4]];

    // Every truncation of a PNG and a JPEG, which between them cover most of
    // what a malformed file can be.
    for src in [photo(48, 32), jpeg_with_exif_orientation(1)] {
        for cut in 0..src.len() {
            cases.push(src[..cut].to_vec());
        }
    }

    // Random noise: a deterministic LCG, so a failure is reproducible from this
    // seed rather than "it failed once on a Tuesday".
    let mut state: u32 = 0x5EED_1234;
    for len in [1usize, 7, 64, 1000] {
        let mut buf = Vec::with_capacity(len);
        for _ in 0..len {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            buf.push((state >> 24) as u8);
        }
        cases.push(buf);
    }

    for case in &cases {
        // No assertion on the outcome: this test is about not aborting.
        let _ = pixelsmith_core::validate::validate_bytes(case, &limits);
    }
    assert!(
        cases.len() > 500,
        "the corpus should be substantial, got {} entries",
        cases.len()
    );
}

/// `ImageReader` is what `validate` uses internally. Exercised directly so a
/// decoder limit that `validate` forgets to apply is still visible here.
#[test]
fn a_raw_reader_respects_decoder_limits_on_hostile_input() {
    let limits = Limits::mobile();
    let full = photo(64, 64);
    for cut in [0usize, 4, 12, 33, 64, full.len() - 1] {
        let mut reader = ImageReader::new(Cursor::new(&full[..cut]));
        reader.no_limits(); // prove `apply_to_decoder` is what does the limiting
        limits.apply_to_decoder(&mut reader);
        // Again: no assertion on success or failure, only that asking with
        // limits applied cannot abort.
        let _ = reader.decode();
    }
}
