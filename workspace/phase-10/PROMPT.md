# phase-10 — Benchmarks and a performance regression gate

**Track:** A — Engine & Security
**Depends on:** phase-09
**Timeout:** 75 minutes

## Objective

Put the hot paths behind `criterion` so a performance regression is a red build rather than a bug report from a user on a slow phone.

## Read first

- `AGENTS.md` — hard rule 2. A benchmark must not be weakened to make a build green.
- `core/src/pipeline.rs`, `core/src/format.rs`, `core/src/target.rs` — the hot paths.

## Scope

### Do

- Add `criterion` as a dev-dependency and a `[[bench]]` entry per hot path:
  - decode: JPEG, PNG, WebP at 2 MP, 12 MP and 24 MP;
  - resize: downscale to 1920, downscale to 400, extreme downscale to 64, upscale 2×;
  - encode: JPEG at q85 and q95, WebP lossy at q80, PNG;
  - `TargetBytes::encode_with`: the full binary-search convergence, reported as encodes-per-successful-fit;
  - EXIF read and EXIF strip on a file with a full tag set.
- Build fixtures programmatically in `benches/common/mod.rs`, and make them **compressible**. Noise makes JPEG benchmarks meaningless because the encoder spends all its time on incompressible data. State this in a comment.
- Run each benchmark with `--profile ci`, which uses fewer iterations. Save the baseline as a committed artefact so a comparison is possible later.
- Add `.github/workflows/bench.yml`: nightly, uploads results, and posts a comment on a PR when a benchmark regresses more than 15%. The 15% figure goes in the workflow with a comment explaining why it is not 5%.
- Write `docs/BENCHMARKS.md` explaining what each benchmark measures and, importantly, what it does not. A benchmark that measures the wrong thing is worse than none.

### Do not

- Do not tune a codec or kernel inside this phase. This phase measures; phase-09 decides.
- Do not let a benchmark pass by shrinking its input. If you change a fixture, say why in the commit message.

## Acceptance criteria

### Machine-checkable

- `cargo bench --all-features -- --profile ci` completes and writes results.
- At least seven benchmarks exist, covering every path in the list.
- Every fixture is compressible; a comment in `benches/common/mod.rs` says so.
- `.github/workflows/bench.yml` exists, is scheduled nightly, and fails a PR on a greater-than-15% regression.

### Needs a human judgement

- Do the benchmarks measure the work users actually wait for, or the work that happens to be easy to measure?
- Is `docs/BENCHMARKS.md` honest about the limits of each measurement?

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha.
- Committed and pushed to `main`.

Do not write `.done`. The workflow does.