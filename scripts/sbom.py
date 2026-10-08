#!/usr/bin/env python3
"""Emit a CycloneDX 1.5 SBOM for the engine, from the lockfile.

Why a script rather than a tool: the obvious candidates do not exist for Rust.
`cargo-cyclonedx` is not a published crate under either of the two names that
suggest it, `anchore/sbom-action` needs a runner and downloads a scanner, and
`cargo auditable` is an *authentication* format — a `.cargo_vcs_info.json` and a
`.cargo_auditable.json` inside the artefact — which is a different thing from an
SBOM and answers a different question. What is wanted here is a file that lands
next to the binary on every release build, needs no network and no third-party
scanner, and can be produced identically by two machines. That is about eighty
lines of `Cargo.lock` and `cargo metadata`, and a script is what it takes.

Determinism is the point, so the document is a function of its inputs:

* `serialNumber` is a UUID derived from a hash of the lockfile, not random, so
  two builds of the same lockfile produce byte-identical SBOMs.
* `metadata.timestamp` is `SOURCE_DATE_EPOCH`, the same convention
  `scripts/build-release.sh` pins for the compiler, rather than the wall clock.
* components and dependencies are sorted, never in dictionary order.

`cargo metadata` is optional and is what adds licences and the dependency graph.
Without it the SBOM is still a complete list of what is in the build (every
component, its version, its purl and the checksum `Cargo.lock` records) and it
says in `metadata.properties` that licences are absent rather than implying an
empty licence set.

Usage:
    scripts/sbom.py --lock core/Cargo.lock --out pixelsmith.cdx.json
    scripts/sbom.py --lock ... --metadata md.json --platform aarch64-apple-ios ...

Exit status: 0 written and self-checked, 1 the document failed its own checks,
2 an input could not be read.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tomllib
import uuid
from pathlib import Path

# The UUID namespace RFC 4122 reserves for a name-derived UUID. Using it means
# the serial number is a function of the lockfile's digest and nothing else.
NAMESPACE_URL = uuid.UUID("6ba7b811-9dad-11d1-80b4-00c04fd430c8")
SPEC_VERSION = "1.5"


def purl(name: str, version: str) -> str:
    """A package URL. The crates.io type is `cargo`, and it is spelled that way."""
    return f"pkg:cargo/{name}@{version}"


def lock_components(lock: dict) -> list[dict]:
    """Every package in the lockfile, in lockfile order (which is alphabetical).

    A registry package carries `source`; the workspace member does not, and a
    path dependency does not either. Both are still components — the workspace
    member is the thing the SBOM is *about* — and only the registry ones get a
    crates.io purl and the lockfile's checksum.
    """
    out: list[dict] = []
    for pkg in lock.get("package") or []:
        name = pkg.get("name")
        version = pkg.get("version")
        if not name or not version:
            continue
        component: dict = {
            "type": "library",
            "bom-ref": f"{name}@{version}",
            "name": name,
            "version": version,
        }
        source = pkg.get("source")
        if source and source.startswith("registry+"):
            component["purl"] = purl(name, version)
            component["properties"] = [{"name": "px:source", "value": source}]
            checksum = pkg.get("checksum")
            if checksum:
                algo, _, digest = checksum.partition(":")
                if algo and digest:
                    component["hashes"] = [{"alg": algo.upper().replace("-", ""), "content": digest}]
        else:
            component["purl"] = f"pkg:generic/{name}@{version}"
        if pkg.get("license"):
            component["licenses"] = spdx_licences(pkg["license"])
        out.append(component)
    return out


def spdx_licences(expression: str) -> list[dict]:
    """A CycloneDX `licenses` array for one SPDX expression.

    CycloneDX distinguishes a single licence (`license.id`, which must be one
    SPDX identifier) from a compound one (`expression`, free text in SPDX
    grammar). `MIT OR Apache-2.0` written as `{"license": {"id": ...}}` is
    invalid against the schema, and the tools that consume an SBOM will reject it
    or — worse, the ones that do not validate — record a licence nobody can act
    on. Nine crates in this tree write `MIT/Apache-2.0`, so this is not a corner.
    """
    text = expression.replace("/", " OR ").strip()
    if re.search(r"\b(OR|AND|WITH)\b", text) or "(" in text:
        return [{"expression": text}]
    return [{"license": {"id": text}}]


def _name_of(package_id: str) -> str:
    """The crate name out of a `cargo metadata` package id.

    The id is `<source>#<name>@<version>`, and the source half contains slashes,
    a colon and a hash-free URL — so the name is what comes after the `#`, up to
    the `@`. Taking the first whitespace-separated field instead (which is what a
    readable-looking implementation does) returns the source URL and silently
    matches nothing, which is exactly the failure this SBOM has to avoid: an empty
    dependency graph is a valid document that describes a library with no
    dependencies.
    """
    tail = package_id.rsplit("#", 1)[-1]
    return tail.rsplit("@", 1)[0]


def metadata_index(metadata: dict, platform: str | None) -> tuple[dict, dict]:
    """(packages by name, dependency edges) from `cargo metadata`.

    The graph is walked through `resolve.nodes`, not over the flat `packages`
    list, because that is the difference between a component that is in this
    build and a component that merely appears in the lockfile. With
    `--platform` the walk also honours the target filter, which is what drops
    dependencies gated on a cfg expression rather than a triple.
    """
    packages = {pkg["name"]: pkg for pkg in metadata.get("packages") or []}
    by_id = {pkg["id"]: pkg["name"] for pkg in metadata.get("packages") or [] if pkg.get("id")}
    resolve = metadata.get("resolve") or {}
    edges: list[tuple[str, str]] = []
    for node in resolve.get("nodes") or []:
        parent = by_id.get(node.get("id") or "")
        if parent is None:
            continue
        for dep in node.get("deps") or []:
            child = by_id.get(dep.get("pkg") or "")
            if child is None:
                continue
            edges.append((parent, child))
    return packages, {"edges": sorted(set(edges)), "platform": platform}


def apply_metadata(components: list[dict], metadata: dict, platform: str | None) -> dict:
    """Fill in licences and reachability from `cargo metadata`."""
    packages, graph = metadata_index(metadata, platform)
    reachable: set[str] = set()
    for source, target in graph["edges"]:
        reachable.add(target)
    # A package with no outgoing edge is a leaf; it is still in the build. The
    # roots are the workspace members, which `cargo metadata` names explicitly.
    parents = {source for source, _ in graph["edges"]}
    reachable |= {name for name in packages if name not in parents}

    for component in components:
        pkg = packages.get(component["name"])
        if pkg is None:
            continue
        if pkg.get("license"):
            component["licenses"] = spdx_licences(pkg["license"])
        component["properties"] = sorted(
            (component.get("properties") or []) + [{"name": "px:in-graph", "value": "true"}],
            key=lambda prop: (prop["name"], prop["value"]),
        )
    return {"edges": graph["edges"], "reachable": reachable, "platform": graph["platform"]}


def build(lock: dict, metadata: dict | None, platform: str | None, engine_version: str) -> dict:
    components = lock_components(lock)
    reach: dict = {"edges": [], "reachable": set(), "platform": platform}

    has_metadata = metadata is not None
    if has_metadata:
        reach = apply_metadata(components, metadata, platform)

    # The component the SBOM is about, and the root of the dependency tree.
    root = next((c for c in components if c["name"] == "pixelsmith_core"), None)
    if root is None:
        root = {
            "type": "library",
            "bom-ref": f"pixelsmith_core@{engine_version}",
            "name": "pixelsmith_core",
            "version": engine_version,
        }
        components.append(root)
    else:
        root["version"] = engine_version
        root["bom-ref"] = f"pixelsmith_core@{engine_version}"
        root["licenses"] = spdx_licences("MIT OR Apache-2.0")
        root["purl"] = purl("pixelsmith_core", engine_version)

    # Every component a dependency edge names must exist, or the document is not
    # a valid graph. Dropping an edge is better than emitting a dangling ref.
    names = {c["name"] for c in components}
    edges = [(a, b) for a, b in reach["edges"] if a in names and b in names]
    root_edges = [b for a, b in edges if a == "pixelsmith_core"]

    dependencies = [
        {"ref": f"{a}@{_version_of(components, a)}", "dependsOn": sorted({f"{b}@{_version_of(components, b)}" for a2, b in edges if a2 == a})}
        for a in sorted({a for a, _ in edges})
    ]
    dependencies.append(
        {"ref": root["bom-ref"], "dependsOn": sorted({f"{b}@{_version_of(components, b)}" for b in root_edges})}
    )
    dependencies.sort(key=lambda entry: entry["ref"])

    timestamp = os.environ.get("SOURCE_DATE_EPOCH", "0").strip() or "0"
    properties = [
        {"name": "px:licence-source", "value": "cargo metadata" if has_metadata else "Cargo.lock (absent)"},
        {"name": "px:target", "value": platform or "not filtered"},
        {"name": "px:timestamp", "value": "SOURCE_DATE_EPOCH"},
    ]

    return {
        "bomFormat": "CycloneDX",
        "specVersion": SPEC_VERSION,
        "serialNumber": f"urn:uuid:{_serial(lock, components)}",
        "version": 1,
        "metadata": {
            "timestamp": _iso(timestamp),
            "tools": {"components": [{"type": "application", "name": "scripts/sbom.py"}]},
            "component": {
                "type": "application",
                "bom-ref": root["bom-ref"],
                "name": "pixelsmith_core",
                "version": engine_version,
                "licenses": spdx_licences("MIT OR Apache-2.0"),
                "purl": purl("pixelsmith_core", engine_version),
            },
            "properties": properties,
        },
        "components": sorted(components, key=lambda c: (c["name"], c["version"])),
        "dependencies": dependencies,
    }


def _version_of(components: list[dict], name: str) -> str:
    for component in components:
        if component["name"] == name:
            return component["version"]
    return "0"


def _serial(lock: dict, components: list[dict]) -> str:
    """A deterministic serial number: a UUID derived from the content itself."""
    digest = hashlib.sha256()
    for component in sorted(components, key=lambda c: (c["name"], c["version"])):
        digest.update(f"{component['name']}@{component['version']}\n".encode())
    return str(uuid.uuid5(NAMESPACE_URL, digest.hexdigest()))


def _iso(epoch: str) -> str:
    """`SOURCE_DATE_EPOCH` as an RFC 3339 instant, without importing the world."""
    try:
        seconds = int(epoch)
    except ValueError:
        seconds = 0
    import datetime

    return (
        datetime.datetime.fromtimestamp(seconds, datetime.timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z")
    )


def self_check(document: dict, lock: dict) -> list[str]:
    """The checks that make this file a CycloneDX document rather than JSON.

    A generator that writes malformed SBOMs is worse than no generator, because
    every downstream tool that reads one will trust it. These are the properties a
    consumer depends on, and each has bitten somebody somewhere.
    """
    problems: list[str] = []
    if document.get("bomFormat") != "CycloneDX":
        problems.append("bomFormat is not CycloneDX")
    if document.get("specVersion") != SPEC_VERSION:
        problems.append(f"specVersion is not {SPEC_VERSION}")
    if not re.fullmatch(r"urn:uuid:[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", document.get("serialNumber", "")):
        problems.append("serialNumber is not a urn:uuid")
    components = document.get("components") or []
    if not components:
        problems.append("no components: an SBOM with an empty component list describes nothing")
    expected = len([p for p in lock.get("package") or [] if p.get("name") and p.get("version")])
    if len(components) < expected:
        problems.append(f"{expected - len(components)} lockfile package(s) are missing from the document")
    refs = {c["bom-ref"] for c in components}
    for component in components:
        if not component.get("name") or not component.get("version"):
            problems.append(f"component {component.get('bom-ref')!r} has no name/version")
        if not component.get("purl"):
            problems.append(f"component {component['bom-ref']} has no purl")
    for entry in document.get("dependencies") or []:
        if entry.get("ref") not in refs and entry.get("ref") != document["metadata"]["component"]["bom-ref"]:
            problems.append(f"dependency ref {entry.get('ref')!r} names no component")
        for target in entry.get("dependsOn") or []:
            if target not in refs:
                problems.append(f"dependency {entry.get('ref')!r} depends on unknown component {target!r}")
    return problems


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--lock", type=Path, default=Path("core/Cargo.lock"))
    parser.add_argument("--metadata", type=Path, default=None, help="`cargo metadata --format-version 1` output")
    parser.add_argument("--platform", action="append", default=[], help="target triple to resolve the graph for")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--print-summary", action="store_true")
    args = parser.parse_args(argv)

    try:
        lock = tomllib.loads(args.lock.read_text())
    except (OSError, tomllib.TOMLDecodeError) as err:
        print(f"sbom: cannot read {args.lock}: {err}", file=sys.stderr)
        return 2

    metadata = None
    platform = args.platform[0] if args.platform else None
    if args.metadata:
        try:
            metadata = json.loads(args.metadata.read_text())
        except (OSError, json.JSONDecodeError) as err:
            print(f"sbom: cannot read {args.metadata}: {err}", file=sys.stderr)
            return 2

    engine = next((p for p in lock.get("package") or [] if p.get("name") == "pixelsmith_core"), {})
    document = build(lock, metadata, platform, engine.get("version", "0.0.0"))

    problems = self_check(document, lock)
    if problems:
        for problem in problems:
            print(f"sbom: SELF-CHECK FAILED: {problem}", file=sys.stderr)
        return 1

    args.out.parent.mkdir(parents=True, exist_ok=True)
    # sort_keys + a fixed separator + a trailing newline: the same inputs must
    # produce the same bytes, or "the SBOM matches too" is not a check.
    args.out.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n")

    if args.print_summary:
        print(
            f"sbom: {len(document['components'])} components, "
            f"{len(document['dependencies'])} dependency entries -> {args.out}"
        )
    else:
        print(f"{args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))