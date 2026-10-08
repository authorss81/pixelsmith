# Architecture

The shape of `core/` today: what each module owns, the order transforms run in,
where the limits are enforced, and the traps that have already cost somebody an
afternoon. Read this before changing `core/src`.

The rules this code exists to satisfy are in `AGENTS.md`. Where this document
and `AGENTS.md` disagree, `AGENTS.md` wins.

## Module map

Eighteen modules, one submodule, no circular references. Dependencies point downward:
`lib.rs` → `worker.rs` / `folder.rs` / `pipeline.rs` / `colour.rs` / `animation.rs` → `resize.rs`
→ `validate.rs` / `heic.rs` → `error.rs`. Nothing below `error.rs` knows anything
exists above it. `animation.rs` sits below `worker.rs` and above `validate.rs`
because it needs the limits and the report the validation pass produces, while
`validate.rs` must not need it — so the *container walk* lives in `validate.rs`
(reading a header is `validate`'s stated job) and the *decision* lives in
`animation.rs`. `dedupe.rs` is below `worker.rs` for the same reason it is not part
of it: the key is a hash of the picture and the request, and `worker.rs` is the only
thing that has both.

| Module | Owns | Key types |
| --- | --- | --- |
| `core/src/lib.rs` | The crate root: the re-export surface, the honest capability list, and the four functions that make up the plain-Rust API (decode, transform, encode, encode-to-target). | `Capabilities`, `capabilities()`, `process()`, `decode_bounded()`, `encode_fixed()`, `encode_to_target()` |
| `core/src/error.rs` | Every failure mode in the engine, as one enum, plus the `Result<T>` alias. Nothing unwinds out of a public entry point, so a malformed file cannot take down the host process. | `Error`, `Result<T>` |
| `core/src/format.rs` | What a format *is*: the `OutputFormat` enum, magic-byte detection, the encode dispatch, the per-format `EncodingOptions` (quality, progressive, chroma), JPEG APP1 splicing, and output-capacity estimation. Reading is broader than writing, which is normal. | `OutputFormat`, `ChromaSubsampling`, `EncodingOptions`, `detect_format()`, `encode()`, `append_exif()` |
| `core/src/validate.rs` | Everything that stands between an untrusted byte slice and an allocation: the limit profiles, the header-only checks (including the GIF frame walk, which reads the container rather than decoding it), and the report the UI shows before the user commits. | `Limits`, `Limits::mobile()`, `check_header()`, `check_animation()`, `validate_bytes()`, `inspect()`, `FrameScan`, `scan_gif_frames()`, `ValidateReport` |
| `core/src/heic.rs` | HEIC/HEIF input: brand-byte detection, a header-only geometry read, and HEVC decode behind the `heic` feature. Read-only; encoding HEIC is refused by name. | `Header`, `HeicError`, `detect()`, `header()`, `decode()`, `built()` |
| `core/src/pipeline.rs` | The transform. Owns the crop/orient/resize order and the EXIF orientation table. Knows nothing about files, encoders or Dart. | `Pipeline`, `CropSpec`, `Orientation`, `ResizeSpec`, `FitMode`, `ResampleFilter`, `resize_to()`, `refused_upscale()` |
| `core/src/stream.rs` | The low-peak-memory decode, behind `streaming`: decode a row at a time, resample it into the destination in the same pass. One resampling pass, one output-sized buffer. | `decode_resized()`, `working_set_bytes()`, `Axis` |
| `core/src/resize.rs` | The single resampling pass and the two kernels that can perform it: `image`'s reference implementation (the default, and the oracle) and `fast_image_resize` behind the `simd` feature. Exists so hard rule 5 has exactly one call site and a benchmark times the same entry point production uses. | `resample()`, `resample_reference()`, `simd::try_resample()` |
| `core/src/colour.rs` | Colour management: reading an ICC profile out of a container, naming the space, and converting Display-P3 to sRGB with the sRGB transfer function. Not an ICC engine, and says so. | `ColourSpace`, `ColourProfile`, `ColourOptions`, `ColourOutcome`, `convert()`, `apply()` |
| `core/src/exif.rs` | Metadata. Reading for the report, stripping by re-encoding, and a filtered write-back that never carries GPS or maker notes. | `read()`, `strip()`, `write_back()`, `ExifInfo` |
| `core/src/target.rs` | "Make this file fit under N bytes", solved by binary search over quality rather than a slider the user has to guess at. | `TargetBytes`, `Encoder` (injected so the search is testable without pixels) |
| `core/src/presets.rs` | The preset catalogue, grouped by intent and carrying a `category` so the UI can present tabs. Custom values are always allowed. | `Preset`, `PRESETS`, `all_presets()`, `find_preset()`, `to_pipeline()` |
| `core/src/worker.rs` | The layer the UI actually calls: per-file work, Rayon parallelism, cancellation, ZIP output, per-file accounting, filename sanitisation, and the skip policy that makes every file a batch does not write account for. | `Job`, `Settings`, `Outcome`, `SkipReason`, `BatchPolicy`, `BatchReport`, `CancelToken`, `process_one()`, `process_animation()`, `process_batch()`, `sanitise_stem()` |
| `core/src/dedupe.rs` | Deciding that two files are the same picture: a BLAKE3 key over the decoded pixels *and* the whole request, so the key is a statement about the output rather than about the file. | `ContentKey`, `Dedup`, `from_pixels()`, `from_encoded()`, `claim()` |
| `core/src/folder.rs` | Folder traversal: enumerate, filter by extension *and* magic bytes, read 4 KiB a file, and hand back a `FolderPlan` a UI can show before the button is worth pressing. Also sizes the worker pool to memory as well as cores. | `FolderPlan`, `FolderEntry`, `EntryVerdict`, `plan()`, `process_folder()`, `pool_size()`, `read_bounded()` |
| `core/src/animation.rs` | What happens to an animation: the policy, the decision, the refusal sentence, the outcome type, and the frame-by-frame path through the pipeline. Does not re-implement GIF composition — see `docs/GIF.md`. | `AnimationPolicy`, `AnimationAction`, `AnimationOutcome`, `decide()`, `preserve()`, `refusal_note()` |
| `core/src/ffi.rs` | The C ABI, where Rust's safety stops protecting the caller. 15 `px_*` entry points, a tagged result struct, and buffer ownership rules. | `PxBuffer`, `PxStatus`, `PxHandle`, `px_*` |
| `core/src/ffi_abi.rs` | The same ABI as data: one declaration per entry point, proved against `ffi.rs` at compile time and rendered into the Dart declarations `app/lib/rust/bindings.dart` must contain. | `ENTRY_POINTS`, `EntryPoint`, `px_buffer_layout()`, `dart_drift()` |

## Request path

One image, one file, front to back. `worker::process_one` is the only path that
does real work; the batch path and the FFI path both call it, so a batch result
and a one-off result cannot drift apart.

```
bytes off disk
  → worker::Settings::validate  a request this build cannot carry out is refused here
  → validate::validate_bytes   input size, magic bytes, header dimensions
                              (heic::header for HEIF, image header otherwise),
                              and a GIF frame count read from the container
  → animation::decide          what happens to the frames: keep, flatten on
                              request, or refuse. Before a pixel is decoded.
  → lib::decode_bounded        decoder limits applied, then decode, then re-check
  → colour::apply              source space → working space, before the geometry:
                              a Cow, so an untagged or sRGB file touches no pixel
  → pipeline::Pipeline::apply  crop → orient → resize (one resampling pass)
  → resize::resample           the one resampling pass: reference kernel, or
                              fast_image_resize when `simd` is on
  → exif::strip                re-encode from raw samples (drops EXIF for real)
  → target / format::encode    byte-target search, or a fixed-quality encode
  → format::append_icc         only if embed_profile was asked for, JPEG/PNG only
  → format::append_exif        only if metadata was explicitly kept, JPEG only
  → worker::sanitise_stem      output name, filtered against traversal
  → Vec<u8>
```

## Encoding options: quality, progressive, chroma

`format::EncodingOptions` is one struct, not three arguments. It started as a
bare `quality: u8` and each new knob would have been another parameter on a
function that already takes an image, a format and a target — through
`format::encode`, `target::Encoder`, `lib::encode_fixed`, `worker::Settings` and
two JSON request structs. The struct also gives the options a home in the JSON
contract, so the next knob is a field rather than a signature change in five
places and a Dart model.

| Field | Default | Honoured by |
| --- | --- | --- |
| `quality` | 85 | JPEG; AVIF and lossy WebP, both of whose encoders are in the default build. Ignored everywhere else. |
| `progressive` | `false` | JPEG only. |
| `chroma_subsampling` | `Luma420` | JPEG only. |

85 is the quality every existing preset already assumed, so the default changes
no existing export's bytes. "Ignored" is deliberate and asserted: a caller with
one global quality slider should not get an error for choosing PNG output, and
`format::tests::an_option_a_format_cannot_honour_changes_nothing` pins that a PNG
is byte-identical whatever the JPEG-only options say. What the engine *refuses*
is a promise it cannot keep — a byte ceiling on a format with no quality setting
— and it says so in a sentence naming a format that can (`Settings::validate`,
`OutputFormat::quality_note`).

Two places a value can be set, and one rule: **`Pipeline::chroma_subsampling`
wins over `Settings::encoding.chroma_subsampling`**, because the pipeline
describes the picture and is what a UI sets on it. `Settings` is the fallback for
a caller that never touches the pipeline. `worker::process_one` applies it, so a
caller who sets it in only one of the two places gets the behaviour they asked
for on the path a UI actually uses.

`Outcome.quality_used` is **0 when the format has no quality setting**, because
that is the honest report: no quality was applied. Zero is also what a failed or
cancelled file carries, which is the same claim. Before phase-07 the engine
reported the *requested* 85 for a PNG, next to a file whose size the slider never
influenced.

### What would change the chroma default

Phase-12 added colour management, and with it ICC colour in the output that
4:2:0 was never discarding. The measurement in the section above was taken before
any of that: a Display-P3 photograph decoded correctly has chroma detail the sRGB
pipeline never had, so a third of the chroma samples is now a third of something
visible. The default has not moved on the strength of a prediction. The next
measurement to take is a P3 fixture through the same bars as
`format::tests::colour_bars`, and if the blue-difference error at 4:2:0 is
materially worse on saturated P3 content than the 21.75 recorded above, the
default moves to 4:2:2 rather than being argued about.

### Why 4:2:0 is the default

The phase prompt asked for the argument rather than the number, so here it is.

**What it is.** JPEG stores luma and two colour-difference channels. Chroma
subsampling is how finely those two are stored: 4:4:4 keeps both at full
resolution, 4:4:2:0 (usually written 4:2:0) stores a quarter of the chroma
samples — one for each 2×2 block of luma. Luma resolution never changes, so
**sharpness is untouched**: this trades colour detail, not edges. That is the
first thing a user has to be told, because "subsampling" sounds like a
resolution setting.

**The case for 4:2:0.** This is a photo resizer, and photographs are luma. A
skin's colour changes across tens of pixels; a sky's across hundreds. Measured
on a 1600×1200 fixture at q85, 4:2:0 is 59,200 bytes against 88,975 at 4:4:4 —
**a third of the file** for a difference you cannot see in a photograph. On the
Web category, where the file is uploaded rather than printed, a third off every
asset is the difference between a fast site and a slow one, and no competing
resizer exposes the knob at all.

**The case against it, which is real.** Content whose subject *is* colour
degrades visibly: red text on a blue background, a logo with hard colour
boundaries, a UI screenshot. Measured on saturated red-on-blue bars at q95, the
mean error of the blue-difference channel is 1.05 at 4:4:4 and 21.75 at 4:2:0 —
**twenty times worse** — while 4:2:2 sits in between at 21.18. Glyphs fringe
against their background because the glyph and the background are different
colours one pixel apart. So a default that is right for photographs is wrong for
screenshots, and the engine must not pretend otherwise.

**The resolution.** The default follows the *content*, and the engine knows what
the content is: `Preset.chroma` is 4:2:0 for all 40 other presets in the
catalogue and 4:4:4 for `store-screenshot`, the one whose content is a UI capture
with text on a coloured background. The JSON request carries
`pipeline.chroma_subsampling`, so the app can expose the same choice for custom
work, and `ChromaSubsampling::trade_off()` is a tooltip the UI can show verbatim.
Nothing is applied silently: `ChromaSubsampling` is `#[serde(default)]` at
4:2:0 and the doc comment on the enum says why.

### Progressive JPEG

Scan-by-scan output shows a coarse picture immediately and refines it, which is
what makes an image appear at all on a slow connection. It is off by default
because it is not cheap: measured at q85 4:2:0, 96,938 bytes against 59,200 —
about 64% more — since the four scans carry some coefficients twice. That belongs
on a share sheet for "sending over a slow link", not on every export. `docs/JPEG.md`
is the encoder comparison behind the choice of `jpeg-encoder` over `image`'s.

### AVIF: on by default, and it costs seconds

The `avif` feature is **on by default** (it was removed in phase-01 and
reinstated here), because it is `image`'s own encoder — rav1e through ravif —
which is pure Rust with no C toolchain, no nasm and no build script, so the two
reasons the other opt-in codecs are off do not apply.

What it costs is time. Measured in release, single-threaded, one 1600×1200 photo
at quality 70: **3.2 s**, against 20 ms for a q85 4:2:0 JPEG — roughly two orders
of magnitude. A 4032×3024 photo takes 17 s. The batch path parallelises across
files, so a 200-image AVIF batch still uses every core, but a single AVIF export
is a progress bar. `AVIF_SPEED` is 8 because 4 measured no smaller (5,967 bytes
against 5,759 at q70) and 10 measured 34% larger for two thirds off the wait.

The decode direction is the honest half of the story: **this build cannot read
AVIF back.** `image`'s AV1 decoder (dav1d) is not enabled and `heic-rs` decodes
HEVC only, so `capabilities().avif_decode` is `false` and a UI must not imply a
round trip it cannot make. Writing a format you cannot read is still worth it —
the file leaves the device and the user opens it in a browser — but the capability
list is where that difference is stated, not left to be discovered.

### WebP: on by default, and it costs a C toolchain

The `webp-lossy` feature was off for most of this project's life for one reason:
libwebp is C. It is still the reason it is a *feature* rather than a hard
dependency, and phase-08's judgement call was whether to move it into `default`.
It is in `default` now.

**What it bought.** Lossless WebP has no quality setting and no byte ceiling, so
the format could only be an also-ran: measured on the 600×400
`format::tests::photo` fixture, **101,656 bytes lossless against 2,638 at q80**.
That is 39×, and it is why `web-hero`, `web-card` and `web-thumb` were exporting
with their byte ceilings silently dropped by `to_pipeline` — the UI showed a
number the engine was not trying to meet. With the encoder in the build those
ceilings are kept, the quality slider does something, and
`format::tests::webp_byte_ceilings_are_reachable` proves the search can actually
meet them rather than reporting `target_met: false` forever.

**What it costs.** `libwebp-sys` vendors libwebp's own C source and compiles it
with `cc`. On this CI runner, a cold `cargo build --release --lib` is 55 s without
the feature and 1 m 34 s with it — **39 extra seconds**, and no new toolchain to
install: `cc` is the same compiler already building the Rust code. No CMake, no
nasm, no code generator. That is the line that mattered, because it is the reason
`avif` and `jpeg-encoder` are pure Rust and pay none of this, and it is why
`webp-lossy` was worth revisiting when `avif` had already shown that a codec does
not have to be free to be in the default set.

**What makes the claim checkable — and what does not yet.** The
`webp-lossy-matrix` job that does this is written but **not in the tree**:
`workspace/phase-08/build-webp-lossy-matrix.patch` adds it to
`.github/workflows/build.yml`, for Linux x64, Windows x64 and arm64, macOS x64
and arm64, Android arm64 and x86_64, and iOS arm64, building the crate with
`--features webp-lossy`, linking the test binary with `--no-run` because that is
what proves libwebp's symbols resolve, and running the suite on the Linux x64
runner so the claim is not eight targets that link and nothing that runs.

It cannot be applied by this pipeline. The push credential is refused
`workflows` permission on `.github/workflows/`, which is the same class of limit
as the `app/` submodule being unpushable (see phase-05), and adding
`workflows: write` to `automation.yml`'s own `permissions:` block would need a
workflow push to take effect.

**So the honest position today is the phase prompt's own rule: do not claim lossy
WebP on a target whose CI job does not build it.** The encoder is in the default
build and `verify.sh` runs it on Linux x64 on every push, which is one target
genuinely built and tested. The other seven are a patch away and are not claimed
until it lands. `engine-matrix` does compile the default feature set, so a
cross-compilation break on another target would still show up there — but a
*silent* stop of the encoder being compiled would not, which is precisely what
the dedicated job is for.

**What stays behind the flag.** The `#[cfg(not(feature = "webp-lossy"))]` arm of
`encode_webp` is kept, not deleted. It is the configuration a target with no C
toolchain builds, it is what the capability flags are written against, and
`is_lossless` reports the truth for it —
`format::tests::webp_reports_the_truth_about_this_build` asserts all four facts
(capability flag, `is_lossless`, `supports_quality`, and the actual VP8/VP8L
chunk in the output) agree, in *both* configurations.

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
aliases badly when decimating that hard. That filter choice is made *above* the
kernel choice, so both implementations get the same `Triangle` for a 10000 → 50
downscale and cannot disagree about which filter a hard reduction uses.

### One pass, two kernels

`resize::resample` is the single call site of hard rule 5. It picks between two
implementations of the *same* filter, so there is never a question of which filter
ran — only of which code did it.

| | `resample_reference` | `simd::try_resample` |
| --- | --- | --- |
| Implementation | `image::imageops::resize` | `fast_image_resize`, behind `simd` |
| Default | yes | no |
| Arithmetic | `f32` coefficients, `f32` accumulation, one `round()` at the end | coefficients quantised to `i16` scaled by `1 << precision`, `i32` accumulation, clipped by an arithmetic right shift |

`try_resample` returns `Option`, and `None` means "the reference kernel handles
this one". That is not only an error path: `Nearest` deliberately returns `None`
(see gotcha 21), because the two crates disagree about which source pixel a
destination sample takes on an exact tie and no tolerance covers a whole-pixel
difference. So the flag is a speed change for four of the five filters and a
no-op for the fifth — which is what "the SIMD kernel is faster" has to mean if it
is to be true of a pixel-art export.

`resample_reference` is public and stays public. It is the oracle the tests check
the SIMD kernel against and the second column of `docs/BENCHMARKS.md`, so it
cannot be private, and it is never on a schedule for deletion: with `simd` on by
default it becomes the fallback, and with `simd` off it is the only kernel.

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
| 4 | `reader.into_dimensions()` in `validate_bytes`, or `heic::header` for a HEIF | Reads the header only — no pixel buffer exists yet — and records a human-readable `suspicious` note when dimensions or pixel count are over budget. A HEIF states its geometry in an `ispe` property box that no image decoder reads, so `heic::header` is the header read for that format, and it is the only place the HEIC limit can be enforced before the codec allocates. |
| 5 | `lib::decode_bounded` → `Limits::check_decoded` (`validate.rs:68`), or `heic::decode`'s `Limits::check_header` | Post-decode assertion, for headers that lied about their size. Zero dimensions are rejected here too. |
| 6 | `worker::process_one` (`worker.rs:134`) | Runs 1–5 for every file, so the batch path cannot skip what the single-file path enforces. |
| 7 | `ffi::px_inspect` (`ffi.rs:177`) | Selects `Limits::mobile()` when Dart passes `mobile_limits = true`. |
| 8 | `pipeline::Pipeline::output_dimensions` | Rejects a crop rectangle that runs past the source edges, before the UI predicts a size for it. |
| 9 | `validate::Limits::check_animation` (`validate.rs`) | The pixel budget applied to `width × height × frames` rather than per frame, before the first frame of an animation is decoded. Every resized frame is resident at once, so a per-frame check would pass all of them. |

Steps 1–4 are cheap and run on a whole folder before the user commits to
anything; step 5 is the belt to step 3's braces.

## Metadata

A HEIF file keeps its metadata in a *separate container item*, referenced from
the picture by a `cdsc` relation — not in a JPEG APP1 segment and not in a PNG
chunk. `exif::read` cannot see one, so `heic::header` reports `has_exif` from
the container and `validate_bytes` uses it, or the UI would tell a user their
iPhone photo carries no metadata while a GPS fix is sitting in the file. Reading
the tags themselves is not done, and phase-12 — which was about colour — did not do
it: a HEIF's `Exif` item needs the same box walker a HEIF's `colr` property needs,
and phase-12 built neither. Until then they are neither read nor written back, which
means they are also not carried through, because the pipeline re-encodes from raw
samples.

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

## Colour management

An untagged file is assumed to be sRGB, which is right. Before phase-12 a *tagged*
file simply had its tag dropped: a photograph shot on a wide-gamut phone, whose
pixels are Display-P3, came out with those same numbers now read as sRGB. That is
not metadata hygiene — stripping a profile without converting it is a colour
change, and hard rule 6 does not excuse it.

### The path, and where it runs

```text
source space  →  working space  →  output space
(from the file)   (sRGB by default)   (the working space, untagged)
```

`colour::apply` is the whole decision, and `worker::process_one` calls it
**after the decode and before `pipeline.apply`**. Before the geometry, because the
source space is a fact about the file while the working space is where the rest of
the chain expects to be, and converting after a downscale would resample values in
the wrong space. It is not part of `Pipeline::apply`, because that function is
documented as knowing nothing about files — and a file's colour space is a property
of the file.

The default case touches no pixel at all: `convert` returns a `Cow`, and an
untagged or sRGB source is borrowed. `worker::tests::an_untagged_photo_exports_byte_for_byte_as_it_did_before`
is the assertion, because "less memory" is not the claim — *identical bytes* is.

### Three answers, and why the default is the first

| `ColourOptions` | What happens | When to want it |
| --- | --- | --- |
| default | convert to the working space (sRGB), drop the profile | almost always: the file's values now mean sRGB, and an untagged JPEG means sRGB to every reader that will ever open it |
| `keep_source_pixels` | no conversion, no profile, values untouched | when the output is going somewhere that honours the *original* profile, and the user has said so |
| `embed_profile` | no conversion, the source profile is written into the output | same, but self-contained: a viewer that reads profiles renders it correctly |

The profile is dropped in the default case **because** the pixels were moved into
the space it described, not in spite of it. The two opt-outs both exist because
converting is a loss of information: `clipping_is_what_costs_a_p3_colour_its_out_of_gamut_colour`
asserts what that costs. Asking for both at once is refused
(`ColourOptions::validate`), because two checkboxes that contradict each other is
a UI that has got out of sync, not a request to guess at.

Embedding is supported for JPEG and PNG (`format::append_icc`,
`format::png_insert_icc`) and **refused for WebP** with a sentence naming JPEG and
PNG. That is a statement about what has been written and tested, not about what
WebP allows: an `ICCP` chunk turns a simple lossy WebP into an extended `VP8X` one
and there is no WebP reader in this tree to check the result against. What gets
embedded is the *original* profile, re-read from the source container, never a
synthesised one — a profile a phone wrote is the only one this engine has any
business putting back into a file.

### Which containers are read

`colour::containers_read()` returns both lists, so the gap is declared rather than
discovered:

| Read | Not read |
| --- | --- |
| JPEG `APP2` (`ICC_PROFILE\0`, reassembled in sequence order) | HEIF/HEIC `colr` |
| PNG `iCCP` (zlib, hence `flate2`) | TIFF |
| WebP `ICCP` | BMP, GIF, ICO — nowhere to put one |

**HEIF is the one that will be asked about first**, because an iPhone photographs
HEIC and in Display-P3 when "most compatible" is off. `heic.rs` has no box walker
to reuse and reading a fourth container is a change to that module's stated job
rather than to this one, so it is named here instead. A HEIC in this build is
therefore reported as untagged and exported as sRGB — which is what a reader
without the profile does with those numbers anyway, and is the safe direction:
wrong values described as untagged rather than wrong values described as P3.

### What is implemented, and what is approximated

Two matrix-shaper RGB spaces, **sRGB** and **Display-P3**, both using the sRGB
transfer function, and an untagged input assumed to be sRGB. That is the entire
claim. Not ICC conformance, and the approximations are listed rather than buried:

* **The transfer function is assumed, not read.** Every profile is treated as
  using the IEC 61966-2-1 piecewise curve. It is *not* a straight gamma 2.2, which
  is the most common bug in this whole area — at mid-grey the sRGB curve gives
  0.7354 and gamma 2.2 gives 0.7296, five code values on an 8-bit scale.
  `colour::tests::the_transfer_function_is_the_srgb_curve_not_gamma_22` is written
  to fail if the two ever come out the same, because a round-trip assertion would
  happily pass for either.
* **Gamut mapping is clipping.** A P3 colour outside sRGB is clipped per channel,
  which is what every other tool does and is *not* perceptual: a saturated P3 red
  clips to pure sRGB red rather than being desaturated towards its own luminance.
* **Classification is by colorant.** A profile is called sRGB or P3 by comparing
  its `rXYZ`/`gXYZ`/`bXYZ` chromaticities against two reference sets after undoing
  ICC's D50 adaptation (Bradford), with the `desc`/`mluc` tag as the fallback.
  Colorants first because they are a measurement while the description is a string
  two unrelated profiles share. Tolerance 0.004 in `xy`, where the two spaces are
  about 0.04 apart.
* **The matrix is derived, not pasted.** `colour::matrix` inverts the RGB→XYZ
  matrix built from the reference primaries, so the two sets of primaries are the
  only numbers in the tree that a reader has to trust.
  `the_derived_matrix_is_the_published_one` checks the result against CSS Color
  4's published Display-P3 → sRGB matrix to 5e-4 — the tolerance is that wide
  because the two published derivations disagree by about 2e-4 on the first
  coefficient, which is a twentieth of one code value on an 8-bit output.
* **LUT-based (A2B0/B2A0) display profiles are not evaluated.** They classify from
  their colorants — which v4 requires of every RGB display profile — but their tone
  curve is ignored.
* **CMYK, Lab and grayscale are not converted.** They are recognised and refused
  in a sentence (`colour::unsupported_note`) rather than mishandled, and the
  sentence always names the space, the alternative and the opt-out.

### The interaction with stripping

`exif::strip` re-encodes from raw samples and therefore drops the ICC profile as a
side effect of dropping the container. That is only correct because the pixels were
converted first: a stripped P3 photo is an sRGB photo with an sRGB body, rather
than P3 values wearing an sRGB label.
`worker::tests::a_p3_photo_stripped_of_its_profile_is_converted_rather_than_left_alone`
is the test that says so, and it compares against the *naive path* — the same photo
with `keep_source_pixels` — rather than a hard-coded triple, because the naive path
is exactly what the engine used to do and exactly what it must now not do.

### What the JSON contract carries

| Field | Where | What the UI can say |
| --- | --- | --- |
| `colour.icc_present` / `colour.source` / `colour.declared` / `colour.description` / `colour.icc_bytes` | `ValidateReport`, `ExifInfo` | "Display-P3 — this will be converted to sRGB", before the user exports |
| `colour.working_space`, `keep_source_pixels`, `embed_profile` | `Pipeline` | the three answers above; `#[serde(default)]`, so a request from an older app still means "convert to sRGB" |
| `Outcome.colour.source` / `.output` / `.converted` / `.profile_embedded` | `Outcome` | what actually happened to the file that was just written |

## Format detection

Format comes from magic bytes. `format::detect_format` is the only thing that
decides what a file is; `OutputFormat::from_extension` exists for naming output
and is documented as untrusted until `detect_format` agrees. A file called
`photo.png` that contains JPEG is a JPEG.

`OutputFormat::all()` is **not** a list of writable formats. `Heic` and `Heif` are
in the enum because they are recognised on input, and `format::encode` rejects
them with `Error::UnsupportedFormat`. `Avif` is the awkward one: it is both, and
which one depends on the build — with the `avif` feature it is written, without
it `is_read_only()` is `true` and the refusal names the feature to turn on. What
the build can actually write is `lib::capabilities()`, which is what the UI greys
out, and every per-format predicate is derived from the same `cfg!` that decides
the dispatch, so the list cannot promise an encoder the build lacks.

**HEIF is detected by reading the container, not by asking a codec.** `image`
has no HEIF sniffer, so `detect_format` falls back to `heic::detect`, which
parses the `ftyp` box's major brand and then its compatible brands. That
fallback is deliberately *not* behind the `heic` feature: a build without the
codec should still be able to say "this is a HEIC" before it says "this build
cannot open it", which is a more useful thing to tell a user than "unknown
format". `Avif` is reachable from the same fallback, so an AV1-coded file in a
HEIF container is named rather than misreported — `image`'s sniffer only claims
`ftypavif`, and a file may be stamped `mif1` with `avif` in its compatible list.

`Heic` and `Heif` are always read-only. `Avif` is read-only only in a build
without the encoder. `is_read_only()` is the flag the UI reads, and a test
asserts it against `format::encode` so the two cannot drift into a format the UI
offers and the encoder refuses.

## Animation

`animation::AnimationPolicy` is one field on `worker::Settings` and the whole
policy is four cells wide; `docs/GIF.md` is the argument and this is the shape.

| Input | Output | `Keep` (default) | `FirstFrame` |
| --- | --- | --- | --- |
| Still | anything | written | written |
| Animated | GIF | every frame, with its delay | first frame, `dropped()` reported |
| Animated | anything else | **nothing written**, refused in a sentence | first frame, `dropped()` reported |

`animation::decide` is that table, called from `worker::process_one` right after
`validate_bytes` and **before a pixel is decoded**, so a refusal costs the header
walk that found the frames. `animation::preserve` is the `Preserved` cell: every
frame through `colour::apply` and `Pipeline::apply`, one resampling pass per
frame, delays carried across, then `format::encode_frames` into one GIF.

Three things about the decision that are worth knowing before changing it:

* **`Keep` does not mean "refuse animations".** It means "do not lose frames
  silently". A GIF exported as a GIF loses nothing and is not refused, so the
  default policy is not the conservative one — it is the one that keeps the most
  user requests working while still never dropping a frame unannounced.
* **The frame count comes from the container, not from a decode.**
  `validate::scan_gif_frames` walks the GIF block stream counting image
  descriptors, so `validate_bytes` can still be cheap enough to run over a whole
  folder. It replaced a `into_frames().count()` that *decoded every frame* during
  that folder scan.
* **`ValidateReport.frames_truncated` is load-bearing, not diagnostic.** It says
  whether `frames` is a count or a lower bound, and a lower bound cannot be
  checked against a write of the same number — so `preserve` refuses a truncated
  animation rather than emitting one that is quietly shorter than the original.

`AnimationOutcome` rides on `worker::Outcome`, on `px_batch`'s report and on
`px_process`'s response. A refused animation is the one *failure* that reports
its frame count, because the file is fine and the frames were read; everything
else reports `AnimationOutcome::unknown()`.

## Folders: the plan, the duplicates, and the skips

A folder is a different question from a file. 400 inputs have to become a number a
user can believe, and "we processed what we could" is not a number anyone can
act on. Three pieces carry that: a plan, a key, and a reason.

### The plan comes first, and reads 4 KiB a file

`folder::plan` is the preview, and its cost is the reason a UI can show it for 400
photographs without the button feeling broken: one `stat` and the first
`folder::HEADER_BYTES` (4 KiB) of each candidate, then a verdict per file.
Nothing is decoded and nothing is written.
`folder::tests::the_plan_reads_a_header_per_file_rather_than_the_whole_thing`
asserts the arithmetic against a real folder rather than describing it.

The verdict is `EntryVerdict`, and the distinctions in it are the ones a user
acts on differently:

| Verdict | What it means | Becomes |
| --- | --- | --- |
| `Ready` | an image this build can open, within the limits | processed |
| `NotListed` | the extension is not one this build opens, so the bytes were never read | **not an outcome at all** |
| `NotAnImage { detected: None }` | the name said image, the bytes say otherwise | `SkipReason::Unreadable` |
| `NotAnImage { detected: Some(f) }` | a real image in a container this build cannot decode | `SkipReason::UnsupportedFormat { format }` |
| `TooLarge { limit }` | over `max_input_bytes`, so processing would refuse it anyway | `SkipReason::TooLarge` |

**`NotListed` is not a skip, and that is deliberate.** A folder next to a set of
photographs holds three hundred `.DS_Store` files and a `.thumbnails` directory;
reporting those as skipped would bury the thirty lines that matter. They are in
the plan — which is where "what is in this folder" is answered — and out of the
report.

The walk does not follow symlinks (`symlink_metadata`, and the module says why),
skips dot entries, stops at `MAX_DEPTH` (8), and stops at `MAX_ENTRIES` (10 000)
*reporting that it stopped* rather than truncating quietly. Every problem it cannot
resolve is a sentence in `FolderPlan::problems`, never an error return: a folder
with one unreadable subdirectory is still a folder the user wants.

`folder::process_folder` then runs the plan's ready entries through
`worker::process_all` with a loader that reads each file *inside its own task*.
The alternative — handing `process_batch` a folder already resident in memory —
is a batch that holds 400 photographs at once, which on a phone is the phase-11
ceiling crossed 400 times before any work starts.

### The pool is sized to memory as well as cores

`folder::pool_size` is `min(cores, MAX_POOL, available_memory / 2 / footprint)`,
floored at one, and it is a *pure* function underneath
(`folder::pool_size_for(cores, available_bytes, largest_input_bytes)`) so a test
can drive the decision without a machine of a particular shape.

Cores are the obvious half and were not the binding one. Each worker holds a
decoded picture plus `image`'s `f32` resize intermediate; `DECODE_EXPANSION` (24)
is a deliberate over-estimate of what one worker's input costs, because the two
errors are not symmetric — over-estimating makes the pool smaller, which costs
time, while under-estimating makes it thrash, which costs the phone. Half of
`MemAvailable` rather than all of it, because the workers are not the only thing
on the device and the page cache wants room for the file being read next.
`MAX_POOL` (8) is a responsiveness bound rather than a speed one.

Where the platform publishes memory it is read (`/proc/meminfo`, which covers
every Android target); where it does not, `FALLBACK_AVAILABLE_BYTES` (2 GiB)
stands in, because `std` publishes nothing portable and a probe that silently
disagreed with the page above it would be worse than a stated constant.

### The key is over the output, not over the file

`dedupe::ContentKey` is a BLAKE3 digest of **the decoded pixels plus the whole
request**, truncated to 128 bits. Two consequences, both asserted:

* Two byte-different encodings of one picture are **one output**. The test is
  `two_byte_different_encodings_of_one_picture_produce_one_output`, and its
  fixture is a flat field because that is the one shape a JPEG round trip is
  exact on — two qualities, two different files, not one of their pixels
  different.
* The same picture under two pipelines is **two outputs**. The request is in the
  key, field by field and tagged, so a caller asking for two sizes gets two files.
  `every_field_of_the_request_reaches_the_key` is the test that catches a field
  added to `Pipeline` or `Settings` and not hashed, which would be a silent merge.

The key is computed **after** `colour::apply`, so a Display-P3 copy and an sRGB
copy of one photograph merge: from that point on they are the same bytes. For an
animation it is `from_encoded` over what `preserve` built, because re-deriving a
pixel key would mean decoding the GIF a second time to save a comparison the
encoder has already made.

What the key deliberately does **not** do is match perceptually. A lossy re-encode
of the same photograph is a different picture to an exact hash, and
`a_lossy_re_encode_is_not_claimed_to_be_a_duplicate` holds the engine to saying
so: a missed duplicate appears in the report, a wrong merge does not. The second
failure mode — two files named `IMG_0001.jpg` from two cameras — is the one hard
rule 7 is about, and it is why nothing here reads a filename.

### A skip is a third thing, and it has a reason

`Outcome.skipped: Option<SkipReason>` is present on every outcome and `None`
whenever a file was written, so a caller never infers a skip from an absent
error. `SkipReason` is `Duplicate { of }`, `Unreadable`,
`UnsupportedFormat { format }`, `TooLarge { limit, actual, unit }` and
`WouldUpscale { requested, actual }`, each with `note()` written for a person.
`BatchReport` therefore reports three counts that add up — `succeeded()`,
`skipped()`, `failed()` — and `skip_reasons()` groups them so a summary does not
have to be re-derived by the UI.

`BatchPolicy` is the switch, not a mode: `deduplicate`, `skip_unprocessable` and
`skip_upscales` are independent, all default on, and
`BatchPolicy::report_everything()` turns all three off for a caller that would
rather count errors itself.

### `would_upscale`: a skip, argued

`pipeline::refused_upscale(src_w, src_h)` returns `Some` only when all four of
these hold, and `None` in every other case:

1. there is a resize at all, so something was asked;
2. no crop and no orientation transform, since either is real work even when the
   geometry comes out the same;
3. the resolved output is the source's own size — the geometry did not change;
4. with `no_upscale` ignored, the request asked for **more** than it got.

The decision is that condition is a **skip**, not a warning and not a silent
write. The argument:

* **There is nowhere to put a warning, and inventing a channel is a bigger
  change than making this a skip.** The engine's whole reporting surface is
  `Outcome`, and it already carries a third state alongside success and failure:
  `skipped`. A "written, but not what you asked for" flag would be a *fourth*,
  and every consumer of the report — the FFI, the JSON, the Dart models — would
  need to learn it. A skip reuses the field that is already there.
* **The user's own guard is what fired.** `no_upscale: true` means "do not enlarge
  this". Writing the file at 640×480 when the request said 1920×1080 does not
  honour the guard; it quietly returns a different file than the one asked for.
  `no_upscale: false` is the opt-in for enlargement and is not affected. So this
  is not the engine refusing a request it could carry out — it is the request
  contradicting itself, reported with both geometries in one sentence: *"this
  picture is 64x64 and the request asked for 4000x3000; enlarging it adds no
  detail, so it was left alone"*.
* **It is a skip, not a failure, and that is the part that matters.** A 64×64 icon
  in a folder of 12 MP photographs is not a broken file. Calling it a failure
  tells the user to fix their folder; saying nothing tells them they have 400
  outputs when they have 370. The skip is the only answer that is both true and
  actionable, and `BatchPolicy::skip_upscales = false` gives the other answer —
  write it at its own size — in one field.
* **It never touches the single-image path.** `process_one` takes no
  `BatchContext`, so `px_process` and a one-off request behave exactly as they did
  before this phase: pick an icon, ask for 4000 px, get 4000 px. That is a working
  path to compress and re-encode a small picture, and a folder is a different
  question — with 30 of 400 files in it, the user needs to know before spending an
  hour on the export.
  `a_single_image_request_is_unchanged_by_the_skip_policy` holds it.

The strongest argument *against*, stated: a common job is making everything the
same size, and in that workflow "copy the small one at its own size" is exactly
what the user wants, so a folder-wide skip is a batch that quietly fails to
equalise. The answer is the switch above, plus a gap named honestly: **the plan
does not count would-upscale skips**, because it reads a header for format
detection and not the geometry, so a user still learns the number when the
export is already running. Predicting it is possible — `ImageReader`'s
`into_dimensions()` is a header read and decodes nothing — and it is the obvious
next thing to add to `EntryVerdict`. It is not here, and saying otherwise would be
claiming a preview this plan cannot give.

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
| `px_process(ptr, len)` | JSON request | `PxBuffer` — JSON result, carrying `animation` as well |
| `px_batch(ptr, len)` | JSON request | `PxBuffer` — JSON `BatchReport` |
| `px_zip(ptr, len)` | JSON `[{name, bytes}]` | `PxBuffer` — ZIP bytes |
| `px_buffer_free(buffer)` | `PxBuffer` | — |
| `px_string_free(s)` | `*mut c_char` | — |
| `px_cancel_new()` | — | `PxHandle` (`0` = failure) |
| `px_cancel_trigger(handle)` | `PxHandle` | `bool` |
| `px_cancel_free(handle)` | `PxHandle` | `bool` |
| `px_selftest_error()` | — | `PxBuffer` — a deliberate error |
| `px_selftest_panic()` | — | `PxBuffer` — see gotcha 8 |
| `px_abi_layout()` | — | `PxBuffer` — JSON `StructLayout` for `PxBuffer` |

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
| `avif` | **on** | AVIF *write*, through `image`'s rav1e encoder: pure Rust, no C toolchain, no nasm, no build script, so it compiles for all four shipped targets. On because the reasons the codec below was off do not apply to it; what it costs is measured encode time, in `docs/ARCHITECTURE.md` above. |
| `webp-lossy` | **on** | Lossy WebP *write*, through the `webp` crate (libwebp, the reference encoder). On since phase-08: without it WebP is lossless, so there is no quality control and no byte ceiling — measured on the 600×400 `format::tests::photo` fixture, 101,656 bytes lossless against 2,638 at q80, which is 39× and the difference between a resizer whose WebP export is worth choosing and one where it is an also-ran. What it costs is a C toolchain: `libwebp-sys` compiles 159 vendored `.c` files with `cc`, about 39 s on a cold release build of this crate — no CMake, no nasm, no code generator, and `cc` is the same compiler already building the Rust. The per-target matrix that checks it is `workspace/phase-08/build-webp-lossy-matrix.patch`, not yet applied; see the WebP section above. With the feature off, WebP output is lossless again and `is_lossless()` says so. |
| `simd` | off | The resize *kernel*, not a format: `pipeline::resize_to` calls `resize::resample`, which uses `image`'s implementation unless this is on, in which case `fast_image_resize` performs the same pass. Pure Rust with runtime CPU detection — no C toolchain, no nasm, no code generation — which is why it costs 10 s on a cold release build where `webp-lossy` costs 39 s. Measured 2.6× to 19.9× on the reference kernel's own workload, agreeing to within one least-significant bit per channel on photographic content (`docs/BENCHMARKS.md`). Off by default because every one of those numbers is x86-64 AVX2 and the crate dispatches on runtime CPU features, so the speedup on the ARM phone this ships to is unmeasured; flipping the default is phase-10's call with an ARM machine in the room. With it off, every byte this engine produces is identical — `image`'s kernel is still the only kernel compiled in, and it is still the oracle the SIMD path is checked against. |
| `streaming` | off | The **decode** path, not a format: `worker::process_one` resizes a row at a time, from the row being decoded into the destination, instead of materialising the source. Off by default for two reasons, both in `docs/BENCHMARKS.md`: it is a second code path whose correctness rests on replicating `image`'s coefficient definition, and the wall-clock comparison that would justify turning it on has not been run on the ARM phone this ships to — the same argument that keeps `simd` off. Measured, at 120 MP to 1000 px wide: **613 MB peak in memory against 4 MB**, and one least-significant bit of difference from the in-memory kernel across five filters and seven size pairs. Behind the flag it is not the default, because "less memory" is only half of what a decode path has to be. Not in `capabilities()`: it changes no format's readability or writability, so a field there would be a capability the UI has nothing to do with — the same reasoning as `simd`. |
| `heic` | off | HEIC/HEIF decode via `heic-rs`: pure Rust, no build script, no C, no new crates in the tree. Off by default, and the reason is now stated rather than deferred: phase-08 was the phase this flag waited for, and what it actually did was turn on `webp-lossy` instead — the reason recorded here was "an opt-in feature that has never been cross-compiled is a promise nobody has checked", and that check still has not happened for `heic`, because the `webp-lossy-matrix` job does not build it. Enabling it would put the flag in front of a target matrix nothing has run against. `docs/HEIC.md` has the comparison behind the choice, including the crate this one is a hard choice *against* (libheif) and the one it beats (AGPL). |

`default = ["avif", "webp-lossy"]`. `simd` and `streaming` are deliberately not in
it, and are also deliberately not in `lib::capabilities()`: it changes no format's
readability or writability, so a field there would be a capability the UI has
nothing to do with. `lib::capabilities()` is generated from the format flags,
so adding a codec means adding a flag *and* a field there — a
capability the UI cannot see is a capability the UI will offer and then fail at
export time. The HEIC flag is `heic_decode`, deliberately not an encode flag: the
engine can read HEIC and cannot write it, and one boolean cannot honestly say
both. AVIF is the same shape inverted — `avif_encode` on, `avif_decode` off,
because this tree has no AV1 decoder in any configuration. `webp_lossy` is the
one flag whose value is a *build* property rather than a format property:
`OutputFormat::is_lossless(WebP)` is `!lossy_webp_enabled()`, so a build without
the feature reports the knob as inert rather than offering a slider that changes
nothing. Both configurations are asserted by
`format::tests::webp_reports_the_truth_about_this_build`.

## The second decode path, and what it is not

`stream::decode_resized` is behind `streaming` and is *not* a smaller
`decode_bounded`. It is a different traversal of the same filter, and the two
claims in its name are worth separating.

**It is still one resampling pass, and the reason is that separable filters
commute.** Every source pixel reaches every destination pixel through exactly one
horizontal weight and one vertical weight, so `h-then-v` and `v-then-h` are the
same filter and a destination pixel is written once from `f32` accumulators.
`image` runs the vertical pass into a `src_width x dst_height` `f32` buffer
because that is the order that lets it start from a whole image; the streaming
path runs the horizontal pass per row because that is the order that does not need
one. The arithmetic — `ratio`, `sratio`, the clamped `left`/`right`, the
normalised weights, one `round()` at the end — is copied from `image` coefficient
for coefficient, which is why the two agree to one LSB rather than to "close
enough".

**The pre-filter is the kernel's own scaled support, and no box pass runs first.**
At a reduction of R each destination pixel integrates `2 * support * R` source
samples with weights `kernel((i - centre) / R)`. That is the anti-aliasing
pre-filter, which is why an area-weighted average is the right *shape* for one: a
box is the simplest kernel guaranteed non-negative and summing to one, where point
sampling throws away pixels and bilinear smears across boundaries it should have
averaged. The two-stage design — box down to twice the target, then resize that —
is measured rather than argued, and it lands within **1 LSB** of the single pass on
a photographic fixture at 16:1 (`docs/BENCHMARKS.md`). So the single pass is here
for memory and for hard rule 5, not because the composite would have been visibly
worse, and the case where that stops being true — hard colour edges — is recorded
as unmeasured rather than as fine.

**It cannot stream most formats, and that is a property of the dependency set.**
`image` 0.25.10 gives every codec `read_image(self, buf: &mut [u8])` and nothing
else; below it `zune-jpeg` 0.5 and `image-webp` 0.2 expose no strip, row or scaled
decode either. `png` 0.18 is the only decoder in the tree with
`Decoder::next_row()`. So `Source` has a row-at-a-time PNG arm and a whole-image
arm for everything else, and the second one still removes the kernel's
intermediate but not the source buffer — **a 120 MP JPEG to 1000 px wide is
therefore a 613 MB job with this flag on, not a 4 MB one.** Interlaced, 16-bit,
palette and animated PNGs also take the whole-image arm, because `png`'s row API
returns Adam7 rows in pass order rather than display order.

**It only runs for a plain resize**: no crop, no orientation transform, and a
resize that changes the size. `worker::process_one` checks all three. Both
exclusions are cases the streaming traversal would need a different ordering for,
not cases it cannot do, and inventing a third ordering of the pipeline is not
something this phase does quietly.

### The two memory bounds, and why they are methods

`Limits` grew no fields, so the JSON contract the Dart side mirrors is untouched.
Two methods instead:

| | `streamed_pixels_budget()` | `streaming_memory_budget()` |
| --- | --- | --- |
| Value | `max_pixels * 4` — 160 MP mobile, 512 MP desktop | `max_pixels * 4` — 160 MB mobile, 512 MB desktop |
| What it is | an input ceiling for a decode that never materialises the picture | a ceiling on the streaming path's own working set |
| Why that number | a streamed decode's peak is about the *destination*, so the pixel budget stops being a memory ceiling and becomes a CPU one | the profile's existing memory intent, stated once: 4 bytes per pixel is what `apply_to_decoder` already pushes into `image::Limits::max_alloc` |

Raising `max_pixels` itself would have been the obvious mistake: it would have
let the **in-memory** path try to materialise 160 MP on a phone, which is exactly
the failure hard rule 4 exists to stop. A build without the `streaming` feature
never consults either method, so its ceiling is unchanged.

And on a target with no sandbox outside it — Windows, where `setrlimit` has no
equivalent and [`sandbox`] enforces nothing — these two are the whole defence,
which is why the second one is derived from the profile rather than being a
constant: a target states what it can afford in `Limits::mobile()` and gets both
bounds from it.

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
3. **`OutputFormat::is_lossless(WebP)` is a property of the *build*, not of the
   format.** With `webp-lossy` on — the default since phase-08 — WebP is lossy, the
   quality slider does something and a byte ceiling can be met. Without it the
   pure-Rust lossless encoder is the only one compiled in, so
   `supports_quality()` and `supports_byte_target()` are both false and a "fit
   under N bytes" request would silently degrade to a fixed encode. The two facts
   are one `cfg!` (`lossy_webp_enabled`) read by both, so the capability list and
   the encoder cannot disagree. Handled in `core/src/format.rs`
   (`lossy_webp_enabled`, `encode_webp`) and reported honestly by
   `core/src/lib.rs` (`capabilities()`). The `#[cfg(not(feature =
   "webp-lossy"))]` arm is kept rather than deleted: it is the configuration the
   capability flags are written for, and it is what a target with no C toolchain
   would build.
4. **libwebp stops at 16383 pixels a side, and `validate::Limits` stops at 30000.**
   An 18 000 × 200 panorama therefore passes every limit the engine applies,
   decodes, resizes — and then dies inside the encoder. `webp::Encoder::encode`
   unwraps internally, so reaching that point used to be a hard crash, which hard
   rule 3 forbids on bytes the user picked off their disk; `encode_simple` returns
   the error instead, and `check_webp_dimensions` refuses first with a sentence
   naming the limit and a format that can do it. Handled in `core/src/format.rs`
   (`check_webp_dimensions`, `WEBP_MAX_DIMENSION`).
5. **Handing libwebp `from_rgba` for an opaque picture costs a copy and a scan,
   and nothing at all in the output.** The obvious reading — that dropping the
   constant alpha plane first makes the *file* smaller — is false: measured at
   q10 through q100, the packed-RGB and RGBA encodes are byte-identical, because
   libwebp detects the constant alpha plane and discards it before compressing.
   The optimisation is kept anyway (25% less input, 3 bytes per pixel instead of
   4 on every opaque export) and asserted as an **equality**, which is what makes
   it a test rather than a hope. `format::tests::the_packed_rgb_path_is_used_for_opaque_pictures_and_matches_the_rgba_path_byte_for_byte`
   also asserts the fixture is genuinely opaque, because `exif::strip` returns an
   `ImageRgba8` unconditionally and a `has_alpha()` test would have taken the
   four-channel path on every export forever. Handled in `core/src/format.rs`
   (`is_opaque`, `encode_webp_lossy_rgb`).
6. **`webp` 0.3 is the only published line and it is not a maintained crate**
   (last release 2019). `cargo add webp@1` fails with "could not be found in
   registry index". It is recorded in `deny.toml` rather than hidden, and it is
   still the right pick because it is the only binding to the *reference*
   encoder — the output every browser and CDN is tested against. Handled in
   `core/Cargo.toml` (`[dependencies.webp]`).
4. **`image::imageops::resize` returns an `ImageBuffer`, not a `DynamicImage`.**
   It is generic over the concrete buffer, so the result has to be normalised
   back with `DynamicImage::from` at exactly one place. Handled in
   `core/src/pipeline.rs` (`resize_to`).
5. **`OutputFormat::from_extension` must never be called before
   `detect_format` has agreed.** It maps a string to a format with no checking
   whatsoever, which is precisely how `payload.png` gets treated as a JPEG.
   Handled in `core/src/format.rs`, and `worker::Job::name` documents that it is
   a display name only.
6. **`OutputFormat::Avif` is writable only when the feature is on.** It is both a
   recognised input and a real output in the default build, and
   `is_read_only()` returns `!avif_encode_enabled()` so the two cannot disagree;
   a build without the feature gets the same "this build cannot write it, choose
   JPEG or WebP" refusal HEIC gets. An earlier `ravif`-based encoder behind an
   `avif` feature never compiled and broke every verification run, so it was
   deleted rather than excepted — `image`'s own AVIF encoder is what replaced
   it. Handled in `core/src/format.rs` (`encode_avif`, both `cfg` arms) and
   `core/src/lib.rs` (`avif_encode: cfg!(feature = "avif")`).
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
11. **A hand-written Dart binding is not a contract, it is a hope.** `dart:ffi`
    has no generator and no `offsetOf`, so `app/lib/rust/bindings.dart` is typed
    by a human against `ffi.rs` — and `px_inspect` took three arguments while the
    declaration passed two, for as long as both existed. The guard is
    `core/src/ffi_abi.rs`: every entry point is declared once, each declaration
    expands to a `const _` function-pointer assignment that only compiles if it
    matches the real signature, and `scripts/check-dart-bindings.sh` reports the
    difference. Handled in `core/src/ffi_abi.rs`.
12. **`px_selftest_panic` aborts a release build, so `flutter test` must not run
    against one.** `[profile.release]` sets `panic = "abort"` (gotcha 8), so the
    test device dies with SIGABRT and `flutter test` exits 1 while printing that
    every test passed. `scripts/verify.sh` therefore builds the *debug* library
    before running the Flutter tests. Handled in `scripts/verify.sh`.
13. **The engine has to be on the loader path before `flutter test` means
    anything.** `DynamicLibrary.open('libpixelsmith_core.so')` searches the loader
    path, not the working directory, so a library in `app/src/rust/` is invisible
    to it and the FFI tests skip themselves — green, and checking nothing. The
    gate now exports `LD_LIBRARY_PATH` and fails if any test skipped.
14. **A HEIF file's dimensions are in a container box, and `Limits` is only real
    if something reads it before the codec allocates.** `image::ImageReader` is
    the header read for every other format, and a HEIF cannot go through it: its
    geometry is in an `ispe` property inside `iprp`, which no image decoder
    looks at. Without the separate `heic::header` step, a 60000×60000 `ispe`
    would be enforced *after* `heic_rs::decode` had allocated for it — and the
    codec's own ceiling (256 MP) is far above `Limits::mobile()`'s 40 MP, so the
    engine's bound would be the only thing standing between a phone and an OOM.
    That is also why `Limits::check_header` exists separately from
    `check_decoded`. Handled in `core/src/heic.rs` (`decode`) and
    `core/src/validate.rs` (`check_header`).
15. **`ftyp` brand detection must not sit behind the `heic` feature.** Gating it
    would make a build without the decoder call an iPhone photo "unknown format"
    instead of "a HEIC this build cannot open", which is the less useful of two
    true statements, and hard rule 9 asks for the useful one. The same applies to
    reporting `OutputFormat::Avif` for an AV1-coded HEIF: naming a format is not
    the same as being able to read it. Handled in `core/src/heic.rs` (`detect`,
    which has no `cfg` on it) and `core/src/format.rs` (`detect_format`).
16. **`image`'s JPEG encoder writes baseline 4:4:4 and chooses its own sampling
    factor from the quality value.** It has no progressive option at all, and it
    switches from 4:2:0 to 4:4:4 at quality 90 — so leaving it in charge would
    make a user's quality slider silently change the picture's colour
    resolution, which is the kind of change no caller asks for and no test sees.
    JPEG is therefore written through `jpeg-encoder`, and the sampling factor is
    set explicitly on every encode. Handled in `core/src/format.rs`
    (`encode_jpeg`), with the comparison in `docs/JPEG.md`.
17. **`jpeg_encoder::SamplingFactor` counts luma samples per chroma sample, so
    JPEG's 4:2:0 is `F_2_2`.** The two spellings read backwards relative to each
    other, and a "correction" applied at the call site instead of in the mapping
    would hand the encoder a file with twice the chroma samples asked for. All
    three levels go through `ChromaSubsampling::sampling_factor`, the only place
    the translation exists.
18. **`quality_used: 0` means no quality setting was applied.** It is not a
    failed encode and not "quality zero". A lossless format has no quality knob,
    so the honest report for a PNG is 0 rather than the 85 the caller asked for —
    the UI shows that number next to the file it just wrote, so a wrong one is a
    claim about the user's own export. Handled in `core/src/worker.rs`
    (`process_one`) and `core/src/target.rs` (`encode_with`).
19. **A size ceiling on a format with no quality setting cannot be honoured at
    any slider position.** The engine refuses it rather than returning an
    oversized file with `target_met: true`, which is the one answer hard rule 9
    forbids. Only the *ceiling* is refused: a quality value on the same format is
    ignored, because one global quality slider sits above the format picker and
    refusing every PNG export because of it would be absurd. Handled in
    `core/src/worker.rs` (`Settings::validate`) with the sentence in
    `OutputFormat::quality_note`.
20. **The suite's wall clock is the preset test, not AVIF.**
    `presets::tests::presets_convert_to_working_pipelines` runs 41 Lanczos3
    resamples of a 4000×3000 image — about six seconds each in the debug build
    `verify.sh` runs, so roughly four minutes of the total. A single AVIF encode
    is ~0.5 s in the same build, which is fast enough not to matter. Neither is a
    regression, but a `cargo test` that takes minutes rather than seconds is
    worth recognising as normal before someone tries to speed the codecs up.
21. **Two nearest-neighbour samplers disagree about which pixel to take, and no
    tolerance covers it.** `image` computes the source position directly as
    `(y + 0.5) * ratio`; `fast_image_resize` accumulates `y += step` in `f64` and
    truncates, so accumulated rounding decides an exact tie. Measured on a 4×4
    row-ramp upscaled to 6×6: `image` takes source rows `[0, 1, 1, 2, 3, 3]`,
    `fast_image_resize` takes `[0, 1, 1, 2, 2, 3]` — the row where
    `(3 + 0.5) × (4/6)` is exactly `3.0` and the accumulated sum is
    `2.9999999999999996`. That is a whole source pixel, not a rounding step, so
    the SIMD kernel returns `None` for `Nearest` and the reference handles it.
    `ResampleFilter::Nearest` is documented for pixel art, where a pixel that
    moves is a visible defect. Handled in `core/src/resize.rs`
    (`simd::algorithm_for`), pinned by
    `resize::tests::simd_agreement::the_two_nearest_implementations_really_do_disagree`,
    which fails the moment either crate changes.
22. **The SIMD kernel's per-channel tolerance (29) is barely below the wrong
    kernel's (31), so the maximum alone cannot detect a mis-mapped filter.** Both
    figures are measured, on `resize::tests`'s deliberately hard fixture, which
    puts a 2-pixel checkerboard in a quarter of the frame — and Lanczos3's four
    negative lobes turn that edge into the overshoot where a coefficient
    quantised to `i16` has the most leverage. The mean separates cleanly (0.48
    against 1.17), so the cross-check asserts *both*, per filter, from a table
    whose worst-case floors are themselves measured in the suite
    (`a_wrong_kernel_always_lands_outside_the_tolerance`). Handled in
    `core/src/resize.rs` (`simd_agreement::TOLERANCES`).
23. **Two rows of a picture are not a strip, but they are what "strip decoding"
    means here.** The phase prompt asked for `image`'s "row-at-a-time reading",
    and `image` 0.25.10 has none: every codec implements
    `read_image(self, buf: &mut [u8])`, which hands you the whole image in a buffer
    you own — an allocation decision, not a traversal one. `png` 0.18 does have
    `Decoder::next_row()`, which is the only reason `streaming` exists at all, and
    it is why a 120 MP **PNG** is a 4 MB job with the flag on and a 120 MP **JPEG**
    is still a 613 MB one. Handled in `core/src/stream.rs` (`Source`), and
    `stream::tests::names_the_formats_it_cannot_stream` fails if that stops being
    true.
24. **`Pipeline::apply` cloned the source, and it was the largest buffer in the
    engine.** `let mut out = img.clone()` ran before every transform, so a resize
    cost two copies of the decoded image plus the kernel's intermediate — about
    1.1 GB for a 120 MP source and a 3.3 MB output. It is a `Cow` now: the source
    is borrowed until the first step that must build a new buffer. Measured after
    the change, 613 MB, and the residual is the `f32` intermediate
    `image::imageops::resize` allocates at `src_width x dst_height` (docs/BENCHMARKS.md).
25. **`image::imageops::resize` filters an axis it was not asked to change.** It
    always runs both passes, so a 800x800 → 800x400 resize runs a Lanczos3 pass
    with `ratio = 1.0` horizontally — a nine-tap blur across the axis that did not
    move. `stream::Axis` reproduces that rather than fixing it, because fixing it
    would change existing exports' pixels and this phase is about memory; it is
    recorded here so the next person to see a soft `FitMode::Height` export knows
    it is pre-existing. Handled in `core/src/stream.rs` (`Axis::new`), and
    `it_agrees_with_the_in_memory_kernel`'s one-axis case is what keeps the two
    paths agreeing about it.
26. **`fast_image_resize` dispatches at runtime, so its speedup is a property of
    the CPU, not of the crate.** `cpu_extensions.rs` picks AVX2, else SSE4.1,
    else scalar. Every number in `docs/BENCHMARKS.md` is x86-64 AVX2 on a CI
    runner, and the crate does not expose which path it took, so the benchmark
    harness reproduces that decision by reading `/proc/cpuinfo`. A number from
    this file quoted as "the SIMD resize kernel is 8× faster" without the ISA is
    a claim about a runner, not about the phone this engine ships to. Handled in
    `core/examples/resize_bench.rs` (`machine()`).

27. **A gamut mapping that "looks right" is not a gamut mapping.** Converting a
    saturated Display-P3 red to sRGB by clipping per channel gives pure sRGB red,
    which is *correct* and *not perceptual* — the colour a photographer chose is
    replaced by one on the sRGB gamut boundary, and no amount of resampling puts it
    back. A real gamut mapper desaturates towards the achromatic axis and keeps the
    hue; this one clips, because that is what every other tool does and a hue-
    preserving mapper is a different and much larger module. The cost is asserted
    rather than argued about: `clipping_is_what_costs_a_p3_colour_its_out_of_gamut_colour`
    holds that an out-of-gamut P3 colour does *not* round-trip, so a future
    replacement has to come with a new expectation rather than silently changing
    what "converted" means. Handled in `core/src/colour.rs` (`convert`).
28. **`exif::strip` drops the ICC profile as a side effect, and that is only
    correct because the pixels were converted first.** The order in
    `worker::process_one` is decode → colour → geometry, and putting the conversion
    after `exif::strip` would produce a file that was P3 all the way to the encoder
    and then untagged sRGB — the exact bug this phase exists to fix, in a place
    where no test looking at "metadata stripped" would have found it. Handled in
    `core/src/worker.rs` (`process_one`) and pinned by
    `a_p3_photo_stripped_of_its_profile_is_converted_rather_than_left_alone`.

29. **A GIF image descriptor is not nine bytes of descriptor.** The packed byte at
   its end can announce a **local colour table**, which sits inside the block
   before the LZW minimum code size and the frame's data. The container walk in
   `validate::scan_gif_frames` stepped straight into it on its first run and
   counted exactly one frame in an animation of four — and `image`'s own encoder
   emits a local table for every frame it writes, so the fixture that exposed it
   was every fixture. The global colour table before the first block is the same
   trap. Handled in `core/src/validate.rs` (`after_image_descriptor`, and the
   packed check before the block walk).
30. **A GIF's frame count is a lower bound until the trailer is found.** A file
   whose frame data runs out mid-stream still decodes: `image`'s decoder stops at
   end-of-file and hands back the frames it managed, which is the right behaviour
   for a viewer and the wrong one for an exporter, because 99 frames of a
   100-frame GIF is a successful-looking animation that is missing a frame.
   `FrameScan::truncated` is how the walk says so and `AnimationPolicy` is what
   acts on it. Handled in `core/src/validate.rs` (`FrameScan`) and
   `core/src/animation.rs` (`preserve`).
31. **`image`'s GIF encoder writes `dispose = Background` for every frame it
   writes**, whatever the caller asked for. That is safe here only because every
   frame this engine writes is a full composited canvas, so there is no disposal
   method to get right — a transparent pixel in a composited frame means nothing
   was ever painted there. Reached by hand with `encode_frame` on a sub-rectangle,
   it would silently blank parts of the animation. Handled in
   `core/src/animation.rs` (`preserve`) by never emitting a sub-rectangle, and
   pinned by `an_animation_exported_as_gif_keeps_every_frame_and_its_delay`.
32. **Which of two identical files gets written is the scheduler's choice, so a
    test cannot assert it.** `Dedup::claim` is a mutex-guarded insert reached from
    Rayon workers, and `SkipReason::Duplicate { of }` names whichever thread
    arrived first. Two tests written for this phase asserted one name
    (`"a.jpg"`, `"one.jpg"`) because the left half of a two-element `par_iter`
    usually runs first — a statement about the thread pool, not about the engine,
    and one that fails on a loaded runner. What is guaranteed, and what both tests
    now assert, is that the reason names one of the two files and that the file it
    names is the one with bytes written. Handled in `core/src/dedupe.rs` (`claim`)
    and `core/src/worker.rs` (`process_all`).

## Release artefacts

The engine is a library and the app is a shell, so "the product" exists only
once somebody packages both and checks the package. This section is about the
packaging, and there are three decisions in it that are not obvious from the file
names.

### The version is one number in four places, and a check

`core/Cargo.toml`'s `[package] version` is the source of truth, because
`px_version()` returns `CARGO_PKG_VERSION` and that is the version the compiled
binary reports to whoever runs it. The other three are compared against it:

| Where | Form | Why it is there |
| --- | --- | --- |
| `core/Cargo.toml` | `0.1.0` | what the binary says (`lib::ENGINE_VERSION`) |
| `app/pubspec.yaml` | `0.1.0+1` | Android `versionCode` and the Windows build suffix |
| `CHANGELOG.md` | `## [0.1.0]` | what shipped, and what did not |
| the tag | `v0.1.0` | what the release is named |

**The scheme is SemVer and this phase did not choose it.** Both manifests already
said `0.1.0` when phase-16 started — `core/Cargo.toml` from phase-01,
`app/pubspec.yaml` from the phase-05 scaffolding — and the phase prompt says
explicitly not to invent one. So `scripts/check-version.sh` reads the two
manifests, the changelog and the tag, and refuses to let them disagree, and
`v0.1.0` is the tag that falls out of files that were already written. Recording
it here is what makes it a decision rather than a coincidence.

**The build number after `+` is deliberately excluded from the comparison.** It is
the Android `versionCode`, it must increase on every Play upload, and a script
that compared the whole `version:` string would fail on every upload after the
first. `0.1.0+1` and `0.1.0+99` are the same release.

The extractor is scoped to the `[package]` table rather than grepping the file,
because `Cargo.toml` carries `version =` lines for path dependencies too and an
extractor that read whichever came last would be reading the wrong one.
`check-version.sh --self-test` has a case for exactly that.

### The artefact is opened, not trusted

`scripts/verify-release-artifact.sh` is the phase's real deliverable. The reason
is in this repository's own history: an earlier revision of `build.yml` wrote the
`.so` files to `app/src/rust/jniLibs`, which is where the desktop builds look and
where **Gradle does not read**. The comment it left says *"The APK built fine and
contained no native library — it opened and would have crashed on first tap."*
That is a failure no build log reports, so it has to be asserted:

* a `classes.dex`, and an `AndroidManifest.xml`;
* an `libpixelsmith_core.so` for **every ABI that carries a `libflutter.so`** —
  and the ABI list is taken from the archive's own Flutter libraries rather than
  from the build's intent, because the archive's list is what a device can
  actually load;
* each engine library's ELF `e_machine` against the ABI it is filed under, and
  `px_version` in its symbol table, because a 32-bit library under `arm64-v8a` and
  an empty stub both pass every name-based check;
* the archive's ABI set against `abiFilters` in `android/app/build.gradle.kts`,
  **in both directions** — a declared ABI with no library is an install that
  crashes, and an undeclared ABI is the classic "works on my device";
* `minSdkVersion` at or above the engine's floor of 21, read with `aapt2`;
* for Windows: `pixelsmith_core.dll` **next to** the EXE, not merely in the
  archive, plus `data/app.so` (the Dart AOT snapshot) and no installer.

Three of those assertions cost something to get right, and each is worth knowing
about:

* **`e_machine` is compared as raw file-order bytes, not as a number.** AArch64 is
  the value `0x00B7`, which little-endian stores as the bytes `b7 00`, so `od
  -tx1` prints `b700`. An earlier revision "corrected" the comparison table to
  the reassembled form and the self-test went green, because the synthetic
  fixtures were generated from the same wrong table. **A synthetic fixture cannot
  tell you your convention is wrong when it is built with the convention.** It
  took a real AArch64 library out of a real APK to catch it.
* **`abiFilters` is parsed with balanced parentheses.** The idiomatic form is
  `abiFilters += listOf("arm64-v8a", ...)`, whose closer is a paren; a range that
  ended at the next `]` ran on into `signingConfigs` and reported `release
  release debug` as three more ABIs.
* **The EXE size floor is 40 KB, not the 100 KB the prompt suggested.** A
  Flutter release launcher genuinely weighs about that much — the Dart AOT
  snapshot ships separately as `data/app.so` — so 100 KB is a coin flip rather
  than a guard, and a floor that flips is worse than no floor. The assertions that
  catch a broken build are the DLL and the snapshot, both of which fail loudly;
  the size floor is only there to catch a truncated file.

### Nothing secret is in this repository, and signing does not fail closed

No keystore, no `.jks`, no `key.properties`, no password. `.gitignore` lists them
and `scripts/verify.sh` fails if one is ever tracked — a `.gitignore` is a
request, and the check is the guarantee.

The release workflow reads the keystore from an Actions secret into `RUNNER_TEMP`
and writes `android/key.properties` only for the build. **With the secrets it
signs with a Play upload key; without them it produces a release-mode,
debug-signed APK and says so in the job summary, in an issue and in the draft
release body.** That is the direction the failure should go: the repository owner
is the only person who can add a secret, and a pipeline that fails closed on a
missing one hands them a red run instead of a release. A debug-signed release
APK still installs on a device with USB debugging on, which means every
contributor can install the change they just made.

### Portable ZIP, not MSIX and not an installer

`ROADMAP.md` asks for a portable ZIP and this phase does that. An EXE is not a
file here — a Flutter Windows release is a *directory* of the EXE, the Flutter
engine DLLs, `data/` and `pico/*.dll` — so the release attaches the whole
directory zipped. No installer means no admin rights, no registry writes and no
`Program Files`, so it runs from a USB stick.

The cost is stated where a user will meet it: the binary is not code-signed,
because a signing certificate is issued to an organisation and paid for by one.
Windows SmartScreen shows "unrecognised app" and on some builds refuses to run
it until *More info → Run anyway*. An MSIX would need the same certificate; an
MSI would additionally want admin rights. The unsigned ZIP is the honest shape,
and both READMEs say so rather than hoping nobody notices.

### What is a patch

`workspace/phase-16/release-ci.patch` carries `release.yml` and the release-grade
jobs in `build.yml`, and `workspace/phase-16/shrinkray-phase-16.patch` carries the
`abiFilters`, the signing config, the app name and the app README. Neither can be
pushed from the pipeline that wrote it — the workflow one because a GitHub App
without the `workflows` permission may not touch `.github/workflows/`, and the app
one because `app/` is `authorss81/shrinkray` and only `authorss81/pixelsmith` is
writable. Same wall as phase-05/06/07/08/10/13/14/15, same answer.

So **the release artefacts in `scripts/RELEASE-SHA256.txt` are the ones this
phase actually built**, on a Linux runner, and the Windows row is `NOT-BUILT`
with the reason in the row. That table is a fact about a release rather than a
file that only existed on somebody's machine, which is why the absence is
recorded in it instead of being left as a gap.

## Verification

`bash scripts/verify.sh` is the gate, and it is what decides whether a phase
counts as done. It enforces hard rule 1 mechanically (no network crate in the
engine tree, no networking symbol in `core/src`), then runs `cargo fmt --check`,
`cargo clippy --all-targets --all-features -- -D warnings`, `cargo test
--all-features`, `cargo build --release` and `cargo doc` with `-D warnings`, then
repository hygiene, then builds the debug engine, puts it on the loader path,
runs the Flutter tests against it and **fails if any of them skipped**. That last
part is the one that matters: the FFI tests skip themselves when the library is
absent, so for most of the project's life the gate was reporting success while
executing none of the boundary.

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
