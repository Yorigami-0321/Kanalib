param(
    [string]$ProcessName = "kanalib",
    [string]$Filter = "",
    [int]$Max = 200
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$proc = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $proc) { Write-Output "no window"; exit 1 }
$root = [System.Windows.Automation.AutomationElement]::FromHandle($proc.MainWindowHandle)
$all = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
Write-Output ("elements=" + $all.Count)
$i = 0
foreach ($el in $all) {
    $i++
    if ($i -gt $Max) { break }
    $name = $el.Current.Name
    if ($Filter -and $name -notlike "*$Filter*") { continue }
    $rect = $el.Current.BoundingRectangle
    Write-Output ("{0}|{1}|{2}|{3},{4},{5},{6}" -f $el.Current.ControlType.ProgrammaticName, $el.Current.AutomationId, $name, [int]$rect.X, [int]$rect.Y, [int]$rect.Width, [int]$rect.Height)
}
