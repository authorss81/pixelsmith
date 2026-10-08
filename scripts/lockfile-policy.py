#!/usr/bin/env python3
"""Evaluate the parts of deny.toml that a lockfile alone can decide.

cargo-deny is the real policy engine and it is what CI runs (see
.github/workflows/supply-chain.yml). It needs a network fetch of the advisory
database, so it is not available in every environment -- it was not installed at
all on the runner this file was written on.

This is not a second copy of the policy. It READS deny.toml, so the allow-list,
the banned names and the skip list live in exactly one place, and an edit to
deny.toml changes what this enforces without anyone editing this file. What it
cannot decide is stated in WHAT IS NOT COVERED below, and `scripts/deny-check.sh`
prints that list every run so a partial verdict never reads as a full one.

WHAT IS COVERED (all four come straight out of deny.toml):

  licence   [licenses].allow  vs every licence expression in the dependency graph
  banned    [bans].deny       vs every package name
  duplicate [bans].skip       vs every package name that resolves to 2+ versions
  source    [bans]/[sources]  vs every package source

WHAT IS NOT COVERED, and why:

  advisories   `[advisories] yanked = "deny"` and `unmaintained = "all"` are
               judgements about the RustSec advisory database. There is no
               database in this repository, so there is nothing offline to
               evaluate them against. cargo deny check advisories is the only
               thing that can, and CI runs it.
  confidence   `[licenses] confidence-threshold` scores how sure the scanner is
               about a licence detection. Computing it offline would mean
               reimplementing cargo-deny's scanner.
  wildcards    Version requirements containing `*` or an `x` wildcard, read
               from the manifest rather than the lockfile. Checked here
               (code WILDCARD) but only the textual form: a transitive crate's
               own manifest is not in this repository.

EXIT STATUS
  0  every covered check passed
  1  at least one finding
  2  the policy or the inputs could not be read -- which is NOT "the tree is
     clean", and is deliberately distinct from 0 so a missing file cannot be
     mistaken for a pass.
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import re
import sys
import tomllib
from collections import defaultdict
from pathlib import Path

CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"

# A path dependency (the workspace member) has no source and is not covered by
# the licence check: it is this repository's own code, licensed MIT OR
# Apache-2.0 in core/Cargo.toml.
LOCAL = "<local>"


def finding(code: str, message: str) -> str:
    """One finding, in a shape both a human and a grep can read.

    The PXDENY[...] prefix is load-bearing: `scripts/deny-check.sh
    --negative-control` injects known violations and then asserts that *this*
    code appears, so a check that fails for an unrelated reason cannot satisfy
    the control.
    """
    return f"PXDENY[{code}] {message}"


# ---------------------------------------------------------------------------
# Licence expressions

# An SPDX licence expression is a boolean formula, and the policy is an
# allow-list, so the question is satisfiability: can every AND-conjunct be
# satisfied by choosing one branch of each OR, with every chosen licence in the
# allow list?
#
# Flattening the expression to a list of identifiers gets that wrong in both
# directions, and both directions were live findings the first time this ran
# against the real tree:
#
#   "MIT OR Apache-2.0 OR LGPL-2.1-or-later"  (r-efi)
#       flat  -> LGPL-2.1-or-later is not allowed, so the crate is refused
#       right -> MIT satisfies it, so the crate is fine. Refusing it would push a
#                copyleft identifier into the allow list to silence a bug, which
#                is precisely the outcome a licence policy must never produce.
#
#   "Apache-2.0 WITH LLVM-exception"
#       flat  -> "LLVM-exception" is treated as a licence and is not allowed
#       right -> WITH is a modifier on a licence that IS allowed.
#
# So the grammar is parsed properly. Precedence is WITH, then AND, then OR, per
# the SPDX specification. Anything that does not tokenise is refused, because a
# licence expression we cannot read is a licence we cannot approve.

_TOKEN = re.compile(r"[A-Za-z0-9][A-Za-z0-9.+_-]*|[()]")
_IDENT_CHAR = re.compile(r"[A-Za-z0-9.+_-]")


def licence_permitted(identifier: str, allowed: list[str]) -> bool:
    """Does `[licenses].allow` permit this identifier?

    Two forms, matching what cargo-deny accepts: an exact SPDX identifier, or a
    glob (`Apache-*`), because an allow-list of every BSD variant is an
    allow-list nobody will keep up to date.
    """
    return any(pattern == identifier or fnmatch.fnmatchcase(identifier, pattern) for pattern in allowed)


class _Spdx:
    """A recursive-descent evaluator for one SPDX licence expression.

    A class rather than a pile of closures because the parser threads one
    position through four mutually-recursive rules, and closures that mutate a
    `nonlocal` position are exactly the shape where a typo turns into a
    `UnboundLocalError` at run time rather than at review time — on the one code
    path whose failure mode is "silently approves everything".
    """

    def __init__(self, tokens: list[str], allowed: list[str]) -> None:
        self.tokens = tokens
        self.allowed = allowed
        self.at = 0

    def _peek(self) -> str | None:
        return self.tokens[self.at] if self.at < len(self.tokens) else None

    def _operator(self, word: str) -> bool:
        token = self._peek()
        if token is None or token.upper() != word:
            return False
        self.at += 1
        return True

    def satisfied(self) -> bool:
        return self._disjunction() and self.at == len(self.tokens)

    def _disjunction(self) -> bool:
        value = self._conjunction()
        while self._operator("OR"):
            # `or` must see both operands, so the right one is always evaluated.
            value = self._conjunction() or value
        return value

    def _conjunction(self) -> bool:
        value = self._atom()
        while self._operator("AND"):
            value = self._atom() and value
        return value

    def _atom(self) -> bool:
        token = self._peek()
        if token == "(":
            self.at += 1
            value = self._disjunction()
            if self._peek() == ")":
                self.at += 1
            else:
                # An unbalanced parenthesis is not an expression.
                value = False
            return value
        if token is None or not _IDENT_CHAR.match(token[0]):
            # Consume it anyway, so one stray character cannot make the rest of
            # the expression parse as something else.
            self.at += 1
            return False
        identifier = token
        self.at += 1
        if self._peek() is not None and self._peek().upper() == "WITH":
            self.at += 1
            exception = self._peek()
            if exception is None:
                return False
            self.at += 1
            # Listing the full "A WITH B" permits exactly that pairing; listing
            # "A" alone permits A with any exception, which is how cargo-deny
            # reads it and why deny.toml carries both spellings.
            if f"{identifier} WITH {exception}" in self.allowed:
                return True
        return licence_permitted(identifier, self.allowed)


def expression_permitted(expression: str, allowed: list[str]) -> bool:
    """Is there a choice of licences in `expression` that `allowed` permits?"""
    # SPDX's original `/` operator is deprecated but still extremely common in
    # the wild — nine crates in this tree write `MIT/Apache-2.0`. cargo-deny
    # normalises it to `OR`, so this does too; leaving it out would report
    # "Apache-2.0/MIT" as unsatisfiable, which is a false positive that pushes
    # somebody to widen the allow list to make a red build green.
    return _Spdx(_TOKEN.findall(expression.replace("/", " OR ")), allowed).satisfied()


# ---------------------------------------------------------------------------
# Which packages are actually in the binary


def built_packages(metadata: dict) -> list[dict]:
    """The packages cargo-deny's graph contains.

    Not quite "what gets compiled", and the difference is deliberate: this walks
    the same graph `cargo deny` does, because a checker that disagreed with the
    gate would report a clean tree while CI reported a violation.

    That means:

    * dev-dependencies of the **workspace member** are included. Cargo resolves
      them and cargo-deny evaluates them, so proptest, tempfile and criterion are
      in scope even though `cargo build --release` never compiles them.
    * dev-dependencies of **transitive** crates are not. `cargo metadata` lists
      them, and a naive pass over the whole `packages` list decides on code that
      is not in the artefact.
    * every cfg target is followed, because `cfg(fuzzing)` is not a target
      triple and `--filter-platform` cannot switch it off. So `libfuzzer-sys`
      (a normal dependency of `rav1e` under `cfg(fuzzing)`) *is* in the graph,
      and its NCSA licence really does have to be permitted for the gate to
      pass. That is not the checker being pedantic; it is what cargo-deny sees.

    If the metadata carries no `resolve` section there is nothing to walk and
    every package is returned: a checker that silently checked less would be
    worse than one that checks more.
    """
    packages = {pkg["id"]: pkg for pkg in metadata.get("packages", []) if pkg.get("id")}
    resolve = metadata.get("resolve")
    nodes = {node["id"]: node for node in (resolve or {}).get("nodes", [])}
    members = set(metadata.get("workspace_members") or [])
    if not resolve or not members or not nodes:
        return list(packages.values())

    seen: set[str] = set()
    stack = list(members)
    while stack:
        node_id = stack.pop()
        if node_id in seen:
            continue
        seen.add(node_id)
        node = nodes.get(node_id)
        if node is None:
            continue
        # A dev edge out of a workspace member is followed; a dev edge out of
        # anything else is not.
        allow_dev = node_id in members
        for dep in node.get("deps", []):
            kinds = dep.get("dep_kinds") or [{}]
            if not allow_dev and all(kind.get("kind") == "dev" for kind in kinds):
                continue
            stack.append(dep["pkg"])
    out = [packages[node_id] for node_id in seen if node_id in packages]
    return out or list(packages.values())


# ---------------------------------------------------------------------------
# Version requirements


def is_wildcard(requirement: str) -> bool:
    """cargo-deny's wildcard test, read off the requirement text.

    `1`, `1.2`, `1.2.3` are pins. `*`, `1.*`, `1.2.*`, `1.x`, `1.2.x` are
    wildcards: a requirement that resolves to whatever a future publish
    happens to be, which is the thing `[bans] wildcards = "deny"` forbids.
    """
    if requirement in ("*",):
        return True
    return bool(re.search(r"(^|[.\s])[*xX](\.|$)", requirement))


def iter_requirements(manifest: dict) -> list[tuple[str, str, str]]:
    """(kind, name, requirement) for every version requirement in a manifest.

    Cargo's optional-dependency syntax puts the dependency name in a table key
    and its requirement in a `version` field, so `[dependencies.webp]` is not a
    crate called `webp` with requirement `[dependencies.webp]` -- hence the
    three table kinds are walked separately and `version` is read from inside.
    """
    out: list[tuple[str, str, str]] = []

    def walk(table: str, names: dict) -> None:
        for name, spec in names.items():
            if isinstance(spec, str):
                out.append((table, name, spec))
            elif isinstance(spec, dict):
                version = spec.get("version")
                if isinstance(version, str):
                    out.append((table, name, version))

    for table in ("dependencies", "dev-dependencies", "build-dependencies"):
        walk(table, manifest.get(table) or {})
    for target in (manifest.get("target") or {}).values():
        for table in ("dependencies", "dev-dependencies", "build-dependencies"):
            walk(f"target.{table}", target.get(table) or {})
    return out


# ---------------------------------------------------------------------------
# The checks


def check_licences(policy: dict, packages: list[dict]) -> list[str]:
    licenses = policy.get("licenses") or {}
    allowed = licenses.get("allow") or []
    if not allowed:
        return [finding("config", "[licenses] has no allow list; nothing is permitted or denied")]
    out: list[str] = []
    for pkg in packages:
        if pkg.get("source") == LOCAL:
            continue
        name = pkg.get("name", "?")
        version = pkg.get("version", "?")
        expression = pkg.get("license")
        if not expression:
            out.append(
                finding(
                    "unknown-licence",
                    f"{name} {version} declares no licence expression"
                    + (f", only a licence file ({pkg['license_file']})" if pkg.get("license_file") else "")
                    + "; cargo-deny treats this as an error, so it is one here",
                )
            )
            continue
        if not expression_permitted(expression, allowed):
            out.append(
                finding(
                    "licence",
                    f"{name} {version} is {expression}, which no choice of licences in "
                    f"[licenses].allow can satisfy",
                )
            )
    return out


def check_banned(policy: dict, packages: list[dict]) -> list[str]:
    banned = {entry.get("name") for entry in (policy.get("bans") or {}).get("deny") or []}
    if not banned:
        return []
    return [
        finding("banned", f"{pkg.get('name')} {pkg.get('version')} is named in [bans].deny")
        for pkg in packages
        if pkg.get("name") in banned
    ]


def check_duplicates(policy: dict, packages: list[dict]) -> list[str]:
    bans = policy.get("bans") or {}
    # `multiple-versions` is the switch and `skip` is the exception list. Both are
    # honoured: honouring only the skip list would report duplicates on a policy
    # that explicitly allows them, which is a finding with no rule behind it.
    if bans.get("multiple-versions") == "allow":
        return []
    skipped = {entry.get("crate") for entry in bans.get("skip") or []}
    versions: dict[str, set[str]] = defaultdict(set)
    for pkg in packages:
        if pkg.get("source") == LOCAL:
            continue
        versions[pkg["name"]].add(pkg["version"])
    out: list[str] = []
    for name in sorted(versions):
        found = sorted(versions[name])
        if len(found) < 2:
            continue
        if name in skipped:
            continue
        out.append(
            finding(
                "duplicate",
                f"{name} resolves to {len(found)} versions ({', '.join(found)}) "
                f"and is not in [bans].skip",
            )
        )
    return out


def check_sources(policy: dict, packages: list[dict]) -> list[str]:
    exceptions = policy.get("sources") or {}
    allowed_git = set(exceptions.get("allow-git") or [])
    allowed_registry = set(exceptions.get("allow-registry") or [])
    out: list[str] = []
    for pkg in packages:
        source = pkg.get("source")
        if source in (None, LOCAL) or source == CRATES_IO:
            continue
        if source.startswith("git+") and source[4:] in allowed_git:
            continue
        if source.startswith("registry+") and source[9:] in allowed_registry:
            continue
        out.append(finding("source", f"{pkg.get('name')} {pkg.get('version')} comes from {source}"))
    return out


def check_wildcards(policy: dict, manifest: dict) -> list[str]:
    bans = policy.get("bans") or {}
    if bans.get("wildcards") == "allow":
        return []
    out: list[str] = []
    for table, name, requirement in iter_requirements(manifest):
        if is_wildcard(requirement):
            out.append(
                finding(
                    "wildcard",
                    f"[{table}] {name} = \"{requirement}\" is a wildcard requirement, "
                    f"and [bans] wildcards is {bans.get('wildcards', 'unset')!r}",
                )
            )
    return out


# ---------------------------------------------------------------------------


def load(path: Path) -> dict:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--policy", default="deny.toml", type=Path, help="the cargo-deny policy (default: deny.toml)")
    parser.add_argument(
        "--metadata",
        type=Path,
        required=True,
        help="`cargo metadata --format-version 1` output; every package needs name, version, source, license",
    )
    parser.add_argument(
        "--manifest",
        default=Path("core/Cargo.toml"),
        type=Path,
        help="the engine manifest, read for version requirements (default: core/Cargo.toml)",
    )
    args = parser.parse_args(argv)

    try:
        policy = load(args.policy)
        metadata = json.loads(args.metadata.read_text())
    except (OSError, tomllib.TOMLDecodeError, json.JSONDecodeError) as err:
        print(f"lockfile-policy: cannot read the inputs: {err}", file=sys.stderr)
        return 2

    packages = metadata.get("packages")
    if not isinstance(packages, list) or not packages:
        print("lockfile-policy: the metadata carries no packages; refusing to call that clean", file=sys.stderr)
        return 2

    # `source` is absent for a path dependency, which is how it is told apart
    # from a registry one without inventing a sentinel in the JSON.
    for pkg in packages:
        pkg.setdefault("source", LOCAL)

    # The dev-dependencies of transitive crates are in the graph but not in the
    # binary. See built_packages.
    built = built_packages(metadata)

    try:
        manifest = load(args.manifest)
    except (OSError, tomllib.TOMLDecodeError) as err:
        print(f"lockfile-policy: cannot read {args.manifest}: {err}", file=sys.stderr)
        return 2

    findings: list[str] = []
    findings += check_licences(policy, built)
    findings += check_banned(policy, built)
    findings += check_duplicates(policy, built)
    findings += check_sources(policy, built)
    findings += check_wildcards(policy, manifest)

    for line in findings:
        print(line)

    scoped = (
        f"{len(built)} of {len(packages)} resolved packages are in cargo-deny's graph"
        if built
        else "no packages are in cargo-deny's graph"
    )
    print(
        f"lockfile-policy: {scoped}; checked against {args.policy}; {len(findings)} finding(s)."
    )
    if findings:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))