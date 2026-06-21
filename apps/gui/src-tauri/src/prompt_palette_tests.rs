use crate::prompt_palette_native;

#[test]
fn accessibility_status_returns_valid_json_with_ok_field() {
    let result = prompt_palette_native::accessibility_status();
    assert!(
        result.is_object(),
        "accessibility_status must return a JSON object"
    );
    assert!(
        result.get("ok").is_some(),
        "must have 'ok' field"
    );
}
