//! Time utilities shared across all Busytok crates.

/// Current time in milliseconds since Unix epoch.
///
/// Used throughout the system for timestamp fields. Returns 0 if the system
/// clock is before the epoch (which should never happen on a functioning host).
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
