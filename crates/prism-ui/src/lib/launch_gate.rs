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
