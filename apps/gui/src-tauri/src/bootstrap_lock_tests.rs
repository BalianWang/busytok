use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use busytok_config::BusytokPaths;

use crate::bootstrap_lock::{bootstrap_lock_path, with_bootstrap_file_lock};

#[test]
fn bootstrap_lock_path_lives_in_data_dir() {
    let root =
        std::env::temp_dir().join(format!("busytok-bootstrap-lock-{}", uuid::Uuid::new_v4()));
    let paths = BusytokPaths::for_test(&root);
    assert_eq!(
        bootstrap_lock_path(&paths),
        paths.data_dir().join("bootstrap.lock")
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn bootstrap_file_lock_serializes_threads() {
    let root =
        std::env::temp_dir().join(format!("busytok-bootstrap-lock-{}", uuid::Uuid::new_v4()));
    let paths = BusytokPaths::for_test(&root);
    paths.ensure_dirs_exist().unwrap();
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(2));

    let run = |active: Arc<AtomicUsize>,
               max_active: Arc<AtomicUsize>,
               barrier: Arc<Barrier>,
               paths: BusytokPaths| {
        std::thread::spawn(move || {
            barrier.wait();
            with_bootstrap_file_lock(&paths, || {
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                max_active.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(50));
                active.fetch_sub(1, Ordering::SeqCst);
                Ok::<_, anyhow::Error>(())
            })
            .unwrap();
        })
    };

    let first = run(
        Arc::clone(&active),
        Arc::clone(&max_active),
        Arc::clone(&barrier),
        paths.clone(),
    );
    let second = run(active, Arc::clone(&max_active), barrier, paths.clone());
    first.join().unwrap();
    second.join().unwrap();
    assert_eq!(max_active.load(Ordering::SeqCst), 1);
    let _ = std::fs::remove_dir_all(root);
}
