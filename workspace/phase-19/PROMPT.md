# phase-19 — Make `cargo test` mean something in every feature configuration

**Track:** A — Engine & Security
**Depends on:** phase-18
**Timeout:** 75 minutes

## Objective

**`cargo test` — the command `README.md:91` tells a contributor to run — does not
compile.** Fix that, and then make the gate check every feature configuration
rather than one, so it can never happen again silently.

This is a small phase and it is first because it gates the honesty of everything
else. `scripts/verify.sh` runs `--all-features`, which hides both defects. That
is not a cosmetic gap: it means **`--no-default-features` has never compiled on
this tree**, so phase-07's claim that the AVIF refusal arm "was additionally run
under `--no-default-features`" describes a run that could not have happened.

## Read first

- `docs/AUDIT.md` finding **2**, which reproduces both errors.
- `core/Cargo.toml:97` — `default = ["avif", "webp-lossy"]` and the five features.
- `scripts/verify.sh` section 2 — where the new checks go.
- `docs/ARCHITECTURE.md`, "Feature flags". It is the file that explains what each
  flag is for; if this phase changes what a flag covers, update it there.

## Scope

### 1. Two compile failures

```
$ cargo check --manifest-path core/Cargo.toml --tests
error[E0433]: cannot find `stream` in `pixelsmith_core`   (tests/streaming_peak.rs:187, :190)

$ cargo test --manifest-path core/Cargo.toml --no-run
error[E0601]: `main` function not found in crate `resize_bench`
```

- **`core/tests/streaming_peak.rs`** calls `pixelsmith_core::stream::decode_resized`
  with no feature gate, and `stream` is `#[cfg(feature = "streaming")]`
  (`lib.rs:24-25`). Gate the test binary the way the module is gated — a
  `#![cfg(feature = "streaming")]` at the top of the file is the honest form,
  because a test file that compiles to nothing should say so rather than be
  absent.
- **`core/examples/resize_bench.rs:29`** is `#![cfg(feature = "simd")]`, so with
  `simd` off the crate has no `main`. The same treatment, or `required-features`
  in `core/Cargo.toml` so cargo skips it cleanly. Pick one and say why.

Then prove it in every configuration. At minimum:

```
cargo test                       # default:            avif + webp-lossy
cargo test --no-default-features # nothing optional
cargo test --features streaming  # the memory path
cargo test --features simd       # the SIMD kernel
cargo test --all-features        # what the gate runs today
```

### 2. The gate checks all of them

Add a feature-matrix section to `scripts/verify.sh` that compiles the test
targets in every combination that matters, including `--no-default-features` and
each single-feature build. Use `cargo check --all-targets --tests` or
`cargo build --tests` rather than a full test run: the gate already spends two
minutes on `cargo test`, and **compiling** is what catches this class.

Follow the rule `verify.sh` already states for every check it gains:

> every check a phase adds into this file gets run with a *passing* input before
> it is believed

There are five recorded instances in this repository of a check that could not
report the truth (phase-01's `check()`, phase-08's `| head -n 40`, phase-10's
`--test`, phase-15's target-triple regex and its `spdx_licenses` typo), and two
of those cost a phase whose actual work was already complete. Run each new check
in both directions and say so in the commit message.

### 3. `core/examples/resize_bench.rs` deserves a decision, not just a gate

It is the SIMD benchmark and `docs/BENCHMARKS.md` quotes its numbers. With
`simd` off it compiles to nothing. Once it builds in both configurations, check
that the numbers the docs quote are still what the example prints. If they are
not, `docs/BENCHMARKS.md` is wrong and fixing the doc is part of this phase.

### Do not

- Do not make `streaming` or `simd` default-on. Both are off for stated,
  documented reasons — an unmeasured ARM speedup for `simd`, an unrun wall-clock
  comparison for `streaming` — and turning either on is a decision this phase
  does not have the hardware to make.
- Do not delete a test to make a configuration compile. `streaming_peak.rs` is
  the phase-11 acceptance criterion; gate it, do not weaken it.
- Do not delete or skip a feature combination that fails. Fix it or say in
  `docs/phase-status.md` which one does not build and why.
- Do not add a network call to fetch a toolchain. If `rust-toolchain.toml` seems
  like the answer, note it and leave it — that is a policy decision.

## Acceptance criteria

### Machine-checkable

- Every command in the five-line list above compiles. Paste the outputs.
- `bash scripts/verify.sh` exits 0, and its new feature-matrix section reports
  **at least four configurations checked**.
- Each new check in `verify.sh` is demonstrated to fail on a deliberately broken
  tree, exactly as the existing ones are. Paste the failing run.
- `grep -n 'cargo test' README.md` shows a command that works. If the README's
  build section needs the feature flags spelled out, rewrite it.
- `docs/BENCHMARKS.md`'s resize numbers match what `resize_bench` prints, or the
  doc is corrected and the correction is stated.
- **No `px_*` signature, JSON field or public Rust type changes in this phase**,
  so `flutter build apk --release` and `flutter build windows --release` are
  unaffected. Say that explicitly in the commit message; do not skip the
  statement.

### Needs a human judgement

- Is `--all-features` in `verify.sh` still the right default for the *test* run,
  or should the gate run the default set and reserve `--all-features` for the
  matrix? Say which you chose.
- Are there feature combinations a reader would expect to work that are not in
  your matrix?

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha.
- Committed and pushed to `main`. Never force-push.
- Do not write `.done`. The workflow does.
