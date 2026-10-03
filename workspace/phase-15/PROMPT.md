# phase-15 — Supply-chain policy and reproducible builds

**Track:** A — Engine & Security
**Depends on:** phase-14
**Timeout:** 90 minutes

## Objective

Make the supply chain auditable. This app's whole value proposition is that it holds your photos, so "what is actually in this binary" must be answerable by a stranger in an afternoon, not by us in a week.

## Read first

- `AGENTS.md` — hard rule 1 is the promise this phase makes checkable.
- `deny.toml` from phase-01 — the licence policy already exists; make it enforced.
- `core/Cargo.toml` — the dependency set.

## Scope

### Do

- Enforce the licence policy in CI with `cargo-deny`: run `cargo deny check` and fail the build on any advisory or licence outside the allow-list. Add it to `scripts/verify.sh` behind `PX_DENY=1` so the default gate stays fast, and run it unconditionally in a dedicated CI job.
- Add a reproducible-build check: build the release binary twice from a clean checkout on two different runners, and compare hashes. Record the result in `docs/SUPPLY-CHAIN.md`. If the hashes do not match, identify the source of non-determinism — build paths, timestamps, or parallel codegen — and fix what can be fixed. Report honestly if it cannot be made reproducible on every target.
- Emit an SBOM in CycloneDX format on every release build. Small tooling change, large audit value.
- Write `docs/SECURITY.md` covering: the threat model, what "secure" means for this project specifically, the no-network claim and how to verify it independently, the fuzzing setup, the vulnerability disclosure process, and a supported-versions table.
- Publish `cargo-vet` audit files for any dependency that cannot use a stock advisory database. Start with the unusual ones and record why.
- Add a `SECURITY.md` at the repository root that points at `docs/SECURITY.md`, so the disclosure route is findable from the front page.

### Do not

- Do not suppress an advisory to make the build green. Either fix the dependency or document the accepted risk with a reason and a review date.
- Do not claim reproducible builds where only one target is reproducible. Say which target and which is not.

## Acceptance criteria

### Machine-checkable

- `cargo deny check` runs in CI and fails on an injected violation. Prove it in the log or a test, do not just enable it.
- `scripts/BUILD-SHA256.txt` exists with the release binary hashes per target.
- An SBOM artefact is attached to every release build.
- `docs/SUPPLY-CHAIN.md` states, per target, whether the build is reproducible. "No" is an acceptable answer; silence is not.
- `docs/SECURITY.md` and root `SECURITY.md` both exist.

### Needs a human judgement

- Could a sceptical stranger verify the no-network claim from the published artefacts alone, without trusting this repository?
- Is the disclosure process realistic for a one-person project, or aspirational?

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha.
- Committed and pushed to `main`.

Do not write `.done`. The workflow does.