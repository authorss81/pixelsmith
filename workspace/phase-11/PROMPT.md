# phase-11 — Low-peak-memory decode for very large images

**Track:** A — Engine & Security
**Depends on:** phase-10
**Timeout:** 90 minutes

## Objective

Decode a 200 MP panorama on a phone without the app being killed by the OS. Today the whole image is materialised, then a second copy exists during resize, so peak memory is roughly twice the pixel buffer. Fix that.

## Read first

- `AGENTS.md` — hard rule 4. Bounded decode is not negotiable, and "streaming" must not become "unbounded".
- `core/src/validate.rs` — `Limits` and `apply_to_decoder`.
- `docs/BENCHMARKS.md` — the decode baseline from phase-10.

## Scope

### Do

- Measure first: instrument peak RSS or peak allocation for decoding a 24 MP, a 60 MP and a 120 MP image, and for the resize that follows. Put the numbers in `docs/BENCHMARKS.md`. The design should follow the measurement, not the other way round.
- Reduce peak memory to approximately one output-sized buffer plus the strip being decoded. `image`'s decoders expose row-at-a-time reading for several formats; use it.
- Decouple input size from output size: a 120 MP source scaled to 1000 px wide should never allocate a 120 MP buffer. Implement at least a box-average pre-reduction for large ratios, and document why box averaging is the correct pre-filter for decimation rather than a shortcut.
- Keep the fix feature-flagged as `streaming` until benchmarks prove it is not slower on the common case.
- Enforce the memory ceiling from outside where phase-04's sandbox allows it. If the sandbox is not available on a target, `Limits` must be adjusted to a value the target can actually afford — in `Limits::mobile()`, not by weakening the default.
- Add tests: a 120 MP fixture decodes without exceeding the configured ceiling; peak allocation during a 120 MP → 1000 px resize is below a stated number of megabytes; the output of the streaming path is pixel-comparable to the in-memory path within a documented tolerance.

### Do not

- Do not raise `Limits` to make a test pass. If a fixture exceeds the limit, the limit is doing its job.
- Do not remove the header check to save a pass. It is what makes streaming safe.
- Do not claim a memory saving without the measured peak in `docs/BENCHMARKS.md`.

## Acceptance criteria

### Machine-checkable

- `docs/BENCHMARKS.md` contains before and after peak-memory numbers for 24 MP, 60 MP and 120 MP.
- A test asserts peak allocation during a 120 MP → 1000 px resize stays under a stated megabyte figure.
- A test asserts streaming and in-memory decode agree within tolerance.
- `cargo test --features streaming --all-features` passes.

### Needs a human judgement

- Is the stated memory ceiling realistic for a low-end Android phone, or optimistic?
- Does the box-average pre-reduction visibly change output quality on a normal downscale? If so, is it gated correctly?

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha.
- Committed and pushed to `main`.

Do not write `.done`. The workflow does.