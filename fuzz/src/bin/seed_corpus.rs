//! Writes `fuzz/seeds/` — the committed starting corpus for every target.
//!
//! The corpus is committed rather than generated on the machine that fuzzes, for
//! two reasons. A fuzzer that starts from nothing spends most of its first ten
//! minutes discovering that a PNG starts with `\x89PNG`, and a corpus built on a
//! developer's laptop would differ from the one in CI, so a bug found by one
//! person would not be reproduced by the next. Committed seeds make the
//! starting point identical everywhere.
//!
//! Two kinds of seed go in:
//!
//! * **Real encodings**, produced by the same fixture generators the unit tests
//!   use (`worker::tests::photo` and friends, reproduced here). A fuzzer mutating
//!   a valid file finds depth-encoding bugs; a fuzzer mutating random bytes finds
//!   nothing but signature errors.
//! * **Hand-written malformed headers** — a PNG whose IHDR claims 60000x60000, a
//!   BMP with negative dimensions, an ICO directory claiming 65535 entries. These
//!   are the inputs that test the limit checks rather than the codecs, and they
//!   are built byte by byte here because no encoder will emit them.
//!
//! Run it with `bash scripts/fuzz.sh seed`. It is idempotent: every file is
//! rewritten from scratch, so re-running after a fixture change keeps the corpus
//! honest.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use pixelsmith_core::{OutputFormat, encode_fixed};

fn photo(w: u32, h: u32, seed: u8) -> image::DynamicImage {
    // The same shape as `worker::tests::photo`: smooth and compressible, so the
    // encoders produce something small enough to commit.
    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
        let fx = f64::from(x as u16) / f64::from(w.max(1)) * 6.0 + f64::from(seed);
        let fy = f64::from(y as u16) / f64::from(h.max(1)) * 4.0;
        let wave = (fx.sin() * 80.0 + fy.cos() * 55.0) + f64::from(seed) * 8.0;
        image::Rgb([
            (128.0 + wave).clamp(0.0, 255.0) as u8,
            (100.0 + wave * 0.6).clamp(0.0, 255.0) as u8,
            (150.0 - wave * 0.5).clamp(0.0, 255.0) as u8,
        ])
    }))
}

fn noise(w: u32, h: u32, seed: u8) -> image::DynamicImage {
    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([
            (x as u8).wrapping_mul(seed).wrapping_add(y as u8),
            (y as u8) ^ seed,
            seed,
        ])
    }))
}

fn alpha(w: u32, h: u32) -> image::DynamicImage {
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(w, h, |x, y| {
        image::Rgba([(x % 256) as u8, (y % 256) as u8, 90, (x * 2 % 256) as u8])
    }))
}

fn encoded(img: &image::DynamicImage, format: OutputFormat) -> Vec<u8> {
    // `unwrap` is fine here: this binary runs on a developer machine or in CI
    // over fixtures it just built, and every one of them is a format the encoder
    // supports. There is no untrusted input anywhere in this program.
    encode_fixed(img, format, 90).expect("engine cannot encode its own fixture")
}

/// Two-frame animated GIF, written by hand because `image`'s encoder emits a
/// single frame and `has_animated` reporting is only exercised by a real one.
fn animated_gif() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"GIF89a");
    out.extend_from_slice(&4u16.to_le_bytes()); // logical screen width
    out.extend_from_slice(&4u16.to_le_bytes()); // logical screen height
    out.push(0xF0); // global colour table, 2 entries
    out.push(0); // background colour index
    out.push(0); // pixel aspect ratio
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF]); // the two colours

    for index in 0..2u16 {
        out.extend_from_slice(&[0x21, 0xF9, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00]); // GCE
        out.extend_from_slice(&[0x2C, 0x00, 0x00, 0x00, 0x00]); // image descriptor at 0,0
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.push(0); // no local colour table
        out.push(2); // LZW minimum code size
        // "Clear", the two pixel values, "End": the smallest legal LZW stream
        // that decodes to one 4x4 frame of two colours.
        let lzw = if index == 0 {
            [0x02u8, 0x02, 0x4C, 0x01, 0x00]
        } else {
            [0x02u8, 0x03, 0x4C, 0x01, 0x00]
        };
        out.push(lzw.len() as u8);
        out.extend_from_slice(&lzw);
        out.push(0); // block terminator
    }
    out.push(0x3B); // trailer
    out
}

/// A PNG IHDR claiming 60000x60000 and nothing else.
///
/// The classic decompression bomb: 8 bytes of signature, 25 of chunk, and not one
/// pixel of data. Decoding it costs 14 GB, which is exactly what hard rule 4
/// exists to prevent, so it is the first thing every decoder target should meet.
fn png_bomb_header(w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    out.extend_from_slice(&13u32.to_be_bytes());
    out.extend_from_slice(b"IHDR");
    out.extend_from_slice(&w.to_be_bytes());
    out.extend_from_slice(&h.to_be_bytes());
    out.push(8); // bit depth
    out.push(2); // colour type: truecolour
    out.push(0); // deflate
    out.push(0); // adaptive filtering
    out.push(0); // no interlace
    out
}

/// A PNG whose IHDR is fine but whose IDAT expands forever: a deflate stream
/// that is all zeroes, so the decompressor keeps producing rows for a canvas
/// that has already run out of image.
///
/// This is the case a dimension check alone does not catch, which is why the
/// harness measures memory rather than trusting the header.
fn png_zip_bomb() -> Vec<u8> {
    let mut out = png_bomb_header(64, 64);
    let idat: &[u8] = &[0x78, 0x01, 0x01, 0x00, 0x00, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0xFF, 0xFF];
    out.extend_from_slice(&(idat.len() as u32).to_be_bytes());
    out.extend_from_slice(b"IDAT");
    out.extend_from_slice(idat);
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(b"IEND");
    out
}

/// `SOI`, an APP1/Exif segment, then nothing: the marker walk stops mid-file.
fn jpeg_truncated_after_exif() -> Vec<u8> {
    let mut out = vec![0xFF, 0xD8];
    let payload = b"Exif\0\0II*\0\x08\x00\x00\x00";
    let len = (payload.len() + 2) as u16;
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// SOI, a SOF0 declaring 65535x65535, and no scan data.
fn jpeg_bomb_header() -> Vec<u8> {
    let mut out = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08];
    out.extend_from_slice(&65535u16.to_be_bytes());
    out.extend_from_slice(&65535u16.to_be_bytes());
    out.push(1); // one component
    out.extend_from_slice(&[1, 0x11, 0]);
    out
}

/// A GIF logical screen of 40000x40000 with a single tiny frame.
fn gif_bomb_header() -> Vec<u8> {
    let mut out = b"GIF89a".to_vec();
    out.extend_from_slice(&40000u16.to_le_bytes());
    out.extend_from_slice(&40000u16.to_le_bytes());
    out.push(0x00); // no global colour table
    out.push(0);
    out.push(0);
    out.extend_from_slice(&[0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00]);
    out.push(2); // LZW minimum code size
    out.extend_from_slice(&[0x02, 0x02, 0x44, 0x01, 0x00]); // one 1x1 frame
    out.push(0x3B);
    out
}

/// Big-endian TIFF, IFD at offset 8, claiming 65535 directory entries.
fn tiff_lying_ifd() -> Vec<u8> {
    let mut out = b"MM\x00\x2a\x00\x00\x00\x08".to_vec();
    out.extend_from_slice(&65535u16.to_be_bytes());
    for i in 0..65535u16 {
        // 12 bytes per entry, tag/typetype/count/value.
        out.extend_from_slice(&0x0100u16.to_be_bytes());
        out.extend_from_slice(&3u16.to_be_bytes());
        out.extend_from_slice(&(i % 8).to_be_bytes());
        out.extend_from_slice(&(i as u32).to_be_bytes());
    }
    out.extend_from_slice(&0u32.to_be_bytes());
    out
}

/// A BMP declaring -40000x-40000, which is a signed overflow one line away from
/// a four-gigabyte allocation.
fn bmp_negative_dimensions() -> Vec<u8> {
    let mut out = b"BM".to_vec();
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(-40000i32).to_le_bytes());
    out.extend_from_slice(&(-40000i32).to_le_bytes());
    out.extend_from_slice(&[1u8, 0, 24, 0]);
    out
}

/// An ICO directory claiming 65535 entries, none of them present.
fn ico_entry_storm() -> Vec<u8> {
    let mut out = vec![0x00, 0x00, 0x01, 0x00];
    out.extend_from_slice(&65535u16.to_le_bytes());
    for i in 0..32u16 {
        // 16 bytes per entry: size, offset, then a 256x256-or-smaller size pair.
        out.push(if i % 256 == 0 { 0 } else { 32 });
        out.push(if i % 256 == 0 { 0 } else { 32 });
        out.push(0);
        out.push(0);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&65535u32.to_le_bytes());
    }
    out
}

/// `RIFF`/`WEBP`/`VP8X` with a 24-bit canvas of 16383x16383 — the largest the
/// extended format can express, which is still 1 GB of RGBA.
fn webp_extended_bomb_header() -> Vec<u8> {
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&30u32.to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(b"VP8X");
    out.extend_from_slice(&10u32.to_le_bytes());
    out.push(0x10); // flags only
    out.extend_from_slice(&[0u8; 3]);
    let canvas = 16383u32;
    out.extend_from_slice(&(canvas - 1).to_le_bytes()[..3]);
    out.extend_from_slice(&(canvas - 1).to_le_bytes()[..3]);
    out
}

#[derive(Clone)]
struct Seed {
    name: &'static str,
    bytes: Vec<u8>,
}

fn common_seeds() -> Vec<Seed> {
    vec![
        Seed { name: "photo.jpg", bytes: encoded(&photo(96, 64, 1), OutputFormat::Jpeg) },
        Seed { name: "noise.png", bytes: encoded(&noise(48, 32, 7), OutputFormat::Png) },
        Seed { name: "alpha.webp", bytes: encoded(&alpha(48, 32), OutputFormat::WebP) },
        Seed { name: "still.gif", bytes: encoded(&photo(32, 32, 2), OutputFormat::Gif) },
        Seed { name: "animated.gif", bytes: animated_gif() },
        Seed { name: "photo.tiff", bytes: encoded(&photo(40, 30, 3), OutputFormat::Tiff) },
        Seed { name: "photo.bmp", bytes: encoded(&photo(40, 30, 4), OutputFormat::Bmp) },
        Seed { name: "photo.ico", bytes: encoded(&alpha(32, 32), OutputFormat::Ico) },
        Seed { name: "bomb_png.png", bytes: png_bomb_header(60000, 60000) },
        Seed { name: "zip_bomb.png", bytes: png_zip_bomb() },
        Seed { name: "truncated_exif.jpg", bytes: jpeg_truncated_after_exif() },
        Seed { name: "bomb_jpeg.jpg", bytes: jpeg_bomb_header() },
        Seed { name: "bomb_gif.gif", bytes: gif_bomb_header() },
        Seed { name: "lying_ifd.tiff", bytes: tiff_lying_ifd() },
        Seed { name: "negative.bmp", bytes: bmp_negative_dimensions() },
        Seed { name: "storm.ico", bytes: ico_entry_storm() },
        Seed { name: "extended_bomb.webp", bytes: webp_extended_bomb_header() },
        Seed { name: "empty", bytes: Vec::new() },
        Seed { name: "text", bytes: b"this is not an image, it is a sentence".to_vec() },
        Seed { name: "magic_only_png", bytes: vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] },
    ]
}

fn named(files: &[(&'static str, Vec<u8>)]) -> Vec<Seed> {
    files
        .iter()
        .map(|(name, bytes)| Seed { name, bytes: bytes.clone() })
        .collect()
}

/// The repository root, derived from this crate's manifest so the seeds land in
/// the same place whatever directory cargo was invoked from.
fn roots() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("fuzz/ has a parent")
        .to_path_buf()
}

fn write(dir: &Path, seeds: &[Seed]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for existing in std::fs::read_dir(dir)? {
        let path = existing?.path();
        if path.extension().is_some_and(|e| e == "bin") {
            std::fs::remove_file(path)?;
        }
    }
    for seed in seeds {
        let mut file = std::fs::File::create(dir.join(seed.name))?;
        file.write_all(&seed.bytes)?;
    }
    Ok(())
}

fn main() -> std::io::Result<()> {
    let seeds = roots().join("fuzz").join("seeds");
    let common = common_seeds();
    let every_target = [
        "detect_format",
        "validate_bytes",
        "decode_bounded",
        "exif_read",
        "decode_jpeg",
        "decode_png",
        "decode_webp",
        "decode_gif",
        "decode_tiff",
        "decode_bmp",
        "decode_ico",
    ];

    let mut written = 0usize;
    for target in every_target {
        let dir = seeds.join(target);
        // Per-decoder targets get their own format's seeds plus the bomb
        // headers, because those are what that decoder has to survive. The four
        // entry-point targets see the whole pile, because they are handed any
        // file a user might pick.
        let subset: Vec<Seed> = match target {
            "decode_jpeg" => named(
                &[
                    ("photo.jpg", encoded(&photo(96, 64, 1), OutputFormat::Jpeg)),
                    ("bomb_jpeg.jpg", jpeg_bomb_header()),
                    ("truncated_exif.jpg", jpeg_truncated_after_exif()),
                    ("bomb_png.png", png_bomb_header(60000, 60000)),
                ],
            ),
            "decode_png" => named(
                &[
                    ("noise.png", encoded(&noise(48, 32, 7), OutputFormat::Png)),
                    ("alpha.png", encoded(&alpha(48, 32), OutputFormat::Png)),
                    ("bomb_png.png", png_bomb_header(60000, 60000)),
                    ("zip_bomb.png", png_zip_bomb()),
                ],
            ),
            "decode_webp" => named(
                &[
                    ("alpha.webp", encoded(&alpha(48, 32), OutputFormat::WebP)),
                    ("extended_bomb.webp", webp_extended_bomb_header()),
                ],
            ),
            "decode_gif" => named(
                &[
                    ("still.gif", encoded(&photo(32, 32, 2), OutputFormat::Gif)),
                    ("animated.gif", animated_gif()),
                    ("bomb_gif.gif", gif_bomb_header()),
                ],
            ),
            "decode_tiff" => named(
                &[
                    ("photo.tiff", encoded(&photo(40, 30, 3), OutputFormat::Tiff)),
                    ("lying_ifd.tiff", tiff_lying_ifd()),
                ],
            ),
            "decode_bmp" => named(
                &[
                    ("photo.bmp", encoded(&photo(40, 30, 4), OutputFormat::Bmp)),
                    ("negative.bmp", bmp_negative_dimensions()),
                ],
            ),
            "decode_ico" => named(
                &[
                    ("photo.ico", encoded(&alpha(32, 32), OutputFormat::Ico)),
                    ("storm.ico", ico_entry_storm()),
                ],
            ),
            _ => common.clone(),
        };
        write(&dir, &subset)?;
        written += subset.len();
        println!("{}: {} seeds", dir.display(), subset.len());
    }
    println!("{written} seed files under {}", seeds.display());
    Ok(())
}