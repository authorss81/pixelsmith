# phase-12 — Colour management: sRGB, Display-P3 and ICC

**Track:** A — Engine & Security
**Depends on:** phase-11
**Timeout:** 90 minutes

## Objective

Handle colour correctly. Right now an untagged file is silently assumed to be sRGB and an embedded ICC profile is dropped without being applied, so a photo shot on a wide-gamut phone comes out with the wrong colours. Every resizer gets this wrong; it is a differentiator.

## Read first

- `AGENTS.md` — hard rule 6. Metadata stripping must not become colour destruction by accident.
- `core/src/exif.rs` — why stripping is done by re-encoding.

## Scope

### Do

- Read and surface the ICC profile in `ValidateReport` and `ExifInfo`: its presence, its colour space, and whether it is sRGB.
- Define the conversion path: source space → working space → output space, with working space sRGB by default. State the conversion matrix assumption rather than claiming full ICC fidelity.
- Implement at minimum: sRGB in, sRGB out; Display-P3 in, sRGB out; untagged in, assumed sRGB out. Convert with correct transfer functions — the sRGB curve is not a straight gamma 2.2, and getting that wrong is the most common bug in this whole area.
- Decide and document what happens to the profile on output. Converting to sRGB and dropping the profile is correct for the overwhelmingly common case. Say so, and offer profile embedding as an explicit option rather than a default.
- Handle the interaction with metadata stripping honestly: stripping the ICC profile while keeping the original pixel values changes the meaning of the image. Make the pipeline convert when it strips, and test that a P3-tagged file exported as sRGB actually has different pixel values than the naive path.
- Add a test matrix over the three source spaces, asserting colour shift in the expected direction using a saturated primary patch. "The output is not identical" is a weak assertion; assert the direction and rough magnitude.
- Record the decision in `docs/ARCHITECTURE.md`.

### Do not

- Do not claim full ICC conformance. Say precisely what is implemented and what is approximated.
- Do not apply a colour transform when the source is already untagged sRGB; it must be a no-op, and a test must prove it.
- Do not drop a profile without converting, unless the user explicitly asked to keep raw pixels.

## Acceptance criteria

### Machine-checkable

- A test matrix covers untagged, sRGB-tagged and Display-P3-tagged input.
- A test asserts an untagged source is byte-identical after a no-op conversion.
- A test asserts P3 → sRGB shifts a saturated red patch in the expected direction by a stated amount.
- `ValidateReport` exposes ICC presence and colour space.

### Needs a human judgement

- Would a photographer look at the P3 → sRGB output and call it correct?
- Is the documented approximation honest, or does it oversell?

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha.
- Committed and pushed to `main`.

Do not write `.done`. The workflow does.