# Benchmarks

Measured numbers, the machine they were taken on, and how to take them again.
Every figure here comes from a harness in the tree; none of it is estimated.

**`resize` is the only thing benchmarked so far.** Phase-09 added a second
resampling kernel and this file is where it is compared against the one that
ships. Later phases that measure something should add a section, not a file.

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

| | |
| --- | --- |
| CPU | Intel Xeon Platinum 6973P-C (Granite Rapids), 4 threads visible |
| ISA in use | **AVX2** |
| OS | Linux 6.17.0-1022-azure, x86_64 |
| Toolchain | rustc 1.99.0 (b940084d7 2026-09-28) |
| Build | `--release`, default profile otherwise |

The AVX2 line is the one that matters for reading the rest of this file, and it is
the harness's most fragile claim. `fast_image_resize` dispatches at runtime on
CPU features (`cpu_extensions.rs`: AVX2, else SSE4.1, else scalar), and the crate
does not expose which one it picked, so the example reproduces that decision by
reading `/proc/cpuinfo` itself. **These numbers are AVX2 numbers.** An ARM phone
runs a different kernel and the speedup there is unmeasured.

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