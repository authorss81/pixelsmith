# phase-02 — Fuzz harness for every decode path

**Track:** A — Engine & Security
**Depends on:** phase-01
**Timeout:** 90 minutes

## Objective

Build a `cargo-fuzz` harness with a target for every untrusted-byte entry point in the engine. The product's security claim is only worth what a fuzzer can enforce, so this turns "we think the parser is safe" into a continuous build signal.

## Read first

- `AGENTS.md` — hard rules 3 and 4 cover panics on untrusted input and bounded decode. Both are what this phase is testing.
- `docs/ARCHITECTURE.md` — the module map and the `Limits` enforcement points.
- `core/src/validate.rs` and `core/src/ffi.rs` — the two places untrusted bytes enter.

## Scope

### Do

- Create `fuzz/` as a workspace member with `cargo-fuzz`. It is a dev tool and must not be a dependency of `core`.
- One fuzz target per entry point that touches attacker-controlled bytes:
  - `fuzz_targets/decode_bounded.rs`
  - `fuzz_targets/validate_bytes.rs`
  - `fuzz_targets/exif_read.rs`
  - `fuzz_targets/detect_format.rs`
  - one target per decoder we claim to support: JPEG, PNG, WebP, GIF, TIFF, BMP, ICO
- Every target must assert the invariants that actually matter, not just that the call returned:
  - no panic, no unwind, no abort — the harness treats a panic as a crash and fails;
  - memory stays bounded, checked with an allocation counter — a target that exceeds the configured `Limits` must fail rather than pass slowly;
  - `validate_bytes` and `decode_bounded` agree: if validation passes, decode must not fail with a resource error.
- Add `fuzz/README.md` explaining how to run each target, how long to run it, what a finding means, and how to minimise a crash before reporting it.
- Wire `cargo fuzz run --fuzz-dir fuzz` behind an opt-in `PX_FUZZ=1` flag in `scripts/verify.sh`, so CI can run a short fuzz session without making the default gate slow.
- Add `.github/workflows/fuzz.yml`: a nightly 10-minute session per target, opening an issue on crash. It must not run on pull requests from forks.
- Seed each corpus with the outputs of the existing unit-test fixtures plus a handful of deliberately malformed headers written by hand.

### Do not

- Do not add a runtime dependency to `core`. The harness uses `libfuzzer-sys` and `arbitrary`, both dev-only.
- Do not commit a crash artefact without first reducing it to a minimal reproducer in a unit test.
- Do not mark a target complete because it "did not crash in ten minutes". Say so explicitly in `fuzz/README.md`.

## Acceptance criteria

### Machine-checkable

- `PX_FUZZ=1 bash scripts/verify.sh` runs the fuzz targets and exits 0.
- `cargo fuzz list` shows a target for every entry point listed above.
- Each target contains at least one assertion beyond "did not panic".
- `fuzz/README.md` documents every target.
- `grep -i fuzz core/Cargo.toml` returns nothing.

### Needs a human judgement

- Do the targets actually reach the decoder, or do they bail out early on the first `validate_bytes` call and never fuzz anything? Trace it.
- Is the memory bound measured, or asserted from the input length? Measuring is the point.

## Definition of done

- `bash scripts/verify.sh` exits 0.
- `PX_FUZZ=1 bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha.
- Committed and pushed to `main`.

Do not write `.done`. The workflow does.