//! Deterministic bundle-resource path resolution for the Busytok app bundle.
//!
//! Higher-level lifecycle code must NOT re-derive paths like
//! `Contents/MacOS/busytok-service` ad hoc. Everything goes through this module
//! so a future bundle restructure changes one place.
//!
//! Pure value type. No file I/O, no platform deps. Compiles everywhere.

use std::path::PathBuf;

/// Well-known name of the Busytok background service launch agent.
pub const SERVICE_LABEL: &str = "com.busytok.service";
pub const SERVICE_PLIST_FILENAME: &str = "com.busytok.service.plist";

/// Layout of the Busytok `.app` bundle, anchored at an app root.
///
/// Construct with [`BundleLayout::for_app_root`] given an absolute path like
/// `/Applications/Busytok.app`. All accessors are deterministic and produce
/// paths relative to that root following the standard macOS bundle layout:
///
/// - `<root>/Contents/MacOS/busytok-service`
/// - `<root>/Contents/MacOS/busytok-gui`
/// - `<root>/Contents/Library/LaunchAgents/com.busytok.service.plist`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleLayout {
    root: PathBuf,
}

impl BundleLayout {
    /// Build a layout anchored at the given app bundle root.
    ///
    /// The root should typically be the path to the `.app` directory itself
    /// (e.g. `/Applications/Busytok.app`), not its `Contents` child.
    pub fn for_app_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn contents(&self) -> PathBuf {
        self.root.join("Contents")
    }

    /// `<root>/Contents/MacOS/busytok-service`.
    pub fn service_binary_path(&self) -> PathBuf {
        self.contents().join("MacOS").join("busytok-service")
    }

    /// `<root>/Contents/MacOS/busytok-gui`.
    pub fn gui_binary_path(&self) -> PathBuf {
        self.contents().join("MacOS").join("busytok-gui")
    }

    /// Bundle-relative plist filename passed to `SMAppService.agent(plistName:)`.
    pub fn service_plist_name(&self) -> &'static str {
        SERVICE_PLIST_FILENAME
    }

    /// `<root>/Contents/Library/LaunchAgents/com.busytok.service.plist`.
    pub fn service_plist_path(&self) -> PathBuf {
        self.contents()
            .join("Library")
            .join("LaunchAgents")
            .join(SERVICE_PLIST_FILENAME)
    }

    /// The original app-bundle root this layout was constructed with.
    pub fn app_root(&self) -> &std::path::Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn resolves_service_plist_name_from_bundle_layout() {
        let layout = BundleLayout::for_app_root("/Applications/Busytok.app");
        assert_eq!(layout.service_plist_name(), "com.busytok.service.plist");
        assert_eq!(
            layout.service_plist_path(),
            Path::new(
                "/Applications/Busytok.app/Contents/Library/LaunchAgents/com.busytok.service.plist"
            )
        );
    }

    #[test]
    fn resolves_binary_paths_from_bundle_layout() {
        let layout = BundleLayout::for_app_root("/Applications/Busytok.app");
        assert_eq!(
            layout.service_binary_path(),
            Path::new("/Applications/Busytok.app/Contents/MacOS/busytok-service")
        );
        assert_eq!(
            layout.gui_binary_path(),
            Path::new("/Applications/Busytok.app/Contents/MacOS/busytok-gui")
        );
    }

    #[test]
    fn app_root_is_preserved() {
        let layout = BundleLayout::for_app_root("/Applications/Busytok.app");
        assert_eq!(layout.app_root(), Path::new("/Applications/Busytok.app"));
    }

    #[test]
    fn supports_relative_roots() {
        // While absolute paths are the norm in production, the layout itself
        // does not enforce absoluteness so tests can use relative roots.
        let layout = BundleLayout::for_app_root("./Busytok.app");
        assert_eq!(
            layout.service_binary_path(),
            Path::new("./Busytok.app/Contents/MacOS/busytok-service")
        );
    }
}
