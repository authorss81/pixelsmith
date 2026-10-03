use crate::error::{Error, Result};
use image::ImageEncoder;
use serde::{Deserialize, Serialize};

/// Formats we can write. Reading is broader than writing, which is normal:
/// every decoder can read what its encoder emits, not the reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Jpeg,
    Png,
    WebP,
    Gif,
    Tiff,
    Bmp,
    Ico,
    /// AV1-coded HEIF: the modern web format. Written through `image`'s AVIF
    /// encoder (rav1e via ravif) behind the `avif` feature, which is on by
    /// default; see `Cargo.toml` for why. Read-only when that feature is off.
    Avif,
    /// HEVC-coded HEIF, the ordinary iPhone photograph. Read-only: recognised,
    /// bounded, decoded behind the `heic` feature, and never written.
    Heic,
    /// The generic HEIF brand (`mif1`), for a still image coded some other way.
    /// Read-only, and refused by name rather than by "damaged file".
    Heif,
}

impl OutputFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::WebP => "webp",
            Self::Gif => "gif",
            Self::Tiff => "tiff",
            Self::Bmp => "bmp",
            Self::Ico => "ico",
            Self::Avif => "avif",
            Self::Heic => "heic",
            Self::Heif => "heif",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::WebP => "image/webp",
            Self::Gif => "image/gif",
            Self::Tiff => "image/tiff",
            Self::Bmp => "image/bmp",
            Self::Ico => "image/x-icon",
            Self::Avif => "image/avif",
            Self::Heic => "image/heic",
            Self::Heif => "image/heif",
        }
    }

    /// True when the quality knob is ignored, so the UI can hide it.
    pub fn is_lossless(self) -> bool {
        match self {
            Self::Jpeg => false,
            Self::WebP => !lossy_webp_enabled(),
            // GIF is palette-quantised, and the `image` encoder emits a single
            // full-colour frame, so quality has no meaningful effect.
            Self::Gif | Self::Png | Self::Bmp | Self::Tiff | Self::Ico => true,
            // AV1 is a lossy codec, so with the encoder compiled in, quality and
            // a byte ceiling are both real. Without it there is no encoder to
            // hand either to, so the honest answer for *this build* is that
            // quality cannot affect the output — which is what `is_lossless`
            // means to the UI. The flag and the encoder cannot disagree,
            // because they are the same `cfg!`.
            Self::Avif => !avif_encode_enabled(),
            // Read-only: there is no encoder to hand a quality to.
            Self::Heic | Self::Heif => true,
        }
    }

    /// True when this build can write the format at all.
    ///
    /// The distinction matters in two directions. A read-only format must not be
    /// offered as an export target, because the user would discover it at save
    /// time; and it must still be reported when *reading*, because "we can open
    /// your iPhone photo and convert it to JPEG" is the product's headline
    /// capability. Hard rule 10 is about not promising the first, not about
    /// hiding the second.
    ///
    /// `Avif` is the one format whose answer is a build-time decision rather
    /// than a property of the format: without the `avif` feature it is as
    /// read-only as HEIC, because there is genuinely no encoder behind it.
    pub fn is_read_only(self) -> bool {
        match self {
            Self::Avif => !avif_encode_enabled(),
            Self::Heic | Self::Heif => true,
            Self::Jpeg
            | Self::Png
            | Self::WebP
            | Self::Gif
            | Self::Tiff
            | Self::Bmp
            | Self::Ico => false,
        }
    }

    /// True when the encoder writes scan-by-scan rather than in one pass.
    ///
    /// A progressive JPEG shows a coarse image immediately and refines it, so a
    /// slow network shows something rather than a blank rectangle. It is not free
    /// and it is not a few per cent: measured on a 1600×1200 fixture at q85, a
    /// progressive 4:2:0 JPEG is 96,938 bytes against 59,200 for the baseline
    /// file — about 64% more — because the four scans it writes carry the
    /// coefficients twice. That is why it is off by default and belongs on the
    /// Web category of a share sheet rather than on every export.
    pub fn supports_progressive(self) -> bool {
        matches!(self, Self::Jpeg)
    }

    /// True when the encoder honours a chroma resolution.
    ///
    /// Only JPEG does in this build. PNG, TIFF and BMP store colour per pixel;
    /// WebP output is lossless unless `webp-lossy` is on (and libwebp's lossy
    /// mode is not given a sampling factor here); AVIF is written by rav1e,
    /// which always uses full-resolution chroma, so asking for 4:2:0 there would
    /// be a promise the encoder cannot keep.
    pub fn supports_chroma_subsampling(self) -> bool {
        matches!(self, Self::Jpeg)
    }

    pub fn supports_alpha(self) -> bool {
        matches!(self, Self::Png | Self::WebP | Self::Gif | Self::Ico)
    }

    pub fn supports_animation(self) -> bool {
        matches!(self, Self::Gif)
    }

    pub fn accepts_multiple_frames(self) -> bool {
        matches!(self, Self::Gif)
    }

    /// Only meaningful for formats whose encoder honours a quality value.
    pub fn supports_quality(self) -> bool {
        !self.is_lossless()
    }

    /// Whether a byte ceiling can be enforced for this format at all.
    pub fn supports_byte_target(self) -> bool {
        self.supports_quality()
    }

    /// What to tell a person who asked for a quality setting, or a size
    /// ceiling, that this format has no use for.
    ///
    /// Every arm is written out rather than composed from a template because the
    /// useful sentence is not the same for each: PNG needs a different next step
    /// from a read-only format, and neither is helped by "unsupported quality".
    /// Returning a sentence for *every* format, including the ones that do have a
    /// quality setting, keeps the match exhaustive — a new format cannot be added
    /// without deciding what it says here.
    pub fn quality_note(self) -> &'static str {
        match self {
            Self::Jpeg => {
                "JPEG has a quality setting, so the slider does something here. \
                 60 is a good default for a photo, 90 and above for a screenshot."
            }
            Self::Png => {
                "PNG is stored exactly as it is drawn, so it has no quality setting: the \
                 file will be as large as the picture needs, whatever the slider says. \
                 Ask for JPEG or WebP if you need it smaller."
            }
            Self::WebP if lossy_webp_enabled() => {
                "WebP has a quality setting in this build, so the slider does something here."
            }
            Self::WebP => {
                "WebP output in this build is lossless, so it has no quality setting: the \
                 file will be as large as the picture needs. Ask for JPEG if you need a \
                 size dial, or rebuild the engine with the webp-lossy feature."
            }
            Self::Gif => {
                "GIF is palette-quantised, so it has no quality setting and a size ceiling \
                 cannot be applied to it. It is the right choice for an animation, not for a \
                 photograph. Ask for JPEG or WebP if you need the file to fit under a limit."
            }
            Self::Tiff => {
                "TIFF is stored without lossy compression, so it has no quality setting and a \
                 size ceiling cannot be applied to it. Use it for editing or printing; ask \
                 for JPEG or WebP if you need the file to fit under a limit."
            }
            Self::Bmp => {
                "BMP is an uncompressed bitmap with no quality setting, so a size ceiling \
                 cannot be applied to it. It is here because some tools only read it, not \
                 because it saves space. Ask for JPEG or WebP if you need a smaller file."
            }
            Self::Ico => {
                "ICO is uncompressed and has no quality setting, so a size ceiling cannot be \
                 applied to it. Ask for JPEG or WebP if you need a size limit; keep PNG for \
                 anything that is not a Windows icon."
            }
            Self::Avif if avif_encode_enabled() => {
                "AVIF has a quality setting, and it is the smallest of the formats here \
                 at a given quality — at the cost of an encode that takes seconds, not \
                 milliseconds."
            }
            Self::Avif => {
                "This build cannot write AVIF at all, so it has no quality setting to \
                 offer. Ask for JPEG or WebP, or rebuild the engine with the avif feature."
            }
            Self::Heic | Self::Heif => {
                "HEIC and HEIF are read-only here: this build opens your iPhone photo and \
                 converts it, but never writes one. Ask for JPEG, PNG or WebP as the \
                 output format."
            }
        }
    }

    /// Maps a file extension to a format. Only trusted after [`detect_format`]
    /// confirms the magic bytes agree.
    pub fn from_extension(ext: &str) -> Option<Self> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" | "jpe" => Self::Jpeg,
            "png" => Self::Png,
            "webp" => Self::WebP,
            "gif" => Self::Gif,
            "tif" | "tiff" => Self::Tiff,
            "bmp" => Self::Bmp,
            "ico" => Self::Ico,
            "avif" => Self::Avif,
            "heic" => Self::Heic,
            // `heif` is the container, not a codec: a `.heif` file is a HEIF whose
            // still image may be HEVC, AV1 or something else. It gets its own
            // variant so the UI never tells a user their file is a .heic when
            // they can see it is not.
            "heif" => Self::Heif,
            _ => return None,
        })
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::Jpeg,
            Self::WebP,
            Self::Png,
            Self::Gif,
            Self::Tiff,
            Self::Bmp,
            Self::Ico,
            Self::Avif,
            Self::Heic,
            Self::Heif,
        ]
    }
}

/// Lossy WebP needs libwebp. When the `webp-lossy` feature is off we fall back
/// to the pure-Rust lossless encoder, which still beats PNG for photographs but
/// cannot be quality-controlled, so the capability flags above reflect it.
#[cfg(feature = "webp-lossy")]
const fn lossy_webp_enabled() -> bool {
    true
}
#[cfg(not(feature = "webp-lossy"))]
const fn lossy_webp_enabled() -> bool {
    false
}

/// AVIF encode is in the default build, but it is a feature so a target that
/// cannot afford the AV1 encoder can drop it — and then every flag in this
/// module has to agree that AVIF is read-only. One `cfg!` feeds all of them, so
/// "the capability list says we can write AVIF" and "we can write AVIF" cannot
/// come to disagree.
#[cfg(feature = "avif")]
const fn avif_encode_enabled() -> bool {
    true
}
#[cfg(not(feature = "avif"))]
const fn avif_encode_enabled() -> bool {
    false
}

/// Chroma resolution: how finely the two colour-difference channels are stored
/// relative to luma.
///
/// This is a bigger lever on file size than most people expect — a 4:2:0 JPEG
/// spends a quarter of the chroma samples of a 4:4:4 one, and on the measured
/// 1600×1200 fixture at q85 that is 59,200 bytes against 88,975, a third off the
/// file. What it costs is *colour detail*, not sharpness: luma stays
/// full-resolution either way, so edges stay crisp while the colour along them
/// smears. Text on a coloured background is where it shows: coloured glyph
/// fringes appear against the background, because the glyph and the background
/// are different colours at a one-pixel boundary.
///
/// The default is [`Luma420`](Self::Luma420) because this is a photo resizer and
/// a photograph's chroma detail is mostly below the visible threshold. Anything
/// whose content *is* colour — text, logos, UI screenshots, hard red-on-blue
/// boundaries — wants [`Luma444`](Self::Luma444), and the argument for that is in
/// `docs/ARCHITECTURE.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChromaSubsampling {
    /// 4:4:4 — chroma at full resolution. Biggest, and the only safe choice for
    /// content with saturated colour edges.
    Luma444,
    /// 4:2:2 — chroma at half horizontal resolution, full vertical.
    Luma422,
    /// 4:2:0 — chroma at half resolution on both axes. The photo default.
    #[default]
    Luma420,
}

impl ChromaSubsampling {
    /// Every level, most to least colour detail. The order the UI shows them in.
    pub const ALL: [Self; 3] = [Self::Luma444, Self::Luma422, Self::Luma420];

    /// The `4:4:4` spelling a person would use, for labels and error messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::Luma444 => "4:4:4",
            Self::Luma422 => "4:2:2",
            Self::Luma420 => "4:2:0",
        }
    }

    /// One line explaining the trade-off, for a tooltip or a report field.
    ///
    /// The point is that this is never applied silently: a user who exports a
    /// screenshot at 4:2:0 and sees colour fringes on the text needs to be able
    /// to find out that the encoder halved their chroma and where the knob is.
    pub fn trade_off(self) -> &'static str {
        match self {
            Self::Luma444 => {
                "full colour resolution: the largest file, and the only one that keeps \
                 colour detail on hard edges such as text"
            }
            Self::Luma422 => {
                "half the horizontal colour resolution: smaller than 4:4:4, and text on a \
                 coloured background still fringes on vertical strokes"
            }
            Self::Luma420 => {
                "quarter of the colour samples: smallest of the three, and colour fringes \
                 around text and hard colour edges — use 4:4:4 for screenshots and logos"
            }
        }
    }

    /// How many chroma samples per pixel this level stores, as a numerator over
    /// the 4:4:4 baseline. Used by the UI to explain *why* the file got smaller.
    pub fn chroma_sample_ratio(self) -> (u32, u32) {
        match self {
            Self::Luma444 => (4, 4),
            Self::Luma422 => (2, 4),
            Self::Luma420 => (2, 2),
        }
    }

    /// The encoder-level sampling factor.
    ///
    /// 4:2:2 in JPEG terms is one chroma sample per two luma samples
    /// horizontally, which is `F_2_1` here: the crate names factors by
    /// *luma* samples per chroma sample, so the two spellings read backwards
    /// relative to each other and this is the one place that translation lives.
    fn sampling_factor(self) -> jpeg_encoder::SamplingFactor {
        use jpeg_encoder::SamplingFactor as Sf;
        match self {
            Self::Luma444 => Sf::F_1_1,
            Self::Luma422 => Sf::F_2_1,
            Self::Luma420 => Sf::F_2_2,
        }
    }
}

impl std::fmt::Display for ChromaSubsampling {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Everything the encoder is asked for, in one value.
///
/// This started as a bare `quality: u8` argument and grew one boolean and one
/// enum; the fourth option would have been a fifth parameter on a function that
/// already takes an image and a format. A struct also gives the options a home
/// in the JSON contract and in the Dart model, so adding an encoder knob is a
/// field rather than a signature change across three layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncodingOptions {
    /// 1..=100. Clamped on the way in, so a caller cannot panic an encoder by
    /// asking for quality 0 — several of them assert on their range.
    pub quality: u8,
    /// Scan-by-scan JPEG. Ignored by formats that have no such concept; see
    /// [`OutputFormat::supports_progressive`].
    pub progressive: bool,
    /// Chroma resolution. Ignored by formats that store colour per pixel; see
    /// [`OutputFormat::supports_chroma_subsampling`].
    pub chroma_subsampling: ChromaSubsampling,
}

impl Default for EncodingOptions {
    /// 85 is the quality every existing preset assumed, so this default changes
    /// no preset's output.
    fn default() -> Self {
        Self {
            quality: 85,
            progressive: false,
            chroma_subsampling: ChromaSubsampling::Luma420,
        }
    }
}

impl EncodingOptions {
    /// The same options at a different quality. Used by the byte-target search,
    /// which varies quality and nothing else.
    pub fn with_quality(mut self, quality: u8) -> Self {
        self.quality = quality;
        self
    }

    /// The same options at a different chroma resolution.
    pub fn with_chroma(mut self, chroma_subsampling: ChromaSubsampling) -> Self {
        self.chroma_subsampling = chroma_subsampling;
        self
    }
}

/// Identify the format from magic bytes only.
///
/// This is the security-relevant check: the UI is handed a filename from an
/// untrusted source, and trusting the extension would let `payload.png` through
/// as a JPEG. `image` does its own sniffing during decode, so this is about
/// giving the *caller* an answer it can act on.
pub fn detect_format(input: &[u8]) -> Result<OutputFormat> {
    use image::ImageFormat as F;
    Ok(match image::guess_format(input).ok() {
        Some(F::Jpeg) => OutputFormat::Jpeg,
        Some(F::Png) => OutputFormat::Png,
        Some(F::WebP) => OutputFormat::WebP,
        Some(F::Gif) => OutputFormat::Gif,
        Some(F::Tiff) => OutputFormat::Tiff,
        Some(F::Bmp) => OutputFormat::Bmp,
        Some(F::Ico) => OutputFormat::Ico,
        Some(F::Avif) => OutputFormat::Avif,
        // `image` has no HEIF sniffer, so a HEIC arrives here as "not an image I
        // know" unless something reads the container's own brand bytes. The
        // fallback runs for every format `image` did not claim, including AVIF
        // inside a HEIF container, which `image` does not claim either — so the
        // answer is read from the box rather than from a codec's opinion.
        _ => return crate::heic::detect(input).ok_or(Error::UnknownFormat),
    })
}

/// Encode a still image.
///
/// `options` carries quality, progressive scan and chroma resolution. Options a
/// format has no use for are ignored rather than rejected, because a caller
/// setting one global quality slider should not get an error for choosing PNG
/// output; [`OutputFormat::supports_progressive`] and
/// [`OutputFormat::supports_chroma_subsampling`] are what the UI reads to decide
/// whether to show the control in the first place.
pub fn encode(
    img: &image::DynamicImage,
    format: OutputFormat,
    options: EncodingOptions,
) -> Result<Vec<u8>> {
    let quality = options.quality.clamp(1, 100);
    let mut out: Vec<u8> = Vec::with_capacity(estimate_capacity(img));
    let mut cursor = std::io::Cursor::new(&mut out);

    match format {
        OutputFormat::Jpeg => encode_jpeg(img, &mut cursor, options, quality)?,
        OutputFormat::Png => {
            // Best compression + adaptive filtering. Larger encode time, smaller
            // file, and no readability cost on our side.
            image::codecs::png::PngEncoder::new_with_quality(
                &mut cursor,
                image::codecs::png::CompressionType::Best,
                image::codecs::png::FilterType::Adaptive,
            )
            .write_image(
                img.as_bytes(),
                img.width(),
                img.height(),
                img.color().into(),
            )
            .map_err(Error::Encode)?;
        }
        OutputFormat::WebP => encode_webp(img, &mut cursor, quality)?,
        OutputFormat::Gif => {
            image::codecs::gif::GifEncoder::new(&mut cursor)
                .encode_frame(image::Frame::new(img.to_rgba8()))
                .map_err(Error::Encode)?;
        }
        OutputFormat::Tiff => {
            image::codecs::tiff::TiffEncoder::new(&mut cursor)
                .write_image(
                    img.as_bytes(),
                    img.width(),
                    img.height(),
                    img.color().into(),
                )
                .map_err(Error::Encode)?;
        }
        OutputFormat::Bmp => {
            image::codecs::bmp::BmpEncoder::new(&mut cursor)
                .write_image(
                    img.as_bytes(),
                    img.width(),
                    img.height(),
                    img.color().into(),
                )
                .map_err(Error::Encode)?;
        }
        OutputFormat::Ico => {
            let rgba = to_rgba8(img);
            image::codecs::ico::IcoEncoder::new(&mut cursor)
                .write_image(
                    rgba.as_raw(),
                    rgba.width(),
                    rgba.height(),
                    image::ExtendedColorType::Rgba8,
                )
                .map_err(Error::Encode)?;
        }
        OutputFormat::Avif => encode_avif(img, &mut cursor, options, quality)?,
        OutputFormat::Heic | OutputFormat::Heif => {
            // Read-only, and saying so is the whole point. A user who reaches
            // this has asked for a HEIC because their input was one, and the
            // useful answer is what to pick instead — the same file as a JPEG or
            // PNG, which is what the rest of this engine is for.
            return Err(Error::UnsupportedFormat(
                "this build reads HEIC/HEIF but cannot write it: choose JPEG, PNG or WebP \
                 as the output format and the photo will be converted",
            ));
        }
    }

    Ok(out)
}

/// rav1e's speed scale is 1 (slowest, smallest) to 10 (fastest, largest).
///
/// 8 rather than 4 because 4 buys nothing measurable here and costs the whole
/// wait: on a 1600×1200 fixture at q70 both 4 and 8 produce ~5.8–6.0 KB, while
/// 10 trades 34% more bytes (7,709) for two thirds of the time (1.2 s against
/// 3.2 s). This is a constant rather than a setting because there is no honest
/// way to expose "how many seconds may I take" as a slider — and 3 seconds for
/// one photo is already the price of AVIF at all, which is why
/// `docs/ARCHITECTURE.md` treats it as an opt-in for a chosen export rather than
/// a batch default.
#[cfg(feature = "avif")]
const AVIF_SPEED: u8 = 8;

/// JPEG with the two knobs that matter: chroma resolution and scan order.
///
/// This uses `jpeg-encoder` rather than `image`'s JPEG encoder because
/// `image`'s is baseline-only 4:4:4 with no way to ask for either. That is not a
/// small gap: 4:2:0 is how JPEG gets small, and progressive is what makes a
/// large image appear at all on a slow connection. See docs/JPEG.md.
fn encode_jpeg(
    img: &image::DynamicImage,
    cursor: &mut std::io::Cursor<&mut Vec<u8>>,
    options: EncodingOptions,
    quality: u8,
) -> Result<()> {
    // Flatten alpha onto white. JPEG has no alpha channel, and leaving it unset
    // turns every transparent PNG into a black rectangle.
    let flat = flatten_alpha(img);

    // The encoder takes u16 dimensions. `Limits` caps a side at 30 000, so this
    // cannot overflow in practice — and it is checked rather than cast, because
    // a truncation here would be a wrong-sized file rather than an error.
    let width = u16::try_from(flat.width()).map_err(|_| Error::ZeroDimension)?;
    let height = u16::try_from(flat.height()).map_err(|_| Error::ZeroDimension)?;

    let mut encoder = jpeg_encoder::Encoder::new(&mut *cursor, quality);
    // Always set the sampling factor explicitly: the encoder picks 4:2:0 below
    // quality 90 and 4:4:4 at or above it, which would make our output depend on
    // the quality slider in a way no caller asked for and could not see.
    encoder.set_sampling_factor(options.chroma_subsampling.sampling_factor());
    encoder.set_progressive(options.progressive);
    encoder
        .encode(flat.as_raw(), width, height, jpeg_encoder::ColorType::Rgb)
        .map_err(Error::Jpeg)
}

/// AVIF through `image`'s encoder, which is rav1e via ravif: pure Rust, no C
/// toolchain, no nasm.
///
/// When the `avif` feature is off this build has no AVIF encoder at all, and
/// `is_read_only` says so, so the useful answer is the same one HEIC gets — what
/// to pick instead.
#[cfg(feature = "avif")]
fn encode_avif(
    img: &image::DynamicImage,
    cursor: &mut std::io::Cursor<&mut Vec<u8>>,
    _options: EncodingOptions,
    quality: u8,
) -> Result<()> {
    // One thread per file. The batch path already runs files in parallel across a
    // rayon pool, and a nested pool here would oversubscribe the machine — the
    // same argument as the `heic` feature's "no own rayon pool".
    let encoder =
        image::codecs::avif::AvifEncoder::new_with_speed_quality(&mut *cursor, AVIF_SPEED, quality)
            .with_num_threads(Some(1));

    if img.color().has_alpha() {
        let rgba = to_rgba8(img);
        encoder
            .write_image(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(Error::Encode)?;
    } else {
        let rgb = img.to_rgb8();
        encoder
            .write_image(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(Error::Encode)?;
    }
    Ok(())
}

#[cfg(not(feature = "avif"))]
fn encode_avif(
    _img: &image::DynamicImage,
    _cursor: &mut std::io::Cursor<&mut Vec<u8>>,
    _options: EncodingOptions,
    _quality: u8,
) -> Result<()> {
    Err(Error::UnsupportedFormat(
        "this build was compiled without the AVIF encoder: choose JPEG, PNG or WebP \
         as the output format, or rebuild the engine with --features avif",
    ))
}

fn encode_webp(
    img: &image::DynamicImage,
    cursor: &mut std::io::Cursor<&mut Vec<u8>>,
    quality: u8,
) -> Result<()> {
    let rgba = to_rgba8(img);
    #[cfg(feature = "webp-lossy")]
    {
        use std::io::Write;
        if img.color().has_alpha() {
            let encoded = webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height())
                .encode(f32::from(quality));
            cursor.write_all(&encoded)?;
        } else {
            // libwebp wants tightly packed RGB for opaque input; handing it RGBA
            // it does not need costs 25% more before compression even starts.
            let mut packed = Vec::with_capacity(rgba.width() as usize * rgba.height() as usize * 3);
            for px in rgba.pixels() {
                packed.extend_from_slice(&px.0[..3]);
            }
            let encoded = webp::Encoder::from_rgb(&packed, rgba.width(), rgba.height())
                .encode(f32::from(quality));
            cursor.write_all(&encoded)?;
        }
        Ok(())
    }
    #[cfg(not(feature = "webp-lossy"))]
    {
        let _ = quality;
        image::codecs::webp::WebPEncoder::new_lossless(&mut *cursor).write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            image::ExtendedColorType::Rgba8,
        )?;
        Ok(())
    }
}

/// Compose transparency over white. The only sane choice for JPEG output,
/// because there is no other way to represent alpha.
fn flatten_alpha(img: &image::DynamicImage) -> image::RgbImage {
    match img {
        image::DynamicImage::ImageRgb8(v) => v.clone(),
        other => {
            let rgba = to_rgba8(other);
            let mut rgb = image::RgbImage::new(rgba.width(), rgba.height());
            for (dst, src) in rgb.pixels_mut().zip(rgba.pixels()) {
                let alpha = f32::from(src.0[3]) / 255.0;
                for c in 0..3 {
                    dst.0[c] = (f32::from(src.0[c]) * alpha + 255.0 * (1.0 - alpha)).round() as u8;
                }
            }
            rgb
        }
    }
}

pub(crate) fn to_rgba8(img: &image::DynamicImage) -> image::RgbaImage {
    img.to_rgba8()
}

/// Pre-size the output buffer so a 20 MP JPEG does not realloc a dozen times.
fn estimate_capacity(img: &image::DynamicImage) -> usize {
    let px = img.width() as usize * img.height() as usize;
    // Empirically ~0.6 bytes/px for q75 JPEG; PNG is worst case 4 bytes/px.
    (px / 2).clamp(64 * 1024, 64 * 1024 * 1024)
}

/// Splice an APP1/Exif segment in directly after the JPEG SOI marker.
///
/// The encoders in `image` cannot write EXIF, and round-tripping through a
/// re-decode would re-filter the image. Writing the segment by hand keeps the
/// compressed scan data byte-identical.
///
/// The payload must begin with the six-byte `Exif\0\0` identifier from
/// [Exif 2.3 section 4.7.2]; readers search for that prefix and will not
/// recognise the segment without it.
pub fn append_exif(jpeg: &mut Vec<u8>, exif_tiff: &[u8]) -> Result<()> {
    /// "Exif\0\0", the APP1 identifier every Exif reader looks for.
    const EXIF_ID: [u8; 6] = [0x45, 0x78, 0x69, 0x66, 0x00, 0x00];

    if jpeg.len() < 2 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return Err(Error::UnknownFormat);
    }
    let payload_len = exif_tiff.len() + EXIF_ID.len();
    let len = u16::try_from(payload_len + 2).map_err(|_| Error::UnknownFormat)?;

    let mut segment = Vec::with_capacity(payload_len + 4);
    segment.extend_from_slice(&[0xFF, 0xE1]);
    segment.extend_from_slice(&len.to_be_bytes());
    segment.extend_from_slice(&EXIF_ID);
    segment.extend_from_slice(exif_tiff);

    // Insert immediately after SOI, before any other marker. If the file
    // already carries an APP1 we simply prepend ours; readers take the first.
    jpeg.splice(2..2, segment);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> image::DynamicImage {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
            image::Rgb([(x * 4) as u8, (y * 5) as u8, 128])
        }))
    }

    /// A smooth, compressible, photo-like fixture.
    ///
    /// `sample` is a hard gradient with one constant channel, which is fine for
    /// "does it encode" and useless for anything involving size: the only way a
    /// size claim can be wrong is by being tested against the wrong kind of
    /// picture.
    fn photo(w: u32, h: u32) -> image::DynamicImage {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            let fx = f64::from(x) / f64::from(w.max(1)) * 6.0;
            let fy = f64::from(y) / f64::from(h.max(1)) * 4.0;
            let wave = (fx).sin() * 80.0 + (fy).cos() * 55.0;
            image::Rgb([
                (128.0 + wave).clamp(0.0, 255.0) as u8,
                (100.0 + wave * 0.6).clamp(0.0, 255.0) as u8,
                (150.0 - wave * 0.5).clamp(0.0, 255.0) as u8,
            ])
        }))
    }

    /// Saturated colour edges on a coloured field: the content that decides
    /// whether 4:2:0 is honest. Red bars 4px wide on pure blue, with thin white
    /// rules across them — a logo or a screenshot rather than a landscape.
    fn colour_bars(w: u32, h: u32) -> image::DynamicImage {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            let on_bar = x % 12 < 4;
            let on_rule = y % 24 == 7;
            match (on_bar, on_rule) {
                (_, true) => image::Rgb([255, 255, 255]),
                (true, false) => image::Rgb([255, 0, 0]),
                (false, false) => image::Rgb([0, 0, 255]),
            }
        }))
    }

    fn jpeg_options(quality: u8, chroma: ChromaSubsampling, progressive: bool) -> EncodingOptions {
        EncodingOptions {
            quality,
            progressive,
            chroma_subsampling: chroma,
        }
    }

    /// Mean absolute error of the blue-difference chroma channel, `B - Y`.
    ///
    /// Luma is deliberately left out of the metric: chroma subsampling does not
    /// touch luma resolution at all, so a metric that included it would dilute
    /// the one thing under test with the part that never changes.
    fn chroma_error(source: &image::RgbImage, decoded: &image::RgbImage) -> f64 {
        assert_eq!(source.dimensions(), decoded.dimensions());
        let mut total = 0.0f64;
        let mut count = 0.0f64;
        for (a, b) in source.pixels().zip(decoded.pixels()) {
            let (ar, ag, ab) = (f64::from(a.0[0]), f64::from(a.0[1]), f64::from(a.0[2]));
            let (br, bg, bb) = (f64::from(b.0[0]), f64::from(b.0[1]), f64::from(b.0[2]));
            total += ((bb - (br + bg + bb) / 3.0) - (ab - (ar + ag + ab) / 3.0)).abs();
            count += 1.0;
        }
        total / count
    }

    /// The `SOFn` marker that starts a frame: `0xC0` baseline, `0xC2` progressive.
    ///
    /// Asserting on the marker is what proves "progressive" rather than "a
    /// different number of bytes", which is the only way to be sure the encoder
    /// really changed the scan order.
    fn jpeg_frame_marker(bytes: &[u8]) -> Option<u8> {
        let mut at = 2usize; // past SOI
        while at + 4 <= bytes.len() {
            if bytes[at] != 0xFF {
                return None;
            }
            let marker = bytes[at + 1];
            // Standalone markers carry no length.
            if (0xD0..=0xD9).contains(&marker) || marker == 0xFF {
                at += 2;
                continue;
            }
            if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 {
                return Some(marker);
            }
            let len = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
            at += 2 + len;
        }
        None
    }

    #[test]
    fn sniffs_by_magic_bytes_not_extension() {
        for format in [
            OutputFormat::Png,
            OutputFormat::Jpeg,
            OutputFormat::WebP,
            OutputFormat::Gif,
        ] {
            let bytes = encode(
                &sample(),
                format,
                EncodingOptions::default().with_quality(80),
            )
            .unwrap();
            assert_eq!(detect_format(&bytes).unwrap(), format);
        }
    }

    #[test]
    fn roundtrips_every_writable_format() {
        for format in [
            OutputFormat::Jpeg,
            OutputFormat::Png,
            OutputFormat::WebP,
            OutputFormat::Tiff,
            OutputFormat::Bmp,
            OutputFormat::Gif,
            OutputFormat::Ico,
        ] {
            let bytes = encode(
                &sample(),
                format,
                EncodingOptions::default().with_quality(80),
            )
            .unwrap();
            assert!(!bytes.is_empty(), "{format:?} produced nothing");
            let back = image::load_from_memory(&bytes).unwrap();
            assert_eq!((back.width(), back.height()), (64, 48), "{format:?} drift");
        }
    }

    #[test]
    fn every_chroma_level_encodes_and_decodes_to_the_same_picture() {
        let img = photo(160, 120);
        let mut sizes = Vec::new();
        for chroma in ChromaSubsampling::ALL {
            let bytes = encode(&img, OutputFormat::Jpeg, jpeg_options(80, chroma, false)).unwrap();
            assert!(!bytes.is_empty(), "{} produced nothing", chroma.label());
            let back = image::load_from_memory(&bytes).unwrap().to_rgb8();
            assert_eq!(
                back.dimensions(),
                (160, 120),
                "{} changed the dimensions",
                chroma.label()
            );
            sizes.push((chroma, bytes.len()));
        }
        // More colour samples, more bytes: the ordering is the claim the size
        // savings rest on, so it is asserted rather than assumed.
        for pair in sizes.windows(2) {
            assert!(
                pair[0].1 > pair[1].1,
                "{} ({}) should exceed {} ({})",
                pair[0].0.label(),
                pair[0].1,
                pair[1].0.label(),
                pair[1].1
            );
        }
    }

    #[test]
    fn four_four_four_is_strictly_larger_than_four_two_zero_at_the_same_quality() {
        let img = photo(600, 400);
        let full = encode(
            &img,
            OutputFormat::Jpeg,
            jpeg_options(80, ChromaSubsampling::Luma444, false),
        )
        .unwrap();
        let subsampled = encode(
            &img,
            OutputFormat::Jpeg,
            jpeg_options(80, ChromaSubsampling::Luma420, false),
        )
        .unwrap();
        let dims = |bytes: &[u8]| -> (u32, u32) {
            let img = image::load_from_memory(bytes).unwrap();
            (img.width(), img.height())
        };
        assert_eq!(
            dims(&full),
            dims(&subsampled),
            "the only difference between these two files is chroma resolution"
        );
        assert!(
            full.len() > subsampled.len(),
            "4:4:4 ({}) should exceed 4:2:0 ({})",
            full.len(),
            subsampled.len()
        );
    }

    #[test]
    fn four_four_four_keeps_a_saturated_colour_edge_that_four_two_zero_loses() {
        // The evidence behind the default: on content whose *content* is colour,
        // halving the chroma resolution is visible, and 4:4:4 is measurably
        // closer to the source. This is what "text on a coloured background
        // fringes at 4:2:0" means in numbers rather than in a warning label.
        let img = colour_bars(240, 160);
        let source = img.to_rgb8();
        let error_for = |chroma: ChromaSubsampling| -> f64 {
            let bytes = encode(&img, OutputFormat::Jpeg, jpeg_options(95, chroma, false)).unwrap();
            chroma_error(&source, &image::load_from_memory(&bytes).unwrap().to_rgb8())
        };
        let full = error_for(ChromaSubsampling::Luma444);
        let half = error_for(ChromaSubsampling::Luma422);
        let quarter = error_for(ChromaSubsampling::Luma420);

        assert!(
            quarter > full * 3.0,
            "4:2:0 should be far worse on colour edges than 4:4:4: {quarter} vs {full}"
        );
        assert!(
            half < quarter && half > full,
            "4:2:2 should sit between them: {half}, vs 4:4:4 {full} and 4:2:0 {quarter}"
        );
    }

    #[test]
    fn progressive_jpeg_is_progressive_and_decodes_to_the_same_size() {
        let img = photo(600, 400);
        let baseline = encode(
            &img,
            OutputFormat::Jpeg,
            jpeg_options(80, ChromaSubsampling::Luma420, false),
        )
        .unwrap();
        let progressive = encode(
            &img,
            OutputFormat::Jpeg,
            jpeg_options(80, ChromaSubsampling::Luma420, true),
        )
        .unwrap();

        // SOF0 is baseline, SOF2 is progressive. Anything else means the flag did
        // not reach the encoder.
        assert_eq!(jpeg_frame_marker(&baseline), Some(0xC0));
        assert_eq!(jpeg_frame_marker(&progressive), Some(0xC2));

        for (label, bytes) in [("baseline", &baseline), ("progressive", &progressive)] {
            let back = image::load_from_memory(bytes).unwrap();
            assert_eq!(
                (back.width(), back.height()),
                (600, 400),
                "{label} decoded to a different size"
            );
        }
    }

    #[test]
    fn progressive_costs_bytes_at_the_same_quality() {
        // Not free, and the number is worth knowing: jpeg-encoder writes four
        // scans with a spectral-selection and successive-approximation split,
        // which costs 20-70% here depending on quality. That is the price of a
        // picture that appears at all on a slow connection.
        let img = photo(600, 400);
        for quality in [50u8, 80, 95] {
            let baseline = encode(
                &img,
                OutputFormat::Jpeg,
                jpeg_options(quality, ChromaSubsampling::Luma420, false),
            )
            .unwrap();
            let progressive = encode(
                &img,
                OutputFormat::Jpeg,
                jpeg_options(quality, ChromaSubsampling::Luma420, true),
            )
            .unwrap();
            assert!(
                progressive.len() > baseline.len(),
                "at q{quality} progressive ({}) should exceed baseline ({})",
                progressive.len(),
                baseline.len()
            );
        }
    }

    #[test]
    fn an_option_a_format_cannot_honour_changes_nothing() {
        // A caller with one quality slider should not get different PNGs
        // depending on a progressive flag aimed at JPEG. Asserted byte-for-byte,
        // because "the flag is ignored" is exactly the kind of claim that rots.
        let img = photo(80, 60);
        let quiet = encode(
            &img,
            OutputFormat::Png,
            EncodingOptions {
                quality: 80,
                progressive: false,
                chroma_subsampling: ChromaSubsampling::Luma420,
            },
        )
        .unwrap();
        let loud = encode(
            &img,
            OutputFormat::Png,
            EncodingOptions {
                quality: 80,
                progressive: true,
                chroma_subsampling: ChromaSubsampling::Luma444,
            },
        )
        .unwrap();
        assert_eq!(
            quiet, loud,
            "PNG output must not depend on JPEG-only options"
        );
    }

    #[test]
    fn every_format_and_option_combination_either_round_trips_or_fails_cleanly() {
        let img = photo(48, 32);
        for format in OutputFormat::all() {
            for chroma in ChromaSubsampling::ALL {
                for progressive in [false, true] {
                    let options = jpeg_options(70, chroma, progressive);
                    match encode(&img, *format, options) {
                        Ok(bytes) => {
                            assert!(
                                !bytes.is_empty(),
                                "{format:?} {}/progressive={progressive} produced nothing",
                                chroma.label()
                            );
                            // Either it decodes, or it is a format this build
                            // writes but cannot read. What it must never do is
                            // panic, and a decode failure must say why.
                            if let Err(e) = image::load_from_memory(&bytes) {
                                assert!(
                                    !e.to_string().is_empty(),
                                    "{format:?} produced a file we cannot read or explain"
                                );
                            }
                        }
                        Err(e) => {
                            assert!(
                                !e.to_string().is_empty(),
                                "{format:?} refused an option without saying why"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn avif_is_offered_exactly_when_this_build_has_the_encoder() {
        // Both branches of the acceptance criterion, asserted as agreement
        // rather than as one hard-coded answer: the capability flag, the
        // read-only flag and what `encode` actually does are one fact stated
        // three ways, and a build with `--no-default-features` has to pass this
        // too.
        assert_eq!(
            crate::capabilities().avif_encode,
            cfg!(feature = "avif"),
            "the capability list must not promise an encoder this build lacks"
        );
        assert_eq!(
            OutputFormat::Avif.is_read_only(),
            !cfg!(feature = "avif"),
            "a format with no encoder must be read-only, or the UI will offer it"
        );
        let encoded = encode(&sample(), OutputFormat::Avif, EncodingOptions::default());
        assert_eq!(
            encoded.is_ok(),
            cfg!(feature = "avif"),
            "encode disagreed with the feature flag: {:?}",
            encoded.err()
        );
        if let Err(e) = &encoded {
            // Hard rule 9: a refusal has to say what to do instead.
            let text = e.to_string();
            assert!(
                text.contains("JPEG") && text.contains("avif"),
                "the AVIF refusal should name the alternatives: {text}"
            );
        }
    }

    #[cfg(feature = "avif")]
    #[test]
    fn avif_output_is_a_real_avif_container() {
        let img = photo(64, 48);
        let bytes = encode(
            &img,
            OutputFormat::Avif,
            EncodingOptions::default().with_quality(70),
        )
        .unwrap();
        assert!(
            bytes.len() > 100,
            "an AVIF of 64x48 should not be {} bytes",
            bytes.len()
        );
        // Recognised by our own detector from the container's brand bytes.
        assert_eq!(detect_format(&bytes).unwrap(), OutputFormat::Avif);
        assert_eq!(
            crate::heic::detect(&bytes).unwrap(),
            OutputFormat::Avif,
            "the ftyp brand must name AVIF"
        );
        // And this build cannot read it back, which is why `avif_decode` is a
        // separate flag. Asserted rather than assumed: if a future phase adds a
        // decoder, this test fails and the flag gets revisited.
        assert!(!crate::capabilities().avif_decode);
        let Err(err) = image::load_from_memory(&bytes) else {
            panic!("this build cannot decode AV1, which is what avif_decode: false says");
        };
        assert!(
            err.to_string().contains("Avif"),
            "the failure should name the format, not 'unsupported': {err}"
        );
    }

    #[test]
    fn every_refusal_says_what_to_choose_instead() {
        // Hard rule 9, asserted over the whole enum so the next format added
        // cannot ship a dead end. A message that names the problem but not the way
        // out leaves the user with a failure and no next action, which is the
        // failure mode the rule is about.
        for format in OutputFormat::all() {
            let note = format.quality_note();
            assert!(!note.trim().is_empty(), "{format:?} has no note at all");
            assert!(
                note.ends_with('.') && !note.contains("  "),
                "{format:?} is not written like a sentence: {note}"
            );
            if !format.supports_byte_target() {
                let names_an_alternative = ["JPEG", "PNG", "WebP", "AVIF"]
                    .iter()
                    .any(|name| note.contains(name));
                assert!(
                    names_an_alternative,
                    "{format:?} cannot take a size ceiling, so its note must name a format \
                     that can: {note}"
                );
            }
        }
    }

    #[test]
    fn chroma_levels_explain_themselves() {
        for chroma in ChromaSubsampling::ALL {
            assert!(!chroma.label().is_empty());
            let (w, h) = chroma.chroma_sample_ratio();
            assert!(
                w <= 4 && h <= 4 && w > 0 && h > 0,
                "{} has no ratio",
                chroma.label()
            );
            // The trade-off text is user-facing, so it has to exist and to say
            // what 4:2:0 costs rather than only what it is.
            assert!(
                chroma.trade_off().contains("chroma") || chroma.trade_off().contains("colour"),
                "{} has no trade-off text",
                chroma.label()
            );
        }
        assert_eq!(
            ChromaSubsampling::default(),
            ChromaSubsampling::Luma420,
            "the photo default is part of the JSON contract; changing it changes every export"
        );
    }

    #[test]
    fn jpeg_flattens_alpha_onto_white() {
        let transparent = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            16,
            16,
            image::Rgba([0, 0, 0, 0]),
        ));
        let bytes = encode(
            &transparent,
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(90),
        )
        .unwrap();
        let back = image::load_from_memory(&bytes).unwrap().to_rgb8();
        let px = back.get_pixel(8, 8).0;
        assert!(
            px[0] > 240 && px[1] > 240 && px[2] > 240,
            "expected white, got {px:?}"
        );
    }

    #[test]
    fn lossy_quality_monotonically_increases_size() {
        if !OutputFormat::Jpeg.supports_quality() {
            return;
        }
        let small = encode(
            &sample(),
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(10),
        )
        .unwrap()
        .len();
        let large = encode(
            &sample(),
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(95),
        )
        .unwrap()
        .len();
        assert!(large > small, "q95 ({large}) should exceed q10 ({small})");
    }

    #[test]
    fn png_is_lossless() {
        let img = sample();
        let bytes = encode(
            &img,
            OutputFormat::Png,
            EncodingOptions::default().with_quality(1),
        )
        .unwrap();
        let back = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert_eq!(img.to_rgb8().into_raw(), back.into_raw());
    }

    #[test]
    fn a_read_only_format_is_refused_by_the_encoder() {
        // `is_read_only` is what the UI greys formats out with, so it has to
        // agree with what `encode` does rather than merely intend to.
        for format in OutputFormat::all().iter().filter(|f| f.is_read_only()) {
            let err = encode(
                &sample(),
                *format,
                EncodingOptions::default().with_quality(80),
            )
            .err()
            .unwrap_or_else(|| panic!("{format:?} is marked read-only but encoded"));
            assert!(
                matches!(err, Error::UnsupportedFormat(_)),
                "{format:?} failed for the wrong reason: {err:?}"
            );
        }
    }

    #[test]
    fn a_jpeg_with_a_heic_extension_is_never_reported_as_heic() {
        // The extension is never consulted, so this holds for a *real* HEIC
        // signature too: the only bytes that decide are the container's.
        let jpeg = encode(
            &sample(),
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(80),
        )
        .unwrap();
        assert_eq!(detect_format(&jpeg).unwrap(), OutputFormat::Jpeg);
    }

    #[test]
    fn extensions_and_mime_types_are_consistent() {
        for format in OutputFormat::all() {
            assert_eq!(
                OutputFormat::from_extension(format.extension()),
                Some(*format)
            );
            assert!(format.mime().starts_with("image/"));
        }
    }

    #[test]
    fn byte_targets_are_only_offered_where_they_work() {
        for format in OutputFormat::all() {
            assert_eq!(
                format.supports_byte_target(),
                format.supports_quality(),
                "{format:?} capability flags disagree"
            );
        }
    }

    #[test]
    fn a_lossy_option_is_never_offered_where_it_cannot_act() {
        // The phase's own rule: no lossy knob on a format that ignores it. The
        // encoder-side half is that `is_lossless` is what `supports_quality` is
        // derived from, so there is no way for the two to disagree quietly.
        for format in OutputFormat::all() {
            assert_eq!(
                format.supports_quality(),
                !format.is_lossless(),
                "{format:?}: supports_quality must be the negation of is_lossless"
            );
            // Encoder options nobody can honour are advertised by nobody.
            for (advertised, name) in [
                (format.supports_progressive(), "progressive"),
                (format.supports_chroma_subsampling(), "chroma"),
            ] {
                if advertised {
                    assert!(
                        !format.is_read_only(),
                        "{format:?} advertises {name} but has no encoder in this build"
                    );
                }
                if format.is_read_only() || format.is_lossless() {
                    assert!(
                        !advertised,
                        "{format:?} must not advertise {name}: it has no lossy encoder"
                    );
                }
            }
        }
    }

    #[test]
    fn exif_segment_is_inserted_after_soi() {
        let mut jpeg = encode(
            &sample(),
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(80),
        )
        .unwrap();
        append_exif(&mut jpeg, b"II*\0fake").unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        assert_eq!(&jpeg[2..4], &[0xFF, 0xE1]);
        // Still decodes, which is the property that actually matters.
        assert!(image::load_from_memory(&jpeg).is_ok());
    }

    #[test]
    fn exif_append_refuses_non_jpeg() {
        let mut not_jpeg = b"PNG\r\n\x1a\n".to_vec();
        assert!(append_exif(&mut not_jpeg, b"x").is_err());
    }
}
