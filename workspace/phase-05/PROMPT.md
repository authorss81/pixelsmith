# phase-05 — Dart FFI binding layer and Flutter app skeleton

**Track:** A — Engine & Security
**Depends on:** phase-04
**Timeout:** 90 minutes

## Objective

Create the Dart side of the boundary and a Flutter app that compiles and tests clean, so every later UI phase has something to attach to. No screens beyond a placeholder: this phase is plumbing.

## Read first

- `AGENTS.md` — hard rule 8, the FFI boundary contract.
- `core/src/ffi.rs` — every `px_*` function, the `PxBuffer` layout, and the ownership rules.

## Scope

### Do

- Create the app: `flutter create --platforms=android,ios,windows,macos,linux --org dev.pixelsmith app`. Set the Dart SDK constraint from `flutter --version` rather than guessing.
- Write the native library build glue:
  - desktop: build `core` as a static or dynamic library, invoked from CMake and from the Windows MSBuild project, copying the artefact where the app expects it;
  - Android: `cargo-ndk`; iOS: `cargo-lipo`.
  - Document the exact commands in `app/README.md`. The next agents will follow that file rather than rediscovering the setup.
- Write `app/lib/rust/`:
  - `bindings.dart` — raw `dart:ffi` declarations for every `px_*` function, matching `#[repr(C)]` exactly. Include a test asserting `PxBuffer`'s offsets and sizes against values computed from the Rust side, because a layout mismatch is silent memory corruption.
  - `engine.dart` — the safe wrapper: owns every `PxBuffer`, frees it in a `finally`, never lets a raw pointer escape.
  - `models.dart` — Dart mirrors of the JSON request and response types, with `fromJson`/`toJson` round-trip tests.
  - `errors.dart` — one exception type per `PxStatus`, carrying the engine's message.
- Run every wrapper method on a separate isolate via `Isolate.run`, so a two-second decode cannot drop a frame. Prove it with a test that starts a large decode and asserts the calling isolate kept ticking.
- Add `test/ffi_contract_test.dart` calling every `px_*` entry point at least once, including the deliberately failing `px_selftest_error` and the panic-containing `px_selftest_panic`.

### Do not

- Do not build any UI beyond a placeholder screen. Later phases own the interface.
- Do not hand-write bindings that can drift. Add a Rust test that prints the expected Dart signatures and fails if they are not regenerated.
- Do not let a `PxBuffer` escape `engine.dart`. If a raw pointer appears in a public signature, that is the bug this phase is supposed to prevent.

## Acceptance criteria

### Machine-checkable

- `cd app && flutter analyze` reports zero issues.
- `cd app && flutter test` passes, including the offsets test and the isolate test.
- Every `px_*` function in `core/src/ffi.rs` has a matching declaration and appears in the contract test.
- A large decode does not block the calling isolate; the test asserts this by counting ticks.
- No buffer leak: run the contract loop 1000 iterations and assert the leaked-buffer count is stable.

### Needs a human judgement

- Would a reviewer trust the wrapper's ownership rules from reading it alone, without the tests?
- Is `app/README.md` complete enough to rebuild the native library from a clean checkout on a machine that has never seen this repo?

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha.
- Committed and pushed to `main`.

Do not write `.done`. The workflow does.