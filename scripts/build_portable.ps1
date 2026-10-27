param(
    [string]$OutDir = "dist/Kanalib-portable",
    [switch]$SkipBuild,
    [switch]$Force,
    [switch]$Zip,
    [switch]$SkipFfmpeg,
    [switch]$AllowGplFfmpeg
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if (-not $SkipBuild) {
    & cargo build --release
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build --release failed with exit code $LASTEXITCODE"
    }
}

$exe = Join-Path $root "target/release/kanalib.exe"
$runtime = Join-Path $root "runtime"
$sidecar = Join-Path $root "sidecar"
$models = Join-Path $root "dist/models"
$readme = Join-Path $root "scripts/portable_readme.txt"
$out = Join-Path $root $OutDir

$required = @(
    $exe,
    (Join-Path $root "runtime/python/python.exe"),
    (Join-Path $root "sidecar/tagger/server.py"),
    (Join-Path $root "sidecar/tagger/repair.py"),
    (Join-Path $root "dist/models/pixai-tagger-v1.0/model.safetensors"),
    $readme
)
foreach ($path in $required) {
    if (-not (Test-Path -LiteralPath $path)) {
        throw "missing packaging input: $path"
    }
}

if (Test-Path -LiteralPath $out) {
    if (-not $Force) {
        throw "output already exists: $out (pass -Force to replace it)"
    }
    Remove-Item -LiteralPath $out -Recurse -Force
}

New-Item -ItemType Directory -Path $out | Out-Null
New-Item -ItemType Directory -Path (Join-Path $out "dist") | Out-Null

Copy-Item -LiteralPath $exe -Destination (Join-Path $out "kanalib.exe")
Copy-Item -LiteralPath $readme -Destination (Join-Path $out "README.txt")
$fetchFfmpeg = Join-Path $root "scripts/fetch_ffmpeg_lgpl.ps1"
if (Test-Path -LiteralPath $fetchFfmpeg) {
    Copy-Item -LiteralPath $fetchFfmpeg -Destination (Join-Path $out "fetch-ffmpeg-lgpl.ps1")
}

function Copy-Tree([string]$Source, [string]$Destination, [string[]]$Exclude = @()) {
    New-Item -ItemType Directory -Path $Destination -Force | Out-Null
    $arguments = @($Source, $Destination, "/E", "/R:1", "/W:1", "/NFL", "/NDL", "/NJH", "/NJS", "/NP")
    if ($Exclude.Count -gt 0) {
        $arguments += "/XD"
        $arguments += $Exclude
    }
    & robocopy @arguments | Out-Null
    if ($LASTEXITCODE -ge 8) {
        throw "robocopy failed: $Source -> $Destination (exit $LASTEXITCODE)"
    }
}

$ffmpegSource = Join-Path $root "runtime/ffmpeg"
Copy-Tree $runtime (Join-Path $out "runtime") @($ffmpegSource)
Copy-Tree $sidecar (Join-Path $out "sidecar")
Copy-Tree $models (Join-Path $out "dist/models")

$ffmpegExe = Join-Path $ffmpegSource "bin/ffmpeg.exe"
if ($SkipFfmpeg) {
    Write-Output "ffmpeg: skipped by -SkipFfmpeg"
} elseif (-not (Test-Path -LiteralPath $ffmpegExe)) {
    Write-Warning "ffmpeg: runtime/ffmpeg/bin/ffmpeg.exe not found; package will use the image crate fallback"
} else {
    $versionLines = @(& $ffmpegExe -hide_banner -version 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw "ffmpeg version probe failed: $ffmpegExe"
    }
    $configLine = ($versionLines | Where-Object { $_ -like "configuration:*" } | Select-Object -First 1)
    if ([string]::IsNullOrWhiteSpace($configLine)) {
        throw "cannot determine ffmpeg license: configuration line missing"
    }
    $isGpl = $configLine -match "--enable-gpl"
    if ($isGpl -and -not $AllowGplFfmpeg) {
        throw "refusing to bundle a GPL ffmpeg build; use scripts/fetch_ffmpeg_lgpl.ps1 or pass -AllowGplFfmpeg after a legal review"
    }
    if ($isGpl) {
        Write-Warning "bundling a GPL ffmpeg build; the package must include the GPL license and source offer"
    }
    $outFfmpeg = Join-Path $out "runtime/ffmpeg"
    $outBin = Join-Path $outFfmpeg "bin"
    New-Item -ItemType Directory -Path $outBin -Force | Out-Null
    Get-ChildItem -LiteralPath (Join-Path $ffmpegSource "bin") -File |
        Where-Object { $_.Name -ne "ffplay.exe" } |
        ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $outBin }
    foreach ($name in @("LICENSE.txt", "FFMPEG-SOURCE.txt")) {
        $path = Join-Path $ffmpegSource $name
        if (Test-Path -LiteralPath $path) {
            Copy-Item -LiteralPath $path -Destination $outFfmpeg
        }
    }
    $hashes = Get-ChildItem -LiteralPath $outBin -File | ForEach-Object {
        $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
        "{0}  {1}" -f $hash, $_.Name
    }
    $licenseLabel = if ($isGpl) { "GPL" } else { "LGPL" }
    $versionLine = [string](& $ffmpegExe -hide_banner -version 2>&1 | Select-Object -First 1)
    $buildRecord = @(
        "version=$versionLine",
        "license=$licenseLabel",
        "gpl=$isGpl",
        $configLine,
        "--- sha256 ---"
    ) + $hashes
    Set-Content -LiteralPath (Join-Path $outFfmpeg "FFMPEG-BUILD.txt") -Value $buildRecord -Encoding UTF8
    Write-Output "ffmpeg: bundled from $ffmpegSource (gpl=$isGpl)"
}

$files = Get-ChildItem -LiteralPath $out -Recurse -File
$bytes = ($files | Measure-Object -Property Length -Sum).Sum
$size = if ($null -eq $bytes) { 0 } else { [math]::Round($bytes / 1GB, 2) }
Write-Output "portable package: $out"
Write-Output ("files={0} size={1} GB" -f $files.Count, $size)

if ($Zip) {
    $zipPath = $out + ".zip"
    if (Test-Path -LiteralPath $zipPath) {
        Remove-Item -LiteralPath $zipPath -Force
    }
    & tar.exe -a -c -f $zipPath -C $out .
    if ($LASTEXITCODE -ne 0) {
        throw "tar zip failed with exit code $LASTEXITCODE"
    }
    Write-Output "zip: $zipPath"
}
