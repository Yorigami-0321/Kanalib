param(
    [string]$ProcessName = "kanalib",
    [string]$Name = "",
    [int]$Index = 0
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$proc = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $proc) { Write-Output "no window"; exit 1 }
$root = [System.Windows.Automation.AutomationElement]::FromHandle($proc.MainWindowHandle)
$all = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)

$hits = @()
foreach ($el in $all) {
    if ($Name -and $el.Current.Name -eq $Name) { $hits += $el }
}
if ($hits.Count -eq 0) { Write-Output "not found: $Name"; exit 2 }
$el = $hits[$Index]
try {
    $pattern = $el.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
    $pattern.Invoke()
    Write-Output "invoked: $($el.Current.Name)"
} catch {
    $rect = $el.Current.BoundingRectangle
    $x = [int]($rect.X + $rect.Width / 2)
    $y = [int]($rect.Y + $rect.Height / 2)
    Write-Output "no invoke pattern ($($_.Exception.Message)); rect=$x,$y"
}
