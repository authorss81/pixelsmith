# Changelog

Every release, newest first. The format is [Keep a
Changelog](https://keepachangelog.com/en/1.1.0/), the versions are SemVer, and
the version is not written here by hand and then copied: `scripts/check-version.sh`
compares the newest heading below against `core/Cargo.toml` and
`app/pubspec.yaml` and fails the gate if they disagree.

## [0.1.0] — first runnable build

An Android APK and an Android app bundle, plus the recipe and the checks for a
portable Windows ZIP. The engine underneath is the real thing; the user
interface on top of it is not, and the section below says exactly how far that
goes.

### Shipped

**The engine** (`core/`, Rust, `cdylib` + `staticlib`, zero network capability in
the dependency tree — [checkable](https://github.com/authorss81/pixelsmith#security)):

- Decode JPEG, PNG, WebP, GIF, TIFF, BMP, ICO, and HEIC/HEIF behind the `heic`
  feature.
- Encode JPEG, PNG, WebP, GIF, TIFF, BMP, ICO, and AVIF; **lossy WebP** through
  libwebp, so WebP has a quality slider and a byte ceiling instead of being an
  also-ran.
- Target-size mode: give it a byte ceiling and it binary-searches quality to land
  under it, returning the highest quality that fits. Converges in about seven
  encodes.
- Five resampling kernels, five fit modes, **exactly one** resampling pass, in the
  fixed order crop → orient → resize.
- Progressive JPEG and 4:2:0 / 4:2:2 / 4:4:4 chroma subsampling, defaulting by
  content (4:2:0 for photographs, 4:4:4 for screenshots).
- EXIF read, and **stripped by re-encoding** rather than by clearing tags, so
  maker notes and embedded thumbnails cannot survive. GPS is never written back.
- sRGB and Display-P3 colour management: the ICC profile is read out of JPEG,
  PNG and WebP, classified by its colorants, and the pixels are converted into
  the working space before any geometry runs.
- Animated GIF handled honestly: an animation exported as a GIF keeps every frame
  and its delay; exported as anything else it is **refused in a sentence** naming
  the frame count and the format that would have kept it.
- Folder batches with a preview, per-file accounting, cancellation, ZIP output,
  content-hash deduplication, and three distinct skip reasons a user can act on.
- Bounded on every decode path: a 60000×60000 header is rejected from the header,
  before any pixel buffer exists.
- Format comes from magic bytes. A file called `photo.png` containing JPEG is a
  JPEG.

**The app** (`app/`, Flutter, over a C ABI with a hand-written `dart:ffi` binding
that a contract test holds against the engine's ABI dump):

- A window that opens and proves the FFI boundary end to end.
- Every engine buffer owned and freed in a `finally`; every call on a worker
  isolate, so a two-second decode cannot drop a frame.

**The release machinery** (this phase):

- One version, checked in four places — `core/Cargo.toml`, `app/pubspec.yaml`,
  this file's newest heading, and the release tag — by `scripts/check-version.sh`,
  which runs in `verify.sh` and in the release workflow.
- `scripts/verify-release-artifact.sh` inspects the built artefact rather than
  trusting that the build printed a success line: a `classes.dex`, an
  `libpixelsmith_core.so` exporting a real entry point for **every** ABI that
  ships a `libflutter.so`, an ELF machine type that matches the ABI it was filed
  under, a `minSdkVersion` at or above the engine's floor, and — for the Windows
  bundle — `pixelsmith_core.dll` sitting next to the EXE.
- SHA-256 checksums for every published artefact, in `scripts/RELEASE-SHA256.txt`
  and in the release body.

### Still missing — what a user cannot yet do

This is the section that matters. A release that overstates itself is worse than
no release, because the person who finds out is not the person who shipped it.

**The user interface is a shell.** The app opens, calls `px_version()` and prints
the engine's version and two capability flags. There is no picker, no preview, no
sliders, no batch list, no export button. **You cannot resize a photograph with
this app.** The engine can do it — `px_process` is a complete, tested API and the
`Requests`/`ProcessResult` Dart models mirror it — but no screen calls it yet.
Every UI item in `ROADMAP.md` is unbuilt.

**The Android build is not Play-distributable.** There is no upload keystore in
this repository and there will not be one: signing material is the repository
owner's to hold. Until `KEYSTORE_BASE64` and friends are set as Actions secrets
and `.github/workflows/release.yml` is applied, the release APK is **debug-signed
with the standard Android debug key**. It installs on a device with USB debugging
on. Google Play will not accept it. Exactly which secrets are needed is in
[docs/RELEASE.md](docs/RELEASE.md).

**The Windows build cannot be produced from a Linux runner.** `flutter build
windows` is MSBuild-based and will not cross-compile. The portable ZIP is built by
the `windows-bundle` job in `.github/workflows/release.yml`, on a Windows runner,
which is a patch away for the same credential reason as every other workflow
change in this project (see below). Until it lands, the Windows artefact has
`NOT-BUILT` rather than a hash in `scripts/RELEASE-SHA256.txt`, and that is a fact
about the file, not an omission in it.

**The Windows binary is unsigned.** A portable ZIP with no installer, no admin
rights and no registry writes is what this project ships, and an unsigned EXE is
what Windows SmartScreen warns about. There is no code-signing certificate here
and no reason to pretend otherwise; the README says so where a user will see it.

**`app/` is a separate repository this pipeline cannot push to.** `app/` is the
submodule `authorss81/shrinkray`, and the only credential available to the
pipeline has write access to `authorss81/pixelsmith`. Every app-side change in
this phase is therefore delivered as
**`workspace/phase-16/shrinkray-phase-16.patch`**, which applies cleanly to the
pinned commit `5d0e9cc`. The same is true of the JSON-contract patches left
undelivered by phase-05, phase-06, phase-07, phase-13 and phase-14: **the Dart
models do not yet mirror the engine's colour, animation or skip-reason fields**,
so the app cannot render a colour conversion, report dropped frames or explain a
skip even once there is a screen that would. Each phase's notes name its patch and
the one-hunk conflicts between them.

**`.github/workflows/` changes are also a patch.** Pushing one is refused without
the `workflows` permission, and granting that permission is itself a workflow
push, so there is no self-resolving path. `workspace/phase-16/release-ci.patch`
carries the release workflow and the release-grade jobs in `build.yml`.

**What the engine still cannot do**, from its own `capabilities()` rather than
from this document:

| | |
| --- | --- |
| AVIF **decode** | Recognised on input and named in an error message; there is no AV1 decoder in any configuration, so `avif_decode` is `false`. This build can write an AVIF and cannot read one back. |
| HEIC decode | Off by default. The `heic` feature is not cross-compilation-tested on any shipped target, so enabling it by default would be a promise nobody has checked. |
| SIMD resize | Behind `simd`, off by default. 2.6×–19.9× on x86-64 AVX2, **unmeasured on the ARM phone this ships to**. |
| Low-peak-memory decode | Behind `streaming`, off by default, and only real for PNG: a 120 MP PNG to 1000 px wide is a 4 MB job with it on and a 613 MB one with it off. |
| Perceptual deduplication | Two lossy re-encodes of one photograph are two files. A missed duplicate appears in the report; a wrong merge would not. |
| WebP/AVIF progressive and chroma | JPEG-only. libwebp's config has no chroma sampling factor and `avif`'s is unwired. |
| Animated output | GIF only. WebP and AVIF both carry animation and neither is written. |
| Folder preview over FFI | `folder::plan` exists and is tested; there is no `px_*` entry point for it, so the app cannot show a folder preview yet. |

**Seven of eight build targets have no reproducibility measurement**, and
libwebp's vendored C cannot be made reproducible across two Linux distributions by
our flags at all. One target — `x86_64-unknown-linux-gnu` — is measured and
reproducible. See [docs/SUPPLY-CHAIN.md](docs/SUPPLY-CHAIN.md).

### Security

See [SECURITY.md](SECURITY.md) for the disclosure route and
[docs/SECURITY.md](docs/SECURITY.md) for the threat model. The one-line version:
the engine has no HTTP, TLS, socket or DNS crate in its dependency tree, so it
cannot upload a photograph even if a future version of it wanted to.

[0.1.0]: https://github.com/authorss81/pixelsmith/releases/tag/v0.1.0