param([switch]$Restore)
$out = "C:\rider-test\monitor-left.out"
Add-Type -TypeDefinition @"
using System; using System.Runtime.InteropServices;
public static class Disp {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct DEVMODE {
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmDeviceName;
        public short dmSpecVersion, dmDriverVersion, dmSize, dmDriverExtra; public int dmFields;
        public int dmPositionX, dmPositionY; public int dmDisplayOrientation, dmDisplayFixedOutput;
        public short dmColor, dmDuplex, dmYResolution, dmTTOption, dmCollate;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmFormName;
        public short dmLogPixels; public int dmBitsPerPel, dmPelsWidth, dmPelsHeight, dmDisplayFlags, dmDisplayFrequency;
        public int dmICMMethod, dmICMIntent, dmMediaType, dmDitherType, dmReserved1, dmReserved2, dmPanningWidth, dmPanningHeight;
    }
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern bool EnumDisplaySettings(string dev, int mode, ref DEVMODE dm);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int ChangeDisplaySettingsEx(string dev, ref DEVMODE dm, IntPtr hwnd, int flags, IntPtr param);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int ChangeDisplaySettingsEx(string dev, IntPtr dm, IntPtr hwnd, int flags, IntPtr param);
}
"@
$second = "\\.\DISPLAY2"
$dm = New-Object Disp+DEVMODE
$dm.dmSize = [Runtime.InteropServices.Marshal]::SizeOf($dm)
if (-not [Disp]::EnumDisplaySettings($second, -1, [ref]$dm)) { Set-Content $out "no second display"; exit 1 }
$saved = "C:\rider-test\monitor-left.orig"
if (-not (Test-Path $saved)) { Set-Content $saved "$($dm.dmPositionX) $($dm.dmPositionY)" }
$origX, $origY = (Get-Content $saved -Raw).Trim() -split ' '
$dm.dmFields = 0x20
$dm.dmPositionX = $(if ($Restore) { [int]$origX } else { -[int]$dm.dmPelsWidth })
$dm.dmPositionY = $(if ($Restore) { [int]$origY } else { 0 })
$set = [Disp]::ChangeDisplaySettingsEx($second, [ref]$dm, [IntPtr]::Zero, 1, [IntPtr]::Zero)
Start-Sleep 2
if ($Restore) { Remove-Item $saved }
Set-Content $out ("set=$set x=$($dm.dmPositionX)")
