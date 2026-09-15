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
/// Rust-allocated UTF-8 message to free with zo_free. Nothing is stored on
/// the session or in a global, so concurrent sessions can never clobber each
/// other's error. An empty message is still a failure (zoxide reports
/// fzf Ctrl-C that way), so callers test the pointer, never the text.
///
/// Strings returned by zo_session_query (results) and by any failing call
/// (error messages) must be freed with zo_free. zo_version strings are static
/// and must NOT be freed.
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
    /// Create a session. Returns NULL on success and writes the handle to
    /// <paramref name="session"/>; on failure returns an error message that
    /// must be freed with <see cref="Free"/> and leaves the handle NULL.
    /// </summary>
    [LibraryImport(LibName, EntryPoint = "zo_session_create")]
    internal static partial IntPtr SessionCreate(out IntPtr session);

    /// <summary>
    /// Destroy a session. Returns NULL on success, otherwise an error message
    /// to free with <see cref="Free"/> (diagnostic only: a panic while dropping
    /// the session is the only way teardown can fail). NULL is a successful
    /// no-op.
    /// </summary>
    [LibraryImport(LibName, EntryPoint = "zo_session_destroy")]
    internal static partial IntPtr SessionDestroy(IntPtr session);

    // ── Database operations ─────────────────────────────────────────────

    /// <summary>
    /// Add a directory to the database. Returns NULL on success, otherwise an
    /// error message to free with <see cref="Free"/>.
    /// </summary>
    [LibraryImport(LibName, EntryPoint = "zo_session_add", StringMarshalling = StringMarshalling.Utf8)]
    internal static partial IntPtr SessionAdd(IntPtr session, string path, double score);

    /// <summary>
    /// Remove a directory from the database. Returns NULL on success,
    /// otherwise an error message to free with <see cref="Free"/>.
    /// </summary>
    [LibraryImport(LibName, EntryPoint = "zo_session_remove", StringMarshalling = StringMarshalling.Utf8)]
    internal static partial IntPtr SessionRemove(IntPtr session, string path);

    /// <summary>
    /// Run a query. Returns NULL on success and writes a Rust-allocated UTF-8
    /// result to <paramref name="output"/> (free it with <see cref="Free"/>);
    /// on failure returns an error message and sets the output to NULL.
    /// </summary>
    [LibraryImport(LibName, EntryPoint = "zo_session_query")]
    internal static partial IntPtr SessionQuery(IntPtr session, IntPtr options, out IntPtr output);

    // ── Statistics and metadata ──────────────────────────────────────────

    /// <summary>
    /// Retrieve session counters. Returns NULL on success, else an error
    /// message to free with <see cref="Free"/>.
    /// </summary>
    [LibraryImport(LibName, EntryPoint = "zo_session_stats")]
    internal static partial IntPtr SessionStats(IntPtr session, out ZoStats stats);

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
