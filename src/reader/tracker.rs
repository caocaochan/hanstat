//! The reading tracker: times reading and counts the Chinese characters
//! paged past, into [`crate::reading_stats`].
//!
//! A pure state machine: the reader tells it, once a frame, the time,
//! whether there was input, whether the window has focus, whether a menu or
//! panel is open and whether read aloud is playing ([`Frame`]); and where
//! the page settled ([`Place`]) after a turn ([`Tracker::turned`]) or
//! anything else (a jump, a new layout: not counted).
//!
//! Time is added when a running stretch ends (a pause, a switch between
//! reading and listening) and every [`FLUSH_SECS`], never per frame.
//! Characters are the net change in position: paging back takes from
//! today's count down to zero and leaves the rest as the book's debt, which
//! paging forward again pays off before anything counts.

use crate::reading_stats::{self, BookStats, Day, ReadingStats, Session};
use std::collections::BTreeMap;

/// A running stretch is written out at least this often.
pub const FLUSH_SECS: f64 = 30.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pause {
    /// By P or the button; only they start it again.
    Manual,
    Idle,
    Unfocused,
    Menu,
    /// A search result is shown (not where reading is).
    Search,
}

impl Pause {
    pub fn label(self) -> &'static str {
        match self {
            Pause::Manual => "Paused",
            Pause::Idle => "Paused: idle",
            Pause::Unfocused => "Paused: window inactive",
            Pause::Menu => "Paused: menu open",
            Pause::Search => "Paused: search result shown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    Paused(Pause),
}

/// What the reader saw this frame.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    /// Unix time.
    pub now: f64,
    /// Keys, pointer movement, clicks, the wheel.
    pub input: bool,
    pub focused: bool,
    /// A menu or panel over the page (not the dictionary popup).
    pub menus: bool,
    /// Read aloud is playing.
    pub listening: bool,
    /// A search result is shown, away from the place.
    pub search: bool,
}

/// Where the reader is: a page of a laid-out chapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub spine: usize,
    pub page: usize,
    /// Chinese characters of the chapter before the page.
    pub han: u32,
    /// The chapter's Chinese characters.
    pub chapter_han: u32,
    /// The layout it is a page of (a new one after a font or size change).
    pub layout: usize,
}

pub struct Tracker {
    book: String,
    title: String,
    state: State,
    /// When the running stretch began, and whether it is listening.
    since: f64,
    listening: bool,
    last_input: f64,
    idle_secs: f64,
    day_start: u32,
    base: Option<Place>,
    turn_pending: bool,
    /// This session: its index in the book's list once it has anything,
    /// what it added per day (to discard it), and the debt before it.
    session: Option<usize>,
    start: f64,
    added: BTreeMap<i32, (f64, i64, f64, i64)>,
    debt_before: u64,
    /// The session's totals, for the reader's line.
    pub totals: Day,
    /// Something changed since the last save.
    pub changed: bool,
}

impl Tracker {
    pub fn new(book: &str, title: &str, s: &ReadingStats, now: f64) -> Tracker {
        let set = &s.settings;
        Tracker {
            book: book.to_owned(),
            title: title.to_owned(),
            state: if set.auto_start {
                State::Running
            } else {
                State::Paused(Pause::Manual)
            },
            since: now,
            listening: false,
            last_input: now,
            idle_secs: set.idle_mins.max(1) as f64 * 60.0,
            day_start: set.day_start_hour,
            base: None,
            turn_pending: false,
            session: None,
            start: now,
            added: BTreeMap::new(),
            debt_before: s.books.get(book).map_or(0, |b| b.debt),
            totals: Day::default(),
            changed: false,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Where the reader was last seen.
    pub fn base(&self) -> Option<Place> {
        self.base
    }

    pub fn running(&self) -> bool {
        self.state == State::Running
    }

    pub fn listening(&self) -> bool {
        self.running() && self.listening
    }

    /// The settings changed: the idle limit and day start apply from now.
    pub fn settings(&mut self, s: &ReadingStats) {
        self.idle_secs = s.settings.idle_mins.max(1) as f64 * 60.0;
        self.day_start = s.settings.day_start_hour;
    }

    /// Reading and listening seconds of the running stretch, not yet added.
    pub fn open_secs(&self, now: f64) -> (f64, f64) {
        if !self.running() {
            return (0.0, 0.0);
        }
        let t = (now - self.since).max(0.0);
        if self.listening { (0.0, t) } else { (t, 0.0) }
    }

    /// When the tracker would pause for idleness, while it runs.
    pub fn idle_at(&self) -> Option<f64> {
        (self.running() && !self.listening).then_some(self.last_input + self.idle_secs)
    }

    /// Once a frame, before any turn is reported.
    pub fn frame(&mut self, s: &mut ReadingStats, f: Frame) {
        // Gone idle since the last frame: the time back to the last input
        // doesn't count.
        if self.state == State::Running
            && !self.listening
            && f.now - self.last_input > self.idle_secs
        {
            let end = self.last_input.max(self.since);
            self.close(s, end);
            self.state = State::Paused(Pause::Idle);
        }
        if f.input {
            self.last_input = f.now;
        }
        let want = if self.state == State::Paused(Pause::Manual) {
            State::Paused(Pause::Manual)
        } else if f.listening {
            State::Running
        } else if !f.focused {
            State::Paused(Pause::Unfocused)
        } else if f.search {
            State::Paused(Pause::Search)
        } else if f.menus {
            State::Paused(Pause::Menu)
        } else if f.now - self.last_input > self.idle_secs {
            State::Paused(Pause::Idle)
        } else {
            State::Running
        };
        let listening = want == State::Running && f.listening;
        if self.state == State::Running {
            // Stopped, switched between reading and listening, or due.
            if want != State::Running
                || listening != self.listening
                || f.now - self.since >= FLUSH_SECS
            {
                self.close(s, f.now);
                self.since = f.now;
            }
        } else if want == State::Running {
            self.since = f.now;
        }
        self.state = want;
        self.listening = listening;
    }

    /// P or the button.
    pub fn toggle(&mut self, s: &mut ReadingStats, now: f64) {
        if self.state == State::Paused(Pause::Manual) {
            self.state = State::Running;
            self.since = now;
            self.last_input = now;
        } else {
            if self.state == State::Running {
                self.close(s, now);
            }
            self.state = State::Paused(Pause::Manual);
        }
    }

    /// A page turn (or read aloud following its sentence): the next place
    /// reported counts.
    pub fn turned(&mut self) {
        self.turn_pending = true;
    }

    /// A jump (from a search result shown): a turn made before it doesn't
    /// count, and the next place only moves the base.
    pub fn jumped(&mut self) {
        self.turn_pending = false;
    }

    /// Where the reader settled this frame. `between(a..b)`: Chinese
    /// characters of the chapters in `a..b`.
    pub fn place(
        &mut self,
        s: &mut ReadingStats,
        p: Place,
        between: &dyn Fn(std::ops::Range<usize>) -> u64,
        now: f64,
    ) {
        let Some(base) = self.base else {
            self.base = Some(p);
            self.turn_pending = false;
            return;
        };
        if base == p {
            // A turn that went nowhere (back on the first page).
            self.turn_pending = false;
            return;
        }
        let same_page = base.spine == p.spine && base.page == p.page;
        if same_page && base.layout == p.layout {
            // Still the last page after the end was counted (`finish`).
            self.turn_pending = false;
            return;
        }
        if self.turn_pending && self.running() && !same_page {
            let delta = han_between(&base, &p, between);
            self.credit(s, delta, now);
        }
        self.turn_pending = false;
        self.base = Some(p);
    }

    /// Next on the book's last page: the rest of it counts, and it is
    /// finished. `true` the first time.
    pub fn finish(&mut self, s: &mut ReadingStats, now: f64) -> bool {
        self.turn_pending = false;
        if let Some(mut b) = self.base {
            if self.running() {
                let rest = b.chapter_han.saturating_sub(b.han) as i64;
                self.credit(s, rest, now);
            }
            b.han = b.chapter_han;
            self.base = Some(b);
        }
        let book = self.book_mut(s);
        let first = book.finished.is_none();
        if first {
            book.finished = Some(now as u64);
            self.changed = true;
        }
        first
    }

    /// The reader's count of the book's characters, once every chapter is in.
    pub fn book_han(&mut self, s: &mut ReadingStats, n: u64) {
        let book = self.book_mut(s);
        if book.han_total != n {
            book.han_total = n;
            self.changed = true;
        }
    }

    /// Closing the book: the running stretch is added.
    pub fn end(&mut self, s: &mut ReadingStats, now: f64) {
        if self.state == State::Running {
            self.close(s, now);
            self.since = now;
        }
    }

    /// Every book's stats were reset: start a new session from `now`, with
    /// nothing before it to add or take back. The page stays the base, so
    /// turns from it count.
    #[cfg(test)]
    pub fn restart(&mut self, now: f64) {
        self.session = None;
        self.added.clear();
        self.debt_before = 0;
        self.totals = Day::default();
        self.start = now;
        self.since = now;
        self.changed = false;
    }

    /// Take this session out of the stats, and start a new one.
    pub fn discard(&mut self, s: &mut ReadingStats, now: f64) {
        let added = std::mem::take(&mut self.added);
        let debt = self.debt_before;
        let session = self.session.take();
        let book = self.book_mut(s);
        for (d, (rs, rc, ls, lc)) in added {
            if let Some(day) = book.days.get_mut(&d) {
                day.read_secs = (day.read_secs - rs).max(0.0);
                day.read_chars = (day.read_chars as i64 - rc).max(0) as u64;
                day.listen_secs = (day.listen_secs - ls).max(0.0);
                day.listen_chars = (day.listen_chars as i64 - lc).max(0) as u64;
                if day.is_empty() {
                    book.days.remove(&d);
                }
            }
        }
        if let Some(i) = session
            && i < book.sessions.len()
        {
            book.sessions.remove(i);
        }
        book.debt = debt;
        self.totals = Day::default();
        self.start = now;
        self.since = now;
        self.changed = true;
    }

    fn book_mut<'a>(&self, s: &'a mut ReadingStats) -> &'a mut BookStats {
        let b = s.books.entry(self.book.clone()).or_default();
        if b.title != self.title && !self.title.is_empty() {
            b.title = self.title.clone();
        }
        b
    }

    /// Add the running stretch up to `end`.
    fn close(&mut self, s: &mut ReadingStats, end: f64) {
        let secs = end - self.since;
        if secs <= 0.0 {
            return;
        }
        let listening = self.listening;
        for (d, t) in reading_stats::split_days(self.since, end, self.day_start) {
            let book = self.book_mut(s);
            let day = book.days.entry(d).or_default();
            let a = self.added.entry(d).or_default();
            if listening {
                day.listen_secs += t;
                a.2 += t;
            } else {
                day.read_secs += t;
                a.0 += t;
            }
        }
        if listening {
            self.totals.listen_secs += secs;
        } else {
            self.totals.read_secs += secs;
        }
        self.touch(s, end);
    }

    /// Characters paged past: forward pays the debt first, back takes from
    /// today down to zero and owes the rest.
    fn credit(&mut self, s: &mut ReadingStats, delta: i64, now: f64) {
        if delta == 0 {
            return;
        }
        let listening = self.listening;
        let d = reading_stats::local_day(now, self.day_start);
        let book = self.book_mut(s);
        let day = book.days.entry(d).or_default();
        let count = if listening {
            &mut day.listen_chars
        } else {
            &mut day.read_chars
        };
        let change: i64 = if delta > 0 {
            let pay = (delta as u64).min(book.debt);
            book.debt -= pay;
            delta - pay as i64
        } else {
            let back = delta.unsigned_abs();
            let take = back.min(*count);
            book.debt += back - take;
            -(take as i64)
        };
        *count = (*count as i64 + change) as u64;
        if day.is_empty() {
            book.days.remove(&d);
        }
        let a = self.added.entry(d).or_default();
        let total = if listening {
            a.3 += change;
            &mut self.totals.listen_chars
        } else {
            a.1 += change;
            &mut self.totals.read_chars
        };
        *total = (*total as i64 + change).max(0) as u64;
        self.touch(s, now);
    }

    /// Keep the session's entry and the book's first date up to date.
    fn touch(&mut self, s: &mut ReadingStats, now: f64) {
        let (start, totals) = (self.start as u64, self.totals);
        let book = self.book_mut(s);
        book.first_read.get_or_insert(start);
        let i = *self.session.get_or_insert_with(|| {
            book.sessions.push(Session {
                start,
                ..Session::default()
            });
            book.sessions.len() - 1
        });
        if let Some(sess) = book.sessions.get_mut(i) {
            sess.end = now as u64;
            sess.totals = totals;
        }
        self.changed = true;
    }
}

/// Signed Chinese characters from `a` to `b`.
fn han_between(a: &Place, b: &Place, between: &dyn Fn(std::ops::Range<usize>) -> u64) -> i64 {
    use std::cmp::Ordering::*;
    match b.spine.cmp(&a.spine) {
        Equal => b.han as i64 - a.han as i64,
        Greater => {
            (a.chapter_han.saturating_sub(a.han) as u64
                + between(a.spine + 1..b.spine)
                + b.han as u64) as i64
        }
        Less => {
            -((b.chapter_han.saturating_sub(b.han) as u64
                + between(b.spine + 1..a.spine)
                + a.han as u64) as i64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: f64 = 1_790_000_000.0;

    fn frame(now: f64) -> Frame {
        Frame {
            now: T0 + now,
            input: false,
            focused: true,
            menus: false,
            listening: false,
            search: false,
        }
    }

    fn input(now: f64) -> Frame {
        Frame {
            input: true,
            ..frame(now)
        }
    }

    fn at(spine: usize, page: usize, han: u32) -> Place {
        Place {
            spine,
            page,
            han,
            chapter_han: 1000,
            layout: 1,
        }
    }

    fn none(_: std::ops::Range<usize>) -> u64 {
        0
    }

    struct Fx {
        s: ReadingStats,
        t: Tracker,
    }

    impl Fx {
        fn new() -> Fx {
            let mut s = ReadingStats::default();
            s.settings.day_start_hour = 0;
            let t = Tracker::new("b", "书", &s, T0);
            Fx { s, t }
        }
        fn frame(&mut self, f: Frame) {
            self.t.frame(&mut self.s, f);
        }
        fn turn(&mut self, now: f64, p: Place) {
            self.t.frame(&mut self.s, input(now));
            self.t.turned();
            self.t.place(&mut self.s, p, &none, T0 + now);
        }
        fn day(&self) -> Day {
            self.s.books["b"].totals()
        }
    }

    #[test]
    fn turns_count_net_and_jumps_dont() {
        let mut f = Fx::new();
        f.t.place(&mut f.s, at(0, 0, 0), &none, T0);
        f.turn(10.0, at(0, 1, 300));
        f.turn(20.0, at(0, 2, 650));
        assert_eq!(f.day().read_chars, 650);
        f.turn(25.0, at(0, 1, 300)); // back
        assert_eq!(f.day().read_chars, 300);
        f.turn(30.0, at(0, 2, 650)); // forward again
        assert_eq!(f.day().read_chars, 650);
        // A jump (no turn) moves the base without counting.
        f.t.place(&mut f.s, at(0, 9, 900), &none, T0 + 31.0);
        assert_eq!(f.day().read_chars, 650);
        // A new layout of the same chapter isn't a turn either.
        f.t.place(
            &mut f.s,
            Place {
                layout: 2,
                han: 880,
                page: 8,
                ..at(0, 8, 0)
            },
            &none,
            T0 + 32.0,
        );
        f.turn(
            40.0,
            Place {
                layout: 2,
                ..at(0, 9, 960)
            },
        );
        assert_eq!(f.day().read_chars, 650 + 80);
        assert_eq!(f.t.totals.read_chars, 730);
    }

    #[test]
    fn turning_into_the_next_chapter_counts_the_rest() {
        let mut f = Fx::new();
        f.t.place(&mut f.s, at(0, 5, 900), &none, T0);
        // Over an empty chapter 1 into chapter 2's first page.
        f.t.frame(&mut f.s, input(5.0));
        f.t.turned();
        let between = |r: std::ops::Range<usize>| r.len() as u64 * 7;
        f.t.place(&mut f.s, at(2, 0, 0), &between, T0 + 5.0);
        assert_eq!(f.day().read_chars, 100 + 7);
        // And back to the last page of chapter 0.
        f.t.frame(&mut f.s, input(6.0));
        f.t.turned();
        f.t.place(&mut f.s, at(0, 5, 950), &between, T0 + 6.0);
        assert_eq!(f.day().read_chars, 107 - (50 + 7));
    }

    #[test]
    fn paging_back_never_goes_below_zero_and_owes_the_rest() {
        let mut f = Fx::new();
        f.t.place(&mut f.s, at(0, 5, 2000), &none, T0);
        f.turn(5.0, at(0, 4, 1500)); // 500 back on an empty day
        assert_eq!(f.day().read_chars, 0);
        assert_eq!(f.s.books["b"].debt, 500);
        f.turn(10.0, at(0, 5, 2000)); // re-reading pays it off
        assert_eq!(f.day().read_chars, 0);
        assert_eq!(f.s.books["b"].debt, 0);
        f.turn(15.0, at(0, 6, 2400));
        assert_eq!(f.day().read_chars, 400);
    }

    #[test]
    fn idle_time_is_taken_off() {
        let mut f = Fx::new();
        f.frame(input(0.0));
        f.frame(input(100.0));
        // Ten minutes with no input, then a key.
        f.frame(input(700.0));
        assert_eq!(f.day().read_secs, 100.0);
        assert_eq!(f.t.state(), State::Running);
        f.frame(frame(760.0));
        f.t.end(&mut f.s, T0 + 760.0);
        assert_eq!(f.day().read_secs, 160.0);
    }

    #[test]
    fn focus_loss_menus_and_p_pause() {
        let mut f = Fx::new();
        f.frame(input(0.0));
        f.frame(Frame {
            focused: false,
            ..frame(20.0)
        });
        assert_eq!(f.t.state(), State::Paused(Pause::Unfocused));
        f.frame(Frame {
            focused: false,
            ..frame(200.0)
        });
        f.frame(input(210.0));
        assert_eq!(f.t.state(), State::Running);
        f.frame(Frame {
            menus: true,
            ..input(230.0)
        });
        assert_eq!(f.t.state(), State::Paused(Pause::Menu));
        f.frame(input(300.0));
        f.t.toggle(&mut f.s, T0 + 310.0);
        assert_eq!(f.t.state(), State::Paused(Pause::Manual));
        f.frame(input(400.0)); // input doesn't end a pause by hand
        assert_eq!(f.t.state(), State::Paused(Pause::Manual));
        assert_eq!(f.day().read_secs, 20.0 + 20.0 + 10.0);
        // Turns while paused move the place without counting.
        f.t.place(&mut f.s, at(0, 0, 0), &none, T0 + 400.0);
        f.turn(401.0, at(0, 1, 300));
        assert_eq!(f.day().read_chars, 0);
        f.t.toggle(&mut f.s, T0 + 410.0);
        f.turn(420.0, at(0, 2, 600));
        assert_eq!(f.day().read_chars, 300);
    }

    #[test]
    fn by_hand_mode_starts_paused() {
        let mut s = ReadingStats::default();
        s.settings.auto_start = false;
        let t = Tracker::new("b", "", &s, T0);
        assert_eq!(t.state(), State::Paused(Pause::Manual));
    }

    #[test]
    fn listening_is_kept_apart_and_never_idles() {
        let mut f = Fx::new();
        f.frame(input(0.0));
        f.frame(Frame {
            listening: true,
            search: false,
            ..frame(60.0)
        });
        f.t.place(&mut f.s, at(0, 0, 0), &none, T0 + 60.0);
        f.t.turned();
        f.t.place(&mut f.s, at(0, 1, 400), &none, T0 + 100.0);
        // Twenty minutes of listening without input, window in the background.
        f.frame(Frame {
            listening: true,
            search: false,
            focused: false,
            ..frame(1260.0)
        });
        f.frame(input(1270.0));
        let d = f.day();
        assert_eq!(d.read_secs, 60.0);
        // Up to the input that ended it (read aloud had stopped by then).
        assert_eq!(d.listen_secs, 1210.0);
        assert_eq!(d.listen_chars, 400);
        assert_eq!(d.read_chars, 0);
        f.t.end(&mut f.s, T0 + 1280.0);
        assert_eq!(f.day().read_secs, 70.0);
    }

    #[test]
    fn a_stretch_over_the_day_start_is_split() {
        let mut s = ReadingStats::default();
        s.settings.day_start_hour = 4;
        let start = reading_stats::days_from_civil(2026, 11, 23) as f64 * 86_400.0;
        let off = reading_stats::utc_offset(start as i64) as f64;
        // 03:50 local.
        let t0 = start - off + 3.0 * 3600.0 + 50.0 * 60.0;
        let mut t = Tracker::new("b", "", &s, t0);
        for i in 1..=4 {
            t.frame(
                &mut s,
                Frame {
                    now: t0 + i as f64 * 300.0,
                    input: true,
                    focused: true,
                    menus: false,
                    listening: false,
                    search: false,
                },
            );
        }
        t.end(&mut s, t0 + 1200.0);
        let d = reading_stats::days_from_civil(2026, 11, 23) as i32;
        let days = &s.books["b"].days;
        assert_eq!(days[&(d - 1)].read_secs, 600.0);
        assert_eq!(days[&d].read_secs, 600.0);
    }

    #[test]
    fn the_end_of_the_book_counts_and_finishes_once() {
        let mut f = Fx::new();
        f.t.place(&mut f.s, at(3, 7, 820), &none, T0);
        f.frame(input(5.0));
        assert!(f.t.finish(&mut f.s, T0 + 5.0));
        assert_eq!(f.day().read_chars, 180);
        // The reader reports the same page again after the turn.
        f.t.turned();
        f.t.place(&mut f.s, at(3, 7, 820), &none, T0 + 5.5);
        assert!(!f.t.finish(&mut f.s, T0 + 6.0));
        assert_eq!(f.day().read_chars, 180);
        // Back from the end takes off the last page too.
        f.turn(7.0, at(3, 6, 500));
        assert_eq!(f.day().read_chars, 0);
        assert!(f.s.books["b"].finished.is_some());
    }

    #[test]
    fn discarding_the_session_takes_it_all_back() {
        let mut f = Fx::new();
        // An earlier session.
        f.s.books.entry("b".into()).or_default().days.insert(
            reading_stats::local_day(T0, 0),
            Day {
                read_secs: 600.0,
                read_chars: 1000,
                ..Day::default()
            },
        );
        f.s.books.get_mut("b").unwrap().debt = 50;
        f.t = Tracker::new("b", "书", &f.s, T0);
        f.t.place(&mut f.s, at(0, 0, 0), &none, T0);
        f.turn(30.0, at(0, 1, 500));
        f.turn(40.0, at(0, 0, 0));
        f.turn(50.0, at(0, 1, 500));
        f.t.end(&mut f.s, T0 + 60.0);
        assert_eq!(f.s.books["b"].sessions.len(), 1);
        f.t.discard(&mut f.s, T0 + 60.0);
        let b = &f.s.books["b"];
        assert_eq!(
            b.totals(),
            Day {
                read_secs: 600.0,
                read_chars: 1000,
                ..Day::default()
            }
        );
        assert_eq!(b.debt, 50);
        assert!(b.sessions.is_empty());
        assert_eq!(f.t.totals, Day::default());
    }

    #[test]
    fn after_a_reset_only_new_reading_counts() {
        let mut f = Fx::new();
        f.t.place(&mut f.s, at(0, 0, 0), &none, T0);
        f.turn(40.0, at(0, 1, 500));
        assert_eq!(f.s.books["b"].sessions.len(), 1);
        // Every book's stats reset while the book is open.
        f.s.books.clear();
        f.t.restart(T0 + 45.0);
        f.turn(70.0, at(0, 2, 800));
        f.t.end(&mut f.s, T0 + 70.0);
        let b = &f.s.books["b"];
        assert_eq!(b.sessions.len(), 1);
        assert_eq!(b.sessions[0].start, (T0 + 45.0) as u64);
        assert_eq!(b.first_read, Some((T0 + 45.0) as u64));
        assert_eq!(b.totals().read_chars, 300);
        assert_eq!(b.totals().read_secs, 25.0);
        assert_eq!(f.t.totals.read_chars, 300);
        // Discarding the session takes back only what came after the reset.
        f.t.discard(&mut f.s, T0 + 80.0);
        assert_eq!(f.s.books["b"].totals(), Day::default());
    }

    #[test]
    fn time_is_flushed_every_30_seconds() {
        let mut f = Fx::new();
        f.frame(input(0.0));
        f.frame(input(29.0));
        assert!(!f.s.books.contains_key("b"));
        f.frame(input(31.0));
        assert_eq!(f.day().read_secs, 31.0);
        assert_eq!(f.s.books["b"].sessions[0].totals.read_secs, 31.0);
        assert!(f.s.books["b"].first_read.is_some());
    }
}
