# phase-22 — Track B: the design system and the shell

**Track:** B — Flutter UI
**Depends on:** phase-21
**Timeout:** 90 minutes

## Objective

Build the layer everything else in Track B stands on: a **design system** (theme,
colour, type, spacing, elevation) and an **app shell** that owns navigation,
window chrome, empty states and loading states.

`app/lib/main.dart` is 80 lines today. It opens a window, calls `px_version` and
shows the engine's version string. That is the whole user interface, and the
`docs/AUDIT.md` classification puts all eleven Visual/UI roadmap items at
`ABSENT`.

Every later UI phase assumes this one exists. Without it, phase-23 invents a
slider in a throwaway container and phase-24 invents a different one beside it.

## Read first

- `docs/AUDIT.md` — the "Visual / UI" classification table. That is the list.
- `ROADMAP.md`, "Visual / UI" and "User-friendliness".
- `app/lib/main.dart` — all 80 lines. `app/pubspec.yaml`,
  `app/analysis_options.yaml`.
- `app/lib/rust/models.dart` and `engine.dart` — phase-20 brought the models up
  to date, so `Capabilities`, `OutputFormat`, `ChromaSubsampling` and
  `ValidateReport` are all there now. **Do not re-add anything.**
- `AGENTS.md`: *"Dart: no `print`. Use the logger. No magic numbers in the UI
  layer."*

## The delivery constraint

`app/` is the `authorss81/shrinkray` submodule and this pipeline cannot push to
it — `git -C app push` returns 403. Every UI phase therefore delivers its Dart as
a patch under `workspace/phase-NN/`, applies cleanly to the pinned commit
`5d0e9cc`, and is run through `dart format`, `flutter analyze` and `flutter test`
**before** being captured. The patch is the deliverable; say in
`docs/phase-status.md` what is in it and what a human must do to land it.

## Scope

### The theme

- **Light, dark and system**, as three states, not two themes and a preference.
- **Hand-tuned contrast in both.** This is the part that takes the time: dark
  surfaces need their own values, not inverted greys. **WCAG AA on all body text
  and all controls** — and assert it. A contrast ratio is computable from the
  palette; write the check rather than eyeballing it.
- **One accent colour**, one surface-elevation model, a **4pt spacing grid**.
  No gradients for decoration. The elevation model should be three or four named
  levels, not arbitrary shadows.
- Respect **high contrast** (`MediaQuery.highContrast`) and
  **`prefers-reduced-motion`** — Flutter exposes the latter as
  `MediaQuery.disableAnimations`.
- A **custom theming hook**: a way to override the accent without forking the
  theme. The roadmap asks for it and it costs a `ThemeExtension`.

### The shell

- Navigation that the later phases plug into: a home screen, and a place for a
  workspace screen. Decide the layout (a side rail on desktop, a bottom bar or
  nothing on a phone) from what phase-24 and phase-25 will put in it, not from
  what looks tidy in a screenshot.
- **Window chrome**: title, icon, minimum size, and a sensible initial size.
- **Empty states that teach.** The roadmap is explicit: *"empty states that
  teach instead of saying 'no files'."* An empty state names what goes here, why
  it is worth doing, and offers the action.
- **Skeleton + shimmer** for anything over ~200 ms.
- **Toast plus inline status for every action**, so nothing completes silently.
- **Full keyboard access**: every interactive element reachable by Tab, visible
  focus, and a sensible focus order. A resizer is a tool; people use it with two
  hands on a keyboard.

### Wiring

- `px_version()` and `px_presets()` at startup. **Do no work in `main`** — the
  roadmap asks for a first frame under 100 ms, and a synchronous FFI call before
  `runApp` is the one thing that guarantees missing it.
- The logger, not `print`.

### Do not

- Do not call `px_process`, `px_batch` or `px_zip` from this phase. The engine
  calls belong to phase-24 and phase-25. This phase builds the frame they go in.
- Do not add a dependency without saying why in the commit message, and prefer
  the Flutter SDK. No `google_fonts`, no theming package.
- Do not localise yet. `ROADMAP.md` asks for ARB files "from day one"; the
  strings do not exist yet either, so the honest move is to keep every
  user-visible string in one place per screen so the extraction in a later phase
  is mechanical. Say so in `docs/phase-status.md`.
- Do not refactor `app/lib/rust/`. Phase-20 owns it.

## Acceptance criteria

### Machine-checkable

- The patch applies cleanly to `app/` at `5d0e9cc`. Paste `git apply --check`.
- `cd app && dart format --output=none --set-exit-if-changed lib test` reports
  nothing new beyond the one allowlisted upstream file, `flutter analyze` reports
  zero issues, and `flutter test` passes with **none skipped**.
- A widget test asserts the light and dark themes both meet **WCAG AA (4.5:1) for
  body text and 3:1 for large text and control boundaries**, computed from the
  palette rather than hard-coded per widget. It must fail if a colour is changed
  to a failing one — prove it.
- A widget test asserts the app renders with the engine library absent, i.e.
  `px_version` failing produces a readable screen rather than a crash.
- `grep -rn 'print(' app/lib` returns nothing outside the logger's own
  implementation.
- `grep -rn 'colors\.\|Colors\.' app/lib` outside the theme file returns nothing:
  **no magic colours in the UI layer**, per `AGENTS.md`.
- **Build check.** `verify.sh` does not build `app/`. State plainly in the
  commit message that `flutter build apk --release` and `flutter build
  windows --release` are the `android-apk` and `windows-exe` jobs in
  `.github/workflows/build.yml`, that this Linux runner has neither an Android
  SDK nor MSVC, and that `flutter analyze` plus `flutter test` against a loaded
  engine are the local substitute. Do not claim a build you did not run.
- `bash scripts/verify.sh` exits 0.

### Needs a human judgement

- Look at the app. Does the dark theme look *designed*, or does it look like
  light with the lights off? This is the question the phase exists to answer.
- Would you use this shell to open a 400-image folder, or does it get in the way?
- Is the accent colour the right one, or is it a placeholder that will be
  regretted?

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha, and names
  the patch and what landing it requires.
- Committed and pushed to `main`. Never force-push.
- Do not write `.done`. The workflow does.
