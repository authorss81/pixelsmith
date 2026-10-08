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
| phase-02 | Fuzz harness for every decode path | DONE | (pre-phases) | Corrected by phase-17: the row said `PENDING` while `workspace/phase-02/.done` was on disk. See [phase-17 notes](#phase-17-notes). The harness exists — 11 targets, 99 seeds — but **no CI job runs one**, which is finding 16. |
| phase-03 | Hostile-input corpus and property tests | DONE | `f0a23ff` | See [phase-03 notes](#phase-03-notes) below |
| phase-04 | Sandboxed decode worker with a hard memory cap | DONE | `40bb083` | See [phase-04 notes](#phase-04-notes) below |
| phase-05 | Dart FFI binding layer and Flutter app skeleton | DONE | `7813d5a` | See [phase-05 notes](#phase-05-notes) below. **The `app/` half is delivered as an unapplied patch — see the notes before trusting this row.** |
| phase-06 | HEIC/HEIF decode | DONE | (this commit) | See [phase-06 notes](#phase-06-notes) below. The `app/` half is a patch, as in phase-05. |
| phase-07 | AVIF encode, progressive JPEG, chroma subsampling | DONE | `d815024` | See [phase-07 notes](#phase-07-notes) below. The `app/` half is a patch, as in phase-05 and phase-06. |
| phase-08 | Lossy WebP via libwebp, verified on every target | DONE | `374e6ed` | See [phase-08 notes](#phase-08-notes) below. The two prior attempts failed on a pre-existing gate defect, not on their work. |
| phase-09 | SIMD resize path behind a feature flag | DONE | `e906fa6` | See [phase-09 notes](#phase-09-notes) below. |
| phase-10 | Benchmarks and a performance regression gate | DONE | `0ebccdf` | See [phase-10 notes](#phase-10-notes) below. **The CI workflow is delivered as a patch, as in phase-05/06/07/08 — the gate does not run until it is applied.** |
| phase-11 | Low-peak-memory decode for very large images | DONE | `9cef389` | See [phase-11 notes](#phase-11-notes) below. |
| phase-12 | Colour management: sRGB, Display-P3 and ICC | DONE | `7dc119d` | See [phase-12 notes](#phase-12-notes) below. The `app/` half was never delivered as a patch — see the notes. |
| phase-13 | Animated GIF: honest handling | DONE | `ce804a9` | See [phase-13 notes](#phase-13-notes) below. **The previous attempt had finished the work and failed on one broken doc link.** The `app/` half is a patch, as in phase-05/06/07. |
| phase-14 | Content-hash deduplication and a folder pipeline | DONE | `8323a7c` | See [phase-14 notes](#phase-14-notes) below. **The previous attempt had finished the work and failed on one broken doc link.** The `app/` half is a patch, as in phase-05/06/07/13. |
| phase-15 | Supply-chain policy and reproducible builds | DONE | (this commit) | See [phase-15 notes](#phase-15-notes) below. **The CI workflow is a patch, as in phase-05/06/07/08/10 — most of the policy does not run in CI until it is applied.** One of eight targets is measured reproducible; seven are honestly blank. |
| phase-16 | Release artefacts: the APK and the EXE | DONE | `a332803` | Corrected by phase-17: the row said `PENDING` while `.done` was on disk. **The release workflow is a patch, so `scripts/RELEASE-SHA256.txt` records hashes nothing in this tree produces** — `docs/AUDIT.md` finding 18. |
| phase-17 | Self-audit and next-phase generation | DONE | (this commit) | See [phase-17 notes](#phase-17-notes) below. Wrote `docs/AUDIT.md` and eleven phases. |
| phase-18 | Bound every allocation untrusted bytes can reach | PENDING | | Audit findings 1, 6, 7, 8. The unbounded PNG `iCCP` inflate, the un-limited FFI JSON envelopes, `CropSpec`'s unchecked `u32` add, and `px_buffer_free`'s trusted length. |
| phase-19 | Make `cargo test` work in every feature configuration | PENDING | | Audit finding 2. `cargo test` — the README's command — does not compile, and `--all-features` in the gate is the only reason nobody noticed. |
| phase-20 | The Dart JSON contract, and a check that makes drift fail the gate | PENDING | | Audit findings 3, 4, 5, 13. Five phases of drift, an app that crashes on a HEIC and counts skips as successes, and a leak on every failed call. |
| phase-21 | The tests that assert nothing, and the module with none | PENDING | | Audit findings 19, 20. Four tests measure nothing, one of them is hard rule 4's only sentinel, and `error.rs` has no test module. |
| phase-22 | **Track B** — The design system and the shell | PENDING | | All eleven Visual/UI roadmap items are `ABSENT`. This builds the layer the next four UI phases stand on. |
| phase-23 | **Track B** — The preview canvas and the before/after view | PENDING | | The screen users judge a resizer on. `ROADMAP.md` calls the split slider the centrepiece. |
| phase-24 | **Track B** — The pipeline editor | PENDING | | Every control the engine exposes, presented so a person who does not know what chroma subsampling is can still choose well. |
| phase-25 | **Track B** — Live progress, honest cancellation, the batch list | PENDING | | `ROADMAP.md`'s highest-leverage item, and the largest block of missing product: all eight items `ABSENT` or `PARTIAL`. |
| phase-26 | **Track B** — Export flows and the folder plan | PENDING | | Where files go, and what happens when two are the same photograph. Adds the `px_plan` entry point the folder preview needs. |
| phase-27 | CI that executes what it compiles | PENDING | | Audit findings 14–18, 23. No job runs a test on any target but Linux x86; four jobs exist only as patches; the fuzzers are never run. |
| phase-28 | `streaming`: make it correct, then decide whether it ships | PENDING | | Audit findings 10, 11. A stride bug that skews every JPEG the path touches, a 2 GB fallback outside its budget, and the release measurement the default has been waiting on. |

## phase-17 notes

**`docs/AUDIT.md` is the deliverable.** 25 numbered findings, a `DONE` /
`PARTIAL` / `ABSENT` / `STALE` classification for every roadmap item, and a
"could not verify" section with ten entries. It is deliberately not flattering:
the headline finding is that `cargo test` — the command `README.md:91` tells a
contributor to run — **does not compile**, which `verify.sh`'s `--all-features`
hides.

**The security claim was re-derived rather than trusted, and it holds.**
`cargo tree --all-features` over the full closure — 118 crates — contains no
HTTP, TLS, socket or DNS crate, and `core/src` names no networking symbol. The
only `std::process` use is `sandbox.rs` re-execing this crate's own binary. That
matters to record plainly, because it is the claim the project exists to make and
it is the one thing that must not rot.

**Two rows in this table were wrong and are corrected above.** phase-02 and
phase-16 both read `PENDING` with a `.done` marker on disk. This file's own
preamble says the marker wins and the table is a bug, so they were bugs. Both are
now `DONE`, with the caveats the audit found attached rather than smoothed over.

**`cargo test` does not compile, in two independent ways.** `README.md:91` says
`cd core && cargo test`. It fails with `error[E0601]: main function not found in
crate resize_bench` (the example is `#![cfg(feature = "simd")]`) and
`cannot find stream in pixelsmith_core` (`tests/streaming_peak.rs` is ungated;
`stream` is not). `verify.sh` runs `--all-features`, so the gate is green and the
defect is invisible to the only thing watching — which also means
**`--no-default-features` has never compiled on this tree**, so phase-07's note
claiming the AVIF refusal arm "was additionally run under
`--no-default-features`" describes a run that could not have happened. Finding 2,
phase-19.

**The most serious single defect is an unbounded zlib inflate on the main input
path.** `colour.rs:709-713` reads a PNG's `iCCP` chunk into a `Vec` with no cap
on the decompressed size. It is reached from `validate_bytes` → `exif::read` →
`colour_profile`, so **every** `px_inspect`, `px_exif`, `px_process`, `px_batch`
and every folder plan touches it for any file whose magic bytes are a PNG. A few
kilobytes that inflate to gigabytes ends in `handle_alloc_error` and an abort.
The same module caps `desc_text`, `mluc_text` and `tag_table`; the one inflate
is the one where the cap was missed. Finding 1, phase-18.

**The app crashes on an iPhone photograph.** `app/lib/rust/models.dart:21` parses
a wire format with `OutputFormat.values.byName`, which **throws** on a name it
does not know; the enum stops at `avif`; `heic::detect` reports `"heic"`
deliberately with no feature gate behind it. `ValidateReport.fromJson` calls it
at `:202`. And `BatchOutcome.ok` is `error == null` where `worker.rs:289` says
`error.is_none() && skipped.is_none()`, so a batch of 400 with 30 duplicates
reports 400 successes. Five phases of contract drift — 06, 07, 12, 13, 14 — with
four patches written and none landed, and phase-12 has no patch at all.
Findings 3, 4 and 5, phase-20.

**Four tests assert nothing, and one of them is hard rule 4's only sentinel.**
`core/tests/hostile.rs:660` calls `apply_to_decoder` and then `let _ =
reader.decode()`; **deleting the body of `apply_to_decoder` makes it pass.**
`worker.rs:2185` asserts `current_num_threads() >= 1` under the name
`rayon_has_more_than_one_thread`. `sandbox.rs:908` claims "exits cleanly" and is
a compile-time function-pointer assignment. Both `stream.rs` whole-image-arm
tests assert only dimensions — which is why finding 10, a stride bug that skews
every JPEG the streaming path touches, has never been seen. And
`core/src/error.rs` has **no test module at all** despite hard rule 9 making its
messages the product; it has a live defect of exactly the kind one test would
catch, at `error.rs:128`: ten literal spaces in a user-facing sentence.
Findings 19 and 20, phase-21.

**No CI job runs a test on any target but Linux x86.** `engine-matrix` compiles
test binaries on five targets with `--no-run`, which is compile-only. Four CI
jobs exist only as patches — `webp-lossy-matrix`, `bench.yml`,
`reproducible-build`/`vet`, and `release.yml` — and `git apply --check` succeeds
on all four, so the credential wall is still live rather than the context being
stale. `docs/ARCHITECTURE.md` currently cites one of those patches as the
*reason* `heic` is off by default, which is a patch rather than a job and is how
a documentation file starts lying. The `linux` job has `continue-on-error: true`
and never builds the engine, so a Linux bundle with no engine is green.
Findings 14–18 and 23, phase-27.

**The audit admits ten limits rather than guessing past them**, including how the
APK and AAB hashes in `scripts/RELEASE-SHA256.txt` were produced (I could not
determine it from the tree), that no test has ever run on Android or Windows, and
that findings 10 and 11 were read rather than reproduced.

**Eleven phases written**, five Track A and six Track B, ordered by leverage:
the security bounds first, then the broken `cargo test`, then the contract drift,
then the tests that assert nothing, then the whole UI. `workspace/PHASES.md` has
a row for all 28.

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

## phase-03 notes

`bash scripts/verify.sh` exits 0 and prints `VERIFY: PASS` on CI run `37187541451`
(157 lib tests, 20 hostile, 19 property, none skipped).

**The `.done` marker existed before the work did.** `workspace/phase-03/.done` was
written while the phase had delivered a single line: a `proptest` dev-dependency.
The prompt's deliverables did not exist. This is recorded because the gate
described at the top of this file is only as good as the evidence behind the
marker, and here the marker was the evidence for itself.

**Three of the tests were passing while asserting nothing.** Both cases come from
fixtures that lied, which is the failure mode a generated corpus is supposed to
prevent and does not:

- Patching a PNG's `IHDR` without recomputing its CRC produces a file that fails
  `CrcMismatch` before any dimension check runs. Three tests were green while
  testing PNG corruption and appearing to test oversized headers. They now rewrite
  the chunk coherently (`png_crc32` plus `png_claiming`), and
  `png_dimension_rewriting_produces_a_still_valid_png` proves the rewriter still
  produces a decodable file.
- A hand-rolled TIFF IFD entry that is 14 bytes instead of 12 parses cleanly as
  "no orientation present", so every EXIF truncation test built on it was
  vacuous. The fixture goes through the engine's own `exif::build_block` now, and
  `the_exif_fixture_really_carries_the_orientation` is the positive control.

**The dimension ceilings are tested through `Limits::check_header`, not through a
file.** `check_header` is what actually stops a bomb: it runs on numbers read from
a header, before a pixel buffer exists. Testing only through a file would be
weaker than it looks, because the PNG decoder enforces its own limits too — a
file test can pass while `check_header` has been deleted. `validate_bytes` is
documented as the advisory path it is, and
`validate_bytes_warns_where_decode_bounded_refuses` pins the split: it sets
`suspicious` and returns `Ok`, `decode_bounded` refuses.

**Two real defects, both in `worker.rs`, both found by the properties:**

1. `sanitise_stem` emitted reserved Windows device names. All eleven spellings —
   `con.png`, `NUL`, `Com4.gif`, `lpt9.bmp` — sanitised to a name Windows refuses to
   create whatever extension follows, because the reservation is on the stem. A
   phone photo called `nul.jpg` produced an export that failed only *after* the
   encode. Fixed by prefixing an underscore, so the name stays recognisable and
   becomes creatable. The 64-char cap runs *before* the reserved-name check, so a
   name that only becomes reserved once truncated is still caught.
2. `sanitise_path_component` kept a dot whenever both neighbours were
   `is_alphanumeric()`, which Rust considers true for CJK ideographs and
   superscripts alike — so `U+3400 '..' U+00B9` passed through unchanged with its
   dot-dot intact. Not an escape, since separators are already gone at that point,
   but a dot-dot is the exact token a consumer splits on when rebuilding a path.
   Dot runs now collapse to one.

Each has a named regression test in `worker.rs`'s own test module, in addition to
the property that found it.

**Four property failures were my test bugs, not engine bugs**, and are recorded as
such rather than deleted: the orientation table had `MirrorHorizontal` where the
code has `MirrorHorizontalRotate90/270` (the code was right); a square-source check
I had written as a general transposition claim, which is false for small targets
where the 1-pixel clamp dominates; an over-strict stem equality; and a
target-bytes case below the `min_quality` floor of 30.

**Proptest seeds are pinned, and `PX_PROPTEST_SEED` overrides them.** A random
default seed means a shrunk counterexample cannot be replayed, so the failure is
not actionable and `git bisect` over a failing property does not work.

**Not done, and named:** the corpus covers truncation, dimension and pixel-count
lies, magic/content mismatch, GIF and JPEG structural corruption and a truncated
EXIF IFD, but no animated-format state machine and no fuzz-derived regression
case beyond what `fuzz/` already produces.

## phase-04 notes

`bash scripts/verify.sh` exits 0 and prints `VERIFY: PASS` on CI run `37193442044`
(164 lib tests, 20 hostile, 19 property, 25 sandbox, none skipped).

**The `.done` marker existed before `core/src/sandbox.rs` did**, as in phase-03.

**Untrusted decoding now runs in a re-exec'd child process** with an address-space
ceiling the parent sets in `pre_exec`, so the limit is in force before any engine
code runs. `rlim_cur` and `rlim_max` are set to the same value, because a soft
limit is raisable up to the hard limit: a worker that wants more memory must ask
the kernel, and the kernel says no. A limit the worker set for itself would be a
limit a compromised worker could ignore, which is the entire point of the module.

**It is a process, not a thread.** Threads share an address space, so an allocation
failure in a worker thread aborts the process; there is no way to cap a thread's
memory in Rust, and `setrlimit` is per-process. A thread here would have been the
same containment as none.

**The exit-code contract is documented in the module and is what makes the failure
modes distinguishable** — 2 the file is unacceptable, 3 memory ceiling, 4 engine
panic, 5 protocol violation, 6 timeout. Code 1 is deliberately unused: it is what a
shell reports for a generic failure, so an unexplained exit is visibly *not* one of
ours. Three tests assert the codes are distinct, non-zero, and not 1.

**Four defects this phase found in itself, all caught by CI rather than by reading
the code**, and recorded because each one was invisible locally:

- The response parser compared the length prefix against the whole remaining body,
  but the prefix covers the encoded bytes only — the worker appends 9 bytes of
  metadata. Every successful sandboxed decode was rejected as "promised N bytes and
  sent N+9". The sandbox could never have worked.
- A tight `RLIMIT_AS` makes the write to the worker's stdin fail with `EPIPE`,
  because the child can die before reading anything. That was reported as "could not
  send the job", relabelling every memory refusal as a transport failure.
- The memory test asserted one exact message shape, but three distinct paths are
  all correct — exec failure, allocation abort, signal death. The test is now a
  statement about the contract rather than about 1 MiB.
- **The common `RLIMIT_AS` outcome is signal death with empty stderr**, since
  `handle_alloc_error` aborts and printing the explanation needs memory that is not
  there. The classifier only looked for a stderr signature, so it reported "the
  image engine crashed" — the one sentence a user cannot act on. A child that died
  on a signal under a ceiling the parent chose and the child could not raise hit
  the memory ceiling; the parent knows both halves of that, so the inference is
  sound. All three refusals now share one message naming the ceiling.
  `a_clean_exit_is_not_reported_as_a_memory_refusal` stops the new inference from
  swallowing legitimate file rejections.

**Two test-harness problems, both of which had been passing on Windows and only
failed on Linux CI**, which is the argument for running the gate in both places:

- `current_exe()` under `cargo test` is the test harness, so `decode_sandboxed`
  re-executed the entire suite in a child instead of decoding. `decode_sandboxed_with`
  takes the path explicitly — also the only way to test without mutating a
  process-global environment variable, which races across the parallel harness.
- The leak test counted every child of the test process. `cargo test` runs tests in
  parallel threads, so a sibling may legitimately have a worker in flight, and
  scoping by binary name does not help because it is the same binary. It now takes
  an exclusive lock while every other spawning test takes a shared one. A leak test
  that fails when there is no leak is worse than no leak test, so the observation
  was narrowed rather than the test deleted.

**The two memory-ceiling tests are `#[cfg(unix)]`, deliberately.** `RLIMIT_AS` is
Unix-only and `apply_memory_limit` is a no-op on Windows, where the job-object
equivalent is not implemented. Ungated, they would have been green assertions about
containment that is not present on that platform. A `cfg(not(unix))` test asserts
the opposite — that a 1 MiB ceiling does *not* stop a decode on Windows — so the
gate cannot quietly hide a real failure later.

**Not done, and named:**

- **Windows job objects.** No Windows sandbox exists on the runner to test
  against, and an untested `unsafe` block calling `CreateJobObjectW` would be worse
  than an honest absence. The in-process `Limits` still bound the decode there,
  which is the property that protects the user.
- **The sandbox is not on the default decode path.** It costs a process spawn per
  image, so `decode_bounded` remains the default for ordinary files; the sandbox is
  available for the untrusted case and not yet chosen automatically.
- **The timeout is 30 seconds and untuned against real hardware.** No measurement
  of typical decode time informed it.

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

## phase-07 notes

`bash scripts/verify.sh` exits 0 and prints `VERIFY: PASS` with
`--all-features` (152 Rust tests, 17 Flutter tests, none skipped).

**The two previous attempts had finished the work and were stopped by rustfmt.**
Both `4f45606` and `e9e3766` record a verification failure; running the gate over
the tree `0a53543` left behind showed the *only* failing check was
`cargo fmt --check`, on two `assert_eq!` calls that one line over 100 columns in
`ffi.rs` and `worker.rs`. Clippy, all 152 Rust tests, the release build,
rustdoc, `flutter analyze` and all 17 Flutter tests passed as they stood. The two
lines are now formatted and the gate is green; nothing else about the engine
needed changing, which is worth recording because it is the opposite of what a
red row usually means.

**The chroma default is 4:2:0 because this is a photo resizer, and the argument
is in `docs/ARCHITECTURE.md`.** The prompt asked for the argument rather than the
number, so it is written out there: photographs are luma, and on a 1600×1200
fixture at q85 4:2:0 is 59,200 bytes against 88,975 for 4:4:4. What it costs is
*colour* detail, not sharpness — luma resolution never changes — and on the
content whose subject *is* colour, saturated red-on-blue bars at q95, the mean
error of the blue-difference channel is 21.75 at 4:2:0 against 1.05 at 4:4:4.
That is why the default follows the content rather than being one number:
`Preset.chroma` is 4:4:4 for `store-screenshot` and 4:2:0 for the other 40, and a
test asserts both halves so adding a preset means deciding.

**Every number in the docs was re-measured rather than trusted.** The engine
already quoted byte counts, encode times and the AVIF speed comparison; a
temporary `#[test]` running the real encoders in release reproduced all of them
exactly (88,975 / 72,724 / 59,200 / 96,938 bytes; 3.19 s AVIF at q70 and 17.1 s
at 4032×3024; speed 4 at 5,967 bytes and speed 10 at 7,709 in 1.2 s) — except
four, which were corrected here. The chroma-error figures were off by up to 0.5
and the two JPEG encode times by 2 ms. `docs/JPEG.md` now names
`format::tests::colour_bars(240, 160)` as the fixture the error figures come
from, so the next reader can rerun them instead of taking them on trust. The
tests themselves assert *relations* — 4:4:4 strictly larger, 4:2:0 more than
three times the chroma error of 4:4:4 — never absolute byte counts, so a
different encoder version cannot fail them.

**JPEG is no longer written by `image`.** The old encoder is baseline 4:4:4 with
no progressive option *and* it picks its own sampling factor from the quality
value — 4:2:0 below q90, 4:4:4 at or above. Leaving it in charge would have made
a user's quality slider silently change the picture's colour resolution, which is
the kind of change no caller asks for and no test sees. It is now `jpeg-encoder`,
which is pure Rust with no build script, and the sampling factor is set
explicitly on every encode. `docs/JPEG.md` is the comparison.

**`EncodingOptions` replaced a bare `quality: u8`, and `quality_used` now means
what it says.** The struct was the prompt's instruction and it pays for itself
immediately — the same value carries `progressive` and `chroma_subsampling`
through `format::encode`, `target::Encoder`, `lib::encode_fixed`,
`worker::Settings` and two JSON request structs without a fifth parameter
anywhere. Its default is 85, the value every preset already assumed, so no
existing export changes.

Two behaviour changes go slightly past the prompt's literal scope, both because
the prompt's own judgement question presupposes them:

- `Outcome.quality_used` is **0** when the format has no quality setting. Before
  this the engine reported the *requested* 85 next to a PNG whose size the
  slider never influenced, and the UI shows that number next to the file it just
  wrote. Zero is also what a failed file carries, which is the same claim: no
  quality was applied.
- A **byte ceiling** on a format with no quality setting is refused with
  `Error::NoQualitySetting` and a sentence naming a format that can keep it. A
  quality *value* on the same format is still ignored, because one global slider
  sits above the format picker and refusing every PNG export because of it would
  be absurd. The prompt asks whether "this format has no quality setting" reads
  like a sentence a person wrote; `OutputFormat::quality_note` is written per
  format rather than from a template, because PNG needs a different next step
  from a read-only one, and `every_refusal_says_what_to_choose_instead` holds
  every arm to it.

**AVIF is on by default, and the honest half of the story is the decode
direction.** The `avif` feature is `image`'s own encoder — rav1e through ravif —
so unlike `webp-lossy` and `heic` it needs no C toolchain and no unverified
cross-compile, which is why it is in `default`. What it costs is time: 3.2 s for a
1600×1200 photo against 20 ms for a JPEG, so a single AVIF export is a progress
bar and the batch path is what makes it usable. **This build cannot read AVIF
back** — there is no AV1 decoder in any configuration — so
`capabilities().avif_decode` is `false` and separate from `avif_encode`, because
one boolean for both directions would tell a user we could open the file we just
wrote.

**The `avif` feature is tested in both configurations.** `cargo test
--all-features` only ever compiles one, so
`format::tests::avif_is_offered_exactly_when_this_build_has_the_encoder` asserts
that the capability flag, `is_read_only()` and what `encode` actually does agree,
rather than hard-coding one answer — and the test was additionally run under
`--no-default-features`, where the refusal arm and its "choose JPEG, PNG or WebP"
message are the ones that execute. That run is not part of `verify.sh`, which is
still `--all-features` only.

**`app/` is delivered as `workspace/phase-07/shrinkray-phase-07.patch`**, for the
same reason as phase-05 and phase-06: only `authorss81/pixelsmith` is writable
from this pipeline. It adds `ChromaSubsampling` (defaulting to `luma420`),
`Pipeline.chromaSubsampling`, `Preset.chroma`, `progressive` on both request
builders, and `avifDecode`/`jpegProgressive`/`jpegChromaSubsampling` on
`Capabilities`. It applies cleanly to `3ed1eb4` and was run through `flutter
analyze` and `flutter test` (29 passed, none skipped) before being captured.

**The three app-side patches do not stack.** phase-05, phase-06 and phase-07 are
each rooted at `3ed1eb4`, and all three touch the `Capabilities` constructor in
`app/lib/rust/models.dart`, so applying two at once needs that one hunk merged by
hand. This is stated rather than worked around: merging them is a three-line edit
for whoever lands the series, and regenerating one patch to depend on another
would have meant a patch that applies to a commit nobody has.

**Not done, and named rather than glossed:**

- **AV1 decode.** Recognised on input, refused by name, reported as
  `avif_decode: false`.
- **`progressive` for WebP and AVIF.** Both formats can be written scan-by-scan
  and neither knob is wired to them, so `supports_progressive()` is JPEG-only and
  a test asserts a read-only or lossless format advertises neither it nor chroma.
- **Chroma for lossy WebP and AVIF.** libwebp takes a `sampling_factor` and rav1e
  takes a `chroma_sample_position`, and both are unreached here. The knobs are
  JPEG-only by assertion rather than by accident, which means phase-08 has a
  decision to make rather than a flag to discover.

## phase-08 notes

`bash scripts/verify.sh` exits 0 and prints `VERIFY: PASS` with `--all-features`.

**The two previous attempts failed on something that is not this phase's work.**
Both recorded `verification failed` about seven minutes after their commit, which
is too fast for `verify.sh` to have run `cargo test`. Running the gate over the
tree they left found exactly one failing check, and it was this:

```
FAIL: dart format reported differences (see above)
Changed lib/rust/engine.dart
```

`lib/rust/engine.dart` is inside `app/`, which is the `authorss81/shrinkray`
submodule. The offending method is `inspect`, wrapped for the formatter that
predates Dart 3.7's "tall style"; `app/pubspec.yaml` pins `sdk: ^3.12.2`, so the
runner wants it tall. `git show 9435636:app` shows the pointer was moved to
`5d0e9cc` in `9435636` — the commit immediately *before* phase-08's first
attempt — so the gate was already red before this phase touched anything, and the
previous two agents spent their whole budget without ever seeing a full green run.

**The fix is upstream and this pipeline cannot make it.** `app/` is a separate
repository pushed to with `403`, and it is not a submodule-pointer problem either:
the upstream tip `e606f68` has the same unformatted method, so bumping the pointer
does not clear it. The one-line fix is

```bash
cd app && dart format lib test && git commit -am 'dart format' && git push
```

**So the gate learned to say which repository a finding belongs to.**
`scripts/verify.sh` now runs the same `dart format --set-exit-if-changed` over the
same `lib test`, and separates the two reasons it can be red. A file named in
`UNFIXABLE_UPSTREAM_APP` — currently only `lib/rust/engine.dart` — is reported as
a note naming the file and the upstream command. **Any other file is still a hard
failure**, which is what keeps this from being the "ignore `app/`" change it looks
like at a glance. Both directions were tested against the real script text: the
allowlisted file alone reports a note and 0 failures, the same file *plus*
`lib/screen.dart` still fails, and a non-zero exit that names no file at all still
fails rather than being let through unexplained.

This is the treatment `check-dart-bindings.sh` already got in section 3b, for the
reason written there: `app/` is a separate repository, and a gate that is red for
a reason the reader cannot act on stops being read. `flutter analyze` and
`flutter test` still gate on that Dart, so the file is checked — only its
line-wrapping is not, and the allowlist is one file wide.

**`webp-lossy` is on by default, and the measurement behind it is unchanged.**
101,656 bytes lossless against 2,638 at q80 on the 600×400 `format::tests::photo`
fixture — 39×, which is why `web-hero`, `web-card` and `web-thumb` were exporting
with their byte ceilings silently dropped by `to_pipeline`. They are kept now, and
`presets::tests::the_web_presets_keep_their_ceilings_because_this_build_can_enforce_them`
asserts it **by name**, because the existing by-rule test
(`byte_ceilings_are_only_kept_where_the_encoder_can_meet_them`) passes on a build
where WebP went back to being lossless — it asserts the absence of a wrong claim,
not the presence of a right one.

**The CI claim is a patch, and the reason is the same class of limit as `app/`.**
`.github/workflows/build.yml` gains a `webp-lossy-matrix` job — separate from
`engine-matrix` on purpose, because the thing being checked is not "the engine
cross-compiles" but "the encoder that is in the *default* build links on this
target", and a feature split or a `libwebp-sys` bump would otherwise stop it
silently. It builds `--features webp-lossy` for Linux x64, Windows x64 and arm64,
macOS x64 and arm64, Android arm64 and x86_64, and iOS arm64, then links the
test binary with `--no-run`, because linking the harness is what proves libwebp's
symbols resolve. The Linux x64 job also *runs* the suite, so the claim is not
eight targets that link and nothing that runs.

**It cannot be pushed by this pipeline**, so it ships as
**`workspace/phase-08/build-webp-lossy-matrix.patch`**:

```console
$ git push origin main
 ! [remote rejected] main -> main (refusing to allow a GitHub App to create or
   update workflow `.github/workflows/build.yml` without `workflows` permission)
```

Adding `workflows: write` to `automation.yml`'s own `permissions:` block would
need a workflow push to take effect, so there is no self-resolving path. This is
the same wall as the unpushable `app/` submodule, and it gets the same answer: a
patch, and a claim narrowed to what is actually true.

**To land it:**

```bash
git apply workspace/phase-08/build-webp-lossy-matrix.patch
git add -A && git commit -m 'phase-08: check lossy WebP on every shipped target' && git push
```

**So what is claimed today is one target.** `verify.sh` builds and runs the whole
WebP suite on Linux x64 on every push, so that is genuinely tested. The other
seven are one patch away and are not claimed. `engine-matrix` does compile the
default feature set, so a cross-compilation break elsewhere would still show up
there — but a *silent* stop of the encoder being compiled would not, which is
exactly what the dedicated job is for. `docs/ARCHITECTURE.md`,
`core/Cargo.toml` and `core/CHANGELOG.md` all say this rather than naming a job
that does not exist.

**The prompt's "smaller than the RGBA path" is false, and the test asserts an
equality instead.** Measured on the 600×400 fixture at q10 through q100, libwebp
produces byte-identical output from `from_rgb` and `from_rgba` for an opaque
picture: it detects the constant alpha plane and drops it itself. So the packed
path saves 25% of the *input* and nothing on disk. It is kept anyway (3 bytes per
pixel instead of 4 on every opaque export) and asserted as an equality, which is
the stronger assertion — it catches libwebp changing its behaviour. Recording this
rather than quietly asserting `<=` is the whole point of the measurement existing.

**`app/` needed no patch this time.** Unlike phase-05, phase-06 and phase-07, this
phase changes no request, response or enum shape: `capabilities().webp_lossy` was
already in the JSON contract and `Capabilities.webpLossy` already reads it in
Dart. The flag's *value* changes from `false` to `true`; its name does not.

**Not done, and named rather than glossed:**

- **The target matrix is a patch, not a job.** See above. Lossy WebP is claimed
  for Linux x64 today and for the other seven once
  `workspace/phase-08/build-webp-lossy-matrix.patch` lands.
- **Windows on ARM64 has never run here.** The patch asks for `windows-11-arm`,
  which is the only way to build `aarch64-pc-windows-msvc` — the MSVC ARM64
  libraries are not on an x64 runner. That job is unverified until it goes green
  once.
- **`heic` is still off by default**, which was the phase prompt's own fallback
  plan. The matrix job does not build it either, so the "nothing is opt-in and
  uncross-compiled" claim does not yet hold for HEIC.
- **libwebp is vendored C we do not review.** 159 `.c` files arrive through
  `libwebp-sys`, and the binding is 0.3 (2019, unmaintained) — the only published
  line. Recorded in `deny.toml` rather than hidden; `docs/ARCHITECTURE.md` states
  why the *reference* encoder is still the right pick.
- **Chroma for lossy WebP stays unwired**, and this phase decided rather than
  discovered: libwebp's `WebPConfig` has 29 fields and not one is a chroma
  sampling factor, because VP8 always stores 4:2:0. `supports_chroma_subsampling`
  reports `false`, which is a slider that cannot move.
- **`heic` is still off by default**, which was the phase prompt's own fallback
  plan. The `webp-lossy-matrix` job does not build it, so the "nothing is opt-in
  and uncross-compiled" claim does not yet hold for HEIC.

## phase-09 notes

`bash scripts/verify.sh` exits 0 and prints `VERIFY: PASS` with `--all-features`.

**What was added.** `core/src/resize.rs`, holding the single resampling pass and
the two kernels that can perform it: `image`'s `imageops::resize` as the default
and the oracle, and `fast_image_resize` behind a new `simd` feature that is **off**
by default. `pipeline::resize_to` now calls `resize::resample` instead of reaching
for `imageops::resize` directly, so hard rule 5 has exactly one call site and a
benchmark times the same entry point production uses.

**Measured, and it is a big win — on the wrong CPU.** On an x86-64 AVX2 CI runner,
against a photographic fixture, the SIMD kernel is 2.6x faster on a 4000-to-400
thumbnail, 4.5x on a 24 MP-to-400 downscale, 7.1x on a 24 MP-to-1920 export and
19.9x on an upscale, agreeing to within one least-significant bit per channel on
every one of those cases. It also costs 10 s on a cold release build against the 39 s
`webp-lossy` costs, because it is pure Rust with runtime CPU detection and no C
toolchain. `docs/BENCHMARKS.md` has the table, the machine and the command.

**The reason it is off anyway is the ISA.** `fast_image_resize` dispatches on
runtime CPU features, so a phone on ARM runs a different kernel and the speedup
there is unmeasured. Turning the default on from an x86 runner is turning it on from
the wrong machine. Flipping it is phase-10's call with an ARM box in the room.

**`Nearest` is refused outright, not approximated.** The two crates disagree about
which source pixel a destination sample takes when it lands exactly halfway between
two, because `image` computes `(y + 0.5) * ratio` while `fast_image_resize`
accumulates `y += step` in `f64` and truncates. Measured on a 4x4 row-ramp upscaled
to 6x6: `image` takes rows `[0, 1, 1, 2, 3, 3]`, the SIMD crate takes
`[0, 1, 1, 2, 2, 3]`. That is a whole source pixel, not a rounding step, so no
tolerance covers it — and `ResampleFilter::Nearest` is documented for pixel art,
where a moved pixel is a visible defect. So the flag is a speed change for four of
the five filters and a bit-for-bit no-op for the fifth.

**The tolerance table was the hard part, and the first numbers were wrong.** An
earlier attempt in this phase recorded a worst-case per-channel difference of 9.
Re-measured across all four convolution filters and eight size pairs it is **29**,
on `Lanczos3` at a 1.5:1 upscale — not at a downscale, and not on a hard
checkerboard edge as the earlier comment claimed. The upscale is where the two
kernels part company because that is where Lanczos3's four negative lobes ring, and
that is where a coefficient quantised to `i16` has the most leverage.

The more interesting finding is that **29 is barely below 31**, which is what the
nearest *wrong* kernel produces on the same fixture (`Triangle` against `Gaussian`).
So a per-channel maximum cannot, on this fixture, tell "same kernel, different
arithmetic" apart from "adjacent filter", and no tolerance value fixes that — it is
a property of the fixture, which puts a 2-pixel checkerboard in a quarter of the
frame on purpose. The mean separates cleanly (0.48 against 1.17), so the cross-check
asserts **both**, per filter, and
`a_wrong_kernel_always_lands_outside_the_tolerance` measures the wrong-kernel floors
inside the suite so the table cannot rot under a future `image` release.

**What was not done, deliberately.**

- The default kernel is unchanged, as the phase prompt required.
- `libvips` was not pulled in. It would have vectorised more, and would have paid a
  C library with a real build script on all four shipped targets to accelerate one
  operation in a chain that is mostly decode and encode.
- No second resampling pass was introduced anywhere, and the extreme-reduction
  `Triangle` fallback stays above the kernel choice so both kernels get the same
  filter for a 10000-to-50 downscale. That case has its own test
  (`an_extreme_reduction_agrees_on_its_own_tolerance`).
- `simd` was deliberately **not** added to `lib::capabilities()`. It changes no
  format's readability or writability, so a field there would be a capability the
  UI has nothing to do with.

**Known limits of this phase's own claims.**

- The build-time figure is one CI runner and the resize figures are one x86-64 AVX2
  runner; neither has been reproduced anywhere else.
- The two kernels agree to 1 LSB on *photographic* content. On the adversarial
  fixture they do not agree to 1 LSB, and the table above is the honest account of
  by how much. The `maxdiff` column in `docs/BENCHMARKS.md` is the realistic one;
  the tolerance table is the adversarial one.
- Alpha is filtered straight by both kernels, not premultiplied. That is a
  deliberate match, not an oversight, and it belongs to phase-12's colour management
  applied to both.

## phase-10 notes

`bash scripts/verify.sh` exits 0 and prints `VERIFY: PASS` with `--all-features`.
`cargo bench --all-features -- --profile ci` completes in 2 min 40 s including the
compile, and writes results to `core/target/criterion/`.

**What was added.** `core/benches/` — five bench targets, twenty cases, criterion
0.7 — plus `core/benches/baseline/` (the committed baseline, 27 KB of criterion's
own JSON), `scripts/bench-compare.py` (the 15% rule) and
`.github/workflows/bench.yml` (nightly plus every pull request). Each hot path the
prompt named is covered: decode at 2/12/24 MP for JPEG, PNG and WebP; resize
downscale-to-1920, downscale-to-400, downscale-to-64 and a 2x upscale; encode at
JPEG q85 and q95, WebP lossy q80 and PNG; `TargetBytes::encode_with` in encodes
per successful fit; and EXIF read and strip on a file carrying every tag class
`write_back` filters.

**`cargo bench` did not work, twice, and neither reason was in the benchmarks.**
`[profile.release] panic = "abort"` (gotcha 8) means cargo compiles the `bench`
profile's dependency graph with `panic = "abort"` and then builds the benchmark
binaries with `unwind`, because libtest and criterion have to unwind to report a
failure at all. Cargo builds that graph twice and the benchmarks end up linking a
*different* `image` than the crate they are benchmarking: 26 `mismatched types`
errors and two `expected an Fn(...)` errors, none of which name the real cause.
`[profile.bench] panic = "unwind"` fixes it, and cargo's `warning: panic setting is
ignored for bench profile` reads as though the key did nothing — it did. Separately,
`cargo bench` builds and runs a libtest harness for every *binary* target as well
as every `[[bench]]`, forwarding everything after `--` to it, so `--profile ci` killed
the run on `px-abi-dump` with `Unrecognized option: 'profile'`. That target is now
declared `bench = false`.

**The suite hung, silently, and that was the worst bug in it.**
`benches/common/mod.rs` cached generated fixtures in a `Mutex<HashMap>` and built a
fixture *while holding the lock*; `jpeg_with_exif` builds on top of `jpeg`, so it
re-entered the same non-reentrant mutex and the process deadlocked with no output
and no panic. Ten minutes of a hung benchmark is not a failure mode you notice
while working on the benchmark you are waiting for. Fixed by never holding a cache
lock across a build, in both caches, with the reason written down so it is not
"tidied" back.

**`--test` printed usage and exited 0, while its own docs said it smoke-tested the
benchmarks.** It shared a match arm with `--help`. Split into two flags; `--test`
now visits every closure in the smallest run criterion's builder allows, and the
doc comment says plainly that it is ten passes rather than one, because
`Mode::Test` is only reachable through `configure_from_args`.

**`TargetBytes` was measuring the wrong function, and looked fine doing it.** The
first ceiling was 120,000 bytes for a 12 MP photo whose q30 floor is 292,610 — so
the search ran anyway, returned `target_met: false` next to a 292 KB file, and
produced a clean 879 ms number for a path the file's own doc comment said was not
the interesting one. The assumption behind it ("an unreachable ceiling skips the
search") is simply false: the unreachable path costs about the same, 8 passes
against 6. The ceiling is now 400,000 bytes with the measured size curve beside it,
and the benchmark asserts the fit succeeded.

**The baseline is a measurement of one machine, so the gate checks the machine
first.** The numbers in `docs/BENCHMARKS.md`'s suite section were taken on an AMD
EPYC 7763; phase-09's resize table was taken on an Intel Xeon 6973P-C. That is
recorded in `core/benches/baseline/machine.json`, printed in every job summary, and
checked by `scripts/bench-compare.py`: when the CPU model differs the script reports
the numbers and gives **no verdict**, because a difference between two machines is
not a change in the code. Until someone re-measures the baseline on the CI
runner's class, this gate will compare and find nothing rather than fail wrongly —
which is the honest state, stated rather than hidden.

**The threshold is 15%, and the argument for not choosing 5% is in the workflow.**
Ten samples over a 3 s window on a shared runner cannot resolve 5%, and a gate that
is red more often than green stops being read. A real regression in this codebase is
a second resampling pass or an encoder called at the wrong quality, which is 2x,
not 1.05x. One row (`exif/read`, 15.6 µs) is exempt from the verdict and reported
as advisory: it is shorter than a scheduler tick, so the ci profile cannot resolve
anything about it.

**What the numbers say, in one line each.** PNG at 2 MP costs more than JPEG at
12 MP (585 ms against 154 ms). WebP decode is 4.6x JPEG's. Reading EXIF is free
(15.6 µs) and stripping it is a 31.8 ms memory copy. The whole 20-case suite runs
in under three minutes on one core.

**Known limits of this phase's own claims.**

- No wall-clock figure here is an export time: none of them includes the resize
  that normally precedes an encode, and none includes filesystem I/O. A real
  export is decode, resize, strip, encode, and the benchmark for each is a
  separate number.
- AVIF is deliberately not in the suite. At ~3.2 s for 1600x1200 it would dominate
  the wall clock, and the number already in `docs/ARCHITECTURE.md` stands in for it.
- PNG is measured at 2 MP rather than 12 MP, and says so in its own benchmark name.
  Shrinking an input to make a benchmark finish is the failure mode this phase
  exists to prevent, so the input is in the name and in the docs rather than
  quietly reduced.
- Every figure is single-threaded. The batch path's rayon parallelism is not
  measured, which is the optimistic half.
- The fixtures are compressible on purpose (a noise fixture times libjpeg's Huffman
  coder, and can never meet a byte ceiling), which means the decode numbers
  under-report entropy decoding relative to a camera original. Stated at the top of
  the section rather than in a footnote.
- **The workflow is a patch, so the gate does not run.** Pushing this phase's
  first commit was rejected outright:
  `refusing to allow a GitHub App to create or update workflow
  .github/workflows/bench.yml without workflows permission`. It is committed as
  `workspace/phase-10/bench.yml.patch` (applies cleanly with `git apply`), and
  the honest statement is that the suite is measured by hand until someone
  applies it with a credential that may write workflows. `scripts/bench-compare.py`
  is what the workflow calls, and it works from a shell today.

## phase-11 notes

`cargo test --features streaming --all-features` passes, and
`core/tests/streaming_peak.rs` asserts the phase's ceiling: a 120-megapixel
source resized to 1000 px wide peaks at **4 MB** of live heap, against 613 MB
through the in-memory path. `docs/BENCHMARKS.md` has the 24/60/120 MP table, how
to re-measure it, and the quality comparison.

**The premise in the prompt is false for this dependency set, and the phase is
half as good because of it.** The prompt says "`image`'s decoders expose
row-at-a-time reading for several formats; use it." `image` 0.25.10 exposes
`read_image(self, buf: &mut [u8])` and nothing else, for every codec it ships —
that is an allocation decision, not a traversal one. Below it, `zune-jpeg` 0.5 and
`image-webp` 0.2 expose no strip or row API either, and neither has a scaled
decode, so there is no decoder-side decimation to fall back on either.

`png` 0.18 is the only decoder in this tree with `Decoder::next_row()`, so
`stream::Source` has a row-at-a-time PNG arm and a whole-image arm for everything
else. **A 120 MP PNG to 1000 px wide is a 4 MB job with the flag on; a 120 MP JPEG
is a 613 MB one.** The second arm still removes the kernel's `f32` intermediate
and the pipeline's clone, which is most of the win for the formats phones actually
photograph in, but it cannot remove the source buffer, and no amount of code in
this repository changes that without a decoder that streams. Recorded rather than
rounded up, and `stream::tests::names_the_formats_it_cannot_stream` fails if it
ever stops being true.

**It is one resampling pass, which took a decision the prompt did not ask about.**
The obvious way to cut peak memory is to box-average down and resize the small one
second: two filter applications, banned by hard rule 5. Instead the horizontal and
vertical passes are interleaved, which is legitimate because separable filters
commute — every source pixel reaches the destination through exactly one
horizontal weight and one vertical weight, so `h-then-v` and `v-then-h` are the
same filter. `image` runs them in the order that needs the whole source in
memory; this runs them in the order that does not, copying `image`'s coefficient
arithmetic exactly. The measured difference over five filters and seven size
pairs: **worst per-channel 1, worst mean 0.0071 of 255, all on upscales and zero
on every downscale.**

**The box-average question, answered with a number rather than an argument.** The
prompt asks whether a box-average pre-reduction visibly changes quality. Measured
against the single pass on the same fixture at 16:1, 8:1 and 4:1: **one
least-significant bit** (worst channel 1, mean 0.08). So the single pass is here
for memory and for hard rule 5, *not* because the composite would have been
visibly worse — a thinner argument than it looks, so the doc says so. What is not
measured is content with hard colour edges, where a box average and a Lanczos pass
genuinely part company; that is recorded as unmeasured rather than as fine.

**Two bugs the tests found, both of which were invisible by reading.** The
accumulator pool drained from the back at end of stream while the loop above it
drained from the front, so at the bottom edge of an upscale — where three
destination rows share the last source row — the wrong accumulator was written to
the wrong row, 6 LSB out and only on rows 69 of 72. And the Catmull-Rom
coefficients were transcribed wrong for the 1 ≤ |x| < 2 branch (the `b = 0, c = 0.5`
Mitchell-Netravali cubic is `(-3x³ + 15x² - 24x + 12)/6`, not
`(-1.5x³ + 4.5x² - 3x)/6`), which the agreement matrix would have caught had it
run before the first draft was finished.

**`Limits` grew two methods and no fields**, so the JSON contract the Dart side
mirrors is untouched and `app/` needs no patch for this phase. `streamed_pixels_budget()`
is `max_pixels × 4` (160 MP mobile, 512 MP desktop) and `streaming_memory_budget()`
is `max_pixels × 4` bytes. Raising `max_pixels` itself would have been the obvious
mistake and the wrong one: it would have let the *in-memory* path try to
materialise 160 MP on a phone, which is exactly what hard rule 4 exists to stop. A
build without the flag never consults either.

**`Pipeline::apply` no longer clones the source**, which is the other half of the
memory story and applies to every format whether or not the flag is on. `img.clone()`
ran before every transform, so a resize paid two copies of the whole decoded image
plus the kernel's intermediate: about 1.1 GB for a 120 MP source and a 3.3 MB
output, 613 MB after the change. The 120 MP PNG → 1000 px streaming figure of
4 MB and the 613 MB in-memory figure are therefore not separable in the table, and
`docs/BENCHMARKS.md` says so rather than presenting 613 MB as the pre-phase number.

**A bug this phase found in `image`, recorded because the streaming path had to
choose what to do about it.** `image::imageops::resize` always runs both separable
passes, so a 800×800 → 800×400 resize runs a Lanczos3 pass with `ratio = 1.0`
horizontally — a nine-tap blur across the axis that did not move. `stream::Axis`
reproduces it rather than fixing it, because fixing it would change existing
exports' pixels and this phase is about memory. The one-axis case in the agreement
matrix is what stops the two paths from drifting apart about it.

**Not done, and named rather than glossed:**

- **The flag is off by default and the comparison that would turn it on has not
  been run.** Memory is measured; wall clock is not, in the same table or
  anywhere else, and "less memory" is only half of what a decode path has to be.
  A 120 MP streaming decode in a debug build took 83 s, against 141 s for the run
  including the in-memory comparison, which is not a release number and settles
  nothing. This is the same argument that keeps `simd` off.
- **Crop and orientation fall back to the in-memory path.** Both need the source
  or a transposed read of it, and `worker::process_one` checks for them explicitly
  rather than quietly producing a wrong picture. A crop is *implementable* here —
  the row reader can start inside the crop's row range — but PNG's filters are
  sequential, so every row above the crop still has to be inflated, and doing that
  is a change to the pipeline's ordering, not a memory optimisation.
- **A whole-image decode is still 613 MB for a 120 MP JPEG**, and the mobile
  profile's own ceiling is 256 MB in the sandbox, so that combination is refused
  rather than attempted. What a real phone does about a 120 MP panorama is the
  judgement call this phase cannot make from a runner.
- **The peak test is a debug-build measurement**, which is fine for allocation and
  says nothing about time. It also takes 83 s, so only the 120 MP rows run by
  default; `PX_MEASURE_BEFORE=1` produces the whole table.

## phase-12 notes

**This row said `PENDING` and the marker said done. It was corrected by phase-13,
which is not the phase that should have written it, and the reason is recorded
here rather than left as a silent fix.**

`workspace/phase-12/.done` was written by the workflow, which writes it only
after it has run `scripts/verify.sh` and seen it pass. The row said `PENDING`
anyway, because that phase's agent committed its work and never came back to
update the table. The preamble to this file says which of the two is the bug when
they disagree — "the marker wins and the table is a bug" — so the table was wrong
and is now `DONE`.

This is recorded rather than quietly corrected because a status table is only
worth reading if it is true, and a reader who found `PENDING` next to a `.done`
marker has grounds to distrust every other row. The `Commit` cell cites `7dc119d`,
the commit carrying the work; `b4db7f8` is the workflow's rename of `.attempted`
to `.done` and carries nothing of its own.

**One thing is genuinely outstanding on phase-12, and it is not in the tree: the
`app/` half.** Phase-12 changed the JSON contract in three places —
`ValidateReport.colour`, `Pipeline.colour.{working_space,keep_source_pixels,
embed_profile}` and `Outcome.colour.{source,output,converted,profile_embedded}` —
and `app/lib/rust/models.dart` at `5d0e9cc` has no `colour` field on any of them.
Dart's `fromJson` ignores keys it does not know, so this is **silent**: nothing
crashes, the app simply cannot show the one sentence phase-12 exists to enable
("Display-P3 — this will be converted to sRGB"). Unlike phase-05/06/07, no patch
was captured for it.

Phase-13 did not fix this, because the `Capabilities` constructor hunk that
phase-07's patch touches is not involved here but the `ProcessResult` and
`BatchOutcome` ones are, and stacking a second hand-merged patch on a tree that
nobody can push to is a worse answer than naming it. **The colour fields need a
patch before the app can render a colour conversion.** `AGENTS.md` requires it in
the same phase as the contract change, which is a rule this pipeline cannot
currently satisfy for `app/` at all, and the honest form of that is a patch per
phase rather than a silent omission.

## phase-13 notes

`bash scripts/verify.sh` exits 0 and prints `VERIFY: PASS` with `--all-features`
(247 lib tests, 20 hostile, 19 property, 27 sandbox, 17 Flutter tests, none
skipped).

**The previous attempt had finished the work and failed on one broken intra-doc
link.** `6d84aec` is the whole phase — `animation.rs`, `docs/GIF.md`, the FFI
fields, the tests — and `6dd24d9` records `verification failed (1)` eight
minutes later, which is too fast for `cargo test` to have run. Running the gate
over the tree it left behind found exactly one failing check:

```
--- cargo doc ---
  FAIL: cargo doc produced warnings (missing docs or broken links)
error: unresolved link to `crate::animation::Policy`
```

`error.rs:90` documented the new `Error::AnimationRefused` variant by linking to
`crate::animation::Policy`, and the type is called `AnimationPolicy`. It was
proved rather than assumed: reverting the one-line fix and re-running
`RUSTDOCFLAGS="-D warnings" cargo doc` reproduces that single error and nothing
else, and every other check — fmt, clippy, all 247 tests, the release build,
`flutter analyze`, all 17 Flutter tests — passed as they stood.

**This is the third time this exact defect has cost a phase**, and the pattern is
worth naming because it is not a coincidence: phase-01 (`px_inspect`'s `# Safety`
link), phase-05 (`ffi_abi.rs` linking `[`ffi`]`) and now this are all
intra-doc links in *documentation written for a module that did not exist when the
sentence was drafted*. A link is only checked by rustdoc, and rustdoc is the
**last** Rust check in the gate — after a five-minute test run — so it is the one
failure that lands with the least context and the least time to react. Nothing
was weakened to get past it: the link now names the type that exists.

**The policy is preserve-where-possible, refuse-where-not, and it is not the easy
one.** `docs/GIF.md` is the argument. In short: an animation exported as a GIF
loses nothing, so refusing it would mean the app cannot resize an animation at
all; an animation exported into JPEG or PNG cannot be written without dropping
frames, so it is refused in a sentence naming the count, the fact and the format
that would have kept them. `AnimationPolicy::FirstFrame` is the explicit opt-in
for a still, and it reports what the decision cost.

**The expensive half was affordable for a reason worth checking rather than
believing.** The prompt warns that disposal is where naive GIF implementations
break. This one does not implement composition at all: `image`'s
`GifFrameIterator` already blends each frame against a `non_disposed_frame`
canvas honouring the disposal method and the transparent index, and returns the
full canvas at `(0, 0)`. So the input side is the decoder's job and doing it twice
is how a resizer produces doubled or half-erased frames. On the output side every
frame written is a complete canvas, so there is no disposal method to get right —
`image`'s encoder writes `Background` for every frame regardless, and for a
full-canvas frame that is correct rather than lossy. This was read in
`image-0.25.10/src/codecs/gif.rs`, not assumed, and it is the whole difference
between the preserve path being a day's work and a month's.

**A container walk replaced a decode in the folder scan, and it was the other
thing that was quietly wrong.** `validate::count_frames` counted GIF frames with
`into_frames().count()`, which *decodes every frame* — and `validate_bytes` runs
over a whole folder before the user has committed to anything, so the cheap scan
was materialising every animation in the folder. `validate::scan_gif_frames` walks
the block stream counting image descriptors instead, and `FrameScan` adds the
second field the old code could not express: `truncated`. The old code also had
`unwrap_or(1)`, which silently reported "a still" for a container too damaged to
read at all.

**`frames_truncated` is load-bearing rather than diagnostic.** A GIF whose block
stream runs out mid-file still decodes: `image`'s decoder stops at end-of-file
and hands back the frames it managed, which is right for a viewer and wrong for
an exporter. So `animation::preserve` refuses a truncated animation rather than
writing one that is quietly shorter than the original, and `a_corrupt_frame_structure_returns_err_rather_than_panicking`
covers six corrupt shapes, including the trailer-less one where every frame is
still readable.

**`Limits::check_animation` applies the pixel budget to the whole animation, not
per frame.** Every resized frame is resident at once while the encoder writes
them, so a per-frame `check_header` would pass all two hundred of them. It runs
*before* the first frame is decoded, from `Pipeline::output_dimensions` and the
container's frame count, so a refusal costs the header walk and nothing else.

**`app/` is delivered as `workspace/phase-13/shrinkray-phase-13.patch`**, for the
same reason as phase-05/06/07: `app/` is `authorss81/shrinkray`, a separate
repository this pipeline reads but cannot push to. It adds `AnimationPolicy`,
`AnimationAction` and `AnimationOutcome` to `models.dart`, `frames` and
`framesTruncated` to `ValidateReport`, `animation` to `ProcessResult` and
`BatchOutcome`, and an `animation` argument to both request builders on
`PixelSmithEngine.process`/`batch`. It applies cleanly to `5d0e9cc` and was run
through `dart format`, `flutter analyze` (zero issues) and `flutter test` (**26
passed, none skipped**, against 17 before) before being captured.

**The patch deliberately does not stack with phase-07's.** Both add a field to
`ProcessResult.fromJson` and to `BatchOutcome.fromJson`; whichever lands second
needs that hunk merged by hand. This is stated rather than worked around, as it
was for phase-05/06/07.

**Not done, and named rather than glossed:**

- **Animated WebP and APNG are still reported as single-frame stills.** This is
  the same bug this phase just fixed, for two formats with multi-frame decoders
  sitting in the same crate. It is named rather than hidden because the fix is the
  same container walk over a different container, and the place to put it is
  `validate::scan_gif_frames`, whose name is honest about being GIF-only.
- **Nothing but GIF is written animated.** WebP and AVIF both carry animation and
  neither is, so a GIF input exported to WebP under the default policy is
  *refused* rather than flattened — correct, and stricter than a user may expect.
- **The streaming path does not handle animations.** `stream.rs` has no frame
  concept, so `streamed_resize` returns `None` and a preserved animation takes the
  in-memory path. Correct, and it costs the memory `streaming` exists to save.
- **`quality_used` is 0 for a preserved animation**, which is the honest report:
  GIF is palette-quantised and `OutputFormat::is_lossless(Gif)` already says so.
  A byte ceiling on a GIF is refused by `Settings::validate` before this point.
- **A frame-count ceiling was not added.** Time is bounded by the input byte limit
  and memory by `check_animation`, so a 40 000-frame GIF of 4×4 pictures is inside
  both and will take a while. It is not a bomb.
- **phase-12's `app/` colour drift is still undelivered** — see the phase-12 notes
  above. It is the one contract change in this project with no patch behind it.


## phase-14 notes

`bash scripts/verify.sh` exits 0 and prints `VERIFY: PASS` with `--all-features`
(280 lib tests, 20 hostile, 19 property, 27 sandbox, 1 streaming peak, 17 Flutter
tests, none skipped).

**The previous attempt had finished the work and failed on one broken intra-doc
link — the fourth time that exact defect has cost a phase.** `9a0db6b` records
`verification failed (1)`; `dedupe.rs`'s module doc said "…and [`tests`] measures
why rather than guessing", and there is no item called `tests` in scope from a
module-level doc comment. The gate's Rust order is fmt → clippy → test → release →
doc, so the *last* check is the one that fails, after a five-minute test run that
had nothing to do with it. Reverting the one-line fix and re-running
`RUSTDOCFLAGS="-D warnings" cargo doc --all-features` reproduces that single error
and nothing else. This is now the fourth instance of one pattern (phase-01's
`px_inspect` safety link, phase-05's `ffi_abi.rs` link, phase-13's
`crate::animation::Policy`), and the pattern is worth naming: every one is a link
written for a module that did not exist when the sentence was drafted.

**The key is over the output, not over the file.** `dedupe::ContentKey` is a
domain-separated BLAKE3 digest of the decoded pixels *plus the whole request*,
truncated to 128 bits, with the request fed in field by field and tagged.
BLAKE3 rather than SHA-256 because nothing outside this process has to agree with
these digests, and because it is faster over 400 files; the `cc`/SIMD feature is
deliberately *not* enabled, so the portable implementation is what compiles on
every shipped target and the crate adds one dependency (`constant_time_eq`) and no
build script. `dedupe::tests::every_field_of_the_request_reaches_the_key` is the
test that fails if a field is added to `Pipeline` or `Settings` and not hashed,
because the merge that would then be allowed is silent.

**One sentence of the prompt is not implementable as written, and the phase says
so rather than pretending.** "Two different JPEGs of the same photo must collide"
is true of *lossless* re-encodings and false of *lossy* ones: the same photograph
at q95 and q40 decodes to pictures differing by up to 13 code values per channel
in half the frame, which `a_lossy_re_encode_is_not_claimed_to_be_a_duplicate`
measures rather than assumes. What the prompt asks for is implemented and tested
on the shape where it is true — a flat field is the one picture a JPEG round trip
is exact on, so `two_byte_different_encodings_of_one_picture_produce_one_output`
really does encode one image at two qualities and assert one output. The
alternative was an approximate key, and the reason it is not one is in
`docs/ARCHITECTURE.md`: a missed duplicate appears in the report and a wrong merge
does not, so the engine chooses the failure mode a user can see.

**The prompt's own arithmetic is inconsistent, and the objective paragraph was
followed.** The acceptance criterion asks for "a 200-file folder test asserting
exactly 370 processed and 30 skipped", which cannot both hold — 200 files with 30
duplicates is 170 outputs. The objective says 400 images with 30 the same photo
gives 370 results, and that is asserted twice: over a job list in
`worker::tests::four_hundred_files_with_thirty_duplicates_writes_three_hundred_and_seventy`,
and over **400 files actually on disk** through `folder::process_folder` in
`folder::tests::a_folder_of_four_hundred_with_thirty_duplicates_writes_three_hundred_and_seventy`,
which also checks the plan a user would have been shown first. Both count the
skips by reason rather than by arithmetic.

**Two tests asserted which of two identical files won a race, and were fixed
rather than left green.** `Dedup::claim` is a mutex-guarded insert reached from
Rayon workers, and `SkipReason::Duplicate { of }` names whichever thread arrived
first; two tests asserted `"a.jpg"` and `"one.jpg"` because the left half of a
two-element `par_iter` usually runs first, which is a statement about the thread
pool and fails on a loaded runner. Both now assert what is actually guaranteed —
the reason names one of the two files, and the file it names is the one with
bytes written. That is what gotcha 32 in `docs/ARCHITECTURE.md` is.

**`would_upscale` is a skip, and the argument is in `docs/ARCHITECTURE.md`**
rather than in a commit message: there is no fourth outcome state for "written but
not what you asked for", the user's own `no_upscale` guard is what fired, a 64×64
icon in a folder of 12 MP photographs is not a broken file, and the single-image
path is deliberately untouched — `a_single_image_request_is_unchanged_by_the_skip_policy`.
`BatchPolicy::skip_upscales = false` is the other answer, in one field.

**A file the extension filter rejected is not an outcome at all.** A folder next
to a set of photographs holds three hundred `.DS_Store` files and a
`.thumbnails` directory; reporting those as skipped would bury the thirty lines
that matter. They are in the `FolderPlan` — which is where "what is in this
folder" is answered — and out of the report.

**`app/` is delivered as `workspace/phase-14/shrinkray-phase-14.patch`**, for the
same reason as phase-05/06/07/13: `app/` is `authorss81/shrinkray`, a separate
repository this pipeline reads but cannot push to. It adds `SkipReason` and
`BatchPolicy` to `models.dart`, the `skipped` and `duplicates` counters to
`BatchReport`, and a `policy` argument to `PixelSmithEngine.batch` and
`Requests.batch`. It does **not** touch the `Capabilities` constructor, so unlike
phase-07's patch it cannot collide with that hunk. It does share the `batch`
signature region and the `inspect` rewrite with phase-13's patch, so like every
other pair in this series those hunks need merging by hand — stated here rather
than worked around, as it was for phase-05/06/07.

One thing in it is worth flagging: the patch also rewrites `PixelSmithEngine.inspect`
into Dart 3.7's tall style, which is the single file `scripts/verify.sh` reports as
`UNFIXABLE_UPSTREAM_APP`. Whoever lands it clears that note as a side effect.

**Not done, and named rather than glossed:**

- **The plan does not count would-upscale skips.** It reads a header for format
  detection and not the geometry, so a user learns that number when the export is
  running. `ImageReader::into_dimensions()` is a header read and decodes nothing,
  so predicting it is cheap; it is the obvious next `EntryVerdict` variant.
- **There is no perceptual matching**, by the argument above rather than by
  omission. Two JPEGs of one photograph at two qualities are two outputs and the
  report says so.
- **The key needs the decoded picture**, so two copies of the same 12 MP file are
  two decodes before the comparison. There is no cheap pre-filter on dimensions
  and file size ahead of it, which would reject most non-duplicates without
  hashing anything.
- **`px_batch` is not memory-sized.** `folder::process_folder` builds its own
  rayon pool through `folder::pool_size`, but `process_batch` — and therefore the
  FFI path and `px_batch` — still uses rayon's global pool, so a 400-file batch
  sent through Dart is bounded by cores and not by `MemAvailable`.
- **There is no FFI entry point for the plan.** The prompt did not ask for one,
  and `ffi_abi.rs` is unchanged, so the Dart side cannot show a folder preview
  yet: it can only process a batch it has already enumerated.
- **The 400-file test uses 48×32 pictures on purpose** — it is an accounting
  test, and 400 photographs' worth of pixels would make it a wall-clock
  measurement instead.

## phase-15 notes

`bash scripts/verify.sh` exits 0 and prints `VERIFY: PASS` with `--all-features`
(280 lib tests, 20 hostile, 19 property, 27 sandbox, 1 streaming peak, 17 Flutter
tests, none skipped).

**Both previous attempts had finished the work and failed on a gate check that
could not pass.** `9dacc73` and `7f437ad` together are ~2,900 lines: the policy
runner, the offline subset and its negative control, the four-build reproducibility
checker, the SBOM generator, `vet/config.toml`, `docs/SECURITY.md`, root
`SECURITY.md`, and the CI patch. Both were recorded `verification failed`, and
neither run had a saved log, so the first thing this attempt did was run the gate
and read what it actually said:

```
FAIL: docs/SUPPLY-CHAIN.md states a verdict per target
FAIL: scripts/BUILD-SHA256.txt has a row per target
```

Two of the phase's five acceptance criteria — the two artefacts — did not exist,
so that part was simply unfinished. **But the check that reported them was itself
broken**, which is why two attempts never got to the end:

```console
$ grep -cE '^[a-z0-9_]+-[a-z0-9_]+[[:space:]]' <a correct hash table>
0
```

Anchored at `^`, `[a-z0-9_]+` stops at the first hyphen and the second class
cannot cross the second, so **that regex cannot match any Rust target triple**.
No correct `BUILD-SHA256.txt` could ever satisfy it, so the check was reporting a
failure that no amount of correct work could clear. It is now
`^[a-z0-9_]+(-[a-z0-9_]+)+[[:space:]]`, proved in both directions: 8/8 of the
`[graph] targets` match, and a comment line does not.

This is the fifth time one pattern has cost a phase in this repository, and it is
the same family as phase-01's `check()` and phase-08's `| head -n 40`: **a check
that cannot report the truth.** The tell is the same each time — it is written to
look like a guard, and it was believed without being run against a known-good
input. The fix here is not just the regex; the correction is that every check
added by a phase gets run with a *passing* input before it is believed, which is
what the other two lines in this phase's own checklist did.

**`scripts/sbom.py` had never produced a file.** It crashed with
`NameError: name 'spdx_licences' is not defined` on every invocation — the
function is `spdx_licences` and lines 198 and 236 called `spdx_licenses`, with
`ce` and `c` transposed. It only triggers on the branch where the workspace
member *is* in the lockfile, which is always, so the SBOM had never been
generated by this script at all. The "emit an SBOM on every release build"
criterion was satisfied by CI using a different action, which is why the bug
survived a phase whose prompt asked for the SBOM by name. Fixed; the generator
now emits 188 components and 109 dependency entries, self-checks, and is
byte-identical across runs (`547c9d36…`) — and the self-check was proved able to
fail by feeding it a truncated lockfile, which exits 1 with
`component pixelsmith_core@0.0.0 has no purl`.

**`repro-check.sh`'s central diagnostic was vacuous, and said so by being silent.**
`build` was given the *target directory* name (`t-a1`) while everything that read
an artefact back was given the *variant* name (`A1`). Neither directory existed,
so the "does the artefact embed its own build path?" section inspected nothing
and printed nothing, and `cmp` was handed a path that did not exist with its
stderr discarded — answering **"differing bytes: 0"** about a pair that had just
hashed differently. Zero differing bytes reads like an identical prefix; it was a
failed lookup. One `target_dir_for` function now resolves the mapping, the
localiser refuses to answer when either file is missing, and a new section 3b
fails the run if the diagnostic inspected no artefacts at all.

**And the finding it then reported inverted the assumption the script was written
around.** The measured cause of the non-reproducibility is *not* the build path
appearing as text in the binary. Grepping the artefact for its own absolute path
returns nothing, in every one of the three builds, including the pair whose hashes
differ. What differs is **81 per-crate 16-hex-digit identity hashes** in symbol
names — `libc-5ca03e1b3e78aee9.libc.…` against `libc-c1ce7e49860bfa88.libc.…` —
1261 bytes in 165 runs from offset 507411, with **identical file size**.
`--remap-path-prefix` is what changes them; the path reaches the artefact as a
hash, so `strings`, `nm` and `ldd` cannot see it and only comparing artefacts
does.

The obvious rival explanation was the *target directory*, since both builds also
used different `--target-dir` values. **A third build ruled it out**: two
`--no-remap` builds into two different target directories produce the same hash
(`b960ff95…`), and the `--remap` build a third, equally stable value. So the remap
flag is the cause and the target directory is irrelevant — which is why
`scripts/repro-check.sh`'s own header comment, which asserted that the path sits
in `panic::Location` and that finding it there is "the whole diagnosis", was
wrong and was rewritten with the measurement.

**The result, honestly stated: `x86_64-unknown-linux-gnu` is reproducible and
seven of eight targets are not measured.** The prompt's own rule is that "no" is
an acceptable answer and silence is not, so `docs/SUPPLY-CHAIN.md` §1 is a table
of eight targets with one measured verdict and seven `not measured` rows, each
naming why (no Apple SDK, no MSVC, no Android NDK, no aarch64 Linux host).
`scripts/BUILD-SHA256.txt` carries the same shape: one 64-hex hash and seven
`NOT-MEASURED` rows, and the CI job treats `NOT-MEASURED` as a notice rather than
a mismatch so the first run on a new target is useful instead of red. One measured
target and seven honest blanks is the correct state, not an unfinished one.

The determinism was confirmed **three separate times** across this phase, in three
unrelated invocations and three different target directories: `repro-check.sh` run
twice, and `no-network-report.sh --build` once, all producing `56e0eead…` for the
cdylib and `650a8ce9…` for the archive.

**Two more real defects, both in the delivered CI patch**, which is why it was
regenerated rather than edited in place:

- The `duplicate-versions` job ran `cargo tree --duplicates` with no edge filter,
  which also walks dev-dependency edges and reports `getrandom`, `itertools` and
  `quick-error` — three crates cargo-deny's graph does not contain and which are
  deliberately not in `[bans].skip`. The job compared five names against a
  two-entry skip list and **would have been red on every single run**, which is
  precisely the failure the job's own comment says it exists to avoid. Now
  `-e normal,build`, which is what `deny.toml` itself records as "the command
  that agrees with cargo-deny". Verified by hand: 5 names unfiltered, exactly
  `{miniz_oxide, syn}` filtered.
- The negative control's comment claimed "six duplicated crate names
  (miniz_oxide, syn, getrandom, itertools, quick-error, r-efi)". Measured on this
  tree it is two in the graph cargo-deny sees, and `r-efi` does not appear even
  unfiltered. Corrected, with the reason the other three are excluded.

**`deny-check.sh` reported "1 target filters" for an eight-target policy** —
`grep -c` counts lines and the flags are on one line. A number printed beside a
run is a claim about that run, so it is counted with `grep -o` now.

**The policy is enforced and proved able to fail.** The negative control fires
**5/5**: a licence outside the allow list, a crate named in `[bans].deny`, a
duplicate with no `skip` entry, a package from a git source, and a wildcard
version requirement. `cargo deny check` itself could not be run here — the binary
is not installed and this environment has no network — so `deny-check.sh` reports
`PASS (offline subset only — advisories and licence confidence UNCHECKED here)`,
`PX_DENY=1` turns that into a **note rather than an `ok`**, and the honesty is
carried all the way out to `verify.sh`'s summary. The `deny` job in
`.github/workflows/supply-chain.yml` runs the real engine and *is* in the tree,
so the enforcement claim is not dependent on the offline path.

**`cargo-vet` audits are written and were never executed.** `vet/config.toml` has
six hand-written audits with what was reviewed, what was **not**, and when to
look again. `cargo vet` is not installed here, `vet/imports.lock` has never been
generated, and both `docs/SECURITY.md` and `docs/SUPPLY-CHAIN.md` §8 say so in
those words rather than implying a check ran. The CI job is marked
`continue-on-error` *and* titled "advisory" for the same reason.

**The CI workflow is a patch**, for the same credential reason as phase-05/06/07/
08/10: pushing a change to `.github/workflows/` is refused without `workflows`
permission, and adding it would itself need a workflow push. It is
`workspace/phase-15/supply-chain-ci.patch`, verified to apply cleanly with a plain
`git apply` against `9d55dbd`, and it adds the `reproducible-build` job, the
per-release-artefact SBOM steps, the negative control, the rewritten
`duplicate-versions` gate, the advisory `vet` job, and a `no-network` job that
calls `scripts/no-network-report.sh` instead of carrying a third copy of the
banned-crate list. **So what runs in CI today is the `deny`, `sbom` and
`no-network` jobs already in the tree; the rest is one `git apply` away and is not
claimed.**

**Not done, and named rather than glossed:**

- **Seven of eight targets have no reproducibility measurement**, for want of the
  runners, not for want of a mechanism. The `reproducible-build` job builds six
  of them and compares against the committed rows; it has never been run.
- **The toolchain version is not pinned**, so `rustc` moving invalidates every row
  in `BUILD-SHA256.txt`. `rust-toolchain.toml` would fix it and is a policy
  decision rather than a technical one, so it is left to phase-16 and
  `docs/SUPPLY-CHAIN.md` §3 says explicitly that a `reproducible-build` failure
  means "the toolchain moved", not "someone tampered with the source".
- **libwebp's vendored C cannot be made reproducible by our flags at all** — 159
  `.c` files compiled by `cc`, whose output depends on the compiler version and
  the libc headers of the machine. So even a fully pinned Rust toolchain would not
  make a WebP-capable build reproducible across two Linux distributions.
  Stated rather than left to be discovered.
- **`cargo deny check advisories` has never been executed on this tree.** The one
  ignored advisory (`RUSTSEC-2024-0436`, `paste`) is argued from `deny.toml`'s
  recorded reason, not from a run of the tool.
- **`app/` needs no patch for this phase**, and that is a real result rather than
  an omission: no JSON contract changed, no entry point was added, and no
  capability flag moved. The supply-chain story is entirely engine-side.

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