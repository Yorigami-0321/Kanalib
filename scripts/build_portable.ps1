param(
    [string]$OutDir = "dist/Kanalib-portable",
    [switch]$SkipBuild,
    [switch]$Force,
    [switch]$Zip,
    [switch]$SkipFfmpeg,
    [switch]$AllowGplFfmpeg,
    [switch]$ValidateOnly
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Stop-Packaging([string]$Message) {
    throw "portable package pre-flight failed: $Message"
}

function Assert-File([string]$Path, [string]$Label, [string]$Hint) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        $message = "missing $Label`: $Path"
        if (-not [string]::IsNullOrWhiteSpace($Hint)) {
            $message = $message + " ($Hint)"
        }
        Stop-Packaging $message
    }
}

function Assert-Directory([string]$Path, [string]$Label) {
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        Stop-Packaging "missing $Label directory: $Path"
    }
}

function Get-AppVersion([string]$ExePath) {
    $cargoToml = Join-Path $root "Cargo.toml"
    if (Test-Path -LiteralPath $cargoToml -PathType Leaf) {
        $text = Get-Content -LiteralPath $cargoToml -Raw
        if ($text -match '(?ms)^\[package\][^\[]*?^version\s*=\s*"([^"]+)"') {
            return $matches[1]
        }
    }
    try {
        $info = (Get-Item -LiteralPath $ExePath).VersionInfo
        if (-not [string]::IsNullOrWhiteSpace($info.FileVersion)) {
            return $info.FileVersion
        }
    } catch {
    }
    return "unknown"
}

function Get-RelativePackagePath([string]$Base, [string]$FullName) {
    $baseFull = [System.IO.Path]::GetFullPath($Base)
    $fileFull = [System.IO.Path]::GetFullPath($FullName)
    $prefix = $baseFull
    if (-not $prefix.EndsWith("\")) {
        $prefix = $prefix + "\"
    }
    if (-not $fileFull.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "internal error: file is outside package root: $FullName"
    }
    return $fileFull.Substring($prefix.Length).Replace("\", "/")
}

function Add-ManifestFile([System.Collections.ArrayList]$List, [string]$PackageRoot, [string]$RelativePath, [string]$Category) {
    $full = Join-Path $PackageRoot ($RelativePath.Replace("/", "\"))
    if (-not (Test-Path -LiteralPath $full -PathType Leaf)) {
        Stop-Packaging "expected packaged file is missing: $RelativePath"
    }
    $item = Get-Item -LiteralPath $full
    $hash = (Get-FileHash -LiteralPath $full -Algorithm SHA256).Hash
    $null = $List.Add([pscustomobject]@{
        Path = $RelativePath
        Category = $Category
        Bytes = $item.Length
        Sha256 = $hash
    })
}

$exe = Join-Path $root "target/release/kanalib.exe"
$runtime = Join-Path $root "runtime"
$python = Join-Path $runtime "python/python.exe"
$sidecar = Join-Path $root "sidecar"
$serverPy = Join-Path $sidecar "tagger/server.py"
$repairPy = Join-Path $sidecar "tagger/repair.py"
$models = Join-Path $root "dist/models"
$modelDir = Join-Path $models "pixai-tagger-v1.0"
$model = Join-Path $modelDir "model.safetensors"
$readme = Join-Path $root "scripts/portable_readme.txt"
$fetchFfmpeg = Join-Path $root "scripts/fetch_ffmpeg_lgpl.ps1"
$out = Join-Path $root $OutDir

if (-not $SkipBuild -and -not $ValidateOnly) {
    & cargo build --release
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build --release failed with exit code $LASTEXITCODE"
    }
}

Assert-File $exe "release executable" "run cargo build --release first, or omit -SkipBuild"
Assert-Directory $runtime "runtime"
Assert-File $python "bundled Python runtime" "expected runtime/python/python.exe from scripts/build_runtime.ps1"
Assert-Directory $sidecar "sidecar"
Assert-Directory (Join-Path $sidecar "tagger") "sidecar/tagger"
Assert-File $serverPy "tagger server" "expected sidecar/tagger/server.py"
Assert-File $repairPy "repair helper" "expected sidecar/tagger/repair.py"
Assert-Directory $models "models"
Assert-Directory $modelDir "model directory"
Assert-File $model "tagger model weights" "expected dist/models/pixai-tagger-v1.0/model.safetensors"
Assert-File $readme "portable README" "expected scripts/portable_readme.txt"

foreach ($path in @($exe, $python, $serverPy, $repairPy, $model, $readme)) {
    $item = Get-Item -LiteralPath $path
    if ($item.Length -le 0) {
        Stop-Packaging "input file is empty: $path"
    }
}

$optionalModelFiles = @("config.json", "preprocessor_config.json", "tagger_pipeline.py")
foreach ($name in $optionalModelFiles) {
    $optionalPath = Join-Path $modelDir $name
    if (-not (Test-Path -LiteralPath $optionalPath -PathType Leaf)) {
        Write-Warning "model metadata missing: $optionalPath (tagger may fail to load)"
    }
}

$outFull = [System.IO.Path]::GetFullPath($out)
$rootFull = [System.IO.Path]::GetFullPath($root).TrimEnd("\")
$outTrimmed = $outFull.TrimEnd("\")
if ($outTrimmed -eq $rootFull) {
    Stop-Packaging "output directory cannot be the repository root: $out"
}
if ($rootFull.StartsWith($outTrimmed + "\", [System.StringComparison]::OrdinalIgnoreCase)) {
    Stop-Packaging "output directory cannot contain the repository root: $out"
}
foreach ($sourceDir in @($runtime, $sidecar, $models)) {
    $sourceFull = [System.IO.Path]::GetFullPath($sourceDir).TrimEnd("\")
    if ($outTrimmed -eq $sourceFull -or $outTrimmed.StartsWith($sourceFull + "\", [System.StringComparison]::OrdinalIgnoreCase)) {
        Stop-Packaging "output directory must not be inside packaging input $sourceDir`: $out"
    }
}

$ffmpegSource = Join-Path $runtime "ffmpeg"
$ffmpegExe = Join-Path $ffmpegSource "bin/ffmpeg.exe"
$ffmpegLicenseFile = Join-Path $ffmpegSource "LICENSE.txt"
$ffmpegSourceFile = Join-Path $ffmpegSource "FFMPEG-SOURCE.txt"
$ffmpegState = "absent"
$ffmpegLicense = "n/a"
$ffmpegVersion = "n/a"
$ffmpegSourceUrl = "n/a"
$ffmpegIsGpl = $false
$ffmpegConfigLine = ""

if ($SkipFfmpeg) {
    $ffmpegState = "skipped"
} elseif (-not (Test-Path -LiteralPath $ffmpegExe -PathType Leaf)) {
    $ffmpegState = "absent"
    Write-Warning "ffmpeg: $ffmpegExe not found; package will use the image crate fallback"
} else {
    if (-not (Test-Path -LiteralPath $ffmpegLicenseFile -PathType Leaf)) {
        Stop-Packaging "ffmpeg is present but LICENSE.txt is missing: $ffmpegLicenseFile (rerun scripts/fetch_ffmpeg_lgpl.ps1 -Force or add the upstream license)"
    }
    if (-not (Test-Path -LiteralPath $ffmpegSourceFile -PathType Leaf)) {
        Stop-Packaging "ffmpeg is present but FFMPEG-SOURCE.txt is missing: $ffmpegSourceFile (record the download source and archive hash before packaging)"
    }
    $versionLines = @(& $ffmpegExe -hide_banner -version 2>&1)
    if ($LASTEXITCODE -ne 0) {
        Stop-Packaging "ffmpeg version probe failed: $ffmpegExe"
    }
    $ffmpegConfigLine = [string]($versionLines | Where-Object { $_ -like "configuration:*" } | Select-Object -First 1)
    if ([string]::IsNullOrWhiteSpace($ffmpegConfigLine)) {
        Stop-Packaging "cannot determine ffmpeg license: configuration line missing from '$ffmpegExe -version'"
    }
    $ffmpegIsGpl = $ffmpegConfigLine -match "--enable-gpl"
    if ($ffmpegIsGpl -and -not $AllowGplFfmpeg) {
        Stop-Packaging "refusing to bundle a GPL ffmpeg build; use scripts/fetch_ffmpeg_lgpl.ps1 or pass -AllowGplFfmpeg after a legal review"
    }
    if ($ffmpegIsGpl) {
        Write-Warning "bundling a GPL ffmpeg build; the package must include the GPL license and source offer"
    }
    $ffmpegState = "bundled"
    $ffmpegLicense = if ($ffmpegIsGpl) { "GPL" } else { "LGPL" }
    $ffmpegVersion = [string]($versionLines | Select-Object -First 1)
    $sourceLine = [string](Get-Content -LiteralPath $ffmpegSourceFile | Where-Object { $_ -like "source=*" } | Select-Object -First 1)
    if (-not [string]::IsNullOrWhiteSpace($sourceLine)) {
        $ffmpegSourceUrl = $sourceLine.Substring(7)
    }
}

$appVersion = Get-AppVersion $exe

if ($ValidateOnly) {
    Write-Output "pre-flight: ok"
    Write-Output "app_version=$appVersion"
    Write-Output "exe=$exe"
    Write-Output "runtime=$runtime"
    Write-Output "sidecar=$sidecar"
    Write-Output "models=$models"
    Write-Output "ffmpeg_state=$ffmpegState"
    if ($ffmpegState -eq "bundled") {
        Write-Output "ffmpeg_license=$ffmpegLicense"
        Write-Output "ffmpeg_version=$ffmpegVersion"
    }
    Write-Output "validate-only: no files were copied"
    exit 0
}

if (Test-Path -LiteralPath $out) {
    $outItem = Get-Item -LiteralPath $out -Force
    if (($outItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        Stop-Packaging "refusing to replace output '$out' because it is a junction or symbolic link; remove it with rmdir first"
    }
    if (-not $Force) {
        throw "output already exists: $out (pass -Force to replace it)"
    }
    Remove-Item -LiteralPath $out -Recurse -Force
}

New-Item -ItemType Directory -Path $out | Out-Null
New-Item -ItemType Directory -Path (Join-Path $out "dist") | Out-Null

Copy-Item -LiteralPath $exe -Destination (Join-Path $out "kanalib.exe")
Copy-Item -LiteralPath $readme -Destination (Join-Path $out "README.txt")
if (Test-Path -LiteralPath $fetchFfmpeg -PathType Leaf) {
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

$dataExcludes = @(
    (Join-Path $runtime ".kanalib"),
    (Join-Path $sidecar ".kanalib"),
    (Join-Path $models ".kanalib")
)
$runtimeExcludes = @($ffmpegSource) + $dataExcludes
Copy-Tree $runtime (Join-Path $out "runtime") $runtimeExcludes
Copy-Tree $sidecar (Join-Path $out "sidecar") $dataExcludes
Copy-Tree $models (Join-Path $out "dist/models") $dataExcludes

if (Test-Path -LiteralPath (Join-Path $out ".kanalib")) {
    Stop-Packaging "refusing to package a .kanalib data directory: $(Join-Path $out '.kanalib')"
}

if ($ffmpegState -eq "bundled") {
    $outFfmpeg = Join-Path $out "runtime/ffmpeg"
    $outBin = Join-Path $outFfmpeg "bin"
    New-Item -ItemType Directory -Path $outBin -Force | Out-Null
    Get-ChildItem -LiteralPath (Join-Path $ffmpegSource "bin") -File |
        Where-Object { $_.Name -ne "ffplay.exe" } |
        ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $outBin }
    Copy-Item -LiteralPath $ffmpegLicenseFile -Destination $outFfmpeg
    Copy-Item -LiteralPath $ffmpegSourceFile -Destination $outFfmpeg
    $hashes = Get-ChildItem -LiteralPath $outBin -File |
        Sort-Object -Property Name |
        ForEach-Object {
            $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
            "{0}  {1}" -f $hash, $_.Name
        }
    $buildRecord = @(
        "version=$ffmpegVersion",
        "license=$ffmpegLicense",
        "gpl=$ffmpegIsGpl",
        $ffmpegConfigLine,
        "--- sha256 ---"
    ) + $hashes
    Set-Content -LiteralPath (Join-Path $outFfmpeg "FFMPEG-BUILD.txt") -Value $buildRecord -Encoding UTF8
    Write-Output "ffmpeg: bundled from $ffmpegSource (license=$ffmpegLicense gpl=$ffmpegIsGpl)"
} elseif ($ffmpegState -eq "skipped") {
    Write-Output "ffmpeg: skipped by -SkipFfmpeg"
} else {
    Write-Output "ffmpeg: not bundled; package will use the image crate fallback"
}

$packageFiles = @(Get-ChildItem -LiteralPath $out -Recurse -File)
$totalBytes = ($packageFiles | Measure-Object -Property Length -Sum).Sum
if ($null -eq $totalBytes) {
    $totalBytes = 0
}
$totalBytes = [int64]$totalBytes

$manifestEntries = New-Object System.Collections.ArrayList
Add-ManifestFile $manifestEntries $out "kanalib.exe" "app"
Add-ManifestFile $manifestEntries $out "README.txt" "docs"
if (Test-Path -LiteralPath (Join-Path $out "fetch-ffmpeg-lgpl.ps1") -PathType Leaf) {
    Add-ManifestFile $manifestEntries $out "fetch-ffmpeg-lgpl.ps1" "tools"
}
Add-ManifestFile $manifestEntries $out "runtime/python/python.exe" "runtime"
if (Test-Path -LiteralPath (Join-Path $out "runtime/python/pythonw.exe") -PathType Leaf) {
    Add-ManifestFile $manifestEntries $out "runtime/python/pythonw.exe" "runtime"
}
Add-ManifestFile $manifestEntries $out "sidecar/tagger/server.py" "sidecar"
Add-ManifestFile $manifestEntries $out "sidecar/tagger/repair.py" "sidecar"
Add-ManifestFile $manifestEntries $out "dist/models/pixai-tagger-v1.0/model.safetensors" "models"
foreach ($name in $optionalModelFiles) {
    $relative = "dist/models/pixai-tagger-v1.0/" + $name
    if (Test-Path -LiteralPath (Join-Path $out ($relative.Replace("/", "\"))) -PathType Leaf) {
        Add-ManifestFile $manifestEntries $out $relative "models"
    }
}
if ($ffmpegState -eq "bundled") {
    $ffmpegFiles = @(Get-ChildItem -LiteralPath (Join-Path $out "runtime/ffmpeg") -Recurse -File)
    foreach ($file in $ffmpegFiles) {
        $relative = Get-RelativePackagePath $out $file.FullName
        Add-ManifestFile $manifestEntries $out $relative "ffmpeg"
    }
}

$manifestLines = New-Object System.Collections.ArrayList
$null = $manifestLines.Add("Kanalib portable package manifest")
$null = $manifestLines.Add("format=1")
$null = $manifestLines.Add("app_version=$appVersion")
$null = $manifestLines.Add("platform=windows-x64")
$null = $manifestLines.Add("ffmpeg_state=$ffmpegState")
$null = $manifestLines.Add("ffmpeg_version=$ffmpegVersion")
$null = $manifestLines.Add("ffmpeg_license=$ffmpegLicense")
$null = $manifestLines.Add("ffmpeg_gpl=$ffmpegIsGpl")
$null = $manifestLines.Add("ffmpeg_source=$ffmpegSourceUrl")
$null = $manifestLines.Add("file_count=$($packageFiles.Count)")
$null = $manifestLines.Add("total_bytes=$totalBytes")
$null = $manifestLines.Add("total_size_mib=$([math]::Round($totalBytes / 1MB, 2))")
$null = $manifestLines.Add("hash_algorithm=SHA256")
$null = $manifestLines.Add("file_count_scope=package excluding PACKAGE-MANIFEST.txt")
$null = $manifestLines.Add("")
$null = $manifestLines.Add("[files]")
$null = $manifestLines.Add("sha256  bytes  category  path")
$sortedEntries = @($manifestEntries | Sort-Object -Property Path)
foreach ($entry in $sortedEntries) {
    $null = $manifestLines.Add(("{0}  {1}  {2}  {3}" -f $entry.Sha256, $entry.Bytes, $entry.Category, $entry.Path))
}
$manifestPath = Join-Path $out "PACKAGE-MANIFEST.txt"
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllLines($manifestPath, [string[]]$manifestLines.ToArray(), $utf8NoBom)

$files = @(Get-ChildItem -LiteralPath $out -Recurse -File)
$bytes = ($files | Measure-Object -Property Length -Sum).Sum
$size = if ($null -eq $bytes) { 0 } else { [math]::Round($bytes / 1GB, 2) }
Write-Output "portable package: $out"
Write-Output ("files={0} size={1} GB" -f $files.Count, $size)
Write-Output "manifest: $manifestPath"
Write-Output ("manifest entries={0} package files={1} bytes={2}" -f $sortedEntries.Count, $packageFiles.Count, $totalBytes)

if ($Zip) {
    $zipPath = $out + ".zip"
    if (Test-Path -LiteralPath $zipPath) {
        Remove-Item -LiteralPath $zipPath -Force
    }
    $tarExe = Join-Path $env:SystemRoot "System32/tar.exe"
    if (-not (Test-Path -LiteralPath $tarExe -PathType Leaf)) {
        $tarExe = "tar.exe"
    }
    & $tarExe -a -c -f $zipPath -C $out .
    if ($LASTEXITCODE -ne 0) {
        throw "tar zip failed with exit code $LASTEXITCODE (tar: $tarExe)"
    }
    Write-Output "zip: $zipPath"
}
