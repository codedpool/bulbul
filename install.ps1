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
# Use the NSIS-specific key explicitly. `windows-x86_64` defaults to MSI in
# Tauri's manifest, but we want the Setup.exe so /PASSIVE flag works directly.
$Platform     = 'windows-x86_64-nsis'
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
    # Tauri's latest.json stores the .sig file content base64-encoded.
    # Decode and write the raw bytes directly. Set-Content on PowerShell 5.1
    # would mangle LF -> CRLF, which breaks minisign's parser.
    $sigBytes = [System.Convert]::FromBase64String($dl.signature)
    [System.IO.File]::WriteAllBytes($sigPath, $sigBytes)

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

    # --- Launch Bulbul -----------------------------------------------------
    # Find the freshly-installed shortcut or exe so the user lands in
    # the onboarding wizard immediately. Falls back silently if nothing
    # is found — Start menu still works.
    $launched = $false
    $startMenus = @(
        "$env:APPDATA\Microsoft\Windows\Start Menu\Programs",
        "$env:PROGRAMDATA\Microsoft\Windows\Start Menu\Programs"
    ) | Where-Object { Test-Path $_ }
    $shortcut = Get-ChildItem -Path $startMenus -Recurse -Filter 'Bulbul.lnk' -ErrorAction SilentlyContinue |
                Select-Object -First 1 -ExpandProperty FullName
    if ($shortcut) {
        Start-Process -FilePath $shortcut
        $launched = $true
    } else {
        $exeCandidates = @(
            "$env:LOCALAPPDATA\Programs\Bulbul\Bulbul.exe",
            "$env:PROGRAMFILES\Bulbul\Bulbul.exe",
            "${env:PROGRAMFILES(X86)}\Bulbul\Bulbul.exe"
        )
        $exe = $exeCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1
        if ($exe) {
            Start-Process -FilePath $exe
            $launched = $true
        }
    }

    Write-Host ''
    Write-Host "  Bulbul $version installed." -ForegroundColor Green
    if ($launched) {
        Write-Host '  Opening Bulbul...' -ForegroundColor Gray
    } else {
        Write-Host '  Look for it in your Start menu.' -ForegroundColor Gray
    }
    Write-Host ''
}
catch {
    $msg = "$($_.Exception.Message)"
    if ($msg -match 'virus or potentially unwanted software' -or
        $msg -match 'Operation did not complete successfully') {
        # Windows Defender heuristics (e.g. Trojan:Win32/Bearfoos.A!ml — an
        # `!ml` machine-learning verdict, not a signature match) sometimes flag
        # Bulbul as a false positive: it uses a global hotkey + keystroke
        # injection to type dictated text into other apps, which on an unsigned
        # binary can look like a keylogger. Replace the raw PowerShell stack
        # trace with a clear, actionable explanation so a blocked user knows
        # it's a false positive and how to recover — not that Bulbul is malware.
        Write-Host ''
        Write-Host '  Windows Defender blocked Bulbul - this is a FALSE POSITIVE.' -ForegroundColor Yellow
        Write-Host '  ----------------------------------------------------------' -ForegroundColor DarkGray
        Write-Host ''
        Write-Host '  Bulbul is not malware. It types your dictated text into other apps' -ForegroundColor Gray
        Write-Host '  (a global hotkey + keystroke injection), and because it is not yet' -ForegroundColor Gray
        Write-Host "  code-signed, Defender's machine-learning heuristic can mistake that" -ForegroundColor Gray
        Write-Host '  for a keylogger. The full source is public and auditable at:' -ForegroundColor Gray
        Write-Host '    https://github.com/codedpool/bulbul' -ForegroundColor Cyan
        Write-Host ''
        Write-Host '  To install it:' -ForegroundColor Gray
        Write-Host '    1. Open Windows Security > Virus & threat protection >' -ForegroundColor Gray
        Write-Host '       Protection history, find the Bulbul entry, and choose' -ForegroundColor Gray
        Write-Host '       Actions > Restore (or "Allow on device").' -ForegroundColor Gray
        Write-Host "    2. Add a folder exclusion for:  $env:LOCALAPPDATA\Bulbul" -ForegroundColor White
        Write-Host '       (Virus & threat protection > Manage settings > Exclusions).' -ForegroundColor Gray
        Write-Host '    3. Re-run this installer.' -ForegroundColor Gray
        Write-Host ''
    } else {
        throw
    }
}
finally {
    Remove-Item -Path $tempDir -Recurse -Force -ErrorAction SilentlyContinue
}
