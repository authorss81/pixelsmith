#!/usr/bin/env bash
# =============================================================================
# repro-check.sh — is the release build reproducible, and if not, why not?
#
# "Build it twice and compare hashes" is the test everybody writes and almost
# nobody designs, because the obvious form of it cannot fail: build twice in the
# same directory with the same toolchain and any deterministic-enough compiler
# agrees with itself, whether or not the binary would match one built anywhere
# else. The two things that break byte-identity in practice are
#
#   * the build path, which rustc bakes into panic locations, and
#   * the toolchain version, which changes code generation,
#
# and neither shows up in a same-directory double build.
#
# So this runs four builds, not two, and compares three pairs:
#
#   A1  this checkout, this path                          \
#   A2  this checkout, this path, fresh target dir         |  determinism at a
#                                                         |  fixed path
#   B1  the same sources copied to another path, no remap  |  path sensitivity
#   B2  the same sources copied to another path, remap on  /  does remapping fix it
#
# Each pair is compared by sha256, and a mismatch is then localised: the first
# differing byte offset, how many bytes differ in total, and whether either
# artefact contains its own build path as a string. That last one is the whole
# diagnosis — rustc puts `core/src/pipeline.rs` in `panic::Location`, and if the
# absolute prefix is in there, the hash cannot match across directories.
#
# Usage:
#   scripts/repro-check.sh                 all four builds
#   scripts/repro-check.sh --quick         A1 and B1 only (same-path + path)
#   scripts/repro-check.sh --record FILE   refresh the hash table afterwards
#
# Exit status: 0 every comparison held, 1 one did not, 2 the build failed.
# =============================================================================

set -uo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
RECORD=""
QUICK=0

die() { printf 'repro-check.sh: %s\n' "$*" >&2; exit 2; }
say() { printf '%s\n' "$*"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --quick)        QUICK=1; shift ;;
    --record)       RECORD="$2"; shift 2 ;;
    --record=*)     RECORD="${1#*=}"; shift ;;
    -h|--help)      sed -n '2,34p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *)              die "unknown argument: $1" ;;
  esac
done

command -v sha256sum >/dev/null 2>&1 || die "sha256sum is required"
[ -f "${ROOT}/core/Cargo.toml" ] || die "no core/Cargo.toml at ${ROOT}"

# -----------------------------------------------------------------------------
say "=== 0. the cheap check first ==="
# A timestamp in the source is a reproducibility bug that costs one grep to find
# and would otherwise only show up as a hash mismatch, with no way to tell which
# of the four builds was the odd one out.
if grep -rnE '__DATE__|__TIME__|__TIMESTAMP__|SystemTime::now|Utc::now' \
     "${ROOT}/core/src" --include='*.rs' >/dev/null 2>&1; then
  say "  FAIL: core/src references a wall-clock source of build output:"
  grep -rnE '__DATE__|__TIME__|__TIMESTAMP__|SystemTime::now|Utc::now' \
    "${ROOT}/core/src" --include='*.rs' | sed 's/^/        /'
  die "fix the timestamp before comparing hashes; the comparison will not tell you which build is wrong"
fi
say "  ok: no __DATE__/__TIME__/wall-clock in core/src"

TARGET=$(rustc -vV | awk '/^host: /{print $2}')
say ""
say "  target:   ${TARGET}"
say "  rustc:    $(rustc --version)"
say "  cargo:    $(cargo --version | head -n1)"

WORK=$(mktemp -d)
trap 'rm -rf "${WORK}"' EXIT

# A second copy of the sources at a different absolute path. Only the files cargo
# reads are copied, and `core/target` is excluded twice over in case a previous
# run left one behind -- a copied target dir would make B2 test nothing.
say ""
say "=== 1. staging a second copy of the sources at a different path ==="
mkdir -p "${WORK}/b"
( cd "${ROOT}" && tar -cf - --exclude=./core/target --exclude=./app \
    ./core ./README.md ./deny.toml ) | ( cd "${WORK}/b" && tar -xf - ) \
  || die "could not stage the second copy"
[ -f "${WORK}/b/core/Cargo.toml" ] || die "the staged copy has no core/Cargo.toml"
say "  ${ROOT}"
say "  ${WORK}/b"

hash_of() {
  # $1 = variant label. The artefacts sit under <target-dir>/<triple>/release,
  # because build-release.sh always passes --target: a host build without one
  # lands in target/release/ where a plain `cargo build` overwrites it, and
  # "the release binary" has to name one file.
  #
  # BOTH the shared library and the static archive are hashed, because they do
  # not have the same answer and reporting only one of them would be a
  # convenient subset: `[profile.release] strip = true` removes the symbols and
  # debug info in which the build path lives from the cdylib, and does not
  # reach inside an archive at all.
  local dir="${WORK}/$1/${TARGET}/release"
  local shared="" static=""
  for candidate in libpixelsmith_core.so libpixelsmith_core.dylib libpixelsmith_core.dll; do
    [ -f "${dir}/${candidate}" ] && shared="${dir}/${candidate}"
  done
  [ -f "${dir}/libpixelsmith_core.a" ] && static="${dir}/libpixelsmith_core.a"
  [ -n "${shared}" ] || return 1
  local line="shared=$(sha256sum "${shared}" | awk '{print $1}')"
  if [ -n "${static}" ]; then
    line="${line} static=$(sha256sum "${static}" | awk '{print $1}')"
  fi
  printf '%s' "${line}"
}

# Reads one field out of a variant's record. $2 is `shared` or `static`.
artefact_hash() {
  local want="$2"
  echo "${H[$1]}" | tr ' ' '\n' | awk -v k="${want}" -F= '$1==k {print $2}'
}

build() {
  # $1 = variant label, $2 = source root, $3 = remap on/off, $4 = target dir
  say ""
  say "=== build ${1} (${2}, remap=${3}) ==="
  if [ "${2}" = "${ROOT}" ]; then
    bash "${ROOT}/scripts/build-release.sh" --quiet --target "${TARGET}" \
      --target-dir "${WORK}/$4" $( [ "${3}" = "1" ] && printf -- '--remap' || printf -- '--no-remap' ) || return 1
  else
    bash "${ROOT}/scripts/build-release.sh" --quiet --target "${TARGET}" \
      --source-root "${2}" --target-dir "${WORK}/$4" \
      $( [ "${3}" = "1" ] && printf -- '--remap' || printf -- '--no-remap' ) || return 1
  fi
}

# -----------------------------------------------------------------------------
say ""
say "=== 2. four builds ==="
declare -A H
build A1 "${ROOT}" 1 "t-a1" || die "A1 failed"
H[A1]=$(hash_of "t-a1") || die "A1 produced no shared library"
build A2 "${ROOT}" 1 "t-a2" || die "A2 failed"
H[A2]=$(hash_of "t-a2") || die "A2 produced no shared library"
[ "${QUICK}" = "1" ] || {
  build B1 "${WORK}/b" 0 "t-b1" || die "B1 failed"
  H[B1]=$(hash_of "t-b1") || die "B1 produced no shared library"
  build B2 "${WORK}/b" 1 "t-b2" || die "B2 failed"
  H[B2]=$(hash_of "t-b2") || die "B2 produced no shared library"
}

sha_of() { echo "${H[$1]}" | cut -d' ' -f1; }
path_of() { echo "${H[$1]}" | cut -d' ' -f2-; }

for v in A1 A2 B1 B2; do
  if [ -n "${H[$v]:-}" ]; then
    say "  ${v}  ${H[$v]}"
  fi
done

# -----------------------------------------------------------------------------
# Does this artefact embed its own build path? If it does, two checkouts in
# different directories cannot produce the same bytes, whatever the toolchain.
#
# Both artefacts are asked, because they answer differently and the difference
# is the whole finding: the stripped cdylib does not contain the path while the
# unstripped archive does.
say ""
say "=== 3. does the artefact embed its own build path? ==="
for v in A1 B1 B2; do
  [ -n "${H[$v]:-}" ] || continue
  case "${v}" in
    A1) root="${ROOT}" ;;
    *)  root="${WORK}/b" ;;
  esac
  dir="${WORK}/${v}/${TARGET}/release"
  for artefact in "${dir}"/libpixelsmith_core.so "${dir}"/libpixelsmith_core.a; do
    [ -f "${artefact}" ] || continue
    hits=$(LC_ALL=C grep -c -a -F -- "${root}" "${artefact}" 2>/dev/null || true)
    if [ "${hits:-0}" -gt 0 ]; then
      say "  ${v} $(basename "${artefact}"): YES — ${hits} line(s) contain ${root}"
    else
      say "  ${v} $(basename "${artefact}"): no  — ${root} does not appear"
    fi
  done
done

# -----------------------------------------------------------------------------
FAILURES=0
# $1 left variant, $2 right variant, $3 question, $4 artefact kind
compare() {
  local left="$1" right="$2" question="$3" kind="${4:-shared}"
  local lh rh
  lh=$(artefact_hash "${left}" "${kind}")
  rh=$(artefact_hash "${right}" "${kind}")
  say ""
  say "--- ${question} [${kind}] ---"
  say "  ${left}  ${lh:-<none>}"
  say "  ${right}  ${rh:-<none>}"
  if [ -z "${lh}" ] || [ -z "${rh}" ]; then
    say "  SKIPPED: this build produced no ${kind} library"
    return 0
  fi
  if [ "${lh}" = "${rh}" ]; then
    say "  MATCH"
    return 0
  fi
  FAILURES=$((FAILURES + 1))
  say "  DIFFER"
  return 1
}

# Every kind, for every pair. A verdict about "the release build" that only
# covers the shared library is not a verdict about the static archive the desktop
# CMake build links.
for kind in shared static; do
  compare A1 A2 "same path, same toolchain, two fresh build dirs" "${kind}"
  compare A1 B1 "different path, remapping off (a plain cargo build)" "${kind}"
  if [ "${QUICK}" != "1" ]; then
    compare A1 B2 "different path, --remap-path-prefix on" "${kind}"
  fi
done
done

# -----------------------------------------------------------------------------
FAILURES=0
compare() {
  local left="$1" right="$2" question="$3"
  say ""
  say "--- ${question} ---"
  say "  ${left}  $(sha_of "${left}")"
  say "  ${right}  $(sha_of "${right}")"
  if [ "$(sha_of "${left}")" = "$(sha_of "${right}")" ]; then
    say "  MATCH"
    return 0
  fi
  FAILURES=$((FAILURES + 1))
  say "  DIFFER"
  local l r
  l=$(path_of "${left}"); r=$(path_of "${right}")
  if command -v cmp >/dev/null 2>&1; then
    first=$(cmp -l "${l}" "${r}" 2>/dev/null | head -n1 | awk '{print $1}')
    total=$(cmp -l "${l}" "${r}" 2>/dev/null | wc -l)
    say "  first differing byte: ${first:-unknown}   differing bytes: ${total}"
  fi
  return 1
}

compare A1 A2 "same path, same toolchain, two fresh build dirs"
compare A1 B1 "different path, remapping off (the default cargo build)"
if [ "${QUICK}" != "1" ]; then
  compare A1 B2 "different path, --remap-path-prefix on"
fi

# -----------------------------------------------------------------------------
if [ -n "${RECORD}" ]; then
  say ""
  say "=== 4. recording ==="
  bash "${ROOT}/scripts/build-release.sh" --quiet --target "${TARGET}" \
    --target-dir "${WORK}/t-a1" --remap --record "${RECORD}" >/dev/null \
    || die "could not record into ${RECORD}"
  say "  wrote the ${TARGET} row in ${RECORD}"
fi

say ""
if [ "${FAILURES}" -eq 0 ]; then
  say "repro-check: REPRODUCIBLE — every comparison matched"
  exit 0
fi
say "repro-check: NOT REPRODUCIBLE — ${FAILURES} comparison(s) differ; see above and docs/SUPPLY-CHAIN.md"
exit 1