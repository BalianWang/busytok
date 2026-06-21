use std::path::PathBuf;

use busytok_config::BusytokPaths;

/// macOS-specific platform paths for the Busytok service.
///
/// Resolves local filesystem paths and LaunchAgent identifiers only.
/// This type contains no proxy, auth, Keychain, or network functionality.
pub struct PlatformPaths {
    home_dir: Option<PathBuf>,
}

impl PlatformPaths {
    pub fn new() -> Self {
        Self { home_dir: None }
    }

    /// Create a PlatformPaths rooted at a specific home directory (for testing).
    pub fn with_home_dir(home_dir: PathBuf) -> Self {
        Self {
            home_dir: Some(home_dir),
        }
    }

    /// LaunchAgent label (also serves as the service identifier on macOS).
    pub fn service_identifier(&self) -> &'static str {
        "com.busytok.service"
    }

    /// `~/Library/LaunchAgents` — directory holding the plist.
    pub fn service_install_root(&self) -> PathBuf {
        self.resolve_home_dir().join("Library").join("LaunchAgents")
    }

    /// Full path to the LaunchAgent plist file.
    pub fn service_definition_path(&self) -> PathBuf {
        self.service_install_root()
            .join(format!("{}.plist", self.service_identifier()))
    }

    pub fn busytok_data_dir(&self) -> PathBuf {
        BusytokPaths::new().data_dir().clone()
    }

    pub fn busytok_db_path(&self) -> PathBuf {
        BusytokPaths::new().db_path()
    }

    /// Resolved home directory — either the custom home_dir set via
    /// [`with_home_dir`](Self::with_home_dir) or the system home directory.
    pub fn resolve_home_dir(&self) -> PathBuf {
        self.home_dir
            .clone()
            .unwrap_or_else(|| dirs::home_dir().expect("could not resolve home directory"))
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
    fn service_identifier_is_busytok_service() {
        assert_eq!(PlatformPaths::new().service_identifier(), "com.busytok.service");
    }

    #[test]
    fn service_install_root_is_launch_agents() {
        let p = PlatformPaths::new();
        let root = p.service_install_root();
        assert!(root.to_string_lossy().contains("Library/LaunchAgents"));
    }

    #[test]
    fn service_definition_path_is_plist_under_install_root() {
        let p = PlatformPaths::new();
        let plist = p.service_definition_path();
        assert!(plist.starts_with(p.service_install_root()));
        assert!(plist.to_string_lossy().ends_with("com.busytok.service.plist"));
    }

    #[test]
    fn busytok_data_dir_delegates_to_busytok_paths() {
        let p = PlatformPaths::new();
        assert_eq!(p.busytok_data_dir(), *busytok_config::BusytokPaths::new().data_dir());
    }

    #[test]
    fn busytok_db_path_delegates_to_busytok_paths() {
        let p = PlatformPaths::new();
        assert_eq!(p.busytok_db_path(), busytok_config::BusytokPaths::new().db_path());
    }
}
