param(
    [string]$CloudHost = "128.1.132.31",
    [string]$CloudUser = "yorigami",
    [string]$SshKey = "$env:USERPROFILE\.ssh\kanalib_deploy",
    [int]$Port = 8793
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$python = Join-Path $root "dist\Kanalib\runtime\python\python.exe"
$server = Join-Path $root "sidecar\tagger\http_server.py"
$model = Join-Path $root "models\pixai-tagger-v1.0"
$outLog = Join-Path $env:TEMP "kanalib-tagger.log"
$errLog = Join-Path $env:TEMP "kanalib-tagger.err"
$tunnelLog = Join-Path $env:TEMP "kanalib-tagger-tunnel.err"
$taggerPidFile = Join-Path $env:TEMP "kanalib-tagger.pid"
$tunnelPidFile = Join-Path $env:TEMP "kanalib-tagger-tunnel.pid"

foreach ($path in @($python, $server, $model, $SshKey)) {
    if (-not (Test-Path -LiteralPath $path)) {
        throw "missing required path: $path"
    }
}

function Test-PidFile([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return $false }
    $saved = Get-Content -LiteralPath $Path -ErrorAction SilentlyContinue
    if (-not $saved) { return $false }
    return [bool](Get-Process -Id ([int]$saved) -ErrorAction SilentlyContinue)
}

if (-not (Test-PidFile $taggerPidFile)) {
    $tagger = Start-Process -FilePath $python -ArgumentList @(
        $server, "--model", $model, "--device", "cuda",
        "--host", "127.0.0.1", "--port", "$Port"
    ) -WindowStyle Hidden -PassThru -RedirectStandardOutput $outLog -RedirectStandardError $errLog
    Set-Content -LiteralPath $taggerPidFile -Value $tagger.Id
    Write-Host "started tagger on 127.0.0.1:$Port"
}

if (-not (Test-PidFile $tunnelPidFile)) {
    $ssh = Join-Path $env:SystemRoot "System32\OpenSSH\ssh.exe"
    $tunnel = Start-Process -FilePath $ssh -ArgumentList @(
        "-N", "-R", "127.0.0.1:$Port:127.0.0.1:$Port",
        "-i", $SshKey,
        "-o", "BatchMode=yes",
        "-o", "ExitOnForwardFailure=yes",
        "-o", "ServerAliveInterval=30",
        "-o", "ServerAliveCountMax=3",
        "$CloudUser@$CloudHost"
    ) -WindowStyle Hidden -PassThru -RedirectStandardError $tunnelLog
    Set-Content -LiteralPath $tunnelPidFile -Value $tunnel.Id
    Write-Host "started reverse tunnel to $CloudUser@$CloudHost"
}

Start-Sleep -Seconds 10
$health = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/health" -TimeoutSec 10
$health | ConvertTo-Json -Compress
