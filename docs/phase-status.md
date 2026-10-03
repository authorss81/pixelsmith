# Phase status

The truth table. `DONE` is written by the workflow only, and only after
`scripts/verify.sh` exits 0 — so a row here saying `DONE` is a claim that has
been mechanically checked, not asserted by an agent.

| Phase | Title | Status | Commit | Notes |
| --- | --- | --- | --- | --- |
| phase-01 | Verification baseline and project scaffolding | PENDING | | |
| phase-02 | Fuzz harness for every decode path | PENDING | | |
| phase-03 | Hostile-input corpus and property tests | PENDING | | |
| phase-04 | Sandboxed decode worker with a hard memory cap | PENDING | | |
| phase-05 | Dart FFI binding layer and Flutter app skeleton | PENDING | | |
| phase-06 | HEIC/HEIF decode | PENDING | | |
| phase-07 | AVIF encode, progressive JPEG, chroma subsampling | PENDING | | |
| phase-08 | Lossy WebP via libwebp, verified on every target | PENDING | | |
| phase-09 | SIMD resize path behind a feature flag | PENDING | | |
| phase-10 | Benchmarks and a performance regression gate | PENDING | | |
| phase-11 | Low-peak-memory decode for very large images | PENDING | | |
| phase-12 | Colour management: sRGB, Display-P3 and ICC | PENDING | | |
| phase-13 | Animated GIF: honest handling | PENDING | | |
| phase-14 | Content-hash deduplication and a folder pipeline | PENDING | | |
| phase-15 | Supply-chain policy and reproducible builds | PENDING | | |
| phase-16 | Self-audit and next-phase generation | PENDING | | |

## Status values

| Value | Meaning |
| --- | --- |
| `PENDING` | not started |
| `IN PROGRESS` | a run is active; look for a `px-wip/<phase>` branch |
| `ATTEMPTED` | work committed, verification failed; check `logs/<phase>.verify.log` |
| `DEFERRED` | rate-limited; retried on the next tick, no action needed |
| `BLOCKED` | needs a human. Delete `.blocked` and `.attempts` after fixing |
| `DONE` | verified. The only value written by the workflow |

## Unblocking a phase

```bash
rm workspace/phase-NN/.blocked workspace/phase-NN/.attempts
git commit -am "unblock phase-NN" && git push
```

The next tick picks it up within about a minute.

## Halting the pipeline

```bash
touch workspace/.stop && git add -A && git commit -m "halt pipeline" && git push
```

Delete `workspace/.stop` to resume. To stop permanently without leaving a
marker in the tree, disable the workflow in the Actions tab instead.