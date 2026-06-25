use busytok_store::repository::{SubagentLogicalSubagentRow, SubagentMemoryRow, SubagentTaskRow};

#[test]
fn subagent_row_for_test_constructors_build_minimal_rows() {
    let sa = SubagentLogicalSubagentRow::for_test("sa-1", "reviewer");
    assert_eq!(sa.id, "sa-1");
    assert_eq!(sa.name, "reviewer");
    assert_eq!(sa.status, "cold");

    let mem = SubagentMemoryRow::for_test("sa-1");
    assert_eq!(mem.subagent_id, "sa-1");

    let task = SubagentTaskRow::for_test("task-1", "sa-1", "pi/search-cheap", "find the bug");
    assert_eq!(task.prompt, Some("find the bug".to_string()));
    assert_eq!(task.status, "queued");
}
