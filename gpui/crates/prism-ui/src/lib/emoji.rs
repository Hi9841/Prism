//! Monograms, monogram hues, and the idle-view app sort (`src/lib/emoji.ts`).

use std::cmp::Ordering;
use std::sync::LazyLock;

use icu_collator::options::CollatorOptions;
use icu_collator::preferences::CollationNumericOrdering;
use icu_collator::{CollatorBorrowed, CollatorPreferences};
use icu_locale_core::Locale;

use crate::js;
use crate::lib::types::AppEntry;

/// Deterministic monogram hue (0-1) derived from a name.
pub fn hue_for_name(name: &str) -> f64 {
    let mut h: u32 = 0;
    for unit in name.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(unit as u32);
    }
    (h % 360) as f64 / 360.0
}

pub fn name_to_monogram(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if "()[]{}".contains(c) { ' ' } else { c }).collect();
    let words: Vec<Vec<u16>> = cleaned
        .split(js::is_whitespace)
        .filter(|w| w.chars().any(|c| c.is_ascii_alphanumeric()))
        .map(|w| w.encode_utf16().collect())
        .collect();
    // BUG(port): the source slices UTF-16 units, so a monogram can hold half a
    // surrogate pair; Rust strings cannot, and the half becomes U+FFFD.
    let monogram = match words.as_slice() {
        [] => return "?".into(),
        [only] => String::from_utf16_lossy(&only[..only.len().min(2)]),
        [first, second, ..] => String::from_utf16_lossy(&[first[0], second[0]]),
    };
    monogram.to_uppercase()
}

/// `a.localeCompare(b, undefined, { numeric: true })` in the WebView: the
/// Windows UI language (`navigator.language`), numeric ordering on (D5, R12).
static COLLATOR: LazyLock<CollatorBorrowed<'static>> = LazyLock::new(|| {
    let locale = sys_locale::get_locale()
        .and_then(|tag| Locale::try_from_str(&tag).ok())
        .unwrap_or(Locale::UNKNOWN);
    let mut prefs = CollatorPreferences::from(&locale);
    prefs.numeric_ordering = Some(CollationNumericOrdering::True);
    CollatorBorrowed::try_new(prefs, CollatorOptions::default())
        .or_else(|_| CollatorBorrowed::try_new(CollatorPreferences::default(), CollatorOptions::default()))
        .expect("root collation data is compiled in")
});

pub fn locale_compare(a: &str, b: &str) -> Ordering {
    COLLATOR.compare(a, b)
}

/// The app list sorted by name. The source caches the result per input array;
/// the palette entity caches it per catalog revision instead.
pub fn sort_apps(apps: &[AppEntry]) -> Vec<&AppEntry> {
    let mut sorted: Vec<&AppEntry> = apps.iter().collect();
    sorted.sort_by(|a, b| locale_compare(&a.name, &b.name));
    sorted
}

// PORT STATUS: confidence=high todos=0
