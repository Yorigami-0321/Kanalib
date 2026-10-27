param(
    [string]$ProcessName = "kanalib",
    [int]$X = 1100,
    [int]$Y = 600,
    [int]$Notches = 3
)

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public class Wheel {
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
$hwnd = [Wheel]::FindLargest($proc.Id)
if ($hwnd -eq [IntPtr]::Zero) { Write-Output "no hwnd"; exit 1 }

$WM_MOUSEWHEEL = 0x020A
$lParam = [IntPtr](($Y -shl 16) -bor ($X -band 0xFFFF))
for ($i = 0; $i -lt $Notches; $i++) {
    $wParam = [IntPtr]0xFF880000  # delta = -120
    [Wheel]::PostMessage($hwnd, $WM_MOUSEWHEEL, $wParam, $lParam) | Out-Null
    Start-Sleep -Milliseconds 200
}
Write-Output "wheel sent: notches=$Notches at $X,$Y hwnd=$hwnd"
