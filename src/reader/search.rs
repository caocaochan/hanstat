//! Ctrl+F: the book searched on a thread of its own, the results in a
//! sidebar over the pages' left edge (the pages don't move), each with its
//! sentence and a go-to button. Going to a result is a peek
//! ([`super::Reader`]'s `peek`): the place, the reading time and the stats
//! stay where reading was until Back (or "Keep reading here").
//!
//! Matching is literal, after folding both sides the same way: Traditional
//! to Simplified (as known words are matched), Latin letters to lower case,
//! and spaces and line breaks left out, so a match can cross a line break.

use super::anki::sentence_around;
use super::theme::Theme;
use super::tree::Chapter;
use eframe::egui::{self, Color32, RichText};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// At most this many results are listed.
pub const MAX_HITS: usize = 1000;

/// Characters of the sentence shown on either side of a match, at most.
const CONTEXT: usize = 50;

/// One match: where it is, and its sentence split around it.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub spine: usize,
    /// Its characters in the chapter's [`Chapter::chars`].
    pub range: Range<u32>,
    pub before: String,
    pub text: String,
    pub after: String,
}

/// A character as matching sees it; `None`: left out.
fn norm(c: char) -> Option<char> {
    if c == '\0' || c.is_whitespace() {
        return None;
    }
    let c = hanstat::anki::fold_char(c, true);
    Some(if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        c
    })
}

/// The query as matching sees it.
pub fn norm_query(q: &str) -> Vec<char> {
    q.chars().filter_map(norm).collect()
}

/// Every match of `q` (from [`norm_query`]) in `chars`: their ranges.
pub fn find(chars: &[char], q: &[char], max: usize) -> Vec<Range<u32>> {
    let mut out = Vec::new();
    if q.is_empty() {
        return out;
    }
    // The text as matching sees it, with where each character was.
    let (text, at): (Vec<char>, Vec<u32>) = chars
        .iter()
        .enumerate()
        .filter_map(|(i, &c)| norm(c).map(|n| (n, i as u32)))
        .unzip();
    let mut i = 0;
    while i + q.len() <= text.len() && out.len() < max {
        if text[i] == q[0] && text[i..i + q.len()] == *q {
            out.push(at[i]..at[i + q.len() - 1] + 1);
            i += q.len();
        } else {
            i += 1;
        }
    }
    out
}

/// `chars[range]` with its sentence around it, cut to [`CONTEXT`] a side.
fn hit(spine: usize, chars: &[char], range: Range<u32>) -> Hit {
    let (s, e) = (range.start as usize, range.end as usize);
    let text: String = chars[s..e].iter().filter(|c| **c != '\0').collect();
    let clean = |cs: &[char]| -> String {
        cs.iter()
            .map(|&c| if c == '\0' { ' ' } else { c })
            .collect::<String>()
    };
    let (before, after) = match sentence_around(chars, s) {
        Some(sent) => {
            let at = sent.at.min(sent.text.len());
            let end = (at + (e - s)).min(sent.text.len());
            let b = &sent.text[at.saturating_sub(CONTEXT)..at];
            let a = &sent.text[end..(end + CONTEXT).min(sent.text.len())];
            let mut before = clean(b);
            if at > CONTEXT {
                before.insert(0, '…');
            }
            let mut after = clean(a);
            if end + CONTEXT < sent.text.len() {
                after.push('…');
            }
            (before, after)
        }
        None => (String::new(), String::new()),
    };
    Hit {
        spine,
        range,
        before: before.trim_start().to_owned(),
        text,
        after: after.trim_end().to_owned(),
    }
}

enum Msg {
    /// A chapter searched: its results (`gen` of the query).
    Chapter {
        generation: u64,
        hits: Vec<Hit>,
        done: usize,
    },
}

/// Search every chapter for `q`, in order, waiting for chapters still
/// being built; stops when a newer query starts.
fn run(
    chapters: Arc<Mutex<Vec<Option<Arc<Chapter>>>>>,
    q: Vec<char>,
    generation: u64,
    latest: Arc<AtomicU64>,
    tx: Sender<Msg>,
    repaint: Arc<dyn Fn() + Send + Sync>,
) {
    let n = chapters.lock().unwrap().len();
    let mut left = MAX_HITS;
    for spine in 0..n {
        let ch = loop {
            if latest.load(Ordering::Relaxed) != generation {
                return;
            }
            if let Some(c) = chapters.lock().unwrap()[spine].clone() {
                break c;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let hits: Vec<Hit> = find(&ch.chars, &q, left)
            .into_iter()
            .map(|r| hit(spine, &ch.chars, r))
            .collect();
        left -= hits.len();
        if tx
            .send(Msg::Chapter {
                generation,
                hits,
                done: spine + 1,
            })
            .is_err()
        {
            return;
        }
        repaint();
        if left == 0 {
            let _ = tx.send(Msg::Chapter {
                generation,
                hits: Vec::new(),
                done: n,
            });
            repaint();
            return;
        }
    }
}

/// What a click in the sidebar asks the reader for.
pub enum Action {
    /// Show this result's page.
    Go(Hit),
}

pub struct Search {
    chapters: Arc<Mutex<Vec<Option<Arc<Chapter>>>>>,
    repaint: Arc<dyn Fn() + Send + Sync>,
    /// Shown (Ctrl+F); hidden again once a result is chosen, its results kept.
    pub open: bool,
    pub query: String,
    /// The box takes the keys next frame (and its text is selected).
    focus: bool,
    /// The query searched for, and when the box last changed.
    searched: String,
    edited_at: f64,
    latest: Arc<AtomicU64>,
    rx: Option<Receiver<Msg>>,
    pub hits: Vec<Hit>,
    /// Chapters searched, of all.
    done: usize,
    chapters_n: usize,
    /// The result chosen last (drawn as chosen).
    pub chosen: Option<usize>,
    /// The sidebar's width, as dragged.
    pub width: f32,
    /// Where the sidebar is on screen this frame (the pages under it take no
    /// clicks or wheel).
    pub rect: Option<egui::Rect>,
}

impl Search {
    pub fn new(
        chapters: Arc<Mutex<Vec<Option<Arc<Chapter>>>>>,
        ctx: &egui::Context,
        width: f32,
    ) -> Search {
        let n = chapters.lock().unwrap().len();
        let ctx = ctx.clone();
        Search {
            chapters,
            repaint: Arc::new(move || ctx.request_repaint()),
            open: false,
            query: String::new(),
            focus: false,
            searched: String::new(),
            edited_at: 0.0,
            latest: Arc::new(AtomicU64::new(0)),
            rx: None,
            hits: Vec::new(),
            done: 0,
            chapters_n: n,
            chosen: None,
            width: width.clamp(220.0, 600.0),
            rect: None,
        }
    }

    /// Ctrl+F: shown, the box focused and its text selected.
    pub fn show(&mut self) {
        self.open = true;
        self.focus = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.rect = None;
    }

    pub fn searching(&self) -> bool {
        self.rx.is_some() && self.done < self.chapters_n
    }

    /// Start searching for the query (the last one stops).
    fn start(&mut self) {
        let generation = self.latest.fetch_add(1, Ordering::Relaxed) + 1;
        self.searched = self.query.clone();
        self.hits.clear();
        self.chosen = None;
        self.done = 0;
        let q = norm_query(&self.query);
        if q.is_empty() {
            self.rx = None;
            return;
        }
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let (chapters, latest, repaint) = (
            self.chapters.clone(),
            self.latest.clone(),
            self.repaint.clone(),
        );
        let _ = std::thread::Builder::new()
            .name("reader-search".into())
            .spawn(move || run(chapters, q, generation, latest, tx, repaint));
    }

    /// Once a frame: results that came, and a query typed a moment ago
    /// searched for.
    pub fn poll(&mut self, now: f64, ctx: &egui::Context) {
        if self.query != self.searched {
            let wait = 0.15 - (now - self.edited_at);
            if wait <= 0.0 {
                self.start();
            } else {
                ctx.request_repaint_after(Duration::from_secs_f64(wait));
            }
        }
        let generation = self.latest.load(Ordering::Relaxed);
        if let Some(rx) = &self.rx {
            for m in rx.try_iter() {
                let Msg::Chapter {
                    generation: g,
                    hits,
                    done,
                } = m;
                if g == generation {
                    self.hits.extend(hits);
                    self.done = done;
                }
            }
        }
    }

    /// The sidebar, over the left of `area` (the pages' area).
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        area: egui::Rect,
        th: &Theme,
        labels: &[String],
        now: f64,
    ) -> Option<Action> {
        if !self.open {
            self.rect = None;
            return None;
        }
        let mut action = None;
        let width = self.width.min(area.width() - 40.0).max(180.0);
        let rect = egui::Rect::from_min_size(area.min, egui::vec2(width, area.height()));
        self.rect = Some(rect);
        let mut close = false;
        egui::Area::new(egui::Id::unique("reader-search"))
            .order(egui::Order::Middle)
            .fixed_pos(rect.min)
            .constrain(false)
            .show(ctx, |ui| {
                ui.set_min_size(rect.size());
                ui.set_max_size(rect.size());
                let painter = ui.painter();
                painter.rect_filled(rect, 0.0, th.bar);
                painter.line_segment(
                    [rect.right_top(), rect.right_bottom()],
                    egui::Stroke::new(1.0, th.muted.gamma_multiply(0.5)),
                );
                let inner = rect.shrink2(egui::vec2(10.0, 8.0));
                ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Search the book").color(th.text).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.visuals_mut().override_text_color = Some(th.subtle);
                            if ui
                                .add(
                                    egui::Button::new(super::icons::text(
                                        ctx,
                                        super::icons::X,
                                        "",
                                        14.0,
                                    ))
                                    .frame(false),
                                )
                                .on_hover_text("Close (Esc)")
                                .clicked()
                            {
                                close = true;
                            }
                        });
                    });
                    let edit = egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Words or a phrase")
                        .desired_width(f32::INFINITY);
                    let mut out = edit.show(ui);
                    if self.focus {
                        self.focus = false;
                        out.response.request_focus();
                        let all = egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0),
                            egui::text::CCursor::new(self.query.chars().count()),
                        );
                        out.state.cursor.set_char_range(Some(all));
                        out.state.store(ctx, out.response.id);
                    }
                    if out.response.changed() {
                        self.edited_at = now;
                    }
                    // egui lets go of the box on Esc while drawing it.
                    if (out.response.has_focus() || out.response.lost_focus())
                        && ui.input(|i| i.key_pressed(egui::Key::Escape))
                    {
                        close = true;
                    }
                    let enter =
                        out.response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if enter && !self.hits.is_empty() {
                        let i = self.chosen.map_or(0, |c| (c + 1) % self.hits.len());
                        self.chosen = Some(i);
                        action = Some(Action::Go(self.hits[i].clone()));
                        // Enter again: the next result.
                        self.focus = true;
                    }
                    ui.add_space(2.0);
                    let status = if self.searched.trim().is_empty() {
                        String::new()
                    } else {
                        let chapters = {
                            let mut s: Vec<usize> = self.hits.iter().map(|h| h.spine).collect();
                            s.dedup();
                            s.len()
                        };
                        let n = self.hits.len();
                        let mut t = match n {
                            0 if self.searching() => "Searching…".to_owned(),
                            0 => "Nothing found".to_owned(),
                            1 => "1 result".to_owned(),
                            _ if n >= MAX_HITS => format!("The first {MAX_HITS} results"),
                            _ => format!(
                                "{n} results in {chapters} chapter{}",
                                if chapters == 1 { "" } else { "s" }
                            ),
                        };
                        if self.searching() && n > 0 {
                            t.push_str(&format!(
                                " (searching, {} of {} chapters)",
                                self.done, self.chapters_n
                            ));
                        }
                        t
                    };
                    ui.label(RichText::new(status).size(12.0).color(th.subtle));
                    ui.separator();
                    let row_h = 64.0;
                    let hits = &self.hits;
                    let chosen = self.chosen;
                    let mut go = None;
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show_rows(ui, row_h, hits.len(), |ui, rows| {
                            for i in rows {
                                if let Some(()) =
                                    result_row(ui, &hits[i], chosen == Some(i), th, labels, row_h)
                                {
                                    go = Some(i);
                                }
                            }
                        });
                    if let Some(i) = go {
                        self.chosen = Some(i);
                        action = Some(Action::Go(self.hits[i].clone()));
                    }
                });
                // Drag the right edge to resize.
                let edge = egui::Rect::from_x_y_ranges(
                    rect.right() - 4.0..=rect.right() + 3.0,
                    rect.y_range(),
                );
                let r = ui.interact(
                    edge,
                    egui::Id::unique("reader-search-edge"),
                    egui::Sense::drag(),
                );
                if r.hovered() || r.dragged() {
                    ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
                if r.dragged() {
                    self.width = (self.width + r.drag_delta().x).clamp(220.0, 600.0);
                }
            });
        if close {
            self.close();
        }
        action
    }
}

/// One result: its chapter, its sentence with the match in bold, and a
/// go-to button (the whole row goes too). `Some` when clicked.
fn result_row(
    ui: &mut egui::Ui,
    h: &Hit,
    chosen: bool,
    th: &Theme,
    labels: &[String],
    row_h: f32,
) -> Option<()> {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, row_h), egui::Sense::click());
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            true,
            format!("Go to {}{}{}", h.before, h.text, h.after),
        )
    });
    if !ui.is_rect_visible(rect) {
        return resp.clicked().then_some(());
    }
    let p = ui.painter();
    if chosen {
        p.rect_filled(rect, 4.0, th.accent.gamma_multiply(0.16));
    } else if resp.hovered() {
        p.rect_filled(rect, 4.0, th.muted.gamma_multiply(0.14));
    }
    let icon = 22.0;
    let text_rect = egui::Rect::from_min_max(
        rect.min + egui::vec2(6.0, 4.0),
        egui::pos2(rect.right() - icon - 10.0, rect.bottom() - 4.0),
    );
    let label = labels
        .get(h.spine)
        .filter(|l| !l.is_empty())
        .cloned()
        .unwrap_or_else(|| format!("Part {}", h.spine + 1));
    let font = egui::FontId::proportional(13.0);
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = text_rect.width();
    job.wrap.max_rows = 2;
    job.wrap.break_anywhere = true;
    let fmt = |color: Color32, strong: bool| egui::TextFormat {
        font_id: font.clone(),
        color,
        background: if strong {
            th.orange.gamma_multiply(if th.dark { 0.3 } else { 0.35 })
        } else {
            Color32::TRANSPARENT
        },
        ..Default::default()
    };
    job.append(&h.before, 0.0, fmt(th.text, false));
    job.append(&h.text, 0.0, fmt(th.ink, true));
    job.append(&h.after, 0.0, fmt(th.text, false));
    let head = ui
        .painter()
        .layout_no_wrap(label, egui::FontId::proportional(11.0), th.subtle);
    let galley = ui.painter().layout_job(job);
    let p = ui.painter();
    p.galley(text_rect.min, head.clone(), th.subtle);
    p.galley(
        text_rect.min + egui::vec2(0.0, head.size().y + 3.0),
        galley,
        th.text,
    );
    // The go-to icon: Phosphor's arrow into a square.
    let c = if resp.hovered() { th.accent } else { th.subtle };
    super::icons::paint(
        p,
        egui::pos2(rect.right() - icon / 2.0 - 6.0, rect.center().y),
        super::icons::ARROW_SQUARE_IN,
        icon,
        false,
        c,
    );
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let clicked = resp.clicked();
    resp.on_hover_text("Go to this page (your reading place stays where it was)");
    clicked.then_some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn finds_across_line_breaks_scripts_and_case() {
        let c = chars("他說：「這是 Tom 的書。」\0下一段，這是\n書包。");
        let q = norm_query("这是");
        let r = find(&c, &q, 10);
        assert_eq!(r.len(), 2, "Simplified finds Traditional");
        assert_eq!(
            c[r[0].start as usize..r[0].end as usize]
                .iter()
                .collect::<String>(),
            "這是"
        );
        // Across a line break, and Latin in any case.
        let r = find(&c, &norm_query("是书"), 10);
        assert_eq!(r.len(), 1);
        assert_eq!(
            c[r[0].start as usize..r[0].end as usize]
                .iter()
                .collect::<String>(),
            "是\n書"
        );
        assert_eq!(find(&c, &norm_query("tom"), 10).len(), 1);
        assert!(find(&c, &norm_query("  "), 10).is_empty());
        assert_eq!(find(&c, &q, 1).len(), 1, "capped");
    }

    #[test]
    fn hits_carry_their_sentence() {
        let c = chars("第一句。他看見了那本書，很高興。\0第三句");
        let r = find(&c, &norm_query("那本书"), 10).remove(0);
        let h = hit(0, &c, r);
        assert_eq!(h.text, "那本書");
        assert_eq!(h.before, "他看見了");
        assert_eq!(h.after, "，很高興。");
    }
}
