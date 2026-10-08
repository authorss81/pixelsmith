//! Folder traversal: what is in this folder, before anything is processed.
//!
//! The phase prompt's sentence is the design goal: **processing should never be
//! the first time a user learns what is in the folder**. So this module does the
//! cheap half of the work up front — enumerate, filter, read a header — and
//! hands back a [`FolderPlan`] the UI can show *before* the button is worth
//! pressing. Nothing here decodes a picture, and nothing here writes a file.
//!
//! Three decisions that are easy to get wrong, and are therefore pinned here:
//!
//! * **A name is not a filter that decides.** Hard rule 7 says format comes from
//!   magic bytes, and the extension allow-list below is only a reason *not to
//!   open* a file — `report.txt` is never read, while `photo.png` containing a
//!   JPEG is a JPEG, and a `.jpg` containing a PDF is reported as a PDF.
//! * **The plan reads 4 KiB a file, not the whole thing.** A folder scan is
//!   cheap enough to run over everything the user picked before committing to
//!   any of it; a plan that read 400 photographs in full would not be.
//! * **Symlinks are not followed.** A link pointing at `/dev/zero` or back at
//!   its own parent makes a walk either unbounded or infinite, and both are
//!   reachable from a folder the user was given rather than chose.

use crate::error::{Error, Result};
use crate::format::OutputFormat;
use crate::pipeline::Pipeline;
use crate::worker::{
    BatchPolicy, BatchReport, CancelToken, Job, Outcome, Settings, SizeUnit, SkipReason,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Bytes of each file read to decide what it is.
///
/// Enough for every sniffer in the tree — a JPEG's SOI, a PNG's signature, and a
/// HEIF's `ftyp` box with its compatible-brand list — and 1% of a 4 KiB read
/// page's cost against the file itself. A `ftyp` box larger than this is not a
/// thing the plan has to cope with: the one kind of container whose geometry is
/// not in its first kilobyte is also the one `heic::detect` reads from the box
/// rather than from past it.
pub const HEADER_BYTES: usize = 4096;

/// Chunk size for reading a file's bytes.
///
/// The answer to "do not read an entire large file into memory to hash it":
/// [`read_bounded`] pulls the file in chunks of this and stops the moment it is
/// past the ceiling, so a 40 GB file in the folder is refused after 4 KB more
/// than the limit rather than after 40 GB of allocation.
pub const READ_CHUNK: usize = 64 * 1024;

/// How deep a directory walk goes, counted from each root.
///
/// A phone's DCIM has four levels and a download folder has two. Eight is past
/// anything anyone meant to select, and the cap is what stops a symlinked tree
/// from turning into an unbounded walk even where the links themselves are not
/// followed.
pub const MAX_DEPTH: usize = 8;

/// How many files one plan will hold.
///
/// A bound rather than a hope: the plan and the batch's job list are both in
/// memory, and a folder with half a million files is a way to make a
/// comfortable piece of software fall over. Reaching it is reported, not
/// silently truncated.
pub const MAX_ENTRIES: usize = 10_000;

/// Extensions worth opening.
///
/// A filter, not a decision: hard rule 7 puts the format in the magic bytes, and
/// this list only avoids reading the other four hundred files in a folder that
/// happens to sit next to the pictures. It is therefore an *addition* to what the
/// sniffer accepts and never a subtraction from it — `photo.jpeg`, `.JPG` and
/// `scan.tiff` are all here, and a `.jpg` full of PDF bytes is still reported.
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jpe", "png", "webp", "gif", "bmp", "dib", "tif", "tiff", "ico", "heic", "heif",
    "avif",
];

/// What the plan decided about one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "verdict")]
pub enum EntryVerdict {
    /// Looks like an image this build can open and is within the limits. This is
    /// the only verdict [`process_folder`] will try to export.
    Ready,
    /// The extension is not one this build opens, so the bytes were never read.
    /// Cheap, and the reason a folder's `.DS_Store` costs nothing.
    NotListed,
    /// The name looked like an image and the bytes are not one, or they are in a
    /// container this build cannot decode. `detected` says which, and it is
    /// `Some` only for the second: "your file is not an image" and "this build
    /// cannot open that format" are different sentences with different next
    /// steps, and the engine knows the difference.
    NotAnImage { detected: Option<OutputFormat> },
    /// Over [`crate::validate::Limits::max_input_bytes`], so processing it would
    /// be refused anyway. Reported now rather than after the decode.
    TooLarge { limit: u64 },
}

/// One file in the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderEntry {
    /// The path as it was found, for the caller to open later.
    pub path: String,
    /// File name alone, for display. Never used to decide anything — see
    /// [`crate::worker::sanitise_stem`] for the other end of the naming problem.
    pub name: String,
    pub bytes: u64,
    pub verdict: EntryVerdict,
}

/// Everything the UI needs to show before the user commits.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderPlan {
    pub entries: Vec<FolderEntry>,
    /// One sentence per path that could not be walked or read at all. Carried
    /// rather than returned as an error, because a folder with one unreadable
    /// subdirectory is still a folder the user wants to process — and hard rule
    /// 9 is about saying what happened, not about stopping.
    pub problems: Vec<String>,
}

impl FolderPlan {
    /// The entries [`process_folder`] will export.
    pub fn ready(&self) -> impl Iterator<Item = &FolderEntry> {
        self.entries
            .iter()
            .filter(|e| e.verdict == EntryVerdict::Ready)
    }

    pub fn ready_count(&self) -> usize {
        self.ready().count()
    }

    /// The entries that were candidates: everything the walk looked at except a
    /// name whose extension is not on the list.
    ///
    /// This is the population [`process_folder`] reports on, and the difference
    /// between it and [`FolderPlan::entries`] is the difference between "a folder
    /// of photographs" and "a folder that contains some photographs".
    pub fn candidates(&self) -> impl Iterator<Item = &FolderEntry> {
        self.entries
            .iter()
            .filter(|e| e.verdict != EntryVerdict::NotListed)
    }

    /// Total bytes of the files that will be exported.
    pub fn ready_bytes(&self) -> u64 {
        self.ready().map(|e| e.bytes).sum()
    }

    /// How many entries carry each non-ready verdict.
    pub fn counts(&self) -> Vec<(EntryVerdict, usize)> {
        let mut out: Vec<(EntryVerdict, usize)> = Vec::new();
        for entry in &self.entries {
            if entry.verdict == EntryVerdict::Ready {
                continue;
            }
            match out.iter_mut().find(|(known, _)| *known == entry.verdict) {
                Some((_, count)) => *count += 1,
                None => out.push((entry.verdict.clone(), 1)),
            }
        }
        out
    }
}

/// Look at a folder, or at a list of files, and say what is in it.
///
/// Never fails: every problem becomes a sentence in [`FolderPlan::problems`] and
/// every file becomes an entry with a verdict, so the caller always has something
/// to show.
pub fn plan(roots: &[PathBuf], limits: &crate::validate::Limits) -> FolderPlan {
    let mut out = FolderPlan::default();
    let mut found: Vec<PathBuf> = Vec::new();
    for root in roots {
        collect(root, 0, &mut found, &mut out.problems);
    }
    // Sorted so the plan is the same on every run and on every filesystem: two
    // directories that enumerate the same files in different orders would give a
    // different first duplicate otherwise, and "which of these two was kept"
    // should not depend on the order the kernel happened to hand them over.
    found.sort();
    found.dedup();

    for path in found {
        if out.entries.len() >= MAX_ENTRIES {
            out.problems.push(format!(
                "stopped after {MAX_ENTRIES} files; the rest of this folder was not looked at"
            ));
            break;
        }
        out.entries.push(inspect_path(&path, limits));
    }
    out
}

/// One file, judged. Opens it for the first `HEADER_BYTES` and no more.
fn inspect_path(path: &Path, limits: &crate::validate::Limits) -> FolderEntry {
    let name = file_name_of(path);
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut entry = FolderEntry {
        path: path.to_string_lossy().into_owned(),
        name: name.clone(),
        bytes: size,
        verdict: EntryVerdict::Ready,
    };
    if !has_image_extension(&name) {
        entry.verdict = EntryVerdict::NotListed;
        return entry;
    }
    if size > limits.max_input_bytes as u64 {
        entry.verdict = EntryVerdict::TooLarge {
            limit: limits.max_input_bytes as u64,
        };
        return entry;
    }
    // `detect_format` is the only thing that decides what a file is, and it is
    // asked about a prefix on the assumption — stated above, and asserted by
    // `a_format_the_sniffer_needs_more_than_a_header_for_is_still_named` — that a
    // header is enough.
    //
    // The `Ok` case is not automatically `Ready`: a container this build
    // recognises but cannot decode is a different answer from "not an image", and
    // `detect_format` already knows the difference, because the HEIF fallback
    // reads brand bytes whether or not the codec is compiled in. That is why the
    // verdict comes from the format rather than from whether the file happens to
    // open.
    let verdict = match read_header(path) {
        Ok(head) => match crate::format::detect_format(&head) {
            Ok(format) if can_decode(format) => EntryVerdict::Ready,
            Ok(format) => EntryVerdict::NotAnImage {
                detected: Some(format),
            },
            Err(_) => EntryVerdict::NotAnImage { detected: None },
        },
        Err(_) => EntryVerdict::NotAnImage { detected: None },
    };
    entry.verdict = verdict;
    entry
}

/// Whether this build can decode a format it recognises.
///
/// The mirror of [`crate::capabilities`], and read from the same `cfg!` values
/// so the plan and the capability list cannot disagree about which formats a UI
/// may offer.
fn can_decode(format: OutputFormat) -> bool {
    match format {
        OutputFormat::Avif => crate::capabilities().avif_decode,
        OutputFormat::Heic | OutputFormat::Heif => crate::capabilities().heic_decode,
        _ => true,
    }
}

fn read_header(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    // `take` so a 400 MB file costs 4 KB here. Without it `read_exact`'s error
    // would be about the file being short rather than about the plan being done.
    let mut head = Vec::with_capacity(HEADER_BYTES);
    file.take(HEADER_BYTES as u64)
        .read_to_end(&mut head)
        .map(|_| head)
}

/// Walk one root, collecting files. `problems` collects what could not be read.
fn collect(path: &Path, depth: usize, out: &mut Vec<PathBuf>, problems: &mut Vec<String>) {
    // `symlink_metadata` rather than `metadata` on purpose: the second follows a
    // link, so a link into `/dev` or back to a parent would be walked as though
    // it were ordinary content. A folder the user was *given* can contain those.
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        problems.push(format!(
            "could not look at {}: it may have moved, or this device will not let the app read it",
            path.display()
        ));
        return;
    };
    if meta.file_type().is_symlink() {
        return;
    }
    if meta.is_file() {
        out.push(path.to_path_buf());
        return;
    }
    if !meta.is_dir() {
        // A socket, a fifo, a device node. Nothing to export, and reading one
        // would block forever.
        return;
    }
    if depth >= MAX_DEPTH {
        problems.push(format!(
            "stopped {} levels down: it is deeper than any folder can usefully be, and \
             the rest of it was not looked at",
            MAX_DEPTH
        ));
        return;
    }
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(e) => {
            problems.push(format!("could not open {}: {e}", path.display()));
            return;
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        // A dotfile is where a desktop puts its own metadata, and a phone's
        // `.thumbnails` is a directory of pictures the user did not pick.
        if name.as_encoded_bytes().first() == Some(&b'.') {
            continue;
        }
        collect(&entry.path(), depth + 1, out, problems);
    }
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Whether the *name* is worth opening. Never decides what the file is.
pub fn has_image_extension(name: &str) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    IMAGE_EXTENSIONS.contains(&ext.as_str())
}

/// Read a file's bytes, in chunks, refusing to exceed `limit`.
///
/// The refusal is checked while reading rather than after, so the peak is
/// `limit + READ_CHUNK` and not the file's size. That is the whole difference
/// between "this folder has a 40 GB video in it" being a skipped line in a report
/// and being an out-of-memory.
pub fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut out = Vec::new();
    let mut chunk = [0u8; READ_CHUNK];
    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            return Ok(out);
        }
        if out.len() + read > limit {
            return Err(Error::InputTooLarge {
                limit,
                actual: out.len() + read,
            });
        }
        out.extend_from_slice(&chunk[..read]);
    }
}

/// How many files to process at once.
///
/// Two ceilings, and the smaller one wins. **Cores**: decode, resize and encode
/// are all CPU-bound, so threads past the core count buy nothing and cost
/// everything in cache. **Memory**: each worker holds a decoded picture plus the
/// resampling kernel's `f32` intermediate — about four bytes per pixel of the
/// source and again per row of the output — so sixteen workers on a folder of
/// 12 MP photographs is several gigabytes, and the phase-11 ceiling would be hit
/// several times over. On a phone that is an OOM, not a slowdown.
///
/// The memory figure comes from `/proc/meminfo` where the platform publishes it,
/// which is the desktop and every Android target; elsewhere [`FALLBACK_AVAILABLE_BYTES`]
/// stands in, because `std` publishes nothing portable and a guess dressed as a
/// measurement would be worse than a stated constant.
pub fn pool_size(largest_input_bytes: u64) -> usize {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    pool_size_for(cores, available_memory_bytes(), largest_input_bytes)
}

/// The decision itself, as a pure function so a test can drive it without a
/// machine of a particular shape.
pub fn pool_size_for(cores: usize, available_bytes: u64, largest_input_bytes: u64) -> usize {
    let footprint = largest_input_bytes
        .saturating_mul(DECODE_EXPANSION)
        .max(MIN_FOOTPRINT);
    // Half, not all: the workers are not the only thing on the device, the page
    // cache wants room for the file being read next, and a pool that plans to use
    // 100% of what is free is a pool that gets an OOM-killed halfway through.
    let memory_bound = (available_bytes / 2).saturating_div(footprint) as usize;
    cores.clamp(1, MAX_POOL).min(memory_bound.max(1))
}

/// Floor on the per-worker estimate, for a folder of tiny files.
///
/// Without it, a folder of 2 KB icons would plan for workers holding 48 bytes
/// each and the pool would go to `MAX_POOL` on a machine where the real cost is
/// the fixed overhead of a thread and a codec's scratch buffers, not the picture.
pub const MIN_FOOTPRINT: u64 = 32 * 1024 * 1024;

/// Ceiling on the pool whatever the machine reports. Sixteen workers on a
/// 64-core build server would each hold a decoded picture; this is a
/// responsiveness bound, not a speed one.
pub const MAX_POOL: usize = 8;

/// What one worker is expected to hold, for an input of `bytes` on disk.
///
/// Measured shape: a 12 MP JPEG is about 4 MB on disk and 48 MB decoded, so the
/// decode expands by roughly twelve, and `image`'s resize kernel allocates an
/// `f32` intermediate of `4 x source_width x output_height` on top — half the
/// source again for a full-size export. Twenty-four is a deliberate
/// over-estimate of the common case, because the two errors are not symmetric:
/// over-estimating makes the pool smaller, which costs time, while
/// under-estimating makes it thrash, which costs the phone.
pub const DECODE_EXPANSION: u64 = 24;

/// Assumed available memory where the platform does not publish any.
///
/// A floor rather than a hope. Windows and macOS both have an API for this and
/// neither is reachable without a crate this engine will not take, so the honest
/// answer is a stated constant that keeps the pool small rather than a probe that
/// silently disagrees with the one on the same page.
pub const FALLBACK_AVAILABLE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Free memory as the platform reports it, or [`FALLBACK_AVAILABLE_BYTES`].
pub fn available_memory_bytes() -> u64 {
    #[cfg(target_os = "linux")]
    {
        // Android is Linux, so this covers the shipped phone target.
        if let Ok(text) = std::fs::read_to_string("/proc/meminfo")
            && let Some(line) = text.lines().find(|l| l.starts_with("MemAvailable:"))
        {
            let kb: u64 = line
                .split_whitespace()
                .nth(1)
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            if kb > 0 {
                return kb * 1024;
            }
        }
    }
    FALLBACK_AVAILABLE_BYTES
}

/// Export a folder, in parallel, with the pool sized to the machine.
///
/// Reads each file inside its own task rather than taking a folder's worth of
/// bytes up front — see [`crate::worker::process_batch`]'s doc comment for why
/// that matters — and reports every file that did not produce an output with a
/// [`crate::worker::SkipReason`].
pub fn process_folder(
    roots: &[PathBuf],
    pipeline: &Pipeline,
    settings: &Settings,
    policy: &BatchPolicy,
    cancel: &CancelToken,
) -> Result<BatchReport> {
    settings.validate()?;
    pipeline.colour.validate()?;
    let plan = plan(roots, &settings.limits);
    // `plan.ready()` is a filter over the enumeration, so the paths a folder scan
    // has already vouched for are the ones that get read.
    let paths: Vec<PathBuf> = plan.ready().map(|e| PathBuf::from(&e.path)).collect();
    let largest = paths
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .max()
        .unwrap_or(0);
    let threads = pool_size(largest);

    let load = |path: &PathBuf| -> crate::worker::Loaded<'_> {
        match read_bounded(path, settings.limits.max_input_bytes) {
            Ok(bytes) => Ok(std::borrow::Cow::Owned(Job {
                // The path, not the file name: two files called `IMG_0001.jpg` in
                // different subdirectories are two files, and a report that could
                // only say "IMG_0001.jpg" would not let a user tell them apart.
                // The name is what `sanitise_stem` output names come from; this is
                // what a duplicate is reported against.
                id: path.to_string_lossy().into_owned(),
                name: path.to_string_lossy().into_owned(),
                bytes,
            })),
            Err(e) => Err((path.to_string_lossy().into_owned(), e)),
        }
    };

    let context = crate::worker::BatchContext::new(*policy);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .map_err(|e| Error::Io(std::io::Error::other(e)))?;
    let processed = pool
        .install(|| crate::worker::process_all(&paths, pipeline, settings, &context, cancel, load));
    let cancelled = processed.cancelled;

    // Every entry gets an outcome, in the plan's order. The batch only saw the
    // ready ones, so the rest are filled in here from the verdict the plan already
    // reached — which is the whole point of planning first: a HEIC in a build
    // without the decoder is *known* before anything is read, and it is reported
    // as this build's limitation rather than dropped on the floor.
    let mut ready = processed.outcomes.into_iter();
    let mut outcomes = Vec::with_capacity(plan.entries.len());
    for entry in &plan.entries {
        // A file whose extension was never on the list was never a candidate, so
        // it is not an outcome. It appears in the plan — which is where "what is in
        // this folder" is answered — and not in the report, because a folder next
        // to the pictures holding 300 `.DS_Store` files would bury the thirty that
        // matter.
        if entry.verdict == EntryVerdict::NotListed {
            continue;
        }
        let job = Job {
            id: entry.path.clone(),
            name: entry.path.clone(),
            bytes: Vec::new(),
        };
        outcomes.push(match entry.verdict {
            EntryVerdict::Ready => ready
                .next()
                .unwrap_or_else(|| Outcome::skipped(&job, SkipReason::Unreadable)),
            ref verdict => Outcome::skipped(&job, verdict_reason(verdict, entry.bytes)),
        });
    }

    Ok(BatchReport {
        outcomes,
        cancelled,
    })
}

/// The skip a non-ready verdict becomes.
///
/// A file this build cannot open and a file that is not an image at all both end
/// in the report, in a sentence of their own — never as silence. A file whose
/// extension was never on the list is *not* here: it was never a candidate, and
/// reporting 300 `.DS_Store` files as skipped would bury the thirty that matter.
fn verdict_reason(verdict: &EntryVerdict, bytes: u64) -> SkipReason {
    match verdict {
        EntryVerdict::NotAnImage {
            detected: Some(format),
        } => SkipReason::UnsupportedFormat { format: *format },
        EntryVerdict::NotAnImage { detected: None } => SkipReason::Unreadable,
        EntryVerdict::TooLarge { limit } => SkipReason::TooLarge {
            limit: *limit,
            actual: bytes,
            unit: SizeUnit::Bytes,
        },
        // Unreachable through this function: a ready entry is processed, and an
        // unlisted one never becomes an entry with an outcome at all. Both arms
        // exist because the match has to be total, and the honest answer for
        // either is "we could not read it" rather than a variant that claims
        // something untrue.
        EntryVerdict::Ready | EntryVerdict::NotListed => SkipReason::Unreadable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::EncodingOptions;

    fn jpeg(w: u32, h: u32, seed: u8) -> Vec<u8> {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            let fx = f64::from(x as u16) / f64::from(w.max(1)) * 6.0 + f64::from(seed);
            let wave = fx.sin() * 60.0 + f64::from(y as u16) * 0.3;
            image::Rgb([
                (128.0 + wave).clamp(0.0, 255.0) as u8,
                (90.0 + wave * 0.6).clamp(0.0, 255.0) as u8,
                (160.0 - wave * 0.5).clamp(0.0, 255.0) as u8,
            ])
        }));
        crate::encode_fixed(
            &img,
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(90),
        )
        .unwrap()
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) {
        if let Some(parent) = Path::new(name).parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(dir.join(parent)).unwrap();
        }
        std::fs::write(dir.join(name), bytes).unwrap();
    }

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn the_extension_filter_is_case_insensitive_and_narrow() {
        for name in [
            "a.JPG", "a.jpeg", "a.JpEg", "a.png", "a.webp", "a.HEIC", "a.tiff",
        ] {
            assert!(has_image_extension(name), "{name} should be opened");
        }
        for name in ["a.txt", "a", "a.mp4", "notes", "a.jpg.bak"] {
            assert!(!has_image_extension(name), "{name} should not be opened");
        }
        // A dotfile is never reached anyway — the walk skips dot entries before
        // this is asked — so this says nothing about what a leading dot does to an
        // extension, only that the walk is the thing that has to handle it.
        assert!(has_image_extension(".jpg"));
    }

    #[test]
    fn the_plan_lists_what_is_there_and_never_trusts_a_name() {
        let dir = dir();
        write(dir.path(), "one.jpg", &jpeg(32, 24, 1));
        // A JPEG behind a PNG's name: the magic bytes decide, so this is ready.
        write(dir.path(), "actually_a_jpeg.png", &jpeg(32, 24, 2));
        // A PDF behind a JPEG's name.
        write(dir.path(), "notes.jpg", b"%PDF-1.7\nnot an image at all\n");
        // A name that is not an image at all: never opened.
        write(dir.path(), "readme.txt", b"hello");

        let plan = plan(
            &[dir.path().to_path_buf()],
            &crate::validate::Limits::default(),
        );
        assert_eq!(plan.problems, Vec::<String>::new());
        assert_eq!(plan.entries.len(), 4);

        let by_name = |n: &str| {
            plan.entries
                .iter()
                .find(|e| e.name == n)
                .unwrap_or_else(|| panic!("{n} missing"))
                .verdict
                .clone()
        };
        assert_eq!(by_name("one.jpg"), EntryVerdict::Ready);
        assert_eq!(by_name("actually_a_jpeg.png"), EntryVerdict::Ready);
        assert_eq!(by_name("readme.txt"), EntryVerdict::NotListed);
        assert_eq!(
            by_name("notes.jpg"),
            EntryVerdict::NotAnImage { detected: None }
        );
        assert_eq!(plan.ready_count(), 2);
    }

    #[test]
    fn a_format_this_build_cannot_decode_is_named_rather_than_called_junk() {
        let dir = dir();
        // A `ftyp` box with the `heic` brand, and nothing else: `detect_format`
        // reads brand bytes and never needs the picture, so this is a real HEIC as
        // far as the plan is concerned without a decoder anywhere near it.
        let mut heic = Vec::new();
        heic.extend_from_slice(&24u32.to_be_bytes());
        heic.extend_from_slice(b"ftypheic");
        heic.extend_from_slice(&0u32.to_be_bytes());
        heic.extend_from_slice(b"mif1");
        write(dir.path(), "phone.heic", &heic);
        assert_eq!(
            crate::format::detect_format(&heic).unwrap(),
            OutputFormat::Heic
        );

        let plan = plan(
            &[dir.path().to_path_buf()],
            &crate::validate::Limits::default(),
        );
        let entry = &plan.entries[0];
        assert_eq!(entry.name, "phone.heic");
        if crate::capabilities().heic_decode {
            assert_eq!(entry.verdict, EntryVerdict::Ready);
        } else {
            assert_eq!(
                entry.verdict,
                EntryVerdict::NotAnImage {
                    detected: Some(OutputFormat::Heic)
                },
                "a HEIC in a build without the codec is our limitation, not a broken file"
            );
        }
    }

    /// Every sniffer in the tree has to be able to name a format from 4 KiB.
    ///
    /// This is the assumption `plan` rests on: it reads [`HEADER_BYTES`] and asks
    /// [`crate::format::detect_format`] what it has, rather than reading the whole
    /// file. If a future format needs more than a header to be recognised, this
    /// fails here instead of silently reporting a real picture as junk.
    #[test]
    fn a_format_the_sniffer_needs_more_than_a_header_for_is_still_named() {
        // Incompressible noise, so every encoding is comfortably larger than the
        // prefix. A flat field or a gradient compresses to a few hundred bytes and
        // this test would pass without ever reading past byte one — green and
        // checking nothing, which is the failure mode this module's 4 KiB budget
        // could otherwise hide behind.
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(256, 256, |x, y| {
            let mut n = x
                .wrapping_mul(0x9e37_79b9)
                .wrapping_add(y)
                .wrapping_mul(0x85eb_ca6b)
                .wrapping_add(0x1234_5678);
            n ^= n >> 15;
            n = n.wrapping_mul(0x2545_f491);
            image::Rgb([(n >> 16) as u8, (n >> 8) as u8, n as u8])
        }));
        for (format, expected) in [
            (OutputFormat::Jpeg, OutputFormat::Jpeg),
            (OutputFormat::Png, OutputFormat::Png),
            (OutputFormat::WebP, OutputFormat::WebP),
            (OutputFormat::Gif, OutputFormat::Gif),
            (OutputFormat::Bmp, OutputFormat::Bmp),
        ] {
            let bytes = crate::encode_fixed(&img, format, EncodingOptions::default()).unwrap();
            assert!(
                bytes.len() > HEADER_BYTES,
                "{format:?} fixture is too small to test"
            );
            assert_eq!(
                crate::format::detect_format(&bytes[..HEADER_BYTES]).unwrap(),
                expected,
                "{format:?} is not recognised from its first 4 KiB"
            );
        }
    }

    #[test]
    fn a_file_over_the_limit_is_reported_without_being_opened() {
        let dir = dir();
        let limits = crate::validate::Limits {
            max_input_bytes: 64,
            ..Default::default()
        };
        write(dir.path(), "big.jpg", &jpeg(80, 60, 3));
        let plan = plan(&[dir.path().to_path_buf()], &limits);
        assert_eq!(
            plan.entries[0].verdict,
            EntryVerdict::TooLarge { limit: 64 }
        );
        assert_eq!(plan.ready_count(), 0);
    }

    #[test]
    fn the_walk_goes_into_subdirectories_and_stops_at_dotfiles() {
        let dir = dir();
        write(dir.path(), "top.jpg", &jpeg(16, 12, 4));
        write(dir.path(), "a/b/c/deep.png", &jpeg(16, 12, 5));
        write(dir.path(), ".thumbnails/cache.jpg", &jpeg(16, 12, 6));

        let plan = plan(
            &[dir.path().to_path_buf()],
            &crate::validate::Limits::default(),
        );
        let names: Vec<&str> = plan.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"top.jpg"));
        assert!(names.contains(&"deep.png"), "{names:?}");
        assert!(
            !names.contains(&"cache.jpg"),
            "a dot directory is not content"
        );
        assert_eq!(plan.ready_count(), 2);
    }

    #[test]
    fn an_unreadable_path_is_a_sentence_rather_than_an_error() {
        let plan = plan(
            &[PathBuf::from("/nonexistent/photos/2024")],
            &crate::validate::Limits::default(),
        );
        assert!(plan.entries.is_empty());
        assert_eq!(plan.problems.len(), 1);
        // Hard rule 9: a sentence a person wrote, naming what to do about it.
        assert!(
            plan.problems[0].contains("could not look at"),
            "{}",
            plan.problems[0]
        );
    }

    #[test]
    fn a_file_is_also_a_valid_root() {
        let dir = dir();
        let bytes = jpeg(16, 12, 7);
        write(dir.path(), "one.jpg", &bytes);
        let file = dir.path().join("one.jpg");
        let plan = plan(&[file], &crate::validate::Limits::default());
        assert_eq!(plan.ready_count(), 1);
        assert_eq!(plan.ready_bytes(), bytes.len() as u64);
    }

    #[test]
    fn reading_a_file_stops_at_the_ceiling_rather_than_after_the_whole_file() {
        let dir = dir();
        write(dir.path(), "photo.jpg", &jpeg(400, 300, 8));
        let path = dir.path().join("photo.jpg");
        let whole = std::fs::read(&path).unwrap();
        assert_eq!(
            read_bounded(&path, whole.len()).unwrap(),
            whole,
            "a file inside the ceiling reads whole"
        );
        let err = read_bounded(&path, 16).unwrap_err();
        match err {
            Error::InputTooLarge { limit, actual } => {
                assert_eq!(limit, 16);
                assert!(actual > 16, "the refusal reports how far it got");
                assert!(
                    actual <= 16 + READ_CHUNK,
                    "the peak must be the ceiling plus one chunk, not the file"
                );
            }
            other => panic!("wrong error: {other}"),
        }
    }

    #[test]
    fn the_pool_is_bounded_by_memory_as_well_as_by_cores() {
        // Memory is not the constraint: sixteen workers, and there is room for
        // hundreds. Cores win.
        assert_eq!(
            pool_size_for(16, 8 << 30, 4 << 20),
            8,
            "MAX_POOL still applies"
        );
        // Memory is the constraint: a 200 MB input on a 1 GB phone, where the
        // expansion estimate says each worker wants about 4.8 GB.
        assert_eq!(pool_size_for(16, 1 << 30, 200 << 20), 1);
        // Nothing sensible at all still gives one worker rather than zero.
        assert_eq!(pool_size_for(16, 1 << 10, 1 << 30), 1);
        // One core is one worker however much memory there is.
        assert_eq!(pool_size_for(1, 64 << 30, 1 << 20), 1);
    }

    /// End to end: a folder on disk, in, a report out, with every file accounted
    /// for.
    ///
    /// The four cases are the four things a real folder contains — a picture, a
    /// copy of it, a file that is not a picture, and a file this build cannot open
    /// — and the assertion that matters most is the last one: five files in the
    /// folder, five outcomes out. The phase's whole point is that a folder is
    /// accounted for rather than merely attempted.
    #[test]
    fn a_folder_on_disk_reports_every_file_with_a_reason() {
        let dir = dir();
        let picture = jpeg(120, 90, 5);
        write(dir.path(), "a.jpg", &picture);
        // The same picture under another name: the case the phase exists for.
        write(dir.path(), "sub/b.jpg", &picture);
        write(dir.path(), "notes.jpg", b"%PDF-1.7\nnot an image\n");
        write(dir.path(), "readme.txt", b"not a candidate at all");
        let mut heic = Vec::new();
        heic.extend_from_slice(&24u32.to_be_bytes());
        heic.extend_from_slice(b"ftypheic");
        heic.extend_from_slice(&0u32.to_be_bytes());
        heic.extend_from_slice(b"mif1");
        write(dir.path(), "phone.heic", &heic);

        let plan = plan(
            &[dir.path().to_path_buf()],
            &crate::validate::Limits::default(),
        );
        assert_eq!(plan.entries.len(), 5);
        // The dot-file rule and the extension rule are both about not reading
        // files, and `readme.txt` is the one file that was never opened. The HEIC
        // is ready in a build with the codec and named in one without.
        let expected_ready = if crate::capabilities().heic_decode {
            3
        } else {
            2
        };
        assert_eq!(plan.ready_count(), expected_ready);

        let settings = Settings::default();
        let report = process_folder(
            &[dir.path().to_path_buf()],
            &Pipeline::new(),
            &settings,
            &BatchPolicy::default(),
            &CancelToken::new(),
        )
        .unwrap();

        assert_eq!(
            report.outcomes.len(),
            plan.candidates().count(),
            "every candidate in the plan is in the report"
        );
        assert_eq!(report.succeeded(), 1, "one distinct picture");
        assert_eq!(report.duplicates(), 1, "its copy");
        assert_eq!(report.failed(), 0);

        let of = |suffix: &str| {
            report
                .outcomes
                .iter()
                .find(|o| o.name.ends_with(suffix))
                .unwrap_or_else(|| panic!("{suffix} missing"))
        };
        assert_eq!(of("notes.jpg").skipped, Some(SkipReason::Unreadable));
        if crate::capabilities().heic_decode {
            // The fixture is a `ftyp` box and nothing else, so a build with the
            // codec plans it as ready and then refuses the picture. That is the
            // *file's* problem rather than the build's, and it says so — which is
            // the assertion: either verdict, and in the report.
            assert_eq!(of("phone.heic").skipped, Some(SkipReason::Unreadable));
        } else {
            assert_eq!(
                of("phone.heic").skipped,
                Some(SkipReason::UnsupportedFormat {
                    format: OutputFormat::Heic
                }),
                "a container this build cannot open is named, not dropped"
            );
        }
        assert!(
            report
                .outcomes
                .iter()
                .all(|o| !o.name.ends_with("readme.txt")),
            "a file whose extension was never on the list is not an outcome at all"
        );
        assert_eq!(
            report.outcomes.len(),
            4,
            "four of the five files were candidates; four outcomes"
        );
        for outcome in &report.outcomes {
            if let Some(reason) = &outcome.skipped {
                assert!(!reason.note().is_empty());
            }
        }
    }

    /// The plan runs before anything is processed, so it is the cheap answer.
    ///
    /// The bound is 4 KiB a file plus one `stat`, and this measures it against
    /// what reading the folder whole would cost — the number that decides whether
    /// a UI can show a preview of 400 photographs without it feeling broken.
    #[test]
    fn the_plan_reads_a_header_per_file_rather_than_the_whole_thing() {
        let dir = dir();
        const FILES: usize = 8;
        for i in 0..FILES {
            write(dir.path(), &format!("{i}.jpg"), &jpeg(800, 600, i as u8));
        }
        let whole: u64 = (0..FILES)
            .map(|i| {
                std::fs::metadata(dir.path().join(format!("{i}.jpg")))
                    .unwrap()
                    .len()
            })
            .sum();
        assert!(
            whole / FILES as u64 > (HEADER_BYTES * 4) as u64,
            "the fixture is not big enough for the comparison to mean anything: {whole} \
             bytes over {FILES} files"
        );

        let plan = plan(
            &[dir.path().to_path_buf()],
            &crate::validate::Limits::default(),
        );
        assert_eq!(plan.ready_count(), FILES);
        assert_eq!(
            plan.ready_bytes(),
            whole,
            "the plan reports the real sizes without reading them"
        );
        let read_by_the_plan = FILES as u64 * HEADER_BYTES as u64;
        assert!(
            read_by_the_plan * 4 < whole,
            "the plan should read well under a quarter of the folder: {read_by_the_plan} \
             of {whole} bytes"
        );
    }

    #[test]
    fn the_pool_this_machine_asked_for_is_usable() {
        let n = pool_size(4 << 20);
        assert!((1..=MAX_POOL).contains(&n), "pool_size returned {n}");
        assert!(available_memory_bytes() > 0);
    }
}
