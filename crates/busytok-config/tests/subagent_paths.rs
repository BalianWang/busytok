#![allow(clippy::unwrap_used)]

use busytok_config::BusytokPaths;

#[test]
fn artifacts_dir_lives_under_data_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    assert_eq!(paths.artifacts_dir(), paths.data_dir().join("artifacts"));
}

#[test]
fn ensure_dirs_exist_creates_artifacts_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = BusytokPaths::for_test(tmp.path());
    paths.ensure_dirs_exist().unwrap();
    assert!(paths.artifacts_dir().is_dir());
}
