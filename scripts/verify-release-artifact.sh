#!/usr/bin/env bash
# =============================================================================
# verify-release-artifact.sh — check the artefact, not the build log.
#
# "Built build/app/outputs/flutter-apk/app-release.apk" is a message from a
# program that was asked whether it finished, not a statement about what is in
# the file. The two failure modes this script exists for are invisible in a log:
#
#   1. An APK with no native engine library. It installs, it opens, it renders
#      the Flutter shell, and it throws at the first `DynamicLibrary.open` — so
#      it looks like a crash in the user's photographs rather than a build that
#      shipped nothing. This repository's own build.yml carries a comment
#      recording exactly that: an earlier revision wrote the .so files to a
#      directory Gradle does not read, "The APK built fine and contained no
#      native library — it opened and would have crashed on first tap."
#
#   2. A Windows bundle with no pixelsmith_core.dll, which is the same defect in
#      a different wrapper: the EXE starts and the engine is not there.
#
# So every artefact is opened and its contents asserted. Nothing here trusts the
# build, and nothing here trusts a filename: the ABI directory inside the archive
# is cross-checked against the ELF machine type of the library filed under it,
# because "lib/arm64-v8a/libpixelsmith_core.so is actually a 32-bit ARM library"
# is a real packaging mistake that every name-based check passes.
#
# Usage:
#   bash scripts/verify-release-artifact.sh apk     PATH.apk  [options]
#   bash scripts/verify-release-artifact.sh aab     PATH.aab  [options]
#   bash scripts/verify-release-artifact.sh windows DIR_OR_ZIP [options]
#   bash scripts/verify-release-artifact.sh --self-test
#
# Options:
#   --gradle FILE        app/android/app/build.gradle.kts, to read abiFilters from.
#                        Default: <repo>/app/android/app/build.gradle.kts when the
#                        repo is the working directory.
#   --abi-source DIR     directory of staged libpixelsmith_core.so files
#                        (app/android/app/src/main/jniLibs). The archive must
#                        carry exactly the ABIs the engine was built for.
#   --no-min-sdk         skip the minSdkVersion assertion and SAY that it was
#                        skipped. Only for a machine with no Android SDK.
#   --exe-name NAME      expected Windows executable name (default: any .exe)
#   --label TEXT         name used in the output, e.g. "release APK"
#
# Exit 0 = every assertion held. 1 = at least one did not. 2 = bad arguments.
# =============================================================================

set -uo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

FAILURES=0
CHECKS=0
MODE=""
TARGET=""
GRADLE=""
ABI_SOURCE=""
MIN_SDK=1
EXE_NAME=""
LABEL=""

# The floor for an Android device this engine claims to run on.
#
# Not a guess about the OS: `rustc`'s std for aarch64-linux-android supports API
# 21 and up, and none of the engine's dependencies (image, libwebp, rav1e,
# blake3) needs a newer platform API than that. So the engine's own floor is 21,
# and what actually binds on a device is Flutter's, which is higher. The
# assertion below is "the manifest's minSdkVersion is at least this, and no less
# than the toolchain's" — and the number is printed, so a reader can see which of
# the two it is.
ENGINE_MIN_SDK=21

# Windows executable size floor, in bytes.
#
# The phase prompt suggested "under about 100 KB means the engine failed to
# link". A Flutter release runner EXE is genuinely in that neighbourhood — the
# Dart AOT snapshot ships separately as data/app.so, so the EXE is a thin
# launcher — which makes 100 KB a coin flip rather than a guard, and a floor
# that flips is worse than no floor. 40 KB is below any real one and well above
# a truncated file, and the substantive assertions here are not about the EXE's
# size at all: they are that pixelsmith_core.dll sits next to it, that it is a
# real PE image exporting a real engine entry point, and that data/app.so — the
# Dart snapshot — is present and not a stub. Those three fail when the build is
# broken; the size floor only catches a truncated file.
EXE_MIN_BYTES=40000

say()  { printf '%s\n' "$*"; }
head1() { printf '\n--- %s ---\n' "$*"; }
ok()   { CHECKS=$((CHECKS + 1)); say "  ok:   $*"; }
bad()  { CHECKS=$((CHECKS + 1)); FAILURES=$((FAILURES + 1)); say "  FAIL: $*"; }
note() { say "  note: $*"; }

die() { printf 'verify-release-artifact.sh: %s\n' "$*" >&2; exit 2; }

while [ $# -gt 0 ]; do
  case "$1" in
    apk|windows|aab)  MODE="$1"; shift ;;
    --gradle)        GRADLE="$2"; shift 2 ;;
    --gradle=*)      GRADLE="${1#*=}"; shift ;;
    --abi-source)    ABI_SOURCE="$2"; shift 2 ;;
    --abi-source=*)  ABI_SOURCE="${1#*=}"; shift ;;
    --no-min-sdk)    MIN_SDK=0; shift ;;
    --exe-name)      EXE_NAME="$2"; shift 2 ;;
    --exe-name=*)    EXE_NAME="${1#*=}"; shift ;;
    --label)         LABEL="$2"; shift 2 ;;
    --label=*)       LABEL="${1#*=}"; shift ;;
    --self-test)     MODE="--self-test"; shift ;;
    -h|--help)       sed -n '2,44p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*)              die "unknown option: $1" ;;
    *)               TARGET="$1"; shift ;;
  esac
done

[ -n "${MODE}" ] || die "no mode given: apk, aab, windows or --self-test"
[ "${LABEL}" ] || case "${MODE}" in
  apk)     LABEL="APK" ;;
  aab)     LABEL="app bundle" ;;
  windows) LABEL="Windows bundle" ;;
esac

if [ "${MODE}" != "--self-test" ]; then
  [ -n "${TARGET}" ] || die "${MODE} mode needs a path"
  [ -e "${TARGET}" ] || die "no such file: ${TARGET}"
fi
[ -n "${GRADLE}" ] || GRADLE="${REPO}/app/android/app/build.gradle.kts"

# =============================================================================
# Small readers. Written against `od` so there is no dependency on readelf,
# objdump or a PE library, and so the byte offsets are visible in this file
# rather than in whatever tool happens to be installed.
# =============================================================================

# ELF e_machine: two bytes at offset 0x12, little-endian on disk. AArch64 is the
# value 0x00B7, which little-endian stores as the bytes b7 00 — so `od -tx1`
# prints "b700" and that is the CORRECT reading, not 0x00b7.
#
# Every value in the table below, and every value read back, is the raw byte
# string in file order and never the reassembled number. Getting that backwards
# is not a hypothetical: an earlier revision of this script "corrected" the
# table to the reassembled form, the self-test went green because the synthetic
# fixtures were built from the same wrong table, and it was a real AArch64
# library from a real APK that caught it. Synthetic fixtures cannot tell you
# your convention is wrong when they are generated with the convention.
elf_machine() {
  local bytes
  bytes=$(od -An -tx1 -j 18 -N 2 "$1" 2>/dev/null | tr -d ' \n')
  [ -n "${bytes}" ] || return 1
  printf '%s' "${bytes}"
}

# The same two bytes, as a name, for the messages. A reader who has to decode
# "6486" in their head is a reader who will mis-read the failure.
elf_machine_name() {
  case "$1" in
    2800) printf 'ARM (0x28)' ;;
    3e00) printf 'x86-64 (0x3E)' ;;
    b700) printf 'AArch64 (0xB7)' ;;
    0300) printf 'i386 (0x03)' ;;
    *)    printf 'e_machine %s' "$1" ;;
  esac
}

# e_machine as two little-endian hex bytes. The archive's ABI name and ELF's
# machine number are different namespaces, which is exactly why the mapping
# lives in one visible table: armeabi-v7a is 0x28 and arm64-v8a is 0xB7, and an
# ABI directory holding the wrong one is a packaging mistake every name-based
# check passes.
elf_arch_for_abi() {
  case "$1" in
    armeabi-v7a) printf '2800' ;;
    arm64-v8a)   printf 'b700' ;;
    x86_64)      printf '3e00' ;;
    x86)         printf '0300' ;;
    *)           printf '' ;;
  esac
}

# PE: "MZ" at 0, e_lfanew at 0x3c (little-endian uint32), "PE\0\0" there, then
# Machine at +4. 0x8664 is x64, 0x14c is i386, 0xaa64 is ARM64.
pe_machine() {
  local off sig
  sig=$(od -An -c -N 2 "$1" 2>/dev/null | tr -d ' \n')
  [ "${sig}" = "MZ" ] || return 1
  off=$(od -An -tu4 -j 60 -N 4 "$1" 2>/dev/null | tr -d ' \n')
  [ -n "${off}" ] || return 1
  sig=$(od -An -c -j "${off}" -N 4 "$1" 2>/dev/null | tr -d ' \n')
  [ "${sig}" = "PE\0\0" ] || return 1
  od -An -tx1 -j $((off + 4)) -N 2 "$1" 2>/dev/null | tr -d ' \n'
}

# PE Machine, as a name. Same convention as ELF: the bytes in file order, so
# 0x8664 is stored as "64 86" and that is what od prints.
pe_machine_name() {
  case "$1" in
    4c01) printf 'x86 (0x14C)' ;;
    6486) printf 'x64 (0x8664)' ;;
    64aa) printf 'ARM64 (0xAA64)' ;;
    *)    printf 'Machine %s' "$1" ;;
  esac
}

# Does the file contain this ASCII string? Used to prove that the library under
# test is the engine and not an empty shell or somebody else's .so: `px_version`
# is an exported FFI symbol, so its name is in the dynamic symbol table, the
# string table and the Rust metadata.
contains_bytes() {
  LC_ALL=C grep -qa -- "$2" "$1"
}

# =============================================================================
# Android: APK and AAB.
# =============================================================================

# Declared ABI set from the Gradle config. abiFilters is the one place the
# intended ABI set is written down, so it is what the archive is compared
# against — the whole point is to catch a disagreement between two lists that
# would otherwise be typed down twice and compared by nobody.
#
# Balanced PARENTHESES, not square brackets, and that is not a detail: the
# idiomatic form is `abiFilters += listOf("arm64-v8a", ...)`, whose closing
# bracket is a paren, so a range that ends at the next `]` ran on into the
# signingConfigs block and reported `release release debug` as three more ABIs.
# Reading a build script with the wrong delimiter is how a correct Gradle file
# produces a wrong answer here.
declared_abis() {
  [ -f "$1" ] || return 1
  local out
  out=$(awk '
    /abiFilters/ { inb = 1; depth = 0 }
    inb {
      s = $0
      while (match(s, /"[^"]+"/)) {
        print substr(s, RSTART + 1, RLENGTH - 2)
        s = substr(s, RSTART + RLENGTH)
      }
      opens = gsub(/\(/, "(")
      closes = gsub(/\)/, ")")
      depth += opens - closes
      if (depth <= 0) inb = 0
    }
  ' "$1")
  [ -n "${out}" ] || return 1
  printf '%s\n' "${out}"
}

find_aapt() {
  local candidate
  for candidate in "${AAPT2:-}" "${ANDROID_SDK_ROOT:-}/build-tools"/*/aapt2 \
                   "${ANDROID_HOME:-}/build-tools"/*/aapt2 "${ANDROID_SDK_ROOT:-}/build-tools"/*/aapt \
                   "${ANDROID_HOME:-}/build-tools"/*/aapt; do
    [ -n "${candidate}" ] || continue
    if [ -x "${candidate}" ]; then printf '%s' "${candidate}"; return 0; fi
  done
  return 1
}

check_android() {
  local archive="$1" layout="$2"
  local size listing
  size=$(stat -c%s "${archive}" 2>/dev/null || stat -f%z "${archive}" 2>/dev/null || echo 0)

  say "${LABEL}: ${archive}"
  say "  size: ${size} bytes"

  head1 "archive"
  if [ "${size}" -gt 0 ] && unzip -l "${archive}" >/dev/null 2>&1; then
    ok "reads as a zip archive (${size} bytes)"
  else
    bad "not a readable zip archive, or empty"
    say ""
    say "verify-release-artifact.sh: FAIL (${FAILURES})"
    return 1
  fi

  listing=$(unzip -Z1 "${archive}" 2>/dev/null)

  head1 "it is an app, not an empty shell"
  # An APK is flat; an app bundle puts everything under base/ and prefixes the
  # native libraries with base/lib/<abi>/lib/. Asserting an APK's paths against
  # a bundle reports a complete, correct bundle as seven failures, so the layout
  # is a parameter rather than something guessed from the extension.
  # Two different layouts, not one with a prefix: an APK holds classes.dex and
  # AndroidManifest.xml at the root, while a bundle holds them under base/dex/
  # and base/manifest/ and adds a native-library lib/ level. Getting this wrong
  # reports a complete, correct bundle as seven failures.
  # Two different layouts, and only the first two paths differ:
  #   APK:     classes.dex                AndroidManifest.xml
  #   bundle:  base/dex/classes.dex       base/manifest/AndroidManifest.xml
  # The native libraries are `lib/<abi>/lib<name>.so` in BOTH — a bundle has
  # no extra lib level. An earlier revision of this comment said it did, on the
  # strength of the synthetic fixture the self-test builds, and the real .aab
  # then reported four failures against a bundle that was entirely correct. A
  # synthetic fixture cannot tell you the shape of the thing it is imitating.
  local prefix="" dex="classes.dex" manifest="AndroidManifest.xml"
  if [ "${layout}" = "aab" ]; then
    prefix="base/"
    dex="base/dex/classes.dex"
    manifest="base/manifest/AndroidManifest.xml"
  fi
  if printf '%s\n' "${listing}" | grep -qx "${dex}"; then
    ok "${dex} present"
  else
    bad "no ${dex} — this archive contains no application code"
  fi
  if printf '%s\n' "${listing}" | grep -qx "${manifest}"; then
    ok "${manifest} present"
  else
    bad "no ${manifest}"
  fi
  if [ "${layout}" = "aab" ]; then
    # BundleConfig.pb is what makes an .aab an .aab rather than a renamed zip.
    if printf '%s\n' "${listing}" | grep -qx 'BundleConfig.pb'; then
      ok "BundleConfig.pb present — this is a real app bundle, not a renamed archive"
    else
      bad "no BundleConfig.pb — Play will reject this"
    fi
  fi

  head1 "the engine ships for every ABI Flutter ships for"
  # The ABI list is taken from Flutter's own libraries, not from the build's
  # intent. That is the list a device can actually load the app on, so it is the
  # list the engine has to be present for. Asserting against the build's own
  # intent is how an ABI set can be wrong in both directions at once and pass.
  local flutter_abis engine_abis abi missing=""
  flutter_abis=$(printf '%s\n' "${listing}" \
    | sed -n "s|^${prefix}lib/\\([^/]*\\)/libflutter\\.so$|\\1|p" | sort -u)
  engine_abis=$(printf '%s\n' "${listing}" \
    | sed -n "s|^${prefix}lib/\\([^/]*\\)/libpixelsmith_core\\.so$|\\1|p" | sort -u)

  if [ -z "${flutter_abis}" ]; then
    bad "no lib/<abi>/libflutter.so at all — not a Flutter build"
  else
    ok "Flutter ABIs in the archive: $(printf '%s' "${flutter_abis}" | tr '\n' ' ')"
  fi

  if [ -z "${engine_abis}" ]; then
    bad "no lib/<abi>/libpixelsmith_core.so at all"
    say "        the app would install, open, and throw on the first resize"
  else
    ok "engine ABIs in the archive: $(printf '%s' "${engine_abis}" | tr '\n' ' ')"
  fi

  for abi in ${flutter_abis}; do
    printf '%s\n' "${engine_abis}" | grep -qx "${abi}" || missing="${missing} ${abi}"
  done
  if [ -n "${missing}" ]; then
    bad "these ABIs ship libflutter.so but no libpixelsmith_core.so:${missing}"
    say "        a device on one of those ABIs installs the app and crashes on first resize"
  else
    ok "every ABI that carries libflutter.so also carries the engine"
  fi

  head1 "each engine library is a real library for the ABI it is filed under"
  local work
  work=$(mktemp -d "${TMPDIR:-/tmp}/px-artefact.XXXXXX") || die "mktemp failed"
  for abi in ${engine_abis}; do
    local member="${prefix}lib/${abi}/libpixelsmith_core.so"
    if ! unzip -p "${archive}" "${member}" > "${work}/lib.so" 2>/dev/null; then
      bad "${member} could not be extracted"
      continue
    fi
    local bytes
    bytes=$(stat -c%s "${work}/lib.so")
    if [ "${bytes}" -lt 1024 ]; then
      bad "${member} is ${bytes} bytes — a real engine library is megabytes"
      continue
    fi
    if ! elf_machine "${work}/lib.so" >/dev/null; then
      bad "${member} is not an ELF shared object"
      continue
    fi
    local want got
    want=$(elf_arch_for_abi "${abi}")
    got=$(elf_machine "${work}/lib.so")
    if [ -z "${got}" ] || [ "${got}" = "0000" ]; then
      bad "${member}: could not read an ELF e_machine at offset 18"
      continue
    fi
    if [ "${want}" != "${got}" ]; then
      bad "${member} is a $(elf_machine_name "${got}") library, but ${abi} is $(elf_machine_name "${want}")"
      say "        the wrong build was filed under this ABI — every name-based check passes this"
      continue
    fi
    if ! contains_bytes "${work}/lib.so" 'px_version'; then
      bad "${member} exports no px_version — it is not the engine"
      continue
    fi
    ok "${member}: $(elf_machine_name "${got}"), ${bytes} bytes, exports px_version"
  done
  rm -rf "${work}"

  head1 "the declared ABI set and the archive agree"
  if [ -f "${GRADLE}" ]; then
    local declared
    if declared=$(declared_abis "${GRADLE}"); then
      ok "abiFilters in $(basename "$(dirname "$(dirname "${GRADLE}")")")/build.gradle.kts: $(printf '%s' "${declared}" | tr '\n' ' ')"
      local dmissing="" dextra=""
      for abi in $(printf '%s\n' "${declared}"); do
        printf '%s\n' "${engine_abis}" | grep -qx "${abi}" || dmissing="${dmissing} ${abi}"
      done
      for abi in ${engine_abis}; do
        printf '%s\n' "${declared}" | grep -qx "${abi}" || dextra="${dextra} ${abi}"
      done
      if [ -n "${dmissing}" ]; then
        bad "declared by abiFilters but absent from the archive:${dmissing}"
        say "        every AAB install is told the ABI is supported and then finds no library"
      fi
      if [ -n "${dextra}" ]; then
        bad "present in the archive but not declared by abiFilters:${dextra}"
        say "        that is the 'works on my device' bug: the APK carries an ABI the build never intended"
      fi
      [ -z "${dmissing}${dextra}" ] && ok "declared ABI set and archive ABI set are the same"
    else
      bad "no abiFilters block in ${GRADLE}"
      say "        without a declared set there is nothing to compare the archive against"
    fi
  else
    note "no Gradle file at ${GRADLE}; the declared-ABI comparison was not run"
  fi

  if [ -n "${ABI_SOURCE}" ] && [ -d "${ABI_SOURCE}" ]; then
    local staged
    staged=$(find "${ABI_SOURCE}" -name 'libpixelsmith_core.so' -printf '%h\n' 2>/dev/null \
             | sed 's|.*/||' | sort -u)
    if [ -n "${staged}" ]; then
      ok "engine was staged for: $(printf '%s' "${staged}" | tr '\n' ' ')"
      local smissing=""
      for abi in ${staged}; do
        printf '%s\n' "${engine_abis}" | grep -qx "${abi}" || smissing="${smissing} ${abi}"
      done
      if [ -n "${smissing}" ]; then
        bad "built for these ABIs but not in the archive:${smissing}"
      else
        ok "every ABI the engine was built for reached the archive"
      fi
    else
      note "no libpixelsmith_core.so under ${ABI_SOURCE}; nothing to compare"
    fi
  fi

  head1 "minSdkVersion"
  if [ "${layout}" = "aab" ]; then
    # Stated rather than guessed. An AAB's base manifest is Protocol Buffers,
    # not the binary XML aapt2 reads, and minSdkVersion in it is a protobuf
    # field rather than an attribute. So this run does NOT check it — and says
    # so, because a check that silently reports nothing is the failure mode
    # this whole script exists to prevent. The APK in the same release carries
    # the same manifest source and IS checked.
    note "not checked for a bundle: base/manifest/AndroidManifest.xml is protobuf,"
    note "so aapt2 cannot read minSdkVersion out of it. The APK in the same release"
    note "is built from the same manifest and is asserted. See docs/RELEASE.md."
  elif [ "${MIN_SDK}" = "0" ]; then
    note "skipped on request (--no-min-sdk): this run asserts nothing about minSdkVersion"
  else
    local aapt
    if aapt=$(find_aapt); then
      local minsdk=""
      # aapt2 print badging is the Android 3.0+ form; aapt dump badging is the
      # older one. Both print "sdkVersion:'NN'".
      minsdk=$("${aapt}" dump badging "${archive}" 2>/dev/null \
                 | sed -n "s/^sdkVersion:'\([0-9]*\)'.*/\1/p" | head -n 1)
      if [ -z "${minsdk}" ]; then
        minsdk=$("${aapt}" dump xmltree "${archive}" AndroidManifest.xml 2>/dev/null \
                 | sed -n 's/.*minSdkVersion.*versionCode[^"]*"\([0-9]*\)".*/\1/p' | head -n 1)
      fi
      if [ -z "${minsdk}" ]; then
        bad "could not read minSdkVersion from ${archive} with ${aapt}"
      elif [ "${minsdk}" -lt "${ENGINE_MIN_SDK}" ]; then
        bad "minSdkVersion is ${minsdk}; this engine needs ${ENGINE_MIN_SDK} or newer"
        say "        a device below API ${minsdk} would install and then fail to load the library"
      else
        ok "minSdkVersion ${minsdk} (engine floor ${ENGINE_MIN_SDK})"
      fi
    else
      bad "no aapt2/aapt found, so minSdkVersion was not checked"
      say "        ANDROID_SDK_ROOT/build-tools/*/aapt2, or pass --no-min-sdk to accept"
      say "        an unchecked minSdkVersion and be told so"
    fi
  fi

  say ""
  if [ "${FAILURES}" -eq 0 ]; then
    say "verify-release-artifact.sh: PASS (${CHECKS} assertions)"
    return 0
  fi
  say "verify-release-artifact.sh: FAIL (${FAILURES} of ${CHECKS} assertions)"
  return 1
}

# =============================================================================
# Windows: a directory, or the portable ZIP of one.
# =============================================================================

check_windows() {
  local input="$1"
  local dir="" extracted=""

  say "${LABEL}: ${input}"

  if [ -d "${input}" ]; then
    dir="${input}"
  elif [ -f "${input}" ]; then
    extracted=$(mktemp -d "${TMPDIR:-/tmp}/px-winbundle.XXXXXX") || die "mktemp failed"
    # NOT `unzip -j`. A Flutter Windows bundle has data/ and pico/
    # subdirectories and the Dart snapshot lives in the first of them, so
    # flattening the archive would delete the very file the next assertion is
    # about — which is exactly what the first version of this script did, and
    # its self-test reported "the portable ZIP is broken" about a bundle that
    # was fine.
    #
    # So the entries are listed and vetted first: an absolute path or a `..`
    # component is refused outright rather than written. Hard rule 7 is about
    # filenames, and a release artefact is a filename somebody will unzip.
    local entries unsafe=""
    entries=$(unzip -Z1 "${input}" 2>/dev/null)
    if [ -z "${entries}" ]; then
      bad "not a readable zip archive"
      rm -rf "${extracted}"
      say ""
      say "verify-release-artifact.sh: FAIL (${FAILURES})"
      return 1
    fi
    unsafe=$(printf '%s\n' "${entries}" \
             | grep -E '^/|(^|/)\.\.(/|$)' || true)
    if [ -n "${unsafe}" ]; then
      bad "the archive carries paths that would escape the extraction directory:"
      printf '%s\n' "${unsafe}" | head -n 5 | sed 's/^/          /'
      rm -rf "${extracted}"
      say ""
      say "verify-release-artifact.sh: FAIL (${FAILURES})"
      return 1
    fi
    if ! unzip -q "${input}" -d "${extracted}" >/dev/null 2>&1; then
      bad "unzip failed on ${input}"
      rm -rf "${extracted}"
      say ""
      say "verify-release-artifact.sh: FAIL (${FAILURES})"
      return 1
    fi
    dir="${extracted}"
    ok "portable ZIP expands to $(find "${dir}" -maxdepth 1 -type f | wc -l) top-level files"
  else
    bad "no such bundle: ${input}"
    return 1
  fi

  local total
  total=$(find "${dir}" -type f -printf '%s\n' 2>/dev/null | awk '{s+=$1} END {print s+0}')
  say "  extracted: ${dir}"
  say "  total:    ${total} bytes"

  head1 "it is a real Flutter bundle, not a bare executable"
  if [ -d "${dir}/data/flutter_assets" ]; then
    ok "data/flutter_assets present ($(find "${dir}/data/flutter_assets" -type f | wc -l) files)"
  else
    bad "no data/flutter_assets — this is not a Flutter bundle"
  fi
  # The Dart AOT snapshot. Its absence is the failure the size floor cannot
  # reach: the launcher starts fine and then has no program to run.
  if [ -s "${dir}/data/app.so" ]; then
    ok "data/app.so present ($(stat -c%s "${dir}/data/app.so" 2>/dev/null) bytes) — the Dart snapshot is in the bundle"
  else
    bad "no data/app.so — the Dart snapshot is missing, so the EXE would start and do nothing"
  fi
  if [ -f "${dir}/flutter_windows.dll" ]; then
    ok "flutter_windows.dll present"
  else
    bad "no flutter_windows.dll"
  fi

  head1 "the executable"
  local exe
  if [ -n "${EXE_NAME}" ]; then
    exe="${dir}/${EXE_NAME}"
  else
    exe=$(find "${dir}" -maxdepth 1 -type f -name '*.exe' | sort | head -n 1)
  fi
  if [ -z "${exe}" ] || [ ! -f "${exe}" ]; then
    bad "no .exe in the bundle"
    find "${dir}" -maxdepth 1 -type f | sed 's/^/        /' | head -n 20
    rm -rf "${extracted}"
    say ""
    say "verify-release-artifact.sh: FAIL (${FAILURES})"
    return 1
  fi
  local esize emach
  esize=$(stat -c%s "${exe}" 2>/dev/null || stat -f%z "${exe}")
  if [ "${esize}" -ge "${EXE_MIN_BYTES}" ]; then
    ok "$(basename "${exe}"): ${esize} bytes (floor ${EXE_MIN_BYTES})"
  else
    bad "$(basename "${exe}") is ${esize} bytes, under the ${EXE_MIN_BYTES}-byte floor"
    say "        a Flutter release launcher is a thin binary; below this it is truncated"
  fi
  if emach=$(pe_machine "${exe}"); then
    ok "$(basename "${exe}"): $(pe_machine_name "${emach}")"
    if [ "${emach}" = "6486" ]; then
      ok "  x64, which is the target this project ships"
    else
      bad "  the EXE is $(pe_machine_name "${emach}"), not x64 — check the CMake generator triplet"
    fi
  else
    bad "$(basename "${exe}"): not a PE image (no MZ/PE signature)"
  fi

  head1 "the engine sits next to the executable"
  # Next to, not somewhere in the tree. `flutter_windows.dll` is found by
  # DynamicLibrary.open through a relative path resolved from the executable's
  # own directory, so a DLL in a subdirectory is a DLL that will not be found,
  # and the failure is a load error on the first call.
  local dll="${dir}/pixelsmith_core.dll"
  if [ -f "${dll}" ]; then
    local dsize dsize_mach
    dsize=$(stat -c%s "${dll}" 2>/dev/null || stat -f%z "${dll}")
    if [ "${dsize}" -ge 65536 ]; then
      ok "pixelsmith_core.dll next to the EXE (${dsize} bytes)"
    else
      bad "pixelsmith_core.dll is ${dsize} bytes — a real engine library is megabytes"
    fi
    if dsize_mach=$(pe_machine "${dll}"); then
      ok "pixelsmith_core.dll: $(pe_machine_name "${dsize_mach}")"
      if [ -n "${emach}" ] && [ "${dsize_mach}" != "${emach}" ]; then
        bad "the engine DLL is $(pe_machine_name "${dsize_mach}") and the EXE is $(pe_machine_name "${emach}") — they cannot link"
      fi
    else
      bad "pixelsmith_core.dll: not a PE image"
    fi
    if contains_bytes "${dll}" 'px_version'; then
      ok "pixelsmith_core.dll exports px_version — it is the engine and not a stub"
    else
      bad "pixelsmith_core.dll contains no px_version — it is not the engine"
    fi
  else
    bad "pixelsmith_core.dll is not next to the EXE"
    say "        the app would start and then throw a load error on the first resize"
    say "        DLLs found, for the diagnosis:"
    find "${dir}" -name '*.dll' | sed 's/^/          /' | head -n 20
  fi

  head1 "no installer, no admin rights"
  # The deliverable is a portable ZIP, so anything that wants to register itself
  # is a defect rather than a feature. Checked as a statement about the bundle's
  # shape, which is what a user can observe.
  if find "${dir}" -maxdepth 2 -type f \
       \( -iname '*.msi' -o -iname 'setup*.exe' -o -iname '*.msix' -o -iname 'install*.exe' \) \
       | grep -q .; then
    bad "the bundle contains an installer; this project ships a portable ZIP"
  else
    ok "no installer in the bundle — unzip it and run the EXE"
  fi

  rm -rf "${extracted}"
  say ""
  if [ "${FAILURES}" -eq 0 ]; then
    say "verify-release-artifact.sh: PASS (${CHECKS} assertions)"
    return 0
  fi
  say "verify-release-artifact.sh: FAIL (${FAILURES} of ${CHECKS} assertions)"
  return 1
}

# =============================================================================
# --self-test
#
# Each case builds a small synthetic artefact that is wrong in exactly one way
# and asserts the script rejects it. A check that has only ever been observed to
# pass has not been observed to work: this repository has five recorded
# instances of a gate that could not report the truth (phase-01's `check()`,
# phase-08's `| head -n 40`, phase-10's `--test`, phase-15's target-triple
# regex, phase-15's `spdx_licenses` typo), and two phases whose work was
# complete and whose runs were lost to them.
#
# Synthetic archives stand in for the real thing here because the point being
# tested is the *assertions*, and an assertion can be exercised without building
# an APK. The real artefact is checked by the same code path in CI.
# =============================================================================

# Global, because the EXIT trap fires after every local in selftest has gone
# out of scope, which is a `set -u` "unbound variable" on the last line of an
# otherwise successful run.
SELFTEST_DIR=""

cleanup_selftest() { [ -n "${SELFTEST_DIR}" ] && rm -rf "${SELFTEST_DIR}"; }

selftest() {
  local cases=0 fired=0 rc base
  SELFTEST_DIR=$(mktemp -d "${TMPDIR:-/tmp}/px-artefact-selftest.XXXXXX") || die "mktemp failed"
  trap cleanup_selftest EXIT
  local work="${SELFTEST_DIR}"
  base="${work}/base"
  mkdir -p "${base}"

  # A stand-in .so that is a real ELF header of the right machine and mentions
  # px_version, so the passing case passes for the right reasons.
  local libdir="${base}/libsrc"
  mkdir -p "${libdir}"
  # A stand-in .so whose e_machine is right for its ABI and which mentions
  # px_version, so the passing case passes for the right reasons and the
  # per-ABI assertion is exercised rather than skipped. Little-endian, so
  # AArch64 0x00B7 is the byte string "00b7" and not "b700".
  for pair in "arm64-v8a b700" "armeabi-v7a 2800" "x86_64 3e00" "x86 0300"; do
    set -- ${pair}
    local abi="$1" mach="$2" f="${libdir}/$1.so"
    # 8 bytes: \177ELF, then EI_CLASS, EI_DATA, EI_VERSION, EI_OSABI and
    # EI_ABIVERSION. Ten zero bytes then put e_type at 8..15 and e_machine at
    # 16..17 — so one more of them and the machine lands at 18, which is where
    # elf_machine() reads. That off-by-one was real: three "passing" cases
    # failed on a correctly built fixture until the arithmetic was counted.
    #
    # The machine bytes are written in FILE ORDER, which for a little-endian
    # field means the low byte first. That is the convention the reader uses and
    # the one a real library agrees with.
    printf '\177ELF\002\001\001\000' > "${f}"
    printf '\000\000\000\000\000\000\000\000\000\000' >> "${f}"
    printf "$(printf '%s' "${mach}" | sed 's/\(..\)/\\x\1/g')" >> "${f}"
    dd if=/dev/zero bs=1 count=2000 >> "${f}" 2>/dev/null
    printf 'px_version px_inspect px_process px_batch' >> "${f}"
  done

  # A stand-in Windows bundle: one PE x64 EXE, the engine DLL, the snapshot.
  mkdir -p "${base}/win/data/flutter_assets"
  # A PE image with a valid header: "MZ", e_lfanew = 64 at offset 0x3c, "PE\0\0"
  # at 64, then the Machine word. Offsets are counted in the comments because
  # getting e_lfanew's position wrong is invisible until a signature check runs.
  make_pe() {  # path machine-bytes-little-endian
    local out="$1" mach="$2" sig
    sig=$(printf '%s' "${mach}" | sed 's/\(..\)/\\x\1/g')
    printf 'MZ' > "${out}"                       # offset 0
    dd if=/dev/zero bs=1 count=58 >> "${out}" 2>/dev/null   # offsets 2..59
    printf '\100\000\000\000' >> "${out}"      # offset 60: e_lfanew = 64
    printf 'PE\000\000' >> "${out}"              # offset 64
    printf "${sig}" >> "${out}"                  # offset 68: Machine
    dd if=/dev/zero bs=1024 count=100 >> "${out}" 2>/dev/null
  }
  make_pe "${base}/win/shrinkray.exe" '6486'
  make_pe "${base}/win/pixelsmith_core.dll" '6486'
  make_pe "${base}/win/flutter_windows.dll" '6486'
  printf 'px_version px_inspect px_process px_batch' >> "${base}/win/pixelsmith_core.dll"
  printf 'dart-aot-snapshot' > "${base}/win/data/app.so"
  dd if=/dev/zero bs=1024 count=512 >> "${base}/win/data/app.so" 2>/dev/null
  printf 'asset' > "${base}/win/data/flutter_assets/AssetManifest.json"

  # A Gradle file declaring the three ABIs, which is what a correct build has.
  cat > "${base}/build.gradle.kts" <<'GRADLE'
android {
    defaultConfig {
        ndk {
            abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64")
        }
    }
}
GRADLE

  # make_apk DIR OUTPUT — zip a directory into an archive.
  make_apk() {
    ( cd "$1" && zip -q -r -X "$2" . ) || die "zip failed"
  }

  # A case is a name, the exit code it must produce, and a sentence saying what
  # is wrong with the artefact it was built from. The Android cases pass
  # --no-min-sdk because a synthetic archive has no compiled manifest for aapt
  # to read; the minSdkVersion assertion is exercised against a real APK in CI.
  #
  # PX_SELFTEST_SHOW=<substring> prints the output of the matching cases. A
  # self-test that only prints pass/fail leaves "it failed, and I cannot see
  # why" as the only debugging step, which is how the first draft of this
  # script spent its time.
  MODEARG=""
  CASE_TARGET=""
  run() {
    local name="$1" expect="$2" desc="$3" show=1
    cases=$((cases + 1))
    if [ -n "${PX_SELFTEST_SHOW:-}" ] && [ "$name" != "${PX_SELFTEST_SHOW}" ]; then show=0; fi
    if [ "${show}" = "1" ]; then
      "${BASH_SOURCE[0]}" "${MODEARG}" "${CASE_TARGET}" \
        --gradle "${base}/build.gradle.kts" --no-min-sdk 2>&1 | sed 's/^/        | /'
      rc=${PIPESTATUS[0]}
    else
      "${BASH_SOURCE[0]}" "${MODEARG}" "${CASE_TARGET}" \
        --gradle "${base}/build.gradle.kts" --no-min-sdk >/dev/null 2>&1
      rc=$?
    fi
    if [ "${rc}" = "${expect}" ]; then
      say "  ok:   ${name} -> exit ${rc} (${desc})"
      fired=$((fired + 1))
    else
      say "  FAIL: ${name} -> exit ${rc}, expected ${expect} (${desc})"
    fi
  }

  say "verify-release-artifact.sh --self-test"
  say ""

  # ---- Android ------------------------------------------------------------
  MODEARG="apk"
  mkdir -p "${work}/good"
  for abi in arm64-v8a armeabi-v7a x86_64; do
    mkdir -p "${work}/good/lib/${abi}"
    cp "${libdir}/${abi}.so" "${work}/good/lib/${abi}/libpixelsmith_core.so"
    cp "${libdir}/${abi}.so" "${work}/good/lib/${abi}/libflutter.so"
  done
  printf 'dex' > "${work}/good/classes.dex"
  printf 'manifest' > "${work}/good/AndroidManifest.xml"
  make_apk "${work}/good" "${work}/good.apk"
  CASE_TARGET="${work}/good.apk"
  run "apk-complete" 0 "dex, manifest, engine for every ABI"

  mkdir -p "${work}/no-dex/lib"
  printf 'manifest' > "${work}/no-dex/AndroidManifest.xml"
  make_apk "${work}/no-dex" "${work}/no-dex.apk"
  MODEARG="apk"; CASE_TARGET="${work}/no-dex.apk"
  run "apk-no-classes-dex" 1 "no classes.dex at all"

  mkdir -p "${work}/no-engine/lib/arm64-v8a"
  cp "${libdir}/arm64-v8a.so" "${work}/no-engine/lib/arm64-v8a/libflutter.so"
  printf 'dex' > "${work}/no-engine/classes.dex"
  printf 'manifest' > "${work}/no-engine/AndroidManifest.xml"
  make_apk "${work}/no-engine" "${work}/no-engine.apk"
  MODEARG="apk"; CASE_TARGET="${work}/no-engine.apk"
  run "apk-no-engine-library" 1 "libflutter.so with no libpixelsmith_core.so"

  mkdir -p "${work}/partial/lib/arm64-v8a" "${work}/partial/lib/x86_64"
  cp "${libdir}/arm64-v8a.so" "${work}/partial/lib/arm64-v8a/libpixelsmith_core.so"
  cp "${libdir}/arm64-v8a.so" "${work}/partial/lib/arm64-v8a/libflutter.so"
  cp "${libdir}/x86_64.so" "${work}/partial/lib/x86_64/libflutter.so"
  printf 'dex' > "${work}/partial/classes.dex"
  printf 'manifest' > "${work}/partial/AndroidManifest.xml"
  make_apk "${work}/partial" "${work}/partial.apk"
  MODEARG="apk"; CASE_TARGET="${work}/partial.apk"
  run "apk-engine-missing-on-one-abi" 1 "32-bit ABI has Flutter and no engine"

  # The wrong library filed under the right ABI: a 32-bit ARM build in the
  # arm64-v8a directory. Every check that reads the filename passes this.
  mkdir -p "${work}/misfiled/lib/arm64-v8a" "${work}/misfiled/lib/armeabi-v7a" "${work}/misfiled/lib/x86_64"
  cp "${libdir}/arm64-v8a.so" "${work}/misfiled/lib/armeabi-v7a/libpixelsmith_core.so"
  cp "${libdir}/arm64-v8a.so" "${work}/misfiled/lib/arm64-v8a/libpixelsmith_core.so"
  cp "${libdir}/x86_64.so" "${work}/misfiled/lib/x86_64/libpixelsmith_core.so"
  for abi in arm64-v8a armeabi-v7a x86_64; do cp "${libdir}/${abi}.so" "${work}/misfiled/lib/${abi}/libflutter.so"; done
  printf 'dex' > "${work}/misfiled/classes.dex"
  printf 'manifest' > "${work}/misfiled/AndroidManifest.xml"
  make_apk "${work}/misfiled" "${work}/misfiled.apk"
  MODEARG="apk"; CASE_TARGET="${work}/misfiled.apk"
  run "apk-wrong-library-for-abi" 1 "32-bit ARM library filed under arm64-v8a"

  # A .so of the right size and the right ELF machine that is not the engine:
  # the "build succeeded, library is a stub" failure.
  mkdir -p "${work}/stub/lib/arm64-v8a" "${work}/stub/lib/armeabi-v7a" "${work}/stub/lib/x86_64"
  for abi in arm64-v8a armeabi-v7a x86_64; do
    cp "${libdir}/${abi}.so" "${work}/stub/lib/${abi}/libpixelsmith_core.so"
    cp "${libdir}/${abi}.so" "${work}/stub/lib/${abi}/libflutter.so"
  done
  for abi in arm64-v8a armeabi-v7a x86_64; do
    tr -d 'px_version' < "${libdir}/${abi}.so" > "${work}/stub/lib/${abi}/libpixelsmith_core.so.tmp"
    mv "${work}/stub/lib/${abi}/libpixelsmith_core.so.tmp" "${work}/stub/lib/${abi}/libpixelsmith_core.so"
  done
  printf 'dex' > "${work}/stub/classes.dex"
  printf 'manifest' > "${work}/stub/AndroidManifest.xml"
  make_apk "${work}/stub" "${work}/stub.apk"
  MODEARG="apk"; CASE_TARGET="${work}/stub.apk"
  run "apk-library-is-a-stub" 1 "library present but exports no px_version"

  # ---- app bundle ---------------------------------------------------------
  # Same bundle content, the layout `flutter build appbundle` actually
  # produces: everything under base/, native libraries under
  # base/lib/<abi>/lib/, and a BundleConfig.pb.
  mkdir -p "${work}/bundle/base/dex" "${work}/bundle/base/manifest"
  for abi in arm64-v8a armeabi-v7a x86_64; do
    mkdir -p "${work}/bundle/base/lib/${abi}"
    cp "${libdir}/${abi}.so" "${work}/bundle/base/lib/${abi}/libpixelsmith_core.so"
    cp "${libdir}/${abi}.so" "${work}/bundle/base/lib/${abi}/libflutter.so"
  done
  printf 'dex' > "${work}/bundle/base/dex/classes.dex"
  printf 'protobuf' > "${work}/bundle/base/manifest/AndroidManifest.xml"
  printf 'config' > "${work}/bundle/BundleConfig.pb"
  make_apk "${work}/bundle" "${work}/good.aab"
  MODEARG="aab"; CASE_TARGET="${work}/good.aab"
  run "aab-complete" 0 "BundleConfig.pb, base/dex/classes.dex, engine for every ABI"

  rm -f "${work}/bundle/BundleConfig.pb"
  make_apk "${work}/bundle" "${work}/no-config.aab"
  MODEARG="aab"; CASE_TARGET="${work}/no-config.aab"
  run "aab-no-bundle-config" 1 "a renamed zip with no BundleConfig.pb is not an app bundle"

  rm -rf "${work}/bundle/base/lib/armeabi-v7a"
  make_apk "${work}/bundle" "${work}/no-v7.aab"
  MODEARG="aab"; CASE_TARGET="${work}/no-v7.aab"
  run "aab-engine-missing-on-one-abi" 1 "32-bit ABI dropped from the bundle"

  # An APK path checked against a bundle layout: the assertion that a bundle is
  # not an empty shell has to survive the prefix, or it reports every correct
  # bundle as broken.
  printf 'config' > "${work}/bundle/BundleConfig.pb"
  make_apk "${work}/bundle" "${work}/good2.aab"

  # An APK with the wrong ABI set: an ABI in the archive that abiFilters never
  # declared. This is the "works on my device" packaging bug.
  mkdir -p "${work}/undeclared/lib/arm64-v8a" "${work}/undeclared/lib/armeabi-v7a" "${work}/undeclared/lib/x86_64" "${work}/undeclared/lib/x86"
  for abi in arm64-v8a armeabi-v7a x86_64 x86; do
    cp "${libdir}/${abi}.so" "${work}/undeclared/lib/${abi}/libpixelsmith_core.so"
    cp "${libdir}/${abi}.so" "${work}/undeclared/lib/${abi}/libflutter.so"
  done
  printf 'dex' > "${work}/undeclared/classes.dex"
  printf 'manifest' > "${work}/undeclared/AndroidManifest.xml"
  make_apk "${work}/undeclared" "${work}/undeclared.apk"
  MODEARG="apk"; CASE_TARGET="${work}/undeclared.apk"
  run "apk-undeclared-abi" 1 "x86 in the archive, not in abiFilters"

  printf 'not a zip at all' > "${work}/junk.apk"
  MODEARG="apk"; CASE_TARGET="${work}/junk.apk"
  run "apk-not-a-zip" 1 "a file that is not an archive"

  # ---- Windows ------------------------------------------------------------
  MODEARG="windows"; CASE_TARGET="${base}/win"
  run "windows-complete" 0 "EXE, engine DLL beside it, Dart snapshot"

  rm -rf "${work}/win-no-dll"
  cp -r "${base}/win" "${work}/win-no-dll"
  rm -f "${work}/win-no-dll/pixelsmith_core.dll"
  MODEARG="windows"; CASE_TARGET="${work}/win-no-dll"
  run "windows-no-engine-dll" 1 "no pixelsmith_core.dll next to the EXE"

  rm -rf "${work}/win-deep-dll"
  cp -r "${base}/win" "${work}/win-deep-dll"
  mkdir -p "${work}/win-deep-dll/lib"
  mv "${work}/win-deep-dll/pixelsmith_core.dll" "${work}/win-deep-dll/lib/"
  MODEARG="windows"; CASE_TARGET="${work}/win-deep-dll"
  run "windows-dll-not-next-to-exe" 1 "engine DLL in a subdirectory, where DynamicLibrary.open will not find it"

  rm -rf "${work}/win-no-snapshot"
  cp -r "${base}/win" "${work}/win-no-snapshot"
  rm -f "${work}/win-no-snapshot/data/app.so"
  MODEARG="windows"; CASE_TARGET="${work}/win-no-snapshot"
  run "windows-no-dart-snapshot" 1 "no data/app.so — the launcher would start and do nothing"

  rm -rf "${work}/win-tiny-exe"
  cp -r "${base}/win" "${work}/win-tiny-exe"
  truncate -s 900 "${work}/win-tiny-exe/shrinkray.exe"
  MODEARG="windows"; CASE_TARGET="${work}/win-tiny-exe"
  run "windows-exe-under-floor" 1 "EXE below the size floor"

  rm -rf "${work}/win-stub-dll"
  cp -r "${base}/win" "${work}/win-stub-dll"
  truncate -s 100000 "${work}/win-stub-dll/pixelsmith_core.dll"
  MODEARG="windows"; CASE_TARGET="${work}/win-stub-dll"
  run "windows-engine-dll-is-a-stub" 1 "engine DLL with no px_version in it"

  rm -rf "${work}/win-installer"
  cp -r "${base}/win" "${work}/win-installer"
  cp "${base}/win/shrinkray.exe" "${work}/win-installer/setup.exe"
  MODEARG="windows"; CASE_TARGET="${work}/win-installer"
  run "windows-has-installer" 1 "an installer in a bundle meant to be portable"

  # The portable ZIP path: the good bundle zipped, which is what the release
  # actually attaches.
  ( cd "${base}/win" && zip -q -r -X "${work}/good.zip" . ) || die "zip failed"
  MODEARG="windows"; CASE_TARGET="${work}/good.zip"
  run "windows-portable-zip" 0 "the zipped bundle extracts and holds"

  say ""
  say "  ${fired}/${cases} cases behaved as specified"
  if [ "${fired}" -ne "${cases}" ]; then
    say "verify-release-artifact.sh --self-test: FAIL"
    return 1
  fi
  say "verify-release-artifact.sh --self-test: PASS"
  return 0
}

if [ "${MODE}" = "--self-test" ]; then
  selftest
  exit $?
fi

case "${MODE}" in
  apk)      check_android "${TARGET}" apk ;;
  aab)      check_android "${TARGET}" aab ;;
  windows)  check_windows "${TARGET}" ;;
  *)        die "unknown mode: ${MODE}" ;;
esac
exit $?