using System.Reflection;
using System.Runtime.InteropServices;

namespace ZoxideNative;

/// <summary>
/// P/Invoke declarations for the zoxide-ffi native library.
///
/// Uses .NET 7+ LibraryImport source generators for compile-time stub
/// generation — faster invocation and AOT-friendly compared to DllImport.
///
/// On Linux/macOS, libzoxide_ffi.so / .dylib must be placed next to
/// ZoxideNative.dll. On Windows, zoxide_ffi.dll must be in the same directory.
///
/// A custom <see cref="NativeLibrary.SetDllImportResolver"/> honors the
/// ZOXIDE_FFI_PATH environment variable (absolute path to the native lib),
/// falling back to the default .NET resolution (the assembly directory).
///
/// Strings returned by zo_session_query and zo_last_error must be freed with
/// zo_free. zo_version strings are static and must NOT be freed.
/// </summary>
internal static unsafe partial class NativeMethods
{
    // Linux: libzoxide_ffi.so, macOS: libzoxide_ffi.dylib, Windows: zoxide_ffi.dll.
    private const string LibName = "zoxide_ffi";

    static NativeMethods()
    {
        NativeLibrary.SetDllImportResolver(typeof(NativeMethods).Assembly, ResolveNativeLibrary);
    }

    private static IntPtr ResolveNativeLibrary(
        string libraryName, Assembly assembly, DllImportSearchPath? searchPath)
    {
        if (!string.Equals(libraryName, LibName, StringComparison.OrdinalIgnoreCase))
            return IntPtr.Zero;

        string? overridePath = System.Environment.GetEnvironmentVariable("ZOXIDE_FFI_PATH");
        if (!string.IsNullOrEmpty(overridePath))
        {
            string fullPath = Path.GetFullPath(overridePath);
            if (File.Exists(fullPath))
            {
                return NativeLibrary.Load(fullPath);
            }
        }

        // Fall back to default resolution (probes the assembly directory).
        return IntPtr.Zero;
    }

    // ── Session lifecycle ──────────────────────────────────────────────

    /// <summary>
    /// Create a session. Returns
    /// <see cref="IntPtr.Zero"/> on failure.
    /// </summary>
    [LibraryImport(LibName, EntryPoint = "zo_session_create")]
    internal static partial IntPtr SessionCreate();

    /// <summary>Destroy a session. NULL-safe.</summary>
    [LibraryImport(LibName, EntryPoint = "zo_session_destroy")]
    internal static partial void SessionDestroy(IntPtr session);

    // ── Database operations ─────────────────────────────────────────────

    /// <summary>Add a directory to the database. Returns 0 on success.</summary>
    [LibraryImport(LibName, EntryPoint = "zo_session_add", StringMarshalling = StringMarshalling.Utf8)]
    internal static partial int SessionAdd(IntPtr session, string path, double score);

    /// <summary>Remove a directory from the database. Returns 0 on success.</summary>
    [LibraryImport(LibName, EntryPoint = "zo_session_remove", StringMarshalling = StringMarshalling.Utf8)]
    internal static partial int SessionRemove(IntPtr session, string path);

    /// <summary>
    /// Run a query. On success (return 0), writes a Rust-allocated UTF-8
    /// string to <paramref name="output"/>. The caller must free it with
    /// <see cref="Free"/>.
    /// </summary>
    [LibraryImport(LibName, EntryPoint = "zo_session_query")]
    internal static partial int SessionQuery(IntPtr session, IntPtr options, out IntPtr output);

    // ── Statistics and metadata ──────────────────────────────────────────

    /// <summary>Retrieve session counters. Returns 0 on success.</summary>
    [LibraryImport(LibName, EntryPoint = "zo_session_stats")]
    internal static partial int SessionStats(IntPtr session, out ZoStats stats);

    /// <summary>Free a string returned by SessionQuery / LastError. NULL-safe.</summary>
    [LibraryImport(LibName, EntryPoint = "zo_free")]
    internal static partial void Free(IntPtr ptr);

    /// <summary>
    /// Return the library version as a static UTF-8 string. Must NOT be freed.
    /// </summary>
    [LibraryImport(LibName, EntryPoint = "zo_version")]
    internal static partial IntPtr Version();

    /// <summary>
    /// Return the last error as a Rust-allocated UTF-8 string. The caller must
    /// free it with <see cref="Free"/>. Returns NULL if no error is recorded.
    /// </summary>
    [LibraryImport(LibName, EntryPoint = "zo_last_error")]
    internal static partial void LastError(out IntPtr output);
}

/// <summary>
/// C-compatible input struct for zo_session_query. Must exactly match the
/// Rust <c>zo_query_options</c> layout.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal struct ZoQueryOptions
{
    public IntPtr Keywords;       // const char* const*
    public UIntPtr KeywordsLen;   // size_t
    public IntPtr Exclude;        // const char*
    public IntPtr BaseDir;        // const char*
    public int All;               // int (C bool)
    public int Interactive;       // int
    public int List;              // int
    public int Score;             // int
}

/// <summary>C-compatible stats struct for zo_session_stats.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct ZoStats
{
    public ulong Adds;
    public ulong Queries;
    public ulong Removes;
    public ulong Entries;
}
