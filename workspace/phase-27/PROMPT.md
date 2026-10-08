# phase-27 — CI that executes what it compiles

**Track:** A — Engine & Security
**Depends on:** phase-26
**Timeout:** 90 minutes

## Objective

Right now **no CI job runs a test on any target other than Linux x86.**
`engine-matrix` compiles test binaries on five targets with `--no-run`, which is
compile-only. The only executed run in the entire pipeline is `verify.sh` →
`cargo test --all-features` on `ubuntu-latest`.

So "the crate cross-compiles" is verified continuously and "the crate *works* on
Windows or Android" is not verified anywhere. Meanwhile four CI jobs exist only
as patches, and all four still apply cleanly — which means the credential wall is
still there, not that the context is stale.

This phase decides what CI is allowed to claim, and makes the claims true.

## Read first

- `.github/workflows/build.yml` — `engine-matrix` (`:350-389`), the `linux` job
  (`:315-347`, `continue-on-error` at `:320`).
- `docs/AUDIT.md` findings **14**, **15**, **16**, **17**, **18**, **23**.
- The four patches: `workspace/phase-08/build-webp-lossy-matrix.patch`,
  `workspace/phase-10/bench.yml.patch`, `workspace/phase-15/supply-chain-ci.patch`,
  `workspace/phase-16/release-ci.patch`. Run `git apply --check` on each.
- `scripts/deny-check.sh`, `scripts/no-network-report.sh`, `scripts/fuzz.sh`,
  `scripts/bench-compare.py`, `scripts/repro-check.sh` — all written, all callable
  from a job today.

## The wall, and what to do about it

The stated reason these are patches is a credential limit: pushing to
`.github/workflows/` is refused without `workflows` permission, and adding that
permission to `automation.yml` would itself need a workflow push.

**Do not assume the wall is still there — test it.** Try the push. If it is
refused, say so in the commit message with the exact rejection, keep the jobs as
patches, and narrow every claim in `docs/` to what actually runs. If it is
**not** refused — the credential may have changed since phase-08 — apply them and
say that you did, because four patches' worth of claimed-but-absent CI is a
documentation liability that compounds.

Whichever way it goes, **`docs/ARCHITECTURE.md` has to stop citing a patch as
though it were a job.** It currently names
`workspace/phase-08/build-webp-lossy-matrix.patch` as the reason `heic` is off by
default, and that patch is not in the tree. Fix every such citation.

## Scope

### 1. Execute the tests, on more than one target

Pick the honest split and write down the reasoning:

- **Compile on every target** (`--no-run`) — cross-compilation break detection.
  Cheap, and it is what the six `engine-matrix` targets do now.
- **Execute where execution is possible and cheap** — Linux x86 obviously, and
  **Windows**, because `windows-latest` is a real runner and phase-04's sandbox
  code has a documented no-op there (`sandbox.rs:618`). A test suite that never
  runs on Windows cannot tell you that.

Whatever you choose, `--no-run` must never be presented as a test run in a job
name, a summary, or a doc.

### 2. The `linux` job

`continue-on-error: true`, and it **never builds the Rust engine**. The CMake
rule that builds and installs it lives only in
`workspace/phase-05/shrinkray-phase-05.patch`, which **no longer fully applies**
(6 of 9 files; the submodule moved past it). So a Linux bundle with no engine in
it is currently a green job.

Either fix it or **delete it** and say why. A job that cannot fail and reports
green is worse than no job: it is a claim.

### 3. Fuzzing — findings 16

`grep -rn fuzz .github/` returns **empty**. Eleven targets, 99 seeds,
`scripts/fuzz.sh`, and nothing has ever run one.

Add a job that runs each fuzz target for a **bounded** time — long enough to be
a signal, short enough for a pull request. `cargo-fuzz` needs nightly and a
`rustup toolchain install` step, so budget for that.

Then close the coverage gaps the audit found. There is **no target** for
`heic::detect` or `heic::header` (and `fuzz/src/lib.rs:280` returns `b""` for
`Heic | Heif`, so the corpus never presents an `ftyp` header at all), none for
`colour::ColourProfile::read` — which is why `docs/AUDIT.md` finding 1 was found
by reading rather than by fuzzing — and none for the five JSON-taking `px_*`
entry points or `worker::process_one`.

One stale comment to fix: `fuzz_targets/decode_gif.rs:8` says the engine
"iterates `into_frames()`", which phase-13 replaced with the non-decoding
`scan_gif_frames`.

### 4. Features that only build on one machine — findings 14

`streaming`, `simd` and `heic` are compiled by `--all-features` on `ubuntu-latest`
and **nowhere else**. `heic` decode has never been cross-compiled and has never
been in a shipped artefact.

At minimum, compile `heic` for the targets `engine-matrix` already covers. At
minimum, run the suite with `heic` **off** — `heic::detect` and `heic::header` are
always compiled and currently have **zero tests in any default build** because
`heic.rs`'s whole test module is `#[cfg(all(test, feature = "heic"))]`.

Also note: `fuzz/Cargo.toml:29` depends on `pixelsmith_core` with **default
features only**, so no fuzz target can ever reach `stream`, `resize::simd` or
`heic::decode`.

### 5. The release workflow — finding 18

There is no `release.yml` in `.github/workflows/`. There are only
`automation.yml`, `build.yml` and `supply-chain.yml`. And
`scripts/RELEASE-SHA256.txt` carries two SHA-256 rows with real hashes and sizes
— an APK at 58,109,122 bytes and an AAB at 57,394,493 — while
`workspace/phase-16/.done` exists.

**I could not determine from the tree how those artefacts were produced.** That is
recorded in `docs/AUDIT.md` under "Could not verify" and it should be resolved
one way or the other: either the workflow exists somewhere and the hashes are
real, or `RELEASE-SHA256.txt` is documenting artefacts whose provenance nobody
can reconstruct. Do not leave it ambiguous.

### Do not

- Do not add a job that cannot fail. Every job this phase adds must be
  demonstrably red on a broken tree — the rule `verify.sh` already states about
  its own checks, and there are five recorded instances in this repository of a
  check that could not report the truth.
- Do not make CI green by relaxing anything. If a job is red because the code is
  wrong, say so and leave it red.
- Do not turn the benchmark gate on with an unmeasured baseline.
  `scripts/bench-compare.py` deliberately gives **no verdict** when the CPU model
  differs from `core/benches/baseline/machine.json`, and that behaviour is
  correct. Do not make it fail on a machine mismatch.
- Do not claim a target builds or a test runs without saying which job did it.
- Do not add a network capability to the *engine*. CI jobs may install tools
  from the network; the crate may not depend on one.

## Acceptance criteria

### Machine-checkable

- Every job added or changed by this phase is run once with a **passing** input
  and once with a **deliberately broken** one, and both results are pasted into
  the commit message.
- The workflow file no longer contains any `--no-run` in a job whose name or
  summary claims a test ran — or each such job says "compile only" in its name.
- `grep -rn fuzz .github/` returns something.
- `cargo test --no-default-features` runs in CI, and it compiles (phase-19
  established this).
- Every citation of a `.patch` file in `docs/*.md` is either (a) a patch, marked
  as one, or (b) replaced by the job it adds. `grep -rn '\.patch' docs/` and
  check each hit by hand.
- `scripts/RELEASE-SHA256.txt` either has a workflow that produces it, or says in
  its own header that it records artefacts from a process this repository cannot
  reproduce.
- `bash scripts/verify.sh` exits 0.
- **No engine source change is required by this phase**, so the APK and EXE
  builds are unaffected. Say so explicitly. If you do change `core/Cargo.toml`
  (for example to widen a feature), say which builds that affects and why.

### Needs a human judgement

- Read every job name and summary line. Would a reader who has never opened the
  repository know, from the job name alone, what ran?
- Is the fuzz budget long enough to be a signal and short enough to be run on
  every pull request? If not, run it on a schedule instead and say which.
- Which claims in `docs/` are now stale because CI does less than the docs say?
  Fix them in this phase rather than leaving them for the next reader.

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha.
- Committed and pushed to `main`. Never force-push.
- Do not write `.done`. The workflow does.
