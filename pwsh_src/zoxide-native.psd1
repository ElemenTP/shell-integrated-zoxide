@{
    # zoxide-native module manifest.
    #
    # Hybrid script + binary module: the .psm1 contains the zoxide shell logic
    # and is the RootModule; the C# assembly (ZoxideNative.dll) is loaded as a
    # NestedModule so its types are available to the .psm1 at import time.

    RootModule           = 'zoxide-native.psm1'
    ModuleVersion        = '0.1.0'
    GUID                 = '5b5c7d1b-9f2d-4b13-a84d-2a6e1c8f0a77'

    Author               = 'ElemenTP'
    CompanyName          = 'ElemenTP'
    Copyright            = '(c) ElemenTP. MIT license.'

    Description          = 'In-process zoxide for PowerShell (pwsh 7+). Runs zoxide add/query inside the shell process through a Rust FFI library — no process spawn for directory hooks or jumps.'

    PowerShellVersion    = '7.2'
    CompatiblePSEditions = @('Core')

    NestedModules        = @('ZoxideNative.dll')

    # z/z_aliases are installed as global functions/aliases by the .psm1 and
    # must NOT be listed here (they do not exist in module scope).
    FunctionsToExport    = @(
        'Get-ZoxideNativeVersion'
        'Get-ZoxideNativeStats'
        'Set-ZoxideEnv'
        'Remove-ZoxideEnv'
    )
    CmdletsToExport      = @()
    VariablesToExport    = @()
    AliasesToExport      = @()

    PrivateData          = @{
        PSData = @{
            Tags = @(
                'zoxide', 'cd', 'shell', 'directory',
                'PSEdition_Core', 'Linux', 'macOS', 'Windows',
                'native', 'in-process', 'rust', 'ffi'
            )
            ProjectUri              = 'https://github.com/ElemenTP/shell-integrated-zoxide'
            LicenseUri              = 'https://github.com/ajeetdsouza/zoxide/blob/main/LICENSE'
            IconUri                 = ''
            ReleaseNotes            = @'
## 0.1.0

- Initial release: in-process zoxide for PowerShell (pwsh 7+).
- Zero-fork `zoxide add` / `zoxide query` through a Rust FFI library.
- Persistent database session for the lifetime of the pwsh process.
- `Get-ZoxideNativeVersion` / `Get-ZoxideNativeStats` helpers.
'@
            Prerelease              = ''
            RequireLicenseAcceptance = $false
        }
    }
}
