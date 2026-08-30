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
//! unwinding across the FFI boundary. Errors are reported via return codes and
//! a global mutex guarded error string accessible via `zo_last_error()`.

// The exported `zo_*` functions are the unsafe boundary of this crate. They
// validate every raw pointer before dereferencing, so they are intentionally
// callable from C without an `unsafe` block in Rust.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

pub mod ffi;
