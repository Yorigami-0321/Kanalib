param(
    [string]$BindAddress = "10.126.126.1",
    [int]$Port = 8793
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$python = Join-Path $root "dist\Kanalib\runtime\python\python.exe"
$server = Join-Path $root "sidecar\tagger\http_server.py"
$model = Join-Path $root "models\pixai-tagger-v1.0"
$outLog = Join-Path $env:TEMP "kanalib-tagger.log"
$errLog = Join-Path $env:TEMP "kanalib-tagger.err"
$pidFile = Join-Path $env:TEMP "kanalib-tagger.pid"
$tokenFile = Join-Path $env:ProgramData "Kanalib\tagger-token"

foreach ($path in @($python, $server, $model)) {
    if (-not (Test-Path -LiteralPath $path)) {
        throw "missing required path: $path"
    }
}

$hasAddress = Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue |
    Where-Object { $_.IPAddress -eq $BindAddress }
if (-not $hasAddress) {
    throw "EasyTier is not up: $BindAddress is missing. Start the local EasyTier client first."
}

if (-not (Test-Path -LiteralPath $tokenFile)) {
    New-Item -ItemType Directory -Force -Path (Split-Path $tokenFile) | Out-Null
    $bytes = New-Object byte[] 32
    [System.Security.Cryptography.RandomNumberGenerator]::Fill($bytes)
    $token = -join ($bytes | ForEach-Object { $_.ToString("x2") })
    Set-Content -LiteralPath $tokenFile -Value $token -NoNewline
    Write-Host "generated tagger token at $tokenFile; copy it to /etc/kanalib/tagger-token on box"
} else {
    $token = (Get-Content -LiteralPath $tokenFile -Raw).Trim()
}
if (-not $token) { throw "tagger token file is empty: $tokenFile" }
$headers = @{ "x-tagger-token" = $token }

if (Test-Path -LiteralPath $pidFile) {
    $saved = Get-Content -LiteralPath $pidFile -ErrorAction SilentlyContinue
    if ($saved -and (Get-Process -Id ([int]$saved) -ErrorAction SilentlyContinue)) {
        Write-Host "tagger already running (pid $saved)"
        Invoke-RestMethod -Uri "http://${BindAddress}:$Port/health" -Headers $headers -TimeoutSec 10 |
            ConvertTo-Json -Compress
        return
    }
}

$tagger = Start-Process -FilePath $python -ArgumentList @(
    $server, "--model", $model, "--device", "cuda",
    "--host", $BindAddress, "--port", "$Port", "--token-file", $tokenFile
) -WindowStyle Hidden -PassThru -RedirectStandardOutput $outLog -RedirectStandardError $errLog
Set-Content -LiteralPath $pidFile -Value $tagger.Id
Write-Host "started tagger on ${BindAddress}:$Port (pid $($tagger.Id))"

for ($attempt = 0; $attempt -lt 20; $attempt++) {
    Start-Sleep -Seconds 2
    try {
        Invoke-RestMethod -Uri "http://${BindAddress}:$Port/health" -Headers $headers -TimeoutSec 5 |
            ConvertTo-Json -Compress
        return
    } catch {
        if ($attempt -eq 19) { throw }
    }
}
