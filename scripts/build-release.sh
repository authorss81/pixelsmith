#!/usr/bin/env bash
# =============================================================================
# build-release.sh — build the shipped library reproducibly, and say its hash.
#
# The engine's release profile already sets the two things that make a Rust
# binary deterministic at all: `codegen-units = 1` and `lto = "thin"`. This
# script adds the two it does not, and neither is optional:
#
#   --remap-path-prefix   rustc bakes the absolute path of every source file
#                         into the binary, in panic locations and in debug
#                         info. Two checkouts in different directories therefore
#                         produce different bytes from identical sources. On a CI
#                         runner the checkout path happens to be
#                         /home/runner/work/<repo>/<repo> every time, which is
#                         why a naive double-build often "reproduces" while
#                         reproducing nothing a stranger could.
#
#   SOURCE_DATE_EPOCH     the conventional build timestamp. Nothing in this tree
#                         calls __DATE__ or __TIME__ — assertable, and asserted
#                         by scripts/repro-check.sh — so today it changes no
#                         output. It is pinned anyway, because a build that is
#                         reproducible today because nothing embeds a timestamp
#                         is one release away from being unreproducible.
#
# CARGO_INCREMENTAL=0 because incremental compilation records absolute paths in
# its own metadata, and `cargo build --release` should not use it regardless.
#
# Usage:
#   scripts/build-release.sh                     host target, prints the sha256
#   scripts/build-release.sh --target <triple>   another target
#   scripts/build-release.sh --no-remap          build the way CI does today
#   scripts/build-release.sh --target-dir DIR    build somewhere else
#   scripts/build-release.sh --record FILE       append/refresh the file's row
#
# Exit status: 0 built, 1 the build failed, 2 bad arguments.
# =============================================================================

set -uo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# The tree being built. Normally this repository, but `scripts/repro-check.sh`
# stages a copy of the sources at a different absolute path and points this at
# it, which is the only way to find out whether a hash depends on where the
# checkout happens to be.
SOURCE_ROOT="${ROOT}"
TARGET=""
REMAP=1
TARGET_DIR=""
RECORD=""
QUIET=0

die() { printf 'build-release.sh: %s\n' "$*" >&2; exit 2; }
say() { [ "${QUIET}" = "1" ] || printf '%s\n' "$*"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --target)      TARGET="$2"; shift 2 ;;
    --target=*)    TARGET="${1#*=}"; shift ;;
    --source-root) SOURCE_ROOT="$2"; shift 2 ;;
    --source-root=*) SOURCE_ROOT="${1#*=}"; shift ;;
    --no-remap)    REMAP=0; shift ;;
    --remap)       REMAP=1; shift ;;
    --target-dir)  TARGET_DIR="$2"; shift 2 ;;
    --target-dir=*) TARGET_DIR="${1#*=}"; shift ;;
    --record)      RECORD="$2"; shift 2 ;;
    --record=*)    RECORD="${1#*=}"; shift ;;
    --quiet)       QUIET=1; shift ;;
    -h|--help)     sed -n '2,32p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *)             die "unknown argument: $1" ;;
  esac
done

[ -f "${SOURCE_ROOT}/core/Cargo.toml" ] || die "no core/Cargo.toml under ${SOURCE_ROOT}"

if [ -z "${TARGET}" ]; then
  TARGET=$(rustc -vV | awk '/^host: /{print $2}')
  [ -n "${TARGET}" ] || die "could not determine the host target"
fi

# `--target` is always passed explicitly, for two reasons. A cross build needs it
# anyway. And a host build without it puts the artefact in target/release/ where
# a plain `cargo build` also puts it, so the two silently overwrite each other
# and "the release binary" stops naming one thing.
CARGO_ARGS=(--manifest-path "${SOURCE_ROOT}/core/Cargo.toml" --release --lib --locked --target "${TARGET}")
if [ -n "${TARGET_DIR}" ]; then
  CARGO_ARGS+=(--target-dir "${TARGET_DIR}")
fi

# --remap-path-prefix is appended to whatever RUSTFLAGS the caller already set
# rather than replacing it: a caller who set RUSTFLAGS to enable something must
# not silently lose it, and `RUSTFLAGS=${RUSTFLAGS:-}` is the difference between
# an additive flag and a destructive one.
if [ "${REMAP}" = "1" ]; then
  # The sysroot path also lands in the binary (it is where std, and the paths
  # recorded inside the precompiled std, come from), so a runner with a
  # differently located toolchain cannot produce the same bytes either way. Both
  # prefixes are remapped to something short and meaningless.
  SYSROOT=$(rustc --print sysroot 2>/dev/null || true)
  RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=${SOURCE_ROOT}=. ${SYSROOT:+--remap-path-prefix=${SYSROOT}=.}"
fi
export RUSTFLAGS

# Fixed unless the caller sets one. 0 is not a date; it is "the epoch", and it is
# the value that makes the answer independent of when the build ran.
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-0}"
export CARGO_INCREMENTAL=0
# The registry cache is part of the input to a build. Say so, because a
# different cache contents is one of the ways two honest builds differ and the
# error message says nothing useful.
export CARGO_NET_OFFLINE=true

say "build-release.sh: target   ${TARGET}"
say "build-release.sh: remap    $([ "${REMAP}" = "1" ] && echo on || echo off)"
say "build-release.sh: epoch    ${SOURCE_DATE_EPOCH}"
say "build-release.sh: rustc    $(rustc --version)"

if ! cargo build "${CARGO_ARGS[@]}"; then
  die "cargo build --release --lib --target ${TARGET} failed"
fi

DIR="${TARGET_DIR:-${SOURCE_ROOT}/core/target}/${TARGET}/release"
# The crate publishes three artefacts from one build (crate-type = lib, cdylib,
# staticlib). The cdylib is what a phone loads and the one whose dynamic imports
# the no-network claim is checked against, so it is the one hashed here; the
# staticlib is recorded alongside it because the desktop CMake build links that
# one, and a hash table with two entries per row is still checkable.
ARTEFACTS=()
for name in libpixelsmith_core.so libpixelsmith_core.dylib libpixelsmith_core.dll libpixelsmith_core.a; do
  [ -f "${DIR}/${name}" ] && ARTEFACTS+=("${DIR}/${name}")
done
[ ${#ARTEFACTS[@]} -gt 0 ] || die "no artefact produced in ${DIR}"

SHA=""
for artefact in "${ARTEFACTS[@]}"; do
  hash=$(sha256sum "${artefact}" | awk '{print $1}')
  size=$(stat -c%s "${artefact}" 2>/dev/null || stat -f%z "${artefact}")
  printf '%s  %s  %s\n' "${hash}" "$(basename "${artefact}")" "${size}"
  [ -z "${SHA}" ] && SHA="${hash}"
done

# The reproducibility claim is about the dynamic library, so name which one the
# hash above belongs to rather than leaving a reader to guess.
printf 'target=%s sha256=%s rustc=%s remap=%s epoch=%s\n' \
  "${TARGET}" "${SHA}" "$(rustc --version)" "$([ "${REMAP}" = "1" ] && echo on || echo off)" "${SOURCE_DATE_EPOCH}"

if [ -n "${RECORD}" ]; then
  # One row per target, rewritten in place. A hash table that only ever appends
  # accumulates two rows for every target the first time a runner version moves,
  # and a reader cannot tell which is current.
  tmp="${RECORD}.tmp.$$"
  if [ -f "${RECORD}" ]; then
    grep -v "^${TARGET}[[:space:]]" "${RECORD}" > "${tmp}" || true
  else
    : > "${tmp}"
  fi
  printf '%-30s %s  %s  remap=%s  epoch=%s  %s\n' \
    "${TARGET}" "${SHA}" "$(basename "${ARTEFACTS[0]}")" \
    "$( [ "${REMAP}" = "1" ] && echo on || echo off )" "${SOURCE_DATE_EPOCH}" "$(rustc --version)" >> "${tmp}"
  mv "${tmp}" "${RECORD}"
  say "build-release.sh: recorded ${TARGET} in ${RECORD}"
fi

exit 0