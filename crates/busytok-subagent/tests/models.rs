#![allow(clippy::unwrap_used)]

use busytok_subagent::models::{DelegateRequest, LogicalSubagent, SubagentStatus, TaskStatus};

#[test]
fn subagent_status_parses_known_values() {
    assert_eq!(
        "hot".parse::<SubagentStatus>().unwrap(),
        SubagentStatus::Hot
    );
    assert_eq!(SubagentStatus::Warm.as_str(), "warm");
    assert!("bogus".parse::<SubagentStatus>().is_err());
}

#[test]
fn task_status_parses_known_values() {
    assert_eq!("queued".parse::<TaskStatus>().unwrap(), TaskStatus::Queued);
    assert_eq!(TaskStatus::Completed.as_str(), "completed");
}

#[test]
fn delegate_request_requires_subagent_and_prompt() {
    let req = DelegateRequest {
        subagent_name: "reviewer".to_string(),
        subagent_id: None,
        cwd: "/tmp/repo".to_string(),
        profile: "pi/search-cheap".to_string(),
        intent: None,
        prompt: "find it".to_string(),
        timeout_seconds: None,
        model_override: None,
        source_harness: None,
        source_session_id: None,
    };
    assert_eq!(req.subagent_name, "reviewer");
    assert!(LogicalSubagent::is_valid_name(&req.subagent_name));
    assert!(!LogicalSubagent::is_valid_name(""));
    assert!(!LogicalSubagent::is_valid_name("has space"));
}
