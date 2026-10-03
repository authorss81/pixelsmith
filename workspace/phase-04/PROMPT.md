# phase-04 — Sandboxed decode worker with a hard memory cap

**Track:** A — Engine & Security
**Depends on:** phase-03
**Timeout:** 90 minutes

## Objective

Move untrusted decoding into a separate process with a hard memory ceiling, so a hostile file can be expensive but can never take down the UI. This is the difference between "we validate the header" and "we contain the failure".

## Read first

- `AGENTS.md` — hard rule 4. The sandbox is a second line of defence, never a replacement for the header checks.
- `core/src/validate.rs` — `Limits`, and where it is enforced.

## Scope

### Do

- Add `core/src/sandbox.rs`: a decode worker running as a **separate process**, not a thread. A thread shares the address space, so an OOM in the worker takes the UI with it.
- Re-exec the current binary with a hidden first argument selecting worker mode. Use `std::process::Command`. Do not add a dependency for this.
- The worker reads a job from stdin, writes the encoded result to stdout, and exits with a status the parent can classify.
- Enforce the memory ceiling **from outside the process**, not from inside it: `RLIMIT_AS` on Linux via `std::os::unix::process::CommandExt::pre_exec`, and a Windows job object behind `cfg(windows)`. A limit the worker sets for itself is a limit a compromised worker can ignore.
- On memory-limit death the parent must distinguish "this file was too big" from "the engine crashed", because those are different sentences for a user. Carry that distinction in the exit code.
- Add a wall-clock timeout as well as a memory cap. A pathological file that never finishes is as bad as one that never allocates.
- Never let the worker's stderr reach the parent's stderr. Capture it, classify it, and surface a clean message. A panic backtrace is not a user-facing error.
- Add `core/tests/sandbox.rs` covering:
  - a file within limits succeeds and returns bytes identical to the in-process path;
  - a file over the memory cap fails with the memory error, not a generic one;
  - a timeout is reported as a timeout;
  - a worker that panics is reported as an engine failure without killing the test process.
- Make the sandbox a feature flag if it complicates the build on a target we ship. If it is optional, the default path must still be bounded by `validate::Limits`.

### Do not

- Do not add a sandboxing crate. `std::process` plus `libc` for the rlimit is enough.
- Do not remove or weaken any check in `validate.rs`.
- Do not treat the sandbox as licence to skip `Limits` on the in-process path.

## Acceptance criteria

### Machine-checkable

- `core/tests/sandbox.rs` passes and covers all four cases.
- The memory limit is set by the parent. A comment states why, and a test asserts the worker cannot raise its own limit.
- A worker panic surfaces as a clean error and does not kill the parent.
- The in-process path still enforces `Limits` when the sandbox is compiled out.

### Needs a human judgement

- Is the exit-code contract documented well enough that a caller cannot confuse the two failure modes?
- Does the sandbox leak processes or file descriptors across many invocations? Check with a loop test.

## Definition of done

- `bash scripts/verify.sh` exits 0.
- Your row in `docs/phase-status.md` says `DONE` with the commit sha.
- Committed and pushed to `main`.

Do not write `.done`. The workflow does.