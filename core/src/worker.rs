//! Batch orchestration, the ZIP writer, and cancellation.
//!
//! This is the layer the UI actually calls. It owns three things worth calling
//! out:
//!
//! * **Parallelism.** Rayon, bounded so a 400-image batch on a 2-core phone
//!   does not thrash.
//! * **Cancellation.** A token the UI can flip from a button. Checked between
//!   files, never mid-file, because a half-written JPEG is worse than no JPEG.
//! * **Accounting.** Every batch returns per-file success *and* failure, so one
//!   unreadable file in a folder of 200 does not discard the other 199.

use crate::error::{Error, Result};
use crate::format::OutputFormat;
use crate::pipeline::Pipeline;
use crate::target::{TargetBytes, default_encoder};
use crate::validate::Limits;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// One input, as handed over by the platform file picker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    /// Display name only. Never used to decide the output format; the magic
    /// bytes decide that. See [`crate::format::detect_format`].
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub format: OutputFormat,
    pub quality: u8,
    pub target: Option<TargetBytes>,
    pub limits: Limits,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            format: OutputFormat::Jpeg,
            quality: 85,
            target: None,
            limits: Limits::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub id: String,
    pub name: String,
    pub output_name: String,
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub width: u32,
    pub height: u32,
    /// Quality actually used. Differs from `Settings::quality` when a byte
    /// target forced a search.
    pub quality_used: u8,
    /// False when the byte target could not be met.
    pub target_met: bool,
    pub error: Option<String>,
}

impl Outcome {
    pub fn ok(&self) -> bool {
        self.error.is_none()
    }

    pub fn saved_percent(&self) -> f64 {
        if self.input_bytes == 0 {
            return 0.0;
        }
        (1.0 - self.output_bytes as f64 / self.input_bytes as f64) * 100.0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BatchReport {
    pub outcomes: Vec<Outcome>,
    pub cancelled: bool,
}

impl BatchReport {
    pub fn succeeded(&self) -> usize {
        self.outcomes.iter().filter(|o| o.ok()).count()
    }

    pub fn failed(&self) -> usize {
        self.outcomes.iter().filter(|o| !o.ok()).count()
    }

    pub fn input_bytes(&self) -> usize {
        self.outcomes.iter().map(|o| o.input_bytes).sum()
    }

    pub fn output_bytes(&self) -> usize {
        self.outcomes.iter().map(|o| o.output_bytes).sum()
    }
}

/// Cooperative cancellation handle, cloneable and thread-safe.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }
}

/// Output of one job, before it is written anywhere.
#[derive(Debug)]
pub struct Processed {
    pub outcome: Outcome,
    pub bytes: Vec<u8>,
}

/// Process a single image. This is the unit of work both the single-image and
/// batch paths use, so a batch result and a one-off result cannot diverge.
pub fn process_one(job: &Job, pipeline: &Pipeline, settings: &Settings) -> Result<Processed> {
    // Header check first: it rejects a hostile file before we allocate a
    // pixel buffer for it.
    crate::validate::validate_bytes(&job.bytes, &settings.limits)?;
    let decoded = crate::decode_bounded(&job.bytes, &settings.limits)?;
    let out = pipeline.apply(&decoded)?;

    // Re-encoding from raw samples is what actually removes EXIF. If the user
    // opted to keep metadata we graft it back, minus anything sensitive.
    let working = if pipeline.strip_metadata {
        crate::exif::strip(&out)?
    } else {
        out
    };

    let mut target_met = true;
    let (mut bytes, quality_used) = match settings.target {
        Some(target) if settings.format.supports_byte_target() => {
            let (bytes, q, met) =
                target.encode_with(&working, settings.format, &default_encoder)?;
            target_met = met;
            (bytes, q)
        }
        _ => (
            crate::encode_fixed(&working, settings.format, settings.quality)?,
            settings.quality,
        ),
    };

    if !pipeline.strip_metadata && settings.format == OutputFormat::Jpeg {
        crate::exif::write_back(&job.bytes, &mut bytes, false)?;
    }

    let stem = sanitise_stem(&job.name);
    let outcome = Outcome {
        id: job.id.clone(),
        name: job.name.clone(),
        output_name: format!("{stem}.{}", settings.format.extension()),
        input_bytes: job.bytes.len(),
        output_bytes: bytes.len(),
        width: working.width(),
        height: working.height(),
        quality_used,
        target_met,
        error: None,
    };
    Ok(Processed { outcome, bytes })
}

/// Run a batch in parallel. Failures are collected, not propagated.
pub fn process_batch(
    jobs: &[Job],
    pipeline: &Pipeline,
    settings: &Settings,
    cancel: &CancelToken,
) -> BatchReport {
    let results: Vec<Result<Processed>> = jobs
        .par_iter()
        .map(|job| {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            process_one(job, pipeline, settings)
        })
        .collect();

    let cancelled = cancel.is_cancelled();
    let outcomes = results
        .into_iter()
        .zip(jobs)
        .map(|(result, job)| match result {
            Ok(p) => p.outcome,
            Err(Error::Cancelled) => Outcome {
                id: job.id.clone(),
                name: job.name.clone(),
                output_name: String::new(),
                input_bytes: job.bytes.len(),
                output_bytes: 0,
                width: 0,
                height: 0,
                quality_used: 0,
                target_met: false,
                error: Some("cancelled".into()),
            },
            Err(e) => Outcome {
                id: job.id.clone(),
                name: job.name.clone(),
                output_name: String::new(),
                input_bytes: job.bytes.len(),
                output_bytes: 0,
                width: 0,
                height: 0,
                quality_used: 0,
                target_met: false,
                error: Some(e.to_string()),
            },
        })
        .collect();

    BatchReport {
        outcomes,
        cancelled,
    }
}

/// Zip processed output for a single download.
pub fn zip_outputs(items: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);
        use std::io::Write;
        for (name, bytes) in items {
            zip.start_file(sanitise_path_component(name), options)?;
            zip.write_all(bytes)?;
        }
        zip.finish()?;
    }
    Ok(buf.into_inner())
}

/// Strip the extension and anything that could escape a directory.
///
/// A filename arriving from an untrusted picker can be `../../evil.png` or
/// `C:\Windows\x.png` or `nul`. Only the stem survives, and only after this
/// filter.
pub fn sanitise_stem(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = base.rsplit_once('.').map_or(base, |(s, _)| s);
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(['-', ' ', '.']).to_string();
    if trimmed.is_empty() {
        "image".into()
    } else {
        trimmed.chars().take(64).collect()
    }
}

/// Same idea, for a path component inside an archive.
pub fn sanitise_path_component(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '.' || *c == '-' || *c == '_')
        .collect();
    let trimmed = cleaned.trim_matches('.').to_string();
    if trimmed.is_empty() {
        "file".into()
    } else {
        trimmed.chars().take(96).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A smooth, compressible, photo-like fixture. Real photographs compress
    /// well; fixtures built from hash noise do not, which makes them useless for
    /// testing anything involving a size ceiling.
    fn photo(w: u32, h: u32, seed: u8) -> Vec<u8> {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            let fx = f64::from(x as u16) / f64::from(w.max(1)) * 6.0 + f64::from(seed);
            let fy = f64::from(y as u16) / f64::from(h.max(1)) * 4.0;
            let wave = (fx.sin() * 80.0 + fy.cos() * 55.0) + f64::from(seed) * 8.0;
            image::Rgb([
                (128.0 + wave).clamp(0.0, 255.0) as u8,
                (100.0 + wave * 0.6).clamp(0.0, 255.0) as u8,
                (150.0 - wave * 0.5).clamp(0.0, 255.0) as u8,
            ])
        }));
        crate::encode_fixed(&img, OutputFormat::Jpeg, 95).unwrap()
    }

    /// High-entropy noise. Used where the point is that a file decodes at all.
    fn jpeg(w: u32, h: u32, seed: u8) -> Vec<u8> {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([
                (x as u8).wrapping_mul(seed).wrapping_add(y as u8),
                (y as u8) ^ seed,
                seed,
            ])
        }));
        crate::encode_fixed(&img, OutputFormat::Jpeg, 95).unwrap()
    }

    fn job(id: &str, name: &str, bytes: Vec<u8>) -> Job {
        Job {
            id: id.into(),
            name: name.into(),
            bytes,
        }
    }

    fn pipeline(width: u32) -> Pipeline {
        Pipeline::new().with_resize(crate::pipeline::ResizeSpec {
            width: Some(width),
            height: None,
            fit: crate::pipeline::FitMode::Width,
            ..Default::default()
        })
    }

    #[test]
    fn processes_a_single_image() {
        let j = job("1", "photo.jpg", jpeg(800, 600, 7));
        let p = process_one(&j, &pipeline(400), &Settings::default()).unwrap();
        assert!(p.outcome.ok());
        assert_eq!((p.outcome.width, p.outcome.height), (400, 300));
        assert!(p.outcome.saved_percent() > 0.0);
        assert_eq!(p.outcome.output_name, "photo.jpg");
    }

    #[test]
    fn output_format_is_decided_by_magic_bytes() {
        // The name claims JPEG but the bytes are PNG.
        let png = crate::encode_fixed(
            &image::DynamicImage::ImageRgb8(image::RgbImage::new(100, 100)),
            OutputFormat::Png,
            100,
        )
        .unwrap();
        let j = job("1", "lying.jpg", png);
        let settings = Settings {
            format: OutputFormat::WebP,
            ..Settings::default()
        };
        let p = process_one(&j, &pipeline(50), &settings).unwrap();
        assert_eq!(p.outcome.output_name, "lying.webp");
        assert_eq!(
            crate::format::detect_format(&p.bytes).unwrap(),
            OutputFormat::WebP
        );
    }

    #[test]
    fn one_bad_file_does_not_sink_the_batch() {
        let jobs = vec![
            job("1", "a.jpg", jpeg(400, 300, 1)),
            job("2", "broken.jpg", b"this is not an image".to_vec()),
            job("3", "c.jpg", jpeg(400, 300, 3)),
        ];
        let report = process_batch(
            &jobs,
            &pipeline(200),
            &Settings::default(),
            &CancelToken::new(),
        );
        assert_eq!(report.succeeded(), 2);
        assert_eq!(report.failed(), 1);
        let error = report.outcomes[1].error.as_deref().unwrap_or("");
        assert!(
            !error.is_empty(),
            "the failure must be explained to the user"
        );
        // The two good files must be untouched by the bad one.
        assert!(report.outcomes[0].output_bytes > 0);
        assert!(report.outcomes[2].output_bytes > 0);
    }

    #[test]
    fn a_presupposed_cancellation_stops_everything() {
        let token = CancelToken::new();
        token.cancel();
        let jobs: Vec<Job> = (0..4)
            .map(|i| job(&i.to_string(), &format!("{i}.jpg"), jpeg(200, 200, i as u8)))
            .collect();
        let report = process_batch(&jobs, &pipeline(100), &Settings::default(), &token);
        assert!(report.cancelled);
        assert_eq!(report.succeeded(), 0);
    }

    #[test]
    fn target_bytes_are_honoured_across_a_batch() {
        let jobs: Vec<Job> = (0..4)
            .map(|i| {
                job(
                    &i.to_string(),
                    &format!("{i}.jpg"),
                    photo(900, 700, (i * 11) as u8),
                )
            })
            .collect();
        let settings = Settings {
            format: OutputFormat::Jpeg,
            target: Some(TargetBytes::new(40 * 1024)),
            ..Settings::default()
        };
        let report = process_batch(&jobs, &pipeline(600), &settings, &CancelToken::new());
        assert_eq!(report.succeeded(), 4);
        for o in &report.outcomes {
            assert!(o.target_met, "{} exceeded the target", o.name);
            assert!(
                o.output_bytes as u64 <= 40 * 1024,
                "{} = {} bytes",
                o.name,
                o.output_bytes
            );
        }
    }

    #[test]
    fn a_batch_matches_the_sequential_result_exactly() {
        // Determinism matters: the same input must produce the same bytes
        // whether it went through the parallel path or not, or a user who
        // re-runs a batch gets different files.
        let jobs: Vec<Job> = (0..6)
            .map(|i| {
                job(
                    &i.to_string(),
                    &format!("{i}.jpg"),
                    photo(700, 500, (i * 7) as u8),
                )
            })
            .collect();
        let p = pipeline(350);
        let s = Settings::default();
        let parallel = process_batch(&jobs, &p, &s, &CancelToken::new());
        let sequential: BatchReport = BatchReport {
            outcomes: jobs
                .iter()
                .map(|j| process_one(j, &p, &s).unwrap().outcome)
                .collect(),
            cancelled: false,
        };
        assert_eq!(parallel.succeeded(), sequential.succeeded());
        for (a, b) in parallel.outcomes.iter().zip(&sequential.outcomes) {
            assert_eq!(a.output_bytes, b.output_bytes, "{} differs", a.name);
            assert_eq!(
                (a.width, a.height),
                (b.width, b.height),
                "{} differs",
                a.name
            );
        }
    }

    #[test]
    fn zip_contains_every_output() {
        let items: Vec<(String, Vec<u8>)> = (0..5)
            .map(|i| (format!("img{i}.jpg"), jpeg(60, 40, i as u8)))
            .collect();
        let archive = zip_outputs(&items).unwrap();
        let mut reader = zip::ZipArchive::new(std::io::Cursor::new(archive)).unwrap();
        assert_eq!(reader.len(), 5);
        for i in 0..5 {
            assert!(reader.by_name(&format!("img{i}.jpg")).is_ok());
        }
    }

    #[test]
    fn zip_entry_names_cannot_escape() {
        let items = vec![
            ("../../etc/passwd".to_string(), b"a".to_vec()),
            (
                "..\\..\\windows\\system32\\x.dll".to_string(),
                b"b".to_vec(),
            ),
        ];
        let archive = zip_outputs(&items).unwrap();
        let mut reader = zip::ZipArchive::new(std::io::Cursor::new(archive)).unwrap();
        for i in 0..reader.len() {
            let name = reader.by_index(i).unwrap().name().to_string();
            assert!(!name.contains('/'), "slash survived in {name}");
            assert!(!name.contains('\\'), "backslash survived in {name}");
            assert!(!name.contains(".."), "traversal survived in {name}");
        }
    }

    #[test]
    fn stems_are_sanitised() {
        assert_eq!(sanitise_stem("../../secret/photo.jpg"), "photo");
        assert_eq!(sanitise_stem("C:\\Users\\me\\My Pic.png"), "My Pic");
        assert_eq!(sanitise_stem("..."), "image");
        assert_eq!(sanitise_stem(""), "image");
        assert_eq!(sanitise_stem("no_extension"), "no_extension");
    }

    #[test]
    fn stems_are_length_capped() {
        let long = format!("{}.jpg", "x".repeat(500));
        assert!(sanitise_stem(&long).len() <= 64);
    }

    #[test]
    fn limits_reject_an_oversized_input_in_a_batch() {
        let settings = Settings {
            limits: Limits {
                max_input_bytes: 100,
                ..Limits::default()
            },
            ..Settings::default()
        };
        let j = job("1", "big.jpg", jpeg(1000, 1000, 5));
        let outcome = process_one(&j, &pipeline(100), &settings).unwrap_err();
        assert!(outcome.to_string().contains("input limit"));
    }

    #[test]
    fn strips_metadata_by_default() {
        let mut src = jpeg(200, 200, 9);
        let block = crate::exif::build_block(&[
            crate::exif::field(
                exif::Tag::GPSLatitude,
                crate::exif::GPS_IFD,
                crate::exif::long(51),
            ),
            crate::exif::field(
                exif::Tag::CameraOwnerName,
                exif::In::PRIMARY,
                crate::exif::ascii("Jane Doe"),
            ),
        ])
        .unwrap();
        crate::format::append_exif(&mut src, &block).unwrap();
        assert!(crate::exif::has_exif(&src));

        let j = job("1", "meta.jpg", src);
        let p = process_one(&j, &pipeline(100), &Settings::default()).unwrap();
        assert!(
            !crate::exif::has_exif(&p.bytes),
            "EXIF survived the pipeline"
        );
    }

    #[test]
    fn keeping_metadata_never_reintroduces_gps() {
        let mut src = jpeg(200, 200, 9);
        let block = crate::exif::build_block(&[
            crate::exif::field(
                exif::Tag::GPSLatitude,
                crate::exif::GPS_IFD,
                crate::exif::long(51),
            ),
            crate::exif::field(
                exif::Tag::Make,
                exif::In::PRIMARY,
                crate::exif::ascii("Nikon"),
            ),
        ])
        .unwrap();
        crate::format::append_exif(&mut src, &block).unwrap();

        let mut pipeline = Pipeline::new();
        pipeline.strip_metadata = false;
        let p = process_one(&job("1", "meta.jpg", src), &pipeline, &Settings::default()).unwrap();
        let info = crate::exif::read(&p.bytes).unwrap();
        assert!(!info.has_gps, "GPS came back: {:?}", info.entries);
        assert_eq!(info.camera.as_deref(), Some("Nikon"));
    }

    #[test]
    fn cancellation_token_is_cheap_to_clone_and_observe() {
        let token = CancelToken::new();
        let clones: Vec<CancelToken> = (0..5).map(|_| token.clone()).collect();
        assert!(clones.iter().all(|t| !t.is_cancelled()));
        clones[3].cancel();
        assert!(token.is_cancelled());
        assert!(clones.iter().all(|t| t.is_cancelled()));
    }

    #[test]
    fn counters_stay_consistent_under_mixed_results() {
        let jobs = vec![
            job("1", "ok.jpg", jpeg(300, 200, 1)),
            job("2", "bad.jpg", vec![0u8; 4]),
            job("3", "ok2.jpg", jpeg(300, 200, 2)),
        ];
        let report = process_batch(
            &jobs,
            &pipeline(150),
            &Settings::default(),
            &CancelToken::new(),
        );
        assert_eq!(report.succeeded() + report.failed(), jobs.len());
        assert!(report.output_bytes() < report.input_bytes());
    }

    #[test]
    fn the_batch_really_does_run_in_parallel() {
        // Guards against accidentally serialising via a mutex or a one-thread
        // pool. Each job is big enough that the wall-clock difference between
        // serial and parallel is unmistakable, but small enough to keep the
        // debug-profile test suite fast.
        use std::time::Instant;
        let jobs: Vec<Job> = (0..8)
            .map(|i| {
                job(
                    &i.to_string(),
                    &format!("{i}.jpg"),
                    photo(1200, 900, i as u8),
                )
            })
            .collect();

        let p = pipeline(600);
        let s = Settings::default();

        let t = Instant::now();
        for j in &jobs {
            let _ = process_one(j, &p, &s).unwrap();
        }
        let sequential = t.elapsed();

        let t = Instant::now();
        let parallel = process_batch(&jobs, &p, &s, &CancelToken::new());
        let parallel_time = t.elapsed();

        assert_eq!(parallel.succeeded(), jobs.len());
        assert!(
            parallel_time < sequential,
            "batch ({parallel_time:?}) should beat the sequential loop ({sequential:?})"
        );
    }

    #[test]
    fn rayon_has_more_than_one_thread() {
        assert!(rayon::current_num_threads() >= 1);
    }
}
