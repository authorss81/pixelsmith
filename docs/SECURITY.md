# Security Policy

## Reporting a vulnerability

Open a private security advisory on this repository:
**Security → Report a vulnerability**. Do not open a public issue.

Include: what you did, what you expected, what happened, the input file if it is
safe to share, and the affected version. A proof-of-concept image is far more
useful than a description of one.

We aim to acknowledge within 72 hours. There is no paid bounty. If you want one,
open an issue asking for it — that is a fair ask, and "no" is the honest default
for a project at this stage.

## What "secure" means here

This app's job is to open files the user did not create. That makes the image
decoder the entire attack surface. So security for this project means:

1. **No panic on untrusted input.** Any decode, parse or dimension computation
   derives from bytes off the user's disk. A panic is a crash, and a crash on a
   crafted file is a vulnerability.
2. **Bounded resources.** Dimensions and pixel counts are validated *before*
   allocation. A file claiming 60000×60000 costs 14 GB once decoded; the header
   is rejected on sight.
3. **Contained failure.** Decoding happens in a separate process with a hard
   memory ceiling and a wall-clock timeout, so even a defect that defeats the
   header checks cannot take down the UI.
4. **No capability to exfiltrate.** The engine has no network dependency, so it
   cannot send a photo anywhere even if a bug tried to.
5. **No privacy leakage by default.** Metadata is stripped by re-encoding, and
   GPS and identifying tags are never written back — not even when a user
   explicitly asks to preserve metadata.
6. **No trust in filenames.** Format comes from magic bytes. Output names are
   sanitised against path traversal, including inside generated ZIP archives.

## How to verify the no-network claim

Do not take our word for it. From a clean checkout:

```bash
# 1. No network-capable crate in the engine's dependency graph.
cargo tree --manifest-path core/Cargo.toml \
  | grep -E 'reqwest|hyper|ureq|curl|isahc|surf|attohttpc|tokio|async-std|smol|quinn|http|tungstenite|openssl|rustls|native-tls|hickory|socket2|mio|dns-lookup'
# Expected: no output.

# 2. No networking symbol in the engine's source.
grep -rnE 'std::net|TcpStream|UdpSocket|lookup_host|reqwest|hyper::' core/src
# Expected: no output.

# 3. Same, for the shipped release binary's dynamic imports.
ldd core/target/release/libpixelsmith_core.so | grep -E 'ssl|crypto|curl'
# macOS: otool -L ... | grep -E 'SSL|Crypto'
# Windows: dumpbin /dependents pixelsmith_core.dll

# 4. Or just disconnect from the network and use the app. It should not care.
```

Check 4 is the one that actually matters. Everything above is a proxy for it.

The first two checks also run on every phase verification, in
`scripts/verify.sh` section 1. A change that breaks them fails the build.

## Supported versions

| Version | Supported |
| --- | --- |
| 0.1.x | yes |

There is one version. When that changes, this table changes with it.

## Fuzzing

`fuzz/` holds one `cargo-fuzz` target per decode entry point. See
`fuzz/README.md` for how to run them and what a finding means. A short session
runs on every verification behind `PX_FUZZ=1`; longer sessions run nightly in
`.github/workflows/fuzz.yml`.

If you find a crash, reduce it before reporting: a corpus entry plus the
`ValidateError` you get is far more useful than a 4 MB file.

## Supply chain

- Licence policy is enforced by `cargo-deny` in CI; see `deny.toml`.
- An SBOM is emitted on every release build.
- Reproducible-build status per target is recorded in `docs/SUPPLY-CHAIN.md`.
  "Not reproducible" is an acceptable published answer; silence is not.
- Security-relevant dependencies are tracked with `cargo-vet` where the stock
  advisory database does not cover them.