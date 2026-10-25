//! Embed the version of the zoxide checkout this crate builds against.
//!
//! Cargo requires `[package].version` to be a literal, so the FFI crate keeps
//! its own wrapper version (0.1.0). The version users care about is the
//! upstream zoxide one (the `in-process` feature is version-locked to it), and
//! that can be derived at build time:
//!
//! 1. CMake passes `ZOXIDE_VERSION` (it derives it from the fetched/selected
//!    zoxide checkout), which also covers the `ZOXIDE_SOURCE` != local case;
//! 2. a plain `cargo build` falls back to reading the manifest of the
//!    `../zoxide` path dependency (the symlink CMake creates).
//!
//! The value is exposed as `env!("ZOXIDE_VERSION")` and returned by
//! [`zo_version`](crate::ffi::zo_version).

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=ZOXIDE_VERSION");
    println!("cargo:rerun-if-env-changed=ZOXIDE_COMMIT");

    let crate_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"),
    );

    // The path dependency is `../zoxide`; its version lives in the root
    // package manifest.
    let manifest = crate_dir.join("../zoxide/Cargo.toml");
    println!("cargo:rerun-if-changed={}", manifest.display());

    let version = std::env::var("ZOXIDE_VERSION")
        .ok()
        .filter(|version| !version.trim().is_empty())
        .or_else(|| manifest_version(&manifest))
        .unwrap_or_else(|| {
            std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION")
        });
    println!("cargo:rustc-env=ZOXIDE_VERSION={version}");

    // The commit CMake pinned the checkout to, for `--version`-style output and
    // bug reports. Optional: a plain cargo build has no pin.
    if let Ok(commit) = std::env::var("ZOXIDE_COMMIT")
        && !commit.trim().is_empty()
    {
        println!("cargo:rustc-env=ZOXIDE_COMMIT={commit}");
    }
}

/// The `version = "x.y.z"` entry of a manifest, if it has a literal one.
///
/// A hand-rolled scan rather than a TOML build-dependency: only one line is
/// needed, and a parser would be compiled on every build to find it.
fn manifest_version(manifest: &Path) -> Option<String> {
    let text = std::fs::read_to_string(manifest).ok()?;
    text.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("version")?.trim_start();
        let rest = rest.strip_prefix('=')?.trim();
        let value = rest.strip_prefix('"')?.strip_suffix('"')?;
        is_semver(value).then(|| value.to_owned())
    })
}

/// `x.y.z`, optionally with a pre-release/build suffix.
fn is_semver(value: &str) -> bool {
    let core = value.split(['-', '+']).next().unwrap_or(value);
    let mut parts = core.split('.');
    let numeric = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(major), Some(minor), Some(patch))
            if numeric(major) && numeric(minor) && numeric(patch)
    ) && parts.next().is_none()
}
