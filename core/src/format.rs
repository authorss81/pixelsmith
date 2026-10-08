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
    ///
    /// WebP's answer is a property of the *build*, not of the format, which is
    /// why it is the one arm derived from a `cfg!`. With `webp-lossy` on (the
    /// default since phase-08) it is lossy and the knob works; without it the
    /// pure-Rust lossless encoder is the only one compiled in, and reporting
    /// otherwise would offer a slider that changes nothing. Both configurations
    /// are asserted by `format::tests::webp_reports_the_truth_about_this_build`.
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
    /// Only JPEG does in this build. PNG, TIFF and BMP store colour per pixel.
    ///
    /// WebP is a genuine omission rather than an unwired knob, and phase-07
    /// flagged the difference as something this phase had to decide. It was
    /// checked against libwebp's `WebPConfig` and the answer is that there is
    /// nothing to set: the struct has 29 fields and not one of them is a chroma
    /// sampling factor. VP8 always stores 4:2:0 and libwebp exposes no way to
    /// ask for otherwise, so a "chroma" control here would be a slider that
    /// cannot move. Answering `false` is the honest report; phase-12 revisits it
    /// if ICC handling changes what "full chroma" would even mean for WebP.
    ///
    /// AVIF is written by rav1e, which always uses full-resolution chroma for the
    /// same reason: asking for 4:2:0 there would be a promise the encoder cannot
    /// keep.
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
                "WebP has a quality setting, and at a given quality it is smaller than \
                 JPEG for most photographs — 60 is a good default for a photo, and a \
                 byte ceiling works here too."
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

    /// Whether an ICC profile can be written into this format by this build.
    ///
    /// True for JPEG and PNG only, and that is a statement about what has been
    /// written and tested rather than about what the formats allow. WebP has an
    /// `ICCP` chunk, but placing one turns a simple lossy file into an extended
    /// `VP8X` one and there is no WebP reader in this tree to check the result
    /// against, so offering it would be offering a file that might not open.
    ///
    /// The profile is dropped rather than embedded by default — see
    /// [`crate::colour::ColourOptions`] — so this is only consulted when a caller
    /// explicitly asked to keep it.
    pub fn supports_icc(self) -> bool {
        matches!(self, Self::Jpeg | Self::Png)
    }

    /// What to say when a caller asked to carry a colour profile into a format
    /// that cannot take one.
    pub fn icc_note(self) -> &'static str {
        match self {
            Self::WebP => {
                "this build cannot write a colour profile into WebP, so the photo's \
                          colours would be attached to nothing. Ask for JPEG or PNG to keep \
                          the profile, or turn the option off to convert the photo to sRGB \
                          instead"
            }
            _ => {
                "this format cannot carry a colour profile. Ask for JPEG or PNG to keep it, \
                  or turn the option off to convert the photo to sRGB instead"
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
        let encoded = if is_opaque(&rgba) {
            encode_webp_lossy_rgb(&rgba, quality)?
        } else {
            encode_webp_lossy_rgba(&rgba, quality)?
        };
        cursor.write_all(&encoded)?;
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

/// True when no pixel is less than fully opaque.
///
/// This asks about the *pixels*, not the colour type, and the distinction is
/// load-bearing: `exif::strip` returns an `ImageRgba8` unconditionally, because
/// re-encoding from raw samples is what actually removes EXIF (hard rule 6). So
/// on the pipeline every image reaching this function has an alpha channel, and
/// a `has_alpha()` test would take the four-channel path for every export
/// forever. The three-channel path below exists to avoid that, so testing the
/// colour type would have made it dead code while appearing to work.
fn is_opaque(rgba: &image::RgbaImage) -> bool {
    rgba.pixels().all(|p| p.0[3] == 255)
}

/// libwebp's `WEBP_MAX_DIMENSION`, from `src/webp/encode.h`. Its encoder refuses
/// anything larger and so does `image`'s lossless WebP encoder.
pub(crate) const WEBP_MAX_DIMENSION: u32 = 16_383;

/// Packed three-channel encode. 25% less input handed to libwebp — 3 bytes per
/// pixel rather than 4.
///
/// The file this produces is **byte-identical** to the four-channel encode of the
/// same opaque picture, at every quality: measured on the 1200x900 `photo`
/// fixture at q10 through q100, all eight were equal. That is not a small
/// correction to the usual claim; it is the reason the optimisation is worth
/// keeping anyway. libwebp detects that the alpha plane is constant and drops it
/// before compressing, so the fourth channel costs a copy and a scan here and
/// nothing at all in the output. The saving is memory bandwidth and cache, not
/// bytes on disk — and it is asserted as an equality rather than an inequality
/// precisely because "the RGB path produces a smaller file" is false.
#[cfg(feature = "webp-lossy")]
fn encode_webp_lossy_rgb(rgba: &image::RgbaImage, quality: u8) -> Result<Vec<u8>> {
    check_webp_dimensions(rgba.width(), rgba.height())?;
    let mut packed = Vec::with_capacity(packed_rgb_len(rgba.width(), rgba.height()));
    for px in rgba.pixels() {
        packed.extend_from_slice(&px.0[..3]);
    }
    let encoded = webp::Encoder::from_rgb(&packed, rgba.width(), rgba.height())
        .encode_simple(false, f32::from(quality))
        // `encode_simple`, not `encode`: the latter unwraps internally, so a
        // dimension libwebp refuses arrives as a panic. Hard rule 3 says no panic
        // on bytes the user picked off their disk, and these pixels came from
        // there.
        .map_err(webp_encode_error)?;
    Ok(encoded.to_vec())
}

#[cfg(feature = "webp-lossy")]
fn encode_webp_lossy_rgba(rgba: &image::RgbaImage, quality: u8) -> Result<Vec<u8>> {
    check_webp_dimensions(rgba.width(), rgba.height())?;
    let encoded = webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height())
        .encode_simple(false, f32::from(quality))
        .map_err(webp_encode_error)?;
    Ok(encoded.to_vec())
}

#[cfg(feature = "webp-lossy")]
fn packed_rgb_len(width: u32, height: u32) -> usize {
    // Both operands are already bounded by `validate::Limits`, and u32 -> usize
    // is lossless on every shipped target, so the product cannot overflow.
    width as usize * height as usize * 3
}

/// Refuse an image libwebp cannot represent, in a sentence that says what to do.
///
/// `Limits` allows 30 000 px per side; libwebp stops at 16 383. Without this an
/// 18 000 x 200 panorama passes every limit the engine applies, decodes, resizes
/// — and then dies inside the encoder. It is a reachable refusal, not a
/// hypothetical: any camera that stitches a wide panorama produces one.
#[cfg(feature = "webp-lossy")]
fn check_webp_dimensions(width: u32, height: u32) -> Result<()> {
    if width > WEBP_MAX_DIMENSION || height > WEBP_MAX_DIMENSION {
        return Err(Error::UnsupportedFormat(
            "this picture is wider or taller than WebP can store (16383 pixels a side): \
             ask for JPEG or PNG as the output format, or crop it to fit",
        ));
    }
    Ok(())
}

/// libwebp's error codes are C enum constants with no message attached, so each
/// is rendered by hand into something a person can act on. Hard rule 9.
///
/// The match is exhaustive over every variant rather than carrying a `_` arm,
/// because the exhaustive form is what makes a libwebp upgrade that adds a code
/// a compile error here instead of a silently generic message in the field.
#[cfg(feature = "webp-lossy")]
fn webp_encode_error(err: webp::WebPEncodingError) -> Error {
    use webp::WebPEncodingError as E;
    let reason = match err {
        E::VP8_ENC_OK => "was accepted by libwebp, which should not be an error",
        E::VP8_ENC_ERROR_OUT_OF_MEMORY | E::VP8_ENC_ERROR_BITSTREAM_OUT_OF_MEMORY => {
            "needed more memory than this device has"
        }
        E::VP8_ENC_ERROR_NULL_PARAMETER => {
            "was passed to libwebp with a missing buffer, which is an engine bug"
        }
        E::VP8_ENC_ERROR_INVALID_CONFIGURATION => {
            "was given settings libwebp rejected, which is an engine bug"
        }
        E::VP8_ENC_ERROR_BAD_DIMENSION => "is larger than WebP can store (16383 pixels a side)",
        E::VP8_ENC_ERROR_PARTITION0_OVERFLOW | E::VP8_ENC_ERROR_PARTITION_OVERFLOW => {
            "has detail that does not fit in WebP's block structure"
        }
        E::VP8_ENC_ERROR_BAD_WRITE => "could not be written out",
        E::VP8_ENC_ERROR_FILE_TOO_BIG => "produced a file too large for the format",
        E::VP8_ENC_ERROR_USER_ABORT => "was cancelled part way through",
        E::VP8_ENC_ERROR_LAST => "was rejected by libwebp",
    };
    Error::Webp(reason)
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
    estimate_pixels(px)
}

/// Encode every frame of an animation into one container.
///
/// GIF is the only encoder in this tree that takes more than one frame, so every
/// other format is refused *by name* rather than quietly given the first one.
/// That refusal is the reason this function exists separately from
/// [`encode`]: the alternative is a caller with a `Vec<Frame>` handing the
/// still encoder its head element and believing it wrote an animation, which is
/// the exact failure [`crate::animation`] exists to make impossible.
///
/// No `EncodingOptions` argument, because GIF has none of the three: it is
/// palette-quantised, has no scan order and stores chroma at full resolution.
/// [`OutputFormat::is_lossless`] already reports that, so a caller that wants to
/// offer the sliders has already been told not to.
pub fn encode_frames(frames: &[image::Frame], format: OutputFormat) -> Result<Vec<u8>> {
    if format != OutputFormat::Gif {
        return Err(Error::UnsupportedFormat(
            "this format holds one picture, so it cannot carry an animation. Choose GIF \
             output to keep every frame",
        ));
    }
    if frames.is_empty() {
        return Err(Error::Encode(image::ImageError::IoError(
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "an animation with no frames in it cannot be encoded",
            ),
        )));
    }

    let pixels: usize = frames
        .iter()
        .map(|f| {
            let b = f.buffer();
            b.width() as usize * b.height() as usize
        })
        .sum();
    let mut out: Vec<u8> = Vec::with_capacity(estimate_pixels(pixels));
    let mut cursor = std::io::Cursor::new(&mut out);
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut cursor);
        for frame in frames {
            encoder.encode_frame(frame.clone()).map_err(Error::Encode)?;
        }
        // The `gif` crate writes its trailer from `Drop`, and this writer is a
        // `Vec` cursor, which cannot fail — so the fallback branch of that
        // `Drop` (a panic on an I/O error) is unreachable here. Scoped so the
        // encoder, and therefore the trailer, is finished before the cursor is
        // read back out.
    }
    Ok(out)
}

/// A capacity hint from a pixel count, for the writers that append to a `Vec`.
///
/// Split out of [`estimate_capacity`] so the animation path can total the frames
/// it is about to write rather than sizing for the first one. A hint, not a
/// limit: over-reserving wastes memory and under-reserving costs one
/// reallocation.
fn estimate_pixels(px: usize) -> usize {
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

/// The most profile bytes one ICC segment can carry: a JPEG segment's payload is
/// at most 65533 bytes, and 16 of those are the `ICC_PROFILE\0` identifier plus
/// the sequence and total counts.
pub const ICC_CHUNK_MAX: usize = 65_533 - 16;

/// Splice a whole ICC profile into a JPEG as `APP2` segments.
///
/// Splitting is the profile's own convention, not a limitation here: a profile
/// longer than one segment is written as several `APP2` segments carrying a
/// one-based sequence number and the total count, and a reader reassembles them.
/// Doing it here rather than refusing a big profile is what keeps a modern
/// phone's profile — several kilobytes — from being un-embeddable.
///
/// Not the default: see [`crate::colour::ColourOptions`], which explains why
/// converting to sRGB and dropping the profile is the right answer for almost
/// every export.
pub fn append_icc(jpeg: &mut Vec<u8>, profile: &[u8]) -> Result<()> {
    if profile.is_empty() {
        return Err(Error::UnknownFormat);
    }
    let chunks = profile.len().div_ceil(ICC_CHUNK_MAX);
    let total = u16::try_from(chunks).map_err(|_| Error::UnknownFormat)?;
    if chunks == 1 {
        return append_icc_chunk(jpeg, 1, 1, profile);
    }
    for (index, part) in profile.chunks(ICC_CHUNK_MAX).enumerate() {
        append_icc_chunk(jpeg, index as u16 + 1, total, part)?;
    }
    Ok(())
}

/// Splice one `APP2` ICC segment in.
///
/// Public because the sequence number is a *fact about the profile* rather than
/// about the file, and the split-profile test in `colour` writes its halves in
/// the wrong order on purpose.
pub fn append_icc_chunk(jpeg: &mut Vec<u8>, sequence: u16, total: u16, part: &[u8]) -> Result<()> {
    /// "ICC_PROFILE\0", the APP2 identifier an ICC reader looks for.
    const ICC_ID: [u8; 12] = [
        0x49, 0x43, 0x43, 0x5F, 0x50, 0x52, 0x4F, 0x46, 0x49, 0x4C, 0x45, 0x00,
    ];
    if jpeg.len() < 2 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return Err(Error::UnknownFormat);
    }
    let payload_len = part
        .len()
        .checked_add(ICC_ID.len() + 4)
        .ok_or(Error::UnknownFormat)?;
    let len = u16::try_from(payload_len + 2).map_err(|_| Error::UnknownFormat)?;

    let mut segment = Vec::with_capacity(payload_len + 4);
    segment.extend_from_slice(&[0xFF, 0xE2]);
    segment.extend_from_slice(&len.to_be_bytes());
    segment.extend_from_slice(&ICC_ID);
    segment.extend_from_slice(&sequence.to_be_bytes());
    segment.extend_from_slice(&total.to_be_bytes());
    segment.extend_from_slice(part);

    // Before any other marker, as `append_exif` does: readers take the first.
    jpeg.splice(2..2, segment);
    Ok(())
}

/// Splice an `iCCP` chunk carrying an ICC profile into encoded PNG bytes.
///
/// The chunk goes immediately after `IHDR`, which is where the PNG spec requires
/// it, and its payload is a keyword, a NUL, the compression method and a
/// **zlib-compressed** profile — so an uncompressed payload would be a chunk
/// every reader rejects, which is why `flate2` is in the tree.
///
/// Takes bytes rather than an image on purpose: the caller has already encoded,
/// and re-encoding to attach three kilobytes of profile would change the pixels
/// and the file size for no reason.
pub fn png_insert_icc(png: &mut Vec<u8>, profile: &[u8]) -> Result<()> {
    use std::io::Write;
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    /// 4 length + 4 type + 13 IHDR data + 4 CRC.
    const AFTER_IHDR: usize = 8 + 25;

    if profile.is_empty() {
        return Err(Error::UnknownFormat);
    }
    if !png.starts_with(&SIGNATURE) || png.len() < AFTER_IHDR {
        return Err(Error::UnknownFormat);
    }

    let mut compressed = Vec::new();
    flate2::write::ZlibEncoder::new(&mut compressed, flate2::Compression::default())
        .write_all(profile)
        .map_err(Error::Io)?;
    if compressed.is_empty() {
        return Err(Error::UnknownFormat);
    }

    // keyword \0 compression-method zlib-profile. The keyword is 1-79 printable
    // Latin-1 characters with no leading or trailing space; the spec reserves
    // `ICC` and recommends it here.
    let mut data = Vec::with_capacity(compressed.len() + 5);
    data.extend_from_slice(b"ICC");
    data.push(0);
    data.push(0);
    data.extend_from_slice(&compressed);

    let len = u32::try_from(data.len()).map_err(|_| Error::UnknownFormat)?;
    let mut chunk = Vec::with_capacity(data.len() + 12);
    chunk.extend_from_slice(&len.to_be_bytes());
    chunk.extend_from_slice(b"iCCP");
    chunk.extend_from_slice(&data);
    chunk.extend_from_slice(&crc32fast::hash(&chunk).to_be_bytes());

    png.splice(AFTER_IHDR..AFTER_IHDR, chunk);
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

    /// The chunk that holds the picture: `VP8 ` for lossy, `VP8L` for lossless.
    ///
    /// The first chunk after the `RIFF`/`WEBP` header, at offset 12. Asserting on
    /// the chunk tag rather than on the file size is what makes "this build
    /// produced a lossy file" a statement about the *codec* instead of a guess
    /// from bytes — a size comparison cannot tell a small lossy encode from a
    /// small lossless one, and an encoder upgrade that silently switched codecs
    /// would leave every size-based test green.
    fn webp_bitstream_chunk(bytes: &[u8]) -> Option<[u8; 4]> {
        let tag: [u8; 4] = bytes.get(12..16)?.try_into().ok()?;
        match &tag {
            b"VP8 " | b"VP8L" | b"VP8X" => Some(tag),
            // ALPH means an alpha plane, which only a still WebP with
            // transparency carries. Treated as lossy-relevant rather than
            // lossy, so callers must handle it explicitly.
            _ => None,
        }
    }

    /// True when the bytes are a WebP whose picture data is a lossy VP8 bitstream.
    fn webp_is_lossy_bitstream(bytes: &[u8]) -> bool {
        webp_bitstream_chunk(bytes) == Some(*b"VP8 ")
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

    #[cfg(feature = "webp-lossy")]
    #[test]
    fn webp_reports_the_truth_about_this_build() {
        // One fact, stated four ways, and the test is that they agree rather than
        // that they equal `true`. A build with `--no-default-features` has to pass
        // this too, and it is the only configuration in which the answer is false.
        let caps = crate::capabilities();
        assert_eq!(
            caps.webp_lossy,
            cfg!(feature = "webp-lossy"),
            "the capability list must not promise lossy WebP to a build that cannot write it"
        );
        assert_eq!(
            OutputFormat::WebP.is_lossless(),
            !cfg!(feature = "webp-lossy"),
            "is_lossless must tell the UI the truth for the encoder this build has"
        );
        assert_eq!(OutputFormat::WebP.supports_quality(), caps.webp_lossy);
        assert_eq!(
            OutputFormat::WebP.supports_byte_target(),
            caps.webp_lossy,
            "a byte ceiling can only be kept by a format with a quality setting"
        );

        // And the bitstream itself agrees, which is the part a flag cannot fake.
        let bytes = encode(
            &photo(200, 150),
            OutputFormat::WebP,
            EncodingOptions::default().with_quality(80),
        )
        .unwrap();
        assert_eq!(
            webp_is_lossy_bitstream(&bytes),
            cfg!(feature = "webp-lossy"),
            "the encoder and the capability flag disagree about which codec ran"
        );
    }

    #[cfg(not(feature = "webp-lossy"))]
    #[test]
    fn webp_reports_the_truth_about_this_build() {
        // The same assertions as the arm above, in the configuration where the
        // answer is false. A test that only existed in the lossy build would go
        // green forever while the lossless build quietly claimed otherwise.
        let caps = crate::capabilities();
        assert!(!caps.webp_lossy);
        assert!(OutputFormat::WebP.is_lossless());
        assert!(!OutputFormat::WebP.supports_quality());
        assert!(!OutputFormat::WebP.supports_byte_target());
        let bytes = encode(
            &photo(200, 150),
            OutputFormat::WebP,
            EncodingOptions::default().with_quality(80),
        )
        .unwrap();
        assert!(
            !webp_is_lossy_bitstream(&bytes),
            "a build without webp-lossy must write VP8L, not VP8"
        );
        // A quality value is ignored rather than refused (hard rule 9's companion
        // rule in `Settings::validate`): one slider sits above the format picker.
        let q1 = encode(
            &photo(120, 90),
            OutputFormat::WebP,
            EncodingOptions::default().with_quality(10),
        )
        .unwrap();
        let q95 = encode(
            &photo(120, 90),
            OutputFormat::WebP,
            EncodingOptions::default().with_quality(95),
        )
        .unwrap();
        assert_eq!(q1, q95, "lossless WebP must not respond to quality");
    }

    #[cfg(feature = "webp-lossy")]
    #[test]
    fn lossy_webp_is_strictly_smaller_than_lossless_at_the_same_dimensions() {
        // The claim the whole feature exists for, and it is large: the lossless
        // encoder has no compression to offer a photographic gradient at this
        // quality setting, while lossy discards what the eye cannot see.
        let img = photo(600, 400);
        let lossy = encode(
            &img,
            OutputFormat::WebP,
            EncodingOptions::default().with_quality(80),
        )
        .unwrap();
        let lossless = encode_lossless_webp(&img);
        assert!(
            lossy.len() < lossless.len(),
            "lossy q80 ({}) should be well under lossless ({})",
            lossy.len(),
            lossless.len()
        );
        // The output is a real picture at the right size, not a thumbnail or an
        // empty buffer that happens to be small.
        let back = image::load_from_memory(&lossy).unwrap();
        assert_eq!((back.width(), back.height()), (600, 400));
        assert!(
            !webp_is_lossy_bitstream(&lossless),
            "the comparison must be real"
        );
    }

    #[cfg(feature = "webp-lossy")]
    #[test]
    fn webp_quality_ten_is_smaller_than_quality_ninety_five() {
        let img = photo(600, 400);
        for format in [OutputFormat::WebP, OutputFormat::Jpeg] {
            let small = encode(&img, format, EncodingOptions::default().with_quality(10))
                .unwrap()
                .len();
            let large = encode(&img, format, EncodingOptions::default().with_quality(95))
                .unwrap()
                .len();
            assert!(
                large > small,
                "{format:?}: q95 ({large}) should exceed q10 ({small})"
            );
        }
    }

    #[cfg(feature = "webp-lossy")]
    #[test]
    fn the_packed_rgb_path_is_used_for_opaque_pictures_and_matches_the_rgba_path_byte_for_byte() {
        // The prompt for this phase asked for an assertion that the packed-RGB
        // output is *smaller* than the RGBA path for an opaque image. It is not,
        // and the measurement is the interesting part: at every quality tested on
        // a 1200x900 opaque fixture, libwebp produced byte-identical output
        // either way. It detects that the alpha plane is constant and drops it
        // before compressing, so the fourth channel costs a copy and a scan and
        // nothing at all on disk.
        //
        // Asserted as an equality rather than the weaker `<=` because that is what
        // is true, and an equality is the assertion that would catch libwebp
        // changing its behaviour. The optimisation is kept anyway: it is 25% less
        // input, and it is the difference between handing libwebp 3 bytes per
        // pixel and 4 on every opaque export.
        let rgba = to_rgba8(&photo(600, 400));
        assert!(is_opaque(&rgba), "the fixture must actually be opaque");
        assert_eq!(
            packed_rgb_len(rgba.width(), rgba.height()) * 4,
            rgba.as_raw().len() * 3,
            "packed RGB must be exactly 25% less input than RGBA"
        );
        for quality in [10u8, 50, 80, 95] {
            let rgb = encode_webp_lossy_rgb(&rgba, quality).unwrap();
            let with_alpha = encode_webp_lossy_rgba(&rgba, quality).unwrap();
            assert_eq!(
                rgb, with_alpha,
                "q{quality}: the packed-RGB path should produce identical bytes, \
                 because libwebp drops a constant alpha plane"
            );
        }
    }

    #[cfg(feature = "webp-lossy")]
    #[test]
    fn opacity_is_decided_by_the_pixels_not_the_colour_type() {
        // `exif::strip` returns an ImageRgba8 unconditionally, so on the pipeline
        // every image has an alpha channel. Testing `color().has_alpha()` would
        // take the four-channel path for every export forever and make the
        // three-channel path unreachable while looking like it worked.
        let opaque_but_rgba = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([10, 20, 30, 255]),
        ));
        assert!(
            opaque_but_rgba.color().has_alpha(),
            "the fixture must have an alpha channel, or the test proves nothing"
        );
        assert!(
            is_opaque(&to_rgba8(&opaque_but_rgba)),
            "every pixel is 255, so the packed path applies"
        );

        let one_transparent =
            image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(8, 8, |_, _| {
                image::Rgba([10, 20, 30, 254])
            }));
        assert!(
            !is_opaque(&to_rgba8(&one_transparent)),
            "a single sub-opaque pixel has to switch the whole picture to RGBA"
        );
    }

    #[cfg(feature = "webp-lossy")]
    #[test]
    fn a_transparent_picture_keeps_its_transparency() {
        // The reason `is_opaque` exists. If the packed path were taken for this
        // image, the alpha would be dropped and the file would claim to be opaque.
        let mut rgba = image::RgbaImage::from_pixel(64, 64, image::Rgba([200, 100, 50, 255]));
        for x in 0..32 {
            for y in 0..64 {
                rgba.put_pixel(x, y, image::Rgba([200, 100, 50, 0]));
            }
        }
        let img = image::DynamicImage::ImageRgba8(rgba);
        assert!(!is_opaque(&to_rgba8(&img)));
        let bytes = encode(
            &img,
            OutputFormat::WebP,
            EncodingOptions::default().with_quality(90),
        )
        .unwrap();
        let back = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(
            back.get_pixel(8, 8).0[3],
            0,
            "a fully transparent pixel must not come back opaque"
        );
        assert_eq!(
            back.get_pixel(48, 8).0[3],
            255,
            "an opaque pixel must not come back transparent"
        );
    }

    #[cfg(feature = "webp-lossy")]
    #[test]
    fn a_picture_wider_than_webp_allows_is_refused_with_a_sentence_not_a_panic() {
        // libwebp stops at 16383 px a side; `validate::Limits` allows 30000. An
        // 18000x100 panorama therefore passes every limit the engine applies,
        // decodes, resizes — and then dies inside the encoder. `Encoder::encode`
        // unwraps internally, so calling it here used to be a hard crash, which
        // hard rule 3 forbids on bytes the user picked off their disk.
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            WEBP_MAX_DIMENSION + 1,
            4,
            image::Rgb([30, 60, 90]),
        ));
        let err = encode(
            &img,
            OutputFormat::WebP,
            EncodingOptions::default().with_quality(80),
        )
        .unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("16383"),
            "the refusal should name the limit: {text}"
        );
        assert!(
            text.contains("JPEG") || text.contains("PNG"),
            "the refusal should name a format that can do it: {text}"
        );

        // The same picture as PNG must succeed, which is what makes the message
        // true rather than merely reassuring.
        assert!(
            encode(
                &img,
                OutputFormat::Png,
                EncodingOptions::default().with_quality(80)
            )
            .is_ok(),
            "PNG has no 16383 limit, so the alternative the message names must work"
        );
    }

    #[cfg(feature = "webp-lossy")]
    #[test]
    fn exactly_at_the_dimension_limit_still_encodes() {
        // The boundary the check above draws. A guard written `<` instead of `<=`
        // would refuse a picture libwebp is perfectly willing to write.
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            WEBP_MAX_DIMENSION,
            2,
            image::Rgb([30, 60, 90]),
        ));
        assert!(check_webp_dimensions(WEBP_MAX_DIMENSION, WEBP_MAX_DIMENSION).is_ok());
        assert!(check_webp_dimensions(WEBP_MAX_DIMENSION + 1, 2).is_err());
        assert!(check_webp_dimensions(2, WEBP_MAX_DIMENSION + 1).is_err());
        assert!(
            encode(
                &img,
                OutputFormat::WebP,
                EncodingOptions::default().with_quality(60)
            )
            .is_ok(),
            "a picture exactly at libwebp's limit must be writable"
        );
    }

    #[cfg(feature = "webp-lossy")]
    #[test]
    fn every_webp_encoding_error_says_what_went_wrong() {
        // Hard rule 9 over libwebp's whole error enum. `webp_encode_error` matches
        // exhaustively, so a new code is a compile error there; this is the
        // companion assertion that no arm produces an empty or engineer-facing
        // message.
        use webp::WebPEncodingError as E;
        for code in [
            E::VP8_ENC_ERROR_OUT_OF_MEMORY,
            E::VP8_ENC_ERROR_BITSTREAM_OUT_OF_MEMORY,
            E::VP8_ENC_ERROR_NULL_PARAMETER,
            E::VP8_ENC_ERROR_INVALID_CONFIGURATION,
            E::VP8_ENC_ERROR_BAD_DIMENSION,
            E::VP8_ENC_ERROR_PARTITION0_OVERFLOW,
            E::VP8_ENC_ERROR_PARTITION_OVERFLOW,
            E::VP8_ENC_ERROR_BAD_WRITE,
            E::VP8_ENC_ERROR_FILE_TOO_BIG,
            E::VP8_ENC_ERROR_USER_ABORT,
            E::VP8_ENC_ERROR_LAST,
        ] {
            let text = webp_encode_error(code).to_string();
            assert!(text.contains("WebP"), "{code:?} lost the format: {text}");
            assert!(text.ends_with('.'), "{code:?} is not a sentence: {text}");
            assert!(
                !text.contains("VP8_ENC"),
                "{code:?} leaked the C constant to the user: {text}"
            );
        }
    }

    #[cfg(feature = "webp-lossy")]
    #[test]
    fn webp_byte_ceilings_are_reachable() {
        // The reason the preset ceilings are switched back on: a search that can
        // never meet its target would report `target_met: false` on every WebP
        // export, which is worse than not offering one.
        let img = photo(600, 400);
        for target in [20_000u64, 60_000, 150_000] {
            let (bytes, quality, met) = crate::TargetBytes::new(target)
                .encode_with(
                    &img,
                    OutputFormat::WebP,
                    EncodingOptions::default(),
                    &crate::target::default_encoder,
                )
                .unwrap();
            assert!(
                met,
                "{target} bytes was not reachable at any quality ({} bytes at q{quality})",
                bytes.len()
            );
            assert!(bytes.len() as u64 <= target);
            assert!(quality > 0, "a lossy format must not report quality 0");
        }
    }

    /// The pure-Rust lossless WebP encoder, for the size comparison above. Written
    /// out here rather than reached through `encode` because `encode` picks the
    /// encoder by build feature, and the comparison is only meaningful against the
    /// other codec in the same build.
    #[cfg(feature = "webp-lossy")]
    fn encode_lossless_webp(img: &image::DynamicImage) -> Vec<u8> {
        let rgba = to_rgba8(img);
        let mut out: Vec<u8> = Vec::new();
        {
            let mut cursor = std::io::Cursor::new(&mut out);
            image::codecs::webp::WebPEncoder::new_lossless(&mut cursor)
                .write_image(
                    rgba.as_raw(),
                    rgba.width(),
                    rgba.height(),
                    image::ExtendedColorType::Rgba8,
                )
                .unwrap();
        }
        out
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
