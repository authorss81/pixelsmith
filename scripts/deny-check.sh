#!/usr/bin/env bash
# =============================================================================
# deny-check.sh — run the supply-chain policy, and prove it can fail.
#
# cargo-deny is the policy engine. `.github/workflows/supply-chain.yml` runs it on
# every push, on every pull request and weekly, and that job is the enforcement.
# This script is the same check made runnable by hand, plus two things the CI
# job does not do:
#
#   * an offline subset, so a contributor without cargo-deny installed can still
#     get a verdict on licences, banned crates, duplicate versions and sources
#     rather than a shrug. It reads deny.toml; it is not a second copy of it.
#
#   * --negative-control, which injects known violations and asserts each one is
#     caught. A gate that has never been observed to fail is not known to work,
#     and "cargo-deny is enabled" is not the same claim as "cargo-deny fails on a
#     GPL dependency".
#
# Usage:
#   scripts/deny-check.sh                      cargo-deny if present, then the subset
#   scripts/deny-check.sh --policy path/to     check a different policy file
#   scripts/deny-check.sh --negative-control   prove the checks are live
#
# Exit status:
#   0  every check that ran passed. Read the banner: when cargo-deny is not
#      installed this is a subset verdict and says so, and the advisories half
#      of the policy is then unchecked HERE. It is not unchecked everywhere —
#      the `deny` job in .github/workflows/supply-chain.yml runs the real engine
#      on every push, every pull request and weekly, and that job is the gate.
#   1  a finding, or a negative control that did not fire
#   2  the policy, the manifest or the dependency graph could not be read, which
#      is not "the tree is clean" and deliberately does not share an exit code
#      with it
# =============================================================================

set -uo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
POLICY="${ROOT}/deny.toml"
MANIFEST="${ROOT}/core/Cargo.toml"
NEGATIVE_CONTROL=0
CARGO_DENY_RAN=0
CARGO_DENY_RC=0

die() { printf 'deny-check.sh: %s\n' "$*" >&2; exit 2; }
say() { printf '%s\n' "$*"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --policy)            POLICY="$2"; shift 2 ;;
    --policy=*)          POLICY="${1#*=}"; shift ;;
    --negative-control)  NEGATIVE_CONTROL=1; shift ;;
    -h|--help)           sed -n '2,30p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *)                   die "unknown argument: $1" ;;
  esac
done

[ -f "${POLICY}" ]   || die "no policy at ${POLICY}"
[ -f "${MANIFEST}" ] || die "no manifest at ${MANIFEST}"

WORK=$(mktemp -d)
trap 'rm -rf "${WORK}"' EXIT

# -----------------------------------------------------------------------------
say "=== dependency metadata ==="
# --offline because this half of the check must work on a machine with no
# network, and --locked because a policy decision taken against a dependency
# graph other than the one that will be built is a decision about nothing. If the
# lockfile is stale, cargo says so and this exits 2 rather than passing.
if ! cargo metadata --format-version 1 --offline --locked --all-features \
      --manifest-path "${MANIFEST}" > "${WORK}/metadata.json" 2>"${WORK}/metadata.err"; then
  sed 's/^/  /' "${WORK}/metadata.err" >&2
  die "cargo metadata failed; the policy cannot be evaluated against a graph that did not resolve"
fi
CRATES=$(python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1]))["packages"]))' "${WORK}/metadata.json")
say "  ${CRATES} packages resolved from core/Cargo.lock (offline, locked)"

# -----------------------------------------------------------------------------
say ""
say "=== cargo deny ==="
if command -v cargo-deny >/dev/null 2>&1; then
  CARGO_DENY_RAN=1
  # `--manifest-path` is not optional. Run from the repository root without it
  # and cargo-deny reports "the directory ... doesn't contain a Cargo.toml file",
  # which looks like a policy failure and is not one.
  if ( cd "${ROOT}" && cargo deny --manifest-path core/Cargo.toml check ); then
    say "  ok: advisories, licences, bans and sources"
  else
    CARGO_DENY_RC=$?
    say "  FAIL: cargo deny check (exit ${CARGO_DENY_RC})"
  fi
else
  say "  skip: cargo-deny is not installed."
  say "        cargo install cargo-deny --locked"
  say "        CI runs the real thing in .github/workflows/supply-chain.yml."
fi

# -----------------------------------------------------------------------------
say ""
say "=== offline subset (reads ${POLICY#"${ROOT}/"}) ==="
SUBSET_RC=0
python3 "${ROOT}/scripts/lockfile-policy.py" \
  --policy "${POLICY}" \
  --metadata "${WORK}/metadata.json" \
  --manifest "${MANIFEST}" || SUBSET_RC=$?
case ${SUBSET_RC} in
  0) say "  ok: licences, banned crates, duplicate versions, sources, wildcards" ;;
  1) say "  FAIL: the offline subset found a violation (see the PXDENY lines above)" ;;
  *) say "  FAIL: the offline subset could not read its inputs" ;;
esac

say ""
say "  NOT COVERED by the offline subset, and not checkable without a network:"
say "    advisories  [advisories] yanked = \"deny\" and unmaintained = \"all\""
say "    confidence  [licenses] confidence-threshold"
say "  Both need the RustSec advisory database. cargo deny check advisories is"
say "  the only thing that evaluates them, and CI runs it on every push."

# -----------------------------------------------------------------------------
CONTROL_RC=0
if [ "${NEGATIVE_CONTROL}" = "1" ]; then
  say ""
  say "=== negative control ==="
  # Injects one violation at a time into copies of the REAL policy, manifest and
  # metadata, and asserts each is caught. If any of these pass when they should
  # fail, the corresponding check above is decorative.
  python3 "${ROOT}/scripts/deny-negative-control.py" \
    --policy "${POLICY}" \
    --metadata "${WORK}/metadata.json" \
    --manifest "${MANIFEST}" || CONTROL_RC=$?
fi

# -----------------------------------------------------------------------------
say ""
if [ "${CARGO_DENY_RC}" -ne 0 ] || [ "${SUBSET_RC}" -ne 0 ] || [ "${CONTROL_RC}" -ne 0 ]; then
  say "RESULT: FAIL — see the findings above"
  exit 1
fi
if [ "${CARGO_DENY_RAN}" = "0" ]; then
  # A partial verdict must not read as a full one. The exit status is still 0,
  # because the checks that did run did pass and the checks that did not run are
  # not a finding -- but the banner above is unmissable, and `PX_DENY=1` in
  # scripts/verify.sh reports this case as a note rather than as a clean "ok".
  say "RESULT: PASS (offline subset only — advisories and licence confidence UNCHECKED here)"
  exit 0
fi
say "RESULT: PASS"
exit 0