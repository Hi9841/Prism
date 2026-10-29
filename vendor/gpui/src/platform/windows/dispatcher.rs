use std::{
    thread::{ThreadId, current},
    time::Duration,
};

use async_task::Runnable;
use flume::Sender;
use util::ResultExt;
use windows::{
    System::Threading::{
        ThreadPool, ThreadPoolTimer, TimerElapsedHandler, WorkItemHandler, WorkItemPriority,
    },
    Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::WindowsAndMessaging::PostMessageW,
    },
};

use crate::{
    HWND, PlatformDispatcher, SafeHwnd, TaskLabel, WM_GPUI_TASK_DISPATCHED_ON_MAIN_THREAD,
};

pub(crate) struct WindowsDispatcher {
    main_sender: Sender<Runnable>,
    main_thread_id: ThreadId,
    platform_window_handle: SafeHwnd,
    validation_number: usize,
}

impl WindowsDispatcher {
    pub(crate) fn new(
        main_sender: Sender<Runnable>,
        platform_window_handle: HWND,
        validation_number: usize,
    ) -> Self {
        let main_thread_id = current().id();
        let platform_window_handle = platform_window_handle.into();

        WindowsDispatcher {
            main_sender,
            main_thread_id,
            platform_window_handle,
            validation_number,
        }
    }

    fn dispatch_on_threadpool(&self, runnable: Runnable) {
        let handler = {
            let mut task_wrapper = Some(runnable);
            WorkItemHandler::new(move |_| {
                task_wrapper.take().unwrap().run();
                Ok(())
            })
        };
        ThreadPool::RunWithPriorityAsync(&handler, WorkItemPriority::High).log_err();
    }

    fn dispatch_on_threadpool_after(&self, runnable: Runnable, duration: Duration) {
        let handler = {
            let mut task_wrapper = Some(runnable);
            TimerElapsedHandler::new(move |_| {
                task_wrapper.take().unwrap().run();
                Ok(())
            })
        };
        ThreadPoolTimer::CreateTimer(&handler, duration.into()).log_err();
    }
}

impl PlatformDispatcher for WindowsDispatcher {
    fn is_main_thread(&self) -> bool {
        current().id() == self.main_thread_id
    }

    fn dispatch(&self, runnable: Runnable, label: Option<TaskLabel>) {
        self.dispatch_on_threadpool(runnable);
        if let Some(label) = label {
            log::debug!("TaskLabel: {label:?}");
        }
    }

    fn dispatch_on_main_thread(&self, runnable: Runnable) {
        match self.main_sender.send(runnable) {
            Ok(_) => unsafe {
                PostMessageW(
                    Some(self.platform_window_handle.as_raw()),
                    WM_GPUI_TASK_DISPATCHED_ON_MAIN_THREAD,
                    WPARAM(self.validation_number),
                    LPARAM(0),
                )
                .log_err();
            },
            Err(runnable) => {
                // NOTE: Runnable may wrap a Future that is !Send.
                //
                // This is usually safe because we only poll it on the main thread.
                // However if the send fails, we know that:
                // 1. main_receiver has been dropped (which implies the app is shutting down)
                // 2. we are on a background thread.
                // It is not safe to drop something !Send on the wrong thread, and
                // the app will exit soon anyway, so we must forget the runnable.
                std::mem::forget(runnable);
            }
        }
    }

    fn dispatch_after(&self, duration: Duration, runnable: Runnable) {
        // PRISM PATCH 31: a thread-pool timer fires on the 15.6 ms system
        // tick. The thread pool stays as the fallback.
        match precise_timer::timers() {
            Some(timers) => timers.schedule(duration, runnable),
            None => self.dispatch_on_threadpool_after(runnable, duration),
        }
    }
}

/// Timers on one thread that waits on a high-resolution waitable timer
/// (Windows 10 1803 and later). The system timer resolution does not change.
pub(crate) mod precise_timer {
    use std::{
        cmp::Ordering,
        collections::BinaryHeap,
        sync::{Arc, Mutex, OnceLock},
        time::{Duration, Instant},
    };

    use async_task::Runnable;
    use windows::Win32::{
        Foundation::HANDLE,
        System::Threading::{
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateEventW, CreateWaitableTimerExW, INFINITE,
            SetEvent, SetWaitableTimer, TIMER_ALL_ACCESS, WaitForMultipleObjects,
        },
    };

    struct Entry {
        due: Instant,
        order: u64,
        runnable: Runnable,
    }

    // A min-heap on (due, order): the earliest timer is at the top, and
    // timers with the same due time run in the order they were set.
    impl Ord for Entry {
        fn cmp(&self, other: &Self) -> Ordering {
            (other.due, other.order).cmp(&(self.due, self.order))
        }
    }
    impl PartialOrd for Entry {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }
    impl PartialEq for Entry {
        fn eq(&self, other: &Self) -> bool {
            self.cmp(other) == Ordering::Equal
        }
    }
    impl Eq for Entry {}

    pub(crate) struct Timers {
        queue: Mutex<(BinaryHeap<Entry>, u64)>,
        /// Set when a new timer is due before the one the thread waits for.
        wake: Handle,
    }

    struct Handle(HANDLE);
    // SAFETY: an event handle can be signaled from any thread.
    unsafe impl Send for Handle {}
    unsafe impl Sync for Handle {}

    static TIMERS: OnceLock<Option<Arc<Timers>>> = OnceLock::new();

    /// The timer thread, started on first use. `None` when Windows has no
    /// high-resolution waitable timer.
    pub(crate) fn timers() -> Option<&'static Timers> {
        TIMERS.get_or_init(start).as_deref()
    }

    impl Timers {
        /// Runs `runnable` on the timer thread after `duration`.
        pub(crate) fn schedule(&self, duration: Duration, runnable: Runnable) {
            let due = Instant::now() + duration;
            let mut queue = self.queue.lock().unwrap();
            let earliest = queue.0.peek().is_none_or(|first| due < first.due);
            let order = queue.1;
            queue.1 += 1;
            queue.0.push(Entry { due, order, runnable });
            drop(queue);
            if earliest {
                unsafe { SetEvent(self.wake.0).ok() };
            }
        }
    }

    fn start() -> Option<Arc<Timers>> {
        let timer = unsafe {
            CreateWaitableTimerExW(
                None,
                None,
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
        }
        .ok()?;
        let wake = unsafe { CreateEventW(None, false, false, None) }.ok()?;
        let timers = Arc::new(Timers {
            queue: Mutex::new((BinaryHeap::new(), 0)),
            wake: Handle(wake),
        });
        let shared = timers.clone();
        let timer = Handle(timer);
        std::thread::Builder::new()
            .name("PreciseTimer".to_owned())
            .spawn(move || run(&shared, timer))
            .ok()?;
        Some(timers)
    }

    fn run(timers: &Timers, timer: Handle) {
        loop {
            let mut due = Vec::new();
            let next = {
                let mut queue = timers.queue.lock().unwrap();
                let now = Instant::now();
                while queue.0.peek().is_some_and(|first| first.due <= now) {
                    due.push(queue.0.pop().unwrap().runnable);
                }
                queue.0.peek().map(|first| first.due - now)
            };
            // A timer's runnable only completes an empty task and wakes its
            // waiter, so it is cheap to run here.
            for runnable in due {
                runnable.run();
            }
            let handles = [timers.wake.0, timer.0];
            match next {
                Some(wait) => {
                    // A negative due time is relative, in 100 ns units.
                    let relative = -((wait.as_nanos() / 100).max(1) as i64);
                    let set = unsafe { SetWaitableTimer(timer.0, &relative, 0, None, None, false) };
                    if set.is_err() {
                        std::thread::sleep(wait);
                        continue;
                    }
                    unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
                }
                None => {
                    unsafe { WaitForMultipleObjects(&handles[..1], false, INFINITE) };
                }
            }
        }
    }
}
