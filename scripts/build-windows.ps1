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
    $packageJson = Join-Path $root 'web/package.json'
    $packageLock = Join-Path $root 'web/package-lock.json'
    $installMarker = Join-Path $root 'web/node_modules/.package-lock.json'
    $needsInstall = -not (Test-Path $installMarker)
    if (-not $needsInstall) {
        $markerTime = (Get-Item $installMarker).LastWriteTimeUtc
        $needsInstall = ((Get-Item $packageJson).LastWriteTimeUtc -gt $markerTime) -or ((Get-Item $packageLock).LastWriteTimeUtc -gt $markerTime)
    }

    if ($needsInstall) {
        npm ci --prefer-offline --no-audit --no-fund
        if ($LASTEXITCODE -ne 0) {
            throw 'Failed to install web dependencies'
        }
    }
    else {
        Write-Host 'Web dependencies unchanged; skipping npm ci.'
    }

    npm run build
    if ($LASTEXITCODE -ne 0) {
        throw 'Web build failed'
    }

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

$installedTargets = @(rustup target list --installed)
$missingTargets = @($targets.RustTarget | Where-Object { $_ -notin $installedTargets })
if ($missingTargets.Count -gt 0) {
    rustup target add @missingTargets
    if ($LASTEXITCODE -ne 0) {
        throw 'Failed to install required Windows Rust targets'
    }
}
else {
    Write-Host 'Windows Rust targets already installed; skipping rustup target add.'
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

    $zip = "$output.zip"
    if (Test-Path $zip) { Remove-Item $zip -Force }
    Compress-Archive -Path "$output/*" -DestinationPath $zip -CompressionLevel NoCompression

    Write-Host "Build completed: $output"
    Write-Host "Archive: $zip"
}

Write-Host "`nAll Windows builds completed."
