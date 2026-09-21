<#
.SYNOPSIS
    Runs a command with the MSVC build environment and Rust toolchain on PATH.

.DESCRIPTION
    Several things go wrong when building from an ordinary shell on Windows,
    and each produces an error that points nowhere near its cause:

      1. "program not found" for cargo, when the shell was opened before rustup
         appended ~/.cargo/bin to the user PATH.

      2. "LINK : fatal error LNK1181: cannot open input file 'dbghelp.lib'",
         because rustc locates link.exe via vswhere but does not populate LIB
         and INCLUDE for every Visual Studio layout (notably VS 2026 / v18
         Build Tools). The SDK is installed; the linker simply cannot see it.

      3. "Failed to lookup version of installed NDK: ... source.properties",
         because Tauri picks the highest-numbered directory under
         $ANDROID_HOME/ndk without checking that it is a complete install.

    This script imports the vcvars64 environment, prepends the cargo bin
    directory, resolves NDK_HOME to a usable NDK, then runs the given command.

    NOTE: keep this file pure ASCII. It is invoked via `powershell` (Windows
    PowerShell 5.1), which reads BOM-less files as ANSI. A stray em dash or
    ellipsis becomes mojibake and produces a cascade of bogus parse errors
    about unterminated strings.

.EXAMPLE
    powershell -NoProfile -File scripts/msvc-env.ps1 cargo test
    powershell -NoProfile -File scripts/msvc-env.ps1 vp exec tauri android dev
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, ValueFromRemainingArguments = $true)]
    [string[]] $Command
)

$ErrorActionPreference = 'Stop'

# -- Module path -----------------------------------------------------------
# Launched from PowerShell 7 (or from pnpm, which inherits PS7's environment),
# Windows PowerShell receives a PSModulePath whose first entry is the PS7
# module directory. It then loads PS7's Microsoft.PowerShell.Utility, which the
# 5.1 engine cannot use: binary cmdlets keep working because they are already
# in the session state, while the script functions the module exports -
# Get-FileHash, Format-Hex, Import-PowerShellDataFile - go missing with a bare
# "is not recognized as the name of a cmdlet" and no hint of a module clash.
#
# Repaired here as well as in package-release.ps1 so that every build launched
# through this wrapper, and anything Gradle or Tauri spawn underneath it,
# inherits a PSModulePath that resolves to the running engine's own modules.
$psHomeModules = Join-Path $PSHOME 'Modules'
$env:PSModulePath = (@($psHomeModules) + (($env:PSModulePath -split ';') |
        Where-Object { $_ -and $_ -ne $psHomeModules })) -join ';'

# -- Rust on PATH ----------------------------------------------------------
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if ((Test-Path $cargoBin) -and ($env:PATH -notlike "*$cargoBin*")) {
    $env:PATH = "$cargoBin;$env:PATH"
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Error "cargo not found. Install Rust from https://rustup.rs and reopen your terminal."
    exit 1
}

# -- MSVC environment ------------------------------------------------------
# Skip the work if a developer prompt already set it up.
if (-not $env:LIB) {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vswhere)) {
        Write-Error "vswhere.exe not found. Install Visual Studio Build Tools with the Desktop C++ workload."
        exit 1
    }

    $vsRoot = & $vswhere -latest -products * -property installationPath
    if (-not $vsRoot) {
        Write-Error "No Visual Studio installation found. Install the Desktop C++ workload."
        exit 1
    }

    $vcvars = Join-Path $vsRoot 'VC\Auxiliary\Build\vcvars64.bat'
    if (-not (Test-Path $vcvars)) {
        Write-Error "vcvars64.bat not found under '$vsRoot'. The Desktop C++ workload may be missing."
        exit 1
    }

    # Run vcvars in cmd, dump the resulting environment, and import it here.
    # `set` output is KEY=VALUE per line; values may themselves contain '='.
    #
    # PSModulePath is skipped. vcvars does not set it, but cmd inherited the
    # unrepaired value and dumps it back out, so importing it blindly would
    # undo the fix above and reintroduce the missing-Get-FileHash failure.
    & cmd /c "`"$vcvars`" >nul 2>&1 && set" | ForEach-Object {
        $pair = $_.Split('=', 2)
        if ($pair.Length -eq 2 -and $pair[0] -ne 'PSModulePath') {
            Set-Item -Path "env:$($pair[0])" -Value $pair[1] -ErrorAction SilentlyContinue
        }
    }

    if (-not $env:LIB) {
        Write-Error "vcvars64.bat ran but LIB is still unset; the Windows SDK component is likely missing."
        exit 1
    }
}

# -- Android ---------------------------------------------------------------
# `tauri android` needs NDK_HOME to point at one NDK, not at the directory that
# holds them. A common mis-set is the parent:
#
#   NDK_HOME = ...\Sdk\ndk              <- wrong, holds 28.2.13676358
#   NDK_HOME = ...\Sdk\ndk\28.2.13676358 <- right
#
# Tauri takes the value at face value and fails with
# "Failed to open ...\ndk\source.properties". An inherited value is therefore
# validated rather than trusted; only a directory containing source.properties
# is a real NDK. This only changes the variable for this process.
if ($env:NDK_HOME -and -not (Test-Path (Join-Path $env:NDK_HOME 'source.properties'))) {
    Write-Warning "NDK_HOME is not an NDK: $env:NDK_HOME"
    Write-Warning 'Ignoring it and detecting one under $ANDROID_HOME\ndk instead.'
    $env:NDK_HOME = $null
}

# The SDK installs NDKs side by side, so pick the highest *complete* one rather
# than hardcoding a version. A directory without source.properties is a failed
# download and must be skipped, or the build dies later complaining about a
# missing toolchain.
if (-not $env:NDK_HOME -and $env:ANDROID_HOME) {
    $ndkRoot = Join-Path $env:ANDROID_HOME 'ndk'

    if (Test-Path $ndkRoot) {
        $all = @(Get-ChildItem $ndkRoot -Directory -ErrorAction SilentlyContinue)

        $complete = @($all | Where-Object {
            (Test-Path (Join-Path $_.FullName 'source.properties')) -and
            (Test-Path (Join-Path $_.FullName 'toolchains\llvm\prebuilt'))
        })

        $chosen = $complete | Sort-Object {
            # Version-aware sort so 28.2 does not lose to 9.x lexically.
            $parsed = [version]'0.0.0'
            if ([version]::TryParse(($_.Name -split '-')[0], [ref]$parsed)) { $parsed } else { [version]'0.0.0' }
        } | Select-Object -Last 1

        if ($chosen) {
            $env:NDK_HOME = $chosen.FullName
        }

        # Warn about partial installs either way: Tauri's own discovery ignores
        # NDK_HOME in some code paths and will still trip over them.
        $broken = @($all | Where-Object { -not (Test-Path (Join-Path $_.FullName 'source.properties')) })
        if ($broken.Count -gt 0) {
            Write-Warning 'Incomplete NDK install(s) found. These will break the Android build:'
            $broken | ForEach-Object { Write-Warning ("  " + $_.FullName) }
            Write-Warning 'Delete them, then reinstall with: sdkmanager "ndk;28.2.13676358"'
        }
    }
}

# -- Run -------------------------------------------------------------------
$exe = $Command[0]
$rest = @()
if ($Command.Length -gt 1) { $rest = $Command[1..($Command.Length - 1)] }

& $exe @rest
exit $LASTEXITCODE
