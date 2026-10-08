#!/usr/bin/env bash
# =============================================================================
# check-test-assertions.sh — a `#[test]` whose body asserts nothing is green.
#
# AGENTS.md: "A test asserts a specific value, not just 'it didn't panic'."
# docs/AUDIT.md finding 20 is four of them, and one of the four was the only
# sentinel hard rule 4 has: core/tests/hostile.rs called
# Limits::apply_to_decoder and then threw the result away with `let _ =`, so
# deleting the body of apply_to_decoder left it green. Nothing in the build was
# watching, and the `let _ =` is what made it invisible.
#
# The check is deliberately crude and deliberately explicit about being crude. A
# test that delegates to a helper which asserts is a false positive, so those are
# listed by name WITH A REASON rather than tolerated by a heuristic that tolerates
# the real thing too. The allow-list is the reviewable half of this script: an
# entry nobody justified is a check that has quietly stopped meaning anything.
#
# Usage:
#   bash scripts/check-test-assertions.sh [--self-test] [ROOT]
#
# ROOT defaults to `core`; reported paths are relative to this repository's root.
# Exit 0 = every `#[test]` in the tree asserts something. Exit 1 = a finding.
# =============================================================================

set -uo pipefail

SELF_TEST=0
ROOT="core"

while [ $# -gt 0 ]; do
  case "$1" in
    --self-test) SELF_TEST=1 ;;
    -h|--help)   sed -n '2,26p' "$0"; exit 0 ;;
    *)           ROOT="$1" ;;
  esac
  shift
done

# -----------------------------------------------------------------------------
# The allow-list: `path::fn | reason`, one per line, path relative to this
# repository's root.
#
# Every entry must carry a non-empty reason. `--self-test` proves that a row
# without one is refused, and that an allow-listed function is skipped while its
# identically-named twin in another file is still reported — because an
# allow-list keyed only on a name would silently exempt both.
#
# Prune aggressively. Each entry is a test whose *name* claims more than its body
# says, and the reason says which test now makes that claim instead.
ALLOWLIST='core/src/stream.rs::a_jpeg_goes_through_the_whole_image_arm|the body is one call to assert_matches_the_in_memory_kernel, which asserts the pixel difference against the in-memory kernel and both tolerances; it was rewritten in phase-21 precisely because the version that asserted only dimensions was green against a stride bug.
core/src/stream.rs::a_jpeg_with_a_non_integer_ratio_still_matches_the_in_memory_kernel|same helper, at a non-integer reduction ratio; adding the assertion inline twice would be two copies of the tolerance argument to keep in step.
core/tests/hostile.rs::truncation_at_every_offset_is_survivable|the assertion is inside assert_bounded_success, the file-wide helper that checks a report against its own limits; the loop body has no other statement that could assert.
core/tests/hostile.rs::truncated_input_never_decodes_beyond_the_limits|as above, the same assert_bounded_success helper; the claim under test is that a truncated file never decodes past the ceiling.
core/tests/hostile.rs::empty_and_tiny_inputs_are_rejected_with_a_named_error|the assertion is inside assert_rejected, the file-wide strict allow-list that panics on any error variant not named in it.
core/tests/sandbox.rs::the_worker_entry_point_has_the_documented_signature|the entry point is fn() -> ! and reads the real stdin of the test process, so the body can only be a function-pointer assignment; a_worker_run_exits_zero_with_a_decodable_response_on_stdout reaches it through the process boundary instead.'

# -----------------------------------------------------------------------------
# The scanner.
#
# A test function's body runs from the `{` that opens it to the `}` that closes
# it, and what delimits it is the *indentation* of that closing brace rather than
# a count of braces. Three reasons:
#
#   * braces inside string literals and `format!` arguments do not balance, so a
#     counting scanner reads a literal `{` as a block and stops early;
#   * `cargo fmt --check` is a gate check (verify.sh section 2), so every body in
#     this tree is rustfmt's shape: the closing brace of a function is alone on a
#     line at the function's own indentation, and a nested block, closure or
#     struct literal all close deeper than that;
#   * therefore nothing inside a body can be mistaken for its end, which is the
#     failure mode a brace counter has and this does not.
#
# The assertion macros are `assert!`, `assert_eq!`, `assert_ne!`, `panic!`,
# `unreachable!` and proptest`s `prop_assert*`. Deliberately NOT `.unwrap()` and
# NOT `.expect()`: a test whose only claim is that nothing returned an Err is
# this same defect in another costume, and counting `expect(` as an assertion
# would wave it through.
#
# Comment text is stripped before the search, so a doc comment that *quotes* an
# assertion does not make a test look as though it has one.
SCAN_AKW='
function lead(s,   i) { i = 1; while (i <= length(s) && substr(s, i, 1) == " ") i++; return i - 1 }
function code(s,   p) { p = index(s, "//"); return p ? substr(s, 1, p - 1) : s }
function asserts(s) { return s ~ /assert!|assert_eq!|assert_ne!|panic!|unreachable!|prop_assert/ }
function rtrim(s) { sub(/[ \t]*$/, "", s); return s }
function lastbrace(s,   i, p) { p = 0; for (i = 1; i <= length(s); i++) if (substr(s, i, 1) == "{") p = i; return p }
# Classify one line as the opening of a test body. Returns "" for "this line is
# not the opener", "{" for "a brace on this line opens a body that continues on
# the next line", and anything else for the text of a body that fits on this line.
#
# The opener is the brace at the END of the line, or - for a one-line body - the
# LAST brace on it. Both halves matter: a proptest generated argument is written
# as input in ".{0,60}", whose brace is inside a string literal and comes first,
# so a scanner that took the first brace ended the body before it started.
function opener(s,   t) {
  t = rtrim(s)
  if (t ~ /\{$/) return "{"
  if (t ~ /\}$/) { s = substr(s, 1, length(rtrim(substr(t, 1, length(t) - 1)))); return substr(s, lastbrace(s) + 1) }
  return ""
}
function finish() {
  if (fname != "") {
    scanned++
    if (!asserted) missing[++found] = FILENAME "\t" startline "\t" fname
  }
  fname = ""; asserted = 0; collecting = 0; want = 0; skipped = 0
}
FILENAME != lastfile { if (collecting || want) finish(); lastfile = FILENAME }
{
  line = $0
  # --- looking for the `fn` a test attribute belongs to ---------------------
  if (want == 1) {
    if (line !~ /^ *fn /) next
    sig = line
    sub(/^ *fn /, "", sig)
    fname = sig
    sub(/[ \t(<].*$/, "", fname)
    startline = FNR
    fnindent = lead(line)
    want = 0
    skipped = 0
  }
  if (fname != "") {
    # --- the opener: here, on a later line, or never ------------------------
    if (collecting) {
      if (line !~ /^ *\/\// && asserts(code(line))) asserted = 1
      if (lead(line) == fnindent && line ~ /^ *}/) finish()
      next
    }
    if (want == 2) {
      # A signature spread over lines, which is the shape a proptest generated
      # argument list takes. Give up rather than swallow the rest of the file.
      if (++skipped > 16 || line ~ /^ *#\[/) { finish(); next }
    }
    body = opener(line)
    if (body == "") {
      if (want == 0) want = 2
      next
    }
    if (body != "{") {                       # a body that fits on this line
      if (asserts(code(body))) asserted = 1
      finish()
      next
    }
    collecting = 1
    next
  }
  if (line ~ /^ *#\[(cfg_attr\([^]]*, *test\)|test)\] *$/) want = 1
}
END {
  if (collecting || want) finish()
  for (i = 1; i <= found; i++) print missing[i]
  printf "@@scanned %d missing %d\n", scanned, found
}
'

# run <root> <allowlist>: the real check. Prints one `ASSERTION-FREE:` line per
# test that asserts nothing and is not on the allow-list, a summary, and returns 1
# when there is at least one such test.
run() {
  local root="$1" allow="$2"
  local files out scanned missing allowed=0 finding key
  files=$(find "${root}" -name '*.rs' -not -path '*/target/*' 2>/dev/null | sort)
  if [ -z "${files}" ]; then
    echo "ASSERTION-SCAN: FAIL (no .rs files under ${root})"
    return 1
  fi
  out=$(printf '%s\n' "${files}" | xargs awk "${SCAN_AKW}")
  scanned=$(printf '%s\n' "${out}" | sed -n 's/^@@scanned \([0-9]*\).*/\1/p')
  missing=$(printf '%s\n' "${out}" | sed -n 's/^.* missing \([0-9]*\)$/\1/p')
  if [ -z "${scanned}" ] || [ -z "${missing}" ]; then
    echo "ASSERTION-SCAN: FAIL (the scanner did not run over ${root})"
    return 1
  fi
  if [ "${scanned}" -lt 1 ]; then
    echo "ASSERTION-SCAN: FAIL (the scanner matched no #[test] under ${root}, so it"
    echo "               checked nothing rather than finding nothing)"
    return 1
  fi

  while IFS= read -r finding; do
    [ -n "${finding}" ] || continue
    key="${finding%%	*}::${finding##*	}"
    if [ -n "${allow}" ] && printf '%s\n' "${allow}" | grep -q "^${key}|."; then
      allowed=$((allowed + 1))
    else
      printf 'ASSERTION-FREE: %s (body at line %s)\n' "${key}" "$(printf '%s' "${finding}" | cut -f2)"
    fi
  done <<<"$(printf '%s\n' "${out}" | grep -v '^@@scanned')"

  printf 'ASSERTIONS: %s test functions scanned, %s with no assertion macro, %s allowed\n' \
    "${scanned}" "${missing}" "${allowed}"
  [ "$((missing - allowed))" -eq 0 ]
}

# -----------------------------------------------------------------------------
# --self-test. The rule this repository holds every check to: a check that has
# never been observed to fail is not known to work, and this repository has seven
# recorded instances of a gate that could not report the truth.
if [ "${SELF_TEST}" = "1" ]; then
  TMP=$(mktemp -d)
  trap 'rm -rf "${TMP}"' EXIT
  fails=0

  # ok <label> <expected-status> <actual-status>
  ok() {
    if [ "$2" = "$3" ]; then
      printf '  ok:   %s\n' "$1"
    else
      printf '  FAIL: %s: expected status %s, got %s\n' "$1" "$2" "$3"
      fails=$((fails + 1))
    fi
  }
  # reports <label> <root> <allowlist> <grep-pattern> — the pattern must appear
  # in the check's own output.
  reports() {
    local out
    out=$(run "$2" "$3")
    if printf '%s\n' "${out}" | grep -q "$4"; then ok "$1" found found
    else printf '  FAIL: %s: %s is not in the output\n' "$1" "$4"; fails=$((fails + 1)); fi
  }

  mkdir -p "${TMP}/tree/src"
  cat >"${TMP}/tree/src/lib.rs" <<'RS'
#[cfg(test)]
mod tests {
    /// Asserts a value.
    #[test]
    fn asserts_a_value() {
        assert_eq!(2 + 2, 4);
    }

    /// The body calls the function and throws the answer away.
    #[test]
    fn asserts_nothing() {
        let _ = checks();
    }

    /// An empty body, the shape `fn f() {}` gives.
    #[test]
    fn empty_body() {}

    /// The assertion sits inside a nested block and a closure, so a scanner that
    /// stopped at the first inner brace would miss it.
    #[test]
    fn asserts_deep_inside() {
        if true {
            (|| {
                panic!("no");
            })();
        }
    }

    /// A quoted assertion in a doc comment is not an assertion.
    #[test]
    fn quotes_an_assertion_in_a_doc_comment() {
        /// this used to be `assert!(x >= 1)`, which is a tautology
        let _ = checks();
    }

    /// An attribute between `#[test]` and `fn` must not hide the function.
    #[cfg_attr(feature = "x", allow(dead_code))]
    #[test]
    fn behind_another_attribute() {
        unreachable!("unreachable");
    }

    fn checks() -> u32 {
        4
    }
}
RS

  run "${TMP}/tree" '' >/dev/null 2>&1; ok "a tree with finding-free tests still reports the others" 1 $?
  reports "an emptied test body is named"            "${TMP}/tree" '' 'src/lib.rs::asserts_nothing'
  reports "an empty body is named"                   "${TMP}/tree" '' 'src/lib.rs::empty_body'
  reports "a quoted assertion does not count"        "${TMP}/tree" '' 'src/lib.rs::quotes_an_assertion'
  OUT=$(run "${TMP}/tree" '' 2>/dev/null)
  # "present" is the failure here: a test that asserts something must NOT be
  # reported, so the expected value is `absent` in every one of these three.
  absent() { # absent <label> <needle>
    case "$OUT" in
      *"$2"*) ok "$1" absent present ;;
      *)      ok "$1" absent absent ;;
    esac
  }
  absent "a nested assertion is not reported"                    'src/lib.rs::asserts_deep_inside'
  absent "an assertion behind another attribute is not reported"  'src/lib.rs::behind_another_attribute'
  absent "a plain asserting test is not reported"                'src/lib.rs::asserts_a_value'
  case "${OUT}" in
    *'src/lib.rs::empty_body'*) ok "an empty body is reported" present present ;;
    *)                         ok "an empty body is reported" present absent ;;
  esac

  mkdir -p "${TMP}/clean/src"
  cat >"${TMP}/clean/src/lib.rs" <<'RS'
#[cfg(test)]
mod tests {
    #[test]
    fn the_only_test_asserts() {
        assert!(true, "with a reason");
    }
}
RS
  run "${TMP}/clean" '' >/dev/null 2>&1; ok "a clean tree passes" 0 $?
  mkdir -p "${TMP}/empty/src"; : >"${TMP}/empty/src/lib.rs"
  run "${TMP}/empty" '' >/dev/null 2>&1; ok "a tree with no tests is refused, not passed" 1 $?
  run "${TMP}/nowhere" '' >/dev/null 2>&1; ok "a missing root is refused, not passed" 1 $?

  # The allow-list, in both directions.
  cp "${TMP}/tree/src/lib.rs" "${TMP}/tree/src/other.rs"
  run "${TMP}/tree" "${TMP}/tree/src/lib.rs::asserts_nothing|the body delegates to checks()" >/dev/null 2>&1
  ok "an allow-listed function is still reported while its twin is not" 1 $?
  OUT=$(run "${TMP}/tree" "${TMP}/tree/src/lib.rs::asserts_nothing|delegates" 2>/dev/null)
  case "${OUT}" in *'src/other.rs::asserts_nothing'*) ok "the same function name in another file is still reported" yes yes ;;
                *) ok "the same function name in another file is still reported" yes no ;; esac
  run "${TMP}/tree" "${TMP}/tree/src/lib.rs::asserts_nothing" >/dev/null 2>&1
  ok "an allow-list row with no reason does not excuse anything" 1 $?

  if [ "${fails}" -eq 0 ]; then
    echo "SELF-TEST: PASS"
    exit 0
  fi
  echo "SELF-TEST: FAIL (${fails} case(s))"
  exit 1
fi

run "${ROOT}" "${ALLOWLIST}"
rc=$?
if [ "${rc}" -eq 0 ]; then
  echo "ASSERTION-SCAN: PASS"
else
  echo "ASSERTION-SCAN: FAIL — a test above asserts nothing. Give it a specific value"
  echo "  to assert, or move it to ALLOWLIST in this script with a one-line reason"
  echo "  saying which other test makes the claim instead."
fi
exit "${rc}"
