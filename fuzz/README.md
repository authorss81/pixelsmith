# Fuzzing

Eleven `cargo-fuzz` targets, one per entry point where untrusted bytes become
something this engine does. They are in this repository because a decoder is
only as good as the worst file somebody has ever handed it, and a corpus nobody
has thrown bytes at is a corpus that has not found anything yet.

## Running them

`scripts/fuzz.sh` is the only supported way to build or run a target. A fuzz
target is a normal cargo binary linked against libFuzzer, so `cargo run --bin
decode_png` would compile it *without* sanitizers and *without* coverage
instrumentation and would quietly fuzz nothing worth finding.

```bash
bash scripts/fuzz.sh list                 # the eleven targets
bash scripts/fuzz.sh build                # build them all (needs nightly + cargo-fuzz)
bash scripts/fuzz.sh run decode_png       # seed, then fuzz one target for PX_FUZZ_SECONDS
bash scripts/fuzz.sh run                  # all eleven, 30s each
bash scripts/fuzz.sh tmin fuzz/artifacts/decode_png/crash-abc t decode_png
```

Prerequisites, both opt-in tooling that the default gate does not need:

```bash
cargo install cargo-fuzz --locked
rustup toolchain install nightly
```

`PX_FUZZ_SECONDS` sets the per-target wall clock (default 30).
`PX_FUZZ_BUILD_STD=1` rebuilds the standard library with `rustfuzz`, which is
slower and catches more; it needs `rustup component add rust-src`.

## The targets, and why each one exists

| Target | Entry point | What a finding means |
| --- | --- | --- |
| `detect_format` | `format::detect_format` | A magic-byte sniffer that panics on a short or malformed header |
| `validate_bytes` | `validate::validate_bytes` | The advisory pass. It runs on a whole folder before the user commits, so it is the highest-multiplier target |
| `decode_bounded` | `lib::decode_bounded` | `Limits` applied, then the decode, then a re-check. The default path for every file |
| `exif_read` | `exif::read` | Parsing attacker-controlled TIFF IFDs, including the truncation cases the hostile corpus in `core/tests/` covers |
| `decode_jpeg` | JPEG decode | `image`'s decoder through our limits |
| `decode_png` | PNG decode | The one format with a row-at-a-time API, and therefore the one the `streaming` feature adds its own arithmetic to |
| `decode_webp` | WebP decode | Lossy and lossless, and the lossless path is `image`'s own encoder's inverse |
| `decode_gif` | GIF decode | A container with a block stream, a local colour table and frame counts (`validate::scan_gif_frames` walks it) |
| `decode_tiff` | TIFF decode | Multi-IFD, and the format EXIF is a dialect of |
| `decode_bmp` | BMP decode | Header-driven, so the classic integer-overflow shape |
| `decode_ico` | ICO decode | A directory of images with an attacker-chosen directory |

## Corpus and seeds

`fuzz/seeds/<target>/` holds committed seeds, regenerated with:

```bash
bash scripts/fuzz.sh seed
```

They come from the engine's own fixture generators (`worker::tests::photo` and
friends), not from photographs: no third-party image whose licence anybody would
have to reason about, and no file a user has to supply to reproduce a crash.

Crashes land in `fuzz/artifacts/<target>/` and stay there — they are not
committed, because `scripts/verify.sh` has no reason to require them and a
missing artefact must not fail the gate.

## What fuzzing has found here

Two defects so far, both in the same place and both found by CI rather than by
reading the code (see `docs/phase-status.md`, phase-04):

- **The sandbox response parser could never have worked.** It compared the length
  prefix against the whole remaining body, but the prefix covers the encoded
  bytes only; the worker appends 9 bytes of metadata. Every successful sandboxed
  decode was rejected.
- **A tight `RLIMIT_AS` makes the write to the worker's stdin fail with `EPIPE`**,
  because the child can die before reading anything. Every memory refusal was
  being reported as "could not send the job".

Both are process-level, not decode-level, which is worth stating plainly: the
eleven targets above have not produced a decode-path panic, and the project's
claim is about panics on untrusted input, so that is the number that matters.

## Not wired into CI

There is no nightly fuzzing job. `.github/workflows/` is a path this repository's
automation credential cannot push to, so a workflow would be a patch nobody has
applied — and a patch that claims nightly fuzzing runs is worse than saying it
does not. `scripts/fuzz.sh run` is the honest current state.

## Reporting

If you find a crash, reduce it with `scripts/fuzz.sh tmin` before reporting. A
corpus entry plus the `Error` you get is far more useful than a 4 MB file.
`docs/SECURITY.md` has the disclosure route.