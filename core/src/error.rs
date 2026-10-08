use std::io;

/// Every failure mode in the engine is one of these. Nothing unwinds out of a
/// public entry point, so a malformed file cannot take down the host process.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported or unrecognised image format")]
    UnknownFormat,

    /// A format we recognise on input but cannot yet write. Distinct from
    /// `UnknownFormat` because the user's file is fine — we are the limitation.
    #[error("{0}")]
    UnsupportedFormat(&'static str),

    /// A setting the chosen format cannot act on was asked for: a byte ceiling,
    /// or a quality value, on a format that stores the picture exactly. The
    /// payload is a sentence written for a person, not a diagnostic — see
    /// [`crate::format::OutputFormat::quality_note`].
    #[error("{0}")]
    NoQualitySetting(&'static str),

    #[error("image is larger than the {limit} byte input limit ({actual} bytes)")]
    InputTooLarge { limit: usize, actual: usize },

    #[error("image exceeds the {limit} megapixel budget ({actual:.2} MP)")]
    PixelBudgetExceeded { limit: u64, actual: f64 },

    #[error("dimensions look like a decompression bomb: {w}x{h} ({mp:.1} MP)")]
    SuspiciousDimensions { w: u32, h: u32, mp: f64 },

    #[error("dimensions must be greater than zero")]
    ZeroDimension,

    /// A crop rectangle that runs past the edge of the picture it was asked
    /// about.
    ///
    /// Its own variant rather than a reuse of `SuspiciousDimensions` for three
    /// reasons, and the third is why the message carries the crop rather than a
    /// sum:
    ///
    /// * `SuspiciousDimensions` says "decompression bomb", which is a claim about
    ///   a *file* and the opposite of what happened: the file is fine and the
    ///   request does not fit it.
    /// * The pair of numbers a user needs is the picture's own size and the
    ///   rectangle they asked for, and this build's answer is to move the crop.
    /// * `{"x": 4294967295, "width": 1}` arrives from Dart and the sum of those
    ///   two is not a `u32`, so there is no `w`/`h` to put in the other
    ///   variant's fields. Adding the two numbers overflows in every
    ///   overflow-checked build — `dev` and `test`, so every CI run — which is a
    ///   panic on a value the caller chose.
    #[error(
        "this picture is {src_width}x{src_height} and the crop asked for a \
         {requested_width}x{requested_height} rectangle at ({requested_x}, {requested_y}), \
         which runs past its edge; crop inside the picture or leave the crop unset"
    )]
    CropOutOfBounds {
        requested_x: u32,
        requested_y: u32,
        requested_width: u32,
        requested_height: u32,
        src_width: u32,
        src_height: u32,
    },

    #[error("decode failed: {0}")]
    Decode(#[source] image::ImageError),

    #[error("encode failed: {0}")]
    Encode(#[source] image::ImageError),

    /// The JPEG encoder refused these pixels. Separate from `Encode` because the
    /// encoder is not `image`, and because the failure is almost always one of two
    /// explainable things: dimensions past the format's 16-bit limit, or a buffer
    /// whose length disagrees with the dimensions.
    #[error("the JPEG encoder rejected these pixels: {0}")]
    Jpeg(#[source] jpeg_encoder::EncodingError),

    /// The WebP encoder refused these pixels, carrying a reason written for a
    /// person rather than libwebp's numeric code. Separate from `Encode` because
    /// the encoder is not `image`, and because `webp`'s errors arrive as bare C
    /// enum constants that say nothing about what to do next.
    ///
    /// The reason is a clause, not a sentence: the period is here rather than in
    /// each of libwebp's twelve codes, so `Error::Webp` renders as one sentence
    /// however it was constructed.
    #[error("this image cannot be written as WebP because {0}.")]
    Webp(&'static str),

    /// The file's colour space cannot be converted into the output's. The message
    /// is a sentence written for a person and names the space, the alternative
    /// and the opt-out — see [`crate::colour::unsupported_note`].
    ///
    /// Its own variant rather than a string on `UnsupportedFormat` because the
    /// user's file is perfectly fine and perfectly readable: it is the *export*
    /// this build cannot carry out, which is a different thing to say and a
    /// different thing for the user to do about.
    #[error("{0}")]
    UnsupportedColourSpace(String),

    /// Two options in one request that contradict each other. Rare in practice
    /// and impossible from a UI that is in sync, which is exactly why it is
    /// refused in a sentence rather than resolved by picking a winner.
    #[error("{0}")]
    ConflictingOptions(&'static str),

    /// A request to carry the file's ICC profile into the output, which this
    /// build cannot carry out — either because the format has nowhere to put it or
    /// because the file has no profile to carry.
    ///
    /// Its own variant because both cases need a sentence that says what to do
    /// next, and neither of them is a decode failure or a format the engine cannot
    /// write: the export would have worked perfectly well with the option turned
    /// off.
    #[error("{0}")]
    ColourProfile(&'static str),

    /// An animation was offered and the output format holds one picture, so
    /// exporting it would have dropped every frame but the first. The message
    /// is written for a person and names the frame count, what was not written
    /// and the format that would keep them — see
    /// [`crate::animation::AnimationPolicy`].
    ///
    /// Its own variant because the file is fine, the frames are readable, and
    /// the request is the thing this build will not carry out. Reporting it as a
    /// decode failure would blame the user's file for the engine's decision.
    #[error("{note}")]
    AnimationRefused {
        /// How many frames the file had, so a batch report can say so in a
        /// field rather than only in a string.
        frames: u32,
        /// The sentence, built by [`crate::animation::refusal_note`] because the
        /// useful wording is a decision about the product rather than about
        /// this enum.
        note: String,
    },

    #[error("output could not be written: {0}")]
    Io(#[source] io::Error),

    /// A HEIF container this build could not read. Distinct from
    /// `UnknownFormat` because the file is a real image and we know exactly what
    /// it is — we are the limitation, and saying so is the whole difference
    /// between "this file is broken" and "we cannot read this one yet".
    #[error("{0}")]
    Heic(#[from] crate::heic::HeicError),

    #[error("metadata could not be parsed: {0}")]
    Metadata(#[from] exif::Error),

    #[error("archive could not be assembled: {0}")]
    Archive(#[from] zip::result::ZipError),

    /// A streamed decode's own working set did not fit the profile's ceiling.
    ///
    /// Separate from [`Error::PixelBudgetExceeded`] because the file is fine and
    /// the *request* is what this build cannot carry out: the pixels fit the
    /// header check, and the output does not fit the memory the profile allows.
    #[error(
        "this resize would need {needed} bytes of working memory and this device allows \
         {budget}; choose a smaller output size"
    )]
    StreamingBudgetExceeded { budget: u64, needed: u64 },

    /// The row reader ran out of rows before the geometry said it should have.
    ///
    /// A truncated or lying stream rather than an arithmetic problem, and the
    /// reason it is an error rather than a black band is that the alternative is
    /// writing a half-picture to the user's disk.
    #[error("this photo's image data ended after {read} of {expected} rows")]
    TruncatedStream { read: u32, expected: u32 },

    #[error("operation was cancelled")]
    Cancelled,

    /// Another file in the same batch already exported this picture, so this one
    /// was not written.
    ///
    /// An error rather than a value because there are no bytes to hand back and
    /// the *batch* is where the decision makes sense: `worker::process_batch`
    /// turns it into a `SkipReason::Duplicate` naming the file that held the key,
    /// and the single-image path never produces it because nothing is deduped
    /// when there is only one file.
    #[error("the same picture is already exported as {of}")]
    Duplicate { of: String },

    /// The request asked for a bigger picture than this file has and the
    /// pipeline refused to invent the detail, so there was nothing to write.
    ///
    /// Carries both sizes because "would upscale" on its own is a half-sentence:
    /// a user needs to see that they asked for 1920 and the photo is 800.
    #[error(
        "this picture is {actual_width}x{actual_height} and the request asked for {requested_width}x{requested_height}; enlarging adds no detail"
    )]
    WouldUpscale {
        requested_width: u32,
        requested_height: u32,
        actual_width: u32,
        actual_height: u32,
    },

    /// A sandboxed decode failed. The message is already written for a person:
    /// it names what happened to the file and what the limit was, because the
    /// alternative is surfacing a child's exit status or a Rust panic string to
    /// someone trying to make a photo smaller.
    ///
    /// One variant rather than four because the *distinction the caller needs* is
    /// carried in the message, and inventing a variant per exit code would give
    /// the UI five near-identical error branches to render. The exit-code
    /// contract itself is documented in [`crate::sandbox`].
    #[error("{0}")]
    Sandbox(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

/// `png`'s decode failures become engine failures rather than a second error
/// type, so the streaming path returns the same [`Error`] every other decode does.
///
/// Behind the feature because `png` is behind it. `DecodingError` carries the
/// codec's own wording ("Not enough image data was provided to be able to decode
/// the image"), which is what a user can act on; `IoError` is the `image`
/// variant that takes an arbitrary cause.
#[cfg(feature = "streaming")]
impl From<png::DecodingError> for Error {
    fn from(value: png::DecodingError) -> Self {
        Error::Decode(image::ImageError::IoError(std::io::Error::other(value)))
    }
}

impl From<image::ImageError> for Error {
    /// Codec errors are indistinguishable from a caller's perspective: both
    /// mean "these bytes are not a picture we can handle".
    fn from(value: image::ImageError) -> Self {
        Self::Decode(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    /// The sentence `ColourOptions::validate` refuses with, extracted rather than
    /// retyped: the payload is a `&'static str` living in `colour.rs`, and a copy
    /// of it here would go stale the moment that one is reworded.
    fn conflicting_options_note() -> &'static str {
        let options = crate::colour::ColourOptions {
            keep_source_pixels: true,
            embed_profile: true,
            ..Default::default()
        };
        match options.validate() {
            Err(Error::ConflictingOptions(note)) => note,
            other => panic!("the two contradictory options must be refused, got {other:?}"),
        }
    }

    /// The variant's own name, by an exhaustive match with no wildcard.
    ///
    /// This function is the coverage mechanism, and it is here rather than in a
    /// script because a compiler is the only thing in this repository that cannot
    /// be talked out of its answer. Adding a variant to [`Error`] makes this fail
    /// to compile, which points at `cases()` below; a script scanning for a
    /// message would have to be run, and `docs/AUDIT.md` finding 19 is what
    /// happens to a module whose tests are only run when somebody thinks to.
    fn variant_name(error: &Error) -> &'static str {
        match error {
            Error::UnknownFormat => "UnknownFormat",
            Error::UnsupportedFormat(_) => "UnsupportedFormat",
            Error::NoQualitySetting(_) => "NoQualitySetting",
            Error::InputTooLarge { .. } => "InputTooLarge",
            Error::PixelBudgetExceeded { .. } => "PixelBudgetExceeded",
            Error::SuspiciousDimensions { .. } => "SuspiciousDimensions",
            Error::ZeroDimension => "ZeroDimension",
            Error::CropOutOfBounds { .. } => "CropOutOfBounds",
            Error::Decode(_) => "Decode",
            Error::Encode(_) => "Encode",
            Error::Jpeg(_) => "Jpeg",
            Error::Webp(_) => "Webp",
            Error::UnsupportedColourSpace(_) => "UnsupportedColourSpace",
            Error::ConflictingOptions(_) => "ConflictingOptions",
            Error::ColourProfile(_) => "ColourProfile",
            Error::AnimationRefused { .. } => "AnimationRefused",
            Error::Io(_) => "Io",
            Error::Heic(_) => "Heic",
            Error::Metadata(_) => "Metadata",
            Error::Archive(_) => "Archive",
            Error::StreamingBudgetExceeded { .. } => "StreamingBudgetExceeded",
            Error::TruncatedStream { .. } => "TruncatedStream",
            Error::Cancelled => "Cancelled",
            Error::Duplicate { .. } => "Duplicate",
            Error::WouldUpscale { .. } => "WouldUpscale",
            Error::Sandbox(_) => "Sandbox",
        }
    }

    /// One case per variant: the error, and the fragments of its message that a
    /// user needs in order to do something about it.
    ///
    /// The payloads are the real ones wherever a function writes them — the PNG
    /// quality note, the WebP ICC note, the HEIF sentences — rather than strings
    /// invented here. A fixture invented here would assert that the variant
    /// renders *its* payload, which is true of every `&'static str` variant and
    /// is precisely the half of the message nobody has read.
    ///
    /// Two of these rows are about the numbers rather than the prose: the audit
    /// found a message with ten literal spaces in the middle of a sentence
    /// (`StreamingBudgetExceeded`), and a message that renders `3600` where the
    /// user needs `3600.0 MP` is the same defect wearing a different hat.
    fn cases() -> [(&'static str, Error, &'static [&'static str]); 26] {
        use crate::format::OutputFormat;
        [
            (
                "UnknownFormat",
                Error::UnknownFormat,
                &["unrecognised", "image format"],
            ),
            (
                "UnsupportedFormat",
                // The HEIC sentence, verbatim from the arm that refuses it.
                Error::UnsupportedFormat(
                    "this build reads HEIC/HEIF but cannot write it: choose JPEG, PNG or WebP \
                     as the output format and the photo will be converted",
                ),
                &["cannot write", "choose JPEG", "PNG or WebP"],
            ),
            (
                "NoQualitySetting",
                // The real PNG note. Hard rule 9's test is the second fragment: a
                // byte ceiling that cannot be honoured has to name a format that
                // could honour it, or the user is left with nothing to pick.
                Error::NoQualitySetting(OutputFormat::Png.quality_note()),
                &["no quality setting", "Ask for JPEG or WebP"],
            ),
            (
                "InputTooLarge",
                Error::InputTooLarge {
                    limit: 134_217_728,
                    actual: 402_653_184,
                },
                &["134217728", "402653184", "input limit"],
            ),
            (
                "PixelBudgetExceeded",
                Error::PixelBudgetExceeded {
                    limit: 40,
                    actual: 225.0,
                },
                &["40", "225.00", "megapixel budget"],
            ),
            (
                "SuspiciousDimensions",
                Error::SuspiciousDimensions {
                    w: 60_000,
                    h: 60_000,
                    mp: 3600.0,
                },
                &["60000x60000", "3600.0", "MP", "decompression bomb"],
            ),
            (
                "ZeroDimension",
                Error::ZeroDimension,
                &["greater than zero"],
            ),
            (
                "CropOutOfBounds",
                Error::CropOutOfBounds {
                    requested_x: 30,
                    requested_y: 40,
                    requested_width: 400,
                    requested_height: 300,
                    src_width: 100,
                    src_height: 80,
                },
                &["100x80", "400x300", "(30, 40)", "crop inside the picture"],
            ),
            (
                "Decode",
                Error::Decode(image::ImageError::IoError(io::Error::other(
                    "corrupt Huffman table",
                ))),
                // The prefix names which half of the work failed; the source names
                // why. Asserting only the prefix would pass on the one message
                // hard rule 9 forbids.
                &["decode failed", "corrupt Huffman table"],
            ),
            (
                "Encode",
                Error::Encode(image::ImageError::Parameter(
                    image::error::ParameterError::from_kind(
                        image::error::ParameterErrorKind::DimensionMismatch,
                    ),
                )),
                &["encode failed", "dimension"],
            ),
            (
                "Jpeg",
                Error::Jpeg(jpeg_encoder::EncodingError::BadImageData {
                    length: 10,
                    required: 20,
                }),
                &["JPEG encoder rejected", "at least 20"],
            ),
            (
                "Webp",
                Error::Webp("it is 20000 pixels wide and libwebp stores at most 16383 a side"),
                &["WebP", "20000", "16383", "because"],
            ),
            (
                "UnsupportedColourSpace",
                Error::UnsupportedColourSpace(
                    "CMYK cannot be converted into sRGB by this build; convert the file to \
                     RGB first, or ask to keep the original colour values"
                        .to_string(),
                ),
                &["CMYK", "sRGB", "or"],
            ),
            (
                "ConflictingOptions",
                Error::ConflictingOptions(conflicting_options_note()),
                &["Pick one"],
            ),
            (
                "ColourProfile",
                // The real WebP ICC refusal, because "embedding is refused for
                // WebP" is only useful if the sentence says what to pick.
                Error::ColourProfile(OutputFormat::WebP.icc_note()),
                &["WebP", "JPEG or PNG", "turn the option off"],
            ),
            (
                "AnimationRefused",
                Error::AnimationRefused {
                    frames: 12,
                    note: crate::animation::refusal_note(12),
                },
                &["12", "frame", "Choose GIF"],
            ),
            (
                "Io",
                Error::Io(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "the destination folder is not writable",
                )),
                &["could not be written", "not writable"],
            ),
            (
                "Heic",
                Error::Heic(crate::heic::HeicError::NotBuilt),
                &["no HEIC/HEIF decoder", "JPEG or PNG"],
            ),
            (
                "Metadata",
                Error::Metadata(exif::Error::InvalidFormat(
                    "exif: the TIFF header is not II or MM",
                )),
                &["metadata could not be parsed", "TIFF header"],
            ),
            (
                "Archive",
                Error::Archive(zip::result::ZipError::InvalidArchive(
                    "the central directory is not where the file ends".into(),
                )),
                &["archive could not be assembled", "central directory"],
            ),
            (
                "StreamingBudgetExceeded",
                Error::StreamingBudgetExceeded {
                    budget: 160_000_000,
                    needed: 402_653_184,
                },
                // One space between every word. This row exists because the string
                // carried ten of them, mid-sentence, in text a user reads; the
                // general property below is what stops the next one.
                &[
                    "working memory",
                    "402653184",
                    "160000000",
                    "smaller output size",
                ],
            ),
            (
                "TruncatedStream",
                Error::TruncatedStream {
                    read: 3,
                    expected: 100,
                },
                &["3 of 100 rows"],
            ),
            ("Cancelled", Error::Cancelled, &["cancelled"]),
            (
                "Duplicate",
                Error::Duplicate {
                    of: "beach-sunset.jpg".to_string(),
                },
                &["already exported", "beach-sunset.jpg"],
            ),
            (
                "WouldUpscale",
                Error::WouldUpscale {
                    requested_width: 4000,
                    requested_height: 3000,
                    actual_width: 64,
                    actual_height: 64,
                },
                &["4000x3000", "64x64", "adds no detail"],
            ),
            (
                "Sandbox",
                Error::Sandbox(
                    "this photo needed more memory than the 512 MB ceiling this device allows; \
                     export it one at a time"
                        .to_string(),
                ),
                &["512 MB", "one at a time"],
            ),
        ]
    }

    /// The claim `AGENTS.md` makes about tests, applied to the one thing hard
    /// rule 9 says the product is: for every variant, the rendered message must
    /// contain the specific thing a user needs.
    #[test]
    fn every_variant_names_what_a_user_needs() {
        for (expected_name, error, fragments) in cases() {
            let name = variant_name(&error);
            assert_eq!(
                name, expected_name,
                "cases() names this variant {expected_name} and the enum calls it {name}"
            );
            let rendered = error.to_string();
            for fragment in fragments {
                assert!(
                    rendered.contains(fragment),
                    "{name} must tell the user {fragment:?}. It renders: {rendered}"
                );
            }
        }
    }

    /// Two properties every message has, because both are cheap and both have
    /// caught a real defect in this file.
    ///
    /// The second one caught ten literal spaces in the middle of
    /// `StreamingBudgetExceeded`'s sentence. `assert!(!s.is_empty())` would not
    /// have: the message was long, and long is exactly what a broken one is.
    #[test]
    fn no_message_is_empty_or_riddled_with_whitespace() {
        for (_, error, _) in cases() {
            let name = variant_name(&error);
            let rendered = error.to_string();
            assert!(
                !rendered.trim().is_empty(),
                "{name} renders an empty message, which is the one thing hard rule 9 forbids"
            );
            assert!(
                !rendered.contains("   "),
                "{name} renders a run of three or more spaces, which a user reads as a \
                 sentence that stopped halfway through: {rendered:?}"
            );
            let last = rendered
                .chars()
                .next_back()
                .expect("just checked it is not empty");
            assert!(
                !matches!(last, ' ' | '\t' | ',' | ';' | ':'),
                "{name} renders a message ending in {last:?}, so it is a half-sentence: \
                 {rendered:?}"
            );
        }
    }

    /// The standard `format::tests::every_refusal_says_what_to_choose_instead`
    /// holds the format refusals to, one level up: every variant that *knows* an
    /// alternative has to name it.
    ///
    /// Listed by variant rather than derived, because "knows an alternative" is
    /// not a property the type system carries — it is a decision about the
    /// product, and a decision has to be written down to be reviewed. A variant
    /// that is not in this list is asserting that there is nothing useful to
    /// offer, which for `UnknownFormat` (nothing at all) is true and for anything
    /// else is worth a sentence in the row above.
    #[test]
    fn every_refusal_that_knows_an_alternative_names_it() {
        let alternatives: [(&str, Error, &[&str]); 10] = [
            (
                "UnsupportedFormat",
                Error::UnsupportedFormat(
                    "this build reads HEIC/HEIF but cannot write it: choose JPEG, PNG or WebP \
                     as the output format and the photo will be converted",
                ),
                &["choose JPEG"],
            ),
            (
                "NoQualitySetting",
                Error::NoQualitySetting(crate::format::OutputFormat::Png.quality_note()),
                &["Ask for JPEG or WebP"],
            ),
            (
                "CropOutOfBounds",
                Error::CropOutOfBounds {
                    requested_x: 0,
                    requested_y: 0,
                    requested_width: 8_192,
                    requested_height: 8_192,
                    src_width: 64,
                    src_height: 64,
                },
                &["crop inside the picture"],
            ),
            (
                "UnsupportedColourSpace",
                Error::UnsupportedColourSpace(
                    "this file's colour space is CMYK, which this build does not convert. \
                     Ask to keep the original colour values, or export as JPEG"
                        .to_string(),
                ),
                &["Ask to keep the original colour values"],
            ),
            (
                "ConflictingOptions",
                Error::ConflictingOptions(conflicting_options_note()),
                &["Pick one"],
            ),
            (
                "ColourProfile",
                Error::ColourProfile(crate::format::OutputFormat::WebP.icc_note()),
                &["Ask for JPEG or PNG"],
            ),
            (
                "AnimationRefused",
                Error::AnimationRefused {
                    frames: 7,
                    note: crate::animation::refusal_note(7),
                },
                &["Choose GIF"],
            ),
            (
                "Heic",
                Error::Heic(crate::heic::HeicError::NotBuilt),
                &["share the photo as JPEG or PNG"],
            ),
            (
                "StreamingBudgetExceeded",
                Error::StreamingBudgetExceeded {
                    budget: 1024,
                    needed: 4096,
                },
                &["choose a smaller output size"],
            ),
            (
                "Duplicate",
                Error::Duplicate {
                    of: "one.jpg".to_string(),
                },
                &["already exported as one.jpg"],
            ),
        ];
        for (name, error, needles) in alternatives {
            assert_eq!(
                variant_name(&error),
                name,
                "this list names {name} and the enum calls it something else"
            );
            let rendered = error.to_string();
            for needle in needles {
                assert!(
                    rendered.contains(needle),
                    "{name} knows an alternative and does not name it: {rendered}"
                );
            }
        }
    }

    /// The three variants that wrap a third-party error, asserted on the seam.
    ///
    /// Hard rule 9 says a failure must say what happened *and* what to do about
    /// it, and in these three the second half is the source's wording. That is a
    /// deliberate division of labour rather than a gap — `image`'s "corrupt
    /// Huffman table" and libwebp's numeric codes are the only description of the
    /// failure anybody has — so the property to assert is that the wrapper does
    /// not swallow it. A wrapper that rendered only its own prefix would fail
    /// here, and that is exactly the "Error: decode failed" the rule names.
    #[test]
    fn a_wrapped_codec_error_keeps_the_codecs_own_words() {
        let cases = [
            (
                Error::Decode(image::ImageError::IoError(io::Error::other(
                    "zlib: bad CRC",
                ))),
                "zlib: bad CRC",
            ),
            (
                Error::Encode(image::ImageError::IoError(io::Error::other(
                    "the output buffer is 3 bytes short",
                ))),
                "the output buffer is 3 bytes short",
            ),
            (
                Error::Metadata(exif::Error::NotFound("the orientation tag is absent")),
                "the orientation tag is absent",
            ),
        ];
        for (error, source_words) in cases {
            let name = variant_name(&error);
            let rendered = error.to_string();
            assert!(
                rendered.contains(source_words),
                "{name} dropped the source's explanation ({source_words:?}): {rendered}"
            );
            assert!(
                error.source().is_some(),
                "{name} wraps a cause but does not report one, so nothing can walk to it"
            );
        }
    }

    /// `thiserror`'s `#[from]` conversions are the reason one enum is the whole
    /// failure surface, and they are the one part of this type with no keyword an
    /// exhaustive match would catch.
    #[test]
    fn the_from_conversions_land_on_the_variant_that_names_them() {
        let io: Error = io::Error::other("no room left").into();
        assert!(matches!(io, Error::Io(_)), "io::Error must become Io");

        let image_error: Error =
            image::ImageError::IoError(io::Error::other("truncated stream")).into();
        assert!(
            matches!(image_error, Error::Decode(_)),
            "a bare image::ImageError must become Decode"
        );

        let heic: Error = crate::heic::HeicError::UnsupportedCoding.into();
        assert!(matches!(heic, Error::Heic(_)), "HeicError must become Heic");

        let meta: Error = exif::Error::TooBig("the maker note is 4 MB").into();
        assert!(
            matches!(meta, Error::Metadata(_)),
            "exif::Error must become Metadata"
        );

        let archive: Error = zip::result::ZipError::FileNotFound.into();
        assert!(
            matches!(archive, Error::Archive(_)),
            "ZipError must become Archive"
        );
    }
}
