# Roadmap

Everything we want this project to become. Ordered roughly by leverage, not by
difficulty.

## How to read this file

Four states, and the difference between them is the whole point:

- **[done]** — implemented, and a test you can find covers it. Verified against
  the tree by `docs/AUDIT.md`, not by memory.
- ~~struck through~~ — **not going to happen as written.** Either it was a bad
  idea, or the code found a better one. The reason is inline.
- **[added by audit 2026-10-08]** — not in the original roadmap; found by
  `phase-17` and given a phase.
- unmarked — not started.

`docs/AUDIT.md` has the per-item classification with evidence, and
`workspace/PHASES.md` has the phase each new item is assigned to.

Principle: the app never uploads a photo, never phones home, and never lies to
the user about what it did to their file.

---

## Visual / UI

*Every item in this category is `ABSENT`. `app/lib/main.dart` is 80 lines: it
opens a window, calls `px_version`, and shows the engine's version string. No
screen calls the engine. Track B is phase-22 through phase-26.*

- **Adaptive light/dark/system theme** with hand-tuned contrast in both, not just inverted greys. WCAG AA on all body text and controls. — *phase-22*
- **Single accent colour**, one surface elevation model, consistent 4pt spacing grid. No gradients-for-decoration. — *phase-22*
- **Before/after slider** as the centrepiece: draggable split view with synced zoom and pan, plus a magnifier loupe at the cursor for pixel-peeping edges. — *phase-23*
- **Zoomable checkerboard** for transparency (not grey squares), with zoom %, fit-to-window, 1:1, and actual-size buttons. — *phase-23*
- **Format/size result card** shown live while dragging any slider: dimensions, estimated bytes, and a savings bar. — *phase-23, phase-24*
- **Non-destructive live preview** using a downscaled proxy while dragging, full-resolution only on release, so the UI never stutters. — *phase-23*
- **Skeleton + shimmer** for loading a photo; **empty states** that teach instead of saying "no files". — *phase-22*
- **Preset chips grouped by category** (Social / Web / Print / Device / Email / Dev) with icons, searchable, recently-used pinned. — *phase-24*
- **Uniform control styling** across Windows (Fluent-ish density) and mobile (Material-ish 48pt touch targets) — same layout grammar, platform-appropriate metrics. — *phase-22, phase-26*
- **Motion**: 120–180ms ease-out for panels, no bounce, respects `prefers-reduced-motion`. — *phase-22*
- **Custom theming hook** so the accent colour can be overridden; respects the OS high-contrast mode. — *phase-22*

## User-friendliness

*No UI to put most of this in. The engine halves that exist are marked `[done]`;
the rest follows Track B.*

- **Paste from clipboard** and **drag-and-drop anywhere** on the window, plus a desktop "open with" shell association. — *phase-26, then a platform phase*
- **Zero-config default**: pick a photo, get a sensible resized output. Advanced controls stay collapsed. — *phase-24*
- **Explain every number**: "1920 × 1080 (from 4000 × 3000, fit inside)" and why `no_upscale` clamped it. — *phase-23, phase-24*
- **Human errors**: never "Error: decode failed". Say "This file isn't an image we can read, or it's damaged." — **[done] in the engine**: 19 `Error` variants, each carrying a written-for-a-person sentence, and `format::tests::every_refusal_says_what_to_choose_instead` holds them to it. **[added by audit 2026-10-08]** `error.rs` has **no test module**, which is finding 19 — phase-21. ABSENT in the app.
- **Warn before destructive things**: dimension collapse below the source, quality floor reached, byte target impossible. — *phase-24, phase-26*
- **Undo/redo across the whole pipeline**, including preset changes and batch runs. — *not yet written down in any phase; see `docs/AUDIT.md`*
- **Remember settings per device** (mobile) and persist last-used preset on desktop. — *not yet written down*
- **Onboarding that respects competence**: first run does a real resize, not a tutorial carousel. — *phase-24*
- **Full keyboard access** on desktop, including a command palette for common actions. — *phase-22 (access), command palette unwritten*
- **Searchable settings**; no more than two levels deep in any menu. — *phase-24 (preset search)*
- **Sensible filename handling**: increment rather than overwrite (`photo-2.jpg`), configurable template. — *phase-26*
- **Explicit progress and cancel** on anything over ~200ms, so nobody wonders if it hung. — *phase-25*

## Live progress

*All eight items are `ABSENT` or `PARTIAL`. `ROADMAP.md` rates this category
highest-leverage of anything in the file, and the audit agrees: it is the largest
block of missing product. phase-25.*

- **Determinate progress bar** with real `done / total`, not an indeterminate spinner. — *phase-25*
- **Per-file rows in the batch list** with individual state: queued → decoding → resizing → encoding → done / failed. — *phase-25*. **PARTIAL in the engine**: `Outcome` carries the terminal state; the intermediate states are internal to `process_one` and never surface, so the UI can only show what the engine reports.
- **Throughput readout**: images/second, ETA, elapsed, and peak memory. — *phase-25*. Images/second, ETA and elapsed are derivable. **Peak memory is not** — nothing measures it outside `core/tests/streaming_peak.rs`.
- **Live savings counter** during the run: "12.4 MB saved so far." — *phase-25*. `Outcome.input_bytes`/`output_bytes` make the total exact.
- **Cancellation that is instant and honest** — no half-written output files, and the UI says how many finished before stopping. — *phase-25*. **PARTIAL in the engine**: `CancelToken` works, and `process_all` checks it *between files*, not during one.
- **Background processing** on mobile with a persistent notification (Android) carrying progress and a cancel action. — *phase-25 decides and records*
- **Streaming preview** of the first result while the rest of the batch finishes. — *phase-25*
- **Error log per file** with the reason, expandable to the underlying decoder message. — *phase-25*
- **Toast + inline status** for every action, so nothing completes silently. — *phase-22, phase-25*

## More features

- **Target-size mode** (already built): "under 250 KB" via binary-searched quality. — **[done]**
- **Batch + ZIP export**, plus "save to folder" and "save next to original". — **[done]** in the engine (`worker`, `folder`); *phase-26* for the destinations.
- **Format conversion** JPEG/PNG/WebP/GIF/TIFF/BMP/ICO with per-format capability awareness. — **[done]** 8 writable formats plus AVIF; `capabilities()` is the honest list and a test asserts it cannot drift from the dispatch.
- **Auto-orient from EXIF**, with a manual override and live rotate/flip. — **[done]** all eight EXIF values tested; *phase-24* for the manual override.
- **Crop with aspect lock**, free-form or locked to the target ratio, draggable handles, 3×3 grid. — **PARTIAL**: `CropSpec` exists and is bounds-checked; *phase-23/24* for the handles, grid and lock.
- **Metadata inspector** with a clear "this file carries GPS" banner and strip-by-default. — **PARTIAL**: `ValidateReport.sensitive_tags` and `ExifInfo.has_gps` exist; *phase-24* for the banner.
- **Watermark** text or image, 9-grid position, opacity, margin. — *not started; needs an engine phase*
- **Offline AI background removal** (ONNX, ~42 MB, downloaded once). — ~~not as written~~. "Downloaded once" requires a network capability and hard rule 1 forbids one in the engine. Either the model ships **in** the app bundle, or this becomes an opt-in companion download handled by the app rather than the engine. **Decide before writing a phase.**
- **Offline AI upscale** (Real-ESRGAN / Swin2SR) with CPU and GPU paths. — *not started; needs an engine phase, and the same "downloaded once" question*
- **Smart compression suggestions**: estimate whether WebP beats JPEG at the same visual quality, and recommend the smaller one. — *not started*. `TargetBytes` measures one format at a time; comparing two is a second encode pass, so the cost needs stating.
- **Deduplicate identical images** by content hash before processing a folder. — **[done]** BLAKE3 over the decoded pixels *and* the whole request, truncated to 128 bits.
- **Animated GIF handling** that tells you it will be flattened instead of quietly destroying frames. — **[done]** preserve-where-possible, refuse-where-not.
- **HEIC/HEIF decode** (iPhone photos) — a real gap in every competitor. — **PARTIAL**: `heic-rs` behind an off-by-default `heic` feature. **[added by audit 2026-10-08]** it has **never been cross-compiled and is in no shipped artefact** — finding 14, phase-27. `docs/HEIC.md` names the crate's weakness (it reads fewer real-world files than libheif) and there is no third-party HEIC in the tree to test against.
- **AVIF encode** behind the existing optional feature. — **[done]** on by default. This build **cannot read AVIF back** and `capabilities().avif_decode` says so.
- **Print sizing**: dimensions in cm/inches at a stated DPI, with a paper-size picker. — *not started; phase-24 is the natural home*
- **RAW** (CR2/CR3/NEF/ARW/DNG) if the licence situation works out. — *not started; no codec in the tree*
- **Developer presets**: full Android density set, iOS app icon, store screenshot sizes. — **PARTIAL**: `presets.rs` has 41 including `store-screenshot`; no iOS icon set.
- **Colour conversion**: sRGB and Display-P3, with the ICC profile read and reported. — **[done]** Explicit gamma handling is *not* done — the engine uses the sRGB transfer function for every profile, which `docs/ARCHITECTURE.md` states rather than hides. HEIF `colr` is not read; a HEIC is reported untagged and exported as sRGB, which is the safe direction.
- **Progressive JPEG** toggle for web delivery. — **[done]** ~64% more bytes at q85 4:2:0, measured.
- **Chroma subsampling control** for JPEG, which is often a bigger lever than quality. — **[done]** defaults by content: 4:2:0 for photographs, 4:4:4 for `store-screenshot`. *phase-24* for the UI.

## Security and privacy

- **Zero network calls at runtime** — the engine has no HTTP/TLS dependency at all, so it *cannot* phone home. Assert this in CI with a dependency audit. — **[done]**, and **[added by audit 2026-10-08] re-verified independently**: `cargo tree --all-features` over the full 118-crate closure contains no HTTP, TLS, socket or DNS crate, and `core/src` names no networking symbol. `scripts/no-network-report.sh` additionally reads the release artefact's undefined dynamic symbols, which is what catches a statically linked `socket()`.
- **Reproducible builds**: locked deps, `--locked` in CI, published checksums. — **PARTIAL**: 1 of 8 targets measured, `x86_64-unknown-linux-gnu`. **[added by audit 2026-10-08]** `rust-toolchain.toml` does not exist, so a moving `rustc` invalidates every row — the `reproducible-build` job is in `workspace/phase-15/supply-chain-ci.patch` and has never run.
- **Dependency audit in CI** (`cargo deny` / `cargo audit`) gating the build. — **PARTIAL**: the jobs exist; `bans` is **currently red** on two duplicate crate versions.
- **Fuzzing**: `cargo-fuzz` targets for every decoder path — this is the real answer to "is it secure," since image parsers are the attack surface. — **PARTIAL**: 11 targets, 99 seeds. **[added by audit 2026-10-08]** **no CI job has ever run one** — `grep -rn fuzz .github/` is empty. There is no target for `heic::detect`/`heic::header`, for `colour::ColourProfile::read`, for the five JSON-taking `px_*` entry points, or for `worker::process_one`. Findings 16, phase-27.
- **Sandboxed decode** in a separate process with a hard memory cap, so a hostile file cannot take down the UI. — **PARTIAL**: real on Unix, re-exec'd child with `RLIMIT_AS` set in `pre_exec`. **[added by audit 2026-10-08]** **a documented no-op on Windows** (`sandbox.rs:618`), and it is not on the default decode path.
- **Decompression-bomb limits** enforced from the header, before any pixel buffer is allocated. — **[done]** eight enforcement points, documented in `docs/ARCHITECTURE.md`. **[added by audit 2026-10-08]** finding 1 is the same class of hole: the PNG `iCCP` inflate is uncapped and aborts the process. Phase-18.
- **Magic-byte format detection**, never the filename. — **[done]**
- **Path traversal sanitisation** on every output name, including ZIP entries. — **[done]** two phase-03 defects found and fixed here.
- **Extension whitelist** on outputs; no surprises like `.exe.jpg`. — *not started; `sanitise_stem` sanitises, nothing whitelists*
- **Optional binary transparency**: an in-app check that hashes the shipped library, plus a CI step that greps the source for network APIs. — **PARTIAL**: the CI grep is real and thorough (`scripts/no-network-report.sh`, six checks including the symbol table). **Nothing hashes the shipped library.**
- **No telemetry, ever** — stated in the README, enforced by the absence of any HTTP client in the dependency tree. — **[done]**
- **Metadata stripped by re-encoding**, not by clearing tags, so maker notes and thumbnails cannot survive. — **[done]**
- **GPS and identifying tags never written back**, even when metadata preservation is explicitly requested. — **[done]** nine tag classes filtered by `exif::is_sensitive`.

## Performance

- **SIMD resize path** (`fast_image_resize` or `libvips`) behind a feature flag, benchmarked against the current pure-Rust path. — **[done]** 2.6×–19.9× on x86-64 AVX2, agreeing to 1 LSB on photographic content. **PARTIAL as shipped**: `simd` is off by default because the speedup on the ARM phone this ships to is unmeasured, and it is compiled only by `verify.sh` on `ubuntu-latest` — never on any shipped target.
- ~~`libvips`~~ — **rejected, recorded in phase-09.** It would vectorise more and pay a C library with a real build script on all four shipped targets to accelerate one operation in a chain that is mostly decode and encode.
- **Thread-pool sized to the device**, not the core count, so phones don't thermal-throttle. — **[done]** `folder::pool_size`, a pure function so the decision is testable without a machine of a particular shape.
- **Streaming decode** for very large images so peak memory stays near `width × output_height`, not the source size. — **PARTIAL**: 4 MB against 613 MB for a 120 MP PNG. **[added by audit 2026-10-08]** two defects nobody found because the tests assert only dimensions: a fixed 4-channel stride that **skews every JPEG** the path touches (`stream.rs:592-625`), and a fallback allocation of up to 2 GB that bypasses the streaming budget (`stream.rs:489-492`). Findings 10 and 11, phase-28.
- **Early decode downscale** for previews: decode at 1/4 when the source is over 40 MP. — *not started; phase-23's proxy is the same idea done in the UI*
- **Startup budget**: no work in `main`; the first frame renders in under 100 ms. — *phase-22*
- **Benchmarks in CI** (`criterion`) so a regression is a red build, not a bug report. — **PARTIAL**: 5 bench targets, 20 cases, a 15% gate. **`bench.yml` is a patch** (`workspace/phase-10/bench.yml.patch`) and the gate does not run; the baseline is from a different machine than the runner, so `bench-compare.py` deliberately gives no verdict.
- **Parallel batch processing** with deterministic output, verified equal to a sequential run. — **[done]**
- **Exactly one resampling pass** per image, with crop → orient → resize order fixed and tested. — **[done]** the property the whole tree is organised around.

## Platform integration

- **Windows**: Explorer context-menu "Resize with…", jump-list recent presets, tray icon for batch mode, `SendTo` target, proper DPI scaling on mixed-DPI monitors. — *not started*
- **Android**: Photo Picker (no storage permission needed), MediaStore save, share sheet, WorkManager for large batches, dark theme. — *not started. The APK builds and carries the engine for every declared ABI; the app behind it is 80 lines.*
- **iOS**: PHPicker, Photos framework save, share extension, no network entitlement declared at all. — *not started. `app/ios/` has no engine integration at all.*
- **CLI** alongside the GUI for scripting, with the same engine. — *not started; the engine has a clean plain-Rust API (`process`, `decode_bounded`, `encode_fixed`, `encode_to_target`) so this is cheap*
- **Deep links / URL scheme** so other apps can hand off images. — *not started*
- **Portable zip for Windows** so it runs without an installer. — **PARTIAL**: the recipe, the checks and the docs exist. **[added by audit 2026-10-08]** there is **no release workflow in the tree**, and `scripts/RELEASE-SHA256.txt` records APK and AAB hashes that nothing in the repository produces — finding 18, phase-27.

## Trust and polish

- **Honest capability reporting** — grey out formats the build cannot write rather than failing at export. — **[done]** in the engine: `capabilities()` is generated from the same `cfg!` that decides the dispatch. **[added by audit 2026-10-08]** `app/lib/rust/models.dart` reads **4 of the 13** capability keys, so the app currently greys out nothing — phase-20.
- **A real README** with a screenshot, a security statement, a build-from-source section, and an honest feature matrix including what is *not* done. — **[done] except the screenshot.** **[added by audit 2026-10-08]** the build-from-source section says `cargo test`, **which does not compile** — finding 2, phase-19.
- **Reproducible bug reports**: a "copy diagnostics" button that produces a version and capability dump with no file contents. — *phase-26; `px_version()` returns exactly it*
- **Changelog with real entries**, and a public roadmap. — **[done]** `scripts/check-version.sh` gates the version against it, and the "Still missing" section is required by `verify.sh`.
- **Contributing guide** and issue templates, since a security- and privacy-focused project attracts security research. — *not started*
- **Localisation from day one** via ARB files, not string concatenation. — *not started, and now late.* **[added by audit 2026-10-08]** 41 presets, 19 error sentences and a UI that does not exist yet are all strings-in-code. Track B prompts require keeping user-visible strings in one place per screen so extraction stays mechanical; the honest statement is that this item was correctly scoped to "day one" and that day one has passed.

---

## Highest leverage, first

If sequencing matters more than completeness, these four do the most to separate
this from the apps already in the market:

1. **Live progress and honest errors** — removes the single biggest source of distrust in batch tools. — *phase-25*
2. **Before/after comparison view** — the feature users actually judge a resizer on. — *phase-23*
3. **HEIC decode** — expected and missing from every competitor worth comparing against. — *phase-27 gets it into a shipped artefact; the codec itself is done*
4. **Fuzz harness** — turns "we think it's secure" into a continuous build signal. — *phase-27 makes it run at all*
