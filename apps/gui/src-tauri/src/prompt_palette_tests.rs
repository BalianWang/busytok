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
use crate::prompt_palette_native;

#[test]
fn accessibility_status_returns_valid_json_with_ok_field() {
    let result = prompt_palette_native::accessibility_status();
    assert!(
        result.is_object(),
        "accessibility_status must return a JSON object"
    );
    assert!(result.get("ok").is_some(), "must have 'ok' field");
}
