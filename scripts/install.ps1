<#
.SYNOPSIS
    Downloads, SHA-256 verifies, and installs the Windows x86_64 `imgforge` binary.
.DESCRIPTION
    Fetches `SHA256SUMS.txt` and `imgforge-<TAG>-x86_64-pc-windows-msvc.zip` from
    GitHub Releases, verifies the SHA-256 digest, installs `imgforge.exe` into
    `$env:LOCALAPPDATA\imgforge\bin`, and ensures that directory is on User PATH.
.PARAMETER Version
    Optional release version (`latest`, `0.1.0`, or `v0.1.0`). Defaults to `$env:IMGFORGE_VERSION` or `latest`.
.PARAMETER InstallDir
    Optional target directory. Defaults to `$env:IMGFORGE_INSTALL_DIR` or `$env:LOCALAPPDATA\imgforge\bin`.
#>
[CmdletBinding()]
param(
    [string]$Version = "",
    [string]$InstallDir = ""
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$Repo = if ($env:IMGFORGE_REPO) { $env:IMGFORGE_REPO } else { "Hartorn-Inc/imgforge" }
$Target = "x86_64-pc-windows-msvc"

if ([string]::IsNullOrWhiteSpace($Version)) {
    $Version = if ($env:IMGFORGE_VERSION) { $env:IMGFORGE_VERSION } else { "latest" }
}
if ([string]::IsNullOrWhiteSpace($InstallDir)) {
    $InstallDir = if ($env:IMGFORGE_INSTALL_DIR) {
        $env:IMGFORGE_INSTALL_DIR
    } else {
        Join-Path $env:LOCALAPPDATA "imgforge\bin"
    }
}
$InstallDir = $InstallDir.TrimEnd('\')

$ExplicitTag = ""
if ($Version -eq "latest") {
    $BaseUrl = "https://github.com/$Repo/releases/latest/download"
} else {
    $NormalizedVersion = $Version -replace '^[vV]', ''
    if ($NormalizedVersion -notmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$') {
        throw "Version '$Version' must be 'latest' or valid SemVer (for example '0.1.0' or 'v0.1.0')."
    }
    $ExplicitTag = "v$NormalizedVersion"
    $BaseUrl = "https://github.com/$Repo/releases/download/$ExplicitTag"
}

$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("imgforge-install-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $TempDir | Out-Null

try {
    $SumsPath = Join-Path $TempDir "SHA256SUMS.txt"
    Write-Host "Fetching release checksums from $BaseUrl/SHA256SUMS.txt..."
    Invoke-WebRequest -Uri "$BaseUrl/SHA256SUMS.txt" -OutFile $SumsPath -UseBasicParsing

    $MatchesList = @()
    $Lines = Get-Content -Path $SumsPath -Encoding UTF8
    foreach ($Line in $Lines) {
        if ($Line -match '^([0-9a-fA-F]{64})\s+\*?([^\s]+)$') {
            $Hash = $Matches[1].ToLowerInvariant()
            $FileName = $Matches[2]
            if ($ExplicitTag) {
                $ExpectedName = "imgforge-$ExplicitTag-$Target.zip"
                if ($FileName -eq $ExpectedName) {
                    $MatchesList += [PSCustomObject]@{ Hash = $Hash; FileName = $FileName }
                }
            } elseif ($FileName -match "^imgforge-v[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.-]+)?-$([regex]::Escape($Target))\.zip$") {
                $MatchesList += [PSCustomObject]@{ Hash = $Hash; FileName = $FileName }
            }
        }
    }

    if ($MatchesList.Count -ne 1) {
        throw "Could not resolve a unique SHA-256 entry for target '$Target' in SHA256SUMS.txt."
    }

    $ExpectedHash = $MatchesList[0].Hash
    $ArchiveName = $MatchesList[0].FileName
    $ArchivePath = Join-Path $TempDir $ArchiveName

    Write-Host "Downloading $BaseUrl/$ArchiveName..."
    Invoke-WebRequest -Uri "$BaseUrl/$ArchiveName" -OutFile $ArchivePath -UseBasicParsing

    $ActualHash = (Get-FileHash -Path $ArchivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($ActualHash -ne $ExpectedHash) {
        throw "SHA-256 checksum verification failed for '$ArchiveName' (expected $ExpectedHash, got $ActualHash)."
    }

    $ExtractDir = Join-Path $TempDir "extract"
    New-Item -ItemType Directory -Force -Path $ExtractDir | Out-Null
    Expand-Archive -Path $ArchivePath -DestinationPath $ExtractDir -Force

    $ExtractedExe = Join-Path $ExtractDir "imgforge.exe"
    if (-not (Test-Path -Path $ExtractedExe -PathType Leaf)) {
        throw "Extracted archive did not contain 'imgforge.exe' at archive root."
    }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $TargetExe = Join-Path $InstallDir "imgforge.exe"
    $StagedExe = Join-Path $InstallDir (".imgforge-" + [System.Guid]::NewGuid().ToString("N") + ".exe")
    try {
        Copy-Item -Path $ExtractedExe -Destination $StagedExe -Force
        Move-Item -Path $StagedExe -Destination $TargetExe -Force
    } finally {
        if (Test-Path -Path $StagedExe) {
            Remove-Item -Path $StagedExe -Force -ErrorAction SilentlyContinue
        }
    }

    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $PathEntries = @()
    if (-not [string]::IsNullOrWhiteSpace($UserPath)) {
        $PathEntries = $UserPath -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    }
    $AlreadyInUserPath = $PathEntries | Where-Object { $_.TrimEnd('\') -ieq $InstallDir }
    if (-not $AlreadyInUserPath) {
        $NewUserPath = if ($PathEntries.Count -gt 0) {
            ($PathEntries + $InstallDir) -join ';'
        } else {
            $InstallDir
        }
        [Environment]::SetEnvironmentVariable("Path", $NewUserPath, "User")
        Write-Host "Added '$InstallDir' to User PATH. Open a new terminal if existing shells do not pick it up."
    }

    if (-not (($env:Path -split ';') | Where-Object { $_.TrimEnd('\') -ieq $InstallDir })) {
        $env:Path = "$InstallDir;$env:Path"
    }

    $InstalledVersion = & $TargetExe --version
    Write-Host "Installed $InstalledVersion to $TargetExe"
} finally {
    if (Test-Path -Path $TempDir) {
        Remove-Item -Path $TempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
