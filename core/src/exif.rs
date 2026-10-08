//! EXIF handling.
//!
//! The threat here is mundane and common: phone photos carry GPS coordinates,
//! serial numbers and owner names. Any tool that re-encodes and keeps metadata
//! silently publishes that. So the default is *strip*, and stripping is done by
//! re-encoding rather than by clearing tags, because a file with cleared tags
//! can still reveal the original through maker notes or embedded thumbnails.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExifEntry {
    pub tag: String,
    pub value: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExifInfo {
    pub entries: Vec<ExifEntry>,
    pub has_gps: bool,
    pub orientation: Option<u32>,
    pub camera: Option<String>,
    pub captured_at: Option<String>,
    /// Tags a user most likely wants gone, surfaced so the UI can say "this file
    /// carries GPS coordinates" before anything is stripped.
    pub sensitive_tags: Vec<String>,
    /// The colour profile the file carries, which is metadata in the sense a user
    /// means but not in the sense this module does: dropping it changes what the
    /// pixels mean, so it is reported rather than quietly stripped, and the
    /// pipeline converts before it strips. See [`crate::colour`].
    pub colour: crate::colour::ColourProfile,
}

pub fn has_exif(bytes: &[u8]) -> bool {
    exif::Reader::new()
        .read_from_container(&mut std::io::Cursor::new(bytes))
        .is_ok()
}

/// Read every EXIF tag we can render as text, plus the colour profile.
///
/// The two are read together because both are "what this file says about itself"
/// and the UI asks for them together — but they are handled differently on the
/// way out. EXIF is stripped by re-encoding; the ICC profile is converted and
/// then dropped, because dropping it without converting it is a colour change
/// rather than a hygiene step. See [`crate::colour`].
pub fn read(bytes: &[u8]) -> Result<ExifInfo> {
    let reader = exif::Reader::new();
    let mut info = ExifInfo {
        colour: colour_profile(bytes),
        ..ExifInfo::default()
    };

    // No EXIF is the common case for PNG and WebP. It is not a failure.
    let Ok(exif) = reader.read_from_container(&mut std::io::Cursor::new(bytes)) else {
        return Ok(info);
    };

    for field in exif.fields() {
        let tag = field.tag.to_string();
        // `display_value` is human-facing and quotes ASCII, renders the
        // orientation tag as prose, and can produce an empty string for values
        // it does not model. We only want values we can both display and act on.
        let value = unquote(field.display_value().to_string().trim());
        if value.is_empty() {
            continue;
        }

        match field.tag {
            // Read the number out of the raw value: `display_value` renders
            // this tag as prose like "row 0 at right and column 0 at top", which
            // is useless for a machine that has to act on it.
            exif::Tag::Orientation => info.orientation = numeric(&field.value),
            exif::Tag::GPSLatitude
            | exif::Tag::GPSLongitude
            | exif::Tag::GPSLatitudeRef
            | exif::Tag::GPSLongitudeRef
            | exif::Tag::GPSAltitude => info.has_gps = true,
            exif::Tag::DateTimeOriginal | exif::Tag::DateTime => {
                info.captured_at = Some(value.clone())
            }
            exif::Tag::Make | exif::Tag::Model => {
                info.camera = Some(match info.camera.take() {
                    None => value.clone(),
                    Some(existing) if existing.contains(&value) => existing,
                    Some(existing) => format!("{existing} {value}"),
                });
            }
            _ => {}
        }

        if is_sensitive(&tag) {
            info.sensitive_tags.push(tag.clone());
        }

        info.entries.push(ExifEntry { tag, value });
    }

    Ok(info)
}

/// Read a file's colour profile, for callers that have no `OutputFormat` yet.
///
/// Format detection is asked for here rather than passed in, because this is the
/// entry point the FFI's `px_exif` uses and it has nothing but bytes. A file
/// whose container cannot be identified is untagged, which is the same answer a
/// file with no profile gets — and neither is a failure.
fn colour_profile(bytes: &[u8]) -> crate::colour::ColourProfile {
    match crate::format::detect_format(bytes) {
        Ok(format) => crate::colour::ColourProfile::read(bytes, format),
        Err(_) => crate::colour::ColourProfile::untagged(),
    }
}

/// Strip the quotes `display_value` wraps around ASCII values.
fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}

/// Extract a single unsigned integer from an EXIF value.
fn numeric(value: &exif::Value) -> Option<u32> {
    match value {
        exif::Value::Short(v) => v.first().map(|n| u32::from(*n)),
        exif::Value::Long(v) => v.first().copied(),
        _ => None,
    }
}

/// Tags that identify a person or a place.
fn is_sensitive(tag: &str) -> bool {
    let upper = tag.to_ascii_uppercase();
    upper.starts_with("GPS")
        || upper.contains("SERIAL")
        || upper.contains("OWNER")
        || upper.contains("ARTIST")
        || upper.contains("COPYRIGHT")
        || upper.contains("MAKERNOTE")
        || upper.contains("LENS")
        || upper.contains("BODY")
        || upper.contains("THUMBNAIL")
}

/// Strip metadata by rebuilding the pixel buffer from raw samples.
///
/// This is the only approach we trust. There is no "keep EXIF but drop GPS"
/// path, because the interaction between maker notes and embedded thumbnails is
/// not something a photo tool should be guessing at. If someone genuinely needs
/// metadata preserved, the right answer is that they should not be publishing a
/// photo with GPS in it.
pub fn strip(img: &image::DynamicImage) -> Result<image::DynamicImage> {
    let rgba = img.to_rgba8();
    let rebuilt = image::RgbaImage::from_raw(rgba.width(), rgba.height(), rgba.into_raw())
        .ok_or(Error::ZeroDimension)?;
    Ok(image::DynamicImage::ImageRgba8(rebuilt))
}

/// Re-attach a filtered subset of EXIF to encoded JPEG bytes.
///
/// Only ever called on explicit opt-in, and sensitive tags are dropped unless
/// the caller passes `keep_gps`, which the UI does not offer.
pub fn write_back(original: &[u8], encoded: &mut Vec<u8>, keep_gps: bool) -> Result<()> {
    let src = exif::Reader::new().read_from_container(&mut std::io::Cursor::new(original))?;

    let kept: Vec<exif::Field> = src
        .fields()
        .filter(|f| keep_gps || !is_sensitive(&f.tag.to_string()))
        .map(|f| exif::Field {
            tag: f.tag,
            ifd_num: f.ifd_num,
            value: f.value.clone(),
        })
        .collect();

    if kept.is_empty() {
        return Ok(());
    }

    // The writer borrows its fields, so they have to outlive it.
    let fields: Vec<&exif::Field> = kept.iter().collect();
    let mut writer = exif::experimental::Writer::new();
    for field in &fields {
        writer.push_field(field);
    }
    let mut buf = std::io::Cursor::new(Vec::new());
    writer.write(&mut buf, true)?;

    crate::format::append_exif(encoded, &buf.into_inner())
}

/// Build an EXIF block from tag/value pairs. Used by tests and by the FFI layer
/// when the UI wants to attach metadata itself.
pub fn build_block(fields: &[exif::Field]) -> Result<Vec<u8>> {
    let owned: Vec<&exif::Field> = fields.iter().collect();
    let mut writer = exif::experimental::Writer::new();
    for field in &owned {
        writer.push_field(field);
    }
    let mut buf = std::io::Cursor::new(Vec::new());
    writer.write(&mut buf, true)?;
    Ok(buf.into_inner())
}

/// Convenience constructor for a single tag.
pub fn field(tag: exif::Tag, ifd: exif::In, value: exif::Value) -> exif::Field {
    exif::Field {
        tag,
        ifd_num: ifd,
        value,
    }
}

/// The GPS IFD.
///
/// `kamadak-exif`'s writer requires every IFD in the chain to be non-empty and
/// contiguously numbered, so a GPS tag cannot be placed in IFD 2 without also
/// populating the thumbnail IFD. We keep every written field in IFD 0 and rely
/// on the tag names, which is what readers key off anyway.
pub const GPS_IFD: exif::In = exif::In::PRIMARY;

/// A 7-bit ASCII value. `kamadak-exif` does not implement `From<&str>`, and
/// every caller here wants exactly this shape.
pub fn ascii(value: &str) -> exif::Value {
    exif::Value::Ascii(vec![value.as_bytes().to_vec()])
}

/// A single 32-bit unsigned integer value.
pub fn long(value: u32) -> exif::Value {
    exif::Value::Long(vec![value])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{EncodingOptions, OutputFormat};

    fn noisy(w: u32, h: u32) -> image::DynamicImage {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 77])
        }))
    }

    #[test]
    fn png_has_no_exif() {
        let png = crate::encode_fixed(
            &noisy(16, 16),
            OutputFormat::Png,
            EncodingOptions::default().with_quality(80),
        )
        .unwrap();
        assert!(!has_exif(&png));
        assert!(read(&png).unwrap().entries.is_empty());
    }

    #[test]
    fn reading_garbage_is_not_fatal() {
        let info = read(b"this is not a jpeg").unwrap();
        assert!(info.entries.is_empty());
        assert!(!info.has_gps);
        assert_eq!(info.colour, crate::colour::ColourProfile::untagged());
    }

    /// Metadata and colour profile are read together and reported together,
    /// because the UI asks about a file and a file has both — but they are
    /// stripped differently, which is why both answers are on the same struct.
    #[test]
    fn a_colour_profile_is_reported_next_to_the_metadata() {
        let mut jpeg = crate::colour::fixtures::encode_with(
            &noisy(16, 16),
            OutputFormat::Jpeg,
            Some(&crate::colour::fixtures::p3_profile()),
        );
        let block =
            build_block(&[field(exif::Tag::Make, exif::In::PRIMARY, ascii("Nikon"))]).unwrap();
        crate::format::append_exif(&mut jpeg, &block).unwrap();

        let info = read(&jpeg).unwrap();
        assert_eq!(info.camera.as_deref(), Some("Nikon"));
        assert!(info.colour.icc_present);
        assert_eq!(info.colour.source, crate::colour::ColourSpace::DisplayP3);
        assert_eq!(
            info.colour.icc_bytes,
            crate::colour::fixtures::p3_profile().len()
        );
    }

    /// A file with no EXIF still has a colour profile, and reporting one without
    /// the other is how a user ends up told their P3 photo has no colour
    /// information in it.
    #[test]
    fn a_profile_survives_on_a_file_with_no_exif_at_all() {
        let png = crate::colour::fixtures::encode_with(
            &noisy(16, 16),
            OutputFormat::Png,
            Some(&crate::colour::fixtures::p3_profile()),
        );
        let info = read(&png).unwrap();
        assert!(info.entries.is_empty());
        assert!(!has_exif(&png), "the fixture is not meant to carry EXIF");
        assert!(info.colour.icc_present);
        assert_eq!(info.colour.source, crate::colour::ColourSpace::DisplayP3);
    }

    #[test]
    fn strip_produces_a_container_with_no_metadata() {
        let mut jpeg = crate::encode_fixed(
            &noisy(32, 32),
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(85),
        )
        .unwrap();
        let block = build_block(&[field(
            exif::Tag::CameraOwnerName,
            exif::In::PRIMARY,
            ascii("Jane Doe"),
        )])
        .unwrap();
        crate::format::append_exif(&mut jpeg, &block).unwrap();
        assert!(has_exif(&jpeg), "test fixture should carry EXIF");

        let stripped = strip(&noisy(32, 32)).unwrap();
        let out = crate::encode_fixed(
            &stripped,
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(85),
        )
        .unwrap();
        assert!(read(&out).unwrap().entries.is_empty());
    }

    #[test]
    fn strip_preserves_pixels_exactly() {
        let src = noisy(32, 32);
        assert_eq!(
            src.to_rgb8().into_raw(),
            strip(&src).unwrap().to_rgb8().into_raw()
        );
    }

    #[test]
    fn sensitive_detection_covers_location_and_ownership() {
        for tag in [
            "GPSLatitude",
            "GPSLongitude",
            "OwnerName",
            "BodySerialNumber",
            "LensModel",
            "MakerNote",
        ] {
            assert!(is_sensitive(tag), "{tag} should be sensitive");
        }
        for tag in ["FNumber", "ISO", "ExposureTime", "ImageWidth"] {
            assert!(!is_sensitive(tag), "{tag} should not be sensitive");
        }
    }

    #[test]
    fn gps_and_orientation_are_detected_when_written() {
        let mut jpeg = crate::encode_fixed(
            &noisy(16, 16),
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(90),
        )
        .unwrap();
        let block = build_block(&[
            field(exif::Tag::Orientation, exif::In::PRIMARY, long(6)),
            field(exif::Tag::GPSLatitude, GPS_IFD, long(51)),
            field(exif::Tag::Make, exif::In::PRIMARY, ascii("Canon")),
        ])
        .unwrap();
        crate::format::append_exif(&mut jpeg, &block).unwrap();

        let info = read(&jpeg).unwrap();
        assert_eq!(info.orientation, Some(6), "entries: {:?}", info.entries);
        assert!(info.has_gps, "entries: {:?}", info.entries);
        assert!(info.sensitive_tags.iter().any(|t| t.contains("GPS")));
        assert_eq!(info.camera.as_deref(), Some("Canon"));
    }

    #[test]
    fn write_back_never_reintroduces_gps() {
        let mut original = crate::encode_fixed(
            &noisy(16, 16),
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(90),
        )
        .unwrap();
        let block = build_block(&[
            field(exif::Tag::GPSLatitude, GPS_IFD, long(51)),
            field(exif::Tag::Make, exif::In::PRIMARY, ascii("Nikon")),
        ])
        .unwrap();
        crate::format::append_exif(&mut original, &block).unwrap();

        let mut clean = crate::encode_fixed(
            &noisy(16, 16),
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(90),
        )
        .unwrap();
        write_back(&original, &mut clean, false).unwrap();

        let info = read(&clean).unwrap();
        assert!(!info.has_gps, "GPS came back: {:?}", info.entries);
        assert_eq!(
            info.camera.as_deref(),
            Some("Nikon"),
            "non-sensitive tags should survive"
        );
    }

    #[test]
    fn write_back_is_a_no_op_when_nothing_survives() {
        let mut original = crate::encode_fixed(
            &noisy(16, 16),
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(90),
        )
        .unwrap();
        let block = build_block(&[field(exif::Tag::GPSLatitude, GPS_IFD, long(51))]).unwrap();
        crate::format::append_exif(&mut original, &block).unwrap();

        let mut clean = crate::encode_fixed(
            &noisy(16, 16),
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(90),
        )
        .unwrap();
        let before = clean.len();
        write_back(&original, &mut clean, false).unwrap();
        assert_eq!(clean.len(), before, "nothing should have been appended");
    }
}
