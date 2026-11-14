//! === IMGFORGE CLI BUILD SCRIPT ===
//!
//! PURPOSE:
//! Injects `IMGFORGE_VERSION` at compile time from `IMGFORGE_BUILD_VERSION` (when
//! provided and non-empty) or falls back to `CARGO_PKG_VERSION`.
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! Release builds inject tag versions via `IMGFORGE_BUILD_VERSION` without
//! mutating tracked `Cargo.toml` manifests. Validating strict SemVer in the
//! build script ensures malformed release tags fail the build immediately rather
//! than embedding invalid version metadata into `imgforge --version` or
//! `/api/health`.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. Register `cargo:rerun-if-env-changed=IMGFORGE_BUILD_VERSION` and source
//!    triggers (`build.rs`, `src/version.rs`).
//! 2. Read `IMGFORGE_BUILD_VERSION` (panicking on non-UTF-8).
//! 3. Resolve and validate strict `MAJOR.MINOR.PATCH` (after stripping optional
//!    leading `'v'`/`'V'`), falling back to `CARGO_PKG_VERSION` when unset/empty.
//! 4. Emit `cargo:rustc-env=IMGFORGE_VERSION=<version>`.

#[path = "src/version.rs"]
mod version;

fn main() {
    println!("cargo:rerun-if-env-changed=IMGFORGE_BUILD_VERSION");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/version.rs");

    let pkg_version = env!("CARGO_PKG_VERSION");
    let raw_override = match std::env::var("IMGFORGE_BUILD_VERSION") {
        Ok(val) => Some(val),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            panic!("IMGFORGE_BUILD_VERSION is not valid UTF-8");
        }
    };

    let resolved = version::resolve_build_version(raw_override.as_deref(), pkg_version)
        .unwrap_or_else(|err| panic!("invalid IMGFORGE_BUILD_VERSION: {err}"));

    println!("cargo:rustc-env=IMGFORGE_VERSION={resolved}");
}
