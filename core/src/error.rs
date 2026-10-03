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
