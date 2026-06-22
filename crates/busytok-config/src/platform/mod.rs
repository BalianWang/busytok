//! Platform-specific helpers for IPC and security identifiers.
//!
//! On Windows we expose helpers for reading the current process token's
//! user SID (S-1-5-21-...) and logon SID (S-1-5-5-...) which we use to
//! build per-user/per-session named-pipe names and ACLs. On non-Windows
//! targets the same helpers are stubbed to return `None`, allowing the
//! config crate to compile everywhere.

#[cfg(not(windows))]
pub mod unsupported;
#[cfg(windows)]
pub mod windows;

#[cfg(not(windows))]
pub use unsupported::{current_logon_sid_string, current_user_sid_string};
#[cfg(windows)]
pub use windows::{current_logon_sid_string, current_user_sid_string};
