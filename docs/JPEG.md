# Why the JPEG encoder is not `image`'s

`core/src/format.rs` writes JPEG through the `jpeg-encoder` crate rather than
through `image`'s own `JpegEncoder`. This is the reasoning, because the choice
looks arbitrary in a diff and is not.

## What `image`'s encoder cannot do

`image::codecs::jpeg::JpegEncoder` takes a quality and nothing else. It writes
**baseline 4:4:4** and exposes no way to ask for:

- a chroma resolution (4:2:0 or 4:2:2), and
- progressive, scan-by-scan output.

Both are not nice-to-haves for a resizer. 4:2:0 is *how JPEG gets small* —
measured on the fixture in `format::tests::four_four_four_is_strictly_larger_than_four_two_zero_at_the_same_quality`,
the same 1600×1200 photo at q85 is 88,975 bytes at 4:4:4 and 59,200 at 4:2:0, a
third of the file for no visible difference on a photograph. And progressive is
what makes an image appear at all on a slow connection, which is the difference
between a preview and a blank rectangle.

Worse than "cannot": `image`'s encoder picks its own sampling factor from the
quality value — 4:2:0 below quality 90, 4:4:4 at or above it. Left alone, the
engine's output would change shape when a user dragged a quality slider, in a way
no caller asked for and no test could see. `format::encode_jpeg` therefore sets
the sampling factor explicitly, every time, so the only thing that decides it is
the option the caller passed.

## Why this crate

| | `jpeg-encoder` (chosen) | `image`'s own |
| --- | --- | --- |
| Sampling factor | explicit, `F_1_1` / `F_2_1` / `F_2_2` | inferred from quality |
| Progressive | `set_progressive` | not available |
| C toolchain | none | none |
| Build script | none | none |
| Licence | MIT OR Apache-2.0 | MIT OR Apache-2.0 |

It is pure Rust with no build script, which is the same reason the `heic` feature
is the one optional codec left, and is why `avif` — another pure-Rust encoder —
could join `default` immediately. `webp-lossy` is the exception that proves the
rule: it is in `default` too, because it vendors libwebp's C source and pays
`cc` for 159 files, which is ~39 seconds on a cold release build and not a new
toolchain. `docs/ARCHITECTURE.md` has that argument; the point here is narrower,
which is that nothing about *JPEG* needs opt-in now.

The one genuine cost is that `image` cannot read back what `jpeg-encoder` writes
through its own JPEG decoder — it can, actually, but the engine does not rely on
that: the tests decode with `image::load_from_memory`, which is the same path a
user's next tool would take. Progressive files decode correctly, and the frame
marker is asserted rather than assumed (`format::tests::progressive_jpeg_is_progressive_and_decodes_to_the_same_size`
checks for `SOF2` against baseline's `SOF0`).

## The naming trap

`jpeg_encoder::SamplingFactor` names factors by **luma samples per chroma
sample**, so JPEG's 4:2:0 is `F_2_2` and 4:2:2 is `F_2_1` — the two spellings
read backwards relative to each other. `ChromaSubsampling::sampling_factor` is the
only place that translation exists, and it is the only place to change it.

## Numbers to quote

Measured in release on the CI runner, 1600×1200, one thread, no EXIF:

| Output | Bytes | Encode |
| --- | --- | --- |
| JPEG q85 4:4:4 baseline | 88,975 | 35 ms |
| JPEG q85 4:2:2 baseline | 72,724 | 26 ms |
| JPEG q85 4:2:0 baseline | 59,200 | 20 ms |
| JPEG q85 4:2:0 progressive | 96,938 | 26 ms |

Progressive costs about 64% at q85 here, not "a few per cent": the four scans
carry some coefficients twice. `OutputFormat::supports_progressive` says so with
the same numbers, because a doc comment that understates a cost is how a user
ends up wondering where the file size went.

Chroma error, measured as mean absolute error of the blue-difference channel on
saturated red-on-blue bars at q95 — the content that decides whether 4:2:0 is
honest. The bars are `format::tests::colour_bars(240, 160)`, so the figure is
reproducible from the test module rather than from a file nobody else has:

| Level | Chroma error |
| --- | --- |
| 4:4:4 | 1.05 |
| 4:2:2 | 21.18 |
| 4:2:0 | 21.75 |

That is the argument for the default, in numbers: 4:2:0 is a third off the file
of a photograph and twenty times worse on a screenshot. `docs/ARCHITECTURE.md`
argues the default from these.
