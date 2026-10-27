//! How often the updater may check (`src/features/updater/update-policy.ts`).

pub const MIN_BACKGROUND_CHECK_INTERVAL_MS: f64 = 5.0 * 60.0 * 1000.0;
/// Even explicit opens must not turn Win-key spam into repeated no-cache
/// requests. Forced checks keep their freshness guarantee without firing a
/// network request on every single palette presentation.
pub const MIN_FORCED_CHECK_INTERVAL_MS: f64 = 60.0 * 1000.0;

pub fn should_check_for_update(last_check_at: f64, now: f64, force: bool) -> bool {
    let interval = if force { MIN_FORCED_CHECK_INTERVAL_MS } else { MIN_BACKGROUND_CHECK_INTERVAL_MS };
    now - last_check_at >= interval
}

// PORT STATUS: confidence=high todos=0
