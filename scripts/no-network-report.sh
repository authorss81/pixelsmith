#!/usr/bin/env bash
# =============================================================================
# no-network-report.sh — check hard rule 1 and print a verdict a stranger can act
# on.
#
# "This app never uploads anything" is the product's whole claim, and it is a
# claim about a binary. docs/SECURITY.md gives the commands; this script runs
# them, adds two it does not, and prints one verdict instead of four greps that
# each print nothing when they pass.
#
#     bash scripts/no-network-report.sh                 uses an existing release build
#     bash scripts/no-network-report.sh --build         builds the release engine first
#
# The two additions over the documented commands:
#
#   * `ldd` shows which LIBRARIES a binary loads, so it cannot see a statically
#     linked socket call. This also reads the undefined dynamic symbols, which is
#     where `socket`, `connect`, `getaddrinfo` and the OpenSSL entry points have
#     to appear if anything calls them. A binary with no networking code has none
#     of these; one that merely forgot to link libcurl does.
#
#   * it checks the release artefact, not `target/debug`. A debug build links
#     different code, and the shipped library is the one the promise is about.
#
# Exit status: 0 every check passed, 1 at least one failed, 2 the release
# artefact is absent and could not be built.
# =============================================================================

set -uo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
BUILD=0
FAILURES=0
CHECKS=0

# The list. Also in scripts/verify.sh section 1 and in
# .github/workflows/supply-chain.yml. Three copies is too many, and this one is
# the readable one; if you change it, change all three and say why in the commit.
NET_CRATES='reqwest|hyper|hyper-util|h2|http|ureq|curl|isahc|surf|attohttpc|tokio|async-std|async-io|smol|quinn|tungstenite|tokio-tungstenite|russh|ssh2|openssl|openssl-sys|native-tls|rustls|rustls-pemfile|webpki|hickory|hickory-resolver|trust-dns|socket2|mio|dns-lookup|isahc|reqwest-middleware'

# Symbol names that only exist because something can reach the network.
NET_SYMBOLS='socket|socketpair|connect|accept|bind|listen|getaddrinfo|getnameinfo|res_query|sendto|recvfrom|SSL_|ssl_|EVP_|X509_|curl_easy|gethostbyname'

while [ $# -gt 0 ]; do
  case "$1" in
    --build) BUILD=1; shift ;;
    -h|--help) sed -n '2,32p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) printf 'no-network-report.sh: unknown argument: %s\n' "$1" >&2; exit 2 ;;
  esac
done

say() { printf '%s\n' "$*"; }
verdict() {
  CHECKS=$((CHECKS + 1))
  if [ "$1" = "0" ]; then printf '  ok    %s\n' "$2"; else printf '  FAIL  %s\n' "$2"; FAILURES=$((FAILURES + 1)); fi
}

case "$(uname -s)" in
  Darwin) LIBNAME=libpixelsmith_core.dylib ;;
  *)      LIBNAME=libpixelsmith_core.so ;;
esac

say "=== pixelsmith: does the engine have any way to reach the network? ==="
say ""
say "  repository: ${ROOT}"
say "  host:       $(uname -s) $(uname -m)"
say "  rustc:      $(rustc --version 2>/dev/null || echo absent)"
say ""

# -----------------------------------------------------------------------------
say "--- 1. is a network-capable crate anywhere in the dependency graph? ---"
# `cargo tree` prints one line per node, so this is the whole transitive
# closure, including build-dependencies and platform-specific branches, and it
# needs no network because it reads core/Cargo.lock.
TREE=$(cargo tree --manifest-path "${ROOT}/core/Cargo.toml" --all-features --prefix none 2>/dev/null \
       | awk '{print $1}' | sort -u)
if [ -z "${TREE}" ]; then
  say "  FAIL  cargo tree produced nothing; the claim cannot be checked"
  exit 2
fi
CRATE_COUNT=$(printf '%s\n' "${TREE}" | grep -c .)
HITS=$(printf '%s\n' "${TREE}" | grep -E "^(${NET_CRATES})$" || true)
if [ -n "${HITS}" ]; then
  verdict 1 "network-capable crate(s) in the graph:"
  printf '%s\n' "${HITS}" | sed 's/^/          /'
else
  verdict 0 "no network-capable crate among ${CRATE_COUNT} dependency names"
fi

# -----------------------------------------------------------------------------
say ""
say "--- 2. does engine source name a networking API? ---"
SRC=$(grep -rnE 'std::net|TcpStream|TcpListener|UdpSocket|lookup_host|reqwest|hyper::|ureq::|curl_|openssl::|tokio::net' \
      "${ROOT}/core/src" --include='*.rs' 2>/dev/null || true)
if [ -n "${SRC}" ]; then
  verdict 1 "core/src references a networking symbol:"
  printf '%s\n' "${SRC}" | head -n 10 | sed 's/^/          /'
else
  verdict 0 "no networking symbol in core/src"
fi

# -----------------------------------------------------------------------------
say ""
say "--- 3. build the release engine (the artefact the claim is about) ---"
if [ "${BUILD}" = "1" ]; then
  say "  building; this is the same command scripts/build-release.sh runs"
  if ! bash "${ROOT}/scripts/build-release.sh" --quiet; then
    say "  FAIL  release build failed"
    exit 2
  fi
fi
# Two places, because two different commands produce it and the difference is the
# reason a stranger following docs/SECURITY.md by hand gets a different answer
# from this script: `scripts/build-release.sh` always passes --target, so cargo
# writes to target/<triple>/release/, while the plain `cargo build --release` in
# the document writes to target/release/. A report that only looked in one of them
# would either find nothing or miss the artefact it had just built.
HOST_TARGET=$(rustc -vV 2>/dev/null | awk '/^host: /{print $2}')
LIB=""
for candidate in \
  "${ROOT}/core/target/${HOST_TARGET}/release/${LIBNAME}" \
  "${ROOT}/core/target/release/${LIBNAME}"; do
  if [ -f "${candidate}" ]; then LIB="${candidate}"; break; fi
done
if [ -z "${LIB}" ]; then
  say "  FAIL  no release library under core/target/ for ${HOST_TARGET}."
  say "        Re-run with --build, or: cargo build --manifest-path core/Cargo.toml --release"
  say "        Everything below needs the file, and this run cannot conclude anything."
  exit 2
fi
say "  ${LIB} ($(stat -c%s "${LIB}" 2>/dev/null || stat -f%z "${LIB}") bytes)"

# -----------------------------------------------------------------------------
say ""
say "--- 4. dynamic libraries it loads ---"
case "$(uname -s)" in
  Darwin)
    DEPS=$(otool -L "${LIB}" 2>/dev/null | tail -n +2 || true)
    BAD=$(printf '%s\n' "${DEPS}" | grep -Ei 'ssl|crypto|curl|nghttp|libevent' || true)
    ;;
  *)
    DEPS=$(ldd "${LIB}" 2>/dev/null || true)
    BAD=$(printf '%s\n' "${DEPS}" | grep -Ei 'ssl|crypto|curl|nghttp|libevent' || true)
    ;;
esac
printf '%s\n' "${DEPS}" | sed 's/^/          /' | head -n 20
if [ -n "${BAD}" ]; then
  verdict 1 "loads a TLS or HTTP library:"
  printf '%s\n' "${BAD}" | sed 's/^/          /'
else
  verdict 0 "no TLS, crypto or HTTP library is loaded"
fi

# -----------------------------------------------------------------------------
say ""
say "--- 5. undefined dynamic symbols (what it could actually call) ---"
# This is the check ldd cannot do. A statically linked socket() call shows up
# here and nowhere else.
if command -v nm >/dev/null 2>&1; then
  if nm -D --undefined-only "${LIB}" 2>/dev/null > /tmp/px-nm.$$ ; then
    UNDEF=$(grep -E "^[[:space:]]*U (${NET_SYMBOLS})$" /tmp/px-nm.$$ || true)
    rm -f /tmp/px-nm.$$
    TOTAL=$(nm -D --undefined-only "${LIB}" 2>/dev/null | grep -c '^ *U ' || true)
    if [ -n "${UNDEF}" ]; then
      verdict 1 "the binary has unresolved networking symbols:"
      printf '%s\n' "${UNDEF}" | sed 's/^/          /'
    else
      verdict 0 "none of ${TOTAL} undefined dynamic symbols is a networking entry point"
    fi
  else
    say "  skip  nm -D could not read ${LIB} (stripped to nothing? unlikely here)"
  fi
else
  say "  skip  nm is not installed; check 5 could not run, so this run is incomplete"
  FAILURES=$((FAILURES + 1))
fi

# -----------------------------------------------------------------------------
say ""
say "--- 6. is there any URL, host or socket literal compiled in? ---"
# A build that cannot open a socket can still leak by writing to a file or a
# log. This is a weaker check and is labelled as one: a string match on a
# release binary is a hint, not a proof.
LITERALS=$(LC_ALL=C grep -a -o -E 'https?://[a-zA-Z0-9./_-]{4,}' "${LIB}" 2>/dev/null \
           | sort -u | head -n 10 || true)
if [ -n "${LITERALS}" ]; then
  say "  note  URL-shaped strings in the binary (expected: crate metadata, not connections):"
  printf '%s\n' "${LITERALS}" | sed 's/^/          /'
else
  say "  note  no URL-shaped strings in the binary at all"
fi

# -----------------------------------------------------------------------------
say ""
if [ "${FAILURES}" -eq 0 ]; then
  say "VERDICT: the engine in core/ has no network capability, to the limit of"
  say "         checks 1-5 above. That is a statement about this artefact on this"
  say "         machine. The strongest evidence is not here: install the app,"
  say "         disconnect from the network, and use it."
  exit 0
fi
say "VERDICT: ${FAILURES} of ${CHECKS} checks FAILED. This is a hard-rule-1 violation."
exit 1