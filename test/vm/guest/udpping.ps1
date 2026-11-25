param([string]$Server = "192.168.56.10", [int]$Port = 47950, [int]$Size = 1200, [int]$Count = 30)
$u = New-Object System.Net.Sockets.UdpClient; $u.Client.ReceiveTimeout = 1000
$u.Connect($Server, $Port); $buf = New-Object byte[] $Size; $r = @()
$ep = New-Object System.Net.IPEndPoint ([System.Net.IPAddress]::Any), 0
for ($i = 0; $i -lt $Count; $i++) {
  $sw = [Diagnostics.Stopwatch]::StartNew(); [void]$u.Send($buf, $buf.Length)
  try { [void]$u.Receive([ref]$ep); $r += $sw.Elapsed.TotalMilliseconds } catch {}
  Start-Sleep -Milliseconds 5
}
$r = $r | Sort-Object
"{0}B: {1}/{5} replies, median {2:N2} ms, p95 {3:N2} ms, max {4:N2} ms" -f $Size, $r.Count, $r[[int]($r.Count/2)], $r[[int]($r.Count*0.95)], $r[-1], $Count
