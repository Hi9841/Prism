//! Launch counts per app. Search uses them to order apps inside one match
//! tier (see `search_apps`). The original had no launch ranking.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The store keeps this many apps. When it grows past the limit, the apps
/// with the lowest counts go first.
const MAX_ENTRIES: usize = 500;
/// The largest rank boost. 63 launches reach it.
const MAX_BOOST: u32 = 6;

#[derive(Default)]
struct Inner {
    path: Option<PathBuf>,
    counts: HashMap<String, u32>,
}

#[derive(Default)]
pub struct LaunchStore {
    inner: Mutex<Inner>,
}

impl LaunchStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the file and loads it. A missing or corrupt file gives empty counts.
    pub fn set_path(&self, path: PathBuf) {
        let counts = load(&path);
        if let Ok(mut inner) = self.inner.lock() {
            inner.counts = counts;
            inner.path = Some(path);
        }
    }

    /// Adds one launch for `app_id` and saves the file.
    pub fn record(&self, app_id: &str) -> Result<(), String> {
        if app_id.is_empty() {
            return Ok(());
        }
        let mut inner = self.inner.lock().map_err(|error| error.to_string())?;
        let count = inner.counts.entry(app_id.to_string()).or_insert(0);
        *count = count.saturating_add(1);
        trim(&mut inner.counts, app_id);
        inner.save()
    }

    /// Removes the count of one app and saves the file.
    pub fn forget(&self, app_id: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|error| error.to_string())?;
        if inner.counts.remove(app_id).is_none() {
            return Ok(());
        }
        inner.save()
    }

    /// Removes all counts and saves the file.
    pub fn clear(&self) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|error| error.to_string())?;
        if inner.counts.is_empty() {
            return Ok(());
        }
        inner.counts.clear();
        inner.save()
    }

    /// The rank boost of an app: floor(log2(count + 1)), at most 6.
    pub fn boost(&self, app_id: &str) -> u32 {
        let count = self
            .inner
            .lock()
            .ok()
            .and_then(|inner| inner.counts.get(app_id).copied())
            .unwrap_or(0);
        boost_for(count)
    }
}

fn boost_for(count: u32) -> u32 {
    (u64::from(count) + 1).ilog2().min(MAX_BOOST)
}

impl Inner {
    fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let json = serde_json::to_vec(&self.counts).map_err(|error| error.to_string())?;
        crate::win32::write_atomic(path, &json).map_err(String::from)
    }
}

fn load(path: &Path) -> HashMap<String, u32> {
    let Ok(bytes) = fs::read(path) else {
        return HashMap::new();
    };
    let mut counts: HashMap<String, u32> = serde_json::from_slice(&bytes).unwrap_or_default();
    trim(&mut counts, "");
    counts
}

/// Drops the lowest counts until the map fits. The app `keep` stays.
fn trim(counts: &mut HashMap<String, u32>, keep: &str) {
    if counts.len() <= MAX_ENTRIES {
        return;
    }
    let mut order: Vec<(u32, String)> = counts
        .iter()
        .filter(|(id, _)| id.as_str() != keep)
        .map(|(id, count)| (*count, id.clone()))
        .collect();
    order.sort();
    let excess = counts.len() - MAX_ENTRIES;
    for (_, id) in order.into_iter().take(excess) {
        counts.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/launch-store-tests");
        fs::create_dir_all(&dir).expect("test folder");
        let path = dir.join(format!("{name}.json"));
        let _ = fs::remove_file(&path);
        path
    }

    #[test]
    fn boost_follows_the_log_curve_and_stops_at_six() {
        assert_eq!(boost_for(0), 0);
        assert_eq!(boost_for(1), 1);
        assert_eq!(boost_for(2), 1);
        assert_eq!(boost_for(3), 2);
        assert_eq!(boost_for(7), 3);
        assert_eq!(boost_for(63), 6);
        assert_eq!(boost_for(10_000), 6);
        assert_eq!(boost_for(u32::MAX), 6);
    }

    #[test]
    fn record_counts_launches() {
        let store = LaunchStore::new();
        assert_eq!(store.boost("a"), 0);
        store.record("a").unwrap();
        assert_eq!(store.boost("a"), 1);
        store.record("a").unwrap();
        store.record("a").unwrap();
        assert_eq!(store.boost("a"), 2);
        assert_eq!(store.boost("b"), 0);
    }

    #[test]
    fn counts_saturate() {
        let path = temp_file("saturate");
        fs::write(&path, format!(r#"{{"a":{}}}"#, u32::MAX)).unwrap();
        let store = LaunchStore::new();
        store.set_path(path.clone());
        store.record("a").unwrap();
        let again = LaunchStore::new();
        again.set_path(path);
        assert_eq!(again.boost("a"), 6);
    }

    #[test]
    fn counts_survive_a_restart() {
        let path = temp_file("round-trip");
        let first = LaunchStore::new();
        first.set_path(path.clone());
        first.record("a").unwrap();
        first.record("a").unwrap();
        first.record("a").unwrap();
        first.record("b").unwrap();
        assert!(!path.with_extension("json.tmp").exists());

        let second = LaunchStore::new();
        second.set_path(path);
        assert_eq!(second.boost("a"), 2);
        assert_eq!(second.boost("b"), 1);
        assert_eq!(second.boost("c"), 0);
    }

    #[test]
    fn a_corrupt_or_missing_file_gives_empty_counts() {
        let path = temp_file("corrupt");
        let store = LaunchStore::new();
        store.set_path(path.clone());
        assert_eq!(store.boost("a"), 0);

        fs::write(&path, b"{ not json").unwrap();
        store.set_path(path.clone());
        assert_eq!(store.boost("a"), 0);

        // A new launch replaces the corrupt file.
        store.record("a").unwrap();
        let again = LaunchStore::new();
        again.set_path(path);
        assert_eq!(again.boost("a"), 1);
    }

    #[test]
    fn forget_and_clear_remove_counts_from_the_file() {
        let path = temp_file("forget-clear");
        let store = LaunchStore::new();
        store.set_path(path.clone());
        store.record("a").unwrap();
        store.record("b").unwrap();

        store.forget("a").unwrap();
        store.forget("never-launched").unwrap();
        let after_forget = LaunchStore::new();
        after_forget.set_path(path.clone());
        assert_eq!(after_forget.boost("a"), 0);
        assert_eq!(after_forget.boost("b"), 1);

        store.clear().unwrap();
        let after_clear = LaunchStore::new();
        after_clear.set_path(path);
        assert_eq!(after_clear.boost("b"), 0);
    }

    #[test]
    fn the_store_keeps_500_apps_and_drops_the_lowest_counts() {
        let store = LaunchStore::new();
        for index in 0..MAX_ENTRIES {
            let id = format!("app-{index:03}");
            // Every app gets at least two launches, so the new app is the lowest.
            store.record(&id).unwrap();
            store.record(&id).unwrap();
        }
        store.record("popular").unwrap();
        store.record("popular").unwrap();
        store.record("popular").unwrap();
        assert_eq!(store.boost("popular"), 2);
        assert_eq!(store.boost("app-000"), 0);
        for index in 1..MAX_ENTRIES {
            assert_eq!(store.boost(&format!("app-{index:03}")), 1, "app-{index:03}");
        }
    }
}
