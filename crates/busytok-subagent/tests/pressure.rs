#![allow(clippy::unwrap_used, clippy::uninlined_format_args)]
use busytok_subagent::pressure::{PressureAction, PressureGate};

#[test]
fn gate_starts_unpaused_with_resume_action() {
    let gate = PressureGate::new();
    assert!(!gate.is_paused());
    assert!(matches!(gate.last_action(), Some(PressureAction::Resume)));
}

#[test]
fn pause_new_tasks_sets_paused_flag() {
    let gate = PressureGate::new();
    gate.set_action(PressureAction::PauseNewTasks);
    assert!(gate.is_paused());
    assert!(matches!(
        gate.last_action(),
        Some(PressureAction::PauseNewTasks)
    ));
}

#[test]
fn resume_clears_paused_flag() {
    let gate = PressureGate::new();
    gate.set_action(PressureAction::PauseNewTasks);
    assert!(gate.is_paused());
    gate.set_action(PressureAction::Resume);
    assert!(!gate.is_paused());
}

#[test]
fn hibernate_lru_does_not_pause() {
    let gate = PressureGate::new();
    gate.set_action(PressureAction::HibernateLru);
    assert!(!gate.is_paused());
}

#[test]
fn force_kill_sets_paused() {
    let gate = PressureGate::new();
    gate.set_action(PressureAction::ForceKill);
    assert!(gate.is_paused());
}
