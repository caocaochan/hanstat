//! Interface icons from Phosphor 2.1.2 (MIT; its licence is beside the
//! fonts in `assets/fonts`): two egui families, outline and filled, with the
//! glyphs the reader uses. Installed with the app's fonts
//! ([`install`]); where they aren't (some tests), icons are drawn as
//! nothing rather than as missing-glyph boxes.

use eframe::egui::{self, Color32, FontDefinitions, FontFamily, FontId, Pos2};
use std::sync::Arc;

static REGULAR: &[u8] = include_bytes!("../../assets/fonts/Phosphor.ttf");
static FILL: &[u8] = include_bytes!("../../assets/fonts/Phosphor-Fill.ttf");

const REGULAR_NAME: &str = "phosphor";
const FILL_NAME: &str = "phosphor-fill";

/// Glyphs (the same code points in both fonts).
pub const SKIP_BACK: char = '\u{e5a4}';
pub const PLAY: char = '\u{e3d0}';
pub const PAUSE: char = '\u{e39e}';
pub const STOP: char = '\u{e46c}';
pub const SKIP_FORWARD: char = '\u{e5a6}';
pub const CHECK_CIRCLE: char = '\u{e184}';
pub const MAGNIFYING_GLASS: char = '\u{e30c}';
pub const X: char = '\u{e4f6}';
pub const ARROW_SQUARE_IN: char = '\u{e5dc}';
pub const ARROW_LEFT: char = '\u{e058}';

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

/// The icon font at `size`, if installed.
pub fn font(ctx: &egui::Context, size: f32, fill: bool) -> Option<FontId> {
    let fam = family(fill);
    ctx.fonts(|f| f.definitions().families.contains_key(&fam))
        .then(|| FontId::new(size, fam))
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

/// `glyph` as text for a label or button (before `text`, if any).
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
