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
| phase-06 | HEIC/HEIF decode | PENDING | | |
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