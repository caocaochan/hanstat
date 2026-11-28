//! "What's new": on the first start of a newer version, a window with the
//! release notes of every version since the one used last, taken from
//! `CHANGELOG.md` (embedded at build time, so the release zip, the macOS
//! bundle and the Flatpak have it). The version last started is kept in
//! `update.json` ([`hanstat::update::Settings::last_version`]).

use eframe::egui::{self, RichText, text::LayoutJob};
use hanstat::update::{Version, format_version, parse_version};

pub const CHANGELOG: &str = include_str!("../CHANGELOG.md");

/// One version's notes: `## [2026.11.17.15] - 2026-11-17` and what follows
/// up to the next `## ` heading.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Section<'a> {
    pub(crate) version: Version,
    /// The heading after the version, e.g. `2026-11-17` (may be empty).
    pub(crate) date: &'a str,
    pub(crate) body: &'a str,
}

/// The changelog's versions, in its order. Headings whose version doesn't
/// parse (an `[Unreleased]` section) are skipped with their notes.
fn sections(text: &str) -> Vec<Section<'_>> {
    let mut out = Vec::new();
    // (version and date, start of its body) of the heading being read.
    let mut open: Option<(Option<(Version, &str)>, usize)> = None;
    let mut pos = 0;
    for line in text.split_inclusive('\n') {
        let start = pos;
        pos += line.len();
        let Some(head) = line.trim_end().strip_prefix("## ") else {
            continue;
        };
        if let Some((Some((version, date)), from)) = open.take() {
            out.push(Section {
                version,
                date,
                body: text[from..start].trim_matches(['\r', '\n']),
            });
        }
        open = Some((heading(head), pos));
    }
    if let Some((Some((version, date)), from)) = open {
        out.push(Section {
            version,
            date,
            body: text[from..].trim_matches(['\r', '\n']),
        });
    }
    out
}

/// `[2026.11.17.15] - 2026-11-17` → its version and `2026-11-17`.
fn heading(h: &str) -> Option<(Version, &str)> {
    let (v, rest) = h.strip_prefix('[')?.split_once(']')?;
    let date = rest.trim().trim_start_matches(['-', '–', '—']).trim();
    Some((parse_version(v.trim())?, date))
}

/// The sections of versions after `after` up to and including `upto`, newest first.
fn between<'a>(secs: &[Section<'a>], after: Version, upto: Version) -> Vec<Section<'a>> {
    let mut out: Vec<Section> = secs
        .iter()
        .filter(|s| after < s.version && s.version <= upto)
        .cloned()
        .collect();
    // Stable: two sections of one version keep the changelog's order.
    out.sort_by_key(|s| std::cmp::Reverse(s.version));
    out
}

/// At startup, from the version recorded last time: the notes to show, and
/// whether to record `current` instead. Nothing recorded (a fresh install,
/// or a version from before this window) or unreadable: nothing to show,
/// record. The same version or a downgrade: nothing to show, and the newer
/// version stays recorded so its notes aren't shown again on returning to it.
pub fn at_start<'a>(
    text: &'a str,
    recorded: Option<&str>,
    current: Version,
) -> (Vec<Section<'a>>, bool) {
    match recorded.and_then(parse_version) {
        None => (Vec::new(), true),
        Some(last) if last >= current => (Vec::new(), false),
        Some(last) => (between(&sections(text), last, current), true),
    }
}

/// Pre-parsed section block so the window does not re-parse all 885+ lines of
/// `CHANGELOG.md` on every frame.
enum ParsedBlock {
    Heading(String),
    Bullet {
        level: usize,
        lead: Option<&'static str>,
        spans: Vec<(Span, &'static str)>,
    },
    Para {
        level: usize,
        spans: Vec<(Span, &'static str)>,
    },
}

struct ParsedSection {
    #[cfg(test)]
    version: Version,
    version_text: String,
    date: &'static str,
    blocks: Vec<ParsedBlock>,
}

impl ParsedSection {
    fn parse(s: Section<'static>) -> Self {
        let blocks = blocks(s.body)
            .into_iter()
            .map(|b| match b {
                Block::Heading(h) => ParsedBlock::Heading(h.to_uppercase()),
                Block::Bullet { level, text } => {
                    if level == 0
                        && let Some((title, rest)) = lead(text)
                    {
                        ParsedBlock::Bullet {
                            level,
                            lead: Some(title),
                            spans: if rest.is_empty() {
                                Vec::new()
                            } else {
                                spans(rest)
                            },
                        }
                    } else {
                        ParsedBlock::Bullet {
                            level,
                            lead: None,
                            spans: spans(text),
                        }
                    }
                }
                Block::Para { level, text } => ParsedBlock::Para {
                    level,
                    spans: spans(text),
                },
            })
            .collect();
        ParsedSection {
            #[cfg(test)]
            version: s.version,
            version_text: format_version(s.version),
            date: s.date,
            blocks,
        }
    }
}

/// The window's state while it is open.
pub struct WhatsNew {
    /// The version the notes start after; `None` for the whole changelog
    /// (the version menu's **Changelog**).
    pub since: Option<Version>,
    parsed: Vec<ParsedSection>,
    /// Last measured height of each section at `measured_width` so off-screen
    /// sections are skipped during scroll without shaping their text.
    heights: Vec<f32>,
    measured_width: f32,
}

impl WhatsNew {
    pub(crate) fn new(since: Option<Version>, sections: Vec<Section<'static>>) -> Self {
        let parsed: Vec<ParsedSection> = sections.into_iter().map(ParsedSection::parse).collect();
        let len = parsed.len();
        WhatsNew {
            since,
            parsed,
            heights: vec![0.0; len],
            measured_width: 0.0,
        }
    }

    /// Every release up to this one, newest first.
    pub fn all() -> Self {
        let current = hanstat::update::current_version();
        Self::new(None, between(&sections(CHANGELOG), (0, 0, 0, 0), current))
    }
}

/// One line of a section's notes, as the changelog writes it.
#[derive(Debug, PartialEq)]
enum Block<'a> {
    /// `### Changed`.
    Heading(&'a str),
    /// `- text`, indented by `level` (two spaces each).
    Bullet { level: usize, text: &'a str },
    /// A paragraph, indented by `level`: under a bullet, aligned with its text.
    Para { level: usize, text: &'a str },
}

fn blocks(body: &str) -> Vec<Block<'_>> {
    body.lines()
        .filter_map(|line| {
            let t = line.trim_start();
            let level = (line.len() - t.len()) / 2;
            let t = t.trim_end();
            if t.is_empty() {
                None
            } else if let Some(h) = t.strip_prefix('#') {
                Some(Block::Heading(h.trim_start_matches('#').trim()))
            } else if let Some(b) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
                Some(Block::Bullet { level, text: b })
            } else {
                Some(Block::Para { level, text: t })
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Span {
    Plain,
    Bold,
    Code,
}

/// Inline Markdown: `**bold**`, `` `code` `` and `[text](url)` (shown as its
/// text). An unclosed marker is plain text.
fn spans(text: &str) -> Vec<(Span, &str)> {
    let mut out = Vec::new();
    let mut rest = text;
    let mut plain = 0; // bytes of `rest` that are plain text so far
    while plain < rest.len() {
        let here = &rest[plain..];
        let found = if let Some(r) = here.strip_prefix("**") {
            r.find("**").map(|e| (Span::Bold, &r[..e], 2 + e + 2))
        } else if let Some(r) = here.strip_prefix('`') {
            r.find('`').map(|e| (Span::Code, &r[..e], 1 + e + 1))
        } else if let Some(r) = here.strip_prefix('[') {
            r.find("](").and_then(|e| {
                let close = r[e + 2..].find(')')?;
                Some((Span::Plain, &r[..e], 1 + e + 2 + close + 1))
            })
        } else {
            None
        };
        match found {
            Some((kind, inner, len)) => {
                if plain > 0 {
                    out.push((Span::Plain, &rest[..plain]));
                }
                if !inner.is_empty() {
                    out.push((kind, inner));
                }
                rest = &here[len..];
                plain = 0;
            }
            None => plain += here.chars().next().map_or(1, char::len_utf8),
        }
    }
    if !rest.is_empty() {
        out.push((Span::Plain, rest));
    }
    out
}

/// A bullet that opens with a bold sentence, `**Idioms count as one word.**
/// The segmenters…`: that sentence (without its full stop) as the entry's
/// title, and the rest. A bold phrase that runs on into the sentence
/// (`**Suggested words** lists…`) is not a title.
fn lead(text: &str) -> Option<(&str, &str)> {
    let r = text.strip_prefix("**")?;
    let end = r.find("**")?;
    let title = &r[..end];
    let title = title
        .strip_suffix('.')
        .or_else(|| title.strip_suffix(':'))
        .or_else(|| title.strip_suffix('!'))?;
    Some((title.trim(), r[end + 2..].trim_start()))
}

/// Body text with more room between lines than egui's default, which is
/// cramped for paragraphs this long.
fn inline_job(ui: &egui::Ui, parts: &[(Span, &str)]) -> LayoutJob {
    let style = ui.style();
    let body = egui::TextStyle::Body.resolve(style);
    let mono = egui::FontId::monospace(body.size * 0.92);
    let line = Some(body.size * 1.5);
    let v = ui.visuals();
    let mut job = LayoutJob::default();
    for &(kind, s) in parts {
        let format = match kind {
            Span::Plain => egui::TextFormat::simple(body.clone(), v.text_color()),
            Span::Bold => egui::TextFormat::simple(body.clone(), v.strong_text_color()),
            // No background: with the taller lines it fills the whole line
            // height and reads as a block; the monospace face is enough.
            Span::Code => egui::TextFormat::simple(mono.clone(), v.text_color()),
        };
        job.append(
            s,
            0.0,
            egui::TextFormat {
                line_height: line,
                valign: egui::Align::Center,
                ..format
            },
        );
    }
    job
}

/// A section's notes: group headings (Added, Changed, Fixed), entries with
/// their bold lead as a title, nested bullets and paragraphs.
fn notes_ui(ui: &mut egui::Ui, blocks: &[ParsedBlock]) {
    const INDENT: f32 = 18.0;
    let size = egui::TextStyle::Body.resolve(ui.style()).size;
    let wrapped = |ui: &mut egui::Ui, job: LayoutJob| {
        ui.add(egui::Label::new(job).wrap());
    };
    for b in blocks {
        match b {
            ParsedBlock::Heading(h) => {
                ui.add_space(12.0);
                ui.label(
                    RichText::new(h.as_str())
                        .size(size * 0.85)
                        .extra_letter_spacing(1.5)
                        .weak(),
                );
                ui.add_space(2.0);
            }
            ParsedBlock::Bullet {
                level,
                lead: title,
                spans,
            } => {
                let level = *level;
                ui.add_space(if level == 0 { 8.0 } else { 3.0 });
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.add_space(level as f32 * INDENT);
                    let marker = if level == 0 { "•" } else { "◦" };
                    let first = if title.is_some() {
                        size * 1.12 * 1.3
                    } else {
                        size * 1.5
                    };
                    ui.add_sized(
                        [INDENT, first],
                        egui::Label::new(RichText::new(marker).weak()),
                    );
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        if let Some(title) = title {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(*title)
                                        .size(size * 1.12)
                                        .color(ui.visuals().strong_text_color()),
                                )
                                .wrap(),
                            );
                        }
                        if !spans.is_empty() {
                            wrapped(ui, inline_job(ui, spans));
                        }
                    });
                });
            }
            ParsedBlock::Para { level, spans } => {
                ui.add_space(6.0);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.add_space(*level as f32 * INDENT);
                    wrapped(ui, inline_job(ui, spans));
                });
            }
        }
    }
}

/// The window's id, shared with its layer (Esc closes the window in front).
pub const WINDOW_ID: &str = "whats-new-window";

/// Draw the window; `false` once it is closed.
pub fn window(ctx: &egui::Context, state: &mut WhatsNew) -> bool {
    let mut open = true;
    let mut close = false;
    let bounds = crate::palette::popup_bounds(ctx);
    let max_inner = crate::palette::popup_max_inner(ctx, bounds);
    // About 80 Latin characters a line: wider is harder to read.
    let default_w =
        (max_inner.x - 16.0).clamp(240.0_f32.min(max_inner.x), 580.0_f32.min(max_inner.x));
    let default_h = (max_inner.y * 0.78).clamp(180.0_f32.min(max_inner.y), max_inner.y);
    let version = hanstat::update::version_string();
    let title = match state.since {
        Some(_) => format!("What's new in hanstat {version}"),
        None => "hanstat changelog".to_owned(),
    };
    crate::palette::popup_window(ctx, title)
        .id(egui::Id::unique(WINDOW_ID))
        .open(&mut open)
        .collapsible(false)
        .default_size(egui::vec2(default_w, default_h))
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(bounds.center())
        .show(ctx, |ui| {
            let intro = match state.since {
                Some(v) => format!(
                    "Changes since {}, the version you used last.",
                    format_version(v)
                ),
                None => format!("Every release up to {version}, newest first."),
            };
            ui.label(RichText::new(intro).weak());
            ui.separator();
            let list_h = (ui.available_height() - 40.0).max(120.0);
            egui::ScrollArea::vertical()
                // Each list starts at its top.
                .id_salt(("whats-new", state.since))
                .max_height(list_h)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let avail_w = ui.available_width();
                    if (state.measured_width - avail_w).abs() > 1.0 {
                        state.heights.fill(0.0);
                        state.measured_width = avail_w;
                    }
                    let clip = ui.clip_rect();
                    for (i, s) in state.parsed.iter().enumerate() {
                        let y = ui.cursor().top();
                        let known_h = state.heights[i];
                        if known_h > 0.0
                            && (y + known_h < clip.top() - 40.0 || y > clip.bottom() + 40.0)
                        {
                            ui.allocate_space(egui::vec2(ui.available_width(), known_h));
                            continue;
                        }
                        let start_y = ui.cursor().top();
                        ui.vertical(|ui| {
                            if i > 0 {
                                ui.add_space(14.0);
                                ui.separator();
                                ui.add_space(6.0);
                            }
                            ui.horizontal(|ui| {
                                ui.heading(&s.version_text);
                                if !s.date.is_empty() {
                                    ui.label(RichText::new(s.date).weak());
                                }
                            });
                            notes_ui(ui, &s.blocks);
                        });
                        let h = (ui.cursor().top() - start_y).max(1.0);
                        state.heights[i] = h;
                    }
                });
            ui.separator();
            ui.horizontal(|ui| {
                close = ui.button("Close").clicked();
                if state.since.is_some() {
                    ui.label(
                        RichText::new(
                            "Every release: click the version at the bottom right → Changelog.",
                        )
                        .weak()
                        .small(),
                    );
                }
            });
        });
    open && !close
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "# Changelog\r\n\
        \r\n\
        ## [Unreleased]\r\n\
        - Not yet.\r\n\
        \r\n\
        ## [2026.11.18.00] - 2026-11-18\r\n\
        \r\n\
        ### Added\r\n\
        - **New.** Thing.\r\n\
        \r\n\
        ## [2026.11.17.10] - 2026-11-17\r\n\
        \r\n\
        ### Fixed\r\n\
        - Ten.\r\n\
        \r\n\
        ## [2026.11.17.9] - 2026-11-17\r\n\
        \r\n\
        ### Fixed\r\n\
        - Nine.\r\n\
        \r\n\
        ## [0.7.1] - 2026-11-15\r\n\
        - Old.\r\n";

    #[test]
    fn parses_sections() {
        let s = sections(LOG);
        let versions: Vec<Version> = s.iter().map(|s| s.version).collect();
        assert_eq!(
            versions,
            [
                (2026, 11, 18, 0),
                (2026, 11, 17, 10),
                (2026, 11, 17, 9),
                (0, 7, 1, 0)
            ]
        );
        assert_eq!(s[0].date, "2026-11-18");
        assert_eq!(s[0].body, "### Added\r\n- **New.** Thing.");
        assert_eq!(s[3].body, "- Old.");
        assert!(!s.iter().any(|s| s.body.contains("Not yet")));
    }

    #[test]
    fn selects_between_versions() {
        let s = sections(LOG);
        let v = |s: &[Section]| s.iter().map(|s| s.version).collect::<Vec<_>>();
        // .9 → .18.00: both newer ones, newest first; .9 itself was seen.
        assert_eq!(
            v(&between(&s, (2026, 11, 17, 9), (2026, 11, 18, 0))),
            [(2026, 11, 18, 0), (2026, 11, 17, 10)]
        );
        // Up to the running version, not past it.
        assert_eq!(
            v(&between(&s, (0, 7, 1, 0), (2026, 11, 17, 10))),
            [(2026, 11, 17, 10), (2026, 11, 17, 9)]
        );
        assert!(between(&s, (2026, 11, 18, 0), (2026, 11, 18, 0)).is_empty());
        // Out of order in the file: still newest first.
        let mut rev = s.clone();
        rev.reverse();
        assert_eq!(
            v(&between(&rev, (0, 0, 0, 0), (2026, 11, 17, 10))),
            [(2026, 11, 17, 10), (2026, 11, 17, 9), (0, 7, 1, 0)]
        );
    }

    #[test]
    fn decides_at_start() {
        let cur = (2026, 11, 18, 0);
        // Fresh install, or unreadable: record, show nothing.
        assert_eq!(at_start(LOG, None, cur), (Vec::new(), true));
        assert_eq!(at_start(LOG, Some("garbage"), cur), (Vec::new(), true));
        // Same version, downgrade: nothing, keep the newer one recorded.
        assert_eq!(
            at_start(LOG, Some("2026.11.18.00"), cur),
            (Vec::new(), false)
        );
        assert_eq!(
            at_start(LOG, Some("2026.11.19.00"), cur),
            (Vec::new(), false)
        );
        // Upgrade.
        let (show, record) = at_start(LOG, Some("2026.11.17.9"), cur);
        assert!(record);
        assert_eq!(show.len(), 2);
        assert_eq!(show[0].version, cur);
    }

    #[test]
    fn whole_changelog() {
        let all = WhatsNew::all();
        assert!(all.parsed.len() > 30);
        assert!(all.parsed.windows(2).all(|w| w[0].version > w[1].version));
    }

    #[test]
    fn real_changelog_parses() {
        let s = sections(CHANGELOG);
        assert!(s.len() > 30);
        // Newest first, each version once: a mistyped heading shows up here.
        assert!(s.windows(2).all(|w| w[0].version > w[1].version));
        assert!(
            s.iter()
                .all(|s| !s.body.is_empty() && !s.body.contains("\n## "))
        );
    }

    #[test]
    fn splits_blocks() {
        let body = "### Changed\n- **A.** text\n  - nested `x`\n\n  More about A.\nPlain.";
        assert_eq!(
            blocks(body),
            [
                Block::Heading("Changed"),
                Block::Bullet {
                    level: 0,
                    text: "**A.** text"
                },
                Block::Bullet {
                    level: 1,
                    text: "nested `x`"
                },
                Block::Para {
                    level: 1,
                    text: "More about A."
                },
                Block::Para {
                    level: 0,
                    text: "Plain."
                },
            ]
        );
    }

    #[test]
    fn entry_titles() {
        assert_eq!(
            lead("**Idioms count as one word.** The segmenters cut"),
            Some(("Idioms count as one word", "The segmenters cut"))
        );
        assert_eq!(
            lead("**Undo removing a book.**"),
            Some(("Undo removing a book", ""))
        );
        // Bold that runs on into the sentence, or doesn't open it.
        assert_eq!(lead("**Suggested words** lists names"), None);
        assert_eq!(lead("It used to **also** do so."), None);
    }

    #[test]
    fn inline_markdown() {
        assert_eq!(
            spans("**Bold** and `code`, a [link](https://x/y) here."),
            [
                (Span::Bold, "Bold"),
                (Span::Plain, " and "),
                (Span::Code, "code"),
                (Span::Plain, ", a "),
                (Span::Plain, "link"),
                (Span::Plain, " here."),
            ]
        );
        // Unclosed markers stay as written; CJK text is cut on char boundaries.
        assert_eq!(
            spans("2 ** 3 `x 若有所思"),
            [(Span::Plain, "2 ** 3 `x 若有所思")]
        );
        assert_eq!(
            spans("若**有所**思"),
            [
                (Span::Plain, "若"),
                (Span::Bold, "有所"),
                (Span::Plain, "思")
            ]
        );
        assert_eq!(spans(""), []);
    }
}
