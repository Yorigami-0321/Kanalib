param(
    [string]$ProcessName = "kanalib",
    [int]$X = 1279,
    [int]$Y = 203,
    [string]$Text = "",
    [int]$Backspace = 0
)

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public class WinMsg {
    [DllImport("user32.dll", SetLastError=true)]
    public static extern bool PostMessage(IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")]
    public static extern bool EnumWindows(EnumProc callback, IntPtr lParam);
    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
    public static IntPtr FindLargest(int pid) {
        IntPtr best = IntPtr.Zero; long area = 0;
        EnumWindows((h, l) => {
            uint p; GetWindowThreadProcessId(h, out p);
            if (p != (uint)pid || !IsWindowVisible(h)) return true;
            RECT r; if (!GetWindowRect(h, out r)) return true;
            long a = (long)(r.Right - r.Left) * (r.Bottom - r.Top);
            if (a > area) { area = a; best = h; }
            return true;
        }, IntPtr.Zero);
        return best;
    }
}
'@

$proc = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $proc) { Write-Output "no window"; exit 1 }
$hwnd = [WinMsg]::FindLargest($proc.Id)
if ($hwnd -eq [IntPtr]::Zero) { Write-Output "no hwnd"; exit 1 }

$WM_LBUTTONDOWN = 0x0201
$WM_LBUTTONUP = 0x0202
$WM_CHAR = 0x0102
$WM_KEYDOWN = 0x0100
$WM_KEYUP = 0x0101
$MK_LBUTTON = 0x0001
$VK_BACK = 0x08

$lParam = [IntPtr](($Y -shl 16) -bor ($X -band 0xFFFF))
if ($Backspace -gt 0) {
    for ($i = 0; $i -lt $Backspace; $i++) {
        [WinMsg]::PostMessage($hwnd, $WM_KEYDOWN, [IntPtr]$VK_BACK, $lParam) | Out-Null
        [WinMsg]::PostMessage($hwnd, $WM_KEYUP, [IntPtr]$VK_BACK, $lParam) | Out-Null
        Start-Sleep -Milliseconds 120
    }
} else {
    [WinMsg]::PostMessage($hwnd, $WM_LBUTTONDOWN, [IntPtr]$MK_LBUTTON, $lParam) | Out-Null
    Start-Sleep -Milliseconds 120
    [WinMsg]::PostMessage($hwnd, $WM_LBUTTONUP, [IntPtr]0, $lParam) | Out-Null
    Start-Sleep -Milliseconds 300

    foreach ($ch in $Text.ToCharArray()) {
        [WinMsg]::PostMessage($hwnd, $WM_CHAR, [IntPtr][int]$ch, $lParam) | Out-Null
        Start-Sleep -Milliseconds 120
    }
}
Write-Output "typed '$Text' into $X,$Y (hwnd=$hwnd)"
