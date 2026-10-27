param(
    [string]$ProcessName = "kanalib",
    [string]$Text = "",
    [string]$ElementName = ""
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$proc = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $proc) { Write-Output "no window"; exit 1 }
$root = [System.Windows.Automation.AutomationElement]::FromHandle($proc.MainWindowHandle)
$all = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)

$edit = $null
foreach ($el in $all) {
    if ($el.Current.ControlType.ProgrammaticName -eq "ControlType.Edit" -and $el.Current.Name -like "*$ElementName*") {
        $edit = $el
        break
    }
}
if (-not $edit) { Write-Output "edit not found"; exit 2 }
Write-Output "found edit: $($edit.Current.Name)"

try {
    $vp = $edit.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
    $vp.SetValue($Text)
    Write-Output "value pattern set: $Text"
} catch {
    Write-Output "no value pattern: $($_.Exception.Message)"
    $edit.SetFocus()
    Write-Output "focused"
}
