use std::{
    sync::{Condvar, LazyLock, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use util::ResultExt;
use windows::Win32::{
    Foundation::HWND,
    Graphics::Dwm::{DWM_TIMING_INFO, DwmFlush, DwmGetCompositionTimingInfo},
    System::Performance::QueryPerformanceFrequency,
};

static QPC_TICKS_PER_SECOND: LazyLock<u64> = LazyLock::new(|| {
    let mut frequency = 0;
    // On systems that run Windows XP or later, the function will always succeed and
    // will thus never return zero.
    unsafe { QueryPerformanceFrequency(&mut frequency).unwrap() };
    frequency as u64
});

const VSYNC_INTERVAL_THRESHOLD: Duration = Duration::from_millis(1);
const DEFAULT_VSYNC_INTERVAL: Duration = Duration::from_micros(16_666); // ~60Hz

// Prism patch 33: the vsync thread waits here while no window is visible.
// A hidden window gets no `WM_PAINT`, so a vsync wakeup does no work for it.
/// A window is about to show. The window procedure sets it; the vsync thread takes it.
static WINDOW_SHOWN: Mutex<bool> = Mutex::new(false);
static WINDOW_SHOWN_SIGNAL: Condvar = Condvar::new();
/// The vsync thread keeps its pace this long after a show: `WM_SHOWWINDOW`
/// arrives before the window is visible, and the first frames follow it.
pub(crate) const WINDOW_SHOWN_GRACE: Duration = Duration::from_secs(1);
/// Safety net: a show that sent no signal delays its second frame by at most this.
const HIDDEN_WAIT: Duration = Duration::from_secs(1);

/// A window is about to show: the vsync thread must run again.
pub(crate) fn notify_window_shown() {
    *WINDOW_SHOWN.lock().unwrap_or_else(|error| error.into_inner()) = true;
    WINDOW_SHOWN_SIGNAL.notify_one();
}

/// Takes the show signal. True when a window was shown since the last call.
pub(crate) fn take_window_shown() -> bool {
    std::mem::take(&mut *WINDOW_SHOWN.lock().unwrap_or_else(|error| error.into_inner()))
}

/// Waits for `notify_window_shown`, at most `HIDDEN_WAIT`. True when a window was shown.
pub(crate) fn wait_for_window_shown() -> bool {
    let shown = WINDOW_SHOWN.lock().unwrap_or_else(|error| error.into_inner());
    let (mut shown, _) = WINDOW_SHOWN_SIGNAL
        .wait_timeout_while(shown, HIDDEN_WAIT, |shown| !*shown)
        .unwrap_or_else(|error| error.into_inner());
    std::mem::take(&mut *shown)
}

pub(crate) struct VSyncProvider {
    interval: Duration,
    f: Box<dyn Fn() -> bool>,
}

impl VSyncProvider {
    pub(crate) fn new() -> Self {
        let interval = get_dwm_interval()
            .context("Failed to get DWM interval")
            .log_err()
            .unwrap_or(DEFAULT_VSYNC_INTERVAL);
        let f = Box::new(|| unsafe { DwmFlush().is_ok() });
        Self { interval, f }
    }

    pub(crate) fn wait_for_vsync(&self) {
        let vsync_start = Instant::now();
        let wait_succeeded = (self.f)();
        let elapsed = vsync_start.elapsed();
        // DwmFlush and DCompositionWaitForCompositorClock returns very early
        // instead of waiting until vblank when the monitor goes to sleep or is
        // unplugged (nothing to present due to desktop occlusion). We use 1ms as
        // a threshold for the duration of the wait functions and fallback to
        // Sleep() if it returns before that. This could happen during normal
        // operation for the first call after the vsync thread becomes non-idle,
        // but it shouldn't happen often.
        if !wait_succeeded || elapsed < VSYNC_INTERVAL_THRESHOLD {
            log::trace!("VSyncProvider::wait_for_vsync() took less time than expected");
            std::thread::sleep(self.interval);
        }
    }
}

fn get_dwm_interval() -> Result<Duration> {
    let mut timing_info = DWM_TIMING_INFO {
        cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32,
        ..Default::default()
    };
    unsafe { DwmGetCompositionTimingInfo(HWND::default(), &mut timing_info) }?;
    let interval = retrieve_duration(timing_info.qpcRefreshPeriod, *QPC_TICKS_PER_SECOND);
    // Check for interval values that are impossibly low. A 29 microsecond
    // interval was seen (from a qpcRefreshPeriod of 60).
    if interval < VSYNC_INTERVAL_THRESHOLD {
        Ok(retrieve_duration(
            timing_info.rateRefresh.uiDenominator as u64,
            timing_info.rateRefresh.uiNumerator as u64,
        ))
    } else {
        Ok(interval)
    }
}

#[inline]
fn retrieve_duration(counts: u64, ticks_per_second: u64) -> Duration {
    let ticks_per_microsecond = ticks_per_second / 1_000_000;
    Duration::from_micros(counts / ticks_per_microsecond)
}
