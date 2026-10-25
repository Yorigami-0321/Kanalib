$source = 'using System; using System.Runtime.InteropServices; public static class KWin32 { [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h); [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h); [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c); [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h); [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h); [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow(); [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint p); [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId(); [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool c); }'
Add-Type -TypeDefinition $source

function Focus-Window([int]$TargetPid) {
    $proc = Get-Process -Id $TargetPid -ErrorAction SilentlyContinue
    if (-not $proc) { return $false }
    $hwnd = $proc.MainWindowHandle
    if ($hwnd -eq [IntPtr]::Zero) { return $false }
    if ([KWin32]::IsIconic($hwnd)) { [KWin32]::ShowWindow($hwnd, 9) | Out-Null }
    if (-not [KWin32]::IsWindowVisible($hwnd)) { [KWin32]::ShowWindow($hwnd, 5) | Out-Null }
    $fg = [KWin32]::GetForegroundWindow()
    if ($fg -eq $hwnd) { return $true }
    [uint32]$fgPid = 0
    $fgThread = [KWin32]::GetWindowThreadProcessId($fg, [ref]$fgPid)
    $selfThread = [KWin32]::GetCurrentThreadId()
    [KWin32]::AttachThreadInput($selfThread, $fgThread, $true) | Out-Null
    [KWin32]::BringWindowToTop($hwnd) | Out-Null
    [KWin32]::SetForegroundWindow($hwnd) | Out-Null
    [KWin32]::AttachThreadInput($selfThread, $fgThread, $false) | Out-Null
    return ([KWin32]::GetForegroundWindow() -eq $hwnd)
}
