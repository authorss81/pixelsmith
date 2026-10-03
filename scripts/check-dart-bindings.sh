#!/usr/bin/env bash
# =============================================================================
# check-dart-bindings.sh — does the Flutter app still match the engine's C ABI?
#
# `app/lib/rust/bindings.dart` is hand-written, because `dart:ffi` has no
# generator and adding one would put a code-generation step between the app's
# pubspec and the engine. Hand-written bindings drift silently: `px_inspect`
# takes three arguments and the Dart declaration passed two, which is a call
# across the FFI boundary with the wrong number of arguments and nothing in
# either repository complained.
#
# This is the check that would have caught it. It is deliberately NOT part of
# `scripts/verify.sh`: the app/ submodule is a separate repository, and this
# script currently fails because that repository needs a fix this one cannot
# make (see workspace/phase-05/FINDINGS.md). A gate that is red for a reason
# nobody can act on gets ignored, and then it catches nothing at all.
#
# Usage:
#   bash scripts/check-dart-bindings.sh                  # check the checked-in app/
#   bash scripts/check-dart-bindings.sh <bindings.dart>  # check another checkout
#
# Exit 0 = in sync. Exit 1 = drift. Exit 2 = could not check.
# =============================================================================

set -uo pipefail

BINDINGS="${1:-app/lib/rust/bindings.dart}"

if [ ! -f core/Cargo.toml ]; then
  echo "error: core/Cargo.toml not found; run this from the repository root" >&2
  exit 2
fi

if [ ! -f "${BINDINGS}" ]; then
  echo "error: ${BINDINGS} not found." >&2
  echo "hint: git submodule update --init --recursive" >&2
  exit 2
fi

cargo run --quiet --manifest-path core/Cargo.toml --bin px-abi-dump -- --check "${BINDINGS}"
rc=$?

if [ ${rc} -eq 0 ]; then
  echo "DART BINDINGS: in sync with the engine ABI"
elif [ ${rc} -eq 1 ]; then
  echo ""
  echo "The expected declarations are printed by:" >&2
  echo "  cargo run --bin px-abi-dump" >&2
  echo "and the reasoning for each outstanding item is in" >&2
  echo "  workspace/phase-05/FINDINGS.md" >&2
fi
exit ${rc}