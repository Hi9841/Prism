//! Offline Jev distillation: a static phrase table plus a local SQLite cache
//! of accepted (query, action) pairs. Lookups stay in-process.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::params;

/// Distilled natural-language phrases -> catalog action ids.
/// Direct titles ("bluetooth") stay in the typed catalog; this table is for
/// phrasing a person would type that does not look like a settings page.
const PHRASES: &[(&str, &str)] = &[
    ("my eyes hurt", "settings.nightlight"),
    ("eyes hurt", "settings.nightlight"),
    ("too bright", "settings.nightlight"),
    ("screen is too bright", "settings.nightlight"),
    ("blue light", "settings.nightlight"),
    ("reduce blue light", "settings.nightlight"),
    ("night mode", "settings.nightlight"),
    ("warm the screen", "settings.nightlight"),
    ("kill audio", "audio.mute"),
    ("kill the audio", "audio.mute"),
    ("shut up", "audio.mute"),
    ("silence", "audio.mute"),
    ("no sound", "audio.mute"),
    ("stop the sound", "audio.mute"),
    ("restore audio", "audio.unmute"),
    ("bring the sound back", "audio.unmute"),
    ("sound on", "audio.unmute"),
    ("clean space", "settings.storagepolicies"),
    ("free space", "settings.storagepolicies"),
    ("free up space", "settings.storagepolicies"),
    ("disk is full", "settings.storagepolicies"),
    ("disk full", "settings.storagepolicies"),
    ("clean up disk", "settings.storagepolicies"),
    ("go to sleep", "power.sleep"),
    ("put to sleep", "power.sleep"),
    ("sleep now", "power.sleep"),
    ("lock the pc", "power.lock"),
    ("lock pc", "power.lock"),
    ("lock computer", "power.lock"),
    ("turn off the computer", "power.shutdown"),
    ("reboot", "power.restart"),
    ("check for updates", "settings.windowsupdate"),
    ("high dynamic range", "settings.hdr"),
    ("pair a device", "settings.bluetooth"),
    ("connect bluetooth", "settings.bluetooth"),
];

#[derive(Default)]
struct IntentInner {
    path: Option<PathBuf>,
    map: HashMap<String, String>,
    loaded: bool,
}

pub struct IntentStore {
    inner: Mutex<IntentInner>,
}

impl Default for IntentStore {
    fn default() -> Self {
        Self::new()
    }
}

impl IntentStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(IntentInner::default()),
        }
    }

    pub fn set_path(&self, path: PathBuf) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.path = Some(path);
            inner.loaded = false;
        }
    }

    pub fn lookup(&self, phrase: &str) -> Option<String> {
        let normalized = normalize_phrase(phrase);
        if normalized.is_empty() {
            return None;
        }
        let mut inner = self.inner.lock().ok()?;
        inner.ensure_loaded();
        inner.map.get(&normalized).cloned()
    }

    pub fn remember(&self, phrase: &str, action_id: &str) -> Result<(), String> {
        if super::catalog::get(action_id).is_none() {
            return Err(format!("unknown action id '{action_id}'"));
        }
        let normalized = normalize_phrase(phrase);
        if normalized.is_empty() {
            return Ok(());
        }
        let mut inner = self.inner.lock().map_err(|error| error.to_string())?;
        inner.ensure_loaded();
        inner.map.insert(normalized.clone(), action_id.to_string());
        if let Some(path) = inner.path.clone() {
            persist_pair(&path, &normalized, action_id)?;
        }
        Ok(())
    }
}

impl IntentInner {
    fn ensure_loaded(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let Some(path) = self.path.clone() else {
            return;
        };
        if let Ok(map) = load_pairs(&path) {
            self.map = map;
        }
    }
}

pub fn resolve(query: &str, store: &IntentStore) -> Option<&'static str> {
    let normalized = normalize_phrase(query);
    if normalized.is_empty() {
        return None;
    }
    if let Some(id) = store.lookup(&normalized) {
        if super::catalog::get(&id).is_some() {
            // Cache holds owned strings; map back to the static catalog id.
            return super::catalog::get(&id).map(|action| action.id);
        }
    }
    map_static(&normalized)
}

pub fn map_static(normalized: &str) -> Option<&'static str> {
    for (phrase, action_id) in PHRASES {
        if *phrase == normalized {
            return Some(*action_id);
        }
    }
    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    if tokens.len() < 2 {
        return None;
    }
    let mut best: Option<(&'static str, usize)> = None;
    for (phrase, action_id) in PHRASES {
        let phrase_tokens: Vec<&str> = phrase.split_whitespace().collect();
        if phrase_tokens.len() < 2 || tokens.len() < phrase_tokens.len() {
            continue;
        }
        let hit = tokens
            .windows(phrase_tokens.len())
            .any(|window| window == phrase_tokens.as_slice());
        if hit {
            let rank = phrase_tokens.len();
            if best.is_none_or(|(_, current)| rank > current) {
                best = Some((*action_id, rank));
            }
        }
    }
    best.map(|(id, _)| id)
}

pub fn normalize_phrase(value: &str) -> String {
    value
        .split_whitespace()
        .map(|token| {
            token
                .chars()
                .filter(|ch| ch.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn load_pairs(path: &PathBuf) -> Result<HashMap<String, String>, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let conn = rusqlite::Connection::open(path).map_err(|error| error.to_string())?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS accepted_intents (
            phrase TEXT PRIMARY KEY,
            action_id TEXT NOT NULL,
            hits INTEGER NOT NULL DEFAULT 1,
            last_used INTEGER NOT NULL
         );",
    )
    .map_err(|error| error.to_string())?;
    let mut stmt = conn
        .prepare("SELECT phrase, action_id FROM accepted_intents")
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| error.to_string())?;
    let mut map = HashMap::new();
    for row in rows {
        let (phrase, action_id) = row.map_err(|error| error.to_string())?;
        map.insert(phrase, action_id);
    }
    Ok(map)
}

fn persist_pair(path: &PathBuf, phrase: &str, action_id: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let conn = rusqlite::Connection::open(path).map_err(|error| error.to_string())?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS accepted_intents (
            phrase TEXT PRIMARY KEY,
            action_id TEXT NOT NULL,
            hits INTEGER NOT NULL DEFAULT 1,
            last_used INTEGER NOT NULL
         );",
    )
    .map_err(|error| error.to_string())?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    conn.execute(
        "INSERT INTO accepted_intents (phrase, action_id, hits, last_used)
         VALUES (?1, ?2, 1, ?3)
         ON CONFLICT(phrase) DO UPDATE SET
            action_id = excluded.action_id,
            hits = hits + 1,
            last_used = excluded.last_used",
        params![phrase, action_id, now],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distilled_phrases_map_to_catalog_ids() {
        assert_eq!(map_static("my eyes hurt"), Some("settings.nightlight"));
        assert_eq!(map_static("kill audio"), Some("audio.mute"));
        assert_eq!(map_static("clean space"), Some("settings.storagepolicies"));
    }

    #[test]
    fn single_generic_word_does_not_false_match() {
        assert_eq!(map_static("hurt"), None);
        assert_eq!(map_static("space"), None);
    }

    #[test]
    fn substring_inside_another_word_does_not_match() {
        assert_eq!(map_static("unlock pc"), None);
        assert_eq!(map_static("block pc"), None);
        assert_eq!(map_static("unlock computer"), None);
    }

    #[test]
    fn longer_query_still_hits_a_multiword_phrase() {
        assert_eq!(
            map_static("my eyes hurt tonight"),
            Some("settings.nightlight")
        );
    }

    #[test]
    fn accepted_pairs_win_over_the_static_table() {
        let store = IntentStore::new();
        store
            .remember("eyeburn", "settings.nightlight")
            .expect("remember");
        assert_eq!(resolve("eyeburn", &store), Some("settings.nightlight"));
    }

    #[test]
    fn rejected_unknown_action_ids() {
        let store = IntentStore::new();
        assert!(store.remember("hello", "not.an.action").is_err());
    }

    #[test]
    fn sqlite_cache_round_trips() {
        let dir = std::env::temp_dir().join(format!(
            "prism-intent-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("intent-cache.db");
        let store = IntentStore::new();
        store.set_path(path.clone());
        store
            .remember("eyeburn please", "settings.nightlight")
            .unwrap();

        let reloaded = IntentStore::new();
        reloaded.set_path(path);
        assert_eq!(
            resolve("eyeburn please", &reloaded),
            Some("settings.nightlight")
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
