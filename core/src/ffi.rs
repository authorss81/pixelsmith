//! C ABI surface for the Flutter front end.
//!
//! Every function here follows the same discipline, because this is the layer
//! where Rust safety stops protecting the caller:
//!
//! * Results are always a tagged struct. There is no path where a Dart caller
//!   receives a null pointer it might dereference.
//! * All strings returned to Dart are owned by Rust and freed with
//!   [`px_string_free`]. Dart copies what it needs before calling anything else.
//! * Buffers are length-prefixed, never NUL-terminated, so binary JPEG data
//!   containing a zero byte is safe.
//! * No function takes a raw pointer the caller could pass as null. Pointers
//!   are checked and turned into an error result.

use crate::animation::{AnimationOutcome, AnimationPolicy};
use crate::error::Result;
use crate::format::{EncodingOptions, OutputFormat};
use crate::pipeline::Pipeline;
use crate::presets;
use crate::target::TargetBytes;
use crate::validate::{Limits, inspect, validate_bytes};
use crate::worker::{CancelToken, Job, Settings, process_one, zip_outputs};
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};

/// Opaque 64-bit handle. Dart holds this as an `int` and never dereferences it.
pub type PxHandle = u64;

const HANDLE_INVALID: PxHandle = 0;

/// Status byte returned alongside every owned result.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PxStatus {
    Ok = 0,
    Error = 1,
    /// The FFI boundary was used incorrectly: a null pointer, a bad handle, a
    /// length that disagrees with the buffer. Never returned for ordinary input
    /// problems, only for caller bugs.
    InvalidArgument = 2,
}

/// An owned result. `#[repr(C)]` because Dart reads it by offset.
#[repr(C)]
pub struct PxBuffer {
    pub status: u32,
    /// Owned pointer, or null when `status != Ok`.
    pub data: *mut u8,
    pub len: usize,
    /// Human-readable explanation. Owned; free with [`px_string_free`].
    pub error: *mut std::ffi::c_char,
}

thread_local! {
    /// `(len, cap)` of every buffer handed to the caller and not yet freed,
    /// keyed by pointer.
    ///
    /// Tracked out of band so `PxBuffer` keeps a layout Dart can rely on. If
    /// this is ever lost, the next `px_buffer_free` for that pointer would
    /// rebuild a `Vec` with the wrong capacity, which is undefined behaviour.
    ///
    /// The **length** is stored as well as the capacity, and
    /// [`px_buffer_free`] ignores the length in the `PxBuffer` it is handed.
    /// `PxBuffer` is `#[repr(C)]` and Dart reads and writes it by offset, so
    /// `len` is caller-supplied; `Vec::from_raw_parts` takes the length as an
    /// argument and `RawVec::cap_set` asserts `cap >= len`, so a host bug that
    /// corrupted the field took the process down with a panic from inside a
    /// `no_mangle` free function.
    static CAPACITIES: std::cell::RefCell<std::collections::HashMap<usize, (usize, usize)>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Move a `Vec` into a raw pointer, remembering its length and capacity.
///
/// Uses `Vec::into_raw_parts` so the caller can `Vec::from_raw_parts(ptr, len,
/// cap)` exactly, with no sentinel and no double-free hazard.
fn into_raw(mut v: Vec<u8>) -> PxBuffer {
    let len = v.len();
    let cap = v.capacity();
    let ptr = v.as_mut_ptr();
    std::mem::forget(v);
    CAPACITIES.with(|c| c.borrow_mut().insert(ptr as usize, (len, cap)));
    PxBuffer {
        status: PxStatus::Ok as u32,
        data: ptr,
        len,
        error: std::ptr::null_mut(),
    }
}

fn err(message: impl Into<String>, status: PxStatus) -> PxBuffer {
    let c = std::ffi::CString::new(message.into())
        .unwrap_or_else(|_| std::ffi::CString::new("error").unwrap());
    PxBuffer {
        status: status as u32,
        data: std::ptr::null_mut(),
        len: 0,
        error: c.into_raw(),
    }
}

fn from_result(result: Result<Vec<u8>>) -> PxBuffer {
    match result {
        Ok(bytes) => into_raw(bytes),
        Err(e) => err(e.to_string(), PxStatus::Error),
    }
}

fn from_json<T: Serialize + ?Sized>(value: &T) -> PxBuffer {
    match serde_json::to_vec(value) {
        Ok(bytes) => into_raw(bytes),
        Err(e) => err(format!("serialisation failed: {e}"), PxStatus::Error),
    }
}

/// Borrow a Dart-supplied byte slice.
///
/// # Safety
/// `ptr` must be valid for `len` bytes, or `ptr` must be null with `len == 0`.
unsafe fn borrow<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if len == 0 {
        return Some(&[]);
    }
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the caller of `borrow` guarantees `ptr`/`len` describe a real
    // allocation, which is the contract of every `px_*` entry point.
    Some(unsafe { std::slice::from_raw_parts(ptr, len) })
}

// ---------------------------------------------------------------------------
// Envelope limits
//
// `Limits::max_input_bytes` bounds a *picture* the engine decodes. It cannot
// bound the JSON document a Dart caller hands over, for two reasons that are
// both structural: the envelope is a different object from the pictures in it,
// and by the time `validate_bytes` is reached the whole document and every byte
// array in it has already been materialised.
//
// So the envelope gets its own numbers, below, and none of them is a `Limits`
// field — `docs/ARCHITECTURE.md` records that phase-11 grew two *methods*
// precisely so the JSON contract the Dart side mirrors would not change, and a
// field here would change it.
// ---------------------------------------------------------------------------

/// How many characters one picture byte costs in an envelope at worst.
///
/// `BatchFile::bytes` is a JSON array of byte values, so one byte is two
/// characters at its cheapest (`0,`) and four at its dearest (`255,`). Four is
/// the number every envelope ceiling below is derived from.
pub const ENVELOPE_EXPANSION: usize = 4;

/// Most picture bytes summed across the files of one envelope.
///
/// **16 MiB.** Not `Limits::max_input_bytes`: that is what this device affords
/// for *one* picture, and an envelope is neither one picture nor free to be
/// large — it is a `Uint8List` the Dart isolate builds, a copy the allocator
/// makes on the way in, a set of parsed `Vec<u8>`s that stay resident for the
/// length of the export, and, in `BatchRequest`, a buffered parse on top because
/// `#[serde(flatten)]` cannot stream a sequence.
///
/// 16 MiB of picture is what a dozen phone photographs come to, which is a batch
/// worth exporting; a folder beyond that is *several requests*, which is how the
/// engine already feeds the worker pool on the folder path
/// (`folder::process_folder` runs the plan's entries through `process_all` one
/// file per task rather than one folder per request). `px_batch` is therefore a
/// small-batch API and says so rather than accepting a request it cannot hold.
pub const MAX_REQUEST_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

/// Most picture bytes in one file of an envelope.
///
/// **8 MiB**, half the envelope's total: one file in a batch is one export, and
/// this is above what a camera JPEG at a normal quality comes to. Separate from
/// the total because the two fail differently — a folder of modest files that
/// adds up to too much is worth telling the user to split, while one enormous
/// file is worth telling them the name of.
pub const MAX_REQUEST_FILE_BYTES: usize = 8 * 1024 * 1024;

/// Most files in one envelope.
///
/// **512.** The engine's own precedent for how many things one caller may hold
/// at once is the 1024 live cancel tokens `px_cancel_new` allows; a batch half
/// that is still two orders of magnitude above what a person exports in one go,
/// and it is only reachable at all by files far too small to need an envelope —
/// [`crate::folder::MAX_ENTRIES`]'s 10,000 is the folder *plan*, which is the
/// ceiling for walking a disk, not for transporting pictures across the FFI.
pub const MAX_REQUEST_FILES: usize = 512;

/// Most bytes in one request envelope.
///
/// Derived, not chosen: [`MAX_REQUEST_PAYLOAD_BYTES`] at the worst-case
/// [`ENVELOPE_EXPANSION`] is exactly this, so an envelope at the ceiling is one
/// carrying the most picture it is allowed to carry. A sparse envelope of `0,`
/// elements reaches the payload ceiling first, which is why the two sentences
/// below are reachable rather than one of them shadowing the other.
pub const MAX_REQUEST_BYTES: usize = MAX_REQUEST_PAYLOAD_BYTES * ENVELOPE_EXPANSION;

/// Refuse an envelope this build will not parse, in a sentence naming the limit.
///
/// Checked before `serde_json::from_slice` on every entry point that takes a
/// request, because the parse is where the document becomes memory: after it,
/// `Limits::max_input_bytes` is consulted inside `validate_bytes`, far too late
/// to have bounded anything.
///
/// The desktop profile is used rather than the mobile one the request may ask
/// for, and that is not an oversight: `mobile_limits` is *inside* the document,
/// so a pre-parse check cannot know which profile was asked for. The outer gate
/// is therefore the more permissive of the two, and the per-file and total
/// payload ceilings are applied afterwards against the profile that was asked
/// for.
fn check_envelope(len: usize) -> Option<PxBuffer> {
    if len <= MAX_REQUEST_BYTES {
        return None;
    }
    Some(err(
        format!(
            "this request is {len} bytes and one request may be at most {MAX_REQUEST_BYTES} \
             bytes; export in smaller groups"
        ),
        PxStatus::InvalidArgument,
    ))
}

thread_local! {
    /// The sentence a bounded deserialiser refused with, if it refused on one.
    ///
    /// Recorded here *as well as* being returned through serde, because
    /// `serde_json` renders a custom error as `"<message> at line 1 column
    /// 33554445"`, and hard rule 9 is about the sentence a user reads rather
    /// than about where in the document the failure happened. The entry point
    /// prefers this to serde's rendering; the slot is cleared at the start of
    /// every bounded parse and taken when the parse fails, so it cannot carry a
    /// stale refusal into a later call.
    static BOUND_REFUSAL: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// The sentence for a request the boundary refused to parse.
fn request_failure(e: serde_json::Error) -> PxBuffer {
    match BOUND_REFUSAL.with(|r| r.borrow_mut().take()) {
        Some(sentence) => err(sentence, PxStatus::InvalidArgument),
        None => err(format!("bad request: {e}"), PxStatus::InvalidArgument),
    }
}

/// The `files` array of a batch or ZIP envelope, bounded as it is built.
///
/// `serde` would materialise a million one-byte files out of a 30 MB document
/// without noticing, so the ceilings are enforced in the visitor rather than
/// checked on the result: a file is pushed only while the count, that file and
/// the running total are all still within budget. Each refusal names which of
/// the three it was, because "too big" is not something a user can act on and
/// "this folder has more files in it than one request holds" is.
#[derive(Debug, Serialize)]
struct BoundedFiles(Vec<BatchFile>);

impl<'de> serde::Deserialize<'de> for BoundedFiles {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> std::result::Result<Self, D::Error> {
        use serde::de::Error as _;

        BOUND_REFUSAL.with(|r| *r.borrow_mut() = None);

        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = BoundedFiles;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(
                    f,
                    "at most {MAX_REQUEST_FILES} files, each at most {MAX_REQUEST_FILE_BYTES} \
                     bytes, totalling at most {MAX_REQUEST_PAYLOAD_BYTES} bytes"
                )
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<BoundedFiles, A::Error> {
                let mut files: Vec<BatchFile> = Vec::new();
                let mut payload = 0usize;
                while let Some(file) = seq.next_element::<BatchFile>()? {
                    let refusal = if files.len() == MAX_REQUEST_FILES {
                        Some(format!(
                            "this request carries more than {MAX_REQUEST_FILES} files; export \
                             them in smaller groups"
                        ))
                    } else if file.bytes.len() > MAX_REQUEST_FILE_BYTES {
                        Some(format!(
                            "{} is {} bytes and one file in a request may be at most \
                             {MAX_REQUEST_FILE_BYTES} bytes",
                            file.name,
                            file.bytes.len()
                        ))
                    } else {
                        payload = payload.saturating_add(file.bytes.len());
                        (payload > MAX_REQUEST_PAYLOAD_BYTES).then(|| {
                            format!(
                                "the files in this request add up to more than \
                                 {MAX_REQUEST_PAYLOAD_BYTES} bytes; export them in smaller \
                                 groups"
                            )
                        })
                    };
                    if let Some(sentence) = refusal {
                        BOUND_REFUSAL.with(|r| *r.borrow_mut() = Some(sentence.clone()));
                        return Err(A::Error::custom(sentence));
                    }
                    files.push(file);
                }
                Ok(BoundedFiles(files))
            }
        }

        de.deserialize_seq(Visitor)
    }
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/// Engine version, as JSON. Useful as a smoke test that the Dart side linked the
/// library it expected rather than a stale copy.
#[unsafe(no_mangle)]
pub extern "C" fn px_version() -> PxBuffer {
    from_json(&serde_json::json!({
        "version": crate::ENGINE_VERSION,
        "capabilities": crate::capabilities(),
    }))
}

/// Release a buffer returned by any `px_*` call.
///
/// Idempotent: a pointer this library does not have — because it was never ours,
/// or because it has already been freed — is left alone. Leaking is strictly
/// better than rebuilding a `Vec` from a pointer whose layout we no longer know.
///
/// `buffer.len` is deliberately **not** used. `PxBuffer` is `#[repr(C)]` and Dart
/// reads and writes it by offset, so every field in it is caller-supplied
/// information; the length and capacity recorded when the buffer was handed out
/// are the only trustworthy ones, and they are what the `Vec` is rebuilt from.
///
/// # Safety
/// `buffer` must be a value returned by this library and not already freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn px_buffer_free(buffer: PxBuffer) {
    if buffer.data.is_null() {
        return;
    }
    let parts = CAPACITIES.with(|c| c.borrow_mut().remove(&(buffer.data as usize)));
    let Some((len, cap)) = parts else {
        // Not one of ours, or already freed. Leaking is strictly better than
        // rebuilding a `Vec` from a pointer we do not own.
        return;
    };
    // SAFETY: `data`, `len` and `cap` all came from `into_raw`, keyed by `data`;
    // the entry is removed above, so this is the first and only free of it.
    drop(unsafe { Vec::from_raw_parts(buffer.data, len, cap) });
}

/// Release an error string returned by any `px_*` call.
///
/// # Safety
/// `s` must come from this library and not already be freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn px_string_free(s: *mut std::ffi::c_char) {
    if !s.is_null() {
        // SAFETY: `s` came from `CString::into_raw` in `err`.
        drop(unsafe { std::ffi::CString::from_raw(s) });
    }
}

// ---------------------------------------------------------------------------
// Inspection
// ---------------------------------------------------------------------------

/// Header report + EXIF summary as JSON. Rejects a hostile file before decoding
/// a pixel buffer.
///
/// # Safety
/// `ptr` must be valid for reads of `len` bytes, or null when `len` is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn px_inspect(ptr: *const u8, len: usize, mobile_limits: bool) -> PxBuffer {
    // `let ... else` cannot take a block-expression scrutinee, so the borrow is
    // hoisted out rather than wrapped in an inline `unsafe { ... }`.
    let borrowed = unsafe { borrow(ptr, len) };
    let Some(bytes) = borrowed else {
        return err(
            "null buffer with non-zero length",
            PxStatus::InvalidArgument,
        );
    };
    let limits = if mobile_limits {
        Limits::mobile()
    } else {
        Limits::default()
    };
    match inspect(bytes, &limits) {
        Ok((report, _)) => from_json(&report),
        Err(e) => err(e.to_string(), PxStatus::Error),
    }
}

/// Full EXIF read as JSON.
///
/// # Safety
/// `ptr` must be valid for reads of `len` bytes, or null when `len` is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn px_exif(ptr: *const u8, len: usize) -> PxBuffer {
    // `let ... else` cannot take a block-expression scrutinee, so the borrow is
    // hoisted out rather than wrapped in an inline `unsafe { ... }`.
    let borrowed = unsafe { borrow(ptr, len) };
    let Some(bytes) = borrowed else {
        return err(
            "null buffer with non-zero length",
            PxStatus::InvalidArgument,
        );
    };
    match crate::exif::read(bytes) {
        Ok(info) => from_json(&info),
        Err(e) => err(e.to_string(), PxStatus::Error),
    }
}

/// The preset catalogue as JSON.
#[unsafe(no_mangle)]
pub extern "C" fn px_presets() -> PxBuffer {
    from_json(presets::all_presets())
}

// ---------------------------------------------------------------------------
// Processing
// ---------------------------------------------------------------------------

/// JSON request for [`px_process`].
///
/// `Serialize` is derived for [`sample_process_request`] and nothing else. The
/// app writes this document and the engine only ever reads it, so a writer for it
/// would be a second way to build a request that has to stay in step with the
/// real one; what the derive buys is that `ffi_json` can ask serde for these field
/// names instead of parsing this file, and that adding a field here without
/// updating the sample is a compile error rather than a silently-missing contract
/// line. No field is added, renamed or removed by deriving it.
#[derive(Debug, Deserialize, Serialize)]
struct ProcessRequest {
    pipeline: Pipeline,
    format: OutputFormat,
    #[serde(default = "default_quality")]
    quality: u8,
    /// Scan-by-scan JPEG. Off by default: progressive costs about 64% in file
    /// size at q85, and only pays off on a slow connection, so it is a choice
    /// rather than a silent improvement.
    #[serde(default)]
    progressive: bool,
    target: Option<u64>,
    /// Human-readable name for the input, used only to suggest an output name.
    #[serde(default)]
    name: String,
    /// The image itself, base64-encoded.
    ///
    /// Base64 rather than a JSON array of byte values: a number array costs up
    /// to four characters per byte, so a 6 MB photo becomes ~24 MB of JSON.
    /// Base64 is 33% overhead and keeps the request inspectable in a log.
    data_base64: String,
    #[serde(default)]
    strip_metadata: Option<bool>,
    /// What to do with an animation the chosen format cannot hold every frame
    /// of. Defaults to keeping the frames, or refusing the export; `first_frame`
    /// is the explicit opt-in for a still. See `core/src/animation.rs`.
    #[serde(default)]
    animation: AnimationPolicy,
    #[serde(default)]
    mobile_limits: bool,
}

fn default_quality() -> u8 {
    85
}

/// Result JSON for [`px_process`].
#[derive(Debug, Serialize)]
struct ProcessResponse {
    output_name: String,
    /// The encoded image.
    bytes: Vec<u8>,
    input_bytes: usize,
    output_bytes: usize,
    width: u32,
    height: u32,
    quality_used: u8,
    target_met: bool,
    detected_format: OutputFormat,
    /// What happened to this file's frames. Carried here as well as on the batch
    /// outcome because the single-image path is what a preview calls, and a
    /// caller must not have to decode the output to find out whether the
    /// animation survived.
    animation: AnimationOutcome,
}

/// Process one image. The request and the response are both JSON so the Dart side
/// needs no per-field marshalling code, and adding a field never breaks the ABI.
///
/// # Safety
/// `request` must point to `request_len` bytes of UTF-8 JSON.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn px_process(request: *const u8, request_len: usize) -> PxBuffer {
    let borrowed = unsafe { borrow(request, request_len) };
    let Some(raw) = borrowed else {
        return err("null request", PxStatus::InvalidArgument);
    };
    if let Some(refusal) = check_envelope(raw.len()) {
        return refusal;
    }
    let parsed: ProcessRequest = match serde_json::from_slice(raw) {
        Ok(v) => v,
        Err(e) => return request_failure(e),
    };

    let mut pipeline = parsed.pipeline;
    if let Some(strip) = parsed.strip_metadata {
        pipeline.strip_metadata = strip;
    }

    let limits = if parsed.mobile_limits {
        Limits::mobile()
    } else {
        Limits::default()
    };
    let settings = Settings {
        format: parsed.format,
        encoding: EncodingOptions {
            quality: parsed.quality,
            progressive: parsed.progressive,
            // The pipeline carries the chroma decision; see
            // `Pipeline::chroma_subsampling`.
            ..EncodingOptions::default()
        },
        target: parsed.target.map(TargetBytes::new),
        limits,
        animation: parsed.animation,
    };
    // The image travels base64-encoded inside the request envelope. An earlier
    // revision passed `raw.to_vec()` — the JSON envelope itself — as the image,
    // which meant px_process could never succeed. Decoding here is also the
    // first thing that happens to any caller-supplied payload at this boundary.
    use base64::Engine as _;
    let bytes = match base64::engine::general_purpose::STANDARD.decode(parsed.data_base64.trim()) {
        Ok(b) if !b.is_empty() => b,
        Ok(_) => return err("request contained no image data", PxStatus::InvalidArgument),
        Err(e) => {
            return err(
                format!("data_base64 is not valid base64: {e}"),
                PxStatus::InvalidArgument,
            );
        }
    };

    let job = Job {
        id: "1".into(),
        name: parsed.name,
        bytes,
    };

    match process_one(&job, &pipeline, &settings) {
        Ok(processed) => {
            let detected = validate_bytes(&job.bytes, &limits)
                .map(|r| r.format)
                .unwrap_or(parsed.format);
            from_json(&ProcessResponse {
                output_name: processed.outcome.output_name,
                width: processed.outcome.width,
                height: processed.outcome.height,
                input_bytes: processed.outcome.input_bytes,
                output_bytes: processed.outcome.output_bytes,
                quality_used: processed.outcome.quality_used,
                target_met: processed.outcome.target_met,
                detected_format: detected,
                animation: processed.outcome.animation,
                bytes: processed.bytes,
            })
        }
        Err(e) => err(e.to_string(), PxStatus::Error),
    }
}

// ---------------------------------------------------------------------------
// Batch
// ---------------------------------------------------------------------------

/// Cancel tokens, so Dart can hold an opaque handle it can flip from a button.
static TOKENS: OnceLock<Mutex<Vec<(PxHandle, CancelToken)>>> = OnceLock::new();

fn tokens() -> &'static Mutex<Vec<(PxHandle, CancelToken)>> {
    TOKENS.get_or_init(|| Mutex::new(Vec::new()))
}

static NEXT_HANDLE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Create a cancellation token. Returns 0 on failure.
#[unsafe(no_mangle)]
pub extern "C" fn px_cancel_new() -> PxHandle {
    let handle = NEXT_HANDLE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if let Ok(mut guard) = tokens().lock()
        && guard.iter().count() < 1024
    {
        guard.push((handle, CancelToken::new()));
        handle
    } else {
        HANDLE_INVALID
    }
}

/// Flip a cancellation token. Returns false for an unknown handle.
#[unsafe(no_mangle)]
pub extern "C" fn px_cancel_trigger(handle: PxHandle) -> bool {
    let Ok(guard) = tokens().lock() else {
        return false;
    };
    guard
        .iter()
        .find(|(h, _)| *h == handle)
        .map(|(_, token)| {
            token.cancel();
            true
        })
        .unwrap_or(false)
}

/// Drop a cancellation token. Returns false for an unknown handle.
#[unsafe(no_mangle)]
pub extern "C" fn px_cancel_free(handle: PxHandle) -> bool {
    let Ok(mut guard) = tokens().lock() else {
        return false;
    };
    let before = guard.len();
    guard.retain(|(h, _)| *h != handle);
    guard.len() != before
}

/// JSON request for [`px_batch`].
///
/// `Serialize` is derived for [`sample_batch_request`] — see
/// [`ProcessRequest`] for why, and for why it is not a second supported way to
/// build a request. `#[serde(flatten)]` on `pipeline` means the serialised form
/// has the pipeline's fields at the top level, which is what `ffi_json` reads
/// and what the Dart side has to send.
#[derive(Debug, Deserialize, Serialize)]
struct BatchRequest {
    #[serde(flatten)]
    pipeline: Pipeline,
    format: OutputFormat,
    #[serde(default = "default_quality")]
    quality: u8,
    /// Scan-by-scan JPEG. See `ProcessRequest::progressive`.
    #[serde(default)]
    progressive: bool,
    /// What to do with an animation. See `ProcessRequest::animation`.
    #[serde(default)]
    animation: AnimationPolicy,
    target: Option<u64>,
    #[serde(default)]
    mobile_limits: bool,
    /// What to do with a file that will not be processed. Absent in a request
    /// from an app predating phase-14, and `#[serde(default)]` therefore means
    /// "collapse duplicates and report every skip" rather than "fail everything",
    /// which is what this path used to do with the first of those and did not do
    /// at all with the rest.
    #[serde(default)]
    policy: crate::worker::BatchPolicy,
    cancel: Option<PxHandle>,
    /// Inputs as base64-free raw arrays would bloat the JSON, so each entry is
    /// `{name, bytes}` where bytes is a JSON array of byte values. Dart builds
    /// this from its `Uint8List`; the alternative (base64) costs 33% size.
    ///
    /// Bounded on the way in by [`BoundedFiles`], so the count, the per-file size
    /// and the total are refused while the sequence is being built.
    files: BoundedFiles,
}

/// One entry of a batch or ZIP request. `Serialize` for the contract dump; see
/// [`ProcessRequest`].
#[derive(Debug, Deserialize, Serialize)]
struct BatchFile {
    name: String,
    bytes: Vec<u8>,
}

/// Representative instances of the four envelopes, for the JSON contract dump.
///
/// `ffi_json` has to read the *names* of the fields in these structs, and it
/// reads them out of serde's own data model rather than by parsing this file's
/// source — see that module for why a source parser would be the wrong answer.
///
/// The request envelopes are written by Dart and read by Rust, so they derive
/// `Deserialize` and nothing in the engine needs to write one. They are built
/// here with every field spelled out, which serves two purposes: the dump sees
/// the real field set, and a field added to a struct is a **compile error** in
/// this function rather than a line the dump silently stops reporting. That
/// second property is the one that matters — a generator which reports fewer
/// fields than exist is worse than no check, because it looks like a pass.
///
/// These structs are private to this module, so the samples are not part of the
/// crate's public API and cannot be mistaken for a supported way to build a
/// request. `bytes` is empty rather than absent so the key is recorded: a field
/// skipped when empty is exactly what this check has to notice, and it cannot
/// notice it if no sample carries one.
/// These return `serde_json::Value` rather than the structs, because the structs
/// are private to this module and handing them to `ffi_json` would make them
/// part of the crate's internal API for no benefit. The construction is what
/// matters and it is here, in one place, spelling out every field.
pub(crate) fn sample_batch_file() -> serde_json::Value {
    serde_json::to_value(BatchFile { name: String::new(), bytes: Vec::new() })
        .expect("a BatchFile is serialisable")
}

pub(crate) fn sample_process_request() -> serde_json::Value {
    let request = ProcessRequest {
        pipeline: Pipeline::new(),
        format: OutputFormat::Jpeg,
        quality: 85,
        progressive: false,
        target: None,
        name: String::new(),
        data_base64: String::new(),
        strip_metadata: None,
        animation: AnimationPolicy::Keep,
        mobile_limits: false,
    };
    serde_json::to_value(request).expect("a ProcessRequest is serialisable")
}

pub(crate) fn sample_batch_request() -> serde_json::Value {
    let request = BatchRequest {
        pipeline: Pipeline::new(),
        format: OutputFormat::Jpeg,
        quality: 85,
        progressive: false,
        animation: AnimationPolicy::Keep,
        target: None,
        mobile_limits: false,
        policy: crate::worker::BatchPolicy::default(),
        cancel: None,
        files: BoundedFiles(vec![BatchFile { name: String::new(), bytes: Vec::new() }]),
    };
    serde_json::to_value(request).expect("a BatchRequest is serialisable")
}

pub(crate) fn sample_process_response() -> serde_json::Value {
    let response = ProcessResponse {
        output_name: String::new(),
        bytes: Vec::new(),
        input_bytes: 0,
        output_bytes: 0,
        width: 1,
        height: 1,
        quality_used: 0,
        target_met: true,
        detected_format: OutputFormat::Jpeg,
        animation: AnimationOutcome::unknown(),
    };
    serde_json::to_value(response).expect("a ProcessResponse is serialisable")
}

/// Run a batch in parallel. Always returns a report, never a hard failure, so
/// the UI can show "17 of 20 succeeded" instead of an error dialog.
///
/// # Safety
/// `request` must point to `request_len` bytes of UTF-8 JSON.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn px_batch(request: *const u8, request_len: usize) -> PxBuffer {
    let borrowed = unsafe { borrow(request, request_len) };
    let Some(raw) = borrowed else {
        return err("null request", PxStatus::InvalidArgument);
    };
    if let Some(refusal) = check_envelope(raw.len()) {
        return refusal;
    }
    let parsed: BatchRequest = match serde_json::from_slice(raw) {
        Ok(v) => v,
        Err(e) => return request_failure(e),
    };

    let cancel = match parsed.cancel {
        Some(h) => tokens()
            .lock()
            .ok()
            .and_then(|guard| guard.iter().find(|(k, _)| *k == h).map(|(_, t)| t.clone())),
        None => None,
    };
    let cancel = cancel.unwrap_or_default();
    let policy = parsed.policy;

    let settings = Settings {
        format: parsed.format,
        encoding: EncodingOptions {
            quality: parsed.quality,
            progressive: parsed.progressive,
            ..EncodingOptions::default()
        },
        target: parsed.target.map(TargetBytes::new),
        limits: if parsed.mobile_limits {
            Limits::mobile()
        } else {
            Limits::default()
        },
        animation: parsed.animation,
    };
    let pipeline = Pipeline {
        strip_metadata: parsed.pipeline.strip_metadata,
        ..parsed.pipeline
    };

    // `f.bytes` moves rather than copies: this is the `Vec` `serde` already
    // built, handed to the job, not a second copy of the picture. What this costs
    // is one `Job` header per file.
    let jobs: Vec<Job> = parsed
        .files
        .0
        .into_iter()
        .enumerate()
        .map(|(i, f)| Job {
            id: i.to_string(),
            name: f.name,
            bytes: f.bytes,
        })
        .collect();

    let report = crate::worker::process_batch(&jobs, &pipeline, &settings, &policy, &cancel);
    from_json(&report)
}

/// Build a ZIP from already-processed outputs.
///
/// # Safety
/// `request` must point to `request_len` bytes of UTF-8 JSON.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn px_zip(request: *const u8, request_len: usize) -> PxBuffer {
    #[derive(Deserialize)]
    struct ZipRequest {
        /// Bounded exactly as `BatchRequest::files` is. `px_zip` builds no
        /// `Limits` at all — there is no picture in it to bound — so the envelope
        /// ceilings above are the only thing standing between a ZIP request and
        /// whatever it asked for.
        files: BoundedFiles,
    }
    let borrowed = unsafe { borrow(request, request_len) };
    let Some(raw) = borrowed else {
        return err("null request", PxStatus::InvalidArgument);
    };
    if let Some(refusal) = check_envelope(raw.len()) {
        return refusal;
    }
    let parsed: ZipRequest = match serde_json::from_slice(raw) {
        Ok(v) => v,
        Err(e) => return request_failure(e),
    };
    let items: Vec<(String, Vec<u8>)> = parsed
        .files
        .0
        .into_iter()
        .map(|f| (f.name, f.bytes))
        .collect();
    from_result(zip_outputs(&items))
}

/// Fail deliberately. Exists so the Dart test suite can assert that an error
/// result is handled correctly without needing a genuinely corrupt file.
#[unsafe(no_mangle)]
pub extern "C" fn px_selftest_error() -> PxBuffer {
    err("deliberate self-test failure", PxStatus::Error)
}

/// Deliberate panic, to verify the host process isolates a crash rather than
/// dying with it. Returns a normal error buffer instead of aborting.
#[unsafe(no_mangle)]
pub extern "C" fn px_selftest_panic() -> PxBuffer {
    let result =
        std::panic::catch_unwind(|| -> Result<()> { panic!("deliberate self-test panic") });
    match result {
        Ok(_) => from_result(Ok(Vec::new())),
        Err(_) => err("panic was contained", PxStatus::Error),
    }
}

/// The memory layout of [`PxBuffer`], as JSON.
///
/// Dart reads `PxBuffer` by byte offset, so the declaration in `bindings.dart`
/// has to agree with the struct above byte for byte. A test that asserts a
/// hand-typed "32" proves the author once counted correctly; this proves the
/// library the app actually loaded has the layout the app expects, on whatever
/// target it was compiled for. A 32-bit target has different numbers, and a
/// hard-coded expectation would be wrong there rather than absent.
#[unsafe(no_mangle)]
pub extern "C" fn px_abi_layout() -> PxBuffer {
    from_json(&crate::ffi_abi::px_buffer_layout())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(w: u32, h: u32) -> Vec<u8> {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 40])
        }));
        crate::encode_fixed(
            &img,
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(90),
        )
        .unwrap()
    }

    /// Base64-encode a fixture the way a Dart caller would send it.
    fn b64(bytes: &[u8]) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// A smooth, compressible fixture.
    ///
    /// `sample` is high-entropy noise, which is fine for proving a file decodes
    /// and useless for proving a byte ceiling can be met — no quality setting
    /// compresses noise to 4 KB. Using the wrong one makes a correct engine look
    /// broken.
    fn photo(w: u32, h: u32) -> Vec<u8> {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            let fx = f64::from(x as u16) / f64::from(w.max(1)) * 6.0;
            let fy = f64::from(y as u16) / f64::from(h.max(1)) * 4.0;
            let wave = fx.sin() * 80.0 + fy.cos() * 55.0;
            image::Rgb([
                (128.0 + wave).clamp(0.0, 255.0) as u8,
                (100.0 + wave * 0.6).clamp(0.0, 255.0) as u8,
                (150.0 - wave * 0.5).clamp(0.0, 255.0) as u8,
            ])
        }));
        crate::encode_fixed(
            &img,
            OutputFormat::Jpeg,
            EncodingOptions::default().with_quality(90),
        )
        .unwrap()
    }

    /// Reclaim a buffer exactly as Dart would.
    unsafe fn take(buffer: PxBuffer) -> (u32, Vec<u8>, String) {
        let status = buffer.status;
        let message = if buffer.error.is_null() {
            String::new()
        } else {
            // SAFETY: `error` came from `CString::into_raw`.
            unsafe { std::ffi::CStr::from_ptr(buffer.error) }
                .to_string_lossy()
                .into_owned()
        };
        unsafe { px_string_free(buffer.error) };
        let data = if buffer.data.is_null() {
            Vec::new()
        } else {
            // SAFETY: `data`/`len` came from `into_raw`.
            unsafe { std::slice::from_raw_parts(buffer.data, buffer.len) }.to_vec()
        };
        unsafe { px_buffer_free(buffer) };
        (status, data, message)
    }

    #[test]
    fn a_thousand_rounds_of_the_boundary_leak_nothing() {
        // `CAPACITIES` holds one entry per buffer handed out and not returned, so
        // its length is the engine's own answer to "how much did you leak". A
        // Dart test cannot do better than run out of memory eventually; this can
        // say so after one round. Every allocation shape goes through the loop:
        // success buffers, error strings, handles, and the deliberate failures,
        // because a leak in the error path is the one nobody notices.
        fn live_buffers() -> usize {
            CAPACITIES.with(|c| c.borrow().len())
        }

        let input = sample(64, 48);
        let process_body = serde_json::to_vec(&serde_json::json!({
            "pipeline": { "crop": null, "orientation": null, "resize": null, "strip_metadata": true },
            "format": "jpeg",
            "quality": 85,
            "target": null,
            "name": "photo.jpg",
            "data_base64": b64(&input)
        }))
        .unwrap();
        let batch_body = serde_json::to_vec(&serde_json::json!({
            "crop": null, "orientation": null, "resize": null, "strip_metadata": true,
            "format": "jpeg", "quality": 85, "target": null,
            "files": [
                { "name": "a.jpg", "bytes": input },
                { "name": "b.jpg", "bytes": b"broken".to_vec() }
            ]
        }))
        .unwrap();
        let zip_body = serde_json::to_vec(
            &serde_json::json!({ "files": [{ "name": "a.jpg", "bytes": input }] }),
        )
        .unwrap();

        assert_eq!(live_buffers(), 0, "a test before this one leaked");
        for round in 0..1000 {
            // A fresh token per round: `px_cancel_free` really does drop it, so
            // reusing the handle would test the "unknown handle" path 999 times.
            let handle = px_cancel_new();
            assert_ne!(handle, HANDLE_INVALID, "round {round}: no token");
            unsafe {
                assert_eq!(take(px_version()).0, PxStatus::Ok as u32);
                assert_eq!(
                    take(px_inspect(input.as_ptr(), input.len(), false)).0,
                    PxStatus::Ok as u32
                );
                assert_eq!(
                    take(px_exif(input.as_ptr(), input.len())).0,
                    PxStatus::Ok as u32
                );
                assert_eq!(take(px_presets()).0, PxStatus::Ok as u32);
                assert_eq!(
                    take(px_process(process_body.as_ptr(), process_body.len())).0,
                    PxStatus::Ok as u32
                );
                assert_eq!(
                    take(px_batch(batch_body.as_ptr(), batch_body.len())).0,
                    PxStatus::Ok as u32
                );
                assert_eq!(
                    take(px_zip(zip_body.as_ptr(), zip_body.len())).0,
                    PxStatus::Ok as u32
                );
                assert_eq!(take(px_selftest_error()).0, PxStatus::Error as u32);
                assert_eq!(take(px_abi_layout()).0, PxStatus::Ok as u32);
                // The contained panic returns an error, so it is part of the
                // loop rather than a separate test that hides a leaked string.
                assert_eq!(take(px_selftest_panic()).0, PxStatus::Error as u32);
            }
            assert!(px_cancel_trigger(handle));
            assert!(px_cancel_free(handle));

            if round % 100 == 0 {
                let live = live_buffers();
                assert_eq!(live, 0, "round {round} left {live} buffers live");
            }
        }
        assert_eq!(live_buffers(), 0, "the loop leaked buffers");
    }

    #[test]
    fn the_abi_layout_endpoint_describes_the_struct_dart_reads() {
        // The Dart side asserts its `PxBuffer` declaration against this, so it
        // has to describe the struct rather than a copy of it that could go
        // stale.
        unsafe {
            let (status, data, msg) = take(px_abi_layout());
            assert_eq!(status, PxStatus::Ok as u32, "{msg}");
            let v: serde_json::Value = serde_json::from_slice(&data).unwrap();
            assert_eq!(v["name"], "PxBuffer");
            assert_eq!(v["size"], std::mem::size_of::<PxBuffer>());
            let fields: Vec<(String, u64)> = v["fields"]
                .as_array()
                .expect("fields is an array")
                .iter()
                .map(|f| {
                    (
                        f["name"].as_str().expect("field name").to_string(),
                        f["offset"].as_u64().expect("offset is a number"),
                    )
                })
                .collect();
            let names: Vec<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
            assert_eq!(names, vec!["status", "data", "len", "error"]);
            for (i, (_, offset)) in fields.iter().enumerate().skip(1) {
                let previous = fields[i - 1].1;
                let previous_size = v["fields"][i - 1]["size"].as_u64().unwrap();
                assert!(
                    *offset >= previous + previous_size,
                    "{} starts inside the field before it",
                    names[i]
                );
            }
        }
    }

    #[test]
    fn version_is_readable_json() {
        unsafe {
            let (status, data, _) = take(px_version());
            assert_eq!(status, PxStatus::Ok as u32);
            let v: serde_json::Value = serde_json::from_slice(&data).unwrap();
            assert_eq!(v["version"], crate::ENGINE_VERSION);
            assert!(v["capabilities"]["jpeg"].as_bool().unwrap());
        }
    }

    #[test]
    fn inspect_reports_dimensions() {
        unsafe {
            let input = sample(120, 90);
            let (status, data, _) = take(px_inspect(input.as_ptr(), input.len(), false));
            assert_eq!(status, PxStatus::Ok as u32);
            let v: serde_json::Value = serde_json::from_slice(&data).unwrap();
            assert_eq!(v["width"], 120);
            assert_eq!(v["height"], 90);
            assert_eq!(v["format"], "jpeg");
        }
    }

    #[test]
    fn inspect_rejects_garbage_without_crashing() {
        unsafe {
            let junk = b"nonsense";
            let (status, data, msg) = take(px_inspect(junk.as_ptr(), junk.len(), false));
            assert_eq!(status, PxStatus::Error as u32);
            assert!(data.is_empty());
            assert!(!msg.is_empty());
        }
    }

    #[test]
    fn a_null_buffer_with_a_length_is_a_caller_error() {
        unsafe {
            let (status, _, msg) = take(px_inspect(std::ptr::null(), 16, false));
            assert_eq!(status, PxStatus::InvalidArgument as u32);
            assert!(msg.contains("null"));
        }
    }

    #[test]
    fn a_zero_length_buffer_is_handled() {
        unsafe {
            let (status, _, _) = take(px_inspect(std::ptr::null(), 0, false));
            assert_eq!(status, PxStatus::Error as u32);
        }
    }

    #[test]
    fn process_round_trips_through_json() {
        unsafe {
            let request = serde_json::json!({
                "pipeline": {
                    "crop": null,
                    "orientation": null,
                    "resize": { "width": 60, "height": null, "fit": "width", "filter": "lanczos3", "no_upscale": true },
                    "strip_metadata": true
                },
                "format": "jpeg",
                "quality": 80,
                "target": null,
                "name": "photo.jpg",
                "data_base64": b64(&sample(300, 200))
            });
            let body = serde_json::to_vec(&request).unwrap();
            let (status, data, msg) = take(px_process(body.as_ptr(), body.len()));
            assert_eq!(status, PxStatus::Ok as u32, "{msg}");
            let v: serde_json::Value = serde_json::from_slice(&data).unwrap();
            assert_eq!(v["width"], 60);
            assert_eq!(v["height"], 40);
            assert_eq!(v["output_name"], "photo.jpg");
            assert!(v["output_bytes"].as_u64().unwrap() > 0);
            // The returned bytes must themselves be a valid JPEG.
            let bytes: Vec<u8> = serde_json::from_value(v["bytes"].clone()).unwrap();
            assert_eq!(
                crate::format::detect_format(&bytes).unwrap(),
                OutputFormat::Jpeg
            );
        }
    }

    /// Run `px_process` and hand back the encoded image bytes.
    ///
    /// The JSON `bytes` field is a byte array, so this also proves the Dart-side
    /// contract: the output really is the file, not a summary of it.
    fn process_to_bytes(request: &serde_json::Value) -> (Vec<u8>, serde_json::Value) {
        let body = serde_json::to_vec(request).unwrap();
        unsafe {
            let (status, data, msg) = take(px_process(body.as_ptr(), body.len()));
            assert_eq!(status, PxStatus::Ok as u32, "{msg}");
            let v: serde_json::Value = serde_json::from_slice(&data).unwrap();
            let bytes: Vec<u8> = serde_json::from_value(v["bytes"].clone()).unwrap();
            (bytes, v)
        }
    }

    fn process_request(format: &str, chroma: &str, progressive: bool) -> serde_json::Value {
        serde_json::json!({
            "pipeline": {
                "crop": null, "orientation": null, "resize": null,
                "strip_metadata": true, "chroma_subsampling": chroma
            },
            "format": format,
            "quality": 80,
            "progressive": progressive,
            "target": null,
            "name": "photo.jpg",
            "data_base64": b64(&photo(240, 160))
        })
    }

    #[test]
    fn px_process_honours_the_requested_chroma_subsampling() {
        let (full, _) = process_to_bytes(&process_request("jpeg", "luma444", false));
        let (subsampled, _) = process_to_bytes(&process_request("jpeg", "luma420", false));
        assert!(
            full.len() > subsampled.len(),
            "4:4:4 ({}) should exceed 4:2:0 ({}) across the FFI boundary",
            full.len(),
            subsampled.len()
        );
        for bytes in [&full, &subsampled] {
            let back = image::load_from_memory(bytes).unwrap();
            assert_eq!((back.width(), back.height()), (240, 160));
        }
    }

    #[test]
    fn px_process_honours_a_progressive_request() {
        let (baseline, _) = process_to_bytes(&process_request("jpeg", "luma420", false));
        let (progressive, _) = process_to_bytes(&process_request("jpeg", "luma420", true));
        // SOF0 baseline, SOF2 progressive.
        let frame = |bytes: &[u8]| -> u8 {
            bytes
                .windows(2)
                .position(|w| w[0] == 0xFF && (0xC0..=0xCF).contains(&w[1]))
                .map(|at| bytes[at + 1])
                .unwrap_or_else(|| panic!("no frame header in a JPEG"))
        };
        assert_eq!(frame(&baseline), 0xC0);
        assert_eq!(frame(&progressive), 0xC2);
    }

    #[test]
    fn an_older_dart_client_still_works() {
        // No `chroma_subsampling` and no `progressive` in the request, which is
        // what a Dart build from before this phase sends. The defaults have to be
        // the documented ones rather than a deserialisation error, or every
        // existing install breaks on an engine upgrade.
        let request = serde_json::json!({
            "pipeline": { "crop": null, "orientation": null, "resize": null, "strip_metadata": true },
            "format": "jpeg",
            "quality": 80,
            "target": null,
            "name": "photo.jpg",
            "data_base64": b64(&photo(240, 160))
        });
        let (bytes, v) = process_to_bytes(&request);
        assert_eq!(v["width"], 240);
        assert!(image::load_from_memory(&bytes).is_ok());
        // The default is the photo default: same bytes as asking for 4:2:0.
        let (explicit, _) = process_to_bytes(&process_request("jpeg", "luma420", false));
        assert_eq!(
            bytes, explicit,
            "an absent chroma must mean the documented default"
        );
    }

    #[test]
    fn process_rejects_malformed_json() {
        unsafe {
            let body = b"{not json";
            let (status, _, msg) = take(px_process(body.as_ptr(), body.len()));
            assert_eq!(status, PxStatus::InvalidArgument as u32);
            assert!(msg.contains("bad request"));
        }
    }

    #[test]
    fn process_honours_a_byte_target() {
        unsafe {
            let request = serde_json::json!({
                "pipeline": {
                    "crop": null, "orientation": null, "resize": null, "strip_metadata": true
                },
                "format": "jpeg",
                "quality": 90,
                "target": 12000,
                "name": "photo.jpg",
                "data_base64": b64(&photo(600, 400))
            });
            let body = serde_json::to_vec(&request).unwrap();
            let (status, data, msg) = take(px_process(body.as_ptr(), body.len()));
            assert_eq!(status, PxStatus::Ok as u32, "{msg}");
            let v: serde_json::Value = serde_json::from_slice(&data).unwrap();
            // Never print `bytes` here: it is a base64 array and would bury the
            // actual failure in tens of kilobytes of noise.
            let summary = format!(
                "output_bytes={} quality_used={} target_met={}",
                v["output_bytes"], v["quality_used"], v["target_met"]
            );
            assert_eq!(v["target_met"], true, "target not met: {summary}");
            assert!(
                v["output_bytes"].as_u64().unwrap() <= 12_000,
                "overshot the ceiling: {summary}"
            );
            let q = v["quality_used"].as_u64().unwrap();
            assert!(
                (30..=95).contains(&q),
                "quality outside the search range: {summary}"
            );
        }
    }

    #[test]
    fn a_byte_target_actually_shrinks_the_output() {
        // Proves the search did work rather than the request being ignored: the
        // same image, same dimensions, must come out smaller with a ceiling.
        unsafe {
            let run = |target: Option<u64>| -> (u64, u64) {
                let request = serde_json::json!({
                    "pipeline": { "crop": null, "orientation": null, "resize": null, "strip_metadata": true },
                    "format": "jpeg",
                    "quality": 95,
                    "target": target,
                    "name": "photo.jpg",
                    "data_base64": b64(&photo(600, 400))
                });
                let body = serde_json::to_vec(&request).unwrap();
                let (status, data, msg) = take(px_process(body.as_ptr(), body.len()));
                assert_eq!(status, PxStatus::Ok as u32, "{msg}");
                let v: serde_json::Value = serde_json::from_slice(&data).unwrap();
                (
                    v["output_bytes"].as_u64().unwrap(),
                    v["quality_used"].as_u64().unwrap(),
                )
            };

            let (unconstrained, q_unconstrained) = run(None);
            let (constrained, q_constrained) = run(Some(12_000));
            assert!(
                constrained < unconstrained,
                "ceiling had no effect: {constrained} vs {unconstrained}"
            );
            assert!(
                q_constrained < q_unconstrained,
                "quality was not lowered: q{constrained} at {constrained} bytes"
            );
        }
    }

    #[test]
    fn batch_reports_partial_failure() {
        unsafe {
            // The two good files are *different* pictures. A batch deduplicates on
            // content as of phase-14, so two identical fixtures in one request are
            // one output and one `skipped` — which is the behaviour
            // `a_batch_collapses_two_encodings_of_one_picture` tests, and would
            // make this one measure the wrong thing.
            let request = serde_json::json!({
                "crop": null, "orientation": null, "resize": null, "strip_metadata": true,
                "format": "jpeg", "quality": 85, "target": null,
                "files": [
                    { "name": "a.jpg", "bytes": sample(200, 100) },
                    { "name": "b.jpg", "bytes": b"broken".to_vec() },
                    { "name": "c.jpg", "bytes": sample(201, 100) }
                ]
            });
            let body = serde_json::to_vec(&request).unwrap();
            let (status, data, msg) = take(px_batch(body.as_ptr(), body.len()));
            assert_eq!(status, PxStatus::Ok as u32, "{msg}");
            let report: crate::worker::BatchReport = serde_json::from_slice(&data).unwrap();
            assert_eq!(report.succeeded(), 2);
            // A file that is not an image is reported as a skip with a reason, not
            // as a failure: the folder is 90% fine and this is one line of the
            // report saying why the other file is not there.
            assert_eq!(report.failed(), 0);
            assert_eq!(report.skipped(), 1);
        }
    }

    /// Two byte-different encodings of one picture, through the real boundary.
    ///
    /// This is the phase's headline claim as the UI sees it: 400 files in, 370
    /// out, and the 30 named. The fixture is the flat field from
    /// `dedupe::tests::two_qualities_of_one_picture_are_one_picture`, because it
    /// is the shape a JPEG round trip is exact on — two qualities, two different
    /// files, one picture.
    #[test]
    fn a_batch_collapses_two_encodings_of_one_picture() {
        unsafe {
            let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                64,
                48,
                image::Rgb([128, 128, 128]),
            ));
            let at = |q: u8| {
                crate::encode_fixed(
                    &img,
                    OutputFormat::Jpeg,
                    EncodingOptions::default().with_quality(q),
                )
                .unwrap()
            };
            let (high, low) = (at(95), at(75));
            assert_ne!(high, low);

            let request = serde_json::json!({
                "crop": null, "orientation": null, "resize": null, "strip_metadata": true,
                "format": "jpeg", "quality": 85, "target": null,
                "files": [
                    { "name": "one.jpg", "bytes": high },
                    { "name": "two.jpg", "bytes": low }
                ]
            });
            let body = serde_json::to_vec(&request).unwrap();
            let (status, data, msg) = take(px_batch(body.as_ptr(), body.len()));
            assert_eq!(status, PxStatus::Ok as u32, "{msg}");
            let report: crate::worker::BatchReport = serde_json::from_slice(&data).unwrap();
            assert_eq!(report.succeeded(), 1);
            assert_eq!(report.duplicates(), 1);
            let skip = report
                .outcomes
                .iter()
                .find_map(|o| match &o.skipped {
                    Some(crate::worker::SkipReason::Duplicate { of }) => Some(of.clone()),
                    Some(other) => panic!("expected a duplicate, got {other:?}"),
                    None => None,
                })
                .expect("one of the two files must be reported as the duplicate");
            // Not "one.jpg" specifically: a batch runs in parallel and the
            // deduplication table is claimed under a mutex, so which of the two
            // identical pictures is written is the scheduler's choice. What the
            // boundary promises is that the reason reaches Dart at all, names one
            // of the two, and names the one that was written.
            assert!(
                ["one.jpg", "two.jpg"].contains(&skip.as_str()),
                "the duplicate must name one of the two files, not {skip}"
            );
            assert!(
                report.outcomes.iter().any(|o| o.name == skip && o.ok()),
                "the duplicate must name the file that was written"
            );
            // A request from an app that predates this field keeps working: the
            // `policy` key above is absent from the JSON on purpose, and
            // `BatchPolicy` is `#[serde(default)]`, so "collapse duplicates" is
            // what an absent key has to mean. `duplicates() == 1` above *is* that
            // assertion — an older app's batch collapses its duplicates without
            // having been taught the field.
        }
    }

    /// `px_batch` flattens the pipeline into the request rather than nesting it,
    /// so `chroma_subsampling` and `progressive` arrive as top-level keys — and
    /// `serde` ignores a key the struct does not declare, which would leave the
    /// UI's controls silently dead rather than an error. The report carries the
    /// output size and nothing else, and size is exactly what these two options
    /// change.
    #[test]
    fn px_batch_honours_chroma_and_progressive_at_the_top_level() {
        let run = |chroma: &str, progressive: bool| -> usize {
            let request = serde_json::json!({
                "crop": null, "orientation": null, "resize": null,
                "strip_metadata": true, "chroma_subsampling": chroma,
                "format": "jpeg", "quality": 80, "progressive": progressive,
                "target": null,
                "files": [{ "name": "a.jpg", "bytes": photo(240, 160) }]
            });
            let body = serde_json::to_vec(&request).unwrap();
            unsafe {
                let (status, data, msg) = take(px_batch(body.as_ptr(), body.len()));
                assert_eq!(status, PxStatus::Ok as u32, "{msg}");
                let report: crate::worker::BatchReport = serde_json::from_slice(&data).unwrap();
                assert_eq!(report.succeeded(), 1, "{:?}", report.outcomes[0]);
                report.outcomes[0].output_bytes
            }
        };

        let full = run("luma444", false);
        let subsampled = run("luma420", false);
        assert!(
            full > subsampled,
            "the batch path ignored chroma_subsampling: {full} vs {subsampled}"
        );
        let progressive = run("luma420", true);
        assert!(
            progressive > subsampled,
            "the batch path ignored progressive: {progressive} vs {subsampled}"
        );
    }

    #[test]
    fn a_size_ceiling_on_png_travels_back_as_a_sentence() {
        // The refusal is only useful if it survives the boundary: Dart shows
        // `Outcome.error` verbatim, so the sentence has to arrive intact rather
        // than as a status code the UI has to guess about.
        let body = serde_json::to_vec(&serde_json::json!({
            "pipeline": {
                "crop": null, "orientation": null, "resize": null,
                "strip_metadata": true, "chroma_subsampling": "luma420"
            },
            "format": "png",
            "quality": 85,
            "progressive": false,
            "target": 8192,
            "name": "photo.jpg",
            "data_base64": b64(&photo(240, 160))
        }))
        .unwrap();
        unsafe {
            let (status, _, msg) = take(px_process(body.as_ptr(), body.len()));
            assert_ne!(
                status,
                PxStatus::Ok as u32,
                "a ceiling PNG cannot keep must not come back as a success"
            );
            assert!(
                msg.contains("quality") && msg.contains("JPEG"),
                "the message must name the problem and a way out: {msg}"
            );
        }

        // And the batch path reports it per file rather than failing the batch:
        // one impossible request must not discard the rest of a folder.
        let request = serde_json::json!({
            "crop": null, "orientation": null, "resize": null, "strip_metadata": true,
            "format": "png", "quality": 85, "target": 8192,
            "files": [
                { "name": "a.jpg", "bytes": sample(200, 100) },
                { "name": "b.jpg", "bytes": sample(200, 100) }
            ]
        });
        let body = serde_json::to_vec(&request).unwrap();
        unsafe {
            let (status, data, msg) = take(px_batch(body.as_ptr(), body.len()));
            assert_eq!(status, PxStatus::Ok as u32, "{msg}");
            let report: crate::worker::BatchReport = serde_json::from_slice(&data).unwrap();
            assert_eq!(
                report.failed(),
                2,
                "every file carries the same bad setting"
            );
            let error = report.outcomes[0].error.as_deref().unwrap_or("");
            assert!(error.contains("quality"), "{error}");
            assert_eq!(report.outcomes[0].quality_used, 0);
        }
    }

    #[test]
    fn cancelling_a_batch_actually_stops_it() {
        unsafe {
            let handle = px_cancel_new();
            assert_ne!(handle, HANDLE_INVALID);
            assert!(px_cancel_trigger(handle));

            let request = serde_json::json!({
                "crop": null, "orientation": null, "resize": null, "strip_metadata": true,
                "format": "jpeg", "quality": 85, "target": null,
                "cancel": handle,
                "files": (0..4).map(|i| serde_json::json!({
                    "name": format!("{i}.jpg"), "bytes": sample(400, 300)
                })).collect::<Vec<_>>()
            });
            let body = serde_json::to_vec(&request).unwrap();
            let (status, data, _) = take(px_batch(body.as_ptr(), body.len()));
            assert_eq!(status, PxStatus::Ok as u32);
            let report: crate::worker::BatchReport = serde_json::from_slice(&data).unwrap();
            assert!(report.cancelled);
            assert_eq!(report.succeeded(), 0);

            assert!(px_cancel_free(handle));
            assert!(!px_cancel_free(handle), "double free must be rejected");
        }
    }

    #[test]
    fn an_unknown_cancel_handle_is_rejected() {
        assert!(!px_cancel_trigger(999_999_999));
        assert!(!px_cancel_free(999_999_999));
    }

    #[test]
    fn zip_round_trips_through_the_boundary() {
        unsafe {
            let request = serde_json::json!({
                "files": [
                    { "name": "a.jpg", "bytes": sample(40, 30) },
                    { "name": "b.jpg", "bytes": sample(40, 30) }
                ]
            });
            let body = serde_json::to_vec(&request).unwrap();
            let (status, archive, msg) = take(px_zip(body.as_ptr(), body.len()));
            assert_eq!(status, PxStatus::Ok as u32, "{msg}");
            let reader = zip::ZipArchive::new(std::io::Cursor::new(archive)).unwrap();
            assert_eq!(reader.len(), 2);
        }
    }

    #[test]
    fn a_contained_panic_becomes_an_error_buffer() {
        unsafe {
            let (status, _, msg) = take(px_selftest_panic());
            assert_eq!(status, PxStatus::Error as u32);
            assert!(msg.contains("contained"));
        }
    }

    // ---- envelope limits -------------------------------------------------

    /// A JSON array of `len` zero bytes, which is the cheapest way to put `len`
    /// payload bytes in a document: two characters per byte, against four for a
    /// byte at 255. The sparse form is what makes the *payload* ceilings
    /// reachable before the *envelope* ceiling rather than behind it.
    fn sparse_bytes(len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len * 2 + 2);
        out.push(b'[');
        for _ in 0..len {
            out.extend_from_slice(b"0,");
        }
        if len > 0 {
            // The separator after the last element would be a trailing comma.
            out.pop();
        }
        out.push(b']');
        out
    }

    fn zip_body_of_payloads(payloads: &[usize]) -> Vec<u8> {
        let mut body = Vec::from(&b"{\"files\":["[..]);
        for (i, len) in payloads.iter().enumerate() {
            if i > 0 {
                body.push(b',');
            }
            body.extend_from_slice(b"{\"name\":\"f");
            body.extend_from_slice(i.to_string().as_bytes());
            body.extend_from_slice(b".jpg\",\"bytes\":");
            body.extend_from_slice(&sparse_bytes(*len));
            body.push(b'}');
        }
        body.extend_from_slice(b"]}");
        body
    }

    /// An envelope past the ceiling is refused by all three request entry points,
    /// and refused *before* the document is parsed.
    ///
    /// "Before it is parsed" is the half that matters: `Limits::max_input_bytes`
    /// is consulted inside `validate_bytes`, which runs after `serde_json` has
    /// built the whole document and every `Vec<u8>` in it, so a ceiling checked
    /// there bounds nothing. The assertion that the sentence is the envelope's
    /// own rather than serde's is what proves the refusal came first — a document
    /// this size does not parse in any reasonable time, so a test that waited for
    /// a parse error would hang rather than fail.
    #[test]
    fn an_envelope_past_the_ceiling_is_refused_before_it_is_parsed() {
        // Two characters per payload byte, so this is a document just over
        // `MAX_REQUEST_BYTES` carrying half that many payload bytes.
        let payload = MAX_REQUEST_BYTES / 2 + 1024;
        let body = zip_body_of_payloads(&[payload]);
        assert!(
            body.len() > MAX_REQUEST_BYTES,
            "the fixture is {} bytes, which is not over the ceiling",
            body.len()
        );

        unsafe {
            for (which, (status, data, msg)) in [
                ("px_zip", take(px_zip(body.as_ptr(), body.len()))),
                ("px_batch", take(px_batch(body.as_ptr(), body.len()))),
                ("px_process", take(px_process(body.as_ptr(), body.len()))),
            ] {
                assert_eq!(status, PxStatus::InvalidArgument as u32, "{which}: {msg}");
                assert!(
                    data.is_empty(),
                    "{which} returned data for a refused request"
                );
                assert!(
                    msg.contains(&MAX_REQUEST_BYTES.to_string()),
                    "{which}: the refusal does not name the limit: {msg}"
                );
                assert!(
                    !msg.contains("bad request"),
                    "{which}: the refusal came from serde rather than from the check: {msg}"
                );
                assert!(!msg.contains("line"), "{which}: {msg}");
            }
        }
    }

    /// More files in one envelope than one request holds.
    ///
    /// Cheap to build and cheap to parse, which is the point: 513 empty files are
    /// about 12 KB, so the count ceiling is reachable without a large document
    /// and a test can cross it honestly.
    #[test]
    fn a_request_carrying_too_many_files_is_refused_with_the_count() {
        let body = zip_body_of_payloads(&vec![0; MAX_REQUEST_FILES + 1]);
        unsafe {
            let (status, _, msg) = take(px_batch(body.as_ptr(), body.len()));
            assert_eq!(status, PxStatus::InvalidArgument as u32);
            assert!(
                msg.contains("more than") && msg.contains(&MAX_REQUEST_FILES.to_string()),
                "the refusal does not name the file count: {msg}"
            );
        }
    }

    /// One file larger than a single export may be.
    ///
    /// The message names the file, because the two payload refusals fail
    /// differently for a user: "this file is 8388609 bytes" is something to act
    /// on, "the request is too big" is not.
    #[test]
    fn one_file_past_the_per_file_ceiling_is_refused_by_name() {
        let body = zip_body_of_payloads(&[MAX_REQUEST_FILE_BYTES + 1]);
        unsafe {
            for (which, (status, _, msg)) in [
                ("px_batch", take(px_batch(body.as_ptr(), body.len()))),
                ("px_zip", take(px_zip(body.as_ptr(), body.len()))),
            ] {
                assert_eq!(status, PxStatus::InvalidArgument as u32, "{which}");
                assert!(msg.contains("f0.jpg"), "{which}: no file name: {msg}");
                assert!(
                    msg.contains(&MAX_REQUEST_FILE_BYTES.to_string()),
                    "{which}: no limit named: {msg}"
                );
            }
        }
    }

    /// Modest files that add up to more than the envelope holds.
    ///
    /// Two files at the per-file ceiling are legal — that is the point of a
    /// separate per-file number — so this is the third file that tips the total,
    /// which is what makes the two ceilings distinct rather than one number
    /// written twice.
    #[test]
    fn files_that_add_up_past_the_payload_ceiling_are_refused_as_a_total() {
        let body = zip_body_of_payloads(&[MAX_REQUEST_FILE_BYTES, MAX_REQUEST_FILE_BYTES, 1]);
        unsafe {
            let (status, _, msg) = take(px_batch(body.as_ptr(), body.len()));
            assert_eq!(status, PxStatus::InvalidArgument as u32);
            assert!(
                msg.contains("add up") && msg.contains(&MAX_REQUEST_PAYLOAD_BYTES.to_string()),
                "the refusal does not name the total: {msg}"
            );
        }
    }

    /// A batch inside every ceiling still works.
    ///
    /// The bound is only worth having if it refuses what it should and nothing
    /// else, so this is the negative case for all three numbers at once.
    #[test]
    fn a_batch_inside_the_envelope_ceilings_still_works() {
        let input = sample(64, 48);
        let body = serde_json::to_vec(&serde_json::json!({
            "crop": null, "orientation": null, "resize": null, "strip_metadata": true,
            "format": "jpeg", "quality": 85, "target": null,
            "files": [{ "name": "a.jpg", "bytes": input }]
        }))
        .unwrap();
        unsafe {
            let (status, data, msg) = take(px_batch(body.as_ptr(), body.len()));
            assert_eq!(status, PxStatus::Ok as u32, "{msg}");
            let report: crate::worker::BatchReport = serde_json::from_slice(&data).unwrap();
            assert_eq!(report.succeeded(), 1, "{msg}");
            // And the refusal slot cannot leak into the next call: this one
            // follows a refused request in the same thread in the tests above, and
            // a stale sentence would turn a successful batch into an error.
            let (status, data, msg) = take(px_batch(body.as_ptr(), body.len()));
            assert_eq!(status, PxStatus::Ok as u32, "{msg}");
            assert!(!data.is_empty());
        }
    }

    #[test]
    fn buffers_can_be_freed_safely_and_repeatedly() {
        unsafe {
            let buffer = px_version();
            let ptr = buffer.data;
            px_buffer_free(buffer);
            // Freeing a null-data buffer must be a no-op, not a crash.
            px_buffer_free(PxBuffer {
                status: 1,
                data: std::ptr::null_mut(),
                len: 0,
                error: std::ptr::null_mut(),
            });
            assert!(!ptr.is_null());
        }
    }

    /// Freeing the same buffer twice is a no-op the second time.
    ///
    /// The property is not "a second free does not crash" — it is that the
    /// bookkeeping that makes the first free legal is *consumed* by it. A second
    /// free that found the entry still there would rebuild a `Vec` from a pointer
    /// the allocator has already reclaimed, which is a double free.
    #[test]
    fn a_second_free_of_the_same_buffer_is_a_no_op() {
        fn live() -> usize {
            CAPACITIES.with(|c| c.borrow().len())
        }
        unsafe {
            let buffer = px_version();
            let kept = PxBuffer {
                status: buffer.status,
                data: buffer.data,
                len: buffer.len,
                error: buffer.error,
            };
            assert_eq!(live(), 1, "one buffer is outstanding");
            px_buffer_free(buffer);
            assert_eq!(live(), 0, "the first free did not take the entry");

            // The same pointer and length, as a caller that kept a copy of the
            // struct across a round trip through Dart would hand back.
            px_buffer_free(kept);
            assert_eq!(live(), 0, "the second free invented an entry");
        }
    }

    /// A `PxBuffer` whose `len` field has been corrupted still frees exactly once,
    /// and does not take the process down doing it.
    ///
    /// This is the audit's finding 8, and the assertion that matters is that the
    /// call *returns*: `Vec::from_raw_parts` asserts `cap >= len` inside
    /// `RawVec::cap_set`, so the pre-fix code — which took the length from the
    /// caller — panicked here, inside a `no_mangle` function that a host calls
    /// for cleanup. Hard rule 3 is about not panicking on numbers somebody else
    /// supplied, and `len` is as supplied as `ptr` is.
    #[test]
    fn a_buffer_with_a_corrupted_length_frees_exactly_once() {
        for wrong_len in [usize::MAX, 1 << 40, 0] {
            let live = CAPACITIES.with(|c| c.borrow().len());
            unsafe {
                let buffer = px_version();
                let (len, cap) =
                    CAPACITIES.with(|c| *c.borrow().get(&(buffer.data as usize)).unwrap());
                assert!(len <= cap, "the registry holds an impossible Vec");
                px_buffer_free(PxBuffer {
                    status: buffer.status,
                    data: buffer.data,
                    len: wrong_len,
                    error: buffer.error,
                });
                assert_eq!(
                    CAPACITIES.with(|c| c.borrow().len()),
                    live,
                    "len {wrong_len}: the free did not take the entry"
                );
            }
        }
    }

    #[test]
    fn process_rejects_a_request_with_no_image_data() {
        unsafe {
            let request = serde_json::json!({
                "pipeline": { "crop": null, "orientation": null, "resize": null, "strip_metadata": true },
                "format": "jpeg",
                "data_base64": ""
            });
            let body = serde_json::to_vec(&request).unwrap();
            let (status, _, msg) = take(px_process(body.as_ptr(), body.len()));
            assert_eq!(status, PxStatus::InvalidArgument as u32, "{msg}");
            assert!(msg.contains("no image data"), "{msg}");
        }
    }

    #[test]
    fn process_rejects_data_that_is_not_base64() {
        unsafe {
            let request = serde_json::json!({
                "pipeline": { "crop": null, "orientation": null, "resize": null, "strip_metadata": true },
                "format": "jpeg",
                "data_base64": "this is definitely not base64!!!"
            });
            let body = serde_json::to_vec(&request).unwrap();
            let (status, _, msg) = take(px_process(body.as_ptr(), body.len()));
            assert_eq!(status, PxStatus::InvalidArgument as u32, "{msg}");
            assert!(msg.contains("base64"), "{msg}");
        }
    }

    #[test]
    fn process_rejects_a_payload_that_decodes_to_junk() {
        // Valid base64, not an image. This must surface as a codec error, not a
        // crash and not a silently returned empty file.
        unsafe {
            let request = serde_json::json!({
                "pipeline": { "crop": null, "orientation": null, "resize": null, "strip_metadata": true },
                "format": "jpeg",
                "data_base64": b64(b"not an image, just some bytes")
            });
            let body = serde_json::to_vec(&request).unwrap();
            let (status, data, msg) = take(px_process(body.as_ptr(), body.len()));
            assert_eq!(status, PxStatus::Error as u32);
            assert!(data.is_empty());
            assert!(!msg.is_empty());
        }
    }

    #[test]
    fn presets_are_reachable_from_the_boundary() {
        /// Owned mirror of `presets::Preset`.
        ///
        /// `Preset` holds `&'static str`, so it can be serialised out but not
        /// read back — a `Deserialize` impl would need to borrow from the input
        /// buffer for `'static`. This mirror is what a Dart consumer does when
        /// it parses `px_presets()`, and duplicating the shape here means the
        /// contract is actually tested rather than assumed.
        #[derive(Debug, serde::Deserialize)]
        struct PresetMirror {
            id: String,
            label: String,
            width: u32,
            height: Option<u32>,
            format: OutputFormat,
        }

        unsafe {
            let (status, data, message) = take(px_presets());
            assert_eq!(status, PxStatus::Ok as u32, "{message}");

            let list: Vec<PresetMirror> = serde_json::from_slice(&data).unwrap();
            assert_eq!(list.len(), presets::PRESETS.len());

            let by_id = |id: &str| list.iter().find(|p| p.id == id).unwrap();
            let ig = by_id("ig-post");
            assert_eq!((ig.width, ig.height), (1080, Some(1080)));
            assert_eq!(ig.format, OutputFormat::Jpeg);
            assert!(!ig.label.is_empty());

            // Every preset must survive the boundary with the fields the UI
            // depends on intact.
            for p in &list {
                assert!(!p.id.is_empty(), "a preset has an empty id");
                assert!(!p.label.is_empty(), "{} has an empty label", p.id);
                assert!(p.width > 0, "{} has zero width", p.id);
            }
        }
    }
}
