# phase-25 — Track B: live progress, honest cancellation, the batch list

**Track:** B — Flutter UI
**Depends on:** phase-24
**Timeout:** 90 minutes

## Objective

`ROADMAP.md` puts live progress and honest errors first in "Highest leverage,
first", and `docs/AUDIT.md` scores **all eight** items in that category `ABSENT`
or `PARTIAL`. This is the largest single block of missing product in the project.

The engine already has most of the data. `BatchReport` reports `succeeded()`,
`skipped()` and `failed()`, and those three add up. `SkipReason::note()` is
written for a person. `CancelToken` works. What does not exist is anything that
consumes them, or any progress at all while a batch runs.

## Read first

- `ROADMAP.md`, "Live progress" — all eight items, quoted below.
- `core/src/worker.rs:249-291` (`Outcome`, `BatchReport`), `:103-134`
  (`SkipReason`), `:367-460` (`process_all`, the cancellation points).
- `core/src/ffi.rs:385-425` — `px_cancel_new`, `px_cancel_trigger`,
  `px_cancel_free`. `CancelToken` is a `u64` handle with a 1024-token cap.
- `docs/phase-status.md`, phase-14 — why a skip is a skip and why
  `would_upscale` is one of them.
- `AGENTS.md` hard rule 9.

## The delivery constraint

Deliver as a patch under `workspace/phase-25/`, applied to the phase-24 result,
run through `dart format`, `flutter analyze` and `flutter test` before capture.
Name the base commit in `docs/phase-status.md`.

## Scope

### Progress that is determinate

- A **progress bar with real `done / total`**, not an indeterminate spinner.
  `BatchReport` gives both.
- **Per-file rows with individual state.** The engine reports the *terminal*
  state per file (`Outcome`). It does **not** report `queued → decoding →
  resizing → encoding` — those are internal to `process_one` and never surface.
  So either derive the intermediate states from what the engine does expose, or
  state honestly that only the terminal state is known. **Do not animate a
  progress bar that is guessing.** Whichever you build, say so in
  `docs/phase-status.md`; if it needs an engine change, propose it there.
- **Throughput readout**: images per second, ETA, elapsed. Derivable from
  completions over time. **Peak memory is not** — nothing measures it in
  production, only in `core/tests/streaming_peak.rs`. If you cannot measure it,
  leave it out rather than inventing a number.
- A **live savings counter**: "12.4 MB saved so far". `Outcome` carries
  `input_bytes` and `output_bytes` per file, so the running total is exact.

### Cancellation that is instant and honest

- **No half-written output files.** This is the one to be careful about. Read
  `worker::process_all` and find where cancellation is checked; if a file can be
  interrupted between opening the output and finishing it, that is a defect and
  it belongs in `docs/AUDIT.md`.
- **The UI says how many finished before stopping.** It must not say "cancelled"
  and imply nothing happened.
- The cancel control must be responsive even when a single file is taking
  seconds. If `process_all` only checks between files, say so in the UI and in
  `docs/phase-status.md` — do not pretend the button is instant.

### Errors a user can act on

- **An error log per file, expandable to the underlying decoder message.** Hard
  rule 9: the engine's sentence is the top line. The decoder's own text is the
  detail. Both must be reachable.
- **Toast plus inline status for every action.** Nothing completes silently.
- A skipped file is shown **as skipped, with its reason**, in the same list as
  the failures. A 64×64 icon in a folder of 12 MP photographs is not an error,
  and putting it in the error list would tell the user to fix their folder.
  phase-20's `BatchOutcome.ok` fix is what makes this possible — build on it.

### Background processing — decide, and record

`ROADMAP.md` asks for a persistent Android notification carrying progress and a
cancel action, plus a WorkManager job for large batches. That is real platform
work. **Decide in this phase whether it is in scope**, and if it is not, write
one paragraph in `docs/phase-status.md` saying what it would take. Do not
half-build it.

### Do not

- Do not fake determinate progress. If the engine cannot report intra-file
  progress, the honest bar is "files completed" and it should say so on screen.
- Do not report a skip as a failure, or a failure as a skip. `Outcome::ok()` is
  `error.is_none() && skipped.is_none()` and the UI must agree.
- Do not swallow the engine's error sentence and show your own. The engine's is
  the one hard rule 9 governs.
- Do not build the export flow. phase-26 owns that.
- Do not add a state-management package without saying why in the commit message.

## Acceptance criteria

### Machine-checkable

- The patch applies cleanly; `dart format`, `flutter analyze` (zero issues) and
  `flutter test` (none skipped) pass on the result.
- A test drives a batch through `px_batch` with a known set of files and asserts
  the progress values are monotonic, end at `total`, and that the three counts
  shown **add up to the number of inputs** — including the skipped ones. This is
  the assertion that catches the phase-14 arithmetic being re-broken in the UI.
- A test cancels mid-batch and asserts: the run stops, the report is returned,
  the UI states **how many finished**, and no outcome is reported as neither
  succeeded, skipped nor failed.
- A test asserts a `Duplicate` and a `WouldUpscale` skip render in the skipped
  state with `SkipReason::note()` verbatim, **not** in the error state.
- A test asserts every error row expands to the engine's full sentence and that
  the top line is non-empty for all five `SkipReason` variants and every
  `Error` variant the batch path can produce.
- `grep -rn 'print(' app/lib` returns nothing outside the logger.
- **Build check.** `flutter build apk --release` and `flutter build windows
  --release` are the `android-apk` and `windows-exe` jobs in
  `.github/workflows/build.yml`; this runner cannot run them. **This phase is the
  first to call `px_batch` and the `px_cancel_*` family**, so name that
  explicitly, along with the engine-backed reason it cannot be built locally.
- `bash scripts/verify.sh` exits 0.

### Needs a human judgement

- Start a 400-image batch and cancel it halfway. Does the app tell you the truth?
  Does anything on disk need cleaning up?
- Is there a moment during a run where you would not know whether it had hung?
- Did you have to invent a number anywhere? If so, remove it.

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha, names the
  patch and what landing it requires, and records your background-processing
  decision.
- Committed and pushed to `main`. Never force-push.
- Do not write `.done`. The workflow does.
