# Video Analyzer Native Launcher
# Chrome Native Messaging protocol: 4-byte length + JSON on stdin/stdout

$ErrorActionPreference = 'Stop'
$exePath = 'C:\Users\29799\Desktop\image-scheduler-rs\image-scheduler-rs.exe'

$stdin  = [Console]::OpenStandardInput()
$stdout = [Console]::OpenStandardOutput()

function Write-Response($obj) {
    $json = $obj | ConvertTo-Json -Compress
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
    $len = [BitConverter]::GetBytes([uint32]$bytes.Length)
    $stdout.Write($len, 0, 4)
    $stdout.Write($bytes, 0, $bytes.Length)
    $stdout.Flush()
}

while ($true) {
    $lenBuf = New-Object byte[] 4
    $read = $stdin.Read($lenBuf, 0, 4)
    if ($read -eq 0) { break }
    $msgLen = [BitConverter]::ToUInt32($lenBuf, 0)
    $msgBuf = New-Object byte[] $msgLen
    $offset = 0
    while ($offset -lt $msgLen) {
        $n = $stdin.Read($msgBuf, $offset, $msgLen - $offset)
        $offset += $n
    }
    $msg = [System.Text.Encoding]::UTF8.GetString($msgBuf)
    $data = $msg | ConvertFrom-Json

    $action = $data.action

    if ($action -eq 'start') {
        $existing = Get-Process -Name 'image-scheduler-rs' -ErrorAction SilentlyContinue
        if ($existing) {
            Write-Response @{status='already_running'; pid=$existing.Id}
        } else {
            try {
                $proc = Start-Process -FilePath $exePath -WindowStyle Hidden -PassThru
                Write-Response @{status='started'; pid=$proc.Id}
            } catch {
                Write-Response @{status='error'; message=$_.Exception.Message}
            }
        }
    }
    elseif ($action -eq 'stop') {
        $procs = Get-Process -Name 'image-scheduler-rs' -ErrorAction SilentlyContinue
        if ($procs) {
            $procs | Stop-Process -Force
            Write-Response @{status='stopped'}
        } else {
            Write-Response @{status='not_running'}
        }
    }
    elseif ($action -eq 'status') {
        $existing = Get-Process -Name 'image-scheduler-rs' -ErrorAction SilentlyContinue
        if ($existing) {
            # also check if port is listening
            $port = Get-NetTCPConnection -LocalPort 5000 -ErrorAction SilentlyContinue | Where-Object State -eq 'Listen'
            Write-Response @{status='running'; pid=$existing.Id; port_ready=($port -ne $null)}
        } else {
            Write-Response @{status='not_running'}
        }
    }
}
