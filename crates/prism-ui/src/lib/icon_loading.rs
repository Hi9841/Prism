//! Which app icons to request next, and the retry schedule (`src/lib/iconLoading.ts`).

use std::collections::{HashMap, HashSet};

const APP_ICON_RETRY_DELAYS_MS: [f64; 2] = [200.0, 800.0];

pub struct AppIconRequestState<'a> {
    pub apps_loaded: bool,
    pub icons: &'a HashMap<String, String>,
    pub settled: &'a HashSet<String>,
    pub in_flight: &'a HashSet<String>,
    pub attempts: &'a HashMap<String, u32>,
}

pub fn select_app_icon_request_ids(ids: &[String], state: &AppIconRequestState) -> Vec<String> {
    if !state.apps_loaded {
        return Vec::new();
    }
    let maximum_attempts = APP_ICON_RETRY_DELAYS_MS.len() as u32 + 1;
    ids.iter()
        .filter(|id| {
            // BUG(port): `id in state.icons` also matches Object.prototype keys
            // ("constructor", "toString", ...); the port checks own keys only.
            !state.icons.contains_key(*id)
                && !state.settled.contains(*id)
                && !state.in_flight.contains(*id)
                && state.attempts.get(*id).copied().unwrap_or(0) < maximum_attempts
        })
        .cloned()
        .collect()
}

/// `APP_ICON_RETRY_DELAYS_MS[attempt - 1] ?? null`: only attempts 1 and 2 have a delay.
pub fn app_icon_retry_delay(attempt: f64) -> Option<f64> {
    let index = attempt - 1.0;
    if index == 0.0 {
        Some(APP_ICON_RETRY_DELAYS_MS[0])
    } else if index == 1.0 {
        Some(APP_ICON_RETRY_DELAYS_MS[1])
    } else {
        None
    }
}

// PORT STATUS: confidence=high todos=0
