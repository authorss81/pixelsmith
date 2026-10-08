# phase-20 — The Dart JSON contract, and a check that makes drift fail the gate

**Track:** A — Engine & Security
**Depends on:** phase-19
**Timeout:** 90 minutes

## Objective

The engine has changed its JSON contract in five phases. `app/lib/rust/models.dart`
has not caught up with any of them, and the consequence is not cosmetic:

**Picking an iPhone photograph crashes the app.** `OutputFormat.fromJson` uses
`byName`, which throws on a name it does not know; the enum stops at `avif`; the
engine reports `"heic"` for an iPhone photo from `heic::detect`, which is
deliberately not behind any feature so a build without the codec can still name
the format.

And **the batch report says every file succeeded when some were skipped.**
`worker::Outcome::ok()` is `error.is_none() && skipped.is_none()`. Dart's
`BatchOutcome.ok` is `error == null`.

Fix the drift, and — the part that matters more — **make the next drift fail the
gate** instead of producing a note.

## Read first

- `docs/AUDIT.md` findings **3**, **4**, **5** and **13**. Each has a `file:line`
  and a table row.
- `core/src/ffi.rs:232-283`, `:426-455` — the two request and response envelopes.
- `core/src/worker.rs:103-134`, `:249-291` — `SkipReason` and `Outcome`.
- `AGENTS.md`, app section rule 4: *"If you change a request, response or enum
  shape in `core/src/ffi.rs`, update `app/lib/rust/models.dart` in the same phase
  and extend the contract tests on both sides."* Five phases have not.
- `scripts/check-dart-bindings.sh` — it checks the **C ABI**, not the JSON. It
  reports "15 entry points match" while five phases of fields are missing.

## The delivery constraint, stated up front

`app/` is the `authorss81/shrinkray` submodule. This pipeline can read it and
cannot push to it — `git -C app push` returns 403. Every phase from 05 onward has
therefore delivered its `app/` half as a patch under `workspace/<phase>/`.

Do the same. But note what the audit found about the existing patches:

- `shrinkray-phase-06`, `-07`, `-13`, `-14` all apply cleanly to `5d0e9cc`.
- **`shrinkray-phase-05` applies only 6 of its 9 files** — the submodule moved
  past it, to a commit that *is* the `px_inspect` arity fix this patch contains.
- phase-07, phase-13 and phase-14 collide in `Capabilities.fromJson`,
  `ProcessResult.fromJson` and `BatchOutcome.fromJson`, so they do not stack.
- **phase-12 has no patch at all**, which is why the colour fields have no Dart
  model.

So: land the accumulated drift **as one consolidated patch** rather than as a
fifth one to stack. Take the four that apply, add phase-12's missing pieces, and
resolve the collisions by hand. Write down in `docs/phase-status.md` what you
superseded, so nobody tries to apply a superseded patch later.

## Scope

### Do

**The crash**

- Add `heic` and `heif` to the Dart `OutputFormat` enum. Phase-06's patch does
  this and applies cleanly.
- **Then stop relying on `byName` for a wire format.** An enum that throws on an
  unrecognised name is a crash waiting for the next variant, and the engine
  gained two read-only variants in one phase. Parse it into an explicit
  `unknown` case or throw a message that names the engine's version, not
  `ArgumentError: No element was found with the given values`. Whichever you
  pick, a **new engine format must not be able to crash an old app.**

**The skipped files**

- Add `SkipReason` and `BatchPolicy` to `models.dart`, add `skipped` to
  `BatchOutcome`, and make `BatchOutcome.ok` mean what `worker.rs:289` means.
- Extend the contract test so a batch report containing a `Duplicate` skip is
  counted as skipped. `app/test/models_test.dart` already counts successes;
  this is the other half.

**The five phases of missing fields**

Everything in the `docs/AUDIT.md` finding-5 table: `colour` on
`ValidateReport`, `ExifInfo` and `Outcome`; `animation` on `Outcome`;
`chroma_subsampling`; `progressive`; the three `ColourOptions`; the
`AnimationPolicy`; `frames` and `frames_truncated`; and the nine
`Capabilities` keys `models.dart:408-417` does not read.

**The leak** — finding 13. `ffi.rs:82-91` puts the message in `PxBuffer::error`
via `CString::into_raw`. `px_buffer_free` frees only `data`. `px_string_free` is
declared at `app/lib/rust/bindings.dart:116` and **called from nowhere**. Every
failed FFI call leaks a `CString` permanently. Call it in the `finally` that
already frees the data.

**And the length** — `app/lib/rust/engine.dart:191-218` passes `request.length`
(Dart **UTF-16** code units) where the byte length of `utf8.encode(request)` is
meant. Today it is an under-read, not an over-read, so it fails as a JSON syntax
error on any non-ASCII filename. It is one encoding change away from being an
out-of-bounds read. Compute the encoded bytes once and pass their length.

### The check — this is the actual deliverable

Write a script, or a Rust test, that compares **the engine's `#[serde]` field
names** against `app/lib/rust/models.dart` and fails on a field the Dart side
does not mention. `scripts/check-dart-bindings.sh` already proves the C ABI at
compile time; this proves the JSON at whatever mechanism is reliable.

It must be a **failure**, not a note. `scripts/verify.sh:324-331` deliberately
downgrades binding drift to a note because `app/` is a separate repository — and
that decision has now cost five phases of silent drift. Find a different
mechanism: a check that compares a generated list of field names against a
committed list, where the *committed* side is this repository's and therefore
landable. That is the trick — the check can live here even when the fix cannot.

Prove it can fail by adding a field to an engine struct and showing the check
goes red. `verify.sh`'s own rule applies: a check that has never been observed to
fail is not known to work.

### Do not

- Do not add a network capability. Nothing here needs one.
- Do not change an engine-side field name to make the Dart side match. The engine
  is the contract; Dart catches up.
- Do not delete the note in `verify.sh` section 3b without replacing it with
  something that fails. If you cannot replace it, say so in
  `docs/phase-status.md` and leave the note alone.
- Do not refactor the Dart beyond this scope. `main.dart` is 80 lines and the UI
  phases start at 22; this phase is the contract, not the app.

## Acceptance criteria

### Machine-checkable

- The new contract check is wired into `scripts/verify.sh` as a **failure**, and
  is demonstrated to fail on a deliberately added field. Paste both runs.
- `bash scripts/check-dart-bindings.sh` still reports 15 entry points matching.
- The consolidated patch applies cleanly to `app/` at `5d0e9cc`, and the
  superseded patches are named in `docs/phase-status.md`. Paste
  `git apply --check` output.
- After applying it, `cd app && dart format --output=none --set-exit-if-changed
  lib test` reports only `lib/rust/engine.dart` (the one allowlisted upstream
  file), `flutter analyze` reports zero issues, and `flutter test` passes with
  **none skipped**.
- A Dart test asserts `BatchOutcome.ok` is false for a report carrying a
  `Duplicate` skip, and a second asserts a format string the Dart enum does not
  know does not throw an unhandled `ArgumentError`.
- This phase changes **no** engine-side JSON field. If you find yourself needing
  one, stop and write that down instead.
- `bash scripts/verify.sh` exits 0.

**Build check.** `app/` changes are not built by `verify.sh`. State plainly in
the commit message that `flutter build apk --release` and `flutter build
windows --release` are run by the `android-apk` and `windows-exe` jobs in
`.github/workflows/build.yml` and could not be run on this Linux runner (no
Android SDK, no MSVC), and that `flutter analyze` plus `flutter test` against a
loaded engine are the local substitute. Do not claim a build you did not run.

### Needs a human judgement

- Is `unknown`-or-throw the right answer for an unrecognised format, or should
  the app refuse to start when its Dart models are older than the engine? Both
  are defensible; say which you built and why.
- Would a user understand a `WouldUpscale` skip? Read `SkipReason::note()` aloud
  for all five variants.

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha.
- Committed and pushed to `main`. Never force-push.
- Do not write `.done`. The workflow does.
