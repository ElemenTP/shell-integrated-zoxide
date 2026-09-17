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
/// Error reporting model: every native call that can fail returns its error
/// message directly — NULL means success, otherwise the pointer is a
/// Rust-allocated UTF-8 message to free with zo_free. Errors stay local to the
/// call, so there is no error slot to inspect or clear. An empty message is
/// still a failure (zoxide reports fzf Ctrl-C that way), so callers test the
/// pointer, never the text.
///
/// The native library owns exactly one process-global session: <see cref="Init"/>
/// creates it and <see cref="Shutdown"/> drops it. The database operations take
/// no session handle.
///
/// Strings returned by zo_query (results) and by any failing call (error
/// messages) must be freed with zo_free. zo_version strings are static and must
/// NOT be freed.
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
    /// Initialize the process-global session. Idempotent. Returns NULL on
    /// success, otherwise an error message to free with <see cref="Free"/>.
    /// </summary>
    [LibraryImport(LibName, EntryPoint = "zo_init")]
    internal static partial IntPtr Init();

    /// <summary>
    /// Drop the process-global session, resetting its counters. The on-disk
    /// database is untouched.
    /// </summary>
    [LibraryImport(LibName, EntryPoint = "zo_shutdown")]
    internal static partial IntPtr Shutdown();

    // ── Database operations ─────────────────────────────────────────────

    /// <summary>
    /// Add a directory to the database. Returns NULL on success, otherwise an
    /// error message to free with <see cref="Free"/>.
    /// </summary>
    [LibraryImport(LibName, EntryPoint = "zo_add", StringMarshalling = StringMarshalling.Utf8)]
    internal static partial IntPtr Add(string path, double score);

    /// <summary>
    /// Remove a directory from the database. Returns NULL on success,
    /// otherwise an error message to free with <see cref="Free"/>.
    /// </summary>
    [LibraryImport(LibName, EntryPoint = "zo_remove", StringMarshalling = StringMarshalling.Utf8)]
    internal static partial IntPtr Remove(string path);

    /// <summary>
    /// Run a query. Returns NULL on success and writes a Rust-allocated UTF-8
    /// result to <paramref name="output"/> (free it with <see cref="Free"/>);
    /// on failure returns an error message and sets the output to NULL.
    /// </summary>
    [LibraryImport(LibName, EntryPoint = "zo_query")]
    internal static partial IntPtr Query(IntPtr options, out IntPtr output);

    // ── Statistics and metadata ──────────────────────────────────────────

    /// <summary>
    /// Retrieve session counters. Returns NULL on success, else an error
    /// message to free with <see cref="Free"/>.
    /// </summary>
    [LibraryImport(LibName, EntryPoint = "zo_stats")]
    internal static partial IntPtr Stats(out ZoStats stats);

    /// <summary>
    /// Free a string returned by any native call — a query result or an error
    /// message. NULL-safe, cannot fail.
    /// </summary>
    [LibraryImport(LibName, EntryPoint = "zo_free")]
    internal static partial void Free(IntPtr ptr);

    /// <summary>
    /// Return the library version as a static UTF-8 string. Must NOT be freed.
    /// </summary>
    [LibraryImport(LibName, EntryPoint = "zo_version")]
    internal static partial IntPtr Version();
}

/// <summary>
/// C-compatible input struct for zo_query. Must exactly match the Rust
/// <c>zo_query_options</c> layout.
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

/// <summary>C-compatible stats struct for zo_stats.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct ZoStats
{
    public ulong Adds;
    public ulong Queries;
    public ulong Removes;
    public ulong Entries;
}
