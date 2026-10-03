# phase-13 — Animated GIF: honest handling

**Track:** A — Engine & Security
**Depends on:** phase-12
**Timeout:** 75 minutes

## Objective

Stop the app from silently destroying animation. Today an animated GIF is decoded to its first frame and re-encoded as a still, and the user finds out later. Decide what the right behaviour is, implement it, and make it visible in the UI contract.

## Read first

- `AGENTS.md` — hard rule 9. Tell the user what happened.
- `core/src/validate.rs` — `has_animated` is detected but never acted on.
- `core/src/worker.rs` — `process_one`, where the frames are collapsed.

## Scope

### Do

- Decide and document the policy in `docs/GIF.md`. There are three defensible options and the project must pick one deliberately:
  1. **Flatten, loudly.** Output the first frame, and surface a warning naming the dropped frame count.
  2. **Preserve.** Carry every frame through resize and re-encode with per-frame delays.
  3. **Refuse.** Refuse to process an animation and say why.
  Recommend one, implement it, and record why the other two were rejected. A defensible refusal beats a beautiful implementation of the wrong thing.
- Whichever policy you choose, surface it through the FFI: `Outcome` gains an explicit field describing what happened to animation. A caller must be able to find out without inspecting bytes.
- If preserving frames: resize each frame with exactly one resampling pass per frame, keep per-frame delays, and handle frame disposal and transparency correctly. Disposal is where every naive GIF implementation breaks.
- Add tests: an animated input produces the documented outcome; a single-frame GIF is not flagged as animated; frame count is preserved or explicitly dropped according to policy; a GIF with a corrupt frame index does not panic.

### Do not

- Do not silently drop frames. Hard rule 9.
- Do not decode an animation and re-encode it as a still without a warning in the outcome.
- Do not spend this phase building a full GIF composition engine if the documented policy is flatten-or-refuse.

## Acceptance criteria

### Machine-checkable

- `docs/GIF.md` states the policy, the reasoning, and why the alternatives were rejected.
- `Outcome` carries an explicit animation field, and a test asserts it is set correctly for animated, single-frame and corrupt inputs.
- A test asserts a corrupt frame index returns `Err` rather than panicking.

### Needs a human judgement

- Is the chosen policy the one a user would thank us for, or the one that is easiest to implement? Be honest in `docs/GIF.md` if the two differ.
- Would someone who lost an animation to this app understand from the warning message what happened?

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha.
- Committed and pushed to `main`.

Do not write `.done`. The workflow does.