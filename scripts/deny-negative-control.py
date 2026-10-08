#!/usr/bin/env python3
"""Prove each of deny.toml's checks is live by making it fail on purpose.

Every check in `scripts/lockfile-policy.py` is asserted against the *real* policy,
the *real* engine manifest and the *real* dependency graph, with exactly one
violation injected into a deep copy. Nothing here is a fixture: the crates, the
licences and the duplicate versions are the ones that will be built.

Why this exists: a security check that has never been observed to reject anything
is indistinguishable from a security check that cannot reject anything. Phase-01
recorded two `deny.toml` keys that could not even be parsed, and the only way
that was found was that somebody ran the real tool. A control that never fails
would have passed just as silently.

A control asserts on the *code*, not merely on the exit status. If the licence
check were removed from the checker and the injected bad licence went unnoticed,
the exit status alone would still be 1 for some other reason; requiring the
specific PXDENY code makes that impossible.

Exit status: 0 every control fired, 1 at least one did not.
"""

from __future__ import annotations

import argparse
import copy
import importlib.util
import json
import sys
import tomllib
from pathlib import Path

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("lockfile_policy", HERE / "lockfile-policy.py")
if SPEC is None or SPEC.loader is None:  # pragma: no cover - a missing sibling file
    print("deny-negative-control: scripts/lockfile-policy.py is missing", file=sys.stderr)
    sys.exit(2)
LP = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(LP)

# A licence identifier that exists in SPDX's grammar and that no crate in this
# graph uses. Injecting it as the WHOLE allow list means every package violates,
# which is what makes the control independent of which licences happen to be in
# the tree today.
CONTROL_LICENCE = "LicenseRef-PXDENY-control"


def evaluate(policy: dict, manifest: dict, packages: list[dict]) -> list[str]:
    """Run every check the same way lockfile-policy.main does."""
    for pkg in packages:
        pkg.setdefault("source", LP.LOCAL)
    findings: list[str] = []
    findings += LP.check_licences(policy, packages)
    findings += LP.check_banned(policy, packages)
    findings += LP.check_duplicates(policy, packages)
    findings += LP.check_sources(policy, packages)
    findings += LP.check_wildcards(policy, manifest)
    return findings


def a_registry_package(packages: list[dict]) -> dict:
    for pkg in packages:
        if pkg.get("source") == LP.CRATES_IO:
            return pkg
    raise SystemExit("deny-negative-control: no crates.io package in the metadata; cannot build a control")


# --- the controls ----------------------------------------------------------
# Each returns (name, expected PXDENY code, findings, policy, manifest, packages).


def control_licence(policy, manifest, packages):
    p = copy.deepcopy(policy)
    p["licenses"]["allow"] = [CONTROL_LICENCE]
    return "a licence outside the allow list", "PXDENY[licence]", evaluate(p, manifest, packages)


def control_banned(policy, manifest, packages):
    p = copy.deepcopy(policy)
    victim = a_registry_package(packages)["name"]
    p["bans"].setdefault("deny", []).append({"name": victim})
    return (
        f"a crate named in [bans].deny ({victim})",
        "PXDENY[banned]",
        evaluate(p, manifest, packages),
    )


def control_duplicate(policy, manifest, packages):
    # Emptying the skip list rather than adding to `deny` is the point: the real
    # policy's own exceptions are what make it green, so a checker that ignored
    # the skip list would still be green on the unmutated policy and would go
    # unnoticed. This asserts the exceptions are load-bearing.
    p = copy.deepcopy(policy)
    p["bans"]["skip"] = []
    return (
        "a duplicate crate version with no [bans].skip entry",
        "PXDENY[duplicate]",
        evaluate(p, manifest, packages),
    )


def control_source(policy, manifest, packages):
    p = copy.deepcopy(policy)
    pkgs = copy.deepcopy(packages)
    victim = a_registry_package(pkgs)
    victim["source"] = "git+https://pxdeny-control.invalid/example.git"
    return (
        "a package from a git source",
        "PXDENY[source]",
        evaluate(p, manifest, pkgs),
    )


def control_wildcard(policy, manifest, packages):
    p = copy.deepcopy(policy)
    m = copy.deepcopy(manifest)
    m.setdefault("dependencies", {})["pxdeny_control"] = {"version": "*"}
    return (
        'a wildcard version requirement ("*")',
        "PXDENY[wildcard]",
        evaluate(p, m, packages),
    )


CONTROLS = (
    control_licence,
    control_banned,
    control_duplicate,
    control_source,
    control_wildcard,
)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Inject one deny.toml violation at a time and assert each is caught.")
    parser.add_argument("--policy", type=Path, default=Path("deny.toml"))
    parser.add_argument("--metadata", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, default=Path("core/Cargo.toml"))
    args = parser.parse_args(argv)

    policy = tomllib.loads(args.policy.read_text())
    manifest = tomllib.loads(args.manifest.read_text())
    packages = json.loads(args.metadata.read_text())["packages"]

    baseline = evaluate(copy.deepcopy(policy), manifest, copy.deepcopy(packages))
    print(f"negative-control: the unmutated policy produces {len(baseline)} finding(s)")
    for line in baseline:
        print(f"  {line}")
    if baseline:
        # Not fatal on its own: the controls below still have to fire. But a
        # policy that is already red means the real gate is red too, and saying
        # so here is cheaper than finding out in CI.
        print("negative-control: NOTE the real policy is not clean; see scripts/deny-check.sh")

    failures = 0
    print("")
    for build in CONTROLS:
        description, expected, findings = build(policy, manifest, packages)
        hit = [line for line in findings if line.startswith(expected)]
        if hit:
            print(f"  ok    {description}")
            print(f"          -> {hit[0]}")
        else:
            failures += 1
            print(f"  FAIL  {description}: nothing reported {expected}")
            for line in findings[:5]:
                print(f"          (reported instead: {line})")

    print("")
    print(f"negative-control: {len(CONTROLS) - failures}/{len(CONTROLS)} controls fired")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))