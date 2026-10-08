# Changelog

All notable changes to `pixelsmith_core` are recorded here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Low-peak-memory decode, behind the off-by-default `streaming` feature.**
  `stream::decode_resized` decodes a row at a time and resamples it into the
  destination in the same pass, so peak memory is the output buffer plus one row
  rather than two copies of the source plus the kernel's `f32` intermediate.
  Measured at 120 MP resized to 1000 px wide: **613 MB in memory, 4 MB
  streaming**, agreeing with `resize::resample_reference` to one least-significant
  bit across five filters and seven size pairs. One resampling pass, not two: the
  horizontal and vertical passes commute, so the two paths run the same filter in
  the order that does not need the whole source in memory.
  `Limits::streamed_pixels_budget()` and `Limits::streaming_memory_budget()` bound
  it — methods, not fields, so the JSON contract the Dart side mirrors is
  unchanged and the in-memory path's ceiling is untouched.
- **AVIF encode, on by default.** The `avif` feature is back and in `default`,
  writing through `image`'s own encoder (rav1e via ravif) rather than the
  hand-written one that never compiled. It is pure Rust with no C toolchain and
  no build script, which is why it does not carry the caveats the `webp-lossy`
  and `heic` features do. What it costs is time: 3.2 s for a 1600×1200 photo
  against 20 ms for JPEG, so a single AVIF export is a progress bar.
  `capabilities().avif_decode` stays `false` — this tree has no AV1 decoder, and
  writing a format the build cannot read back is only honest because the flag
  says so.
- **Progressive JPEG.** `EncodingOptions::progressive` writes scan-by-scan output,
  so an image appears at all on a slow connection. It costs about 64% in file
  size at q85, so it is off by default and JPEG-only.
- **Chroma subsampling control.** `EncodingOptions::chroma_subsampling`
  (`Luma444`, `Luma422`, `Luma420`) is honoured by the JPEG encoder, carried by
  `Pipeline` and `Settings`, and exposed in the JSON request. The default is
  4:2:0 for photographs; `Preset::chroma` is 4:4:4 for `store-screenshot`, whose
  content is text on a coloured background. `ChromaSubsampling::trade_off()` is
  a one-line explanation a UI can show verbatim, and the argument for the default
  is in `docs/ARCHITECTURE.md`.
- `Capabilities::jpeg_progressive` and `Capabilities::jpeg_chroma_subsampling`,
  derived from the same predicates the UI reads per format rather than written
  as constants, so the list cannot describe a build that no longer exists.
- **Lossy WebP, on by default.** The `webp-lossy` feature is in `default`, writing
  through `webp` (libwebp, the reference encoder). Until now WebP output was
  lossless, which meant no quality setting and no byte ceiling: 101,656 bytes
  against 2,638 at q80 on the 600×400 `format::tests::photo` fixture, 39×. The
  `web-hero`, `web-card` and `web-thumb` presets were exporting with their byte
  ceilings dropped by `to_pipeline`; they are kept now, and
  `format::tests::webp_byte_ceilings_are_reachable` proves the search meets them.
  What it costs is a C toolchain — `libwebp-sys` compiles 159 vendored `.c` files
  with `cc`, about 39 s on a cold release build, and no new compiler. The
  per-target matrix that checks it is
  `workspace/phase-08/build-webp-lossy-matrix.patch` — Linux x64, Windows x64 and
  arm64, macOS x64 and arm64, Android arm64 and x86_64, and iOS arm64 — and it is
  **not applied yet**, because this pipeline cannot push to `.github/workflows/`.
  Until it is, lossy WebP is claimed for Linux x64 only, which is the one target
  the gate really builds and runs it on. The lossless fallback is kept and
  `is_lossless()` reports the truth for a build without the feature.

### Changed

- `format::encode`, `lib::encode_fixed`, `lib::process`, `lib::encode_to_target`,
  `target::Encoder` and `worker::Settings` take an `EncodingOptions` instead of a
  bare `quality: u8`. Its default is 85, the value every preset already assumed,
  so no existing export changes.
- JPEG is written through `jpeg-encoder` rather than `image`'s encoder. That
  encoder is baseline 4:4:4 with no progressive option, and it picks its own
  sampling factor from the quality value — which would have made a user's quality
  slider silently change the picture's colour resolution. `docs/JPEG.md` is the
  comparison, with the measurements.
- `Outcome::quality_used` is 0 when the format has no quality setting, instead of
  reporting the requested number next to a file the slider never influenced.
- A byte ceiling on a format with no quality setting is now refused with
  `Error::NoQualitySetting` and a sentence naming a format that can keep it,
  rather than returning an oversized file with `target_met: true`. A *quality*
  value on the same format is still ignored: one global slider sits above the
  format picker, and refusing every PNG export because of it would be absurd.
- **WebP encodes through libwebp on an opaque picture.** `from_rgb` is used rather
  than `from_rgba` when no pixel is less than fully opaque — 25% less input handed
  to the encoder. The *file* is byte-identical either way, because libwebp drops a
  constant alpha plane itself; `format::tests` asserts the equality rather than the
  smaller-than claim the phase prompt asked for, with the measurement attached.
- **A picture over 16383 px a side is refused for WebP.** `validate::Limits` allows
  30000 and libwebp stops at 16383, so an 18000×100 panorama passed every limit,
  decoded and resized, and then died inside `webp::Encoder::encode`, which unwraps
  internally. `encode_simple` returns the error instead and `check_webp_dimensions`
  refuses first, naming the limit and a format that can do it.

### Known limitations

- **AVIF cannot be read back.** Recognised on input by brand bytes, refused by
  name on decode, and reported as `avif_decode: false`. AV1 decode is a second
  codec and is not in this tree.
- **HEIC cannot be decoded in the default build.** Recognised by `ftyp` brand bytes
  without any feature, bounded from the `ispe` property before the codec allocates,
  and reported as `heic_decode: false`. The `heic` feature is off because nothing
  has cross-compiled it for the shipped targets, and the `webp-lossy-matrix` job
  does not build it.
- **WebP has no chroma setting.** Not an unwired knob: libwebp's `WebPConfig` has
  29 fields and none is a chroma sampling factor, because VP8 always stores 4:2:0.
  `supports_chroma_subsampling()` reports `false` rather than offering a slider
  that cannot move.

### Fixed

- **`Pipeline::apply` no longer clones the decoded source.** It ran
  `img.clone()` before every transform, so a straight resize paid for two copies
  of the whole image: about 1.1 GB for a 120 MP source and a 3.3 MB output, now
  613 MB. The source is borrowed until the first step that has to build a new
  buffer.


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