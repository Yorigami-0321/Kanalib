param(
    [string]$ProcessName = "kanalib",
    [string]$ScrollAmount = "large",
    [int]$Times = 1
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$proc = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $proc) { Write-Output "no window"; exit 1 }
$root = [System.Windows.Automation.AutomationElement]::FromHandle($proc.MainWindowHandle)
$all = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)

$scroller = $null
$best = -1
foreach ($el in $all) {
    try {
        $pattern = $el.GetCurrentPattern([System.Windows.Automation.ScrollPattern]::Pattern)
        $v = $pattern.Current.VerticalScrollable
        $percent = $pattern.Current.VerticalScrollPercent
        if ($v -and $percent -lt 100) {
            $size = $pattern.Current.VerticalViewSize
            if ($size -gt $best) { $best = $size; $scroller = $pattern }
        }
    } catch { }
}
if (-not $scroller) { Write-Output "no scrollable element"; exit 2 }
for ($i = 0; $i -lt $Times; $i++) {
    if ($ScrollAmount -eq "large") { $scroller.Scroll([System.Windows.Automation.ScrollAmount]::LargeDecrement) }
    else { $scroller.Scroll([System.Windows.Automation.ScrollAmount]::SmallDecrement) }
    Start-Sleep -Milliseconds 300
}
Write-Output ("scrolled={0} percent={1}" -f $ScrollAmount, $scroller.Current.VerticalScrollPercent)
