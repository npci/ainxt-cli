#!/bin/bash
#
# Ainxt CLI installer (enterprise channel) — https://ainxt.example.test/cli/enterprise-install.sh
#
# Standalone installer for the enterprise channel. This is intentionally a full
# copy of the install logic (not a wrapper around install.sh) so that changes to
# the stable installer cannot accidentally break enterprise deployments.
#
# Auth: AINXT_DEPLOYMENT_KEY (takes precedence) or ~/.ainxt/auth.json from `ainxt login`.
# Env: AINXT_BIN_DIR, AINXT_PROXY_URL
#
# Usage:
#   curl -fsSL https://ainxt.example.test/cli/enterprise-install.sh | bash            # latest enterprise
#   curl -fsSL https://ainxt.example.test/cli/enterprise-install.sh | bash -s 0.1.42  # specific version
#   AINXT_DEPLOYMENT_KEY=<key> bash <(curl -fsSL https://ainxt.example.test/cli/enterprise-install.sh)
#
# Windows: run under Git for Windows / MSYS2 Bash (same curl | bash flow); WSL
# uses the Linux binary.

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

# Enterprise installs pull from an artifact host the ORGANISATION operates, so
# AINXT_BASE_URL is required — there is no default and no fallback origin.
# Until 2026-08-29 this fell back to a hardcoded storage.googleapis.com bucket
# whenever the primary (the reserved domain ainxt.example.test) was unreachable,
# i.e. always: an unconfigured run executed a binary from a third-party origin.
# See CLI-026.
if [ -z "${AINXT_BASE_URL:-}" ]; then
    echo "Error: AINXT_BASE_URL is not set." >&2
    echo "       Set it to the artifact host your organisation operates, e.g.:" >&2
    echo "         AINXT_BASE_URL=https://artifacts.your-org.example ./install-enterprise.sh" >&2
    echo "       For the public build, use install.sh instead (GitHub Releases)." >&2
    exit 1
fi
BASE_URL_PRIMARY="$AINXT_BASE_URL"
BASE_URL_FALLBACK="$AINXT_BASE_URL"
DOWNLOAD_DIR="$HOME/.ainxt/downloads"
BIN_DIR="${AINXT_BIN_DIR:-$HOME/.ainxt/bin}"
mkdir -p "$DOWNLOAD_DIR" "$BIN_DIR"

# Artifact-name OS token — MUST match scripts/build-release.sh output
# (darwin|linux|win32). $os stays macos/linux/windows for behaviour. See CLI-027.
case "$os" in
    macos)   artifact_os="darwin" ;;
    linux)   artifact_os="linux"  ;;
    windows) artifact_os="win32"  ;;
esac
platform="${os}-${arch}"            # local cache filenames + messages
artifact_platform="${artifact_os}-${arch}"   # REMOTE artifact name only
CHANNEL="enterprise"

# Pick a working BASE_URL: try Cloudflare-fronted ainxt.dev first, fall back to
# direct GCS if it's unreachable. The probe doubles as the channel-pointer
# fetch when no explicit TARGET was passed, so the happy path costs zero
# extra HTTP requests.
if [ -z "$TARGET" ]; then echo "Fetching latest ${CHANNEL} version..." >&2; fi
probe_result=$(download_file "${BASE_URL_PRIMARY}/${CHANNEL}" 2>/dev/null) || true
if [ -n "$probe_result" ]; then
    BASE_URL="$BASE_URL_PRIMARY"
else
    echo "Note: ${BASE_URL_PRIMARY} unreachable, falling back to direct GCS." >&2
    BASE_URL="$BASE_URL_FALLBACK"
    probe_result=$(download_file "${BASE_URL}/${CHANNEL}" 2>/dev/null) || true
fi

if [ -n "$TARGET" ]; then
    version="$TARGET"
else
    version=$(printf '%s' "$probe_result" | tr -d '\r' | head -n1 | tr -d '[:space:]')
    if [ -z "$version" ]; then
        echo "Error: failed to fetch latest version from ${BASE_URL_PRIMARY}/${CHANNEL} and ${BASE_URL_FALLBACK}/${CHANNEL}" >&2
        exit 1
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
artifact_base="${BASE_URL}/ainxt-${version}-${artifact_platform}"

if [ "$os" = "windows" ]; then
    binary_path="${binary_path}.exe"
fi

echo "  Downloading ainxt ${version}..." >&2
if [ "$os" = "windows" ]; then
    if ! download_file_parallel "${artifact_base}.exe" "$binary_path"; then
        if ! download_file_parallel "$artifact_base" "$binary_path"; then
            rm -f "$binary_path"
            if is_not_found "${artifact_base}.exe"; then
                echo "Error: Ainxt is not yet available for your system ($platform)." >&2
            else
                echo "Error: binary download failed (${artifact_base}.exe and ${artifact_base})" >&2
            fi
            exit 1
        fi
    fi
elif ! download_file_parallel "$artifact_base" "$binary_path"; then
    rm -f "$binary_path"
    if is_not_found "$artifact_base"; then
        echo "Error: Ainxt is not yet available for your system ($platform)." >&2
    else
        echo "Error: binary download failed from ${artifact_base}" >&2
    fi
    exit 1
fi

if [ "$os" = "windows" ]; then
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
    chmod +x "$binary_path"
    ln -sf "$binary_path" "$BIN_DIR/ainxt"
    ln -sf "$binary_path" "$BIN_DIR/agent"
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
CLI_BLOCK="installer = \"internal\"\nchannel = \"enterprise\""
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
