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

foreach ($path in @($python, $server, $model, $SshKey)) {
    if (-not (Test-Path -LiteralPath $path)) {
        throw "missing required path: $path"
    }
}

$listening = netstat -ano | Select-String ":$Port\s+.*LISTENING"
if (-not $listening) {
    Start-Process -FilePath $python -ArgumentList @(
        $server, "--model", $model, "--device", "cuda",
        "--host", "127.0.0.1", "--port", "$Port"
    ) -WindowStyle Hidden -RedirectStandardOutput $outLog -RedirectStandardError $errLog
    Write-Host "started tagger on 127.0.0.1:$Port"
}

$tunnel = netstat -ano | Select-String "127.0.0.1:$Port\s+.*LISTENING"
if (-not $tunnel) {
    $ssh = Join-Path $env:SystemRoot "System32\OpenSSH\ssh.exe"
    Start-Process -FilePath $ssh -ArgumentList @(
        "-N", "-R", "127.0.0.1:$Port:127.0.0.1:$Port",
        "-i", $SshKey,
        "-o", "BatchMode=yes",
        "-o", "ExitOnForwardFailure=yes",
        "-o", "ServerAliveInterval=30",
        "-o", "ServerAliveCountMax=3",
        "$CloudUser@$CloudHost"
    ) -WindowStyle Hidden -RedirectStandardError $tunnelLog
    Write-Host "started reverse tunnel to $CloudUser@$CloudHost"
}

Start-Sleep -Seconds 10
$health = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/health" -TimeoutSec 10
$health | ConvertTo-Json -Compress
