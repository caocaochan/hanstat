//! Whether a dictionary's announced revision is newer than the installed one.
//!
//! Yomitan's rule (`compareRevisions` in `dictionary-data-util.js`): two
//! revisions that are both dot-separated integers with as many parts are
//! compared part by part as numbers; anything else as strings. Dates
//! (`2026-11-16`) and timestamps (`2026-11-13T14:02:01Z`) sort correctly as
//! strings. One change: dotted integers with a different number of parts are
//! compared as numbers too (missing parts are 0), where Yomitan's string
//! comparison misses `2026.9.1` → `2026.10.1.1`.

use std::cmp::Ordering;

fn dotted(s: &str) -> Option<Vec<u64>> {
    if s.is_empty() {
        return None;
    }
    s.split('.')
        .map(|p| {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                None
            } else {
                p.parse().ok()
            }
        })
        .collect()
}

/// `latest` is newer than `current`.
pub fn is_newer(current: &str, latest: &str) -> bool {
    let (c, l) = (current.trim(), latest.trim());
    match (dotted(c), dotted(l)) {
        (Some(a), Some(b)) => {
            let n = a.len().max(b.len());
            for i in 0..n {
                let (x, y) = (
                    a.get(i).copied().unwrap_or(0),
                    b.get(i).copied().unwrap_or(0),
                );
                match x.cmp(&y) {
                    Ordering::Less => return true,
                    Ordering::Greater => return false,
                    Ordering::Equal => {}
                }
            }
            false
        }
        _ => c < l,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The revision formats of the dictionaries in `dicts/` (2026-11-18).
    #[test]
    fn real_formats() {
        assert!(is_newer("2026-11-16", "2026-11-17"), "CC-CEDICT");
        assert!(!is_newer("2026-11-17", "2026-11-16"));
        assert!(!is_newer("2026-11-16", "2026-11-16"));
        assert!(is_newer("2026-11-13T14:02:01Z", "2026-11-14T00:00:00Z"));
        assert!(!is_newer("2026-11-13T14:02:01Z", "2026-11-13T14:02:01Z"));
        assert!(
            is_newer("2026.11.16.3", "2026.11.16.10"),
            "numbers, not strings"
        );
        assert!(!is_newer("2026.11.16.10", "2026.11.16.3"));
        assert!(is_newer("2026.11.08.2.3.1", "2026.11.08.2.3.2"));
        assert!(is_newer("2025.12.13.9.1", "2026.1.2"), "different lengths");
        assert!(
            is_newer("2026.9.1", "2026.10.1.1"),
            "Yomitan misses this one"
        );
        assert!(!is_newer("2026.10.1.1", "2026.9.1"));
        assert!(!is_newer("1.0", "1"), "missing parts are 0");
        assert!(is_newer("2023.7.11.7.1", "2023.7.11.7.2"));
        assert!(!is_newer("2026.09.06", "2026.09.06"));
    }

    #[test]
    fn non_numeric_compare_as_strings() {
        assert!(is_newer("v1", "v2"));
        // Not sortable by either rule: "v10" < "v2" as strings.
        assert!(!is_newer("v2", "v10"));
        assert!(is_newer("1.0.0-alpha", "1.0.0-beta"));
    }
}
