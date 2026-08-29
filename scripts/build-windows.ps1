$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = Get-Date -Format 'yy.MM.dd.HHmm'
$env:CHATCMD_BUILD_VERSION = $version
$arch = if ([Environment]::Is64BitOperatingSystem) { 'x64' } else { 'x86' }
$output = Join-Path $root "release/ChatCMD-$version-windows-$arch"

Write-Host "Building ChatCMD $version for Windows $arch"

Push-Location (Join-Path $root 'web')
npm ci
npm run build
Pop-Location

cargo build --release --features embedded-web

if (Test-Path $output) { Remove-Item $output -Recurse -Force }
New-Item -ItemType Directory -Force -Path $output | Out-Null
Copy-Item (Join-Path $root 'target/release/chat-cmd-client.exe') (Join-Path $output 'ChatCMD.exe')

$zip = "$output.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path "$output/*" -DestinationPath $zip -CompressionLevel Optimal

Write-Host "Build completed: $output"
Write-Host "Archive: $zip"
