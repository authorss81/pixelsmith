# phase-18 — Bound every allocation untrusted bytes can reach

**Track:** A — Engine & Security
**Depends on:** phase-17
**Timeout:** 90 minutes

## Objective

Close the four places where a number derived from bytes the user picked off their
disk reaches an allocation or an arithmetic operation **without a check in front
of it**. Every one of these is a specific finding in `docs/AUDIT.md`, with a
file and a line, and each is a small fix with a regression test that fails on the
code as it stands.

This engine has been exemplary about bounding decodes — eight documented
enforcement points, a `Limits` profile per platform, and a design where the
60000×60000 header is refused on sight. These four are the places where that
property has a hole, and hole 1 is the most serious defect this audit found.

## Read first

- `docs/AUDIT.md` findings **1**, **6**, **7** and **8**. Each has a `file:line`.
- `core/src/colour.rs:680-722` — `png_icc`, and the `MAX_TEXT` / `MAX_TAGS`
  caps the neighbouring parsers already have. Follow their shape.
- `core/src/validate.rs:1-60` — the `Limits` profile.
- `AGENTS.md` hard rules 3 and 4.

## Scope

### 1. The unbounded PNG `iCCP` inflate — `colour.rs:709-713`

```rust
let mut profile = Vec::new();
flate2::read::ZlibDecoder::new(&data[nul + 2..])
    .read_to_end(&mut profile)
    .ok()?;
```

No ceiling on the decompressed size. A PNG of about 4 KiB whose `iCCP` payload
inflates to gigabytes grows one `Vec` until the allocator refuses, and Rust's
allocation-failure handler **aborts the process**.

This is on the main path, not an opt-in: `validate_bytes` → `exif::read` →
`colour_profile` → `ColourProfile::read`. Every `px_inspect`, `px_exif`,
`px_process`, `px_batch` and every folder plan reaches it.

Cap it the way `desc_text` and `mluc_text` are capped — with a named constant and
a reason. Read into a `Take`-wrapped reader (or bound the loop) so the cap is
enforced **during** the inflate, not by checking the length afterwards: a
post-hoc check does not stop the allocation.

Pick the number deliberately and say so in the comment. An ICC profile is
specified in kilobytes; the largest real one in the tree's own fixtures is worth
measuring and quoting. A cap that is too tight rejects a legitimate profile and a
cap that is too loose leaves the hole open — this is the decision, so make it
explicitly.

### 2. No limit on any JSON the FFI parses — `ffi.rs:298`, `:472`, `:536`

`serde_json::from_slice(raw)` on Dart-supplied bytes, with nothing in front of
it. `Limits::max_input_bytes` is only consulted afterwards, inside
`validate_bytes`, after the whole document and every embedded byte array has been
materialised.

`px_zip` is the worst: `BatchFile { name: String, bytes: Vec<u8> }` with no
ceiling on the request, on the file count, or on `bytes` per file — and
**`px_zip` never constructs `Limits` at all**.

Add a bounded check before each parse. A batch or ZIP envelope is inherently
several times the size of the files in it, so the number is not
`max_input_bytes`; derive it, name it, and comment why. Cap the file count and
the summed payload separately from the request length, because the two fail
differently and the sentences a user reads should say which one happened.

Note that `px_batch` currently makes a **second full copy** of every byte array
(`ffi.rs:507-516`) before `process_batch` runs. Say whether you fixed that or
left it, and why.

### 3. Unchecked `u32` addition on `CropSpec` — `pipeline.rs:255`, `:426`

```rust
if crop.x + crop.width > w || crop.y + crop.height > h {
```

`CropSpec` derives `Deserialize` with no range constraint and is reachable from
`px_process` and `px_batch`. `x = 4294967295, width = 1` overflows: a **panic**
in every overflow-checked build — which is every CI run and every `cargo test` —
and a silent wrap in release, where `image` clamps it into a zero-width buffer
that happens to become a clean error downstream.

Use `checked_add` or compare against `w - crop.x` after a `crop.x >= w` guard.
Both `output_dimensions` and `apply` need it; they are the same three lines.

Add a property case. `core/tests/properties.rs:278-280` currently generates
`crop_x in 0u32..100`, so the overflow region is never produced — extend the
range so it is.

### 4. `px_buffer_free` trusts the caller's length — `ffi.rs:54-62`, `:153`

`CAPACITIES` stores the `Vec`'s **capacity** but not its length, and
`Vec::from_raw_parts(buffer.data, buffer.len, cap)` takes the length from the
caller. `PxBuffer` is `#[repr(C)]` and Dart reads and writes it by offset, so a
host bug can corrupt it.

Store `(len, cap)` and ignore the caller's `len` entirely. Keep the free
idempotent — that property has a test and it must survive this change.

### Do not

- Do not add a `Limits` field. `docs/ARCHITECTURE.md` records that phase-11 grew
  two *methods* rather than fields precisely so the Dart-mirrored JSON contract
  would not change; keep that property.
- Do not weaken a hard rule to make a bound work. If a bound is not expressible
  without one, write it up and leave it.
- Do not add `unwrap()` or `expect()` to the code you touch. Every fix here is in
  a function that runs on hostile bytes.
- Do not "fix" the overflow by clamping the crop to the source. Refuse it with a
  sentence that names what was asked for and what the picture is.
- Do not reformat or refactor neighbouring code. Four small fixes, four tests.

## Acceptance criteria

### Machine-checkable

- A test constructs a PNG whose `iCCP` payload is a zlib stream that expands to
  more than the cap, and asserts the profile read returns `None` rather than
  allocating. It must fail against the code as it stands.
- A test asserts `px_process` / `px_batch` / `px_zip` refuse an envelope past
  their limit with a message that names the limit, and that the refusal happens
  **before** any file bytes are decoded.
- A test drives `Pipeline::output_dimensions` and `Pipeline::apply` with
  `x = u32::MAX`, `width = 1` and asserts a named `Err`, in a test profile where
  `overflow-checks` is on. Plus a proptest case covering the region.
- A test asserts `px_buffer_free` is still idempotent, and a second one that a
  `PxBuffer` with a deliberately wrong `len` still frees exactly once without
  undefined behaviour.
- `cargo test --all-features` is green and the test count has risen by at least
  five.
- `cargo clippy --all-targets --all-features -- -D warnings` is clean.
- `bash scripts/verify.sh` exits 0.
- **`cargo build --release --all-features` succeeds.** This phase does not change
  any `px_*` signature, any JSON field, or any public Rust type, so
  `flutter build apk --release` and `flutter build windows --release` cannot be
  affected — but say that explicitly in the commit message rather than leaving
  it unstated, because those jobs are the only thing standing between a refactor
  and a binary that no longer starts.

### Needs a human judgement

- Is the ICC cap the right number? Say what you measured and what you would
  raise it to.
- Would a user understand the new envelope refusals? Read the sentences aloud.
- Did you find a fifth hole while reading these four? Add it to `docs/AUDIT.md`
  rather than fixing it here.

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha.
- Committed and pushed to `main`. Never force-push.
- Do not write `.done`. The workflow does.
