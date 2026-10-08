# phase-28 — `streaming`: make it correct, then decide whether it ships

**Track:** A — Engine & Security
**Depends on:** phase-27
**Timeout:** 90 minutes

## Objective

`stream.rs` has a **stride bug that no test can see**, a fallback allocation that
bypasses the budget the module exists to enforce, and — after this phase fixes
those — an unanswered question: **should it be on by default?**

It is the second-most-measured thing in this project (4 MB against 613 MB for a
120 MP source) and it is off by default because the wall-clock comparison that
would turn it on has never been run. This phase fixes the correctness problems
and produces the measurement, so the decision can be made on a number instead of
an argument.

## Read first

- `docs/AUDIT.md` findings **10** and **11**, with `file:line`.
- `core/src/stream.rs:480-630` — `Source::open` and `read_row`.
- `core/tests/streaming_peak.rs` — the phase-11 acceptance criterion.
- `docs/ARCHITECTURE.md`, "The second decode path, and what it is not", and gotchas
  23 and 25. Read what it claims before changing what it does.
- `core/src/worker.rs:712-760` — where `process_one` decides to take this path.
- `core/Cargo.toml` `streaming`, and `docs/BENCHMARKS.md` for the numbers.

## Scope

### 1. The stride — `stream.rs:592-625`

```rust
let start = y as usize * width as usize * CHANNELS;   // CHANNELS == 4
let Some(src) = img.as_bytes().get(start..start + out.len()) else { out.fill(0); return; };
match img.color() { ColorType::Rgba8 => …, ColorType::Rgb8 => …, ColorType::L8 => …, … }
```

`start` and the window size both assume an RGBA8 source. `DynamicImage::ImageRgb8`'s
`as_bytes()` is `W*H*3`, so **row 0 is right and every row `y >= 1` is read from
the wrong offset**; the last rows fail the `get` and are silently zero-filled.
`image` decodes every JPEG to `ImageRgb8`, and `Source::Whole` is the arm every
non-PNG takes.

The consequence is a skewed picture with a black band — no panic, no error.

It survived because both whole-image-arm tests assert **only dimensions**:
`stream.rs:860` and `stream.rs:876`. The second is named
`a_png_this_arm_cannot_decode_falls_back_rather_than_scrambling` and never looks
at a pixel.

Fix the arithmetic — the source's real byte-per-pixel stride, and a window sized
for it. Then write the pixel test that should have existed: a row ramp through
the whole-image arm, compared against the in-memory kernel's output, for at
least `Rgb8`, `Rgba8`, `L8` and `La8`. **That test fails today**; paste the
failure before the fix and the pass after.

### 2. The unbudgeted fallback — `stream.rs:489-492`

```rust
crate::decode_bounded(input, limits).unwrap_or_else(|_| DynamicImage::new_rgba8(src_w, src_h))
```

`src_w`/`src_h` come from the **header**. The only ceiling is
`check_streamed_header` = `4 × max_pixels`, so on the desktop profile the
fallback blank is about **2 GB**. It is in neither `working_set_bytes` nor
`streaming_memory_budget`, so it bypasses the ceiling the feature exists to
enforce.

Either bound it or refuse. A blank image that big is worse than an error, and the
whole point of the module is that the peak is a function of the *destination*.

`core/src/worker.rs:741` has the same shape in miniature — a full RGBA buffer
allocated to evaluate the boolean `ResizeSpec::has_effect` — which is up to
512 MB thrown away on the next line. Fix it while you are here; it is the same
defect at a smaller scale.

### 3. The measurement that decides the flag

The reason `streaming` is off is recorded in `docs/ARCHITECTURE.md` and repeated
in phase-11's notes: memory is measured, **wall clock is not**, and *"less memory"
is only half of what a decode path has to be*. A 120 MP streaming decode took 83 s
in a **debug** build against 141 s for the run including the in-memory
comparison — not a release number, and it settles nothing.

Produce a release number. `core/examples/resize_bench.rs` already exists and
`docs/BENCHMARKS.md` already has the harness; extend it to the streaming path
rather than writing a new one.

Then **decide**, and write the decision down:

- **On by default** if release wall clock is within noise of the in-memory path
  and the memory win holds. Say which measurements.
- **Still off** if wall clock is meaningfully worse, or if you cannot get an ARM
  machine — in which case say *that*, because it is the same argument that keeps
  `simd` off and it is a legitimate answer.

**Do not flip the default to make a phase look productive.** If the honest
answer is "still off, now with a number", that is the deliverable.

### 4. What this path still cannot do — restate it

It only runs for a plain resize: no crop, no orientation, and a resize that
changes the size. It cannot stream most formats — `image` 0.25.10 gives every
codec `read_image(self, buf)` and nothing else, so only PNG has a row API.
`stream::tests::names_the_formats_it_cannot_stream` already fails if that stops
being true; keep it.

If you change what the path supports, update `docs/ARCHITECTURE.md` in the same
commit. `AGENTS.md` requires the docs to say what the code does, not what it was
intended to do.

### Do not

- Do not introduce a second resampling pass. Hard rule 5. The interleaved
  horizontal/vertical order is legitimate precisely because separable filters
  commute; keep that argument intact and keep the agreement matrix.
- Do not make the module look better than it is. `docs/ARCHITECTURE.md` records
  that hard colour edges are the unmeasured case; leave that recorded as
  unmeasured unless you measured it.
- Do not add a `cfg(target_arch = "x86_64")` shortcut to make the numbers work.
- Do not weaken the 4 MB peak assertion in `core/tests/streaming_peak.rs` to fit
  a change you made.

## Acceptance criteria

### Machine-checkable

- A test drives a **JPEG** (so `ImageRgb8`) and a greyscale PNG through
  `Source::Whole` and asserts **pixel equality** with the in-memory kernel, not
  just dimensions. It fails on the current code — paste the failure.
- The same test covers `Rgba8`, `Rgb8`, `L8` and `La8` sources.
- A test asserts no fallback allocation exceeds the streaming memory budget, for
  a header that passes `check_streamed_header` on both the desktop and the mobile
  profile.
- `worker.rs:741` no longer allocates a `DynamicImage` to evaluate a boolean; a
  test covers the path it took to get there.
- A **release-mode** benchmark reports wall clock for the streaming and
  in-memory paths over at least three source sizes, and the table is in
  `docs/BENCHMARKS.md` with the machine named.
- `cargo test --features streaming` and `cargo test --all-features` are both
  green; `core/tests/streaming_peak.rs` still asserts its 4 MB ceiling.
- `bash scripts/verify.sh` exits 0.
- **No FFI signature, JSON field or public Rust type changes**, so the APK and
  EXE builds are unaffected — **unless** you flip the default, in which case
  every shipped binary changes and the `android-apk`, `windows-exe` and
  `engine-matrix` jobs all exercise it. Say which happened and what it affects.

### Needs a human judgement

- Read `docs/ARCHITECTURE.md`'s "The second decode path" section as it stands
  after your change. Does it still describe what the code does? Anything you
  found that made it wrong belongs in `docs/AUDIT.md`.
- Was the stride bug reachable by a user, or only by a test? Say which, and
  adjust the severity you recorded rather than leaving it overstated.

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha, and states
  the shipping decision for `streaming` with the measurement behind it.
- Committed and pushed to `main`. Never force-push.
- Do not write `.done`. The workflow does.
