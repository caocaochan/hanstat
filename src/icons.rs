//! Interface icons from Phosphor 2.1.2 (MIT; its licence is beside the
//! fonts in `assets/fonts`): two egui families, outline and filled, and the
//! glyphs the app uses. Installed with the app's fonts ([`install`]); where
//! they aren't (some tests), icons are left out rather than drawn as
//! missing-glyph boxes.
//!
//! Phosphor isn't a fallback of the text fonts: it also maps space, `-` and
//! `a`–`z` (for ligatures). So text with icons in it goes through [`rich`],
//! which sets only Phosphor's code points (the Private Use Area) in its font,
//! or an icon stands alone as [`glyph`].

use eframe::egui::{self, Color32, FontDefinitions, FontFamily, FontId, Pos2, RichText};
use std::sync::Arc;

static REGULAR: &[u8] = include_bytes!("../assets/fonts/Phosphor.ttf");
static FILL: &[u8] = include_bytes!("../assets/fonts/Phosphor-Fill.ttf");

const REGULAR_NAME: &str = "phosphor";
const FILL_NAME: &str = "phosphor-fill";

macro_rules! glyphs {
    ($($(#[$m:meta])* $name:ident = $cp:literal, $ph:literal;)*) => {
        $($(#[$m])* pub const $name: char = $cp;)*
        /// Every glyph and its Phosphor name (from `@phosphor-icons/web`
        /// 2.1.2's `regular/style.css`).
        #[cfg(test)]
        const ALL: &[(&str, char)] = &[$(($ph, $name)),*];
    };
}

// Glyphs (the same code points in both fonts).
glyphs! {
    ARROW_CLOCKWISE = '\u{e036}', "arrow-clockwise";
    ARROW_COUNTER_CLOCKWISE = '\u{e038}', "arrow-counter-clockwise";
    ARROW_LEFT = '\u{e058}', "arrow-left";
    ARROW_SQUARE_IN = '\u{e5dc}', "arrow-square-in";
    ARROW_U_UP_LEFT = '\u{e08a}', "arrow-u-up-left";
    BOOK_BOOKMARK = '\u{e0e4}', "book-bookmark";
    BOOK_OPEN = '\u{e0e6}', "book-open";
    BOOKS = '\u{e758}', "books";
    CALENDAR = '\u{e10a}', "calendar-blank";
    CARDS = '\u{e0f8}', "cards";
    CARET_DOWN = '\u{e136}', "caret-down";
    CARET_LEFT = '\u{e138}', "caret-left";
    CARET_RIGHT = '\u{e13a}', "caret-right";
    CARET_UP = '\u{e13c}', "caret-up";
    CHECK = '\u{e182}', "check";
    CHECK_CIRCLE = '\u{e184}', "check-circle";
    COLUMNS = '\u{e546}', "columns";
    CONFETTI = '\u{e81a}', "confetti";
    DOTS_SIX_VERTICAL = '\u{eae2}', "dots-six-vertical";
    DOWNLOAD = '\u{e20c}', "download-simple";
    EXPORT = '\u{eaf0}', "export";
    FIRE = '\u{e242}', "fire";
    FLOPPY_DISK = '\u{e248}', "floppy-disk";
    FOLDER_OPEN = '\u{e256}', "folder-open";
    GEAR = '\u{e270}', "gear";
    HOURGLASS = '\u{e2b2}', "hourglass";
    HOUSE = '\u{e2c2}', "house";
    IMAGE = '\u{e2ca}', "image";
    INFO = '\u{e2ce}', "info";
    LIGHTBULB = '\u{e2dc}', "lightbulb";
    LIST = '\u{e2f0}', "list";
    LIST_CHECKS = '\u{eadc}', "list-checks";
    MAGNIFYING_GLASS = '\u{e30c}', "magnifying-glass";
    PAUSE = '\u{e39e}', "pause";
    PAUSE_CIRCLE = '\u{e3a0}', "pause-circle";
    PENCIL = '\u{e3b4}', "pencil-simple";
    PLAY = '\u{e3d0}', "play";
    SKIP_BACK = '\u{e5a4}', "skip-back";
    SKIP_FORWARD = '\u{e5a6}', "skip-forward";
    STAR = '\u{e46a}', "star";
    STOP = '\u{e46c}', "stop";
    TRASH = '\u{e4a6}', "trash";
    WARNING = '\u{e4e0}', "warning";
    X = '\u{e4f6}', "x";
}

/// Phosphor's code points: the Private Use Area.
fn is_icon(c: char) -> bool {
    ('\u{e000}'..='\u{f8ff}').contains(&c)
}

/// Add the two icon families to `defs` (nothing else uses their fonts).
pub fn install(defs: &mut FontDefinitions) {
    for (name, bytes) in [(REGULAR_NAME, REGULAR), (FILL_NAME, FILL)] {
        defs.font_data.insert(
            name.to_owned(),
            Arc::new(egui::FontData::from_static(bytes)),
        );
        defs.families
            .insert(FontFamily::Name(name.into()), vec![name.to_owned()]);
    }
}

fn family(fill: bool) -> FontFamily {
    FontFamily::Name(if fill { FILL_NAME } else { REGULAR_NAME }.into())
}

fn installed(ctx: &egui::Context) -> bool {
    ctx.fonts(|f| f.definitions().families.contains_key(&family(false)))
}

/// The icon font at `size`, if installed.
pub fn font(ctx: &egui::Context, size: f32, fill: bool) -> Option<FontId> {
    installed(ctx).then(|| FontId::new(size, family(fill)))
}

/// `glyph` centred on `center`.
pub fn paint(
    painter: &egui::Painter,
    center: Pos2,
    glyph: char,
    size: f32,
    fill: bool,
    color: Color32,
) {
    if let Some(font) = font(painter.ctx(), size, fill) {
        painter.text(center, egui::Align2::CENTER_CENTER, glyph, font, color);
    }
}

/// `glyph` on its own, at the text size of wherever it goes (a button, a
/// label…); empty where the fonts aren't installed.
pub fn glyph(ctx: &egui::Context, glyph: char, fill: bool) -> RichText {
    if installed(ctx) {
        RichText::new(glyph).family(family(fill))
    } else {
        RichText::new("")
    }
}

/// `text` (a `&str`, `String` or [`RichText`], with its colour, size,
/// weak, strong…) laid out as `ui` would, with each icon in it ([`GEAR`]…)
/// set in the icon font. For labels, buttons, tooltips and hints alike.
pub fn rich(ui: &egui::Ui, text: impl Into<RichText>) -> egui::text::LayoutJob {
    rich_in(ui.ctx(), ui.style(), text)
}

fn rich_in(
    ctx: &egui::Context,
    style: &egui::Style,
    text: impl Into<RichText>,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    text.into().append_to(
        &mut job,
        style,
        egui::FontSelection::Default,
        egui::Align::Center,
    );
    if !job.text.chars().any(is_icon) {
        return job;
    }
    let icons = installed(ctx);
    let mut out = egui::text::LayoutJob {
        wrap: job.wrap.clone(),
        halign: job.halign,
        justify: job.justify,
        ..Default::default()
    };
    for s in &job.sections {
        let part = &job.text[s.byte_range.start.0..s.byte_range.end.0];
        let mut run = String::new();
        let mut run_icon = false;
        let mut first = true;
        let mut flush = |run: &mut String, icon: bool, first: &mut bool| {
            if run.is_empty() {
                return;
            }
            let mut format = s.format.clone();
            if icon {
                format.font_id.family = family(false);
                format.valign = egui::Align::Center;
            }
            out.append(run, if *first { s.leading_space } else { 0.0 }, format);
            *first = false;
            run.clear();
        };
        for c in part.chars() {
            let icon = is_icon(c);
            if icon && !icons {
                continue;
            }
            if icon != run_icon {
                flush(&mut run, run_icon, &mut first);
                run_icon = icon;
            }
            run.push(c);
        }
        flush(&mut run, run_icon, &mut first);
    }
    out
}

/// `text` in `font` with its icons in the icon font (`fill`: the filled
/// one), for a painter. The fonts must be installed.
pub fn job(text: &str, font: FontId, color: Color32, fill: bool) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let mut run = String::new();
    let mut run_icon = false;
    let mut flush = |run: &mut String, icon: bool| {
        if run.is_empty() {
            return;
        }
        let mut f = font.clone();
        if icon {
            f.family = family(fill);
        }
        job.append(
            run,
            0.0,
            egui::TextFormat {
                font_id: f,
                color,
                valign: egui::Align::Center,
                ..Default::default()
            },
        );
        run.clear();
    };
    for c in text.chars() {
        if is_icon(c) != run_icon {
            flush(&mut run, run_icon);
            run_icon = is_icon(c);
        }
        run.push(c);
    }
    flush(&mut run, run_icon);
    job
}

/// `glyph` as text for a label or button (before `text`, if any), at `size`.
pub fn text(ctx: &egui::Context, glyph: char, text: &str, size: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let body = egui::TextFormat {
        font_id: FontId::proportional(size),
        color: Color32::PLACEHOLDER,
        valign: egui::Align::Center,
        ..Default::default()
    };
    if let Some(font) = font(ctx, size * 1.1, false) {
        job.append(
            &glyph.to_string(),
            0.0,
            egui::TextFormat {
                font_id: font,
                color: Color32::PLACEHOLDER,
                valign: egui::Align::Center,
                ..Default::default()
            },
        );
        if !text.is_empty() {
            job.append(text, size * 0.35, body);
        }
    } else {
        job.append(text, 0.0, body);
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(with_icons: bool) -> egui::Context {
        let ctx = egui::Context::default();
        let mut defs = FontDefinitions::default();
        if with_icons {
            install(&mut defs);
        }
        ctx.set_fonts(defs);
        let mut out = ctx.run_ui(Default::default(), |_| {});
        out.textures_delta.clear();
        ctx
    }

    #[test]
    fn every_icon_is_in_both_fonts() {
        let ctx = ctx(true);
        for &(name, c) in ALL {
            assert!(is_icon(c), "{name}");
            for fill in [false, true] {
                let f = FontId::new(14.0, family(fill));
                assert!(
                    ctx.fonts_mut(|x| x.has_glyph(&f, c)),
                    "{name} (fill: {fill})"
                );
            }
        }
        // The text fonts don't draw them: they need `rich`.
        let body = FontId::proportional(14.0);
        assert!(!ctx.fonts_mut(|x| x.has_glyph(&body, GEAR)));
    }

    #[test]
    fn rich_sets_only_icons_in_the_icon_font() {
        let ctx = ctx(true);
        let style = ctx.global_style();
        let job = rich_in(
            &ctx,
            &style,
            RichText::new(format!("{GEAR} Settings {X}")).weak(),
        );
        let parts: Vec<(&str, bool)> = job
            .sections
            .iter()
            .map(|s| {
                (
                    &job.text[s.byte_range.start.0..s.byte_range.end.0],
                    s.format.font_id.family == family(false),
                )
            })
            .collect();
        assert_eq!(
            parts,
            [
                ("\u{e270}", true),
                (" Settings ", false),
                ("\u{e4f6}", true)
            ]
        );
        let weak = style.visuals.weak_text_color();
        assert!(job.sections.iter().all(|s| s.format.color == weak));
        // Without the icon fonts: the icons are left out.
        let ctx = self::ctx(false);
        let job = rich_in(&ctx, &ctx.global_style(), format!("{GEAR} Settings"));
        assert_eq!(job.text, " Settings");
        assert!(
            job.sections
                .iter()
                .all(|s| s.format.font_id.family == FontFamily::Proportional)
        );
    }
}
