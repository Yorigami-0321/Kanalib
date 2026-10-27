param(
    [string]$Dest = "runtime/ffmpeg",
    [switch]$Force
)

$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
# In the repo the script lives in scripts/; in a portable copy it sits next to the exe.
$root = if ((Split-Path -Leaf $PSScriptRoot) -eq "scripts") {
    Split-Path -Parent $PSScriptRoot
} else {
    $PSScriptRoot
}
Set-Location $root

$url = "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-10-26-14-25/ffmpeg-n8.1.3-24-g92611900bb-win64-lgpl-shared-8.1.zip"
$expectedSha256 = "96AFE9BEE66012F4CBC336A473F0B595060D73BDC5EFE4331D55101500D0A93E"
$destPath = Join-Path $root $Dest
$ffmpegExe = Join-Path $destPath "bin/ffmpeg.exe"

if ((Test-Path -LiteralPath $ffmpegExe) -and -not $Force) {
    throw "ffmpeg already installed at $destPath (pass -Force to replace)"
}

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("kanalib-ffmpeg-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $work | Out-Null
$zip = Join-Path $work "ffmpeg.zip"
try {
    Write-Output "download: $url"
    Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
    $actualSha256 = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash
    if ($actualSha256 -ne $expectedSha256) {
        throw "sha256 mismatch: expected $expectedSha256, got $actualSha256"
    }
    Write-Output "sha256: $actualSha256"

    $extract = Join-Path $work "extract"
    Expand-Archive -LiteralPath $zip -DestinationPath $extract -Force
    $package = Get-ChildItem -LiteralPath $extract -Directory | Select-Object -First 1
    if ($null -eq $package) {
        throw "archive layout not recognized"
    }
    $sourceBin = Join-Path $package.FullName "bin"
    $sourceLicense = Join-Path $package.FullName "LICENSE.txt"
    if (-not (Test-Path -LiteralPath (Join-Path $sourceBin "ffmpeg.exe"))) {
        throw "ffmpeg.exe missing from archive"
    }
    if (-not (Test-Path -LiteralPath $sourceLicense)) {
        throw "LICENSE.txt missing from archive"
    }

    if (Test-Path -LiteralPath $destPath) {
        Remove-Item -LiteralPath $destPath -Recurse -Force
    }
    $destBin = Join-Path $destPath "bin"
    New-Item -ItemType Directory -Path $destBin -Force | Out-Null
    Get-ChildItem -LiteralPath $sourceBin -File |
        Where-Object { $_.Name -ne "ffplay.exe" } |
        ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $destBin }
    Copy-Item -LiteralPath $sourceLicense -Destination (Join-Path $destPath "LICENSE.txt")

    $version = (& (Join-Path $destBin "ffmpeg.exe") -hide_banner -version 2>&1 | Select-Object -First 1) -join " "
    $sha = (Get-FileHash -LiteralPath (Join-Path $destBin "ffmpeg.exe") -Algorithm SHA256).Hash
    $source = @(
        "source=$url",
        "archive_sha256=$expectedSha256",
        "version=$version",
        "license=LGPL (configuration has no --enable-gpl)",
        "retrieved=$((Get-Date).ToString('yyyy-MM-dd'))"
    )
    Set-Content -LiteralPath (Join-Path $destPath "FFMPEG-SOURCE.txt") -Value $source -Encoding UTF8
    Write-Output "installed: $destPath"
    Write-Output "ffmpeg sha256: $sha"
    Write-Output "version: $version"
}
finally {
    $resolvedWork = [System.IO.Path]::GetFullPath($work)
    $tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
    if ($resolvedWork.StartsWith($tempRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $resolvedWork -Recurse -Force -ErrorAction SilentlyContinue
    }
}
