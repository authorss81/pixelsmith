# Architecture

The shape of `core/` today: what each module owns, the order transforms run in,
where the limits are enforced, and the traps that have already cost somebody an
afternoon. Read this before changing `core/src`.

The rules this code exists to satisfy are in `AGENTS.md`. Where this document
and `AGENTS.md` disagree, `AGENTS.md` wins.

## Module map

Ten modules, no submodules, no circular references. Dependencies point downward:
`lib.rs` → `worker.rs` / `pipeline.rs` → `validate.rs` → `error.rs`. Nothing
below `error.rs` knows anything exists above it.

| Module | Owns | Key types |
| --- | --- | --- |
| `core/src/lib.rs` | The crate root: the re-export surface, the honest capability list, and the four functions that make up the plain-Rust API (decode, transform, encode, encode-to-target). | `Capabilities`, `capabilities()`, `process()`, `decode_bounded()`, `encode_fixed()`, `encode_to_target()` |
| `core/src/error.rs` | Every failure mode in the engine, as one enum, plus the `Result<T>` alias. Nothing unwinds out of a public entry point, so a malformed file cannot take down the host process. | `Error`, `Result<T>` |
| `core/src/format.rs` | What a format *is*: the `OutputFormat` enum, magic-byte detection, the encode dispatch, JPEG APP1 splicing, and output-capacity estimation. Reading is broader than writing, which is normal. | `OutputFormat`, `detect_format()`, `encode()`, `append_exif()` |
| `core/src/validate.rs` | Everything that stands between an untrusted byte slice and an allocation: the limit profiles, the header-only check, and the report the UI shows before the user commits. | `Limits`, `Limits::mobile()`, `validate_bytes()`, `inspect()`, `ValidateReport` |
| `core/src/pipeline.rs` | The transform. Owns the crop/orient/resize order and the EXIF orientation table. Knows nothing about files, encoders or Dart. | `Pipeline`, `CropSpec`, `Orientation`, `ResizeSpec`, `FitMode`, `ResampleFilter`, `resize_to()` |
| `core/src/exif.rs` | Metadata. Reading for the report, stripping by re-encoding, and a filtered write-back that never carries GPS or maker notes. | `read()`, `strip()`, `write_back()`, `ExifInfo` |
| `core/src/target.rs` | "Make this file fit under N bytes", solved by binary search over quality rather than a slider the user has to guess at. | `TargetBytes`, `Encoder` (injected so the search is testable without pixels) |
| `core/src/presets.rs` | The preset catalogue, grouped by intent and carrying a `category` so the UI can present tabs. Custom values are always allowed. | `Preset`, `PRESETS`, `all_presets()`, `find_preset()`, `to_pipeline()` |
| `core/src/worker.rs` | The layer the UI actually calls: per-file work, Rayon parallelism, cancellation, ZIP output, per-file accounting, and filename sanitisation. | `Job`, `Settings`, `Outcome`, `BatchReport`, `CancelToken`, `process_one()`, `process_batch()`, `sanitise_stem()` |
| `core/src/ffi.rs` | The C ABI, where Rust's safety stops protecting the caller. 14 `px_*` entry points, a tagged result struct, and buffer ownership rules. | `PxBuffer`, `PxStatus`, `PxHandle`, `px_*` |

## Request path

One image, one file, front to back. `worker::process_one` is the only path that
does real work; the batch path and the FFI path both call it, so a batch result
and a one-off result cannot drift apart.

```
bytes off disk
  → validate::validate_bytes   input size, magic bytes, header dimensions
  → lib::decode_bounded        decoder limits applied, then decode, then re-check
  → pipeline::Pipeline::apply  crop → orient → resize (one resampling pass)
  → exif::strip                re-encode from raw samples (drops EXIF for real)
  → target / format::encode    byte-target search, or a fixed-quality encode
  → format::append_exif        only if metadata was explicitly kept, JPEG only
  → worker::sanitise_stem      output name, filtered against traversal
  → Vec<u8>
```

## The fixed transform order: crop → orient → resize

`pipeline::Pipeline::apply` runs exactly three steps, always in this order, and
at most one of them resamples. The order is not a preference; each position is
load-bearing.

1. **Crop first**, so the crop rectangle is expressed in the coordinates the
   user actually saw in the preview. Cropping after orientation means the
   rectangle has to be re-derived for the rotated axes, and getting it subtly
   wrong crops the wrong part of the picture.
2. **Orient second**, because an un-oriented image has the wrong aspect ratio.
   EXIF values 5–8 swap width and height, so resizing first letterboxes against
   the wrong axis: a 90°-rotated phone photo resized to "width 1920" comes out
   1080×1920 with the wrong content in frame.
3. **Resize last**, so exactly one resampling pass ever touches the pixels. Two
   passes is the single most common cause of "why is my export blurry", and it
   is unrecoverable afterwards — you cannot un-blur it.

`Pipeline::output_dimensions` mirrors the same three steps without touching
pixels, so the UI can show "1920 × 1080" while a slider is being dragged. If
that function and `apply` ever disagree, the preview is lying.

`resize_to` also drops to a `Triangle` filter for extreme reductions (target
side below a third of the source's long side), because any convolution kernel
aliases badly when decimating that hard.

## Limits: where they are enforced

`validate::Limits` has two profiles. `Limits::default()` is 512 MiB / 128 MP /
30 000 px per side; `Limits::mobile()` is 128 MiB / 40 MP / 16 000 px, because a
128 MP decode OOMs a phone rather than merely slowing it down.

The rule is that nothing is allocated before a limit has been consulted. The
enforcement points, in the order a file meets them:

| # | Where | What it stops |
| --- | --- | --- |
| 1 | `validate::validate_bytes` (`validate.rs:112`) | `input.len() > max_input_bytes` before any parsing. |
| 2 | `validate::validate_bytes` → `format::detect_format` | Bytes that are not an image at all. |
| 3 | `Limits::apply_to_decoder` (`validate.rs:46`) | Pushes `image::Limits` into the reader *before* decode: `max_image_width`/`max_image_height` = `max_dimension`, `max_alloc` = `max_pixels × 4`. This is the point that stops a 60000×60000 header, which costs ~14 GB once decoded. |
| 4 | `reader.into_dimensions()` in `validate_bytes` | Reads the header only — no pixel buffer exists yet — and records a human-readable `suspicious` note when dimensions or pixel count are over budget. |
| 5 | `lib::decode_bounded` → `Limits::check_decoded` (`validate.rs:68`) | Post-decode assertion, for headers that lied about their size. Zero dimensions are rejected here too. |
| 6 | `worker::process_one` (`worker.rs:134`) | Runs 1–5 for every file, so the batch path cannot skip what the single-file path enforces. |
| 7 | `ffi::px_inspect` (`ffi.rs:177`) | Selects `Limits::mobile()` when Dart passes `mobile_limits = true`. |
| 8 | `pipeline::Pipeline::output_dimensions` | Rejects a crop rectangle that runs past the source edges, before the UI predicts a size for it. |

Steps 1–4 are cheap and run on a whole folder before the user commits to
anything; step 5 is the belt to step 3's braces.

## Metadata

`exif::strip` strips by **re-encoding from raw samples** — it builds a fresh
`RgbaImage` and drops the original container — not by clearing tags. Clearing
tags leaves the maker note and the embedded thumbnail in place, and those are
exactly the fields that carry a serial number, a GPS fix and a preview of the
unredacted original.

`exif::write_back` is the only path that puts metadata back, it is only reached
when the user explicitly kept metadata, and it filters through `is_sensitive`
(`exif.rs:110`): anything matching GPS, SERIAL, OWNER, ARTIST, COPYRIGHT,
MAKERNOTE, LENS, BODY or THUMBNAIL is dropped. `worker::process_one` calls it
with `keep_gps: false` and the UI does not offer the flag, so GPS is not
reachable through the product at all.

## Format detection

Format comes from magic bytes. `format::detect_format` is the only thing that
decides what a file is; `OutputFormat::from_extension` exists for naming output
and is documented as untrusted until `detect_format` agrees. A file called
`photo.png` that contains JPEG is a JPEG.

`OutputFormat::all()` is **not** a list of writable formats. `Avif` is in the
enum because it is recognised on input, and `format::encode` rejects it with
`Error::UnsupportedFormat`. What the build can actually write is
`lib::capabilities()`, which is what the UI greys out.

## The FFI boundary

Hostile by default. Every Dart-supplied pointer and length is checked, and every
result is a tagged struct, so there is no path where Dart receives a null it
might dereference.

| Entry point | Takes | Returns |
| --- | --- | --- |
| `px_version()` | — | `PxBuffer` — JSON `{ version }` |
| `px_inspect(ptr, len, mobile_limits)` | bytes | `PxBuffer` — JSON `ValidateReport` |
| `px_exif(ptr, len)` | bytes | `PxBuffer` — JSON `ExifInfo` |
| `px_presets()` | — | `PxBuffer` — JSON preset catalogue |
| `px_process(ptr, len)` | JSON request | `PxBuffer` — JSON result |
| `px_batch(ptr, len)` | JSON request | `PxBuffer` — JSON `BatchReport` |
| `px_zip(ptr, len)` | JSON `[{name, bytes}]` | `PxBuffer` — ZIP bytes |
| `px_buffer_free(buffer)` | `PxBuffer` | — |
| `px_string_free(s)` | `*mut c_char` | — |
| `px_cancel_new()` | — | `PxHandle` (`0` = failure) |
| `px_cancel_trigger(handle)` | `PxHandle` | `bool` |
| `px_cancel_free(handle)` | `PxHandle` | `bool` |
| `px_selftest_error()` | — | `PxBuffer` — a deliberate error |
| `px_selftest_panic()` | — | `PxBuffer` — see gotcha 8 |

Buffer ownership, in full:

- `PxBuffer` is `#[repr(C)]`: `status: u32`, `data: *mut u8`, `len: usize`,
  `error: *mut c_char`. Dart reads it by offset, so the layout is a contract.
- `data` and `error` are **owned by Rust**. Dart copies what it needs and then
  calls `px_buffer_free` / `px_string_free`. Dart never frees them itself.
- Buffers are length-prefixed, never NUL-terminated, so JPEG data containing a
  zero byte survives the crossing intact.
- `px_buffer_free` is idempotent. The pointer's original `Vec` capacity is held
  in a thread-local map keyed by address (`ffi.rs:59`), out of band so the struct
  layout stays Dart-stable; the first free removes the entry and rebuilds the
  `Vec` with the exact capacity, and a second free finds nothing and returns
  without touching the pointer. Losing that map would mean calling
  `Vec::from_raw_parts` with the wrong capacity, which is undefined behaviour.
- Dart-supplied buffers go through `borrow` (`ffi.rs:110`): a null pointer with
  `len == 0` is an empty slice, and a null pointer with a non-zero length is
  `PxStatus::InvalidArgument` — never a dereference.
- Handles are opaque `u64`. `0` means failure, an unknown handle returns
  `false`, and at most 1024 live tokens exist at once. There is no handle value
  that causes a wild access.
- `px_batch` always returns a report rather than failing: one unreadable file in
  a folder of 200 must not discard the other 199.

## Feature flags

| Flag | Default | Effect |
| --- | --- | --- |
| `webp-lossy` | off | Swaps in the `webp` crate (libwebp) for lossy WebP. Off by default so the pure-Rust build needs no C toolchain and compiles fast in CI; with it off, WebP output is lossless. |

`default = []`. `lib::capabilities()` is generated from these flags, so adding a
codec means adding a flag *and* a field there — a capability the UI cannot see
is a capability the UI will offer and then fail at export time.

## Gotchas

Each of these is a trap that is already handled somewhere specific. The file is
named so you can check the handling rather than re-derive it.

1. **An APP1 segment must begin with the six bytes `Exif\0\0`.** Readers search
   for that identifier; a payload without it is a segment they walk past, so the
   metadata is present and invisible. Handled in `core/src/format.rs`
   (`append_exif`, the `EXIF_ID` constant).
2. **EXIF orientation 6 means rotate 90° clockwise**, not counter-clockwise.
   `image::imageops::rotate90` is also clockwise, so the two agree — but EXIF 8
   is counter-clockwise, and the enum is named for what it does, not for the
   number. Handled in `core/src/pipeline.rs` (`Orientation::from_exif`,
   `Orientation::apply`).
3. **WebP output is lossless unless the `webp-lossy` feature is on**, which means
   no quality control and no byte ceiling: `supports_quality()` and
   `supports_byte_target()` are both false, so a "fit under N bytes" request
   silently degrades to a fixed encode. Handled in `core/src/format.rs`
   (`lossy_webp_enabled`, `encode_webp`) and reported honestly by
   `core/src/lib.rs` (`capabilities()`).
4. **`image::imageops::resize` returns an `ImageBuffer`, not a `DynamicImage`.**
   It is generic over the concrete buffer, so the result has to be normalised
   back with `DynamicImage::from` at exactly one place. Handled in
   `core/src/pipeline.rs` (`resize_to`).
5. **`OutputFormat::from_extension` must never be called before
   `detect_format` has agreed.** It maps a string to a format with no checking
   whatsoever, which is precisely how `payload.png` gets treated as a JPEG.
   Handled in `core/src/format.rs`, and `worker::Job::name` documents that it is
   a display name only.
6. **`OutputFormat::Avif` exists but cannot be written.** It is in the enum so
   AVIF input is recognised and reported; `format::encode` returns
   `Error::UnsupportedFormat` for it. An earlier `ravif`-based encoder behind an
   `avif` feature never compiled and broke every verification run, so it was
   deleted rather than excepted. Handled in `core/src/format.rs` (`encode`) and
   `core/src/lib.rs` (`avif_encode: false`).
7. **`image::Limits` is `#[non_exhaustive]`.** It cannot be constructed with a
   struct literal, so `apply_to_decoder` starts from `Default` and narrows only
   the three fields it cares about. Handled in `core/src/validate.rs`
   (`Limits::apply_to_decoder`).
8. **`panic = "abort"` is set for the release profile, so `catch_unwind` cannot
   actually catch.** `px_selftest_panic` returns a normal error buffer under
   `cargo test` and under a debug build; in a release `cdylib` — which is what
   ships — the process aborts instead. That is the intended trade (hard rule 3
   says no panic may escape; aborting is the loudest possible answer), but the
   entry point's doc comment reads as though containment is unconditional.
   Handled in `core/src/ffi.rs` (`px_selftest_panic`) and
   `core/Cargo.toml` (`[profile.release]`).
9. **The GPS IFD has to be `exif::In::PRIMARY`.** `kamadak-exif`'s writer
   requires every IFD in the chain to be non-empty and contiguously numbered, so
   a GPS tag cannot live in IFD 2 without also populating the thumbnail IFD.
   Every written field therefore goes in IFD 0 and readers key off the tag names
   anyway. Handled in `core/src/exif.rs` (`GPS_IFD`, `write_back`).
10. **JPEG has no alpha channel.** Encoding a transparent PNG to JPEG without
    flattening first produces a black rectangle. Handled in `core/src/format.rs`
    (`flatten_alpha`, in the `OutputFormat::Jpeg` arm).

## Verification

`bash scripts/verify.sh` is the gate, and it is what decides whether a phase
counts as done. It enforces hard rule 1 mechanically (no network crate in the
engine tree, no networking symbol in `core/src`), then runs `cargo fmt --check`,
`cargo clippy --all-targets --all-features -- -D warnings`, `cargo test
--all-features`, `cargo build --release` and `cargo doc` with `-D warnings`, then
repository hygiene.

Two things about it worth knowing before you touch it:

- `check()` runs its command in a **subshell**. Several hygiene checks are
  written `cmd && exit 1 || exit 0`, and a bare `exit` inside `eval` would
  otherwise terminate the whole script — which made the gate exit 0 at the first
  hygiene check, skip everything after it, and never print `VERIFY: PASS`.
- Checks that shell out filter cargo's output with `grep -E '^(warning|error)'`,
  so cargo runs with `--color=never`. With the default ANSI colouring the escape
  sequences defeat the filter and a failing run reports no reason at all.

`cargo deny` is **not** part of the gate; it runs in
`.github/workflows/supply-chain.yml`. Its `bans` check is currently red on two
dependency duplicates and is documented as such in `deny.toml`.
