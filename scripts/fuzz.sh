#!/usr/bin/env bash
# =============================================================================
# fuzz.sh — the only supported way to build or run a fuzz target.
#
# A fuzz target is a normal cargo bin that happens to be linked against
# libFuzzer. `cargo run --bin decode_png` would compile it without sanitizers and
# without coverage instrumentation, and would quietly fuzz nothing worth finding.
# Everything here goes through `cargo fuzz`, which sets those flags.
#
#   scripts/fuzz.sh seed              rewrite fuzz/seeds/ from the fixtures
#   scripts/fuzz.sh list              list the targets
#   scripts/fuzz.sh build [target...] build every target, or just the named ones
#   scripts/fuzz.sh run [target...]   build, seed the corpus and fuzz
#   scripts/fuzz.sh tmin <artifact> <target>   minimise a crash
#
# Environment:
#   PX_FUZZ_SECONDS   per-target wall clock for `run`, default 30
#   PX_FUZZ_BUILD_STD set to 1 to fuzz with -Zbuild-std (slower, catches more)
#
# Exit 0 = clean. A non-zero exit from `run` means a crash: libFuzzer has written
# a reproducer to fuzz/artifacts/<target>/ and the run stops there.
# =============================================================================

set -uo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
FUZZ_DIR="${ROOT}/fuzz"
TOOLCHAIN=${PX_FUZZ_TOOLCHAIN:-nightly}
SECONDS_PER_TARGET=${PX_FUZZ_SECONDS:-30}

# Every target, in one place, so the workflow, the gate and this script cannot
# disagree about what exists. `cargo fuzz list` reads the same directory.
TARGETS=(
  detect_format
  validate_bytes
  decode_bounded
  exif_read
  decode_jpeg
  decode_png
  decode_webp
  decode_gif
  decode_tiff
  decode_bmp
  decode_ico
)

die() { printf 'fuzz.sh: %s\n' "$*" >&2; exit 1; }

require_cargo_fuzz() {
  command -v cargo-fuzz >/dev/null 2>&1 || die \
    "cargo-fuzz is not installed.
  install it with:  cargo install cargo-fuzz --locked
  and a nightly toolchain with:  rustup toolchain install nightly
  This is opt-in tooling: nothing in the default gate needs it."

  rustup toolchain list 2>/dev/null | grep -q "^${TOOLCHAIN}" || die \
    "no '${TOOLCHAIN}' toolchain.
  install it with:  rustup toolchain install ${TOOLCHAIN}
  Override the name with PX_FUZZ_TOOLCHAIN."

  # cargo-fuzz defaults to --build-std, which needs rust-src and rebuilds the
  # standard library. Our code already gets debug assertions and overflow checks;
  # only std does not, so that is opt-in rather than the default.
  BUILD_STD_ARGS=(--build-std false)
  if [ "${PX_FUZZ_BUILD_STD:-0}" = "1" ]; then
    rustup component list --toolchain "${TOOLCHAIN}" 2>/dev/null \
      | grep -q '^rust-src.*(installed)' \
      || die "PX_FUZZ_BUILD_STD=1 needs:  rustup component add rust-src --toolchain ${TOOLCHAIN}"
    BUILD_STD_ARGS=(--build-std true)
  fi
}

# Copy the committed seeds into the working corpus, without clobbering anything
# the fuzzer has already found.
seed_corpus() {
  for target in "${TARGETS[@]}"; do
    local src="${FUZZ_DIR}/seeds/${target}"
    [ -d "${src}" ] || continue
    mkdir -p "${FUZZ_DIR}/corpus/${target}"
    cp -n "${src}"/* "${FUZZ_DIR}/corpus/${target}/" 2>/dev/null
  done
}

cmd="${1:-}"
shift || true

case "${cmd}" in
  seed)
    # cargo +stable is enough: this is a fixture generator, not a fuzz target.
    ( cd "${ROOT}" && cargo run --quiet --manifest-path fuzz/Cargo.toml --bin seed-corpus )
    ;;

  list)
    require_cargo_fuzz
    ( cd "${ROOT}" && cargo +"${TOOLCHAIN}" fuzz list --fuzz-dir fuzz )
    ;;

  build)
    require_cargo_fuzz
    for target in "${@:-${TARGETS[@]}}"; do
      printf '=== building %s ===\n' "${target}"
      ( cd "${ROOT}" && cargo +"${TOOLCHAIN}" fuzz build --fuzz-dir fuzz "${target}" \
          "${BUILD_STD_ARGS[@]}" ) || die "failed to build ${target}"
    done
    ;;

  run)
    require_cargo_fuzz
    seed_corpus
    # One target per invocation: cargo-fuzz takes a single target, and a single
    # target per process is also what makes the wall clock per target honest.
    for target in "${@:-${TARGETS[@]}}"; do
      printf '=== fuzzing %s for %ss ===\n' "${target}" "${SECONDS_PER_TARGET}"
      ( cd "${ROOT}" && cargo +"${TOOLCHAIN}" fuzz run --fuzz-dir fuzz "${target}" \
          "${BUILD_STD_ARGS[@]}" \
          -- -max_total_time="${SECONDS_PER_TARGET}" -rss_limit_mb=2048 -print_final_stats=1 )
      status=$?
      if [ ${status} -ne 0 ]; then
        printf '=== %s crashed after %ss; reproducer in fuzz/artifacts/%s ===\n' \
          "${target}" "${SECONDS_PER_TARGET}" "${target}" >&2
        exit ${status}
      fi
    done
    ;;

  tmin)
    [ $# -eq 2 ] || die "usage: scripts/fuzz.sh tmin <artifact-file> <target>"
    require_cargo_fuzz
    ( cd "${ROOT}" && cargo +"${TOOLCHAIN}" fuzz tmin --fuzz-dir fuzz "$2" "$1" "${BUILD_STD_ARGS[@]}" )
    ;;

  *)
    sed -n '2,25p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit 1
    ;;
esac