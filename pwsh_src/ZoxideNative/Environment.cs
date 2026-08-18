using System.Runtime.InteropServices;

namespace ZoxideNative;

/// <summary>
/// Cross-platform environment variable helper.
///
/// On Linux/macOS, .NET's <c>Environment.SetEnvironmentVariable</c> and
/// PowerShell's <c>$env:</c> only affect the .NET-managed environment block.
/// They do NOT call the native C library's <c>setenv()</c>, so in-process
/// native code (the zoxide-ffi Rust library loaded via P/Invoke) cannot see
/// those variables via <c>getenv()</c> / <c>std::env::var()</c>.
///
/// Use <c>Set-ZoxideEnv</c> from the module (or this class directly) for any
/// <c>_ZO_*</c> variable that the embedded library must read.
/// </summary>
public static partial class ZoxideEnvironment
{
    /// <summary>Set a variable in both the .NET and native environment blocks.</summary>
    public static void Set(string name, string value)
    {
        System.Environment.SetEnvironmentVariable(name, value);

        if (!RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
        {
            SetEnv(name, value, 1);
        }
    }

    /// <summary>Remove a variable from both environment blocks.</summary>
    public static void Remove(string name)
    {
        System.Environment.SetEnvironmentVariable(name, null);

        if (!RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
        {
            UnsetEnv(name);
        }
    }

    private const string LibC = "libc";

    [LibraryImport(LibC, EntryPoint = "setenv", StringMarshalling = StringMarshalling.Utf8)]
    private static partial int SetEnv(string name, string value, int overwrite);

    [LibraryImport(LibC, EntryPoint = "unsetenv", StringMarshalling = StringMarshalling.Utf8)]
    private static partial int UnsetEnv(string name);
}
