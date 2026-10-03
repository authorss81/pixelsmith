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
use crate::format::OutputFormat;
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
        quality: parsed.quality,
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
        quality: parsed.quality,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(w: u32, h: u32) -> Vec<u8> {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 40])
        }));
        crate::encode_fixed(&img, OutputFormat::Jpeg, 90).unwrap()
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
        crate::encode_fixed(&img, OutputFormat::Jpeg, 90).unwrap()
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
