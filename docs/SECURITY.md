# Security policy

This is the full policy for pixelsmith. The root [`SECURITY.md`](../SECURITY.md)
points here so the disclosure route is findable from the front page.

---

## 1. The threat model

### What this app holds

Photographs. Not documents, not a spreadsheet of somebody's finances — a
directory of pictures of their children, their house, their passport, and the
places those pictures were taken. A photo library is the most complete record of
a person's life available on a consumer device, and this app is handed a path to
it on purpose. That is the whole reason the design is what it is.

### What the attacker wants

| Goal | Why it is plausible | What stops it |
| --- | --- | --- |
| **Read files the user did not choose** | A crafted image that reaches outside its own directory | `worker::sanitise_stem` filters every output name; the ZIP path filters the same way. Format comes from magic bytes, never from the filename (hard rule 7) |
| **Exfiltrate the photo library** | The most valuable thing on the device is the photo library, and the app has a legitimate reason to open it | The engine has no network dependency, so it cannot send anything anywhere even if a bug tried to. See §4 — this is the claim everything else is subordinate to |
| **Crash the app with a file the user picked** | The user opens a photo from a message, a download, or a memory card. Crafted images are a well-established attack class | Every decode path is bounded (`validate::Limits`) and every failure is an `Error`, not a panic (hard rules 3 and 4). `sandbox.rs` adds a re-exec'd child process with an address-space ceiling for the untrusted case |
| **Read metadata the user believed was gone** | GPS, device serial, owner name, embedded thumbnail | Metadata is stripped by re-encoding from raw samples, not by clearing tags (hard rule 6). `exif::write_back` filters GPS and every identifying field, and the product does not expose the flag |
| **Trick the user into exporting the wrong thing** | A file called `photo.png` containing JPEG; a format the build cannot write, offered anyway | Format comes from magic bytes; `capabilities()` is generated from the same `cfg!` that decides the encoder dispatch (hard rule 10) |
| **Path traversal via a crafted filename** | Output names go into ZIP archives and onto the filesystem | `sanitise_stem` and `sanitise_path_component`, with the reserved-Windows-device-name list and Unicode dot handling both covered by named regression tests |
| **A supply-chain compromise** | 188 crates, one of which compiles 159 vendored C files and one of which is a binding last published in 2019 | `deny.toml` enforced by `cargo deny check` in CI, an SBOM per build, and `vet/config.toml` for the crates the advisory database cannot answer for. See `docs/SUPPLY-CHAIN.md` |

### Adversaries, ranked by how likely they are

1. **A stranger on the internet.** They do not run this app. They send the user a
   JPEG. Everything in rows 3 and 4 of the table above is about them.
2. **A compromised dependency.** Not an attacker with a CVE, necessarily — a
   publisher account taken over, a malicious version bump. This is why the
   dependency set is small, why `deny.toml` is enforced rather than documented,
   and why `Cargo.lock` is committed and every build uses `--locked`.
3. **Someone with the unlocked phone.** Out of reach: if they can unlock the
   device they can read the photo library without this app. This app adds no
   protection against that and does not claim any.
4. **A malicious or negligent local user.** Out of scope.

### Explicitly out of scope

- **The unlocked device.** See above.
- **Traffic analysis.** There is none; see §4.
- **The Flutter app and the Dart FFI boundary.** The engine is memory-safe Rust
  behind a C ABI; the app is Dart in another repository
  ([`authorss81/shrinkray`](https://github.com/authorss81/shrinkray)). A
  vulnerability in the app is a vulnerability in that project. The boundary
  *between* them is checked on both sides — `core/src/ffi_abi.rs` proves every
  declaration against the real signature at compile time, and `scripts/verify.sh`
  fails if any Dart test skipped rather than passing quietly.
- **Denial of service by volume.** A folder of 400 photographs takes as long as a
  folder of 400 photographs takes. There is a memory ceiling and a wall-clock
  timeout per file; there is no quota.
- **Confidentiality of the pixels in memory.** The engine reads the user's photo
  into RAM. It does not wipe it afterwards, and pretending otherwise would be a
  false claim.

---

## 2. What "secure" means here, specifically

"Secure" is not a property you can assert; it is a list of things this project
has decided to be true. Here is the list, and where each one is enforced.

1. **No panic on untrusted input.** Any decode, parse or dimension computation
   derives from bytes off the user's disk, so a panic is a crash and a crash on a
   crafted file is a vulnerability. `unwrap` and `expect` are not used on any
   value derived from those bytes. `sandbox.rs` gives this a process boundary as
   well as a discipline.
2. **Bounded resources, checked before allocation.** A file claiming 60000×60000
   costs ~14 GB once decoded, so the header is rejected on sight. The enforcement
   points are enumerated in `docs/ARCHITECTURE.md`; the two that matter most are
   `Limits::apply_to_decoder`, which pushes ceilings into `image`'s reader
   *before* it decodes, and `heic::header`, which reads a HEIF's geometry out of
   its `ispe` box because no image decoder does that.
3. **Contained failure.** Nothing unwinds out of a public entry point, so a
   malformed file cannot take down the host process. Every `Error` variant names
   what happened and what to do about it — "this picture is 64x64 and the request
   asked for 4000x3000; enlarging it adds no detail, so it was left alone", not
   "Error: decode failed".
4. **No capability to exfiltrate.** §4.
5. **No privacy leakage by default.** §1, row 4.
6. **No trust in filenames.** §1, rows 1 and 6.
7. **An auditable supply chain.** §5 and `docs/SUPPLY-CHAIN.md`.

### What is not finished, and is not claimed

- **Windows has no process sandbox.** `sandbox.rs` uses `setrlimit`, which is
  Unix-only. On Windows the in-process `Limits` still bound the decode — which is
  the property that protects the user — but there is no job object and no
  separate address space. `docs/phase-status.md` phase-04 records this as
  deliberately not implemented rather than implemented and untested.
- **The sandbox is not the default decode path.** It costs a process spawn per
  image, so `lib::decode_bounded` remains the default for ordinary files and the
  sandbox is available for the untrusted case.
- **`heic` is off by default.** A HEVC decoder is a large attack surface and the
  crate has not been cross-compiled to every shipped target. `vet/config.toml`
  records exactly which parts of it were reviewed and which were not.
- **A Display-P3 image loses its out-of-gamut colours.** That is a colour
  decision, not a security one, but it is the kind of thing a user should not
  discover after the fact. `docs/ARCHITECTURE.md` §"What is implemented, and what
  is approximated" is the honest account.

---

## 3. Reporting a vulnerability

Open a private security advisory: **Security → Report a vulnerability** on this
repository. Do not open a public issue, and please do not post a proof-of-concept
somewhere public before it is fixed.

Include:

- what you did, what you expected, and what happened;
- the input file, **if it is safe to share**. A reduced corpus entry is worth
  more than a 4 MB original: `scripts/fuzz.sh tmin <artifact> <target>` reduces a
  crash to a few hundred bytes, and `fuzz/README.md` has the rest;
- the affected version — the app version and, if you can get it,
  `core/CHANGELOG.md`'s most recent entry;
- whether you have told anyone else.

**What to expect.** An acknowledgement within 72 hours. A fix or a mitigation
plan, with a date, within 14 days for a crash, a decode-path memory-safety defect
or a privacy leak; there is no SLA for anything else and pretending otherwise
would be dishonest for a project of this size. Credit is yours to take, and you
can stay anonymous.

**There is no paid bounty.** This is a one-person project. If you want one, open
an issue asking for it — that is a fair ask, and "no" is the honest default at
this stage rather than a way of not saying no.

**Not a security issue.** A format this build cannot read, a colour-management
approximation, a HEIC metadata field that is not read, or a performance
regression are bugs, not vulnerabilities, and an issue is the right place. The
engine refusing a format with a sentence saying so is the designed behaviour
(hard rule 9), not a denial of service.

---

## 4. The no-network claim, and how to check it

> The engine has no network dependency, so it cannot send a photo anywhere even
> if a bug tried to.

This is the project's central promise, so it is written to be checkable by
somebody who does not trust this repository. Nothing here requires reading a
source file, and nothing requires trusting a sentence like this one.

### One command

```bash
git clone https://github.com/authorss81/pixelsmith
cd pixelsmith
bash scripts/no-network-report.sh --build
```

It runs the five checks below, prints each one with its evidence, and exits
non-zero if any fails. It is the same list `scripts/verify.sh` section 1 and the
`no-network` job in `.github/workflows/supply-chain.yml` enforce, with the
reasoning written down in the script.

### By hand

```bash
# 1. No network-capable crate in the transitive dependency graph.
cargo tree --manifest-path core/Cargo.toml --all-features \
  | grep -E 'reqwest|hyper|ureq|curl|tokio|openssl|rustls|native-tls|hickory|socket2|mio|dns-lookup'
# Expected: no output.

# 2. No networking symbol in the engine's source.
grep -rnE 'std::net|TcpStream|UdpSocket|lookup_host' core/src
# Expected: no output.

# 3. The release library loads no TLS or HTTP library.
cargo build --manifest-path core/Cargo.toml --release
ldd core/target/release/libpixelsmith_core.so | grep -Ei 'ssl|crypto|curl'
# macOS:  otool -L core/target/release/libpixelsmith_core.dylib | grep -Ei 'SSL|Crypto'
# Windows: dumpbin /dependents core\target\release\pixelsmith_core.dll
# Expected: no output.

# 4. And it has no networking entry points to call in the first place.
nm -D --undefined-only core/target/release/libpixelsmith_core.so \
  | grep -E '^ *U (socket|connect|getaddrinfo|SSL_|curl_)'
# Expected: no output.
```

Check 4 is the one the documented set was missing, and it is the strongest of
the four. `ldd` tells you which libraries are loaded, so it cannot see a
*statically linked* `socket()` call; the undefined dynamic symbols are where any
such call has to appear. An engine with no networking code has none of these
symbols, while one that merely forgot to link libcurl would pass check 3.

### From the published artefact

The same checks apply to the shipped binary, without building anything. Download
the release artefact and run checks 3 and 4 against it. `docs/SUPPLY-CHAIN.md`
records the SHA-256 of every release engine build per target, so "this is the
binary we published" is a checkable statement and not a promise.

### The check that actually matters

**Install the app, disconnect from the network, and use it.** It should not
notice. Checks 1–4 are proxies for that, and a proxy that stops being
maintained stops meaning anything — which is why they are in CI and in
`scripts/verify.sh` rather than only in this document.

### What the claim does not cover

- The Flutter app. Dart has its own HTTP stack and this repository does not build
  the app; `app/` is a submodule pointing at another repository. The engine claim
  is about `core/`.
- Whatever the operating system does with the process. A compromised OS can read
  anything.

---

## 5. Fuzzing

`fuzz/` holds eleven `cargo-fuzz` targets, one per entry point where untrusted
bytes become something the engine does: `detect_format`, `validate_bytes`,
`decode_bounded`, `exif_read`, and one per decoder (JPEG, PNG, WebP, GIF, TIFF,
BMP, ICO). `fuzz/README.md` explains each one and how to run it.

```bash
bash scripts/fuzz.sh run                 # all eleven, 30 seconds each
bash scripts/fuzz.sh run decode_png      # one, longer
bash scripts/fuzz.sh tmin <artifact> decode_png
```

**There is no nightly fuzzing job.** That is stated rather than implied: it would
have to be a patch in `.github/workflows/`, and a document claiming nightly
fuzzing runs is worse than one saying it does not. Running the suite is a
thirty-second-per-target command and the seeds are committed.

---

## 6. Supply chain

- **Licences and advisories are enforced**, not documented: `cargo deny check`
  runs on every push, every pull request and weekly in
  `.github/workflows/supply-chain.yml`, against `deny.toml`. The local entry
  point is `scripts/deny-check.sh`, which also runs an offline subset and a
  negative control.
- **An SBOM in CycloneDX** is produced for every build.
- **Release binary hashes per target** are in `scripts/BUILD-SHA256.txt`, and
  whether each target's build is reproducible — including the ones where the
  answer is "no" — is in [`docs/SUPPLY-CHAIN.md`](SUPPLY-CHAIN.md).
- **Crates the advisory database cannot answer for** are audited by hand in
  [`vet/config.toml`](../vet/config.toml), each with what was reviewed, what was
  not, and when to look again.

---

## 7. Supported versions

| Version | Supported | Engine |
| --- | --- | --- |
| 0.1.x | yes | `pixelsmith_core` 0.1.0, `core/Cargo.toml` |

There is one version, because there has been one release. When 0.2.0 ships, this
table becomes two rows and the 0.1.x row gains an end-of-life date rather than
losing support silently.

**Pre-releases and `main`** are not supported. There is no user-facing
pre-release to support; a build from `main` is a build nobody has tested on a
device.

**What "supported" means concretely:** security fixes are released for it, and a
report against it is triaged. It does not mean every format works on every
platform — `lib::capabilities()` is the per-build truth, and a format this build
cannot write is refused by name rather than failed at export time.