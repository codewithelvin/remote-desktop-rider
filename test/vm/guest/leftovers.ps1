"service:   " + [bool](Get-Service RemoteDesktopRider -ErrorAction SilentlyContinue)
"processes: " + @(Get-Process rider-host -ErrorAction SilentlyContinue).Count
"binary:    " + (Test-Path "C:\Program Files\RemoteDesktopRider")
"firewall:  " + [bool](Get-NetFirewallRule -DisplayName "Remote Desktop Rider (UDP)" -ErrorAction SilentlyContinue)
"sas policy:" + [bool](Get-ItemProperty HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System -Name SoftwareSASGeneration -ErrorAction SilentlyContinue)
"state dir: " + (Test-Path C:\ProgramData\RemoteDesktopRider\host\host.toml)
