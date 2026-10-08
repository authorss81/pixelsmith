# Audit

Newest first. One entry per audit run, headed with the date and the commit the
audit was written at. The classifier is deliberately harsh: a roadmap item
marked `DONE` that I could not find in the tree is reported as `ABSENT`, and the
phase table is treated as a claim to be checked rather than as evidence.

## Contents

- [2026-10-08 — audit at phase-17](#2026-10-08--audit-at-phase-17)

---

## 2026-10-08 — audit at phase-17

Audited at `7f2ed88` (`px: phase-16 verified`), the commit phase-16 was verified
on. `cargo test --all-features` was run and passes: **280 lib + 20 hostile + 19
property + 27 sandbox + 1 streaming-peak = 347 tests, 0 failed, 0 ignored.**

### Classification per roadmap category

`DONE` — implemented, and I found the test.
`PARTIAL` — implemented, with a named gap.
`ABSENT` — not implemented.
`STALE` — the roadmap describes something the code no longer matches.

#### Visual / UI — 0 done, 10 absent

`app/lib/main.dart` is **80 lines**. It opens a window, calls `px_version`, and
prints the engine's version string. No screen calls the engine. Every item in
this category is `ABSENT` and none of it is a gap in a roadmap — it is the
entire category.

| Item | State |
| --- | --- |
| Adaptive light/dark/system theme | ABSENT |
| Single accent, elevation model, 4pt grid | ABSENT |
| Before/after slider with magnifier | ABSENT |
| Zoomable checkerboard | ABSENT |
| Format/size result card | ABSENT |
| Non-destructive live preview | ABSENT |
| Skeleton + empty states | ABSENT |
| Preset chips by category | ABSENT |
| Uniform control styling across platforms | ABSENT |
| Motion, `prefers-reduced-motion` | ABSENT |
| Custom theming hook | ABSENT |

#### User-friendliness — 0 done, 11 absent

| Item | State |
| --- | --- |
| Clipboard paste, drag-and-drop, "open with" | ABSENT (no UI to put them in) |
| Zero-config default | PARTIAL — `presets.rs` has 41 presets and `all_presets()` exists; nothing presents them |
| Explain every number | PARTIAL — `pipeline::output_dimensions` and `SkipReason::note()` both exist and are honest; nothing renders them |
| Human errors | **DONE in the engine, ABSENT in the app.** Every `Error` variant carries a written-for-a-person message; `error.rs` has **zero tests** (finding 19) |
| Warn before destructive things | PARTIAL — `Error::SuspiciousDimensions`, `NoQualitySetting` and `TargetBytes`'s "unreachable" return exist; unreachable from a UI |
| Undo/redo | ABSENT |
| Remember settings | ABSENT |
| Onboarding | ABSENT |
| Full keyboard access, command palette | ABSENT |
| Searchable settings | ABSENT |
| Filename incrementing | PARTIAL — `sanitise_stem` sanitises; nothing increments |
| Explicit progress and cancel | PARTIAL — `CancelToken`, `CancelHandle`, `px_cancel_*` all exist and work; nothing calls them |

#### Live progress — 0 done, 8 absent

`worker::BatchReport` reports `succeeded()`, `skipped()` and `failed()`, and
`SkipReason::note()` groups them. That is the data; there is no consumer.

| Item | State |
| --- | --- |
| Determinate progress bar | ABSENT |
| Per-file rows with per-file state | PARTIAL — `Outcome` carries the terminal state per file; the intermediate states (`queued → decoding → resizing → encoding`) do not exist in the engine |
| Throughput readout (images/s, ETA, peak memory) | ABSENT — nothing samples progress mid-file |
| Live savings counter | PARTIAL — `input_bytes`/`output_bytes` are on every `Outcome`, so the total is derivable |
| Instant, honest cancellation | PARTIAL — `CancelToken` exists; `process_all` checks it between files, not during one |
| Background processing + notification | ABSENT |
| Streaming preview of the first result | ABSENT |
| Per-file error log expandable to the decoder message | PARTIAL — the engine keeps `Error::to_string()`, which is the human sentence, not the decoder's own text |
| Toast + inline status | ABSENT |

#### More features — 5 done, 12 partial, 8 absent

| Item | State | Note |
| --- | --- | --- |
| Target-size mode | **DONE** | `target.rs`, binary search, ~7 encodes |
| Batch + ZIP export | **DONE** | `worker::zip_outputs`, `folder::process_folder` |
| Format conversion | **DONE** | 8 writable formats + AVIF; `capabilities()` is the honest list |
| Auto-orient from EXIF | **DONE** | `pipeline::Orientation`, all 8 EXIF values tested |
| Crop with aspect lock, draggable handles, 3×3 grid | PARTIAL | `CropSpec` exists and is bounds-checked; no UI, no handles, no lock |
| Metadata inspector, GPS banner | PARTIAL | `ValidateReport.sensitive_tags` and `ExifInfo.has_gps` exist; no banner |
| Watermark | ABSENT | never started |
| Offline AI background removal | ABSENT | not started; would need an ONNX runtime |
| Offline AI upscale | ABSENT | not started |
| Smart compression suggestions | ABSENT | `TargetBytes` measures one format; nothing compares two |
| Deduplicate by content hash | **DONE** | `dedupe.rs`, BLAKE3 over pixels + request |
| Animated GIF handled honestly | **DONE** | `animation.rs`; preserve or refuse, never silently flatten |
| HEIC/HEIF decode | PARTIAL | `heic-rs` behind an off-by-default `heic` feature; **never cross-compiled and never in a shipped artefact** (finding 14) |
| AVIF encode | **DONE** | on by default; this build cannot *read* AVIF and `capabilities().avif_decode` says so |
| Print sizing (cm/inch, paper picker) | ABSENT | |
| RAW (CR2/CR3/NEF/ARW/DNG) | ABSENT | |
| Developer presets | PARTIAL — `presets.rs` has store-screenshot and density-ish presets; no iOS icon or store-screenshot *set* |
| Colour conversion (sRGB/P3/ICC) | **DONE** | `colour.rs`, 32 tests; HEIF `colr` not read, stated in ARCHITECTURE |
| Progressive JPEG | **DONE** | `jpeg-encoder`, ~64% more bytes at q85 4:2:0, measured |
| Chroma subsampling | **DONE** | `ChromaSubsampling`, defaults by content |

#### Security and privacy — 6 done, 2 partial, 5 absent

| Item | State | Note |
| --- | --- | --- |
| Zero network calls at runtime | **DONE — verified independently** | See "The core claim, re-derived" below |
| Reproducible builds | PARTIAL | 1 of 8 targets measured; `rust-toolchain.toml` absent |
| Dependency audit in CI | PARTIAL | `supply-chain.yml` has the jobs; the `bans` check is **currently red** on two duplicates |
| Fuzzing | PARTIAL | 11 targets, 99 seeds, `scripts/fuzz.sh` — and **no CI job ever runs one** (finding 16) |
| Sandboxed decode with a hard memory cap | PARTIAL | `sandbox.rs` is real and tested on Unix; **a documented no-op on Windows** (`sandbox.rs:618`) |
| Decompression-bomb limits from the header | **DONE** | eight enforcement points, documented |
| Magic-byte detection | **DONE** | |
| Path traversal sanitisation | **DONE** | two phase-03 defects found and fixed here |
| Extension whitelist on outputs | ABSENT | |
| Binary transparency | PARTIAL | `scripts/no-network-report.sh` checks the *symbol table*; nothing hashes the shipped library |
| No telemetry | **DONE** | follows from the first row |
| Metadata stripped by re-encoding | **DONE** | |
| GPS never written back | **DONE** | `is_sensitive` filters 9 tag classes |

#### Performance — 3 done, 3 partial, 2 absent

| Item | State | Note |
| --- | --- | --- |
| SIMD resize behind a flag | PARTIAL | `resize.rs`; **off by default, built only on x86-64 Linux** (finding 14) |
| Thread pool sized to the device | **DONE** | `folder::pool_size`; a pure function so it is testable |
| Streaming decode | PARTIAL | `stream.rs`; 4 MB vs 613 MB for a 120 MP PNG, **but `stream.rs:592-625` emits skewed rows for every RGB source** (finding 10) |
| Early decode downscale for previews | ABSENT | |
| Startup budget | ABSENT | nothing to measure yet |
| Benchmarks in CI | PARTIAL | 5 bench targets, 20 cases, `bench.yml` is a **patch** (finding 17) |
| Parallel batch, deterministic | **DONE** | |
| Exactly one resampling pass | **DONE** | the property the whole tree is organised around |

#### Platform integration — 1 partial, 5 absent

| Item | State |
| --- | --- |
| Windows shell integration (context menu, jump list, tray, SendTo) | ABSENT |
| Android Photo Picker, MediaStore, share sheet, WorkManager | ABSENT — the APK builds and contains the engine; the app behind it is 80 lines |
| iOS PHPicker, Photos framework, share extension | ABSENT — `app/ios/` has no engine integration at all |
| CLI | ABSENT |
| Deep links / URL scheme | ABSENT |
| Portable zip for Windows | PARTIAL — `docs/RELEASE.md` and phase-16's checks exist; there is no release workflow (finding 18) |

#### Trust and polish — 1 done, 4 partial, 2 absent

| Item | State |
| --- | --- |
| Honest capability reporting | **DONE** — `capabilities()` is generated from the same `cfg!` that decides the dispatch, and a test asserts the two agree |
| A real README | **DONE** — has a security statement, a build-from-source section, and an honest "what is not done" |
| Reproducible bug reports | ABSENT |
| Changelog with real entries | **DONE** — `scripts/check-version.sh` gates the version against it |
| Contributing guide, issue templates | ABSENT |
| Localisation from day one | ABSENT — and adding strings after 40 presets exist is the expensive order |

### The core claim, re-derived

Hard rule 1 is the product's entire promise, so I checked it rather than
trusting `AGENTS.md`.

- `cargo tree --all-features` over the full closure — **84 crates** in the
  release build, **118** with `--all-features`. Grepped for `reqwest|hyper|ureq|
  curl|tokio|async-std|smol|quinn|h2|http|tungstenite|russh|ssh2|openssl|
  native-tls|rustls|webpki|hickory|trust-dns|socket2|mio|dns-lookup`: **none**.
- `grep` for `std::net|reqwest|hyper::|TcpStream|UdpSocket|lookup_host` across
  `core/src`: **none**. The only `std::process` use is `sandbox.rs`, re-execing
  this crate's own binary.
- `scripts/no-network-report.sh` additionally reads the release artefact's
  undefined dynamic symbols, which is the check that catches a statically linked
  socket call.

**The claim holds.** That is the most important thing to get right and it is
right.

---

## Findings

Numbered. Each is: what it is, why it matters, where it lives, how confident I
am. Severity is about *reachability from bytes the user picked off their disk*.

### 1. An unbounded zlib inflate sits on the main untrusted-input path — HIGH

`core/src/colour.rs:709-713`:

```rust
let mut profile = Vec::new();
flate2::read::ZlibDecoder::new(&data[nul + 2..])
    .read_to_end(&mut profile)
    .ok()?;
```

There is no ceiling on the decompressed size. A PNG of about 4 KiB whose `iCCP`
payload is a zlib stream expanding to gigabytes grows one `Vec` until the
allocator refuses, at which point Rust calls `handle_alloc_error` and the
process **aborts**.

This is not an opt-in path. `validate::validate_bytes` calls
`crate::exif::read` (`validate.rs:313`), which calls `colour_profile`
(`exif.rs:109`), which calls `ColourProfile::read`, which routes PNG to
`png_icc`. So **every** `px_inspect`, `px_exif`, `px_process`, `px_batch`, and
every folder `plan` hits it for any file whose magic bytes are a PNG. So does
`worker.rs:593`.

The inconsistency is visible in the same function: `desc_text` and `mluc_text`
cap at `MAX_TEXT` (1024) and `tag_table` caps at `MAX_TAGS` (1024). The one
inflate in the module is the one where the cap was missed.

**Why it matters.** Hard rule 3 says no panic on a crafted file; an allocation
failure is an abort, which is louder and worse. This is the same class of defect
as the 60000×60000 header, and the engine's answer to that one is a limit check
before the allocation. This is the limit check that is missing.

**Confidence: high.** Read the code, traced every caller, confirmed the call
chain is ungated by any feature.

**Fixed in phase-18.** `colour::png_icc` now inflates through a
`Read::take(MAX_ICC_PROFILE)` — 4 MiB, chosen against the largest profile that
exists in the wild rather than against taste — and a profile over it is reported
as untagged rather than allocated. `colour::tests::an_iccp_payload_that_inflates_past_the_cap_is_ignored`
builds a 60 KB PNG carrying a 4 MiB profile and fails against the code as it
stood.

### 2. `cargo test` — the command the README tells a contributor to run — does not compile — HIGH

`README.md:91` says:

```bash
cd core
cargo test
```

That fails, in two independent ways:

```
$ cargo check --manifest-path core/Cargo.toml --tests
error[E0433]: cannot find `stream` in `pixelsmith_core`
  --> tests/streaming_peak.rs:187:30
error[E0433]: cannot find `stream` in `pixelsmith_core`
  --> tests/streaming_peak.rs:190:42

$ cargo test --manifest-path core/Cargo.toml --no-run
error[E0601]: `main` function not found in crate `resize_bench`
error: could not compile `pixelsmith_core` (example "resize_bench")
```

`core/tests/streaming_peak.rs` calls `pixelsmith_core::stream::decode_resized`
with no feature gate, and `stream` is `#[cfg(feature = "streaming")]`
(`lib.rs:24-25`). `core/examples/resize_bench.rs:29` is
`#![cfg(feature = "simd")]`, so with `simd` off the crate has no `main`.

**Why it matters beyond the broken command.** `scripts/verify.sh` runs
`--all-features`, so the gate is green and the defect is invisible to the only
thing that watches. That means:

- **`--no-default-features` has never compiled.** phase-07's notes claim the
  AVIF refusal arm "was additionally run under `--no-default-features`". It
  cannot have been, on this tree.
- **`cargo test --features streaming` without `simd`** fails on the example.
- A contributor who follows the README gets a compiler error, not a test run.
- `engine-matrix`'s `cargo test --release --lib --target X --no-run` does not
  catch it either, because `--lib` skips integration tests *and* examples.

**Confidence: high.** Both errors reproduced by hand, in this run.

### 3. The app crashes on an iPhone photograph — HIGH

`app/lib/rust/models.dart:21`:

```dart
static OutputFormat fromJson(String value) =>
    OutputFormat.values.byName(value.toLowerCase());
```

The enum stops at `avif`. `core/src/format.rs` has ten variants; `Heic` and
`Heif` are among them and `heic::detect` reports them **with no feature gate at
all**, precisely so a build without the codec can still say "this is a HEIC".

`ValidateReport.fromJson` calls it at `models.dart:202`, and
`ProcessResult.fromJson` at `models.dart:326` (`detected_format`).
`Enum.byName` **throws** on a name it does not know.

So: the user picks a HEIC from the photo picker, the engine correctly reports
`"heic"`, and the Dart side throws `ArgumentError`. Hard rule 9's engine half is
honest and hard rule 9's product half is a crash.

The fix already exists — `workspace/phase-06/shrinkray-phase-06.patch` adds both
variants and applies cleanly (I ran `git apply --check`). It has never landed,
because `app/` is a submodule this pipeline cannot push to. **This is finding 17
reaching the user.**

**Confidence: high.** Enum read, call sites read, patch applies cleanly.

### 4. The Dart side cannot tell a skipped file from a written one — HIGH

`worker.rs:282` emits `skipped: Option<SkipReason>` on every `Outcome`, and
`worker.rs:289` defines `ok()` as `error.is_none() && skipped.is_none()`. The
Dart `BatchOutcome.ok` (`models.dart:358`) is `error == null`.

So a duplicate, an unreadable file, a file over the size limit and a file whose
upscale was refused all report `ok == true`. **The batch report says 400
succeeded when 30 were skipped.** The engine built three skip reasons, a grouped
`skip_reasons()`, and a `BatchReport` whose three counts are designed to add up,
and the only consumer of that report counts two of the three states.

The Dart side also has no `SkipReason` type at all, so even if `ok` were fixed
there is nothing to render.

`workspace/phase-14/shrinkray-phase-14.patch` fixes this and applies cleanly.

**Confidence: high.**

### 5. Dart has no model for five phases of the JSON contract — HIGH

Counted: `grep -c 'colour\|animation\|skipped\|chroma' app/lib/rust/models.dart`
returns **0**.

| Contract surface | Engine emits it | Dart has it |
| --- | --- | --- |
| `ValidateReport.colour` (5 fields) | `validate.rs:232` | **no** |
| `ExifInfo.colour` | `exif.rs:32` | **no** |
| `Outcome.colour` (4 fields) | `worker.rs:268` | **no** |
| `Outcome.animation` / `AnimationOutcome` | `worker.rs:275` | **no** |
| `Pipeline.colour_subsampling` | `pipeline.rs:187` | **no** |
| `ProcessRequest.progressive` | `ffi.rs:240` | **no** |
| `Pipeline.colour.{working_space,keep_source_pixels,embed_profile}` | `pipeline.rs:202` | **no** |
| `Pipeline.animation` (policy) | `ffi.rs:255` | **no** |
| `BatchRequest.{progressive,animation,policy}` | `ffi.rs:432-444` | **no** |
| `SkipReason`, `BatchPolicy` | `worker.rs:103-224` | **no** |
| `ValidateReport.{frames,frames_truncated}` | `validate.rs:217,225` | **no** |
| `Capabilities.{jpeg,png,gif,tiff,bmp,ico,gif_encode,heic_decode,avif_decode,jpeg_progressive,jpeg_chroma_subsampling}` | `lib.rs:52-101` | **no** (reads 4 of 13) |

The engine changed the contract in phases 06, 07, 12, 13 and 14. `AGENTS.md`
rule 4 of the app section requires the Dart side in the same phase. **Five
phases of drift, each with a patch that was never landed except phase-12, which
has no patch at all.**

Dart's `fromJson` ignores unknown keys, so none of this throws. It is **silent**:
the app simply cannot show the one sentence phase-12 exists to enable, cannot
offer a chroma control, cannot show what happened to an animation, and greys out
nothing (finding 4's cousin).

**Confidence: high.** Every row checked against both sides.

### 6. No size or count limit on any JSON the FFI parses — MEDIUM

`ffi.rs:298`, `ffi.rs:472`, `ffi.rs:536` are
`serde_json::from_slice(raw)` on Dart-supplied bytes, and **nothing precedes
any of them**. `Limits::max_input_bytes` is consulted only afterwards, inside
`validate_bytes` (`validate.rs:253`) — after the whole document, including every
embedded base64 byte array, has been materialised into `Vec<u8>`s.

`px_zip` is the worst of the three: `BatchFile { name: String, bytes: Vec<u8> }`
(`ffi.rs:455-459`) with no ceiling on the request, on the file count, or on
`bytes` per file; `zip_outputs` (`worker.rs:960-975`) deflates everything into one
in-memory `Vec`. **`px_zip` never constructs `Limits` at all.**

**Why it matters.** The blast radius is a user picking a huge folder rather than
a remote attacker, so this is not a vulnerability. It is a hole in the
"everything is bounded before it is allocated" property that hard rule 4 states
without exception, and this engine has been otherwise exemplary about it.

**Confidence: high** for the absence; **low** for whether it matters in
practice, because the input originates from a platform file picker.

**Fixed in phase-18**, with one correction. `ffi.rs` states four envelope
numbers — `MAX_REQUEST_BYTES` (64 MiB, derived as the payload ceiling times the
JSON expansion factor), `MAX_REQUEST_FILES` (512), `MAX_REQUEST_FILE_BYTES` (8
MiB) and `MAX_REQUEST_PAYLOAD_BYTES` (16 MiB). The first is checked before
`serde_json` sees the document; the other three are enforced in the deserialiser's
visitor, because a check on the parsed `Vec` is a check after the allocation.
`px_batch`'s copy of every byte array into a `Job` is **not** a copy — `f.bytes`
moves the `Vec` — so it was left alone; the prompt's claim that it is a second
full copy is wrong, and the only allocation there is one `Job` header per file.
`px_zip` still constructs no `Limits`, which is correct: there is no picture in
it, so the envelope numbers are the only bounds that mean anything there.

### 7. `CropSpec` does unchecked `u32` addition on Dart-supplied numbers — MEDIUM

`pipeline.rs:255`, `:257-259`, and the same shape at `:426`, `:428-430`:

```rust
if crop.x + crop.width > w || crop.y + crop.height > h {
```

`CropSpec { x: u32, y: u32, width: u32, height: u32 }` derives `Deserialize`
with no range constraint, and both `Pipeline` (`pipeline.rs:173`) and
`ProcessRequest` (`ffi.rs:231`) are reachable from `px_process` and `px_batch`.

`{"crop":{"x":4294967295,"y":0,"width":1,"height":1}}` overflows.

- In an **overflow-checked** build — which is `dev` and `test`, so every CI run
  and every `cargo test` — this **panics**.
- In a **release** build it wraps to `0`, the crop is accepted, and `image`
  clamps it into a zero-width buffer that becomes `Error::ZeroDimension`
  downstream. A clean error, by luck of a third-party clamp rather than by a
  check here.

`core/tests/properties.rs:278-280` generates `crop_x in 0u32..100`, so the
overflow region is never produced.

**Confidence: high** on the code; **medium** on release behaviour, which depends
on `image`'s clamping rather than on anything this crate asserts.

**Fixed in phase-18.** Both halves now call one `check_crop`, which compares by
subtraction (`crop.x < src_w && crop.width <= src_w - crop.x`) and refuses with a
new `Error::CropOutOfBounds` that names the picture and the rectangle. The
out-of-bounds crop in a batch therefore reports a **failure** rather than the
`TooLarge` skip it reported before, which is the honest classification: the file
is fine and the request does not fit it.

### 8. `px_buffer_free` trusts the caller's length — MEDIUM

`ffi.rs:54-62` stores the `Vec`'s **capacity** in `CAPACITIES`, keyed by address.
`ffi.rs:153` then rebuilds with `Vec::from_raw_parts(buffer.data, buffer.len, cap)`
— and `buffer.len` is caller-supplied, because `PxBuffer` is `#[repr(C)]` and
Dart reads and writes it by offset.

A host that corrupts `len` builds a `Vec<u8>` whose length exceeds the
initialised region. Nothing dereferences it today (`u8` has no `Drop`), so this is
UB-by-the-standard rather than an exploitable bug, and the pointer side *is*
checked. Storing `(len, cap)` and ignoring the caller's `len` would make it
impossible.

**Confidence: high** on the mechanism; **low** on exploitability.

**Fixed in phase-18**, and the mechanism is worse than "UB-by-the-standard":
`RawVec::cap_set` *asserts* `cap >= len`, so a host that corrupted `len` took the
process down with a non-unwinding panic inside a `no_mangle` free function — an
abort, on the cleanup path, in the build that ships. The registry stores
`(len, cap)` now and `px_buffer_free` ignores the caller's `len` entirely.

### 9. `px_selftest_panic` is an unconditionally exported abort in a shipping library — MEDIUM

`ffi.rs:557-565` is `#[unsafe(no_mangle)] pub extern "C" fn`, not behind
`#[cfg(test)]` and not behind a feature. `[profile.release] panic = "abort"`
(`Cargo.toml:359`) means its `catch_unwind` cannot catch, so in the shipped
`cdylib` it calls `abort()` and kills the Flutter app. The doc comment says it
"returns a normal error buffer instead of aborting", which is only true under
`cargo test`.

Nothing in the product calls it — but `DynamicLibrary.process()` (used at
`app/lib/rust/engine.dart:25-31`) hands the **entire symbol table** to the app
process, so the abort is reachable by anything that can run Dart.

Gotcha 8 in `docs/ARCHITECTURE.md` documents the abort. It does not document the
export.

**Confidence: high.**

### 10. `stream.rs` walks every row with a hard-coded 4-channel stride — MEDIUM

`stream.rs:592-625`:

```rust
let start = y as usize * width as usize * CHANNELS;   // CHANNELS == 4
let Some(src) = img.as_bytes().get(start..start + out.len()) else { out.fill(0); return; };
match img.color() { ColorType::Rgba8 => …, ColorType::Rgb8 => …, … }
```

`start` and the window size both assume an RGBA8 source. `DynamicImage::ImageRgb8`'s
`as_bytes()` is `W*H*3`, so **row 0 is right and every row `y >= 1` is read from
the wrong offset**, and the last rows fail the `get` and are silently
zero-filled. `image` decodes every JPEG to `ImageRgb8`, and `Source::Whole` is
the arm every non-PNG takes.

No panic, no error — a skewed picture with a black band.

The tests miss it because both whole-image-arm tests assert **only dimensions**:

- `stream.rs:860` `a_jpeg_goes_through_the_whole_image_arm` → `got.dimensions() == (100, 67)`
- `stream.rs:876` `a_png_this_arm_cannot_decode_falls_back_rather_than_scrambling` — the
  *name* promises pixels and the body never looks at one.

Behind `streaming`, which is off by default. That is why it has never been seen.

**Confidence: high** on the mechanism (the arithmetic is wrong for a 3-channel
source); **medium** on the end-to-end consequence, which I did not execute.

### 11. The streaming fallback allocates up to 2 GB outside its own budget — MEDIUM

`stream.rs:489-492`:

```rust
crate::decode_bounded(input, limits).unwrap_or_else(|_| DynamicImage::new_rgba8(src_w, src_h))
```

`src_w`/`src_h` come from the **header** (`stream.rs:127`). The only ceiling
applied is `check_streamed_header` = `4 × max_pixels` (`:114-116`), which on the
desktop profile is 512 MP — so the fallback blank is ~2 GB. It is in neither
`working_set_bytes` (`stream.rs:152-170`) nor `streaming_memory_budget`
(`validate.rs:129-131`), so it bypasses the ceiling the feature exists to
enforce.

Requires `PngRows::open == None` **and** a failing `decode_bounded`. Behind
`streaming`.

**Confidence: high** on the arithmetic; **low** on reachability, since I did not
construct the input.

### 12. A full RGBA buffer is allocated to evaluate a boolean — LOW

`worker.rs:741`:

```rust
if !spec.has_effect(&image::DynamicImage::new_rgba8(src_w, src_h)) {
```

`ResizeSpec::has_effect` takes a `&DynamicImage` and answers a question about
the *spec*. This allocates `src_w × src_h × 4` — up to 512 MB desktop, 160 MB
mobile — on every streamed file, and throws it away on the next line.

**Confidence: high.**

### 13. The FFI error string leaks on every failed call — MEDIUM (Dart side)

`ffi.rs:82-91` puts the message in `PxBuffer::error` via `CString::into_raw`.
`px_buffer_free` (`ffi.rs:142-154`) frees **only `data`** — it never touches
`error`. `px_string_free` is declared at `app/lib/rust/bindings.dart:116` and
**called from nowhere** in `app/lib/` or `app/test/`. `engine.dart:256-277` reads
the message and frees only the data buffer.

So every `PxStatus::Error` and `PxStatus::InvalidArgument` leaks a `CString`
permanently, and `Isolate.run` guarantees a fresh isolate — hence a fresh
`CAPACITIES` map — per call. The Rust test at `ffi.rs:650`
(`a_thousand_rounds_of_the_boundary_leak_nothing`) calls `px_string_free`
explicitly, so the leak is invisible to the Rust suite.

**Confidence: high** — read both sides and grepped for the call.

### 14. Three features exist only on x86-64 Linux — MEDIUM

`streaming`, `simd` and `heic` are all off by default. `scripts/verify.sh` builds
them with `--all-features` **on `ubuntu-latest` only**. Every other job —
`android-apk`, `windows-exe`, all six `engine-matrix` targets, `supply-chain.yml`,
the FFI contract test — builds the default set.

So `heic` decode has **never been cross-compiled**. It has never been in a
shipped artefact. `simd` has never been on ARM, which is the stated reason it is
off. `streaming` has never left the runner.

`docs/ARCHITECTURE.md` gives the reason for `heic` as "the `webp-lossy-matrix`
job does not build it" — **and that job is a patch** (finding 17), so the stated
reason names something that does not exist.

The fuzz crate is worse: `fuzz/Cargo.toml:29` depends on `pixelsmith_core` with
**default features only**, so no fuzz target can ever reach `stream`,
`resize::simd` or `heic::decode`.

**Confidence: high.**

### 15. No CI job ever runs a test on any target but Linux x86 — MEDIUM

`engine-matrix` (`build.yml:350-389`) compiles test binaries on five targets with
`cargo test --release --lib --target X --**no-run**`. `--no-run` is compile-only.

The only executed test run in the entire pipeline is `scripts/verify.sh` →
`cargo test --all-features` on `ubuntu-latest`, reached from `build.yml:61` and
`automation.yml:487`.

So "the crate cross-compiles" is continuously verified and "the crate *works* on
Windows or Android" is not verified anywhere. `--lib` also means finding 2's
integration-test and example failures are structurally invisible to that job.

**Confidence: high.**

### 16. The fuzz harness is never run by anything — MEDIUM

`grep -rn fuzz .github/` → **empty**. `grep -n fuzz scripts/verify.sh` → empty.
Only `scripts/fuzz.sh` invokes `cargo fuzz`, and only when a human runs it.
`fuzz/README.md:83-88` says so honestly.

Coverage gaps, against the entry points that touch untrusted bytes:

| Entry point | Fuzz target |
| --- | --- |
| `format::detect_format`, `validate::validate_bytes`, `lib::decode_bounded`, `exif::read`, and the seven per-codec decoders | **yes** (11 targets, 99 seeds) |
| `heic::detect`, `heic::header` | **no** — and `fuzz/src/lib.rs:280` returns `b""` for `Heic | Heif`, so the seeded corpus never presents an `ftyp` header at all |
| `heic::decode` | **no** — unreachable anyway, `heic` is off in `fuzz/` |
| `colour::ColourProfile::read` | **no** — which is why finding 1 was found by reading rather than by fuzzing |
| `lib::process` (the whole chain end to end) | **no** |
| `worker::process_one`, `folder::plan` | **no** |
| All five JSON-taking `px_*` entry points | **no** |
| `sandbox::Job::read_from` | **no** |

One stale comment: `fuzz_targets/decode_gif.rs:8` says the engine "iterates
`into_frames()`" — that was replaced in phase-13 by the non-decoding
`scan_gif_frames`, so the comment describes a decoder path the engine no longer
has.

**Confidence: high.**

### 17. Four CI jobs and one release workflow exist only as patches — MEDIUM

`ls workspace/*/*.patch` finds ten. Four of them add jobs that are **not in the
tree**, and — this is the part that matters — **`git apply --check` succeeds on
all four right now**:

| Patch | Adds | Status |
| --- | --- | --- |
| `workspace/phase-08/build-webp-lossy-matrix.patch` | `webp-lossy-matrix`, 8 targets | not applied |
| `workspace/phase-10/bench.yml.patch` | the benchmark regression gate | not applied |
| `workspace/phase-15/supply-chain-ci.patch` | `reproducible-build`, `vet` (the `supply-chain.yml` half **is** in the tree) | partially applied |
| `workspace/phase-16/release-ci.patch` | `release.yml` — `identity`, `android`, `windows`, `publish` | not applied |

The stated reason is a credential wall: the push is refused `workflows`
permission on `.github/workflows/`, and adding `workflows: write` to
`automation.yml` would itself need a workflow push. **That wall is still there**,
which is why the patches still apply.

The compounding cost is that `docs/ARCHITECTURE.md` now cites
`workspace/phase-08/build-webp-lossy-matrix.patch` as the *reason* `heic` is off
— which is a patch, not a job (finding 14). A chain of claims resting on
patches that cannot be applied is how a documentation file starts lying.

**Confidence: high.** All ten patches checked with `git apply --check`; nine
apply, and `shrinkray-phase-05.patch` applies 6 of its 9 files because the
submodule has since moved past it.

### 18. There is no release workflow, and `RELEASE-SHA256.txt` records hashes — MEDIUM

`.github/workflows/` contains `automation.yml`, `build.yml` and
`supply-chain.yml`. Nothing else. `workspace/phase-16/.done` exists.

`scripts/RELEASE-SHA256.txt` carries two SHA-256 rows with real hashes and sizes
(58,109,122 B APK; 57,394,493 B AAB) and a provenance line. `scripts/build-release.sh`
exists. **Nothing in the tree invokes any of it.**

I could not determine from the tree how those artefacts were produced, and I am
recording that rather than guessing.

**Confidence: high** on the absence; **unresolved** on the provenance.

### 19. `core/src/error.rs` has no tests at all — MEDIUM

`grep -c 'cfg(test)' core/src/error.rs` → **0**. It is the only module in the
crate with none.

`AGENTS.md` says "Every module has them", and hard rule 9 makes the message the
product: *"Never surface 'Error: decode failed'. Say what actually happened and
what to do about it."* There is 19 variants of that product and nothing asserting
a single one.

There is a live defect of exactly the kind that test would have caught.
`error.rs:128`:

```rust
#[error(
    "this resize would need {needed} bytes of working memory and this device          allows {budget}; choose a smaller output size"
)]
StreamingBudgetExceeded { budget: u64, needed: u64 },
```

**Ten literal spaces** mid-sentence, in a string shown verbatim to a user.

**Confidence: high.**

### 20. Four tests assert nothing, and one of them is hard rule 4's sentinel — MEDIUM

`assert!(true)` appears nowhere. These four assert *nothing measurable*:

| Location | Why it matters |
| --- | --- |
| `core/tests/hostile.rs:660` `a_raw_reader_respects_decoder_limits_on_hostile_input` | It calls `reader.no_limits()`, then `limits.apply_to_decoder(&mut reader)`, then `let _ = reader.decode();`. **Deleting the body of `apply_to_decoder` makes this test pass.** It is the only test that exercises `ImageReader` directly, its comment says it exists so "a decoder limit that `validate` forgets to apply is still visible here", and it asserts that neither is visible. |
| `core/src/worker.rs:2185` `rayon_has_more_than_one_thread` | `assert!(rayon::current_num_threads() >= 1)`. A tautology. The name claims `> 1`; a single-threaded pool passes. |
| `core/tests/sandbox.rs:908` `the_worker_entry_point_is_reachable_and_exits_cleanly` | Body is `let _entry: fn() -> ! = run_worker;`. A compile-time existence check. The name claims "exits cleanly", which nothing tests. |
| `core/src/stream.rs:859, 875` | Both whole-image-arm tests assert only `dimensions()`. Finding 10 lives in the gap between the name and the assertion. |

Several more assert only `.is_ok()` / `.is_err()` where a specific variant was
available — `lib.rs:281` (`!err.to_string().is_empty()`), `format.rs:2090`,
`validate.rs:535`, `validate.rs:540` (whose name claims "rather than half
decoded" and never checks for partial output).

**Confidence: high.** All read directly.

### 21. `docs/phase-status.md` contradicts the `.done` markers it declares authoritative — MEDIUM

The file's preamble: *"The table and the marker are written by different actors
on purpose … If the two ever disagree, the marker wins and the table is a bug."*

Rows 16 and 30 read `phase-02 … PENDING` and `phase-16 … PENDING`. Both
directories have a `.done` marker on disk:

```
workspace/phase-02/.done
workspace/phase-16/.done
```

So by the file's own rule, both rows are bugs. phase-16 is the more expensive one:
its row says "Publishes the installable binaries" while `RELEASE-SHA256.txt`
carries hashes for binaries no workflow in the tree produces (finding 18).

**Confidence: high.**

### 22. `docs/ARCHITECTURE.md` says "Eighteen modules" and omits the sandbox — LOW

Line 12: "Eighteen modules, one submodule, no circular references." The module
table has 18 rows. `core/src/sandbox.rs` (32.6 KB, `pub mod sandbox` at
`lib.rs:23`) is **not in the table**.

This matters more than a miscount. ARCHITECTURE's "The two memory bounds" section
calls those two methods "the whole defence" on Windows — and on Windows,
`sandbox.rs:618`'s `apply_memory_limit` is a **documented no-op returning
`self`**. The module that is the memory defence on Unix, and is absent on
Windows, is the one module the architecture document does not describe.

**Confidence: high.**

### 23. The `linux` CI job ships no engine and cannot fail — LOW

`build.yml:315-347`, `continue-on-error: true` at `:320`. It runs
`flutter pub get` and `flutter build linux --release` and **never builds the Rust
engine**. `app/linux/CMakeLists.txt` at the pinned submodule commit `5d0e9cc` has
no rule to build or install it — that rule lives only in
`workspace/phase-05/shrinkray-phase-05.patch`, which **does not fully apply**
anymore (6 of 9 files; the submodule moved past it).

So a Linux bundle with no engine in it is a green job.

**Confidence: high.**

### 24. `px_process` walks the container twice — LOW

`worker.rs:465` runs `validate_bytes`; `ffi.rs:350-352` runs it again to recover
`r.format`. Each walk includes the ICC inflate of finding 1.

**Confidence: high.**

### 25. Two arithmetic claims in the code are wrong on 32-bit targets — LOW

- `colour.rs:934` — `body.get(start..start + len)` where `start` is an
  unchecked `u32` from the profile. `len` is capped at 1024; on a 32-bit target
  a large `start` overflows.
- `colour.rs:742` — `at = end + (len & 1)` where `end` is bounds-checked only
  inside the `ICCP` branch.
- `format.rs:779-781` — the comment says "`u32 -> usize` is lossless on every
  shipped target, so the product cannot overflow". That reasoning is **wrong as
  written** for `u32 × u32 × 3` on a 32-bit target. Every shipped target is
  64-bit, so nothing breaks today; the comment states an invariant the code does
  not have.

**Confidence: high** on the code; **low** on severity, since no shipped target
is 32-bit.

### 26. `px_exif` consults no `Limits` at all — LOW (found while fixing finding 6)

`ffi.rs`'s `px_exif(ptr, len)` calls `crate::exif::read(bytes)` and nothing else.
Every other entry point that takes a file reaches `validate_bytes`, which checks
`max_input_bytes` first; this one does not, and neither `Limits::mobile()` nor
`Limits::default()` is ever constructed on this path.

So every allocation `kamadak-exif` makes while walking a hostile tag tree —
arrays whose count is stated in the file rather than found in it — is bounded only
by the buffer Dart already had in memory. It is not a hole of the shape finding 1
was: the bytes are resident before this function is called, so an input-size check
would bound nothing by itself. What is missing is the ceiling *inside* the parse,
which is the same kind of thing `MAX_TAGS` and `MAX_TEXT` are for one module over.

**Not fixed by phase-18**, which was scoped to the four findings it was given;
recorded here so it is a decision rather than an oversight. It belongs with a
`Limits` consultation on `px_exif` and a cap inside `exif::read`.

**Confidence: high** on the absence — it is two functions long; **low** on
exploitability, because the input comes from a file picker and the decoder refuses
values longer than the buffer it was given.

---

## Could not verify

Stated plainly, because guessing silently is worse than admitting a limit.

1. **The Android and Windows artefacts in `scripts/RELEASE-SHA256.txt`.** The
   hashes and sizes are there; nothing in this tree produces them. There is no
   release workflow (finding 18). I cannot tell whether they were built by an
   unapplied workflow, by a job that has since been removed, or by hand.
2. **Whether any of this works on a real Android device or a real Windows
   machine.** This runner is Linux x86-64. `cargo test` was run here and nowhere
   else. Every claim about the Windows sandbox, the Windows job objects, the
   Android NDK build and the iOS target is a claim about a compile, not a run
   (finding 15 makes that structural).
3. **`heic` decode on any real HEIC file.** The only fixtures in the tree are
   synthetic containers built by `heic::synthetic` with an HEVC bitstream from
   `heic_rs::hevc::synth`. `docs/HEIC.md` names this as the weakness of the
   chosen crate — it reads fewer real-world files than libheif. No third-party
   photograph exists in the tree, for licence reasons, so this cannot be closed
   from here.
4. **The performance numbers.** I did not re-run the criterion suite or
   `streaming_peak` (the latter takes 83 s in a debug build per phase-11's
   notes). Every figure in `docs/BENCHMARKS.md` is quoted, not re-measured. The
   machine is one x86-64 runner; the baseline in `core/benches/baseline/` was
   taken on a different one, which is why `scripts/bench-compare.py` deliberately
   gives no verdict on a machine mismatch.
5. **`cargo deny check advisories`.** Not installed, and this environment has no
   network. `deny.toml`'s `unmaintained = "all"` and the one ignored advisory
   (`RUSTSEC-2024-0436`, `paste`) are arguments, not results. `scripts/deny-check.sh`
   says so in its own output.
6. **`cargo vet`.** `vet/config.toml` has six hand-written audits.
   `vet/imports.lock` has never been generated. No audit has ever been executed.
7. **Findings 10 and 11 were not reproduced end to end.** Both are behind the
   off-by-default `streaming` feature; I read the arithmetic and it is wrong for
   a 3-channel source, but I did not build a JPEG through `Source::Whole` and look
   at the pixels. A reviewer should treat "the stride is wrong" as high confidence
   and "a user sees a skewed picture" as unverified.
8. **Whether the `app/` patches, merged, produce a correct Dart app.** I checked
   that nine of ten apply. I did not stack them, because `phase-05/07/13/14`
   collide in `Capabilities.fromJson`, `ProcessResult.fromJson` and
   `BatchOutcome.fromJson`, and the status notes say so. Stacking them is a
   human merge and I would rather name it than guess at the result.
9. **Whether the FFI error-string leak (finding 13) and the UTF-16 length bug
   (`app/lib/rust/engine.dart:191-218`) have been observed in the field.** The
   code is unambiguous; I have no runtime evidence and no issue tracker.
10. **`scripts/no-network-report.sh` check 5** (`nm -D` on the release artefact)
    needs a release build under `core/target/`. I verified checks 1 and 2 by hand
    from `cargo tree` and `grep` — which is the part the claim rests on — and did
    not run the symbol-table check, so "no statically linked `socket()`" is my
    inference from an absent dependency rather than an observation.

---

## What the phases that follow are for

Eleven phases, in `workspace/phase-18` through `workspace/phase-28`. Five are
Track A and six are Track B; see `workspace/PHASES.md`. In leverage order:

1. **phase-18** — bound every allocation untrusted bytes can reach (findings 1, 6, 7, 8)
2. **phase-19** — make `cargo test` work in every feature configuration (finding 2)
3. **phase-20** — the Dart JSON contract, plus a checker that makes drift fail the gate (findings 3, 4, 5, 13)
4. **phase-21** — the tests that assert nothing, and the module with none (findings 19, 20)
5. **phase-22** — Track B: the design system and the shell
6. **phase-23** — Track B: the preview canvas and before/after
7. **phase-24** — Track B: the pipeline editor
8. **phase-25** — Track B: live progress, cancellation and the batch list
9. **phase-26** — Track B: export flows and the folder plan
10. **phase-27** — CI that executes what it compiles (findings 14, 15, 16, 17, 18, 23)
11. **phase-28** — `streaming` correctness, and the shipping decision (findings 10, 11)

Findings 9, 12, 21, 22, 24 and 25 are folded into the phases above rather than
getting a phase each; each names them.
