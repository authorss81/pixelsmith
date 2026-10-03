# phase-09 — SIMD resize path behind a feature flag

**Track:** A — Engine & Security
**Depends on:** phase-08
**Timeout:** 90 minutes

## Objective

Make resizing fast enough that the UI never waits. The pure-Rust kernel in `image` is the reference implementation and stays as the correctness oracle; add a SIMD path and prove they agree.

## Read first

- `AGENTS.md` — hard rule 5. Exactly one resampling pass per image, always.
- `core/src/pipeline.rs` — `resize_to`, `ResampleFilter`, and the extreme-reduction `Triangle` fallback.

## Scope

### Do

- Add a `simd` feature flag selecting `fast_image_resize` (pure Rust, SIMD) as the resize kernel. Do not pull in `libvips` for this; its build cost and C dependency are not justified for one operation.
- Keep `image`'s kernel as the default and as the reference. The SIMD path is opt-in until benchmarks justify flipping it.
- Write a cross-check test: for every `FilterType`, over a set of representative sizes including extreme downscales and a 1-pixel target, the SIMD output and the reference output agree within a documented per-channel tolerance. Choose the tolerance from measurement, not from wishful thinking, and record the measurement in a comment.
- Pay attention to the existing extreme-reduction fallback. If the SIMD path has its own decimation behaviour, the two must not silently disagree for a 10000→50 downscale. That case needs its own test and its own tolerance.
- Verify no colour-type surprise: the reference works on several buffer types, the SIMD path may only work on RGBA8. If so, normalise to RGBA8 once and say so in a comment rather than leaving a silent per-call conversion.
- Benchmark both paths across a realistic matrix: 24 MP → 1920 wide, 24 MP → 400 wide, 4000 → thumbnail, upscale 800 → 4000. Record the numbers in `docs/BENCHMARKS.md`.

### Do not

- Do not change the default kernel until the benchmarks in `docs/BENCHMARKS.md` show a clear win and the cross-check passes.
- Do not introduce a second resampling pass anywhere. If a filter combination needs two, redesign the kernel instead.
- Do not delete the reference implementation. It is the oracle.

## Acceptance criteria

### Machine-checkable

- `cargo test --features simd --all-features` passes, including the cross-check matrix.
- The cross-check test covers every `FilterType` and at least six size pairs, including 1-pixel and 3:1 extreme reductions.
- `docs/BENCHMARKS.md` contains measured numbers for both kernels on all four realistic cases, with the machine described.

### Needs a human judgement

- Is the measured difference large enough to be worth a feature flag, a second code path, and the maintenance cost forever?
- Are the tolerances defensible to someone who knows nothing about this codebase?

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha.
- Committed and pushed to `main`.

Do not write `.done`. The workflow does.