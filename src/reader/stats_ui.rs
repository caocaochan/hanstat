//! Reading stats in the reader: the tracker's hooks, the line in the bottom
//! margin and the Stats panel. The figures are kept in
//! [`crate::reading_stats`]; the tracker is `tracker.rs`.

use super::layout::ChapterLayout;
use super::theme::Theme;
use super::tracker::{Frame, Pause, Place, State};
use super::{Geometry, Reader, goal_sound, tts};
use crate::fmt_int;
use crate::reading_stats::{self as rs, BookStats, Day, Goal, Summary};
use eframe::egui::{self, FontId, Rect, RichText, pos2};
use std::sync::Arc;

/// What the line and the panel show, made again when the stats change.
#[derive(Default)]
pub(super) struct Cache {
    rev: u64,
    today: i32,
    settings: rs::TrackerSettings,
    /// Every book's reading today, as saved (the running stretch not in it).
    today_day: Day,
    goal: Option<Goal>,
    book_speed: Option<f64>,
    speed: Option<f64>,
    /// For the panel, made while it is open.
    summary: Option<(u64, Summary, Option<BookStats>)>,
    /// The book's Chinese characters, once every chapter is built, and
    /// those before the open chapter.
    book_han: Option<u64>,
    before: Option<(usize, u64)>,
    saved_at: f64,
    unsaved: bool,
    last_state: Option<State>,
    confirm_discard: bool,
    goal_edit: Option<(u32, u64)>,
    /// Whether today's goal was met at the last look, and for which day,
    /// goal and counting of listening: the glow starts only when the same
    /// one turns met.
    goal_seen: Option<(GoalKey, bool)>,
    /// When the goal-met glow started ([`rs::now`]).
    pub(super) glow_since: Option<f64>,
    /// The goal-met sound being made, and the voices for it.
    pub(super) sound: goal_sound::State,
}

/// What a goal's "met" is about: the day, the goal, whether listening counts.
type GoalKey = (i32, Option<Goal>, bool);

/// How long the goal-met glow lasts, in seconds.
pub(super) const GLOW_SECS: f64 = 5.0;

/// The line under the pages.
pub(super) struct Line {
    pub(super) left: String,
    left_short: String,
    left_hover: String,
    pub(super) right: Option<(String, String, String)>,
    /// Today's goal: how far (0..=1) and the bar's hover text.
    goal: Option<(f32, String)>,
}

/// The goal bar's hover text: what it measures and where today stands.
/// `d` is today as the goal measures it; `listening`: listening counts.
fn goal_hover(g: &Goal, d: &Day, listening: bool) -> String {
    let mut need = Vec::new();
    if g.minutes > 0 {
        need.push(format!(
            "{} of {} min",
            (d.read_secs / 60.0).floor(),
            g.minutes
        ));
    }
    if g.chars > 0 {
        need.push(format!(
            "{} of {} characters",
            fmt_int(d.read_chars),
            fmt_int(g.chars)
        ));
    }
    let mut s = format!(
        "Today's reading goal: {}{}.",
        need.join(", "),
        if g.met(d) { " — met" } else { "" }
    );
    if need.len() == 2 {
        s.push_str(" The bar follows whichever is further behind.");
    }
    s.push_str(if listening {
        " Reading and read aloud in every book count. Set the goal in Stats."
    } else {
        " Reading in every book counts; read aloud doesn't. Set the goal in Stats."
    });
    s
}

/// The goal-met glow `t` seconds in: a rainbow round the edge of `rect`,
/// its hues turning, fading in, then out by [`GLOW_SECS`].
pub(super) fn paint_glow(painter: &egui::Painter, rect: Rect, t: f64) {
    let up = (t / 0.4).clamp(0.0, 1.0);
    let down = ((GLOW_SECS - t) / 1.5).clamp(0.0, 1.0);
    let a = up.min(down) as f32;
    let a = a * a * (3.0 - 2.0 * a);
    if a <= 0.0 {
        return;
    }
    let spin = (t * 0.3) as f32;
    let breathe = 1.0 + 0.12 * (t as f32 * std::f32::consts::TAU * 0.7).sin();
    // The glow, fading inward, then a bright rim.
    glow_band(painter, rect, 30.0 * breathe, 0.6 * a, 0.0, spin);
    glow_band(painter, rect, 4.0, 0.95 * a, 0.7 * a, spin);
}

/// A band `width` wide inside the edge of `rect`, its hue going once round
/// the edge (turned by `spin`), `outer`/`inner` its opacity at each side.
fn glow_band(painter: &egui::Painter, rect: Rect, width: f32, outer: f32, inner: f32, spin: f32) {
    use egui::ecolor::HsvaGamma;
    let width = width.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let o = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ];
    let r = rect.shrink(width);
    let i = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
    ];
    let perimeter = 2.0 * (rect.width() + rect.height());
    let colour = |h: f32, a: f32| -> egui::Color32 {
        HsvaGamma {
            h: (h + spin).rem_euclid(1.0),
            s: 0.85,
            v: 1.0,
            a,
        }
        .into()
    };
    let mut mesh = egui::Mesh::default();
    let mut along = 0.0;
    for side in 0..4 {
        let (o0, o1) = (o[side], o[(side + 1) % 4]);
        let (i0, i1) = (i[side], i[(side + 1) % 4]);
        let len = o0.distance(o1);
        let n = (len / 12.0).ceil().max(1.0) as usize;
        for k in 0..=n {
            let f = k as f32 / n as f32;
            let h = (along + len * f) / perimeter;
            let base = mesh.vertices.len() as u32;
            mesh.colored_vertex(o0.lerp(o1, f), colour(h, outer));
            mesh.colored_vertex(i0.lerp(i1, f), colour(h, inner));
            if k > 0 {
                mesh.add_triangle(base - 2, base - 1, base);
                mesh.add_triangle(base - 1, base + 1, base);
            }
        }
        along += len;
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// Where the speed for "time left" comes from: set by hand, or the best
/// measured one.
fn speed_source(c: &Cache, cpm: f32, by_hand: bool) -> (f64, &'static str) {
    if by_hand {
        (cpm.max(1.0) as f64, "set by hand in Settings")
    } else if let Some(v) = c.book_speed {
        (v, "your speed in this book")
    } else if let Some(v) = c.speed {
        (v, "your speed over the last 30 days")
    } else {
        (
            cpm.max(1.0) as f64,
            "the reading speed in Settings, until yours is measured",
        )
    }
}

/// "<1 min", "42 min", "1 h 05 min".
pub(crate) fn mins(secs: f64) -> String {
    let m = (secs.max(0.0) / 60.0).floor() as u64;
    if m == 0 {
        "<1 min".to_owned()
    } else if m < 60 {
        format!("{m} min")
    } else {
        format!("{} h {:02} min", m / 60, m % 60)
    }
}

fn speed_text(chars: u64, secs: f64) -> Option<String> {
    (secs >= 60.0 && chars > 0).then(|| format!("{:.0}/min", chars as f64 / (secs / 60.0)))
}

impl Reader {
    /// Once a frame, with whether a menu or panel is over the page.
    pub(super) fn stats_frame(&mut self, ctx: &egui::Context, menus: bool) {
        let now = rs::now();
        let (input, focused) = ctx.input(|i| {
            (
                i.raw.events.iter().any(|e| {
                    !matches!(
                        e,
                        egui::Event::WindowFocused(_) | egui::Event::ModifiersChanged(_)
                    )
                }),
                i.focused && i.viewport().minimized != Some(true),
            )
        });
        let listening = matches!(
            self.tts.state(),
            tts::PlayState::Playing | tts::PlayState::Loading
        );
        let f = Frame {
            now,
            input,
            focused,
            menus,
            listening,
        };
        rs::edit(|s| self.tracker.frame(s, f));
        self.stats_after(now);
        // Frames for the line's minutes and its idle pause.
        if self.tracker.running() && (self.stats.settings.live_line || self.stats_open) {
            let (r, l) = self.tracker.open_secs(now);
            let shown = if self.tracker.listening() {
                self.tracker.totals.listen_secs + l
            } else {
                self.tracker.totals.read_secs + r
            };
            let mut wait = 60.0 - shown.rem_euclid(60.0) + 0.05;
            if let Some(t) = self.tracker.idle_at() {
                wait = wait.min(t - now + 0.5);
            }
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait.max(1.0)));
        }
    }

    /// Last in a frame, once the page's characters are counted: when
    /// today's goal turns met, start the glow and play the sound (each if
    /// on); wake when a minutes goal will be met; draw the glow over the
    /// whole window while it lasts.
    pub(super) fn goal_met_effects(&mut self, ctx: &egui::Context) {
        let now = rs::now();
        self.stats_refresh(now);
        let day = self.stats.settings.goal_day(&self.today_now(now));
        let goal = self.stats.goal;
        let met = goal.is_some_and(|g| g.met(&day));
        let key = (self.stats.today, goal, self.stats.settings.goal_listening);
        let (glow, sound) = (
            self.stats.settings.goal_glow,
            self.stats.settings.goal_sound,
        );
        // The first look (a book opened, a new day, the goal or what counts
        // changed) only notes where today stands. Once met, it stays met
        // for that look, so paging back under the goal and on again doesn't
        // celebrate twice.
        let was = self
            .stats
            .goal_seen
            .filter(|(k, _)| *k == key)
            .map(|(_, m)| m);
        if was == Some(false) && met {
            if glow {
                self.stats.glow_since = Some(now);
            }
            if sound {
                let secs = goal_sound::play(&self.stats.settings);
                // Not over read aloud: it pauses, and goes on after (with a
                // little for the sound device to open).
                self.tts.pause_until(now + secs + 0.4);
            }
        }
        self.stats.goal_seen = Some((key, met || was == Some(true)));
        // A sound of the reader's own made: it plays from now on.
        if let Some(made) = self.stats.sound.poll() {
            rs::update_meta(|s| s.settings.goal_sound_made = made);
            self.stats.unsaved = true;
            if self.persist {
                self.stats_save(now);
            }
        }
        if let Some(left) = self.tts.sound_pause(now, &self.settings.tts) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(left + 0.01));
        }
        // A minutes goal is met by time going by, with no input to draw a
        // frame: wake for it once the characters (if any) are there.
        if let Some(g) = goal
            && !met
            && (glow || sound)
            && self.tracker.running()
            && g.minutes > 0
            && day.read_chars >= g.chars
        {
            let left = g.minutes as f64 * 60.0 - day.read_secs;
            if left > 0.0 {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(left + 0.05));
            }
        }
        let Some(since) = self.stats.glow_since else {
            return;
        };
        let t = now - since;
        if !(0.0..GLOW_SECS).contains(&t) || !glow {
            self.stats.glow_since = None;
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::unique("reader-goal-glow"),
        ));
        paint_glow(&painter, ctx.content_rect(), t);
        ctx.request_repaint();
    }

    /// Where the page settled this frame.
    pub(super) fn stats_place(&mut self, l: &Arc<ChapterLayout>) {
        let now = rs::now();
        let p = Place {
            spine: self.spine,
            page: self.page,
            han: l.page_han(self.page),
            chapter_han: l.han_total,
            layout: Arc::as_ptr(l) as usize,
        };
        let chapters = self.chapters.clone();
        let between = move |r: std::ops::Range<usize>| -> u64 {
            let c = chapters.lock().unwrap();
            r.filter_map(|i| c.get(i)?.as_ref().map(|ch| ch.han_len as u64))
                .sum()
        };
        rs::edit(|s| self.tracker.place(s, p, &between, now));
        if self.stats.book_han.is_none() || self.stats.before.is_none_or(|b| b.0 != self.spine) {
            let c = self.chapters.lock().unwrap();
            let sum = |r: std::ops::Range<usize>| -> Option<u64> {
                c[r].iter()
                    .map(|ch| ch.as_ref().map(|ch| ch.han_len as u64))
                    .sum()
            };
            let total = sum(0..c.len());
            let before = sum(0..self.spine.min(c.len())).map(|n| (self.spine, n));
            drop(c);
            self.stats.before = before;
            if let (None, Some(n)) = (self.stats.book_han, total) {
                self.stats.book_han = Some(n);
                rs::edit(|s| self.tracker.book_han(s, n));
            }
        }
        self.stats_after(now);
    }

    /// Next on the book's last page.
    pub(super) fn stats_finish(&mut self) {
        let now = rs::now();
        if rs::edit(|s| self.tracker.finish(s, now)) {
            self.finished = true;
        }
        self.stats_after(now);
        if self.stats.unsaved && self.persist {
            self.stats_save(now);
        }
    }

    pub(super) fn stats_toggle(&mut self) {
        let now = rs::now();
        rs::edit(|s| self.tracker.toggle(s, now));
        self.stats_after(now);
    }

    /// The book closes (or the app): add the running stretch and save.
    pub fn end_stats(&mut self) {
        let now = rs::now();
        rs::edit(|s| self.tracker.end(s, now));
        self.stats_after(now);
        if self.stats.unsaved && self.persist {
            self.stats_save(now);
        }
    }

    /// Mark the stats changed and save them now and then: every
    /// [`super::tracker::FLUSH_SECS`] and when the tracker pauses or starts.
    fn stats_after(&mut self, now: f64) {
        if self.tracker.changed {
            self.tracker.changed = false;
            rs::book_changed(&self.book_id);
            self.stats.unsaved = true;
        }
        let state = self.tracker.state();
        if self.stats.unsaved
            && self.persist
            && (self.stats.last_state != Some(state)
                || now - self.stats.saved_at >= super::tracker::FLUSH_SECS)
        {
            self.stats_save(now);
        }
        self.stats.last_state = Some(state);
    }

    fn stats_save(&mut self, now: f64) {
        self.stats.unsaved = false;
        self.stats.saved_at = now;
        if let Err(e) = rs::save() {
            self.error = Some(format!("Cannot save the reading stats: {e}"));
        }
    }

    /// Bring the cache up to date with the stats and the day.
    fn stats_refresh(&mut self, now: f64) {
        let rev = rs::revision();
        let today = rs::local_day(now, self.stats.settings.day_start_hour);
        if rev == self.stats.rev && today == self.stats.today {
            return;
        }
        let book = self.book_id.clone();
        let c = &mut self.stats;
        rs::with(|s| {
            c.settings = s.settings.clone();
            let today = rs::local_day(now, s.settings.day_start_hour);
            c.today = today;
            c.today_day = Day::default();
            for b in s.books.values() {
                if let Some(d) = b.days.get(&today) {
                    c.today_day.add(d);
                }
            }
            c.goal = s.goal_on(today);
            c.book_speed = s.books.get(&book).and_then(BookStats::speed);
            c.speed = s.measured_speed(today);
        });
        c.rev = rev;
    }

    /// Today's reading, the running stretch included.
    fn today_now(&self, now: f64) -> Day {
        let (r, l) = self.tracker.open_secs(now);
        let mut d = self.stats.today_day;
        d.read_secs += r;
        d.listen_secs += l;
        d
    }

    /// The line under the pages, unless hidden.
    pub(super) fn stats_line(&mut self, now: f64) -> Option<Line> {
        self.stats_refresh(now);
        if !self.stats.settings.live_line {
            return None;
        }
        let t = &self.tracker;
        let (r, l) = t.open_secs(now);
        let read = t.totals.read_secs + r;
        let chars = t.totals.read_chars;
        let listening = t.listening();
        let (time, n) = if listening {
            (t.totals.listen_secs + l, t.totals.listen_chars)
        } else {
            (read, chars)
        };
        let mut parts = Vec::new();
        let state = match t.state() {
            State::Running if listening => Some("Listening".to_owned()),
            State::Running => None,
            State::Paused(p) => Some(p.label().to_owned()),
        };
        if let Some(s) = &state {
            parts.push(s.clone());
        }
        parts.push(mins(time));
        parts.push(format!("{} chars", fmt_int(n)));
        if !listening && let Some(sp) = speed_text(chars, read) {
            parts.push(sp);
        }
        let left = parts.join(" · ");
        let left_short = match &state {
            Some(s) => s.clone(),
            None => mins(time),
        };
        let how = "Chinese characters paged past: paging back takes them off again, and jumps (contents, links) don't count.";
        let action = match t.state() {
            State::Paused(Pause::Manual) => "Click or press P to start timing.".to_owned(),
            State::Paused(Pause::Idle) => format!(
                "No input for {} min, so the time since then isn't counted. Any input starts timing again.",
                self.stats.settings.idle_mins
            ),
            State::Paused(Pause::Unfocused) => {
                "The window isn't active. Timing goes on when you come back.".to_owned()
            }
            State::Paused(Pause::Menu) => "Timing waits while a menu or panel is open.".to_owned(),
            State::Running if listening => {
                "Read aloud is playing: this counts as listening, apart from reading.".to_owned()
            }
            State::Running => "Click or press P to pause.".to_owned(),
        };
        let left_hover = format!(
            "This session: {} of reading, {} {how}\n{action}",
            mins(read),
            if read >= 60.0 && chars > 0 {
                format!(
                    "{} characters ({:.0} a minute).",
                    fmt_int(chars),
                    chars as f64 / (read / 60.0)
                )
            } else {
                format!("{} characters.", fmt_int(chars))
            },
        );
        // Time left, at the best speed known.
        let right = self.tracker.base().map(|p| {
            let (speed, source) = speed_source(&self.stats, self.cpm, self.cpm_by_hand);
            let chapter = p.chapter_han.saturating_sub(p.han) as f64 / speed * 60.0;
            let book = match (self.stats.book_han, self.stats.before) {
                (Some(total), Some((spine, before))) if spine == p.spine => {
                    Some(total.saturating_sub(before + p.han as u64) as f64 / speed * 60.0)
                }
                _ => None,
            };
            let full = match book {
                Some(b) => format!("Chapter {} · Book {}", mins(chapter), mins(b)),
                None => format!("Chapter {}", mins(chapter)),
            };
            let short = match book {
                Some(b) => format!("Book {}", mins(b)),
                None => format!("Chapter {}", mins(chapter)),
            };
            let hover = format!(
                "Time left to the end of the chapter and of the book, at {:.0} characters a minute ({source}).",
                speed
            );
            (full, short, hover)
        });
        let goal = self.stats.goal.map(|g| {
            let d = self.stats.settings.goal_day(&self.today_now(now));
            (
                g.progress(&d),
                goal_hover(&g, &d, self.stats.settings.goal_listening),
            )
        });
        Some(Line {
            left,
            left_short,
            left_hover,
            right,
            goal,
        })
    }

    /// Draw the line under the pages; returns the clickable part (the
    /// session), the time-left part and the goal bar with their hover text.
    pub(super) fn draw_stats_line(
        ui: &egui::Ui,
        th: &Theme,
        g: &Geometry,
        step: usize,
        line: &Line,
        font: &FontId,
    ) -> Vec<(Rect, String, bool)> {
        let p = ui.painter();
        let mut out = Vec::new();
        let first = g.pages[0];
        let last = g.pages[step - 1];
        let y = first.bottom() - g.inset.y * 0.5;
        // Half a page less the folio.
        let room = first.width() / 2.0 - g.inset.x - 36.0;
        let fit = |a: &str, b: &str| -> Option<std::sync::Arc<egui::Galley>> {
            [a, b]
                .into_iter()
                .map(|s| p.layout_no_wrap(s.to_owned(), font.clone(), th.muted))
                .find(|gal| gal.size().x <= room)
        };
        if let Some(gal) = fit(&line.left, &line.left_short) {
            let pos = pos2(first.left() + g.inset.x, y - gal.size().y / 2.0);
            let r = Rect::from_min_size(pos, gal.size());
            p.galley(pos, gal, th.muted);
            out.push((r, line.left_hover.clone(), true));
        }
        if let Some((full, short, hover)) = &line.right
            && let Some(gal) = fit(full, short)
        {
            let pos = pos2(
                last.right() - g.inset.x - gal.size().x,
                y - gal.size().y / 2.0,
            );
            let r = Rect::from_min_size(pos, gal.size());
            p.galley(pos, gal, th.muted);
            out.push((r, hover.clone(), false));
        }
        // Today's goal: a hairline along the foot of the pages, its hover
        // area the whole width and a few points taller, so it can be found.
        if let Some((f, hover)) = &line.goal {
            let x0 = first.left();
            let x1 = last.right();
            let yy = first.bottom() - 2.0;
            let r = Rect::from_min_max(pos2(x0, yy), pos2(x0 + (x1 - x0) * f, yy + 2.0));
            p.rect_filled(r, 0.0, th.accent.gamma_multiply(0.75));
            let area = Rect::from_min_max(pos2(x0, yy - 4.0), pos2(x1, first.bottom() + 2.0));
            out.push((area, hover.clone(), false));
        }
        out
    }

    /// The Stats button and panel in the top bar.
    pub(super) fn stats_menu(&mut self, ui: &mut egui::Ui) {
        let button = ui
            .selectable_label(self.stats_open, RichText::new("Stats").size(14.0))
            .on_hover_text("Reading time, characters and speed (P pauses timing)");
        if button.clicked() {
            self.stats_open = !self.stats_open;
            self.stats.confirm_discard = false;
            self.stats.goal_edit = None;
            self.stats.sound.forget_voices();
        }
        let mut open = self.stats_open;
        egui::Popup::menu(&button)
            .id(egui::Id::unique("reader-stats"))
            .open_bool(&mut open)
            .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
            .show(|ui| self.stats_panel(ui));
        self.stats_open = open;
    }

    fn stats_panel(&mut self, ui: &mut egui::Ui) {
        let th = super::theme::get(ui.ctx());
        let now = rs::now();
        self.stats_refresh(now);
        let rev = rs::revision();
        if self.stats.summary.as_ref().is_none_or(|s| s.0 != rev) {
            let today = self.stats.today;
            let id = self.book_id.clone();
            let (sum, book) = rs::with(|s| (s.summary(today), s.books.get(&id).cloned()));
            self.stats.summary = Some((rev, sum, book));
        }
        ui.set_min_width(380.0);
        ui.set_max_width(440.0);
        let (r_open, l_open) = self.tracker.open_secs(now);
        let open = Day {
            read_secs: r_open,
            listen_secs: l_open,
            ..Day::default()
        };
        let Some((rev, sum, book)) = self.stats.summary.take() else {
            return;
        };
        self.stats_panel_body(ui, th, now, open, &sum, book.as_ref());
        self.stats.summary.get_or_insert((rev, sum, book));
    }

    fn stats_panel_body(
        &mut self,
        ui: &mut egui::Ui,
        th: &Theme,
        now: f64,
        open: Day,
        sum: &Summary,
        book: Option<&BookStats>,
    ) {
        let muted = th.muted;

        // State and the button.
        let mut toggle = false;
        ui.horizontal(|ui| {
            let (text, color) = match self.tracker.state() {
                State::Running if self.tracker.listening() => ("Listening (read aloud)", th.text),
                State::Running => ("Timing", th.text),
                State::Paused(Pause::Menu) => ("Timing waits while this panel is open", muted),
                State::Paused(p) => (p.label(), muted),
            };
            ui.label(RichText::new(text).color(color).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let manual = self.tracker.state() == State::Paused(Pause::Manual);
                let b = ui.button(if manual { "Start  (P)" } else { "Pause  (P)" });
                toggle = b.clicked();
            });
        });
        ui.add_space(4.0);

        // The figures.
        let mut session = self.tracker.totals;
        session.add(&open);
        let mut today = sum.days.get(&sum.today).copied().unwrap_or_default();
        today.add(&open);
        let mut this_book = book.map(BookStats::totals).unwrap_or_default();
        this_book.add(&open);
        let mut all = sum.totals;
        all.add(&open);
        let listened = all.listen_secs >= 1.0;
        egui::Grid::new("reader-stats-grid")
            .num_columns(if listened { 5 } else { 4 })
            .spacing([18.0, 4.0])
            .show(ui, |ui| {
                ui.label("");
                for (h, tip) in [
                    ("Time", "Time reading, idle stretches and listening left out"),
                    ("Characters", "Chinese characters paged past while timing"),
                    ("Speed", "Characters a minute of reading (after a minute of it)"),
                ] {
                    ui.label(RichText::new(h).color(muted)).on_hover_text(tip);
                }
                if listened {
                    ui.label(RichText::new("Listening").color(muted)).on_hover_text(
                        if self.stats.settings.goal_listening {
                            "Time and characters while read aloud played: kept out of speed, counted toward the daily goal (Settings, ⚙)"
                        } else {
                            "Time and characters while read aloud played: kept out of speed and goals"
                        },
                    );
                }
                ui.end_row();
                for (name, d) in [
                    ("This session", session),
                    ("Today", today),
                    ("This book", this_book),
                    ("All books", all),
                ] {
                    ui.label(name);
                    ui.label(mins(d.read_secs));
                    ui.label(fmt_int(d.read_chars));
                    ui.label(speed_text(d.read_chars, d.read_secs).unwrap_or_else(|| "–".into()));
                    if listened {
                        ui.label(if d.listen_secs >= 1.0 {
                            format!("{} · {}", mins(d.listen_secs), fmt_int(d.listen_chars))
                        } else {
                            "–".into()
                        });
                    }
                    ui.end_row();
                }
            });

        // The book.
        if let Some(b) = book {
            let mut bits = Vec::new();
            if let Some(t) = b.first_read {
                bits.push(format!(
                    "Started {}",
                    rs::date(rs::day_of(t, &self.stats.settings))
                ));
            }
            let days = b
                .days
                .values()
                .filter(|d| d.read_secs >= rs::DAY_READ_SECS)
                .count();
            if days > 0 {
                bits.push(format!(
                    "{days} day{} read",
                    if days == 1 { "" } else { "s" }
                ));
            }
            if let Some(t) = b.finished {
                bits.push(format!(
                    "finished {}",
                    rs::date(rs::day_of(t, &self.stats.settings))
                ));
            }
            if !bits.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new(bits.join(" · ")).color(muted));
            }
        }
        ui.separator();

        // Goal and streaks.
        let goal = sum.goal;
        if let Some(g) = goal {
            let gday = self.stats.settings.goal_day(&today);
            let mut need = Vec::new();
            if g.minutes > 0 {
                need.push(format!(
                    "{} of {} min",
                    (gday.read_secs / 60.0).floor(),
                    g.minutes
                ));
            }
            if g.chars > 0 {
                need.push(format!(
                    "{} of {} chars",
                    fmt_int(gday.read_chars),
                    fmt_int(g.chars)
                ));
            }
            let p = g.progress(&gday);
            ui.label(format!(
                "Today's goal: {}{}",
                need.join(", "),
                if p >= 1.0 { " — met" } else { "" }
            ));
            ui.add(
                egui::ProgressBar::new(p)
                    .desired_height(6.0)
                    .fill(th.accent),
            );
        }
        let (cur, best) = sum.read_streak;
        let mut streak = if cur > 0 {
            format!(
                "Read {cur} day{} in a row (longest {best})",
                if cur == 1 { "" } else { "s" }
            )
        } else if best > 0 {
            format!("No current streak (longest {best} days)")
        } else {
            "Read a minute or more a day to start a streak".to_owned()
        };
        if goal.is_some() || sum.goal_streak.1 > 0 {
            let (gc, gb) = sum.goal_streak;
            streak.push_str(&format!(
                " · goal met {gc} day{} running (longest {gb})",
                if gc == 1 { "" } else { "s" }
            ));
        }
        ui.label(RichText::new(streak).color(muted))
            .on_hover_text("A day counts once it has a minute of reading. Today doesn't break a streak until it is over.");

        // Measured speed, and what reading times use.
        ui.separator();
        match sum.speed {
            Some(v) => {
                ui.label(format!("Your speed: {v:.0} chars/min"))
                    .on_hover_text(
                        "Characters read ÷ minutes read over the last 30 days, listening left out",
                    );
            }
            None => {
                ui.label(
                    RichText::new("Your speed shows after 10 minutes of reading.").color(muted),
                );
            }
        }
        let note = if self.cpm_by_hand {
            format!(
                "Reading times use {:.0} chars/min, set by hand in Settings (⚙).",
                self.cpm
            )
        } else if sum.speed.is_some() {
            "Reading times in the Library and time left here use it.".to_owned()
        } else {
            format!(
                "Until then, reading times use {:.0} chars/min from Settings (⚙).",
                self.cpm
            )
        };
        ui.label(RichText::new(note).color(muted).small());

        // Discard.
        let session_any = !session.is_empty();
        ui.horizontal(|ui| {
            if self.stats.confirm_discard {
                ui.label(format!(
                    "Discard {} and {} chars?",
                    mins(session.read_secs + session.listen_secs),
                    fmt_int(session.read_chars + session.listen_chars)
                ));
                if ui.button("Discard").clicked() {
                    rs::edit(|s| self.tracker.discard(s, now));
                    self.stats.confirm_discard = false;
                }
                if ui.button("Keep").clicked() {
                    self.stats.confirm_discard = false;
                }
            } else if ui
                .add_enabled(session_any, egui::Button::new("Discard this session…"))
                .on_hover_text("Take this session's time and characters out of the stats")
                .clicked()
            {
                self.stats.confirm_discard = true;
            }
        });

        // Settings.
        ui.separator();
        let mut set = self.stats.settings.clone();
        let mut goal_edit = self
            .stats
            .goal_edit
            .unwrap_or_else(|| goal.map_or((0, 0), |g| (g.minutes, g.chars)));
        egui::CollapsingHeader::new("Settings")
            .id_salt("reader-stats-settings")
            .show(ui, |ui| {
                egui::Grid::new("reader-stats-settings-grid")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Start timing");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut set.auto_start, true, "When a book opens");
                            ui.radio_value(&mut set.auto_start, false, "By hand (P)");
                        });
                        ui.end_row();
                        ui.label("Pause after");
                        ui.add(
                            egui::DragValue::new(&mut set.idle_mins)
                                .range(1..=60)
                                .suffix(" min without input"),
                        );
                        ui.end_row();
                        ui.label("Days start at");
                        ui.add(
                            egui::DragValue::new(&mut set.day_start_hour)
                                .range(0..=23)
                                .suffix(":00"),
                        )
                        .on_hover_text(
                            "Reading after midnight and before this counts toward the day before",
                        );
                        ui.end_row();
                        ui.label("Under the pages");
                        ui.checkbox(&mut set.live_line, "Show session and time left");
                        ui.end_row();
                        ui.label("Daily goal");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut goal_edit.0)
                                    .range(0..=600)
                                    .suffix(" min"),
                            );
                            ui.label("and");
                            ui.add(
                                egui::DragValue::new(&mut goal_edit.1)
                                    .range(0..=200_000)
                                    .speed(50.0)
                                    .suffix(" chars"),
                            );
                        })
                        .response
                        .on_hover_text("0 turns either off. A change applies from today on.");
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut set.goal_glow, "Glow round the window when it's met")
                            .on_hover_text(
                                "A rainbow border for 5 seconds when today's goal is reached while reading",
                            );
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut set.goal_sound, "Play a sound when it's met")
                            .on_hover_text(
                                "A voice cheers when today's goal is reached while reading: the built-in one, or one of yours saying your own message",
                            );
                        ui.end_row();
                        if set.goal_sound {
                            self.stats.sound.rows(
                                ui,
                                &mut set,
                                &self.settings.tts.local,
                                muted,
                                th.red,
                            );
                        }
                    });
            });
        let goal_now = goal.map_or((0, 0), |g| (g.minutes, g.chars));
        let goal_changed = goal_edit != goal_now;
        // A goal is set once the drag ends.
        let dragging = ui.ctx().input(|i| i.pointer.any_down());
        self.stats.goal_edit = (goal_changed && dragging).then_some(goal_edit);
        if set != self.stats.settings || (goal_changed && !dragging) {
            let today = self.stats.today;
            rs::update_meta(|s| {
                s.settings = set;
                if goal_changed && !dragging {
                    s.set_goal(today, goal_edit.0, goal_edit.1);
                }
                self.tracker.settings(s);
            });
            self.stats.unsaved = true;
            if self.persist {
                self.stats_save(now);
            }
        }
        if toggle {
            self.stats_toggle();
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn minutes_read_well() {
        assert_eq!(super::mins(0.0), "<1 min");
        assert_eq!(super::mins(59.0), "<1 min");
        assert_eq!(super::mins(61.0), "1 min");
        assert_eq!(super::mins(3600.0 + 300.0), "1 h 05 min");
        assert_eq!(super::speed_text(300, 59.0), None);
        assert_eq!(super::speed_text(300, 60.0).as_deref(), Some("300/min"));
    }
}
