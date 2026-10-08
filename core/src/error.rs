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
        "this resize would need {needed} bytes of working memory and this device          allows {budget}; choose a smaller output size"
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
