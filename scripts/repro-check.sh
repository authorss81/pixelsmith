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
# differing byte offset, how many bytes differ, whether the sizes differ, and
# whether either artefact contains its own build path as a literal string.
#
# That last check is worth reading carefully, because on this tree it answers
# "no" for a pair that genuinely differs, and the reason is the interesting part.
# MEASURED on x86_64-unknown-linux-gnu, staticlib, no-remap against remap: 1261
# differing bytes in 165 runs from offset 507411, identical file size, and NOT ONE
# ABSOLUTE PATH in either artefact. What differs is 81 per-crate 16-hex-digit
# identity hashes that appear in symbol names — `libc-5ca03e1b3e78aee9.libc.…`
# against `libc-c1ce7e49860bfa88.libc.…` — one per crate in the graph, and
# `--remap-path-prefix` is what changes them. The second hex group is stable; the
# first is not.
#
# So the lesson is a negative one worth stating, because it is the assumption that
# makes people ship unreproducible builds: **grepping the binary for the build
# path does not detect this**, and neither does building twice in one directory.
# The path reaches the artefact as a hash. Only comparing the artefacts finds it.
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

# Where a variant's target directory is. One function, because two conventions
# in this script is not a style question: `build` was given the directory name
# (`t-a1`) and everything that reads the artefact back was given the variant name
# (`A1`), so the build-path diagnostic found no files at all and reported
# nothing, and `cmp` was handed a path that did not exist and — with its stderr
# discarded — answered "0 differing bytes". That reads like an identical prefix,
# which is the one thing it must never say about a pair that just hashed
# differently. A diagnostic that silently finds nothing is worse than no
# diagnostic, because its silence reads as a clean result.
target_dir_for() {
  case "$1" in
    A1) printf 't-a1' ;;
    A2) printf 't-a2' ;;
    B1) printf 't-b1' ;;
    B2) printf 't-b2' ;;
    *)  die "unknown variant: $1" ;;
  esac
}

# Report what two artefacts disagree about, in the terms that distinguish a
# hash-of-the-input problem from a path-length problem. $1 and $2 are files, $3 is
# a label. Deliberately python rather than shell: comparing two 70 MB binaries
# byte by byte in bash is a `cmp -l | wc -l` per pair, and the crate-identity
# extraction needs a regex, and doing either in awk would be three unreadable
# lines that compute less than this does.
describe_difference() {
  local left="$1" right="$2" label="$3"
  say ""
  say "--- ${label} ---"
  if [ ! -f "${left}" ] || [ ! -f "${right}" ]; then
    say "  SKIPPED: one of the two artefacts is missing"
    return 0
  fi
  if cmp -s "${left}" "${right}"; then
    say "  identical"
    return 0
  fi
  python3 - "${left}" "${right}" <<'PY' || say "  (could not analyse: python3 or a read error)"
import re, sys

a = open(sys.argv[1], "rb").read()
b = open(sys.argv[2], "rb").read()
print(f"  sizes: {len(a)} vs {len(b)}" +
      ("  (equal — this is not a path-length difference)"
       if len(a) == len(b) else "  (DIFFERENT — consistent with a path of a different length)"))

diffs = [i for i in range(min(len(a), len(b))) if a[i] != b[i]]
extra = abs(len(a) - len(b))
if not diffs and not extra:
    print("  no differing bytes")
    raise SystemExit(0)
print(f"  differing bytes: {len(diffs) + extra}"
      + (f"   first at offset {diffs[0] + 1}" if diffs else ""))

runs, start, prev = [], (diffs[0] if diffs else 0), (diffs[0] if diffs else 0)
for i in diffs[1:]:
    if i == prev + 1:
        prev = i
    else:
        runs.append((start, prev)); start = prev = i
if diffs:
    runs.append((start, prev))
print(f"  contiguous runs: {len(runs)}")

# crate-identity names: <crate>-<16 hex>.<crate>.<16 hex>
pat = re.compile(rb"([a-z0-9_]+)-([0-9a-f]{16})\.\1\.([0-9a-f]{16})")
lo = diffs[0] if diffs else 0
hi = (diffs[-1] if diffs else 0) + 1
left, right = set(), set()
for buf, sink in ((a, left), (b, right)):
    for m in pat.finditer(buf[lo:hi]):
        sink.add(m.group(1).decode())
shared = left & right
print(f"  crate-identity names in the differing region: {len(left | right)}"
      f" ({len(shared)} of them in both)")
sample = sorted(shared)[:4]
if sample:
    print("  e.g. " + ", ".join(sample))
if shared:
    print("  => per-crate identity hashes: the differing input reaches the artefact as a")
    print("     HASH, not as a path string. --remap-path-prefix is what neutralises it,")
    print("     and grepping the binary for the build path cannot find it.")
else:
    # The verdict above is only true for a difference that actually consists of
    # these names. Printing it unconditionally would be a script asserting a
    # diagnosis it did not make, which is worse than printing nothing.
    print("  => NOT per-crate identity hashes: no such name occurs in the differing")
    print("     region, so this is something else. The size line above is the first")
    print("     thing to read; a size difference points at a path of a different")
    print("     length and an equal size points at an input this script cannot name.")
PY
}

hash_of() {
  # $1 = target directory label. The artefacts sit under
  # <target-dir>/<triple>/release, because build-release.sh always passes
  # --target: a host build without one lands in target/release/ where a plain
  # `cargo build` overwrites it, and "the release binary" has to name one file.
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

# Where one variant's artefact actually is, so a mismatch can be localised with
# `cmp` instead of only reported as two different hashes.
artefact_path() {
  local dir="${WORK}/$(target_dir_for "$1")/${TARGET}/release"
  case "${2}" in
    static) printf '%s' "${dir}/libpixelsmith_core.a" ;;
    *)
      for candidate in libpixelsmith_core.so libpixelsmith_core.dylib libpixelsmith_core.dll; do
        [ -f "${dir}/${candidate}" ] && { printf '%s' "${dir}/${candidate}"; return 0; }
      done
      return 1
      ;;
  esac
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

for v in A1 A2 B1 B2; do
  if [ -n "${H[$v]:-}" ]; then
    say "  ${v}  ${H[$v]}"
  fi
done

# -----------------------------------------------------------------------------
# Does this artefact embed its own build path as a literal string? If it does,
# two checkouts in different directories cannot produce the same bytes.
#
# BOTH artefacts are asked, and the honest answer on this tree is "no" for both,
# even for a pair that differs — see the header. That is reported here rather than
# hidden, because "the path is not in the binary" is the finding that makes the
# obvious reproducibility check useless, and a reader who saw only a MATCH line
# would draw the opposite conclusion.
say ""
say "=== 3. does the artefact embed its own build path as a literal string? ==="
for v in A1 B1 B2; do
  [ -n "${H[$v]:-}" ] || continue
  case "${v}" in
    A1) root="${ROOT}" ;;
    *)  root="${WORK}/b" ;;
  esac
  dir="${WORK}/$(target_dir_for "${v}")/${TARGET}/release"
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
# A variant that produced nothing is reported rather than skipped in silence.
# "This diagnostic found no artefacts" is the one answer here that means the
# script is broken, and it has to be visible when it happens.
SAW_ARTEFACT=0
for v in A1 B1 B2; do
  [ -n "${H[$v]:-}" ] || continue
  dir="${WORK}/$(target_dir_for "${v}")/${TARGET}/release"
  for artefact in "${dir}"/libpixelsmith_core.*; do
    [ -f "${artefact}" ] && SAW_ARTEFACT=1
  done
done
if [ "${SAW_ARTEFACT}" = "0" ]; then
  say "  FAIL: this diagnostic inspected no artefacts at all, so its silence below"
  say "        means nothing. Check target_dir_for() against the directories build created."
  die "the build-path diagnostic was vacuous"
fi

# -----------------------------------------------------------------------------
# And when a pair differs, what actually differs?
#
# Section 3 says whether the path appears as a string, and on this tree the
# answer is "no" even for a pair whose hashes differ — because the path reaches
# the artefact as a per-crate identity hash rather than as text. So the question
# this block answers is the one that actually localises it: how many bytes, are
# the sizes the same, and do the differing regions contain crate-identity names?
#
# A pair that differs with identical sizes and a few thousand differing bytes in
# 16-hex-digit groups is a hash-of-the-input problem. A pair that differs with
# different sizes is a path-length problem. They have different fixes and the
# output says which one this is.
say ""
say "=== 3b. what differs between A1 and B1 (the pair most likely to differ) ==="
describe_difference \
  "$(artefact_path A1 shared)" "$(artefact_path B1 shared)" "shared (cdylib)"
describe_difference \
  "$(artefact_path A1 static)" "$(artefact_path B1 static)" "static (staticlib)"

# -----------------------------------------------------------------------------
FAILURES=0
# $1 left variant, $2 right variant, $3 question, $4 artefact kind.
#
# Both artefact kinds, for every pair. A verdict about "the release build" that
# only covers the shared library is not a verdict about the static archive the
# desktop CMake build links, and they answer differently: `[profile.release]
# strip = true` removes the symbols and debug info the build path lives in from
# the cdylib, and does not reach inside an archive at all.
compare() {
  local left="$1" right="$2" question="$3" kind="${4:-shared}"
  local lh rh l r
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
  l=$(artefact_path "${left}" "${kind}")
  r=$(artefact_path "${right}" "${kind}")
  # Both files must exist before `cmp` is worth running. If they do not, that is
  # a defect in this script and not a finding about the build, and saying "0
  # differing bytes" about it would be the most misleading sentence in the file.
  if [ ! -f "${l}" ] || [ ! -f "${r}" ]; then
    say "  NOT LOCALISED: ${l} or ${r} does not exist; this script cannot read"
    say "  back what it built, which is a bug here rather than a property of the build."
    return 1
  fi
  if command -v cmp >/dev/null 2>&1; then
    first=$(cmp -l "${l}" "${r}" 2>/dev/null | head -n1 | awk '{print $1}')
    total=$(cmp -l "${l}" "${r}" 2>/dev/null | wc -l)
    say "  first differing byte: ${first:-unknown}   differing bytes: ${total}"
    say "  sizes: $(stat -c%s "${l}" 2>/dev/null || stat -f%z "${l}") vs $(stat -c%s "${r}" 2>/dev/null || stat -f%z "${r}")"
  fi
  return 1
}

for kind in shared static; do
  compare A1 A2 "same path, same toolchain, two fresh build dirs" "${kind}"
  compare A1 B1 "different path, remapping off (a plain cargo build)" "${kind}"
  if [ "${QUICK}" != "1" ]; then
    compare A1 B2 "different path, --remap-path-prefix on" "${kind}"
  fi
done

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