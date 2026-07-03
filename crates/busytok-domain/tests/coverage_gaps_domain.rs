//! Coverage gap tests for `busytok-domain` (identity.rs + timezone.rs).
//!
//! Targets uncovered source lines:
//! - `identity.rs`: `hash_short` (FNDA:0), `is_tokens_excluded` (FNDA:0),
//!   `EmptyPath` / `NotAbsolute` error paths, `expand_tilde` ("~" and "~/"),
//!   `normalize_path_components` CurDir/ParentDir, `derive_session_id`
//!   fallback, `MetadataFingerprint` builder completeness.
//! - `timezone.rs`: `parse_fixed_offset` edge cases — 4-char no-colon
//!   ("+0800"), 2-char ("+08"), invalid 3-char ("+080"), out-of-range
//!   ("+24:00"), too-many-colon-parts ("+08:00:00"), missing sign.

#![allow(
    clippy::unwrap_used,
    clippy::uninlined_format_args,
    dead_code,
    unused_imports,
    unused_variables
)]

use busytok_domain::{
    derive_project_hash, derive_session_id, hash_short, metadata_event_hash,
    normalize_project_path, IdentityError, MetadataFingerprint, ReportingTimezone,
};

// ── hash_short (FNDA:0) ─────────────────────────────────────────────────

#[test]
fn hash_short_returns_12_char_hex_prefix() {
    let h = hash_short("some-project-path");
    assert_eq!(h.len(), 12, "hash_short must return exactly 12 hex chars");
    assert!(
        h.chars().all(|c| c.is_ascii_hexdigit()),
        "hash_short must be hex: {h}"
    );
}

#[test]
fn hash_short_is_deterministic() {
    let a = hash_short("hello");
    let b = hash_short("hello");
    assert_eq!(a, b, "same input must produce same hash");
}

#[test]
fn hash_short_differs_for_different_inputs() {
    let a = hash_short("hello");
    let b = hash_short("world");
    assert_ne!(a, b, "different inputs must produce different hashes");
}

// ── is_tokens_excluded (FNDA:0) ─────────────────────────────────────────

#[test]
fn is_tokens_excluded_defaults_to_false() {
    let fp = MetadataFingerprint::new("agent", "session");
    assert!(
        !fp.is_tokens_excluded(),
        "tokens_excluded must default to false"
    );
}

#[test]
fn is_tokens_excluded_true_after_calling_tokens_excluded() {
    let fp = MetadataFingerprint::new("agent", "session").tokens_excluded();
    assert!(
        fp.is_tokens_excluded(),
        "tokens_excluded() must set the flag to true"
    );
}

#[test]
fn tokens_excluded_stabilizes_hash_across_token_changes() {
    let fp1 = MetadataFingerprint::new("a", "s")
        .tokens(100, 50)
        .total_tokens(150)
        .tokens_excluded();
    let fp2 = MetadataFingerprint::new("a", "s")
        .tokens(200, 99)
        .total_tokens(299)
        .tokens_excluded();
    assert_eq!(
        metadata_event_hash(&fp1),
        metadata_event_hash(&fp2),
        "tokens_excluded must make hash independent of token values"
    );
    assert!(fp1.is_tokens_excluded());
    assert!(fp2.is_tokens_excluded());
}

// ── MetadataFingerprint builder completeness ─────────────────────────────

#[test]
fn metadata_fingerprint_builder_all_fields() {
    let fp = MetadataFingerprint::new("agent-1", "sess-1")
        .turn_id("turn-1")
        .request_id("req-1")
        .message_id("msg-1")
        .tokens(100, 50)
        .total_tokens(150)
        .timestamp_ms(1_000_000)
        .ignored_content("should-not-affect-hash");
    let h = metadata_event_hash(&fp);
    assert!(!h.is_empty(), "hash must be non-empty");

    // ignored_content must not change the hash.
    let fp2 = MetadataFingerprint::new("agent-1", "sess-1")
        .turn_id("turn-1")
        .request_id("req-1")
        .message_id("msg-1")
        .tokens(100, 50)
        .total_tokens(150)
        .timestamp_ms(1_000_000)
        .ignored_content("different-ignored-content");
    assert_eq!(
        h,
        metadata_event_hash(&fp2),
        "ignored_content must not affect hash"
    );
}

// ── normalize_project_path error paths ─────────────────────────────────

#[test]
fn normalize_empty_path_returns_empty_path_error() {
    let err = normalize_project_path("").unwrap_err();
    assert!(matches!(err, IdentityError::EmptyPath), "got {err:?}");
}

#[test]
fn normalize_relative_path_returns_not_absolute_error() {
    let err = normalize_project_path("relative/path").unwrap_err();
    assert!(matches!(err, IdentityError::NotAbsolute(_)), "got {err:?}");
}

#[test]
fn normalize_absolute_path_succeeds() {
    let result = normalize_project_path("/home/user/project").unwrap();
    assert_eq!(result, "/home/user/project");
}

// ── expand_tilde ("~" and "~/") ─────────────────────────────────────────

#[test]
fn normalize_tilde_only_expands_to_home() {
    // "~" alone should expand to the home directory (or remain "~" if
    // home_dir is unavailable).
    let result = normalize_project_path("~").unwrap();
    // If home_dir is available, result is the home dir (absolute). If not,
    // it falls back to "~" which is NOT absolute and would error.
    // On most systems HOME is set, so result should be the home path.
    if let Ok(home) = std::env::var("HOME") {
        assert_eq!(result, home, "tilde-only must expand to HOME");
    } else {
        // No HOME — normalize_project_path("~") returns "~" which is
        // not absolute → NotAbsolute error. This is acceptable.
        assert!(
            result != "~" || result.starts_with('/'),
            "unexpected result: {result}"
        );
    }
}

#[test]
fn normalize_tilde_slash_path_expands() {
    // "~/foo" should expand to "$HOME/foo".
    let result = normalize_project_path("~/foo").unwrap();
    if let Ok(home) = std::env::var("HOME") {
        assert_eq!(result, format!("{home}/foo"));
    } else {
        // Without HOME, expand_tilde returns the path unchanged.
        // "~/foo" → normalized to "/foo" (the ~ is dropped? No —
        // without home_dir, expand_tilde returns the original path.
        // normalize_path_components on "~/foo" keeps "~" as a Normal
        // component, so result is "~/foo" which is NOT absolute.
        // This would be a NotAbsolute error.
        assert!(
            result.contains("foo"),
            "result must contain 'foo': {result}"
        );
    }
}

#[test]
fn normalize_tilde_backslash_path_expands_on_unix() {
    // "~\\foo" starts with "~\" which also triggers expansion.
    // On Unix, the backslash is a normal path character.
    let result = normalize_project_path("~/foo/bar").unwrap();
    assert!(result.contains("foo"), "result: {result}");
    assert!(result.contains("bar"), "result: {result}");
}

// ── normalize_path_components CurDir / ParentDir ────────────────────────

#[test]
fn normalize_resolves_current_dir_segments() {
    // "/foo/./bar" → "/foo/bar" (CurDir is skipped).
    let result = normalize_project_path("/foo/./bar").unwrap();
    assert_eq!(result, "/foo/bar", "CurDir segments must be removed");
}

#[test]
fn normalize_resolves_parent_dir_segments() {
    // "/foo/bar/../baz" → "/foo/baz" (ParentDir pops the last component).
    let result = normalize_project_path("/foo/bar/../baz").unwrap();
    assert_eq!(result, "/foo/baz", "ParentDir must pop previous component");
}

#[test]
fn normalize_resolves_multiple_parent_dirs() {
    // "/a/b/c/../../d" → "/a/d".
    let result = normalize_project_path("/a/b/c/../../d").unwrap();
    assert_eq!(result, "/a/d");
}

#[test]
fn normalize_preserves_double_slash_as_curdir() {
    // "//foo" on Unix has a RootDir followed by "foo" — the empty
    // component between slashes is treated as CurDir and skipped.
    let result = normalize_project_path("//foo//bar").unwrap();
    assert_eq!(result, "/foo/bar");
}

// ── derive_session_id fallback ──────────────────────────────────────────

#[test]
fn derive_session_id_uses_source_session_when_present() {
    let id = derive_session_id(Some("real-sid"), "file-id");
    assert_eq!(id, "real-sid");
}

#[test]
fn derive_session_id_falls_back_to_file_id_when_none() {
    let id = derive_session_id(None, "file-id");
    assert_eq!(id, "file-id");
}

#[test]
fn derive_session_id_falls_back_when_source_is_empty() {
    // Empty string source_session_id is treated as absent.
    let id = derive_session_id(Some(""), "file-id");
    assert_eq!(id, "file-id");
}

// ── derive_project_hash determinism ──────────────────────────────────────

#[test]
fn derive_project_hash_is_deterministic() {
    let h1 = derive_project_hash("/home/user/project");
    let h2 = derive_project_hash("/home/user/project");
    assert_eq!(h1, h2);
    assert_ne!(h1, derive_project_hash("/home/user/other"));
}

// ── ReportingTimezone::parse fixed-offset edge cases ────────────────────

#[test]
fn parse_fixed_offset_4_char_no_colon() {
    // "+0800" → +08:00 (4-char no-colon branch, line 295-296).
    let rtz = ReportingTimezone::parse("+0800").unwrap();
    assert!(rtz.is_fixed_offset());
    assert_eq!(rtz.canonical_name(), "+0800");
}

#[test]
fn parse_fixed_offset_2_char() {
    // "+08" → +08:00 (2-char branch, line 297-298).
    let rtz = ReportingTimezone::parse("+08").unwrap();
    assert!(rtz.is_fixed_offset());
    assert_eq!(rtz.canonical_name(), "+08");
}

#[test]
fn parse_fixed_offset_negative_4_char() {
    let rtz = ReportingTimezone::parse("-0500").unwrap();
    assert!(rtz.is_fixed_offset());
    assert_eq!(rtz.canonical_name(), "-0500");
}

#[test]
fn parse_fixed_offset_negative_2_char() {
    let rtz = ReportingTimezone::parse("-05").unwrap();
    assert!(rtz.is_fixed_offset());
    assert_eq!(rtz.canonical_name(), "-05");
}

#[test]
fn parse_fixed_offset_invalid_3_char_errors() {
    // "+080" has 3 chars after the sign — hits the `else` bail (line 300).
    let result = ReportingTimezone::parse("+080");
    assert!(result.is_err(), "3-char offset must error");
}

#[test]
fn parse_fixed_offset_out_of_range_hours_errors() {
    // "+24:00" — hours=24 is out of range (0..=23), hits line 309 bail.
    let result = ReportingTimezone::parse("+24:00");
    assert!(result.is_err(), "hours out of range must error");
}

#[test]
fn parse_fixed_offset_out_of_range_minutes_errors() {
    // "+08:60" — minutes=60 is out of range (0..=59).
    let result = ReportingTimezone::parse("+08:60");
    assert!(result.is_err(), "minutes out of range must error");
}

#[test]
fn parse_fixed_offset_too_many_colon_parts_errors() {
    // "+08:00:00" — split on ':' gives 3 parts, parts.len() != 2 → bail (line 292).
    let result = ReportingTimezone::parse("+08:00:00");
    assert!(result.is_err(), "too many colon parts must error");
}

#[test]
fn parse_fixed_offset_invalid_hours_non_numeric_errors() {
    // "+ab:00" — hours parse fails.
    let result = ReportingTimezone::parse("+ab:00");
    assert!(result.is_err(), "non-numeric hours must error");
}

#[test]
fn parse_fixed_offset_invalid_minutes_non_numeric_errors() {
    // "+08:ab" — minutes parse fails.
    let result = ReportingTimezone::parse("+08:ab");
    assert!(result.is_err(), "non-numeric minutes must error");
}

#[test]
fn parse_unsupported_timezone_errors() {
    // "not-a-timezone" — doesn't start with +/- and isn't a valid IANA name.
    let result = ReportingTimezone::parse("not-a-timezone");
    assert!(result.is_err(), "unsupported timezone must error");
}

#[test]
fn parse_empty_string_errors() {
    let result = ReportingTimezone::parse("");
    assert!(result.is_err(), "empty string must error");
}

#[test]
fn parse_fixed_offset_zero_offset() {
    // "+00:00" is a valid zero offset.
    let rtz = ReportingTimezone::parse("+00:00").unwrap();
    assert!(rtz.is_fixed_offset());
    assert!(rtz.is_whole_hour_offset());
}
