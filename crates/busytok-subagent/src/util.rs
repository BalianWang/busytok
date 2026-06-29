//! Shared utility helpers.

use serde::de::DeserializeOwned;

/// Parse an `Option<String>` JSON blob into a `Vec<T>`. Returns an empty vec
/// when the input is `None` or fails to deserialize.
pub(crate) fn parse_json_vec<T: DeserializeOwned>(json: &Option<String>) -> Vec<T> {
    json.as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}
