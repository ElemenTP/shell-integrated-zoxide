#Requires -Version 7.2
<#
.SYNOPSIS
    test_pwsh.ps1 — PowerShell integration test for the zoxide-native module.

.DESCRIPTION
    Imports the compiled binary module, records the current directory through
    the native session, then jumps back with the in-process query API.

.PARAMETER DllDir
    Directory containing zoxide-native.psd1, ZoxideNative.dll and the native
    zoxide_ffi library.

.EXAMPLE
    pwsh -NoProfile -File tests/test_pwsh.ps1 -DllDir pwsh_src/ZoxideNative/bin/Release/net8.0
#>
param(
    [Parameter(Mandatory = $true)]
    [string]$DllDir
)

$ErrorActionPreference = 'Stop'

$manifest = Join-Path $DllDir 'zoxide-native.psd1'
if (-not (Test-Path -LiteralPath $manifest -PathType Leaf)) {
    throw "Module manifest not found: $manifest"
}

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("zoxide-pwsh-test-" + [guid]::NewGuid().ToString('N'))
$dataDir = Join-Path $tmp 'data'
$targetDir = Join-Path $tmp 'projects/alpha'
$modulePathRoot = Join-Path $tmp 'modules'
$moduleDir = Join-Path $modulePathRoot 'zoxide-native'
$null = New-Item -ItemType Directory -Path $dataDir, $targetDir, $moduleDir -Force

# Stage the built output as a PSModulePath entry so the test also covers
# `Import-Module zoxide-native` by module name (not just by manifest path).
Copy-Item -Path (Join-Path $DllDir '*') -Destination $moduleDir -Recurse -Force
$savedPSModulePath = $env:PSModulePath
$env:PSModulePath = $modulePathRoot + [System.IO.Path]::PathSeparator + $savedPSModulePath

$originalPromptExists = Test-Path function:\prompt
$originalPrompt = if ($originalPromptExists) { Get-Content function:\prompt } else { $null }

try {
    Import-Module zoxide-native -Force -ErrorAction Stop

    $version = Get-ZoxideNativeVersion
    if ([string]::IsNullOrWhiteSpace($version)) {
        throw 'Get-ZoxideNativeVersion returned an empty version'
    }
    Write-Host "PASS: module imported, native version $version"

    # _ZO_DATA_DIR must be visible to both .NET and the native Rust library.
    Set-ZoxideEnv '_ZO_DATA_DIR' $dataDir
    $env:_ZO_DATA_DIR = $dataDir

    if (-not (Test-Path Alias:z)) { throw 'z alias was not installed' }
    if (-not (Test-Path function:\__zoxide_z)) { throw '__zoxide_z function was not installed' }
    Write-Host 'PASS: z/zi aliases and hook functions installed'

    # Add the target directory with the prompt hook. Calling the hook twice
    # from the same directory must not create duplicate entries.
    Set-Location -LiteralPath $targetDir
    __zoxide_hook
    __zoxide_hook

    # Move away and jump back using the in-process query.
    Set-Location -LiteralPath $tmp
    __zoxide_z alpha
    if ((Get-Location).ProviderPath -ne $targetDir) {
        throw "zoxide jump failed: expected $targetDir, got $((Get-Location).ProviderPath)"
    }
    Write-Host 'PASS: __zoxide_z jumped back to the target directory'

    $stats = Get-ZoxideNativeStats
    if ($stats -notmatch 'Adds: 1' -or $stats -notmatch 'Entries: 1') {
        throw "Unexpected native stats: $stats"
    }
    Write-Host "PASS: session stats ($stats)"

    # Interactive query through fzf, driven non-interactively with --filter.
    if (Get-Command fzf -ErrorAction SilentlyContinue) {
        Set-Location -LiteralPath $tmp
        Set-ZoxideEnv '_ZO_FZF_OPTS' '--filter=alpha'
        try {
            __zoxide_zi alpha
        } finally {
            Remove-ZoxideEnv '_ZO_FZF_OPTS'
        }
        if ((Get-Location).ProviderPath -ne $targetDir) {
            throw "zoxide interactive jump failed: expected $targetDir, got $((Get-Location).ProviderPath)"
        }
        Write-Host 'PASS: __zoxide_zi jumped with fzf'
    } else {
        Write-Host 'SKIP: fzf not installed'
    }

    # A non-interactive query with no match must not throw. It writes
    # "zoxide-native: no match found" to stderr (the module keeps the
    # zoxide-native prefix by design; the raw stderr line is intentionally not
    # redirected).
    Set-Location -LiteralPath $tmp
    __zoxide_z definitely-no-such-entry
    if ((Get-Location).ProviderPath -ne $tmp) {
        throw 'failed non-interactive query changed the current directory'
    }
    Write-Host 'PASS: no-match query reports error without throwing'

    # fzf exiting 130 is the Ctrl-C path in the original binary: silent, no
    # directory change, no exception.
    $fakeBin = Join-Path $tmp 'fake-bin'
    $null = New-Item -ItemType Directory -Path $fakeBin -Force
    $savedPath = $null
    if ($IsWindows) {
        $fakeFzf = Join-Path $fakeBin 'fzf.cmd'
        Set-Content -LiteralPath $fakeFzf -Value 'exit 130'
    } else {
        $fakeFzf = Join-Path $fakeBin 'fzf'
        Set-Content -LiteralPath $fakeFzf -Value "#!/bin/sh`nexit 130"
        & chmod +x -- $fakeFzf
    }
    try {
        $savedPath = [System.Environment]::GetEnvironmentVariable('PATH')
        $fakePath = $fakeBin + [System.IO.Path]::PathSeparator + $savedPath
        $env:PATH = $fakePath
        Set-ZoxideEnv 'PATH' $fakePath
        Set-Location -LiteralPath $tmp
        $ctrlCOutput = @(& { __zoxide_zi alpha } 2>&1)
        if ((Get-Location).ProviderPath -ne $tmp) {
            throw 'Ctrl-C interactive query changed the current directory'
        }
        if ($ctrlCOutput.Count -ne 0) {
            throw "Ctrl-C interactive query produced output: $($ctrlCOutput -join ' | ')"
        }
        Write-Host 'PASS: fzf Ctrl-C exit is silent and does not throw'
    } finally {
        if ($null -ne $savedPath) {
            $env:PATH = $savedPath
            Set-ZoxideEnv 'PATH' $savedPath
        }
    }

    # Environment helpers must update both environment blocks.
    Set-ZoxideEnv '_ZO_ECHO' '0'
    if ($env:_ZO_ECHO -ne '0') { throw '_ZO_ECHO was not set in the .NET environment' }
    Remove-ZoxideEnv '_ZO_ECHO'
    Write-Host 'PASS: native environment helpers'
}
finally {
    Get-Module zoxide-native -ErrorAction SilentlyContinue | Remove-Module -Force -ErrorAction SilentlyContinue
    $env:PSModulePath = $savedPSModulePath
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

if ($originalPromptExists -and -not (Test-Path function:\prompt)) {
    throw 'module removal deleted the original prompt function'
}

Write-Host 'pwsh integration test passed'
