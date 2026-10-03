# Phase table

Every phase is a directory under `workspace/` containing a `PROMPT.md`. The
workflow selects the lowest-numbered phase that has no `.done` and no `.blocked`
marker, so **order in this table is execution order**. A phase may depend only on
phases above it.

## How the state machine works

Markers live in `workspace/<phase>/`:

| Marker | Meaning | Written by |
| --- | --- | --- |
| `.attempted` | the run produced real work, awaiting verification | `scripts/phase_runner.sh` |
| `.done` | verification passed; the phase is complete | `.github/workflows/automation.yml` only |
| `.deferred` | hit a rate limit, retry on a later tick | `scripts/phase_runner.sh` |
| `.blocked` | needs a human | `scripts/phase_runner.sh` or the verify job |
| `.attempts` | real-failure counter, cap 3 | `scripts/phase_runner.sh` |
| `.deferred_attempts` | rate-limit counter, cap 5 | `scripts/phase_runner.sh` |
| `.verify_failures` | verification-failure counter, cap 3 | verify job |
| `.timeout` | job budget in minutes, default 60 | committed by hand |
| `.checkpoint` | partial work merged from a WIP branch | `scripts/phase_runner.sh` |

Two global switches:

| Path | Effect |
| --- | --- |
| `workspace/.stop` | halts the pipeline entirely; delete to resume |
| `workspace/.runs` | run counter; the pipeline stops itself at 250 |

The phase agent never writes `.done`. `scripts/verify.sh` decides, and the
workflow commits the marker. That separation is deliberate: a phase that compiles,
passes its tests and has not broken a hard rule is verified, and one that does
not gets another attempt.

## Track A — Engine and security

Current scope. `core/` is the Rust engine; `app/` does not exist until phase-05.

| Phase | Title | Depends on | Timeout |
| --- | --- | --- | --- |
| [phase-01](phase-01/PROMPT.md) | Verification baseline and project scaffolding | — | 60 |
| [phase-02](phase-02/PROMPT.md) | Fuzz harness for every decode path | 01 | 90 |
| [phase-03](phase-03/PROMPT.md) | Hostile-input corpus and property tests | 02 | 75 |
| [phase-04](phase-04/PROMPT.md) | Sandboxed decode worker with a hard memory cap | 03 | 90 |
| [phase-05](phase-05/PROMPT.md) | Dart FFI binding layer and Flutter app skeleton | 04 | 90 |
| [phase-06](phase-06/PROMPT.md) | HEIC/HEIF decode | 05 | 90 |
| [phase-07](phase-07/PROMPT.md) | AVIF encode, progressive JPEG, chroma subsampling | 06 | 90 |
| [phase-08](phase-08/PROMPT.md) | Lossy WebP via libwebp, verified on every target | 07 | 90 |
| [phase-09](phase-09/PROMPT.md) | SIMD resize path behind a feature flag | 08 | 90 |
| [phase-10](phase-10/PROMPT.md) | Benchmarks and a performance regression gate | 09 | 75 |
| [phase-11](phase-11/PROMPT.md) | Low-peak-memory decode for very large images | 10 | 90 |
| [phase-12](phase-12/PROMPT.md) | Colour management: sRGB, Display-P3 and ICC | 11 | 90 |
| [phase-13](phase-13/PROMPT.md) | Animated GIF: honest handling | 12 | 75 |
| [phase-14](phase-14/PROMPT.md) | Content-hash deduplication and a folder pipeline | 13 | 75 |
| [phase-15](phase-15/PROMPT.md) | Supply-chain policy and reproducible builds | 14 | 90 |
| [phase-16](phase-16/PROMPT.md) | **Release artefacts: the APK and the EXE** | 15 | 90 |
| [phase-17](phase-17/PROMPT.md) | Self-audit and next-phase generation | 16 | 90 |

### phase-16 is the delivery phase

Everything before it builds a library and a test suite. phase-16 turns that into
two things a person can install:

- an **Android APK** (plus an `.aab`, because Play Store distribution requires
  one and an APK alone cannot be published) — signed, with declared ABIs, and
  verified to contain `lib/*/libpixelsmith_core.so` rather than being an empty
  shell that crashes on first tap;
- a **Windows EXE** as a portable ZIP — the whole `Release/` directory, because
  an EXE without its DLLs does not start — verified to carry
  `pixelsmith_core.dll` next to the executable.

It publishes a tagged GitHub Release with SHA-256 checksums and the SBOM, and
writes a `CHANGELOG.md` whose "what is still missing" section is the important
one.

`.github/workflows/build.yml` already produces both artefacts on every push, so
this phase makes them release-grade rather than reinventing them. It also means
the build is checked continuously from phase-05 onward: a refactor that breaks
the binary shows up as a red build long before anyone installs it.

Signing secrets belong to the repository owner. phase-16 must not fail when they
are absent — it produces a debug-signed release, attaches it, and opens an issue
saying exactly which secret to add.

## Track B and beyond — generated, not written

There is no hand-authored UI track. phase-17 writes it.

The reasoning: a prompt for "implement the before/after split slider" written
before the app exists would describe a UI nothing had built, and the agent
executing it would spend its budget inventing a shell to put the slider in. Phase-16
runs against a real tree, so it can write phases that reference actual files, and
it can notice the things no roadmap predicted — which is the entire point of an
audit.

## Adding a phase by hand

1. Create `workspace/phase-NN/PROMPT.md`.
2. Optionally add `workspace/phase-NN/.timeout` with a minute budget.
3. Push. The next tick picks it up.

Numbering must continue from the highest existing phase. `sort -V` is what the
selector uses, so `phase-9` after `phase-10` will be selected first.