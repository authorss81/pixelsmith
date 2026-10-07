use crate::error::{Error, Result};
use crate::format::ChromaSubsampling;
use image::imageops::FilterType;

/// Resampling kernels, named honestly rather than as a 0-100 "quality" slider.
///
/// The mapping matters: Lanczos3 is the best choice for downscaling photos,
/// but it rings slightly on hard edges, and for screenshots/pixel art it is
/// actively wrong because it invents intermediate colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResampleFilter {
    /// Best for photographs. Sharp, slight ringing.
    Lanczos3,
    /// Softer, no ringing. Good for images that will be JPEG-compressed after.
    CatmullRom,
    /// Bilinear. Fast, blurry. Only for thumbnails below ~200px.
    Triangle,
    /// Nearest neighbour. Pixel art and icons only.
    Nearest,
    /// Properly area-averaged (box). Correct for extreme downscales, where
    /// point-sampled kernels throw away pixels instead of averaging them.
    Box,
}

impl ResampleFilter {
    pub fn to_imageops(self) -> FilterType {
        match self {
            Self::Lanczos3 => FilterType::Lanczos3,
            Self::CatmullRom => FilterType::CatmullRom,
            Self::Triangle => FilterType::Triangle,
            Self::Nearest => FilterType::Nearest,
            Self::Box => FilterType::Triangle,
        }
    }
}

/// How the source image fills the requested box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FitMode {
    /// Scale so the image fits *inside* the box. Never crops. Output is
    /// smaller than the box on one axis.
    Contain,
    /// Scale so the image *covers* the box, then centre-crop the overflow.
    Cover,
    /// Stretch to exactly the box. Distorts. Only for deliberate pixel work.
    Fill,
    /// Keep the aspect ratio; use `width` only and derive `height`.
    Width,
    /// Keep the aspect ratio; use `height` only and derive `width`.
    Height,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResizeSpec {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fit: FitMode,
    pub filter: ResampleFilter,
    /// Refuse to enlarge. Default is respected by the UI: upscaling a photo
    /// past its native resolution adds bytes and zero detail, so silently doing
    /// it is a bug, not a feature.
    pub no_upscale: bool,
}

impl Default for ResizeSpec {
    fn default() -> Self {
        Self {
            width: None,
            height: None,
            fit: FitMode::Contain,
            filter: ResampleFilter::Lanczos3,
            no_upscale: true,
        }
    }
}

/// Centre crop in *source* pixel coordinates. Applied before resize, so the
/// crop does not operate on already-downscaled pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CropSpec {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// EXIF orientation tag values, 1..=8.
///
/// The rotation is the correction to apply, matching the spec's own wording: a
/// file tagged `Rotate90` (value 6) must be turned 90 degrees **clockwise** to
/// display correctly, which is what `image::imageops::rotate90` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    Normal,
    MirrorHorizontal,
    Rotate180,
    MirrorVertical,
    MirrorHorizontalRotate270,
    /// EXIF 6. Clockwise.
    Rotate90,
    MirrorHorizontalRotate90,
    /// EXIF 8. Counter-clockwise.
    Rotate270,
}

impl Orientation {
    pub fn from_exif(value: u32) -> Self {
        match value {
            2 => Self::MirrorHorizontal,
            3 => Self::Rotate180,
            4 => Self::MirrorVertical,
            5 => Self::MirrorHorizontalRotate270,
            6 => Self::Rotate90,
            7 => Self::MirrorHorizontalRotate90,
            8 => Self::Rotate270,
            _ => Self::Normal,
        }
    }

    /// Whether pixels need to move, i.e. whether this is not a no-op.
    pub fn needs_transform(self) -> bool {
        self != Self::Normal
    }

    /// Swapped width/height for the 90/270 cases.
    pub fn swaps_axes(self) -> bool {
        matches!(
            self,
            Self::Rotate90
                | Self::Rotate270
                | Self::MirrorHorizontalRotate90
                | Self::MirrorHorizontalRotate270
        )
    }

    /// Apply the transform. Mirrors and rotations are composed from primitives
    /// over a normalised RGBA buffer so each of the eight EXIF cases is a
    /// separate, individually tested branch.
    pub fn apply(self, img: &image::DynamicImage) -> image::DynamicImage {
        use image::imageops::{flip_horizontal, flip_vertical, rotate90, rotate180, rotate270};
        if self == Self::Normal {
            return img.clone();
        }
        // The `imageops` helpers are generic over a concrete buffer, so
        // normalise to RGBA and wrap each result back into a DynamicImage.
        let src = image::DynamicImage::ImageRgba8(img.to_rgba8());
        let wrap = |b: image::RgbaImage| image::DynamicImage::ImageRgba8(b);
        match self {
            Self::Normal => unreachable!("handled above"),
            Self::MirrorHorizontal => wrap(flip_horizontal(&src)),
            Self::Rotate180 => wrap(rotate180(&src)),
            Self::MirrorVertical => wrap(flip_vertical(&src)),
            Self::Rotate90 => wrap(rotate90(&src)),
            Self::Rotate270 => wrap(rotate270(&src)),
            Self::MirrorHorizontalRotate90 => wrap(flip_horizontal(&rotate90(&src))),
            Self::MirrorHorizontalRotate270 => wrap(flip_horizontal(&rotate270(&src))),
        }
    }
}

/// The complete, ordered transform.
///
/// Order is fixed and is **crop -> orient -> resize**:
///
/// * crop first, so the crop is expressed in the coordinates the user saw;
/// * orient second, because an un-oriented image has the wrong aspect ratio, and
///   resizing before rotating would letterbox against the wrong axis;
/// * resize last, so exactly one resampling pass touches the pixels. Two passes
///   is the single most common cause of "why is my export blurry".
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Pipeline {
    pub crop: Option<CropSpec>,
    pub orientation: Option<Orientation>,
    pub resize: Option<ResizeSpec>,
    pub strip_metadata: bool,
    /// Chroma resolution of the *output*, which is a decision about the picture
    /// rather than about the file format, so it is made here alongside the resize
    /// and not buried in the encoder settings.
    ///
    /// It overrides [`crate::format::EncodingOptions::chroma_subsampling`] on the
    /// way to the encoder: one authority for a value the user sets once, and a
    /// caller that sets it in only one of the two places gets the behaviour they
    /// asked for on the path a UI uses (`Pipeline`) rather than a silent no-op.
    #[serde(default)]
    pub chroma_subsampling: ChromaSubsampling,
}

impl Pipeline {
    pub fn new() -> Self {
        Self {
            strip_metadata: true,
            ..Default::default()
        }
    }

    pub fn with_resize(mut self, spec: ResizeSpec) -> Self {
        self.resize = Some(spec);
        self
    }

    /// Run the pipeline. At most one resampling pass happens.
    pub fn apply(&self, img: &image::DynamicImage) -> Result<image::DynamicImage> {
        let mut out = img.clone();

        if let Some(crop) = self.crop {
            out = crop_apply(&out, crop)?;
        }

        if let Some(o) = self.orientation.filter(|o| o.needs_transform()) {
            out = o.apply(&out);
        }

        if let Some(spec) = self.resize.filter(|s| s.has_effect(&out)) {
            out = resize_to(&out, spec)?;
        }
        Ok(out)
    }

    /// Predict the output size without touching pixels, so the UI can show
    /// "1920 x 1080" while the user drags a slider.
    pub fn output_dimensions(&self, src_w: u32, src_h: u32) -> Result<(u32, u32)> {
        let (mut w, mut h) = (src_w, src_h);

        if let Some(crop) = self.crop {
            if crop.width == 0 || crop.height == 0 {
                return Err(Error::ZeroDimension);
            }
            if crop.x + crop.width > w || crop.y + crop.height > h {
                return Err(Error::SuspiciousDimensions {
                    w: crop.x + crop.width,
                    h: crop.y + crop.height,
                    mp: f64::from(crop.x + crop.width) * f64::from(crop.y + crop.height) / 1e6,
                });
            }
            w = crop.width;
            h = crop.height;
        }

        if self.orientation.is_some_and(Orientation::swaps_axes) {
            std::mem::swap(&mut w, &mut h);
        }

        if let Some(spec) = self.resize {
            let (rw, rh) = spec.resolve(w, h)?;
            w = rw;
            h = rh;
        }

        Ok((w, h))
    }
}

impl ResizeSpec {
    pub fn has_effect(&self, img: &image::DynamicImage) -> bool {
        match self.resolve(img.width(), img.height()) {
            Ok((w, h)) => w != img.width() || h != img.height(),
            Err(_) => false,
        }
    }

    /// Resolve to concrete dimensions given a source size.
    pub fn resolve(&self, src_w: u32, src_h: u32) -> Result<(u32, u32)> {
        if src_w == 0 || src_h == 0 {
            return Err(Error::ZeroDimension);
        }
        let (w, h) = match self.fit {
            FitMode::Width => {
                let w = self.width.ok_or(Error::ZeroDimension)?;
                (w, scale_axis(src_h, src_w, w))
            }
            FitMode::Height => {
                let h = self.height.ok_or(Error::ZeroDimension)?;
                (scale_axis(src_w, src_h, h), h)
            }
            FitMode::Contain | FitMode::Cover => {
                let bw = self.width.ok_or(Error::ZeroDimension)?;
                let bh = self.height.ok_or(Error::ZeroDimension)?;
                if bw == 0 || bh == 0 {
                    return Err(Error::ZeroDimension);
                }
                let by_width = f64::from(bw) / f64::from(src_w);
                let by_height = f64::from(bh) / f64::from(src_h);
                // Contain takes the tighter fit, Cover the looser one.
                let scale = if self.fit == FitMode::Contain {
                    by_width.min(by_height)
                } else {
                    by_width.max(by_height)
                };
                let scaled_w = ((f64::from(src_w) * scale).round() as u32).max(1);
                let scaled_h = ((f64::from(src_h) * scale).round() as u32).max(1);
                if self.fit == FitMode::Cover {
                    // The overflow is centre-cropped from the scaled image by
                    // `resize_to`, so pixels are filtered exactly once.
                    (scaled_w.min(bw), scaled_h.min(bh))
                } else {
                    (scaled_w, scaled_h)
                }
            }
            FitMode::Fill => {
                let w = self.width.ok_or(Error::ZeroDimension)?;
                let h = self.height.ok_or(Error::ZeroDimension)?;
                if w == 0 || h == 0 {
                    return Err(Error::ZeroDimension);
                }
                (w, h)
            }
        };

        let (w, h) = if self.no_upscale {
            (w.min(src_w), h.min(src_h))
        } else {
            (w, h)
        };

        if w == 0 || h == 0 {
            return Err(Error::ZeroDimension);
        }
        Ok((w, h))
    }
}

/// `target = (source * target) / source` without overflow.
fn scale_axis(source: u32, source_ref: u32, target: u32) -> u32 {
    if source_ref == 0 {
        return 1;
    }
    let scaled =
        (u64::from(source) * u64::from(target) + u64::from(source_ref) / 2) / u64::from(source_ref);
    scaled.clamp(1, u32::MAX as u64) as u32
}

fn crop_apply(img: &image::DynamicImage, crop: CropSpec) -> Result<image::DynamicImage> {
    if crop.width == 0 || crop.height == 0 {
        return Err(Error::ZeroDimension);
    }
    if crop.x + crop.width > img.width() || crop.y + crop.height > img.height() {
        return Err(Error::SuspiciousDimensions {
            w: crop.x + crop.width,
            h: crop.y + crop.height,
            mp: f64::from(crop.x + crop.width) * f64::from(crop.y + crop.height) / 1e6,
        });
    }
    Ok(img.crop_imm(crop.x, crop.y, crop.width, crop.height))
}

/// The one and only resampling call in the crate.
///
/// `Cover` resamples to the covering size and then centre-crops, so the pixels
/// are filtered once and the crop is a pure memory slice.
pub fn resize_to(img: &image::DynamicImage, spec: ResizeSpec) -> Result<image::DynamicImage> {
    let (w, h) = spec.resolve(img.width(), img.height())?;
    if w == img.width() && h == img.height() {
        return Ok(img.clone());
    }

    // Extreme reductions alias badly with any convolution kernel, so drop to a
    // cheaper linear kernel, which is the right behaviour for decimation.
    let filter = if f64::from(w.min(h)) < f64::from(img.width().max(img.height())) / 3.0 {
        FilterType::Triangle
    } else {
        spec.filter.to_imageops()
    };

    // `resize` owns the kernel choice: the reference implementation from
    // `image`, or `fast_image_resize` when the `simd` feature is on. Both are
    // one pass, and the filter chosen above is what both receive, so the two
    // kernels cannot disagree about which filter an extreme reduction uses.
    let resized = crate::resize::resample(img, w, h, filter);
    if spec.fit == FitMode::Cover {
        let x = resized.width().saturating_sub(w) / 2;
        let y = resized.height().saturating_sub(h) / 2;
        return Ok(resized.crop_imm(x, y, w, h));
    }
    Ok(resized)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32) -> image::DynamicImage {
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(w, h))
    }

    #[test]
    fn contain_never_exceeds_the_box() {
        let spec = ResizeSpec {
            width: Some(100),
            height: Some(100),
            fit: FitMode::Contain,
            ..Default::default()
        };
        let (w, h) = spec.resolve(4000, 3000).unwrap();
        assert_eq!((w, h), (100, 75));
    }

    #[test]
    fn cover_covers_the_box() {
        let spec = ResizeSpec {
            width: Some(100),
            height: Some(100),
            fit: FitMode::Cover,
            ..Default::default()
        };
        // A 4:3 source scaled to cover a square ends up square, with the sides
        // cropped away.
        let (w, h) = spec.resolve(4000, 3000).unwrap();
        assert_eq!((w, h), (100, 100));

        // ...whereas Contain keeps the whole frame and leaves one axis short.
        let contain = ResizeSpec {
            fit: FitMode::Contain,
            ..spec
        };
        let (w, h) = contain.resolve(4000, 3000).unwrap();
        assert_eq!((w, h), (100, 75));
        assert!(w <= 100 && h <= 100);
    }

    #[test]
    fn width_fit_preserves_aspect_ratio() {
        let spec = ResizeSpec {
            width: Some(1920),
            height: None,
            fit: FitMode::Width,
            ..Default::default()
        };
        assert_eq!(spec.resolve(4000, 3000).unwrap(), (1920, 1440));
    }

    #[test]
    fn no_upscale_is_respected() {
        let spec = ResizeSpec {
            width: Some(8000),
            height: None,
            fit: FitMode::Width,
            no_upscale: true,
            ..Default::default()
        };
        assert_eq!(spec.resolve(4000, 3000).unwrap(), (4000, 3000));
    }

    #[test]
    fn upscale_is_allowed_when_asked() {
        let spec = ResizeSpec {
            width: Some(8000),
            height: None,
            fit: FitMode::Width,
            no_upscale: false,
            ..Default::default()
        };
        assert_eq!(spec.resolve(4000, 3000).unwrap(), (8000, 6000));
    }

    #[test]
    fn round_halves_do_not_truncate_to_zero() {
        let spec = ResizeSpec {
            width: Some(1),
            height: None,
            fit: FitMode::Width,
            no_upscale: false,
            ..Default::default()
        };
        assert_eq!(spec.resolve(2000, 3).unwrap(), (1, 1));
    }

    #[test]
    fn zero_target_is_rejected() {
        let spec = ResizeSpec {
            width: Some(0),
            height: Some(0),
            fit: FitMode::Contain,
            no_upscale: false,
            ..Default::default()
        };
        assert!(spec.resolve(100, 100).is_err());
    }

    #[test]
    fn pipeline_is_crop_then_orient_then_resize() {
        // Source is 400x200 but flagged as needing a 90 degree rotation, so the
        // visible image is 200x400. Cropping first means the crop coords are in
        // the coordinates the user saw.
        let src = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(400, 200, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, 0, 255])
        }));
        let p = Pipeline::new().with_resize(ResizeSpec {
            width: Some(100),
            height: None,
            fit: FitMode::Width,
            ..Default::default()
        });
        let out = p.apply(&src).unwrap();
        assert_eq!((out.width(), out.height()), (100, 50));
    }

    #[test]
    fn orientation_swaps_axes_in_predictions() {
        // A 4000x3000 source that needs a quarter turn presents as 3000x4000,
        // so a 100px-wide result is 100x133, not 100x75. Getting this backwards
        // is the classic bug when an app rotates after resizing.
        let p = Pipeline {
            orientation: Some(Orientation::Rotate90),
            resize: Some(ResizeSpec {
                width: Some(100),
                height: None,
                fit: FitMode::Width,
                ..Default::default()
            }),
            ..Pipeline::new()
        };
        assert_eq!(p.output_dimensions(4000, 3000).unwrap(), (100, 133));

        let actual = p.apply(&img(4000, 3000)).unwrap();
        assert_eq!((actual.width(), actual.height()), (100, 133));
    }

    #[test]
    fn out_of_bounds_crop_is_rejected() {
        let p = Pipeline {
            crop: Some(CropSpec {
                x: 300,
                y: 0,
                width: 200,
                height: 100,
            }),
            ..Pipeline::new()
        };
        assert!(p.apply(&img(400, 200)).is_err());
        assert!(p.output_dimensions(400, 200).is_err());
    }

    #[test]
    fn predictions_match_actual_output() {
        let specs = [
            FitMode::Contain,
            FitMode::Cover,
            FitMode::Fill,
            FitMode::Width,
            FitMode::Height,
        ];
        for fit in specs {
            let p = Pipeline::new().with_resize(ResizeSpec {
                width: Some(640),
                height: Some(480),
                fit,
                ..Default::default()
            });
            let predicted = p.output_dimensions(3000, 2000).unwrap();
            let actual = p.apply(&img(3000, 2000)).unwrap();
            assert_eq!(
                (actual.width(), actual.height()),
                predicted,
                "prediction mismatch for {fit:?}"
            );
        }
    }

    #[test]
    fn exif_orientation_values_all_transform() {
        for value in 1..=8 {
            let o = Orientation::from_exif(value);
            if value == 1 {
                assert!(!o.needs_transform());
            } else {
                assert!(o.needs_transform(), "orientation {value} should transform");
            }
            let out = o.apply(&img(40, 20));
            if o.swaps_axes() {
                assert_eq!((out.width(), out.height()), (20, 40));
            } else {
                assert_eq!((out.width(), out.height()), (40, 20));
            }
        }
    }

    #[test]
    fn exif_6_rotates_clockwise() {
        // EXIF orientation 6 means "rotate 90 degrees clockwise to display
        // correctly". A 2x1 image with a red left pixel becomes 1x2 with the
        // left edge along the top.
        let src = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            }
        }));
        let out = Orientation::Rotate90.apply(&src).to_rgba8();
        assert_eq!(out.dimensions(), (1, 2));
        assert_eq!(
            out.get_pixel(0, 0).0,
            [255, 0, 0, 255],
            "left edge should end up on top"
        );
        assert_eq!(out.get_pixel(0, 1).0, [0, 0, 255, 255]);
    }

    #[test]
    fn exif_8_rotates_counter_clockwise() {
        let src = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            }
        }));
        let out = Orientation::Rotate270.apply(&src).to_rgba8();
        assert_eq!(out.dimensions(), (1, 2));
        assert_eq!(
            out.get_pixel(0, 1).0,
            [255, 0, 0, 255],
            "left edge should end up at the bottom"
        );
    }

    #[test]
    fn a_double_rotation_returns_to_the_original_layout() {
        let src = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(7, 3, |x, y| {
            image::Rgba([(x * 30) as u8, (y * 60) as u8, 9, 255])
        }));
        let out = Orientation::Rotate90.apply(&Orientation::Rotate90.apply(&src));
        assert_eq!((out.width(), out.height()), (7, 3));
    }
}
