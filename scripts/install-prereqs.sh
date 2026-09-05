#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# AiNxt CLI — prerequisite auto-installer.
#
# Installs whatever setup.sh's prerequisite check found missing:
#   - Rust, via the official rustup installer (https://sh.rustup.rs). On
#     Windows, explicitly requests the MSVC host (--default-host
#     x86_64-pc-windows-msvc) — left to its own defaults, rustup silently
#     falls back to the GNU ABI when Visual Studio Build Tools aren't
#     detected yet, and the GNU ABI then depends on whichever mingw-w64-like
#     toolchain happens to be first on PATH, including old incompatible ones
#     (see the "Windows compatibility" write-up for the class of failure that
#     causes: dlltool "Invalid bfd target" deep inside a build).
#   - protoc: on Unix, dotslash's official prebuilt binary release on GitHub
#     (provides the hermetic bin/protoc via its `#!/usr/bin/env dotslash`
#     self-execution) — no C compiler required. On Windows, a real
#     protoc.exe instead, via winget (Google.Protobuf) — bin/protoc's
#     shebang trick has no Windows equivalent at all (running it directly
#     fails with "%1 is not a valid Win32 application"), so `dotslash` on
#     PATH does nothing for protoc resolution there.
#   - On Windows: Visual Studio Build Tools (the MSVC compiler/linker), via
#     winget with the C++ workload. This is what lets the MSVC host link
#     anything at all; without it `cargo build` fails immediately.
#   - On Linux (including WSL): a C compiler/linker (gcc/clang) plus
#     pkg-config, via the distro's own package manager (apt/dnf/yum/pacman/
#     zypper) — whichever is found on PATH first. rustc uses `cc` as its
#     default linker on this target; a minimal distro image often ships
#     without one at all. Requires sudo; the password prompt is left visible
#     (not redirected to the log file) so it doesn't look like a hang.
#
# Does NOT install a Windows mingw-w64 (MinGW) compiler/linker toolchain.
# That path (MSYS2 + `pacman`) is a second, independent package manager that
# can restart its own runtime mid-update and may collide with whatever's
# already providing `gcc`/`dlltool` on the machine — too invasive to run
# unattended, and superseded by defaulting to MSVC above anyway. Someone who
# deliberately wants the GNU ABI instead still gets manual instructions from
# setup.sh, the same way it already does for low disk space.
#
# Messages are written for someone who may not have a software development
# background but is comfortable running commands in a terminal (e.g. IT
# support staff). Technical names are kept, each with a one-line explanation
# of what it's for and why it's needed. The underlying installers' raw,
# very technical output is redirected to a log file instead of the screen,
# and only surfaced (by path) if a step actually fails.
#
# Works on macOS, Linux, and Windows under a bash-compatible shell (WSL,
# Git Bash, or MSYS2) — the same shells setup.sh itself requires. Safe to
# re-run: anything already installed is left alone. Does not touch disk-space
# shortfalls; that's on you.
#
# Usage: scripts/install-prereqs.sh
#
set -uo pipefail   # not -e: keep going and report everything that failed

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

if [ -t 1 ]; then
  B=$'\033[1m'; R=$'\033[0m'; GRN=$'\033[32m'; YEL=$'\033[33m'; RED=$'\033[31m'
else
  B=''; R=''; GRN=''; YEL=''; RED=''
fi
ok()   { printf '  %s✓%s %s\n' "$GRN" "$R" "$1"; }
warn() { printf '  %s!%s %s\n' "$YEL" "$R" "$1"; }
bad()  { printf '  %s✗%s %s\n' "$RED" "$R" "$1"; }
hdr()  { printf '\n%s%s%s\n' "$B" "$1" "$R"; }

# --- OS detection (for the banner only; every step below works the same way
# regardless of platform) --------------------------------------------------------
detect_os_label() {
  case "$(uname -s 2>/dev/null)" in
    Darwin) echo "macOS" ;;
    Linux)
      if [ -n "${WSL_DISTRO_NAME:-}" ] || grep -qi microsoft /proc/version 2>/dev/null; then
        echo "Windows (WSL)"
      else
        echo "Linux"
      fi
      ;;
    MINGW*|MSYS*) echo "Windows (Git Bash / MSYS2)" ;;
    CYGWIN*)      echo "Windows (Cygwin)" ;;
    *)            echo "unrecognized platform" ;;
  esac
}

is_windows() {
  case "$(uname -s 2>/dev/null)" in
    MINGW*|MSYS*|CYGWIN*) return 0 ;;
    *) return 1 ;;
  esac
}

hdr "Installing prerequisites"
printf 'Detected OS: %s\n' "$(detect_os_label)"
printf 'This requires an internet connection and may take a few minutes.\n'

INSTALL_FAILED=0

# Checked once, up front: both Windows-only winget steps below (protoc,
# Build Tools) would otherwise hit the exact same failure twice, each time
# with the real reason buried in a log file instead of shown here immediately.
WINGET_MISSING=0
if is_windows && ! command -v winget >/dev/null 2>&1; then
  WINGET_MISSING=1
fi

# --- fetch helper: curl, falling back to wget ----------------------------------
fetch() {  # fetch URL DEST
  if command -v curl >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSfL "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$2" "$1"
  else
    return 1
  fi
}

# --- dotslash: install the official prebuilt binary release -------------------
# No C compiler required (unlike `cargo install dotslash`, which compiles it
# from source and fails on machines without a working linker for the host
# target — e.g. Windows without a properly configured MinGW-w64/MSVC toolchain).
DOTSLASH_RELEASE_BASE="https://github.com/facebook/dotslash/releases/latest/download"

dotslash_asset_name() {
  case "$(uname -s 2>/dev/null)" in
    Darwin) echo "dotslash-macos.tar.gz" ;;
    Linux)
      case "$(uname -m 2>/dev/null)" in
        aarch64|arm64) echo "dotslash-linux-musl.aarch64.tar.gz" ;;
        *)             echo "dotslash-linux-musl.x86_64.tar.gz" ;;
      esac
      ;;
    MINGW*|MSYS*|CYGWIN*) echo "dotslash-windows.tar.gz" ;;
    *) echo "" ;;
  esac
}

install_dotslash_binary() {  # install_dotslash_binary DEST_DIR
  local dest_dir="$1"
  local asset
  asset="$(dotslash_asset_name)"
  if [ -z "$asset" ]; then
    echo "Unrecognized platform; no prebuilt dotslash binary available for it."
    return 1
  fi

  local tmp_tar
  tmp_tar="$(mktemp -t dotslash-download.XXXXXX.tar.gz)"
  echo "Downloading $DOTSLASH_RELEASE_BASE/$asset"
  if ! fetch "$DOTSLASH_RELEASE_BASE/$asset" "$tmp_tar"; then
    echo "Download failed."
    rm -f "$tmp_tar"
    return 1
  fi

  mkdir -p "$dest_dir"
  tar -xzf "$tmp_tar" -C "$dest_dir"
  rm -f "$tmp_tar"
  chmod +x "$dest_dir/dotslash" 2>/dev/null
  chmod +x "$dest_dir/dotslash.exe" 2>/dev/null

  # macOS: a binary downloaded directly (not via a signed installer/package
  # manager) carries no signature at all, and macOS — especially Apple
  # Silicon — can refuse to exec it with "Operation not permitted" (EPERM),
  # which looks nothing like a missing-file or permission-bit error and is
  # easy to mistake for a broken checkout. Ad-hoc sign it and strip any
  # quarantine flag so it actually runs; both are no-ops if unneeded, and
  # `codesign`/`xattr` only exist on macOS so this is harmless elsewhere.
  if [ "$(uname -s 2>/dev/null)" = "Darwin" ] && [ -x "$dest_dir/dotslash" ]; then
    xattr -d com.apple.quarantine "$dest_dir/dotslash" 2>/dev/null || true
    codesign --force --deep --sign - "$dest_dir/dotslash" 2>/dev/null || true
  fi

  [ -x "$dest_dir/dotslash" ] || [ -x "$dest_dir/dotslash.exe" ]
}

# --- protoc on Windows: a real protoc.exe, not dotslash -----------------------
# bin/protoc's `#!/usr/bin/env dotslash` self-execution trick only works on
# Unix (shebang support); Windows can't execute it directly at all
# ("%1 is not a valid Win32 application"), so `dotslash` on PATH does nothing
# for protoc resolution here. The build's own protoc lookup
# (crates/build/ainxt-proto-build) falls back to a bare `protoc` on PATH as
# its last resort, so installing a real protoc.exe there is what's needed.
install_protoc_windows() {
  if ! command -v winget >/dev/null 2>&1; then
    echo "winget is not available to install protoc automatically." >&2
    return 1
  fi
  winget install --id Google.Protobuf -e --silent \
    --accept-source-agreements --accept-package-agreements || return 1

  local found
  # $LOCALAPPDATA is a Windows-style path (C:\Users\...); concatenating it
  # with forward slashes for a glob corrupts it (the drive letter gets
  # dropped) under Git Bash/MSYS2's PATH translation, so convert it to a
  # POSIX path first via cygpath (falling back to the $HOME-relative
  # equivalent, which is always POSIX-style here, if cygpath is unavailable).
  local local_appdata
  local_appdata="$(command -v cygpath >/dev/null 2>&1 && cygpath -u "$LOCALAPPDATA" || echo "$HOME/AppData/Local")"
  found="$(ls -1 "$local_appdata/Microsoft/WinGet/Packages"/Google.Protobuf_*/bin/protoc.exe 2>/dev/null | head -1)"
  if [ -z "$found" ] || [ ! -x "$found" ]; then
    echo "protoc.exe not found after installing the winget package." >&2
    return 1
  fi
  echo "$found"
}

# --- Visual Studio Build Tools: the MSVC compiler/linker (Windows only) -------
VSWHERE="/c/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe"

msvc_build_tools_ok() {
  is_windows || return 0
  [ -x "$VSWHERE" ] || return 1
  "$VSWHERE" -latest -products '*' \
    -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 \
    -property installationPath >/dev/null 2>&1
}

install_msvc_build_tools() {
  if ! command -v winget >/dev/null 2>&1; then
    echo "winget is not available to install Visual Studio Build Tools automatically."
    return 1
  fi
  winget install --id Microsoft.VisualStudio.BuildTools -e --silent \
    --accept-source-agreements --accept-package-agreements \
    --override "--wait --quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended" \
    || return 1
  msvc_build_tools_ok
}

# Runs a command with its output redirected to a log file, printing a
# progress dot every couple of seconds so it's clear the process is still
# running (some of these steps take a few minutes with no output otherwise).
# Returns the command's exit code.
run_quietly() {  # run_quietly LOGFILE CMD [ARGS...]
  local logfile="$1"; shift
  "$@" >"$logfile" 2>&1 &
  local pid=$!
  printf '  Installing '
  while kill -0 "$pid" 2>/dev/null; do
    printf '.'
    sleep 2
  done
  printf '\n'
  wait "$pid"
}

# Print the last few lines of a failed step's log directly to the screen.
# The path alone is useless in a remote support conversation (over chat,
# screen-photo, etc.) where nobody can actually open the file — this way the
# real underlying error (winget/curl/rustup/apt output) is visible
# immediately, without anyone needing to fetch it first.
show_log_tail() {  # show_log_tail LOGFILE
  local logfile="$1"
  [ -s "$logfile" ] || return 0
  printf '  %s--- last lines of the log ---%s\n' "$YEL" "$R"
  tail -n 8 "$logfile" | sed 's/^/  | /'
  printf '  %s------------------------------%s\n' "$YEL" "$R"
}

# --- Step 1: Rust ---------------------------------------------------------------
hdr "Step 1 of 3: Rust (programming language and build toolchain)"
if command -v cargo >/dev/null 2>&1; then
  ok "Already installed — skipping."
else
  echo "  Installing via the official rustup installer (https://rustup.rs/)."
  RUSTUP_SH="$(mktemp -t rustup-init.XXXXXX)"
  RUST_LOG="$(mktemp -t ainxt-install-rust.XXXXXX.log)"
  if ! fetch https://sh.rustup.rs "$RUSTUP_SH"; then
    bad "Could not download the rustup installer. Check your internet connection"
    bad "and re-run this script."
    bad "To install manually instead: https://rustup.rs/"
    INSTALL_FAILED=1
  else
    # --default-toolchain none: this project pins its own exact Rust version
    # in rust-toolchain.toml and installs it automatically on first build, so
    # a second, generic toolchain is not needed here.
    RUSTUP_ARGS=(-y --default-toolchain none --profile minimal)
    if is_windows; then
      # Force the MSVC host explicitly. Left to its own defaults, rustup
      # silently falls back to the GNU ABI when Visual Studio Build Tools
      # aren't detected at install time — and the GNU ABI then depends on
      # whichever mingw-w64-like toolchain happens to be first on PATH,
      # including old incompatible ones. Step 3 below installs the MSVC
      # Build Tools this host actually needs.
      RUSTUP_ARGS+=(--default-host x86_64-pc-windows-msvc)
    fi
    run_quietly "$RUST_LOG" sh "$RUSTUP_SH" "${RUSTUP_ARGS[@]}"
    RUSTUP_STATUS=$?
    rm -f "$RUSTUP_SH"
    # shellcheck disable=SC1090
    [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
    export PATH="$HOME/.cargo/bin:$PATH"
    if [ "$RUSTUP_STATUS" -eq 0 ] && command -v cargo >/dev/null 2>&1; then
      ok "Rust installed successfully."
    else
      bad "Rust installation failed."
      bad "Details logged to: $RUST_LOG"
      show_log_tail "$RUST_LOG"
      bad "Re-run this script — this is often a temporary network issue."
      bad "If it persists, share that log file with technical support."
      INSTALL_FAILED=1
    fi
  fi
fi

# Idempotent regardless of whether Rust was just installed above or was
# already present: make sure this repo's pinned toolchain (rust-toolchain.toml)
# resolves to the MSVC host on Windows when it's auto-installed on first use.
if is_windows && command -v rustup >/dev/null 2>&1; then
  rustup set default-host x86_64-pc-windows-msvc >/dev/null 2>&1 || true
fi

# --- Step 2: protoc ---------------------------------------------------------
hdr "Step 2 of 3: protoc (Protocol Buffers compiler)"
if [ -n "${PROTOC:-}" ] && [ -x "${PROTOC:-}" ]; then
  ok "protoc already available via \$PROTOC — skipping."
elif command -v protoc >/dev/null 2>&1; then
  ok "Compatible system protoc found — skipping."
elif ! is_windows && command -v dotslash >/dev/null 2>&1; then
  ok "dotslash already installed — skipping."
elif is_windows && [ "$WINGET_MISSING" -eq 1 ]; then
  bad "winget (Windows Package Manager) isn't available on this machine, so"
  bad "  protoc can't be installed automatically."
  bad "  Install \"App Installer\" from the Microsoft Store, then re-run this"
  bad "  script. Or skip winget entirely: download protoc.exe from"
  bad "  https://github.com/protocolbuffers/protobuf/releases, then either add"
  bad "  its folder to PATH or set \$PROTOC to its full path, then re-run."
  INSTALL_FAILED=1
elif is_windows; then
  echo "  Installing protoc via winget (a real protoc.exe on PATH)."
  PROTOC_LOG="$(mktemp -t ainxt-install-protoc.XXXXXX.log)"
  if run_quietly "$PROTOC_LOG" install_protoc_windows && \
     PROTOC_EXE="$(tail -n 1 "$PROTOC_LOG")" && [ -x "$PROTOC_EXE" ]; then
    export PATH="$(dirname "$PROTOC_EXE"):$PATH"
    ok "protoc installed successfully. ($PROTOC_EXE)"
  else
    bad "protoc installation failed."
    bad "Details logged to: $PROTOC_LOG"
    show_log_tail "$PROTOC_LOG"
    bad "Re-run this script — this is often a temporary network issue."
    bad "If it persists, share that log file with technical support."
    INSTALL_FAILED=1
  fi
else
  # Installed as a prebuilt binary (no C compiler needed) into the same
  # directory rustup uses for cargo-installed tools, so setup.sh's existing
  # `export PATH="$HOME/.cargo/bin:$PATH"` re-check picks it up automatically.
  echo "  Installing dotslash from the official prebuilt release."
  DOTSLASH_LOG="$(mktemp -t ainxt-install-dotslash.XXXXXX.log)"
  DOTSLASH_INSTALL_DIR="$HOME/.cargo/bin"
  if run_quietly "$DOTSLASH_LOG" install_dotslash_binary "$DOTSLASH_INSTALL_DIR"; then
    export PATH="$DOTSLASH_INSTALL_DIR:$PATH"
    ok "dotslash installed successfully."
  else
    bad "dotslash installation failed."
    bad "Details logged to: $DOTSLASH_LOG"
    show_log_tail "$DOTSLASH_LOG"
    bad "Re-run this script — this is often a temporary network issue."
    bad "If it persists, share that log file with technical support."
    INSTALL_FAILED=1
  fi
fi

# --- Linux C compiler/linker: gcc/clang + pkg-config via the distro's own
# package manager. Tries each in the order a machine is likely to have it;
# stops at the first one found. ------------------------------------------------
linux_c_compiler_ok() {
  command -v cc >/dev/null 2>&1 || command -v gcc >/dev/null 2>&1 || command -v clang >/dev/null 2>&1
}

# Root (common in minimal containers, which often have no `sudo` binary at
# all) needs no elevation; everyone else goes through sudo.
as_root() {
  if [ "$(id -u)" = "0" ]; then
    "$@"
  else
    sudo "$@"
  fi
}

install_linux_c_compiler() {
  if command -v apt-get >/dev/null 2>&1; then
    as_root apt-get update && as_root apt-get install -y build-essential pkg-config
  elif command -v dnf >/dev/null 2>&1; then
    as_root dnf groupinstall -y "Development Tools" && as_root dnf install -y pkgconf-pkg-config
  elif command -v yum >/dev/null 2>&1; then
    as_root yum groupinstall -y "Development Tools" && as_root yum install -y pkgconfig
  elif command -v pacman >/dev/null 2>&1; then
    as_root pacman -Sy --noconfirm base-devel pkgconf
  elif command -v zypper >/dev/null 2>&1; then
    as_root zypper --non-interactive install -t pattern devel_basis && as_root zypper --non-interactive install pkg-config
  else
    echo "No supported package manager found (apt/dnf/yum/pacman/zypper)." >&2
    return 1
  fi
}

# --- Step 3: C compiler/linker (MSVC on Windows, gcc/clang on Linux) ----------
hdr "Step 3 of 3: C compiler/linker"
if is_windows; then
  if msvc_build_tools_ok; then
    ok "Visual Studio Build Tools already installed — skipping."
  elif [ "$WINGET_MISSING" -eq 1 ]; then
    bad "winget (Windows Package Manager) isn't available on this machine, so"
    bad "  Visual Studio Build Tools can't be installed automatically."
    bad "  Install \"App Installer\" from the Microsoft Store, then re-run this"
    bad "  script. Or install manually: https://visualstudio.microsoft.com/"
    bad "  visual-cpp-build-tools/ — choose the \"Desktop development with C++\""
    bad "  workload, then re-run."
    INSTALL_FAILED=1
  else
    echo "  Installing via winget (Microsoft's official installer). This is a"
    echo "  large download (several GB) and can take a while even on a fast"
    echo "  connection."
    MSVC_LOG="$(mktemp -t ainxt-install-msvc.XXXXXX.log)"
    if run_quietly "$MSVC_LOG" install_msvc_build_tools; then
      ok "Visual Studio Build Tools installed successfully."
    else
      bad "Visual Studio Build Tools installation failed."
      bad "Details logged to: $MSVC_LOG"
      show_log_tail "$MSVC_LOG"
      bad "Re-run this script — this is often a temporary network issue."
      bad "If it persists, share that log file with technical support."
      INSTALL_FAILED=1
    fi
  fi
elif [ "$(uname -s 2>/dev/null)" = "Linux" ]; then
  if linux_c_compiler_ok; then
    ok "Already installed — skipping."
  else
    echo "  Installing via your distro's package manager. This needs sudo —"
    echo "  you may be prompted for your password below (left visible on"
    echo "  purpose, unlike the other steps, so it doesn't look like a hang)."
    if [ "$(id -u)" != "0" ] && ! sudo -v; then
      bad "Could not obtain sudo access — installation cancelled."
      INSTALL_FAILED=1
    else
      LINUX_CC_LOG="$(mktemp -t ainxt-install-cc.XXXXXX.log)"
      if run_quietly "$LINUX_CC_LOG" install_linux_c_compiler && linux_c_compiler_ok; then
        ok "C compiler/linker installed successfully."
      else
        bad "C compiler/linker installation failed."
        bad "Details logged to: $LINUX_CC_LOG"
        show_log_tail "$LINUX_CC_LOG"
        bad "Re-run this script, or install manually for your distro, e.g.:"
        bad "  Debian/Ubuntu: sudo apt install build-essential pkg-config"
        bad "  Fedora/RHEL:   sudo dnf groupinstall \"Development Tools\""
        bad "  Arch:          sudo pacman -S base-devel"
        INSTALL_FAILED=1
      fi
    fi
  fi
else
  ok "Not applicable on this OS — skipping."
fi

if [ "$INSTALL_FAILED" -ne 0 ]; then
  printf '\n%sOne or more prerequisites could not be installed automatically.%s\n' "$RED" "$R"
  printf 'See the messages above for next steps, or share them with technical support.\n'
  exit 1
fi

printf '\n%sPrerequisites installed successfully.%s\n' "$GRN" "$R"
printf 'Returning to setup.sh to continue automatically.\n'
