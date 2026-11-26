param([int]$Port, [string]$Token)
$l = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, $Port)
$l.Start()
while ($true) {
    $c = $l.AcceptTcpClient()
    $w = [System.IO.StreamWriter]::new($c.GetStream())
    $w.Write("$Token`n")
    $w.Flush()
    $c.Close()
}
