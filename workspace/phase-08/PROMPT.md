# phase-08 — Lossy WebP via libwebp, verified on every target

**Track:** A — Engine & Security
**Depends on:** phase-07
**Timeout:** 90 minutes

## Objective

Make WebP genuinely useful. Right now the default build can only write lossless WebP, which means no quality control, no byte ceiling, and file sizes that lose to JPEG for most photographs. Link libwebp behind the existing `webp-lossy` feature and prove it builds everywhere we ship.

## Read first

- `AGENTS.md` — hard rule 1. A C dependency is acceptable; a network dependency is not.
- `core/src/format.rs` — `encode_webp` and both `cfg` branches.
- `docs/ARCHITECTURE.md` Gotchas — the WebP entry is there for a reason.

## Scope

### Do

- Enable `webp-lossy` in the default build. Verify the `webp` crate builds and links for: Linux x64, Windows x64 and arm64, macOS x64 and arm64, Android arm64 and x86_64, iOS arm64.
- Add a CI job per target that builds the crate with `--features webp-lossy`. A feature that breaks one target is a broken feature, not a caveat in a README.
- Confirm the packed-RGB path: for opaque images, `from_rgb` must be used rather than `from_rgba`, because handing libwebp a channel it does not need costs 25% before compression even starts. Assert the output is smaller than the RGBA path for an opaque image.
- Add a test that lossy WebP is strictly smaller than lossless WebP at the same dimensions, and that quality 10 is smaller than quality 95.
- Once it is on by default, remove the dead `#[cfg(not(feature = "webp-lossy"))]` branch or keep it only if a target genuinely cannot link. If you keep it, `is_lossless` must still report the truth for that target, and a test covers it.
- Enable the WebP byte ceilings in the preset catalogue that `to_pipeline` currently drops, and re-enable `web-card`, `web-thumb` and `web-hero` ceilings.

### Do not

- Do not vendor libwebp source into the repository. Use the crate's build system.
- Do not claim lossy WebP support on a target whose CI job does not build it.

## Acceptance criteria

### Machine-checkable

- A CI matrix builds `cargo build --release --features webp-lossy` on every listed target.
- `cargo test --all-features` includes a test asserting lossy WebP is smaller than lossless WebP.
- A test asserts the opaque RGB path produces smaller output than the RGBA path.
- Every WebP preset with a byte ceiling in `workspace/PHASES.md` now keeps it.

### Needs a human judgement

- Is the default-build decision right, given the build cost libwebp adds to a cold CI runner?
- Do the docs still describe WebP accurately after this change? Update every place that says "lossless unless the feature is on".

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha.
- Committed and pushed to `main`.

Do not write `.done`. The workflow does.