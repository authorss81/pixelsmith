# phase-05 findings — the Dart boundary, verified

`workspace/phase-05/PROMPT.md` asked for every acceptance criterion to be checked
against what actually exists. This is what checking them found.

## The blocker

**`app/` is a submodule pointing at `authorss81/shrinkray`, and this phase cannot
push to it.** The commit identity available here is `github-actions[bot]` with
write access to `authorss81/pixelsmith` only:

```console
$ git -C app push --dry-run origin main
remote: Permission to authorss81/shrinkray.git denied to github-actions[bot].
fatal: ... The requested URL returned error: 403
```

`AGENTS.md` is unambiguous about the consequence: *"Commit inside the submodule
first, push it, then commit the pointer update here. A superproject commit
pointing at an unpushed submodule SHA is broken for everyone else."* So the
submodule stays at `3ed1eb4`, everything below it is delivered as
`shrinkray-phase-05.patch` in this directory, and the phase is recorded as
BLOCKED rather than DONE.

**To land it:**

```bash
cd app                       # or a fresh clone of authorss81/shrinkray
git apply ../workspace/phase-05/shrinkray-phase-05.patch
flutter pub get && dart format lib test && flutter analyze
# build the engine first, or the FFI tests skip:
bash native/build-engine.sh --engine-dir /path/to/pixelsmith
flutter test
git commit -m "..." && git push
```

The patch was validated in this repository's checkout of the submodule against
the engine built from the current `core/`: `flutter analyze` reports no issues,
`flutter test` reports 34 passed and **0 skipped**.

## What was wrong

Every item below was found by running the thing, not by reading it. All seven
are fixed in the patch.

### 1. `px_inspect` is declared with the wrong arity — memory-unsafe

`core/src/ffi.rs` has taken three arguments since it was written:

```rust
pub unsafe extern "C" fn px_inspect(ptr: *const u8, len: usize, mobile_limits: bool) -> PxBuffer
```

`bindings.dart` declared two, so a Dart call passed a `bool` where the engine's
second `usize` belongs, and the phone limit profile was unreachable from the app
— the one thing `mobile_limits` exists for.

This survived because nothing called it: the contract test exercised
`px_version`, `px_selftest_error`, `px_selftest_panic` and `px_buffer_free`, and
those four were the only entry points it touched. **9 of the 15 entry points were
never called from Dart at all.**

### 2. The contract test never ran

`ffi_contract_test.dart` skipped itself when the library could not be loaded, and
`DynamicLibrary.open('libpixelsmith_core.so')` searches the *loader path*, not the
working directory. The engine builds to `app/src/rust/`, which is neither, so on
every run before this phase:

```
🎉 13 tests passed, 4 skipped.
```

Four of the seventeen tests were the FFI ones. A skipped test is green.

### 3. `flutter test` fails against a release engine, while reporting success

`[profile.release]` sets `panic = "abort"` (documented in
`docs/ARCHITECTURE.md`, gotcha 8). Against a release library:

```console
$ LD_LIBRARY_PATH=src/rust flutter test
🎉 15 tests passed.
TestDeviceException(Shell subprocess crashed with SIGABRT (-6).)
$ echo $?
1
```

`px_selftest_panic` aborts the test device. The README told a developer to build
with `--release` and then run `flutter test`, in that order.

### 4. The app could not find its own engine on desktop

`loadEngineLibrary()` only ever called `DynamicLibrary.open` with a bare file
name. That works on Android, where the `.so` is extracted from the APK, and
nowhere else. There is also no desktop glue at all: `app/linux/CMakeLists.txt`,
`app/windows/CMakeLists.txt` and the macOS project contain no reference to the
engine, so nothing builds it and nothing copies it into the bundle.

### 5. `build-engine.sh` built for Android when the environment said so

The target was inferred from `ANDROID_NDK_HOME` being set. GitHub's runners set
it, so on a Linux runner the script cross-compiled for Android — and failed,
because that runner has no Android Rust target installed — while the developer
asked for a desktop build. Android is now opt-in with `--target android`.

### 6. `px_process` truncated any request whose name was not ASCII

`engine.dart` passed `request.length` — UTF-16 code units — as the byte length
of a buffer it had filled with `utf8.encode(request)`. `café.jpg` produced a
truncated envelope and a "bad request" from the engine. The patch passes
`body.length`, and there is a test named after the failure.

### 7. `px_zip` would have been broken by the obvious refactor

`_zipStatic` had its own copy of the buffer-handling logic because
`_takeDecoded` `jsonDecode`s the payload, and a ZIP archive is not JSON. Worth
naming because the duplication looks like redundancy.

## What landed here instead

The prevention machinery belongs in the repository that owns the ABI, so it is
in `core/`, not in the patch:

- **`core/src/ffi_abi.rs`** declares every entry point once. Each row expands to
  a `const _: unsafe extern "C" fn(..) -> ..` assignment, so an arity change in
  `ffi.rs` that is not made here is a **build failure**. Verified by deleting a
  parameter and watching `cargo build` fail.
- **`px_abi_layout()`** publishes `PxBuffer`'s offsets computed with
  `offset_of!`, so the Dart test can compare its struct declaration against the
  library it loaded instead of against a constant somebody typed.
- **`cargo run --bin px-abi-dump -- --check <bindings.dart>`** reports every
  arity that has drifted, and **`scripts/check-dart-bindings.sh`** wraps it.
- **`ffi::tests::a_thousand_rounds_of_the_boundary_leak_nothing`** runs the whole
  boundary 1000 times and asserts the engine's own table of outstanding buffers is
  empty afterwards. Verified to fail when one `px_buffer_free` is removed.
- **`scripts/verify.sh`** builds the debug engine, puts it on the loader path,
  and fails if any Flutter test skipped. The gate previously certified a boundary
  it had not executed.

Two known-drift entries are listed in `ffi_abi.rs` rather than waved through:
`px_inspect`'s arity and the missing `px_abi_layout` declaration. When the patch
lands they go stale, and the test that compares them fails until they are removed.
A test that skipped the comparison instead would stay green forever and catch
nothing.

## Left for whoever can push

- Apply the patch, push, then update the submodule pointer here.
- The desktop CMake/MSBuild glue: build the engine as part of the desktop build
  and install the artefact into the bundle. It does not exist in any form.
- `KNOWN_DART_DRIFT` in `core/src/ffi_abi.rs` must be emptied once the patch
  lands, or the engine test fails on purpose.