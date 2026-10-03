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
the engine built from the current `core/`; see *Validation of the patch* below for
the re-run from a clean tree with the commands and their output.

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

## Second attempt: the gate was red, and one scope item was still missing

The first attempt's work verified as far as it went and then failed
`scripts/verify.sh` on one line, so this section is about what that run turned up
and what it added.

### Two defects in this repository, not in `app/`

**`ffi_abi.rs` did not pass `cargo doc -D warnings`.** Its first doc line linked
to `[`ffi`]`, which is not in scope from a sibling module; rustdoc wants
`[`crate::ffi`]`. Same mistake `ffi.rs` made in phase-01 and that phase fixed —
so it was fixed again here, and the lesson repeated rather than learned.

**`cargo fmt --check` in `verify.sh` could not fail.** It was written

```bash
if cargo fmt --all -- --check 2>&1 | head -n 40; then
```

which is the identical defect phase-01 fixed on the `dart format` line one check
later: an `if` condition containing a pipeline takes the exit status of the *last*
command, and `head`'s is always 0. It reported "formatting clean" while printing
rustfmt's own diff — which is why `ffi_abi.rs`, `ffi.rs` and `px-abi-dump.rs` were
committed in a state rustfmt wanted to rewrite. Captured and tested after the
change, and proved in both directions: a deliberately misformatted function makes
it exit 1 and print the diff, a clean tree exits 0.

### The desktop build glue, which the first attempt left undone

`PROMPT.md` asks for `core` to be built from CMake and from the Windows MSBuild
project, copying the artefact where the app expects it. It did not exist in any
form, so the patch now adds it: `linux/CMakeLists.txt` and
`windows/CMakeLists.txt` run `native/build-engine.sh` as an `ALL` target and
`install(FILES ...)` the result into the bundle — `bundle/lib` on Linux, next to
the EXE on Windows. Debug builds get a debug engine, everything else gets
`--release`, via a `$<$<NOT:$<CONFIG:Debug>>:--release>` generator expression (which
is why the single-config and multi-config generators both do the right thing),
and `-DPIXELSMITH_ENGINE_DIR=` overrides the auto-detected checkout. A missing
`bash` is a `FATAL_ERROR` rather than a silent skip: a bundle with no engine in it
fails at the first engine call, which is a much worse place to find out.

What was verified, and what was not, stated plainly:

- **Verified:** the generator expressions, the `WORKING_DIRECTORY`, the
  `--engine-dir` argument list and the install rule, by exercising them in a
  throwaway CMake project — Debug produced no `--release`, Release produced
  `--release`, `-DPIXELSMITH_ENGINE_DIR=x` produced `--engine-dir x --release`,
  and the working directory was the app root, which is what makes
  `build-engine.sh` find `../core` in the superproject layout. The real script was
  then run from `app/` with no arguments and found the engine, built it and copied
  it to `src/rust/`.
- **Not verified:** `flutter build linux` and `flutter build windows` themselves.
  No CI job runs a desktop build and this runner has no GTK headers or MSVC, so
  the glue has never produced a bundle. It is deliberately thin — it delegates
  every decision to a script that is tested — but "never executed" is the honest
  description and the README now says so.

macOS is still not wired up. The Apple targets link the engine statically
(`DynamicLibrary.process()`), and adding a build phase to `project.pbxproj`
without a Mac to run Xcode on would be a guess dressed as a fix; the README
documents the two commands instead.

## Validation of the patch, re-run from a clean tree

`app/` is a submodule this repository cannot push to, so the patch is applied here,
checked, and reverted:

```console
$ git -C app apply ../workspace/phase-05/shrinkray-phase-05.patch
$ (cd app && flutter analyze --no-pub)      # No issues found!
$ (cd app && dart format --set-exit-if-changed lib test)   # 0 changed
$ (cd app && LD_LIBRARY_PATH=src/rust flutter test)
🎉 34 tests passed.
$ bash scripts/check-dart-bindings.sh
app/lib/rust/bindings.dart: 15 entry points match the engine ABI
```

34 tests, 0 skipped, against the engine built from the current `core/`. The
submodule is back at `3ed1eb4` with a clean working tree.

The drift tripwire was confirmed to fire rather than assumed: with the patch
applied, `cargo test dart_bindings_match_the_engine_abi` fails with `left: []`
against the two documented entries, which is exactly the signal the patch's author
needs to empty the list.

## Left for whoever can push

- Apply the patch, push, then update the submodule pointer here.
- `KNOWN_DART_DRIFT` in `core/src/ffi_abi.rs` must be emptied once the patch
  lands, or the engine test fails on purpose.
- Run `flutter build linux` and `flutter build windows` once, on a machine that can.