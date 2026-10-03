# Changelog

All notable changes to `pixelsmith_core` are recorded here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Nothing yet. The next entry appears when a phase lands.

## [0.1.0]

First version of the engine. It is a library plus a C ABI shim; there is no
installable application yet (`app/` does not exist until phase-05).

### Added

**Decoding and format handling**

- `format::detect_format` identifies a file from its magic bytes. Filenames and
  extensions are never trusted to decide what a file is (hard rule 7).
- Decode of JPEG, PNG, WebP, GIF, TIFF, BMP and ICO via the `image` crate.
- Encode to JPEG, PNG, WebP, GIF, TIFF, BMP and ICO. AVIF is recognised on input
  and reports itself in `capabilities()`, but `format::encode` rejects it with
  `Error::UnsupportedFormat` rather than pretending to support it (hard rule 10).
- `OutputFormat` exposes `extension`, `mime`, `is_lossless`, `supports_alpha`,
  `supports_animation`, `supports_quality` and `supports_byte_target`, so a UI
  can grey out what the build cannot produce instead of failing at export time.

**Transform**

- `pipeline::Pipeline` runs crop → orient → resize, in that order, with at most
  one resampling pass (hard rule 5).
- Five resampling filters (`Lanczos3`, `CatmullRom`, `Triangle`, `Nearest`,
  `Box`) and five fit modes (`Contain`, `Cover`, `Fill`, `Width`, `Height`),
  plus automatic downscaling of the filter to `Triangle` for reductions below a
  third of the source's long side, where convolution kernels alias badly.
- All eight EXIF orientation values, composed from `imageops` primitives.
- `Pipeline::output_dimensions` predicts the result size without touching pixels,
  for a live preview.

**Limits**

- `validate::Limits` with two profiles: `Limits::default()` (512 MiB, 128 MP,
  30 000 px per side) and `Limits::mobile()` (128 MiB, 40 MP, 16 000 px).
- `image::Limits` are pushed into the decoder *before* decoding, and the header
  is read on its own, so a 60000×60000 file costs a few hundred bytes of
  inspection rather than 14 GB of allocation (hard rule 4).
- `validate::inspect` returns a `ValidateReport` — format, dimensions,
  animation, EXIF presence, sensitive tags, orientation, and a human-readable
  `suspicious` note — so the UI can warn before the user commits to anything.

**Metadata**

- `exif::read` summarises EXIF for the report.
- `exif::strip` strips by re-encoding from raw samples rather than by clearing
  tags, so maker notes and embedded thumbnails cannot survive (hard rule 6).
- `exif::write_back` re-attaches a filtered subset only on explicit opt-in, and
  drops anything matching GPS, SERIAL, OWNER, ARTIST, COPYRIGHT, MAKERNOTE,
  LENS, BODY or THUMBNAIL. `worker::process_one` always calls it with
  `keep_gps: false`.
- `format::append_exif` splices an APP1 segment in after SOI by hand, so JPEG
  metadata can be re-attached without a decode round-trip and without touching
  the compressed scan data.

**Byte targets**

- `target::TargetBytes` solves "fit under N bytes" by binary search over quality
  rather than a linear ramp, converging in roughly seven encoder passes and
  always returning the highest quality that fits.
- The encoder is injected as a `target::Encoder` closure so the search is unit
  tested without allocating pixels.

**Presets**

- `presets::PRESETS`: a catalogue grouped into `Social`, `Web`, `Print`,
  `Device`, `Email` and `Developer`, each carrying a `category` so the UI can
  present tabs. Custom values are always allowed.

**Batch work**

- `worker::process_one` is the single unit of work behind both the single-file
  and batch paths, so the two cannot diverge.
- `worker::process_batch` runs files in parallel with Rayon and returns a
  `BatchReport` containing per-file outcomes. One unreadable file in a folder of
  200 never discards the other 199.
- `worker::CancelToken` cancels between files rather than mid-file, so a batch
  never leaves a half-written JPEG behind.
- `worker::zip_outputs` assembles a deflate ZIP.
- `worker::sanitise_stem` and `sanitise_path_component` filter path separators,
  traversal and reserved names out of every output name (hard rule 7).

**C ABI**

- `ffi`: 14 `px_*` entry points (`px_version`, `px_inspect`, `px_exif`,
  `px_presets`, `px_process`, `px_batch`, `px_zip`, `px_buffer_free`,
  `px_string_free`, `px_cancel_new`, `px_cancel_trigger`, `px_cancel_free`,
  `px_selftest_error`, `px_selftest_panic`).
- Every result is a `#[repr(C)] PxBuffer` with a `status` tag, so there is no
  path where Dart receives a null it might dereference (hard rule 8).
- Buffers are length-prefixed rather than NUL-terminated, so binary data
  containing a zero byte crosses the boundary intact.
- `px_buffer_free` is idempotent: the `Vec` capacity is tracked in a thread-local
  map keyed by address, and a second free is a no-op rather than undefined
  behaviour.
- Dart-supplied pointers are checked before use; a null pointer with a non-zero
  length yields `PxStatus::InvalidArgument` and is never dereferenced.
- Cancellation tokens are opaque `u64` handles, capped at 1024 live tokens, with
  no value that can cause a wild access.

**Errors**

- `error::Error` is a single enum covering every failure mode, each carrying a
  message written for the user rather than the engineer (hard rule 9). Nothing
  unwinds out of a public entry point.

**Security posture**

- The engine has no network capability of any kind: no HTTP, TLS, socket, DNS or
  URL-fetching dependency, and no `std::net` reference in `core/src`. This is
  enforced mechanically by `scripts/verify.sh` and again in CI.
- The release profile builds with `panic = "abort"`, `lto = "thin"`,
  `codegen-units = 1` and `strip = true`.

### Known limitations

- **No AVIF encoder.** Recognised on input, rejected on encode. phase-07.
- **WebP output is lossless by default.** The `webp-lossy` feature is off, so
  there is no quality control and no byte ceiling for WebP until phase-08 turns
  it on.
- **Animation is not preserved.** Animated GIF is detected and reported, and
  `GifEncoder` writes a single full-colour frame. Animated WebP and APNG are
  reported as stills so the UI can say so rather than quietly flattening them.
  phase-13 handles this honestly.
- **No colour management.** sRGB is assumed throughout; ICC profiles and
  Display-P3 are neither read nor converted. phase-12.
- **No fuzzing yet.** The decode paths have unit tests but no fuzz harness.
  phase-02.
- **Tests run on the host only.** No target matrix, no benchmarks, no
  regression gate. phase-08 through phase-10.