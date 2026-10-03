# phase-06 — HEIC/HEIF decode

**Track:** A — Engine & Security
**Depends on:** phase-05
**Timeout:** 90 minutes

## Objective

Add HEIC/HEIF decode. Every competitor in this space claims support and none of them deliver it; iPhone photos are the single most common input a user will actually pick. This is the highest-visibility gap in the product.

## Read first

- `AGENTS.md` — hard rule 1. Adding a codec must not add a network capability.
- `core/src/format.rs` — `detect_format`, the `OutputFormat` enum, and how decode support differs from encode support.
- `core/src/validate.rs` — where a new format plugs into `validate_bytes`.

## Scope

### Do

- Evaluate the decode options and record the decision in `docs/HEIC.md` with a comparison: `libheif-rs` (libheif, HEVC, widest device support), `heic-decode`/`heic` (pure Rust, narrower), `imagepipe` + `libheif`. State licence, build complexity per target, and whether each can be built for Android and iOS from the same source.
- Pick one and implement it behind a feature flag named `heic`. Do not make it default until it builds on all four shipped targets.
- Register HEIC and HEIF in `detect_format` so magic-byte detection recognises them. Verify the container brand bytes are checked, not just the extension.
- Add HEIC to `validate_bytes` so it is bounded by `Limits` exactly like every other format.
- Wire it into `OutputFormat` as **read-only**. Encoding HEIC is out of scope; return a clear "read-only" error rather than pretending.
- Add tests: a synthetic HEIC round-trips through decode; a truncated HEIC returns a specific error rather than a panic; a HEIC declaring absurd dimensions is rejected by `Limits` before allocation.
- Update `capabilities()` to report HEIC decode support so the UI can show it honestly.

### Do not

- Do not add HEIC as a default feature until phase-08 proves it builds on every target.
- Do not accept a crate that pulls a network stack in transitively. Check the dependency tree with `cargo tree` and paste the relevant part into `docs/HEIC.md`.
- Do not widen `Limits` to accommodate the codec.

## Acceptance criteria

### Machine-checkable

- `cargo test --features heic --all-features` passes and includes HEIC decode tests.
- `grep -E 'reqwest|hyper|tokio|std::net' <the new dependency tree>` returns nothing.
- `detect_format` recognises a real `.heic` file by its container brand bytes, not by filename.
- A truncated HEIC returns an `Err`, not a panic. A test asserts this.

### Needs a human judgement

- Is `docs/HEIC.md` an honest comparison, or does it read as a justification after the fact?
- Would a user who picks an iPhone photo and hits an unsupported secondary-image format understand what happened?

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha.
- Committed and pushed to `main`.

Do not write `.done`. The workflow does.