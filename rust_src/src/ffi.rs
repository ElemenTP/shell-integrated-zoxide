//! C FFI exports for zoxide in-process directory jumping.
//!
//! All public functions follow the `zo_` prefix convention and use C ABI.
//! Heap strings returned to C are owned by Rust and must be freed with
//! `zo_free`; `zo_version` returns a static string instead.

use libc::c_char;
use std::any::Any;
use std::ffi::{CStr, CString};
use std::os::raw::{c_double, c_int};
use std::path::PathBuf;
use std::ptr;
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------
//
// Error protocol: every fallible `zo_*` FFI function returns `*mut c_char`.
//   * NULL means success.
//   * A non-NULL value is a heap-allocated, NUL-terminated UTF-8 error string.
//     The caller owns it and must release it with `zo_free()`.
//   * An empty message is still an error with no text: zoxide reports
//     SilentExit (fzf Ctrl-C) that way, so callers test the pointer first and
//     print only when `*err` is non-zero.
//
// `zo_init` establishes the one process-global session; `zo_shutdown` drops
// it again. Every other fallible call returns its error string directly.
// `zo_free` cannot fail and therefore returns void.
//
// There is exactly one session per process. zoxide is a CLI whose state is
// meant to die with the process, and the shell modules are themselves
// singletons, so a multi-session API would only add handle plumbing without a
// real consumer. The global session is guarded by a `Mutex` so the FFI stays
// sound even if a host calls it from more than one thread; shell calls are
// serialized by the shell itself.
//
// Each call still carries its own error value directly in its return value, so
// no error state is shared between calls and there is no TLS destructor to
// dangle after dlclose().

/// Copy a Rust string into a C-owned NUL-terminated UTF-8 buffer.
///
/// Interior NUL bytes are truncated (paths and error messages never contain NUL
/// in practice, but the FFI boundary must stay total).
fn string_into_c(value: String) -> *mut c_char {
    CString::new(value)
        .unwrap_or_else(|e| {
            let pos = e.nul_position();
            let mut bytes = e.into_vec();
            bytes.truncate(pos);
            CString::new(bytes).unwrap()
        })
        .into_raw()
}

/// Build an allocated error string for `msg`.
fn error_string(msg: impl Into<String>) -> *mut c_char {
    string_into_c(msg.into())
}

/// Convert a caught panic into an allocated error string.
fn panic_to_error(panic: Box<dyn Any + Send>) -> *mut c_char {
    let msg = if let Some(s) = panic.downcast_ref::<&str>() {
        format!("panic: {s}")
    } else if let Some(s) = panic.downcast_ref::<String>() {
        format!("panic: {s}")
    } else {
        "panic: unknown error".to_string()
    };
    string_into_c(msg)
}

/// FFI panic guard for fallible exports.
///
/// The wrapped expression must itself return `*mut c_char` using the error
/// protocol above. Panics are caught and converted to an allocated error
/// string instead of unwinding across the C boundary.
macro_rules! ffi_guard_error {
    ($expr:expr) => {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| $expr)) {
            Ok(result) => result,
            Err(panic) => panic_to_error(panic),
        }
    };
}

// ---------------------------------------------------------------------------
// C-compatible types
// ---------------------------------------------------------------------------

/// Query parameters for `zo_query`.
#[repr(C)]
pub struct zo_query_options {
    /// Array of keyword strings (lowercased by zoxide itself).
    pub keywords: *const *const c_char,
    /// Number of entries in `keywords`.
    pub keywords_len: usize,
    /// Exclude this exact path from the result (NULL = don't exclude).
    pub exclude: *const c_char,
    /// Restrict results to this parent directory (NULL = no restriction).
    pub base_dir: *const c_char,
    /// Include directories that no longer exist.
    pub all: c_int,
    /// Run fzf and return the interactive selection.
    pub interactive: c_int,
    /// Return every match instead of only the best one.
    pub list: c_int,
    /// Prefix each result with its frecency score.
    pub score: c_int,
}

/// Session counters exposed to shell tooling.
#[repr(C)]
#[derive(Default)]
pub struct zo_stats {
    pub adds: u64,
    pub queries: u64,
    pub removes: u64,
    /// Number of entries currently in the in-memory database.
    pub entries: u64,
}

// ---------------------------------------------------------------------------
// Helper: convert C input to Rust
// ---------------------------------------------------------------------------

fn cstr_to_option_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the caller is required to pass a valid NUL-terminated string for
    // the duration of the call. The shell modules copy strings before calling.
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .ok()
        .map(str::to_string)
}

fn cstr_array_to_vec(ptr: *const *const c_char, len: usize) -> Option<Vec<String>> {
    if len == 0 {
        return Some(Vec::new());
    }
    if ptr.is_null() {
        return None;
    }
    let mut result = Vec::with_capacity(len);
    // SAFETY: `ptr` points to `len` valid `char *` values, all readable for the
    // duration of the FFI call (zsh argv / unmanaged pwsh memory).
    unsafe {
        for i in 0..len {
            let s = *ptr.add(i);
            if s.is_null() {
                return None;
            }
            if let Ok(s) = CStr::from_ptr(s).to_str() {
                result.push(s.to_string());
            } else {
                return None;
            }
        }
    }
    Some(result)
}

// ---------------------------------------------------------------------------
// Global session
// ---------------------------------------------------------------------------

/// The one process-global zoxide session.
///
/// `None` until `zo_init` is called (and again after `zo_shutdown`). The
/// session has no long-lived database handle, so dropping it only resets the
/// counters; `db.zo` itself is untouched.
static SESSION: Mutex<Option<zoxide::session::Session>> = Mutex::new(None);

/// Run `f` with the global session, or fail when `zo_init` has not been called.
///
/// The lock is held for the duration of one native operation. Shell hosts are
/// single-threaded, so this is serialization, not a concurrency feature; the
/// mutex is what makes the global state sound for a multi-threaded host.
fn with_session<T, E: std::fmt::Display>(
    f: impl FnOnce(&mut zoxide::session::Session) -> Result<T, E>,
) -> Result<T, String> {
    let mut guard = SESSION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(session) = guard.as_mut() else {
        return Err(format!("session is not initialized"));
    };
    f(session).map_err(|e| format!("{e:#}"))
}

// ---------------------------------------------------------------------------
// Public C API
// ---------------------------------------------------------------------------

/// Initialize the process-global zoxide session.
///
/// Idempotent: when the session already exists this is a successful no-op, so
/// module load / reload paths can call it unconditionally. Returns NULL on
/// success or an allocated error string (free it with `zo_free()`) when the
/// session cannot be created.
///
/// The session only tracks counters; each add/query/remove opens and closes
/// `db.zo` itself, so updates made by other processes are never overwritten by
/// stale in-memory state.
#[unsafe(no_mangle)]
pub extern "C" fn zo_init() -> *mut c_char {
    ffi_guard_error!({
        let mut guard = SESSION
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if guard.is_none() {
            match zoxide::session::Session::new() {
                Ok(session) => *guard = Some(session),
                Err(e) => return error_string(format!("{e:#}")),
            }
        }

        ptr::null_mut()
    })
}

/// Drop the process-global zoxide session.
///
/// Resets the counters held by the session; the on-disk database is untouched.
/// Calling this without a previous `zo_init` is a safe no-op.
#[unsafe(no_mangle)]
pub extern "C" fn zo_shutdown() -> *mut c_char {
    ffi_guard_error!({
        let mut guard = SESSION
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = None;
        ptr::null_mut()
    })
}

/// Add a directory to the database.
///
/// `score` is the frecency increment (use 1.0 for the standard shell hook).
/// Returns NULL on success or an allocated error string (free it with
/// `zo_free()`).
///
/// # Safety
///
/// `path` must be NULL or a valid NUL-terminated UTF-8 string for the duration
/// of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zo_add(path: *const c_char, score: c_double) -> *mut c_char {
    ffi_guard_error!({
        if path.is_null() {
            return error_string("null argument");
        }
        let Some(path) = cstr_to_option_string(path) else {
            return error_string("path is not valid UTF-8");
        };

        match with_session(|session| session.add(PathBuf::from(path), score)) {
            Ok(()) => ptr::null_mut(),
            Err(e) => error_string(e),
        }
    })
}

/// Remove a directory from the database.
///
/// Returns NULL on success or an allocated error string (free it with
/// `zo_free()`); a path that is not in the database is reported as an error,
/// like the original binary.
///
/// # Safety
///
/// `path` must be NULL or a valid NUL-terminated UTF-8 string for the duration
/// of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zo_remove(path: *const c_char) -> *mut c_char {
    ffi_guard_error!({
        if path.is_null() {
            return error_string("null argument");
        }
        let Some(path) = cstr_to_option_string(path) else {
            return error_string("path is not valid UTF-8");
        };

        match with_session(|session| session.remove(&path)) {
            Ok(()) => ptr::null_mut(),
            Err(e) => error_string(e),
        }
    })
}

/// Run a query against the database.
///
/// Returns NULL on success and writes a Rust-allocated UTF-8 result to `*out`
/// (free it with `zo_free()`). On failure returns an allocated error string
/// (also freed with `zo_free()`) and sets `*out` to NULL.
///
/// An empty message is a failure with no text: zoxide reports SilentExit
/// (fzf Ctrl-C) that way and callers must print nothing for it.
///
/// # Safety
///
/// `options` must be NULL or point to a valid `zo_query_options`, and `out`
/// must be NULL or point to writable `*mut c_char` storage for the duration of
/// this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zo_query(
    options: *const zo_query_options,
    out: *mut *mut c_char,
) -> *mut c_char {
    ffi_guard_error!({
        if out.is_null() {
            return error_string("null argument");
        }

        // Always reset the caller's output slot before any other validation.
        // On failure the caller may otherwise keep a stale/dangling pointer
        // from a previous call.
        // SAFETY: `out` was checked non-null above and is writable for the
        // duration of this call.
        unsafe { *out = ptr::null_mut() };

        if options.is_null() {
            return error_string("null argument");
        }

        // SAFETY: `options` points to a valid C struct for the duration of this
        // call. The struct is declared `#[repr(C)]` and the header matches
        // field-for-field.
        let options = unsafe { &*options };
        let Some(keywords) = cstr_array_to_vec(options.keywords, options.keywords_len) else {
            return error_string("invalid keywords array");
        };
        let exclude = if options.exclude.is_null() {
            None
        } else {
            let Some(exclude) = cstr_to_option_string(options.exclude) else {
                return error_string("exclude is not valid UTF-8");
            };
            Some(exclude)
        };
        let base_dir = if options.base_dir.is_null() {
            None
        } else {
            let Some(base_dir) = cstr_to_option_string(options.base_dir) else {
                return error_string("base_dir is not valid UTF-8");
            };
            Some(base_dir)
        };
        let query = zoxide::session::QueryOptions {
            keywords,
            exclude,
            base_dir,
            all: options.all != 0,
            interactive: options.interactive != 0,
            list: options.list != 0,
            score: options.score != 0,
        };

        match with_session(|session| session.query(query)) {
            Ok(result) => {
                // SAFETY: `out` is a writable `char *` slot (checked above).
                unsafe { *out = string_into_c(result) };
                ptr::null_mut()
            }
            Err(e) => error_string(e),
        }
    })
}

/// Retrieve session statistics.
///
/// Returns NULL on success and writes the snapshot to `*out`; otherwise returns
/// an allocated error string (free it with `zo_free()`).
///
/// `adds` / `queries` / `removes` are the process-global session counters
/// (reset by `zo_shutdown`); `entries` is read from the on-disk database for
/// this call.
///
/// # Safety
///
/// `out` must be NULL or point to writable `zo_stats` storage for the duration
/// of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zo_stats(out: *mut zo_stats) -> *mut c_char {
    ffi_guard_error!({
        if out.is_null() {
            return error_string("null argument");
        }

        let result = with_session(|session| {
            let zoxide::session::SessionStats {
                adds,
                queries,
                removes,
            } = session.stats();
            let entries = match session.entry_count() {
                Ok(entries) => entries as u64,
                Err(e) => return Err(e),
            };
            Ok(zo_stats {
                adds,
                queries,
                removes,
                entries,
            })
        });

        match result {
            Ok(stats) => {
                // SAFETY: `out` is writable for this call (checked above).
                unsafe { *out = stats };
                ptr::null_mut()
            }
            Err(e) => error_string(e),
        }
    })
}

/// Free a string previously returned by any fallible `zo_*` function: a query
/// result or an error message.
///
/// Passing NULL is safe (no-op). This function cannot fail, so it returns void
/// and is exempt from the error protocol.
///
/// # Safety
///
/// `ptr` must be NULL or a pointer previously returned by this library that has
/// not already been freed. Static strings from `zo_version()` must NOT be
/// passed here.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zo_free(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: callers must pass a pointer previously returned by this library.
    unsafe {
        let _ = CString::from_raw(ptr);
    }
}

/// Return the library version as a static string.
///
/// The returned pointer is valid for the lifetime of the process and must NOT
/// be freed. This accessor cannot fail, so it is exempt from the error
/// protocol.
#[unsafe(no_mangle)]
pub extern "C" fn zo_version() -> *const c_char {
    // A string literal has static storage duration and the trailing NUL is
    // included in the literal itself, so no LazyLock/allocation is needed.
    static VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");
    VERSION.as_ptr().cast()
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, sync::Mutex, thread};

    /// `_ZO_DATA_DIR` is a process-global environment variable and the session
    /// itself is process-global, so tests that touch them serialize with this
    /// lock. Call-local error tests need no lock.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Point `_ZO_DATA_DIR` at `data_dir` and start from a fresh global
    /// session. The returned guard must be kept alive for the whole test.
    fn setup(data_dir: &std::path::Path) -> std::sync::MutexGuard<'static, ()> {
        let guard = test_lock();
        unsafe {
            env::set_var("_ZO_DATA_DIR", data_dir.as_os_str());
        }
        assert_ok(zo_shutdown(), "zo_shutdown");
        assert_ok(zo_init(), "zo_init");
        guard
    }

    fn cstr(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    /// Read and free an allocated C error string.
    fn take_error(err: *mut c_char) -> String {
        assert!(!err.is_null(), "expected an error string");
        let msg = unsafe { CStr::from_ptr(err) }
            .to_string_lossy()
            .into_owned();
        unsafe { zo_free(err) };
        msg
    }

    /// Assert that a call succeeded (it returned NULL).
    fn assert_ok(err: *mut c_char, what: &str) {
        assert!(err.is_null(), "{what} failed: {}", take_error(err));
    }

    /// Assert that a call failed with a message containing `needle`.
    fn assert_error(err: *mut c_char, needle: &str) {
        let msg = take_error(err);
        assert!(
            msg.contains(needle),
            "error {msg:?} does not contain {needle:?}"
        );
    }

    /// Test that version returns a non-null static string.
    #[test]
    fn test_version() {
        let version = zo_version();
        assert!(!version.is_null());
        let text = unsafe { CStr::from_ptr(version) }.to_str().unwrap();
        assert!(!text.is_empty());
    }

    /// Test that zo_free handles NULL safely.
    #[test]
    fn test_free_null_is_safe() {
        unsafe { zo_free(ptr::null_mut()) };
    }

    /// Test that the silent-exit contract holds: an empty message is still an
    /// error (a non-NULL pointer), not success.
    #[test]
    fn test_empty_error_message_is_not_success() {
        let err = error_string("");
        assert!(!err.is_null());
        let msg = take_error(err);
        assert!(msg.is_empty(), "expected an empty message, got {msg:?}");
    }

    /// Test that initializing twice is a successful no-op that preserves the
    /// existing session state.
    #[test]
    fn test_init_is_idempotent() {
        let _guard = test_lock();
        let data_dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        unsafe {
            env::set_var("_ZO_DATA_DIR", data_dir.path().as_os_str());
        }
        assert_ok(zo_shutdown(), "zo_shutdown");
        assert_ok(zo_init(), "first zo_init");

        let path = cstr(target.path().to_str().unwrap());
        assert_ok(unsafe { zo_add(path.as_ptr(), 1.0) }, "zo_add");

        assert_ok(zo_init(), "second zo_init");
        let mut stats = zo_stats::default();
        assert_ok(unsafe { zo_stats(&mut stats as *mut _) }, "zo_stats");
        assert_eq!(stats.adds, 1, "a second zo_init must not reset counters");

        assert_ok(zo_shutdown(), "zo_shutdown");
    }

    /// Test that shutting down twice (and before any init) is safe.
    #[test]
    fn test_shutdown_is_safe() {
        let _guard = test_lock();
        assert_ok(zo_shutdown(), "zo_shutdown");
        assert_ok(zo_shutdown(), "zo_shutdown");
        assert_ok(zo_init(), "zo_init");
        assert_ok(zo_shutdown(), "zo_shutdown");
        assert_ok(zo_shutdown(), "zo_shutdown");
    }

    /// Test that data operations refuse to run before `zo_init`.
    #[test]
    fn test_operations_require_init() {
        let _guard = test_lock();
        assert_ok(zo_shutdown(), "zo_shutdown");

        let path = cstr("/tmp");
        assert_error(
            unsafe { zo_add(path.as_ptr(), 1.0) },
            "session is not initialized",
        );
        assert_error(
            unsafe { zo_remove(path.as_ptr()) },
            "session is not initialized",
        );

        let options = zo_query_options {
            keywords: ptr::null(),
            keywords_len: 0,
            exclude: ptr::null(),
            base_dir: ptr::null(),
            all: 0,
            interactive: 0,
            list: 0,
            score: 0,
        };
        let mut out: *mut c_char = ptr::null_mut();
        assert_error(
            unsafe { zo_query(&options as *const _, &mut out) },
            "session is not initialized",
        );

        let mut stats = zo_stats::default();
        assert_error(
            unsafe { zo_stats(&mut stats as *mut _) },
            "session is not initialized",
        );
    }

    /// Test that NULL arguments are reported by the call itself.
    #[test]
    fn test_null_arguments_return_errors() {
        let data_dir = tempfile::tempdir().unwrap();
        let _guard = setup(data_dir.path());
        let mut out: *mut c_char = ptr::null_mut();
        let mut stats = zo_stats::default();

        assert_error(unsafe { zo_add(ptr::null(), 1.0) }, "null argument");
        assert_error(unsafe { zo_remove(ptr::null()) }, "null argument");
        assert_error(unsafe { zo_query(ptr::null(), &mut out) }, "null argument");
        assert_error(unsafe { zo_stats(ptr::null_mut()) }, "null argument");
        assert_error(
            unsafe { zo_query(ptr::null(), ptr::null_mut()) },
            "null argument",
        );
        assert_error(unsafe { zo_query(ptr::null(), &mut out) }, "null argument");

        // A valid stats target must still work after the failure paths above.
        assert_ok(unsafe { zo_stats(&mut stats as *mut _) }, "zo_stats");
    }

    /// Test a full add/query roundtrip.
    #[test]
    fn test_add_query_roundtrip() {
        let data_dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let _guard = setup(data_dir.path());
        let path = cstr(target.path().to_str().unwrap());

        assert_ok(unsafe { zo_add(path.as_ptr(), 1.0) }, "zo_add");

        let keyword = cstr(target.path().file_name().unwrap().to_str().unwrap());
        let keywords = [keyword.as_ptr()];
        let options = zo_query_options {
            keywords: keywords.as_ptr(),
            keywords_len: keywords.len(),
            exclude: ptr::null(),
            base_dir: ptr::null(),
            all: 0,
            interactive: 0,
            list: 0,
            score: 0,
        };

        let mut out: *mut c_char = ptr::null_mut();
        let err = unsafe { zo_query(&options as *const _, &mut out) };
        assert_ok(err, "zo_query");
        assert!(!out.is_null(), "query output should be non-null");
        let result = unsafe { CStr::from_ptr(out) }.to_str().unwrap();
        assert!(result.ends_with(target.path().file_name().unwrap().to_str().unwrap()));
        unsafe { zo_free(out) };
    }

    /// Test `list` queries and the stats snapshot.
    #[test]
    fn test_query_list_and_stats() {
        let data_dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let _guard = setup(data_dir.path());
        let path = cstr(target.path().to_str().unwrap());
        assert_ok(unsafe { zo_add(path.as_ptr(), 1.0) }, "zo_add");

        let options = zo_query_options {
            keywords: ptr::null(),
            keywords_len: 0,
            exclude: ptr::null(),
            base_dir: ptr::null(),
            all: 1,
            interactive: 0,
            list: 1,
            score: 0,
        };
        let mut out: *mut c_char = ptr::null_mut();
        let err = unsafe { zo_query(&options as *const _, &mut out) };
        assert_ok(err, "zo_query");
        assert!(!out.is_null(), "query output should be non-null");
        unsafe { zo_free(out) };

        let mut stats = zo_stats::default();
        assert_ok(unsafe { zo_stats(&mut stats as *mut _) }, "zo_stats");
        assert_eq!(stats.adds, 1);
        assert_eq!(stats.queries, 1);
        assert_eq!(stats.entries, 1);
    }

    /// Test that removing an entry drops it from the database.
    #[test]
    fn test_remove_roundtrip() {
        let data_dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let _guard = setup(data_dir.path());
        let path = cstr(target.path().to_str().unwrap());

        assert_ok(unsafe { zo_add(path.as_ptr(), 1.0) }, "zo_add");
        assert_ok(unsafe { zo_remove(path.as_ptr()) }, "zo_remove");

        let mut stats = zo_stats::default();
        assert_ok(unsafe { zo_stats(&mut stats as *mut _) }, "zo_stats");
        assert_eq!(stats.entries, 0);
    }

    /// Test that conflicting query modes are reported by the call itself.
    #[test]
    fn test_query_conflicting_modes() {
        let data_dir = tempfile::tempdir().unwrap();
        let _guard = setup(data_dir.path());
        let options = zo_query_options {
            keywords: ptr::null(),
            keywords_len: 0,
            exclude: ptr::null(),
            base_dir: ptr::null(),
            all: 0,
            interactive: 1,
            list: 1,
            score: 0,
        };
        let mut out: *mut c_char = ptr::null_mut();
        let err = unsafe { zo_query(&options as *const _, &mut out) };
        assert_error(err, "cannot be used together");
    }

    /// Test that non-UTF-8 arguments are rejected before touching the database.
    #[test]
    fn test_invalid_utf8_arguments() {
        let data_dir = tempfile::tempdir().unwrap();
        let _guard = setup(data_dir.path());
        let invalid = CString::new(vec![0xff, b'x']).unwrap();

        assert_error(
            unsafe { zo_add(invalid.as_ptr(), 1.0) },
            "path is not valid UTF-8",
        );
        assert_error(
            unsafe { zo_remove(invalid.as_ptr()) },
            "path is not valid UTF-8",
        );

        let options = zo_query_options {
            keywords: ptr::null(),
            keywords_len: 0,
            exclude: invalid.as_ptr(),
            base_dir: ptr::null(),
            all: 0,
            interactive: 0,
            list: 0,
            score: 0,
        };
        let mut out: *mut c_char = ptr::null_mut();
        assert_error(
            unsafe { zo_query(&options as *const _, &mut out) },
            "exclude is not valid UTF-8",
        );
    }

    /// Test that a failed query clears the caller's output slot.
    #[test]
    fn test_query_failure_clears_output_slot() {
        let data_dir = tempfile::tempdir().unwrap();
        let _guard = setup(data_dir.path());

        let options = zo_query_options {
            keywords: ptr::null(),
            keywords_len: 0,
            exclude: ptr::null(),
            base_dir: ptr::null(),
            all: 0,
            interactive: 0,
            list: 0,
            score: 0,
        };
        let sentinel = cstr("stale-pointer");
        let mut out: *mut c_char = sentinel.as_ptr() as *mut c_char;
        // No matching entry: the query fails and must clear the caller's slot.
        let err = unsafe { zo_query(&options as *const _, &mut out) };
        assert_error(err, "");
        assert!(
            out.is_null(),
            "a failed query must clear the caller's output slot"
        );
    }

    /// Test that a NULL keyword array with a positive length is rejected.
    #[test]
    fn test_null_keyword_array_with_positive_len_is_rejected() {
        let data_dir = tempfile::tempdir().unwrap();
        let _guard = setup(data_dir.path());

        let options = zo_query_options {
            keywords: ptr::null(),
            keywords_len: 1,
            exclude: ptr::null(),
            base_dir: ptr::null(),
            all: 0,
            interactive: 0,
            list: 0,
            score: 0,
        };
        let mut out: *mut c_char = ptr::null_mut();
        assert_error(
            unsafe { zo_query(&options as *const _, &mut out) },
            "invalid keywords array",
        );
    }

    /// Test that every call carries its own error: a failure does not stick to
    /// the global session and a later success is simply NULL.
    #[test]
    fn test_errors_are_call_local() {
        let data_dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let _guard = setup(data_dir.path());
        let invalid = CString::new(vec![0xff, b'x']).unwrap();
        let path = cstr(target.path().to_str().unwrap());

        assert_error(unsafe { zo_add(invalid.as_ptr(), 1.0) }, "not valid UTF-8");
        assert_error(unsafe { zo_add(ptr::null(), 1.0) }, "null argument");
        assert_ok(unsafe { zo_add(path.as_ptr(), 1.0) }, "zo_add");

        // A later, unrelated failure still reports its own message.
        let options = zo_query_options {
            keywords: ptr::null(),
            keywords_len: 0,
            exclude: ptr::null(),
            base_dir: ptr::null(),
            all: 0,
            interactive: 1,
            list: 1,
            score: 0,
        };
        let mut out: *mut c_char = ptr::null_mut();
        assert_error(
            unsafe { zo_query(&options as *const _, &mut out) },
            "cannot be used together",
        );
    }

    /// Test that `zo_shutdown` resets the counters while the on-disk database
    /// is untouched.
    #[test]
    fn test_shutdown_resets_counters() {
        let first_dir = tempfile::tempdir().unwrap();
        let second_dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let _guard = setup(first_dir.path());
        let path = cstr(target.path().to_str().unwrap());

        assert_ok(unsafe { zo_add(path.as_ptr(), 1.0) }, "zo_add");
        let mut stats = zo_stats::default();
        assert_ok(unsafe { zo_stats(&mut stats as *mut _) }, "zo_stats");
        assert_eq!(stats.adds, 1);
        assert_eq!(stats.entries, 1);

        assert_ok(zo_shutdown(), "zo_shutdown");
        unsafe {
            env::set_var("_ZO_DATA_DIR", second_dir.path().as_os_str());
        }
        assert_ok(zo_init(), "zo_init after shutdown");

        let mut stats = zo_stats::default();
        assert_ok(unsafe { zo_stats(&mut stats as *mut _) }, "zo_stats");
        assert_eq!(stats.adds, 0, "counters must reset on shutdown");
        assert_eq!(stats.queries, 0);
        assert_eq!(stats.removes, 0);
        assert_eq!(stats.entries, 0, "a fresh data dir must report no entries");

        assert_ok(zo_shutdown(), "zo_shutdown");
    }

    /// Destroying and recreating the global session must leave the database
    /// fully usable: counters start over, but commands still read and write
    /// `db.zo`.
    #[test]
    fn test_reinit_after_shutdown_reuses_database() {
        let data_dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let _guard = setup(data_dir.path());
        let path = cstr(target.path().to_str().unwrap());
        let keyword = cstr(target.path().file_name().unwrap().to_str().unwrap());

        assert_ok(unsafe { zo_add(path.as_ptr(), 1.0) }, "zo_add");
        assert_ok(zo_shutdown(), "zo_shutdown");
        assert_ok(zo_init(), "zo_init after shutdown");

        // Counters reset, but the on-disk entry is still there.
        let mut stats = zo_stats::default();
        assert_ok(unsafe { zo_stats(&mut stats as *mut _) }, "zo_stats");
        assert_eq!(stats.adds, 0);
        assert_eq!(stats.entries, 1, "database on disk must survive shutdown");

        // Queries and removes work after the session was recreated.
        let keywords = [keyword.as_ptr()];
        let options = zo_query_options {
            keywords: keywords.as_ptr(),
            keywords_len: keywords.len(),
            exclude: ptr::null(),
            base_dir: ptr::null(),
            all: 0,
            interactive: 0,
            list: 0,
            score: 0,
        };
        let mut out: *mut c_char = ptr::null_mut();
        assert_ok(unsafe { zo_query(&options, &mut out) }, "zo_query");
        assert!(!out.is_null());
        unsafe { zo_free(out) };

        assert_ok(unsafe { zo_remove(path.as_ptr()) }, "zo_remove");

        let mut stats = zo_stats::default();
        assert_ok(unsafe { zo_stats(&mut stats as *mut _) }, "zo_stats");
        assert_eq!(stats.queries, 1);
        assert_eq!(stats.removes, 1);
        assert_eq!(stats.entries, 0);

        assert_ok(zo_shutdown(), "zo_shutdown");
    }

    /// Invalid calls are rejected before touching the global session, so they
    /// can run concurrently and each still receives its own error string.
    #[test]
    fn test_concurrent_invalid_calls_get_their_own_error() {
        const ITERATIONS: usize = 200;

        thread::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..ITERATIONS {
                    assert_error(unsafe { zo_add(ptr::null(), 1.0) }, "null argument");
                }
            });
            scope.spawn(|| {
                for _ in 0..ITERATIONS {
                    assert_error(unsafe { zo_remove(ptr::null()) }, "null argument");
                }
            });
        });
    }
}
