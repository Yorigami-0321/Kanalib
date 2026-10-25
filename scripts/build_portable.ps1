param(
    [string]$OutDir = "dist/Kanalib-portable",
    [switch]$SkipBuild,
    [switch]$Force,
    [switch]$Zip
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

function Copy-Tree([string]$Source, [string]$Destination) {
    New-Item -ItemType Directory -Path $Destination -Force | Out-Null
    & robocopy $Source $Destination /E /R:1 /W:1 /NFL /NDL /NJH /NJS /NP | Out-Null
    if ($LASTEXITCODE -ge 8) {
        throw "robocopy failed: $Source -> $Destination (exit $LASTEXITCODE)"
    }
}

Copy-Tree $runtime (Join-Path $out "runtime")
Copy-Tree $sidecar (Join-Path $out "sidecar")
Copy-Tree $models (Join-Path $out "dist/models")

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
