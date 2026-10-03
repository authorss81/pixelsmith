#!/usr/bin/env bash
# =============================================================================
# verify.sh — the gate that decides whether a phase counts as done.
#
# The phase agent never writes .done. This script's result does, via the
# workflow. A phase that compiles, passes its tests and has not broken any of the
# project's hard rules is verified; anything else gets another attempt.
#
# Checks are staged so that a Rust-only phase is not blocked by a missing Flutter
# toolchain, and an early phase is not blocked by a not-yet-existing app.
#
# Exit 0 = verified. Exit 1 = failed, phase will be retried.
# =============================================================================

set -uo pipefail

FAILURES=0
CHECKS=0

say()  { printf '%s\n' "$*"; }
head1() { printf '\n=== %s ===\n' "$*"; }

fail() {
  say "  FAIL: $*"
  FAILURES=$((FAILURES + 1))
}

pass() { say "  ok:   $*"; }

check() {
  CHECKS=$((CHECKS + 1))
  if eval "$2" >/dev/null 2>&1; then pass "$1"; else fail "$1"; fi
}

# -----------------------------------------------------------------------------
head1 "0. Environment"
say "  pwd:   $(pwd)"
say "  rustc: $(rustc --version 2>/dev/null || echo 'absent')"
say "  cargo: $(cargo --version 2>/dev/null | head -n1 || echo 'absent')"
say "  dart:  $(dart --version 2>&1 | head -n1 || echo 'absent')"

# -----------------------------------------------------------------------------
# Hard rule 1: no network capability in the engine. This is the product's core
# promise, so it is checked mechanically rather than trusted.
head1 "1. Hard rule — engine has no network capability"
NET_CRATES='^(reqwest|hyper|ureq|curl|isahc|isahc|surf|attohttpc|isahc|tokio|tokio-util|async-std|smol|quinn|h2|http|tungstenite|tokio-tungstenite|russh|ssh2|openssl|native-tls|rustls|webpki|trust-dns|hickory-dns|socket2|mio|libloading)-?[0-9.]* '
if [ -f core/Cargo.toml ]; then
  NET=$(cargo tree --manifest-path core/Cargo.toml --prefix none 2>/dev/null \
        | awk '{print $1}' | sort -u \
        | grep -E '^(reqwest|hyper|ureq|curl|isahc|surf|attohttpc|tokio|async-std|smol|quinn|h2|http|tungstenite|russh|ssh2|openssl|native-tls|rustls|webpki|trust-dns|hickory-dns|socket2|mio|dns-lookup)$' || true)
  if [ -n "${NET}" ]; then
    fail "network-capable crates in the engine dependency tree:"
    printf '        %s\n' ${NET}
  else
    pass "no network-capable crate in core/ dependency tree"
  fi

  if grep -rnE 'std::net|reqwest|hyper::|TcpStream|UdpSocket|lookup_host' core/src \
       --include='*.rs' >/dev/null 2>&1; then
    fail "core/src references a networking symbol:"
    grep -rnE 'std::net|reqwest|hyper::|TcpStream|UdpSocket|lookup_host' core/src --include='*.rs' | head -n 10
  else
    pass "no networking symbol referenced in core/src"
  fi
else
  say "  skip: core/Cargo.toml not present yet"
fi

# -----------------------------------------------------------------------------
head1 "2. Rust toolchain"
if [ -f core/Cargo.toml ]; then
  say "--- cargo fmt --check ---"
  if cargo fmt --manifest-path core/Cargo.toml --all -- --check 2>&1 | head -n 40; then
    pass "formatting clean"
  else
    fail "cargo fmt --check reported differences (see above)"
  fi

  say "--- cargo clippy ---"
  CLIPPY=$(cargo clippy --manifest-path core/Cargo.toml --all-targets --all-features -- -D warnings 2>&1)
  RC=$?
  if [ ${RC} -eq 0 ]; then
    pass "clippy clean with -D warnings"
  else
    fail "clippy found warnings treated as errors:"
    printf '%s\n' "${CLIPPY}" | grep -E '^(warning|error)' | head -n 30
  fi

  say "--- cargo test ---"
  TESTOUT=$(cargo test --manifest-path core/Cargo.toml --all-features 2>&1)
  if [ $? -eq 0 ]; then
    pass "$(printf '%s' "${TESTOUT}" | grep -E 'test result' | tr '\n' ' ')"
  else
    fail "cargo test failed:"
    printf '%s\n' "${TESTOUT}" | grep -E 'FAILED|panicked|test result|^error' | head -n 30
  fi

  say "--- cargo build --release ---"
  if cargo build --manifest-path core/Cargo.toml --release --all-features >/dev/null 2>&1; then
    pass "release build succeeds"
  else
    fail "cargo build --release failed"
  fi

  say "--- cargo doc ---"
  if RUSTDOCFLAGS="-D warnings" cargo doc --manifest-path core/Cargo.toml --no-deps --all-features >/dev/null 2>&1; then
    pass "rustdoc builds without warnings"
  else
    fail "cargo doc produced warnings (missing docs or broken links)"
  fi
else
  say "  skip: core/Cargo.toml not present yet"
fi

# -----------------------------------------------------------------------------
head1 "3. Dart / Flutter"
if [ -f app/pubspec.yaml ]; then
  if command -v flutter >/dev/null 2>&1; then
    say "--- flutter pub get ---"
    ( cd app && flutter pub get >/dev/null 2>&1 ) && pass "dependencies resolved" || fail "flutter pub get failed"

    say "--- dart format --set-exit-if-changed ---"
    if ( cd app && dart format --output=none --set-exit-if-changed lib test 2>&1 | tail -n 5 ); then
      pass "formatting clean"
    else
      fail "dart format reported differences (see above)"
    fi

    say "--- flutter analyze ---"
    ANALYZE=$( cd app && flutter analyze --no-pub 2>&1 )
    if [ $? -eq 0 ]; then
      pass "no analyzer issues"
    else
      fail "flutter analyze found issues:"
      printf '%s\n' "${ANALYZE}" | grep -E '^\s*(info|warning|error)' | head -n 30
    fi

    say "--- flutter test ---"
    FTOUT=$( cd app && flutter test --no-pub 2>&1 )
    if [ $? -eq 0 ]; then
      pass "$(printf '%s' "${FTOUT}" | grep -E 'All tests passed' | head -n1)"
    else
      fail "flutter test failed:"
      printf '%s\n' "${FTOUT}" | grep -E 'FAILED|Error|error|\+[0-9]+ -[0-9]+' | head -n 30
    fi
  else
    say "  skip: flutter not installed on this runner (toolchain step installs it)"
  fi
else
  say "  skip: app/pubspec.yaml not present yet"
fi

# -----------------------------------------------------------------------------
head1 "4. Repository hygiene"
check "no .done marker committed by an agent" \
  "grep -rq '^done\|touch .*\.done' workspace/*/PROMPT.md 2>/dev/null && exit 1 || exit 0"

check "no secret-looking strings in tracked source" \
  "! grep -rInE '(sk-[A-Za-z0-9]{20,}|ghp_[A-Za-z0-9]{20,}|api[_-]?key\"[[:space:]]*[:=][[:space:]]*\"[A-Za-z0-9]{20,})' core app scripts .github 2>/dev/null"

check "every phase directory has a PROMPT.md" \
  "for d in workspace/phase-*; do [ -f \"\$d/PROMPT.md\" ] || exit 1; done"

check "AGENTS.md exists" "[ -f AGENTS.md ]"
check "ROADMAP.md exists" "[ -f ROADMAP.md ]"
check "docs/phase-status.md exists" "[ -f docs/phase-status.md ]"
check "opencode.json declares the model" "grep -q '\"model\"' opencode.json"

# -----------------------------------------------------------------------------
head1 "5. Result"
say "  checks run: ${CHECKS}"
say "  failures:   ${FAILURES}"
if [ "${FAILURES}" -eq 0 ]; then
  say ""
  say "VERIFY: PASS"
  exit 0
fi
say ""
say "VERIFY: FAIL (${FAILURES} failing check(s))"
exit 1