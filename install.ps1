# Bulbul installer
# Usage:  irm https://bulbultypes.xyz/install.ps1 | iex
#
# Downloads the latest Bulbul release from GitHub, verifies its minisign
# signature against the public key baked in below, and installs passively.
# No clicks required from the user. Bulbul's own onboarding wizard runs the
# first time the app launches.

$ErrorActionPreference = 'Stop'

# --- Config ----------------------------------------------------------------
$ManifestUrl  = 'https://github.com/codedpool/bulbul/releases/latest/download/latest.json'
$Platform     = 'windows-x86_64'
$MinisignKey  = 'RWTLvdvsrlMNS4LQvsKO03T8kF+5jZ1s7KiyU4lKZmYPcd0+1qxm2gKt'
$MinisignUrl  = 'https://github.com/jedisct1/minisign/releases/download/0.12/minisign-0.12-win64.zip'

# --- Pre-flight ------------------------------------------------------------
if (-not [Environment]::Is64BitOperatingSystem) {
    throw 'Bulbul requires 64-bit Windows.'
}

Write-Host ''
Write-Host '  Bulbul installer' -ForegroundColor Cyan
Write-Host '  ----------------' -ForegroundColor DarkGray
Write-Host ''

# --- Fetch updater manifest ------------------------------------------------
Write-Host '  > Fetching latest release info...' -ForegroundColor Gray
$manifest = Invoke-RestMethod -Uri $ManifestUrl
$version  = $manifest.version
$dl       = $manifest.platforms.$Platform
if (-not $dl) {
    throw "No build found for platform '$Platform' in the release manifest."
}
Write-Host "    Bulbul $version" -ForegroundColor Green

# --- Workspace -------------------------------------------------------------
$tempDir = Join-Path $env:TEMP "bulbul-install-$([guid]::NewGuid().Guid)"
New-Item -ItemType Directory -Path $tempDir | Out-Null

try {
    $setupPath = Join-Path $tempDir 'BulbulSetup.exe'
    $sigPath   = Join-Path $tempDir 'BulbulSetup.exe.minisig'

    # --- Download installer + signature ------------------------------------
    Write-Host '  > Downloading installer...' -ForegroundColor Gray
    Invoke-WebRequest -Uri $dl.url -OutFile $setupPath -UseBasicParsing
    Set-Content -Path $sigPath -Value $dl.signature -Encoding ASCII -NoNewline

    # --- Pull minisign for verification ------------------------------------
    Write-Host '  > Verifying signature...' -ForegroundColor Gray
    $msZip = Join-Path $tempDir 'minisign.zip'
    $msDir = Join-Path $tempDir 'minisign'
    Invoke-WebRequest -Uri $MinisignUrl -OutFile $msZip -UseBasicParsing
    Expand-Archive -Path $msZip -DestinationPath $msDir -Force
    # Pick the x86_64 build explicitly — the zip also contains an aarch64 build
    # which would land first alphabetically and fail on x64 Windows.
    $msExe = Get-ChildItem -Path $msDir -Filter 'minisign.exe' -Recurse |
             Where-Object { $_.FullName -match '[\\/]x86_64[\\/]' } |
             Select-Object -First 1 -ExpandProperty FullName
    if (-not $msExe) { throw 'minisign.exe (x86_64) not found in download.' }

    # --- Verify ------------------------------------------------------------
    $verify = & $msExe -V -P $MinisignKey -m $setupPath -x $sigPath 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Host $verify -ForegroundColor Red
        throw 'Signature verification failed. Aborting installation.'
    }
    Write-Host '    Signature verified.' -ForegroundColor Green

    # --- Install passively -------------------------------------------------
    Write-Host '  > Installing...' -ForegroundColor Gray
    $proc = Start-Process -FilePath $setupPath -ArgumentList '/PASSIVE' -PassThru -Wait
    if ($proc.ExitCode -ne 0) {
        throw "Installer exited with code $($proc.ExitCode)."
    }

    Write-Host ''
    Write-Host "  Bulbul $version installed." -ForegroundColor Green
    Write-Host '  Look for it in your Start menu.' -ForegroundColor Gray
    Write-Host ''
}
finally {
    Remove-Item -Path $tempDir -Recurse -Force -ErrorAction SilentlyContinue
}
