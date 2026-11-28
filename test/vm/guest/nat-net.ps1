param([Parameter(Mandatory)][ValidateSet("on", "off")][string]$Mode, [string]$Mac = "")
$log = "C:\rider-test\nat-net.out"
$saved = "C:\rider-test\nat-net.saved"
$inside = "10.77.0.2"
$router = "10.77.0.1"
$outside = "192.168.56.0/24"

function Save-Config([int]$index) {
    $dhcp = (Get-NetIPInterface -InterfaceIndex $index -AddressFamily IPv4).Dhcp
    $addresses = @(Get-NetIPAddress -InterfaceIndex $index -AddressFamily IPv4 -PrefixOrigin Manual -ErrorAction SilentlyContinue |
        ForEach-Object { "$($_.IPAddress)/$($_.PrefixLength)" })
    @{ Dhcp = "$dhcp"; Addresses = $addresses } | ConvertTo-Json | Set-Content $saved
}

function Clear-Config([int]$index) {
    Get-NetRoute -InterfaceIndex $index -DestinationPrefix $outside -ErrorAction SilentlyContinue |
        Where-Object NextHop -eq $router | Remove-NetRoute -Confirm:$false
    Set-NetIPInterface -InterfaceIndex $index -Dhcp Disabled
    Get-NetIPAddress -InterfaceIndex $index -AddressFamily IPv4 -ErrorAction SilentlyContinue | Remove-NetIPAddress -Confirm:$false
}

function Restore-Config([int]$index) {
    $config = Get-Content $saved -Raw | ConvertFrom-Json
    foreach ($a in @($config.Addresses)) {
        $ip, $prefix = $a -split '/'
        New-NetIPAddress -InterfaceIndex $index -IPAddress $ip -PrefixLength ([int]$prefix) | Out-Null
    }
    if ($config.Dhcp -eq "Enabled") { Set-NetIPInterface -InterfaceIndex $index -Dhcp Enabled }
    Remove-Item $saved
}

try {
    $adapter = Get-NetAdapter | Where-Object { ($_.MacAddress -replace '-', '') -eq $Mac.ToUpper() }
    if (-not $adapter) { throw "no adapter with MAC $Mac" }
    $index = $adapter.ifIndex
    if ($Mode -eq "on") {
        if (-not (Test-Path $saved)) { Save-Config $index }
        Clear-Config $index
        New-NetIPAddress -InterfaceIndex $index -IPAddress $inside -PrefixLength 24 | Out-Null
        New-NetRoute -InterfaceIndex $index -DestinationPrefix $outside -NextHop $router | Out-Null
        $deadline = (Get-Date).AddSeconds(20)
        while (-not (Test-Connection $router -Count 1 -Quiet) -and (Get-Date) -lt $deadline) { Start-Sleep 1 }
        if (-not (Test-Connection $router -Count 1 -Quiet)) { throw "the router $router does not answer" }
    } elseif (Test-Path $saved) {
        Clear-Config $index
        Restore-Config $index
    }
    Restart-Service RemoteDesktopRider
    "$Mode ok" | Set-Content $log
} catch {
    "$Mode error: $_" | Set-Content $log
}
