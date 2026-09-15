//! C FFI bindings for zoxide directory jumping.
//!
//! This crate exposes a C-compatible API that allows shell modules (zsh, pwsh)
//! to call zoxide's `add` and `query` hot paths in-process, without spawning a
//! subprocess for every shell hook / directory jump.
//!
//! # Upstream dependency
//!
//! The repo-root `zoxide` symlink points at the zoxide `in-process` checkout.
//! `Cargo.toml` consumes it as a normal path dependency with the
//! `in-process` feature, which exposes `zoxide::session::Session`. `Session`
//! deliberately does not keep the database open: every add/query/remove
//! constructs the same command structs as the zoxide CLI and opens/closes
//! `db.zo` per command, so updates from other shells are never overwritten
//! with stale in-memory state.
//!
//! # Safety
//!
//! All FFI functions use `catch_unwind` wrappers to prevent Rust panics from
//! unwinding across the FFI boundary. Errors are reported as **return
//! values**, not as stored state:
//!
//! ```c
//! char *err = zo_session_add(session, path, 1.0);
//! if (err) {
//!     fprintf(stderr, "zoxide: %s\n", err);
//!     zo_free(err);            /* caller owns the message */
//! }
//! ```
//!
//! NULL means success; a non-NULL pointer is a Rust-allocated message that
//! must be freed with `zo_free()`. An empty message is still a failure (zoxide
//! reports fzf Ctrl-C that way) — callers test the pointer first and print
//! only when the text is non-empty.
//!
//! Because nothing is stored, there is no shared error state: concurrent
//! sessions can never clobber each other's message, and there is no
//! `thread_local!` (whose TLS destructor would dangle after the shell
//! `dlclose()`s this cdylib — glibc skips destructors of unloaded DSOs, macOS
//! and Windows do not).
//!
//! `zo_session_create` writes the handle to an out-parameter and returns the
//! error instead (the handle is the payload). `zo_session_destroy` returns an
//! error string too, so a failure while tearing the session down is visible for
//! debugging. `zo_free` cannot fail and returns void.

// The exported `zo_*` functions are the unsafe boundary of this crate: each one
// documents its pointer contract in a `# Safety` section. Callers from C are
// unaffected by the Rust `unsafe` marker; Rust callers (the unit tests) wrap the
// calls in `unsafe` blocks.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

pub mod ffi;
