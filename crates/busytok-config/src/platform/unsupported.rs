//! Non-Windows stub for the platform SID helpers.
//!
//! On Unix we have no concept of an NT user SID, so these always return
//! `None`. Callers in cross-platform code must already handle `None` by
//! falling back to filesystem paths.

pub fn current_user_sid_string() -> Option<String> {
    None
}

pub fn current_logon_sid_string() -> Option<String> {
    None
}
