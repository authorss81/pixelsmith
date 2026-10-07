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
    #[error("this image cannot be written as WebP because {0}")]
    Webp(&'static str),

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

impl From<image::ImageError> for Error {
    /// Codec errors are indistinguishable from a caller's perspective: both
    /// mean "these bytes are not a picture we can handle".
    fn from(value: image::ImageError) -> Self {
        Self::Decode(value)
    }
}
