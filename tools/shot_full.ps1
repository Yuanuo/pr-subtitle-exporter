param(
    [Parameter(Mandatory=$true)][string]$Exe,
    [Parameter(Mandatory=$true)][string]$Out,
    [int]$Wait = 3500,
    [int]$Width = 0,
    [int]$Height = 0
)

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms

Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win32Cap {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hWnd);
}
"@

$proc = Start-Process -FilePath $Exe -PassThru
Start-Sleep -Milliseconds $Wait
$proc.Refresh()
[void][Win32Cap]::SetForegroundWindow($proc.MainWindowHandle)
Start-Sleep -Milliseconds 600

$bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$w = $bounds.Width
$h = $bounds.Height

$bmp = New-Object System.Drawing.Bitmap $w, $h
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($bounds.X, $bounds.Y, 0, 0, $bmp.Size)
$dir = Split-Path -Parent $Out
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir | Out-Null }
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose()
$bmp.Dispose()

$r = New-Object Win32Cap+RECT
[void][Win32Cap]::GetWindowRect($proc.MainWindowHandle, [ref]$r)

Write-Output "SCREEN=${w}x${h}"
Write-Output "WINDOW=$($r.Left),$($r.Top)-$($r.Right),$($r.Bottom)"
Write-Output "TITLE=$($proc.MainWindowTitle)"

$proc.CloseMainWindow() | Out-Null
Start-Sleep -Milliseconds 800
if (-not $proc.HasExited) { $proc.Kill() }
