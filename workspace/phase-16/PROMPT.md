# phase-16 — Self-audit and next-phase generation

**Track:** A — Engine & Security
**Depends on:** phase-15
**Timeout:** 90 minutes

## Objective

You are the last planned phase. Your job is to look at what was actually built, work out what is genuinely missing or wrong, and then **write the phases that come next**. You do not implement anything yourself.

Read `.opencode/agent/auditor.md` in full before starting. It is your instruction sheet, and it is more specific than this file. Where the two differ, the auditor sheet wins on method and this file wins on intent.

## What makes this phase work

You create new `workspace/phase-NN/PROMPT.md` directories and push them to `main`. The `select-phase` job picks the lowest-numbered phase without a `.done` marker, so the phase you write is picked up on the next tick with **no change to the workflow**. That is the loop closing. It is the whole design.

Numbering continues from the highest existing phase. Find it with `ls workspace | grep '^phase-' | sort -V | tail -1`.

## Order of work

1. **Establish the truth.** Read `ROADMAP.md`, `docs/phase-status.md`, `docs/ARCHITECTURE.md`, `AGENTS.md`, `git log --oneline`, and the source tree. Run `cargo test` and `cargo tree`. For every roadmap item, classify it `DONE`, `PARTIAL`, `ABSENT` or `STALE`.

   Be sceptical of `DONE`. A `DONE` in the status file that you cannot find in the tree is not `DONE` — report it as `ABSENT` and say the status file overclaimed.

2. **Find what nobody wrote down.** This is the highest-value part. The security claim, the `unsafe` blocks at the FFI boundary, decode paths without a pixel cap, tests that assert nothing, `cfg(target_os)` paths no CI job builds, error messages that blame the user for a decoder failure. Verify the no-network claim yourself with `cargo tree` rather than trusting `AGENTS.md`.

3. **Write `docs/AUDIT.md`.** Summary table per roadmap category, then every finding numbered with location, impact and confidence, then an explicit "could not verify" section. Guessing silently is worse than admitting a limit.

4. **Write the next phases.** Between 6 and 20 of them, ordered by leverage not by ease, each independently verifiable so a reviewer can pass or fail it from the diff alone. A blocker finding becomes an early phase even if it is awkward. If you can only justify four, write four — a short list of real phases beats a long list of padding.

   The Flutter UI track is the obvious next thing: the app skeleton exists after phase-05, so the design system, shell, preview canvas, before/after view, pipeline editor, live progress, batch list and export flows are all writable now. Include them. Also include whatever *you* found that the roadmap never mentioned.

5. **Never generate a phase that weakens a hard rule.** If you think a hard rule in `AGENTS.md` is wrong, write it up as a finding and let a human decide. Do not route around it.

6. **Update state.** Append the new phases to `workspace/PHASES.md`, add a `PENDING` row per phase to `docs/phase-status.md`, and tick or cross out the corresponding `ROADMAP.md` items, marking newly discovered work `[added by audit]`.

7. **Stop.** Print `AUDIT: complete`, the number of phases written, and one paragraph on the most important thing you found.

## Acceptance criteria

### Machine-checkable

- `docs/AUDIT.md` exists, is newest-first, and contains the classification table, the numbered findings, and the "could not verify" section.
- At least six new `workspace/phase-NN/PROMPT.md` files exist with numbers continuing from phase-16, unless you argue in writing that fewer are warranted.
- `workspace/PHASES.md` lists every phase directory present in the tree.
- `docs/phase-status.md` has exactly one row per phase directory.
- `ROADMAP.md` reflects the real state: items are ticked or crossed, not aspirationally ticked.
- `bash scripts/verify.sh` exits 0.

### Needs a human judgement

- Are the generated phases specific enough that a `build` agent can execute one without guessing? Read one at random and check.
- Is the audit honest, or does it flatter the work that has already been done?
- Did you find at least one thing that was wrong and say so plainly?

## When there is nothing left

If the honest answer is that there is no worthwhile work remaining, write `docs/AUDIT.md` saying so, write no phase files, and print `AUDIT: complete — no further phases warranted`. That is a legitimate outcome and better than inventing work. The pipeline will idle, and the run budget in the workflow stops it cleanly.

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Committed and pushed to `main`. Never force-push.
- Do not write `.done`. The workflow does.