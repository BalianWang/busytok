//! Coverage gap tests for `busytok-config`.
//!
//! Targets uncovered source lines reported by `cargo llvm-cov`:
//! - `lib.rs`: `BusytokSettings::load` paths (missing file, timezone
//!   canonicalization, invalid timezone, corrupt TOML), `save`, atomic_write
//!   rename-error path, `canonicalize_builtin_profiles` tracing.
//! - `logging.rs`: `init_logging` (Service/Gui/Cli) and `prune_old_logs`
//!   branches (old mtime removal, files without dots, rotated-file removal).
//! - `paths.rs`: sidecar path methods (`sidecar_runtime_dir`,
//!   `sidecar_bundle_path`, `sidecar_manifest_path`, `sidecar_bundled_node_path`).
//! - `service_marker.rs`: parent-dir creation path on `write`.
//! - `platform/unsupported.rs`: SID stubs returning None on non-Windows.
//!
//! These tests deliberately use `BusytokPaths::for_test` to avoid touching the
//! real user's config/data directories. `init_logging` is exercised once per
//! process via a single sequential test because the global subscriber cannot
//! be re-initialized.

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

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use busytok_config::{
    atomic_write, init_logging, prune_old_logs, BusytokPaths, BusytokSettings, LogSource,
};
use tempfile::TempDir;

// ── BusytokSettings::load paths ────────────────────────────────────────

#[test]
fn load_returns_defaults_when_settings_file_missing() {
    let tmp = TempDir::new().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    // No settings.toml exists yet.
    let settings = BusytokSettings::load(&paths).expect("load ok");
    // Defaults include system IANA timezone + all 3 built-in profiles.
    assert!(!settings.timezone.is_empty());
    assert_eq!(settings.subagent.profiles.len(), 3);
    assert!(settings.discovery.claude_code_default_paths);
}

#[test]
fn load_canonicalizes_local_timezone_alias() {
    // Writing `timezone = "local"` triggers the canonicalization branch:
    // `ReportingTimezone::parse("local")` succeeds but the canonical name
    // differs from "local", so `load` rewrites and saves the file.
    let tmp = TempDir::new().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());

    let toml_str = r#"timezone = "local"
week_starts_on = 1
"#;
    // Create the config_dir BEFORE writing the file.
    fs::create_dir_all(paths.config_dir()).unwrap();
    fs::write(paths.config_dir().join("settings.toml"), toml_str).unwrap();

    let settings = BusytokSettings::load(&paths).expect("load ok");
    assert_ne!(
        settings.timezone, "local",
        "local must be canonicalized to a real IANA name"
    );
    assert!(!settings.timezone.is_empty());

    // The canonicalized value must have been persisted back to disk.
    let on_disk = fs::read_to_string(paths.config_dir().join("settings.toml")).unwrap();
    assert!(
        !on_disk.contains("\"local\""),
        "settings.toml should no longer contain literal \"local\""
    );
}

#[test]
fn load_falls_back_on_unparseable_timezone() {
    // An invalid timezone string makes `ReportingTimezone::parse` fail;
    // `load` falls back to `resolve_local_timezone()` and saves.
    let tmp = TempDir::new().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    fs::create_dir_all(paths.config_dir()).unwrap();

    let toml_str = r#"timezone = "not-a-real-timezone"
week_starts_on = 1
"#;
    fs::write(paths.config_dir().join("settings.toml"), toml_str).unwrap();

    let settings = BusytokSettings::load(&paths).expect("load ok");
    assert_ne!(
        settings.timezone, "not-a-real-timezone",
        "invalid timezone must be replaced with the system local fallback"
    );
    assert!(!settings.timezone.is_empty());

    // The fallback value must have been persisted back to disk.
    let on_disk = fs::read_to_string(paths.config_dir().join("settings.toml")).unwrap();
    assert!(!on_disk.contains("not-a-real-timezone"));
}

#[test]
fn load_corrupt_toml_returns_defaults() {
    let tmp = TempDir::new().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    fs::create_dir_all(paths.config_dir()).unwrap();

    fs::write(
        paths.config_dir().join("settings.toml"),
        "this is not valid toml {{{}",
    )
    .unwrap();

    let settings = BusytokSettings::load(&paths).expect("load ok");
    // Defaults should be returned: built-in profiles present, timezone detected.
    assert_eq!(settings.subagent.profiles.len(), 3);
    assert!(!settings.timezone.is_empty());
}

#[test]
fn load_canonicalizes_iana_timezone_when_already_canonical() {
    // An IANA name that already IS its canonical form should NOT trigger the
    // save-back path. Verify the file contents are unchanged after load.
    let tmp = TempDir::new().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    fs::create_dir_all(paths.config_dir()).unwrap();

    let original = r#"timezone = "Asia/Shanghai"
week_starts_on = 1
"#;
    fs::write(paths.config_dir().join("settings.toml"), original).unwrap();

    let mtime_before = fs::metadata(paths.config_dir().join("settings.toml"))
        .and_then(|m| m.modified())
        .ok();

    let settings = BusytokSettings::load(&paths).expect("load ok");
    assert_eq!(settings.timezone, "Asia/Shanghai");

    // Verify the file was NOT rewritten (canonical == original).
    let after = fs::read_to_string(paths.config_dir().join("settings.toml")).unwrap();
    assert_eq!(
        after, original,
        "file should not be rewritten if canonical matches"
    );

    // mtime should be unchanged (no save happened).
    let mtime_after = fs::metadata(paths.config_dir().join("settings.toml"))
        .and_then(|m| m.modified())
        .ok();
    if let (Some(before), Some(after)) = (mtime_before, mtime_after) {
        assert_eq!(
            before, after,
            "mtime must be unchanged when no save happens"
        );
    }
}

#[test]
fn save_persists_settings_through_paths() {
    let tmp = TempDir::new().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());

    let mut settings = BusytokSettings::default();
    settings.timezone = "America/New_York".to_string();
    settings.week_starts_on = 0;
    settings.save(&paths).expect("save ok");

    let loaded = BusytokSettings::load(&paths).expect("load ok");
    // Note: load canonicalizes — America/New_York is its own canonical form.
    assert_eq!(loaded.timezone, "America/New_York");
    assert_eq!(loaded.week_starts_on, 0);
}

#[test]
fn save_then_load_roundtrips_through_paths() {
    let tmp = TempDir::new().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());

    let settings = BusytokSettings::default();
    settings.save(&paths).expect("save ok");

    let loaded = BusytokSettings::load(&paths).expect("load ok");
    // Timezone round-trips (default uses system-detected IANA name).
    assert_eq!(loaded.timezone, settings.timezone);
    assert_eq!(loaded.week_starts_on, settings.week_starts_on);
    assert_eq!(
        loaded.subagent.profiles.len(),
        settings.subagent.profiles.len()
    );
}

#[test]
fn canonicalize_logs_when_filling_missing_builtins() {
    // Loading a settings file that lacks all built-in profiles triggers
    // `canonicalize_builtin_profiles`, which fills them in and emits a
    // tracing log. The function returns Ok with all 3 profiles present.
    let tmp = TempDir::new().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    fs::create_dir_all(paths.config_dir()).unwrap();

    let toml_str = r#"timezone = "UTC"
[subagent]
enabled = true
"#;
    fs::write(paths.config_dir().join("settings.toml"), toml_str).unwrap();

    let settings = BusytokSettings::load(&paths).expect("load ok");
    assert_eq!(
        settings.subagent.profiles.len(),
        3,
        "all 3 built-in profiles must be filled in"
    );
    assert!(settings.subagent.profiles.contains_key("pi/search-cheap"));
    assert!(settings.subagent.profiles.contains_key("pi/review-cheap"));
    assert!(settings.subagent.profiles.contains_key("pi/plan-cheap"));
}

// ── atomic_write ───────────────────────────────────────────────────────

#[test]
fn atomic_write_writes_content_to_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("out.txt");
    atomic_write(&path, "hello world").expect("ok");
    let content = fs::read_to_string(&path).unwrap();
    assert_eq!(content, "hello world");
}

#[test]
fn atomic_write_creates_parent_dirs() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("nested/deep/dir/out.txt");
    atomic_write(&path, "deep").expect("ok");
    assert_eq!(fs::read_to_string(&path).unwrap(), "deep");
}

#[test]
fn atomic_write_overwrites_existing_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("out.txt");
    atomic_write(&path, "first").expect("ok");
    atomic_write(&path, "second").expect("ok");
    assert_eq!(fs::read_to_string(&path).unwrap(), "second");
}

// ── prune_old_logs branches ─────────────────────────────────────────────

#[test]
fn prune_removes_old_rotated_file() {
    // Cover the `mtime < cutoff` branch + `remove_file` call.
    let dir = TempDir::new().unwrap();
    let old = dir.path().join("service.log.2020-01-01");
    fs::write(&old, "test").unwrap();

    // Set mtime to 30 days ago so prune (keep_days=7) removes it.
    let thirty_days_ago = SystemTime::now() - Duration::from_secs(30 * 86400);
    let f = fs::File::open(&old).unwrap();
    f.set_modified(thirty_days_ago).ok();

    prune_old_logs(dir.path(), 7);
    assert!(!old.exists(), "old rotated log should be removed");
}

#[test]
fn prune_keeps_file_without_dot_extension() {
    // Cover the "no dot in filename → continue" branch.
    let dir = TempDir::new().unwrap();
    let no_dot = dir.path().join("readme");
    fs::write(&no_dot, "test").unwrap();
    // Force an old mtime so the file would qualify for removal if it matched
    // the rotated-file pattern.
    let old = SystemTime::now() - Duration::from_secs(30 * 86400);
    let f = fs::File::open(&no_dot).unwrap();
    f.set_modified(old).ok();

    prune_old_logs(dir.path(), 7);
    assert!(no_dot.exists(), "file without dot must be kept");
}

#[test]
fn prune_keeps_file_with_short_date_suffix() {
    // Cover the "date_part length != 10 → continue" branch.
    let dir = TempDir::new().unwrap();
    let short = dir.path().join("service.log.2020");
    fs::write(&short, "test").unwrap();
    let old = SystemTime::now() - Duration::from_secs(30 * 86400);
    let f = fs::File::open(&short).unwrap();
    f.set_modified(old).ok();

    prune_old_logs(dir.path(), 7);
    assert!(short.exists(), "file with short date suffix must be kept");
}

#[test]
fn prune_keeps_file_with_invalid_date_format() {
    // Cover the "date_part has wrong number of dashes → continue" branch.
    let dir = TempDir::new().unwrap();
    let bad = dir.path().join("service.log.abcdefghij");
    fs::write(&bad, "test").unwrap();
    let old = SystemTime::now() - Duration::from_secs(30 * 86400);
    let f = fs::File::open(&bad).unwrap();
    f.set_modified(old).ok();

    prune_old_logs(dir.path(), 7);
    assert!(bad.exists(), "file with non-date suffix must be kept");
}

#[test]
fn prune_keeps_recent_rotated_file() {
    // Cover the "mtime >= cutoff → keep" branch.
    let dir = TempDir::new().unwrap();
    let recent = dir.path().join("service.log.2026-05-22");
    fs::write(&recent, "test").unwrap();
    // mtime defaults to now, so it should be kept.
    prune_old_logs(dir.path(), 7);
    assert!(recent.exists());
}

#[test]
fn prune_handles_nonexistent_dir() {
    // Cover the early-return branch when read_dir fails.
    let dir = PathBuf::from("/nonexistent/prune/test/dir/coverage");
    prune_old_logs(&dir, 7);
    // No assertion needed — verifying it does not panic.
}

#[test]
fn prune_handles_empty_dir() {
    let dir = TempDir::new().unwrap();
    prune_old_logs(dir.path(), 7);
}

// ── service_marker ──────────────────────────────────────────────────────

#[test]
fn service_marker_write_creates_missing_parent_dir() {
    // Cover the `create_dir_all(parent)?` branch in `write` when the parent
    // directory does not exist yet.
    use busytok_config::service_marker;

    let tmp = TempDir::new().unwrap();
    // Use a nested path under tmp so the parent doesn't exist.
    let data_dir = tmp.path().join("nested/deep/data");
    assert!(!data_dir.exists());

    let path = service_marker::write(&data_dir).expect("write ok");
    assert!(path.exists());
    assert!(service_marker::exists(&data_dir));

    // Cleanup.
    service_marker::remove(&data_dir).expect("remove ok");
    assert!(!service_marker::exists(&data_dir));
}

#[test]
fn service_marker_marker_path_under_data_dir() {
    use busytok_config::service_marker;

    let tmp = TempDir::new().unwrap();
    let path = service_marker::marker_path(tmp.path());
    assert_eq!(path, tmp.path().join("service.ready"));
}

#[test]
fn service_marker_remove_when_missing_is_ok() {
    // Cover the `Err(NotFound) → Ok(())` branch in `remove`.
    use busytok_config::service_marker;

    let tmp = TempDir::new().unwrap();
    service_marker::remove(tmp.path()).expect("remove is ok when missing");
    assert!(!service_marker::exists(tmp.path()));
}

#[test]
fn service_marker_write_then_exists_then_remove() {
    use busytok_config::service_marker;

    let tmp = TempDir::new().unwrap();
    assert!(!service_marker::exists(tmp.path()));
    service_marker::write(tmp.path()).expect("write ok");
    assert!(service_marker::exists(tmp.path()));
    service_marker::remove(tmp.path()).expect("remove ok");
    assert!(!service_marker::exists(tmp.path()));
}

#[test]
fn service_marker_write_is_idempotent() {
    use busytok_config::service_marker;

    let tmp = TempDir::new().unwrap();
    service_marker::write(tmp.path()).expect("first write");
    service_marker::write(tmp.path()).expect("second write");
    assert!(service_marker::exists(tmp.path()));
}

// ── platform SID stubs (non-Windows only) ──────────────────────────────

#[test]
fn platform_sid_stubs_return_none_on_non_windows() {
    // The unsupported module's stubs are compiled on every non-Windows
    // target (including macOS, Linux). Calling them directly is the only
    // way to cover their `None` returns — they are never reached by
    // `control_endpoint()` on Unix.
    #[cfg(not(windows))]
    {
        use busytok_config::platform::{current_logon_sid_string, current_user_sid_string};
        assert!(current_user_sid_string().is_none());
        assert!(current_logon_sid_string().is_none());
    }
    #[cfg(windows)]
    {
        // On Windows the real implementation is exercised elsewhere; skip.
    }
}

// ── sidecar paths ───────────────────────────────────────────────────────

#[test]
fn sidecar_runtime_dir_uses_override_when_provided() {
    let tmp = TempDir::new().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());

    let override_dir = "/custom/sidecar/runtime";
    let got = paths.sidecar_runtime_dir(Some(override_dir));
    assert_eq!(got, PathBuf::from(override_dir));
}

#[test]
fn sidecar_runtime_dir_uses_dev_fallback_when_none() {
    let tmp = TempDir::new().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());

    let got = paths.sidecar_runtime_dir(None);
    // Dev fallback: ${CARGO_MANIFEST_DIR}/../../../apps/pi-sidecar/dist
    assert!(got.to_string_lossy().contains("apps/pi-sidecar/dist"));
}

#[test]
fn sidecar_bundle_path_under_runtime_dir() {
    let tmp = TempDir::new().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());

    let got = paths.sidecar_bundle_path(Some("/custom"));
    assert_eq!(got, PathBuf::from("/custom/pi-sidecar.bundle.js"));
}

#[test]
fn sidecar_manifest_path_under_runtime_dir() {
    let tmp = TempDir::new().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());

    let got = paths.sidecar_manifest_path(Some("/custom"));
    assert_eq!(got, PathBuf::from("/custom/manifest.json"));
}

#[test]
fn sidecar_bundled_node_path_includes_arch() {
    let tmp = TempDir::new().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());

    let got = paths.sidecar_bundled_node_path(Some("/custom"));
    let s = got.to_string_lossy().into_owned();
    assert!(s.starts_with("/custom/node/"));
    assert!(
        s.contains(std::env::consts::ARCH),
        "path must include current arch: {s}"
    );
    assert!(s.ends_with("/node"), "path must end with /node: {s}");
}

// ── init_logging smoke test ─────────────────────────────────────────────
//
// `tracing::subscriber::set_global_default` can succeed at most once per
// process. To exercise every branch of `init_logging` we run a SINGLE test
// that calls it four times in sequence:
//   1. LogSource::Service — first call, succeeds, returns Some(LoggingGuards).
//   2. LogSource::Gui — `try_init` fails (subscriber already set), returns None.
//   3. LogSource::Cli with BUSYTOK_LOG_DIR unset — routes through
//      init_cli_logging which also fails try_init, returns None.
//   4. LogSource::Cli with BUSYTOK_LOG_DIR set — routes through the file
//      layer path; try_init also fails, returns None.
//
// Guards are kept alive for the duration of the test so the non-blocking
// worker threads do not abort the process. All four calls must run inside
// the same test because the global subscriber cannot be reset between tests.

#[test]
fn init_logging_covers_service_gui_and_cli_paths() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    fs::create_dir_all(&log_dir).unwrap();

    // ── 1. Service: first successful init. Returns guards. ────────────
    let guards_service = init_logging(&log_dir, LogSource::Service, "svc-session-1");
    assert!(
        guards_service.is_some(),
        "first init_logging (Service) must succeed and return guards"
    );
    let guards_service = guards_service.unwrap();
    assert!(guards_service.file_guard.is_some());
    assert!(
        guards_service.bootstrap_guard.is_none(),
        "Service has no bootstrap guard"
    );

    // ── 2. Gui: try_init fails (subscriber already set). Returns None. ─
    let guards_gui = init_logging(&log_dir, LogSource::Gui, "gui-session-1");
    assert!(
        guards_gui.is_none(),
        "second init_logging (Gui) must return None because the subscriber is already set"
    );

    // ── 3. Cli without BUSYTOK_LOG_DIR: init_cli_logging, try_init fails. ──
    std::env::remove_var("BUSYTOK_LOG_DIR");
    let guards_cli = init_logging(&log_dir, LogSource::Cli, "cli-session-1");
    assert!(
        guards_cli.is_none(),
        "init_logging (Cli, no BUSYTOK_LOG_DIR) must return None because the subscriber is already set"
    );

    // ── 4. Cli with BUSYTOK_LOG_DIR: file-layer path, try_init fails. ──
    let custom_log_dir = tmp.path().join("custom-cli-logs");
    fs::create_dir_all(&custom_log_dir).unwrap();
    std::env::set_var("BUSYTOK_LOG_DIR", custom_log_dir.as_os_str());
    let guards_cli_with_dir = init_logging(&log_dir, LogSource::Cli, "cli-with-dir");
    assert!(
        guards_cli_with_dir.is_none(),
        "init_logging (Cli, with BUSYTOK_LOG_DIR) must return None because the subscriber is already set"
    );
    std::env::remove_var("BUSYTOK_LOG_DIR");

    // Keep guards alive past the end of the test scope to avoid the
    // non-blocking worker threads aborting the process during teardown.
    drop(guards_service);
}
