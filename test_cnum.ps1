param([string]$PortName = 'COM1')

function Send-AT {
    param($Port, [string]$Cmd)
    $Port.DiscardInBuffer()
    $Port.Write($Cmd + "`r`n")
    Start-Sleep -Milliseconds 1500
    if ($Port.BytesToRead -gt 0) {
        return $Port.ReadExisting()
    }
    return "(no response)"
}

Write-Host "=== Testing $PortName ===" -ForegroundColor Cyan
$port = New-Object System.IO.Ports.SerialPort $PortName, 115200, 'None', 8, 'One'
$port.ReadTimeout  = 3000
$port.WriteTimeout = 2000
$port.DtrEnable    = $true
$port.RtsEnable    = $false
$port.Open()
Start-Sleep -Milliseconds 300

Send-AT $port "ATE0" | Out-Null
Start-Sleep -Milliseconds 500

$r1 = Send-AT $port 'AT$QCPBMPREF=1'
Write-Host "AT`$QCPBMPREF=1 => [$($r1.Trim() -replace "`r`n",' | ')]"

$r2 = Send-AT $port 'AT+CNUM'
Write-Host "AT+CNUM       => [$($r2.Trim() -replace "`r`n",' | ')]"

$port.Close()
Write-Host "Done."
