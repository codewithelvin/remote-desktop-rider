# Test VM only, as SYSTEM: what `rider-host info` says (host-info.out),
# e.g. the "Place:" line an admin would see for this computer.
& "C:\Program Files\RemoteDesktopRider\rider-host.exe" info *>&1 | Out-File C:\rider-test\host-info.out -Encoding UTF8
