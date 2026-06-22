#![allow(
    clippy::await_holding_lock,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::field_reassign_with_default,
    clippy::uninlined_format_args,
    clippy::inconsistent_digit_grouping,
    clippy::len_zero,
    clippy::identity_op,
    clippy::useless_vec,
    clippy::manual_dangling_ptr,
    clippy::unwrap_used,
    clippy::unused_async,
    clippy::absurd_extreme_comparisons,
    unused_variables,
    unused_imports,
    dead_code
)]
//! CI-limited schtasks integration test. Windows-only.
//!
//! Creates a task under test name \Busytok\Test-{pid}-{counter}, verifies
//! query, deletes. Never uses production task name. Never starts
//! busytok-service.exe.

#![cfg(target_os = "windows")]

use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

const TEST_TASK_PREFIX: &str = r"\Busytok\Test";

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn test_task_name() -> String {
    let pid = std::process::id();
    let counter = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{TEST_TASK_PREFIX}-{pid}-{counter}")
}

struct TaskGuard(String);
impl Drop for TaskGuard {
    fn drop(&mut self) {
        let _ = Command::new("schtasks")
            .args(["/Delete", "/TN", &self.0, "/F"])
            .output();
    }
}

#[test]
fn schtasks_create_query_delete_roundtrip() {
    let task = test_task_name();
    let _guard = TaskGuard(task.clone());

    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Actions>
    <Exec>
      <Command>C:\Windows\System32\cmd.exe</Command>
      <Arguments>/c exit 0</Arguments>
    </Exec>
  </Actions>
</Task>"#
    );
    let tmp = std::env::temp_dir().join(format!("busytok-test-{}.xml", std::process::id()));
    std::fs::write(&tmp, xml).unwrap();

    let create = Command::new("schtasks")
        .args([
            "/Create",
            "/TN",
            &task,
            "/XML",
            &tmp.display().to_string(),
            "/F",
        ])
        .output()
        .unwrap();
    assert!(
        create.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&create.stderr)
    );

    let query = Command::new("schtasks")
        .args(["/Query", "/TN", &task, "/FO", "CSV", "/V"])
        .output()
        .unwrap();
    assert!(query.status.success(), "query failed");
    let stdout = String::from_utf8_lossy(&query.stdout);
    assert!(stdout.contains("Ready") || stdout.contains("Running"));

    let delete = Command::new("schtasks")
        .args(["/Delete", "/TN", &task, "/F"])
        .output()
        .unwrap();
    assert!(delete.status.success());
}
