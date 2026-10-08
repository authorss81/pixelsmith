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
# Every feature configuration compiles.
#
# Section 2 runs `--all-features`, which is ONE of the nine configurations this
# crate has. That hid a real defect for the whole of the project's life:
#
#   error[E0433]: cannot find `stream` in `pixelsmith_core`   (tests/streaming_peak.rs)
#   error[E0601]: `main` function not found in crate `resize_bench`
#
# so `cd core && cargo test` — the command README.md tells a contributor to run —
# did not compile, and `--no-default-features` had never been compiled at all.
# docs/AUDIT.md finding 2; closed in phase-19.
#
# The check is `cargo check` over the target set `cargo test` compiles, because
# compiling is what catches this class and the gate already spends five minutes
# running the suite. What it buys is stated rather than implied: in the
# configurations other than `--all-features`, this proves the tests *compile*, not
# that they pass. See scripts/feature-matrix.sh for the configuration list and
# docs/phase-status.md under phase-19 for the wall clock.
head1 "2b. Feature matrix — every configuration compiles"
if [ -f core/Cargo.toml ]; then
  say "--- scripts/feature-matrix.sh ---"
  # Exit status first, summary line only for the reader. A check that printed the
  # summary line as its condition could report "9 configurations checked" over a
  # run that failed — the `cmd | head` defect phase-01 fixed twice in this file.
  MATRIX=$( bash scripts/feature-matrix.sh 2>&1 )
  MRC=$?
  if [ ${MRC} -eq 0 ]; then
    pass "$(printf '%s' "${MATRIX}" | grep -E '^MATRIX:' | tail -n 1)"
  else
    fail "a feature configuration does not compile:"
    # The per-configuration lines are indented by two spaces, so the filter has to
    # allow for that: a red gate that reports three compiler errors without naming
    # the build that produced them is hard rule 9's failure mode in a gate.
    printf '%s\n' "${MATRIX}" | grep -E '^(  FAIL|MATRIX|        )' | head -n 30
  fi

  # Proved able to fail, offline, in both directions — including the check that
  # fails when a feature is added to core/Cargo.toml without a row for it here.
  check "scripts/feature-matrix.sh --self-test" \
    "bash scripts/feature-matrix.sh --self-test"

  say "--- the codec refusal arms, executed without the default features ---"
  # Compiling is not running, and in one configuration the difference is the whole
  # point: `--no-default-features` is the only build in which the AVIF and lossy
  # WebP *refusal* arms are the ones that execute, so it is the configuration in
  # which `capabilities()` and `format::encode` can be caught disagreeing. phase-07
  # claimed that arm "was additionally run under --no-default-features"; on the
  # tree as it stood that run could not have happened, because nothing in that
  # configuration compiled. This is that run.
  #
  # One filter, not the whole lib suite: the suite's wall clock is
  # `presets::tests` (41 Lanczos3 resamples of a 12 MP image, about four minutes
  # in this debug profile — docs/ARCHITECTURE.md gotcha 20) and it is the same
  # arithmetic in every configuration. `format::tests` is 23 tests and 0.6 s, and
  # it is where the capability/refusal agreement lives.
  NOTEST=$( cargo test --manifest-path core/Cargo.toml --no-default-features \
    --lib format::tests --color=never 2>&1 )
  NRC=$?
  if [ ${NRC} -eq 0 ]; then
    pass "$(printf '%s' "${NOTEST}" | grep -E 'test result' | tail -n 1)"
  else
    fail "cargo test --no-default-features (format tests) failed:"
    printf '%s\n' "${NOTEST}" | grep -E 'FAILED|panicked|^error|test result' | head -n 20
  fi
else
  say "  skip: core/Cargo.toml not present yet"
fi

# -----------------------------------------------------------------------------
# The JSON contract: does the app still model what the engine emits?
#
# This is a FAILURE where section 3b's C-ABI check is a note, and the difference
# is the whole reason it can be. `scripts/check-dart-bindings.sh` compares two
# things this repository does not own — `ffi.rs` here, `bindings.dart` over
# there in the `authorss81/shrinkray` submodule — so a red result has no fix
# available to whoever reads it, and a check like that stops being read.
#
# `scripts/check-json-contract.sh` compares the engine's own serde field names,
# generated at run time, against `core/contract/json-fields.txt` — which is in
# THIS repository and is therefore landable. So the answer to "has the contract
# drifted?" is a fact about this repository, a red result means somebody has to
# add a row and write the Dart member, and five phases of silent drift could not
# have accumulated behind a check that failed when it should.
#
# `docs/AUDIT.md` findings 3, 4 and 5 are the drift this closes: an app that
# crashed on an iPhone photograph, a batch report that counted skips as
# successes, and five phases of fields the Dart models had never heard of.
head1 "2c. JSON contract — every engine field has a row, and every row a Dart member"
if [ -f core/Cargo.toml ]; then
  say "--- scripts/check-json-contract.sh ---"
  # Capture first, test after: `cmd | head` as an `if` condition takes the exit
  # status of `head`, which is always 0 — the eighth instance of that defect in
  # this file, and the reason the failure branch prints the script's own words
  # rather than a bare status.
  CONTRACT=$( bash scripts/check-json-contract.sh 2>&1 )
  CRC=$?
  if [ ${CRC} -eq 0 ]; then
    pass "$(printf '%s' "${CONTRACT}" | grep -E '^JSON CONTRACT:' | tail -n 1)"
  else
    fail "the engine emits JSON the committed contract file does not record:"
    printf '%s\n' "${CONTRACT}" | head -n 40
  fi

  # Proved able to fail, in four directions and offline, including the positive
  # control: a drift check that fails on everything catches drift and also
  # catches nothing else.
  check "scripts/check-json-contract.sh --self-test" \
    "bash scripts/check-json-contract.sh --self-test"
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
# Deliberately not a gate check, and deliberately kept rather than deleted:
# `app/` is a separate repository and the drift between them is recorded in
# workspace/phase-05/FINDINGS.md; a gate that is red for a reason the reader
# cannot act on stops being read. Run it by hand:
#   bash scripts/check-dart-bindings.sh
#
# The JSON half of the boundary *is* a gate check, and section 2c is where it
# lives. The difference is not leniency: it is that check-json-contract.sh's
# committed side (`core/contract/json-fields.txt`) is in this repository, so a
# red result here has an action available to the person reading it. That is the
# whole mechanism phase-20 introduced, and it is why the C-ABI note stayed a note
# while the JSON contract became a failure. Run it by hand for the detail:
#   bash scripts/check-json-contract.sh
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

# A row is a target triple followed by whitespace, and the triple pattern has to
# allow more than one hyphen: `^[a-z0-9_]+-[a-z0-9_]+[[:space:]]` cannot match
# `x86_64-unknown-linux-gnu`, because anchored at `^` the first class stops at the
# first hyphen and the second one cannot cross the second. That regex was in this
# file for two attempts of phase-15 and both failed this check against a table
# that was correct — the same class of defect as phase-01's `check()` and
# phase-08's `| head -n 40`: a check that cannot report the truth. The grouped
# form below is the one that matches a triple, and both directions are asserted by
# the hand test in docs/SUPPLY-CHAIN.md.
TRIPLE='[a-z0-9_]+(-[a-z0-9_]+)+'

check "docs/SUPPLY-CHAIN.md states a verdict per target" \
  "[ -f docs/SUPPLY-CHAIN.md ] && [ \"\$(grep -cE '^\\| \`?${TRIPLE}' docs/SUPPLY-CHAIN.md)\" -ge 6 ]"

check "scripts/BUILD-SHA256.txt has a row per target" \
  "[ -f scripts/BUILD-SHA256.txt ] && [ \"\$(grep -cE '^${TRIPLE}[[:space:]]' scripts/BUILD-SHA256.txt)\" -ge 6 ]"

check "vet/ audits present for the crates the advisory DB cannot answer for" \
  "[ -f vet/config.toml ] && grep -q 'libwebp-sys' vet/config.toml && grep -q 'heic-rs' vet/config.toml"

# -----------------------------------------------------------------------------
# Release identity and release secrets.
#
# These are hygiene checks in the same sense the rest of section 5 is: nothing in
# the build depends on them, they are cheap to get wrong by accident, and both
# mistakes are silent until a user is holding the file.

# One version, in core/Cargo.toml, app/pubspec.yaml and CHANGELOG.md. Three
# places to update is three places to forget, and px_version() reporting 0.1.0
# next to a store listing that says 0.2.0 is a bug nobody notices until a bug
# report quotes the wrong one.
check "core/Cargo.toml, app/pubspec.yaml and CHANGELOG.md agree on the version" \
  "bash scripts/check-version.sh"

# No signing material in the tree, ever. .gitignore lists the extensions, and a
# gitignore is a request rather than a guarantee — this is the guarantee. The
# release key is the one secret this project genuinely cannot lose, and a
# committed one cannot be un-committed from every clone already made.
check "no keystore, .jks or key.properties is tracked" \
  "! git ls-files | grep -Ei '\\.(jks|keystore)$|(^|/)key\\.properties$'"

# A changelog whose newest entry is not the shipped version is a release nobody
# wrote down, and the section that makes this one worth reading is the one about
# what is still missing.
check "CHANGELOG.md names this version and its gaps" \
  "[ -f CHANGELOG.md ] && grep -qE '^## +\\[?[0-9]+\\.[0-9]+\\.[0-9]+' CHANGELOG.md && grep -q 'Still missing' CHANGELOG.md"

check "docs/RELEASE.md exists and says which secrets make a Play build" \
  "[ -f docs/RELEASE.md ] && grep -q 'ANDROID_KEYSTORE_BASE64' docs/RELEASE.md && grep -q 'SHA-256' docs/RELEASE.md"

# One row per artefact, including the ones that were not built. `grep -c` counts
# lines, so this is the number of rows and not something else.
check "scripts/RELEASE-SHA256.txt has a row per release artefact" \
  "[ -f scripts/RELEASE-SHA256.txt ] && [ \"\$(grep -cE '^pixelsmith-[0-9]+\\.[0-9]+\\.[0-9]+-' scripts/RELEASE-SHA256.txt)\" -ge 3 ]"

# -----------------------------------------------------------------------------
# The release tooling, proved able to fail.
#
# Every check a phase adds into this file gets run with a passing input before
# it is believed. This repository has seven recorded instances of a check that
# could not report the truth: phase-01's `check()`, the `dart format … | tail -n 5`
# exit status, the `cargo fmt … | head -n 40` exit status, and the clippy/rustdoc
# filter that ANSI colouring defeated; phase-10's `--test`; and phase-15's
# target-triple regex and its `spdx_licenses` typo. Two of them cost a phase whose
# actual work was already complete.
#
# The three scripts below are the ones phase-16 adds, so each carries its own
# `--self-test` and each is exercised here rather than trusted.
#
# phase-19 adds a fourth: `scripts/feature-matrix.sh`, whose `--self-test` runs a
# deliberately failing configuration in both directions and fails if a feature is
# declared in core/Cargo.toml with no row for it. Section 2b runs it in place of
# section 5b, because that is where the matrix itself runs.
head1 "5b. Release tooling can report failure"
for tool in check-version verify-release-artifact release-checksums; do
  check "scripts/${tool}.sh --self-test" "bash scripts/${tool}.sh --self-test"
done

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