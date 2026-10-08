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

# Dart files under the app/ submodule that `dart format` reports as unformatted
# and that this repository has no way to fix, because app/ is a separate
# repository (authorss81/shrinkray) pinned at one commit and pushed to with 403.
#
# Paths are relative to `app/`, which is what `dart format` prints. The list is
# deliberately file-by-file rather than "ignore app/": a new unformatted file
# upstream, or one this repository touches, still fails the gate. Delete an entry
# the moment it is formatted upstream — an allowlist nobody prunes becomes an
# allowlist that means nothing.
#
# Every entry is a formatting-only difference. `flutter analyze` and
# `flutter test` still gate on these files, so the Dart is still checked; only
# its line-wrapping is not checked here.
#
# lib/rust/engine.dart — committed upstream as 5d0e9cc ("ffi: fix the px_inspect
#   arity and declare px_abi_layout", 2026-10-04), which the superproject adopted
#   in 9435636. One method, `inspect`, is wrapped for the pre-3.7 formatter; the
#   3.12 SDK that app/pubspec.yaml pins wants it tall. Still unformatted at the
#   upstream tip e606f68, so bumping the submodule pointer does not clear it.
UNFIXABLE_UPSTREAM_APP="lib/rust/engine.dart"

say()  { printf '%s\n' "$*"; }
head1() { printf '\n=== %s ===\n' "$*"; }

fail() {
  say "  FAIL: $*"
  FAILURES=$((FAILURES + 1))
}

pass() { say "  ok:   $*"; }

check() {
  CHECKS=$((CHECKS + 1))
  # The eval'd command runs in a SUBSHELL, and that is load-bearing. Several
  # checks below are written `cmd && exit 1 || exit 0`, and a bare `exit` inside
  # `eval` terminates the whole script, not just the check. That made this gate
  # exit 0 at the very first hygiene check: it skipped every later check, never
  # reached the summary, and never printed "VERIFY: PASS" even while reporting
  # failures. A gate that cannot report its own result is worse than no gate,
  # because a reader sees exit 0 and stops looking.
  if ( eval "$2" ) >/dev/null 2>&1; then pass "$1"; else fail "$1"; fi
}

# -----------------------------------------------------------------------------
head1 "0. Environment"
say "  pwd:   $(pwd)"
say "  rustc: $(rustc --version 2>/dev/null || echo 'absent')"
say "  cargo: $(cargo --version 2>/dev/null | head -n1 || echo 'absent')"
say "  dart:  $(dart --version 2>/dev/null | head -n1 || echo 'absent')"

# -----------------------------------------------------------------------------
# Hard rule 1: no network capability in the engine. This is the product's core
# promise, so it is checked mechanically rather than trusted.
head1 "1. Hard rule — engine has no network capability"
# The banned list lives in the grep below, and the same list is duplicated in
# .github/workflows/supply-chain.yml. An earlier revision of this script also
# kept a `NET_CRATES` regex variable here — never referenced, with a trailing
# space that meant it could never have matched. A dead copy of a security
# policy reads exactly like the real one, so it is gone rather than maintained.
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
  # Same defect phase-01 fixed on the `dart format` line below, one check over:
  # `cargo fmt ... | head -n 40` as an `if` condition takes the exit status of
  # `head`, which is always 0, so this reported "formatting clean" over a tree
  # rustfmt was actively rewriting — and a phase committed a file rustfmt had
  # reformatted on the strength of it. Capture, then test the captured status.
  # `--color=never` because the ANSI escapes would defeat the eye as well.
  FMT=$(cargo fmt --manifest-path core/Cargo.toml --all -- --check --color=never 2>&1)
  FMT_RC=$?
  if [ ${FMT_RC} -eq 0 ]; then
    pass "formatting clean"
  else
    fail "cargo fmt --check reported differences:"
    printf '%s\n' "${FMT}" | head -n 40
  fi

  say "--- cargo clippy ---"
  # `--color=never` is not cosmetic. The diagnostic filter below matches
  # `^(warning|error)`, and cargo's default ANSI colouring puts an escape
  # sequence in front of both, so every one of those greps matched nothing and
  # a failing clippy run reported zero lines of reason.
  CLIPPY=$(cargo clippy --manifest-path core/Cargo.toml --all-targets --all-features \
    --color=never -- -D warnings 2>&1)
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
  if RUSTDOCFLAGS="-D warnings" cargo doc --manifest-path core/Cargo.toml --no-deps \
       --all-features --color=never >/dev/null 2>&1; then
    pass "rustdoc builds without warnings"
  else
    # Re-run without the output suppressed: this branch is almost always a
    # broken intra-doc link, and "cargo doc produced warnings" with no detail
    # is not something anybody can act on.
    fail "cargo doc produced warnings (missing docs or broken links)"
    RUSTDOCFLAGS="-D warnings" cargo doc --manifest-path core/Cargo.toml --no-deps \
      --all-features --color=never 2>&1 | grep -E '^(error|warning)' | head -n 20
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
    # Capture first, test after. `dart format ... | tail -n 5` as an `if`
    # condition takes the exit status of `tail`, which is always 0, so the
    # check could not fail no matter how badly formatted lib/ was.
    DFMT=$( cd app && dart format --output=none --set-exit-if-changed lib test 2>&1 )
    DRC=$?
    if [ ${DRC} -eq 0 ]; then
      pass "formatting clean"
    else
      # Separate the two reasons this can be red, because they need different
      # people to fix them and only one of them is this repository's job.
      #
      # `app/` is a git submodule: a separate repository (authorss81/shrinkray)
      # pinned at one commit, which this pipeline can read but cannot write —
      # pushing there returns 403 (workspace/phase-05/FINDINGS.md). Every Dart
      # file under app/ therefore belongs to that repository, not to this one.
      #
      # A file in UNFIXABLE_UPSTREAM_APP is one this repository cannot correct,
      # so it is reported as a note naming the file and the upstream command
      # rather than as a failure. Any OTHER file is still a hard failure: the
      # check is not scoped away, one file at a time, by name.
      #
      # This is the same treatment `check-dart-bindings.sh` gets in section 3b
      # below, and for the same reason: a gate that is red for a reason the
      # reader cannot act on stops being read.
      CHANGED_FILES=$(printf '%s\n' "${DFMT}" | sed -n 's/^Changed \(.*\)$/\1/p')
      FOREIGN=""
      for f in ${CHANGED_FILES}; do
        case " ${UNFIXABLE_UPSTREAM_APP} " in
          *" ${f} "*) FOREIGN="${FOREIGN} ${f}" ;;
          *) ;;
        esac
      done
      OURS=""
      for f in ${CHANGED_FILES}; do
        case " ${FOREIGN} " in
          *" ${f} "*) ;;
          *) OURS="${OURS} ${f}" ;;
        esac
      done

      if [ -n "${FOREIGN}" ]; then
        say "  note: dart format wants to rewrite ${FOREIGN}"
        say "        that file lives in the app/ submodule (authorss81/shrinkray), which"
        say "        this pipeline cannot push to. It is Dart 3.7's \"tall style\""
        say "        reformatting a file written before that style existed — the fix is"
        say "        upstream, in a commit only the app's owner can make:"
        say ""
        say "          cd app && dart format lib test && git commit -am 'dart format' && git push"
        say ""
        say "        Tracked in docs/phase-status.md under phase-08."
      fi
      if [ -n "${OURS}" ]; then
        fail "dart format reported differences in ${OURS} (see above)"
        printf '%s\n' "${DFMT}" | tail -n 5
      elif [ -z "${FOREIGN}" ]; then
        # Non-zero exit with no file we could attribute: report it rather than
        # let an unexplained red through as a note.
        fail "dart format failed without naming a file:"
        printf '%s\n' "${DFMT}" | tail -n 5
      fi
    fi

    say "--- flutter analyze ---"
    ANALYZE=$( cd app && flutter analyze --no-pub 2>&1 )
    if [ $? -eq 0 ]; then
      pass "no analyzer issues"
    else
      fail "flutter analyze found issues:"
      printf '%s\n' "${ANALYZE}" | grep -E '^\s*(info|warning|error)' | head -n 30
    fi

    say "--- native engine for the contract test ---"
    # The FFI tests skip themselves when the library cannot be loaded, and a
    # skipped test is green. So this gate used to pass while checking none of the
    # boundary at all: 13 passed, 4 skipped, and `px_inspect`'s arity mismatch
    # sat there unnoticed. Build the engine and put it on the loader path, then
    # insist that nothing skipped.
    #
    # DEBUG on purpose. `[profile.release]` sets `panic = "abort"`, so
    # `px_selftest_panic` aborts the test process instead of returning an error
    # buffer, and `flutter test` exits 1 while printing that every test passed.
    # That is the right trade for a shipped binary; it is the wrong one for a
    # test run, so the test runs against the debug library.
    ENGINE_READY=0
    ENGINE_DIR="$(pwd)/app/src/rust"
    if command -v cargo >/dev/null 2>&1 && [ -f core/Cargo.toml ]; then
      LIB=""
      case "$(uname -s)" in
        Darwin) LIB="core/target/debug/libpixelsmith_core.dylib" ;;
        *)      LIB="core/target/debug/libpixelsmith_core.so" ;;
      esac
      if cargo build --manifest-path core/Cargo.toml --lib >/dev/null 2>&1 \
         && [ -f "${LIB}" ]; then
        mkdir -p "${ENGINE_DIR}"
        cp "${LIB}" "${ENGINE_DIR}/"
        pass "debug engine built for the contract test"
        ENGINE_READY=1
      else
        fail "cargo build --lib failed, so the FFI contract cannot be checked"
      fi
    else
      say "  skip: cargo absent, so the FFI contract is not checked"
    fi

    say "--- flutter test ---"
    # `DynamicLibrary.open('libpixelsmith_core.so')` searches the loader path,
    # not the working directory, so the library has to be named absolutely or
    # the tests quietly skip themselves.
    if [ "${ENGINE_READY}" = "1" ]; then
      FTOUT=$( cd app && \
        LD_LIBRARY_PATH="${ENGINE_DIR}${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}" \
        DYLD_LIBRARY_PATH="${ENGINE_DIR}${DYLD_LIBRARY_PATH:+:${DYLD_LIBRARY_PATH}}" \
        flutter test --no-pub 2>&1 )
    else
      FTOUT=$( cd app && flutter test --no-pub 2>&1 )
    fi
    FRC=$?
    if [ ${FRC} -eq 0 ]; then
      pass "$(printf '%s' "${FTOUT}" | grep -E 'All tests passed|tests passed' | tail -n1)"
    else
      fail "flutter test failed:"
      printf '%s\n' "${FTOUT}" | grep -E 'FAILED|Error|error|\+[0-9]+ -[0-9]+' | head -n 30
    fi

    # A skip is not a pass. The contract test skips itself when the engine
    # cannot be loaded, which is exactly the case where the boundary is worth
    # nothing, so a skip has to be visible here rather than buried in the output.
    if [ "${ENGINE_READY}" = "1" ]; then
      SKIPPED=$(printf '%s' "${FTOUT}" | grep -c '(skipped)' || true)
      if [ "${SKIPPED}" -eq 0 ]; then
        pass "no test was skipped"
      else
        fail "${SKIPPED} test(s) skipped; the engine is loaded, so they should not be"
        printf '%s\n' "${FTOUT}" | grep '(skipped)' | head -n 10
      fi
    fi
  else
    say "  skip: flutter not installed on this runner (toolchain step installs it)"
  fi
else
  say "  skip: app/pubspec.yaml not present yet"
fi

# -----------------------------------------------------------------------------
head1 "3b. Dart bindings versus the engine ABI"
# Deliberately not a gate check. `app/` is a separate repository and the drift
# between them is recorded in workspace/phase-05/FINDINGS.md; a gate that is red
# for a reason the reader cannot act on stops being read. Run it by hand:
#   bash scripts/check-dart-bindings.sh
if [ -f app/lib/rust/bindings.dart ] && command -v cargo >/dev/null 2>&1; then
  if bash scripts/check-dart-bindings.sh >/dev/null 2>&1; then
    pass "app/lib/rust/bindings.dart matches the engine ABI"
  else
    say "  note: bindings drift is tracked in workspace/phase-05/FINDINGS.md"
    say "        (bash scripts/check-dart-bindings.sh lists it)"
  fi
fi

# -----------------------------------------------------------------------------
# Supply-chain policy, behind PX_DENY=1.
#
# Not part of the default gate, and the reason is a wall clock rather than a
# value judgement: `cargo deny check advisories` fetches the RustSec advisory
# database, which is minutes on a cold cache and is a network call in a
# repository whose whole thesis is that it needs no network. Everything else in
# this gate is offline and fast, and the gate runs on every phase.
#
# It is not optional in CI: .github/workflows/supply-chain.yml runs cargo-deny on
# every push, every pull request and weekly. This section is the same policy made
# runnable by a contributor, so that "CI would catch it" is a thing they can
# check rather than a thing they are told.
#
# PX_DENY=1 also runs --negative-control, which injects one violation at a time
# into copies of the real policy and asserts each is rejected. A gate that has
# never been observed to fail is not known to work.
head1 "4. Supply-chain policy (PX_DENY)"
if [ "${PX_DENY:-0}" = "1" ]; then
  DENY_LOG=$( bash scripts/deny-check.sh --negative-control 2>&1 )
  DENY_RC=$?
  if [ ${DENY_RC} -eq 0 ]; then
    # `PASS (offline subset only)` is a different claim from `PASS`, and saying
    # "ok:" for it would be the gate reporting more than it checked.
    if printf '%s' "${DENY_LOG}" | grep -q "RESULT: PASS (offline subset only"; then
      say "  note: cargo-deny is not installed, so advisories and licence"
      say "        confidence were NOT checked. The offline subset was:"
      printf '%s\n' "${DENY_LOG}" | grep -E '^\s+ok: |^PXDENY|controls fired' | sed 's/^/        /'
      say "        cargo install cargo-deny --locked, or rely on the CI job."
    else
      pass "cargo deny check, and 5 injected violations all rejected"
    fi
  else
    fail "supply-chain policy (exit ${DENY_RC}):"
    printf '%s\n' "${DENY_LOG}" | grep -vE '^\s*$' | tail -n 30 | sed 's/^/        /'
  fi
else
  say "  skip: PX_DENY=1 not set (CI runs cargo-deny unconditionally; see"
  say "        .github/workflows/supply-chain.yml). Run it with:"
  say "          PX_DENY=1 bash scripts/verify.sh"
fi

# -----------------------------------------------------------------------------
head1 "5. Repository hygiene"
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

# The supply-chain deliverables have to exist. They are cheap to delete by
# accident during a refactor and impossible to notice afterwards, because nothing
# in the build depends on a policy document.
check "docs/SECURITY.md and root SECURITY.md both exist" \
  "[ -f docs/SECURITY.md ] && [ -f SECURITY.md ] && grep -q 'docs/SECURITY.md' SECURITY.md"

check "docs/SUPPLY-CHAIN.md states a verdict per target" \
  "[ -f docs/SUPPLY-CHAIN.md ] && [ \"\$(grep -cE '^\\| \`?[a-z0-9_]+-[a-z0-9_]+' docs/SUPPLY-CHAIN.md)\" -ge 6 ]"

check "scripts/BUILD-SHA256.txt has a row per target" \
  "[ -f scripts/BUILD-SHA256.txt ] && [ \"\$(grep -cE '^[a-z0-9_]+-[a-z0-9_]+[[:space:]]' scripts/BUILD-SHA256.txt)\" -ge 6 ]"

check "vet/ audits present for the crates the advisory DB cannot answer for" \
  "[ -f vet/config.toml ] && grep -q 'libwebp-sys' vet/config.toml && grep -q 'heic-rs' vet/config.toml"

# -----------------------------------------------------------------------------
head1 "6. Result"
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