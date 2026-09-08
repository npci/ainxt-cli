#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# AiNxt CLI — first-run setup.
#
# Verifies prerequisites, builds the `ainxt` binary, creates .env from
# env.example, and prints what to run next. Safe to re-run.
#
#   ./setup.sh                 # check prerequisites, then release build
#   ./setup.sh --debug         # build the debug binary instead
#   ./setup.sh --release       # build the optimised (release-dist) binary [default]
#   ./setup.sh --check         # inspect prerequisites only; change nothing
#   ./setup.sh --auto-install  # if prerequisites are missing, install them without asking
#   ./setup.sh --no-auto-install  # if prerequisites are missing, only show manual instructions
#
# When prerequisites are missing and the terminal is interactive, you'll be
# asked whether to auto-install them (via scripts/install-prereqs.sh) or fix
# them yourself. --auto-install / --no-auto-install skip that question, which
# is useful for CI or scripted runs.
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$REPO_ROOT"

CHECK_ONLY=0
PROFILE="release-dist"
# Unset = ask interactively if something is missing. 1 = always auto-install.
# 0 = never auto-install (old behaviour: print instructions and exit).
AUTO_INSTALL=""

while [ $# -gt 0 ]; do
  case "$1" in
    --check)          CHECK_ONLY=1 ;;
    --release)        PROFILE="release-dist" ;;
    --debug)          PROFILE="debug" ;;
    --auto-install)    AUTO_INSTALL=1 ;;
    --no-auto-install) AUTO_INSTALL=0 ;;
    -h|--help)
      sed -n '4,18p' "$0" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *)
      printf 'setup.sh: unknown option %s (try --help)\n' "$1" >&2
      exit 2 ;;
  esac
  shift
done

# --- output helpers -----------------------------------------------------------
if [ -t 1 ]; then
  B=$'\033[1m'; R=$'\033[0m'; GRN=$'\033[32m'; YEL=$'\033[33m'; RED=$'\033[31m'
else
  B=''; R=''; GRN=''; YEL=''; RED=''
fi
ok()   { printf '  %s✓%s %s\n' "$GRN" "$R" "$1"; }
warn() { printf '  %s!%s %s\n' "$YEL" "$R" "$1"; }
bad()  { printf '  %s✗%s %s\n' "$RED" "$R" "$1"; }
hdr()  { printf '\n%s%s%s\n' "$B" "$1" "$R"; }

# --- OS detection (informational; install-prereqs.sh re-detects for itself) --
detect_os() {
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
    *)            echo "unknown" ;;
  esac
}

is_windows() {
  case "$(uname -s 2>/dev/null)" in
    MINGW*|MSYS*|CYGWIN*) return 0 ;;
    *) return 1 ;;
  esac
}

# rustc's active host on Windows determines which linker matters: MSVC needs
# Visual Studio Build Tools; the GNU ABI needs a real mingw-w64 toolchain.
# Empty if Rust isn't installed yet (nothing to check until it is).
rust_host() {
  command -v rustc >/dev/null 2>&1 || return 0
  rustc -vV 2>/dev/null | sed -n 's/^host: //p'
}

VSWHERE="/c/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe"

# scripts/install-prereqs.sh can install this one automatically (a single,
# official winget package) — unlike the mingw-w64 check below.
msvc_build_tools_ok() {
  [ -x "$VSWHERE" ] || return 1
  # vswhere exits 0 even when -requires matches nothing (verified against a
  # bogus component id) — the only real signal is whether it printed a path.
  local path
  path="$("$VSWHERE" -latest -products '*' \
    -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 \
    -property installationPath 2>/dev/null)"
  [ -n "$path" ]
}

# Only relevant for anyone still deliberately on Rust's GNU host
# (x86_64-pc-windows-gnu) rather than the MSVC default this script now sets
# up. Returns success (nothing to fix) on every other host.
mingw_w64_linker_ok() {
  command -v dlltool >/dev/null 2>&1 || return 1
  # The legacy 32-bit-only mingw.org toolchain's dlltool supports only
  # i386/arm/ppc/... — no x86-64 entry at all — which is exactly what
  # produces "Invalid bfd target" when rustc links a 64-bit import library.
  # A real mingw-w64 dlltool lists x86-64 here.
  dlltool --help 2>&1 | grep -qi 'x86-64'
}

# --- 1. prerequisites ---------------------------------------------------------
# Wrapped in a function so it can be re-run after an auto-install pass.
FAILED=0
INSTALLABLE_MISSING=0   # missing pieces that scripts/install-prereqs.sh can fix

check_prereqs() {
  FAILED=0
  INSTALLABLE_MISSING=0
  hdr "Step 1: Checking prerequisites (detected OS: $(detect_os))"

  # Rust. rust-toolchain.toml pins the version; rustup installs it on first build.
  if command -v cargo >/dev/null 2>&1; then
    ok "Rust is installed. (cargo $(cargo --version 2>/dev/null | cut -d' ' -f1-2))"
  else
    bad "Rust is not installed. Rust is the programming language and build"
    bad "  tooling this application is written in and compiled with."
    bad "  This script can install it automatically — answer 'y' when prompted"
    bad "  below."
    bad "  To install it yourself instead: visit https://rustup.rs/, follow the"
    bad "  instructions there, then re-run this script."
    FAILED=1
    INSTALLABLE_MISSING=1
  fi

  if [ -f rust-toolchain.toml ]; then
    PINNED="$(sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml | head -1)"
    [ -n "$PINNED" ] && ok "Required Rust version ($PINNED) will be installed automatically on first build."
  fi

  # protoc. On Unix, the repo ships a hermetic protoc under bin/ that
  # self-executes via dotslash's `#!/usr/bin/env dotslash` shebang line. That
  # mechanism does not exist on Windows at all (no shebang support), so
  # having `dotslash` on PATH does nothing there — Windows needs a real
  # protoc.exe on PATH (or $PROTOC set); the build's own protoc-lookup
  # (crates/build/ainxt-proto-build) already tries a bare `protoc` on PATH
  # as its last resort, so PATH alone is sufficient, $PROTOC is optional.
  if [ -n "${PROTOC:-}" ] && [ -x "${PROTOC:-}" ]; then
    ok "protoc is available. (\$PROTOC=$PROTOC)"
  elif command -v protoc >/dev/null 2>&1; then
    ok "protoc is installed. ($(command -v protoc))"
  elif ! is_windows && command -v dotslash >/dev/null 2>&1; then
    ok "dotslash is installed. ($(command -v dotslash)) — provides protoc for this build."
  else
    bad "protoc (Protocol Buffers compiler) is not installed — required to build"
    bad "  this application's API definitions."
    if is_windows; then
      bad "  This script installs a real protoc.exe via winget (Google.Protobuf)."
    else
      bad "  This script installs it via dotslash, a small tool that fetches the"
      bad "  exact version this repo needs."
    fi
    bad "  This script can install it automatically — answer 'y' when prompted"
    bad "  below."
    FAILED=1
    INSTALLABLE_MISSING=1
  fi

  # Linux (including WSL): cargo build itself needs a working C compiler —
  # rustc uses `cc` as its default linker on this target, and a minimal
  # distro image (a fresh WSL Ubuntu install, a slim container, ...) often
  # has none. Without one, the build fails deep into compiling with
  # "linker `cc` not found", not at any check this script could otherwise
  # catch earlier. Unlike the Windows GNU-host case below, there's exactly
  # one official system package manager to reach for here, so this is safe
  # to offer as an automatic install rather than manual-only instructions.
  if ! is_windows && [ "$(uname -s 2>/dev/null)" = "Linux" ]; then
    if command -v cc >/dev/null 2>&1 || command -v gcc >/dev/null 2>&1 || command -v clang >/dev/null 2>&1; then
      ok "C compiler/linker found. ($(command -v cc || command -v gcc || command -v clang))"
    else
      bad "No C compiler/linker (gcc/clang) found. rustc needs one to link"
      bad "  anything at all, even a trivial program."
      bad "  This script can install it automatically via your distro's package"
      bad "  manager (requires sudo) — answer 'y' when prompted below."
      bad "  To install it yourself instead:"
      bad "    Debian/Ubuntu: sudo apt install build-essential pkg-config"
      bad "    Fedora/RHEL:   sudo dnf groupinstall \"Development Tools\""
      bad "    Arch:          sudo pacman -S base-devel"
      FAILED=1
      INSTALLABLE_MISSING=1
    fi
  fi

  # Windows: cargo build itself (not just this script) needs a working
  # linker to link almost anything (windows-sys, getrandom, ...) — without
  # one, the build fails deep into compiling, not at any check this script
  # could otherwise catch earlier.
  if is_windows; then
    case "$(rust_host)" in
      *-pc-windows-msvc)
        if msvc_build_tools_ok; then
          ok "Visual Studio Build Tools (MSVC compiler/linker) found."
        else
          bad "Visual Studio Build Tools (MSVC compiler/linker) not found."
          bad "  Rust's MSVC host needs it to link anything at all."
          bad "  This script can install it automatically — answer 'y' when"
          bad "  prompted below. It's a large download (several GB)."
          bad "  To install it yourself instead: https://visualstudio.microsoft.com/"
          bad "  visual-cpp-build-tools/ — choose the 'Desktop development with"
          bad "  C++' workload, then re-run this script."
          FAILED=1
          INSTALLABLE_MISSING=1
        fi
        ;;
      *-pc-windows-gnu)
        # Not auto-installable: fixing this means installing a second,
        # independent package manager (MSYS2 + pacman) system-wide, which is
        # too invasive to run unattended — same treatment as low disk space
        # below. Anyone hitting this chose the GNU host deliberately; the
        # default this script sets up for a fresh install is MSVC instead.
        if ! mingw_w64_linker_ok; then
          bad "No working 64-bit mingw-w64 compiler/linker found on PATH. Rust's"
          bad "  x86_64-pc-windows-gnu target needs one to link anything —"
          bad "  without it, the build fails deep into compiling with"
          bad "  'dlltool ... Invalid bfd target', even though Rust and protoc"
          bad "  are both fine."
          bad "  This usually means the only MinGW on PATH is the old"
          bad "  32-bit-only mingw.org project (a different, incompatible"
          bad "  project despite the similar name) rather than mingw-w64."
          bad "  This script cannot fix this automatically — install MSYS2"
          bad "  (https://www.msys2.org/), then from an MSYS2 shell run:"
          bad "    pacman -S mingw-w64-x86_64-gcc"
          bad "  then make sure C:\\msys64\\mingw64\\bin comes before any"
          bad "  other MinGW on PATH, open a new terminal, and re-run this"
          bad "  script. Or switch to the MSVC host instead (this script's"
          bad "  default for a fresh install): 'rustup default"
          bad "  1.96.0-x86_64-pc-windows-msvc' after installing Visual Studio"
          bad "  Build Tools."
          FAILED=1
        fi
        ;;
      *) : ;;  # Rust not installed yet, or a host with no linker concerns here
    esac
  fi

  # Disk. A debug target/ is ~10 GB; release-dist is larger.
  NEED_GB=10
  [ "$PROFILE" = "release-dist" ] && NEED_GB=15
  if AVAIL_KB="$(df -Pk . 2>/dev/null | awk 'NR==2 {print $4}')" && [ -n "$AVAIL_KB" ]; then
    AVAIL_GB=$(( AVAIL_KB / 1024 / 1024 ))
    if [ "$AVAIL_GB" -ge "$NEED_GB" ]; then
      ok "Sufficient disk space (${AVAIL_GB} GB free, ~${NEED_GB} GB required)."
    else
      bad "Insufficient disk space: only ${AVAIL_GB} GB free, ~${NEED_GB} GB required."
      bad "  Free up disk space and re-run this script. This is the one prerequisite"
      bad "  this script cannot fix automatically."
      FAILED=1
      # Disk space can't be auto-installed; don't set INSTALLABLE_MISSING for this.
    fi
  else
    warn "Could not determine free disk space; this build needs ~${NEED_GB} GB."
  fi
}

check_prereqs

if [ "$FAILED" -ne 0 ]; then
  if [ "$CHECK_ONLY" -eq 1 ]; then
    printf '\n%sPrerequisites missing — see above.%s\n' "$RED" "$R"
    exit 1
  fi

  DO_INSTALL=0
  if [ "$INSTALLABLE_MISSING" -eq 0 ]; then
    : # only disk space is short; nothing scripts/install-prereqs.sh can do about it
  elif [ "$AUTO_INSTALL" = "1" ]; then
    DO_INSTALL=1
  elif [ "$AUTO_INSTALL" = "0" ]; then
    DO_INSTALL=0
  elif [ -t 0 ]; then
    printf '\n%sSome prerequisites are missing.%s\n' "$YEL" "$R"
    printf 'This script can install them automatically using the official installers\n'
    printf '(rustup for Rust, a prebuilt release for dotslash, and on Windows,\n'
    printf 'winget for Visual Studio Build Tools if needed). This requires an\n'
    printf 'internet connection and may take a while — Build Tools alone can be\n'
    printf 'several GB.\n'
    read -r -p "Install missing prerequisites automatically? [y/N] " REPLY
    case "$REPLY" in
      [yY]|[yY][eE][sS]) DO_INSTALL=1 ;;
      *) DO_INSTALL=0 ;;
    esac
  else
    # Not a terminal (e.g. piped input) and no --auto-install/--no-auto-install
    # flag was given: don't hang on a prompt nobody can answer.
    DO_INSTALL=0
  fi

  if [ "$DO_INSTALL" -eq 1 ]; then
    hdr "Step 2: Installing missing prerequisites"
    # Same executable-bit loss as bin/protoc below: a plain file copy (e.g. a
    # downloaded zip instead of `git clone`) can lose +x on this script too.
    if [ -f "$REPO_ROOT/scripts/install-prereqs.sh" ] && [ ! -x "$REPO_ROOT/scripts/install-prereqs.sh" ]; then
      chmod +x "$REPO_ROOT/scripts/install-prereqs.sh"
      ok "Fixed file permissions on scripts/install-prereqs.sh (would have failed here)."
    fi
    if "$REPO_ROOT/scripts/install-prereqs.sh"; then
      # Pick up cargo/dotslash that the installer just placed on PATH.
      # shellcheck disable=SC1090
      [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
      export PATH="$HOME/.cargo/bin:$PATH"

      # Same PATH-loss issue, Windows-only: install-prereqs.sh installs a
      # real protoc.exe via winget and exports its directory onto PATH, but
      # that only affects install-prereqs.sh's own process (it runs as a
      # separate script, not sourced) — this shell never sees it. Re-locate
      # it the same way install-prereqs.sh did and export it here too.
      if is_windows; then
        WINGET_LOCALAPPDATA="$(command -v cygpath >/dev/null 2>&1 && cygpath -u "$LOCALAPPDATA" || echo "$HOME/AppData/Local")"
        PROTOC_EXE="$(ls -1 "$WINGET_LOCALAPPDATA/Microsoft/WinGet/Packages"/Google.Protobuf_*/bin/protoc.exe 2>/dev/null | head -1)"
        [ -n "$PROTOC_EXE" ] && export PATH="$(dirname "$PROTOC_EXE"):$PATH"
      fi

      hdr "Step 3: Re-checking prerequisites"
      check_prereqs
    else
      bad "Automatic installation did not complete successfully — see above for details."
    fi
  fi

  if [ "$FAILED" -ne 0 ]; then
    printf '\n%sPrerequisites are still missing — see above.%s\n' "$RED" "$R"
    printf 'If you are unsure how to proceed, share the messages above with your\n'
    printf 'IT or technical support team.\n'
    exit 1
  fi
fi

if [ "$CHECK_ONLY" -eq 1 ]; then
  printf '\n%sPrerequisites satisfied.%s Re-run without --check to build.\n' "$GRN" "$R"
  exit 0
fi

# A plain file copy (e.g. from a downloaded zip instead of `git clone`) can
# lose the executable bit on files that need it. bin/protoc is a DotSlash
# script the build runs directly; without +x it fails deep inside a cargo
# build script with a confusing "Permission denied" error.
if [ -f bin/protoc ] && [ ! -x bin/protoc ]; then
  chmod +x bin/protoc
  ok "Fixed file permissions on bin/protoc (would have failed the build)."
fi

# macOS-specific variant of the same problem: a ZIP downloaded via a
# browser (rather than `git clone`) tags every extracted file with a
# com.apple.quarantine attribute, which blocks execution outright —
# "operation not permitted", even with +x set — independently of the
# executable-bit check above. Hit a real user this way.
if [ "$(uname -s 2>/dev/null)" = "Darwin" ] && [ -f bin/protoc ] \
   && xattr -p com.apple.quarantine bin/protoc >/dev/null 2>&1; then
  xattr -d com.apple.quarantine bin/protoc
  ok "Cleared macOS quarantine flag on bin/protoc (would have failed the build)."
fi

# --- 2. .env ------------------------------------------------------------------
hdr "Configuration"
if [ -f .env ]; then
  ok ".env already exists — left untouched"
elif [ -f env.example ]; then
  cp env.example .env
  ok ".env created from env.example"
else
  warn "env.example not found; skipping .env creation"
fi

# --- 3. build -----------------------------------------------------------------
# Some networks (VPN / corporate proxy / antivirus doing TLS inspection)
# intermittently stall individual HTTP/2 requests cargo makes to
# index.crates.io. This surfaces as a random per-crate timeout ("curl
# failed ... Timeout was reached") or a cascading "oneshot canceled" once one
# request stalls and cargo tears down the batch — a different crate each
# time, even though a plain `curl` to the same URL works fine in isolation.
# These env vars make cargo's registry fetches tolerate that: disable HTTP/2
# multiplexing (avoids one stalled stream blocking others sharing its
# connection), retry failed requests, and allow more time per request.
export CARGO_HTTP_MULTIPLEXING="${CARGO_HTTP_MULTIPLEXING:-false}"
export CARGO_NET_RETRY="${CARGO_NET_RETRY:-10}"
export CARGO_HTTP_TIMEOUT="${CARGO_HTTP_TIMEOUT:-60}"

hdr "Building ainxt ($PROFILE)"
echo "  This takes a while on a cold target/ — roughly 80 crates."
if [ "$PROFILE" = "release-dist" ]; then
  cargo build --profile release-dist -p ainxt-pager-bin --bin ainxt
  BIN="target/release-dist/ainxt"
else
  cargo build -p ainxt-pager-bin --bin ainxt
  BIN="target/debug/ainxt"
fi

if [ ! -x "$BIN" ]; then
  printf '\n%sBuild reported success but %s is missing.%s\n' "$RED" "$BIN" "$R"
  exit 1
fi
hdr "Built"
ok "$BIN — $("./$BIN" --version 2>/dev/null || echo 'version unavailable')"

# --- 4. what to do next -------------------------------------------------------
hdr "Next steps"
cat <<EOF
  1. Point the CLI at a gateway, or at a local model.

     Edit .env. The one setting that matters is AINXT_GATEWAY_URL, and it must
     be an AiNxt Platform gateway (or another server exposing
     /ainxt/v1/api/models and /ainxt/v1/api/messages).

     To call a provider DIRECTLY with your own API key instead -- Anthropic,
     OpenAI, Together, Ollama, vLLM, LiteLLM -- do NOT use AINXT_GATEWAY_URL.
     Declare the provider as a [model.*] entry in config.toml: see section 6 of
     env.example, and "Provider Examples" in
     crates/codegen/ainxt-pager/docs/user-guide/11-custom-models.md

     The #1 mistake here is the wrong api_backend -- it varies by provider and
     is NOT interchangeable:
       messages          -> Anthropic / Claude
       responses         -> OpenAI / GPT (current models -- NOT chat_completions)
       chat_completions  -> local models (Ollama, vLLM, ...) and most others
                            (this is also the default when api_backend is omitted)

     Local models (Ollama, vLLM, ...) still need SOME credential set, even
     though the endpoint itself checks nothing -- ainxt refuses to start a
     session without one. Add this to .env:

       AINXT_API_KEY=local

  2. Load it:

       set -a && . ./.env && set +a

  3. Authenticate (skip if you set AINXT_TOKEN or AINXT_API_KEY in .env):

       ./$BIN login

  4. Run:

       ./$BIN                        # full-screen TUI
       ./$BIN -p "explain this repo"  # headless, one-shot

  5. Optional: put it on PATH so you can just run \`ainxt\` from any directory,
     instead of typing ./$BIN each time:

       mkdir -p ~/.ainxt/bin
       cp $BIN ~/.ainxt/bin/ainxt
       echo 'export PATH="\$HOME/.ainxt/bin:\$PATH"' >> ~/.bashrc   # or ~/.zshrc
       exec \$SHELL

  Scripting this in CI? Set AINXT_MAX_RETRIES low (e.g. 2). The default retry
  budget is ~6 minutes of silent retries against an unreachable gateway.

  Docs: README.md · RUN.md · crates/codegen/ainxt-pager/docs/user-guide/
EOF
