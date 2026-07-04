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
fn subagent_status_as_str_and_fromstr_roundtrip_all_variants() {
    for s in ["hot", "warm", "cold", "deleted"] {
        let parsed: SubagentStatus = s.parse().unwrap();
        assert_eq!(parsed.as_str(), s);
    }
    assert_eq!(SubagentStatus::Hot.as_str(), "hot");
    assert_eq!(SubagentStatus::Cold.as_str(), "cold");
    assert_eq!(SubagentStatus::Deleted.as_str(), "deleted");
    // Unknown status string yields a parse error mentioning the value.
    let err = "frozen".parse::<SubagentStatus>().unwrap_err();
    assert!(err.contains("frozen"));
}

#[test]
fn task_status_parses_known_values() {
    assert_eq!("queued".parse::<TaskStatus>().unwrap(), TaskStatus::Queued);
    assert_eq!(TaskStatus::Completed.as_str(), "completed");
}

#[test]
fn task_status_as_str_and_fromstr_roundtrip_all_variants() {
    for s in ["queued", "running", "completed", "failed", "cancelled"] {
        let parsed: TaskStatus = s.parse().unwrap();
        assert_eq!(parsed.as_str(), s);
    }
    assert_eq!(TaskStatus::Running.as_str(), "running");
    assert_eq!(TaskStatus::Failed.as_str(), "failed");
    assert_eq!(TaskStatus::Cancelled.as_str(), "cancelled");
    let err = "exploded".parse::<TaskStatus>().unwrap_err();
    assert!(err.contains("exploded"));
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
        prompt_artifact_ref: None,
        timeout_seconds: None,
        model_override: None,
        source_harness: None,
        source_session_id: None,
        bound_provider_id: None,
        bound_model_id: None,
    };
    assert_eq!(req.subagent_name, "reviewer");
    assert!(LogicalSubagent::is_valid_name(&req.subagent_name));
    assert!(!LogicalSubagent::is_valid_name(""));
    assert!(!LogicalSubagent::is_valid_name("has space"));
}

#[test]
fn is_valid_name_edge_cases() {
    // leading dot rejected
    assert!(!LogicalSubagent::is_valid_name(".hidden"));
    // too long (65 chars) rejected, 64 accepted
    let exactly_64 = "a".repeat(64);
    let too_long = "a".repeat(65);
    assert!(LogicalSubagent::is_valid_name(&exactly_64));
    assert!(!LogicalSubagent::is_valid_name(&too_long));
    // allowed punctuation
    assert!(LogicalSubagent::is_valid_name("review.bot-v2"));
    // disallowed punctuation
    assert!(!LogicalSubagent::is_valid_name("review bot"));
    assert!(!LogicalSubagent::is_valid_name("review!"));
    // single char allowed
    assert!(LogicalSubagent::is_valid_name("a"));
}
