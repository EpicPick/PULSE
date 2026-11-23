param(
    [string]$Exe = '.\target\debug\pulse-app.exe',
    [string]$Out = '.\assets\captures'
)
$ErrorActionPreference = 'Stop'
$exePath = (Resolve-Path -LiteralPath $Exe).Path
$directory = New-Item -ItemType Directory -Path $Out -Force

function Capture($Scene, $File, $Size, $Scale) {
    $path = Join-Path $directory.FullName $File
    & $exePath --snap $path --scene $Scene --size $Size --scale $Scale
    if ($LASTEXITCODE -ne 0 -or !(Test-Path -LiteralPath $path) -or (Get-Item -LiteralPath $path).Length -lt 1024) {
        throw "Capture Rune manquante : $path"
    }
}

foreach ($scene in @('overview', 'processes', 'monitor', 'gpu', 'memory', 'diagnostics', 'io', 'history', 'gaming')) {
    Capture $scene "pulse-$scene.png" '1440x900' '1'
}
Capture 'overview' 'pulse-overview-125.png' '1440x900' '1.25'
Capture 'processes' 'pulse-processes-compact-150.png' '1024x640' '1.5'
Capture 'overview' 'pulse-overview-compact.png' '800x640' '1'
