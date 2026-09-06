#
# Ainxt CLI installer (enterprise channel) for PowerShell — https://ainxt.example.test/cli/enterprise-install.ps1
#
# Standalone installer for the enterprise channel. Intentionally a full copy of
# the install logic so changes to the stable installer cannot break enterprise.
#
# Auth: AINXT_DEPLOYMENT_KEY env var (takes precedence) or ~/.ainxt/auth.json from `ainxt login`.
# Env: AINXT_BIN_DIR, AINXT_PROXY_URL
#
# Usage:
#   irm https://ainxt.example.test/cli/enterprise-install.ps1 | iex                                       # latest enterprise
#   & ([scriptblock]::Create((irm https://ainxt.example.test/cli/enterprise-install.ps1))) -Version 0.1.42 # specific version
#   $env:AINXT_VERSION="0.1.42"; irm https://ainxt.example.test/cli/enterprise-install.ps1 | iex           # specific version (alt)
#   $env:AINXT_DEPLOYMENT_KEY="<key>"; irm https://ainxt.example.test/cli/enterprise-install.ps1 | iex
#

param(
    [Parameter(Position = 0)]
    [string]$Version
)

$ErrorActionPreference = 'Stop'

# PS 5.1 defaults to TLS 1.0; GCS requires TLS 1.2.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

# PS 5.1's Invoke-WebRequest progress bar is extremely slow; disable it.
$ProgressPreference = 'SilentlyContinue'

# Accept version from environment variable (useful with irm | iex).
if (-not $Version -and $env:AINXT_VERSION) {
    $Version = $env:AINXT_VERSION
}

# This script is Windows-only. PS 5.1 has no Platform property and only runs on Windows.
if ($PSVersionTable.Platform -and $PSVersionTable.Platform -ne 'Win32NT') {
    Write-Error "This installer is for Windows. On macOS/Linux, run install-enterprise.sh from this repository's root, with AINXT_BASE_URL set to your artifact host."
    exit 1
}

$AinxtDir = Join-Path $env:USERPROFILE '.ainxt'

# --- Helpers ---

function Download-String([string]$Url) {
    try {
        $response = Invoke-WebRequest -Uri $Url -UseBasicParsing
        return $response.Content
    } catch {
        return $null
    }
}

function Download-File([string]$Url, [string]$OutFile) {
    # TODO: parallel byte-range download (matches install-enterprise.sh download_file_parallel).
    # Skipped for now: requires Start-ThreadJob / RunspacePool for true parallelism on PS 5.1
    # and HEAD + Range request orchestration. Single-connection HttpWebRequest below remains.
    # Stream via HttpWebRequest — faster than Invoke-WebRequest on PS 5.1 and supports progress.
    $request = [System.Net.HttpWebRequest]::Create($Url)
    $request.Timeout = 300000  # 5 min
    $request.AutomaticDecompression = [System.Net.DecompressionMethods]::GZip -bor [System.Net.DecompressionMethods]::Deflate
    $response = $request.GetResponse()
    $totalBytes = $response.ContentLength
    $stream = $response.GetResponseStream()
    $fileStream = [System.IO.File]::Create($OutFile)
    $buffer = New-Object byte[] 65536
    $totalRead = 0
    $lastPercent = -1
    $lastMb = -1

    try {
        while (($read = $stream.Read($buffer, 0, $buffer.Length)) -gt 0) {
            $fileStream.Write($buffer, 0, $read)
            $totalRead += $read
            $mb = [math]::Round($totalRead / 1MB, 1)
            if ($totalBytes -gt 0) {
                $percent = [math]::Min(100, [math]::Floor(($totalRead / $totalBytes) * 100))
                if ($percent -ne $lastPercent) {
                    $totalMb = [math]::Round($totalBytes / 1MB, 1)
                    Write-Host "`r  Downloading... ${mb} MB / ${totalMb} MB (${percent}%)" -NoNewline
                    $lastPercent = $percent
                }
            } elseif ($mb -ne $lastMb) {
                Write-Host "`r  Downloading... ${mb} MB" -NoNewline
                $lastMb = $mb
            }
        }
        Write-Host ''
    } finally {
        $fileStream.Close()
        $stream.Close()
        $response.Close()
    }
}

function Read-AinxtToken([string]$Scope) {
    $authFile = Join-Path $AinxtDir 'auth.json'
    if (-not (Test-Path $authFile)) { return $null }
    try {
        $auth = Get-Content -Raw $authFile | ConvertFrom-Json
        $entry = $auth.$Scope
        if ($entry -and $entry.key) { return $entry.key }
    } catch {}
    return $null
}

# --- Validate version ---

if ($Version -and $Version -notmatch '^\d+\.\d+\.\d+(-\S+)?$') {
    Write-Error "Invalid version format: $Version (expected X.Y.Z or X.Y.Z-suffix)"
    exit 1
}

# --- Resolve auth ---

# Overridable so a fork or self-hosted deployment installs from its own host.
$OidcScope = if ($env:AINXT_OIDC_SCOPE) { $env:AINXT_OIDC_SCOPE } else { 'https://auth.example.test::b1a00492-073a-47ea-816f-4c329264a828' }
$LegacyScope = if ($env:AINXT_LEGACY_SCOPE) { $env:AINXT_LEGACY_SCOPE } else { 'https://accounts.example.test/sign-in' }
$AuthSource = ''

if ($env:AINXT_DEPLOYMENT_KEY) {
    $AuthSource = 'deployment key'
    Write-Host 'Auth: using deployment key.' -ForegroundColor DarkGray
} else {
    $oidcToken = Read-AinxtToken $OidcScope
    $legacyToken = Read-AinxtToken $LegacyScope
    if ($oidcToken) {
        $AuthSource = 'auth.json (oidc)'
        Write-Host 'Auth: using OIDC token from ~/.ainxt/auth.json.' -ForegroundColor DarkGray
    } elseif ($legacyToken) {
        $AuthSource = 'auth.json (legacy)'
        Write-Host 'Auth: using legacy token from ~/.ainxt/auth.json.' -ForegroundColor DarkGray
    }
}

# --- Detect architecture ---

$arch = switch ($env:PROCESSOR_ARCHITECTURE) {
    'AMD64'   { 'x86_64' }
    'x86'     { 'x86_64' }   # 32-bit PS on 64-bit Windows
    'ARM64'   { 'aarch64' }
    default   { $null }
}

if (-not $arch) {
    Write-Error "Unsupported architecture: $env:PROCESSOR_ARCHITECTURE"
    exit 1
}

# Artifact-name OS token — MUST match scripts/build-release.sh (win32). CLI-027.
$platform = "win32-$arch"

# --- Resolve version ---

# Enterprise installs pull from an org-operated artifact host, so AINXT_BASE_URL
# is required — no default, no fallback origin. Until 2026-08-29 this fell back
# to a hardcoded storage.googleapis.com bucket with no override. See CLI-026.
if (-not $env:AINXT_BASE_URL) {
    Write-Error @"
AINXT_BASE_URL is not set.
Set it to the artifact host your organisation operates, e.g.:
  `$env:AINXT_BASE_URL = 'https://artifacts.your-org.example'
For the public build, use install.ps1 instead (GitHub Releases).
"@
    exit 1
}
$BaseUrlPrimary = $env:AINXT_BASE_URL
$BaseUrlFallback = $env:AINXT_BASE_URL
$DownloadDir = Join-Path $AinxtDir 'downloads'
$BinDir = if ($env:AINXT_BIN_DIR) { $env:AINXT_BIN_DIR } else { Join-Path $AinxtDir 'bin' }

New-Item -ItemType Directory -Path $DownloadDir -Force | Out-Null
New-Item -ItemType Directory -Path $BinDir -Force | Out-Null

$Channel = 'enterprise'

# Pick a working BaseUrl: try Cloudflare-fronted ainxt.dev first, fall back to
# direct GCS if it's unreachable. The probe doubles as the channel-pointer
# fetch when no -Version was passed, so the happy path costs zero extra requests.
if (-not $Version) { Write-Host "Fetching latest $Channel version..." -ForegroundColor DarkGray }
$probeResult = Download-String "$BaseUrlPrimary/$Channel"
if ($probeResult) {
    $BaseUrl = $BaseUrlPrimary
} else {
    Write-Host "Note: $BaseUrlPrimary unreachable, falling back to direct GCS." -ForegroundColor Yellow
    $BaseUrl = $BaseUrlFallback
    $probeResult = Download-String "$BaseUrl/$Channel"
}

if ($Version) {
    $resolvedVersion = $Version
} elseif ($probeResult) {
    $resolvedVersion = $probeResult.Trim()
} else {
    Write-Error "Failed to fetch latest version from $BaseUrlPrimary/$Channel and $BaseUrlFallback/$Channel"
    exit 1
}

if ($AuthSource) {
    Write-Host "Installing Ainxt $resolvedVersion ($platform, $AuthSource)..." -ForegroundColor Cyan
} else {
    Write-Host "Installing Ainxt $resolvedVersion ($platform)..." -ForegroundColor Cyan
}

# --- Download binary ---

$binaryPath = Join-Path $DownloadDir "ainxt-$platform.exe"
$artifactBase = "$BaseUrl/ainxt-$resolvedVersion-$platform"

$downloaded = $false
foreach ($url in @("$artifactBase.exe", $artifactBase)) {
    try {
        Download-File $url $binaryPath
        $downloaded = $true
        break
    } catch {
        continue
    }
}

if (-not $downloaded) {
    if (Test-Path $binaryPath) { Remove-Item $binaryPath -Force }
    Write-Error "Binary download failed from $artifactBase.exe and $artifactBase"
    exit 1
}

# --- Install binary (locked-file safe) ---

foreach ($binName in @('ainxt.exe', 'agent.exe')) {
    $dest = Join-Path $BinDir $binName
    $old = "$dest.old"

    if (Test-Path $old) { Remove-Item $old -Force -ErrorAction SilentlyContinue }

    try {
        Copy-Item -Path $binaryPath -Destination $dest -Force
    } catch {
        try {
            if (Test-Path $dest) { Rename-Item $dest $old -Force -ErrorAction SilentlyContinue }
            Copy-Item -Path $binaryPath -Destination $dest -Force
        } catch {
            if (Test-Path $old) { Rename-Item $old $dest -Force -ErrorAction SilentlyContinue }
            Write-Error "Failed to install $binName"
            exit 1
        }
    }
}

Write-Host "  Installed to $BinDir\ainxt.exe and $BinDir\agent.exe." -ForegroundColor DarkGray

# --- Generate completions (best-effort) ---

$completionsDir = Join-Path (Join-Path $AinxtDir 'completions') 'powershell'
try {
    New-Item -ItemType Directory -Path $completionsDir -Force | Out-Null
    & (Join-Path $BinDir 'ainxt.exe') completions powershell 2>$null |
        Set-Content (Join-Path $completionsDir 'ainxt.ps1') -ErrorAction SilentlyContinue
} catch {}

# --- Persist installer config ---

$ConfigFile = Join-Path $AinxtDir 'config.toml'
$cliLines = @('installer = "internal"', 'channel = "enterprise"')

if (-not (Test-Path $ConfigFile)) {
    New-Item -ItemType Directory -Path (Split-Path $ConfigFile) -Force | Out-Null
    $content = "[cli]`r`n" + ($cliLines -join "`r`n") + "`r`n"
    [System.IO.File]::WriteAllText($ConfigFile, $content, [System.Text.Encoding]::UTF8)
} elseif ((Get-Content -Raw $ConfigFile) -match '(?m)^\[cli\]') {
    # Section-aware: only replace installer/channel under [cli], not other sections.
    $existingLines = Get-Content $ConfigFile
    $output = [System.Collections.ArrayList]::new()
    $inCli = $false

    foreach ($line in $existingLines) {
        if ($line -match '^\[cli\]\s*(#.*)?$') {
            [void]$output.Add($line)
            foreach ($cl in $cliLines) { [void]$output.Add($cl) }
            $inCli = $true
            continue
        }
        if ($line -match '^\[.+\]\s*(#.*)?$') {
            $inCli = $false
        }
        if ($inCli -and $line -match '^\s*(installer|channel)\s*=') {
            continue
        }
        [void]$output.Add($line)
    }
    [System.IO.File]::WriteAllLines($ConfigFile, [string[]]$output.ToArray(), [System.Text.Encoding]::UTF8)
} else {
    Add-Content -Path $ConfigFile -Value "`r`n[cli]`r`n$($cliLines -join "`r`n")`r`n"
}

# --- Fetch deployment config (deployment key only) ---

if ($env:AINXT_DEPLOYMENT_KEY) {
    $ProxyUrl = if ($env:AINXT_PROXY_URL) { $env:AINXT_PROXY_URL } else { 'https://api.example.test/v1' }
    Write-Host '  Fetching deployment config...' -ForegroundColor DarkGray
    try {
        $headers = @{ 'Authorization' = "Bearer $($env:AINXT_DEPLOYMENT_KEY)" }
        $deployResponse = Invoke-RestMethod -Uri "$ProxyUrl/deployment/config" -Headers $headers -UseBasicParsing
    } catch {
        Write-Host "  Warning: failed to fetch deployment config from $ProxyUrl/deployment/config" -ForegroundColor Yellow
        $deployResponse = $null
    }

    if ($deployResponse) {
        $managedConfig = $deployResponse.managed_config
        $requirements = $deployResponse.requirements

        $managedConfigPath = Join-Path $AinxtDir 'managed_config.toml'
        $requirementsPath = Join-Path $AinxtDir 'requirements.toml'

        if ($managedConfig -and $managedConfig -ne 'null') {
            [System.IO.File]::WriteAllText($managedConfigPath, $managedConfig, [System.Text.Encoding]::UTF8)
            Write-Host '  Managed config applied.' -ForegroundColor DarkGray
        } else {
            if (Test-Path $managedConfigPath) { Remove-Item $managedConfigPath -Force }
        }

        if ($requirements -and $requirements -ne 'null') {
            [System.IO.File]::WriteAllText($requirementsPath, $requirements, [System.Text.Encoding]::UTF8)
            Write-Host '  Requirements applied.' -ForegroundColor DarkGray
        } else {
            if (Test-Path $requirementsPath) { Remove-Item $requirementsPath -Force }
        }
    }
}

Write-Host "Ainxt $resolvedVersion installed to $BinDir\ainxt.exe" -ForegroundColor Green

# --- Ensure ainxt is on PATH ---

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$pathEntries = if ($userPath) { $userPath -split ';' | Where-Object { $_ -ne '' } } else { @() }
if ($pathEntries -notcontains $BinDir) {
    $newPath = (@($BinDir) + $pathEntries) -join ';'
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    Write-Host "  Added $BinDir to your User PATH." -ForegroundColor DarkGray
    # Update current session so ainxt works immediately.
    if ($env:Path -notlike "*$BinDir*") {
        $env:Path = "$BinDir;$env:Path"
    }
}

Write-Host ''
Write-Host "Run 'ainxt' or 'agent' to get started!" -ForegroundColor Cyan
