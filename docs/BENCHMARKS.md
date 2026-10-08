# Benchmarks

Measured numbers, the machine they were taken on, and how to take them again.
Every figure here comes from a harness in the tree; none of it is estimated.

Two harnesses, and they answer different questions:

| | `core/benches/` (criterion) | `core/examples/resize_bench.rs` |
| --- | --- | --- |
| What | the five hot paths, twenty cases, one per stage of a real export | the two resize kernels, side by side, with their disagreement measured |
| Run by | `.github/workflows/bench.yml`, nightly and on every pull request | by hand |
| Answers | "did this get slower?" | "is the fast kernel the same picture as the slow one?" |
| Numbers below | [the regression suite](#the-regression-suite) | [the two resize kernels](#the-two-resize-kernels) |

**Read the machine before the number.** The two sections were measured on
*different machines*, and the resize rows in the suite are the SIMD kernel while
the table further down is the reference kernel, so the two sets of resize numbers
are not two opinions about one thing. There is a [Machine](#machines) section
that says which is which; it is the first thing to read before quoting anything
here.

## The two resize kernels

The engine has one resampling pass, and hard rule 5 says there can only ever be
one, so this is not a choice between two filters — it is a choice between two
implementations of the same filter.

| | Reference | SIMD |
| --- | --- | --- |
| Crate | `image`'s `imageops::resize` | `fast_image_resize` 6.1 |
| Selected by | the default build | the `simd` feature, **off by default** |
| Entry point | `resize::resample_reference` | `resize::simd::try_resample` |
| Dispatch | `resize::resample` picks it whenever the SIMD kernel declines | |
| Arithmetic | coefficients in `f32`, accumulated in `f32`, one `round()` at the end | coefficients quantised to `i16` scaled by `1 << precision`, accumulated in `i32`, clipped through a 1280-entry table by an arithmetic right shift |
| Colour type in, out | any `DynamicImage` buffer, always RGBA8 out | normalised to RGBA8 once, always RGBA8 out |
| Alpha | straight | straight (`use_alpha(false)`) |

The reference kernel is not deprecated and is not on a schedule. It is the
correctness oracle: `resize::tests` compares the SIMD output against it, so
deleting it would delete the only way to check the other one.

## Reproducing

```sh
cargo run --release --features simd --example resize_bench     # from core/
```

Release, and `--features simd`: the example needs the second kernel, and a debug
build of `image`'s kernel is roughly an order of magnitude slower than the one that
ships. It prints a warning to stderr if you forget. Five timed repetitions per
case, median reported.

## Machine

Taken on the Intel Xeon Platinum 6973P-C (Granite Rapids), 4 threads visible,
AVX2, Linux 6.17.0-1022-azure, rustc 1.99.0, `--release`. **Every number in this
section and in [Build cost](#build-cost) is from that machine.** The regression
suite below was measured on a *different* one; see [Machines](#machines).

The AVX2 line is the one that matters for reading the rest of this section, and
it is the harness's most fragile claim. `fast_image_resize` dispatches at runtime
on CPU features (`cpu_extensions.rs`: AVX2, else SSE4.1, else scalar), and the
crate does not expose which one it picked, so the example reproduces that
decision by reading `/proc/cpuinfo` itself. **These numbers are AVX2 numbers.** An
ARM phone runs a different kernel and the speedup there is unmeasured.

## Machines

Two runs, two machines, and the tables below are **not comparable across them**.
Quoting a ratio from one against a number from the other would be the easiest
mistake to make with this file, so the machines are named up front.

| | Resize kernels, build cost | Regression suite |
| --- | --- | --- |
| CPU | Intel Xeon Platinum 6973P-C (Granite Rapids) | AMD EPYC 7763 (Milan, Zen 3) |
| Threads visible | 4 | 4 |
| ISA | AVX2 | AVX2 |
| OS | Linux 6.17.0-1022-azure x86_64 | same |
| Toolchain | rustc 1.99.0 (b940084d7 2026-09-28) | same |
| Features | default, plus `simd` for the SIMD column | `--all-features`, so `simd` is **on** |
| Recorded in | this section | `core/benches/baseline/machine.json` |

The feature row matters most for resize. **`resize/*` in the suite is the SIMD
kernel**, because `--all-features` turns it on, so the suite's 79.8 ms for a 24 MP
to 1920 resize is the fast column of the table below and not its reference column.
The two-kernel comparison is `core/examples/resize_bench.rs`'s job precisely
because one benchmark ID cannot carry both.

`scripts/bench-compare.py` reads the machine record and refuses to call a
regression when the CPU model does not match. A difference between two machines
is not a change in the code, and a gate that reports one anyway is a gate that
gets muted.

## The regression suite

`core/benches/`, twenty cases across five bench targets, criterion 0.7.

```sh
cd core
cargo bench --all-features -- --profile ci         # the gate's run
cargo bench --all-features -- --profile full        # tight numbers
cargo bench --all-features -- --test resize         # smoke test, seconds
```

`--profile ci` is **not** a criterion flag — criterion removed it after 0.4, and
its own argument parser exits 2 on an unknown option. `core/benches/common/mod.rs`
parses the handful of flags this suite supports and builds the runner from
criterion's builder methods, so `--profile ci` is this repository's flag and not a
criterion one. An unrecognised flag stops the run rather than being ignored,
because silently ignoring one produces a full-speed run that still prints
`profile: ci`, which is the specific way a gate stops being a gate.

The ci profile is 10 samples, 1 s warm-up, 3 s measurement, 10 000 bootstrap
resamples. Ten is criterion's floor — `configure_from_args` rejects less — and it
is why the threshold is 15% rather than 5%; the argument is at the bottom of
`.github/workflows/bench.yml`. `--profile full` is criterion's own 100-sample
setting, for when a number in this file needs to be tight.

### The fixture, and why it is compressible

Generated in `core/benches/common/mod.rs` at three sizes: 1632×1224 (2 MP),
4000×3000 (12 MP), 6000×4000 (24 MP). No image files are committed and none are
fetched — the content is a multi-octave wave, a 2-pixel comb every 128 columns and
a vignette, and it is **deliberately compressible**.

That is load-bearing, not aesthetic. On random noise an encoder spends all its
time in the entropy coder, so a noise-fixture JPEG benchmark measures the Huffman
coder rather than the DCT and prediction pass — the part that would change if the
pipeline changed. Worse, noise is incompressible, so a byte ceiling can never be
met at any quality, and `TargetBytes::encode_with` would report `target_met: false`
and time the failure path.

The honest cost is that **the decode rows under-report entropy-decoding work**
relative to a camera original, because a decoder's cost is dominated by exactly
the stage a compressible fixture makes cheap. Those nine numbers are a floor, and
a real photograph's JPEG decode is meaningfully slower. Decode is also the one
place the alternative is worse: a noise fixture would be as large as the raw image
(96 MB at 24 MP) and the benchmark would end up measuring the filesystem.

### decode

`lib::decode_bounded` — the path `worker::process_one` takes for an ordinary file,
limits and format detection included. Bytes are already in memory.

| Format | 2 MP | 12 MP | 24 MP |
| --- | ---: | ---: | ---: |
| JPEG (q90) | 4.65 ms | 42.2 ms | 80.3 ms |
| PNG | 9.75 ms | 75.7 ms | 149.3 ms |
| WebP (q80) | 32.6 ms | 192.3 ms | 369.7 ms |

**WebP decode is 4.6× JPEG's**, and that is not a fixture artefact: this build
writes lossy WebP through libwebp and reads it back through `image`'s VP8
decoder, and the two do not share an implementation. A user who picks WebP for
the file size and picks it again on import pays this on the way back in.

What this does **not** measure: filesystem and I/O (a 24 MP JPEG off an SD card is
a different question); the sandbox, whose cost is a process spawn; the real
entropy-decoding load above; and a hostile file — nothing here is over `Limits`,
and refusing a 60000×60000 header takes microseconds and is a correctness
property, not a latency one.

### resize

`pipeline::resize_to`, the one call site hard rule 5 permits, through whichever
kernel this build selected.

| Case | Source → target | Time |
| --- | --- | ---: |
| downscale to 1920 wide | 6000×4000 → 1920×1280 | 79.8 ms |
| downscale to 400 wide | 6000×4000 → 400×267 | 84.0 ms |
| extreme downscale to 64 | 6000×4000 → 64×43 | 77.5 ms |
| upscale 2× | 1632×1224 → 3264×2448 | 19.4 ms |

**All four are the SIMD kernel.** For the reference kernel's numbers, see
[the table below](#results). What this does **not** measure: crop and orientation,
which are a memory move and a buffer transpose — microseconds against tens of
milliseconds, and timing them would make a benchmark whose number is mostly
scheduler noise; `Cover`, which is the resample plus a slice; and the output
pixels, which `resize::tests` asserts by value rather than by clock.

### encode

`lib::encode_fixed` — the fixed-quality path, no search.

| Case | Size | Time |
| --- | --- | ---: |
| JPEG q85 | 12 MP | 153.7 ms |
| JPEG q95 | 12 MP | 155.3 ms |
| WebP lossy q80 | 12 MP | 679.6 ms |
| PNG, deflate best | 2 MP | 584.8 ms |

Three things worth reading off that table:

**PNG at 2 MP costs more than JPEG at 12 MP** — 585 ms against 154 ms. PNG here is
`deflate` at `CompressionType::Best` over four channels, and it is measured at
2 MP rather than 12 MP *because of that*: at 12 MP it would take roughly 3.5
seconds and dominate the suite's wall clock. The input is in the benchmark's own
name rather than left to look like a 12 MP number, because quietly shrinking an
input to make a benchmark finish is exactly what turns it into a lie.

**q85 → q95 costs 1% more time and 60% more bytes.** The size curve for this
fixture, measured to make the byte-target ceiling reachable rather than guessed:

```text
q30 292,610   q50 329,332   q70 381,528   q85 496,710   q90 589,601
q40 311,102   q60 345,708   q80 442,645                q95 795,860
```

The flatness from q30 to q60 is a property of a smooth synthetic fixture; a real
photograph's curve is much steeper. The shape that *is* general is the last two
rows: past q90 the file grows far faster than the encoder works, which is the
whole argument for byte ceilings over quality sliders.

**AVIF is not in the suite.** It is the slowest encoder in the build — ~3.2 s for
1600×1200, ~17 s for 4032×3024 (`docs/ARCHITECTURE.md`) — so at 12 MP it would
dominate everything else and the ci sample count is what keeps it out. It is named
here rather than silently dropped, and the number already on record stands in for
it.

What this does **not** measure: the resize that normally precedes every one of
these, so no number here is an export time; and the batch path's rayon
parallelism — every figure is single-threaded, which is the pessimistic half and
the one that matters on a phone.

### target_bytes

`TargetBytes::encode_with`, reported in **encoder passes per successful fit**
rather than seconds, because the passes are what a user pays for: six encodes of
a 12 MP photo is six times the wait, the heat and the battery.

**6 encoder passes, 879 ms, converging on 393,838 bytes at q73 for a 400,000-byte
ceiling.** The ceiling lives in the benchmark's own source with the size curve above
beside it, because a ceiling below the q30 floor times the *unreachable* path: the
search still walks the interval and then spends one more encode returning the
floor attempt, so it costs about the same, reports `target_met: false`, and looks
perfectly respectable for the wrong function. The benchmark asserts the fit
succeeded, so that cannot happen silently.

What this does **not** measure: whether the ceiling was met — that is
`target::tests`, which asserts a byte count rather than a clock; or a pathological
size curve, which `target::tests` checks exactly with a synthetic encoder rather
than by sampling this one.

### exif

| Case | Size | Time |
| --- | --- | ---: |
| `exif::read`, full phone tag set | 12 MP JPEG | 15.6 µs |
| `exif::strip` | 12 MP | 31.8 ms |

The fixture carries orientation, make, model, timestamps, exposure, lens and
serials, and **every sensitive class `write_back` filters** — GPS, owner, artist,
copyright, body and lens serials — because both functions cost what the tag count
costs. A three-field fixture would have made the read row a quarter of what it is
and would have left a regression in the filter list invisible.

The 2000× gap between the two rows is the honest answer to "is metadata
expensive?": **parsing it is free; getting rid of it is a memory copy.** `strip`
rebuilds an RGBA buffer from raw samples — which is how a maker note or an embedded
thumbnail cannot survive — so the cost of not carrying metadata is one linear pass
over the pixel buffer, about 1.5 GB/s of copying here.

What this does **not** measure: `exif::write_back`, the only path that puts
metadata back, which runs on explicit opt-in only and is covered by tests because
it is not a path anyone waits for; a HEIF's metadata, which lives in a separate
container item behind a `cdsc` relation where `exif::read` cannot see it at all
(`docs/ARCHITECTURE.md`'s metadata section); and malformed input, which belongs to
`core/tests/hostile.rs` and is not something a clock should be asked about.

### The gate

`workspace/phase-10/bench.yml.patch`, which adds `.github/workflows/bench.yml`:
nightly on `main` and on every pull request.

**It is a patch, not a file in the tree.** This pipeline's push credential is
refused `workflows` permission on `.github/workflows/`, so a commit carrying the
workflow cannot be pushed at all — the same limit that left phase-08's
`webp-lossy-matrix` job, and the `app/` half of phase-05, 06 and 07, as patches.
Apply it with `git apply workspace/phase-10/bench.yml.patch` from the repository
root. **Until someone does, this suite is measured by hand and the gate does not
run**, which is the honest statement rather than a workflow that appears to be
protecting something.

What the workflow does when it is applied:

- **Nightly** runs the suite with `--save-baseline main` and uploads the result as
  an artefact. It renders no verdict and rewrites nothing: moving the committed
  baseline stays a commit a person makes, with a reason in the message.
- **A pull request** installs `core/benches/baseline/` into criterion's output
  directory, runs with `--baseline main`, and applies the rule with
  `scripts/bench-compare.py`.
- **The rule** is per benchmark, never aggregated, on criterion's own `mean` point
  estimate, with the confidence intervals printed so a reader can see whether a
  change is distinguishable from noise. **More than 15% slower fails the pull
  request** and comments a table on it.
- **One row is exempt from the verdict**: `exif/read/full_tag_set` runs in 15.6 µs,
  which is shorter than a scheduler tick on a loaded machine. It is reported every
  run and it is never the reason a pull request goes red.
- **The machine check**: if the CPU model differs from
  `core/benches/baseline/machine.json`, the script reports the numbers and gives no
  verdict. Cross-machine differences are not regressions.

Both halves of it work today without CI, by hand:

```sh
cd core
# save a baseline
cargo bench --all-features -- --profile ci --save-baseline main
# compare against the committed one
mkdir -p target/criterion && cp -r benches/baseline/. target/criterion/
cargo bench --all-features -- --profile ci --baseline main
cd .. && python3 scripts/bench-compare.py
```

`scripts/bench-compare.py` exits 1 when something regressed by more than the
threshold, so it drops into a script or a pre-push hook unchanged.

Why 15% and not 5% is argued at length in the workflow file. The short version:
ten samples over a three-second window on a shared runner cannot resolve 5%, and a
gate that is red more often than green stops being read. A real regression in this
codebase is a second resampling pass, an encoder at the wrong quality, a `Vec`
copied per iteration — those are 2×, not 1.05×.

## Results

Fixture: a photograph stand-in — a smooth multi-octave wave, a hard 2-pixel comb
every 128 columns, opaque throughout. Deliberately *not* the adversarial fixture
in `resize::tests`, which puts a 2-pixel checkerboard in a quarter of the frame to
make disagreement visible; benchmarking that would flatter the reference and say
nothing about a real export.

`maxdiff` is the largest absolute per-channel difference between the two kernels'
output on that case, and `mean` the mean of them. Both out of 255.

| Case | Filter | Source → target | Reference | SIMD | Speedup | maxdiff | mean |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |
| 24 MP to 1920 wide | Lanczos3 | 6000×4000 → 1920×1280 | 555.8 ms | 68.4 ms | **8.13×** | 1 | 0.01 |
| 24 MP to 400 wide | Lanczos3 | 6000×4000 → 400×267 | 348.1 ms | 77.3 ms | **4.50×** | 1 | 0.03 |
| 4000 px to thumbnail | Triangle | 4000×3000 → 400×300 | 66.1 ms | 25.0 ms | **2.64×** | 1 | 0.04 |
| 800 px to 4000 px upscale | Lanczos3 | 800×600 → 4000×3000 | 558.9 ms | 28.1 ms | **19.90×** | 1 | 0.03 |
| 24 MP to 1920 wide | Gaussian | 6000×4000 → 1920×1280 | 482.5 ms | 68.3 ms | **7.06×** | 1 | 0.01 |

Medians of five, from one run; three further runs of the same binary gave 7.10×,
7.54× and 8.13× on the first row, so treat the third significant figure as noise.
The first row's 555.8 ms and 498.8 ms are the same case measured minutes apart on
a shared runner.

### What the numbers say

**The win is real and it is the one the UI needs.** A single 24-megapixel phone
photo exported to 1920 wide goes from about half a second to about 70 ms. That is
the difference between a progress bar and an instant result, and it is the case
`web-hero` and `web-card` presets actually run.

**The upscale is the extreme of it — 19.9×.** That is not the SIMD kernel being
clever; it is `image`'s kernel being quadratic in a way that hurts. Upscaling
800×600 to 4000×3000 produces 12 MP of destination from 0.5 MP of source, so the
reference kernel's per-destination-sample overhead dominates, and a filter-width
loop over 6 taps is exactly what vectorises well.

**Downscales gain least — 2.6× on the thumbnail, 4.5× at 400 wide.** Both kernels
scale their filter support by the reduction ratio and become area averages, and
the work is dominated by streaming 96 MB of source pixels rather than by
arithmetic. This is the honest floor: whatever the vectorisation is worth, a
memory-bound pass cannot get more than a small multiple of it.

**Agreement on realistic content is one least-significant bit.** Every row's
`maxdiff` is 1 and the mean is 0.01 to 0.04 of 255. On a photograph the two
kernels are the same picture to within rounding.

### And where they do not

`resize::tests` uses a deliberately harder fixture, and there the two kernels
disagree by more than 1. Per filter, over eight size pairs including a 1-pixel
target and a 30:1 downscale:

| Filter | worst per-channel | worst mean | tolerance the test enforces |
| --- | ---: | ---: | ---: |
| `Triangle` | 1 | 0.12 | 2 / 0.4 |
| `Gaussian` | 7 | 0.15 | 10 / 0.4 |
| `CatmullRom` | 15 | 0.25 | 20 / 0.6 |
| `Lanczos3` | 29 | 0.48 | 36 / 0.8 |

Those numbers are identical in debug and in release, and they are largest on
**upscales**, not downscales — `Lanczos3` at 300×200 → 450×300 reaches 29, while
every 30:1 downscale in the matrix is at 1. The reason is ringing: the fixture's
checkerboard puts a hard edge where Lanczos3's four negative lobes overshoot, and
that is where an i16-truncated coefficient has the most leverage. `Triangle` is a
tent function with no negative part and comes out at 1 everywhere.

The gap between 29 and the nearest *wrong* kernel's 31 is the most interesting
number in this phase, and it is why the test asserts a mean as well as a maximum.
The mean separates where the maximum cannot: no wrong-kernel pairing anywhere in
the 4×4 filter square keeps its mean under 1.17, and most are above 3.
`resize::tests::simd_agreement::TOLERANCES` has the full argument, and
`a_wrong_kernel_always_lands_outside_the_tolerance` measures the floors in the
suite so the table cannot rot.

## Peak memory: a 120-megapixel source, resized to 1000 px wide

Phase-11. The measurement is peak *live heap*, from a counting `GlobalAlloc` in
`core/tests/streaming_peak.rs`, not a model and not `VmHWM`: the question is what
the engine allocates, and a counter answers that on every platform the suite runs
on. Bytes already in memory — the compressed input — are marked out of both
columns, so the numbers are what the decode and the transform cost.

| Source | Output | In memory | Streaming | What `working_set_bytes` predicts |
| --- | --- | ---: | ---: | ---: |
| 24 MP (6000×4000) | 1000×667 | 155 MB | **3 MB** | 2.7 MB |
| 60 MP (12000×5000) | 1000×417 | 306 MB | **2 MB** | 1.8 MB |
| 120 MP (12000×10000) | 1000×833 | 613 MB | **4 MB** | 3.5 MB |

```sh
cd core
PX_MEASURE_BEFORE=1 cargo test --features streaming --test streaming_peak -- --nocapture
```

The 120 MP row runs by default (it is what the test asserts a ceiling on); the
other two rows and the in-memory column are behind `PX_MEASURE_BEFORE`, because a
120 MP Lanczos3 resize at `opt-level = 0` costs about three minutes and the
streaming number next to it costs seconds.

**Before this phase the same job needed about 1.1 GB**, and the 613 MB above is
the figure *after* the clone in `Pipeline::apply` was removed, because the two
changes are not separable in the table: 480 MB of RGBA twice over plus 160 MB of
`f32` intermediate is where 1.1 GB came from, and one of those two copies is gone.
The remaining 613 MB is 480 MB of decoded RGBA8 and 160 MB of
`image::imageops::resize`'s `src_width x dst_height` intermediate — the buffer
whose shape is the whole problem, because it scales with the *source* width while
its contents are destination pixels.

**The 60 MP row is smaller than the 24 MP row.** That is not a mistake in the
table: the streaming peak is `dst + src_width * 4 + window * dst_width * 16`, and
the 60 MP source is 12000 px wide against 6000, so it pays more for the decoded
row and less for the coefficients. It is also why the streaming column is flat in
the source's pixel count and not in its width: **input size is decoupled from
output size, and the only thing that still scales with the input is one row of
it.** A 400 MP panorama would read the same 4 MB.

### What the two paths cost in time

Not measured in a table here, because the memory numbers are what the phase turns
on and a wall-clock comparison deserves the phase-10 harness rather than a row
added by hand. What is known: the streaming path is O(source pixels × kernel
taps per destination pixel) doing exactly the arithmetic `image` does, in a
different order, and it cannot be free — a debug-build 120 MP run above took
83 s, against 141 s for the run including the in-memory comparison. **The flag
stays off by default for that reason as much as the memory one**, and the numbers
that would settle it are not in this file.

### Quality: one pass, and what the box-first design would have cost

The alternative to this module is the one most resizers use: box-average the
source down to something small, then resize that. That is two filter
applications, which hard rule 5 bans, so it was measured rather than argued:
`stream::tests::one_pass_beats_a_box_pre_reduction` runs both designs on the same
1200x1200 fixture at 16:1, 8:1 and 4:1.

| Reduction | One pass vs box-then-resize, worst channel | Mean |
| --- | ---: | ---: |
| 1200 → 75 (16:1) | 1 | 0.082 |
| 1200 → 150 (8:1) | 1 | 0.088 |
| 1200 → 300 (4:1) | 1 | 0.070 |

**One least-significant bit, out of 255.** So the single pass is here for memory
and for hard rule 5, not because the composite would have been visibly worse — and
that is a thinner argument than it looks, so it is stated rather than dressed up.
What this table does *not* cover is content where the subject is colour: saturated
red on blue is the case that punished 4:2:0 in phase-07, and a box average takes a
hard edge and a Lanczos pass takes it differently. Nothing here has measured that,
and `docs/ARCHITECTURE.md` records it as unknown rather than as fine.

The two paths agree with each other, and with `resize::resample_reference`, to the
same one LSB: worst per-channel 1 and worst mean 0.0071 of 255 over five filters
and seven size pairs, all of it on an upscale and zero of it on any downscale
(`stream::tests::it_agrees_with_the_in_memory_kernel`).

## Build cost

| | Cold `cargo build --release --lib` |
| --- | ---: |
| Default features | 70 s |
| `--features simd` | 80 s |

**10 seconds**, against the 39 s that `webp-lossy` costs (`docs/ARCHITECTURE.md`).
Three new crates in the lockfile — `fast_image_resize`, `document-features`,
`litrs` — and `pulp`, which `ravif` already pulled in. One build script among
them, in `pulp`, and it only asks `version_check` what the Rust version is. No C
compiler, no nasm, no code generation.

That asymmetry is the interesting part of choosing this crate over `libvips`,
which the phase prompt ruled out. `libvips` would have bought more vectorisation
and paid a C library with a real build script on every one of the four shipped
targets, to accelerate one operation in a chain that is mostly decode and encode.
This buys most of the win for 10 seconds of CI and no new toolchain.

## The judgement call

The phase prompt asks whether the difference is worth a feature flag, a second code
path and the maintenance cost forever. Measured: **2.6× to 19.9×, agreeing to
within one LSB on real content, for 10 seconds of build time.**

Yes, that is worth it — but **not enough to turn the flag on yet**, and the reason
is not the numbers.

1. **Every number here is x86-64 AVX2.** `fast_image_resize` dispatches at
   runtime, so a phone on ARM runs a different kernel with a different amount of
   vectorisation. Phase-09's build is the one shipped to a phone, and its speedup
   is unmeasured. Turning the default on from an x86 runner is turning it on from
   the wrong machine.
2. **The wrong-kernel floor (31) is barely above the right-kernel ceiling (29).**
   Not a build risk — the tests catch it — but a signal that the two
   implementations are similar enough that future upgrades are the real risk.
   `image` and `fast_image_resize` both resample; they will both change their
   kernels in a point release, and a point release that moves the reference's
   Lanczos3 is a change this engine would have to re-measure against. The tests
   make that a build failure rather than a silent export change, which is the
   right place for it to land.
3. **`Nearest` is not faster under this flag.** It is refused outright, because
   the two crates disagree about which source pixel a destination sample takes on
   an exact tie, and no tolerance covers a whole-pixel difference. So a build with
   `simd` on still pays the reference kernel's cost for every pixel-art export —
   the flag makes four of the five filters faster and the fifth bit-identical.

### What would change the answer

- An ARM machine, one benchmark run, and a `fast_image_resize` scalar/NEON speedup
  above about 2×. That is the phase-10 job.
- Then: add `simd` to `default`, move the tolerance table's margins down to the
  measurement they were taken at, and delete nothing. The reference kernel stays
  as the oracle regardless — that is the one part of this phase that is not
  negotiable.