# phase-01 — Verification baseline and project scaffolding

**Track:** A — Engine & Security
**Depends on:** none
**Timeout:** 60 minutes

## Objective

Make the automated verification gate green on day one, and give the repository the configuration and documentation every later phase depends on. Nothing else. This phase is infrastructure, not features.

## Read first

- `AGENTS.md` — the hard rules. Every one of them is checked.
- `docs/phase-status.md` — what the pipeline thinks has happened.
- `core/src/` — the files you are about to touch. Read before changing.

## Scope

### Do

- Create `rustfmt.toml` with an explicit `edition = "2024"` and `max_width = 100`, plus a comment on why the width is not the default.
- Create `deny.toml` permitting only licences that keep the project permissively distributable. Start from: `MIT`, `Apache-2.0`, `Apache-2.0 WITH LLVM-exception`, `BSD-2-Clause`, `BSD-3-Clause`, `ISC`, `Unicode-3.0`, `Zlib`, `CC0-1.0`. Explicitly deny `GPL-2.0`, `GPL-3.0`, `AGPL-3.0`, and anything unknown. Add an `advisories` section denying unmaintained crates and a `bans` section forbidding duplicate versions of the same crate.
- Create `docs/ARCHITECTURE.md`: a module map for `core/src` with one line per module saying what it owns; the fixed crop → orient → resize order and why; the `Limits` enforcement points; the FFI entry points and their buffer-ownership rules; the feature flags. Include a **Gotchas** section listing at least these four, each pointing at the file that handles it:
  1. The six-byte `Exif\0\0` identifier is mandatory at the start of an APP1 segment — readers will not find the metadata without it.
  2. EXIF orientation value 6 means rotate 90° **clockwise**.
  3. WebP output is lossless unless the `webp-lossy` feature is enabled, which means no quality control and no byte ceiling.
  4. `image::imageops::resize` returns an `ImageBuffer`, not a `DynamicImage`.
- Create `docs/phase-status.md`: a table with columns Phase, Title, Status, Commit, Notes. One row per phase directory.
- Create `workspace/PHASES.md`: the ordered phase table with a Track column and a one-line description each.
- Fix any `scripts/verify.sh` check that fails for a reason unrelated to a real defect. If a check is genuinely wrong, fix the check and record why in `docs/phase-status.md`. If the code is wrong, fix the code.
- Add `core/CHANGELOG.md` with a `0.1.0` entry describing what exists today.

### Do not

- Do not add a feature. Do not add a dependency. Do not restructure existing modules.
- Do not weaken a verify check to make it pass. If a check is wrong, argue it in `docs/phase-status.md` and change it there in the open.

## Acceptance criteria

### Machine-checkable

`bash scripts/verify.sh` must exit 0. Beyond that:

- `rustfmt.toml`, `deny.toml`, `docs/ARCHITECTURE.md`, `docs/phase-status.md`, `workspace/PHASES.md` and `core/CHANGELOG.md` all exist.
- `docs/ARCHITECTURE.md` has a Gotchas section with at least four entries, each naming a specific trap and naming the file where it is handled.
- The phase-status table has exactly one row per phase directory, and the row count equals `ls workspace | grep -c phase-`.

### Needs a human judgement

- Would a reviewer reading only this diff agree the phase is complete?
- Is anything here the objective did not ask for?
- Is any acceptance criterion satisfied by a check that would also pass against a broken implementation?

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha.
- Committed and pushed to `main`. Never force-push.

Do not write `.done`. The workflow does, and only after verification passes.