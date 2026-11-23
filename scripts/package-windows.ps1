param([switch]$NoSmoke)
$ErrorActionPreference = 'Stop'
$version = '2.3.1'
$sha = '364e5d98d4d134bd54dd25c22ed2ca2f4883f8bc3ed6502bee0c151e3436d30c'
$vendor = New-Item -ItemType Directory -Path '.\target\third-party' -Force
$presentMon = Join-Path $vendor.FullName "PresentMon-$version-x64.exe"
if (!(Test-Path -LiteralPath $presentMon) -or (Get-FileHash -LiteralPath $presentMon -Algorithm SHA256).Hash -ne $sha) {
    $download = Join-Path $vendor.FullName ('PresentMon-' + [guid]::NewGuid() + '.download')
    try {
        Invoke-WebRequest -Uri "https://github.com/GameTechDev/PresentMon/releases/download/v$version/PresentMon-$version-x64.exe" -OutFile $download
        if ((Get-FileHash -LiteralPath $download -Algorithm SHA256).Hash -ne $sha) { throw 'Invalid SHA-256 for official PresentMon.' }
        Move-Item -LiteralPath $download -Destination $presentMon -Force
    } finally {
        if (Test-Path -LiteralPath $download) { Remove-Item -LiteralPath $download }
    }
}
$previousBundle = $env:PULSE_PRESENTMON_BUNDLE
try {
    $env:PULSE_PRESENTMON_BUNDLE = $presentMon
    cargo build --locked --release -p pulse-app
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally {
    $env:PULSE_PRESENTMON_BUNDLE = $previousBundle
}
$source = (Resolve-Path '.\target\release\pulse-app.exe').Path
if (!$NoSmoke) {
    & "$PSScriptRoot\smoke-windows.ps1" -Exe $source
}
$out = New-Item -ItemType Directory -Path '.\artifacts' -Force
$archive = Join-Path $out.FullName 'PULSE-windows-x64.zip'
$stage = New-Item -ItemType Directory -Path (Join-Path ([IO.Path]::GetTempPath()) ('pulse-package-' + [guid]::NewGuid()))
$stagedExe = Join-Path $stage.FullName 'PULSE.exe'
$stagedZip = Join-Path $stage.FullName 'PULSE-windows-x64.zip'
try {
    Copy-Item -LiteralPath $source -Destination $stagedExe
    Compress-Archive -LiteralPath $stagedExe, '.\README.md', '.\LICENSE', '.\third-party\PresentMon-LICENSE.txt' -DestinationPath $stagedZip
    Copy-Item -LiteralPath $stagedZip -Destination $archive -Force
    Copy-Item -LiteralPath $stagedExe -Destination (Join-Path $out.FullName 'PULSE.exe') -Force
    $hash = Get-FileHash -LiteralPath $archive -Algorithm SHA256
    "$($hash.Hash.ToLowerInvariant())  PULSE-windows-x64.zip" | Set-Content -LiteralPath "$archive.sha256" -Encoding Ascii
    try {
        Copy-Item -LiteralPath $stagedExe -Destination '.\PULSE.exe' -Force
        Write-Host 'PULSE.exe updated in the workspace.'
    } catch {
        Write-Warning 'Could not replace PULSE.exe. The updated executable is in artifacts\PULSE.exe.'
    }
} finally {
    Remove-Item -LiteralPath $stage.FullName -Recurse -Force
}
