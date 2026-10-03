# Phase status

The truth table. A row here saying `DONE` is a claim that has been mechanically
checked: whoever wrote it ran `scripts/verify.sh` and saw `VERIFY: PASS` and exit
0. An agent must not write `DONE` on a claim alone.

The authoritative signal is the `workspace/<phase>/.done` marker, which only the
workflow writes, and only after it has run the same script. The table and the
marker are written by different actors on purpose — the table says what a phase
was for and what surprised us, the marker says what the gate measured. If the two
ever disagree, the marker wins and the table is a bug.

| Phase | Title | Status | Commit | Notes |
| --- | --- | --- | --- | --- |
| phase-01 | Verification baseline and project scaffolding | DONE | `4cf3df6` | See [phase-01 notes](#phase-01-notes) below |
| phase-02 | Fuzz harness for every decode path | PENDING | | |
| phase-03 | Hostile-input corpus and property tests | PENDING | | |
| phase-04 | Sandboxed decode worker with a hard memory cap | PENDING | | |
| phase-05 | Dart FFI binding layer and Flutter app skeleton | DONE | `7813d5a` | See [phase-05 notes](#phase-05-notes) below. **The `app/` half is delivered as an unapplied patch — see the notes before trusting this row.** |
| phase-06 | HEIC/HEIF decode | DONE | (this commit) | See [phase-06 notes](#phase-06-notes) below. The `app/` half is a patch, as in phase-05. |
| phase-07 | AVIF encode, progressive JPEG, chroma subsampling | PENDING | | |
| phase-08 | Lossy WebP via libwebp, verified on every target | PENDING | | |
| phase-09 | SIMD resize path behind a feature flag | PENDING | | |
| phase-10 | Benchmarks and a performance regression gate | PENDING | | |
| phase-11 | Low-peak-memory decode for very large images | PENDING | | |
| phase-12 | Colour management: sRGB, Display-P3 and ICC | PENDING | | |
| phase-13 | Animated GIF: honest handling | PENDING | | |
| phase-14 | Content-hash deduplication and a folder pipeline | PENDING | | |
| phase-15 | Supply-chain policy and reproducible builds | PENDING | | |
| phase-16 | Release artefacts: the APK and the EXE | PENDING | | Publishes the installable binaries |
| phase-17 | Self-audit and next-phase generation | PENDING | | Generates the next phase set |

## phase-01 notes

`bash scripts/verify.sh` exits 0 and prints `VERIFY: PASS`. It did not print
either before this phase, and it did not mean what it appeared to mean.

**The gate could not report failure.** `check()` ran `eval "$2"` in the current
shell. Several hygiene checks are written `cmd && exit 1 || exit 0`, and a bare
`exit` inside `eval` terminates the whole script, not just the check. So the
script exited 0 at the *first* hygiene check, skipped the other six, never reached
the summary, and never printed `VERIFY: PASS` — while having already reported
real failures above it. `check()` now runs the eval in a subshell. Nothing was
weakened: the same checks, the same commands, and both directions of the exit
idiom were tested against a deliberately broken tree.

**Two checks that could not fail were made able to fail:**

- `dart format … | tail -n 5` used as an `if` condition takes the exit status of
  `tail`, which is always 0. The output is now captured first and the status
  tested after.
- clippy and rustdoc output is filtered with `grep -E '^(warning|error)'`, which
  cargo's default ANSI colouring defeats, so a failing run reported zero lines
  of reason. Both now run with `--color=never`, and the rustdoc branch re-runs to
  print the offending lines.

**Two real code defects, both pre-existing, both left by the removal of the
`avif` feature:**

- `capabilities()` still evaluated `cfg!(feature = "avif")` for a feature that no
  longer exists. `--all-features` therefore failed clippy and rustdoc. It is now
  a literal `false`, which is the honest answer: `format::encode` rejects AVIF
  with an explicit error, and hard rule 10 says the capability list must not
  promise what the build cannot produce.
- `px_inspect` and `px_exif` documented `# Safety` by linking to the private
  `borrow` fn, which `rustdoc -D warnings` rejects. This was masked by the
  `avif` error above. The contract is now written out in prose, matching what
  `px_process` and `px_batch` already did.

**`rustfmt.toml` and `deny.toml` corrections.** The `rustfmt.toml` comment
claimed "100 rather than the default 100", which is not a sentence; 100 *is*
rustfmt's default and is now documented as pinned on purpose rather than
inherited. `deny.toml` gained `unmaintained = "all"` under `[advisories]`.

Two `deny.toml` claims were checked against real cargo-deny (0.20.2) rather than
assumed, and both were wrong as written:

- `[licenses] deny` no longer exists — cargo-deny removed the key outright
  (EmbarkStudios/cargo-deny#611), and naming a removed key is a hard config
  failure, not a warning. GPL-2.0, GPL-3.0 and AGPL-3.0 are therefore denied by
  omission from the allow list, which is how cargo-deny has always denied
  anything.
- `unmaintained` does not take `"deny"`/`"warn"` in the current advisories
  schema; it takes `"all"`/`"workspace"`/`"transitive"`/`"none"`.

**Left red, deliberately.** `cargo deny check bans` fails on two duplicate crate
versions — `miniz_oxide` 0.8/0.9 and `syn` 2/3 — both pinned by upstream
version requirements inside `image` and its dependency set. Collapsing them needs
a dependency change, which this phase forbids, and adding `skip` entries would
turn the supply-chain job green while making the policy mean less. It is
documented in `deny.toml` and assigned to phase-15. Nothing in
`scripts/verify.sh` runs cargo-deny, so the phase gate is unaffected.

**The duplicate-version claim was checked, not assumed.** `core/Cargo.lock`
really does carry `miniz_oxide` 0.8.9 alongside 0.9.1 and `syn` 2.0.119 alongside
3.0.6, so the "left red, deliberately" note above describes the actual tree
rather than a remembered one.

**Reversed: `workspace/PHASES.md` now has a `Track` column.** An earlier attempt
omitted it, on the reasonable-sounding grounds that there is exactly one track, so
the column would be seventeen identical values and the track is already a section
heading. That argument is about taste and the prompt asked for the column
explicitly, so a reviewer checking the diff against the scope list would have
found the deliverable incomplete. It is now there, carrying `A` on every row, and
the paragraph that justified its absence has been rewritten rather than deleted —
the reasoning was not wrong about the redundancy, only about whose call it is.
The reason it earns its place is phase-17: that phase writes Track B, at which
point these rows stop being the whole list and the column is what lets a reader
compare two phases without first working out which section a row sits in.

**What a later attempt re-checked.** Before accepting the phase as done, the
deliverables were verified against the code rather than against their own claims:
all 14 `px_*` entry points in `ffi.rs` are documented and no others are; each of
the four required gotchas resolves to real code (`EXIF_ID` in `format.rs`,
`Orientation::from_exif`/`rotate90` in `pipeline.rs`, `lossy_webp_enabled`
behind `webp-lossy` in `format.rs`, `DynamicImage::from` at `pipeline.rs:358`);
and the status table has 17 rows for 17 phase directories, matching by `diff`
rather than by eye.

**On the recorded sha.** The row cites `4cf3df6`, the commit carrying the
substantive work, which `git merge-base --is-ancestor` confirms is on `main`. It
is not the tip, and it cannot be: a commit cannot contain its own sha, so
recording it always needs a follow-up commit. Two such follow-ups already exist
(`8a33b6f`, `510267b`) because an earlier recorded sha was invalidated by a
rebase. The convention is therefore "the commit holding the work", and the
bookkeeping commits that follow are expected rather than a sign of drift.

## phase-05 notes

**Read this before trusting the row.** The phase is verified — `scripts/verify.sh`
exits 0 and prints `VERIFY: PASS` — and half of what it covers is *not* in the
tree. `app/` is a git submodule pointing at `authorss81/shrinkray`, and the only
credential available to this pipeline has write access to `authorss81/pixelsmith`
alone:

```console
$ git -C app push --dry-run origin main
remote: Permission to authorss81/shrinkray.git denied to github-actions[bot].
fatal: ... The requested URL returned error: 403
```

`AGENTS.md` is unambiguous about the consequence: a superproject commit pointing at
an unpushed submodule SHA is broken for everyone else. So the submodule stays at
`3ed1eb4` and every `app/`-side change is delivered as
**`workspace/phase-05/shrinkray-phase-05.patch`**, which applies cleanly to that
commit. Three of the phase's five machine-checkable criteria are met *by the
patch* and not by the tree:

| Criterion | In the tree at `3ed1eb4` | Met by |
| --- | --- | --- |
| `flutter analyze` reports zero issues | yes | — |
| `flutter test` passes, offsets and isolate tests included | yes (17 tests) | — |
| Large decode does not block the calling isolate | yes | — |
| 1000 iterations leak no buffers | yes | — |
| Every `px_*` has a matching declaration and appears in the contract test | **no** | the patch |

The last row is the interesting one. `px_inspect` has taken three arguments since
it was written and `bindings.dart` declared two, so a Dart call passed a `bool`
where the engine's second `usize` belongs and the phone limit profile was
unreachable from the app. It survived because the contract test called four of
fifteen entry points. Nine entry points had never been called from Dart at all.
The patch fixes all of it; `bash scripts/check-dart-bindings.sh` reports
`15 entry points match the engine ABI` once it is applied.

**To land it:**

```bash
cd app                       # or a fresh clone of authorss81/shrinkray
git apply ../workspace/phase-05/shrinkray-phase-05.patch
flutter pub get && dart format lib test && flutter analyze
bash native/build-engine.sh  # build the engine first, or the FFI tests skip
flutter test
git commit -m "phase-05: the Dart FFI boundary, verified end to end" && git push
```

Then, in this repository: empty `KNOWN_DART_DRIFT` in `core/src/ffi_abi.rs` and
commit the new submodule pointer. The engine test fails on purpose until that list
is emptied — a test that skipped the comparison instead would stay green forever
and catch nothing.

**What landed in `core/`, which is this repository's half of the boundary:**

- **`ffi_abi.rs` declares every entry point once**, and each row expands to a
  `const _: unsafe extern "C" fn(..) -> ..` assignment. An arity change in
  `ffi.rs` that is not mirrored here is a build failure, not a call with the wrong
  number of arguments.
- **`px_abi_layout()`** publishes `PxBuffer`'s offsets via `offset_of!`, so the
  Dart test compares its struct declaration against the library it loaded rather
  than against a number somebody typed.
- **`px-abi-dump -- --check`** and **`scripts/check-dart-bindings.sh`** report any
  declaration that has drifted.
- **`verify.sh` builds the debug engine, puts it on the loader path, and fails if
  any Flutter test skipped.** It previously certified a boundary it had not
  executed: `DynamicLibrary.open` searches the loader path, not the working
  directory, so a library in `app/src/rust/` was invisible and the FFI tests
  skipped themselves while reporting green.

**Two defects this phase found in this repository, both from the gate rather than
from reading code:**

- `ffi_abi.rs` did not pass `cargo doc -D warnings`: its opening doc line linked
  to `[`ffi`]`, which is out of scope from a sibling module. The same mistake
  `ffi.rs` made in phase-01.
- **`cargo fmt --check` in `verify.sh` could not fail.** `if cargo fmt … | head -n
  40; then` takes the exit status of `head`, which is always 0 — the identical
  defect phase-01 fixed on the `dart format` line one check later. It printed
  "formatting clean" while printing rustfmt's diff, which is how three files were
  committed in a state rustfmt wanted to rewrite. Fixed, and proved in both
  directions: a deliberately misformatted function now makes it exit 1 and print
  the diff.

**Also not done, and named rather than glossed:** the desktop CMake and MSBuild
glue is in the patch but has never produced a bundle, because no CI job runs a
desktop build and this runner has neither GTK headers nor MSVC. Its mechanics
(generator expressions, working directory, install rule) were exercised
separately; `flutter build linux` and `flutter build windows` were not. macOS is
unwired by design — the Apple targets link statically and adding a build phase to
`project.pbxproj` without a Mac would be a guess. Full detail, including the
console output, is in `workspace/phase-05/FINDINGS.md`.

## phase-06 notes

`bash scripts/verify.sh` exits 0 and prints `VERIFY: PASS` with
`--all-features` (124 Rust tests, 20 Flutter tests, none skipped).

**The codec is `heic-rs` 0.1.1, behind an off-by-default `heic` feature.**
`docs/HEIC.md` is the comparison — `libheif-rs`, `heic` (AGPL, rejected on
licence) and `oxideav-heif` against it, with licence, transitive-crate count,
build complexity per target, and whether each can cross-compile to Android and
iOS from one source. Two facts decided it: `heic-rs` adds **zero** crates to the
dependency tree, and `libheif-sys` needs a 5.5 MB vendored C++ build (or a
libheif installed on the machine, which this runner does not have —
`pkg-config --exists libheif` fails). The doc names the one thing libheif wins
at — it reads more real-world files — and says where to reopen the question.

**`heic-decode`, named in the phase prompt, is not a crate on crates.io.** The
pure-Rust crate it stands for is `heic-rs`. Recorded in the doc rather than in a
commit message nobody reads later.

**Detection is not behind the feature; decoding is.** `image` has no HEIF
sniffer, so `detect_format` falls back to `heic::detect`, which parses the `ftyp`
major brand and then the compatible brands. That fallback has no `cfg` on it, so
a build without the codec still answers "this is a HEIC" and then says "this
build cannot open it" — naming the feature — instead of "unknown format", which
is the less useful of two true statements. The same fallback reports `Avif` for
an AV1-coded file in a HEIF container, which `image`'s sniffer misses when the
file is stamped `mif1` with `avif` in its compatible list.

**The synthetic HEIC in the tests is generated, not committed.** A HEIC needs an
HEVC-coded primary item, and there is no HEVC encoder in this tree, so
`core/src/heic.rs`'s `synthetic` module builds the container byte by byte
(`ftyp`, `meta`, `hdlr`, `pitm`, `iinf`, `iloc`, `iprp`, `ipma`, `mdat`) and gets
the bitstream from `heic_rs::hevc::synth`, which writes the parameter sets and
the CABAC slice itself. The consequence is that the pixels going in are known
exactly, so the round-trip test asserts them: a flat DC-predicted field, every
channel equal and near mid-grey. That is a stronger test than a fixture would
give, and there is no third-party photograph in the tree whose licence anybody
would have to reason about. `synth` is behind `heic-rs`'s `bench` feature, pulled
in as a **dev-dependency** so it never reaches a `cargo build --release`.

**The 14 tests cover the phase's four machine-checkable criteria** and one thing
none of them asked for: a HEIC declaring 60000×60000 is refused by `Limits` from
a few hundred bytes of container, on both the desktop and the mobile profile,
while still being *reported* as a 60000×60000 HEIC — so the UI can say "too big
to open" rather than "unsupported". One test found a real over-claim while being
written: truncating a HEIC inside its picture data still passes `validate_bytes`,
because the header read never promised to look at pixels. That is now the
asserted behaviour with the reasoning attached, rather than a weakened test.

**Two structural changes to the engine, both small:**

- `Limits::check_header(w, h)` split out of `check_decoded`. A HEIF states its
  geometry in an `ispe` property box that `image::ImageReader` cannot read, so
  `heic::decode` has to apply the bounds itself before the codec allocates.
  Without it, the codec's own 256 MP ceiling would be the only thing between a
  phone and an OOM on a 60000×60000 `ispe`, because `Limits::mobile()` is 40 MP.
- `OutputFormat::Heic` and `Heif` are new **read-only** variants.
  `is_read_only()` is the flag the UI reads, `encode()` refuses both with a
  message naming JPEG/PNG/WebP instead, and a test asserts the flag and the
  behaviour cannot drift apart. `Capabilities.heic_decode` reports decode support
  separately from `avif_encode`, because one boolean cannot honestly say both.

**Not done, and named rather than glossed:**

- **Colour management.** A Display-P3 iPhone photo decodes with its declared
  matrix and range but the ICC profile is not applied to the output, so it comes
  out washed out until phase-12.
- **HEIC metadata tags.** `exif::read` cannot see a HEIF's `Exif` item — it lives
  in a separate container item, not an APP1 segment — so `heic::header` reports
  `has_exif` from the container and `ValidateReport` stays honest, but the tags
  are not parsed. A HEIC's GPS is therefore never *read*; and because the
  pipeline re-encodes from raw samples and `exif::write_back` only writes what
  `exif::read` returns, it is never carried through either.
- **AV1-in-HEIF decode** (refused by name, not as a broken file) and HEVC
  inter prediction, which a still-picture decoder does not need.
- **`app/` is delivered as `workspace/phase-06/shrinkray-phase-06.patch`**, for
  the same reason as phase-05: only `authorss81/pixelsmith` is writable from this
  pipeline. It adds `heic`/`heif` to the Dart `OutputFormat` enum (whose
  `fromJson` throws on an unknown name, so a format the engine reports and Dart
  lacks is a crash) and `heicDecode` to `Capabilities`. It applies cleanly to
  `3ed1eb4` and was run through `flutter analyze` and `flutter test` (20 passed)
  before being captured.

## Status values

| Value | Meaning |
| --- | --- |
| `PENDING` | not started |
| `IN PROGRESS` | a run is active; look for a `px-wip/<phase>` branch |
| `ATTEMPTED` | work committed, verification failed; check `logs/<phase>.verify.log` |
| `DEFERRED` | rate-limited; retried on the next tick, no action needed |
| `BLOCKED` | needs a human. Delete `.blocked` and `.attempts` after fixing |
| `DONE` | verified: `scripts/verify.sh` printed `VERIFY: PASS`. The `.done` marker is written by the workflow; this cell is written by whoever ran the gate |

## Unblocking a phase

```bash
rm workspace/phase-NN/.blocked workspace/phase-NN/.attempts
git commit -am "unblock phase-NN" && git push
```

The next tick picks it up within about a minute.

## Halting the pipeline

```bash
touch workspace/.stop && git add -A && git commit -m "halt pipeline" && git push
```

Delete `workspace/.stop` to resume. To stop permanently without leaving a
marker in the tree, disable the workflow in the Actions tab instead.