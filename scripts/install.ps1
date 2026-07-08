#requires -Version 5.1
$ErrorActionPreference = "Stop"

$Repo = "Kinetic27/rktop"
$DefaultInstallDir = Join-Path $env:LOCALAPPDATA "rktop\bin"
$InstallDir = if ($env:RKTOP_INSTALL_DIR) { $env:RKTOP_INSTALL_DIR } else { $DefaultInstallDir }
$RequestedVersion = $env:RKTOP_VERSION
$NonInteractive = $env:RKTOP_NON_INTERACTIVE -eq "1"
$SkipPath = $env:RKTOP_SKIP_PATH -eq "1"

function Write-Step($Message) {
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Write-Ok($Message) {
    Write-Host "✓ $Message" -ForegroundColor Green
}

function Fail($Message) {
    Write-Error "rktop install failed: $Message"
    exit 1
}

function Normalize-PathText($PathText) {
    try {
        return [System.IO.Path]::GetFullPath($PathText).TrimEnd('\')
    } catch {
        return $PathText.TrimEnd('\')
    }
}

function Add-UserPath($Directory) {
    $normalized = Normalize-PathText $Directory
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (-not $userPath) { $userPath = "" }

    $entries = $userPath -split ';' | Where-Object { $_ -and $_.Trim() -ne "" }
    foreach ($entry in $entries) {
        if ((Normalize-PathText $entry) -ieq $normalized) {
            if (-not (($env:Path -split ';' | ForEach-Object { Normalize-PathText $_ }) -icontains $normalized)) {
                $env:Path = "$Directory;$env:Path"
            }
            return $false
        }
    }

    $newPath = if ($userPath.Trim()) { "$userPath;$Directory" } else { $Directory }
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    $env:Path = "$Directory;$env:Path"
    return $true
}

function Get-WindowsZipAsset($Release) {
    $Release.assets | Where-Object { $_.name -match '^rktop_.*_windows_x86_64\.zip$' } | Select-Object -First 1
}

function New-ResolvedRelease($Release, $Asset) {
    [PSCustomObject]@{
        Release = $Release
        Asset = $Asset
    }
}

function Get-ReleaseMetadata() {
    $headers = @{ "User-Agent" = "rktop-installer" }
    if ($RequestedVersion) {
        $tag = $RequestedVersion
        if (-not $tag.StartsWith("v")) { $tag = "v$tag" }
        $url = "https://api.github.com/repos/$Repo/releases/tags/$tag"
        $release = Invoke-RestMethod -Uri $url -Headers $headers
        $asset = Get-WindowsZipAsset $release
        if (-not $asset) {
            Fail "no Windows x86_64 zip asset found in release $($release.tag_name)"
        }
        return New-ResolvedRelease $release $asset
    }

    $url = "https://api.github.com/repos/$Repo/releases?per_page=20"
    $releases = Invoke-RestMethod -Uri $url -Headers $headers
    foreach ($release in $releases) {
        if ($release.draft -or $release.prerelease) { continue }
        $asset = Get-WindowsZipAsset $release
        if ($asset) {
            return New-ResolvedRelease $release $asset
        }
    }

    Fail "no published release with a Windows x86_64 zip asset found"
}

if (-not $IsWindows -and $PSVersionTable.PSEdition -eq "Core") {
    Fail "this installer is for Windows PowerShell/PowerShell on Windows. Use the .deb or scripts/install.sh on Linux."
}

if (-not $env:LOCALAPPDATA -and -not $env:RKTOP_INSTALL_DIR) {
    Fail "LOCALAPPDATA is not set. Set RKTOP_INSTALL_DIR to choose an install directory."
}

$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -and $arch -notin @("AMD64", "x86_64")) {
    Write-Host "Warning: current architecture is $arch; the published Windows package is x86_64." -ForegroundColor Yellow
}
if ($NonInteractive) {
    Write-Host "Running in non-interactive mode because RKTOP_NON_INTERACTIVE=1" -ForegroundColor DarkGray
}

try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
} catch {
    # PowerShell 7+ may not need this on newer runtimes.
}

Write-Step "Resolving latest rktop Windows release"
$resolved = Get-ReleaseMetadata
$release = $resolved.Release
$asset = $resolved.Asset

$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("rktop-install-" + [Guid]::NewGuid().ToString("N"))
$zipPath = Join-Path $tempRoot $asset.name
$extractDir = Join-Path $tempRoot "extract"

try {
    New-Item -ItemType Directory -Force -Path $tempRoot, $extractDir | Out-Null

    Write-Step "Downloading $($asset.name)"
    Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $zipPath -Headers @{ "User-Agent" = "rktop-installer" }

    Write-Step "Extracting package"
    Expand-Archive -Path $zipPath -DestinationPath $extractDir -Force

    $exe = Get-ChildItem -Path $extractDir -Recurse -Filter "rktop.exe" | Select-Object -First 1
    if (-not $exe) {
        Fail "downloaded package did not contain rktop.exe"
    }

    Write-Step "Installing to $InstallDir"
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item -Path (Join-Path $exe.DirectoryName "*") -Destination $InstallDir -Recurse -Force

    $installedExe = Join-Path $InstallDir "rktop.exe"
    if (-not (Test-Path $installedExe)) {
        Fail "rktop.exe was not installed to $InstallDir"
    }

    if (-not $SkipPath) {
        $pathChanged = Add-UserPath $InstallDir
        if ($pathChanged) {
            Write-Ok "Added $InstallDir to your user PATH"
        } else {
            Write-Ok "$InstallDir is already on your user PATH"
        }
    } else {
        Write-Host "Skipped PATH update because RKTOP_SKIP_PATH=1" -ForegroundColor Yellow
    }

    & $installedExe --help *> $null
    if ($LASTEXITCODE -ne 0) {
        Fail "installed rktop.exe did not pass a --help smoke check"
    }

    Write-Ok "Installed rktop $($release.tag_name)"
    Write-Host ""
    Write-Host "Run:" -ForegroundColor Cyan
    Write-Host "  rktop config"
    Write-Host "  rktop doctor"
    Write-Host "  rktop"
    if (-not $SkipPath) {
        Write-Host ""
        Write-Host "If 'rktop' is not found in this terminal, open a new PowerShell window." -ForegroundColor Yellow
    }
} finally {
    if (Test-Path $tempRoot) {
        Remove-Item -Path $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
