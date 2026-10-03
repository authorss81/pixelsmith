# Troubleshooting and recovery

Every state the pipeline can be in, what it means, and what to do about it. The
design intent is that **the pipeline recovers itself from transient failure and
asks a human only when repeating itself would be pointless.**

## Does the next run pick it up?

Short answer: yes for transient failures, no for a phase that needs a decision.

| What failed | Recovered automatically? | How |
| --- | --- | --- |
| Rate limit on the model | Yes | `.deferred`, retried on the next tick, up to 5 times |
| Runner killed mid-phase (timeout, cancel) | Yes | Partial work checkpointed to `px-wip/<phase>` every 5 min; the next run merges it and the agent continues rather than restarting |
| Push to `main` rejected 5 times | Yes | Work force-pushed to `px-recovery/<phase>`; the next run of that phase merges it |
| Artifact upload failed | Yes | The log tail is written to the job's step summary, which the job writes itself and cannot fail with the upload |
| Verification failed | Partly | `.verify_failures` increments and the phase is retried, up to 3 times |
| Model ran but changed nothing | No | Blocked after 2 attempts. Repeating a 90-minute run that produced nothing teaches nothing |
| A phase is genuinely impossible right now | No | Blocked. Needs a human to change the prompt, the toolchain, or the decision |
| A run hangs and never finalises | No | Needs `gh run cancel`. Every job now has a `timeout-minutes`, so this should not recur |

## Markers

All in `workspace/<phase>/`.

| Marker | Meaning | Written by | Remove to |
| --- | --- | --- | --- |
| `.attempted` | The agent produced real work; awaiting verification | runner | — |
| `.done` | Verified. **Only the workflow writes this** | verify job | re-run the phase |
| `.deferred` | Rate-limited | runner | — (auto-retries) |
| `.blocked` | Needs a human | runner or verify job | retry |
| `.attempts` | Real-failure count, cap 3 | runner | retry |
| `.no_work` | The model ran and changed nothing | runner | — |
| `.no_work.n` | Consecutive no-work count, cap 2 | runner | retry |
| `.verify_failures` | Verification-failure count, cap 3 | verify job | retry |
| `.deferred_attempts` | Rate-limit count, cap 5 | runner | — (auto-retries) |
| `.checkpoint` | Partial work was merged from a WIP branch | runner | — |
| `.session` | Session id to resume | runner | — (auto-cleared when stale) |
| `.timeout` | Job budget in minutes, default 60 | committed by hand | — |

Global:

| Path | Effect |
| --- | --- |
| `workspace/.stop` | Halts the pipeline entirely |
| `workspace/.runs` | Run counter. The pipeline stops itself at 250 |

## Recipes

**Unblock a phase**

```bash
rm workspace/phase-NN/.blocked \
   workspace/phase-NN/.attempts \
   workspace/phase-NN/.no_work \
   workspace/phase-NN/.no_work.n \
   workspace/phase-NN/.verify_failures
git commit -am "unblock phase-NN" && git push
```

**Halt everything now**

```bash
touch workspace/.stop && git commit -am "halt" && git push
```

Delete `workspace/.stop` to resume. To stop permanently without a marker in the
tree, disable the workflow in the Actions tab instead.

**Force a specific phase**

Actions → automation → Run workflow → set `phase` to e.g. `phase-07`. It runs
that phase regardless of selection order.

**Recover by hand from a WIP or recovery branch**

```bash
git fetch origin px-recovery/phase-07
git merge --no-edit FETCH_HEAD
```

Inspect first with `git log --oneline FETCH_HEAD` and
`git diff main...FETCH_HEAD`. These branches are force-pushed, so do not treat
them as durable.

**Cancel a stuck run**

```bash
gh run list --workflow automation --limit 5
gh run cancel <run-id>
```

The concurrency group is `px-pipeline` with `cancel-in-progress: false`, so one
stuck run blocks every other run until it finishes or is cancelled. That setting
is deliberate — `true` would destroy work done between two checkpoints — which
is why every job now carries an explicit `timeout-minutes`.

**Reset the run budget**

```bash
echo 0 > workspace/.runs && git commit -am "reset budget" && git push
```

**See why a phase is blocked**

The `Report status` step opens an issue with the log tail and a link to the run.
If the issue does not exist, the artifacts are still there:

```bash
gh run download <run-id> --name px-logs-phase-NN-<run-id> --dir ./logs
```

The same tail is in the run's **step summary**, which survives an artifact upload
failure.

## Why the verification gate is separate

The phase agent never writes `.done`. `scripts/verify.sh` decides, and the
workflow commits the marker. That separation exists because of a real incident:
`verify.sh` checks whether the *repository* is green, and the tree is always
green at the start of a phase, so verify used to pass while the agent had done
nothing at all. Two phases were certified complete that way.

So a phase is only eligible for `.done` when `.attempted` exists, and
`.attempted` is only written when the run left real file changes or real
commits. If you ever see a phase marked `.done` with an empty diff, that gate has
been broken again.

## Limits, and what happens at each

| Limit | Value | On reaching it |
| --- | --- | --- |
| Runs | 250 | Pipeline exits with `budget_exhausted` |
| Phases | 80 | Pipeline exits with `phase_ceiling` |
| Real failures per phase | 3 | `.blocked` |
| Rate-limit deferrals per phase | 5 | `.blocked` |
| Verification failures per phase | 3 | `.blocked` |
| No-work failures per phase | 2 | `.blocked` |

Every one of these stops the pipeline rather than continuing. An autonomous loop
that can run forever is a liability, not a feature.
