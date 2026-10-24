param(
    [string]$ProcessName = "kanalib",
    [string]$OutFile = "D:\Code\Kanalib\.kanalib\window.png"
)

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win32Capture {
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumProc callback, IntPtr lParam);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
    [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint nFlags);

    private delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }

    public static IntPtr FindLargestVisibleWindow(int processId) {
        IntPtr best = IntPtr.Zero;
        long bestArea = 0;
        EnumWindows((hWnd, lParam) => {
            uint pid;
            GetWindowThreadProcessId(hWnd, out pid);
            if (pid != (uint)processId || !IsWindowVisible(hWnd)) return true;
            RECT rect;
            if (!GetWindowRect(hWnd, out rect)) return true;
            long area = (long)(rect.Right - rect.Left) * (rect.Bottom - rect.Top);
            if (area > bestArea) {
                bestArea = area;
                best = hWnd;
            }
            return true;
        }, IntPtr.Zero);
        return best;
    }
}
"@

$processes = @(Get-Process -Name $ProcessName -ErrorAction Stop)
$hwnd = [IntPtr]::Zero
$bestArea = 0
foreach ($process in $processes) {
    $candidate = [Win32Capture]::FindLargestVisibleWindow($process.Id)
    if ($candidate -eq [IntPtr]::Zero) { continue }
    $rect = New-Object Win32Capture+RECT
    if (-not [Win32Capture]::GetWindowRect($candidate, [ref]$rect)) { continue }
    $area = ($rect.Right - $rect.Left) * ($rect.Bottom - $rect.Top)
    if ($area -gt $bestArea) {
        $bestArea = $area
        $hwnd = $candidate
    }
}
if ($hwnd -eq [IntPtr]::Zero) { throw "no visible $ProcessName window" }

$rect = New-Object Win32Capture+RECT
if (-not [Win32Capture]::GetWindowRect($hwnd, [ref]$rect)) { throw "GetWindowRect failed" }
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top
if ($width -le 0 -or $height -le 0) { throw "bad window size $width x $height" }

$bitmap = New-Object System.Drawing.Bitmap($width, $height)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$hdc = $graphics.GetHdc()
$ok = [Win32Capture]::PrintWindow($hwnd, $hdc, 2)
$graphics.ReleaseHdc($hdc)
$graphics.Dispose()

$dir = Split-Path -Parent $OutFile
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$bitmap.Save($OutFile, [System.Drawing.Imaging.ImageFormat]::Png)
$bitmap.Dispose()
Write-Output "printwindow=$ok size=${width}x${height} hwnd=$hwnd file=$OutFile"
