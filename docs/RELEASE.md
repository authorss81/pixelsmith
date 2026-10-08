# Cutting a release

What a person does to turn `main` into an APK, an app bundle and a portable
Windows ZIP that someone can download. The engine half of this is automated and
checked by `scripts/verify.sh`; the app half is in another repository, and two
things in the list below need a credential this pipeline does not have.

---

## 0. What a release is, and what it is not

| Artefact | What it is | Play-distributable? |
| --- | --- | --- |
| `pixelsmith-<version>-android-arm64.apk` | Release-mode APK, one ABI, for sideloading | no — needs the upload key |
| `pixelsmith-<version>-android-universal.apk` | Release-mode APK, every ABI | no |
| `pixelsmith-<version>-android.aab` | App bundle, what Play Store requires | yes, once signed with the upload key |
| `pixelsmith-<version>-windows-x64.zip` | Portable Windows bundle, zipped | n/a |

**An APK is not enough for Play.** Google Play requires an Android App Bundle,
and no amount of signing turns an APK into one. Both are produced here; the AAB
is the one that matters for distribution.

## 1. Prerequisites

- A clean tree on `main`, with `bash scripts/verify.sh` printing `VERIFY: PASS`.
- `git tag` push access, and a GitHub token with `contents: write` for the
  release itself.
- For a *Play-distributable* build, the five secrets in §4. Without them the
  Android artefacts are produced **debug-signed** and the run says so.

## 2. The version, and why there is a check for it

The version lives in two files and a tag, and it has to be the same number in all
three:

| Where | Example |
| --- | --- |
| `core/Cargo.toml` `[package] version` | `0.1.0` |
| `app/pubspec.yaml` `version:` | `0.1.0+1` — the part after `+` is the Android `versionCode` and the Windows build suffix, and is **not** part of the version |
| `CHANGELOG.md` newest `## ` heading | `## [0.1.0]` |
| the tag | `v0.1.0` |

`scripts/check-version.sh` compares all four and is in `verify.sh`, so a
mismatch is a red push rather than a wrong release. To bump:

```bash
# 1. core/Cargo.toml
sed -i '0,/^version = /s/^version = .*/version = 0.2.0"/' core/Cargo.toml
# 2. app/pubspec.yaml — raise the build number too, or Play rejects the upload
sed -i 's/^version:.*/version: 0.2.0+2/' app/pubspec.yaml
# 3. CHANGELOG.md — new heading at the top
# 4. prove it
bash scripts/check-version.sh && bash scripts/check-version.sh --self-test
```

The build number after the `+` must increase on every upload to Play. It is
deliberately excluded from the equality check for exactly that reason: it changes
on every release while the version does not.

The scheme is SemVer, and it was not chosen in this phase — both manifests
already said `0.1.0` before it started. `docs/ARCHITECTURE.md` records the
decision and the reasoning.

## 3. Build and verify, on the runners that can

### Android — Linux runner

```bash
# 1. The engine, for the three ABIs Flutter packages libflutter.so for.
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install cargo-ndk
( cd core && cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 \
    -o ../app/android/app/src/main/jniLibs build --release --lib )

# 2. The app. Release mode, so the Dart is AOT and the assertions are real.
( cd app && flutter build apk --release )
( cd app && flutter build appbundle --release )

# 3. Open the artefacts and assert what is inside them.
bash scripts/verify-release-artifact.sh apk \
  app/build/app/outputs/flutter-apk/app-release.apk \
  --abi-source app/android/app/src/main/jniLibs
bash scripts/verify-release-artifact.sh aab \
  app/build/app/outputs/bundle/release/app-release.aab
```

Step 3 is not optional politeness. An APK with no native library installs, opens
and throws at the first resize, and no build log says so — the engine going
somewhere Gradle does not read is the single most likely way for this project to
publish a shell that looks like an app.

### Windows — Windows runner, and nowhere else

`flutter build windows` is MSBuild-based and does not cross-compile. On
`windows-latest`:

```powershell
cd core; cargo build --release --lib
# CMake installs the DLL from windows/runner, and `flutter build windows`
# ignores one that is merely sitting there, so this copy is the load-bearing
# step rather than the one above.
Copy-Item target\release\pixelsmith_core.dll ..\app\windows\runner\ -Force
cd ..\app; flutter build windows --release
Compress-Archive -Path build\windows\x64\runner\Release\* `
  -DestinationPath ..\artifacts\pixelsmith-<version>-windows-x64.zip
```

Then, on any runner:

```bash
bash scripts/verify-release-artifact.sh windows artifacts/pixelsmith-0.1.0-windows-x64.zip
```

which asserts `pixelsmith_core.dll` is **next to** the EXE, not merely in the
archive. `flutter_windows.dll` is found by `DynamicLibrary.open` relative to the
executable's own directory, so a DLL in a subdirectory is a DLL that will not be
found, and the failure is a load error on the first call.

## 4. The signing secrets, and what happens without them

**Nothing secret is committed to this repository.** No keystore, no `.jks`, no
`key.properties`, no password. `.gitignore` lists them and `scripts/verify.sh`
fails if one is ever tracked.

Add these as Actions secrets (**Settings → Secrets and variables → Actions**):

| Secret | What it is |
| --- | --- |
| `ANDROID_KEYSTORE_BASE64` | `base64 -w0 upload-keystore.jks`. The upload key, which signs the AAB. Generate it once: `keytool -genkey -v -keystore upload-keystore.jks -keyalg RSA -keysize 2048 -validity 10000 -alias upload` |
| `ANDROID_KEYSTORE_PASSWORD` | Its password |
| `ANDROID_KEY_ALIAS` | `upload` |
| `ANDROID_KEY_PASSWORD` | The key's password, if it differs |
| `GITHUB_TOKEN` | Built in; needs `contents: write` for the release step |

The workflow decodes the keystore into the runner's temporary directory, writes
`android/key.properties`, runs the build, and deletes both. Nothing survives the
job.

**Without the secrets the build still runs, and the artefact is debug-signed.**
That is deliberate. A pipeline that fails closed on a missing secret means the
repository owner, who is the only person who can add one, gets a red run instead
of a release; a pipeline that publishes a debug-signed APK with "install with USB
debugging on" written on it means every contributor and every reviewer can
install the thing they just changed. The run's summary says which of the two
happened, and `docs/TROUBLESHOOTING.md` has the table.

What a debug-signed release APK **cannot** do: install on a device without USB
debugging, and be uploaded to Play. Nothing else differs — the code is release
mode, the Dart is AOT-compiled, and every assertion in §3 applies identically.

## 5. Checksums, the SBOM and the release

```bash
# One row per artefact, including the ones not built, so the absence is in the
# file rather than inferred from it.
bash scripts/release-checksums.sh --write scripts/RELEASE-SHA256.txt \
  artifacts/* --version 0.1.0

# Fail if a published artefact's hash is not in the table.
bash scripts/release-checksums.sh --check scripts/RELEASE-SHA256.txt artifacts/*
```

The engine hashes (`scripts/BUILD-SHA256.txt`) and the artefact hashes
(`scripts/RELEASE-SHA256.txt`) are different tables and answer different
questions: the first is about *whether the build reproduces*, the second is about
*whether the file you downloaded is the file we built*. Neither is derived from
the other. See [SUPPLY-CHAIN.md](SUPPLY-CHAIN.md) §5.

The SBOM is produced by the `sbom` job in `.github/workflows/supply-chain.yml`
(CycloneDX, from `core/Cargo.lock`) and by `scripts/sbom.py`. Download the
workflow's artefact and attach it to the release: **the release body must contain
the SHA-256 of every file attached to it.**

```bash
gh release create v0.1.0 \
  artifacts/pixelsmith-0.1.0-android-arm64.apk \
  artifacts/pixelsmith-0.1.0-android.aab \
  artifacts/pixelsmith-0.1.0-windows-x64.zip \
  artifacts/SHA256SUMS.txt \
  artifacts/pixelsmith-sbom.cdx.json \
  --draft --title "v0.1.0" --notes-file artifacts/RELEASE-NOTES.md
```

`--draft` on purpose. Publishing is a human decision: the release body has a
"still missing" section, and the person who reads that section should be the
person who decides to publish.

The notes are the "Still missing" section of [CHANGELOG.md](../CHANGELOG.md),
which is written to be pasted in unchanged.

## 6. What the release workflow is, and why it is a patch

`.github/workflows/release.yml` does §3, §4 and §5 on a tag push, and
`build.yml` gains the release-grade Android and Windows jobs. Neither can be
pushed from this pipeline:

```console
$ git push origin main
 ! [remote rejected] main -> main (refusing to allow a GitHub App to create or
   update workflow `.github/workflows/build.yml` without `workflows` permission)
```

Granting that permission is itself a workflow push, so there is no self-resolving
path. Both are committed as **`workspace/phase-16/release-ci.patch`** and apply
with a plain `git apply`. Until then, every step above is a command a person
types, which is a worse release process and a correct one.

## 7. After the release

- Fill any `NOT-BUILT` row in `scripts/RELEASE-SHA256.txt` from the artefact the
  workflow actually produced, so the next reader sees a hash rather than a gap.
- If a target has become measurable, fill its `NOT-MEASURED` row in
  `scripts/BUILD-SHA256.txt` and its `not measured` row in
  [SUPPLY-CHAIN.md](SUPPLY-CHAIN.md) §1. That table's whole value is that its
  blanks are real.
- `docs/phase-status.md` gets the phase's row updated, and the next phase prompt
  gets the release's known gaps in its "Do not" list.