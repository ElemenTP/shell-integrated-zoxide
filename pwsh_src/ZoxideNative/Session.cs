using System.Runtime.InteropServices;
using System.Text;

namespace ZoxideNative;

/// <summary>
/// Result of a native zoxide query. Query failures are returned as values
/// rather than thrown exceptions so the PowerShell module can reproduce the
/// original `zoxide` binary behavior (write a short error to stderr and
/// continue the shell session).
/// </summary>
public sealed class QueryResult
{
    public QueryResult(bool success, string? output, string? error)
    {
        Success = success;
        Output = output ?? string.Empty;
        Error = error;
    }

    /// <summary>True when zo_query succeeded (returned no error).</summary>
    public bool Success { get; }

    /// <summary>Query output; empty when <see cref="Success"/> is false.</summary>
    public string Output { get; }

    /// <summary>Native error message; null/empty for silent exits (Ctrl-C).</summary>
    public string? Error { get; }
}

/// <summary>
/// Managed wrapper around the single process-global zoxide-ffi session.
///
/// zoxide is a CLI whose state is meant to die with the process, and one pwsh
/// process loads this module once, so the native side keeps exactly one
/// session instead of handing out handles. <see cref="Initialize"/> creates it
/// (idempotent), <see cref="Shutdown"/> drops it and resets its counters, and
/// the data operations work on the global session directly.
///
/// Each Add / Query / Remove call opens and closes the zoxide database,
/// matching the original binary's cross-process behavior. All calls are
/// single-threaded; no fork guard or thread-pool shutdown is needed.
///
/// Error model: every native call returns its error message (NULL on success)
/// and the wrapper converts it into an exception (Add / Remove /
/// GetStatsReport) or into <see cref="QueryResult.Error"/> (Query). Errors are
/// local to the call, so there is no stored error state.
/// </summary>
public static class Session
{
    /// <summary>
    /// Initialize the process-global native session. Idempotent; safe to call
    /// again when the module is re-imported.
    /// </summary>
    public static void Initialize()
    {
        IntPtr error = NativeMethods.Init();
        if (error != IntPtr.Zero)
        {
            throw new InvalidOperationException(
                $"Failed to initialize zoxide session: {TakeError(error)}");
        }
    }

    /// <summary>
    /// Drop the process-global native session and reset its counters. The
    /// on-disk database is untouched.
    /// </summary>
    public static void Shutdown()
    {
        IntPtr error = NativeMethods.Shutdown();
        if (error != IntPtr.Zero)
        {
            throw new InvalidOperationException(
                $"Failed to shutdown zoxide session: {TakeError(error)}");
        }
    }

    /// <summary>Add a directory to the database.</summary>
    public static void Add(string path, double score = 1.0)
    {
        ArgumentException.ThrowIfNullOrEmpty(path);

        // Every call returns its own error message; nothing is stored.
        IntPtr error = NativeMethods.Add(path, score);
        if (error != IntPtr.Zero)
        {
            throw new InvalidOperationException(
                $"zoxide add failed: {TakeError(error)}");
        }
    }

    /// <summary>Remove a directory from the database.</summary>
    public static void Remove(string path)
    {
        ArgumentException.ThrowIfNullOrEmpty(path);

        IntPtr error = NativeMethods.Remove(path);
        if (error != IntPtr.Zero)
        {
            throw new InvalidOperationException(
                $"zoxide remove failed: {TakeError(error)}");
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
    /// <returns>
    /// A <see cref="QueryResult"/>. On native failure <c>Success</c> is false,
    /// <c>Output</c> is empty and <c>Error</c> contains the native error
    /// message (empty for silent exits such as fzf Ctrl-C).
    /// </returns>
    public static QueryResult Query(
        IReadOnlyList<string>? keywords = null,
        string? exclude = null,
        string? baseDir = null,
        bool all = false,
        bool interactive = false,
        bool list = false,
        bool score = false)
    {
        using var input = BuildInput(keywords, exclude, baseDir, all,
                                      interactive, list, score);
        try
        {
            IntPtr optionsPtr = Marshal.AllocHGlobal(Marshal.SizeOf<ZoQueryOptions>());
            try
            {
                Marshal.StructureToPtr(input.Options, optionsPtr, false);
                IntPtr error = NativeMethods.Query(optionsPtr, out IntPtr output);
                if (error != IntPtr.Zero)
                {
                    // The failing call hands back its own message; an empty
                    // message means a silent exit (fzf Ctrl-C).
                    return new QueryResult(false, null, TakeError(error));
                }
                if (output == IntPtr.Zero)
                {
                    return new QueryResult(false, null, "query returned no output");
                }

                try
                {
                    return new QueryResult(
                        true, Marshal.PtrToStringUTF8(output) ?? string.Empty, null);
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

    /// <summary>
    /// Consume an error pointer returned by a native call: NULL means success
    /// (returns null), otherwise the message is copied into a managed string
    /// and the native allocation is released with zo_free.
    /// </summary>
    private static string? TakeError(IntPtr error)
    {
        if (error == IntPtr.Zero)
            return null;
        string? message = Marshal.PtrToStringUTF8(error);
        NativeMethods.Free(error);
        return message;
    }

    /// <summary>Get a human-readable session statistics report.</summary>
    public static string GetStatsReport()
    {
        IntPtr error = NativeMethods.Stats(out var stats);
        if (error != IntPtr.Zero)
        {
            throw new InvalidOperationException(
                $"zoxide stats failed: {TakeError(error)}");
        }

        return $"Adds: {stats.Adds}, Queries: {stats.Queries}, " +
               $"Removes: {stats.Removes}, Entries: {stats.Entries}";
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
