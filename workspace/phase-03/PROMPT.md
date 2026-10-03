# phase-03 — Hostile-input corpus and property tests

**Track:** A — Engine & Security
**Depends on:** phase-02
**Timeout:** 75 minutes

## Objective

Turn the fuzz harness's findings into permanent regression tests, and add property-based tests for the parts of the engine fuzzing cannot reach: dimension arithmetic, path sanitisation, and the target-bytes search.

## Read first

- `AGENTS.md` — hard rule 2. A failing property test must become a fix plus a named regression test, not a deleted property.
- `core/src/pipeline.rs`, `core/src/worker.rs`, `core/src/target.rs` — the three modules under test.

## Scope

### Do

- Create `core/tests/hostile.rs` with a corpus of malformed inputs, generated programmatically from a valid fixture so it is reproducible rather than a pile of opaque binaries:
  - headers truncated at every offset of a valid file;
  - dimensions of `0` and `u32::MAX`;
  - absurd pixel counts;
  - mismatched magic bytes;
  - a PNG declaring an oversized `IHDR`;
  - a GIF with a broken frame count;
  - EXIF with a truncated IFD;
  - a segment length that would overflow a `u16`.
- Every corpus entry must assert that the call returns `Err` with a **useful** message — assert the specific `error::Error` variant — or returns `Ok` with output inside the configured `Limits`. "Returns an error" is not an assertion.
- Add `proptest` as a dev-dependency and write property tests for:
  - `ResizeSpec::resolve`: output is never zero, never exceeds the `no_upscale` bound, and preserves aspect ratio within one pixel for `FitMode::Width` and `FitMode::Height`.
  - `Pipeline::output_dimensions` agrees exactly with `Pipeline::apply` across random crop/orient/resize combinations.
  - `worker::sanitise_stem` and `sanitise_path_component` never emit `/`, `\`, `..`, a NUL, a reserved Windows device name, or a name longer than the cap — for arbitrary input including Unicode and combining marks.
  - `TargetBytes::encode_with`: given a synthetic encoder whose size is monotonic in quality, the returned quality is the highest that fits, and repeated runs are identical.
- Fix any real bug the property tests find. A property test that reveals a genuine defect is the point of this phase; commit the fix with a regression test named after the defect.
- Give every `proptest!` a fixed seed and at least 256 cases, with a comment explaining why CI determinism is required here.

### Do not

- Do not weaken a property to make it pass. If the property is wrong, say why in a comment and change the property, not the bound.
- Do not add `proptest` to `[dependencies]`. Dev-dependencies only.
- Do not disable a failing case with `#![proptest(ignore)]` without writing a linked issue in the comment.

## Acceptance criteria

### Machine-checkable

- `core/tests/hostile.rs` exists and every corpus entry asserts a specific error variant or a bounded success.
- At least four `proptest!` blocks exist, each with a comment explaining the seed.
- `cargo test --all-features` is deterministic: two consecutive runs produce identical output.
- Every bug the property tests found has a named regression test.

### Needs a human judgement

- Are the hostile inputs actually hostile, or accidentally valid? Check one or two by hand.
- Would a reservation-test author accept the sanitiser properties as written, or are they too weak to fail?

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha.
- Committed and pushed to `main`.

Do not write `.done`. The workflow does.