//! Platform-specific prompt palette native implementations.
//!
//! On macOS, this module handles accessibility checking and paste injection.
//! On Windows, placeholder stubs are provided (actual implementation in Task 6.5).

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(target_os = "windows")]
pub use windows::*;
