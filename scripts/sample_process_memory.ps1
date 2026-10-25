param(
    [Parameter(Mandatory = $true)][string]$ProcessName,
    [Parameter(Mandatory = $true)][string]$LogPath,
    [int]$IntervalMs = 2000
)

$ErrorActionPreference = "Stop"
"elapsed_ms,working_set_mb,private_mb" | Set-Content -Path $LogPath
$started = Get-Date
while ($true) {
    $proc = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $proc) { break }
    $elapsed = [int]((Get-Date) - $started).TotalMilliseconds
    "{0},{1:N1},{2:N1}" -f $elapsed, ($proc.WorkingSet64 / 1MB), ($proc.PrivateMemorySize64 / 1MB) |
        Add-Content -Path $LogPath
    Start-Sleep -Milliseconds $IntervalMs
}
