param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [Parameter(Mandatory = $true)][string]$Folder,
    [string]$Root = "",
    [double]$ProbeSecs = 6,
    [switch]$ScrollTest,
    [int]$Stride = 0,
    [int]$Steps = 0,
    [int]$IdleSecs = 0,
    [int]$AtlasStatsSecs = 0,
    [string]$MetricsPath = "",
    [string]$LogPath = "D:\Code\Kanalib\.kanalib\frame_probe.log",
    [int]$WaitSecs = 90
)

$ErrorActionPreference = "Stop"
if (Test-Path $LogPath) { Remove-Item $LogPath -Force }
$outPath = "$LogPath.out"
if (Test-Path $outPath) { Remove-Item $outPath -Force }

$env:KANALIB_FRAME_METRICS = "$ProbeSecs"
$env:KANALIB_UI_METRICS = "1"
$env:KANALIB_SKIP_TAG = "1"
if ($Root -ne "") { $env:KANALIB_ROOT = $Root } else { Remove-Item Env:KANALIB_ROOT -ErrorAction SilentlyContinue }
if ($ScrollTest) { $env:KANALIB_UI_SCROLL_TEST = "1" } else { Remove-Item Env:KANALIB_UI_SCROLL_TEST -ErrorAction SilentlyContinue }
if ($Stride -gt 0) { $env:KANALIB_UI_SCROLL_STRIDE = "$Stride" } else { Remove-Item Env:KANALIB_UI_SCROLL_STRIDE -ErrorAction SilentlyContinue }
if ($Steps -gt 0) { $env:KANALIB_UI_SCROLL_STEPS = "$Steps" } else { Remove-Item Env:KANALIB_UI_SCROLL_STEPS -ErrorAction SilentlyContinue }
if ($MetricsPath -ne "") { $env:KANALIB_METRICS_PATH = $MetricsPath } else { Remove-Item Env:KANALIB_METRICS_PATH -ErrorAction SilentlyContinue }
if ($AtlasStatsSecs -gt 0) { $env:KANALIB_ATLAS_STATS = "$AtlasStatsSecs" } else { Remove-Item Env:KANALIB_ATLAS_STATS -ErrorAction SilentlyContinue }

$proc = Start-Process -FilePath $Exe -ArgumentList @("`"$Folder`"") `
    -RedirectStandardError $LogPath -RedirectStandardOutput $outPath -PassThru
Write-Host "started pid=$($proc.Id) probe=$ProbeSecs scroll=$ScrollTest"

# gpui 只在窗口处于活动状态时记录 present 间隔，所以反复把本进程窗口取到前台。
$shell = New-Object -ComObject WScript.Shell
function Invoke-Foreground([int]$targetPid) {
    try { return $shell.AppActivate($targetPid) } catch { return $false }
}
Start-Sleep -Milliseconds 600
Write-Host "foreground_first=$([bool](Invoke-Foreground $proc.Id))"

$deadline = (Get-Date).AddSeconds($WaitSecs)
$hit = $null
while ((Get-Date) -lt $deadline) {
    [bool](Invoke-Foreground $proc.Id) | Out-Null
    if (Test-Path $LogPath) {
        $hit = Select-String -Path $LogPath -Pattern "frame metrics:" -SimpleMatch -ErrorAction SilentlyContinue
        if ($hit) { break }
    }
    if ($proc.HasExited) { break }
    Start-Sleep -Milliseconds 1000
}

if ($hit) {
    $hit.Line
} else {
    Write-Host "NO frame metrics line (process exited=$($proc.HasExited))"
}
if ($hit -and $IdleSecs -gt 0 -and -not $proc.HasExited) {
    Start-Sleep -Seconds $IdleSecs
    $proc.Refresh()
    Write-Host ("idle_working_set_mb={0:N1} idle_private_mb={1:N1}" -f ($proc.WorkingSet64 / 1MB), ($proc.PrivateMemorySize64 / 1MB))
}
Write-Host "--- probe lifecycle lines ---"
Select-String -Path $LogPath -Pattern "frame probe|scan metrics|ui scroll test|atlas stats|alloc stats|image watch" -ErrorAction SilentlyContinue |
    ForEach-Object { $_.Line }

if (-not $proc.HasExited) {
    $proc.Refresh()
    Write-Host ("peak_working_set_mb={0:N1} rss_mb={1:N1} private_mb={2:N1}" -f ($proc.PeakWorkingSet64 / 1MB), ($proc.WorkingSet64 / 1MB), ($proc.PrivateMemorySize64 / 1MB))
    Stop-Process -Id $proc.Id -Force
}
Write-Host "stopped pid=$($proc.Id)"
