param([string]$Exe = '.\target\release\pulse-app.exe')
$ErrorActionPreference = 'Stop'
$path = (Resolve-Path -LiteralPath $Exe).Path
$pe = [IO.File]::ReadAllBytes($path)
$offset = [BitConverter]::ToInt32($pe, 60)
$subsystem = [BitConverter]::ToUInt16($pe, $offset + 24 + 68)
if ($subsystem -ne 2) { throw "Expected a Windows GUI executable, got subsystem $subsystem" }

function Wait-PulseWindow($Process) {
    for ($i = 0; $i -lt 80; $i++) {
        $Process.Refresh()
        if ($Process.HasExited) { throw "PULSE exited: $($Process.ExitCode)" }
        if ($Process.MainWindowHandle -ne 0 -and $Process.MainWindowTitle -eq 'PULSE') { return }
        Start-Sleep -Milliseconds 250
    }
    throw 'Native PULSE window not found'
}

function Close-Pulse($Process) {
    $Process.Refresh()
    if (!$Process.HasExited) {
        if (!$Process.CloseMainWindow() -or !$Process.WaitForExit(10000)) {
            throw 'PULSE did not close normally'
        }
        if ($Process.ExitCode -ne 0) { throw "PULSE exit code: $($Process.ExitCode)" }
    }
}

$scratch = New-Item -ItemType Directory -Path (Join-Path ([System.IO.Path]::GetTempPath()) ('pulse-smoke-' + [guid]::NewGuid()))
$db = Join-Path $scratch.FullName 'capture.sqlite'
try {
    $default = Start-Process -FilePath $path -PassThru
    try { Wait-PulseWindow $default } finally { Close-Pulse $default }
    $live = Start-Process -FilePath $path -ArgumentList "--record `"$db`" --retain 4" -PassThru
    try {
        Wait-PulseWindow $live
        Start-Sleep -Seconds 3
        $live.Refresh()
        $cpu = $live.TotalProcessorTime.TotalSeconds
        $clock = [Diagnostics.Stopwatch]::StartNew()
        Start-Sleep -Seconds 8
        $live.Refresh()
        [pscustomobject]@{
            Window = $live.MainWindowTitle
            CPU_One_Core_Pct = [math]::Round(100 * ($live.TotalProcessorTime.TotalSeconds - $cpu) / $clock.Elapsed.TotalSeconds, 2)
            Private_MiB = [math]::Round($live.PrivateMemorySize64 / 1MB, 1)
            Working_Set_MiB = [math]::Round($live.WorkingSet64 / 1MB, 1)
        } | Format-List | Out-Host
    } finally { Close-Pulse $live }
    cargo run --quiet --locked -p pulse-history --example check_smoke -- $db
    if ($LASTEXITCODE -ne 0) { throw 'SQLite check failed' }
    $replay = Start-Process -FilePath $path -ArgumentList "--replay `"$db`"" -PassThru
    try { Wait-PulseWindow $replay } finally { Close-Pulse $replay }
} finally {
    Remove-Item -LiteralPath $scratch.FullName -Recurse -Force
}
