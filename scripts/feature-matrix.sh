#!/usr/bin/env bash
# =============================================================================
# feature-matrix.sh — compile every feature configuration that matters.
#
# WHY THIS EXISTS
#
# `scripts/verify.sh` runs `cargo test --all-features`, which compiles exactly
# one of the thirty-two configurations this crate has. Five `cfg`-gated modules,
# two gated test targets and one gated example sit behind those flags, and a
# build that has only ever been compiled with all five on cannot tell the
# difference between "the gate is on" and "nothing refers to it".
#
# That is not hypothetical. `docs/AUDIT.md` finding 2:
#
#   $ cargo check --manifest-path core/Cargo.toml --tests
#   error[E0433]: cannot find `stream` in `pixelsmith_core`   (tests/streaming_peak.rs)
#   $ cargo test --manifest-path core/Cargo.toml --no-run
#   error[E0601]: `main` function not found in crate `resize_bench`
#
# Both are a `cfg` on one side of a reference and not the other, and both were
# invisible for as long as the gate ran one configuration. `--no-default-features`
# had never compiled on this tree at all, which also means phase-07's note
# claiming the AVIF refusal arm "was additionally run under
# `--no-default-features`" describes a run that could not have happened.
#
# WHAT IT DOES, AND WHY IT COMPILES RATHER THAN RUNS
#
# `cargo check --lib --bins --tests --examples`: exactly the target set `cargo
# test` compiles, which is the claim this script exists to make. Compiling is
# what catches this class — a gate, a missing `main` and a missing import are all
# decided at compile time — and the gate already spends five minutes in
# `cargo test`. Running the suite nine times would add forty minutes to every
# push to re-answer a question compilation answers. The consequence is stated
# rather than hidden: in the configurations other than `--all-features` this
# script proves the *tests compile*, not that they pass. `docs/ARCHITECTURE.md`
# records the same split.
#
# `--all-targets` would also compile `core/benches/`, and the target list is
# spelled out instead of using it because the benches are not part of the claim:
# `cargo test` does not build them, `cargo bench` does, and criterion is a
# per-configuration compile this check does not need to pay. The benches were
# checked in all thirty-two combinations by hand; see the phase-19 notes in
# `docs/phase-status.md`.
#
# The configuration list below is the thing that rots, so `--self-test` checks it
# against `core/Cargo.toml`'s own `[features]` table: adding a sixth feature
# without a row for it fails the self-test and names the feature. A matrix nobody
# maintains is how this happened once already.
#
# Usage:
#   bash scripts/feature-matrix.sh              compile every configuration
#   bash scripts/feature-matrix.sh --list       print the configurations, compile none
#   bash scripts/feature-matrix.sh --self-test  prove the checks can fail (offline)
#
# Exit 0 = every configuration compiles. 1 = at least one does not. 2 = bad
# arguments.
# =============================================================================

set -uo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

SELF_TEST=0
LIST_ONLY=0
# Self-test hooks. PX_MATRIX_NO_CARGO skips the compiler entirely; PX_FAKE_FAIL
# names configurations to report as failing. Neither can make the gate greener:
# both only ever make it redder, and both are ignored unless --self-test is given.
NO_CARGO=0
FAKE_FAIL=""

die() { printf 'feature-matrix.sh: %s\n' "$*" >&2; exit 2; }
say()  { printf '%s\n' "$*"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --self-test)  SELF_TEST=1; shift ;;
    --list)       LIST_ONLY=1; shift ;;
    -h|--help)    awk 'NR>1 && /^set -uo/{exit} NR>1{print}' "${BASH_SOURCE[0]}"; exit 0 ;;
    *)            die "unknown argument: $1" ;;
  esac
done

if [ "${SELF_TEST}" = "1" ]; then
  NO_CARGO=1
  FAKE_FAIL="${PX_FAKE_FAIL:-}"
fi

# -----------------------------------------------------------------------------
# The configurations.
#
# `label|cargo arguments`. Nine rows, and the reasoning for each is in the comment
# above it, because a row without a reason is a row somebody deletes to save
# thirty seconds.
#
# The three canonical builds come first, because they are the three a reader
# expects a matrix to carry and the three this file has to be able to say it
# checked: what ships, nothing optional, and everything at once. Then one row per
# feature, then the two that exist because two gates together are a different
# tree from either alone.
CONFIGS=(
  # What ships: avif + webp-lossy, and neither of the two opt-in paths.
  "default|"
  # No optional codec at all. This is the build with no C toolchain, and the one
  # configuration in which the AVIF and lossy-WebP *refusal* arms are the ones
  # that run — the arms `format::tests` says must agree with `capabilities()`.
  "no-optional-codec|--no-default-features"
  # Each feature alone with the default set off. Adding `avif` to a build that
  # already has it is the same tree; removing `webp-lossy` from a build that has
  # `heic` is not the same tree as a build with neither, and the `#[cfg]` arms in
  # format.rs are written per feature rather than as a group.
  "avif-only|--no-default-features --features avif"
  "webp-lossy-only|--no-default-features --features webp-lossy"
  "heic-only|--no-default-features --features heic"
  # The default set plus one opt-in path each, which is how a contributor
  # actually reaches for them: `cargo test --features streaming`.
  "streaming|--features streaming"
  "simd|--features simd"
  # Both opt-in paths and no optional codec: the smallest tree that still
  # compiles the gated example and the gated test binary. The two gates together
  # are what makes this its own row — `streaming_peak.rs` and `resize_bench.rs`
  # are the two targets that were broken, and this is the configuration in which
  # both are simultaneously present.
  "streaming-and-simd-only|--no-default-features --features streaming,simd"
  # What `verify.sh`'s own clippy and test runs use. Checked here so this script
  # stands alone: run by hand on a tree where the gate has never run, it is the
  # complete answer.
  "all-features|--all-features"
)

# The three builds the list must carry whatever else changes.
REQUIRED_BUILDS=("default|" "no-optional-codec|--no-default-features" "all-features|--all-features")

# -----------------------------------------------------------------------------
# Feature names, read out of core/Cargo.toml's own [features] table rather than
# repeated here. A list of features in two places is a list that rots in one of
# them, and this one rots silently: a sixth feature with no matrix row is a
# configuration nothing has ever compiled, which is the defect this script exists
# to prevent.
#
# The `default` key is excluded because it is a feature *set*, not a flag; the
# first row already covers it.
declared_features() {
  awk '
    /^\[/ { in_features = ($0 ~ /^\[features\][[:space:]]*($|#)/); next }
    in_features && /^[A-Za-z0-9_-]+[[:space:]]*=/ {
      name = $1
      sub(/[[:space:]]*=$/, "", name)
      if (name != "default" && name != "") print name
    }
  ' "${1}/core/Cargo.toml" 2>/dev/null
}

has_config() {
  local want="$1" entry
  for entry in "${CONFIGS[@]}"; do
    [ "${entry%%|*}" = "${want}" ] && return 0
  done
  return 1
}

# True when some configuration enables `feature` on its own — an exact match on
# the comma-separated `--features` list rather than a substring, because a
# substring test would call `simd` covered by a row that only enables
# `streaming,simd` under some future rename and miss the row that enables `simd`
# by itself under another.
row_enables() {
  local feature="$1" entry args list one
  for entry in "${CONFIGS[@]}"; do
    args="${entry#*|}"
    case "${args}" in
      *"--features"*) list="${args#*--features }" ;;
      *) continue ;;
    esac
    for one in ${list//,/ }; do
      [ "${one}" = "${feature}" ] && return 0
    done
  done
  return 1
}

in_fake_fail() {
  local want="$1" one
  [ -n "${FAKE_FAIL}" ] || return 1
  for one in ${FAKE_FAIL//,/ }; do
    [ "${one}" = "${want}" ] && return 0
  done
  return 1
}

# -----------------------------------------------------------------------------
# The run. Written as a function so --self-test can call it with a configuration
# forced to fail, which is the only way to know the loop reports failure rather
# than printing "ok" over a broken tree — the defect class phase-01's check() and
# phase-08's `| head -n 40` both were.
run_matrix() {
  local failures=0 checked=0 entry label args out
  for entry in "${CONFIGS[@]}"; do
    label="${entry%%|*}"
    args="${entry#*|}"
    checked=$((checked + 1))
    if [ "${NO_CARGO}" = "1" ] && in_fake_fail "${label}"; then
      say "  FAIL: ${label}  (${args:-no flags})"
      failures=$((failures + 1))
      continue
    fi
    if [ "${NO_CARGO}" = "1" ]; then
      say "  ok:   ${label}  (${args:-no flags})"
      continue
    fi
    if out=$(cd "${REPO}" && cargo check --manifest-path core/Cargo.toml \
        --lib --bins --tests --examples --color=never ${args} 2>&1); then
      say "  ok:   ${label}  (${args:-no flags})"
    else
      say "  FAIL: ${label}  (${args:-no flags})"
      # The reason, filtered. `--color=never` because cargo's default colouring
      # puts an escape sequence in front of `error` and the filter then matches
      # nothing — the same defect phase-01 fixed twice in verify.sh.
      printf '%s\n' "${out}" | grep -E '^(error|warning: unused)' | head -n 12 | sed 's/^/        /'
      failures=$((failures + 1))
    fi
  done
  say ""
  say "MATRIX: ${checked} configurations checked, ${failures} failed"
  [ "${failures}" -eq 0 ] || return 1
  return 0
}

# -----------------------------------------------------------------------------
# --self-test: the matrix proved able to report the truth, offline and in both
# directions. verify.sh runs it on every push (section 2b).
self_test() {
  local failures=0 feature label out rc want_count

  say "feature-matrix.sh --self-test"

  # 1. Every feature core/Cargo.toml declares has a row that enables it. This is
  #    the check that fails when somebody adds a feature and forgets the matrix,
  #    and it names the feature rather than reporting a count.
  while read -r feature; do
    [ -n "${feature}" ] || continue
    if row_enables "${feature}"; then
      say "  ok:   feature '${feature}' has a matrix row"
    else
      say "  FAIL: feature '${feature}' is declared in core/Cargo.toml and no configuration enables it"
      failures=$((failures + 1))
    fi
  done <<EOF
$(declared_features "${REPO}")
EOF

  # 2. The three canonical builds are present, whatever else changes.
  for want in "${REQUIRED_BUILDS[@]}"; do
    label="${want%%|*}"
    if has_config "${label}"; then
      say "  ok:   build '${label}' is in the matrix"
    else
      say "  FAIL: build '${label}' is missing from the matrix"
      failures=$((failures + 1))
    fi
  done

  # 3. The list is longer than the three builds: a matrix of three is the one
  #    configuration the gate already ran, which is the defect rather than the
  #    cure. The floor is derived — one row per declared feature plus the three
  #    builds — so it moves when a feature is added rather than being a number
  #    somebody has to remember to raise.
  want_count=$(($(declared_features "${REPO}" | wc -l) + ${#REQUIRED_BUILDS[@]}))
  if [ "${#CONFIGS[@]}" -ge "${want_count}" ]; then
    say "  ok:   ${#CONFIGS[@]} configurations, at least the ${want_count} the floor requires"
  else
    say "  FAIL: ${#CONFIGS[@]} configurations, below the floor of ${want_count}"
    failures=$((failures + 1))
  fi

  # 4. A passing run reports every configuration and exits 0. Measured against
  #    the summary line rather than the exit code alone, because that is the line
  #    verify.sh shows the reader.
  out=$(run_matrix 2>&1)
  rc=$?
  if [ ${rc} -eq 0 ] && printf '%s\n' "${out}" | grep -q "^MATRIX: ${#CONFIGS[@]} configurations checked, 0 failed"; then
    say "  ok:   a clean run checks all ${#CONFIGS[@]} configurations and exits 0"
  else
    say "  FAIL: a clean run did not report ${#CONFIGS[@]} configurations and exit 0:"
    printf '%s\n' "${out}" | tail -n 4 | sed 's/^/        /'
    failures=$((failures + 1))
  fi

  # 5. The other direction, which is the one that matters: a configuration that
  #    does not compile is named, counted in the summary, and the script exits
  #    non-zero. `no-optional-codec` is the configuration that had never been
  #    compiled on this tree, so it is the one to break.
  #
  #    All three assertions are needed. An exit code without the name is a red
  #    gate nobody can act on; a name without the count is a report that could
  #    still be read as "the other eight were fine" without saying how many
  #    there were; and a count is only a fact if the loop that produced it can
  #    also produce a different one.
  FAKE_FAIL="no-optional-codec"
  out=$(run_matrix 2>&1)
  rc=$?
  FAKE_FAIL=""
  if [ ${rc} -ne 0 ]; then
    say "  ok:   a broken configuration makes the run exit non-zero"
  else
    say "  FAIL: a broken configuration was reported as a pass"
    failures=$((failures + 1))
  fi
  if printf '%s\n' "${out}" | grep -q 'FAIL: no-optional-codec'; then
    say "  ok:   the broken configuration is named"
  else
    say "  FAIL: the run exited non-zero without naming the broken configuration"
    failures=$((failures + 1))
  fi
  if printf '%s\n' "${out}" | grep -q "^MATRIX: ${#CONFIGS[@]} configurations checked, 1 failed"; then
    say "  ok:   the summary line counts the failure against the total"
  else
    say "  FAIL: the summary line did not count the failure:"
    printf '%s\n' "${out}" | grep '^MATRIX:' | sed 's/^/        /'
    failures=$((failures + 1))
  fi

  # 6. Two broken configurations are counted as two, not as one — the loop has to
  #    keep going after a failure, or "the first one" is all the gate ever learns.
  FAKE_FAIL="no-optional-codec,simd"
  out=$(run_matrix 2>&1)
  rc=$?
  FAKE_FAIL=""
  if [ ${rc} -ne 0 ] && printf '%s\n' "${out}" | grep -q "^MATRIX: ${#CONFIGS[@]} configurations checked, 2 failed"; then
    say "  ok:   two broken configurations are counted as two"
  else
    say "  FAIL: two broken configurations did not report two failures:"
    printf '%s\n' "${out}" | grep '^MATRIX:' | sed 's/^/        /'
    failures=$((failures + 1))
  fi

  say ""
  if [ "${failures}" -eq 0 ]; then
    say "feature-matrix.sh --self-test: PASS"
    return 0
  fi
  say "feature-matrix.sh --self-test: FAIL (${failures})"
  return 1
}

if [ ! -f "${REPO}/core/Cargo.toml" ]; then
  die "core/Cargo.toml not found at ${REPO} — run this from a clone"
fi

if [ "${SELF_TEST}" = "1" ]; then
  self_test
  exit $?
fi

if [ "${LIST_ONLY}" = "1" ]; then
  for entry in "${CONFIGS[@]}"; do
    printf '%s\t%s\n' "${entry%%|*}" "${entry#*|}"
  done
  exit 0
fi

run_matrix
exit $?
