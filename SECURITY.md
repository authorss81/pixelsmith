# Security Policy

The full policy — threat model, what "secure" means here, the no-network claim
and how to verify it yourself, the fuzzing setup, disclosure and supported
versions — is in **[`docs/SECURITY.md`](docs/SECURITY.md)**.

## Reporting a vulnerability

**Security → Report a vulnerability** on this repository. Please do not open a
public issue, and please do not publish a proof-of-concept before it is fixed.
Include a reduced input file if you can; a corpus entry beats a 4 MB original.
The full document has what to include and what to expect in reply.

## The one claim worth checking first

This app's entire job is opening files the user did not create, and its central
promise is that it holds their photographs and never sends them anywhere. That
promise is checkable without trusting this repository:

```bash
bash scripts/no-network-report.sh --build
```

It checks the dependency graph, the source, the release library's dynamic
imports and its undefined dynamic symbols, and prints a verdict. `docs/SECURITY.md`
§4 has the same four checks as plain commands, plus what to do when you have only
the published binary and not a checkout.

The check that actually settles it is not a script: install the app, disconnect
from the network, and use it. It should not notice.

## Short version of what "secure" means here

No panic on untrusted input; bounded memory checked *before* allocation;
contained failure with messages a person can act on; no capability to exfiltrate;
no privacy leakage by default (metadata is stripped by re-encoding, and GPS and
identifying tags are never written back even when metadata preservation is asked
for); no trust in filenames; and an auditable supply chain
([`docs/SUPPLY-CHAIN.md`](docs/SUPPLY-CHAIN.md)).

## Known gaps, stated rather than implied

- **Windows has no process sandbox.** `setrlimit` is Unix-only, so on Windows the
  in-process limits bound the decode but there is no job object and no separate
  address space.
- **No nightly fuzzing job.** `bash scripts/fuzz.sh run` is the honest current
  state; `fuzz/README.md` says why.
- **`cargo vet` has not been run.** The audit files in `vet/config.toml` are
  written and unexecuted, and `docs/SUPPLY-CHAIN.md` says so.

The full list, with the reasoning behind each, is in `docs/SECURITY.md` §2.