param(
    [string]$Version = $env:CHATCMD_BUILD_VERSION
)

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = if ([string]::IsNullOrWhiteSpace($Version)) { Get-Date -Format 'yy.MM.dd.HHmm' } else { $Version.Trim() }
if ($version -notmatch '^[0-9A-Za-z][0-9A-Za-z._+\-]{0,79}$') {
    throw "Invalid ChatCMD version: $version"
}
$env:CHATCMD_BUILD_VERSION = $version
$extensionSource = Join-Path $root 'chatgpt-extension'

$targets = @(
    @{ RustTarget = 'x86_64-pc-windows-msvc'; Label = '64' },
    @{ RustTarget = 'i686-pc-windows-msvc'; Label = '32' }
)

Write-Host "Building ChatCMD $version for Windows 64-bit + 32-bit"

if (-not (Test-Path $extensionSource)) {
    throw "ChatGPT extension folder not found: $extensionSource"
}

Push-Location (Join-Path $root 'web')
try {
    npm ci
    npm run build
    npm run obfuscate -- dist
    $sourceMaps = Get-ChildItem -Path (Join-Path $root 'web/dist') -Recurse -File -Filter '*.map'
    if ($sourceMaps) {
        throw "Source map files were generated in web/dist: $($sourceMaps.FullName -join ', ')"
    }
}
finally {
    Pop-Location
}

if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
    throw 'rustup is required to install both Windows Rust targets'
}

rustup target add x86_64-pc-windows-msvc i686-pc-windows-msvc
if ($LASTEXITCODE -ne 0) {
    throw 'Failed to install required Windows Rust targets'
}

foreach ($entry in $targets) {
    $target = $entry.RustTarget
    $label = $entry.Label
    $output = Join-Path $root "release/${version}_${label}"
    $binary = Join-Path $root "target/$target/release/chat-cmd-client.exe"
    $extensionOutput = Join-Path $output 'chatgpt-extension'

    Write-Host "`nBuilding Rust target $target (${label}-bit)..."
    cargo build --release --features embedded-web --target $target
    if ($LASTEXITCODE -ne 0) {
        throw "Cargo build failed for target $target"
    }

    if (Test-Path $output) { Remove-Item $output -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $output | Out-Null

    Copy-Item $binary (Join-Path $output 'ChatCMD.exe')
    Copy-Item $extensionSource $extensionOutput -Recurse -Force

    Push-Location (Join-Path $root 'web')
    try {
        npm run obfuscate -- $extensionOutput
    }
    finally {
        Pop-Location
    }

    $zip = "$output.zip"
    if (Test-Path $zip) { Remove-Item $zip -Force }
    Compress-Archive -Path "$output/*" -DestinationPath $zip -CompressionLevel Optimal

    Write-Host "Build completed: $output"
    Write-Host "Archive: $zip"
}

Write-Host "`nAll Windows builds completed."
