#!/usr/bin/env bash
# =============================================================================
# check-version.sh — one version, in every place it appears.
#
# The engine reads its version from Cargo.toml, the app reads it from
# pubspec.yaml, and a release is named by its tag. Three places to update is
# three places to forget, and the failure is silent in the worst way possible:
# the engine reports 0.1.0 in `px_version()`, the Play Store listing says
# 0.2.0, and a bug report quotes whichever one the reporter happened to see.
#
# So the agreement is checked, not trusted. This script is in `verify.sh`, so a
# mismatch is a red gate on the next push, and it is in the release workflow, so
# a tag that disagrees with the manifests cannot produce a release.
#
# The version scheme is SemVer and was NOT chosen here: both manifests already
# said 0.1.0 when this phase started (core/Cargo.toml from phase-01,
# app/pubspec.yaml from the phase-05 scaffolding). This script reads the two, the
# CHANGELOG and the tag, and refuses to run if they disagree — which is a
# different thing from inventing a number. The decision is recorded in
# docs/ARCHITECTURE.md.
#
# Usage:
#   bash scripts/check-version.sh                 check the tree at the repo root
#   bash scripts/check-version.sh --root DIR      check a copy (used by --self-test)
#   bash scripts/check-version.sh --tag v0.1.0    also require a matching tag
#   bash scripts/check-version.sh --self-test     prove the checks can fail
#
# Exit 0 = every source agrees. 1 = they do not. 2 = bad arguments.
# =============================================================================

set -uo pipefail

ROOT=""
TAG=""
SELF_TEST=0
QUIET=0

die() { printf 'check-version.sh: %s\n' "$*" >&2; exit 2; }
say() { printf '%s\n' "$*"; }
note() { [ "${QUIET}" = "1" ] || printf '%s\n' "$*"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --root)      ROOT="$2"; shift 2 ;;
    --root=*)    ROOT="${1#*=}"; shift ;;
    --tag)       TAG="$2"; shift 2 ;;
    --tag=*)     TAG="${1#*=}"; shift ;;
    --self-test) SELF_TEST=1; shift ;;
    --quiet)     QUIET=1; shift ;;
    -h|--help)   sed -n '2,30p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *)           die "unknown argument: $1" ;;
  esac
done

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

# -----------------------------------------------------------------------------
# Extractors. Each prints one value on stdout.
#
# Deliberately sed/awk rather than a Python script: this has to run on a bare
# runner, and a value read by a regular expression is a value this script can
# prove wrong — see --self-test.

engine_version() {
  awk '
    /^\[/ { in_pkg = ($0 ~ /^\[package\][[:space:]]*($|#)/) ; next }
    in_pkg && /^version[[:space:]]*=/ {
      gsub(/^version[[:space:]]*=[[:space:]]*"/, "")
      gsub(/".*$/, "")
      print; exit
    }
  ' "$1/core/Cargo.toml" 2>/dev/null
}

# app/pubspec.yaml's `version: 1.2.3+4`. The build number after `+` is the
# Android versionCode and is deliberately NOT part of the version: it is a
# monotonically increasing integer that changes on every upload, so comparing it
# against a SemVer triple would fail on every single upload.
app_version() {
  sed -n 's/^version:[[:space:]]*\([0-9][0-9.]*\).*$/\1/p' "$1/app/pubspec.yaml" 2>/dev/null | head -n 1
}

app_build_number() {
  sed -n 's/^version:[[:space:]]*[0-9][0-9.]*+\([0-9][0-9]*\).*$/\1/p' "$1/app/pubspec.yaml" 2>/dev/null | head -n 1
}

# The newest CHANGELOG heading. `## [0.1.0]` and `## 0.1.0` are both accepted.
# `## [Unreleased]` is skipped, because a changelog with an Unreleased section is
# normal and one whose newest *version* section is not the shipped version is a
# release nobody wrote down.
changelog_version() {
  sed -n 's/^##[[:space:]]*\[*\([0-9][0-9.]*\)\]*.*$/\1/p' "$1/CHANGELOG.md" 2>/dev/null | head -n 1
}

# -----------------------------------------------------------------------------
# The real check. Run as a function rather than at the bottom of the file so the
# self-test can call it against a deliberately broken tree; a self-test that can
# only test the happy path tests nothing.

run_check() {
  local root="$1" tag="$2"
  local failures=0
  local engine="" app="" build="" changelog=""

  engine=$(engine_version "${root}")
  app=$(app_version "${root}")
  build=$(app_build_number "${root}")
  changelog=$(changelog_version "${root}")

  if [ -z "${engine}" ]; then
    say "  FAIL: core/Cargo.toml carries no [package] version — nothing to compare against"
    failures=$((failures + 1))
    say ""
    say "check-version.sh: FAIL (${failures})"
    return 1
  fi

  # `engine_version` is read first because it is the version the *binary*
  # reports: px_version() returns CARGO_PKG_VERSION, so the crate that is
  # actually compiled is the ground truth for what the engine says it is.
  # Everything else is compared against it, so a disagreement always names the
  # engine as the side that is right.
  if [ "${app}" = "${engine}" ] && [ -n "${app}" ]; then
    note "  ok:   app/pubspec.yaml: ${app}"
  elif [ -z "${app}" ]; then
    note "  FAIL: app/pubspec.yaml carries no version"
    failures=$((failures + 1))
  else
    note "  FAIL: app/pubspec.yaml: ${app} != core/Cargo.toml's ${engine}"
    failures=$((failures + 1))
  fi

  if [ "${changelog}" = "${engine}" ] && [ -n "${changelog}" ]; then
    note "  ok:   CHANGELOG.md newest heading: ${changelog}"
  elif [ -z "${changelog}" ]; then
    note "  FAIL: CHANGELOG.md has no '## <version>' heading — the release is unwritten"
    failures=$((failures + 1))
  else
    note "  FAIL: CHANGELOG.md newest heading: ${changelog} != core/Cargo.toml's ${engine}"
    failures=$((failures + 1))
  fi

  if [ -n "${tag}" ]; then
    if [ "${tag}" = "v${engine}" ]; then
      note "  ok:   release tag: ${tag}"
    else
      note "  FAIL: release tag is ${tag}; the version is ${engine}, so the tag is v${engine}"
      failures=$((failures + 1))
    fi
  fi

  note "  version: ${engine}"
  [ -n "${build}" ] && note "  build:   ${build} (Android versionCode, Windows build suffix)"
  note ""
  if [ "${failures}" -eq 0 ]; then
    note "check-version.sh: PASS"
    return 0
  fi
  note "check-version.sh: FAIL (${failures})"
  return 1
}

# =============================================================================
# --self-test
#
# A gate that has never been observed to fail is not known to work. This
# repository has been bitten by that repeatedly — phase-01's `check()`, phase-08's
# `| head -n 40`, phase-10's `--test`, phase-15's target-triple regex and its
# `spdx_licenses` typo — so every check a phase adds is run against a *passing*
# input and against deliberately broken ones before it is believed.
#
# Each case copies the real manifests into a throwaway tree and breaks exactly
# one thing, so a case that fails for a missing file instead of the reason named
# would itself show up as a failure here.
# =============================================================================

# Global, because the EXIT trap fires after every local in self_test has gone
# out of scope — which is a `set -u` "unbound variable" at the very end of a
# successful run, and reads like the self-test failing when it passed.
SELFTEST_DIR=""

cleanup() { [ -n "${SELFTEST_DIR}" ] && rm -rf "${SELFTEST_DIR}"; }

self_test() {
  local cases=0 fired=0 rc

  SELFTEST_DIR=$(mktemp -d "${TMPDIR:-/tmp}/px-version-selftest.XXXXXX") || die "mktemp failed"
  trap cleanup EXIT

  # One pristine copy that every case starts from.
  local base="${SELFTEST_DIR}/base"
  mkdir -p "${base}/core" "${base}/app"
  cp "${REPO}/core/Cargo.toml" "${base}/core/" || die "cannot stage core/Cargo.toml"
  cp "${REPO}/app/pubspec.yaml" "${base}/app/" 2>/dev/null
  cp "${REPO}/CHANGELOG.md" "${base}/" 2>/dev/null

  # case NAME EXPECTED_RC DESCRIPTION, then the mutation commands on stdin.
  cases_run() {
    local name="$1" expect="$2" desc="$3"
    local dir="${SELFTEST_DIR}/case-${name}"
    cases=$((cases + 1))
    rm -rf "${dir}"
    cp -r "${base}" "${dir}"
    ( cd "${dir}" && eval "${MUTATE}" ) >/dev/null 2>&1
    ( "${BASH_SOURCE[0]}" --root "${dir}" --quiet ${TAGARG} ) >/dev/null 2>&1
    rc=$?
    if [ "${rc}" = "${expect}" ]; then
      say "  ok:   ${name} -> exit ${rc} (${desc})"
      fired=$((fired + 1))
    else
      say "  FAIL: ${name} -> exit ${rc}, expected ${expect} (${desc})"
    fi
  }

  say "check-version.sh --self-test"
  say ""

  TAGARG=""
  MUTATE="true"
  cases_run "passing-tree" 0 "nothing broken"

  TAGARG=""
  MUTATE="sed -i 's/^version:.*/version: 9.9.9+1/' app/pubspec.yaml"
  cases_run "app-ahead" 1 "pubspec on a different version"

  TAGARG=""
  MUTATE="sed -i 's/^version = \"0\.1\.0\"/version = \"0.2.0\"/' core/Cargo.toml"
  cases_run "engine-bumped-only" 1 "engine bumped, nothing else"

  TAGARG=""
  MUTATE="sed -i '/^## /{s/^## .*/## [99.0.0] - a release that did not happen/;}' CHANGELOG.md"
  cases_run "changelog-ahead" 1 "changelog names another release"

  TAGARG=""
  MUTATE="rm -f CHANGELOG.md"
  cases_run "changelog-absent" 1 "no changelog at all"

  # A `version =` that is not in the [package] table belongs to something else —
  # a path dependency, a metadata table — and must not be mistaken for the
  # engine's version. Cargo.toml has several of them, and an extractor that
  # grepped the whole file would read whichever came last.
  TAGARG=""
  MUTATE="sed -i 's/^\[dependencies\]/[package.metadata]\n\n# the real [package] version is above this line\nversion = \"3.3.3\"\n\n[dependencies]/' core/Cargo.toml"
  cases_run "stray-version-ignored" 0 "a version= under another table is not the engine's"

  # ...and a manifest with no [package] version at all cannot be compared with
  # anything, so it is a failure rather than a pass.
  TAGARG=""
  MUTATE="sed -i '/^version = \"0\.1\.0\"$/d' core/Cargo.toml"
  cases_run "engine-version-absent" 1 "core/Cargo.toml has no version to compare against"

  # The build number is not part of the version. 0.1.0+1 and 0.1.0+99 are the
  # same release; a script that compared the whole string would fail on every
  # upload after the first.
  TAGARG=""
  MUTATE="sed -i 's/^version:.*/version: 0.1.0+99/' app/pubspec.yaml"
  cases_run "build-number-ignored" 0 "pubspec build number bumped, version the same"

  TAGARG="--tag v0.1.0"
  MUTATE="true"
  cases_run "tag-matches" 0 "tag v0.1.0 against version 0.1.0"

  TAGARG="--tag v0.1.1"
  MUTATE="true"
  cases_run "tag-behind" 1 "tag v0.1.1 against version 0.1.0"

  TAGARG="--tag 0.1.0"
  MUTATE="true"
  cases_run "tag-missing-v" 1 "tag without the v prefix"

  say ""
  say "  ${fired}/${cases} cases behaved as specified"
  if [ "${fired}" -ne "${cases}" ]; then
    say "check-version.sh --self-test: FAIL"
    return 1
  fi
  say "check-version.sh --self-test: PASS"
  return 0
}

if [ "${SELF_TEST}" = "1" ]; then
  self_test
  exit $?
fi

[ -n "${ROOT}" ] || ROOT="${REPO}"
[ -d "${ROOT}" ] || die "no such directory: ${ROOT}"

note "check-version.sh: root ${ROOT}"
note ""
run_check "${ROOT}" "${TAG}"
exit $?