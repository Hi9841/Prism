//! One launch at a time. The palette stays open until `ShellExecuteW` returns
//! (the new process inherits Prism's foreground rights), and on a slow or busy
//! PC that can take seconds. Without this gate each extra Enter press in that
//! time queued one more launch, and all of them opened when Windows caught up.

use std::cell::Cell;
use std::rc::Rc;

#[derive(Default)]
struct GateState {
    current: Cell<Option<u64>>,
    issued: Cell<u64>,
}

/// Admits one launch at a time. A fresh palette open calls [`LaunchGate::reset`],
/// so a launch that never returns cannot block launches for good.
#[derive(Clone, Default)]
pub struct LaunchGate {
    state: Rc<GateState>,
}

/// Holds the gate while a launch runs; dropping it (the task ended or was
/// cancelled) admits the next launch.
pub struct LaunchTicket {
    state: Rc<GateState>,
    id: u64,
}

impl LaunchGate {
    /// A ticket for a new launch, or `None` while one is in flight.
    pub fn try_begin(&self) -> Option<LaunchTicket> {
        if self.state.current.get().is_some() {
            return None;
        }
        let id = self.state.issued.get() + 1;
        self.state.issued.set(id);
        self.state.current.set(Some(id));
        Some(LaunchTicket { state: self.state.clone(), id })
    }

    pub fn reset(&self) {
        self.state.current.set(None);
    }
}

impl Drop for LaunchTicket {
    fn drop(&mut self) {
        // A ticket from before a reset must not release a newer launch.
        if self.state.current.get() == Some(self.id) {
            self.state.current.set(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admits_one_launch_until_it_ends() {
        let gate = LaunchGate::default();
        let first = gate.try_begin().expect("first launch admitted");
        for _ in 0..29 {
            assert!(gate.try_begin().is_none(), "a repeat press while the launch runs is ignored");
        }
        drop(first);
        assert!(gate.try_begin().is_some(), "the next launch is admitted after the first ends");
    }

    #[test]
    fn reset_admits_again_and_a_stale_ticket_keeps_the_new_launch() {
        let gate = LaunchGate::default();
        let stale = gate.try_begin().expect("first launch admitted");
        gate.reset();
        let fresh = gate.try_begin().expect("a reopen admits a new launch");
        drop(stale);
        assert!(gate.try_begin().is_none(), "the stale ticket did not release the new launch");
        drop(fresh);
        assert!(gate.try_begin().is_some());
    }
}
