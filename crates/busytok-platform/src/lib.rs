#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported;

#[cfg(target_os = "macos")]
pub use macos::PlatformPaths;
#[cfg(target_os = "windows")]
pub use windows::PlatformPaths;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub use unsupported::PlatformPaths;
