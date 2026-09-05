#!/bin/bash
#
# AiNxt CLI installer.
#
# Installs the `ainxt` binary for the current platform. By default it pulls from
# this repository's own GitHub Releases; there is no third-party fallback origin
# (see the long note at the BASE_URL block below, and CLI-026).
#
# Usage — one line, from a release:
#   curl -fsSL https://raw.githubusercontent.com/npci/ainxt-cli/main/install.sh | bash
#
# Recommended (enforces integrity; refuses to install anything unverifiable):
#   curl -fsSL .../install.sh | AINXT_REQUIRE_CHECKSUM=1 bash
#
# Pin a version:                  ./install.sh 0.2.101
# Use your own artifact host:     AINXT_BASE_URL=https://artifacts.example ./install.sh
# Use a different GitHub repo:    AINXT_GITHUB_REPO=myorg/ainxt-cli ./install.sh
#
# NOTE ON `curl | bash`. Piping a remote script into a shell makes the transport
# part of the trust boundary. That is a real cost, accepted here for the sake of
# a one-command install, and mitigated by: HTTPS only, a single origin the
# project controls, no fallback origin, and checksum verification of the
# downloaded binary (mandatory with AINXT_REQUIRE_CHECKSUM=1). If you would
# rather not pipe to a shell, download this file, read it, then run it — or
# build from source with ./setup.sh, which needs no artifact host at all.
#
# Auth: AINXT_DEPLOYMENT_KEY (takes precedence) or ~/.ainxt/auth.json from `ainxt login`.
# Env: AINXT_CHANNEL (stable|alpha|enterprise, default: stable), AINXT_BIN_DIR, AINXT_PROXY_URL
#
# Integrity:
#   AINXT_EXPECTED_SHA256=<hex>   pin the artifact digest; mismatch aborts.
#   AINXT_REQUIRE_CHECKSUM=1      refuse to install if nothing can be verified.
#   Otherwise a published <artifact>.sha256 is fetched and enforced when present,
#   and the install WARNS (printing the actual digest) when it is not.
#
# Windows: run under Git for Windows / MSYS2 Bash; WSL uses the Linux binary.
# Native PowerShell users want install.ps1 instead.

set -e

TARGET="$1"

if [[ -n "$TARGET" ]] && [[ ! "$TARGET" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9._]+)?$ ]]; then
    echo "Invalid version format: $TARGET (expected X.Y.Z or X.Y.Z-suffix)" >&2
    exit 1
fi

DOWNLOADER=""
if command -v curl >/dev/null 2>&1; then
    DOWNLOADER="curl"
elif command -v wget >/dev/null 2>&1; then
    DOWNLOADER="wget"
else
    echo "Either curl or wget is required but neither is installed" >&2
    exit 1
fi

download_file() {
    local url="$1" output="$2"
    if [ "$DOWNLOADER" = "curl" ]; then
        if [ -n "$output" ]; then
            curl -fsSL -o "$output" "$url"
        else
            curl -fsSL "$url"
        fi
    else
        if [ -n "$output" ]; then
            wget -q -O "$output" "$url"
        else
            wget -q -O - "$url"
        fi
    fi
}

# Parallel byte-range download. Falls back to single-connection download_file
# whenever HEAD lacks Content-Length, the file is small (<16 MiB), curl is
# unavailable, or any chunk fetch / concat fails.
download_file_parallel() {
    local url="$1" output="$2"
    if [ "$DOWNLOADER" != "curl" ]; then
        download_file "$url" "$output"
        return
    fi
    local size
    size=$(curl -fsSL --head "$url" 2>/dev/null | awk -F'[: \r\n]+' 'tolower($1)=="content-length"{print $2; exit}')
    if [ -z "$size" ] || ! [ "$size" -ge 16777216 ] 2>/dev/null; then
        download_file "$url" "$output"
        return
    fi
    local n=8
    local chunk_size=$(( (size + n - 1) / n ))
    local tmpdir
    tmpdir=$(mktemp -d 2>/dev/null) || { download_file "$url" "$output"; return; }
    local pids=() i start end
    for i in $(seq 0 $((n - 1))); do
        start=$((i * chunk_size))
        end=$((start + chunk_size - 1))
        [ $end -ge $size ] && end=$((size - 1))
        curl -fsSL -r "${start}-${end}" -o "${tmpdir}/$(printf 'chunk.%03d' "$i")" "$url" &
        pids+=($!)
    done
    local all_ok=true pid
    for pid in "${pids[@]}"; do
        wait "$pid" || all_ok=false
    done
    if [ "$all_ok" = true ] && cat "${tmpdir}"/chunk.* > "$output" 2>/dev/null; then
        rm -rf "$tmpdir"
        return 0
    fi
    rm -rf "$tmpdir"
    download_file "$url" "$output"
}

# Return 0 if a HEAD request for the URL gets HTTP 404.
is_not_found() {
    local url="$1" code
    if [ "$DOWNLOADER" = "curl" ]; then
        code=$(curl -o /dev/null -sSL -w '%{http_code}' --head "$url" 2>/dev/null) || true
    else
        code=$(wget --server-response --spider "$url" 2>&1 | awk '/HTTP\//{print $2}' | tail -1) || true
    fi
    [ "$code" = "404" ]
}

# JSON field extractor — extract a top-level string value using sed.
json_get() {
    local json="$1" field="$2"
    # Extract value (handling \" inside strings), then unescape JSON sequences.
    printf '%s' "$json" | sed -n -E 's/.*"'"$field"'"[[:space:]]*:[[:space:]]*"(([^"\\]|\\.)*)".*/\1/p' | head -1 \
        | sed -e 's/\\"/"/g' -e 's/\\n/\'$'\n''/g' -e 's/\\t/\'$'\t''/g' -e 's/\\\\/\\/g'
}

# Read a token from ~/.ainxt/auth.json for the given scope key.
# Format: {"scope_url": {"key": "token"}, ...}
read_ainxt_token() {
    local auth_file="$HOME/.ainxt/auth.json"
    local scope="$1"
    [ -f "$auth_file" ] || return 1
    # Flatten to one line then extract: find the scope, then the "key" value after it
    tr -d '\n' < "$auth_file" | sed -n 's|.*"'"$scope"'"[[:space:]]*:[[:space:]]*{[^}]*"key"[[:space:]]*:[[:space:]]*"\([^"]*\)".*|\1|p' | head -1
}

# Resolve auth: AINXT_DEPLOYMENT_KEY > OIDC token > legacy token
# Endpoints are overridable so a fork or a self-hosted deployment can install
# from its own host without patching this script.
OIDC_SCOPE="${AINXT_OIDC_SCOPE:-https://auth.example.test::b1a00492-073a-47ea-816f-4c329264a828}"
LEGACY_SCOPE="${AINXT_LEGACY_SCOPE:-https://accounts.example.test/sign-in}"
AUTH_SOURCE=""

if [ -n "$AINXT_DEPLOYMENT_KEY" ]; then
    AUTH_SOURCE="deployment key"
    echo "Auth: using deployment key." >&2
else
    OIDC_TOKEN=$(read_ainxt_token "$OIDC_SCOPE" 2>/dev/null) || true
    LEGACY_TOKEN=$(read_ainxt_token "$LEGACY_SCOPE" 2>/dev/null) || true
    if [ -n "$OIDC_TOKEN" ]; then
        AUTH_SOURCE="auth.json (oidc)"
        echo "Auth: using OIDC token from ~/.ainxt/auth.json." >&2
    elif [ -n "$LEGACY_TOKEN" ]; then
        AUTH_SOURCE="auth.json (legacy)"
        echo "Auth: using legacy token from ~/.ainxt/auth.json." >&2
    fi
fi

case "$(uname -s)" in
    Darwin) os="macos" ;;
    Linux)  os="linux" ;;
    # Git for Windows / MSYS2 / Cygwin host — native Windows builds
    MINGW* | MSYS* | CYGWIN*) os="windows" ;;
    *)      echo "Unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac

case "$(uname -m)" in
    x86_64|amd64|AMD64) arch="x86_64" ;;
    arm64|aarch64|ARM64) arch="aarch64" ;;
    *)                    echo "Unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

# Artifact-name OS token. This MUST match what scripts/build-release.sh emits:
#   dist/ainxt-<version>-{darwin|linux|win32}-{aarch64|x86_64}[.exe]
# It is deliberately separate from $os above, which stays macos/linux/windows
# because the rest of this script branches on it for platform behaviour.
# (Until 2026-08-29 this script requested `macos-*` and `windows-*`, which the
# builder never produces — every download 404'd except on Linux. See CLI-027.)
case "$os" in
    macos)   artifact_os="darwin" ;;
    linux)   artifact_os="linux"  ;;
    windows) artifact_os="win32"  ;;
esac

DOWNLOAD_DIR="$HOME/.ainxt/downloads"
BIN_DIR="${AINXT_BIN_DIR:-$HOME/.ainxt/bin}"
mkdir -p "$DOWNLOAD_DIR" "$BIN_DIR"

platform="${os}-${arch}"            # local cache filenames + messages
artifact_platform="${artifact_os}-${arch}"   # REMOTE artifact name only
CHANNEL="${AINXT_CHANNEL:-stable}"

# ---------------------------------------------------------------------------
# Where binaries come from.
#
# Default: this repository's own GitHub Releases. That is an origin the project
# publishing this script actually controls, and it needs no infrastructure.
#
# There is deliberately NO third-party fallback origin. Until 2026-08-29 this
# script fell back to a hardcoded `storage.googleapis.com/...` bucket whenever
# the primary was unreachable — which, since the primary default was the
# reserved domain `ainxt.example.test`, was ALWAYS. An unconfigured
# `curl ... | bash` therefore fetched an executable from a bucket this project
# does not necessarily control, marked it executable and put it on PATH. The
# install channel is the highest-value trust anchor in the whole product; a
# fallback origin that silently changes who you are trusting is not a
# convenience. See CLI-026.
#
# Running your own artifact host? Set AINXT_BASE_URL to it. Then the layout
# expected is flat: <base>/<channel> holds the latest version string, and
# <base>/ainxt-<version>-<platform> holds the binaries.
# ---------------------------------------------------------------------------
GH_REPO="${AINXT_GITHUB_REPO:-npci/ainxt-cli}"
BASE_URL="${AINXT_BASE_URL:-}"

if [ -n "$BASE_URL" ]; then
    # Operator-hosted flat layout.
    if [ -z "$TARGET" ]; then
        echo "Fetching latest ${CHANNEL} version from ${BASE_URL}..." >&2
        version=$(download_file "${BASE_URL}/${CHANNEL}" 2>/dev/null | tr -d '\r' | head -n1 | tr -d '[:space:]') || true
        if [ -z "$version" ]; then
            echo "Error: failed to fetch latest version from ${BASE_URL}/${CHANNEL}" >&2
            exit 1
        fi
    else
        version="$TARGET"
    fi
    artifact_dir="$BASE_URL"
else
    # GitHub Releases.
    if [ -z "$TARGET" ]; then
        echo "Fetching latest release of ${GH_REPO}..." >&2
        version=$(download_file "https://api.github.com/repos/${GH_REPO}/releases/latest" 2>/dev/null \
                  | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"v\{0,1\}\([^"]*\)".*/\1/p' | head -n1)
        if [ -z "$version" ]; then
            echo "Error: could not determine the latest release of ${GH_REPO}." >&2
            echo "       No releases published yet? Build from source instead:" >&2
            echo "         git clone https://github.com/${GH_REPO}.git && cd ainxt-cli && ./setup.sh" >&2
            echo "       Or point this installer at your own host: AINXT_BASE_URL=https://..." >&2
            exit 1
        fi
        artifact_dir="https://github.com/${GH_REPO}/releases/latest/download"
    else
        version="$TARGET"
        artifact_dir="https://github.com/${GH_REPO}/releases/download/v${version}"
    fi
fi

if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9._]+)?$ ]]; then
    echo "Invalid version format: $version (expected X.Y.Z or X.Y.Z-suffix)" >&2
    exit 1
fi

if [ -n "$AUTH_SOURCE" ]; then
    echo "Installing Ainxt $version ($platform, $AUTH_SOURCE)..." >&2
else
    echo "Installing Ainxt $version ($platform)..." >&2
fi

binary_path="$DOWNLOAD_DIR/ainxt-$platform"
artifact_base="${artifact_dir}/ainxt-${version}-${artifact_platform}"

if [ "$os" = "windows" ]; then
    binary_path="${binary_path}.exe"
fi

# ---------------------------------------------------------------------------
# Integrity verification.
#
# This installer is invoked as `curl ... | bash`, so the download it performs is
# the whole trust boundary: whatever bytes arrive get marked executable and put
# on PATH. Until this was added nothing was checked -- not a checksum, not a
# signature -- so a compromised mirror, a hostile proxy, or a MITM on a network
# that strips TLS would install an attacker's binary silently.
#
# Behaviour:
#   * a `<artifact>.sha256` published beside the artifact is fetched and ENFORCED;
#     a mismatch aborts and the download is deleted.
#   * `AINXT_EXPECTED_SHA256=<hex>` pins a specific digest and is enforced even
#     if no published file exists -- this is the flag to use in an automated or
#     air-gapped rollout.
#   * if neither is available the install proceeds but WARNS, loudly, naming what
#     was not verified.
#
# The warning is deliberate rather than a hard failure: making verification
# mandatory before the release pipeline publishes digests would break every
# existing install line, and a silent skip is what created the problem. Set
# AINXT_REQUIRE_CHECKSUM=1 to turn the warning into an error.
# ---------------------------------------------------------------------------
sha256_of() {
    local file="$1"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$file" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$file" | awk '{print $1}'
    elif command -v openssl >/dev/null 2>&1; then
        openssl dgst -sha256 "$file" | awk '{print $NF}'
    else
        echo ""
    fi
}

verify_download() {
    local file="$1" url="$2" expected="" published="" actual=""

    actual=$(sha256_of "$file")
    if [ -z "$actual" ]; then
        echo "  WARNING: no sha256 tool found (sha256sum, shasum or openssl)." >&2
        echo "           The downloaded binary could NOT be verified." >&2
        [ -n "${AINXT_REQUIRE_CHECKSUM:-}" ] && { rm -f "$file"; echo "Aborting: AINXT_REQUIRE_CHECKSUM is set." >&2; exit 1; }
        return 0
    fi

    if [ -n "${AINXT_EXPECTED_SHA256:-}" ]; then
        expected="$AINXT_EXPECTED_SHA256"
    else
        published=$(download_file "${url}.sha256" 2>/dev/null) || published=""
        # Accept either a bare digest or `<digest>  <filename>`.
        expected=$(printf '%s' "$published" | tr -d '\r' | awk 'NR==1{print $1}')
    fi

    if [ -z "$expected" ]; then
        echo "  WARNING: no checksum available for this artifact." >&2
        echo "           Tried AINXT_EXPECTED_SHA256 and ${url}.sha256 -- neither was present." >&2
        echo "           The binary was NOT verified. Its sha256 is:" >&2
        echo "             $actual" >&2
        echo "           Compare it against the published release digest before trusting it." >&2
        if [ -n "${AINXT_REQUIRE_CHECKSUM:-}" ]; then
            rm -f "$file"
            echo "Aborting: AINXT_REQUIRE_CHECKSUM is set and no checksum was available." >&2
            exit 1
        fi
        return 0
    fi

    # Case-insensitive compare; published digests appear in both cases.
    if [ "$(printf '%s' "$expected" | tr 'A-Z' 'a-z')" != "$(printf '%s' "$actual" | tr 'A-Z' 'a-z')" ]; then
        rm -f "$file"
        echo "Error: CHECKSUM MISMATCH -- the download was not what the release publishes." >&2
        echo "  expected: $expected" >&2
        echo "  actual:   $actual" >&2
        echo "The file has been deleted and nothing was installed. Do not retry blindly:" >&2
        echo "this is what a tampered mirror, a hostile proxy, or a corrupted CDN object" >&2
        echo "looks like." >&2
        exit 1
    fi

    echo "  Verified sha256 ${actual}" >&2
}

binary_tmp="${binary_path}.tmp.$$"
rm -f "$binary_tmp" 2>/dev/null || true

echo "  Downloading ainxt ${version}..." >&2
# Which URL actually served the bytes.  The Windows path falls back from
# `<base>.exe` to `<base>`, and verifying against the URL we *tried first*
# rather than the one that answered would look for a checksum that does not
# exist and silently downgrade to the unverified warning path.
downloaded_url=""
if [ "$os" = "windows" ]; then
    if ! download_file_parallel "${artifact_base}.exe" "$binary_tmp"; then
        if ! download_file_parallel "$artifact_base" "$binary_tmp"; then
            rm -f "$binary_tmp"
            if is_not_found "${artifact_base}.exe"; then
                echo "Error: Ainxt is not yet available for your system ($platform)." >&2
            else
                echo "Error: binary download failed (${artifact_base}.exe and ${artifact_base})" >&2
            fi
            exit 1
        else
            downloaded_url="$artifact_base"
        fi
    else
        downloaded_url="${artifact_base}.exe"
    fi
elif ! download_file_parallel "$artifact_base" "$binary_tmp"; then
    rm -f "$binary_tmp"
    if is_not_found "$artifact_base"; then
        echo "Error: Ainxt is not yet available for your system ($platform)." >&2
    else
        echo "Error: binary download failed from ${artifact_base}" >&2
    fi
    exit 1
else
    downloaded_url="$artifact_base"
fi

# Verify BEFORE the artifact is moved into place or marked executable.
verify_download "$binary_tmp" "$downloaded_url"

if [ "$os" = "windows" ]; then
    mv -f "$binary_tmp" "$binary_path"
    # Symlinks require Developer Mode on Windows; copy instead.
    # If the exe is locked by a running process, rename it aside then retry.
    for bin_name in ainxt.exe agent.exe; do
        rm -f "$BIN_DIR/$bin_name.old" 2>/dev/null || true  # stale backup from prior update
        if ! cp -f "$binary_path" "$BIN_DIR/$bin_name" 2>/dev/null; then
            mv -f "$BIN_DIR/$bin_name" "$BIN_DIR/$bin_name.old" 2>/dev/null || true
            if ! cp -f "$binary_path" "$BIN_DIR/$bin_name" 2>/dev/null; then
                # Rollback: restore the old binary so the install isn't broken.
                mv -f "$BIN_DIR/$bin_name.old" "$BIN_DIR/$bin_name" 2>/dev/null || true
                echo "Error: failed to install $bin_name" >&2
                exit 1
            fi
        fi
    done
    echo "  Binary installed to $BIN_DIR/ainxt.exe and $BIN_DIR/agent.exe." >&2
else
    chmod +x "$binary_tmp"
    if ! "$binary_tmp" --version </dev/null >/dev/null 2>&1; then
        echo "Error: downloaded ainxt failed to run; keeping the existing install." >&2
        rm -f "$binary_tmp"
        exit 1
    fi
    mv -f "$binary_tmp" "$binary_path"
    # Use relative symlinks when BIN_DIR and DOWNLOAD_DIR share a parent
    # (default layout: ~/.ainxt/bin and ~/.ainxt/downloads are siblings).
    # Relative symlinks survive Docker bind-mounts with a different $HOME.
    if [ "$(dirname "$BIN_DIR")" = "$(dirname "$DOWNLOAD_DIR")" ]; then
        link_target="../$(basename "$DOWNLOAD_DIR")/$(basename "$binary_path")"
    else
        link_target="$binary_path"
    fi
    ln -sf "$link_target" "$BIN_DIR/ainxt"
    ln -sf "$link_target" "$BIN_DIR/agent"
    echo "  Binary linked to $BIN_DIR/ainxt and $BIN_DIR/agent." >&2
fi

# Generate shell completions (best-effort)
mkdir -p "$HOME/.ainxt/completions/bash" "$HOME/.ainxt/completions/zsh"
"$BIN_DIR/ainxt" completions bash > "$HOME/.ainxt/completions/bash/ainxt.bash" 2>/dev/null || true
"$BIN_DIR/ainxt" completions zsh  > "$HOME/.ainxt/completions/zsh/_ainxt"     2>/dev/null || true
# Fish: write to the auto-loaded completions dir so it works immediately
if mkdir -p "$HOME/.config/fish/completions" 2>/dev/null; then
    "$BIN_DIR/ainxt" completions fish > "$HOME/.config/fish/completions/ainxt.fish" 2>/dev/null || true
fi

# Persist installer source and channel to config
CONFIG_FILE="$HOME/.ainxt/config.toml"
CLI_BLOCK="installer = \"internal\""
if [ "$CHANNEL" != "stable" ]; then
    CLI_BLOCK="${CLI_BLOCK}\nchannel = \"${CHANNEL}\""
fi
if [ ! -f "$CONFIG_FILE" ]; then
    printf '[cli]\n%b\n' "$CLI_BLOCK" > "$CONFIG_FILE"
elif grep -q '^\[cli\]' "$CONFIG_FILE"; then
    tmp="$CONFIG_FILE.tmp.$$"
    awk -v block="$CLI_BLOCK" '
        /^\[cli\][[:space:]]*(#.*)?$/ { print; printf "%s\n", block; in_cli=1; next }
        /^\[.*\][[:space:]]*(#.*)?$/  { in_cli=0 }
        in_cli && /^[[:space:]]*(installer|channel)[[:space:]]*=/ { next }
        { print }
    ' "$CONFIG_FILE" > "$tmp" && mv "$tmp" "$CONFIG_FILE"
else
    printf '\n[cli]\n%b\n' "$CLI_BLOCK" >> "$CONFIG_FILE"
fi

# Fetch managed_config.toml + requirements.toml from server (deployment key only).
if [ -n "$AINXT_DEPLOYMENT_KEY" ]; then
    PROXY_URL="${AINXT_PROXY_URL:-https://api.example.test/v1}"
    echo "  Fetching deployment config..." >&2
    DEPLOY_RESPONSE=""
    AUTH_HEADER_FILE=$(mktemp 2>/dev/null) || AUTH_HEADER_FILE=""
    if [ -n "$AUTH_HEADER_FILE" ]; then
        chmod 600 "$AUTH_HEADER_FILE" 2>/dev/null || true
        printf 'Authorization: Bearer %s\n' "$AINXT_DEPLOYMENT_KEY" > "$AUTH_HEADER_FILE"
        DEPLOY_RESPONSE=$(curl -sS -f \
            -H "@${AUTH_HEADER_FILE}" \
            "${PROXY_URL}/deployment/config" 2>/dev/null) || DEPLOY_RESPONSE=""
        : > "$AUTH_HEADER_FILE" 2>/dev/null || true
        rm -f "$AUTH_HEADER_FILE"
    fi
    if [ -z "$DEPLOY_RESPONSE" ]; then
        echo "  Warning: failed to fetch deployment config from ${PROXY_URL}/deployment/config" >&2
    fi
    if [ -n "$DEPLOY_RESPONSE" ]; then
        MANAGED_CONFIG=$(json_get "$DEPLOY_RESPONSE" "managed_config")
        REQUIREMENTS=$(json_get "$DEPLOY_RESPONSE" "requirements")
        if [ -n "$MANAGED_CONFIG" ] && [ "$MANAGED_CONFIG" != "null" ]; then
            printf '%s\n' "$MANAGED_CONFIG" > "$HOME/.ainxt/managed_config.toml"
            echo "  Managed config applied." >&2
        else
            rm -f "$HOME/.ainxt/managed_config.toml"
        fi
        if [ -n "$REQUIREMENTS" ] && [ "$REQUIREMENTS" != "null" ]; then
            printf '%s\n' "$REQUIREMENTS" > "$HOME/.ainxt/requirements.toml"
            echo "  Requirements applied." >&2
        else
            rm -f "$HOME/.ainxt/requirements.toml"
        fi
    fi
fi

if [ "$os" = "windows" ]; then
    echo "Ainxt $version installed to $BIN_DIR/ainxt.exe" >&2
else
    echo "Ainxt $version installed to $BIN_DIR/ainxt" >&2
fi

# --- Ensure ainxt is on PATH ---

path_has_dir() {
    case ":$PATH:" in *":$1:"*) return 0 ;; *) return 1 ;; esac
}

# Try to symlink into a directory already on PATH so ainxt works immediately
# without restarting the shell. Candidate dirs in preference order.
SYMLINK_CREATED=""
if [ "$os" != "windows" ] && ! path_has_dir "$BIN_DIR"; then
    for candidate in "$HOME/.local/bin" "/usr/local/bin"; do
        if path_has_dir "$candidate" && [ -d "$candidate" ] && [ -w "$candidate" ]; then
            ln -sf "$BIN_DIR/ainxt" "$candidate/ainxt"
            ln -sf "$BIN_DIR/agent" "$candidate/agent"
            SYMLINK_CREATED="$candidate"
            echo "  Symlinked $candidate/ainxt -> $BIN_DIR/ainxt" >&2
            echo "  Symlinked $candidate/agent -> $BIN_DIR/agent" >&2
            break
        fi
    done
fi

# Also update shell config so ~/.ainxt/bin is on PATH for future sessions
user_shell="$(basename "${SHELL:-}")"
config_file=""

case "$user_shell" in
    bash) config_file="$HOME/.bashrc" ;;
    zsh)  config_file="$HOME/.zshrc" ;;
    fish) config_file="$HOME/.config/fish/config.fish" ;;
esac

if [ -n "$config_file" ]; then
    mkdir -p "$(dirname "$config_file")"

    # Resolve symlinks so tmp+mv rewrites the stow/dotfiles target, not the link.
    if [ -e "$config_file" ] || [ -L "$config_file" ]; then
        _cf="$config_file"
        _depth=0
        while [ -L "$_cf" ] && [ "$_depth" -lt 40 ]; do
            _link="$(readlink "$_cf")" || break
            case "$_link" in
                /*) _cf="$_link" ;;
                *)  _cf="$(cd "$(dirname "$_cf")" && pwd -P)/$_link" ;;
            esac
            _depth=$((_depth + 1))
        done
        # Still a symlink (cycle/cap): leave original path so we never rewrite the link.
        if [ ! -L "$_cf" ]; then
            config_file="$(cd "$(dirname "$_cf")" && pwd -P)/$(basename "$_cf")"
        fi
        unset _cf _link _depth
    fi

    # Build the new installer block
    if [ "$user_shell" = "fish" ]; then
        new_block='# >>> ainxt installer >>>
fish_add_path $HOME/.ainxt/bin
# <<< ainxt installer <<<'
    elif [ "$user_shell" = "zsh" ]; then
        new_block='# >>> ainxt installer >>>
export PATH="$HOME/.ainxt/bin:$PATH"
fpath=(~/.ainxt/completions/zsh $fpath)
autoload -Uz compinit && compinit -C
# <<< ainxt installer <<<'
    else
        new_block='# >>> ainxt installer >>>
export PATH="$HOME/.ainxt/bin:$PATH"
[[ -r "$HOME/.ainxt/completions/bash/ainxt.bash" ]] && source "$HOME/.ainxt/completions/bash/ainxt.bash"
# <<< ainxt installer <<<'
    fi

    if grep -qs "ainxt installer" "$config_file" 2>/dev/null; then
        # Replace existing block in-place (strip old >>> to <<< lines, insert new)
        tmp="$config_file.tmp.$$"
        awk '
            /# >>> ainxt installer >>>/ { skip=1; next }
            /# <<< ainxt installer <<</ { skip=0; next }
            !skip { print }
        ' "$config_file" > "$tmp" && mv "$tmp" "$config_file"
    else
        [ -f "$config_file" ] && cp "$config_file" "$config_file.bak.$(date +%s)"

        # macOS bash: ensure bash_profile sources bashrc
        if [ "$user_shell" = "bash" ] && [ "$(uname -s)" = "Darwin" ]; then
            if [ -f "$HOME/.bash_profile" ] && ! grep -qs "source ~/.bashrc" "$HOME/.bash_profile"; then
                printf '\n[[ -r ~/.bashrc ]] && source ~/.bashrc\n' >> "$HOME/.bash_profile"
            fi
        fi
    fi

    printf '\n%s\n' "$new_block" >> "$config_file"
    echo "  Updated $BIN_DIR in PATH in $config_file." >&2
fi

echo "" >&2
if path_has_dir "$BIN_DIR" || [ -n "$SYMLINK_CREATED" ]; then
    echo "Run 'ainxt' or 'agent' to get started!" >&2
elif [ -n "$config_file" ]; then
    echo "Restart your terminal, then run 'ainxt' or 'agent' to get started!" >&2
else
    echo "Add $BIN_DIR to your PATH, then run 'ainxt' or 'agent' to get started:" >&2
    echo '  export PATH="$HOME/.ainxt/bin:$PATH"' >&2
fi

if [ "$os" = "windows" ]; then
    echo "To use ainxt from cmd.exe or PowerShell, add %USERPROFILE%\\.ainxt\\bin to your PATH." >&2
fi
