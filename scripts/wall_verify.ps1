#Requires -Version 7.0
<#
.SYNOPSIS
    图片墙前端 headless 验收：构建 web/ -> 起临时静态服务 -> 截图 + 抓渲染后 DOM。

.DESCRIPTION
    只读验收脚本，产物全部落在 -OutDir（默认 .kanalib/wall_verify/）：
        wall.png        整墙截图（找不到浏览器时输出「未截图」）
        dom.html        浏览器渲染后的 DOM（--dump-dom）
        report.json     本次验收的结构化结果
        build.log       npm run build 输出
        server.*.log    静态服务日志（python -m http.server 或 vite preview）

    检查项：
      1) /data/index.json 的 HTTP 状态码为 200 且可解析；
      2) 抽样一条 t512 缩略图的 HTTP 状态码为 200（index -> shard -> 图片链路通）；
      3) 渲染后 DOM 里 <img> 数量（0 视为墙没渲染出来）；
      4) 渲染后 DOM 的可见文本是否出现 error 文案（去掉 script/style 后判断）；
      5) 截图存在且不是空白占位。

    找不到 msedge/chrome 时降级为「仅 HTTP + 静态 shell 检查」：输出里明确写「未截图」，
    退出码 3。无浏览器机器上仍能验证构建产物与数据链路，但不假装跑过截图。
    端口随机（-Port 0 自动挑空闲端口），静态服务只按自己的 PID 停止，不按进程名杀。

.PARAMETER WebDir
    前端目录（含 package.json / index.html），默认 web。
.PARAMETER DataDir
    发布器导出目录，默认 .kanalib/wall_export。构建后若 dist/data 缺失会把它拷进 dist/data；
    也认 web/public/data（vite 构建时自动进 dist）。
.PARAMETER OutDir
    产物目录，默认 .kanalib/wall_verify。
.PARAMETER Port
    固定端口；0 表示随机空闲端口。
.PARAMETER PageWaitMs
    传给浏览器的 --virtual-time-budget（毫秒），给懒加载图片留时间。
.PARAMETER NoBuild
    跳过 npm run build，直接用已有 web/dist。
.PARAMETER SkipScreenshot
    完全跳过浏览器，只做 HTTP 检查（等价于强制降级）。

.EXAMPLE
    pwsh -NoLogo -NoProfile -File scripts/wall_verify.ps1
.EXAMPLE
    pwsh -NoLogo -NoProfile -File scripts/wall_verify.ps1 -NoBuild

.NOTES
    退出码：0 全部通过；1 检查失败（HTTP/渲染/DOM 文案）；2 环境问题（web/ 未就绪、
    构建失败、数据缺失、静态服务起不来）；3 降级通过（无浏览器，未截图）。
    不开 StrictMode：报告用 ordered 字典拼装，严格模式只放大噪音。
#>
[CmdletBinding()]
param(
    [string]$WebDir = "web",
    [string]$DataDir = ".kanalib/wall_export",
    [string]$OutDir = ".kanalib/wall_verify",
    [int]$Port = 0,
    [int]$Width = 1440,
    [int]$Height = 900,
    [int]$PageWaitMs = 8000,
    [int]$ReadyTimeoutSec = 30,
    [int]$BrowserTimeoutSec = 120,
    [string]$Browser = "",
    [ValidateSet("auto", "python", "node")]
    [string]$Server = "auto",
    [string]$UrlHash = "#/",
    [switch]$NoBuild,
    [switch]$SkipScreenshot
)

$ErrorActionPreference = "Stop"

# ---- 输出与失败收集 ----
function Write-Step([string]$Text) {
    Write-Host ""
    Write-Host "==> $Text" -ForegroundColor Cyan
}
function Write-Ok([string]$Text) { Write-Host "[通过] $Text" -ForegroundColor Green }
function Write-Note([string]$Text) { Write-Host "[提示] $Text" -ForegroundColor DarkGray }
function Write-WarnLine([string]$Text) { Write-Host "[警告] $Text" -ForegroundColor Yellow }
function Write-FailLine([string]$Text) { Write-Host "[失败] $Text" -ForegroundColor Red }

$script:Failures = [System.Collections.Generic.List[string]]::new()
$script:Notes = [System.Collections.Generic.List[string]]::new()

function Add-Failure([string]$Text) {
    $script:Failures.Add($Text)
    Write-FailLine $Text
}
function Add-Note([string]$Text) {
    $script:Notes.Add($Text)
    Write-Note $Text
}

# 脚本在 scripts/，仓库根是父目录；参数里的相对路径按仓库根解析，与 cwd 无关。
$repoRoot = Split-Path -Parent $PSScriptRoot

function Resolve-RepoPath([string]$Path) {
    if ([System.IO.Path]::IsPathRooted($Path)) { return [System.IO.Path]::GetFullPath($Path) }
    return [System.IO.Path]::GetFullPath((Join-Path $repoRoot $Path))
}

function Get-FreePort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $listener.Start()
    try {
        return ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
    } finally {
        $listener.Stop()
    }
}

# Start-Process 的 -ArgumentList 是空格拼接的字符串，带空格的参数必须自己加引号。
function Quote-Arg([string]$Text) {
    if ($Text -match '\s') { return '"' + $Text + '"' }
    return $Text
}

function Wait-HttpReady([string]$Url, [int]$TimeoutSec) {
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        try {
            $resp = Invoke-WebRequest -Uri $Url -TimeoutSec 5 -SkipHttpErrorCheck -ErrorAction Stop
            if ($resp.StatusCode -eq 200) { return $true }
        } catch {
            Start-Sleep -Milliseconds 300
        }
    }
    return $false
}

function Invoke-Headless([string]$BrowserPath, [string[]]$Arguments, [string]$StdoutPath, [string]$StderrPath, [int]$TimeoutSec) {
    # 只等自己启动的浏览器 PID；超时按 PID 精确结束，不按进程名杀。
    $argString = ($Arguments | ForEach-Object { Quote-Arg $_ }) -join ' '
    $startArgs = @{
        FilePath               = $BrowserPath
        ArgumentList           = $argString
        PassThru               = $true
        RedirectStandardOutput = $StdoutPath
        RedirectStandardError  = $StderrPath
    }
    $proc = Start-Process @startArgs
    if (-not $proc.WaitForExit($TimeoutSec * 1000)) {
        try { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue } catch { }
        Add-Note ("浏览器 {0} 超时（{1}s），已按 PID {2} 结束" -f (Split-Path -Leaf $BrowserPath), $TimeoutSec, $proc.Id)
        return $false
    }
    return $true
}

# Node/npm 解析：优先 node + npm-cli.js，绕开 npm.ps1 的执行策略问题。
function Resolve-NpmInvocation {
    $node = Get-Command node -ErrorAction SilentlyContinue
    if (-not $node) { return $null }
    $nodeDir = Split-Path -Parent $node.Source
    $npmCli = Join-Path $nodeDir "node_modules\npm\bin\npm-cli.js"
    if (Test-Path $npmCli) {
        return [pscustomobject]@{ File = $node.Source; Prefix = @($npmCli) }
    }
    $npmCmd = Get-Command npm.cmd -ErrorAction SilentlyContinue
    if (-not $npmCmd) { $npmCmd = Get-Command npm -ErrorAction SilentlyContinue }
    if (-not $npmCmd) { return $null }
    return [pscustomobject]@{ File = $npmCmd.Source; Prefix = @() }
}

function Find-BrowserPath([string]$Explicit) {
    if ($Explicit) {
        if (-not (Test-Path $Explicit)) { throw "指定的浏览器不存在：$Explicit" }
        return (Get-Item $Explicit).FullName
    }
    $x86 = [Environment]::GetEnvironmentVariable("ProgramFiles(x86)")
    $candidates = [System.Collections.Generic.List[string]]::new()
    foreach ($root in @($env:ProgramFiles, $x86, $env:LOCALAPPDATA)) {
        if (-not $root) { continue }
        $candidates.Add((Join-Path $root "Microsoft\Edge\Application\msedge.exe"))
        $candidates.Add((Join-Path $root "Google\Chrome\Application\chrome.exe"))
    }
    foreach ($candidate in $candidates) {
        if (Test-Path $candidate) { return (Get-Item $candidate).FullName }
    }
    foreach ($name in @("msedge", "chrome")) {
        $cmd = Get-Command $name -ErrorAction SilentlyContinue
        if ($cmd) { return $cmd.Source }
    }
    return $null
}

# 只保留可见文本（去 script/style 内容与标签），避免把 minified JS 里的 error 当页面报错。
function Get-VisibleText([string]$Html) {
    $text = [regex]::Replace($Html, '(?is)<script\b.*?</script>', ' ')
    $text = [regex]::Replace($text, '(?is)<style\b.*?</style>', ' ')
    $text = [regex]::Replace($text, '(?s)<[^>]*>', ' ')
    $text = [System.Net.WebUtility]::HtmlDecode($text)
    return ([regex]::Replace($text, '\s+', ' ')).Trim()
}

function Get-PngSize([string]$Path) {
    # PNG IHDR：8 字节签名 + 4 字节长度 + "IHDR" 之后是宽高（大端 uint32）。
    try {
        $bytes = [System.IO.File]::ReadAllBytes($Path)
        if ($bytes.Length -lt 24) { return $null }
        # 注意：必须显式 [int] 转换，byte 上的 -shl 会按 byte 截断，只有最低字节的结果是对的。
        $w = ([int]$bytes[16] -shl 24) -bor ([int]$bytes[17] -shl 16) -bor ([int]$bytes[18] -shl 8) -bor [int]$bytes[19]
        $h = ([int]$bytes[20] -shl 24) -bor ([int]$bytes[21] -shl 16) -bor ([int]$bytes[22] -shl 8) -bor [int]$bytes[23]
        return [pscustomobject]@{ Width = $w; Height = $h }
    } catch { return $null }
}


# ---------------- 0. 路径与产物目录 ----------------
$webPath = Resolve-RepoPath $WebDir
$dataPath = Resolve-RepoPath $DataDir
$outPath = Resolve-RepoPath $OutDir
$distPath = Join-Path $webPath "dist"

Write-Host "图片墙 headless 验收 —— $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')"
Write-Host "  仓库根   $repoRoot"
Write-Host "  前端     $webPath"
Write-Host "  数据     $dataPath"
Write-Host "  产物     $outPath"
if (-not (Test-Path $outPath)) { New-Item -ItemType Directory -Path $outPath -Force | Out-Null }

$result = [ordered]@{
    started_at         = (Get-Date).ToString("o")
    repo_root          = $repoRoot
    web_dir            = $webPath
    data_dir           = $dataPath
    out_dir            = $outPath
    site_dir           = $null
    port               = 0
    server             = $null
    url                = $null
    build              = "skipped"
    browser            = $null
    screenshot         = $null
    screenshot_size    = $null
    dom_file           = $null
    img_count          = $null
    img_t512_refs      = $null
    error_text         = @()
    http               = [ordered]@{}
    degraded           = $false
    screenshot_skipped = $false
    failures           = @()
    notes              = @()
}

$serverProc = $null
$browserPath = $null
$exitCode = 0

try {
    # ---------------- 1. 前置检查 ----------------
    Write-Step "1/6 前置检查"
    $packageJson = Join-Path $webPath "package.json"
    if (-not (Test-Path $packageJson)) {
        Write-FailLine "web/ 尚未就绪：缺少 $packageJson（F-081 前端未落地或路径不对）"
        Write-Host "        本脚本只做验收，不生成前端；可用 -WebDir 指定别的目录。" -ForegroundColor DarkGray
        $exitCode = 2
        return
    }
    if (-not (Test-Path (Join-Path $webPath "index.html"))) {
        Write-FailLine "缺少 $(Join-Path $webPath 'index.html')（vite 入口缺失）"
        $exitCode = 2
        return
    }
    Write-Ok "web/ 就绪（package.json + index.html）"

    # ---------------- 2. 构建 ----------------
    Write-Step "2/6 构建前端"
    $buildLog = Join-Path $outPath "build.log"
    if ($NoBuild) {
        if (-not (Test-Path (Join-Path $distPath "index.html"))) {
            Write-FailLine "指定了 -NoBuild，但 $distPath 里没有构建产物（缺 index.html）"
            $exitCode = 2
            return
        }
        Write-Note "跳过构建（-NoBuild），直接用已有 dist"
        $result.build = "skipped"
    } else {
        $npm = Resolve-NpmInvocation
        if (-not $npm) {
            Write-FailLine "找不到 node/npm，无法构建；装 Node 或改用 -NoBuild 复用已有 dist"
            $exitCode = 2
            return
        }
        Write-Note ("运行：{0} {1} run build（cwd={2}）" -f $npm.File, ($npm.Prefix -join ' '), $webPath)
        Push-Location $webPath
        try {
            $buildOutput = & $npm.File @($npm.Prefix) run build 2>&1
            $buildExit = $LASTEXITCODE
        } finally {
            Pop-Location
        }
        ($buildOutput | Out-String) | Set-Content -LiteralPath $buildLog -Encoding utf8
        $buildOutput | Select-Object -Last 25 | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkGray }
        if ($buildExit -ne 0) {
            $result.build = "failed"
            Write-FailLine "npm run build 失败（exit $buildExit），完整日志：$buildLog"
            $exitCode = 2
            return
        }
        if (-not (Test-Path (Join-Path $distPath "index.html"))) {
            $result.build = "missing-dist"
            Write-FailLine "构建退出码 0 但 $distPath 没有 index.html，检查 vite 的 build.outDir"
            $exitCode = 2
            return
        }
        $result.build = "ok"
        Write-Ok "npm run build 通过（日志 $buildLog）"
    }

    # ---------------- 3. 站点副本与数据落位 ----------------
    Write-Step "3/6 站点副本与数据落位"
    # 验收不往 web/ 里写：把 dist 整份复制到产物目录下的 site/，data 也叠加在副本里，静态服务只读这份副本。
    $sitePath = Join-Path $outPath "site"
    if (Test-Path $sitePath) { Remove-Item -LiteralPath $sitePath -Recurse -Force }
    Copy-Item -Path $distPath -Destination $sitePath -Recurse -Force
    $result.site_dir = $sitePath
    $siteData = Join-Path $sitePath "data"
    $distData = Join-Path $distPath "data"
    $exportIndex = Join-Path $dataPath "index.json"

    if (Test-Path $exportIndex) {
        # 发布器导出目录与 vite public 不是一处：整份覆盖站点副本的 data/（先删，避免上一轮残留分片）。
        if (Test-Path $siteData) { Remove-Item -LiteralPath $siteData -Recurse -Force }
        New-Item -ItemType Directory -Path $siteData -Force | Out-Null
        Copy-Item -Path (Join-Path $dataPath "*") -Destination $siteData -Recurse -Force
        $siteFiles = @(Get-ChildItem $siteData -Recurse -File).Count
        if (-not (Test-Path (Join-Path $siteData "index.json"))) {
            Write-FailLine "拷贝后站点副本里仍缺少 data/index.json，检查导出目录结构（$dataPath）"
            $exitCode = 2
            return
        }
        Write-Ok "已把导出目录拷入站点副本 data/（$siteFiles 个文件）"
    } elseif (Test-Path (Join-Path $siteData "index.json")) {
        Write-Note "导出目录 $DataDir 不可用，沿用构建产物自带的 data/（web/public/data）"
        if (Test-Path $distData) { Add-Note "构建产物的 dist/data 与站点副本 data/ 内容一致，仅供本次验收" }
    } else {
        Write-FailLine "找不到数据：$exportIndex 与站点副本 data/index.json 都不存在"
        Write-Host "        先跑发布器导出到 $dataPath，再用 -DataDir 指向它。" -ForegroundColor DarkGray
        $exitCode = 2
        return
    }

    $seedIndexPath = Join-Path $siteData "index.json"
    try {
        $seedIndex = Get-Content -LiteralPath $seedIndexPath -Raw -Encoding utf8 | ConvertFrom-Json
    } catch {
        Write-FailLine "index.json 不是合法 JSON：$($_.Exception.Message)"
        $exitCode = 2
        return
    }
    if (-not $seedIndex.PSObject.Properties.Name.Contains("count")) {
        Add-Failure "index.json 缺少 count 字段（契约 v1）"
    }
    Write-Note ("数据摘要：count={0}，shard_size={1}，shards={2}" -f $seedIndex.count, $seedIndex.shard_size, @($seedIndex.shards).Count)

    # ---------------- 4. 起临时静态服务 ----------------
    Write-Step "4/6 起临时静态服务"
    if ($Port -eq 0) { $Port = Get-FreePort }
    $baseUrl = "http://127.0.0.1:$Port"
    $pageUrl = "$baseUrl/$UrlHash"
    $result.port = $Port
    $result.url = $pageUrl

    $serverOut = Join-Path $outPath "server.stdout.log"
    $serverErr = Join-Path $outPath "server.stderr.log"
    $python = Get-Command python -ErrorAction SilentlyContinue
    if ($Server -eq "python" -and -not $python) {
        Write-FailLine "指定了 -Server python，但找不到 python 可执行文件"
        $exitCode = 2
        return
    }
    if ($Server -ne "node" -and $python) {
        $serverArgs = (@("-m", "http.server", "$Port", "--bind", "127.0.0.1", "--directory", $sitePath) | ForEach-Object { Quote-Arg $_ }) -join ' '
        $startArgs = @{
            FilePath               = $python.Source
            ArgumentList           = $serverArgs
            PassThru               = $true
            WindowStyle            = "Hidden"
            RedirectStandardOutput = $serverOut
            RedirectStandardError  = $serverErr
        }
        $serverProc = Start-Process @startArgs
        $result.server = "python -m http.server $Port（PID $($serverProc.Id)，root=$sitePath）"
    } else {
        # 没有 python 时用内置的 node 静态服务（十几行、只读、零依赖），只服务站点副本。
        $node = Get-Command node -ErrorAction SilentlyContinue
        if (-not $node) {
            Write-FailLine "既没有 python（-m http.server），也没有 node：无法起静态服务"
            $exitCode = 2
            return
        }
        $serveJs = Join-Path $outPath "serve_site.mjs"
        $serveSource = @'
// 只读静态服务：给没有 python 的环境用，避免为了预览引入任何依赖。
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize, sep } from "node:path";

const root = normalize(process.argv[2]);
const port = Number(process.argv[3]);
const types = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".png": "image/png",
  ".webp": "image/webp",
  ".svg": "image/svg+xml",
  ".ico": "image/x-icon",
  ".txt": "text/plain; charset=utf-8"
};

createServer(async (req, res) => {
  try {
    const url = new URL(req.url, "http://127.0.0.1");
    let rel = decodeURIComponent(url.pathname);
    if (rel.endsWith("/")) rel += "index.html";
    const target = normalize(join(root, rel));
    if (target !== root && !target.startsWith(root + sep)) {
      res.writeHead(403).end("forbidden");
      return;
    }
    const body = await readFile(target);
    const headers = {
      "content-type": types[extname(target).toLowerCase()] || "application/octet-stream",
      "cache-control": "no-store",
      "content-length": String(body.length)
    };
    if (req.method === "HEAD") {
      res.writeHead(200, headers).end();
      return;
    }
    res.writeHead(200, headers).end(body);
  } catch {
    res.writeHead(404, { "content-type": "text/plain; charset=utf-8" }).end("not found");
  }
}).listen(port, "127.0.0.1", () => {
  console.log("serving " + root + " on " + port);
});
'@
        Set-Content -LiteralPath $serveJs -Value $serveSource -Encoding utf8
        $serverArgs = (@($serveJs, $sitePath, "$Port") | ForEach-Object { Quote-Arg $_ }) -join ' '
        $startArgs = @{
            FilePath               = $node.Source
            ArgumentList           = $serverArgs
            WorkingDirectory       = $outPath
            PassThru               = $true
            WindowStyle            = "Hidden"
            RedirectStandardOutput = $serverOut
            RedirectStandardError  = $serverErr
        }
        $serverProc = Start-Process @startArgs
        $result.server = "node serve_site.mjs $Port（PID $($serverProc.Id)，root=$sitePath）"
    }
    Write-Note $result.server

    if (-not (Wait-HttpReady "$baseUrl/" $ReadyTimeoutSec)) {
        $tail = ""
        foreach ($log in @($serverErr, $serverOut)) {
            if (Test-Path $log) { $tail += (Get-Content -LiteralPath $log -Tail 15 -Encoding utf8 | Out-String) }
        }
        Write-FailLine "静态服务 $ReadyTimeoutSec 秒内没起来：$baseUrl/"
        if ($tail.Trim()) { Write-Host ($tail.Trim()) -ForegroundColor DarkGray }
        $exitCode = 2
        return
    }
    Write-Ok "静态服务就绪：$baseUrl/"

    # ---------------- 5. HTTP 检查 ----------------
    Write-Step "5/6 HTTP 检查"
    $indexUrl = "$baseUrl/data/index.json"
    $indexResp = $null
    try {
        $indexResp = Invoke-WebRequest -Uri $indexUrl -TimeoutSec 15 -SkipHttpErrorCheck
    } catch {
        Add-Failure "GET $indexUrl 抛异常：$($_.Exception.Message)"
    }
    if ($indexResp) {
        $result.http["data/index.json"] = [int]$indexResp.StatusCode
        if ($indexResp.StatusCode -eq 200) {
            Write-Ok "GET /data/index.json -> 200（$($indexResp.RawContentLength) 字节）"
            try {
                $servedIndex = $indexResp.Content | ConvertFrom-Json
                if ($null -ne $servedIndex.count) { Write-Note ("服务端 index.count = {0}" -f $servedIndex.count) }
            } catch {
                Add-Failure "服务端返回的 index.json 无法解析：$($_.Exception.Message)"
            }
        } else {
            Add-Failure "GET /data/index.json -> $($indexResp.StatusCode)（期望 200）"
        }
    }

    # 抽样一条 t512：index -> shard -> 图片 链路打通才算能看。
    $sampleShard = @($seedIndex.shards)[0]
    if ($sampleShard -and $sampleShard.path) {
        $shardRel = ($sampleShard.path -replace '^/', '')
        $shardUrl = "$baseUrl/data/$shardRel"
        try {
            $shardResp = Invoke-WebRequest -Uri $shardUrl -TimeoutSec 15 -SkipHttpErrorCheck
            $result.http[$sampleShard.path] = [int]$shardResp.StatusCode
            if ($shardResp.StatusCode -ne 200) {
                Add-Failure "GET /data/$($sampleShard.path) -> $($shardResp.StatusCode)（期望 200）"
            } else {
                Write-Ok "GET /data/$($sampleShard.path) -> 200"
                $firstItem = @(($shardResp.Content | ConvertFrom-Json).items)[0]
                if ($firstItem -and $firstItem.id) {
                    $imgUrl = "$baseUrl/data/t512/$($firstItem.id).jpg"
                    $imgResp = Invoke-WebRequest -Uri $imgUrl -Method Head -TimeoutSec 15 -SkipHttpErrorCheck
                    $result.http["t512 抽样"] = [int]$imgResp.StatusCode
                    if ($imgResp.StatusCode -eq 200) {
                        Write-Ok "抽样 t512/$($firstItem.id).jpg -> 200（$($imgResp.Headers['Content-Type'])）"
                    } else {
                        Add-Failure "抽样 t512/$($firstItem.id).jpg -> $($imgResp.StatusCode)（期望 200）"
                    }
                } else {
                    Add-Note "首个分片没有 items，跳过 t512 抽样（空库属正常）"
                }
            }
        } catch {
            Add-Failure "GET /data/$($sampleShard.path) 抛异常：$($_.Exception.Message)"
        }
    } else {
        Add-Note "index.shards 为空，跳过分片/图片抽样"
    }

    # ---------------- 6. 浏览器截图 + DOM ----------------
    Write-Step "6/6 浏览器渲染与截图"
    if ($SkipScreenshot) {
        Add-Note "指定了 -SkipScreenshot：未截图（只做 HTTP 检查）"
        $result.screenshot = "未截图"
        $result.screenshot_skipped = $true
        $result.degraded = $true
    } else {
        $browserPath = Find-BrowserPath $Browser
        if (-not $browserPath) {
            Add-Note "未找到 msedge/chrome：降级为仅 HTTP + 静态 shell 检查，未截图"
            $result.degraded = $true
            $result.screenshot = "未截图"
        }
    }

    if ($script:Failures.Count -gt 0 -and $result.degraded -and $browserPath) {
        Write-Note "已有失败项，仍继续跑一次浏览器，尽量把渲染信息留全"
    }

    if ($browserPath) {
        $result.browser = $browserPath
        Write-Note "浏览器：$browserPath"
        $pngPath = Join-Path $outPath "wall.png"
        $domPath = Join-Path $outPath "dom.html"
        $profileDir = Join-Path $outPath "browser-profile"
        if (Test-Path $pngPath) { Remove-Item -LiteralPath $pngPath -Force }
        $browserOut = Join-Path $outPath "browser.stdout.log"
        $browserErr = Join-Path $outPath "browser.stderr.log"

        $commonArgs = @(
            "--headless=new"
            "--disable-gpu"
            "--hide-scrollbars"
            "--no-first-run"
            "--no-default-browser-check"
            "--disable-extensions"
            "--force-device-scale-factor=1"
            "--window-size=$Width,$Height"
            "--virtual-time-budget=$PageWaitMs"
            "--user-data-dir=$profileDir"
        )

        # 截图：--screenshot 在 headless 下写完自行退出。
        $shotArgs = $commonArgs + @("--screenshot=$pngPath", $pageUrl)
        $null = Invoke-Headless $browserPath $shotArgs $browserOut $browserErr $BrowserTimeoutSec
        if (Test-Path $pngPath) {
            $pngItem = Get-Item $pngPath
            $size = Get-PngSize $pngPath
            $result.screenshot = $pngPath
            $result.screenshot_size = [int]$pngItem.Length
            # 阈值只是空白页的粗筛（1440x900 纯色图约 5–10KB），真正的判据是 DOM 里的 img 数。
            if ($pngItem.Length -lt 2000) {
                Add-Failure ("截图只有 {0} 字节，疑似空白页（{1}）" -f $pngItem.Length, $pngPath)
            } elseif ($size) {
                Write-Ok ("截图 {0}（{1}x{2}，{3} 字节）" -f $pngPath, $size.Width, $size.Height, $pngItem.Length)
            } else {
                Write-Ok ("截图 {0}（{1} 字节）" -f $pngPath, $pngItem.Length)
            }
        } else {
            Add-Failure "浏览器没有产出截图：$pngPath（看 browser.stderr.log）"
        }

        # dump-dom：渲染后的 DOM，用来数 <img> 与找 error 文案。
        $domArgs = $commonArgs + @("--dump-dom", $pageUrl)
        $null = Invoke-Headless $browserPath $domArgs $domPath $browserErr $BrowserTimeoutSec
        $domLength = 0
        if (Test-Path $domPath) { $domLength = (Get-Item $domPath).Length }
        # 空态/错误态页面本身可能只有一两百字节，所以只看是否小于 60 字节，不按大小判完整。
        if ($domLength -lt 60) {
            # 少数环境 --headless=new 的 --dump-dom 输出为空，退回旧 headless 重试一次。
            Add-Note "dom.html 过短（$domLength 字节），改用 --headless 重试"
            $legacyArgs = @("--headless") + @($commonArgs | Where-Object { $_ -ne "--headless=new" }) + @("--dump-dom", $pageUrl)
            $null = Invoke-Headless $browserPath $legacyArgs $domPath $browserErr $BrowserTimeoutSec
            if (Test-Path $domPath) { $domLength = (Get-Item $domPath).Length }
        }
        $domHtml = ""
        if (Test-Path $domPath) { $domHtml = Get-Content -LiteralPath $domPath -Raw -Encoding utf8 }
        if ($domLength -lt 60 -or $domHtml -notmatch '(?i)<html') {
            Add-Failure "没能抓到渲染后的 DOM（dom.html 仅 $domLength 字节，或缺 <html>）"
        } else {
            $result.dom_file = $domPath
            $imgCount = ([regex]::Matches($domHtml, '<img\b', 'IgnoreCase')).Count
            $t512Refs = ([regex]::Matches($domHtml, 'src="[^"]*t512/', 'IgnoreCase')).Count
            $result.img_count = $imgCount
            $result.img_t512_refs = $t512Refs
            if ($imgCount -gt 0) {
                Write-Ok ("渲染后 <img> {0} 个（其中 t512 引用 {1} 个）" -f $imgCount, $t512Refs)
            } else {
                Add-Failure "渲染后 DOM 里没有任何 <img>：墙没出图（数据没进 dist/data 或前端报错）"
            }
            if ($domHtml -match 'vite-error-overlay') {
                Add-Failure "DOM 里出现 vite 错误浮层（构建或运行时异常）"
            }
            $visible = Get-VisibleText $domHtml
            $errorPatterns = @(
                "加载失败", "加载出错", "出错了", "无法加载", "未能加载", "图片加载失败",
                "Failed to fetch", "NetworkError", "Uncaught", "TypeError", "Cannot read"
            )
            $hits = @()
            foreach ($pattern in $errorPatterns) {
                if ($visible.Contains($pattern)) { $hits += $pattern }
            }
            $result.error_text = $hits
            if ($hits.Count -gt 0) {
                Add-Failure ("页面可见文本出现 error 文案：{0}" -f ($hits -join ", "))
                $visibleIndex = $visible.IndexOf($hits[0])
                if ($visibleIndex -ge 0) {
                    $start = [Math]::Max(0, $visibleIndex - 60)
                    $len = [Math]::Min(160, $visible.Length - $start)
                    Write-Host ("        上下文：" + $visible.Substring($start, $len)) -ForegroundColor DarkGray
                }
            } else {
                Write-Ok "页面可见文本没有 error 文案"
            }
        }
    } else {
        # 降级：没有浏览器也确认静态 shell 能被 HTTP 取到、含 #root 挂载点。
        try {
            $shellResp = Invoke-WebRequest -Uri "$baseUrl/" -TimeoutSec 15 -SkipHttpErrorCheck
            $result.http["/"] = [int]$shellResp.StatusCode
            if ($shellResp.StatusCode -eq 200) {
                if ($shellResp.Content -match '<div id="root"') {
                    Write-Ok "静态 shell -> 200，含 #root 挂载点（未截图）"
                } else {
                    Add-Failure "index.html 缺少 #root 挂载点"
                }
            } else {
                Add-Failure "GET / -> $($shellResp.StatusCode)（期望 200）"
            }
        } catch {
            Add-Failure "GET / 抛异常：$($_.Exception.Message)"
        }
        $result.screenshot = "未截图"
    }
} catch {
    Add-Failure "脚本异常：$($_.Exception.Message)"
    if ($_.ScriptStackTrace) { Write-Host $_.ScriptStackTrace -ForegroundColor DarkGray }
} finally {
    # 只停自己启动的 PID（不按名字 kill，避免误伤用户已有的 python/npm）。
    if ($serverProc -and -not $serverProc.HasExited) {
        try {
            Stop-Process -Id $serverProc.Id -Force -ErrorAction Stop
            Write-Note "已停止静态服务 PID=$($serverProc.Id)"
        } catch {
            Write-WarnLine "停止静态服务 PID=$($serverProc.Id) 失败：$($_.Exception.Message)"
        }
    }
    # 浏览器 profile 是临时目录，收掉免得每次留几十 MB。
    $profileDir = Join-Path $outPath "browser-profile"
    if (Test-Path $profileDir) {
        try { Remove-Item -LiteralPath $profileDir -Recurse -Force -ErrorAction Stop } catch { }
    }

    # ---------------- 结论 ----------------
    $result.failures = @($script:Failures)
    $result.notes = @($script:Notes)
    $result.finished_at = (Get-Date).ToString("o")
    $reportPath = Join-Path $outPath "report.json"
    try {
        ($result | ConvertTo-Json -Depth 8) | Set-Content -LiteralPath $reportPath -Encoding utf8
    } catch {
        Write-WarnLine "report.json 写入失败：$($_.Exception.Message)"
    }

    Write-Host ""
    Write-Host "—— 结论 ——" -ForegroundColor Cyan
    # 前置检查/构建/数据/服务失败走的是 return + 提前置 2，这里不能把退出码刷回 0。
    $envBlocked = ($exitCode -eq 2)
    if ($script:Failures.Count -gt 0) {
        Write-Host ("  × 验收失败：{0} 项" -f $script:Failures.Count) -ForegroundColor Red
        foreach ($failure in $script:Failures) { Write-Host "    - $failure" -ForegroundColor Red }
        $exitCode = 1
    } elseif ($envBlocked) {
        Write-Host "  ! 环境未就绪，验收未完成（原因见上面 1/6–4/6 步骤）" -ForegroundColor Yellow
        $exitCode = 2
    } elseif ($result.degraded) {
        Write-Host "  ~ 降级通过：未截图（HTTP / 静态 shell 检查通过，浏览器不可用）" -ForegroundColor Yellow
        $exitCode = 3
    } else {
        Write-Host "  √ 验收通过" -ForegroundColor Green
        $exitCode = 0
    }
    if ($null -ne $result.img_count) { Write-Host "    <img> 数量：$($result.img_count)" }
    if ($result.screenshot) { Write-Host "    截图：$($result.screenshot)" } else { Write-Host "    截图：未截图" }
    Write-Host "    报告：$reportPath"
    Write-Host "    退出码：$exitCode（0 通过 / 1 失败 / 2 环境问题 / 3 降级未截图）"
    # 退出码必须在这里落下：步骤失败走的是 return（跳过脚本末尾），finally 里 exit 才能保住 2。
    exit $exitCode
}

# 正常走到这里说明 finally 没接管（理论上不会出现），保底返回 0。
exit 0

