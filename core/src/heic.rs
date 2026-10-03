//! HEIC/HEIF input: the format a phone camera actually writes.
//!
//! Every competitor in this space advertises HEIC support, so the honest
//! position is either "it works" or "it says it does not work". This module is
//! behind the `heic` feature and says both: [`crate::Capabilities`] reports
//! `heic_decode`, and a build without the feature refuses a HEIC with an error
//! that names the feature rather than pretending the file is unreadable.
//!
//! Three things are load-bearing here, and each is a way the obvious
//! implementation goes wrong:
//!
//! * **Detection is by container brand, not by extension and not by the
//!   codec.** [`detect`] reads the `ftyp` box, which is the first twelve bytes
//!   of the file and cannot be forged by renaming a JPEG. A file whose brand
//!   says AVIF is reported as AVIF, so the UI can name the format even when it
//!   cannot open it.
//! * **Dimensions come from a metadata box, not from a decode.** [`header`]
//!   parses `ispe` and stops. That is what lets
//!   [`crate::validate::validate_bytes`] apply [`Limits`] to a HEIC before a
//!   single pixel buffer exists — the same order every other format gets.
//! * **Encoding HEIC is not implemented and says so.** The read-only formats
//!   are refused by [`crate::format::encode`] with a message that tells the
//!   user what to do instead.
//!
//! # The codec
//!
//! `heic-rs`: pure Rust, `#![forbid(unsafe_code)]`, MIT OR Apache-2.0, one
//! optional dependency, and a `no_std` core that performs no I/O — it takes
//! `&[u8]` and returns a value, which is exactly the shape this engine needs.
//! `docs/HEIC.md` has the comparison it was chosen over, including the two
//! candidates rejected for licence and for build weight.
//!
//! Only HEVC-coded HEIF is decoded. AV1-coded HEIF — an `.avif` in the same
//! container — is recognised by [`detect`] and refused by name, because telling
//! a user their HEIC is "damaged" when it is a codec we never implemented is the
//! kind of lie hard rule 9 exists to prevent.

use crate::error::{Error, Result};
use crate::format::OutputFormat;
use crate::validate::Limits;

/// Every way reading a HEIF file can fail, as our own error type.
///
/// The decoder's own error type is deliberately not this module's public
/// surface: it is `#[non_exhaustive]`, and its messages name the crate that
/// produced them. Hard rule 9 asks for text a user can act on, so the two cases
/// a user can act on differently — "we did not build this" and "the picture
/// inside uses a codec we do not read" — are variants here rather than strings
/// passed through.
#[derive(Debug, thiserror::Error)]
pub enum HeicError {
    /// The build has no HEIC decoder compiled in.
    #[error(
        "this build has no HEIC/HEIF decoder, so iPhone photos cannot be read. \
         Rebuilding with the `heic` feature adds it; until then, share the photo \
         as JPEG or PNG and it will open"
    )]
    NotBuilt,

    /// The container parses, but the picture inside it uses a coding tool this
    /// build does not implement. A limitation of ours, not a fault in the file,
    /// and the two need different words.
    #[error(
        "this HEIF file's picture coding is not HEVC, which is the only kind this \
         build decodes. If it is an AVIF or a still image sequence, that is what \
         this is; a HEIC straight off a camera will open"
    )]
    UnsupportedCoding,

    /// The container or its bitstream was refused. `detail` is the decoder's own
    /// reason, which names the box or field it gave up on.
    #[error(
        "this HEIC/HEIF file could not be read: {detail}. Re-saving it as JPEG or PNG will always work"
    )]
    Rejected { detail: String },
}

/// What a HEIF file says about itself, from its metadata alone.
///
/// Every field is read without decoding a pixel, so this is safe to call on a
/// whole folder of untrusted files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Width the caller will receive, after `clap`, `irot` and `imir`.
    pub width: u32,
    /// Height the caller will receive.
    pub height: u32,
    /// Width as coded, which is what the decoder allocates for.
    pub coded_width: u32,
    /// Height as coded.
    pub coded_height: u32,
    /// Luma bit depth.
    pub bit_depth: u8,
    /// Whether an `Exif` item describes the primary image.
    ///
    /// A HEIF file keeps its EXIF in a separate item referenced from the
    /// picture, not in a JPEG APP1 segment, so [`crate::exif::read`] cannot see
    /// it. Asking the container is the only way
    /// [`crate::validate::ValidateReport::has_exif`] stays honest about a photo
    /// that really does carry metadata.
    pub has_exif: bool,
}

/// True when this build can decode HEVC-coded HEIF.
pub const fn built() -> bool {
    cfg!(feature = "heic")
}

/// Identify a HEIF file from its `ftyp` box, if it has one.
///
/// Returns [`OutputFormat::Avif`] for an AVIF-coded HEIF too, because naming the
/// format correctly is useful even when the decoder is absent: the UI's job is
/// to tell the user what the file is, and only then whether it can open it.
///
/// Allocation-free by construction — it reads the first box and nothing else.
/// Never returns `Some` for bytes that are some other format, which is what
/// lets [`crate::format::detect_format`] fall back to it.
pub fn detect(input: &[u8]) -> Option<OutputFormat> {
    let (major, compatible) = ftyp(input)?;
    brand_format(major).or_else(|| {
        compatible
            .chunks_exact(4)
            .find_map(|c| brand_format([c[0], c[1], c[2], c[3]]))
    })
}

/// The major brand and the compatible-brand list of a file's `ftyp` box.
///
/// Hand-rolled rather than delegated to the decoder's own `ftyp` parser for two
/// reasons: it has to work in a build with no decoder compiled in, and it has to
/// answer "AVIF" where that parser answers "unsupported". A file truncated after
/// its brand still reports the brand, because that is exactly the file the user
/// needs an explanation for.
fn ftyp(input: &[u8]) -> Option<([u8; 4], &[u8])> {
    // A box is a 32-bit size, a four-character type, then a payload. A size of 1
    // means a 64-bit size follows and a size of 0 means "to the end of the
    // file"; neither belongs in the first box of a file, and honouring either
    // would mean trusting a length field to bound a read.
    let size = u32::from_be_bytes(input.get(0..4)?.try_into().ok()?) as usize;
    if input.get(4..8)? != b"ftyp" || size < 16 {
        return None;
    }
    // Clamped to what is actually there rather than refused, so that a file cut
    // short after its brand is still identified as the thing it was.
    let payload = input.get(8..size.min(input.len()))?;
    Some((payload.get(0..4)?.try_into().ok()?, payload.get(8..)?))
}

fn brand_format(code: [u8; 4]) -> Option<OutputFormat> {
    Some(match &code {
        // The HEVC still-picture brands: `heic` is the ordinary iPhone photo,
        // `heix` the same with 10-bit and extended-range profiles, `heim`
        // multiview, `heis` layered. All four are HEVC in this container.
        b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" => OutputFormat::Heic,
        // The generic HEIF brands, for a file whose still image is coded
        // something else — which we recognise and then refuse by name.
        b"mif1" | b"msf1" => OutputFormat::Heif,
        b"avif" | b"avis" => OutputFormat::Avif,
        _ => return None,
    })
}

/// Read a HEIF file's geometry and metadata, decoding nothing.
///
/// This is the header path [`crate::validate::validate_bytes`] uses for a HEIC,
/// in place of the image-header read it does for every other format. It reads
/// the container's property boxes and stops, so a file declaring a 60000x60000
/// picture costs a few hundred bytes of parsing to reject.
pub fn header(input: &[u8]) -> Result<Header> {
    // The codec is not linked into a build without the feature, so the two arms
    // are two different functions wearing one signature. Detection is *not*
    // behind the feature, so a build without it still says "this is a HEIC" —
    // it just cannot open one, and says which feature would.
    #[cfg(feature = "heic")]
    {
        let info = heic_rs::probe(input).map_err(from_decoder)?;
        Ok(Header {
            width: info.width,
            height: info.height,
            coded_width: info.coded_width,
            coded_height: info.coded_height,
            bit_depth: info.bit_depth,
            has_exif: info.has_exif,
        })
    }
    #[cfg(not(feature = "heic"))]
    {
        let _ = input;
        Err(Error::Heic(HeicError::NotBuilt))
    }
}

/// Decode a HEIF file to pixels, with the same limits every other format gets.
///
/// The order is the one `docs/ARCHITECTURE.md` fixes: the declared size is
/// checked against [`Limits`] before the decoder is allowed to allocate
/// anything, and the result is checked again on the way out. A HEIC is the most
/// expensive input the product accepts — a 12 MP HEVC intra frame is the largest
/// thing any user will ever hand this engine — so the bound is enforced from the
/// container, not from the codec.
pub fn decode(input: &[u8], limits: &Limits) -> Result<image::DynamicImage> {
    #[cfg(feature = "heic")]
    {
        let header = header(input)?;

        // Both sizes, not just the delivered one. The decoder allocates from the
        // coded size, and a `clap` or `irot` can only make the delivered size
        // larger: a 90-degree rotation turns a 20000x400 panorama into a
        // 400x20000 one, which is the same pixel count but fails a per-side
        // limit.
        limits.check_header(header.coded_width, header.coded_height)?;
        limits.check_header(header.width, header.height)?;

        // RGBA out, because an iPhone HEIC can carry an auxiliary alpha plane
        // and discarding it here would flatten a cut-out photo to black. Four
        // bytes a pixel is exactly what `Limits::apply_to_decoder` allows the
        // other decoders, so the peak does not move.
        let options = heic_rs::DecodeOptions::default()
            .with_layout(heic_rs::PixelLayout::Rgba8)
            .with_max_pixels(Some(limits.max_pixels))
            // Serial per file. The engine's own rayon pool runs the files, and a
            // nested pool inside a rayon worker would oversubscribe the machine.
            .with_threads(Some(1));

        let decoded = heic_rs::decode(input, &options).map_err(from_decoder)?;
        let pixels = image::RgbaImage::from_raw(decoded.width, decoded.height, decoded.data)
            .ok_or(Error::Heic(HeicError::Rejected {
                detail: "the decoder returned a buffer that does not match the size it declared"
                    .to_owned(),
            }))?;
        let img = image::DynamicImage::ImageRgba8(pixels);
        // Belt to the braces above: this is the assertion that keeps a decoder
        // regression from becoming an allocation the caller never agreed to.
        limits.check_decoded(&img)?;
        Ok(img)
    }
    #[cfg(not(feature = "heic"))]
    {
        let _ = (input, limits);
        Err(Error::Heic(HeicError::NotBuilt))
    }
}

/// Translate the decoder's failure into one a user can act on.
#[cfg(feature = "heic")]
fn from_decoder(err: heic_rs::Error) -> Error {
    use heic_rs::Error as E;
    Error::Heic(match err {
        // Its text names its own crate and its own vocabulary, which is not what
        // a user needs; `UnsupportedCoding` says the same thing in ours.
        E::Unsupported(_) => HeicError::UnsupportedCoding,
        E::PixelLimit { pixels, max_pixels } => HeicError::Rejected {
            detail: format!(
                "it declares {pixels} pixels, above the {max_pixels} this build allows"
            ),
        },
        other => HeicError::Rejected {
            detail: other.to_string(),
        },
    })
}

// Every test here decodes a real HEIF, which needs the codec. Gated rather than
// skipped: `cargo test` with no features must still compile and pass, and a test
// that quietly does nothing is worse than one that is not built.
#[cfg(all(test, feature = "heic"))]
mod tests {
    use super::*;
    use crate::validate::Limits;
    use image::GenericImageView;

    #[test]
    fn a_synthetic_heic_is_recognised_by_its_brand_bytes() {
        let bytes = synthetic::heic(64, 64);
        // The brand sits at a fixed offset, which is what "magic bytes" means.
        assert_eq!(&bytes[4..8], b"ftyp");
        assert_eq!(&bytes[8..12], b"heic");
        assert_eq!(
            crate::format::detect_format(&bytes).unwrap(),
            OutputFormat::Heic
        );
    }

    #[test]
    fn a_jpeg_named_heic_is_still_a_jpeg() {
        let jpeg = crate::encode_fixed(
            &image::DynamicImage::ImageRgb8(image::RgbImage::new(8, 8)),
            OutputFormat::Jpeg,
            80,
        )
        .unwrap();
        // The filename is the user's, not ours: only the bytes decide.
        assert_eq!(
            crate::format::detect_format(&jpeg).unwrap(),
            OutputFormat::Jpeg
        );
    }

    #[test]
    fn a_heic_is_decoded_to_the_picture_that_went_in() {
        let img = decode(&synthetic::heic(64, 64), &Limits::default()).unwrap();
        assert_eq!(img.dimensions(), (64, 64));
        // Every coding unit is intra DC with no residual, so every sample comes
        // back as one value. The file carries no `colr`, which HEIF defines as
        // BT.709 *limited* range, so the neutral 128 luma lands a couple of codes
        // above 128 once the range is expanded. Pinning the exact triple would
        // pin a rounding detail of the colour pass; pinning the shape pins the
        // decode, which is what this test is for.
        for px in img.to_rgba8().pixels() {
            assert_eq!(px.0[0], px.0[1], "grey sample is not neutral: {px:?}");
            assert_eq!(px.0[1], px.0[2], "grey sample is not neutral: {px:?}");
            assert!(
                (125..=135).contains(&px.0[0]),
                "expected mid-grey, got {px:?}"
            );
            assert_eq!(px.0[3], 255, "an opaque file must decode opaque: {px:?}");
        }
    }

    #[test]
    fn the_whole_pipeline_takes_a_heic_and_hands_back_a_jpeg() {
        let report =
            crate::validate::validate_bytes(&synthetic::heic(64, 64), &Limits::default()).unwrap();
        assert_eq!(report.format, OutputFormat::Heic);
        assert_eq!((report.width, report.height), (64, 64));
        assert!(report.suspicious.is_none());

        let out = crate::process(
            &synthetic::heic(64, 64),
            &crate::Pipeline::new(),
            OutputFormat::Jpeg,
            85,
            &Limits::default(),
        )
        .unwrap();
        assert_eq!(
            crate::format::detect_format(&out).unwrap(),
            OutputFormat::Jpeg
        );
        assert_eq!(
            image::load_from_memory(&out).unwrap().dimensions(),
            (64, 64)
        );
    }

    #[test]
    fn the_batch_path_accepts_a_heic_too() {
        let job = crate::worker::Job {
            id: "1".to_owned(),
            name: "photo.heic".to_owned(),
            bytes: synthetic::heic(64, 64),
        };
        let settings = crate::worker::Settings::default();
        let processed =
            crate::worker::process_one(&job, &crate::Pipeline::new(), &settings).unwrap();
        assert_eq!(processed.outcome.output_name, "photo.jpg");
        assert_eq!(
            (processed.outcome.width, processed.outcome.height),
            (64, 64)
        );
    }

    #[test]
    fn a_truncated_heic_is_an_error_rather_than_a_panic() {
        let full = synthetic::heic(64, 64);
        // Cut points chosen to land in different structures: the `ftyp` header,
        // the `meta` property boxes, and the coded slice at the end.
        for cut in [20, full.len() / 3, full.len() / 2, full.len() - 8] {
            assert!(
                decode(&full[..cut], &Limits::default()).is_err(),
                "a HEIC cut at {cut} of {} bytes decoded",
                full.len()
            );
        }
        // A cut inside `meta` is caught by the header pass, so a folder scan
        // rejects it without decoding anything.
        for cut in [20, full.len() / 3, full.len() / 2] {
            assert!(
                crate::validate::validate_bytes(&full[..cut], &Limits::default()).is_err(),
                "a HEIC cut at {cut} of {} bytes passed validation",
                full.len()
            );
        }
        // A cut inside the picture data is *not* caught there, and must not be:
        // the header read never promised to look at pixels, and making it hold
        // the whole file to find out would defeat the point of having a cheap
        // pass. The decode is what catches it.
        let no_pixels = &full[..full.len() - 8];
        assert!(
            crate::validate::validate_bytes(no_pixels, &Limits::default()).is_ok(),
            "a header-only read should not need the picture data to be present"
        );
        assert!(decode(no_pixels, &Limits::default()).is_err());
    }

    #[test]
    fn a_heic_declaring_absurd_dimensions_is_rejected_before_allocation() {
        // 60000x60000 is 3.6 gigapixels: 14 GB at four bytes a pixel. The file is
        // a few hundred bytes, so nothing about its size warns anyone — only the
        // header does, which is the whole point of reading one first.
        let bomb = synthetic::declared(60_000, 60_000);
        assert!(
            bomb.len() < 4096,
            "the bomb is {} bytes; it should be a header and nothing else",
            bomb.len()
        );
        let err = decode(&bomb, &Limits::default()).unwrap_err();
        assert!(
            matches!(
                err,
                Error::SuspiciousDimensions { .. } | Error::PixelBudgetExceeded { .. }
            ),
            "expected a limits error, got {err:?}"
        );
        // And the file still reports as a HEIC of the size it claimed, which is
        // what lets the UI say "this photo is 60000x60000, too big to open"
        // rather than "unsupported".
        let report = crate::validate::validate_bytes(&bomb, &Limits::default()).unwrap();
        assert_eq!((report.width, report.height), (60_000, 60_000));
        assert!(report.suspicious.is_some());
        // The tighter profile rejects it too.
        assert!(decode(&bomb, &Limits::mobile()).is_err());
    }

    #[test]
    fn a_report_for_a_heic_says_what_the_container_says() {
        let report =
            crate::validate::validate_bytes(&synthetic::heic(64, 64), &Limits::default()).unwrap();
        assert_eq!(report.format, OutputFormat::Heic);
        assert_eq!(report.megapixels, 64.0 * 64.0 / 1_000_000.0);
        assert!(!report.has_animated, "a HEIC is one still image");
        assert!(!report.has_exif, "the synthetic file carries no Exif item");
    }

    #[test]
    fn an_exif_item_in_the_container_is_reported_as_metadata() {
        // The metadata lives in a separate item, not in an APP1 segment, so this
        // is the only way the report can know. An iPhone photo without this
        // would be stripped of its GPS by the *next* tool in the chain while the
        // app told the user it had none.
        let file = synthetic::heic_with_exif(64, 64);
        assert!(header(&file).unwrap().has_exif);
        let report = crate::validate::validate_bytes(&file, &Limits::default()).unwrap();
        assert!(
            report.has_exif,
            "the report must not claim a HEIC has no metadata"
        );
        // And it decodes regardless: reading the container must not be a
        // precondition for reading the picture.
        assert_eq!(
            decode(&file, &Limits::default()).unwrap().dimensions(),
            (64, 64)
        );
    }

    #[test]
    fn an_avif_in_a_heif_container_is_named_rather_than_called_broken() {
        let mut bytes = synthetic::heic(64, 64);
        bytes[8..12].copy_from_slice(b"avif");
        assert_eq!(
            crate::format::detect_format(&bytes).unwrap(),
            OutputFormat::Avif
        );
        let text = decode(&bytes, &Limits::default()).unwrap_err().to_string();
        assert!(
            text.contains("HEVC"),
            "a user needs to be told what we can read, not that the file is broken: {text}"
        );
    }

    #[test]
    fn a_generic_heif_brand_is_read_as_heif_and_not_as_heic() {
        let mut bytes = synthetic::heic(64, 64);
        bytes[8..12].copy_from_slice(b"mif1");
        assert_eq!(
            crate::format::detect_format(&bytes).unwrap(),
            OutputFormat::Heif
        );
    }

    #[test]
    fn writing_heic_is_refused_with_advice_rather_than_a_pretense() {
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(4, 4));
        for format in [OutputFormat::Heic, OutputFormat::Heif] {
            let text = crate::format::encode(&img, format, 85)
                .unwrap_err()
                .to_string();
            assert!(
                text.contains("cannot write") && text.contains("JPEG"),
                "a read-only refusal should say what to do instead: {text}"
            );
        }
    }

    #[test]
    fn capabilities_report_heic_decode_honestly() {
        assert_eq!(
            crate::capabilities().heic_decode,
            cfg!(feature = "heic"),
            "the capability list must not promise a codec the build lacks"
        );
    }

    #[test]
    fn ftyp_needs_a_plausible_box_header() {
        assert_eq!(ftyp(&[]), None);
        assert_eq!(ftyp(b"ftyp"), None);
        assert_eq!(ftyp(&[0, 0, 0, 16, b'f', b't', b'y', b'p']), None);
        // A box that declares less than the header it just spent.
        assert_eq!(ftyp(&[0, 0, 0, 8, b'f', b't', b'y', b'p']), None);
        // A PNG is a box file too, and is not ours.
        let png = crate::encode_fixed(
            &image::DynamicImage::ImageRgb8(image::RgbImage::new(2, 2)),
            OutputFormat::Png,
            80,
        )
        .unwrap();
        assert_eq!(ftyp(&png), None);
        // MPEG-4 video is a box file with a brand list, and is not ours either:
        // the compatible list is scanned, so an unrecognised major brand must not
        // fall through to a recognisable one by accident.
        let mut mp4 = vec![0, 0, 0, 24, b'f', b't', b'y', b'p', b'm', b'p', b'4', b'2'];
        mp4.extend_from_slice(&[0, 0, 0, 0]); // minor_version
        mp4.extend_from_slice(b"isommp42"); // compatible brands
        assert_eq!(ftyp(&mp4), Some((*b"mp42", &b"isommp42"[..])));
        assert_eq!(detect(&mp4), None);
    }

    /// Builds HEIF containers byte by byte.
    ///
    /// Everything here is a box header, a field width and an offset: no fixture
    /// file, no encoder, nothing to expire and no third-party photograph whose
    /// licence anyone would have to reason about. The picture data comes from
    /// `heic_rs::hevc::synth`, which writes the parameter sets and the CABAC
    /// slice itself, so the pixels that go in are known exactly.
    mod synthetic {
        use heic_rs::hevc::synth;

        /// An `Exif` item's payload: the six-byte identifier a reader searches
        /// for, then a little-endian TIFF header.
        const EXIF: &[u8] = b"Exif\0\0II*\0\x08\x00\x00\x00\x00\x00";

        fn be16(v: u16) -> [u8; 2] {
            v.to_be_bytes()
        }

        fn be32(v: u32) -> [u8; 4] {
            v.to_be_bytes()
        }

        /// A box: size, type, payload.
        fn bxml(ty: &[u8; 4], payload: &[u8]) -> Vec<u8> {
            let mut out = Vec::with_capacity(payload.len() + 8);
            out.extend_from_slice(&be32((payload.len() + 8) as u32));
            out.extend_from_slice(ty);
            out.extend_from_slice(payload);
            out
        }

        /// A full box: the above, then a version and three flag bytes.
        fn full(ty: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
            let mut body = Vec::with_capacity(payload.len() + 4);
            body.push(version);
            body.extend_from_slice(&flags.to_be_bytes()[1..]);
            body.extend_from_slice(payload);
            bxml(ty, &body)
        }

        fn ftyp(major: &[u8; 4]) -> Vec<u8> {
            let mut p = Vec::new();
            p.extend_from_slice(major); // major_brand
            p.extend_from_slice(&be32(0)); // minor_version
            p.extend_from_slice(b"mif1"); // compatible brands
            p.extend_from_slice(major);
            bxml(b"ftyp", &p)
        }

        /// The HEVC decoder configuration record, one array per parameter set.
        fn hvcc(sets: &[Vec<u8>]) -> Vec<u8> {
            let mut p = vec![1u8, 3]; // configurationVersion, general_profile_idc
            p.extend_from_slice(&[0x60, 0, 0, 0]); // profile compatibility flags
            p.extend_from_slice(&[0; 6]); // constraint indicator flags
            p.push(30); // general_level_idc
            p.extend_from_slice(&be16(0xf000)); // reserved, min_spatial_segmentation_idc
            p.push(0xfc); // reserved, parallelismType
            p.push(0xfd); // reserved, chromaFormat 4:2:0
            p.push(0xf8); // reserved, bitDepthLumaMinus8
            p.push(0xf8); // reserved, bitDepthChromaMinus8
            p.extend_from_slice(&be16(0)); // avgFrameRate: meaningless for a still
            p.push(0b0000_1100 | 3); // constantFrameRate, layers, lengthSizeMinusOne
            p.push(sets.len() as u8); // numOfArrays
            for nal in sets {
                // The NAL unit type is the top six bits of the two-byte header.
                p.push(0b1000_0000 | (nal[0] >> 1)); // array_completeness, NAL_unit_type
                p.extend_from_slice(&be16(1)); // numNalus
                p.extend_from_slice(&be16(nal.len() as u16));
                p.extend_from_slice(nal);
            }
            p
        }

        /// Length-prefixed item data, which is how HEIF stores NAL units: a bare
        /// sequence with a big-endian length, never Annex-B start codes.
        fn length_prefixed(nal: &[u8]) -> Vec<u8> {
            let mut out = Vec::with_capacity(nal.len() + 4);
            out.extend_from_slice(&be32(nal.len() as u32));
            out.extend_from_slice(nal);
            out
        }

        /// One catalogue entry and the bytes it points at.
        struct Item {
            id: u16,
            kind: [u8; 4],
            bytes: Vec<u8>,
        }

        impl Item {
            fn infe(&self) -> Vec<u8> {
                let mut p = Vec::new();
                p.extend_from_slice(&be16(self.id)); // item_ID
                p.extend_from_slice(&be16(0)); // item_protection_index
                p.extend_from_slice(&self.kind); // item_type
                p.push(0); // empty name
                full(b"infe", 2, 0, &p)
            }
        }

        /// `ispe`: the coded size of an item. `probe` reads the geometry from
        /// here, so this is the box that makes a bomb cheap to reject.
        fn size_property(width: u32, height: u32) -> Vec<u8> {
            let mut p = Vec::new();
            p.extend_from_slice(&be32(width));
            p.extend_from_slice(&be32(height));
            full(b"ispe", 0, 0, &p)
        }

        /// What to build. Every field changes bytes at a fixed width, which is
        /// what lets the payload offset be arithmetic rather than a search.
        struct Spec {
            width: u32,
            height: u32,
            /// A real coded picture, or only a declaration of one.
            picture: bool,
            /// Attach an `Exif` item that describes the picture.
            exif: bool,
        }

        /// A decodable HEIC file of the given size, with nothing else in it.
        pub fn heic(width: u32, height: u32) -> Vec<u8> {
            build(&Spec {
                width,
                height,
                picture: true,
                exif: false,
            })
        }

        /// The same, plus an `Exif` item describing the picture.
        pub fn heic_with_exif(width: u32, height: u32) -> Vec<u8> {
            build(&Spec {
                width,
                height,
                picture: true,
                exif: true,
            })
        }

        /// A file that declares a size and carries no coded picture.
        ///
        /// This is the shape of a decompression bomb: a few hundred bytes that
        /// claim to be 60000x60000. Decoding it would cost 14 GB, so the tests
        /// that use it prove the size is refused from the header alone.
        pub fn declared(width: u32, height: u32) -> Vec<u8> {
            build(&Spec {
                width,
                height,
                picture: false,
                exif: false,
            })
        }

        fn build(spec: &Spec) -> Vec<u8> {
            // A picture is a length-prefixed slice plus the three parameter sets
            // in the `hvcC`. Without one, the record carries the SPS alone: the
            // geometry is still stated twice, in the SPS and in `ispe`.
            let (sets, data) = if spec.picture {
                let (sets, slice) = synth::picture_fmt(spec.width, spec.height, 1, 8);
                (sets, length_prefixed(&slice))
            } else {
                let sps = synth::to_nal(33, &synth::build_sps(spec.width, spec.height, 1, 8));
                (vec![sps], Vec::new())
            };

            let mut items = vec![Item {
                id: 1,
                kind: *b"hvc1",
                bytes: data,
            }];
            if spec.exif {
                items.push(Item {
                    id: 2,
                    kind: *b"Exif",
                    bytes: EXIF.to_vec(),
                });
            }

            let ftyp = ftyp(b"heic");
            // `meta` is fixed width for a given item list, so it can be measured
            // with placeholder offsets and then rebuilt with the real ones.
            let meta_len = meta(&items, &sets, spec, 0).len();
            let payload_start = (ftyp.len() + meta_len + 8) as u32; // +8 for the mdat header
            let meta = meta(&items, &sets, spec, payload_start);
            debug_assert_eq!(
                meta.len(),
                meta_len,
                "meta must not resize with its offsets"
            );

            let mut out = ftyp;
            out.extend_from_slice(&meta);
            let mut mdat = Vec::new();
            for item in &items {
                mdat.extend_from_slice(&item.bytes);
            }
            out.extend_from_slice(&bxml(b"mdat", &mdat));
            out
        }

        fn meta(items: &[Item], sets: &[Vec<u8>], spec: &Spec, payload_start: u32) -> Vec<u8> {
            let mut p = Vec::new();

            let mut hdlr = Vec::new();
            hdlr.extend_from_slice(&be32(0)); // pre_defined
            hdlr.extend_from_slice(b"pict"); // handler_type: a still-image file
            hdlr.extend_from_slice(&[0u8; 12]); // reserved
            hdlr.push(0); // empty name
            p.extend_from_slice(&full(b"hdlr", 0, 0, &hdlr));

            // The picture is item 1, and `pitm` is what makes it the one shown.
            p.extend_from_slice(&full(b"pitm", 0, 0, &be16(items[0].id)));

            let mut iinf = be16(items.len() as u16).to_vec(); // entry_count
            for item in items {
                iinf.extend_from_slice(&item.infe());
            }
            p.extend_from_slice(&full(b"iinf", 0, 0, &iinf));

            if spec.exif {
                // `cdsc` is how metadata describes a picture: from the Exif item
                // to the picture. Without it the Exif item is in the file and not
                // attached to anything, which is not the same thing at all.
                let mut cdsc = Vec::new();
                cdsc.extend_from_slice(&be16(items[1].id)); // from_item_ID
                cdsc.extend_from_slice(&be16(1)); // reference_count
                cdsc.extend_from_slice(&be16(items[0].id)); // to_item_ID
                p.extend_from_slice(&full(b"iref", 0, 0, &bxml(b"cdsc", &cdsc)));
            }

            let mut iloc = vec![0x44, 0x00]; // offset_size 4, length_size 4; no base offset
            iloc.extend_from_slice(&be16(items.len() as u16)); // item_count
            let mut at = payload_start;
            for item in items {
                iloc.extend_from_slice(&be16(item.id));
                iloc.extend_from_slice(&be16(0)); // data_reference_index
                iloc.extend_from_slice(&be16(1)); // extent_count
                iloc.extend_from_slice(&be32(at)); // absolute file offset
                iloc.extend_from_slice(&be32(item.bytes.len() as u32));
                at += item.bytes.len() as u32;
            }
            p.extend_from_slice(&full(b"iloc", 0, 0, &iloc));

            // Property 1 is hvcC, 2 is ispe, 3 is pixi, and `ipma` is what maps
            // those numbers onto an item. hvcC is essential: a reader that cannot
            // act on it must refuse the picture rather than guess at it.
            let mut ipco = Vec::new();
            ipco.extend_from_slice(&bxml(b"hvcC", &hvcc(sets)));
            ipco.extend_from_slice(&size_property(spec.width, spec.height));
            ipco.extend_from_slice(&full(b"pixi", 0, 0, &[3, 8, 8, 8]));
            let ipco = bxml(b"ipco", &ipco);

            let mut ipma = Vec::new();
            ipma.extend_from_slice(&be32(1)); // entry_count
            ipma.extend_from_slice(&be16(items[0].id));
            ipma.push(3); // association_count
            ipma.push(0x81); // hvcC, essential
            ipma.push(0x02); // ispe
            ipma.push(0x03); // pixi
            let ipma = full(b"ipma", 0, 0, &ipma);

            let mut iprp = Vec::new();
            iprp.extend_from_slice(&ipco);
            iprp.extend_from_slice(&ipma);
            p.extend_from_slice(&bxml(b"iprp", &iprp));

            full(b"meta", 0, 0, &p)
        }
    }
}
