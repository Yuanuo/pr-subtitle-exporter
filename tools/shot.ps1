param(
    [Parameter(Mandatory=$true)][string]$Exe,
    [Parameter(Mandatory=$true)][string]$Out,
    [int]$Wait = 3500,
    [string]$Keys = "",
    [string]$ExeArgs = "",
    [int]$SizeW = 0,
    [int]$SizeH = 0
)

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms

Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win32Shot {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")]
    public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")]
    public static extern bool SetWindowPos(IntPtr hWnd, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll")]
    public static extern bool MoveWindow(IntPtr hWnd, int x, int y, int cx, int cy, bool repaint);
}
"@

[void][Win32Shot]::SetProcessDPIAware()

$proc = Start-Process -FilePath $Exe -ArgumentList $ExeArgs -PassThru
Start-Sleep -Milliseconds $Wait

if ($Keys -ne "") {
    [System.Windows.Forms.SendKeys]::SendWait($Keys)
    Start-Sleep -Milliseconds 1200
}

$proc.Refresh()
$h = $proc.MainWindowHandle
if ($h -eq [IntPtr]::Zero) {
    Write-Output "NO_WINDOW"
    $proc.Kill()
    exit 1
}

[void][Win32Shot]::SetForegroundWindow($h)
if ($SizeW -gt 0 -and $SizeH -gt 0) {
    $r0 = New-Object Win32Shot+RECT
    [void][Win32Shot]::GetWindowRect($h, [ref]$r0)
    [void][Win32Shot]::MoveWindow($h, $r0.Left, $r0.Top, $SizeW, $SizeH, $true)
    Start-Sleep -Milliseconds 1500
    $proc.Refresh()
    $h = $proc.MainWindowHandle
}
Start-Sleep -Milliseconds 600

$r = New-Object Win32Shot+RECT
[void][Win32Shot]::GetWindowRect($h, [ref]$r)
$w = $r.Right - $r.Left
$ht = $r.Bottom - $r.Top

$bmp = New-Object System.Drawing.Bitmap $w, $ht
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($r.Left, $r.Top, 0, 0, $bmp.Size)
$dir = Split-Path -Parent $Out
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir | Out-Null }
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose()
$bmp.Dispose()

$title = $proc.MainWindowTitle
$proc.CloseMainWindow() | Out-Null
Start-Sleep -Milliseconds 800
if (-not $proc.HasExited) { $proc.Kill() }

Write-Output "TITLE=$title"
Write-Output "SIZE=${w}x${ht}"
