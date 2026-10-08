# Supply chain

What is actually in the shipped binary, where each claim comes from, and which of
them you have to take on trust.

[`docs/SECURITY.md`](SECURITY.md) is the threat model and the disclosure route.
This document is the evidence underneath §6 of it. The one-paragraph version:
the engine is a Rust crate with a committed lockfile, an enforced licence and
advisory policy, a CycloneDX SBOM on every build, recorded hashes for the release
artefacts, and — for the crates no advisory database can answer for — written
down audits.

Everything below was **measured on the runner named in §7**, not assumed. Where
something was not measured, the table says `not measured` and a paragraph says
why. Silence is the only answer this document does not give.

---

## 1. The per-target reproducibility table

This is the table the phase prompt asked for, and the answer is one measured
target and seven unmeasured ones.

| Target | Reproducible? | Measured how | Verdict |
| --- | --- | --- | --- |
| `x86_64-unknown-linux-gnu` | **yes, for both artefacts** | 4 builds, 3 pair comparisons | Reproducible. §2 |
| `aarch64-unknown-linux-gnu` | **not measured** | — | No aarch64 Linux host and no cross toolchain on the measuring machine |
| `x86_64-pc-windows-msvc` | **not measured** | — | Needs the MSVC toolchain, which exists only on a Windows runner |
| `aarch64-pc-windows-msvc` | **not measured** | — | Needs the MSVC ARM64 libraries, which exist only on a Windows ARM64 runner |
| `x86_64-apple-darwin` | **not measured** | — | Needs the Apple SDK, which exists only on macOS |
| `aarch64-apple-darwin` | **not measured** | — | Needs the Apple SDK, which exists only on macOS |
| `aarch64-linux-android` | **not measured** | — | Needs the Android NDK for the linker |
| `aarch64-apple-ios` | **not measured** | — | Needs Xcode for the SDK and the signing toolchain |

These eight are the `[graph] targets` in `deny.toml`, which is where the set of
targets this project claims to ship is written down. It is deliberately the same
list the licence check uses: a target that is not in the policy is not a target
this project has made a claim about.

**Read the "not measured" rows as a gap, not as a failure.** Nothing is known to
be non-reproducible on any of them. Nothing is known to be reproducible on any of
them either, and a table claiming otherwise for a target nobody built would be the
kind of sentence this project's docs exist to avoid. What *is* known is recorded
in §3: the two things that make a Rust build reproducible were made
reproducible, and one of them was **measured to be load-bearing**.

---

## 2. The one measurement, and what it found

`scripts/repro-check.sh` does four builds and compares three pairs, because the
obvious form of this test cannot fail:

| | sources at | target dir | `--remap-path-prefix` |
| --- | --- | --- | --- |
| **A1** | this checkout, this path | fresh | on |
| **A2** | this checkout, this path | fresh | on |
| **B1** | a copy of the same sources at a different absolute path | fresh | **off** |
| **B2** | the same copy at that different path | fresh | on |

Two builds in the same directory agree with each other whether or not the binary
would match one built anywhere else. The two things that actually break
byte-identity are the **build path** (rustc bakes the absolute path of every
source file into panic locations and debug info) and the **toolchain version**,
and neither of them shows up in a same-directory double build. So A2 is a
sanity check, and B1 against B2 is the experiment that can fail.

### The result

```
--- same path, same toolchain, two fresh build dirs [shared] ---   MATCH
--- different path, remapping off (a plain cargo build) [shared] --- MATCH
--- different path, --remap-path-prefix on [shared] ---            MATCH

--- same path, same toolchain, two fresh build dirs [static] ---   MATCH
--- different path, remapping off (a plain cargo build) [static] --- DIFFER
--- different path, --remap-path-prefix on [static] ---            MATCH
```

**One of the six comparisons failed, and it is the informative one.** The
static archive built at a different path *without* `--remap-path-prefix` does not
match the one built here. With the flag on, it does. So path remapping is not
decoration on this project: it is the single difference between a build that can
be checked by a stranger and one that cannot.

**The two artefacts answer differently, which is why both are hashed.** The
cdylib matched across different directories *even with remapping off*, because
`[profile.release] strip = true` removes the symbols and debug info the build
path lives in. The staticlib has no such strip and does not match. Reporting
only the cdylib would have produced a clean table and a false conclusion — the
desktop CMake build links the archive, not the shared library.

### The source of the non-determinism, identified

The prompt asks for the cause rather than the observation, so it was measured
rather than guessed. The two archives were built side by side — one `--no-remap`,
one `--remap`, same sources — and the differing bytes read out directly:

```
sizes: 73869392 vs 73869392   (equal — not a path-length difference)
differing bytes: 1261   first at offset 507411
contiguous runs: 165
crate-identity names in the differing region: 79 of them in both
```

```
libc-5ca03e1b3e78aee9.libc.78b96294aa72792d      (no remap)
libc-c1ce7e49860bfa88.libc.78b96294aa72792d      (remap on)
```

**It is not the build path as text. It is a hash of the build path**, one per
crate — 81 crates in the graph have one of these names and all 81 changed. The
second hex group is stable across both builds; the first is not, and
`--remap-path-prefix` is what changes it. rustc derives a crate identity that
incorporates the source paths and embeds it in symbol names, and that identity is
what differs.

**A third build settles which input is responsible.** The obvious rival
explanation is the *target directory* rather than the remap flag, since both
builds also used different `--target-dir` values. It is not that: two `--no-remap`
builds into two different target directories produce **the same** hash
(`b960ff95…`), and the `--remap` build produces a third, equally stable value.
The remap flag is the cause, and the target directory is irrelevant.

### The lesson, which is the useful part

**Grepping the binary for the build path does not detect this.** Not one absolute
path appears in either artefact, in any of the three builds — `repro-check.sh`
section 3 now reports that per artefact, and the honest answer is "no" for a pair
whose hashes differ. The path reaches the output *as a hash*. Which means:

- a same-directory double build does not catch it;
- `ldd`/`nm`/`strings` do not catch it;
- and the only thing that catches it is **comparing the artefacts**.

That is why this phase's CI job compares hashes against a committed table rather
than scanning the binary, and why the table is a real deliverable rather than
bookkeeping. `scripts/repro-check.sh` section 3b now reports sizes, byte counts
and whether the differing region contains these names, and prints
`NOT per-crate identity hashes` when it does not — a diagnostic that asserts a
diagnosis it did not make would be worse than silence.

### Reproduce it yourself

```bash
bash scripts/repro-check.sh
```

Exit status is 0 when every comparison held, 1 when one did not, 2 when a build
failed. The script first greps `core/src` for `__DATE__`, `__TIME__`,
`SystemTime::now` and `Utc::now` and refuses to compare hashes if it finds any —
a timestamp is a reproducibility bug that costs one grep to find and would
otherwise surface only as a hash mismatch with no way to tell which of the four
builds was the odd one out. That grep currently finds nothing, and that is an
assertion rather than a hope.

---

## 3. What makes the build reproducible, and what is still not fixed

Four things, all applied by `scripts/build-release.sh`:

1. **`[profile.release]` already sets two of them**: `codegen-units = 1` and
   `lto = "thin"`. Parallel codegen merges units non-deterministically;
   `codegen-units = 1` removes the question. These were in the profile before
   this phase.
2. **`--remap-path-prefix`** maps both the source root and the Rust sysroot to
   `.`. Measured load-bearing — see §2, where it is also shown to be the *only*
   flag that matters here: the target directory was ruled out by a third build.
   The sysroot is remapped as well as the checkout, because the stdlib's own
   recorded paths come from there, so a runner with a differently located
   toolchain would otherwise produce different bytes while every flag was
   correctly set. **What it actually changes is a per-crate identity hash rather
   than a path string**, which is why nothing about it is visible to `strings`.
3. **`SOURCE_DATE_EPOCH=0`** is pinned. Nothing in this tree calls `__DATE__` or
   `__TIME__` — assertable, and asserted — so today it changes no output. It is
   pinned anyway, because a build that is reproducible today because nothing
   embeds a timestamp is one release away from being unreproducible.
4. **`CARGO_INCREMENTAL=0`** and **`--locked`**. Incremental compilation records
   absolute paths in its own metadata, and `--locked` means the dependency graph
   is the committed one rather than whatever the registry resolves today.

### What is not fixed, and cannot be from this repository

- **The toolchain version is not pinned.** The release profile uses `stable`,
  so a rustc release changes codegen and every hash in
  `scripts/BUILD-SHA256.txt` becomes stale. `rust-toolchain.toml` with a pinned
  version would fix it, at the cost of never getting new compiler fixes
  automatically — a policy decision, not a technical one, and it belongs to
  whoever owns the releases (phase-16). **Until it is decided, a `reproducible-build`
  CI failure means "the toolchain moved", not "someone tampered with the source".**
  The hashes record the rustc version they were produced with for exactly this
  reason.
- **Seven targets are unmeasured**, for the reasons in §1. The mechanism to
  measure them exists — the `reproducible-build` job in the patch below builds
  each one and compares against the committed row — but running it requires
  runners this pipeline does not have.
- **libwebp's C is not reproducible by our flags.** `libwebp-sys` compiles 159
  vendored `.c` files with `cc`, and a C compiler's output depends on the
  version of `cc` and the libc headers on the machine. So even a fully pinned
  Rust toolchain would not make a WebP-capable build reproducible across two
  different Linux distributions. This is stated here rather than discovered by
  somebody in six months.

---

## 4. The licence and advisory policy

`deny.toml` is the policy; `cargo deny check` is what enforces it.

| | Where it runs | Can a contributor run it |
| --- | --- | --- |
| `cargo deny check advisories licenses bans sources` | the `deny` job in `.github/workflows/supply-chain.yml`, on every push, every pull request and weekly | `scripts/deny-check.sh` |
| the offline subset (licences, bans, sources, wildcards) | `scripts/deny-check.sh`, and `PX_DENY=1 bash scripts/verify.sh` | yes, offline |
| the negative control | both of the above, and a CI step | `scripts/deny-check.sh --negative-control` |

**Why the policy is behind a flag in `scripts/verify.sh`.** `cargo deny check
advisories` fetches the RustSec advisory database: minutes on a cold cache, and a
network call in a repository whose whole thesis is that it needs no network.
Everything else in the gate is offline and fast, and the gate runs on every
phase. It is *not* optional in CI — the `deny` job runs it unconditionally — so
`PX_DENY=1` is the same policy made runnable by a contributor, which is the
difference between "CI would catch it" and a thing they can check.

**What is not suppressed.** Exactly one advisory is ignored:
`RUSTSEC-2024-0436` (`paste`, archived, reachable only through
rav1e ← ravif ← `image`'s AVIF encoder). The reason is in `deny.toml`, and the
review date — 2027-01-01 — is there too, because the pinned cargo-deny accepts
only `id` and `reason` on an ignore entry and `expiry` is a config-parse error.
The alternatives were deleting AVIF, which was weighed, and which lost.

**What is skipped, and why it is not the same thing.** Two duplicate crate
names, `miniz_oxide` and `syn`, each a pair of different MAJOR versions required
by different parents inside `image`'s dependency set. They cannot be collapsed
from here. A list of *four more* was removed by this phase: `cargo deny check
bans` reported each as `warning[unnecessary-skip]`, because cargo-deny's
duplicate detection does not follow dev-dependency edges and the `[graph]
targets` filter drops UEFI-only crates — so those four pairs exist in
`cargo tree --duplicates` and do not exist in the graph the policy is about.
An exception that no longer corresponds to anything is an exception nobody
re-reads.

### The negative control

**"cargo-deny is enabled" is a configuration claim. "cargo-deny rejects a GPL
dependency" is a different one**, and only the second is tested.
`scripts/deny-negative-control.py` injects known violations one at a time into
copies of the real policy, manifest and metadata, and asserts each is rejected —
a banned crate that is actually in the tree, a duplicate with no `skip` entry, a
wildcard version requirement, a crate whose licence is outside the allow list, and
a policy that tries to allow everything. If any of them is *accepted*, the
corresponding check above is decorative and the run fails.

That runs in three places, and `scripts/verify.sh` runs it under `PX_DENY=1`.

---

## 5. The SBOM

`scripts/sbom.py` emits a **CycloneDX 1.5** document for the engine from
`Cargo.lock`, optionally enriched with `cargo metadata` for licences and the
dependency graph.

```bash
cargo metadata --format-version 1 --all-features --manifest-path core/Cargo.toml > md.json
scripts/sbom.py --lock core/Cargo.lock --metadata md.json --platform aarch64-apple-ios \
  --out pixelsmith.cdx.json
```

Why a script rather than a tool: the obvious candidates do not exist for Rust.
`cargo-cyclonedx` is not published under either of the two names that suggest it,
`anchore/sbom-action` needs a runner and downloads a scanner, and
`cargo auditable` is an *authentication* format — a `.cargo_auditable.json`
inside the artefact — which is a different thing that answers a different
question. What is wanted is a file that lands next to the binary on every release
build, needs no network and no third-party scanner, and that two machines produce
identically.

That last property is why the document is a function of its inputs: the
`serialNumber` is a UUID derived from a hash of the lockfile rather than a random
one, `metadata.timestamp` is `SOURCE_DATE_EPOCH` rather than the wall clock, and
components and dependencies are sorted.

**It self-checks before it writes.** A generator that emits malformed SBOMs is
worse than no generator, because every tool downstream will trust the file.
`self_check` verifies the format and spec version, that the serial number is a
`urn:uuid`, that every lockfile package is present, that every component has a
name, a version and a purl, and that no dependency edge names a component that
does not exist. A failure exits 1 and writes nothing.

**The claim about artefacts.** `.github/workflows/build.yml` gains a step per
release build — one for the APK, one for the EXE — so the SBOM describing a
binary is attached to that binary rather than uploaded separately. `anchore/sbom-action`
already produces one per push in `supply-chain.yml`. The per-release steps are in
`workspace/phase-15/supply-chain-ci.patch`; see §6.

---

## 6. What is a patch, and why

`.github/workflows/build.yml` and `.github/workflows/supply-chain.yml` cannot be
modified from this pipeline. Pushing a workflow change is refused:

```console
$ git push origin main
 ! [remote rejected] main -> main (refusing to allow a GitHub App to create or
   update workflow `.github/workflows/build.yml` without `workflows` permission)
```

Adding `workflows: write` to `automation.yml`'s own `permissions:` block would
need a workflow push to take effect, so there is no self-resolving path. This is
the same wall as the unpushable `app/` submodule, and it gets the same answer: a
patch, and a claim narrowed to what is actually true.

**`workspace/phase-15/supply-chain-ci.patch` adds:**

| Job | What it does |
| --- | --- |
| `reproducible-build` (build.yml) | Builds each of six targets the way `build-release.sh` does and compares against the committed row in `scripts/BUILD-SHA256.txt`. A row reading `NOT-MEASURED` is a notice, not a failure — that is what makes the first run of a new target useful rather than red |
| SBOM per release artefact (build.yml) | One CycloneDX SBOM attached to the APK, one to the EXE |
| Negative control (supply-chain.yml) | Injects violations into a copy of `deny.toml` and fails if any is accepted, then `git diff --exit-code` to prove no tracked file was touched |
| `duplicate-versions` (supply-chain.yml) | Rewritten from "print the duplicates and emit a `::warning::`" into a real gate: every duplicate must be covered by a `[bans].skip` entry |
| `vet` (supply-chain.yml) | `cargo vet`, marked `continue-on-error` and named *advisory* in the job title, because `vet/imports.lock` has never been generated |
| `no-network` (supply-chain.yml) | Replaced by `scripts/no-network-report.sh --build`, so the banned-crate list exists once instead of three times |

**To land it:**

```bash
git apply workspace/phase-15/supply-chain-ci.patch
git add -A && git commit -m 'phase-15: enforce the supply-chain policy in CI' && git push
```

**So what is claimed today is what is in the tree.** `deny.toml` is committed and
`cargo deny check` runs in the `deny` job, which is in the tree today. The
negative control, the SBOM-per-release steps and the reproducible-build job are
one `git apply` away and are not claimed until it lands. This is the convention
every workflow change in this project follows; `docs/ARCHITECTURE.md` says the
same about `webp-lossy-matrix` and `docs/phase-status.md` about `bench.yml`.

---

## 7. Reproducing every number in this document

| Claim | Command | Machine |
| --- | --- | --- |
| §2, the four-build comparison | `bash scripts/repro-check.sh` | Linux x86_64, 4 cores, 15.9 GiB |
| §2, the measured hash | field 1 of `scripts/BUILD-SHA256.txt` | same, `rustc 1.99.0 (b940084d7 2026-09-28)` |
| §4, the policy verdict | `bash scripts/deny-check.sh --negative-control` | same |
| §4, which duplicates exist | `cargo tree --manifest-path core/Cargo.toml --duplicates -e normal,build` | same |
| §5, the SBOM | the two commands in §5 | same |
| the no-network claim | `bash scripts/no-network-report.sh --build` | same |

The `-e normal,build` on the duplicate-versions command is not decoration:
`cargo tree --duplicates` without it also reports `getrandom`, `itertools`,
`quick-error` and `r-efi`, all of which arrive only through dev-dependency edges
or are UEFI-only, and none of which cargo-deny's graph contains. A gate written
against the unfiltered command is red on every run for four crates the policy
deliberately does not cover.

**The whole of §1 was measured on one machine, one target.** The figures for the
other seven targets do not exist. Section 1 says so, and this table says it
again, because a table with one measured row and seven honest blanks is worth
more than eight confident guesses.

---

## 8. `cargo vet` — written, and never run

[`vet/config.toml`](../vet/config.toml) carries six hand-written audits:
`libwebp-sys`, `webp`, `heic-rs`, `jpeg-encoder`, `fast_image_resize` and
`paste`. Each records what was reviewed, what was **not**, and when to look
again.

**It has not been executed.** `cargo vet` was not installed on the machine this
was written on and could not be fetched. `vet/imports.lock` has never been
generated, so cargo-vet treats every crate not named in the config as unvetted
— which is the safe direction, and is why the config names exceptions rather
than approvals.

The rule this file is written to is that an audit entry asserts *somebody read
the code*. Writing one for a crate nobody read would make the file a list of good
intentions, which is worse than a short file. So it is six entries, not a hundred
and eighty-eight, and the reason each one exists is that it brings source this
project cannot review from source — vendored C, an unmaintained binding, or a
codec implementation with no upstream security process.

To run it, and to make the CI job gating rather than advisory:

```bash
cargo install cargo-vet --locked
cargo vet --manifest-path core/Cargo.toml      # generates vet/imports.lock
git add vet/imports.lock && git commit -m 'cargo vet: record the audited graph'
# then flip continue-on-error: false in the vet job of supply-chain.yml
```