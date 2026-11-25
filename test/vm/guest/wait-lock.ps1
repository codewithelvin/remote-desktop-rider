# Test VM only: wait (up to 60 s) until the console session really is locked
# ("on") or unlocked ("off"): in a slow VM the lock screen can take a while.
# Reads the same session flag as the host. C:\rider-test\wait-lock.out:
# "on 4" / "off 2" (seconds it took) or "timeout".
# -Define: only load [Wts.Api]::Locked() (dot-sourced by unlock.ps1).
param([ValidateSet("on", "off")][string]$State = "on", [switch]$Define)
# Once per process: the tests' agent runs every script in the same one.
if (-not ("Wts.Api" -as [type])) { Add-Type -Namespace Wts -Name Api -MemberDefinition @"
[DllImport("kernel32.dll")] public static extern uint WTSGetActiveConsoleSessionId();
[DllImport("wtsapi32.dll")] static extern bool WTSQuerySessionInformationW(IntPtr s, uint id, int cls, out IntPtr buf, out uint len);
[DllImport("wtsapi32.dll")] static extern void WTSFreeMemory(IntPtr p);
// WTSINFOEXW: Level, then (8-aligned: it holds LARGE_INTEGERs) WTSINFOEX_LEVEL1_W { SessionId, SessionState, SessionFlags, ... }.
public static bool Locked() {
    IntPtr buf; uint len;
    if (!WTSQuerySessionInformationW(IntPtr.Zero, WTSGetActiveConsoleSessionId(), 25, out buf, out len)) return false;
    try { return Marshal.ReadInt32(buf, 0) == 1 && Marshal.ReadInt32(buf, 16) == 0; }   // SessionFlags: WTS_SESSIONSTATE_LOCK = 0
    finally { WTSFreeMemory(buf); }
}
"@ }
if ($Define) { return }
$want = $State -eq "on"
$t0 = Get-Date
while ([Wts.Api]::Locked() -ne $want) {
    if (((Get-Date) - $t0).TotalSeconds -gt 60) { Set-Content C:\rider-test\wait-lock.out "timeout"; exit 0 }
    Start-Sleep -Milliseconds 250
}
Set-Content C:\rider-test\wait-lock.out "$State $([math]::Round(((Get-Date) - $t0).TotalSeconds))"
