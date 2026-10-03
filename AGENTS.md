# AGENTS.md

Instructions for every agent working in this repository. Read this first, then
`docs/ARCHITECTURE.md`, then your phase prompt.

## What this project is

A local-first image resizer: a Rust engine (`core/`) with a Flutter front end
(`app/`). It resizes, converts, compresses and strips metadata. It never uploads
anything, because it has no network capability to upload with.

## Hard rules

These are not preferences. A phase that breaks one of these is not done, and the
reviewer will block it.

1. **The engine must have no network capability.** No HTTP, TLS, socket, DNS,
   or URL-fetching dependency in `core/Cargo.toml`, ever. No `reqwest`,
   `hyper`, `ureq`, `curl`, `tokio`, `std::net`, `std::process::Command` shelling
   out to a downloader. `scripts/verify.sh` enforces this and the audit checks
   it. If a task seems to need the network, it needs to be restructured.
2. **Never delete or weaken a test to make a build pass.** No
   `#[ignore]`, no `#[allow(clippy::::)]` on a real finding, no `--no-verify`, no
   deleting an assertion. If a test is wrong, fix the test and say why in the
   commit message. If the code is wrong, fix the code.
3. **No `unwrap()` or `expect()` on untrusted input.** Any decode, parse, or
   dimension computation derives from bytes the user picked off their disk. A
   panic there is a crash, and a crash on a crafted file is a vulnerability.
4. **Every decode path is bounded.** Check pixel count and dimensions against
   `validate::Limits` *before* allocating. A 60000×60000 header costs 14 GB once
   decoded; the header must be rejected on sight.
5. **Exactly one resampling pass per image.** Order is fixed:
   crop → orient → resize. Two resampling passes is the single most common cause
   of "why is my export blurry", and it is banned.
6. **Metadata is stripped by re-encoding**, never by clearing tags. Maker notes
   and embedded thumbnails survive a tag-clear. GPS and identifying tags are
   never written back, even when metadata preservation is explicitly requested.
7. **Never trust a filename.** Format comes from magic bytes
   (`format::detect_format`). A file called `photo.png` containing JPEG is a
   JPEG. Output names are sanitised against traversal (`worker::sanitise_stem`).
8. **The FFI boundary is hostile by default.** Every Dart-supplied pointer and
   length is checked. Results are a tagged struct; there is no path where Dart
   receives a null it might dereference. Owned buffers are freed exactly once,
   and freeing twice is a no-op rather than undefined behaviour.
9. **Failures are explained to the user, not to the engineer.** Never surface
   "Error: decode failed". Say what actually happened and what to do about it.
10. **Ship the honest capability list.** If the build cannot write a format, the
    UI greys it out (`capabilities()`). Never promise a format the build cannot
    produce.

## Conventions

- Rust edition 2024. `cargo fmt` clean, `cargo clippy --all-targets` warning-free.
- Tests are unit tests next to the code in `#[cfg(test)] mod tests`. Every module
  has them. A test asserts a *specific* value, not just "it didn't panic".
- Fixtures must match the claim. Use `worker::tests::photo` (compressible) when
  testing a size ceiling; use noise only when testing that a file decodes.
  Compressed and uncompressible data are not interchangeable.
- Public API has doc comments explaining *why*, not restating the signature.
- Comments earn their place by explaining a decision a reader would otherwise
  have to reverse-engineer. Delete commented-out code.
- Dart: no `print`. Use the logger. No magic numbers in the UI layer.

## The Flutter app lives in another repository

`app/` is a git submodule pointing at
[authorss81/shrinkray](https://github.com/authorss81/shrinkray), where the
Flutter UI, the Dart FFI layer and the APK/EXE builds live. The engine in
`core/` is the source of truth here; the app consumes it.

Rules for working inside `app/`:

1. The submodule is checked out at a pinned commit. `git submodule update
   --init --recursive` first, or you are editing a stale or empty directory.
2. Commit inside the submodule first, push it, then commit the pointer update
   here. A superproject commit pointing at an unpushed submodule SHA is broken
   for everyone else.
3. Never commit build artefacts from `app/` into this repository. The native
   libraries (`src/rust/*.so`, `*.dll`) and `build/` are gitignored in both
   repos; CI rebuilds them every run.
4. The Dart code mirrors the engine's JSON contract exactly. If you change a
   request, response or enum shape in `core/src/ffi.rs`, update
   `app/lib/rust/models.dart` in the same phase and extend the contract tests on
   both sides. A drifting contract is silent corruption.
5. Slow builds (APK, EXE, release bundles) run in the shrinkray repository's own
   `build.yml`, not here and not on a developer machine.

1. Read `workspace/<phase>/PROMPT.md` completely.
2. Read the files you are about to change.
3. Run the existing verification first, so you know the tree was green before
   you touched it: `bash scripts/verify.sh`.
4. Implement the phase. Nothing more. Do not opportunistically refactor
   neighbouring code.
5. Run `bash scripts/verify.sh` again. It must pass.
6. Update your row in `docs/phase-status.md`.
7. Commit with a message describing what changed and why:

```
git add -A && git commit -m "phase-NN: <what you did>" && git push
```

If the push is rejected, `git pull --rebase origin main` and push again. Never
force-push to `main`.

## When you are stuck

A phase that is genuinely blocked should say so, plainly, in the commit message
and in `docs/phase-status.md`, and leave the tree green. A blocked phase that
admits it costs one human minute. A phase that quietly does half the work and
marks itself done costs far more.

Do not create `.done` yourself. The workflow writes it, and only after
`scripts/verify.sh` passes.