# Kill existing process on port 8090 before starting
$p = Get-NetTCPConnection -LocalPort 8090 -State Listen -ErrorAction SilentlyContinue
if ($p) {
    Write-Host "Stopping existing backend (PID $($p.OwningProcess))..."
    Stop-Process -Id $p.OwningProcess -Force
    Start-Sleep -Seconds 2
}
Write-Host "Starting backend on 0.0.0.0:8090..."
.\target\debug\backend.exe
