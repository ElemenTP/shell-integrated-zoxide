//! C FFI bindings for zoxide directory jumping.
//!
//! This crate exposes a C-compatible API that allows shell modules (zsh, pwsh)
//! to call zoxide's `add` and `query` hot paths in-process, without spawning a
//! subprocess for every shell hook / directory jump.
//!
//! # Why the zoxide source modules are included here
//!
//! Upstream zoxide is a binary-only crate, so it cannot be consumed with a
//! normal Cargo `path` dependency. The symlink at `rust_src/zoxide` points at
//! the upstream checkout and the modules below are compiled into this crate
//! via `#[path]`. Only the database/configuration half is included; the CLI
//! half (`clap`, templates, importers) is unnecessary for the hot paths.
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
