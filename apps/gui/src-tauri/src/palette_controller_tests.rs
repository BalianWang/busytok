use crate::palette_controller::PaletteControllerState;
use crate::palette_native::palette_dev_url;

#[test]
fn initial_state_is_hidden() {
    assert_eq!(
        PaletteControllerState::default(),
        PaletteControllerState::Hidden
    );
}

#[test]
fn visible_states_are_visible() {
    assert!(PaletteControllerState::Showing.is_visible());
    assert!(PaletteControllerState::Visible.is_visible());
}

#[test]
fn hidden_states_are_not_visible() {
    assert!(!PaletteControllerState::Hidden.is_visible());
    assert!(!PaletteControllerState::Hiding.is_visible());
}

#[test]
fn dev_url_is_localhost() {
    let url = palette_dev_url();
    assert!(
        url.starts_with("http://localhost:1420/"),
        "expected localhost URL, got: {url}"
    );
    assert!(
        url.contains("window=prompt-palette"),
        "expected window=prompt-palette param, got: {url}"
    );
}
