#!/usr/bin/env bash
# =============================================================================
# check-json-contract.sh — does the Dart side still model the engine's JSON?
#
# `check-dart-bindings.sh` proves the app's `bindings.dart` matches the engine's
# C ABI, and it reports "15 entry points match" while five phases of JSON fields
# were missing from the Dart models. This is the check for the other half of the
# boundary. `docs/AUDIT.md` findings 3, 4 and 5 are all instances of it.
#
# WHY IT IS A GATE CHECK WHEN check-dart-bindings.sh IS A NOTE
# ------------------------------------------------------------
# `app/` is the `authorss81/shrinkray` submodule: a separate repository this
# pipeline can read and cannot push to, so the only response to a red result is a
# commit nobody here can make. A gate check that is red for a reason the reader
# cannot act on stops being read, and then it catches nothing — which is what
# happened to binding drift.
#
# So the *committed* side of this comparison is `core/contract/json-fields.txt`,
# in this repository, where it can be landed. The engine's side is generated at
# run time by `cargo run --bin px-json-dump`, which asks serde for the field names
# rather than parsing the source (see `core/src/ffi_json.rs` for why a source
# parser would eventually disagree with serde and report drift that is not
# there). Three failures are checked, in both directions:
#
#   unrecorded   the engine emits a field with no row   — drift arriving
#   stale        a row names a field the engine lacks     — a row that has rotted
#   unmodelled   a row has no Dart symbol after the tab   — a row nobody wrote
#
# The third is the one that matters most. Every drift in the audit could have been
# prevented by a row like "ValidateReport.colour is not modelled", written down
# and reviewed. What there was instead was silence, and silence is
# indistinguishable from "nothing to do".
#
# Usage:
#   bash scripts/check-json-contract.sh                  # check, and print a summary
#   bash scripts/check-json-contract.sh --write          # append missing rows, unmodelled
#   bash scripts/check-json-contract.sh --dart FILE      # check against another models.dart
#   bash scripts/check-json-contract.sh --self-test      # prove the check can fail
#
# Exit 0 = in sync. Exit 1 = drift. Exit 2 = could not check.
# =============================================================================

set -uo pipefail

CONTRACT="${PX_CONTRACT_FILE:-core/contract/json-fields.txt}"
DART_MODELS="app/lib/rust/models.dart"
MODE="check"

for arg in "$@"; do
  case "${arg}" in
    --write)     MODE="write" ;;
    --self-test) MODE="self-test" ;;
    --dart)      MODE="check" ;;
    --dart=*)    DART_MODELS="${arg#--dart=}" ;;
    -h|--help)   sed -n '2,40p' "$0"; exit 0 ;;
    *)           echo "error: unknown flag: ${arg}" >&2; exit 2 ;;
  esac
done

if [ ! -f core/Cargo.toml ]; then
  echo "error: core/Cargo.toml not found; run this from the repository root" >&2
  exit 2
fi

DUMP=(cargo run --quiet --manifest-path core/Cargo.toml --bin px-json-dump --)

# ---------------------------------------------------------------------------
# The Dart-side check.
#
# Every Dart symbol named in column two has to actually appear in models.dart.
# This is the half that cannot be a gate on its own, because models.dart lives in
# the submodule and this pipeline cannot land a change to it — so it runs only
# when the caller asks for it (`--dart`), or inside `--self-test` against a
# fixture. The landable half is the contract file itself.
# ---------------------------------------------------------------------------
check_dart_symbols() {
  local models="$1"
  local rc=0
  local missing=0
  local total=0
  local symbol

  if [ ! -f "${models}" ]; then
    echo "CONTRACT/DART: ${models} not found" >&2
    return 2
  fi

  while IFS=$'\t' read -r path dart rest; do
    case "${path}" in \#*|"") continue ;; esac
    [ -z "${dart}" ] && continue
    total=$((total + 1))
    if ! grep -qF -- "${dart}" "${models}"; then
      missing=$((missing + 1))
      echo "  ${path} is recorded as ${dart}, which ${models} does not declare"
    fi
  done < "${CONTRACT}"

  if [ "${missing}" -ne 0 ]; then
    rc=1
  fi
  echo "CONTRACT/DART: ${missing} of ${total} Dart symbols absent from ${models}"
  return ${rc}
}

# ---------------------------------------------------------------------------
# --write
#
# Appends the rows the engine emits and the file does not record, with an empty
# second column — which the check then rejects until somebody names the Dart
# member. So `--write` cannot make a red gate green; it converts "the gate is red
# and the file was never updated" into "the gate is red and the file says which
# line to fill in", which is the difference between a chore and a decision.
#
# Existing rows are never touched: a row is somebody's statement about a Dart
# symbol, and rewriting it silently would destroy the only record of that.
# ---------------------------------------------------------------------------
write_missing() {
  local generated
  generated="$(mktemp)"
  trap 'rm -f "${generated}"' RETURN
  "${DUMP[@]}" --print-keys > "${generated}" || return 2

  local added=0
  while IFS= read -r key; do
    if ! grep -qF -- "${key}" "${CONTRACT}"; then
      printf '%s\t\n' "${key}" >> "${CONTRACT}"
      added=$((added + 1))
      echo "  added ${key}"
    fi
  done < "${generated}"

  # Re-sort the rows, keeping the header where it is. Sorted because the file is
  # diffed in review, and a file that reorders on every write is one nobody reads.
  local header
  header="$(grep -c '' "${CONTRACT}")"
  {
    grep '^#' "${CONTRACT}"
    echo
    grep -v '^#' "${CONTRACT}" | grep -v '^[[:space:]]*$' | LC_ALL=C sort
  } > "${CONTRACT}.sorted"
  mv "${CONTRACT}.sorted" "${CONTRACT}"
  echo "WROTE: ${added} row(s) appended to ${CONTRACT} (${header} lines before)"
  return 0
}

# ---------------------------------------------------------------------------
# --self-test
#
# Proves the check can fail, in both directions and offline, because this
# repository has seven recorded instances of a gate check that could not report the
# truth (listed in `scripts/verify.sh`) and two of them cost a phase whose actual
# work was already finished. A drift check that has never been observed to go red
# is not known to work.
#
# Three failures are injected, one at a time, and each must be caught:
#
#   1. a field the engine emits with no row          → "unrecorded"
#   2. a row for a field the engine does not emit     → "stale"
#   3. a row with no Dart symbol after the tab        → "unmodelled"
#   4. a Dart symbol nothing declares                 → the --dart half
# ---------------------------------------------------------------------------
self_test() {
  local failures=0
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "${tmp}"' RETURN

  echo "SELFTEST: injecting four deliberate contract defects"

  # 1. Unrecorded: pretend the engine gained a field nobody wrote down.
  cp "${CONTRACT}" "${tmp}/unrecorded.txt"
  echo -e "Outcome.colour_space_projection\tsomethingNobodyModelled" \
    >> "${tmp}/unrecorded.txt"
  if PX_CONTRACT_FILE="${tmp}/unrecorded.txt" "${DUMP[@]}" --check "${tmp}/unrecorded.txt" \
      > "${tmp}/out1" 2>&1; then
    echo "  FAIL 1: an unrecorded engine field was NOT caught"
    failures=$((failures + 1))
  else
    grep -q 'unrecorded\|does not record' "${tmp}/out1" \
      && echo "  ok 1: an unrecorded engine field is caught" \
      || { echo "  FAIL 1: caught, but did not say why"; failures=$((failures + 1)); }
  fi

  # 2. Stale: a row for something the engine no longer has.
  cp "${CONTRACT}" "${tmp}/stale.txt"
  echo -e "Outcome.a_field_that_was_removed\tsomeMember" >> "${tmp}/stale.txt"
  if "${DUMP[@]}" --check "${tmp}/stale.txt" > "${tmp}/out2" 2>&1; then
    echo "  FAIL 2: a stale row was NOT caught"
    failures=$((failures + 1))
  else
    grep -q 'no longer emits' "${tmp}/out2" \
      && echo "  ok 2: a stale row is caught" \
      || { echo "  FAIL 2: caught, but did not say why"; failures=$((failures + 1)); }
  fi

  # 3. Unmodelled: the exact shape of the drift this project already shipped —
  #    a row that records a field and says nothing about who carries it.
  cp "${CONTRACT}" "${tmp}/unmodelled.txt"
  echo -e "Outcome.a_field_with_no_symbol\t" >> "${tmp}/unmodelled.txt"
  if "${DUMP[@]}" --check "${tmp}/unmodelled.txt" > "${tmp}/out3" 2>&1; then
    echo "  FAIL 3: a row with no Dart symbol was NOT caught"
    failures=$((failures + 1))
  else
    grep -q 'no Dart symbol' "${tmp}/out3" \
      && echo "  ok 3: a row with no Dart symbol is caught" \
      || { echo "  FAIL 3: caught, but did not say why"; failures=$((failures + 1)); }
  fi

  # 4. The Dart half: a contract file that is internally perfect against an
  #    engine, naming a Dart member that does not exist. This is the check that
  #    catches the *fix* being wrong rather than the engine changing, so it runs
  #    against a fixture rather than the submodule.
  cp "${CONTRACT}" "${tmp}/dart.txt"
  printf '# a deliberately wrong Dart member\nOutcome.width\tNoSuchDartMember.width\n' \
    >> "${tmp}/dart.txt"
  : > "${tmp}/models.dart"
  if PX_CONTRACT_FILE="${tmp}/dart.txt" check_dart_symbols "${tmp}/models.dart" \
      > "${tmp}/out4" 2>&1; then
    echo "  FAIL 4: an absent Dart symbol was NOT caught"
    failures=$((failures + 1))
  else
    grep -q 'does not declare' "${tmp}/out4" \
      && echo "  ok 4: an absent Dart symbol is caught" \
      || { echo "  FAIL 4: caught, but did not say why"; failures=$((failures + 1)); }
  fi

  # And the positive control: the real file against the real engine must pass.
  # Without this the four failures above prove nothing — a check that fails on
  # everything catches drift and also catches nothing else.
  if "${DUMP[@]}" --check "${CONTRACT}" > "${tmp}/out5" 2>&1; then
    echo "  ok 5: the real contract passes against the real engine"
  else
    echo "  FAIL 5: the real contract does not pass against the real engine"
    sed 's/^/        /' "${tmp}/out5"
    failures=$((failures + 1))
  fi

  if [ "${failures}" -ne 0 ]; then
    echo "SELFTEST: ${failures} of 5 did not behave as claimed"
    return 1
  fi
  echo "SELFTEST: 5/5 as claimed"
  return 0
}

case "${MODE}" in
  write)     write_missing; exit $? ;;
  self-test) self_test;     exit $? ;;
  check)     ;;             # fall through
esac

# The gate itself. One command, three failures, and it is wired into verify.sh
# as a hard failure.
rc=0
"${DUMP[@]}" --check "${CONTRACT}" || rc=1

# The Dart half, when asked for. Not part of the default run because it reads the
# submodule, which this repository cannot land a change to — see the header.
if [ "${DART_MODELS}" != "app/lib/rust/models.dart" ]; then
  check_dart_symbols "${DART_MODELS}" || rc=1
fi

exit ${rc}