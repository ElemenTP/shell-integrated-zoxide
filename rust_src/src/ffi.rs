//! C FFI exports for zoxide in-process directory jumping.
//!
//! All public functions follow the `zo_` prefix convention and use the C ABI.
//! Strings returned to C are owned by Rust and must be freed with `zo_free`.

use std::ffi::{CStr, CString};
use std::os::raw::{c_double, c_int};
use std::path::PathBuf;
use std::ptr;
use std::sync::Mutex;

use libc::c_char;

// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------
//
// NOTE: deliberately NOT thread_local!. A thread_local! would register a TLS
// destructor on the HOST thread (zsh/pwsh main thread) the first time it is
// touched. After dlclose() unmaps this dylib, that destructor pointer dangles —
// glibc skips destructors of unloaded DSOs, but macOS and Windows do not,
// causing a potential SIGSEGV at host-thread exit.
//
// A global Mutex has no per-thread state and is safe to unload. FFI calls are
// serialized by the shell's single thread anyway, so contention is nil.

static LAST_ERROR: Mutex<Option<CString>> = Mutex::new(None);

fn set_error(msg: &str) {
    if let Ok(mut e) = LAST_ERROR.lock() {
        *e = CString::new(msg).ok();
    }
}

fn clear_error() {
    if let Ok(mut e) = LAST_ERROR.lock() {
        *e = None;
    }
}

/// FFI panic guard: wraps a closure, catching any panic and converting it to
/// an error return with the panic message stored in the global error slot.
///
/// Pattern adapted from zsh-native-syntax / shell-integrated-starship.
macro_rules! ffi_guard {
    ($expr:expr, $error_val:expr) => {{
        clear_error();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| $expr)) {
            Ok(result) => result,
            Err(panic) => {
                let msg = if let Some(s) = panic.downcast_ref::<&str>() {
                    format!("panic: {s}")
                } else if let Some(s) = panic.downcast_ref::<String>() {
                    format!("panic: {s}")
                } else {
                    "panic: unknown error".to_string()
                };
                set_error(&msg);
                $error_val
            }
        }
    }};
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
// Session wrapper
// ---------------------------------------------------------------------------

/// Opaque session handle passed to C code.
pub struct SessionHandle {
    session: zoxide::session::Session,
}

// ---------------------------------------------------------------------------
// Conversion helpers
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
            let s = CStr::from_ptr(s).to_str().ok()?;
            result.push(s.to_string());
        }
    }
    Some(result)
}

fn string_into_c(value: String) -> *mut c_char {
    CString::new(value)
        .unwrap_or_else(|e| {
            // Truncate at the first NUL byte. Zoxide paths cannot contain NUL
            // in practice, but keeping the FFI boundary total is safer than
            // returning a null pointer here.
            let pos = e.nul_position();
            let mut bytes = e.into_vec();
            bytes.truncate(pos);
            CString::new(bytes).unwrap()
        })
        .into_raw()
}

// ---------------------------------------------------------------------------
// Public C API
// ---------------------------------------------------------------------------

/// Create a new zoxide session and open the database.
///
/// Returns a non-null opaque pointer on success, or null on failure
/// (check `zo_last_error()`).
#[unsafe(no_mangle)]
pub extern "C" fn zo_session_create() -> *mut SessionHandle {
    ffi_guard!(
        {
            let session = {
                match zoxide::session::Session::new() {
                    Ok(session) => session,
                    Err(e) => {
                        set_error(&format!("{e:#}"));
                        return ptr::null_mut();
                    }
                }
            };

            Box::into_raw(Box::new(SessionHandle { session }))
        },
        ptr::null_mut()
    )
}

/// Destroy a session previously created with `zo_session_create`.
///
/// Passing NULL is safe (no-op).
#[unsafe(no_mangle)]
pub extern "C" fn zo_session_destroy(handle: *mut SessionHandle) {
    if handle.is_null() {
        return;
    }
    ffi_guard!(
        {
            // SAFETY: the handle was returned by `zo_session_create` and is
            // destroyed exactly once by the shell module.
            unsafe {
                let _ = Box::from_raw(handle);
            }
        },
        ()
    );
}

/// Add a directory to the session database.
///
/// Returns 0 on success. `score` is the frecency increment (use 1.0 for the
/// standard shell hook).
#[unsafe(no_mangle)]
pub extern "C" fn zo_session_add(
    handle: *mut SessionHandle,
    path: *const c_char,
    score: c_double,
) -> c_int {
    ffi_guard!(
        {
            if handle.is_null() || path.is_null() {
                set_error("zo_session_add: null argument");
                return -1;
            }
            let Some(path) = cstr_to_option_string(path) else {
                set_error("zo_session_add: path is not valid UTF-8");
                return -1;
            };

            // SAFETY: `handle` is valid, non-null and exclusively borrowed for
            // this call (all FFI calls run on the shell's single thread).
            let handle = unsafe { &mut *handle };
            if let Err(e) = handle.session.add(PathBuf::from(path), score) {
                set_error(&format!("{e:#}"));
                return -1;
            }
            0
        },
        -1
    )
}

/// Remove a directory from the session database.
///
/// Returns 0 on success, <0 if the path is not in the database.
#[unsafe(no_mangle)]
pub extern "C" fn zo_session_remove(handle: *mut SessionHandle, path: *const c_char) -> c_int {
    ffi_guard!(
        {
            if handle.is_null() || path.is_null() {
                set_error("zo_session_remove: null argument");
                return -1;
            }
            let Some(path) = cstr_to_option_string(path) else {
                set_error("zo_session_remove: path is not valid UTF-8");
                return -1;
            };

            let handle = unsafe { &mut *handle };
            if let Err(e) = handle.session.remove(&path) {
                set_error(&format!("{e:#}"));
                return -1;
            }
            0
        },
        -1
    )
}

/// Run a query against the session database.
///
/// On success, returns 0 and writes a Rust-allocated UTF-8 string to `*out`.
/// The caller must free `*out` with `zo_free()`.
/// On failure, returns a negative value; check `zo_last_error()`.
#[unsafe(no_mangle)]
pub extern "C" fn zo_session_query(
    handle: *mut SessionHandle,
    options: *const zo_query_options,
    out: *mut *mut c_char,
) -> c_int {
    ffi_guard!(
        {
            if handle.is_null() || options.is_null() || out.is_null() {
                set_error("zo_session_query: null argument");
                return -1;
            }

            // SAFETY: `options` points to a valid C struct for the duration of
            // this call. The struct is declared `#[repr(C)]` and the header
            // matches field-for-field.
            let options = unsafe { &*options };
            let Some(keywords) = cstr_array_to_vec(options.keywords, options.keywords_len) else {
                set_error("zo_session_query: invalid keywords array");
                return -1;
            };
            let exclude = if options.exclude.is_null() {
                None
            } else {
                let Some(exclude) = cstr_to_option_string(options.exclude) else {
                    set_error("zo_session_query: exclude is not valid UTF-8");
                    return -1;
                };
                Some(exclude)
            };
            let base_dir = if options.base_dir.is_null() {
                None
            } else {
                let Some(base_dir) = cstr_to_option_string(options.base_dir) else {
                    set_error("zo_session_query: base_dir is not valid UTF-8");
                    return -1;
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

            let handle = unsafe { &mut *handle };
            match handle.session.query(query) {
                Ok(result) => {
                    // SAFETY: `out` is a valid pointer to a `char *` slot.
                    unsafe { *out = string_into_c(result) };
                    0
                }
                Err(e) => {
                    set_error(&format!("{e:#}"));
                    -1
                }
            }
        },
        -1
    )
}

/// Retrieve session statistics.
///
/// Returns 0 on success.
#[unsafe(no_mangle)]
pub extern "C" fn zo_session_stats(handle: *mut SessionHandle, out: *mut zo_stats) -> c_int {
    ffi_guard!(
        {
            if handle.is_null() || out.is_null() {
                set_error("zo_session_stats: null argument");
                return -1;
            }
            let handle = unsafe { &*handle };
            let zoxide::session::SessionStats {
                adds,
                queries,
                removes,
            } = handle.session.stats();
            let stats = zo_stats {
                adds,
                queries,
                removes,
                entries: handle.session.entry_count() as u64,
            };
            // SAFETY: `out` is writable for the duration of this call.
            unsafe { *out = stats };
            0
        },
        -1
    )
}

/// Free a string previously returned by `zo_session_query` or
/// `zo_last_error`. Passing NULL is safe (no-op).
#[unsafe(no_mangle)]
pub extern "C" fn zo_free(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    ffi_guard!(
        {
            // SAFETY: `ptr` was allocated by `CString::into_raw` in this
            // library and has not been freed before.
            unsafe {
                let _ = CString::from_raw(ptr);
            }
        },
        ()
    );
}

/// Return the library version as a static string.
///
/// The returned pointer is valid for the lifetime of the process and must NOT
/// be freed.
#[unsafe(no_mangle)]
pub extern "C" fn zo_version() -> *const c_char {
    // A string literal has static storage duration and the trailing NUL is
    // included in the literal itself, so no LazyLock/allocation is needed.
    static VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");
    VERSION.as_ptr().cast()
}

/// Return the last error message.
///
/// The caller must free `*out` with `zo_free()`. `*out` is set to NULL when no
/// error has been recorded.
#[unsafe(no_mangle)]
pub extern "C" fn zo_last_error(out: *mut *mut c_char) {
    if out.is_null() {
        return;
    }
    let c_string = LAST_ERROR.lock().ok().and_then(|e| e.clone());
    // SAFETY: `out` points to a writable `char *` slot.
    unsafe {
        *out = match c_string {
            Some(c_string) => c_string.into_raw(),
            None => ptr::null_mut(),
        };
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, ffi::CString};

    /// The FFI error slot is process-global. Rust runs unit tests in parallel,
    /// so tests that inspect `zo_last_error` take this lock to avoid races.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn session(data_dir: &std::path::Path) -> *mut SessionHandle {
        unsafe { env::set_var("_ZO_DATA_DIR", data_dir.as_os_str()); };
        let handle = zo_session_create();
        assert!(!handle.is_null());
        handle
    }

    fn cstr(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    #[test]
    fn test_session_create_destroy() {
        let _guard = test_lock();
        let data_dir = tempfile::tempdir().unwrap();
        let handle = session(data_dir.path());
        zo_session_destroy(handle);
    }

    #[test]
    fn test_destroy_null_is_safe() {
        let _guard = test_lock();
        zo_session_destroy(ptr::null_mut());
    }

    #[test]
    fn test_null_arguments_return_errors() {
        let _guard = test_lock();
        let data_dir = tempfile::tempdir().unwrap();
        let handle = session(data_dir.path());
        let mut out: *mut c_char = ptr::null_mut();

        assert!(zo_session_add(handle, ptr::null(), 1.0) < 0);
        assert!(zo_session_remove(handle, ptr::null()) < 0);
        assert!(zo_session_query(handle, ptr::null(), &mut out) < 0);
        assert!(zo_session_query(ptr::null_mut(), ptr::null(), &mut out) < 0);

        zo_session_destroy(handle);
    }

    #[test]
    fn test_add_query_roundtrip() {
        let _guard = test_lock();
        let data_dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let handle = session(data_dir.path());
        let path = cstr(target.path().to_str().unwrap());

        assert_eq!(zo_session_add(handle, path.as_ptr(), 1.0), 0);

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
        assert_eq!(zo_session_query(handle, &options as *const _, &mut out), 0);
        assert!(!out.is_null());
        let result = unsafe { CStr::from_ptr(out) }.to_str().unwrap();
        assert!(result.ends_with(target.path().file_name().unwrap().to_str().unwrap()));
        zo_free(out);

        zo_session_destroy(handle);
    }

    #[test]
    fn test_query_list_and_stats() {
        let _guard = test_lock();
        let data_dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let handle = session(data_dir.path());
        let path = cstr(target.path().to_str().unwrap());
        zo_session_add(handle, path.as_ptr(), 1.0);

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
        assert_eq!(zo_session_query(handle, &options as *const _, &mut out), 0);
        assert!(!out.is_null());
        zo_free(out);

        let mut stats = zo_stats::default();
        assert_eq!(zo_session_stats(handle, &mut stats as *mut _), 0);
        assert_eq!(stats.adds, 1);
        assert_eq!(stats.queries, 1);
        assert_eq!(stats.entries, 1);

        zo_session_destroy(handle);
    }

    #[test]
    fn test_version_and_error_handling() {
        let _guard = test_lock();
        let version = zo_version();
        assert!(!version.is_null());
        assert!(
            !unsafe { CStr::from_ptr(version) }
                .to_string_lossy()
                .is_empty()
        );

        let mut out: *mut c_char = ptr::null_mut();
        zo_last_error(&mut out);
        assert!(out.is_null(), "no error recorded before first call");

        assert!(zo_session_query(ptr::null_mut(), ptr::null(), &mut out) < 0);
        zo_last_error(&mut out);
        assert!(
            !out.is_null(),
            "error should be available after a failing call"
        );
        let message = unsafe { CStr::from_ptr(out) }
            .to_string_lossy()
            .into_owned();
        assert!(!message.is_empty());
        zo_free(out);
    }

    #[test]
    fn test_remove_roundtrip() {
        let _guard = test_lock();
        let data_dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let handle = session(data_dir.path());
        let path = cstr(target.path().to_str().unwrap());

        zo_session_add(handle, path.as_ptr(), 1.0);
        assert_eq!(zo_session_remove(handle, path.as_ptr()), 0);

        let mut stats = zo_stats::default();
        zo_session_stats(handle, &mut stats as *mut _);
        assert_eq!(stats.entries, 0);

        zo_session_destroy(handle);
    }

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
        assert!(zo_session_query(handle, &options as *const _, &mut out) < 0);

        zo_last_error(&mut out);
        assert!(!out.is_null());
        zo_free(out);
        zo_session_destroy(handle);
    }

    #[test]
    fn test_invalid_utf8_arguments() {
        let _guard = test_lock();
        let data_dir = tempfile::tempdir().unwrap();
        let handle = session(data_dir.path());
        let invalid = CString::new(vec![0xff, b'x']).unwrap();

        assert!(zo_session_add(handle, invalid.as_ptr(), 1.0) < 0);
        zo_last_error(&mut ptr::null_mut()); // NULL out is tolerated
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
        assert!(zo_session_query(handle, &options as *const _, &mut out) < 0);

        zo_session_destroy(handle);
    }

    #[test]
    fn test_free_null_is_safe() {
        let _guard = test_lock();
        zo_free(ptr::null_mut());
    }
}
