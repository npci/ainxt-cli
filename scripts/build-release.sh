#!/usr/bin/env bash
# Build the `ainxt` binary for macOS, Linux, and Windows locally.
#
# Native builds always work for the host platform. Cross-compilation uses
# `cargo-zigbuild` (Linux targets) and the MSVC/GNU Windows target when the
# matching toolchain is available. Missing cross toolchains are skipped with a
# warning rather than failing the whole run.
#
# Usage:
#   scripts/build-release.sh [VERSION]
#
# Output: dist/ainxt-<VERSION>-<os>-<arch>[.exe]
#
# Headless mode is NOT a separate binary. The same `ainxt` binary runs headless
# (for scripting / CI) via:  ainxt -p "your prompt"
# So every artifact below supports interactive TUI, headless, and ACP/stdio.
set -uo pipefail

VERSION="${1:-$(git describe --tags --always 2>/dev/null || echo 0.0.0-dev)}"
VERSION="${VERSION#v}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
mkdir -p dist

# Compile the reported version (`ainxt --version`) to match the artifact name,
# so the binary self-reports the same version stamped into the filename.
export AINXT_VERSION="$VERSION"

# Windows cross-compile: mimalloc's C build trips -Werror on __DATE__/__TIME__
# under zig clang; relax those so the build proceeds.
export CFLAGS="${CFLAGS:-} -Wno-error -Wno-date-time"

PROFILE="release-dist"
PKG="ainxt-pager-bin"
BIN="ainxt"

# target-triple  os-label  arch      ext
TARGETS=(
  "aarch64-apple-darwin      darwin  aarch64 "
  "x86_64-apple-darwin       darwin  x86_64  "
  "x86_64-unknown-linux-gnu  linux   x86_64  "
  "aarch64-unknown-linux-gnu linux   aarch64 "
  "x86_64-pc-windows-gnu     win32   x86_64  .exe"
)

have() { command -v "$1" >/dev/null 2>&1; }
ZIG=""
if have cargo-zigbuild; then ZIG="zigbuild"; fi

build_one() {
  local triple="$1" os="$2" arch="$3" ext="$4"
  echo "==> $triple"
  rustup target add "$triple" >/dev/null 2>&1 || true

  local cmd="build"
  # Use zigbuild for cross targets when available: it ships the C toolchain and
  # a cross linker, which native `cargo build` lacks for Linux/Windows from a
  # macOS host. Native darwin targets use plain `cargo build`.
  case "$triple" in
    *-unknown-linux-gnu | *-pc-windows-gnu)
      if [ -n "$ZIG" ]; then cmd="$ZIG"; fi
      ;;
  esac

  if ! cargo "$cmd" --profile "$PROFILE" -p "$PKG" --bin "$BIN" --target "$triple" 2>/tmp/ainxt-build-"$triple".log; then
    echo "    SKIPPED ($triple) — toolchain unavailable or build failed. See /tmp/ainxt-build-$triple.log" >&2
    return 0
  fi
  local src="target/$triple/$PROFILE/$BIN$ext"
  local out="dist/ainxt-$VERSION-$os-$arch$ext"
  cp "$src" "$out" && echo "    -> $out"
}

for row in "${TARGETS[@]}"; do
  # shellcheck disable=SC2086
  set -- $row
  build_one "$1" "$2" "$3" "${4:-}"
done

echo
echo "Artifacts in dist/:"
ls -1 dist/ 2>/dev/null || echo "  (none built)"
