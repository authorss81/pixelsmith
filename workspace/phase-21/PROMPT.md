# phase-21 — The tests that assert nothing, and the module with none

**Track:** A — Engine & Security
**Depends on:** phase-20
**Timeout:** 75 minutes

## Objective

Four tests in this repository assert nothing measurable, and **one of them is
the only sentinel hard rule 4 has.** A module with nineteen user-facing error
messages has no tests at all. Fix both, then add the check that stops the pattern
coming back.

`AGENTS.md` says: *"A test asserts a **specific** value, not just 'it didn't
panic'."* Four tests do not, and one of them exists for exactly the reason that
rule exists.

## Read first

- `docs/AUDIT.md` finding **19** (`error.rs` has zero tests, and has a live
  defect that a test would have caught on sight) and finding **20** (the four
  tests).
- `core/tests/hostile.rs:100-135` — `assert_rejected` and
  `assert_bounded_success`. This file has the right shape; copy it.
- `AGENTS.md`, Conventions: *"Every module has them."*

## Scope

### 1. `hostile.rs:660` — the hard-rule-4 sentinel that cannot fail

```rust
#[test]
fn a_raw_reader_respects_decoder_limits_on_hostile_input() {
    let limits = Limits::mobile();
    let full = photo(64, 64);
    for cut in [0usize, 4, 12, 33, 64, full.len() - 1] {
        let mut reader = ImageReader::new(Cursor::new(&full[..cut]));
        reader.no_limits(); // prove `apply_to_decoder` is what does the limiting
        limits.apply_to_decoder(&mut reader);
        let _ = reader.decode();
    }
}
```

Its own comment says it exists "so a decoder limit that `validate` forgets to
apply is still visible here", and then it asserts nothing. **Deleting the body
of `Limits::apply_to_decoder` makes this test pass.** The one test standing
directly between `validate` and a silently unlimited decoder is measuring
nothing.

Rewrite it to assert the actual property: build inputs **above the mobile
pixel ceiling**, decode with the limits applied, and assert every outcome is
`Err` with a named variant. Then prove it can fail — comment out the body of
`apply_to_decoder`, run it, paste the failure, put it back.

`core/tests/properties.rs` already tests dimension arithmetic through
`Limits::check_header` rather than through a file, for exactly this reason.
Read that and follow it.

### 2. Three more that assert less than their name claims

| Location | Claim | Reality |
| --- | --- | --- |
| `core/src/worker.rs:2185` | `rayon_has_more_than_one_thread` | `assert!(current_num_threads() >= 1)` — a tautology. A single-threaded pool passes. |
| `core/tests/sandbox.rs:908` | `the_worker_entry_point_is_reachable_and_exits_cleanly` | `let _entry: fn() -> ! = run_worker;` — a compile-time existence check. Nothing tests "exits cleanly". |
| `core/src/stream.rs:859`, `:875` | `a_jpeg_goes_through_the_whole_image_arm`; `a_png_this_arm_cannot_decode_falls_back_rather_than_scrambling` | Both assert only `dimensions()`. The second's name promises pixels and never looks at one. |

`stream.rs` is the important one: `docs/AUDIT.md` finding 10 is a **stride bug in
`stream.rs:592-625`** that these two tests cannot see because they never compare
a pixel. Assert pixel equality against the in-memory kernel's output, on a
fixture where a wrong stride is visibly wrong (a row ramp, or the checkerboard
`resize.rs` already uses). **That test should fail today.** If it does not, your
assertion is still too weak.

`worker.rs:2185`: either raise the assertion to what the name says and skip
honestly when rayon gives one thread, or rename it to what it checks. The first
is better — the batch path's parallelism is a claim this project makes.

### 3. `core/src/error.rs` has no test module

`grep -c 'cfg(test)' core/src/error.rs` → **0**. It is the only module in the
crate with none, and hard rule 9 makes it the product: *"Never surface 'Error:
decode failed'. Say what actually happened and what to do about it."*

Write tests for the nineteen variants. Not "it formats" — **for each variant,
assert the message names the thing a user needs.** For `SuspiciousDimensions`,
assert it carries the numbers. For `NoQualitySetting`, assert it names a format
that can honour the request. `format.rs` already has
`every_refusal_says_what_to_choose_instead`; this is the same idea one level up
and it will find things.

**It will find this immediately.** `error.rs:128`:

```rust
#[error(
    "this resize would need {needed} bytes of working memory and this device          allows {budget}; choose a smaller output size"
)]
StreamingBudgetExceeded { budget: u64, needed: u64 },
```

**Ten literal spaces** mid-sentence, in a string shown verbatim to a user. Fix
it, and let the test prove it.

Also worth adding: no message may be empty, and no message may contain a run of
three or more consecutive spaces. Both are cheap properties and both would have
caught the above.

### 4. The weak `is_ok()` / `is_err()` cluster

`lib.rs:281` asserts only `!err.to_string().is_empty()`. `format.rs:2090`,
`validate.rs:535` and `validate.rs:540` assert only `.is_err()`, though
`validate.rs` has eight named variants — and `validate.rs:540`'s name claims
"rather than half decoded" while nothing checks for partial output.

Tighten the ones where a specific variant was available. Use
`hostile.rs`'s `assert_rejected` shape: a strict allow-list that panics on
anything new, so a new error variant is a deliberate decision rather than a
silent pass.

### 5. A check, so the pattern does not come back

Add to `scripts/verify.sh` a scan for test functions whose body contains no
assertion macro. It is a crude check and it will have false positives (tests that
delegate to an asserting helper) — so make the list explicit, the way
`UNFIXABLE_UPSTREAM_APP` is, and require every entry to carry a one-line reason.

Follow the rule the gate already states: run the new check against a passing
tree and a deliberately broken one, in both directions.

### Do not

- Do not delete or `#[ignore]` a test. Hard rule 2. If a test is wrong, fix it
  and say why in the commit message.
- Do not weaken an assertion to make it pass. The `stream.rs` pixel test is
  *supposed* to fail against the current stride bug — if it does, that is a real
  finding: record it in `docs/AUDIT.md`, and fix the stride in
  `core/src/stream.rs` if the fix is small enough. It is a four-line change
  (`CHANNELS` → the source's real byte-per-pixel). If it is larger than that,
  stop and record the finding rather than leaving the test red.
- Do not chase a coverage number. This phase is about tests that assert nothing,
  not about how many lines are executed.

## Acceptance criteria

### Machine-checkable

- All four tests in finding 20 assert a **specific value**, and each is
  demonstrated to fail against a deliberately broken implementation. Paste the
  failing runs.
- `core/src/error.rs` has a `#[cfg(test)] mod tests` covering all nineteen
  variants, including the two properties in scope item 3.
- `grep -c 'cfg(test)' core/src/error.rs` is non-zero, and no other module in
  `core/src` is zero.
- The new assertion-scan check in `verify.sh` reports clean on this tree, and
  reports a finding when a deliberately emptied test body is committed.
- `cargo test --all-features` is green and the test count has risen.
- `bash scripts/verify.sh` exits 0.
- **No public Rust type, no `px_*` signature and no JSON field changes.** Say so
  in the commit message; the APK and EXE jobs are unaffected.

### Needs a human judgement

- Do the nineteen error messages actually read like sentences a person wrote?
  Read them out loud, in order. `format.rs`'s
  `every_refusal_says_what_to_choose_instead` is the standard to hold them to.
- Did you find any test that is green for the wrong reason beyond the four
  named? Add it to `docs/AUDIT.md`.

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha.
- Committed and pushed to `main`. Never force-push.
- Do not write `.done`. The workflow does.
