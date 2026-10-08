# Roadmap

Everything we want this project to become. Ordered roughly by leverage, not by
difficulty. Items marked **[done]** are already implemented in
`core/` and covered by tests.

Principle: the app never uploads a photo, never phones home, and never lies to
the user about what it did to their file.

---

## Visual / UI

- **Adaptive light/dark/system theme** with hand-tuned contrast in both, not just inverted greys. WCAG AA on all body text and controls.
- **Single accent colour**, one surface elevation model, consistent 4pt spacing grid. No gradients-for-decoration.
- **Before/after slider** as the centrepiece: draggable split view with synced zoom and pan, plus a magnifier loupe at the cursor for pixel-peeping edges.
- **Zoomable checkerboard** for transparency (not grey squares), with zoom %, fit-to-window, 1:1, and actual-size buttons.
- **Format/size result card** shown live while dragging any slider: dimensions, estimated bytes, and a savings bar.
- **Non-destructive live preview** using a downscaled proxy while dragging, full-resolution only on release, so the UI never stutters.
- **Skeleton + shimmer** for loading a photo; **empty states** that teach instead of saying "no files".
- **Preset chips grouped by category** (Social / Web / Print / Device / Email / Dev) with icons, searchable, recently-used pinned.
- **Uniform control styling** across Windows (Fluent-ish density) and mobile (Material-ish 48pt touch targets) — same layout grammar, platform-appropriate metrics.
- **Motion**: 120–180ms ease-out for panels, no bounce, respects `prefers-reduced-motion`.
- **Custom theming hook** so the accent colour can be overridden; respects the OS high-contrast mode.

## User-friendliness

- **Paste from clipboard** and **drag-and-drop anywhere** on the window, plus a desktop "open with" shell association.
- **Zero-config default**: pick a photo, get a sensible resized output. Advanced controls stay collapsed.
- **Explain every number**: "1920 × 1080 (from 4000 × 3000, fit inside)" and why `no_upscale` clamped it.
- **Human errors**: never "Error: decode failed". Say "This file isn't an image we can read, or it's damaged."
- **Warn before destructive things**: dimension collapse below the source, quality floor reached, byte target impossible.
- **Undo/redo across the whole pipeline**, including preset changes and batch runs.
- **Remember settings per device** (mobile) and persist last-used preset on desktop.
- **Onboarding that respects competence**: first run does a real resize, not a tutorial carousel.
- **Full keyboard access** on desktop, including a command palette for common actions.
- **Searchable settings**; no more than two levels deep in any menu.
- **Sensible filename handling**: increment rather than overwrite (`photo-2.jpg`), configurable template.
- **Explicit progress and cancel** on anything over ~200ms, so nobody wonders if it hung.

## Live progress

- **Determinate progress bar** with real `done / total`, not an indeterminate spinner.
- **Per-file rows in the batch list** with individual state: queued → decoding → resizing → encoding → done / failed.
- **Throughput readout**: images/second, ETA, elapsed, and peak memory.
- **Live savings counter** during the run: "12.4 MB saved so far."
- **Cancellation that is instant and honest** — no half-written output files, and the UI says how many finished before stopping.
- **Background processing** on mobile with a persistent notification (Android) carrying progress and a cancel action.
- **Streaming preview** of the first result while the rest of the batch finishes.
- **Error log per file** with the reason, expandable to the underlying decoder message.
- **Toast + inline status** for every action, so nothing completes silently.

## More features

- **Target-size mode** (already built): "under 250 KB" via binary-searched quality.
- **Batch + ZIP export**, plus "save to folder" and "save next to original".
- **Format conversion** JPEG/PNG/WebP/GIF/TIFF/BMP/ICO with per-format capability awareness.
- **Auto-orient from EXIF**, with a manual override and live rotate/flip.
- **Crop with aspect lock**, free-form or locked to the target ratio, draggable handles, 3×3 grid.
- **Metadata inspector** with a clear "this file carries GPS" banner and strip-by-default.
- **Watermark** text or image, 9-grid position, opacity, margin.
- **Offline AI background removal** (ONNX, ~42 MB, downloaded once).
- **Offline AI upscale** (Real-ESRGAN / Swin2SR) with CPU and GPU paths.
- **Smart compression suggestions**: estimate whether WebP beats JPEG at the same visual quality, and recommend the smaller one.
- **Deduplicate identical images** by content hash before processing a folder.
- **Animated GIF handling** that tells you it will be flattened instead of quietly destroying frames.
- **HEIC/HEIF decode** (iPhone photos) — a real gap in every competitor.
- **AVIF encode** behind the existing optional feature.
- **Print sizing**: dimensions in cm/inches at a stated DPI, with a paper-size picker.
- **RAW** (CR2/CR3/NEF/ARW/DNG) if the licence situation works out.
- **Developer presets**: full Android density set, iOS app icon, store screenshot sizes.
- **[done]** **Colour conversion**: sRGB and Display-P3, with the ICC profile read
  and reported. Explicit gamma handling is *not* done — the engine uses the sRGB
  transfer function for every profile, which `docs/ARCHITECTURE.md` states rather
  than hides.
- **Progressive JPEG** toggle for web delivery.
- **Chroma subsampling control** for JPEG, which is often a bigger lever than quality.

## Security and privacy

- **[done]** **Zero network calls at runtime** — the engine has no HTTP/TLS dependency at all, so it *cannot* phone home. Assert this in CI with a dependency audit.
- **Reproducible builds**: locked deps, `--locked` in CI, published checksums.
- **Dependency audit in CI** (`cargo deny` / `cargo audit`) gating the build.
- **Fuzzing**: `cargo-fuzz` targets for every decoder path — this is the real answer to "is it secure," since image parsers are the attack surface.
- **Sandboxed decode** in a separate process with a hard memory cap, so a hostile file cannot take down the UI.
- **[done]** **Decompression-bomb limits** enforced from the header, before any pixel buffer is allocated.
- **[done]** **Magic-byte format detection**, never the filename.
- **[done]** **Path traversal sanitisation** on every output name, including ZIP entries.
- **Extension whitelist** on outputs; no surprises like `.exe.jpg`.
- **Optional binary transparency**: an in-app check that hashes the shipped library, plus a CI step that greps the source for network APIs.
- **No telemetry, ever** — stated in the README, enforced by the absence of any HTTP client in the dependency tree.
- **[done]** **Metadata stripped by re-encoding**, not by clearing tags, so maker notes and thumbnails cannot survive.
- **[done]** **GPS and identifying tags never written back**, even when metadata preservation is explicitly requested.

## Performance

- **SIMD resize path** (`fast_image_resize` or `libvips`) behind a feature flag, benchmarked against the current pure-Rust path.
- **Thread-pool sized to the device**, not the core count, so phones don't thermal-throttle.
- **Streaming decode** for very large images so peak memory stays near `width × output_height`, not the source size.
- **Early decode downscale** for previews: decode at 1/4 when the source is over 40 MP.
- **Startup budget**: no work in `main`; the first frame renders in under 100 ms.
- **Benchmarks in CI** (`criterion`) so a regression is a red build, not a bug report.
- **[done]** **Parallel batch processing** with deterministic output, verified equal to a sequential run.
- **[done]** **Exactly one resampling pass** per image, with crop → orient → resize order fixed and tested.

## Platform integration

- **Windows**: Explorer context-menu "Resize with…", jump-list recent presets, tray icon for batch mode, `SendTo` target, proper DPI scaling on mixed-DPI monitors.
- **Android**: Photo Picker (no storage permission needed), MediaStore save, share sheet, WorkManager for large batches, dark theme.
- **iOS**: PHPicker, Photos framework save, share extension, no network entitlement declared at all.
- **CLI** alongside the GUI for scripting, with the same engine.
- **Deep links / URL scheme** so other apps can hand off images.
- **Portable zip for Windows** so it runs without an installer.

## Trust and polish

- **[done]** **Honest capability reporting** — grey out formats the build cannot write rather than failing at export.
- **A real README** with a screenshot, a security statement, a build-from-source section, and an honest feature matrix including what is *not* done.
- **Reproducible bug reports**: a "copy diagnostics" button that produces a version and capability dump with no file contents.
- **Changelog with real entries**, and a public roadmap.
- **Contributing guide** and issue templates, since a security- and privacy-focused project attracts security research.
- **Localisation from day one** via ARB files, not string concatenation.

---

## Highest leverage, first

If sequencing matters more than completeness, these four do the most to separate
this from the apps already in the market:

1. **Live progress and honest errors** — removes the single biggest source of distrust in batch tools.
2. **Before/after comparison view** — the feature users actually judge a resizer on.
3. **HEIC decode** — expected and missing from every competitor worth comparing against.
4. **Fuzz harness** — turns "we think it's secure" into a continuous build signal.