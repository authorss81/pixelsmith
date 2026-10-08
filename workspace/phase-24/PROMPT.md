# phase-24 — Track B: the pipeline editor

**Track:** B — Flutter UI
**Depends on:** phase-23
**Timeout:** 90 minutes

## Objective

Turn the engine into a UI. Every control the engine already exposes, presented so
that a person who does not know what a chroma sampling factor is can still make
a good decision — and told the truth when the build cannot do what they asked.

This is the phase where `app/lib/rust/models.dart` stops being a contract and
becomes an interface. Phase-20 made the models match the engine; nothing has ever
rendered them.

## Read first

- `ROADMAP.md`, "More features" and "User-friendliness".
- `core/src/presets.rs` — 41 presets, each with a `category`, and each carrying
  its own `chroma` default.
- `core/src/pipeline.rs:173-210` — the `Pipeline` the UI must produce, field for
  field.
- `core/src/worker.rs:103-134` (`SkipReason`), `:213-224` (`BatchPolicy`),
  `:249-291` (`Outcome`) — for the warnings this phase has to show.
- `core/src/error.rs` — every variant is a sentence a user will read, and hard
  rule 9 says they are the product.
- `app/lib/rust/models.dart` — phase-20's version. Every type here is one you can
  build a control from.
- `core/src/target.rs` and `format.rs:detect_format` — for the capability greying.

## The delivery constraint

Deliver as a patch under `workspace/phase-24/`, applied to the phase-23 result,
run through `dart format`, `flutter analyze` and `flutter test` before capture.
Name the base commit in `docs/phase-status.md`.

## Scope

### Presets

- **Preset chips grouped by category** — Social / Web / Print / Device / Email /
  Dev — **searchable**, with **recently-used pinned**.
- Every preset must round-trip: picking one fills the controls, and the filled
  controls must produce the same `Pipeline` the preset's `to_pipeline()` returns.
  Assert it for **all 41**, not a sample.

### The controls

Every field on `Pipeline`, and nothing invented:

| Control | Backing field | The honesty work |
| --- | --- | --- |
| Output size | `ResizeSpec`, `FitMode` | Five fit modes. Show "1920 × 1080 (from 4000 × 3000, fit inside)". When `no_upscale` clamps it, **say so** — do not show the clamped number as though it was asked for. |
| Crop | `CropSpec` | Free-form and locked to the target ratio. Draggable handles and a 3×3 grid on the phase-23 canvas. Live crop bounds feedback; the engine refuses a crop that runs past the edge, so the UI should not offer one. |
| Orientation | `Orientation` | Auto from EXIF, with a **manual override**, and live rotate/flip on the canvas. |
| Format | `OutputFormat` | **Grey out what the build cannot write.** `Capabilities` decides this. A format that is read-only is not offered as an output at all. |
| Quality | `EncodingOptions.quality` | Only for formats that honour it — `supports_quality()`. |
| Chroma | `ChromaSubsampling` | Three values. `ChromaSubsampling::trade_off()` is written as tooltip text; use it verbatim. **This trades colour detail, not sharpness** — say that in the UI, because "subsampling" sounds like a resolution setting. |
| Progressive | `EncodingOptions.progressive` | JPEG only, and it costs about 64% more bytes. Show that. |
| Byte target | `TargetBytes` | Only where `supports_byte_target()`. A ceiling on a format with no quality setting is **refused by the engine**, so the control must not offer one. |
| Metadata | `strip_metadata` | Strip by default, per hard rule 6 and `ROADMAP.md`. |
| Colour | `ColourOptions` | Three answers: convert to sRGB, keep source pixels, embed profile. "Display-P3 — this will be converted to sRGB" comes from `ValidateReport.colour`. |

### Zero-config default

`ROADMAP.md`: *"pick a photo, get a sensible resized output. Advanced controls
stay collapsed."* Pick a default by intent — and the engine has 41 presets
grouped by intent. Advanced controls start collapsed and the default is visible.

### Explain every number

Every number on screen says what it came from. **Estimated bytes must be labelled
an estimate.** A skipped or refused file says why, in the engine's own sentence.

### Do not

- Do not invent a control the engine cannot honour. Every knob here maps to a
  field on `Pipeline` or `Settings`; if you want a new one, that is an engine
  phase.
- Do not compute a dimension in Dart when the engine has an opinion. The engine's
  arithmetic is the contract, and phase-23 flagged the absence of a
  predict-dimensions entry point — if you need one, record it in
  `docs/phase-status.md` rather than reimplementing it.
- Do not build the batch list or the export flow. phase-25 and phase-26 own those.
- Do not localise yet, but keep user-visible strings in one place per screen.

## Acceptance criteria

### Machine-checkable

- The patch applies cleanly; `dart format`, `flutter analyze` (zero issues) and
  `flutter test` (none skipped) pass on the result.
- A test asserts **all 41 presets round-trip**: selecting a preset produces the
  `Pipeline` its `to_pipeline()` returns, serialising both through
  `Requests.process` and comparing the JSON.
- A test asserts the format picker **greys out exactly** what
  `Capabilities` says cannot be written, for both `avif` on and off and for
  `heic_decode` on and off. `format.rs`'s
  `webp_reports_the_truth_about_this_build` is the model.
- A test asserts the quality slider, the chroma control, the progressive toggle
  and the byte-target field are **present only for formats that honour them**,
  driven by the same `Capabilities` the engine serialises.
- A test asserts the size field renders `1920 × 1080 (from 4000 × 3000, fit
  inside)` for a width-fit request, and that a `no_upscale` clamp is stated in
  the text rather than silently applied.
- A test asserts a `Suspect`-free round trip: every control state serialises to a
  JSON body the engine accepts, checked against `Requests.process`'s shape.
- `grep -rn 'print(' app/lib` returns nothing outside the logger; `grep -rn
  'Colors\.\|colors\.' app/lib` outside the theme file returns nothing.
- **Build check.** `flutter build apk --release` and `flutter build windows
  --release` are the `android-apk` and `windows-exe` jobs in
  `.github/workflows/build.yml`; this Linux runner has neither an Android SDK nor
  MSVC, so state that and name `flutter analyze` plus `flutter test` against a
  loaded engine as the local substitute. **This phase calls `px_process` for the
  first time**, so say so explicitly — it is the first UI phase where a mistake
  reaches the engine rather than staying in the widget tree.
- `bash scripts/verify.sh` exits 0.

### Needs a human judgement

- Pick a photo, change nothing, export. Is the default a photo you would have
  chosen yourself?
- Is every number on screen something you can explain to the person looking at
  it?
- Is there a control here you would remove, or one the engine supports that you
  would add?

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha, and names
  the patch and what landing it requires.
- Committed and pushed to `main`. Never force-push.
- Do not write `.done`. The workflow does.
