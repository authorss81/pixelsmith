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
    /// Original `Vec` capacities, keyed by pointer.
    ///
    /// Tracked out of band so `PxBuffer` keeps a layout Dart can rely on. If
    /// this is ever lost, the next `px_buffer_free` for that pointer would
    /// rebuild a `Vec` with the wrong capacity, which is undefined behaviour.
    static CAPACITIES: std::cell::RefCell<std::collections::HashMap<usize, usize>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Move a `Vec` into a raw pointer, remembering its capacity.
///
/// Uses `Vec::into_raw_parts` so the caller can `Vec::from_raw_parts(ptr, len,
/// cap)` exactly, with no sentinel and no double-free hazard.
fn into_raw(mut v: Vec<u8>) -> PxBuffer {
    let len = v.len();
    let cap = v.capacity();
    let ptr = v.as_mut_ptr();
    std::mem::forget(v);
    CAPACITIES.with(|c| c.borrow_mut().insert(ptr as usize, cap));
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
/// # Safety
/// `buffer` must be a value returned by this library and not already freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn px_buffer_free(buffer: PxBuffer) {
    if buffer.data.is_null() {
        return;
    }
    let cap = CAPACITIES.with(|c| c.borrow_mut().remove(&(buffer.data as usize)));
    let Some(cap) = cap else {
        // Not one of ours, or already freed. Leaking is strictly better than
        // rebuilding a `Vec` from a pointer we do not own.
        return;
    };
    // SAFETY: `data`, `len` and `cap` all came from `into_raw`.
    drop(unsafe { Vec::from_raw_parts(buffer.data, buffer.len, cap) });
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
#[derive(Debug, Deserialize)]
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
    let parsed: ProcessRequest = match serde_json::from_slice(raw) {
        Ok(v) => v,
        Err(e) => return err(format!("bad request: {e}"), PxStatus::InvalidArgument),
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
#[derive(Debug, Deserialize)]
struct BatchRequest {
    #[serde(flatten)]
    pipeline: Pipeline,
    format: OutputFormat,
    #[serde(default = "default_quality")]
    quality: u8,
    /// Scan-by-scan JPEG. See `ProcessRequest::progressive`.
    #[serde(default)]
    progressive: bool,
    target: Option<u64>,
    #[serde(default)]
    mobile_limits: bool,
    cancel: Option<PxHandle>,
    /// Inputs as base64-free raw arrays would bloat the JSON, so each entry is
    /// `{name, bytes}` where bytes is a JSON array of byte values. Dart builds
    /// this from its `Uint8List`; the alternative (base64) costs 33% size.
    files: Vec<BatchFile>,
}

#[derive(Debug, Deserialize)]
struct BatchFile {
    name: String,
    bytes: Vec<u8>,
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
    let parsed: BatchRequest = match serde_json::from_slice(raw) {
        Ok(v) => v,
        Err(e) => return err(format!("bad request: {e}"), PxStatus::InvalidArgument),
    };

    let cancel = match parsed.cancel {
        Some(h) => tokens()
            .lock()
            .ok()
            .and_then(|guard| guard.iter().find(|(k, _)| *k == h).map(|(_, t)| t.clone())),
        None => None,
    };
    let cancel = cancel.unwrap_or_default();

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
    };
    let pipeline = Pipeline {
        strip_metadata: parsed.pipeline.strip_metadata,
        ..parsed.pipeline
    };

    let jobs: Vec<Job> = parsed
        .files
        .into_iter()
        .enumerate()
        .map(|(i, f)| Job {
            id: i.to_string(),
            name: f.name,
            bytes: f.bytes,
        })
        .collect();

    let report = crate::worker::process_batch(&jobs, &pipeline, &settings, &cancel);
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
        files: Vec<BatchFile>,
    }
    let borrowed = unsafe { borrow(request, request_len) };
    let Some(raw) = borrowed else {
        return err("null request", PxStatus::InvalidArgument);
    };
    let parsed: ZipRequest = match serde_json::from_slice(raw) {
        Ok(v) => v,
        Err(e) => return err(format!("bad request: {e}"), PxStatus::InvalidArgument),
    };
    let items: Vec<(String, Vec<u8>)> = parsed
        .files
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
            let request = serde_json::json!({
                "crop": null, "orientation": null, "resize": null, "strip_metadata": true,
                "format": "jpeg", "quality": 85, "target": null,
                "files": [
                    { "name": "a.jpg", "bytes": sample(200, 100) },
                    { "name": "b.jpg", "bytes": b"broken".to_vec() },
                    { "name": "c.jpg", "bytes": sample(200, 100) }
                ]
            });
            let body = serde_json::to_vec(&request).unwrap();
            let (status, data, msg) = take(px_batch(body.as_ptr(), body.len()));
            assert_eq!(status, PxStatus::Ok as u32, "{msg}");
            let report: crate::worker::BatchReport = serde_json::from_slice(&data).unwrap();
            assert_eq!(report.succeeded(), 2);
            assert_eq!(report.failed(), 1);
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
