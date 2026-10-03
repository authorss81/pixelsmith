# phase-07 — AVIF encode, progressive JPEG, chroma subsampling control

**Track:** A — Engine & Security
**Depends on:** phase-06
**Timeout:** 90 minutes

## Objective

Finish the modern-format story: enable AVIF encoding, add progressive JPEG, and expose chroma subsampling — which is a bigger lever on file size than most users realise, and which no competing resizer exposes.

## Read first

- `AGENTS.md` — hard rule 10. Grey out what the build cannot do rather than failing at export.
- `core/src/format.rs` — `encode`, `is_lossless`, `supports_quality`, `supports_byte_target`.

## Scope

### Do

- Enable the existing `avif` feature and make `OutputFormat::Avif` actually encode. It is currently opt-in and returns `UnknownFormat` when off; make the default build decide deliberately and document the decision.
- Progressive JPEG: add an `EncodingOptions` struct so per-format options stop growing as new arguments. Move `quality` into it, and add `progressive: bool` and `chroma_subsampling: ChromaSubsampling` with `Luma444`, `Luma422`, `Luma420`.
- Make chroma subsampling reachable end to end: `Pipeline` and `Settings` carry it, the JSON request carries it, `px_process` honours it, and the Dart model in `app/lib/rust/models.dart` exposes it with a default of `Luma420`.
- `Luma444` must be the honest default for images with saturated colour edges, and the engine must document the trade-off rather than silently choosing. Note in the docs that 4:2:0 halves chroma resolution and that text on a coloured background will show fringes at 4:2:0.
- Add tests: 4:4:4 output is measurably larger than 4:2:0 at the same quality; progressive output decodes and is larger than baseline at the same quality; a round-trip through every format with every option combination either succeeds or fails cleanly.
- Update `capabilities()` and the preset catalogue so any preset relying on a format the build lacks is adjusted rather than left lying.

### Do not

- Do not silently change the default quality for existing presets.
- Do not make a lossy option available for a format where it has no effect. `is_lossless` and `supports_quality` must stay consistent, and a test must assert it.

## Acceptance criteria

### Machine-checkable

- AVIF encodes and decodes in the default build, or the capability flag says so truthfully and the test covers both branches.
- `cargo test --all-features` passes with a test per chroma subsampling level.
- A test asserts `4:4:4` output is strictly larger than `4:2:0` at identical quality and dimensions.
- A test asserts progressive and baseline JPEG at identical quality and dimensions both decode to the same dimensions.

### Needs a human judgement

- Is the chroma subsampling default right for a photo app? Argue it in `docs/ARCHITECTURE.md`.
- Does the error for "this format has no quality setting" read like a sentence a person wrote?

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha.
- Committed and pushed to `main`.

Do not write `.done`. The workflow does.