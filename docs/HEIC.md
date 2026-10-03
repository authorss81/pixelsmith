# HEIC/HEIF decode

phase-06. The decision, the alternatives, and what is still unproven.

## The decision

**`heic-rs` 0.1.1, behind a `heic` feature that is off by default.** Decode
only. No encoder, and none is planned — encoding HEIC is a different project
with a different answer (see "What this is not").

## Why this crate

The requirements, in the order they mattered:

1. **Nothing that can reach the network.** Hard rule 1 is checked mechanically
   by `scripts/verify.sh`, so this is a constraint on the dependency tree rather
   than a preference. See the tree below.
2. **No C toolchain in the engine.** The engine builds with `cargo build` on four
   targets, one of them two Windows triples. Every extra native dependency is a
   per-target build matrix to get wrong, and phase-08's job is to prove this
   crate compiles everywhere — not to debug a C++ cross-compile.
3. **`#![forbid(unsafe_code)]`.** Every byte of a user's photo passes through
   this decoder, and this project treats memory safety as the product, not as an
   implementation detail.
4. **A header-only read.** `validate_bytes` must be able to enforce `Limits`
   against a HEIC without allocating a pixel buffer, because a HEIC's declared
   geometry is attacker-controlled metadata and a 60000×60000 `ispe` costs 14 GB
   once decoded. `heic_rs::probe()` reads `meta`, `iprp` and `ispe` and stops.
5. **A pixel ceiling the caller sets.** `DecodeOptions::max_pixels` is the seam
   `Limits::max_pixels` is pushed into, rather than a constant we would have to
   work around.

`heic-rs` meets all five, and meets them with **one optional dependency**
(`rayon`, which this engine already has, and which is not enabled here) and a
`no_std` core that does no I/O: `decode()` takes `&[u8]` and returns a value.

## The candidates

Facts below are from `cargo info <crate>` and from the vendored crate sources in
`~/.cargo/registry/src`, checked on the day this was written rather than recalled.

| Crate | Version | Licence | New crates in the tree | Build complexity per target | Android + iOS from one source |
| --- | --- | --- | --- | --- | --- |
| **`heic-rs`** (chosen) | 0.1.1 | MIT OR Apache-2.0 | **none** | `cargo build` only. No build script, no C, no system libraries. rust-version 1.85, the same floor as `core`. | Yes: pure Rust, no target-specific configuration, `no_std` core even builds for wasm. No build script and no `target_arch` conditionals anywhere in its 16k lines. |
| `libheif-rs` | 3.0.0 | MIT | `libheif-sys`, `system-deps`, `cmake`, plus `vcpkg` on Windows | **A C++ build.** The default path resolves libheif with `system-deps`/pkg-config, so the machine needs libheif installed; `embedded-libheif` switches to a `cmake` build of 5.5 MB of vendored C/C++ sources, which then needs an HEVC decoder (libde265 or ffmpeg) present too. `use-bindgen` adds clang. On this runner `pkg-config --exists libheif` fails, so the default path does not even configure. | Plausible in principle, and that is the argument for it: libheif is the reference implementation and is what most phones' hardware decodes. In practice it means a C++ cross-compile in the NDK and Xcode build phases, four times, before any Rust runs. |
| `heic` | 0.1.6 | **AGPL-3.0-only OR commercial** | 13 crates: `archmage`, `magetypes`, `enough`, `whereat`, `rgb`, `zenflate`, `zencodec`, `imgref`, `garb`, `bytemuck`, `memchr`, `safe_unaligned_simd`, and optional `rav1d-safe`/`rayon` | Pure Rust, and the most capable of the three (AV1 too). | Yes, technically. **Rejected on licence:** AGPL-3.0 is denied by omission from `deny.toml`'s allow list, and the alternative is a commercial licence. A photo resizer under AGPL is a different product with different obligations, and that is not a decision one phase makes. |
| `oxideav-heif` | 0.0.9 | MIT | `oxideav-core`, `oxideav-h265`, `oxideav-h264`, `oxideav-av1`, `compcol` | Pure Rust, no C. | Yes. Rejected for two reasons that are about risk rather than capability: it is pre-1.0 (0.0.9, with four of its five dependencies themselves at 0.0.x/0.1.x), and its container API is built for demuxing and muxing as well as decoding — a much larger surface to depend on for "give me pixels". It is also a registry-based design, where an item is only decodable if the right sibling crate is registered, which is more ways to get a runtime `Unsupported` than this product wants. |

`heic-decode` — the name in the phase prompt — **does not exist on crates.io**
(`cargo search heic-decode` finds nothing under that name; the pure-Rust
alternative people mean is `heic-rs`). Recorded here because a phase prompt
naming a crate that is not there is worth correcting in writing rather than in a
commit message nobody reads later.

### The one thing `libheif-rs` wins on

It is worth being honest about this, because it is the reason to revisit the
decision: **libheif decodes more real-world files.** It has years of
format-quirk fixes from being the reference implementation, and it reads HEVC
profiles, tiles and layers this crate refuses. heic-rs decodes intra still
pictures only, 4:2:0/4:2:2/4:4:4/monochrome, 8 and 10-bit.

The counter is that "reads slightly more files" is worth much less than "builds on
a phone without a C++ cross-compile in the critical path", and a file it cannot
read now produces a specific message naming what it cannot read. If a user
report ever shows a real iPhone photo being refused, this comparison is the place
to reopen.

## The dependency tree

`heic-rs` adds nothing to the tree. This is the whole of what the feature adds:

```console
$ cargo tree --manifest-path core/Cargo.toml --features heic --depth 1
pixelsmith_core v0.1.0 (/…/core)
├── base64 v0.23.1
├── heic-rs v0.1.1
├── image v0.25.10
├── kamadak-exif v0.6.1
├── rayon v1.12.0
├── serde v1.0.229
├── serde_json v1.0.151
├── thiserror v2.0.21
└── zip v6.0.0
```

`heic-rs` itself has exactly one dependency, `rayon`, and it is optional and not
enabled here:

```console
$ cargo tree --manifest-path core/Cargo.toml -p heic-rs --features heic --prefix none
heic-rs v0.1.1
```

The grep the phase asks for, on the full tree with the feature on:

```console
$ cargo tree --manifest-path core/Cargo.toml --features heic --prefix none \
    | awk '{print $1}' | sort -u \
    | grep -E 'reqwest|hyper|tokio|std::net'
$ echo $?
1
```

Nothing. `scripts/verify.sh` runs the stricter version of this check (twenty-odd
crate names, plus a `grep` for networking symbols in `core/src`) on every
verification.

## What is in, and what is not

In, and tested in `core/src/heic.rs`:

- Detection by `ftyp` container brand — `heic`, `heix`, `heim`, `heis`, `hevc`
  (reported as HEIC) and `mif1`, `msf1` (reported as HEIF). AVIF-in-HEIF is
  reported as AVIF, so the UI can name what it cannot open.
- Header-only geometry, bounded by `Limits` before any allocation.
- Decode of HEVC intra still pictures to RGBA, including a `grid` derivation
  (the tiled layout iPhones use for very large photos) and `irot`/`imir`/`clap`
  transforms, and an auxiliary alpha plane when the file has one.
- `OutputFormat::Heic` and `OutputFormat::Heif` as **read-only**: `is_read_only()`
  is true, `encode()` refuses them with a message naming the formats to use
  instead, and a test asserts the flag and the behaviour cannot drift apart.

Not in, and refused **by name** rather than as a broken file:

- AV1-coded HEIF. `heic-rs` decodes HEVC only. An `.avif` in a HEIF container is
  recognised as AVIF and gets a message saying the picture coding is not HEVC.
  Phase-07 adds AVIF *encoding*; AV1 *decode* would be a second codec.
- `iovl` overlay derivations and still-image sequences.
- Inter prediction, P and B slices. A still-picture decoder does not need them
  and pretending otherwise is how a codec grows an attack surface nobody tested.

Deliberately not in:

- **Colour management.** A file with an ICC profile or an nclx `colr` box is
  decoded with its declared matrix and range, and the profile itself is not yet
  applied to the output. That is phase-12, and it is a visible gap: a
  Display-P3 iPhone photo will come out looking washed out until then.
- **Metadata extraction.** `exif::read` cannot see a HEIF's `Exif` item — it
  lives in a separate container item, not in a JPEG APP1 segment. `heic::header`
  reports `has_exif` from the container so `ValidateReport` is honest, but the
  tags themselves are not parsed yet, so a HEIC's GPS is not being *read* (it is
  also not carried through: the pipeline re-encodes from raw samples, and
  `exif::write_back` only ever writes from `exif::read`, which sees nothing).
  Reading them properly is part of phase-12's colour/metadata work.
- **Encoding.** `OutputFormat::Heic` and `Heif` are read-only. HEVC encoding in
  pure Rust does not exist at a quality anyone would ship, and libheif's encoder
  would mean the C++ toolchain this phase exists to avoid.

## Why the feature is off by default

The prompt is explicit — do not make it default until phase-08 proves it builds on
all four shipped targets — and the reasoning stands on its own. `--all-features`
is what CI builds, so the feature is *compiled and tested* on every run; it is
simply not in the default set that a released APK and EXE are built from, because
no CI job has yet cross-compiled it for Android, iOS, Windows and Linux.

Turning it on is one line in `core/Cargo.toml`:

```toml
[features]
default = ["heic"]
```

Phase-08 does that, and phase-08's gate is "the APK, the EXE and both Apple
targets link a binary that decodes a HEIC", not "the code compiles".

## What phase-08 still has to prove

- `aarch64-linux-android`, `aarch64-apple-ios`, `x86_64-pc-windows-msvc` and
  `x86_64-unknown-linux-gnu` all cross-compile with the feature on. Nothing about
  `heic-rs` suggests they will not — it is `no_std`-clean Rust with no build
  script — but "should" is not "verified", and the reason the feature is off by
  default is precisely that this has not been checked.
- The decode is serial per file (`DecodeOptions::threads = Some(1)`) so it does
  not nest a rayon pool inside the batch pool. On a phone that is the right
  default, but a 12 MP HEVC intra frame is slow single-threaded; phase-09's SIMD
  work and phase-10's benchmarks are where that gets looked at.
- An APK that is meaningfully larger with HEIC in it, and by how much.

## Reproducing the comparison

```console
$ cargo info heic-rs && cargo info libheif-rs && cargo info heic && cargo info oxideav-heif
$ cargo search heic-decode        # not a crate
$ cargo tree --manifest-path core/Cargo.toml --features heic
$ bash scripts/verify.sh
```