//! DuChinese stories as Library books: browse duchinese.net's graded
//! stories and courses, see how much of each you'd know, and import one as
//! an EPUB that the Library analyses and the reader opens like any other.
//! Premium chapters come with a signed-in subscription; new chapters of an
//! imported story are fetched and added as they come out.
//!
//! - [`data`]: what the site serves, and a chapter as paragraphs.
//! - [`api`]: talking to the site (and signing in).
//! - [`epub`]: the book written from the chapters.
//! - [`store`]: the imported books' records and the cache.
//! - [`worker`]: fetching, estimating and building off the UI thread.
//! - [`ui`]: the DuChinese window and what the Library and the reader show.

pub mod api;
pub mod data;
pub mod epub;
pub mod store;
pub mod ui;
pub mod worker;

pub use ui::DuState;

/// `2026-11-20…` as seconds since the Unix epoch (that day's UTC midnight).
pub fn date_secs(date: &str) -> Option<u64> {
    let d = date.get(..10)?;
    let mut it = d.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let day: i64 = it.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    let days = crate::reading_stats::days_from_civil(y, m, day);
    u64::try_from(days * 86_400).ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates() {
        assert_eq!(super::date_secs("1970-01-02T00:00:00Z"), Some(86_400));
        assert_eq!(super::date_secs("2026-11-20"), Some(1_795_132_800));
        assert_eq!(super::date_secs("soon"), None);
    }
}
