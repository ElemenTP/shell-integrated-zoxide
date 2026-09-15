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
// `zo_session_create` is the one fallible call whose payload is not a string:
// the handle goes through an out-parameter and the error is the return value.
// `zo_session_destroy` also returns an error string so a failure during
// teardown is visible for debugging; `zo_free` cannot fail and therefore
// returns void.
//
// This deliberately eliminates all process-global / per-session error slots.
// Each call carries its own error value directly in its return value, so there
// is no shared mutable state to race on and no TLS destructor to dangle after
// dlclose().

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

/// Query parameters for `zo_session_query`.
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
// Session wrapper
// ---------------------------------------------------------------------------

/// Opaque session handle passed to C code.
///
/// Holds only the zoxide session: errors are carried by each call's return
/// value, so no error state is stored here.
pub struct SessionHandle {
    session: zoxide::session::Session,
}

// ---------------------------------------------------------------------------
// Public C API
// ---------------------------------------------------------------------------

/// Create a new zoxide session handle.
///
/// Writes the new handle to `*out` on success and returns NULL. On failure
/// `*out` is set to NULL and an allocated error string is returned; free it
/// with `zo_free()`.
///
/// The handle only tracks per-session counters: each add/query/remove opens and
/// closes `db.zo` itself, so updates made by other shells are never overwritten
/// by stale in-memory state.
///
/// # Safety
///
/// `out` must be NULL or point to writable `*mut SessionHandle` storage for the
/// duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zo_session_create(out: *mut *mut SessionHandle) -> *mut c_char {
    ffi_guard_error!({
        if out.is_null() {
            return error_string("zo_session_create: out is NULL");
        }
        // SAFETY: `out` was checked non-null and is writable for this call.
        unsafe { *out = ptr::null_mut() };

        let session = match zoxide::session::Session::new() {
            Ok(session) => session,
            Err(e) => return error_string(format!("{e:#}")),
        };

        // SAFETY: `out` is writable for this call.
        unsafe { *out = Box::into_raw(Box::new(SessionHandle { session })) };
        ptr::null_mut()
    })
}

/// Destroy a session previously created with `zo_session_create`.
///
/// Returns NULL on success or an allocated error string (free it with
/// `zo_free()`). The message is diagnostic only — a panic while dropping the
/// session is the only way teardown can fail, and the caller has nothing to
/// retry — but it makes such a bug visible instead of silent. Passing NULL is a
/// successful no-op.
///
/// # Safety
///
/// `handle` must be NULL or a live handle returned by `zo_session_create` that
/// has not already been destroyed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zo_session_destroy(handle: *mut SessionHandle) -> *mut c_char {
    ffi_guard_error!({
        if handle.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: `handle` came from `zo_session_create` and is destroyed once.
        unsafe {
            let _ = Box::from_raw(handle);
        }
        ptr::null_mut()
    })
}

/// Add a directory to the session database.
///
/// `score` is the frecency increment (use 1.0 for the standard shell hook).
/// Returns NULL on success or an allocated error string (free it with
/// `zo_free()`).
///
/// # Safety
///
/// `handle` must be a live session handle, and `path` must be NULL or a valid
/// NUL-terminated UTF-8 string for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zo_session_add(
    handle: *mut SessionHandle,
    path: *const c_char,
    score: c_double,
) -> *mut c_char {
    ffi_guard_error!({
        if handle.is_null() {
            return error_string("zo_session_add: session is NULL");
        }
        if path.is_null() {
            return error_string("zo_session_add: path is NULL");
        }
        let Some(path) = cstr_to_option_string(path) else {
            return error_string("zo_session_add: path is not valid UTF-8");
        };

        // SAFETY: `handle` is non-null and live for this call.
        let session = unsafe { &mut *handle };
        if let Err(e) = session.session.add(PathBuf::from(path), score) {
            return error_string(format!("{e:#}"));
        }
        ptr::null_mut()
    })
}

/// Remove a directory from the session database.
///
/// Returns NULL on success or an allocated error string (free it with
/// `zo_free()`); a path that is not in the database is reported as an error,
/// like the original binary.
///
/// # Safety
///
/// `handle` must be a live session handle, and `path` must be NULL or a valid
/// NUL-terminated UTF-8 string for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zo_session_remove(
    handle: *mut SessionHandle,
    path: *const c_char,
) -> *mut c_char {
    ffi_guard_error!({
        if handle.is_null() {
            return error_string("zo_session_remove: session is NULL");
        }
        if path.is_null() {
            return error_string("zo_session_remove: path is NULL");
        }
        let Some(path) = cstr_to_option_string(path) else {
            return error_string("zo_session_remove: path is not valid UTF-8");
        };

        // SAFETY: `handle` is non-null and live for this call.
        let session = unsafe { &mut *handle };
        if let Err(e) = session.session.remove(&path) {
            return error_string(format!("{e:#}"));
        }
        ptr::null_mut()
    })
}

/// Run a query against the session database.
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
/// `handle` must be a live session handle, `options` must be NULL or point to a
/// valid `zo_query_options`, and `out` must be NULL or point to writable
/// `*mut c_char` storage for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zo_session_query(
    handle: *mut SessionHandle,
    options: *const zo_query_options,
    out: *mut *mut c_char,
) -> *mut c_char {
    ffi_guard_error!({
        if out.is_null() {
            return error_string("zo_session_query: out is NULL");
        }

        // Always reset the caller's output slot before any other validation.
        // On failure the caller may otherwise keep a stale/dangling pointer
        // from a previous call.
        // SAFETY: `out` was checked non-null above and is writable for the
        // duration of this call.
        unsafe { *out = ptr::null_mut() };

        if handle.is_null() {
            return error_string("zo_session_query: session is NULL");
        }
        if options.is_null() {
            return error_string("zo_session_query: options is NULL");
        }

        // SAFETY: `options` points to a valid C struct for the duration of this
        // call. The struct is declared `#[repr(C)]` and the header matches
        // field-for-field.
        let options = unsafe { &*options };
        let Some(keywords) = cstr_array_to_vec(options.keywords, options.keywords_len) else {
            return error_string("zo_session_query: invalid keywords array");
        };
        let exclude = if options.exclude.is_null() {
            None
        } else {
            let Some(exclude) = cstr_to_option_string(options.exclude) else {
                return error_string("zo_session_query: exclude is not valid UTF-8");
            };
            Some(exclude)
        };
        let base_dir = if options.base_dir.is_null() {
            None
        } else {
            let Some(base_dir) = cstr_to_option_string(options.base_dir) else {
                return error_string("zo_session_query: base_dir is not valid UTF-8");
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

        let result = {
            // SAFETY: `handle` is non-null and live for this call.
            let session = unsafe { &mut *handle };
            session.session.query(query)
        };
        match result {
            Ok(result) => {
                // SAFETY: `out` is a writable `char *` slot (checked above).
                unsafe { *out = string_into_c(result) };
                ptr::null_mut()
            }
            Err(e) => error_string(format!("{e:#}")),
        }
    })
}

/// Retrieve session statistics.
///
/// Returns NULL on success and writes the snapshot to `*out`; otherwise returns
/// an allocated error string (free it with `zo_free()`).
///
/// # Safety
///
/// `handle` must be a live session handle, and `out` must be NULL or point to
/// writable `zo_stats` storage for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zo_session_stats(
    handle: *mut SessionHandle,
    out: *mut zo_stats,
) -> *mut c_char {
    ffi_guard_error!({
        if handle.is_null() {
            return error_string("zo_session_stats: session is NULL");
        }
        if out.is_null() {
            return error_string("zo_session_stats: out is NULL");
        }
        // SAFETY: `handle` is non-null and live for this call.
        let session = unsafe { &*handle };
        let zoxide::session::SessionStats {
            adds,
            queries,
            removes,
        } = session.session.stats();
        let entries = match session.session.entry_count() {
            Ok(entries) => entries as u64,
            Err(e) => return error_string(format!("{e:#}")),
        };
        let stats = zo_stats {
            adds,
            queries,
            removes,
            entries,
        };
        // SAFETY: `out` is writable for this call.
        unsafe { *out = stats };
        ptr::null_mut()
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

    /// `_ZO_DATA_DIR` is a process-global environment variable that the zoxide
    /// database layer reads when an operation touches the database, so tests
    /// doing real database work serialize themselves with this lock. Error
    /// handling needs no lock at all: every error is returned to its caller.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Create a session with `_ZO_DATA_DIR` pointed at `data_dir`, panicking if
    /// creation fails. Callers must hold `TEST_LOCK`.
    fn session(data_dir: &std::path::Path) -> *mut SessionHandle {
        unsafe {
            env::set_var("_ZO_DATA_DIR", data_dir.as_os_str());
        };
        let mut handle: *mut SessionHandle = ptr::null_mut();
        let err = unsafe { zo_session_create(&mut handle) };
        assert_ok(err, "zo_session_create");
        assert!(!handle.is_null(), "session creation returned NULL");
        handle
    }

    /// Destroy a session, asserting that teardown reported no error.
    fn destroy(handle: *mut SessionHandle) {
        let err = unsafe { zo_session_destroy(handle) };
        assert_ok(err, "zo_session_destroy");
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

    /// Test basic session creation and destruction.
    #[test]
    fn test_session_create_destroy() {
        let _guard = test_lock();
        let data_dir = tempfile::tempdir().unwrap();
        let handle = session(data_dir.path());
        destroy(handle);
    }

    /// Test that a null out-slot on session creation returns an error.
    #[test]
    fn test_session_create_null_out() {
        let err = unsafe { zo_session_create(ptr::null_mut()) };
        assert_error(err, "out is NULL");
    }

    /// Test that destroying NULL is a safe no-op.
    #[test]
    fn test_session_destroy_null_is_safe() {
        let err = unsafe { zo_session_destroy(ptr::null_mut()) };
        assert!(err.is_null(), "destroying NULL is a successful no-op");
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

    /// Test that NULL arguments are reported by the call itself.
    #[test]
    fn test_null_arguments_return_errors() {
        let _guard = test_lock();
        let data_dir = tempfile::tempdir().unwrap();
        let handle = session(data_dir.path());
        let mut out: *mut c_char = ptr::null_mut();
        let mut stats = zo_stats::default();

        // NULL session handle: the call has nothing to operate on.
        assert_error(
            unsafe { zo_session_add(ptr::null_mut(), ptr::null(), 1.0) },
            "session is NULL",
        );
        assert_error(
            unsafe { zo_session_remove(ptr::null_mut(), ptr::null()) },
            "session is NULL",
        );
        assert_error(
            unsafe { zo_session_query(ptr::null_mut(), ptr::null(), &mut out) },
            "session is NULL",
        );
        assert_error(
            unsafe { zo_session_stats(ptr::null_mut(), &mut stats) },
            "session is NULL",
        );

        // Valid session, NULL payload arguments.
        assert_error(
            unsafe { zo_session_add(handle, ptr::null(), 1.0) },
            "path is NULL",
        );
        assert_error(
            unsafe { zo_session_remove(handle, ptr::null()) },
            "path is NULL",
        );
        assert_error(
            unsafe { zo_session_query(handle, ptr::null(), &mut out) },
            "options is NULL",
        );
        assert_error(
            unsafe { zo_session_stats(handle, ptr::null_mut()) },
            "out is NULL",
        );
        assert_error(
            unsafe { zo_session_query(handle, ptr::null(), ptr::null_mut()) },
            "out is NULL",
        );

        destroy(handle);
    }

    /// Test a full add/query roundtrip.
    #[test]
    fn test_add_query_roundtrip() {
        let _guard = test_lock();
        let data_dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let handle = session(data_dir.path());
        let path = cstr(target.path().to_str().unwrap());

        assert_ok(
            unsafe { zo_session_add(handle, path.as_ptr(), 1.0) },
            "zo_session_add",
        );

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
        let err = unsafe { zo_session_query(handle, &options as *const _, &mut out) };
        assert_ok(err, "zo_session_query");
        assert!(!out.is_null(), "query output should be non-null");
        let result = unsafe { CStr::from_ptr(out) }.to_str().unwrap();
        assert!(result.ends_with(target.path().file_name().unwrap().to_str().unwrap()));
        unsafe { zo_free(out) };

        destroy(handle);
    }

    /// Test `list` queries and the stats snapshot.
    #[test]
    fn test_query_list_and_stats() {
        let _guard = test_lock();
        let data_dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let handle = session(data_dir.path());
        let path = cstr(target.path().to_str().unwrap());
        assert_ok(
            unsafe { zo_session_add(handle, path.as_ptr(), 1.0) },
            "zo_session_add",
        );

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
        let err = unsafe { zo_session_query(handle, &options as *const _, &mut out) };
        assert_ok(err, "zo_session_query");
        assert!(!out.is_null(), "query output should be non-null");
        unsafe { zo_free(out) };

        let mut stats = zo_stats::default();
        let err = unsafe { zo_session_stats(handle, &mut stats as *mut _) };
        assert_ok(err, "zo_session_stats");
        assert_eq!(stats.adds, 1);
        assert_eq!(stats.queries, 1);
        assert_eq!(stats.entries, 1);

        destroy(handle);
    }

    /// Test that removing an entry drops it from the database.
    #[test]
    fn test_remove_roundtrip() {
        let _guard = test_lock();
        let data_dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let handle = session(data_dir.path());
        let path = cstr(target.path().to_str().unwrap());

        assert_ok(
            unsafe { zo_session_add(handle, path.as_ptr(), 1.0) },
            "zo_session_add",
        );
        assert_ok(
            unsafe { zo_session_remove(handle, path.as_ptr()) },
            "zo_session_remove",
        );

        let mut stats = zo_stats::default();
        assert_ok(
            unsafe { zo_session_stats(handle, &mut stats as *mut _) },
            "zo_session_stats",
        );
        assert_eq!(stats.entries, 0);

        destroy(handle);
    }

    /// Test that conflicting query modes are reported by the call itself.
    #[test]
    fn test_query_conflicting_modes() {
        let _guard = test_lock();
        let data_dir = tempfile::tempdir().unwrap();
        let handle = session(data_dir.path());
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
        let err = unsafe { zo_session_query(handle, &options as *const _, &mut out) };
        assert_error(err, "cannot be used together");

        destroy(handle);
    }

    /// Test that non-UTF-8 arguments are rejected before touching the database.
    #[test]
    fn test_invalid_utf8_arguments() {
        let _guard = test_lock();
        let data_dir = tempfile::tempdir().unwrap();
        let handle = session(data_dir.path());
        let invalid = CString::new(vec![0xff, b'x']).unwrap();

        assert_error(
            unsafe { zo_session_add(handle, invalid.as_ptr(), 1.0) },
            "path is not valid UTF-8",
        );
        assert_error(
            unsafe { zo_session_remove(handle, invalid.as_ptr()) },
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
            unsafe { zo_session_query(handle, &options as *const _, &mut out) },
            "exclude is not valid UTF-8",
        );

        destroy(handle);
    }

    /// Test that a failed query clears the caller's output slot.
    #[test]
    fn test_query_failure_clears_output_slot() {
        let _guard = test_lock();
        let data_dir = tempfile::tempdir().unwrap();
        let handle = session(data_dir.path());

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
        let err = unsafe { zo_session_query(handle, &options as *const _, &mut out) };
        assert_error(err, "");
        assert!(
            out.is_null(),
            "a failed query must clear the caller's output slot"
        );

        destroy(handle);
    }

    /// Test that a NULL keyword array with a positive length is rejected.
    #[test]
    fn test_null_keyword_array_with_positive_len_is_rejected() {
        let _guard = test_lock();
        let data_dir = tempfile::tempdir().unwrap();
        let handle = session(data_dir.path());

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
            unsafe { zo_session_query(handle, &options as *const _, &mut out) },
            "invalid keywords array",
        );

        destroy(handle);
    }

    /// Test that every call returns its own error value: a failure on one
    /// session does not affect another session, and successful calls return
    /// NULL with no shared error slot to inspect.
    #[test]
    fn test_errors_are_call_local() {
        let _guard = test_lock();
        let data_dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let first = session(data_dir.path());
        let second = session(data_dir.path());
        let invalid = CString::new(vec![0xff, b'x']).unwrap();
        let path = cstr(target.path().to_str().unwrap());

        // The first session fails while the second is untouched.
        assert_error(
            unsafe { zo_session_add(first, invalid.as_ptr(), 1.0) },
            "not valid UTF-8",
        );
        // The second session fails with a different problem.
        assert_error(
            unsafe { zo_session_add(second, ptr::null(), 1.0) },
            "path is NULL",
        );
        // The second session can succeed afterwards: success is simply NULL.
        assert_ok(
            unsafe { zo_session_add(second, path.as_ptr(), 1.0) },
            "zo_session_add",
        );
        // The first session still reports its own, later error.
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
            unsafe { zo_session_query(first, &options as *const _, &mut out) },
            "cannot be used together",
        );

        destroy(first);
        destroy(second);
    }

    /// Test concurrent sessions: each thread owns its session, so its failures
    /// carry that thread's message back to it. Errors live in the return value,
    /// so there is no shared state to race on and the test needs neither a lock
    /// nor barriers (the failure paths never touch the database, so the process
    /// environment is not involved either).
    #[test]
    fn test_concurrent_sessions_report_their_own_errors() {
        const ITERATIONS: usize = 200;

        thread::scope(|scope| {
            scope.spawn(|| {
                let mut handle: *mut SessionHandle = ptr::null_mut();
                let err = unsafe { zo_session_create(&mut handle) };
                assert_ok(err, "zo_session_create");
                let invalid = CString::new(vec![0xff, b'x']).unwrap();
                for _ in 0..ITERATIONS {
                    let err = unsafe { zo_session_add(handle, invalid.as_ptr(), 1.0) };
                    assert_error(err, "not valid UTF-8");
                }
                let err = unsafe { zo_session_destroy(handle) };
                assert_ok(err, "zo_session_destroy");
            });
            scope.spawn(|| {
                let mut handle: *mut SessionHandle = ptr::null_mut();
                let err = unsafe { zo_session_create(&mut handle) };
                assert_ok(err, "zo_session_create");
                for _ in 0..ITERATIONS {
                    let err = unsafe { zo_session_add(handle, ptr::null(), 1.0) };
                    assert_error(err, "path is NULL");
                }
                let err = unsafe { zo_session_destroy(handle) };
                assert_ok(err, "zo_session_destroy");
            });
        });
    }
}
