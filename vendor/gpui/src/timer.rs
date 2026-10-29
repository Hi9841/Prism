//! PRISM PATCH 31: a timer that fires on time on Windows.
//!
//! `smol::Timer` waits in the async-io reactor, and on Windows the reactor
//! wakes on the 15.6 ms system tick: a 35 ms timer fired after 46.9 ms, and a
//! 16 ms timer after 31 ms. On Windows this timer waits on the dispatcher's
//! high-resolution waitable timer. Elsewhere, and on a Windows without that
//! timer, it is a `smol::Timer`.

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};

/// A future that completes after a duration, with the time it completed.
pub struct Timer(Inner);

enum Inner {
    #[cfg(target_os = "windows")]
    Precise(async_task::Task<()>),
    Reactor(smol::Timer),
}

impl Timer {
    /// Completes after `duration`.
    pub fn after(duration: Duration) -> Self {
        #[cfg(target_os = "windows")]
        if let Some(timers) = crate::platform::precise_timer::timers() {
            let (runnable, task) = async_task::spawn(async {}, move |runnable| {
                timers.schedule(duration, runnable)
            });
            runnable.schedule();
            return Self(Inner::Precise(task));
        }
        Self(Inner::Reactor(smol::Timer::after(duration)))
    }
}

impl Future for Timer {
    type Output = Instant;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Instant> {
        match &mut self.0 {
            #[cfg(target_os = "windows")]
            Inner::Precise(task) => Pin::new(task).poll(cx).map(|()| Instant::now()),
            Inner::Reactor(timer) => Pin::new(timer).poll(cx),
        }
    }
}
