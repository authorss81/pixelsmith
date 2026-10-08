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

Current scope through phase-17. `core/` is the Rust engine; `app/` does not exist
until phase-05.

Order is execution order, so this table is the single place to look when asking
"what happens next". The Track column carries `A` seventeen times and `B` six
times. It earns its place from phase-17 onward: once there are two tracks, a
reader comparing two phases needs the track visible in the same glance as the
title rather than inferring it from which section the row happens to sit in.

| Phase | Track | Title | Depends on | Timeout | What it does |
| --- | --- | --- | --- | --- | --- |
| [phase-01](phase-01/PROMPT.md) | A | Verification baseline and project scaffolding | — | 60 | Makes `scripts/verify.sh` green and writes the config and docs every later phase depends on. |
| [phase-02](phase-02/PROMPT.md) | A | Fuzz harness for every decode path | 01 | 90 | Puts a fuzz target on every entry point that touches untrusted bytes. |
| [phase-03](phase-03/PROMPT.md) | A | Hostile-input corpus and property tests | 02 | 75 | Turns fuzz findings into permanent regression tests, plus property tests for dimension arithmetic, path sanitisation and the byte-target search. |
| [phase-04](phase-04/PROMPT.md) | A | Sandboxed decode worker with a hard memory cap | 03 | 90 | Moves decode into a subprocess with a real address-space limit. |
| [phase-05](phase-05/PROMPT.md) | A | Dart FFI binding layer and Flutter app skeleton | 04 | 90 | The first user-facing code: a Dart wrapper over `px_*`, and a window that opens. |
| [phase-06](phase-06/PROMPT.md) | A | HEIC/HEIF decode | 05 | 90 | The format every modern phone camera actually writes. |
| [phase-07](phase-07/PROMPT.md) | A | AVIF encode, progressive JPEG, chroma subsampling | 06 | 90 | Closes the honesty gap: the formats we report must be the formats we write. |
| [phase-08](phase-08/PROMPT.md) | A | Lossy WebP via libwebp, verified on every target | 07 | 90 | Turns on `webp-lossy` and proves it builds and behaves on all five targets. |
| [phase-09](phase-09/PROMPT.md) | A | SIMD resize path behind a feature flag | 08 | 90 | A SIMD resize path, with the pure-Rust kernel kept as the correctness oracle and the two proven to agree. |
| [phase-10](phase-10/PROMPT.md) | A | Benchmarks and a performance regression gate | 09 | 75 | Makes "slower" a red build instead of something nobody notices. |
| [phase-11](phase-11/PROMPT.md) | A | Low-peak-memory decode for very large images | 10 | 90 | Decodes a 200 MP panorama in roughly one pixel buffer instead of two, so a phone OS does not kill it. |
| [phase-12](phase-12/PROMPT.md) | A | Colour management: sRGB, Display-P3 and ICC | 11 | 90 | Stops wide-gamut photos from washing out on an sRGB display. |
| [phase-13](phase-13/PROMPT.md) | A | Animated GIF: honest handling | 12 | 75 | Either preserves animation properly or refuses clearly, never silently flattens. |
| [phase-14](phase-14/PROMPT.md) | A | Content-hash deduplication and a folder pipeline | 13 | 75 | Does the whole folder, and skips the copies that are already identical. |
| [phase-15](phase-15/PROMPT.md) | A | Supply-chain policy and reproducible builds | 14 | 90 | Closes the `cargo deny` bans failure and makes a build reproducible byte-for-byte. |
| [phase-16](phase-16/PROMPT.md) | A | **Release artefacts: the APK and the EXE** | 15 | 90 | Publishes installable, checksummed binaries. Delivery phase. |
| [phase-17](phase-17/PROMPT.md) | A | Self-audit and next-phase generation | 16 | 90 | Audits what exists and generates the phase set after this one. |
| [phase-18](phase-18/PROMPT.md) | A | Bound every allocation untrusted bytes can reach | 17 | 90 | Four places where a number from a file reaches an allocation or an arithmetic operation unchecked. One of them aborts the process. |
| [phase-19](phase-19/PROMPT.md) | A | Make `cargo test` work in every feature configuration | 18 | 75 | `cargo test` — the command in the README — does not compile. Fix it, and make the gate check every configuration. |
| [phase-20](phase-20/PROMPT.md) | A | The Dart JSON contract, and a check that makes drift fail the gate | 19 | 90 | The app crashes on an iPhone photo and reports skipped files as successes. Fixes five phases of drift and makes the next one fail the build. |
| [phase-21](phase-21/PROMPT.md) | A | The tests that assert nothing, and the module with none | 20 | 90 | Four tests assert nothing; one of them is hard rule 4's only sentinel. `error.rs` has zero tests. |
| [phase-22](phase-22/PROMPT.md) | B | The design system and the shell | 21 | 90 | Theme, spacing, navigation, empty states. Everything in Track B stands on this. |
| [phase-23](phase-23/PROMPT.md) | B | The preview canvas and the before/after view | 22 | 90 | The screen users judge a resizer on: split view, synced zoom and pan, magnifier, checkerboard. |
| [phase-24](phase-24/PROMPT.md) | B | The pipeline editor | 23 | 90 | Every control the engine exposes, presented so a person who does not know what chroma subsampling is can still choose well. |
| [phase-25](phase-25/PROMPT.md) | B | Live progress, honest cancellation, the batch list | 24 | 90 | `ROADMAP.md`'s highest-leverage item, and the largest block of missing product: all eight items absent or partial. |
| [phase-26](phase-26/PROMPT.md) | B | Export flows and the folder plan | 25 | 90 | Where files go, and what happens when two are the same photograph. Adds the `px_plan` entry point the folder preview needs. |
| [phase-27](phase-27/PROMPT.md) | A | CI that executes what it compiles | 26 | 90 | No job runs a test on any target but Linux x86. Runs the fuzzers, resolves four patch-only CI jobs, fixes the docs that cite them. |
| [phase-28](phase-28/PROMPT.md) | A | `streaming`: make it correct, then decide whether it ships | 27 | 90 | A stride bug no test can see, a 2 GB fallback outside its budget, and the release measurement the default has been waiting on. |

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

## Track B — Flutter UI

**Generated by phase-17.** There is no hand-authored UI track; phase-17 wrote it,
against a real tree rather than against a guess about what the app would be.

The reasoning for generating rather than hand-authoring: a prompt for "implement
the before/after split slider" written before the app exists would describe a UI
nothing had built, and the agent executing it would spend its budget inventing a
shell to put the slider in. phase-17 ran against phase-16's tree, so these phases
reference actual files — `app/lib/rust/models.dart`, `worker::Outcome`,
`folder::plan`, `SkipReason::note()` — and the order is set by what each phase
depends on.

**The `app/` constraint applies to all of Track B.** `app/` is the
`authorss81/shrinkray` submodule and only `authorss81/pixelsmith` is writable
from this pipeline, so every phase from 05 onward delivers its Dart as a patch
under `workspace/<phase>/`, run through `dart format`, `flutter analyze` and
`flutter test` before capture. phase-20 consolidates the accumulated drift into
one patch and supersedes four earlier ones; phases 22 onwards stack on each
other's patches, and each prompt says which base commit it assumes.

## Adding a phase by hand


1. Create `workspace/phase-NN/PROMPT.md`.
2. Optionally add `workspace/phase-NN/.timeout` with a minute budget.
3. Push. The next tick picks it up.

Numbering must continue from the highest existing phase. `sort -V` is what the
selector uses, so `phase-9` after `phase-10` will be selected first.