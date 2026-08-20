#Requires -Version 7.2
<#
.SYNOPSIS
    zoxide-native — in-process zoxide for PowerShell (pwsh 7+).

.DESCRIPTION
    This module mirrors the official `zoxide init powershell` script, replacing
    the `zoxide add` / `zoxide query` process invocations with the in-process
    [ZoxideNative.Session] managed wrapper. The zoxide database stays open for
    the lifetime of the pwsh process.

    The native library (libzoxide_ffi.so / libzoxide_ffi.dylib /
    zoxide_ffi.dll) must be present next to ZoxideNative.dll. To load it from
    another location, set the ZOXIDE_FFI_PATH environment variable.

.EXAMPLE
    Import-Module zoxide-native
    z proj
    Get-ZoxideNativeStats
#>

Set-StrictMode -Version Latest

# ---- Ensure the binary assembly is available --------------------------------
# Normally it is loaded as a NestedModule from the manifest; this fallback only
# matters when the .psm1 is imported directly during development.
if (-not ('ZoxideNative.Session' -as [type])) {
    Import-Module (Join-Path $PSScriptRoot 'ZoxideNative.dll') -ErrorAction Stop
}

# ---- Point the FFI resolver at a colocated native library -------------------
$script:NativeLibName = switch ($true) {
    $IsWindows { 'zoxide_ffi.dll' }
    $IsMacOS   { 'libzoxide_ffi.dylib' }
    default    { 'libzoxide_ffi.so' }
}
if (-not $env:ZOXIDE_FFI_PATH) {
    $colocated = Join-Path $PSScriptRoot $script:NativeLibName
    if (Test-Path -LiteralPath $colocated -PathType Leaf) {
        $env:ZOXIDE_FFI_PATH = $colocated
    }
}

# ---- Native session (created lazily, lives for the pwsh process) -------------
$script:__Session = $null

function Get-Session {
    if ($null -eq $script:__Session) {
        # The native session reads _ZO_DATA_DIR through getenv(). Copy the
        # managed value into the native environment before creating it.
        $dataDir = [System.Environment]::GetEnvironmentVariable('_ZO_DATA_DIR')
        if ($null -ne $dataDir) {
            [ZoxideNative.ZoxideEnvironment]::Set('_ZO_DATA_DIR', $dataDir)
        }
        $script:__Session = [ZoxideNative.Session]::new()
    }
    return $script:__Session
}

# ---- Environment helper for native-visible variables -------------------------
# On Linux/macOS, $env:_ZO_MAXAGE = 100 only updates the .NET environment
# block. The Rust library reads getenv(), so these helpers write both blocks.
$script:__ZoxideNativeEnvNames = @(
    '_ZO_EXCLUDE_DIRS'
    '_ZO_FZF_OPTS'
    '_ZO_MAXAGE'
    '_ZO_RESOLVE_SYMLINKS'
)

foreach ($envName in $script:__ZoxideNativeEnvNames) {
    $envValue = [System.Environment]::GetEnvironmentVariable($envName)
    if ($null -ne $envValue) {
        [ZoxideNative.ZoxideEnvironment]::Set($envName, $envValue)
    }
}

function Set-ZoxideEnv {
    <#
    .SYNOPSIS
        Sets an environment variable so it is visible to the in-process native
        Rust library (also updates the .NET environment block).
    .EXAMPLE
        Set-ZoxideEnv _ZO_MAXAGE 10000
    #>
    param([string]$Name, [string]$Value)
    [ZoxideNative.ZoxideEnvironment]::Set($Name, $Value)
}

function Remove-ZoxideEnv {
    <#
    .SYNOPSIS
        Removes an environment variable from both the .NET and native blocks.
    .EXAMPLE
        Remove-ZoxideEnv _ZO_FZF_OPTS
    #>
    param([string]$Name)
    [ZoxideNative.ZoxideEnvironment]::Remove($Name)
}

# ---- Public metadata helpers -------------------------------------------------

function Get-ZoxideNativeVersion {
    <#
    .SYNOPSIS
        Returns the version of the embedded zoxide-ffi native library.
    #>
    return [ZoxideNative.Session]::Version()
}

function Get-ZoxideNativeStats {
    <#
    .SYNOPSIS
        Returns session counters for the current zoxide session.
    #>
    return (Get-Session).GetStatsReport()
}

# ---- The rest is adapted from zoxide init powershell -------------------------

# pwd based on zoxide's format.
function global:__zoxide_pwd {
    $cwd = Microsoft.PowerShell.Management\Get-Location
    if ($cwd.Provider.Name -eq "FileSystem") {
        $cwd.ProviderPath
    }
}

# cd + custom logic based on the value of _ZO_ECHO.
function global:__zoxide_cd($dir, $literal) {
    $dir = if ($literal) {
        if ($null -eq $dir) {
            Microsoft.PowerShell.Management\Set-Location
        } else {
            Microsoft.PowerShell.Management\Set-Location -LiteralPath $dir -Passthru -ErrorAction Stop
        }
    } else {
        if ($dir -eq '-' -and ($PSVersionTable.PSVersion -lt 6.1)) {
            Microsoft.PowerShell.Utility\Write-Error "cd - is not supported below PowerShell 6.1. Please upgrade your version of PowerShell."
        }
        elseif ($dir -eq '+' -and ($PSVersionTable.PSVersion -lt 6.2)) {
            Microsoft.PowerShell.Utility\Write-Error "cd + is not supported below PowerShell 6.2. Please upgrade your version of PowerShell."
        }
        else {
            Microsoft.PowerShell.Management\Set-Location -Path $dir -Passthru -ErrorAction Stop
        }
    }
}

# Hook to add new entries to the database.
$script:__zoxide_oldpwd = __zoxide_pwd
function global:__zoxide_hook {
    $result = __zoxide_pwd
    if ($result -ne $script:__zoxide_oldpwd) {
        if ($null -ne $result) {
            (Get-Session).Add($result, 1.0)
        }
        $script:__zoxide_oldpwd = $result
    }
}

# Initialize the prompt hook (mirrors `zoxide init powershell` default).
$script:__zoxideOriginalPrompt = $null
if (Test-Path function:\prompt) {
    $script:__zoxideOriginalPrompt = $function:prompt
}

function global:prompt {
    if ($null -ne $script:__zoxideOriginalPrompt) {
        & $script:__zoxideOriginalPrompt
    }
    try {
        $null = __zoxide_hook
    } catch {
        # A database problem must never break the prompt.
    }
}

# ---- Jump functions ----------------------------------------------------------

function Invoke-ZoxideQuery {
    <#
    .SYNOPSIS
        Internal helper: bind PowerShell named parameters onto the C# Session
        method. Returns [ZoxideNative.QueryResult] rather than throwing when
        the native query fails (e.g. no match or fzf Ctrl-C).
    #>
    param(
        [string[]]$Keywords = @(),
        [string]$Exclude,
        [string]$BaseDir,
        [bool]$All = $false,
        [bool]$Interactive = $false,
        [bool]$List = $false,
        [bool]$Score = $false
    )
    return (Get-Session).Query(
        $Keywords, $Exclude, $BaseDir, $All, $Interactive, $List, $Score)
}

function Write-ZoxideQueryError {
    <#
    .SYNOPSIS
        Mirror `zoxide` binary stderr behavior for a failed query. Silent exits
        (fzf Ctrl-C) have an empty native error and produce no output.
    #>
    param([string]$Message)
    if (-not [string]::IsNullOrEmpty($Message)) {
        [Console]::Error.WriteLine("zoxide-native: $Message")
    }
}

# Jump to a directory using only keywords.
function global:__zoxide_z {
    if ($args.Count -eq 0) {
        __zoxide_cd $null $true
        return
    }
    elseif ($args.Count -eq 1 -and ($args[0] -eq '-' -or $args[0] -eq '+')) {
        __zoxide_cd $args[0] $false
        return
    }
    elseif ($args.Count -eq 1 -and (Microsoft.PowerShell.Management\Test-Path -PathType Container -LiteralPath $args[0])) {
        __zoxide_cd $args[0] $true
        return
    }
    elseif ($args.Count -eq 1 -and (Microsoft.PowerShell.Management\Test-Path -PathType Container -Path $args[0])) {
        __zoxide_cd $args[0] $false
        return
    }

    $keywords = if ($args.Count -gt 0) { [string[]]$args } else { [string[]]@() }
    $exclude = __zoxide_pwd
    $query = Invoke-ZoxideQuery -Keywords $keywords -Exclude $exclude
    if (-not $query.Success) {
        Write-ZoxideQueryError -Message $query.Error
        return
    }
    if (-not [string]::IsNullOrEmpty($query.Output)) {
        __zoxide_cd $query.Output $true
    }
}

# Jump to a directory using interactive search.
function global:__zoxide_zi {
    $keywords = if ($args.Count -gt 0) { [string[]]$args } else { [string[]]@() }
    $query = Invoke-ZoxideQuery -Keywords $keywords -Interactive $true
    if (-not $query.Success) {
        Write-ZoxideQueryError -Message $query.Error
        return
    }
    if (-not [string]::IsNullOrEmpty($query.Output)) {
        __zoxide_cd $query.Output $true
    }
}

# Commands for zoxide. Disable these with $env:ZOXIDE_NO_CMD = '1' before
# importing the module.
if ($env:ZOXIDE_NO_CMD -ne '1') {
    Microsoft.PowerShell.Utility\Set-Alias -Name z -Value __zoxide_z -Option AllScope -Scope Global -Force
    Microsoft.PowerShell.Utility\Set-Alias -Name zi -Value __zoxide_zi -Option AllScope -Scope Global -Force
}

# ---- Export the public API ---------------------------------------------------
Export-ModuleMember -Function @(
    "Get-ZoxideNativeVersion"
    "Get-ZoxideNativeStats"
    "Set-ZoxideEnv"
    "Remove-ZoxideEnv"
)

# ---- Restore the original prompt when the module is removed ------------------
$MyInvocation.MyCommand.ScriptBlock.Module.OnRemove = {
    if ($null -ne $script:__zoxideOriginalPrompt) {
        Set-Item -Path function:\prompt -Value $script:__zoxideOriginalPrompt
    } else {
        Remove-Item -Path function:\prompt -ErrorAction SilentlyContinue
    }
    if ($null -ne $script:__Session) {
        try { $script:__Session.Dispose() } catch {}
        $script:__Session = $null
    }
}
