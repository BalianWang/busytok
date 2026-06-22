#![allow(warnings, clippy::all)]
#![allow(unexpected_cfgs)]
#![allow(dead_code)]
#![cfg_attr(test, allow(clippy::all))]
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::PlatformPaths;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub use unsupported::PlatformPaths;
#[cfg(target_os = "windows")]
pub use windows::PlatformPaths;
