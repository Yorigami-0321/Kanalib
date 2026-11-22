<#
.SYNOPSIS
Verifies the Kanalib P1 private-network checklist without storing private addresses or credentials.

.EXAMPLE
./scripts/verify_private_net.ps1 -PublicBaseUrl https://kanalib.example.com

.EXAMPLE
./scripts/verify_private_net.ps1 -WebPrivateBaseUrl http://kanalib-web.et.net:8788 -BoxPrivateBaseUrl http://kanalib-box.et.net:8790

Pass endpoint values at execution time. No defaults contain virtual-network IPs or secrets.
#>

[CmdletBinding()]
param(
    [string]$PublicBaseUrl,
    [string]$WebPrivateBaseUrl,
    [string]$BoxPrivateBaseUrl,
    [string]$BoxTransferUrl,
    [string]$EasyTierCli = "D:\APP\Easytier\easytier-cli.exe",
    [string]$RelayTcpHost,
    [int]$RelayTcpPort = 11010,
    [int]$TimeoutSeconds = 5,
    [int]$TransferSizeBytes = 100MB
)

$ErrorActionPreference = "Continue"
$results = [System.Collections.Generic.List[object]]::new()

function Add-Check {
    param(
        [string]$Name,
        [ValidateSet("PASS", "FAIL", "BLOCKED")][string]$Status,
        [string]$Detail
    )
    $results.Add([pscustomobject]@{
        Check = $Name
        Status = $Status
        Detail = $Detail
    })
    $color = switch ($Status) {
        "PASS" { "Green" }
        "FAIL" { "Red" }
        default { "Yellow" }
    }
    Write-Host ("{0,-38} {1,-8} {2}" -f $Name, $Status, $Detail) -ForegroundColor $color
}

function Get-HttpResult {
    param(
        [string]$Url,
        [string]$Name,
        [string]$MissingPrerequisite
    )
    if ([string]::IsNullOrWhiteSpace($Url)) {
        Add-Check $Name BLOCKED "missing: $MissingPrerequisite"
        return $null
    }
    try {
        $response = Invoke-WebRequest -Uri $Url -Method Get -TimeoutSec $TimeoutSeconds -MaximumRedirection 0 -ErrorAction Stop
        return $response
    } catch {
        $response = $_.Exception.Response
        if ($response) {
            return [pscustomobject]@{ StatusCode = [int]$response.StatusCode }
        }
        Add-Check $Name FAIL $_.Exception.Message
        return $null
    }
}

Write-Host "Kanalib P1 private-network verification" -ForegroundColor Cyan
Write-Host "Time: $((Get-Date).ToString("o"))"

if (Test-Path -LiteralPath $EasyTierCli) {
    try {
        $routeOutput = & $EasyTierCli route 2>&1 | Out-String
        if ($routeOutput -match "15888|connection refused|refused") {
            Add-Check "EasyTier route output" BLOCKED "CLI found, but EasyTier core RPC is not reachable (default 127.0.0.1:15888); start the local core/network"
        } else {
        $routeLines = @($routeOutput -split "`r?`n" | Where-Object { $_ -match "kanalib-(web|box|desktop)" })
        if ($routeLines.Count -gt 0) {
            $routeText = $routeLines -join " | "
            if ($routeText -match "\bDIRECT\b") {
                Add-Check "EasyTier direct route" PASS $routeText
            } else {
                Add-Check "EasyTier route state" PASS "reachable; relay or non-direct route: $routeText"
            }
        } else {
            Add-Check "EasyTier route output" FAIL "cli present but no expected kanalib-* peers"
        }
        }
    } catch {
        Add-Check "EasyTier route output" FAIL $_.Exception.Message
    }
} else {
    Add-Check "EasyTier route output" BLOCKED "missing: $EasyTierCli"
}

foreach ($endpoint in @(
        @{ Name = "DNS/web"; HostName = "kanalib-web.et.net" },
        @{ Name = "DNS/box"; HostName = "kanalib-box.et.net" },
        @{ Name = "DNS/desktop"; HostName = "kanalib-desktop.et.net" }
    )) {
    $resolved = Resolve-DnsName $endpoint.HostName -ErrorAction SilentlyContinue
    if ($resolved -and $resolved.IPAddress) {
        $isLocalPrivateNetwork = @($resolved.IPAddress).Count -gt 0 -and (@($resolved.IPAddress | Where-Object { $_ -notmatch "^(10\.126\.126\.|127\.)" }).Count -eq 0)
        if ($isLocalPrivateNetwork) {
            Add-Check $endpoint.Name PASS (@($resolved.IPAddress) -join ", ")
        } else {
            Add-Check $endpoint.Name BLOCKED "resolved outside EasyTier private range; public/wildcard DNS is not private-network resolution"
        }
    } else {
        Add-Check $endpoint.Name BLOCKED "EasyTier Magic DNS not available"
    }
}

if ([string]::IsNullOrWhiteSpace($WebPrivateBaseUrl)) {
    Add-Check "desktop -> web private" BLOCKED "missing: -WebPrivateBaseUrl"
} else {
    $health = Get-HttpResult -Url ($WebPrivateBaseUrl.TrimEnd("/") + "/api/health") -Name "desktop -> web private" -MissingPrerequisite "-WebPrivateBaseUrl"
    if ($health) {
        if ($health.StatusCode -eq 200) { Add-Check "desktop -> web private" PASS "health=200" }
        else { Add-Check "desktop -> web private" FAIL "health status=$($health.StatusCode)" }
    }
}

if ([string]::IsNullOrWhiteSpace($BoxPrivateBaseUrl)) {
    Add-Check "desktop -> box private" BLOCKED "missing: -BoxPrivateBaseUrl"
} else {
    $box = Get-HttpResult -Url $BoxPrivateBaseUrl -Name "desktop -> box private" -MissingPrerequisite "-BoxPrivateBaseUrl"
    if ($box) {
        if ($box.StatusCode -in 200, 301, 302, 307, 308) { Add-Check "desktop -> box private" PASS "reachable status=$($box.StatusCode)" }
        else { Add-Check "desktop -> box private" FAIL "status=$($box.StatusCode)" }
    }
}

if ([string]::IsNullOrWhiteSpace($WebPrivateBaseUrl) -or [string]::IsNullOrWhiteSpace($BoxPrivateBaseUrl)) {
    Add-Check "box -> web private" BLOCKED "missing: box-side web endpoint test"
} else {
    Add-Check "box -> web private" BLOCKED "requires a probe run on box or an SSH/agent endpoint; this script runs locally"
}

if ([string]::IsNullOrWhiteSpace($PublicBaseUrl)) {
    Add-Check "public private routes return 404" BLOCKED "missing: -PublicBaseUrl"
} else {
    foreach ($path in @("/internal/ingest/batch", "/api/desktop/push")) {
        $response = Get-HttpResult -Url ($PublicBaseUrl.TrimEnd("/") + $path) -Name "public$path" -MissingPrerequisite "-PublicBaseUrl"
        if ($response) {
            if ($response.StatusCode -eq 404) { Add-Check "public$path" PASS "404" }
            else { Add-Check "public$path" FAIL "status=$($response.StatusCode), expected 404" }
        }
    }
}

if ([string]::IsNullOrWhiteSpace($RelayTcpHost)) {
    Add-Check "TCP relay reachability" BLOCKED "missing: -RelayTcpHost"
} else {
    $test = Test-NetConnection -ComputerName $RelayTcpHost -Port $RelayTcpPort -InformationLevel Detailed -WarningAction SilentlyContinue
    if ($test.TcpTestSucceeded) { Add-Check "TCP relay reachability" PASS "$RelayTcpHost`:$RelayTcpPort" }
    else { Add-Check "TCP relay reachability" FAIL "$RelayTcpHost`:$RelayTcpPort" }
}

if ([string]::IsNullOrWhiteSpace($BoxTransferUrl)) {
    Add-Check "100MB transfer" BLOCKED "missing: -BoxTransferUrl"
} else {
    $temporaryFile = Join-Path ([System.IO.Path]::GetTempPath()) ("kanalib-private-net-" + [Guid]::NewGuid().ToString("N") + ".bin")
    try {
        $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
        Invoke-WebRequest -Uri $BoxTransferUrl -OutFile $temporaryFile -TimeoutSec 300 -ErrorAction Stop
        $stopwatch.Stop()
        $actualSize = (Get-Item -LiteralPath $temporaryFile).Length
        if ($actualSize -ge $TransferSizeBytes) {
            $mbps = [math]::Round(($actualSize * 8 / 1MB) / $stopwatch.Elapsed.TotalSeconds, 2)
            Add-Check "100MB transfer" PASS ("bytes={0}; seconds={1:N2}; Mbps={2}" -f $actualSize, $stopwatch.Elapsed.TotalSeconds, $mbps)
        } else {
            Add-Check "100MB transfer" FAIL "received $actualSize bytes; expected at least $TransferSizeBytes"
        }
    } catch {
        Add-Check "100MB transfer" FAIL $_.Exception.Message
    } finally {
        Remove-Item -LiteralPath $temporaryFile -Force -ErrorAction SilentlyContinue
    }
}

Add-Check "2c2g egress delta" BLOCKED "requires NIC counters on web host and synchronized transfer window"
Add-Check "UDP-blocked fallback" BLOCKED "requires deliberate client/endpoint configuration that blocks UDP; not changed by this script"

Write-Host ""
Write-Host "Summary" -ForegroundColor Cyan
$results | Format-Table -AutoSize
$failed = @($results | Where-Object Status -eq "FAIL").Count
$blocked = @($results | Where-Object Status -eq "BLOCKED").Count
Write-Host ("PASS={0} FAIL={1} BLOCKED={2}" -f ($results.Count - $failed - $blocked), $failed, $blocked)
if ($failed -gt 0) { exit 1 }
