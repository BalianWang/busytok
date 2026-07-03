//! Coverage gap tests for `busytok-platform`.
//!
//! Targets uncovered source lines reported by `cargo llvm-cov`:
//! - `macos.rs`: `PlatformPaths::with_home_dir` (lines 19-23) and `Default` impl
//!   (lines 59-61).
//!
//! These tests deliberately exercise the custom-home-dir path and the
//! `Default` impl — neither of which is reached by the existing
//! `platform_paths.rs` integration tests.

#![allow(
    clippy::await_holding_lock,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::field_reassign_with_default,
    clippy::uninlined_format_args,
    clippy::inconsistent_digit_grouping,
    clippy::len_zero,
    clippy::identity_op,
    clippy::useless_vec,
    clippy::manual_dangling_ptr,
    clippy::unwrap_used,
    clippy::unused_async,
    clippy::absurd_extreme_comparisons,
    unused_variables,
    unused_imports,
    dead_code
)]

use std::path::PathBuf;

use busytok_platform::PlatformPaths;

/// `with_home_dir` stores the supplied directory verbatim and exposes it
/// through `resolve_home_dir`. The service identifier is unaffected.
#[test]
fn with_home_dir_uses_custom_home_for_service_paths() {
    let custom = if cfg!(target_os = "macos") {
        PathBuf::from("/tmp/busytok-coverage-home")
    } else {
        PathBuf::from("/tmp/busytok-coverage-unsupported-home")
    };

    let p = PlatformPaths::with_home_dir(custom.clone());

    // `resolve_home_dir` must return exactly the supplied path, not the
    // system home directory.
    assert_eq!(p.resolve_home_dir(), custom);

    // service_identifier is a static constant — unaffected by home_dir.
    assert!(!p.service_identifier().is_empty());

    // On macOS, the install root and plist path must be anchored at the
    // custom home directory.
    if cfg!(target_os = "macos") {
        let install_root = p.service_install_root();
        assert!(install_root.starts_with(&custom), "install root should be under custom home: {} vs {}", install_root.display(), custom.display());
        assert!(
            install_root.to_string_lossy().contains("Library/LaunchAgents"),
            "install root should point at LaunchAgents"
        );

        let plist = p.service_definition_path();
        assert!(plist.starts_with(&install_root));
        assert!(plist
            .to_string_lossy()
            .ends_with("com.busytok.service.plist"));
    }
}

/// `resolve_home_dir` falls back to the system home when `with_home_dir` was
/// never called (the `new()` path). Verifies the `Option::None` branch of
/// `unwrap_or_else` resolves to the system home directory.
#[test]
fn resolve_home_dir_prefers_custom_over_system() {
    let custom = PathBuf::from("/tmp/busytok-coverage-alt");
    let with_custom = PlatformPaths::with_home_dir(custom.clone());
    assert_eq!(with_custom.resolve_home_dir(), custom);

    // The default-constructed PlatformPaths should resolve to a real system
    // home directory — distinct from the custom override.
    let default = PlatformPaths::new();
    let resolved = default.resolve_home_dir();
    assert!(!resolved.as_os_str().is_empty(), "system home must not be empty");
    assert_ne!(resolved, custom, "default should not resolve to custom override");
}

/// `PlatformPaths::default()` is equivalent to `PlatformPaths::new()`:
/// both leave `home_dir = None`. Required to cover the `Default` impl body.
#[test]
fn default_impl_matches_new() {
    let from_new = PlatformPaths::new();
    let from_default = PlatformPaths::default();

    // Both should resolve to the same system home (None path).
    assert_eq!(from_new.resolve_home_dir(), from_default.resolve_home_dir());

    // Both should produce identical service paths.
    assert_eq!(
        from_new.service_identifier(),
        from_default.service_identifier()
    );
    assert_eq!(
        from_new.service_install_root(),
        from_default.service_install_root()
    );
    assert_eq!(
        from_new.service_definition_path(),
        from_default.service_definition_path()
    );
    assert_eq!(
        from_new.busytok_data_dir(),
        from_default.busytok_data_dir()
    );
    assert_eq!(
        from_new.busytok_db_path(),
        from_default.busytok_db_path()
    );
}
