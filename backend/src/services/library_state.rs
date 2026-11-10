//! Atomic run admission/cancellation/termination for the library orchestrator.
//! Unlike Python's single event loop, RPCs can execute on different workers;
//! one mutex protects the lifecycle pair, with no await while it is held.
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum RunSlot {
    #[default]
    Idle,
    Running(String),
    Cancelling(String),
}

impl RunSlot {
    fn owner(&self) -> Option<&str> {
        match self { Self::Idle => None, Self::Running(id) | Self::Cancelling(id) => Some(id) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelOutcome { NoSync, Stale, Cancelling }

#[derive(Default)]
pub struct LibraryRunState { slot: Mutex<RunSlot> }

impl LibraryRunState {
    pub fn try_begin_run(&self, run_id: String) -> bool {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if *slot != RunSlot::Idle { return false; }
        *slot = RunSlot::Running(run_id);
        true
    }

    pub fn request_cancel(&self, run_id: Option<&str>) -> CancelOutcome {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        let Some(owner) = slot.owner() else { return CancelOutcome::NoSync; };
        if run_id.is_some_and(|id| !id.is_empty() && id != owner) { return CancelOutcome::Stale; }
        *slot = RunSlot::Cancelling(owner.to_string());
        CancelOutcome::Cancelling
    }

    pub fn finish_run(&self, run_id: Option<&str>) -> bool {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if slot.owner() != run_id { return false; }
        *slot = RunSlot::Idle;
        true
    }

    pub fn current_run_id(&self) -> Option<String> {
        self.slot.lock().unwrap_or_else(|e| e.into_inner()).owner().map(str::to_string)
    }

    pub fn is_in_flight(&self) -> bool {
        *self.slot.lock().unwrap_or_else(|e| e.into_inner()) != RunSlot::Idle
    }

    pub fn is_cancelling(&self) -> bool {
        matches!(*self.slot.lock().unwrap_or_else(|e| e.into_inner()), RunSlot::Cancelling(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn lifecycle_scopes_cancel_and_terminal_to_the_owner() {
        let state = LibraryRunState::default();
        assert!(!state.is_in_flight());
        assert!(!state.is_cancelling());
        assert_eq!(state.request_cancel(Some("a")), CancelOutcome::NoSync);
        assert!(state.try_begin_run("a".into()));
        assert!(!state.try_begin_run("b".into()));
        assert_eq!(state.request_cancel(Some("b")), CancelOutcome::Stale);
        assert!(!state.is_cancelling());
        assert!(!state.finish_run(Some("b")));
        assert!(!state.finish_run(None));
        assert_eq!(state.request_cancel(Some("a")), CancelOutcome::Cancelling);
        assert!(state.is_in_flight());
        assert!(state.is_cancelling());
        assert!(!state.try_begin_run("b".into()));
        assert!(state.finish_run(Some("a")));
        assert!(!state.finish_run(Some("a")));
        assert!(state.try_begin_run("b".into()));
        assert!(!state.finish_run(Some("a")));
        assert_eq!(state.current_run_id().as_deref(), Some("b"));
        assert!(!state.is_cancelling());
        assert!(state.finish_run(Some("b")));
        assert_eq!(state.current_run_id(), None);
    }

    #[test]
    fn legacy_cancel_without_id_or_with_empty_id_remains_unconditional() {
        for id in [None, Some("")] {
            let state = LibraryRunState::default();
            state.try_begin_run("owner".into());
            assert_eq!(state.request_cancel(id), CancelOutcome::Cancelling);
            assert_eq!(state.request_cancel(id), CancelOutcome::Cancelling);
            assert_eq!(state.current_run_id().as_deref(), Some("owner"));
            assert!(state.finish_run(Some("owner")));
        }
    }

    #[test]
    fn simultaneous_admissions_have_exactly_one_winner() {
        let state = Arc::new(LibraryRunState::default());
        let barrier = Arc::new(Barrier::new(16));
        let handles: Vec<_> = (0..16).map(|id| {
            let state = state.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let id = id.to_string();
                state.try_begin_run(id.clone()).then_some(id)
            })
        }).collect();
        let winners: Vec<_> = handles.into_iter().filter_map(|h| h.join().unwrap()).collect();
        assert_eq!(winners.len(), 1);
        assert_eq!(state.current_run_id().as_ref(), winners.first());
    }

    #[test]
    fn exhaustive_short_interleavings_preserve_owner_and_cancellation() {
        // Enumerate all 6^6 sequences, including stale calls after re-admission.
        for mut sequence in 0..6_usize.pow(6) {
            let state = LibraryRunState::default();
            let mut owner: Option<String> = None;
            let mut cancelling = false;
            for step in 0..6 {
                let operation = sequence % 6;
                sequence /= 6;
                let foreign = format!("run-{step}");
                match operation {
                    0 => {
                        let admitted = state.try_begin_run(foreign.clone());
                        assert_eq!(admitted, owner.is_none());
                        if admitted { owner = Some(foreign); cancelling = false; }
                    }
                    1 | 2 | 3 => {
                        let id = match operation { 1 => owner.as_deref(), 2 => Some(foreign.as_str()), _ => Some("") };
                        let expected = if owner.is_none() { CancelOutcome::NoSync }
                            else if operation == 2 { CancelOutcome::Stale }
                            else { CancelOutcome::Cancelling };
                        assert_eq!(state.request_cancel(id), expected);
                        if expected == CancelOutcome::Cancelling { cancelling = true; }
                    }
                    _ => {
                        let id = if operation == 4 { owner.as_deref() } else { Some(foreign.as_str()) };
                        let expected = id == owner.as_deref();
                        assert_eq!(state.finish_run(id), expected);
                        if expected { owner = None; cancelling = false; }
                    }
                }
                assert_eq!(state.current_run_id(), owner);
                assert_eq!(state.is_in_flight(), owner.is_some());
                assert_eq!(state.is_cancelling(), cancelling);
            }
        }
    }
}
