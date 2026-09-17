<#
.SYNOPSIS
    Builds, signs, verifies, and collects release artifacts into deploy/.

.DESCRIPTION
    One command to go from a checkout to distributable files. The steps it
    automates are the ones that are easy to get wrong by hand:

      1. ANDROID SIGNING ORDER. zipalign must run BEFORE apksigner. Signing a
         misaligned APK succeeds and then fails at install with nothing but
         "App not installed" on the device, which is indistinguishable from not
         having signed it at all.

      2. VERIFICATION. An unsigned or wrongly-signed APK is rejected by the
         package manager before it looks at the code, so the failure surfaces
         on the phone rather than in the build. This script runs
         `apksigner verify` and refuses to publish an artifact that fails.

      3. STALE ARTIFACTS. `tauri android build && adb install` installs the
         PREVIOUS apk if the build failed, because only the last exit code
         propagates. Every build step here is exit-code checked before its
         output is collected.

      4. Windows code signing is optional and off unless a certificate is
         configured, because the project has no publisher certificate. An
         unsigned installer works; it just shows a SmartScreen warning.

    Nothing is ever signed with the debug key unless -AllowDebugKey is passed,
    and artifacts signed that way are named "-debugkey" so they cannot be
    mistaken for something distributable.

    NOTE: keep this file pure ASCII. It is invoked via `powershell` (Windows
    PowerShell 5.1), which reads BOM-less files as ANSI, so a stray em dash
    becomes mojibake and produces a cascade of bogus parse errors.

.PARAMETER Target
    Which platform to package: android, windows, or all (default).

.PARAMETER SkipBuild
    Sign and collect whatever is already in the build output. Useful after a
    long release build, or to re-sign without a ~15 minute rebuild.

.PARAMETER AllowDebugKey
    Sign the Android release with ~/.android/debug.keystore when no release
    keystore is configured. For sideloading only: the debug key is shared by
    every Android SDK install, so builds signed with it cannot be distributed
    and cannot be upgraded by a properly signed build later.

.PARAMETER DeployDir
    Output directory, relative to the repo root. Defaults to "deploy".

.EXAMPLE
    pnpm deploy
    Builds and packages both platforms into deploy/.

.EXAMPLE
    pnpm deploy:resign
    Re-signs and re-collects the existing build output, with no rebuild.

.EXAMPLE
    pnpm deploy:android:debugkey
    Sideload-only Android build signed with the shared SDK debug key.

.EXAMPLE
    pnpm deploy:windows
    Builds the .msi and NSIS .exe, signing them if a certificate is configured.

.NOTES
    Windows code signing reads these environment variables:

      LOCALDROP_WIN_CERT             path to a .pfx / .p12 file
      LOCALDROP_WIN_CERT_PASSWORD    its password
      LOCALDROP_WIN_CERT_THUMBPRINT  alternative: SHA-1 thumbprint of a
                                     certificate already in the user's store
                                     (use this for a hardware token or HSM)
      LOCALDROP_WIN_TIMESTAMP_URL    RFC 3161 timestamp server; defaults to
                                     http://timestamp.digicert.com

    A timestamp matters more than it looks: without one, every signature stops
    validating the day the certificate expires, including on copies already
    downloaded.
#>

[CmdletBinding()]
param(
    [ValidateSet('all', 'android', 'windows')]
    [string] $Target = 'all',

    [switch] $SkipBuild,

    [switch] $AllowDebugKey,

    [string] $DeployDir = 'deploy'
)

# NOTE ON INVOCATION: do not use `pnpm deploy:android -- -AllowDebugKey`.
#
# pnpm forwards the `--` separator itself, and PowerShell's parameter binder
# rejects a bare `--` as an ambiguous parameter name *before* this script runs —
# so no amount of in-script argument handling can absorb it. A
# ValueFromRemainingArguments catch-all does not help for the same reason.
#
# Use the dedicated package.json scripts instead (deploy:android:debugkey,
# deploy:resign, ...), or invoke this file directly:
#
#   powershell -NoProfile -ExecutionPolicy Bypass `
#     -File scripts/package-release.ps1 -Target android -AllowDebugKey

$ErrorActionPreference = 'Stop'

$scriptDir = $PSScriptRoot
$repoRoot = Split-Path $scriptDir -Parent
$msvcEnv = Join-Path $scriptDir 'msvc-env.ps1'

# Collected as we go and printed as a table at the end, so a long run finishes
# with a summary rather than leaving the reader to scroll back through it.
$results = New-Object System.Collections.ArrayList
$problems = New-Object System.Collections.ArrayList

function Write-Step  { param([string] $Message) Write-Host "`n==> $Message" -ForegroundColor Cyan }
function Write-Ok    { param([string] $Message) Write-Host "    OK   $Message" -ForegroundColor Green }
function Write-Note  { param([string] $Message) Write-Host "    note $Message" -ForegroundColor DarkGray }
function Write-Warn  { param([string] $Message) Write-Host "    warn $Message" -ForegroundColor Yellow }

function Add-Problem {
    param([string] $Message)
    [void] $problems.Add($Message)
    Write-Warn $Message
}

# -- Version ---------------------------------------------------------------
# Taken from tauri.conf.json rather than package.json: it is what the installer
# and the APK actually embed, so a mismatch would mean the filename lied.
function Get-AppVersion {
    $confPath = Join-Path $repoRoot 'src-tauri\tauri.conf.json'
    if (-not (Test-Path $confPath)) {
        throw "tauri.conf.json not found at $confPath"
    }
    $conf = Get-Content $confPath -Raw | ConvertFrom-Json
    if (-not $conf.version) {
        throw "No 'version' field in tauri.conf.json"
    }
    return $conf.version
}

# -- Tool discovery --------------------------------------------------------
# Highest *complete* build-tools directory, mirroring how msvc-env.ps1 picks an
# NDK. A partial SDK download otherwise wins the version sort and then fails
# with a missing-file error that names neither the SDK nor the download.
function Get-AndroidBuildTools {
    if (-not $env:ANDROID_HOME) {
        throw "ANDROID_HOME is not set. Install the Android SDK and set it, or use -Target windows."
    }

    $root = Join-Path $env:ANDROID_HOME 'build-tools'
    if (-not (Test-Path $root)) {
        throw "No build-tools under $root. Install with: sdkmanager `"build-tools;36.0.0`""
    }

    $candidates = Get-ChildItem $root -Directory -ErrorAction SilentlyContinue | Where-Object {
        (Test-Path (Join-Path $_.FullName 'zipalign.exe')) -and
        (Test-Path (Join-Path $_.FullName 'apksigner.bat'))
    }

    $chosen = $candidates | Sort-Object {
        $parsed = [version]'0.0.0'
        if ([version]::TryParse(($_.Name -split '-')[0], [ref]$parsed)) { $parsed } else { [version]'0.0.0' }
    } | Select-Object -Last 1

    if (-not $chosen) {
        throw "No build-tools directory contains both zipalign and apksigner. Reinstall with: sdkmanager `"build-tools;36.0.0`""
    }

    return [pscustomobject]@{
        ZipAlign  = Join-Path $chosen.FullName 'zipalign.exe'
        ApkSigner = Join-Path $chosen.FullName 'apksigner.bat'
        Version   = $chosen.Name
    }
}

# signtool ships in the Windows SDK, not on PATH. Prefer the x64 build from the
# highest SDK version present.
function Get-SignTool {
    $roots = @(
        (Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'),
        (Join-Path $env:ProgramFiles 'Windows Kits\10\bin')
    ) | Where-Object { $_ -and (Test-Path $_) }

    foreach ($root in $roots) {
        $found = Get-ChildItem $root -Directory -ErrorAction SilentlyContinue |
            Sort-Object {
                $parsed = [version]'0.0.0'
                if ([version]::TryParse($_.Name, [ref]$parsed)) { $parsed } else { [version]'0.0.0' }
            } -Descending |
            ForEach-Object { Join-Path $_.FullName 'x64\signtool.exe' } |
            Where-Object { Test-Path $_ } |
            Select-Object -First 1

        if ($found) { return $found }
    }
    return $null
}

# -- Build helper ----------------------------------------------------------
# Everything goes through msvc-env.ps1 so MSVC, cargo, and the NDK are resolved
# exactly as they are for `pnpm tauri:build`. The exit-code check is the point:
# see item 3 in the description.
function Invoke-Build {
    param([Parameter(Mandatory = $true)][string[]] $BuildArgs)

    Push-Location $repoRoot
    try {
        & powershell -NoProfile -ExecutionPolicy Bypass -File $msvcEnv @BuildArgs
        if ($LASTEXITCODE -ne 0) {
            throw ("Build failed (exit {0}): {1}" -f $LASTEXITCODE, ($BuildArgs -join ' '))
        }
    }
    finally {
        Pop-Location
    }
}

# -- Deploy helper ---------------------------------------------------------
function Publish-Artifact {
    param(
        [Parameter(Mandatory = $true)][string] $Source,
        [Parameter(Mandatory = $true)][string] $Name,
        [Parameter(Mandatory = $true)][string] $Kind,
        [string] $Signing = 'unsigned'
    )

    $destination = Join-Path $deployPath $Name
    Copy-Item -LiteralPath $Source -Destination $destination -Force

    $hash = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLower()
    $sizeMb = [math]::Round((Get-Item -LiteralPath $destination).Length / 1MB, 1)

    [void] $results.Add([pscustomobject]@{
        Artifact = $Name
        Kind     = $Kind
        Signing  = $Signing
        SizeMB   = $sizeMb
        Sha256   = $hash
    })

    Write-Ok "$Name ($sizeMb MB, $Signing)"
}

# =========================================================================
#  Android
# =========================================================================
function Invoke-AndroidRelease {
    param([string] $Version)

    Write-Step 'Android release'

    $keystoreProps = Join-Path $repoRoot 'src-tauri\keystore.properties'
    $haveReleaseKey = Test-Path $keystoreProps

    # Decided up front so the run fails in seconds rather than after a
    # four-minute-per-ABI LTO build that cannot be signed at the end.
    if (-not $haveReleaseKey -and -not $AllowDebugKey) {
        Add-Problem 'No src-tauri/keystore.properties: skipping Android.'
        Write-Note 'Create a release keystore (see docs/BUILDING.md), or pass -AllowDebugKey to'
        Write-Note 'produce a sideload-only build signed with the shared Android debug key.'
        return
    }

    $tools = Get-AndroidBuildTools
    Write-Note "build-tools $($tools.Version)"

    if (-not $SkipBuild) {
        Write-Note 'Building release APK and AAB (fat LTO, one link per ABI; this takes a while)'
        Invoke-Build @('vp', 'exec', 'tauri', 'android', 'build')
    }

    $outputs = Join-Path $repoRoot 'src-tauri\gen\android\app\build\outputs'
    if (-not (Test-Path $outputs)) {
        Add-Problem "No Android build output at $outputs. Run without -SkipBuild."
        return
    }

    # -- APK ---------------------------------------------------------------
    # Globbed rather than hardcoded: Gradle names the file
    # `app-universal-release.apk` when its signingConfig applied and
    # `...-release-unsigned.apk` when it did not, and which one appears depends
    # on whether keystore.properties existed at build time.
    $apk = Get-ChildItem (Join-Path $outputs 'apk') -Recurse -Filter '*release*.apk' -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -notlike '*-aligned*' -and $_.Name -notlike '*-signed*' } |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1

    if (-not $apk) {
        Add-Problem 'No release APK found in the build output.'
    }
    else {
        $signing = if ($apk.Name -like '*unsigned*') { 'needs-signing' } else { 'gradle-signed' }
        Write-Note "found $($apk.Name) [$signing]"

        $stagedApk = $apk.FullName
        $suffix = ''

        if ($signing -eq 'needs-signing') {
            if (-not $AllowDebugKey) {
                Add-Problem 'Gradle produced an unsigned APK even though keystore.properties exists.'
                Write-Note 'Check that storeFile in keystore.properties resolves, then rebuild.'
                return
            }

            $debugKeystore = Join-Path $env:USERPROFILE '.android\debug.keystore'
            if (-not (Test-Path $debugKeystore)) {
                Add-Problem "Debug keystore not found at $debugKeystore."
                return
            }

            # Staged in TEMP, not in deploy/: the Gradle output is left
            # untouched so a re-run is idempotent, and deploy/ only ever
            # contains finished artifacts rather than intermediates.
            $aligned = Join-Path $env:TEMP "localdrop-staging-$PID.apk"

            # zipalign BEFORE apksigner. Reversing these two produces an APK
            # that signs cleanly and then fails to install. See item 1.
            & $tools.ZipAlign -p -f 4 $apk.FullName $aligned
            if ($LASTEXITCODE -ne 0) { Add-Problem 'zipalign failed.'; return }

            # The documented defaults for the SDK-generated debug keystore.
            #
            # v4 signing off: it writes a separate <name>.apk.idsig that only
            # `adb install --incremental` consumes, and which would otherwise be
            # left beside the artifact and land in the checksum file.
            & $tools.ApkSigner sign `
                --ks $debugKeystore `
                --ks-pass pass:android `
                --key-pass pass:android `
                --ks-key-alias androiddebugkey `
                --v4-signing-enabled false `
                $aligned
            if ($LASTEXITCODE -ne 0) { Add-Problem 'apksigner failed.'; return }

            $stagedApk = $aligned
            $suffix = '-debugkey'
            Write-Warn 'Signed with the shared debug key: sideload only, not distributable.'
        }

        # Verified whatever the path taken, including for a Gradle-signed APK.
        # This is the check that catches a signing config that silently did
        # nothing, which is otherwise only discovered on a device.
        $verify = & $tools.ApkSigner verify --print-certs $stagedApk 2>&1
        if ($LASTEXITCODE -ne 0) {
            Add-Problem 'apksigner verify FAILED; refusing to publish this APK.'
            $verify | ForEach-Object { Write-Note $_ }
        }
        else {
            $signer = ($verify | Select-String 'Signer #1 certificate DN' | Select-Object -First 1)
            if ($signer) { Write-Note ($signer.ToString().Trim()) }

            $label = if ($suffix) { 'debug-key (sideload only)' } else { 'release-key' }
            Publish-Artifact -Source $stagedApk `
                -Name "LocalDrop-$Version-android-universal$suffix.apk" `
                -Kind 'Android APK' -Signing $label

            # Staging file and any sidecar apksigner may still have written.
            if ($suffix) {
                Remove-Item -LiteralPath $stagedApk -Force -ErrorAction SilentlyContinue
                Remove-Item -LiteralPath "$stagedApk.idsig" -Force -ErrorAction SilentlyContinue
            }
        }
    }

    # -- AAB ---------------------------------------------------------------
    # Play Store format. Not signed here: Play re-signs with the app signing key
    # on upload, and the upload key signature is applied by Gradle. An unsigned
    # AAB is still collected, because it is only useful to a Play upload which
    # reports its own signing errors clearly.
    $aab = Get-ChildItem (Join-Path $outputs 'bundle') -Recurse -Filter '*release*.aab' -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1

    if ($aab) {
        $aabSigning = if ($haveReleaseKey) { 'release-key (upload key)' } else { 'unsigned' }
        Publish-Artifact -Source $aab.FullName `
            -Name "LocalDrop-$Version-android-universal.aab" `
            -Kind 'Android AAB' -Signing $aabSigning
    }
    else {
        Write-Note 'No .aab in the build output (expected if only an APK was requested).'
    }
}

# =========================================================================
#  Windows
# =========================================================================
function Invoke-WindowsRelease {
    param([string] $Version)

    Write-Step 'Windows release'

    if (-not $SkipBuild) {
        Write-Note 'Building .msi and NSIS .exe'
        Invoke-Build @('vp', 'exec', 'tauri', 'build')
    }

    $bundle = Join-Path $repoRoot 'src-tauri\target\release\bundle'
    if (-not (Test-Path $bundle)) {
        Add-Problem "No Windows bundle at $bundle. Run without -SkipBuild."
        return
    }

    # -- Certificate -------------------------------------------------------
    # Resolved once, before signing anything, so a misconfigured certificate is
    # reported as one problem rather than once per artifact.
    $signTool = Get-SignTool
    $certFile = $env:LOCALDROP_WIN_CERT
    $certPass = $env:LOCALDROP_WIN_CERT_PASSWORD
    $certThumb = $env:LOCALDROP_WIN_CERT_THUMBPRINT
    $timestampUrl = if ($env:LOCALDROP_WIN_TIMESTAMP_URL) { $env:LOCALDROP_WIN_TIMESTAMP_URL } else { 'http://timestamp.digicert.com' }

    $canSign = $false
    if (-not $signTool) {
        Write-Note 'signtool.exe not found in any Windows SDK; artifacts will be unsigned.'
    }
    elseif ($certThumb) {
        $canSign = $true
        Write-Note 'Signing with the certificate matching LOCALDROP_WIN_CERT_THUMBPRINT.'
    }
    elseif ($certFile -and (Test-Path $certFile)) {
        if (-not $certPass) {
            Write-Note 'LOCALDROP_WIN_CERT is set but LOCALDROP_WIN_CERT_PASSWORD is not; trying without a password.'
        }
        $canSign = $true
        Write-Note "Signing with $certFile"
    }
    elseif ($certFile) {
        Add-Problem "LOCALDROP_WIN_CERT points at a missing file: $certFile"
    }
    else {
        Write-Note 'No code-signing certificate configured; artifacts will be unsigned.'
        Write-Note 'Unsigned installers work but show a SmartScreen warning on first run.'
    }

    # Both bundle targets from tauri.conf.json. The .msi is per-machine-capable
    # and enterprise-deployable; the NSIS .exe is the per-user installer that
    # carries the firewall hooks from src-tauri/nsis/.
    $targets = @(
        @{ Pattern = '*.msi'; Name = "LocalDrop-$Version-windows-x64.msi";       Kind = 'Windows MSI' },
        @{ Pattern = '*.exe'; Name = "LocalDrop-$Version-windows-x64-setup.exe"; Kind = 'Windows NSIS' }
    )

    foreach ($t in $targets) {
        $artifact = Get-ChildItem $bundle -Recurse -Filter $t.Pattern -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending |
            Select-Object -First 1

        if (-not $artifact) {
            Write-Note "No $($t.Pattern) in the bundle output."
            continue
        }

        # Copied to deploy/ first, then signed in place: the target/ tree is
        # rebuilt constantly, and signing a file that a later build overwrites
        # wastes the timestamping round trip.
        $destination = Join-Path $deployPath $t.Name
        Copy-Item -LiteralPath $artifact.FullName -Destination $destination -Force

        $signing = 'unsigned'
        if ($canSign) {
            $signArgs = @('sign', '/fd', 'SHA256', '/td', 'SHA256', '/tr', $timestampUrl)
            if ($certThumb) {
                $signArgs += @('/sha1', $certThumb)
            }
            else {
                $signArgs += @('/f', $certFile)
                if ($certPass) { $signArgs += @('/p', $certPass) }
            }
            $signArgs += $destination

            & $signTool @signArgs | Out-Null
            if ($LASTEXITCODE -ne 0) {
                Add-Problem "signtool failed for $($t.Name); leaving it unsigned."
            }
            else {
                # Verified with /pa (default authenticode policy), which is what
                # Windows itself applies when the user runs the installer.
                & $signTool verify /pa /q $destination | Out-Null
                if ($LASTEXITCODE -ne 0) {
                    Add-Problem "Signature verification failed for $($t.Name)."
                }
                else {
                    $signing = 'authenticode + timestamp'
                }
            }
        }

        # Published from the already-copied file, so the recorded hash is of the
        # signed bytes rather than the pre-signing ones.
        $hash = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLower()
        $sizeMb = [math]::Round((Get-Item -LiteralPath $destination).Length / 1MB, 1)
        [void] $results.Add([pscustomobject]@{
            Artifact = $t.Name
            Kind     = $t.Kind
            Signing  = $signing
            SizeMB   = $sizeMb
            Sha256   = $hash
        })
        Write-Ok "$($t.Name) ($sizeMb MB, $signing)"
    }
}

# =========================================================================
#  Main
# =========================================================================
$version = Get-AppVersion
$deployPath = Join-Path $repoRoot $DeployDir

Write-Host "LocalDrop $version -> $DeployDir/" -ForegroundColor White

if (-not (Test-Path $deployPath)) {
    New-Item -ItemType Directory -Path $deployPath | Out-Null
}

# Stale artifacts are cleared so the directory always reflects the latest build:
# a leftover from a previous version is worse than nothing, because once the
# filenames differ only by version the two are hard to tell apart.
#
# Only the artifacts this run is about to replace are removed, though. Clearing
# the whole directory would mean `pnpm deploy:windows` silently deleted the
# Android artifacts from an earlier `pnpm deploy:android`, which is exactly the
# workflow the per-platform scripts exist to support.
$clearPatterns = switch ($Target) {
    'windows' { @('*windows*') }
    'android' { @('*android*') }
    default   { @('*') }
}

foreach ($pattern in $clearPatterns) {
    Get-ChildItem $deployPath -File -Filter $pattern -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -ne 'SHA256SUMS.txt' } |
        Remove-Item -Force
}

if ($Target -eq 'all' -or $Target -eq 'windows') {
    # Windows first: it is the faster of the two, so a failure that is going to
    # happen surfaces before the multi-minute Android build.
    try { Invoke-WindowsRelease -Version $version }
    catch { Add-Problem "Windows packaging failed: $($_.Exception.Message)" }
}

if ($Target -eq 'all' -or $Target -eq 'android') {
    try { Invoke-AndroidRelease -Version $version }
    catch { Add-Problem "Android packaging failed: $($_.Exception.Message)" }
}

# -- Checksums -------------------------------------------------------------
# `sha256sum -c` compatible, so a recipient can verify a download with tooling
# they already have rather than trusting the file came from where they think.
#
# Rebuilt from everything currently in deploy/, not just this run's results.
# That is what makes a per-platform run additive: packaging Windows after
# Android leaves one checksum file covering both, rather than a file that
# silently omits half the release.
if ($results.Count -gt 0) {
    $sumsPath = Join-Path $deployPath 'SHA256SUMS.txt'

    $lines = Get-ChildItem $deployPath -File |
        Where-Object { $_.Name -ne 'SHA256SUMS.txt' } |
        Sort-Object Name |
        ForEach-Object { "$((Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLower())  $($_.Name)" }

    # Written with explicit LF endings via WriteAllText rather than Set-Content,
    # which emits CRLF on Windows PowerShell. `sha256sum -c` treats the trailing
    # CR as part of the filename and reports every entry as a missing file, so a
    # recipient on Linux or macOS cannot verify a CRLF checksum file at all.
    # UTF8Encoding($false) suppresses the BOM, which would corrupt line one.
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($sumsPath, (($lines -join "`n") + "`n"), $utf8NoBom)

    Write-Step 'Artifacts'
    $results | Format-Table Artifact, Kind, Signing, SizeMB -AutoSize
    Write-Host "    SHA256SUMS.txt covers $($lines.Count) file(s) in $DeployDir/" -ForegroundColor Green
}

# -- Summary ---------------------------------------------------------------
if ($problems.Count -gt 0) {
    Write-Host "`nFinished with $($problems.Count) problem(s):" -ForegroundColor Yellow
    $problems | ForEach-Object { Write-Host "  - $_" -ForegroundColor Yellow }
}

if ($results.Count -eq 0) {
    Write-Host "`nNo artifacts produced." -ForegroundColor Red
    exit 1
}

Write-Host "`nDone. $($results.Count) artifact(s) in $DeployDir/" -ForegroundColor Green

# Non-zero on partial success, so CI does not treat "one platform silently
# missing" as a clean release.
if ($problems.Count -gt 0) { exit 2 }
exit 0
