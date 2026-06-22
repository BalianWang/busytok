//! Windows PlatformPaths — Task Scheduler paths.
//!
//! Phase 1 lands a stub that compiles and exposes the right API surface;
//! full schtasks behavior activates in Phase 4.

use busytok_config::BusytokPaths;
use std::path::PathBuf;

pub struct PlatformPaths;

impl PlatformPaths {
    pub fn new() -> Self {
        Self
    }

    /// Task Scheduler path identifying the Busytok service task.
    pub fn service_identifier(&self) -> &'static str {
        r"\Busytok\Service"
    }

    /// `%LocalAppData%\Busytok` — data/metadata root (NOT the binary install dir).
    pub fn service_install_root(&self) -> PathBuf {
        self.local_app_data().join("Busytok")
    }

    /// `service_install_root()\scheduled-task.xml`.
    pub fn service_definition_path(&self) -> PathBuf {
        self.service_install_root().join("scheduled-task.xml")
    }

    pub fn busytok_data_dir(&self) -> PathBuf {
        BusytokPaths::new().data_dir().clone()
    }

    pub fn busytok_db_path(&self) -> PathBuf {
        BusytokPaths::new().db_path()
    }

    fn local_app_data(&self) -> PathBuf {
        dirs::data_local_dir()
            .expect("could not resolve %LocalAppData% — Busytok requires a per-user install")
    }
}

impl Default for PlatformPaths {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identifier_is_busytok_service_path() {
        assert_eq!(
            PlatformPaths::new().service_identifier(),
            r"\Busytok\Service"
        );
    }

    #[test]
    fn service_definition_path_under_install_root() {
        let p = PlatformPaths::new();
        assert!(p
            .service_definition_path()
            .starts_with(p.service_install_root()));
        assert!(p
            .service_definition_path()
            .to_string_lossy()
            .ends_with("scheduled-task.xml"));
    }
}
