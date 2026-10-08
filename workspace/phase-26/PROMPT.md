# phase-26 — Track B: the export flows and the folder plan

**Track:** B — Flutter UI
**Depends on:** phase-25
**Timeout:** 90 minutes

## Objective

The last screen a user meets: **where the files go, and what happens when two of
them are the same photograph.**

Phase-14 built `folder::plan` — a preview of a whole folder that reads 4 KiB a
file and returns a per-file verdict, in about the time it takes to read a
directory listing. **Nothing has ever called it**, and there is no FFI entry point
for it. `docs/AUDIT.md` records that as an outstanding gap in phase-14's own
notes.

So this phase does two things: it builds the export UI, and it adds the one FFI
entry point the engine is missing.

## Read first

- `core/src/folder.rs:1-140` — `FolderPlan`, `FolderEntry`, `EntryVerdict`.
- `core/src/worker.rs:103-134` (`SkipReason`), `:367-460` (`process_batch`,
  `zip_outputs`), `:960-975`.
- `docs/ARCHITECTURE.md`, "Folders: the plan, the duplicates, and the skips" and
  "`would_upscale`: a skip, argued". Read the argument before designing the UI;
  it settles three questions this phase will otherwise re-litigate.
- `docs/AUDIT.md`, phase-14's "Not done, and named" list.
- `core/src/ffi.rs:467-546` — `px_batch` and `px_zip`, and `ffi_abi.rs`, which
  declares every entry point **once** and proves it against `ffi.rs` at compile
  time.

## The delivery constraint

This phase **changes the engine's FFI surface**, so it has two halves: the engine
change lands in this repository, and the Dart half is a patch under
`workspace/phase-26/`.

Per `AGENTS.md` app rule 4, the Dart side goes in the same phase and the
contract tests go on both sides. Do not skip the Dart half because it is a patch.

## Scope

### The engine change — one entry point

- **A `px_plan` entry point** taking a JSON list of paths and returning the
  `FolderPlan` as JSON, so the UI can show a folder preview before the user
  commits to an export.
- Register it in **`ffi_abi.rs`** — that is what makes a signature mismatch a
  build failure rather than a wrong call — and update
  `scripts/check-dart-bindings.sh`'s expected count from 15 to 16.
- It reads paths, not file contents, so it needs its own bounds: a maximum entry
  count, a maximum path length, and a **per-platform path-separator rule** before
  anything is opened. `worker::sanitise_stem` sanitises *output* names; an input
  path arrives from Dart and is used to open a file, so it needs its own
  thinking. **Say in the commit message what an input path can and cannot do.**
- Respect `MAX_DEPTH` (8) and `MAX_ENTRIES` (10 000) and report having stopped,
  the way `folder::plan` already does. Do not truncate quietly.

### The export UI

- **Save to folder**, **save next to the original**, and **ZIP** — the three
  destinations the roadmap names. `px_zip` exists; the other two are
  `px_batch` with a destination.
- **Sensible filename handling**: increment rather than overwrite
  (`photo-2.jpg`), with a **configurable template**. The engine sanitises the
  stem; the *increment* is a decision that needs a rule stated: is it based on
  what is on disk, or on what this run has already written? Pick one, and handle
  the collision rather than assuming there is none.
- The **folder plan before the button is worth pressing**: `Ready`, `NotListed`,
  `NotAnImage`, `TooLarge`, in the engine's own words.
  **`NotListed` is not a skip** — `docs/ARCHITECTURE.md` explains why a folder
  holds three hundred `.DS_Store` files and reporting them as skipped would bury
  the thirty lines that matter. The UI must show them in the plan and leave them
  out of the report.
- **The duplicates.** `dedupe::ContentKey` merges two byte-different encodings of
  one picture and refuses to merge a lossy re-encode. `SkipReason::Duplicate { of }`
  names the file that won. **The UI must not claim which file was written** —
  that is the scheduler's choice and gotcha 32 exists because two tests got it
  wrong. Show the reason; do not reorder around it.
- **The would-upscale skips** are the hardest thing on this screen. 30 of 400
  files is a batch that quietly failed to equalise. Say so plainly, and offer
  `BatchPolicy.skip_upscales = false` as the other answer, in one control.
- **"Copy diagnostics"** — the roadmap asks for a version and capability dump
  with **no file contents**. `px_version()` returns exactly that. This is a
  twenty-line screen and it is on the trust-and-polish list.

### Do not

- Do not add a network capability. Nothing here needs one, and the ZIP is local.
- Do not let the UI compute a filename the engine would sanitise differently.
  `worker::sanitise_stem` is the contract; increment in a way that survives it.
- Do not claim a duplicate was "removed". It was skipped, and the report says so.
- Do not follow symlinks or read outside what the user picked.
- Do not re-derive what `folder::plan` already computes. The UI renders a verdict;
  it does not re-judge a file.

## Acceptance criteria

### Machine-checkable

- `ffi_abi.rs` declares `px_plan`, and **adding a mismatched declaration fails to
  compile** — prove it by writing the wrong signature, pasting the error, and
  reverting.
- `bash scripts/check-dart-bindings.sh` reports 16 entry points matching.
- A test asserts `px_plan` refuses a path over the length limit, an entry list
  over the count limit, and a traversal sequence, each with a sentence naming the
  limit.
- A test asserts `px_plan` reports `EntryVerdict::NotListed` for an unlisted
  extension and that such an entry is **absent from the processable count**.
- A test asserts a folder containing a duplicate produces one output and one
  `Duplicate { of }` skip naming a file **that has bytes written**, and that the
  assertion does not assume *which* of the two won.
- A test asserts the increment rule: two inputs that would produce the same
  output name produce `name-1` and `name-2`, both written, neither overwritten.
- "Copy diagnostics" is asserted to produce `px_version()`'s output verbatim and
  to contain **no** filename, path, dimension or byte count of any input.
- The Dart patch applies cleanly; `dart format`, `flutter analyze` (zero issues)
  and `flutter test` (none skipped) pass.
- **Build check.** This phase adds a `px_*` entry point and changes the Dart
  model's use of the ABI, so both builds are affected in principle. `flutter
  build apk --release` and `flutter build windows --release` are the `android-apk`
  and `windows-exe` jobs in `.github/workflows/build.yml`; this Linux runner has
  neither an Android SDK nor MSVC. **State that, and name what you ran instead**
  — `cargo test --all-features`, `check-dart-bindings.sh`, and `flutter test`
  against a loaded engine. Do not claim a build you did not run.
- `bash scripts/verify.sh` exits 0.

### Needs a human judgement

- Run a 400-image folder with 30 duplicates and 30 tiny icons. Does the summary
  read as a number you can act on, or as a wall?
- Would you understand why `nul.jpg` was renamed? Read `sanitise_stem`'s
  behaviour aloud.
- Is "copy diagnostics" reassuring? It is the feature that turns a bug report
  into a good one.

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha, and names
  the Dart patch and what landing it requires.
- Committed and pushed to `main`. Never force-push.
- Do not write `.done`. The workflow does.
