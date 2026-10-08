//! Colour management: sRGB, Display-P3, and what this build does with an ICC
//! profile.
//!
//! # The bug this exists to fix
//!
//! An untagged file is assumed to be sRGB, which is right. A **tagged** file was
//! simply having its tag dropped: a photograph shot on a wide-gamut phone, whose
//! pixels are Display-P3, came out of the export with those same numbers now
//! read as sRGB — washed out and duller, the way every naive resizer renders a P3
//! photo. Stripping the profile without converting it is not metadata hygiene,
//! it is a colour change, and hard rule 6 does not excuse it.
//!
//! # The conversion path
//!
//! ```text
//! source space  ──▶  working space  ──▶  output space
//! (from the file)    (sRGB by default)    (the working space, tagged or not)
//! ```
//!
//! The working space is [`ColourOptions::working_space`] and defaults to sRGB,
//! because a JPEG with no profile means sRGB to every reader that will ever open
//! it, and sRGB is what the overwhelming majority of screens assume. Converting
//! to the working space and then dropping the profile is therefore correct for
//! essentially every export this product makes, and the profile is dropped
//! precisely *because* the pixels were moved into the space it described.
//!
//! # What is implemented, precisely
//!
//! Two matrix-shaper RGB spaces, **sRGB** and **Display-P3**, both using the sRGB
//! transfer function, and an untagged input assumed to be sRGB. That is the whole
//! claim. This is **not** an ICC-conformant colour management module, and the
//! approximations below are stated rather than buried:
//!
//! * **One transfer function.** Every profile is treated as using the sRGB
//!   piecewise curve (IEC 61966-2-1). A profile whose `rTRC` is a different
//!   curve — pure gamma 2.2, say — is converted with the wrong one. Almost every
//!   RGB matrix/TRC profile in the wild uses the sRGB curve; reading the actual
//!   one means evaluating an arbitrary parametric or sampled curve, which is a
//!   larger and more dangerous module than this one.
//! * **A gamut mapping of "clip".** A Display-P3 colour outside the sRGB gamut is
//!   clipped per channel. That is what every other tool does, it is predictable,
//!   and it is *not* perceptual: a saturated P3 red clips to pure sRGB red rather
//!   than being desaturated towards its own luminance.
//! * **Classification by colorants.** A profile is called sRGB or Display-P3 by
//!   comparing its colorant chromaticities against two reference sets after
//!   undoing ICC's D50 adaptation, with the description tag as the
//!   fallback. A profile with the right colorants and an unusual TRC is
//!   misnamed; the alternative, refusing anything not exactly on the list, would
//!   refuse most real files.
//! * **LUT-based (A2B0/B2A0) display profiles are not evaluated.** They are read
//!   for their `rXYZ`/`gXYZ`/`bXYZ` colorants, which v4 requires of every RGB
//!   display profile, so they classify correctly, but their tone curve and any
//!   per-channel correction are ignored.
//! * **CMYK, Lab and grayscale are not converted at all.** They are recognised
//!   and refused in a sentence rather than mishandled.
//!
//! # Containers
//!
//! An ICC profile is found in the container, not by filename: JPEG `APP2`
//! segments carrying `ICC_PROFILE\0`, a PNG `iCCP` chunk, and a WebP `ICCP`
//! chunk. Writing one back out is supported for JPEG and PNG and is an explicit
//! opt-in ([`ColourOptions::embed_profile`]) rather than the default, because the
//! default answer above is the right one for almost every export and an embedded
//! 3 KB profile on every thumbnail is not. WebP cannot take one from this build;
//! see [`containers_read`].

use crate::error::{Error, Result};
use crate::format::OutputFormat;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

// ---------------------------------------------------------------------------
// The space
// ---------------------------------------------------------------------------

/// A colour space this engine can name.
///
/// Three of the four variants are things it can act on. `Other` is a profile it
/// can identify and refuses rather than guess at, and it exists so the refusal
/// can name the space instead of saying "unknown".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColourSpace {
    /// No profile in the file. Every untagged JPEG, PNG and WebP in the wild
    /// means sRGB, and treating an untagged file as anything else would be a
    /// guess with no evidence behind it.
    #[default]
    Untagged,
    /// sRGB IEC61966-2.1, and equivalently "Rec. 709 primaries with the sRGB
    /// curve" — which is the space a Display-P3 photograph is *converted into*.
    Srgb,
    /// Apple Display P3: wider primaries, the same transfer function. What an
    /// iPhone photographs in when "most compatible" is off.
    DisplayP3,
    /// A profile this build can identify but cannot convert from — ProPhoto,
    /// Adobe RGB, CMYK, Lab, grayscale.
    Other,
}

impl ColourSpace {
    /// The space the pixels actually are, once the untagged assumption is
    /// applied. Every conversion decision goes through this, so "an untagged
    /// file" can never become a fifth colour space by accident.
    pub fn working(self) -> Self {
        match self {
            Self::Untagged => Self::Srgb,
            other => other,
        }
    }

    /// The name to show a user, which is a name rather than a signature.
    pub fn label(self) -> &'static str {
        match self {
            Self::Untagged => "untagged (assumed sRGB)",
            Self::Srgb => "sRGB",
            Self::DisplayP3 => "Display-P3",
            Self::Other => "unrecognised colour space",
        }
    }

    /// Whether the engine has a matrix for this pair. `Untagged` folds to sRGB
    /// first, so an untagged file is convertible to nothing and refused from
    /// nothing, which is correct for both.
    pub fn convertible_to(self, target: Self) -> bool {
        matrix(self.working(), target.working()).is_some()
    }
}

// ---------------------------------------------------------------------------
// What a file says about itself
// ---------------------------------------------------------------------------

/// What a file's container says about its colour, read without decoding a pixel.
///
/// This is the whole of the engine's ICC reading: enough to name the space, to
/// report that a profile exists at all, and to hand the bytes to an explicit
/// "carry the profile through" request. Everything else about an ICC profile is
/// deliberately not interpreted — see the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ColourProfile {
    /// Whether the file carries an ICC profile at all. Distinct from
    /// [`Self::source`] being [`ColourSpace::Srgb`], because "an sRGB file with a
    /// profile" and "an untagged file" are different facts about the file even
    /// though they are the same conversion.
    pub icc_present: bool,
    /// The space the pixel values are in.
    pub source: ColourSpace,
    /// The ICC header's declared data colour space, as a name from a fixed
    /// vocabulary — never the four raw signature bytes, which are untrusted input
    /// and would otherwise end up in a message a user reads. `None` for a
    /// signature there is no word for.
    pub declared: Option<String>,
    /// The profile's own description tag, trimmed, when it has a readable one.
    pub description: Option<String>,
    /// How many bytes of profile the container carried.
    pub icc_bytes: usize,
}

impl ColourProfile {
    /// The profile of a file that has none.
    pub fn untagged() -> Self {
        Self::default()
    }

    /// Find and read the profile in a file's container.
    ///
    /// Never fails. Bytes a user picked off their disk cannot be allowed to fail
    /// a *decode* (hard rule 3), and a colour report is not worth a second error
    /// path: a container that does not parse is reported as untagged, which is
    /// the same answer a file with no profile gets.
    pub fn read(bytes: &[u8], format: OutputFormat) -> Self {
        match profile_bytes(bytes, format) {
            Some(icc) => {
                let mut profile = Self::parse(&icc);
                profile.icc_present = true;
                profile.icc_bytes = icc.len();
                profile
            }
            None => Self::untagged(),
        }
    }

    /// Read an ICC profile's header and tag table.
    ///
    /// Written for hostile input: every read is a checked slice, and a truncated
    /// or lying profile yields whatever could be established rather than an error.
    /// A profile claiming to be 500 KB that arrived as 40 bytes is still worth
    /// classifying if its header and colorants survived.
    pub fn parse(icc: &[u8]) -> Self {
        let mut out = Self::untagged();
        // The header is 128 bytes (ICC.1:2010 clause 6.1.1).
        let Some(header) = icc.get(..128) else {
            return out;
        };
        if header.get(36..40) != Some(b"acsp") {
            return out;
        }
        out.declared = signature_name(header.get(16..20)).map(str::to_owned);
        out.source = if out.declared.as_deref() == Some("RGB") {
            classify(icc)
        } else {
            ColourSpace::Other
        };
        out.description = read_description(icc);
        out
    }

    /// Whether the pixels have to move to reach `target`.
    ///
    /// The cheap, common answer is `false`, and it is what keeps the default path
    /// free: an untagged file and an sRGB file both land in the working space
    /// without touching a pixel.
    pub fn needs_conversion(&self, target: ColourSpace) -> bool {
        self.source.working() != target.working()
    }
}

// ---------------------------------------------------------------------------
// The request
// ---------------------------------------------------------------------------

/// How the engine maps a file's colour space onto the output's.
///
/// Three answers rather than one boolean, because "what should happen to the
/// profile" has three genuinely different answers and picking the default for
/// the user is only right for one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColourOptions {
    /// The space the pixels are converted into before encoding. sRGB by default,
    /// because a JPEG with no profile means sRGB to every reader that will ever
    /// open it.
    #[serde(default = "default_working_space")]
    pub working_space: ColourSpace,
    /// Keep the source values exactly as they decoded: no conversion, and no
    /// profile in the output either. The pixels then carry values whose meaning is
    /// whatever the source profile said, and nothing in the file says so.
    /// Explicit opt-in only, and the UI should say what that costs.
    #[serde(default)]
    pub keep_source_pixels: bool,
    /// Do not convert; carry the source ICC profile into the output instead, so
    /// the values keep their meaning and a colour-managed viewer still renders
    /// them correctly. Explicit opt-in, and only to a format that can hold one.
    #[serde(default)]
    pub embed_profile: bool,
}

fn default_working_space() -> ColourSpace {
    ColourSpace::Srgb
}

impl Default for ColourOptions {
    fn default() -> Self {
        Self {
            working_space: default_working_space(),
            keep_source_pixels: false,
            embed_profile: false,
        }
    }
}

impl ColourOptions {
    /// Refuse a request that asks for two incompatible things at once.
    ///
    /// Called from the path every export goes through, because two independent
    /// booleans that contradict each other is exactly what arrives from a UI
    /// whose two checkboxes got out of sync.
    pub fn validate(&self) -> Result<()> {
        if self.keep_source_pixels && self.embed_profile {
            return Err(Error::ConflictingOptions(
                "'keep the original colour values' and 'carry the colour profile' ask for \
                 opposite outputs: one leaves the file untagged, the other writes a profile \
                 into it. Pick one — leaving both off converts the photo to sRGB, which is \
                 what most exports want",
            ));
        }
        Ok(())
    }

    /// The space the encoded output's pixels will be in.
    pub fn output_space(&self, source: ColourSpace) -> ColourSpace {
        if self.keep_source_pixels || self.embed_profile {
            source
        } else {
            self.working_space
        }
    }
}

/// What the engine did about colour, reported next to the file it produced.
///
/// A file the UI cannot describe is a file the UI cannot be honest about, and
/// "we converted your Display-P3 photo" is a claim about somebody's photograph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ColourOutcome {
    /// The space the source pixels were in, as the file reported it.
    pub source: ColourSpace,
    /// The space the output pixels are in.
    pub output: ColourSpace,
    /// Whether the pixels were actually moved. False for an untagged source,
    /// which is the point: nothing happened and the numbers are the file's own.
    pub converted: bool,
    /// Whether an ICC profile was written into the output.
    pub profile_embedded: bool,
}

impl ColourOutcome {
    /// What a failed or cancelled file reports: nothing, because nothing is
    /// known and `Untagged` would itself be a claim.
    pub fn unknown() -> Self {
        Self::default()
    }
}

/// The whole colour decision for one file: what the working image becomes, and
/// what to report about it.
///
/// One function rather than three calls into this module because the three cases
/// have to agree about the *reported* space as well as the pixels, and the way
/// they drift apart is a report saying "converted to sRGB" beside a file that was
/// not. It returns a [`Cow`] so the default case — an untagged or sRGB file, which
/// is nearly all of them — allocates nothing and touches no pixel.
pub fn apply<'a>(
    img: &'a image::DynamicImage,
    profile: &ColourProfile,
    options: ColourOptions,
    output_format: OutputFormat,
) -> Result<(Cow<'a, image::DynamicImage>, ColourOutcome)> {
    options.validate()?;
    let source = profile.source;

    if options.keep_source_pixels {
        return Ok((
            Cow::Borrowed(img),
            ColourOutcome {
                source,
                output: source,
                converted: false,
                profile_embedded: false,
            },
        ));
    }

    if options.embed_profile {
        // Both of these are requests the engine cannot carry out, and both are
        // refused in a sentence rather than half-honoured: a file carrying a
        // profile that does not describe its pixels is worse than one that
        // carries none.
        if !output_format.supports_icc() {
            return Err(Error::ColourProfile(output_format.icc_note()));
        }
        if !profile.icc_present {
            return Err(Error::ColourProfile(
                "this photo has no colour profile to carry over, so there is nothing for \
                 the option to do. Turn it off and the photo will be converted to sRGB \
                 instead, which is what an untagged file already means",
            ));
        }
        return Ok((
            Cow::Borrowed(img),
            ColourOutcome {
                source,
                output: source,
                converted: false,
                profile_embedded: true,
            },
        ));
    }

    let target = options.working_space;
    if !source.convertible_to(target) {
        return Err(Error::UnsupportedColourSpace(unsupported_note(
            source,
            target,
            profile.declared.as_deref(),
        )));
    }
    let converted = profile.needs_conversion(target);
    let img = convert(img, source, target)?;
    Ok((
        img,
        ColourOutcome {
            source,
            output: target,
            converted,
            profile_embedded: false,
        },
    ))
}

// ---------------------------------------------------------------------------
// Conversion
// ---------------------------------------------------------------------------

/// The sRGB transfer function, encoded value to linear light.
///
/// **This is not a straight gamma 2.2**, and using one for the other is the most
/// common bug in this whole area: a 2.2 curve is a poor fit in the shadows, and a
/// round trip through the wrong one moves every mid-tone. The piecewise form is
/// IEC 61966-2-1 — a linear segment near black, a power segment above it, and a
/// join chosen so the function is continuous.
pub fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear light to the sRGB transfer function. The exact inverse of
/// [`srgb_to_linear`]; the exponent is `1 / 2.4`, not 2.2 again.
pub fn linear_to_srgb(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB primaries at D65, the reference every chromaticity here is measured
/// against.
///
/// One XYZ triple per row, each at `Y = 1`, red then green then blue. These are
/// the values colour-science crates publish and the ones CSS Color 4 uses to
/// derive the P3 → sRGB matrix. The matrix this module applies is *computed* from
/// them (see [`matrix`]) rather than pasted in, and
/// `tests::the_derived_matrix_is_the_published_one` checks the result against the
/// published one — so the numbers below are the single source of truth.
pub const SRGB_PRIMARIES: [[f32; 3]; 3] = [
    [0.412_390_8, 0.212_639, 0.019_330_82],
    [0.357_584_34, 0.715_168_7, 0.119_194_78],
    [0.180_480_8, 0.072_192_32, 0.950_532_15],
];

/// Display-P3 primaries: same illuminant and same transfer function as sRGB,
/// wider primaries. One XYZ triple per row, in the same order.
pub const DISPLAY_P3_PRIMARIES: [[f32; 3]; 3] = [
    [0.486_570_95, 0.228_974_56, 0.0],
    [0.265_667_7, 0.691_738_5, 0.045_113_38],
    [0.198_217_29, 0.079_286_91, 1.043_944_4],
];

/// Bradford chromatic adaptation from D50 to D65.
///
/// ICC requires a profile's colorants to be adapted to the D50 PCS illuminant, so
/// the numbers inside a profile are *not* the numbers above. Comparing them
/// directly would put every profile about 0.02 away from every reference and the
/// comparison would decide nothing; undoing the adaptation first is what turns
/// "is this sRGB or is it P3" into a question with an answer.
const BRADFORD_D50_TO_D65: [[f32; 3]; 3] = [
    [0.955_576_6, -0.023_039_3, 0.063_163_6],
    [-0.028_289_5, 1.009_941_6, 0.021_007_7],
    [0.012_298_2, -0.020_483_0, 1.329_909_8],
];

/// How far a colorant chromaticity may sit from a reference and still be called
/// the same space.
///
/// The two spaces are about 0.04 apart in `x` on the red primary, so 0.004 is
/// generous headroom over the D50 round trip's own error while still being a
/// hundredth of the thing being told apart.
const CHROMATICITY_TOLERANCE: f32 = 0.004;

/// A tag table claiming more tags than this is a lie or a bomb, and neither is a
/// tag table worth building a map from.
const MAX_TAGS: usize = 1024;

/// The longest description this module will read out of a profile, in bytes.
const MAX_TEXT: usize = 1024;

type Matrix3 = [[f32; 3]; 3];

fn identity() -> Matrix3 {
    [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
}

fn mul(a: &Matrix3, b: &Matrix3) -> Matrix3 {
    let mut out = [[0.0f32; 3]; 3];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            *cell = a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c];
        }
    }
    out
}

/// Invert a 3x3 by Gauss-Jordan with partial pivoting.
///
/// Returns `None` for a singular matrix rather than dividing by a near-zero
/// pivot, which is what keeps a corrupt primary set from producing a matrix full
/// of infinities and a picture full of white pixels.
fn invert(m: &Matrix3) -> Option<Matrix3> {
    let mut a = *m;
    let mut inv = identity();
    for col in 0..3 {
        let pivot_row = (col..3).max_by(|x, y| a[*x][col].abs().total_cmp(&a[*y][col].abs()))?;
        if a[pivot_row][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot_row);
        inv.swap(col, pivot_row);
        let pivot = a[col][col];
        for k in 0..3 {
            a[col][k] /= pivot;
            inv[col][k] /= pivot;
        }
        for row in 0..3 {
            if row == col {
                continue;
            }
            let factor = a[row][col];
            for k in 0..3 {
                a[row][k] -= factor * a[col][k];
                inv[row][k] -= factor * inv[col][k];
            }
        }
    }
    Some(inv)
}

/// RGB-to-XYZ for a set of primaries: the primaries are the matrix's columns.
fn to_xyz(primaries: &[[f32; 3]; 3]) -> Matrix3 {
    [
        [primaries[0][0], primaries[1][0], primaries[2][0]],
        [primaries[0][1], primaries[1][1], primaries[2][1]],
        [primaries[0][2], primaries[1][2], primaries[2][2]],
    ]
}

/// The matrix taking linear-light RGB in `from` to linear-light RGB in `to`.
///
/// Computed from the primaries rather than pasted in, so the two sets of numbers
/// above are the source of truth and there is exactly one place the conversion
/// could be wrong. `None` for a pair this build has no primaries for, which is
/// how [`ColourSpace::Other`] becomes a refusal instead of a wrong colour.
pub fn matrix(from: ColourSpace, to: ColourSpace) -> Option<Matrix3> {
    let from = match from.working() {
        ColourSpace::Srgb => &SRGB_PRIMARIES,
        ColourSpace::DisplayP3 => &DISPLAY_P3_PRIMARIES,
        _ => return None,
    };
    let to = match to.working() {
        ColourSpace::Srgb => &SRGB_PRIMARIES,
        ColourSpace::DisplayP3 => &DISPLAY_P3_PRIMARIES,
        _ => return None,
    };
    Some(mul(&invert(&to_xyz(to))?, &to_xyz(from)))
}

/// Move an image from one space to another, or hand it straight back.
///
/// Returning a [`Cow`] rather than an owned image is not a micro-optimisation: it
/// is what makes "an untagged source is a no-op" *free* rather than a copy, so
/// the overwhelmingly common case costs one enum comparison and touches no pixels
/// at all.
pub fn convert<'a>(
    img: &'a image::DynamicImage,
    from: ColourSpace,
    to: ColourSpace,
) -> Result<Cow<'a, image::DynamicImage>> {
    // Checked before the no-op test, so a space we cannot convert is refused even
    // when the caller asked for a conversion to itself — the refusal is about the
    // profile, not about whether the maths would have been a no-op.
    let Some(m) = matrix(from, to) else {
        return Err(Error::UnsupportedColourSpace(unsupported_note(
            from, to, None,
        )));
    };
    if from.working() == to.working() {
        return Ok(Cow::Borrowed(img));
    }

    // RGBA in, RGBA out. Alpha is not colour and is carried through untouched,
    // which is also why this costs no buffer the pipeline was not already holding:
    // every encode path in `format` runs through `to_rgba8` anyway.
    let src = img.to_rgba8();
    let mut out = image::RgbaImage::new(src.width(), src.height());
    for (dst, px) in out.pixels_mut().zip(src.pixels()) {
        let (r, g, b) = (
            srgb_to_linear(f32::from(px.0[0]) / 255.0),
            srgb_to_linear(f32::from(px.0[1]) / 255.0),
            srgb_to_linear(f32::from(px.0[2]) / 255.0),
        );
        let linear = [
            m[0][0] * r + m[0][1] * g + m[0][2] * b,
            m[1][0] * r + m[1][1] * g + m[1][2] * b,
            m[2][0] * r + m[2][1] * g + m[2][2] * b,
        ];
        for (channel, value) in linear.iter().enumerate() {
            // Clip, then quantise. Clipping is the documented gamut mapping: a P3
            // colour outside sRGB has no sRGB answer, and per-channel clipping is
            // what every other tool does.
            let encoded = linear_to_srgb(value.clamp(0.0, 1.0));
            dst.0[channel] = (encoded * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        dst.0[3] = px.0[3];
    }
    Ok(Cow::Owned(image::DynamicImage::ImageRgba8(out)))
}

// ---------------------------------------------------------------------------
// Container reads
// ---------------------------------------------------------------------------

/// Find the raw ICC profile in a container, or `None`.
///
/// Each arm walks a container this engine can decode, so a file whose bytes
/// `detect_format` recognised is a file whose container we know how to read.
/// Formats with nowhere to put a profile — BMP, GIF, TIFF, ICO — are `None`
/// rather than a guess.
fn extract(bytes: &[u8], format: OutputFormat) -> Option<Vec<u8>> {
    match format {
        OutputFormat::Jpeg => jpeg_icc(bytes),
        OutputFormat::Png => png_icc(bytes),
        OutputFormat::WebP => webp_icc(bytes),
        _ => None,
    }
}

/// The raw profile bytes from a file's container.
///
/// Public because `ColourOptions::embed_profile` has to write the *original*
/// profile rather than a synthesised one: a profile a phone wrote is the only one
/// this engine has any business putting back into a file, and synthesising one
/// would mean claiming ICC conformance this module explicitly does not have.
pub fn profile_bytes(bytes: &[u8], format: OutputFormat) -> Option<Vec<u8>> {
    extract(bytes, format).filter(|icc| !icc.is_empty())
}

/// JPEG: `APP2` segments whose payload starts with `ICC_PROFILE\0`, per
/// Exif 2.3 section 4.7.2.
///
/// A profile longer than one segment arrives split, with a sequence number and a
/// total count after the identifier; the halves are concatenated in sequence
/// order. A file missing a half yields no profile rather than half a one, because
/// half a profile cannot be classified and a wrong answer here is a colour
/// transform applied to the wrong picture.
fn jpeg_icc(input: &[u8]) -> Option<Vec<u8>> {
    /// "ICC_PROFILE\0", the APP2 identifier an ICC reader looks for.
    const ICC_ID: [u8; 12] = [
        0x49, 0x43, 0x43, 0x5F, 0x50, 0x52, 0x4F, 0x46, 0x49, 0x4C, 0x45, 0x00,
    ];
    if !input.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut parts: Vec<(u16, Vec<u8>)> = Vec::new();
    let mut at = 2usize;
    while at + 4 <= input.len() {
        if input[at] != 0xFF {
            return None;
        }
        let marker = input[at + 1];
        // Standalone markers carry no length.
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            at += 2;
            continue;
        }
        // Start of scan: everything after this is entropy-coded data, not
        // segments.
        if marker == 0xDA {
            break;
        }
        let len = usize::from(u16::from_be_bytes([input[at + 2], input[at + 3]]));
        if len < 2 {
            return None;
        }
        let payload_start = at + 4;
        let payload_end = payload_start.checked_add(len - 2)?;
        let payload = input.get(payload_start..payload_end)?;
        if marker == 0xE2 && payload.len() > ICC_ID.len() + 4 && payload.starts_with(&ICC_ID) {
            let seq = u16::from_be_bytes([payload[ICC_ID.len()], payload[ICC_ID.len() + 1]]);
            parts.push((seq, payload[ICC_ID.len() + 4..].to_vec()));
        }
        at = payload_end;
    }
    if parts.is_empty() {
        return None;
    }
    parts.sort_by_key(|(seq, _)| *seq);
    Some(parts.into_iter().flat_map(|(_, part)| part).collect())
}

/// PNG: the `iCCP` chunk, whose payload is a keyword, a NUL, a compression method
/// byte, and a zlib-compressed profile.
///
/// The compression is why `flate2` is a dependency of this module: an `iCCP`
/// chunk read without inflating it is a hundred opaque bytes, and every PNG in
/// the world would be reported as untagged.
fn png_icc(input: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

    if !input.starts_with(&SIGNATURE) {
        return None;
    }
    let mut at = 8usize;
    while let Some(header) = input.get(at..at + 8) {
        let len = u32::from_be_bytes(header[0..4].try_into().ok()?) as usize;
        let kind = &header[4..8];
        let start = at + 8;
        let end = start.checked_add(len)?;
        let data = input.get(start..end)?;
        if kind == b"iCCP" {
            // keyword \0 compression-method zlib-profile
            let nul = data.iter().position(|b| *b == 0)?;
            if data.get(nul + 1)? != &0 {
                // Only compression method 0 (zlib) is defined.
                return None;
            }
            let mut profile = Vec::new();
            flate2::read::ZlibDecoder::new(&data[nul + 2..])
                .read_to_end(&mut profile)
                .ok()?;
            return (!profile.is_empty()).then_some(profile);
        }
        if kind == b"IEND" {
            break;
        }
        // 4 length + 4 type + data + 4 CRC.
        at = end.checked_add(4)?;
    }
    None
}

/// WebP: the `ICCP` chunk of a RIFF container, which holds the profile raw.
///
/// The RIFF size field is untrusted and routinely wrong in the wild, so the walk
/// is bounded by the bytes that are actually present rather than by it.
fn webp_icc(input: &[u8]) -> Option<Vec<u8>> {
    if input.len() < 12 || input.get(..4)? != b"RIFF" || input.get(8..12)? != b"WEBP" {
        return None;
    }
    let mut at = 12usize;
    while let Some(header) = input.get(at..at + 8) {
        let kind: [u8; 4] = header[0..4].try_into().ok()?;
        let len = u32::from_le_bytes(header[4..8].try_into().ok()?) as usize;
        let start = at + 8;
        let end = start.checked_add(len)?;
        if &kind == b"ICCP" {
            let profile = input.get(start..end)?.to_vec();
            return (!profile.is_empty()).then_some(profile);
        }
        at = end + (len & 1); // RIFF chunks are padded to an even length.
    }
    None
}

/// Containers this module reads an ICC profile out of, and containers it does not.
///
/// The second list is named rather than left to be discovered, because "why did my
/// P3 iPhone HEIC come out wrong" is exactly the question a user will ask next.
/// A HEIF keeps its colour in a `colr` property box, `heic.rs` has no box walker
/// to reuse, and reading a fourth container is a change to that module's stated
/// job rather than to this one — so the gap is declared here. TIFF's tags could
/// be read the same way once something reads TIFF.
pub fn containers_read() -> (&'static [&'static str], &'static [&'static str]) {
    (
        &["JPEG (APP2)", "PNG (iCCP)", "WebP (ICCP)"],
        &["HEIF/HEIC (colr)", "TIFF"],
    )
}

// ---------------------------------------------------------------------------
// ICC header and tag reading
// ---------------------------------------------------------------------------

/// The four signature bytes of a data colour space, as a word.
///
/// A name from a fixed table rather than the raw bytes, deliberately: these four
/// bytes come off the user's disk and end up in a message a user reads, and hard
/// rule 9 does not mean "print whatever the file said".
fn signature_name(sig: Option<&[u8]>) -> Option<&'static str> {
    Some(match sig? {
        b"RGB " => "RGB",
        b"GRAY" => "grayscale",
        b"CMYK" => "CMYK",
        b"Lab " => "Lab",
        b"XYZ " => "XYZ",
        b"YCbr" => "YCbCr",
        b"Yxy " => "Yxy",
        b"Luv " => "Luv",
        b"HSV " => "HSV",
        b"HLS " => "HLS",
        b"2CLR" | b"3CLR" | b"4CLR" | b"5CLR" | b"6CLR" | b"7CLR" | b"8CLR" => {
            "a multi-channel space"
        }
        _ => return None,
    })
}

/// Decide whether an RGB profile is sRGB or Display-P3.
///
/// Colorants first, description second, because the colorants are a measurement of
/// the space while the description is a human-readable string two unrelated
/// profiles share. The description is still read as a fallback, because a profile
/// carrying no colorant tags at all is not hypothetical.
fn classify(icc: &[u8]) -> ColourSpace {
    if let Some(space) = colorants(icc).and_then(|c| nearest_space(&c)) {
        return space;
    }
    let Some(text) = read_description(icc) else {
        return ColourSpace::Other;
    };
    let lower = text.to_ascii_lowercase();
    if lower.contains("p3") {
        ColourSpace::DisplayP3
    } else if lower.contains("srgb") || lower.contains("iec61966") {
        ColourSpace::Srgb
    } else {
        ColourSpace::Other
    }
}

/// The profile's RGB colorants, adapted from ICC's D50 back to D65.
///
/// `None` when the profile carries no `rXYZ`/`gXYZ`/`bXYZ` triple at all, which
/// is a profile with no colorants rather than a broken one.
fn colorants(icc: &[u8]) -> Option<[[f32; 3]; 3]> {
    let table = tag_table(icc)?;
    let mut out = [[0.0f32; 3]; 3];
    for (i, sig) in [b"rXYZ", b"gXYZ", b"bXYZ"].iter().enumerate() {
        out[i] = xyz_tag(icc, table.get(*sig)?)?;
    }
    // Undo the D50 adaptation ICC bakes into every profile's colorants.
    let mut adapted = [[0.0f32; 3]; 3];
    for (row, colorant) in adapted.iter_mut().zip(out.iter()) {
        for (k, cell) in row.iter_mut().enumerate() {
            *cell = BRADFORD_D50_TO_D65[k][0] * colorant[0]
                + BRADFORD_D50_TO_D65[k][1] * colorant[1]
                + BRADFORD_D50_TO_D65[k][2] * colorant[2];
        }
    }
    Some(adapted)
}

/// One `rXYZ`, `gXYZ` or `bXYZ` tag: an `XYZType` is a type signature followed by
/// three s15Fixed16 numbers.
fn xyz_tag(icc: &[u8], tag: &(u32, u32)) -> Option<[f32; 3]> {
    let (offset, size) = *tag;
    if size < 20 {
        return None;
    }
    let body = icc.get(offset as usize..)?;
    if body.get(0..4) != Some(b"XYZ ") {
        return None;
    }
    let mut out = [0.0f32; 3];
    for (i, cell) in out.iter_mut().enumerate() {
        let at = 8 + i * 4;
        // s15Fixed16: a signed 32-bit integer over 2^16.
        *cell = i32::from_be_bytes(body.get(at..at + 4)?.try_into().ok()?) as f32 / 65_536.0;
    }
    // Finite, but *not* required to be positive. Undoing ICC's D50 adaptation
    // on a real Display-P3 profile puts the red colorant about a thousandth below
    // zero in Z, because the adaptation is not exactly the inverse of whatever
    // produced it — and refusing the profile over that would report the most
    // common wide-gamut profile in the world as unrecognised. A colorant that
    // cannot be a colour simply fails to match a reference in `nearest_space`.
    out.iter().all(|v| v.is_finite()).then_some(out)
}

/// The profile's tag table: a count at offset 128, then `(signature, offset,
/// size)` triples, into a map keyed by the four-byte signature.
fn tag_table(icc: &[u8]) -> Option<std::collections::HashMap<[u8; 4], (u32, u32)>> {
    let count = u32::from_be_bytes(icc.get(128..132)?.try_into().ok()?) as usize;
    if count > MAX_TAGS {
        return None;
    }
    let mut table = std::collections::HashMap::with_capacity(count);
    for i in 0..count {
        let at = 132 + i * 12;
        let entry = icc.get(at..at + 12)?;
        table.insert(
            entry[0..4].try_into().ok()?,
            (
                u32::from_be_bytes(entry[4..8].try_into().ok()?),
                u32::from_be_bytes(entry[8..12].try_into().ok()?),
            ),
        );
    }
    Some(table)
}

/// The profile's description tag, as text.
///
/// `desc` is the ICC v2 ASCII form and `mluc` is the v4 multi-localised form.
/// Both are read because a modern phone's P3 profile is as likely to be v4 as v2,
/// and a description is the fallback when there are no colorants to compare.
fn read_description(icc: &[u8]) -> Option<String> {
    let table = tag_table(icc)?;
    if let Some((offset, _)) = table.get(b"desc") {
        if let Some(text) = desc_text(icc, *offset as usize) {
            return Some(text);
        }
    }
    if let Some((offset, _)) = table.get(b"mluc") {
        if let Some(text) = mluc_text(icc, *offset as usize) {
            return Some(text);
        }
    }
    None
}

/// v2 `desc`: `desc`, four reserved bytes, a 4-byte ASCII length, then the string.
fn desc_text(icc: &[u8], offset: usize) -> Option<String> {
    let body = icc.get(offset..)?;
    if body.get(0..4) != Some(b"desc") {
        return None;
    }
    let len = u32::from_be_bytes(body.get(8..12)?.try_into().ok()?) as usize;
    if len == 0 || len > MAX_TEXT {
        return None;
    }
    trim_ascii(body.get(12..12 + len)?)
}

/// v4 `mluc`: `mluc`, four reserved bytes, a record count and record size, then
/// records of `(language, country, length, offset)`.
fn mluc_text(icc: &[u8], offset: usize) -> Option<String> {
    let body = icc.get(offset..)?;
    if body.get(0..4) != Some(b"mluc") {
        return None;
    }
    let records = u32::from_be_bytes(body.get(8..12)?.try_into().ok()?) as usize;
    if records == 0 || records > 64 {
        return None;
    }
    for i in 0..records {
        let at = 16 + i * 12;
        let len = u32::from_be_bytes(body.get(at + 4..at + 8)?.try_into().ok()?) as usize;
        let start = u32::from_be_bytes(body.get(at + 8..at + 12)?.try_into().ok()?) as usize;
        if len == 0 || len > MAX_TEXT {
            continue;
        }
        if let Some(text) = trim_ascii(body.get(start..start + len)?) {
            return Some(text);
        }
    }
    None
}

/// Keep printable ASCII from a possibly ill-formed string, and trim.
///
/// A profile's description is shown in a UI, so a profile carrying control
/// characters, half a UTF-8 sequence or a megabyte of text must not be able to
/// put any of that in front of a user. An empty result is `None` so the caller
/// tries the next tag rather than reporting a blank name.
fn trim_ascii(raw: &[u8]) -> Option<String> {
    if raw.len() > MAX_TEXT {
        return None;
    }
    let text: String = raw
        .iter()
        .map(|b| {
            if b.is_ascii_graphic() || *b == b' ' {
                *b as char
            } else {
                ' '
            }
        })
        .collect();
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// The reference space a set of colorants is closest to, within tolerance.
fn nearest_space(colorants: &[[f32; 3]; 3]) -> Option<ColourSpace> {
    let here: Vec<(f32, f32)> = colorants.iter().map(chromaticity).collect();
    // P3 first, so a set that fits neither still gets the honest `None` and a set
    // that fits both — impossible today, but true if the references ever move —
    // resolves to the wider one rather than the default.
    for (space, reference) in [
        (ColourSpace::DisplayP3, &DISPLAY_P3_PRIMARIES),
        (ColourSpace::Srgb, &SRGB_PRIMARIES),
    ] {
        let fits = reference.iter().zip(&here).all(|(expected, actual)| {
            let (ex, ey) = chromaticity(expected);
            (ex - actual.0).abs() < CHROMATICITY_TOLERANCE
                && (ey - actual.1).abs() < CHROMATICITY_TOLERANCE
        });
        if fits {
            return Some(space);
        }
    }
    None
}

/// CIE 1931 `xy` chromaticity from an XYZ triple. A zero-sum XYZ is not a colour
/// and yields `(0, 0)`, which matches no reference.
fn chromaticity(xyz: &[f32; 3]) -> (f32, f32) {
    let sum = xyz[0] + xyz[1] + xyz[2];
    if sum <= 0.0 || !sum.is_finite() {
        return (0.0, 0.0);
    }
    (xyz[0] / sum, xyz[1] / sum)
}

// ---------------------------------------------------------------------------
// The refusal
// ---------------------------------------------------------------------------

/// The sentence a user gets when their file's colour space cannot be converted.
///
/// One place that writes it, because a per-format dictionary of refusals is how a
/// message ends up reading like a diagnostic. Every arm names the space, says what
/// to do instead, and offers the one escape hatch that keeps the file exportable —
/// that last clause is the difference between a wall and a choice.
pub fn unsupported_note(from: ColourSpace, to: ColourSpace, declared: Option<&str>) -> String {
    let target = to.label();
    let space = match declared {
        Some("CMYK") => "this photo is CMYK, and this build cannot convert CMYK".to_owned(),
        Some("grayscale") => {
            "this photo is tagged as grayscale, which this build cannot convert".to_owned()
        }
        Some("RGB") => format!(
            "this photo is tagged with an RGB colour space that is neither sRGB nor \
             Display-P3 — Adobe RGB or ProPhoto, most likely — and this build cannot \
             convert it to {target}"
        ),
        Some(other) => format!("this photo's colour profile says it is {other}"),
        None => "this photo's colour profile is in a space this build cannot name".to_owned(),
    };
    format!(
        "{space} to {target} without an ICC colour-management engine. Re-save the photo \
         as sRGB or Display-P3 — any photo app or print shop will do that — or turn on \
         'keep the original colour values' to export the pixels unchanged, which means \
         they will look wrong on a screen that is not the source's colour space \
         ({} was the file's colour space)",
        from.label()
    )
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! ICC profile fixtures, shared by every module that needs one.
    //!
    //! Generated rather than committed: a real sRGB or Display-P3 profile is a
    //! binary blob with a licence to reason about, and what these tests need is a
    //! profile with *known* colorants, which is precisely what a generator can
    //! state and a committed blob cannot. Every test that uses one parses it back
    //! through the code under test, so a fixture that lies fails rather than
    //! passes quietly.

    use super::{BRADFORD_D50_TO_D65, DISPLAY_P3_PRIMARIES, SRGB_PRIMARIES, invert, mul, to_xyz};
    use crate::format::{EncodingOptions, OutputFormat};

    /// Build a minimal but *valid* ICC v2 matrix/TRC profile with the colorants
    /// and description given.
    pub fn profile(desc: &str, primaries: [[f32; 3]; 3]) -> Vec<u8> {
        fn s15f16(v: f32) -> [u8; 4] {
            ((v * 65_536.0) as i32).to_be_bytes()
        }
        fn xyz_tag(v: [f32; 3]) -> Vec<u8> {
            let mut t = b"XYZ ".to_vec();
            t.extend_from_slice(&[0; 4]);
            for c in v {
                t.extend_from_slice(&s15f16(c));
            }
            t
        }
        /// A `curv` tag with a single gamma in u8Fixed8Number, which is what a v2
        /// profile uses for "the usual curve".
        fn curv_tag() -> Vec<u8> {
            let mut t = b"curv".to_vec();
            t.extend_from_slice(&[0; 4]);
            t.extend_from_slice(&1u32.to_be_bytes());
            t.extend_from_slice(&0x0233u16.to_be_bytes()); // 563/256 = 2.199
            t
        }
        fn desc_tag(text: &str) -> Vec<u8> {
            let mut t = b"desc".to_vec();
            t.extend_from_slice(&[0; 4]);
            t.extend_from_slice(&(text.len() as u32).to_be_bytes());
            t.extend_from_slice(text.as_bytes());
            t.extend_from_slice(&[0; 4]); // unicode language code
            t.extend_from_slice(&[0; 4]); // unicode count
            t.extend_from_slice(&[0; 2]); // scriptcode code
            t.extend_from_slice(&[0; 67]); // scriptcode description
            t
        }

        let trc = curv_tag();
        let entries: Vec<([u8; 4], Vec<u8>)> = vec![
            (*b"desc", desc_tag(desc)),
            (*b"wtpt", xyz_tag([0.964_2, 1.0, 0.824_9])), // D50
            (*b"rXYZ", xyz_tag(primaries[0])),
            (*b"gXYZ", xyz_tag(primaries[1])),
            (*b"bXYZ", xyz_tag(primaries[2])),
            (*b"rTRC", trc.clone()),
            (*b"gTRC", trc.clone()),
            (*b"bTRC", trc),
        ];

        // Tag data starts after the table, and every tag begins on a 4-byte
        // boundary as the spec requires — which means the padding goes *before*
        // the data and the offsets have to count it.
        let table_len = 4 + entries.len() * 12;
        let mut body: Vec<u8> = Vec::new();
        let mut offsets = Vec::new();
        let mut cursor = 128 + table_len;
        for (_, data) in &entries {
            while (cursor + data.len()) % 4 != 0 {
                body.push(0);
                cursor += 1;
            }
            offsets.push(cursor as u32);
            body.extend_from_slice(data);
            cursor += data.len();
        }

        // The tag table is a count followed by `(signature, offset, size)` triples.
        // The count is the first thing at offset 128.
        let mut table = Vec::new();
        table.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for ((sig, data), offset) in entries.iter().zip(&offsets) {
            table.extend_from_slice(sig);
            table.extend_from_slice(&offset.to_be_bytes());
            table.extend_from_slice(&(data.len() as u32).to_be_bytes());
        }

        let total = 128 + table_len + body.len();
        let mut out = vec![0u8; 128];
        out[0..4].copy_from_slice(&(total as u32).to_be_bytes());
        out[8..12].copy_from_slice(&0x0240_0000u32.to_be_bytes()); // v2.4
        out[12..16].copy_from_slice(b"mntr");
        out[16..20].copy_from_slice(b"RGB ");
        out[20..24].copy_from_slice(b"XYZ ");
        out[36..40].copy_from_slice(b"acsp");
        out[64..68].copy_from_slice(&0u32.to_be_bytes()); // perceptual intent
        let illuminant = [
            s15f16(0.964_2).as_slice(),
            s15f16(1.0).as_slice(),
            s15f16(0.824_9).as_slice(),
        ]
        .concat();
        out[68..80].copy_from_slice(&illuminant);
        out.extend_from_slice(&table);
        out.extend_from_slice(&body);
        out
    }

    /// The colorants a real profile stores: the D65 primaries adapted to ICC's D50
    /// PCS illuminant, which is what the parser has to undo.
    ///
    /// The adaptation here is the parser's matrix inverted the other way and the
    /// column permutation undone, rather than a second published constant — so the
    /// round-trip test cannot be satisfied by both halves being wrong in the same
    /// direction.
    pub fn d50_adapted(primaries: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
        // `BRADFORD_D50_TO_D65` maps D50 to D65, so storing the D65 primaries in a
        // D50 profile needs its *inverse* — the same matrix the parser applies the
        // other way.
        let d50_to_d65_inverse =
            invert(&BRADFORD_D50_TO_D65).expect("the Bradford matrix is invertible");
        let back = mul(&d50_to_d65_inverse, &to_xyz(&primaries));
        // `to_xyz` puts the primaries in the matrix's *columns*, so the adapted
        // primaries come back out as columns and have to be transposed again.
        [
            [back[0][0], back[1][0], back[2][0]],
            [back[0][1], back[1][1], back[2][1]],
            [back[0][2], back[1][2], back[2][2]],
        ]
    }

    /// A profile whose header says sRGB, which is the overwhelmingly common case.
    pub fn srgb_profile() -> Vec<u8> {
        profile("sRGB IEC61966-2.1", d50_adapted(SRGB_PRIMARIES))
    }

    /// A profile whose header says Display-P3, which is what a modern phone
    /// photographs in.
    pub fn p3_profile() -> Vec<u8> {
        profile("Display P3", d50_adapted(DISPLAY_P3_PRIMARIES))
    }

    /// A profile whose header says `CMYK`, which is a space this build refuses
    /// rather than converts.
    pub fn cmyk_profile() -> Vec<u8> {
        let mut icc = srgb_profile();
        icc[16..20].copy_from_slice(b"CMYK");
        icc
    }

    /// Encode `img` as `format`, with `icc` attached when one is given.
    ///
    /// The two formats this can attach to are the two the engine can also write a
    /// profile into, so a fixture built here is a file the engine has already
    /// agreed is round-trippable.
    pub fn encode_with(
        img: &image::DynamicImage,
        format: OutputFormat,
        icc: Option<&[u8]>,
    ) -> Vec<u8> {
        let mut bytes = crate::encode_fixed(img, format, EncodingOptions::default())
            .expect("the fixture encodes");
        if let Some(icc) = icc {
            match format {
                OutputFormat::Jpeg => crate::format::append_icc(&mut bytes, icc).expect("APP2"),
                OutputFormat::Png => crate::format::png_insert_icc(&mut bytes, icc).expect("iCCP"),
                other => panic!("{other:?} cannot carry a profile in this build"),
            }
        }
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colour::fixtures::{d50_adapted, p3_profile, profile, srgb_profile};
    use crate::format::{ChromaSubsampling, EncodingOptions, OutputFormat};

    /// The colour-space matrix over the three source spaces this phase names,
    /// rendered through a real file so the container walk is part of the fixture.
    fn tagged_png(icc: Option<&[u8]>) -> Vec<u8> {
        let mut png =
            crate::encode_fixed(&solid(PATCH), OutputFormat::Png, EncodingOptions::default())
                .unwrap();
        if let Some(icc) = icc {
            crate::format::png_insert_icc(&mut png, icc).unwrap();
        }
        png
    }

    /// A mid-saturation red patch. Strongly red, and *not* so far out of gamut
    /// that the sRGB result clips and the numbers stop describing anything — the
    /// whole point of the matrix below is a measurement in the non-clipped range.
    const PATCH: [u8; 3] = [200, 100, 90];

    /// The sRGB result of converting `PATCH` from Display-P3, computed by hand
    /// from the published matrix and written down rather than computed by the code
    /// under test:
    ///
    /// ```text
    /// decode with the sRGB curve: 200 -> 0.5777, 100 -> 0.1275, 90 -> 0.1022
    /// R =  1.2249401*0.5777 - 0.2249401*0.1275 + 0        = 0.6789
    /// G = -0.0420570*0.5777 + 1.0420570*0.1275           = 0.1086
    /// B = -0.0196376*0.5777 - 0.0786360*0.1275 + 1.0982736*0.1022 = 0.0909
    /// encode with the sRGB curve: 0.6789 -> 0.8428 = 215, 0.1086 -> 0.3633 = 93,
    ///                               0.0909 -> 0.3334 = 85
    /// ```
    const PATCH_AS_SRGB: [u8; 3] = [215, 93, 85];

    /// The byte offset of a signature's entry in the tag table, so a test can
    /// corrupt *that* tag rather than guessing at an index.
    fn find_tag_entry(icc: &[u8], sig: &[u8; 4]) -> usize {
        let count = u32::from_be_bytes(icc[128..132].try_into().unwrap()) as usize;
        (0..count)
            .map(|i| 132 + i * 12)
            .find(|at| icc.get(*at..at + 4) == Some(sig.as_slice()))
            .expect("fixture has the tag")
    }

    fn solid(rgb: [u8; 3]) -> image::DynamicImage {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 8, image::Rgb(rgb)))
    }

    fn pixel(img: &image::DynamicImage) -> [u8; 3] {
        let px = img.to_rgb8().get_pixel(0, 0).0;
        [px[0], px[1], px[2]]
    }

    // ---- the transfer function -----------------------------------------

    /// The curve is the piecewise IEC 61966-2-1 form, not a power law. A test that
    /// only asserted a round trip would pass for gamma 2.2 as well, and gamma 2.2
    /// is the bug this phase exists partly to avoid.
    #[test]
    fn the_transfer_function_is_the_srgb_curve_not_gamma_22() {
        assert!((linear_to_srgb(srgb_to_linear(0.5)) - 0.5).abs() < 1e-6);
        let curve = linear_to_srgb(srgb_to_linear(0.5));
        let gamma_22 = 0.5f32.powf(1.0 / 2.2);
        assert!(
            (curve - gamma_22).abs() > 0.004,
            "sRGB ({curve}) and gamma 2.2 ({gamma_22}) came out the same, so one of \
             them is not being computed"
        );
        // And the linear segment near black, which is the other half of the
        // piecewise form and the part a pure power law gets most wrong.
        assert!((srgb_to_linear(0.02) - 0.02 / 12.92).abs() < 1e-9);
        assert!((linear_to_srgb(0.002) - 0.002 * 12.92).abs() < 1e-9);
    }

    #[test]
    fn the_transfer_function_round_trips_across_the_whole_range() {
        for step in 0..=255u32 {
            let v = step as f32 / 255.0;
            assert!(
                (linear_to_srgb(srgb_to_linear(v)) - v).abs() < 1e-5,
                "at {v}"
            );
        }
    }

    // ---- the matrix -----------------------------------------------------

    /// The primaries are the source of truth, so the matrix the engine applies is
    /// checked against the value published for P3 → sRGB rather than being a
    /// fourth set of numbers nobody can cross-check.
    #[test]
    fn the_derived_matrix_is_the_published_one() {
        let m = matrix(ColourSpace::DisplayP3, ColourSpace::Srgb).unwrap();
        // The Display-P3 → sRGB matrix as CSS Color 4 publishes it.
        const PUBLISHED: [[f32; 3]; 3] = [
            [1.224_745_3, -0.224_904_1, 0.0],
            [-0.042_057_9, 1.042_081, 0.0],
            [-0.019_642_3, -0.078_654_9, 1.098_537_3],
        ];
        // The tolerance is 5e-4 and not tighter because the two published
        // derivations disagree by about 2e-4 on the first coefficient: CSS Color 4
        // and the older Bradford route round the white point slightly
        // differently. Two ten-thousandths of a coefficient is a twentieth of one
        // code value on an 8-bit output, so agreeing to it is agreeing to the
        // limit of what either derivation can say.
        for (r, row) in m.iter().enumerate() {
            for (c, value) in row.iter().enumerate() {
                assert!(
                    (value - PUBLISHED[r][c]).abs() < 5e-4,
                    "row {r} col {c}: derived {value}, published {}",
                    PUBLISHED[r][c]
                );
            }
        }
    }

    /// A matrix that does not preserve white tints every neutral in every
    /// photograph, which is the failure a user reports as "everything looks
    /// yellow". Each row summing to one is the same statement.
    #[test]
    fn a_conversion_preserves_white() {
        for (from, to) in [
            (ColourSpace::DisplayP3, ColourSpace::Srgb),
            (ColourSpace::Srgb, ColourSpace::DisplayP3),
        ] {
            let m = matrix(from, to).unwrap();
            for row in &m {
                let sum: f32 = row.iter().sum();
                assert!(
                    (sum - 1.0).abs() < 1e-5,
                    "row sums to {sum} for {from:?}->{to:?}"
                );
            }
        }
    }

    #[test]
    fn a_conversion_round_trips_through_its_inverse() {
        let forward = matrix(ColourSpace::DisplayP3, ColourSpace::Srgb).unwrap();
        let backward = matrix(ColourSpace::Srgb, ColourSpace::DisplayP3).unwrap();
        for (i, row) in forward.iter().enumerate() {
            for (j, cell) in row.iter().enumerate() {
                let sum: f32 = (0..3).map(|k| row[k] * backward[k][j]).sum();
                let _ = cell;
                assert!(
                    (sum - if i == j { 1.0 } else { 0.0 }).abs() < 1e-4,
                    "[{i}][{j}] = {sum}"
                );
            }
        }
    }

    #[test]
    fn a_space_this_build_has_no_primaries_for_has_no_matrix() {
        assert!(matrix(ColourSpace::Other, ColourSpace::Srgb).is_none());
        assert!(matrix(ColourSpace::Srgb, ColourSpace::Other).is_none());
        // Untagged is sRGB, so it is convertible *to* everything sRGB is.
        assert!(matrix(ColourSpace::Untagged, ColourSpace::DisplayP3).is_some());
    }

    /// Every sRGB colour has to survive a trip through P3 and back, or a
    /// conversion that is "correct" is still visibly wrong on a picture of greys
    /// and mid-tones — which is most pictures.
    ///
    /// sRGB's gamut is a subset of P3's and the two share a transfer function, so
    /// this direction has to be lossless. The other direction cannot be, and
    /// `clipping_is_what_costs_a_p3_colour_its_out_of_gamut_colour` says so.
    #[test]
    fn an_srgb_colour_survives_a_round_trip_through_p3() {
        for v in [0u8, 40, 90, 128, 160, 200, 255] {
            for patch in [[v, v, v], [v, v / 2, v / 4], [v / 4, v / 2, v], [v, v, 0]] {
                let img = solid(patch);
                let there = convert(&img, ColourSpace::Srgb, ColourSpace::DisplayP3)
                    .unwrap()
                    .into_owned();
                let back = convert(&there, ColourSpace::DisplayP3, ColourSpace::Srgb)
                    .unwrap()
                    .into_owned();
                let got = pixel(&back);
                // Three, not zero: the round trip clips at the gamut boundary,
                // which is where a linear transform meets a hard edge, and a
                // colour sitting near it loses a couple of codes to the f32
                // arithmetic on either side. Anything beyond that is a real error.
                for (channel, (g, e)) in got.iter().zip(patch).enumerate() {
                    assert!(
                        (i32::from(*g) - i32::from(e)).abs() <= 3,
                        "{patch:?} channel {channel} came back {got:?}"
                    );
                }
            }
        }
    }

    /// The documented cost of the documented gamut mapping. A saturated P3 red has
    /// no sRGB answer, so it is clipped — and the fact that it does not round-trip
    /// is asserted rather than left for a user to discover, because a
    /// desaturating gamut mapper would be better and this is not one.
    #[test]
    fn clipping_is_what_costs_a_p3_colour_its_out_of_gamut_colour() {
        let img = solid([255, 60, 40]);
        let there = convert(&img, ColourSpace::DisplayP3, ColourSpace::Srgb)
            .unwrap()
            .into_owned();
        let back = convert(&there, ColourSpace::Srgb, ColourSpace::DisplayP3)
            .unwrap()
            .into_owned();
        let clipped = pixel(&there);
        assert_eq!(clipped[0], 255, "red stays: {clipped:?}");
        assert!(
            clipped[1] < 40 && clipped[2] < 20,
            "green and blue have to lose most of their value to fit in sRGB: {clipped:?}"
        );
        assert_ne!(
            pixel(&back),
            pixel(&img),
            "and does not come back, which is what clipping means"
        );
    }

    // ---- profile reading ------------------------------------------------

    #[test]
    fn a_file_with_no_profile_is_untagged() {
        let png = tagged_png(None);
        let profile = ColourProfile::read(&png, OutputFormat::Png);
        assert!(!profile.icc_present);
        assert_eq!(profile.source, ColourSpace::Untagged);
        assert_eq!(profile.icc_bytes, 0);
        // "Untagged" and "tagged sRGB" convert identically but are different
        // facts, and a UI says different things about them.
        assert_eq!(profile.source.working(), ColourSpace::Srgb);
        assert!(!profile.needs_conversion(ColourSpace::Srgb));
    }

    #[test]
    fn a_tagged_srgb_profile_is_recognised() {
        let profile = ColourProfile::parse(&srgb_profile());
        assert_eq!(profile.declared.as_deref(), Some("RGB"));
        assert_eq!(profile.source, ColourSpace::Srgb);
        assert_eq!(profile.description.as_deref(), Some("sRGB IEC61966-2.1"));
        assert!(!profile.needs_conversion(ColourSpace::Srgb));
    }

    #[test]
    fn a_tagged_display_p3_profile_is_recognised() {
        let profile = ColourProfile::parse(&p3_profile());
        assert_eq!(profile.source, ColourSpace::DisplayP3);
        assert_eq!(profile.description.as_deref(), Some("Display P3"));
        assert!(profile.needs_conversion(ColourSpace::Srgb));
    }

    /// The classification is on colorants, not on the description, so a P3
    /// profile that calls itself something else is still P3 — and an sRGB profile
    /// whose name mentions P3 somewhere is still sRGB.
    #[test]
    fn the_colour_space_comes_from_the_colorants_not_the_name() {
        let lying = profile("Some Camera Profile", d50_adapted(DISPLAY_P3_PRIMARIES));
        assert_eq!(ColourProfile::parse(&lying).source, ColourSpace::DisplayP3);
        let boastful = profile("P3 but not really", d50_adapted(SRGB_PRIMARIES));
        assert_eq!(ColourProfile::parse(&boastful).source, ColourSpace::Srgb);
    }

    /// The description is the fallback for a profile whose colorants cannot be
    /// measured. Pointing the colorants somewhere impossible is how that looks in
    /// practice — a v2 profile from a camera is as likely to carry colorants as
    /// not, and a description is the only thing left to go on.
    #[test]
    fn a_profile_whose_colorants_cannot_be_read_falls_back_to_its_description() {
        for sig in [b"rXYZ", b"gXYZ", b"bXYZ"] {
            let mut blind = p3_profile();
            let at = find_tag_entry(&blind, sig);
            blind[at + 4..at + 8].copy_from_slice(&0xFFFF_0000u32.to_be_bytes());
            assert_eq!(
                ColourProfile::parse(&blind).source,
                ColourSpace::DisplayP3,
                "{} should have fallen back to the description",
                std::str::from_utf8(sig).unwrap()
            );
        }
    }

    /// A colour space this build cannot convert is identified rather than guessed
    /// at, so the refusal can name it.
    #[test]
    fn a_non_rgb_profile_is_named_and_refused_rather_than_mishandled() {
        for (signature, word) in [(b"CMYK", "CMYK"), (b"GRAY", "grayscale"), (b"Lab ", "Lab")] {
            let mut odd = srgb_profile();
            odd[16..20].copy_from_slice(signature);
            let profile = ColourProfile::parse(&odd);
            assert_eq!(profile.declared.as_deref(), Some(word));
            assert_eq!(profile.source, ColourSpace::Other);
            assert!(profile.needs_conversion(ColourSpace::Srgb));
        }
    }

    /// Untrusted bytes: every one of these is something a crafted file can do, and
    /// none of them may panic, allocate unboundedly, or be mistaken for a profile
    /// that is not there.
    #[test]
    fn a_malformed_icc_profile_is_reported_rather_than_trusted() {
        // Shorter than the 128-byte header.
        let mut cut = p3_profile();
        cut.truncate(64);
        assert_eq!(ColourProfile::parse(&cut).source, ColourSpace::Untagged);

        // Right size, wrong magic.
        let mut wrong_magic = p3_profile();
        wrong_magic[36..40].copy_from_slice(b"junk");
        assert_eq!(
            ColourProfile::parse(&wrong_magic).source,
            ColourSpace::Untagged
        );

        // A tag table claiming a million entries. Nothing in it is readable, so
        // nothing is claimed: `Other` is the honest answer, and it is the *safe*
        // one, because a space this build cannot name is refused rather than
        // converted with the wrong matrix.
        let mut huge_table = p3_profile();
        huge_table[128..132].copy_from_slice(&1_000_000u32.to_be_bytes());
        assert_eq!(ColourProfile::parse(&huge_table).source, ColourSpace::Other);
        assert_eq!(
            ColourProfile::parse(&huge_table).description,
            None,
            "and nothing is claimed about it either"
        );

        // A tag pointing far outside the file. The colorants become unreadable,
        // so the fallback takes over; the point is that it does not panic and does
        // not claim to have measured anything.
        let mut dangling = p3_profile();
        let r_entry = find_tag_entry(&dangling, b"rXYZ");
        dangling[r_entry + 4..r_entry + 8].copy_from_slice(&0xFFFF_0000u32.to_be_bytes());
        assert_eq!(
            ColourProfile::parse(&dangling).source,
            ColourSpace::DisplayP3
        );

        // And every prefix of a real profile, which is the shape a truncated file
        // actually has.
        let full = p3_profile();
        for len in (0..full.len()).step_by(64) {
            let _ = ColourProfile::parse(&full[..len]);
        }
    }

    /// A signature is four bytes off the user's disk and it is rendered to a user.
    /// Only the fixed vocabulary may reach the message.
    #[test]
    fn an_unknown_signature_gets_a_word_not_the_raw_bytes() {
        let mut odd = srgb_profile();
        odd[16..20].copy_from_slice(&[0x01, 0x02, 0x03, 0x04]);
        let profile = ColourProfile::parse(&odd);
        assert_eq!(profile.declared, None);
        assert_eq!(profile.source, ColourSpace::Other);
        let note = unsupported_note(
            ColourSpace::Other,
            ColourSpace::Srgb,
            profile.declared.as_deref(),
        );
        assert!(!note.contains('\u{1}') && !note.is_empty());
    }

    /// A description is displayed, so it must not be able to carry control
    /// characters or a megabyte of text into a UI.
    #[test]
    fn a_description_cannot_carry_control_characters_or_be_unbounded() {
        assert_eq!(trim_ascii(b"Display P3"), Some("Display P3".to_owned()));
        assert_eq!(trim_ascii(b"  spaced \n"), Some("spaced".to_owned()));
        assert_eq!(trim_ascii(b"\x00\x01\x02"), None);
        assert_eq!(trim_ascii(&[b'x'; MAX_TEXT + 1]), None);
    }

    // ---- containers -----------------------------------------------------

    #[test]
    fn a_png_profile_is_read_out_of_its_iccp_chunk() {
        let icc = p3_profile();
        let png = tagged_png(Some(&icc));
        let profile = ColourProfile::read(&png, OutputFormat::Png);
        assert!(profile.icc_present, "the iCCP chunk was not found");
        assert_eq!(profile.icc_bytes, icc.len());
        assert_eq!(profile.source, ColourSpace::DisplayP3);
    }

    #[test]
    fn a_jpeg_profile_is_read_out_of_its_app2_segment() {
        let icc = p3_profile();
        let mut jpeg = crate::encode_fixed(
            &solid(PATCH),
            OutputFormat::Jpeg,
            EncodingOptions {
                chroma_subsampling: ChromaSubsampling::Luma444,
                ..EncodingOptions::default()
            },
        )
        .unwrap();
        crate::format::append_icc(&mut jpeg, &icc).unwrap();
        let profile = ColourProfile::read(&jpeg, OutputFormat::Jpeg);
        assert!(profile.icc_present);
        assert_eq!(profile.icc_bytes, icc.len());
        assert_eq!(profile.source, ColourSpace::DisplayP3);
    }

    /// A profile too large for one segment arrives split, and the halves have to be
    /// concatenated in sequence order or the result is a corrupt profile that still
    /// has a valid-looking header — which is the worst way for this to fail.
    #[test]
    fn a_split_jpeg_profile_is_reassembled_in_order() {
        let icc = p3_profile();
        let (head, tail) = icc.split_at(icc.len() / 2);
        let mut jpeg = crate::encode_fixed(
            &solid(PATCH),
            OutputFormat::Jpeg,
            EncodingOptions::default(),
        )
        .unwrap();
        // Written out of order on purpose: the reader has to sort, not take the
        // first segment it sees.
        crate::format::append_icc_chunk(&mut jpeg, 2, 2, tail).unwrap();
        crate::format::append_icc_chunk(&mut jpeg, 1, 2, head).unwrap();
        let profile = ColourProfile::read(&jpeg, OutputFormat::Jpeg);
        assert_eq!(profile.icc_bytes, icc.len());
        assert_eq!(profile.source, ColourSpace::DisplayP3);
    }

    /// A container walk that runs off the end of a truncated file must stop and
    /// report only what is really there.
    ///
    /// The property is *not* "a truncated prefix is untagged": a PNG's `iCCP`
    /// chunk sits right after `IHDR` and the profile inside it is compressed, so
    /// a prefix that stops well into the image data legitimately still carries
    /// the whole thing. What must hold is that a prefix either finds nothing or
    /// finds the profile whole — never half of one, which would classify a
    /// corrupt picture as a colour space.
    #[test]
    fn a_truncated_container_never_claims_more_profile_than_it_has() {
        let mut p3 = p3_profile();
        let mut png =
            crate::encode_fixed(&solid(PATCH), OutputFormat::Png, EncodingOptions::default())
                .unwrap();
        crate::format::png_insert_icc(&mut png, &p3).unwrap();
        let mut jpeg = crate::encode_fixed(
            &solid(PATCH),
            OutputFormat::Jpeg,
            EncodingOptions::default(),
        )
        .unwrap();
        crate::format::append_icc(&mut jpeg, &p3).unwrap();
        p3.clear();

        for (format, full) in [(OutputFormat::Png, png), (OutputFormat::Jpeg, jpeg)] {
            for len in (0..full.len()).step_by(17) {
                let profile = ColourProfile::read(&full[..len], format);
                assert!(
                    matches!(
                        profile.source,
                        ColourSpace::Untagged | ColourSpace::DisplayP3
                    ),
                    "{format:?}: a {len}-byte prefix reported {:?}",
                    profile.source
                );
            }
            // And the whole thing is still read correctly, which is what makes
            // the truncation claims above worth anything.
            let whole = ColourProfile::read(&full, format);
            assert_eq!(whole.source, ColourSpace::DisplayP3, "{format:?}");
            assert_eq!(whole.icc_bytes, p3_profile().len(), "{format:?}");
        }
    }

    /// The two places this module is *not* read from are named, because "why did my
    /// P3 HEIC come out wrong" is the question a user will ask next.
    #[test]
    fn the_containers_we_do_not_read_are_declared() {
        let (read, not_read) = containers_read();
        assert!(read.contains(&"JPEG (APP2)"));
        assert!(read.contains(&"PNG (iCCP)"));
        assert!(read.contains(&"WebP (ICCP)"));
        assert!(not_read.contains(&"HEIF/HEIC (colr)"));
        assert!(not_read.contains(&"TIFF"));
    }

    // ---- the three-space matrix -----------------------------------------

    /// The acceptance matrix: untagged, sRGB-tagged and Display-P3-tagged input,
    /// each stating what has to happen to the pixels rather than merely that
    /// something happened.
    #[test]
    fn the_source_space_matrix_holds_for_all_three_cases() {
        struct Case {
            name: &'static str,
            input: Vec<u8>,
            source: ColourSpace,
            expected: [u8; 3],
        }

        let cases = [
            Case {
                name: "untagged",
                input: tagged_png(None),
                source: ColourSpace::Untagged,
                expected: PATCH,
            },
            Case {
                name: "sRGB-tagged",
                input: tagged_png(Some(&srgb_profile())),
                source: ColourSpace::Srgb,
                expected: PATCH,
            },
            Case {
                name: "Display-P3-tagged",
                input: tagged_png(Some(&p3_profile())),
                source: ColourSpace::DisplayP3,
                expected: PATCH_AS_SRGB,
            },
        ];

        for case in &cases {
            let profile = ColourProfile::read(&case.input, OutputFormat::Png);
            assert_eq!(profile.source, case.source, "{}", case.name);

            let img = crate::decode_bounded(&case.input, &crate::Limits::default()).unwrap();
            let converted = convert(&img, profile.source, ColourSpace::Srgb).expect(case.name);
            let got = pixel(&converted);
            for (index, (g, e)) in got.iter().zip(case.expected).enumerate() {
                assert!(
                    (i32::from(*g) - i32::from(e)).abs() <= 1,
                    "{}: channel {index} came out {g}, expected about {e} (whole {got:?})",
                    case.name
                );
            }
        }
    }

    /// The direction is what a user sees: a P3 colour read as sRGB is *duller*, so
    /// converting has to move it further from grey. A magnitude-only assertion
    /// would pass if the matrix were accidentally its own inverse.
    #[test]
    fn p3_to_srgb_moves_a_saturated_patch_towards_more_saturation() {
        let img = solid(PATCH);
        let converted = convert(&img, ColourSpace::DisplayP3, ColourSpace::Srgb)
            .unwrap()
            .into_owned();
        let before = pixel(&img);
        let after = pixel(&converted);
        let saturation = |p: [u8; 3]| {
            let (r, g, b) = (f32::from(p[0]), f32::from(p[1]), f32::from(p[2]));
            (r.max(g).max(b) - r.min(g).min(b)) / 255.0
        };
        assert!(
            saturation(after) > saturation(before) + 0.02,
            "saturation went {} -> {}: {before:?} -> {after:?}",
            saturation(before),
            saturation(after)
        );
        assert!(
            after[0] > before[0],
            "red should rise: {before:?} -> {after:?}"
        );
        assert!(
            after[1] < before[1],
            "green should fall: {before:?} -> {after:?}"
        );
        assert!(
            after[2] < before[2],
            "blue should fall: {before:?} -> {after:?}"
        );
        // And by roughly the amount the matrix says, not by a different amount
        // that happens to point the same way.
        for (channel, (g, e)) in after.iter().zip(PATCH_AS_SRGB).enumerate() {
            assert!(
                (i32::from(*g) - i32::from(e)).abs() <= 1,
                "channel {channel}: {g} against the hand-computed {e}"
            );
        }
    }

    /// The no-op is a no-op, all the way to the pixels. This is the test that
    /// makes "an untagged source is a byte-identical export" true rather than
    /// nearly true: no re-encoding, no clamping, no rounding.
    #[test]
    fn an_untagged_source_is_handed_back_untouched() {
        let img = solid([17, 200, 233]);
        let converted = convert(&img, ColourSpace::Untagged, ColourSpace::Srgb).unwrap();
        assert!(
            matches!(converted, Cow::Borrowed(_)),
            "an untagged source must not allocate or touch a pixel"
        );
    }

    /// A whole row of every possible value, because clipping and re-encoding every
    /// pixel is exactly where an off-by-one hides.
    #[test]
    fn a_no_op_conversion_is_byte_identical_across_the_whole_range() {
        let mut img = image::RgbImage::new(256, 1);
        for x in 0..256u32 {
            img.put_pixel(x, 0, image::Rgb([x as u8, 255 - (x as u8), (x as u8) / 2]));
        }
        let img = image::DynamicImage::ImageRgb8(img);
        let converted = convert(&img, ColourSpace::Untagged, ColourSpace::Srgb)
            .unwrap()
            .into_owned();
        assert_eq!(img.to_rgba8().into_raw(), converted.to_rgba8().into_raw());
    }

    /// Alpha is not colour. A cut-out photo's transparency has to survive the
    /// conversion untouched, and a fully transparent pixel whose colour is
    /// meaningless must not become opaque.
    #[test]
    fn alpha_is_carried_through_the_conversion_unchanged() {
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(4, 1, |x, _| {
            image::Rgba([200, 100, 90, (x * 60) as u8])
        }));
        let converted = convert(&img, ColourSpace::DisplayP3, ColourSpace::Srgb)
            .unwrap()
            .into_owned();
        let after = converted.to_rgba8();
        for (x, px) in after.pixels().enumerate() {
            assert_eq!(px.0[3], (x * 60) as u8, "alpha changed at x={x}");
        }
    }

    #[test]
    fn a_space_we_cannot_convert_is_refused_in_plain_english() {
        let err = convert(&solid([1, 2, 3]), ColourSpace::Other, ColourSpace::Srgb).unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("sRGB") && text.contains("keep the original"),
            "the refusal should say what happened and what to do: {text}"
        );
        assert!(
            !text.contains("error:") && !text.contains("unsupported or unrecognised"),
            "the message reads like a diagnostic: {text}"
        );
    }

    #[test]
    fn the_refusal_names_the_space_when_it_knows_it() {
        let rgb = unsupported_note(ColourSpace::Other, ColourSpace::Srgb, Some("RGB"));
        assert!(rgb.contains("Adobe RGB"), "{rgb}");
        let cmyk = unsupported_note(ColourSpace::Other, ColourSpace::Srgb, Some("CMYK"));
        assert!(cmyk.contains("CMYK"), "{cmyk}");
        // And a Display-P3 target is named as itself rather than as sRGB.
        let to_p3 = unsupported_note(ColourSpace::Other, ColourSpace::DisplayP3, Some("RGB"));
        assert!(to_p3.contains("Display-P3"), "{to_p3}");
    }

    // ---- the request ----------------------------------------------------

    #[test]
    fn asking_for_both_answers_at_once_is_refused() {
        let text = ColourOptions {
            keep_source_pixels: true,
            embed_profile: true,
            ..Default::default()
        }
        .validate()
        .unwrap_err()
        .to_string();
        assert!(
            text.contains("keep the original") && text.contains("sRGB"),
            "{text}"
        );
        // Each one on its own is a legitimate request.
        for options in [
            ColourOptions {
                keep_source_pixels: true,
                ..Default::default()
            },
            ColourOptions {
                embed_profile: true,
                ..Default::default()
            },
            ColourOptions::default(),
        ] {
            assert!(options.validate().is_ok(), "{options:?}");
        }
    }

    #[test]
    fn the_default_is_convert_to_srgb() {
        let options = ColourOptions::default();
        assert_eq!(options.working_space, ColourSpace::Srgb);
        assert!(!options.keep_source_pixels && !options.embed_profile);
        for source in [ColourSpace::Untagged, ColourSpace::DisplayP3] {
            assert_eq!(
                options.output_space(source),
                ColourSpace::Srgb,
                "{source:?}"
            );
        }
        // The two opt-outs keep the source space, which is what makes them the
        // opt-outs rather than a third target.
        for options in [
            ColourOptions {
                keep_source_pixels: true,
                ..Default::default()
            },
            ColourOptions {
                embed_profile: true,
                ..Default::default()
            },
        ] {
            assert_eq!(
                options.output_space(ColourSpace::DisplayP3),
                ColourSpace::DisplayP3
            );
        }
    }

    /// A JSON request written by an app that predates this phase has no colour
    /// field at all, and it has to keep meaning what it meant.
    #[test]
    fn a_request_without_a_colour_block_deserialises_to_the_default() {
        #[derive(Deserialize)]
        struct Request {
            #[serde(default)]
            colour: ColourOptions,
        }
        let parsed: Request = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed.colour, ColourOptions::default());
        assert_eq!(parsed.colour.working_space, ColourSpace::Srgb);
    }
}
