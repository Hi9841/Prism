//! In-process scorer for Phase 1. No allocations on the SQLite/FTS path.

/// A query prepared once for one keystroke: phase 1 scores it against every
/// title and keyword of the app, tool, setting, and action indexes (about
/// 11,000 targets), so nothing here is computed again for each target.
pub struct Query {
    lower: String,
    chars: Vec<char>,
    /// Letters and digits in the query: the fuzzy tier's minimum score comes from it.
    significant: usize,
}

impl Query {
    pub fn new(query: &str) -> Self {
        let lower = query.to_lowercase();
        Self {
            chars: lower.chars().collect(),
            significant: lower.chars().filter(|c| c.is_ascii_alphanumeric()).count(),
            lower,
        }
    }

    /// The score of `target` for this query: the whole text (1,000), a
    /// prefix (900 and up), a substring (800 and up), or a fuzzy subsequence.
    /// Case does not count.
    pub fn score(&self, target: &str) -> Option<i32> {
        if self.lower.is_empty() || target.is_empty() {
            return None;
        }
        // Lowercase of ASCII text is ASCII lowercase, byte for byte: compare
        // in place. Almost every title and keyword is ASCII.
        if target.is_ascii() {
            return self.score_ascii(target.as_bytes());
        }
        let lower = target.to_lowercase();
        let query = self.lower.as_str();
        if lower == query {
            return Some(1_000);
        }
        if lower.starts_with(query) {
            return Some(900 + query.len() as i32);
        }
        if lower.contains(query) {
            return Some(800 + query.len() as i32);
        }
        if query.len() > lower.len() {
            return None;
        }
        let chars: Vec<char> = lower.chars().collect();
        self.fuzzy(chars.len(), |index| chars[index])
    }

    /// `score` plus the keywords: a keyword scores 50 below a title.
    pub fn score_with_keywords<S: AsRef<str>>(
        &self,
        title: &str,
        keywords: impl IntoIterator<Item = S>,
    ) -> Option<i32> {
        let mut best = self.score(title);
        for keyword in keywords {
            let Some(score) = self.score(keyword.as_ref()) else {
                continue;
            };
            // A weak fuzzy keyword hit that the penalty takes to zero is no match.
            let keyword_score = score - 50;
            if keyword_score > 0 && best.is_none_or(|current| keyword_score > current) {
                best = Some(keyword_score);
            }
        }
        best
    }

    fn score_ascii(&self, target: &[u8]) -> Option<i32> {
        let query = self.lower.as_bytes();
        // A query byte outside ASCII equals no target byte, as in the lowercase text.
        let matches_at = |start: usize| {
            target[start..start + query.len()]
                .iter()
                .zip(query)
                .all(|(byte, wanted)| byte.to_ascii_lowercase() == *wanted)
        };
        if query.len() <= target.len() {
            if matches_at(0) {
                return Some(if query.len() == target.len() { 1_000 } else { 900 + query.len() as i32 });
            }
            if (1..=target.len() - query.len()).any(matches_at) {
                return Some(800 + query.len() as i32);
            }
        } else {
            return None;
        }
        self.fuzzy(target.len(), |index| target[index].to_ascii_lowercase() as char)
    }

    /// The fuzzy tier over a lowercase target of `length` characters.
    fn fuzzy(&self, length: usize, target: impl Fn(usize) -> char) -> Option<i32> {
        let mut qi = 0usize;
        let mut score = 0i32;
        let mut prev = isize::MIN;
        let mut consecutive = 0i32;
        for ti in 0..length {
            if qi >= self.chars.len() {
                break;
            }
            if target(ti) != self.chars[qi] {
                continue;
            }
            let mut s = 1i32;
            if prev == ti as isize - 1 {
                consecutive += 1;
                s += consecutive * 3;
            } else {
                consecutive = 0;
                if prev >= 0 {
                    s -= ((ti as isize - prev - 1) as i32).min(8);
                }
            }
            if ti == 0 {
                s += 12;
            } else if is_word_separator(target(ti - 1)) {
                s += 6;
            }
            score += s;
            prev = ti as isize;
            qi += 1;
        }
        if qi < self.chars.len() {
            return None;
        }
        let minimum = if self.significant >= 3 {
            (self.significant as i32 * 3).min(24)
        } else {
            i32::MIN
        };
        score -= (length as i32) / 12;
        if score < minimum {
            None
        } else {
            Some(score)
        }
    }
}

/// One query against one target. A caller that scores many targets prepares
/// a [`Query`] once.
pub fn score_text(query: &str, target: &str) -> Option<i32> {
    Query::new(query).score(target)
}

pub fn score_with_keywords(query: &str, title: &str, keywords: &[&str]) -> Option<i32> {
    Query::new(query).score_with_keywords(title, keywords)
}

fn is_word_separator(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '-' | '_' | '.' | '/' | '\\')
}
