//! Download progress as a whole percent (`src/features/updater/update-progress.ts`).

use crate::js;

/// `None` when the total is unknown, zero, or negative.
pub fn update_percent(downloaded_bytes: f64, total_bytes: Option<f64>) -> Option<f64> {
    let total = total_bytes.filter(|t| *t > 0.0)?;
    let percent = js::round(js::max(0.0, downloaded_bytes) / total * 100.0);
    Some(js::min(100.0, percent))
}

// PORT STATUS: confidence=high todos=0
