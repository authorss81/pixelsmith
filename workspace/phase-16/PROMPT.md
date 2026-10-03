# phase-16 — Release artefacts: the APK and the EXE

**Track:** A — Engine & Security
**Depends on:** phase-15
**Timeout:** 90 minutes

## Objective

Turn a repository of source into two things a person can download and run: an **Android APK** and a **Windows EXE**. Everything before this phase builds a library and a test suite; this phase is where the project becomes software someone can actually install.

`.github/workflows/build.yml` already exists and already produces both on every push, debug-signed or unsigned. This phase makes them release-grade: signed, versioned, checksummed, attached to a tagged GitHub Release, and verified to contain the native engine rather than an empty shell.

## Read first

- `AGENTS.md` — hard rule 10. A binary that cannot do what the UI claims is worse than no binary.
- `.github/workflows/build.yml` — what already works. Do not rebuild it from scratch.
- `app/README.md` — written in phase-05; it has the exact native library commands.
- `docs/SUPPLY-CHAIN.md` — the reproducibility result, if phase-15 produced it.

## Scope

### Do

**Version and identity**

- One source of truth for the version, and it must agree everywhere. The engine is `core/Cargo.toml`; the app is `app/pubspec.yaml`; the release tag is `vX.Y.Z`. Add a check that fails when they disagree, in `scripts/verify.sh` and in CI. Three places to update is three places to forget.
- Set `applicationId` / `bundle id` to something stable and reverse-DNS. Do not leave a template default.
- The app name, description and version shown to the operating system must match the repository, not a placeholder.

**The APK**

- Produce a **release** APK via `flutter build apk --release`, and an **app bundle** (`.aab`) via `flutter build appbundle`, because Play Store distribution requires an AAB and an APK alone cannot be published there.
- Sign it. Use a GitHub Actions secret keystore. **Never commit a keystore, a password, or a `key.properties` file** — verify with `git ls-files` that none exists in the tree before you finish.
- Because signing secrets belong to the repository owner, the phase must not fail when they are absent. Generate an upload-key-free debug-signed release APK, attach it, and open an issue explaining exactly which secrets to add for a Play-distributable build. Failing closed on a missing secret is correct; failing the whole pipeline is not.
- Verify the artefact rather than trusting that the build printed "Built". Unzip it and assert: a `classes.dex` exists, at least one `lib/*/libpixelsmith_core.so` exists for each declared ABI, and the manifest's `minSdkVersion` is one the engine actually supports. An APK with no native library opens and then crashes on first tap, which is the worst possible failure mode.
- Declare the exact ABIs in `abiFilters` in the Gradle config, and assert the count matches the ABIs the Rust library was actually built for. A mismatch here is the classic "works on my device" Android bug.

**The EXE**

- Produce `flutter build windows --release`, which emits a bundle, not a single file: the EXE, the Flutter engine DLLs, `flutter_windows.dll`, `data/` and `pico/*.dll`. **Ship the whole directory**, zipped. An EXE without its DLLs does not start.
- Decide on and implement one of: MSIX, or a portable ZIP. The roadmap asks for a portable ZIP so it runs without an installer, so do that — and say in the README that Windows SmartScreen will warn on an unsigned binary, because it will.
- Assert `pixelsmith_core.dll` is present in the bundle and sits next to the EXE. A missing DLL here means the app shows a load error on startup.
- Verify the EXE is not implausibly small. A Flutter release EXE under about 100 KB means the engine failed to link, and the size check catches that before a user does.
- No installer, no admin rights, no registry writes. Run it from a USB stick and it must work.

**Reproducibility and provenance**

- Publish SHA-256 checksums for every artefact, in the release body and in `docs/SUPPLY-CHAIN.md`.
- Attach the SBOM from the `supply-chain` workflow to the release.
- Write a `CHANGELOG.md` at the repository root for this release: what shipped, what is still missing, and what a user cannot yet do. The "what is still missing" section is the important one.

**The release itself**

- Create a GitHub Release from the tag, with the APK, the AAB, the portable Windows ZIP, the checksums and the SBOM attached. Use `softprops/action-gh-release` or `gh release create`.
- Tag as `vX.Y.Z` where X.Y.Z is the agreed version. Do not invent a version scheme during this phase; ask if none has been decided, and record the decision in `docs/ARCHITECTURE.md`.

### Do not

- Do not commit a keystore, a `.jks`, a `key.properties`, or any password. Check `git ls-files` and paste the result in the commit message.
- Do not upload to the Play Store or anywhere else. This phase produces artefacts and a draft release; publishing is a human decision.
- Do not add a network capability to satisfy a build step. If a tool wants to phone home, choose a different tool.
- Do not "fix" a failing build by loosening a check. If the engine will not cross-compile to a target, say so in `CHANGELOG.md` and drop that target explicitly rather than shipping something that half-works.
- Do not delete the `build.yml` jobs for Android and Windows. They are the regression check that stops this phase from silently breaking in three weeks.

## Acceptance criteria

### Machine-checkable

- `flutter build apk --release` and `flutter build appbundle --release` both succeed, or the phase documents precisely which secret is missing and why the artefact is debug-signed instead.
- `flutter build windows --release` succeeds, and the bundle contains `pixelsmith_core.dll` next to the EXE.
- A test or CI step asserts the APK contains `classes.dex` and at least one `lib/*/libpixelsmith_core.so`.
- `git ls-files | grep -Ei '\.(jks|keystore)$|key\.properties'` returns nothing. Paste the output in the commit message.
- The version in `core/Cargo.toml`, `app/pubspec.yaml` and the release tag are identical, enforced by a check.
- SHA-256 checksums exist for every published artefact.
- A tagged GitHub Release exists with the APK, AAB and portable Windows ZIP attached.

### Needs a human judgement

- Have you actually installed the APK on a device or emulator and the ZIP on a clean Windows machine, or only inspected the CI log? Say which.
- Does the README tell a first-time user where to download, what format to pick, and why SmartScreen will warn them?
- Would you trust this release with a photo you would not want uploaded?

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha.
- Committed and pushed to `main`. The release tag is pushed.
- Do not write `.done`. The workflow does.