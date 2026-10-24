param(
    [string]$SourcePython = "D:\APP\ComfyUI-aki-v3\python",
    [string]$Dest = "",
    [switch]$Force
)

$ErrorActionPreference = "Stop"

if (-not $Dest) {
    $Dest = Join-Path $PSScriptRoot "..\runtime\python"
}
$source = (Resolve-Path $SourcePython).Path
$dest = [System.IO.Path]::GetFullPath($Dest)

if (Test-Path $dest) {
    if (-not $Force) {
        throw "$dest already exists; pass -Force to rebuild"
    }
    Remove-Item -Recurse -Force $dest
}

$site = Join-Path $source "Lib\site-packages"
Write-Host "base  : $source -> $dest"
robocopy $source $dest /E /XD $site /NFL /NDL /NJH /NJS /NP | Out-Null
if ($LASTEXITCODE -ge 8) {
    throw "robocopy base failed: $LASTEXITCODE"
}

$target = Join-Path $dest "Lib\site-packages"
New-Item -ItemType Directory -Force -Path $target | Out-Null

# 运行 PixAI tagger 所需的最小依赖集合；dist 名与目录名不一致的显式列出
$packages = [ordered]@{
    "torch"               = "torch"
    "torchvision"         = "torchvision"
    "transformers"        = "transformers"
    "timm"                = "timm"
    "tokenizers"          = "tokenizers"
    "safetensors"         = "safetensors"
    "numpy"               = "numpy"
    "numpy.libs"          = $null
    "torchgen"            = $null
    "PIL"                 = "pillow"
    "huggingface_hub"     = "huggingface_hub"
    "filelock"            = "filelock"
    "fsspec"              = "fsspec"
    "packaging"           = "packaging"
    "yaml"                = "pyyaml"
    "requests"            = "requests"
    "tqdm"                = "tqdm"
    "regex"               = "regex"
    "urllib3"             = "urllib3"
    "certifi"             = "certifi"
    "idna"                = "idna"
    "charset_normalizer"  = "charset_normalizer"
    "jinja2"              = "jinja2"
    "markupsafe"          = "markupsafe"
    "typing_extensions"   = "typing_extensions"
    "sympy"               = "sympy"
    "networkx"            = "networkx"
    "mpmath"              = "mpmath"
}

foreach ($name in $packages.Keys) {
    $packageDir = Join-Path $site $name
    $packageFile = Join-Path $site "$name.py"
    if (Test-Path $packageDir) {
        robocopy $packageDir (Join-Path $target $name) /E /NFL /NDL /NJH /NJS /NP | Out-Null
        if ($LASTEXITCODE -ge 8) {
            throw "copy $name failed: $LASTEXITCODE"
        }
    }
    elseif (Test-Path $packageFile) {
        Copy-Item $packageFile $target
    }
    else {
        throw "missing dependency: $name"
    }

    $distName = $packages[$name]
    if (-not $distName) {
        continue
    }
    Get-ChildItem $site -Directory -Filter "$($distName.ToLower())-*.dist-info" | ForEach-Object {
        Copy-Item $_.FullName (Join-Path $target $_.Name) -Recurse -Force
    }
}

$python = Join-Path $dest "python.exe"

# 裁剪：运行时用不到的头文件、链接库、测试工具、未加载的模型实现
# 注意 torch/lib 下的 DLL 全部被 torch_global_deps 预加载，一个都不能删
$pruneDirs = @(
    "torch\include",
    "torch\bin",
    "torch\share",
    "networkx"
)
foreach ($dir in $pruneDirs) {
    Remove-Item -Recurse -Force (Join-Path $target $dir) -ErrorAction SilentlyContinue
}
Get-ChildItem (Join-Path $target "torch\lib") -Filter "*.lib" -File | Remove-Item -Force
Get-ChildItem $target -Directory -Filter "networkx-*.dist-info" | Remove-Item -Recurse -Force

# 只保留加载时实际引用到的模型实现
$modelsDir = Join-Path $target "transformers\models"
$keepModels = @("auto", "bert", "encoder_decoder", "speecht5")
Get-ChildItem $modelsDir -Directory | Where-Object {
    $_.Name -ne "__pycache__" -and $keepModels -notcontains $_.Name
} | Remove-Item -Recurse -Force

Get-ChildItem $target -Recurse -Directory -Filter "__pycache__" | Remove-Item -Recurse -Force

Write-Host "verify: $python"
& $python -c "import torch, torchvision, transformers, timm, PIL, numpy, safetensors; from transformers import AutoModel; print('runtime ok', torch.__version__, 'cuda', torch.cuda.is_available())"
if ($LASTEXITCODE -ne 0) {
    throw "runtime smoke test failed"
}

$size = (Get-ChildItem $dest -Recurse -File | Measure-Object -Property Length -Sum).Sum / 1GB
Write-Host ("runtime size: {0:N2} GB" -f $size)
