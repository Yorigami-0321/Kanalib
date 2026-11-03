param(
    [long]$Hwnd = 0,
    [string]$OutDir = "D:\Code\Kanalib\.kanalib\ui_baseline",
    # Physical pixels. They must fit inside the target monitor: the secondary is 1920x1280 at 150%,
    # so 1800x1200 is 1200x800 logical and 1536x1000 is 1024x667 logical.
    [string[]]$Sizes = @("1800x1200", "1536x1000"),
    [string]$Origin = "-1900,-610",
    # Activity-rail child indices to visit, resolved by structure so no localised literals are needed.
    [int[]]$PageIndices = @(0, 1, 2, 4),
    [int]$ThemeIndex = 3,
    [int]$SettleMs = 900
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class Baseline {
    [DllImport("user32.dll")]
    public static extern bool SetWindowPos(IntPtr hWnd, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll")]
    public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint flags);
    [DllImport("user32.dll")]
    public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr ctx);
    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("dwmapi.dll")]
    public static extern int DwmGetWindowAttribute(IntPtr hwnd, int attr, out RECT value, int size);
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }
}
'@

$SWP_NOZORDER = 0x4
$SWP_NOACTIVATE = 0x10
$PW_RENDERFULLCONTENT = 2

[Baseline]::SetThreadDpiAwarenessContext([IntPtr](-4)) | Out-Null
$root = [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$Hwnd)
$descendants = [System.Windows.Automation.TreeScope]::Descendants
$children = [System.Windows.Automation.TreeScope]::Children
$trueCondition = [System.Windows.Automation.Condition]::TrueCondition

function Get-Rail {
    $rails = $root.FindAll($descendants, $trueCondition) |
        Where-Object { $_.Current.ControlType.ProgrammaticName -eq "ControlType.ToolBar" }
    foreach ($rail in $rails) {
        $b = $rail.Current.BoundingRectangle
        if ($b.Width -lt 120 -and $b.Height -gt 400) { return $rail }
    }
    return $null
}

function Rail-Buttons {
    $rail = Get-Rail
    if (-not $rail) { return @() }
    return @($rail.FindAll($children, $trueCondition) |
        Where-Object { $_.Current.ControlType.ProgrammaticName -eq "ControlType.Button" })
}

function Invoke-Element($el) {
    if (-not $el) { return $false }
    try {
        $p = $el.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
        $p.Invoke()
        return $true
    } catch { return $false }
}

# Overflow audit: horizontal only, and inside UIA's own coordinate space (window root vs descendants),
# so no DPI translation is involved. Elements above/below the viewport are scroll content, not clips.
function Audit-Overflow([string]$label) {
    $hostRect = $root.Current.BoundingRectangle
    $bad = @()
    foreach ($el in $root.FindAll($descendants, $trueCondition)) {
        $r = $el.Current.BoundingRectangle
        if ($r.Width -le 0 -or $r.Height -le 0) { continue }
        if ($r.Y + $r.Height -lt $hostRect.Y -or $r.Y -gt ($hostRect.Y + $hostRect.Height)) { continue }
        $over = [math]::Max(($r.X + $r.Width) - ($hostRect.X + $hostRect.Width), $hostRect.X - $r.X)
        if ($over -gt 1) {
            $name = $el.Current.Name
            if ($name.Length -gt 20) { $name = $name.Substring(0, 20) }
            $bad += ("{0}|{1}|x{2} right{3} over={4}px" -f $el.Current.ControlType.ProgrammaticName, $name,
                [int]$r.X, [int]($r.X + $r.Width), [int]$over)
        }
    }
    if ($bad.Count -eq 0) {
        Write-Output ("AUDIT {0} h-overflow=0" -f $label)
    } else {
        Write-Output ("AUDIT {0} h-overflow={1}" -f $label, $bad.Count)
        foreach ($b in ($bad | Select-Object -First 6)) { Write-Output ("  " + $b) }
    }
}

function Save-Shot([string]$file) {
    $rect = New-Object Baseline+RECT
    [Baseline]::DwmGetWindowAttribute([IntPtr]$Hwnd, 9, [Ref]$rect,
        [System.Runtime.InteropServices.Marshal]::SizeOf([type][Baseline+RECT])) | Out-Null
    $w = $rect.Right - $rect.Left
    $h = $rect.Bottom - $rect.Top
    if ($w -le 0 -or $h -le 0) { Write-Output "bad window rect, skipped"; return }
    $bmp = New-Object System.Drawing.Bitmap($w, $h)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    [Baseline]::PrintWindow([IntPtr]$Hwnd, $hdc, $PW_RENDERFULLCONTENT) | Out-Null
    $g.ReleaseHdc($hdc)
    $g.Dispose()
    $bmp.Save($file)
    $bmp.Dispose()
    Write-Output ("shot {0} {1}x{2}" -f (Split-Path $file -Leaf), $w, $h)
}

if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Path $OutDir | Out-Null }
$originParts = $Origin.Split(',')
$ox = [int]$originParts[0]
$oy = [int]$originParts[1]

$buttons = Rail-Buttons
Write-Output ("rail buttons=" + $buttons.Count)
for ($i = 0; $i -lt $buttons.Count; $i++) {
    Write-Output ("  [{0}] {1}" -f $i, $buttons[$i].Current.Name)
}

foreach ($size in $Sizes) {
    $parts = $size.Split('x')
    $w = [int]$parts[0]
    $h = [int]$parts[1]
    # Move first, then set the size: Windows rescales a window when it crosses monitors with different
    # DPI, so a single call can push the right/bottom edge back onto the other screen.
    [Baseline]::SetWindowPos([IntPtr]$Hwnd, [IntPtr]::Zero, $ox, $oy, 800, 600, ($SWP_NOZORDER -bor $SWP_NOACTIVATE)) | Out-Null
    Start-Sleep -Milliseconds 300
    [Baseline]::SetWindowPos([IntPtr]$Hwnd, [IntPtr]::Zero, $ox, $oy, $w, $h, ($SWP_NOZORDER -bor $SWP_NOACTIVATE)) | Out-Null
    Start-Sleep -Milliseconds $SettleMs
    $rect = New-Object Baseline+RECT
    [Baseline]::GetWindowRect([IntPtr]$Hwnd, [Ref]$rect) | Out-Null
    Write-Output ("=== size {0}x{1} physical, rect {2},{3}..{4},{5} ===" -f $w, $h, $rect.Left, $rect.Top, $rect.Right, $rect.Bottom)

    $buttons = Rail-Buttons
    foreach ($index in $PageIndices) {
        if ($index -ge $buttons.Count) { continue }
        $pageName = ($buttons[$index].Current.Name -replace '[^\w\-]', '_')
        if (-not (Invoke-Element $buttons[$index])) { Write-Output "skip: no invoke for index $index"; continue }
        Start-Sleep -Milliseconds $SettleMs
        foreach ($theme in @("dark", "light")) {
            if ($theme -eq "light") {
                $buttons = Rail-Buttons
                [void](Invoke-Element $buttons[$ThemeIndex])
                Start-Sleep -Milliseconds $SettleMs
            }
            $file = Join-Path $OutDir ("{0}_{1}x{2}_{3}.png" -f $pageName, $w, $h, $theme)
            Save-Shot $file
            Audit-Overflow ("{0}/{1}x{2}/{3}" -f $pageName, $w, $h, $theme)
            if ($theme -eq "light") {
                $buttons = Rail-Buttons
                [void](Invoke-Element $buttons[$ThemeIndex])
                Start-Sleep -Milliseconds $SettleMs
            }
        }
    }
}
Write-Output "baseline done"
