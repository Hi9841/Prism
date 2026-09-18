//! In-process scorer for Phase 1. No allocations on the SQLite/FTS path.

pub fn score_text(query: &str, target: &str) -> Option<i32> {
    if query.is_empty() || target.is_empty() {
        return None;
    }
    let q = query.to_lowercase();
    let t = target.to_lowercase();
    if t == q {
        return Some(1_000);
    }
    if t.starts_with(&q) {
        return Some(900 + q.len() as i32);
    }
    if t.contains(&q) {
        return Some(800 + q.len() as i32);
    }
    fuzzy_subsequence(&q, &t)
}

pub fn score_with_keywords(query: &str, title: &str, keywords: &[&str]) -> Option<i32> {
    let mut best = score_text(query, title);
    for keyword in keywords {
        let Some(score) = score_text(query, keyword) else {
            continue;
        };
        let keyword_score = (score - 50).max(0);
        if best.is_none_or(|current| keyword_score > current) {
            best = Some(keyword_score);
        }
    }
    best
}

fn fuzzy_subsequence(q: &str, t: &str) -> Option<i32> {
    if q.len() > t.len() {
        return None;
    }
    let q_chars: Vec<char> = q.chars().collect();
    let t_chars: Vec<char> = t.chars().collect();
    let mut qi = 0usize;
    let mut score = 0i32;
    let mut prev = isize::MIN;
    let mut consecutive = 0i32;
    for (ti, ch) in t_chars.iter().enumerate() {
        if qi >= q_chars.len() {
            break;
        }
        if *ch != q_chars[qi] {
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
        } else if is_word_separator(t_chars[ti - 1]) {
            s += 6;
        }
        score += s;
        prev = ti as isize;
        qi += 1;
    }
    if qi < q_chars.len() {
        return None;
    }
    let significant = q.chars().filter(|c| c.is_ascii_alphanumeric()).count();
    let minimum = if significant >= 3 {
        (significant as i32 * 3).min(24)
    } else {
        i32::MIN
    };
    score -= (t_chars.len() as i32) / 12;
    if score < minimum {
        None
    } else {
        Some(score)
    }
}

fn is_word_separator(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '-' | '_' | '.' | '/' | '\\')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_title_outranks_prefix() {
        let exact = score_text("display", "Display").unwrap();
        let prefix = score_text("dis", "Display").unwrap();
        assert!(exact > prefix);
    }

    #[test]
    fn keyword_match_is_weaker_than_title() {
        let title = score_with_keywords("night", "Night Light", &["blue light"]).unwrap();
        let keyword = score_with_keywords("blue light", "Night Light", &["blue light"]).unwrap();
        assert!(title >= 800);
        assert!(keyword > 0);
    }

    #[test]
    fn rejects_unrelated_strings() {
        assert_eq!(score_text("zzzz", "Night Light"), None);
    }
}
