#!/usr/bin/env bash
# =============================================================================
# release-checksums.sh — what was published, and what was not.
#
# Two hash tables live in this repository and they answer different questions,
# which is why they are two files:
#
#   scripts/BUILD-SHA256.txt   the engine library per target. "Does this build
#                              reproduce?" — one row per Rust target triple,
#                              possibly NOT-MEASURED.
#   scripts/RELEASE-SHA256.txt the shipped artefact. "Is the file the user
#                              downloaded the file we built?" — one row per
#                              artefact in the release, including the ones that
#                              were not built.
#
# The second table is the one that answers "is this download trustworthy", so it
# records the *absence* of an artefact as a row rather than leaving the gap for a
# reader to infer. A list of the files that were built, saying nothing about the
# one that was not, is a list that reads as complete.
#
# Usage:
#   release-checksums.sh --write FILE --version X.Y.Z [--expect NAME --expect-reason WHY]... FILE...
#   release-checksums.sh --check FILE FILE...
#   release-checksums.sh --self-test
#
# Exit 0 = written, or every present file matched. 1 = a mismatch or a missing
# row. 2 = bad arguments.
# =============================================================================

set -uo pipefail

FILE=""
VERSION=""
MODE=""
REASON=""
CHECKED=0
MISMATCH=0
# Declared before the argument loop, because the loop appends to them.
EXPECT=()
TARGETS=()

die() { printf 'release-checksums.sh: %s\n' "$*" >&2; exit 2; }
say() { printf '%s\n' "$*"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --write)     MODE="write"; FILE="$2"; shift 2 ;;
    --check)     MODE="check"; FILE="$2"; shift 2 ;;
    --version)   VERSION="$2"; shift 2 ;;
    --version=*) VERSION="${1#*=}"; shift ;;
    --expect)    EXPECT+=("$2"); shift 2 ;;
    --expect=*)  EXPECT+=("${1#*=}"); shift ;;
    --reason)    REASON="$2"; shift 2 ;;
    --reason=*)  REASON="${1#*=}"; shift ;;
    --self-test) MODE="--self-test"; shift ;;
    -h|--help)   sed -n '2,38p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*)          die "unknown option: $1" ;;
    *)           TARGETS+=("$1"); shift ;;
  esac
done

hash_of() {
  sha256sum "$1" 2>/dev/null | awk '{print $1}'
}

size_of() {
  stat -c%s "$1" 2>/dev/null || stat -f%z "$1" 2>/dev/null || echo 0
}

header() {
  cat <<EOF
# Release artefact checksums — version ${VERSION}.
#
#     <artefact>  <sha256 | NOT-BUILT>  <bytes>  — <what it is, or why it is absent>
#
# Written by
#     bash scripts/release-checksums.sh --write scripts/RELEASE-SHA256.txt \\
#         --version ${VERSION} <files...>
#
# This is NOT scripts/BUILD-SHA256.txt. That table is the engine library per Rust
# target triple and answers "does the build reproduce". This one is the shipped
# artefact and answers "is the file you downloaded the file we built". Neither is
# derived from the other.
#
# A row reading NOT-BUILT is not a placeholder. It is an artefact the release was
# supposed to contain and did not, with the reason stated in the row — a Windows
# bundle cannot be produced on a Linux runner, a Play-signed AAB needs an upload
# key the repository does not hold. Leaving the row out instead would make the
# table read as a complete list of the release, which is the one thing a
# checksum table must not do.
#
# Verify a download with:
#     sha256sum -c <(grep -v '^#' ${FILE})
EOF
}

do_write() {
  [ -n "${FILE}" ] || die "--write needs a file path"
  [ -n "${VERSION}" ] || die "--write needs --version X.Y.Z"
  [ ${#EXPECT[@]} -gt 0 ] && [ -z "${REASON}" ] \
    && die "--expect was given but --reason was not; a NOT-BUILT row with no reason is silence"

  local tmp="${FILE}.tmp.$$"
  header > "${tmp}"

  local name hash size
  for name in "${TARGETS[@]+"${TARGETS[@]}"}"; do
    if [ ! -f "${name}" ]; then
      die "no such file: ${name} (use --expect for an artefact that was not built)"
    fi
    hash=$(hash_of "${name}")
    size=$(size_of "${name}")
    printf '%-52s %s  %s\n' "$(basename "${name}")" "${hash}" "${size}" >> "${tmp}"
    say "  ${hash}  $(basename "${name}")"
  done

  for name in ${EXPECT[@]+"${EXPECT[@]}"}; do
    printf '%-52s %s  %s  — %s\n' "${name}" "NOT-BUILT" "-" "${REASON}" >> "${tmp}"
    say "  NOT-BUILT  ${name}  — ${REASON}"
  done

  if [ ${#TARGETS[@]} -eq 0 ] && [ ${#EXPECT[@]} -eq 0 ]; then
    die "nothing to write: give files, or --expect NAME --reason WHY"
  fi

  mv "${tmp}" "${FILE}"
  say ""
  say "release-checksums.sh: wrote ${FILE}"
  return 0
}

do_check() {
  [ -f "${FILE}" ] || die "no such table: ${FILE}"
  CHECKED=0
  MISMATCH=0
  for name in ${TARGETS[@]+"${TARGETS[@]}"}; do
    if [ ! -f "${name}" ]; then
      say "  ${name}: not present, nothing to compare"
      MISMATCH=$((MISMATCH + 1))
      continue
    fi
    local want got
    want=$(grep -F "$(basename "${name}")" "${FILE}" 2>/dev/null \
           | awk '{print $2}' | head -n 1)
    if [ -z "${want}" ] || [ "${want}" = "NOT-BUILT" ]; then
      say "  ${name}: no usable hash in ${FILE}"
      MISMATCH=$((MISMATCH + 1))
      continue
    fi
    got=$(hash_of "${name}")
    if [ "${got}" = "${want}" ]; then
      say "  ok:   ${name}"
      CHECKED=$((CHECKED + 1))
    else
      say "  FAIL: ${name}"
      say "        table: ${want}"
      say "        file:  ${got}"
      MISMATCH=$((MISMATCH + 1))
    fi
  done
  say ""
  if [ "${MISMATCH}" -eq 0 ]; then
    say "release-checksums.sh: PASS (${CHECKED} file(s) matched)"
    return 0
  fi
  say "release-checksums.sh: FAIL (${MISMATCH} problem(s))"
  return 1
}

# =============================================================================
# --self-test. Same reasoning as the other two scripts in this directory: a
# table that has only ever been compared against a matching file has not been
# shown to reject a mismatched one.
# Global, because the EXIT trap fires after every local in selftest has gone out
# of scope — a `set -u` "unbound variable" on the last line of a passing run.
SELFTEST_DIR=""
cleanup_selftest() { [ -n "${SELFTEST_DIR}" ] && rm -rf "${SELFTEST_DIR}"; }

selftest() {
  local cases=0 fired=0 rc cmd work
  SELFTEST_DIR=$(mktemp -d "${TMPDIR:-/tmp}/px-checksums.XXXXXX") || die "mktemp failed"
  trap cleanup_selftest EXIT
  work="${SELFTEST_DIR}"

  local apk="${work}/pixelsmith-0.1.0-android.aab"
  printf 'pixelsmith-0.1.0-android.aab\n' > "${apk}"

  say "release-checksums.sh --self-test"
  say ""

  # run NAME EXPECTED_RC DESCRIPTION — with CMD holding the command line to run.
  run() {
    local name="$1" expect="$2" desc="$3"
    cases=$((cases + 1))
    ( eval "${cmd}" ) >/dev/null 2>&1
    rc=$?
    if [ "${rc}" = "${expect}" ]; then
      say "  ok:   ${name} -> exit ${rc} (${desc})"
      fired=$((fired + 1))
    else
      say "  FAIL: ${name} -> exit ${rc}, expected ${expect} (${desc})"
    fi
  }

  ME="${BASH_SOURCE[0]}"
  cmd="${ME} --write \"\${work}/t-good.txt\" --version 0.1.0 \"\${apk}\" && ${ME} --check \"\${work}/t-good.txt\" \"\${apk}\""
  run "write-then-check" 0 "the hash just written matches the file it came from"

  # One byte appended after the table was written is the whole point of a
  # checksum table, and the case that proves --check is not a no-op.
  cmd="${ME} --write \"\${work}/t-tamper.txt\" --version 0.1.0 \"\${apk}\" && printf 'tampered\\n' >> \"\${apk}\" && ${ME} --check \"\${work}/t-tamper.txt\" \"\${apk}\""
  run "tampered-file" 1 "one byte appended to the file after the table was written"

  cmd="${ME} --check \"\${work}/t-tamper.txt\" \"\${work}/absent\""
  run "check-a-missing-file" 1 "a file that is not there at all"

  # A row marked NOT-BUILT must not be mistaken for a match.
  cmd="${ME} --write \"\${work}/t-notbuilt.txt\" --version 0.1.0 --expect pixelsmith-0.1.0-windows-x64.zip --reason 'needs a Windows runner; flutter build windows does not cross-compile'"
  run "write-a-not-built-row" 0 "an artefact that was not built is recorded, not omitted"

  cmd="grep -c NOT-BUILT \"\${work}/t-notbuilt.txt\""
  run "not-built-row-is-in-the-table" 0 "and the row really says NOT-BUILT"

  cmd="${ME} --check \"\${work}/t-notbuilt.txt\" \"\${apk}\""
  run "check-against-not-built-table" 1 "no usable hash for a file that is present"

  # --expect with no --reason is a refusal: a NOT-BUILT row with no reason says
  # nothing about why, which is the silence this table exists to avoid.
  cmd="${ME} --write \"\${work}/t-noreason.txt\" --version 0.1.0 --expect pixelsmith-0.1.0-windows-x64.zip \"\${apk}\""
  run "expect-without-reason" 2 "a NOT-BUILT row with no reason is rejected"

  cmd="${ME} --write \"\${work}/t-missing.txt\" --version 0.1.0 \"\${work}/absent.apk\""
  run "write-a-missing-file" 2 "asking for a file that does not exist"

  cmd="${ME} --write \"\${work}/t-empty.txt\" --version 0.1.0"
  run "write-nothing" 2 "a table with no rows in it"

  cmd="${ME} --write \"\${work}/t-noversion.txt\" \"\${apk}\""
  run "write-without-a-version" 2 "no --version, so the table cannot say which release"

  cmd="${ME} --check \"\${work}/does-not-exist.txt\" \"\${apk}\""
  run "check-missing-table" 2 "checking against a table that is not there"

  say ""
  say "  ${fired}/${cases} cases behaved as specified"
  if [ "${fired}" -ne "${cases}" ]; then
    say "release-checksums.sh --self-test: FAIL"
    return 1
  fi
  say "release-checksums.sh --self-test: PASS"
  return 0
}

case "${MODE}" in
  --self-test) selftest; exit $? ;;
  write)       do_write; exit $? ;;
  check)       do_check; exit $? ;;
  "")          die "no mode: --write, --check or --self-test" ;;
  *)           die "unknown mode: ${MODE}" ;;
esac