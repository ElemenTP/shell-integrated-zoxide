using System.Runtime.InteropServices;
using System.Text;

namespace ZoxideNative;

/// <summary>
/// Safe managed wrapper around the zoxide-ffi native session.
///
/// The session keeps the zoxide database open in unmanaged memory for the
/// lifetime of the pwsh process. All calls are single-threaded; no fork guard
/// or thread-pool shutdown is needed.
/// </summary>
public sealed class Session : IDisposable
{
    private IntPtr _handle;
    private bool _disposed;

    /// <summary>
    /// Create a session.
    /// </summary>
    public Session()
    {
        _handle = NativeMethods.SessionCreate();
        if (_handle == IntPtr.Zero)
        {
            string? err = LastError();
            throw new InvalidOperationException(
                $"Failed to create zoxide session: {err ?? "unknown error"}");
        }
    }

    /// <summary>Add a directory to the database.</summary>
    public void Add(string path, double score = 1.0)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        ArgumentException.ThrowIfNullOrEmpty(path);
 
        int rc = NativeMethods.SessionAdd(_handle, path, score);
        if (rc != 0)
        {
            string? err = LastError();
            throw new InvalidOperationException(
                $"zoxide add failed (rc={rc}): {err ?? "unknown error"}");
        }
    }

    /// <summary>Remove a directory from the database.</summary>
    public void Remove(string path)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        ArgumentException.ThrowIfNullOrEmpty(path);

        int rc = NativeMethods.SessionRemove(_handle, path);
        if (rc != 0)
        {
            string? err = LastError();
            throw new InvalidOperationException(
                $"zoxide remove failed (rc={rc}): {err ?? "unknown error"}");
        }
    }

    /// <summary>
    /// Run a zoxide query.
    /// </summary>
    /// <param name="keywords">Query keywords, or null/empty for every entry.</param>
    /// <param name="exclude">Exact path to exclude, or null.</param>
    /// <param name="baseDir">Restrict results below this directory, or null.</param>
    /// <param name="all">Include directories that no longer exist.</param>
    /// <param name="interactive">Run fzf and return the selected path.</param>
    /// <param name="list">Return every match, joined with newlines.</param>
    /// <param name="score">Prefix each result with its frecency score.</param>
    /// <returns>The query result (path, or lines for list mode).</returns>
    public string Query(
        IReadOnlyList<string>? keywords = null,
        string? exclude = null,
        string? baseDir = null,
        bool all = false,
        bool interactive = false,
        bool list = false,
        bool score = false)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);

        using var input = BuildInput(keywords, exclude, baseDir, all,
                                      interactive, list, score);
        try
        {
            IntPtr optionsPtr = Marshal.AllocHGlobal(Marshal.SizeOf<ZoQueryOptions>());
            try
            {
                Marshal.StructureToPtr(input.Options, optionsPtr, false);
                int rc = NativeMethods.SessionQuery(_handle, optionsPtr, out IntPtr output);
                if (rc != 0 || output == IntPtr.Zero)
                {
                    string? err = LastError();
                    throw new InvalidOperationException(
                        $"zoxide query failed (rc={rc}): {err ?? "unknown error"}");
                }

                try
                {
                    return Marshal.PtrToStringUTF8(output) ?? string.Empty;
                }
                finally
                {
                    NativeMethods.Free(output);
                }
            }
            finally
            {
                Marshal.FreeHGlobal(optionsPtr);
            }
        }
        finally
        {
            input.Dispose();
        }
    }

    /// <summary>Get the native library version string.</summary>
    public static string Version()
    {
        IntPtr ptr = NativeMethods.Version();
        return Marshal.PtrToStringUTF8(ptr) ?? "unknown";
    }

    /// <summary>Get the last native error, or null if none.</summary>
    public static string? LastError()
    {
        NativeMethods.LastError(out IntPtr ptr);
        if (ptr == IntPtr.Zero)
            return null;
        string? ret = Marshal.PtrToStringUTF8(ptr);
        NativeMethods.Free(ptr);
        return ret;
    }

    /// <summary>Get a human-readable session statistics report.</summary>
    public string GetStatsReport()
    {
        ObjectDisposedException.ThrowIf(_disposed, this);

        int rc = NativeMethods.SessionStats(_handle, out var stats);
        if (rc != 0)
        {
            string? err = LastError();
            throw new InvalidOperationException(
                $"zoxide stats failed: {err ?? "unknown error"}");
        }

        return $"Adds: {stats.Adds}, Queries: {stats.Queries}, " +
               $"Removes: {stats.Removes}, Entries: {stats.Entries}";
    }

    public void Dispose()
    {
        if (!_disposed && _handle != IntPtr.Zero)
        {
            NativeMethods.SessionDestroy(_handle);
            _handle = IntPtr.Zero;
        }
        _disposed = true;
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    private static NativeInput BuildInput(
        IReadOnlyList<string>? keywords,
        string? exclude,
        string? baseDir,
        bool all,
        bool interactive,
        bool list,
        bool score)
    {
        var native = new NativeInput();
        try
        {
            native.Options = new ZoQueryOptions
            {
                Keywords = native.AllocUtf8Array(keywords),
                KeywordsLen = (UIntPtr)(keywords?.Count ?? 0),
                Exclude = native.AllocUtf8(exclude),
                BaseDir = native.AllocUtf8(baseDir),
                All = all ? 1 : 0,
                Interactive = interactive ? 1 : 0,
                List = list ? 1 : 0,
                Score = score ? 1 : 0,
            };
            return native;
        }
        catch
        {
            native.Dispose();
            throw;
        }
    }

    /// <summary>
    /// Owns the unmanaged memory backing a <see cref="ZoQueryOptions"/> and
    /// frees every block on <see cref="Dispose"/>. Strings are copied into
    /// unmanaged memory with <see cref="Marshal.AllocHGlobal"/>, so the native
    /// call can read them without GC pinning.
    /// </summary>
    private sealed class NativeInput : IDisposable
    {
        private readonly List<IntPtr> _blocks = new();

        public ZoQueryOptions Options;

        /// <summary>Copy a null-terminated UTF-8 string into unmanaged memory. Null-safe.</summary>
        public IntPtr AllocUtf8(string? s)
        {
            if (s == null) return IntPtr.Zero;
            byte[] bytes = Encoding.UTF8.GetBytes(s + '\0');
            IntPtr ptr = Marshal.AllocHGlobal(bytes.Length);
            _blocks.Add(ptr);
            Marshal.Copy(bytes, 0, ptr, bytes.Length);
            return ptr;
        }

        /// <summary>Copy strings into an unmanaged array of char* pointers. Null-safe.</summary>
        public IntPtr AllocUtf8Array(IReadOnlyList<string>? values)
        {
            if (values is not { Count: > 0 }) return IntPtr.Zero;

            IntPtr array = Marshal.AllocHGlobal(IntPtr.Size * values.Count);
            _blocks.Add(array);
            for (int i = 0; i < values.Count; i++)
            {
                Marshal.WriteIntPtr(array, i * IntPtr.Size, AllocUtf8(values[i]));
            }
            return array;
        }

        public void Dispose()
        {
            foreach (IntPtr ptr in _blocks)
                Marshal.FreeHGlobal(ptr);
            _blocks.Clear();
        }
    }
}
