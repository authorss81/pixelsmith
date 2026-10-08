# phase-23 — Track B: the preview canvas and the before/after view

**Track:** B — Flutter UI
**Depends on:** phase-22
**Timeout:** 90 minutes

## Objective

Build the screen users actually judge a resizer on: **a preview canvas with a
draggable before/after split, synced zoom and pan, and a magnifier loupe** — plus
the zoomable checkerboard the transparency needs.

`ROADMAP.md` calls the before/after slider "the centrepiece". `docs/AUDIT.md`
lists it `ABSENT`, along with the checkerboard, the result card and the
non-destructive live preview.

Phase-22 built the frame. This phase fills the most important hole in it.

## Read first

- `ROADMAP.md`, "Visual / UI" — the five items this phase covers, quoted below.
- `docs/AUDIT.md`, "Visual / UI" classification.
- `app/lib/rust/models.dart` — `ValidateReport`, `Capabilities`, `OutputFormat`.
  Phase-20 brought these up to date.
- `core/src/pipeline.rs:246-268` — `output_dimensions`, which exists precisely so
  a UI can show "1920 × 1080" while a slider is being dragged. **This phase does
  not have a "predict size" FFI entry point**, so decide in this phase how the UI
  gets one, and record the decision.
- Phase-22's theme and shell.

## The delivery constraint

`app/` cannot be pushed to. Deliver the Dart as a patch under
`workspace/phase-23/`, applied to the phase-22 result, run through `dart format`,
`flutter analyze` and `flutter test` before capture. If phase-22's patch has not
landed upstream, this patch is rooted on top of it — say which, in
`docs/phase-status.md`.

## Scope

### The canvas

- Render a decoded image with **zoom, pan, fit-to-window and 1:1**, and a
  **zoomable checkerboard** behind it. Zoomable, not a fixed 8-pixel tile — a
  checkerboard that does not scale with the image is a lie about the pixel grid.
- The checkerboard must be **light and dark aware**, and it must stay subtle at
  every zoom level.
- Handle a very large image without janking. `DecodeImageFromList` or an
  `instantiateImageCodec` with a target size; **do not** put a 120 MP bitmap in
  a widget tree.

### Before/after

- A **draggable split view** with **synced zoom and pan**: both sides stay
  registered to each other while dragging, and while zooming. Desync here is the
  single most common way these get built wrong, because it is invisible until a
  user drags the split.
- A **magnifier loupe** at the cursor for pixel-peeping edges — because the whole
  point of the comparison is deciding whether an edge survived, which you cannot
  do at fit-to-window scale.
- Handle: a source with alpha, a source with no alpha, a 1×1 image, an image
  smaller than the viewport, and a source that is portrait when the viewport is
  landscape.

### The result card

- Shown **live while any slider is dragged**: output dimensions, **estimated**
  bytes, and a savings bar.
- **Estimate honestly.** Say it is an estimate. If the number comes from the
  engine's own `target::TargetBytes`, say so; if it is a heuristic from the
  source's bytes-per-pixel, say that instead. A live number the user later finds
  is wrong on the real export is worse than no number.
- Dimensions come from `output_dimensions` semantics: **"1920 × 1080 (from 4000 ×
  3000, fit inside)"**, per `ROADMAP.md`. When `no_upscale` clamps it, the card
  has to say so rather than showing the clamped number as though it was asked
  for.

### Non-destructive live preview

- While dragging, preview from a **downscaled proxy**; on release, run the real
  thing at full resolution. The roadmap's exact words.
- The proxy must be visibly a proxy. Nothing in this phase should make the user
  wait on a full-resolution render while dragging.

### Do not

- Do not call `px_process` to make a preview. There is no resize-only, encode-
  free preview entry point in the FFI today, and adding one is an engine change
  with its own contract consequences. **Either** preview the source image and
  show the target geometry as an overlay (honest, cheap, and what phase-24 can
  refine), **or** propose the engine entry point in `docs/phase-status.md` and
  do not implement it here. Say which you built and why.
- Do not add a package for gestures, matrices or the checkerboard. Flutter's
  `InteractiveViewer` and a `CustomPainter` cover this.
- Do not build the pipeline editor. phase-24 owns the controls; this phase owns
  the picture. They meet at the result card, and you may stub its inputs.
- Do not localise.

## Acceptance criteria

### Machine-checkable

- The patch applies cleanly and `dart format`, `flutter analyze` (zero issues)
  and `flutter test` (none skipped) all pass on the result.
- Widget tests cover: split drag at three positions; zoom at three levels with
  the two sides asserted to stay in registration; fit-to-window and 1:1 on a
  portrait source and a landscape one; a 1×1 source; and an RGBA source over a
  checkerboard that is asserted to be **two alternating colours of equal count**
  at three zoom levels.
- A test asserts the checkerboard scales: the tile size in logical pixels at 4×
  zoom is four times its size at 1×. A non-scaling checkerboard fails this.
- A golden or property test asserts **zoom and pan stay in sync** across the
  split after an arbitrary sequence of drag, zoom and pan gestures.
- `grep -rn 'print(' app/lib` returns nothing outside the logger.
- **Build check.** State in the commit message that `flutter build apk --release`
  and `flutter build windows --release` are the `android-apk` and `windows-exe`
  jobs in `.github/workflows/build.yml`, that this runner cannot run them (no
  Android SDK, no MSVC), and that `flutter analyze` plus `flutter test` against
  a loaded engine are the local substitute. This phase adds no `px_*` call and no
  engine change, so say that too.
- `bash scripts/verify.sh` exits 0.

### Needs a human judgement

- Open the app with a real photograph. Is the split drag comfortable with a
  trackpad and with a finger? Is the loupe readable, or is it a smear?
- Does the result card's estimate survive contact with a real export, or does it
  need to say something more modest?
- Is the checkerboard subtle enough to judge an edge against, or does it compete
  with the image?

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha, and names
  the patch and what landing it requires.
- Committed and pushed to `main`. Never force-push.
- Do not write `.done`. The workflow does.
