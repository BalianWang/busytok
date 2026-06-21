//! Fallback PlatformPaths used when target is neither macOS nor Windows
//! (e.g. Ubuntu CI). Methods return placeholder paths under
//! `BusytokPaths::data_dir()` so the workspace links; runtime calls are
//! never reached on unsupported targets.

use std::path::PathBuf;
use busytok_config::BusytokPaths;

pub struct PlatformPaths;

impl PlatformPaths {
    pub fn new() -> Self { Self }

    pub fn service_identifier(&self) -> &'static str {
        "busytok-unsupported"
    }

    pub fn service_install_root(&self) -> PathBuf {
        self.data_root()
    }

    pub fn service_definition_path(&self) -> PathBuf {
        self.data_root().join("scheduled-task.xml")
    }

    pub fn busytok_data_dir(&self) -> PathBuf {
        BusytokPaths::new().data_dir().clone()
    }

    pub fn busytok_db_path(&self) -> PathBuf {
        BusytokPaths::new().db_path()
    }

    fn data_root(&self) -> PathBuf {
        BusytokPaths::new().data_dir().join("unsupported")
    }
}

impl Default for PlatformPaths {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identifier_is_unsupported() {
        assert_eq!(PlatformPaths::new().service_identifier(), "busytok-unsupported");
    }

    #[test]
    fn service_definition_path_under_data_root() {
        let p = PlatformPaths::new();
        assert!(p.service_definition_path().starts_with(p.service_install_root()));
    }

    #[test]
    fn data_dir_delegates_to_busytok_paths() {
        let p = PlatformPaths::new();
        assert_eq!(p.busytok_data_dir(), *BusytokPaths::new().data_dir());
    }
}
