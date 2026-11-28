//! Shareable quote-card PNG export for selected text in the reader.
//!
//! Renders a bookplate-inspired literary card in the current reader theme's
//! colours across Square (`1024×1024`), Horizontal (`1024×640`) and Vertical
//! (`640×1024`) layouts. The same painter draws both the live preview in the
//! export window and the offscreen PNG via [`crate::raster`].

use super::fonts;
use super::layout::{self, VertGlyph};
use super::theme::{self, Theme};
use eframe::egui::{
    self, Align, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Pos2, Rect,
    RichText, Sense, Stroke, StrokeKind, Vec2,
    epaint::{Mesh, PathShape, Shape, TextShape, Vertex},
    pos2,
    text::LayoutJob,
    vec2,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Output longest edge in pixels.
pub(super) const LONG_EDGE_PX: usize = 1024;
/// Offscreen raster scale (points → pixels).
const SCALE: f32 = 2.0;
/// Logical longest edge in points (`512.0 * 2.0 == 1024`).
const LONG_EDGE_PT: f32 = LONG_EDGE_PX as f32 / SCALE;
/// Logical short edge for landscape/portrait (`320.0 * 2.0 == 640`, 16:10 / 10:16).
const SHORT_EDGE_PT: f32 = 320.0;

/// Bundled WenKai family used for book titles (and fallback for quotes).
const TITLE_FONT_NAME: &str = "quote-title";
const WENKAI_DATA_KEY: &str = "quote-wenkai";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum QuoteLayout {
    #[default]
    Square,
    Horizontal,
    Vertical,
}

impl QuoteLayout {
    pub(super) const ALL: [QuoteLayout; 3] = [
        QuoteLayout::Square,
        QuoteLayout::Horizontal,
        QuoteLayout::Vertical,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            QuoteLayout::Square => "Square",
            QuoteLayout::Horizontal => "Horizontal",
            QuoteLayout::Vertical => "Vertical",
        }
    }

    /// Logical canvas size in points (rendered at [`SCALE`] to pixels).
    pub(super) fn logical_size(self) -> Vec2 {
        match self {
            QuoteLayout::Square => vec2(LONG_EDGE_PT, LONG_EDGE_PT),
            QuoteLayout::Horizontal => vec2(LONG_EDGE_PT, SHORT_EDGE_PT),
            QuoteLayout::Vertical => vec2(SHORT_EDGE_PT, LONG_EDGE_PT),
        }
    }

    /// Exact PNG dimensions in pixels (longest edge is always [`LONG_EDGE_PX`]).
    pub(super) fn pixel_size(self) -> [usize; 2] {
        let s = self.logical_size();
        [
            (s.x * SCALE).round() as usize,
            (s.y * SCALE).round() as usize,
        ]
    }
}

/// Text flow direction inside a quote card.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum TextDirection {
    #[default]
    Horizontal,
    Vertical,
}

impl TextDirection {
    pub(super) const ALL: [TextDirection; 2] = [TextDirection::Horizontal, TextDirection::Vertical];

    pub(super) fn label(self) -> &'static str {
        match self {
            TextDirection::Horizontal => "Horizontal",
            TextDirection::Vertical => "Vertical",
        }
    }

    pub(super) fn from_vertical(vertical: bool) -> Self {
        if vertical {
            TextDirection::Vertical
        } else {
            TextDirection::Horizontal
        }
    }
}

/// Content and layout of a quote card.
#[derive(Clone, Debug)]
pub(super) struct QuoteCard {
    pub(super) quote: String,
    pub(super) title: String,
    pub(super) author: String,
    pub(super) date: String,
    pub(super) layout: QuoteLayout,
    pub(super) direction: TextDirection,
}

/// Sentinel value for [`QuoteDialog::title_font`] meaning "match the quote font".
const TITLE_SAME_AS_QUOTE: &str = "";

#[derive(Clone, PartialEq, Eq)]
struct PreviewKey {
    quote: String,
    title: String,
    author: String,
    date: String,
    layout: QuoteLayout,
    direction: TextDirection,
    theme_id: &'static str,
    quote_font: Option<String>,
    title_font: Option<String>,
    reader_fonts_key: Option<(String, String)>,
    sans_default: bool,
    raster_scale_bits: u32,
    fonts_scanned: bool,
}

/// State of the "Export quote" modal window.
pub(super) struct QuoteDialog {
    pub(super) card: QuoteCard,
    /// Chosen card theme id; `None` follows the reader's current theme.
    pub(super) theme_id: Option<String>,
    /// Chosen quote text font family name; `None` follows the reader's font.
    pub(super) quote_font: Option<String>,
    /// Chosen book title font family name; `None` uses bundled LXGW WenKai,
    /// `Some("")` matches the quote font.
    pub(super) title_font: Option<String>,
    pub(super) error: Option<String>,
    preview: Option<(PreviewKey, egui::TextureHandle)>,
}

impl QuoteDialog {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        quote: String,
        title: String,
        author: String,
        layout: QuoteLayout,
        direction: TextDirection,
        theme_id: Option<String>,
        quote_font: Option<String>,
        title_font: Option<String>,
    ) -> Self {
        QuoteDialog {
            card: QuoteCard {
                quote: clean_quote(&quote),
                title: title.trim().to_owned(),
                author: author.trim().to_owned(),
                date: today_string(),
                layout,
                direction,
            },
            theme_id,
            quote_font,
            title_font,
            error: None,
            preview: None,
        }
    }

    /// The theme used to render and export the quote card: the user's chosen
    /// theme if set (and known), otherwise the reader's current theme.
    pub(super) fn resolved_theme<'a>(&self, reader_th: &'a Theme) -> &'a Theme {
        self.theme_id
            .as_deref()
            .and_then(|id| theme::THEMES.iter().find(|t| t.id == id))
            .unwrap_or(reader_th)
    }

    /// Draw the export dialog. Returns `(keep_open, save_clicked)`.
    pub(super) fn ui(
        &mut self,
        ctx: &egui::Context,
        reader_th: &Theme,
        reader_fonts: Option<&fonts::Installed>,
        sans_default: bool,
    ) -> (bool, bool) {
        let fams = fonts::families();
        let mut open = true;
        let mut save = false;
        let mut close_clicked = false;
        let bounds = crate::palette::popup_bounds(ctx);
        let max_inner = crate::palette::popup_max_inner(ctx, bounds);
        crate::palette::popup_window(ctx, "Export quote")
            .id(egui::Id::unique("reader-quote-export"))
            .open(&mut open)
            .collapsible(false)
            .default_width(470.0_f32.min(max_inner.x))
            .default_height((max_inner.y * 0.88).min(640.0))
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(bounds.center())
            .show(ctx, |ui| {
                // Layout switcher.
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Layout").color(reader_th.subtle));
                    ui.add_space(6.0);
                    for layout in QuoteLayout::ALL {
                        let [pw, ph] = layout.pixel_size();
                        ui.selectable_value(&mut self.card.layout, layout, layout.label())
                            .on_hover_text(format!("{pw} × {ph} px"));
                    }
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        let [pw, ph] = self.card.layout.pixel_size();
                        ui.label(
                            RichText::new(format!("{pw} × {ph} px"))
                                .size(12.0)
                                .color(reader_th.muted),
                        );
                    });
                });
                ui.add_space(6.0);

                // Text direction switcher.
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Text").color(reader_th.subtle));
                    ui.add_space(6.0);
                    for dir in TextDirection::ALL {
                        let tip = match dir {
                            TextDirection::Horizontal => "Left to right, top to bottom",
                            TextDirection::Vertical => {
                                "Traditional vertical: top to bottom, right to left"
                            }
                        };
                        ui.selectable_value(&mut self.card.direction, dir, dir.label())
                            .on_hover_text(tip);
                    }
                });
                ui.add_space(6.0);

                // Card theme switcher (independent from the reader's theme).
                let current_override = self
                    .theme_id
                    .as_deref()
                    .and_then(|id| theme::THEMES.iter().position(|t| t.id == id));
                let shown_th = current_override.map_or(reader_th, |i| &theme::THEMES[i]);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Theme").color(reader_th.subtle));
                    ui.add_space(6.0);
                    let (swatch_rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    let sp = ui.painter();
                    sp.rect_filled(swatch_rect, 3.5, shown_th.page);
                    sp.rect_stroke(
                        swatch_rect,
                        3.5,
                        Stroke::new(1.0, theme::mix(shown_th.ink, shown_th.page, 0.25)),
                        StrokeKind::Inside,
                    );
                    sp.circle_filled(swatch_rect.center(), 3.2, shown_th.accent);

                    let btn_label =
                        current_override.map_or("Same as pages", |i| theme::THEMES[i].name);
                    let button = ui
                        .add(
                            egui::Button::new(btn_label)
                                .right_text(crate::icons::glyph(
                                    ui.ctx(),
                                    crate::icons::CARET_DOWN,
                                    false,
                                ))
                                .min_size(vec2(200.0, 0.0)),
                        )
                        .on_hover_text(
                            "Theme used for the exported PNG (independent of the reader theme)",
                        );
                    egui::Popup::menu(&button)
                        .id(egui::Id::unique("reader-quote-theme-list"))
                        .show(|ui| {
                            let max = crate::palette::dropdown_max_size(ui.ctx(), 72.0);
                            let col_w = super::THEME_COLUMN;
                            let w = 2.0 * col_w + ui.spacing().item_spacing.x;
                            crate::palette::resizable_scroll_popup(
                                ui,
                                "reader-quote-theme-list-box",
                                vec2(w + 12.0, 490.0),
                                vec2(220.0, 140.0),
                                max,
                                |ui| {
                                    let same = ui
                                        .allocate_ui(vec2(w, 30.0), |ui| {
                                            theme::swatch_as(
                                                ui,
                                                reader_th,
                                                "Same as pages",
                                                current_override.is_none(),
                                            )
                                        })
                                        .inner;
                                    if same.clicked() {
                                        self.theme_id = None;
                                        ui.close();
                                    }
                                    ui.horizontal_top(|ui| {
                                        for dark in [false, true] {
                                            ui.vertical(|ui| {
                                                ui.set_width(col_w);
                                                ui.label(
                                                    RichText::new(if dark {
                                                        "Dark"
                                                    } else {
                                                        "Light"
                                                    })
                                                    .size(12.0)
                                                    .color(reader_th.muted),
                                                );
                                                for (i, t) in theme::THEMES
                                                    .iter()
                                                    .enumerate()
                                                    .filter(|(_, t)| t.dark == dark)
                                                {
                                                    let r = theme::swatch(
                                                        ui,
                                                        t,
                                                        Some(i) == current_override,
                                                    );
                                                    if r.clicked() {
                                                        self.theme_id = Some(t.id.to_owned());
                                                        ui.close();
                                                    }
                                                }
                                            });
                                        }
                                    });
                                },
                            );
                        });
                });
                ui.add_space(6.0);

                // Independent quote & title font switchers.
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Quote").color(reader_th.subtle))
                        .on_hover_text("Font used for the quote body text");
                    let quote_label = self.quote_font.as_deref().unwrap_or("Same as reader");
                    egui::ComboBox::from_id_salt("reader-quote-body-font")
                        .selected_text(quote_label)
                        .width(160.0)
                        .height(360.0)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(self.quote_font.is_none(), "Same as reader")
                                .clicked()
                            {
                                self.quote_font = None;
                            }
                            ui.separator();
                            match &fams {
                                None => {
                                    ui.label("Looking for fonts…");
                                }
                                Some(list) => {
                                    for fam in list.iter() {
                                        let sel = self
                                            .quote_font
                                            .as_deref()
                                            .is_some_and(|n| n.eq_ignore_ascii_case(&fam.name));
                                        if ui.selectable_label(sel, fam.label()).clicked() {
                                            self.quote_font = Some(fam.name.clone());
                                        }
                                    }
                                }
                            }
                        });

                    ui.add_space(8.0);
                    ui.label(RichText::new("Title").color(reader_th.subtle))
                        .on_hover_text("Font used for the book title in the footer");
                    let title_label = match self.title_font.as_deref() {
                        None => fonts::BUNDLED,
                        Some(TITLE_SAME_AS_QUOTE) => "Same as quote",
                        Some(name) => name,
                    };
                    egui::ComboBox::from_id_salt("reader-quote-title-font")
                        .selected_text(title_label)
                        .width(160.0)
                        .height(360.0)
                        .show_ui(ui, |ui| {
                            let same_sel = self.title_font.as_deref() == Some(TITLE_SAME_AS_QUOTE);
                            if ui.selectable_label(same_sel, "Same as quote").clicked() {
                                self.title_font = Some(TITLE_SAME_AS_QUOTE.to_owned());
                            }
                            ui.separator();
                            match &fams {
                                None => {
                                    ui.label("Looking for fonts…");
                                }
                                Some(list) => {
                                    for fam in list.iter() {
                                        let sel = !same_sel
                                            && self
                                                .title_font
                                                .as_deref()
                                                .unwrap_or(fonts::BUNDLED)
                                                .eq_ignore_ascii_case(&fam.name);
                                        if ui.selectable_label(sel, fam.label()).clicked() {
                                            if fam.name.eq_ignore_ascii_case(fonts::BUNDLED) {
                                                self.title_font = None;
                                            } else {
                                                self.title_font = Some(fam.name.clone());
                                            }
                                        }
                                    }
                                }
                            }
                        });
                });
                ui.add_space(10.0);

                // Resolve after switchers so changes update the preview on this frame.
                let card_th = self.resolved_theme(reader_th);

                // Live WYSIWYG preview rendered via the same offscreen rasterizer
                // as `export_png` into a cached texture. This lets any system font
                // be previewed without ever calling `ctx.set_fonts` on the live
                // window (which would wipe egui's font atlas and corrupt cached
                // reader pages).
                let logical = self.card.layout.logical_size();
                let max_box = vec2(420.0, 320.0);
                let preview_scale = (max_box.x / logical.x).min(max_box.y / logical.y);
                let preview_size = logical * preview_scale;
                let (slot, _) =
                    ui.allocate_exact_size(vec2(max_box.x, preview_size.y), Sense::hover());
                let preview_rect = Rect::from_center_size(slot.center(), preview_size);
                let raster_scale = (preview_scale * ctx.pixels_per_point()).clamp(0.5, SCALE);
                let key = PreviewKey {
                    quote: self.card.quote.clone(),
                    title: self.card.title.clone(),
                    author: self.card.author.clone(),
                    date: self.card.date.clone(),
                    layout: self.card.layout,
                    direction: self.card.direction,
                    theme_id: card_th.id,
                    quote_font: self.quote_font.clone(),
                    title_font: self.title_font.clone(),
                    reader_fonts_key: reader_fonts.map(|f| f.key.clone()),
                    sans_default,
                    raster_scale_bits: raster_scale.to_bits(),
                    fonts_scanned: fams.is_some(),
                };
                let needs_render = self.preview.as_ref().is_none_or(|(k, _)| *k != key);
                if needs_render {
                    let scene = render_scene_at_scale(
                        ctx,
                        &self.card,
                        card_th,
                        reader_fonts,
                        sans_default,
                        self.quote_font.as_deref(),
                        self.title_font.as_deref(),
                        fams.as_deref().map(Vec::as_slice),
                        raster_scale,
                    );
                    let img = scene.to_color_image();
                    match &mut self.preview {
                        Some((k, tex)) => {
                            tex.set(img, egui::TextureOptions::LINEAR);
                            *k = key;
                        }
                        None => {
                            let tex = ctx.load_texture(
                                "reader-quote-preview",
                                img,
                                egui::TextureOptions::LINEAR,
                            );
                            self.preview = Some((key, tex));
                        }
                    }
                }
                if let Some((_, tex)) = &self.preview {
                    ui.painter().image(
                        tex.id(),
                        preview_rect,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                ui.painter().rect_stroke(
                    preview_rect,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, theme::mix(reader_th.text, reader_th.page, 0.16)),
                    StrokeKind::Outside,
                );

                ui.add_space(12.0);
                // Editable attribution fields (pre-filled from the book & today).
                egui::Grid::new("reader-quote-fields")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        let field_w = 340.0;
                        ui.label(RichText::new("Title").color(reader_th.subtle));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.card.title)
                                .desired_width(field_w)
                                .hint_text("Book title"),
                        );
                        ui.end_row();
                        ui.label(RichText::new("Author").color(reader_th.subtle));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.card.author)
                                .desired_width(field_w)
                                .hint_text("Author (optional)"),
                        );
                        ui.end_row();
                        ui.label(RichText::new("Date").color(reader_th.subtle));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.card.date)
                                .desired_width(field_w)
                                .hint_text("Date"),
                        );
                        ui.end_row();
                    });

                if let Some(err) = &self.error {
                    ui.add_space(6.0);
                    ui.label(RichText::new(err).color(reader_th.red).size(13.0));
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(card_th.name)
                            .size(12.0)
                            .color(reader_th.muted),
                    );
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        let primary = egui::Button::new(RichText::new("Save PNG…").strong().color(
                            if reader_th.dark {
                                reader_th.page
                            } else {
                                Color32::WHITE
                            },
                        ))
                        .fill(reader_th.accent);
                        if ui.add(primary).clicked() {
                            save = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close_clicked = true;
                        }
                    });
                });
            });
        (open && !close_clicked, save)
    }
}

/// Render `card` into an offscreen [`crate::raster::Scene`] at exact 1024px
/// longest edge (`SCALE = 2.0`).
#[allow(clippy::too_many_arguments)]
pub(super) fn render_scene(
    ctx: &egui::Context,
    card: &QuoteCard,
    th: &Theme,
    reader_fonts: Option<&fonts::Installed>,
    sans_default: bool,
    quote_font: Option<&str>,
    title_font: Option<&str>,
    fams: Option<&[fonts::FamilyInfo]>,
) -> crate::raster::Scene {
    render_scene_at_scale(
        ctx,
        card,
        th,
        reader_fonts,
        sans_default,
        quote_font,
        title_font,
        fams,
        SCALE,
    )
}

#[allow(clippy::too_many_arguments)]
fn render_scene_at_scale(
    ctx: &egui::Context,
    card: &QuoteCard,
    th: &Theme,
    reader_fonts: Option<&fonts::Installed>,
    sans_default: bool,
    quote_font: Option<&str>,
    title_font: Option<&str>,
    fams: Option<&[fonts::FamilyInfo]>,
    scale: f32,
) -> crate::raster::Scene {
    let mut defs = ctx.fonts(|f| f.definitions().clone());
    install_in_defs(&mut defs);
    let (quote_fam, quote_vert) = quote_font
        .and_then(|name| register_custom_family(&mut defs, "quote-body-custom", name, fams))
        .unwrap_or_else(|| quote_font_family(reader_fonts, sans_default));
    let (title_fam, title_vert) = match title_font {
        Some(TITLE_SAME_AS_QUOTE) => (quote_fam.clone(), quote_vert),
        Some(name) => register_custom_family(&mut defs, "quote-title-custom", name, fams)
            .unwrap_or_else(|| (title_font_family(), fonts::wenkai_vert())),
        None => (title_font_family(), fonts::wenkai_vert()),
    };
    let meta_fam = FontFamily::Proportional;
    let meta_vert = fonts::wenkai_vert();
    let egui_theme = ctx.theme();
    let base_style = (*ctx.global_style()).clone();
    let logical = card.layout.logical_size();
    let card = card.clone();

    crate::raster::lay_out_from(
        defs,
        egui_theme,
        base_style,
        logical.x,
        scale,
        th.desk,
        |_| {},
        |_| {},
        |_| (),
        move |ui, _| {
            let (rect, _) = ui.allocate_exact_size(logical, Sense::hover());
            let painter = ui.painter_at(rect);
            let card_fonts = CardFonts {
                quote: &quote_fam,
                quote_vert,
                title: &title_fam,
                title_vert,
                meta: &meta_fam,
                meta_vert,
            };
            paint_quote_card(&painter, rect, &card, th, &card_fonts);
        },
    )
}

/// Write `card` to `path` as a PNG (1024px on its longest edge).
#[allow(clippy::too_many_arguments)]
pub(super) fn export_png(
    ctx: &egui::Context,
    card: &QuoteCard,
    th: &Theme,
    reader_fonts: Option<&fonts::Installed>,
    sans_default: bool,
    quote_font: Option<&str>,
    title_font: Option<&str>,
    fams: Option<&[fonts::FamilyInfo]>,
    path: &Path,
) -> Result<(), String> {
    let scene = render_scene(
        ctx,
        card,
        th,
        reader_fonts,
        sans_default,
        quote_font,
        title_font,
        fams,
    );
    scene.write_png(path)
}

/// Suggested filename for the save dialog.
pub(super) fn suggested_filename(title: &str) -> String {
    let cleaned: String = title
        .trim()
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .take(48)
        .collect();
    let base = cleaned.trim().trim_matches('.');
    if base.is_empty() {
        "quote.png".to_owned()
    } else {
        format!("{base} - quote.png")
    }
}

fn title_font_family() -> FontFamily {
    FontFamily::Name(Arc::from(TITLE_FONT_NAME))
}

fn quote_font_family(
    reader_fonts: Option<&fonts::Installed>,
    sans_default: bool,
) -> (FontFamily, fonts::VertForms) {
    match reader_fonts {
        Some(f) if sans_default => (f.sans.clone(), f.sans_vert),
        Some(f) => (f.serif.clone(), f.serif_vert),
        None => (title_font_family(), fonts::wenkai_vert()),
    }
}

fn register_custom_family(
    defs: &mut FontDefinitions,
    slot_key: &str,
    family_name: &str,
    fams: Option<&[fonts::FamilyInfo]>,
) -> Option<(FontFamily, fonts::VertForms)> {
    if family_name.eq_ignore_ascii_case(fonts::BUNDLED) {
        return Some((title_font_family(), fonts::wenkai_vert()));
    }
    let (blob, index) = fonts::load_family_face(fams?, family_name)?;
    let vert = fonts::VertForms::from_font(blob.as_ref().as_ref(), index);
    defs.font_data.insert(
        slot_key.to_owned(),
        Arc::new(FontData::from_blob(blob, index)),
    );
    let mut chain = vec![slot_key.to_owned()];
    if let Some(prop) = defs.families.get(&FontFamily::Proportional) {
        for name in prop {
            if !chain.contains(name) {
                chain.push(name.clone());
            }
        }
    }
    let fam = FontFamily::Name(Arc::from(slot_key));
    defs.families.insert(fam.clone(), chain);
    Some((fam, vert))
}

fn install_in_defs(defs: &mut FontDefinitions) {
    defs.font_data
        .entry(WENKAI_DATA_KEY.to_owned())
        .or_insert_with(|| Arc::new(FontData::from_static(crate::png_report::WENKAI)));

    let mut title_chain = vec![WENKAI_DATA_KEY.to_owned()];
    if let Some(prop) = defs.families.get(&FontFamily::Proportional) {
        for name in prop {
            if !title_chain.contains(name) {
                title_chain.push(name.clone());
            }
        }
    }
    defs.families.insert(title_font_family(), title_chain);

    // Ensure Proportional has a CJK fallback for authors/titles even in minimal contexts.
    let prop = defs.families.entry(FontFamily::Proportional).or_default();
    if !prop.iter().any(|n| n == WENKAI_DATA_KEY) {
        prop.push(WENKAI_DATA_KEY.to_owned());
    }
}

/// Clean selected chapter text into tidy paragraphs for display on a card.
fn clean_quote(raw: &str) -> String {
    let mut paras: Vec<String> = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim_matches(|c: char| c.is_whitespace() || c == '\u{3000}');
        if !trimmed.is_empty() {
            paras.push(trimmed.to_owned());
        }
    }
    paras.join("\n")
}

/// Local today formatted cleanly (e.g. `"Nov 27, 2026"`).
fn today_string() -> String {
    let day = crate::reading_stats::local_day(crate::reading_stats::now(), 0);
    let (y, m, d) = crate::reading_stats::civil(day as i64);
    format_date(y, m, d)
}

fn format_date(year: i64, month: u32, day: u32) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mon = MONTHS.get(month.saturating_sub(1) as usize).unwrap_or(&"");
    format!("{mon} {day}, {year}")
}

fn has_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        ('\u{4E00}'..='\u{9FFF}').contains(&c)
            || ('\u{3400}'..='\u{4DBF}').contains(&c)
            || ('\u{3040}'..='\u{30FF}').contains(&c)
    })
}

fn format_book_title(title: &str) -> String {
    let t = title.trim();
    if t.is_empty() {
        return String::new();
    }
    if has_cjk(t) && !t.starts_with('《') && !t.starts_with('「') && !t.starts_with('『') {
        format!("《{t}》")
    } else {
        t.to_owned()
    }
}

/// Derived harmonious colour roles from the reader's [`Theme`], guaranteed to
/// keep depth and legibility across all 16 themes (including monochrome Ecru
/// and pure-black Black).
struct CardPalette {
    mat_tl: Color32,
    mat_tr: Color32,
    mat_bl: Color32,
    mat_br: Color32,
    glow_a: Color32,
    glow_b: Color32,
    shadow: Color32,
    shadow_alpha: f32,
    paper_top: Color32,
    paper_bot: Color32,
    card_border: Color32,
    inner_frame: Color32,
    corner_accent: Color32,
    divider: Color32,
    accent_bar: Color32,
    quote_mark: Color32,
    quote_ink: Color32,
    title_ink: Color32,
    author_ink: Color32,
    date_ink: Color32,
}

impl CardPalette {
    fn from_theme(th: &Theme) -> Self {
        let distinct_desk = th.desk != th.page;
        let mat_base = if distinct_desk {
            th.desk
        } else if th.dark {
            // Pure-black Black theme: lift the outer mat gently so the card stands out.
            theme::mix(th.ink, th.page, 0.085)
        } else {
            // Ecru: deepen the outer mat gently into warm parchment.
            theme::mix(th.ink, th.page, 0.065)
        };

        let (mat_tl, mat_tr, mat_bl, mat_br) = if th.dark {
            (
                theme::mix(th.accent, mat_base, 0.10),
                theme::mix(th.page, mat_base, 0.25),
                theme::mix(Color32::BLACK, mat_base, 0.18),
                theme::mix(th.cyan, mat_base, 0.08),
            )
        } else {
            (
                theme::mix(th.accent, mat_base, 0.07),
                theme::mix(th.page, mat_base, 0.25),
                theme::mix(th.ink, mat_base, 0.045),
                theme::mix(th.yellow, mat_base, 0.09),
            )
        };

        let glow_a = th.accent.gamma_multiply(if th.dark { 0.13 } else { 0.09 });
        let glow_b = if th.dark {
            th.cyan.gamma_multiply(0.09)
        } else {
            th.yellow.gamma_multiply(0.09)
        };

        let paper_top = if th.dark {
            theme::mix(th.ink, th.page, 0.025)
        } else {
            theme::mix(Color32::WHITE, th.page, 0.18)
        };
        let paper_bot = if th.dark {
            th.page
        } else {
            theme::mix(th.desk, th.page, 0.14)
        };

        CardPalette {
            mat_tl,
            mat_tr,
            mat_bl,
            mat_br,
            glow_a,
            glow_b,
            shadow: if th.dark {
                Color32::BLACK
            } else {
                theme::mix(th.ink, mat_base, 0.55)
            },
            shadow_alpha: if th.dark { 0.42 } else { 0.14 },
            paper_top,
            paper_bot,
            card_border: theme::mix(th.ink, th.page, if th.dark { 0.16 } else { 0.11 }),
            inner_frame: theme::mix(th.muted, th.page, if th.dark { 0.34 } else { 0.28 }),
            corner_accent: theme::mix(th.accent, th.page, if th.dark { 0.62 } else { 0.55 }),
            divider: theme::mix(th.muted, th.page, if th.dark { 0.38 } else { 0.32 }),
            accent_bar: theme::mix(th.accent, th.page, if th.dark { 0.85 } else { 0.78 }),
            quote_mark: theme::mix(th.accent, th.page, if th.dark { 0.48 } else { 0.42 }),
            quote_ink: th.ink,
            title_ink: th.text,
            author_ink: th.subtle,
            date_ink: th.muted,
        }
    }
}

/// Resolved font families and vertical-form coverage tables for painting a card.
struct CardFonts<'a> {
    quote: &'a FontFamily,
    quote_vert: fonts::VertForms,
    title: &'a FontFamily,
    title_vert: fonts::VertForms,
    meta: &'a FontFamily,
    meta_vert: fonts::VertForms,
}

/// Paint the entire quote card into `rect`, scaling proportionally from its
/// logical size so preview and 1024px PNG are identical.
fn paint_quote_card(
    painter: &egui::Painter,
    rect: Rect,
    card: &QuoteCard,
    th: &Theme,
    fonts: &CardFonts<'_>,
) {
    let logical = card.layout.logical_size();
    let s = rect.width() / logical.x;
    let pal = CardPalette::from_theme(th);

    // 1. Rich ambient mat backdrop (4-corner bilinear gradient + soft radial corner glows).
    paint_quad_gradient(
        painter, rect, pal.mat_tl, pal.mat_tr, pal.mat_bl, pal.mat_br,
    );
    paint_radial_glow(
        painter,
        pos2(
            rect.left() + rect.width() * 0.12,
            rect.top() + rect.height() * 0.10,
        ),
        rect.width().max(rect.height()) * 0.62,
        pal.glow_a,
    );
    paint_radial_glow(
        painter,
        pos2(
            rect.right() - rect.width() * 0.10,
            rect.bottom() - rect.height() * 0.10,
        ),
        rect.width().max(rect.height()) * 0.58,
        pal.glow_b,
    );

    // 2. Floating paper card rectangle.
    let (mx, my) = match card.layout {
        QuoteLayout::Square => (32.0 * s, 32.0 * s),
        QuoteLayout::Horizontal => (30.0 * s, 24.0 * s),
        QuoteLayout::Vertical => (26.0 * s, 28.0 * s),
    };
    let card_rect = rect.shrink2(vec2(mx, my));
    let card_radius = 10.0 * s;

    // Multi-layered feathered drop shadow.
    paint_card_shadow(
        painter,
        card_rect,
        card_radius,
        pal.shadow,
        pal.shadow_alpha,
        s,
    );

    // Card surface: solid base prevents any subpixel fan seam, topped with a
    // rounded vertical cotton-paper sheen.
    painter.rect_filled(card_rect, card_radius, th.page);
    paint_rounded_vertical_gradient(
        painter,
        card_rect,
        card_radius,
        pal.paper_top,
        pal.paper_bot,
    );
    // Crisp hairline card outline.
    painter.rect_stroke(
        card_rect,
        card_radius,
        Stroke::new((1.0 * s).max(0.5), pal.card_border),
        StrokeKind::Inside,
    );

    // 3. Editorial ex-libris inner frame + refined L-corner accents.
    let frame_inset = match card.layout {
        QuoteLayout::Square => 11.0 * s,
        QuoteLayout::Horizontal | QuoteLayout::Vertical => 10.0 * s,
    };
    let frame_rect = card_rect.shrink(frame_inset);
    painter.rect_stroke(
        frame_rect,
        4.0 * s,
        Stroke::new((0.75 * s).max(0.5), pal.inner_frame),
        StrokeKind::Inside,
    );
    paint_corner_ticks(
        painter,
        frame_rect,
        8.5 * s,
        (1.25 * s).max(0.6),
        pal.corner_accent,
    );

    // 4. Layout- and text-direction-specific editorial composition inside `frame_rect`.
    match (card.layout, card.direction) {
        (QuoteLayout::Square, TextDirection::Horizontal) => {
            paint_square_content(
                painter,
                frame_rect,
                card,
                &pal,
                fonts.quote,
                fonts.title,
                fonts.meta,
                s,
            );
        }
        (QuoteLayout::Horizontal, TextDirection::Horizontal) => {
            paint_horizontal_content(
                painter,
                frame_rect,
                card,
                &pal,
                fonts.quote,
                fonts.title,
                fonts.meta,
                s,
            );
        }
        (QuoteLayout::Vertical, TextDirection::Horizontal) => {
            paint_vertical_content(
                painter,
                frame_rect,
                card,
                &pal,
                fonts.quote,
                fonts.title,
                fonts.meta,
                s,
            );
        }
        (QuoteLayout::Square, TextDirection::Vertical) => {
            paint_square_vertical_content(painter, frame_rect, card, &pal, fonts, s);
        }
        (QuoteLayout::Horizontal, TextDirection::Vertical) => {
            paint_horizontal_vertical_content(painter, frame_rect, card, &pal, fonts, s);
        }
        (QuoteLayout::Vertical, TextDirection::Vertical) => {
            paint_vertical_vertical_content(painter, frame_rect, card, &pal, fonts, s);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_square_content(
    painter: &egui::Painter,
    frame: Rect,
    card: &QuoteCard,
    pal: &CardPalette,
    quote_family: &FontFamily,
    title_family: &FontFamily,
    meta_family: &FontFamily,
    s: f32,
) {
    let pad_x = 34.0 * s;
    let pad_top = 30.0 * s;
    let pad_bot = 26.0 * s;
    let inner = Rect::from_min_max(
        pos2(frame.left() + pad_x, frame.top() + pad_top),
        pos2(frame.right() - pad_x, frame.bottom() - pad_bot),
    );

    // Top-left sculpted opening quote mark + short accent bar.
    let qm_size = 18.0 * s;
    let quote_top = inner.top() + qm_size + 18.0 * s;

    // Footer measured first so the quote zone claims every remaining point.
    let footer = measure_side_footer(
        painter,
        inner.width(),
        card,
        pal,
        title_family,
        meta_family,
        13.0 * s,
        10.5 * s,
        10.0 * s,
        s,
    );
    let divider_gap_below = 14.0 * s;
    let divider_y = inner.bottom() - footer.height - divider_gap_below;
    let quote_bottom = divider_y - 18.0 * s;
    let quote_zone = Rect::from_min_max(
        pos2(inner.left(), quote_top),
        pos2(inner.right(), quote_bottom.max(quote_top + 20.0 * s)),
    );

    let span = paint_fitted_quote(
        painter,
        quote_zone,
        &card.quote,
        quote_family,
        pal.quote_ink,
        11.5 * s,
        24.0 * s,
        1.68,
        0.48,
        Align::LEFT,
    );

    // Position the opening quote ornament near the top of the quote block so
    // short quotes feel cohesive while long quotes keep full room.
    let qm_y = match span {
        Some((y0, _)) => {
            (y0 - qm_size - 16.0 * s).clamp(inner.top() + 2.0 * s, inner.top() + 48.0 * s)
        }
        None => inner.top() + 2.0 * s,
    };
    paint_quote_mark(painter, pos2(inner.left(), qm_y), qm_size, pal.quote_mark);
    let rule_y = qm_y + qm_size * 0.52;
    painter.line_segment(
        [
            pos2(inner.left() + qm_size * 1.45, rule_y),
            pos2(inner.left() + qm_size * 1.45 + 26.0 * s, rule_y),
        ],
        Stroke::new((1.5 * s).max(0.75), pal.accent_bar),
    );

    paint_left_accent_divider(
        painter,
        inner.left(),
        inner.right(),
        divider_y,
        28.0 * s,
        pal,
        s,
    );
    paint_side_footer(
        painter,
        inner.left(),
        inner.right(),
        divider_y + divider_gap_below,
        &footer,
    );
}

#[allow(clippy::too_many_arguments)]
fn paint_horizontal_content(
    painter: &egui::Painter,
    frame: Rect,
    card: &QuoteCard,
    pal: &CardPalette,
    quote_family: &FontFamily,
    title_family: &FontFamily,
    meta_family: &FontFamily,
    s: f32,
) {
    let pad_x = 34.0 * s;
    let pad_top = 22.0 * s;
    let pad_bot = 20.0 * s;
    let inner = Rect::from_min_max(
        pos2(frame.left() + pad_x, frame.top() + pad_top),
        pos2(frame.right() - pad_x, frame.bottom() - pad_bot),
    );

    let footer = measure_side_footer(
        painter,
        inner.width(),
        card,
        pal,
        title_family,
        meta_family,
        12.0 * s,
        10.0 * s,
        9.5 * s,
        s,
    );
    let divider_gap_below = 11.0 * s;
    let divider_y = inner.bottom() - footer.height - divider_gap_below;

    // Subtle sculpted quote mark at top-left, then indented quote with an
    // editorial left accent rail running alongside its exact laid-out height.
    let qm_size = 13.0 * s;
    let rail_x = inner.left() + 2.0 * s;
    let quote_left = inner.left() + 18.0 * s;
    let quote_top = inner.top() + qm_size + 10.0 * s;
    let quote_bottom = divider_y - 14.0 * s;
    let quote_zone = Rect::from_min_max(
        pos2(quote_left, quote_top),
        pos2(inner.right(), quote_bottom.max(quote_top + 20.0 * s)),
    );

    let span = paint_fitted_quote(
        painter,
        quote_zone,
        &card.quote,
        quote_family,
        pal.quote_ink,
        10.5 * s,
        21.0 * s,
        1.64,
        0.42,
        Align::LEFT,
    );
    let qm_y = match span {
        Some((y0, _)) => {
            (y0 - qm_size - 10.0 * s).clamp(inner.top() + 1.0 * s, inner.top() + 30.0 * s)
        }
        None => inner.top() + 1.0 * s,
    };
    paint_quote_mark(painter, pos2(inner.left(), qm_y), qm_size, pal.quote_mark);

    if let Some((y0, y1)) = span {
        let rail_w = (2.4 * s).max(1.2);
        let rail_rect = Rect::from_min_max(
            pos2(rail_x, (y0 + 2.0 * s).min(y1)),
            pos2(rail_x + rail_w, (y1 - 2.0 * s).max(y0 + 4.0 * s)),
        );
        painter.rect_filled(rail_rect, rail_w * 0.5, pal.accent_bar);
    }

    paint_left_accent_divider(
        painter,
        inner.left(),
        inner.right(),
        divider_y,
        26.0 * s,
        pal,
        s,
    );
    paint_side_footer(
        painter,
        inner.left(),
        inner.right(),
        divider_y + divider_gap_below,
        &footer,
    );
}

#[allow(clippy::too_many_arguments)]
fn paint_vertical_content(
    painter: &egui::Painter,
    frame: Rect,
    card: &QuoteCard,
    pal: &CardPalette,
    quote_family: &FontFamily,
    title_family: &FontFamily,
    meta_family: &FontFamily,
    s: f32,
) {
    let pad_x = 24.0 * s;
    let pad_top = 26.0 * s;
    let pad_bot = 24.0 * s;
    let inner = Rect::from_min_max(
        pos2(frame.left() + pad_x, frame.top() + pad_top),
        pos2(frame.right() - pad_x, frame.bottom() - pad_bot),
    );
    let cx = inner.center().x;

    // Top centered diamond-and-hairlines bookplate ornament.
    let top_orn_y = inner.top() + 6.0 * s;
    paint_diamond_divider(painter, cx, top_orn_y, 34.0 * s, 3.2 * s, pal, s);
    let qm_size = 14.0 * s;
    paint_quote_mark(
        painter,
        pos2(cx - qm_size * 0.52, top_orn_y + 14.0 * s),
        qm_size,
        pal.quote_mark,
    );
    let quote_top = top_orn_y + 14.0 * s + qm_size + 14.0 * s;

    // Centered colophon at the bottom.
    let colophon = measure_centered_footer(
        painter,
        inner.width(),
        card,
        pal,
        title_family,
        meta_family,
        12.5 * s,
        10.2 * s,
        9.5 * s,
        s,
    );
    let divider_gap_below = 15.0 * s;
    let divider_y = inner.bottom() - colophon.height - divider_gap_below;
    let quote_bottom = divider_y - 18.0 * s;
    let quote_zone = Rect::from_min_max(
        pos2(inner.left(), quote_top),
        pos2(inner.right(), quote_bottom.max(quote_top + 20.0 * s)),
    );

    // Center short single-paragraph quotes for poetic symmetry; left-align longer passages.
    let short_single = !card.quote.contains('\n') && card.quote.chars().count() <= 42;
    let align = if short_single {
        Align::Center
    } else {
        Align::LEFT
    };
    paint_fitted_quote(
        painter,
        quote_zone,
        &card.quote,
        quote_family,
        pal.quote_ink,
        11.0 * s,
        19.5 * s,
        1.70,
        0.50,
        align,
    );

    paint_diamond_divider(painter, cx, divider_y, 44.0 * s, 3.0 * s, pal, s);
    paint_centered_footer(painter, cx, divider_y + divider_gap_below, &colophon);
}

#[allow(clippy::too_many_arguments)]
fn paint_square_vertical_content(
    painter: &egui::Painter,
    frame: Rect,
    card: &QuoteCard,
    pal: &CardPalette,
    fonts: &CardFonts<'_>,
    s: f32,
) {
    let pad_x = 30.0 * s;
    let pad_y = 32.0 * s;
    let inner = Rect::from_min_max(
        pos2(frame.left() + pad_x, frame.top() + pad_y),
        pos2(frame.right() - pad_x, frame.bottom() - pad_y),
    );

    let colophon = measure_vertical_colophon(
        painter,
        inner.height(),
        card,
        pal,
        fonts,
        13.5 * s,
        10.5 * s,
        9.5 * s,
        s,
    );
    let divider_gap_left = 14.0 * s;
    let (divider_x, quote_left) = if colophon.width > 0.0 {
        let dx = inner.left() + colophon.width + divider_gap_left;
        (Some(dx), dx + 20.0 * s)
    } else {
        (None, inner.left())
    };

    // Reserve room on the right for the top-right vertical quote bracket + accent dash.
    let qm_size = 16.0 * s;
    let quote_right = (inner.right() - qm_size - 16.0 * s).max(quote_left + 24.0 * s);
    let quote_zone = Rect::from_min_max(
        pos2(quote_left, inner.top()),
        pos2(quote_right, inner.bottom()),
    );

    let span = paint_fitted_vertical_quote(
        painter,
        quote_zone,
        &card.quote,
        fonts.quote,
        fonts.quote_vert,
        pal.quote_ink,
        11.5 * s,
        24.0 * s,
        1.68,
        0.48,
        1.04,
    );

    // Position the opening vertical bracket near the right edge of the first column
    // so short quotes feel cohesive while dense passages keep full room.
    let orn_right = match span {
        Some(sp) => (sp.right + 14.0 * s + qm_size).min(inner.right() - 2.0 * s),
        None => inner.right() - 2.0 * s,
    };
    let orn_top = match span {
        Some(sp) => (sp.top - 2.0 * s).clamp(inner.top(), inner.bottom() - qm_size - 32.0 * s),
        None => inner.top(),
    };
    paint_vertical_quote_mark(painter, pos2(orn_right, orn_top), qm_size, pal.quote_mark);
    let rule_x = orn_right - qm_size * 0.48;
    let rule_y0 = orn_top + qm_size * 1.25;
    let rule_y1 = (rule_y0 + 26.0 * s).min(inner.bottom());
    if rule_y1 > rule_y0 + 4.0 * s {
        painter.line_segment(
            [pos2(rule_x, rule_y0), pos2(rule_x, rule_y1)],
            Stroke::new((1.5 * s).max(0.75), pal.accent_bar),
        );
    }

    if let Some(dx) = divider_x {
        paint_top_accent_vertical_divider(
            painter,
            dx,
            inner.top(),
            inner.bottom(),
            28.0 * s,
            pal,
            s,
        );
        paint_vertical_colophon(
            painter,
            inner.left(),
            inner.top(),
            inner.bottom(),
            &colophon,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_horizontal_vertical_content(
    painter: &egui::Painter,
    frame: Rect,
    card: &QuoteCard,
    pal: &CardPalette,
    fonts: &CardFonts<'_>,
    s: f32,
) {
    let pad_x = 28.0 * s;
    let pad_y = 24.0 * s;
    let inner = Rect::from_min_max(
        pos2(frame.left() + pad_x, frame.top() + pad_y),
        pos2(frame.right() - pad_x, frame.bottom() - pad_y),
    );

    let colophon = measure_vertical_colophon(
        painter,
        inner.height(),
        card,
        pal,
        fonts,
        12.5 * s,
        10.0 * s,
        9.0 * s,
        s,
    );
    let divider_gap_left = 12.0 * s;
    let (divider_x, quote_left) = if colophon.width > 0.0 {
        let dx = inner.left() + colophon.width + divider_gap_left;
        (Some(dx), dx + 18.0 * s)
    } else {
        (None, inner.left())
    };

    // Reserve room on the right for the vertical accent rail and subtle top-right bracket.
    let qm_size = 13.0 * s;
    let quote_right = (inner.right() - 22.0 * s).max(quote_left + 24.0 * s);
    let quote_zone = Rect::from_min_max(
        pos2(quote_left, inner.top() + 2.0 * s),
        pos2(quote_right, inner.bottom() - 2.0 * s),
    );

    let span = paint_fitted_vertical_quote(
        painter,
        quote_zone,
        &card.quote,
        fonts.quote,
        fonts.quote_vert,
        pal.quote_ink,
        10.5 * s,
        21.0 * s,
        1.64,
        0.42,
        1.04,
    );

    if let Some(sp) = span {
        let rail_w = (2.4 * s).max(1.2);
        let rail_x = (sp.right + 12.0 * s).min(inner.right() - rail_w - 2.0 * s);
        let orn_right = (rail_x + rail_w + 3.0 * s).min(inner.right());
        let orn_top = (sp.top - 2.0 * s).max(inner.top());
        let rail_top = if sp.bottom - orn_top >= qm_size + 18.0 * s {
            paint_vertical_quote_mark(painter, pos2(orn_right, orn_top), qm_size, pal.quote_mark);
            orn_top + qm_size + 6.0 * s
        } else {
            (sp.top + 2.0 * s).min(sp.bottom)
        };
        let rail_bot = (sp.bottom - 2.0 * s).max(rail_top + 4.0 * s);
        let rail_rect = Rect::from_min_max(
            pos2(rail_x, rail_top),
            pos2(rail_x + rail_w, rail_bot.min(inner.bottom())),
        );
        painter.rect_filled(rail_rect, rail_w * 0.5, pal.accent_bar);
    }

    if let Some(dx) = divider_x {
        paint_top_accent_vertical_divider(
            painter,
            dx,
            inner.top(),
            inner.bottom(),
            26.0 * s,
            pal,
            s,
        );
        paint_vertical_colophon(
            painter,
            inner.left(),
            inner.top(),
            inner.bottom(),
            &colophon,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_vertical_vertical_content(
    painter: &egui::Painter,
    frame: Rect,
    card: &QuoteCard,
    pal: &CardPalette,
    fonts: &CardFonts<'_>,
    s: f32,
) {
    let pad_x = 24.0 * s;
    let pad_top = 26.0 * s;
    let pad_bot = 24.0 * s;
    let inner = Rect::from_min_max(
        pos2(frame.left() + pad_x, frame.top() + pad_top),
        pos2(frame.right() - pad_x, frame.bottom() - pad_bot),
    );
    let cx = inner.center().x;

    // Top centered diamond-and-hairlines bookplate ornament.
    let top_orn_y = inner.top() + 6.0 * s;
    paint_diamond_divider(painter, cx, top_orn_y, 34.0 * s, 3.2 * s, pal, s);
    let quote_top = top_orn_y + 22.0 * s;

    // Centered colophon at the bottom.
    let colophon = measure_centered_footer(
        painter,
        inner.width(),
        card,
        pal,
        fonts.title,
        fonts.meta,
        12.5 * s,
        10.2 * s,
        9.5 * s,
        s,
    );
    let divider_gap_below = 15.0 * s;
    let divider_y = inner.bottom() - colophon.height - divider_gap_below;
    let quote_bottom = divider_y - 18.0 * s;
    let quote_zone = Rect::from_min_max(
        pos2(inner.left(), quote_top),
        pos2(inner.right(), quote_bottom.max(quote_top + 20.0 * s)),
    );

    paint_fitted_vertical_quote(
        painter,
        quote_zone,
        &card.quote,
        fonts.quote,
        fonts.quote_vert,
        pal.quote_ink,
        11.0 * s,
        20.0 * s,
        1.66,
        0.48,
        1.04,
    );

    paint_diamond_divider(painter, cx, divider_y, 44.0 * s, 3.0 * s, pal, s);
    paint_centered_footer(painter, cx, divider_y + divider_gap_below, &colophon);
}

/// Fitted paragraphs laid out and painted inside `zone`, returning the painted
/// `(top_y, bottom_y)` span if non-empty.
#[allow(clippy::too_many_arguments)]
fn paint_fitted_quote(
    painter: &egui::Painter,
    zone: Rect,
    quote: &str,
    family: &FontFamily,
    color: Color32,
    min_size: f32,
    max_size: f32,
    line_factor: f32,
    para_factor: f32,
    align: Align,
) -> Option<(f32, f32)> {
    let paras: Vec<&str> = quote.split('\n').filter(|l| !l.trim().is_empty()).collect();
    if paras.is_empty() {
        return None;
    }

    let layout_at = |font_size: f32, cap_rows: Option<usize>| -> (Vec<Arc<egui::Galley>>, f32) {
        let font = FontId::new(font_size, family.clone());
        let line_h = font_size * line_factor;
        let para_gap = font_size * para_factor;
        let max_w = zone.width().max(20.0);
        let mut width_cache = HashMap::new();
        let mut galleys = Vec::with_capacity(paras.len());
        let mut total_h = 0.0f32;
        let mut rows_left = cap_rows;
        for (i, &para) in paras.iter().enumerate() {
            if let Some(0) = rows_left {
                break;
            }
            let wrapped =
                wrap_horizontal_paragraph(painter, para, &font, max_w * 0.99, &mut width_cache);
            let mut job = LayoutJob::default();
            // Safety wrap width slightly above our DP target so proportional kerning
            // differences never cause unexpected extra wrapping.
            job.wrap.max_width = max_w + 1.0;
            job.halign = align;
            if let Some(r) = rows_left {
                job.wrap.max_rows = r;
                job.wrap.overflow_character = Some('…');
            }
            job.append(
                &wrapped,
                0.0,
                egui::TextFormat {
                    font_id: font.clone(),
                    color,
                    line_height: Some(line_h),
                    ..Default::default()
                },
            );
            let gal = painter.layout_job(job);
            if i > 0 {
                total_h += para_gap;
            }
            total_h += gal.size().y;
            if let Some(r) = &mut rows_left {
                *r = r.saturating_sub(gal.rows.len());
            }
            galleys.push(gal);
        }
        (galleys, total_h)
    };

    // Binary search for the largest comfortable font size that fits cleanly.
    let mut lo = min_size;
    let mut hi = max_size;
    for _ in 0..10 {
        let mid = (lo + hi) * 0.5;
        let (_, h) = layout_at(mid, None);
        if h <= zone.height() {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let chosen = (lo * 2.0).floor() / 2.0;
    let chosen = chosen.clamp(min_size, max_size);
    let (mut galleys, mut total_h) = layout_at(chosen, None);

    // If an extraordinarily long selection still overflows at `min_size`, cap
    // cleanly at rows with an ellipsis so it never collides with the colophon.
    if total_h > zone.height() {
        let max_rows = (zone.height() / (min_size * line_factor)).floor().max(1.0) as usize;
        let res = layout_at(min_size, Some(max_rows));
        galleys = res.0;
        total_h = res.1;
    }

    let para_gap = chosen * para_factor;
    // Optical vertical center (46% down the available slack).
    let start_y = zone.top() + ((zone.height() - total_h).max(0.0) * 0.46);
    let mut y = start_y;
    for (i, gal) in galleys.iter().enumerate() {
        if i > 0 {
            y += para_gap;
        }
        let x = match align {
            Align::Center => zone.center().x,
            Align::RIGHT => zone.right(),
            Align::LEFT => zone.left(),
        };
        painter.galley(pos2(x, y), gal.clone(), color);
        y += gal.size().y;
    }
    Some((start_y, y))
}

#[derive(Clone, Copy)]
struct VertAtom {
    ch: char,
    vert: VertGlyph,
    adv: f32,
}

#[derive(Clone)]
struct VertColumn {
    atoms: Vec<VertAtom>,
    height: f32,
    para_break_before: bool,
}

#[derive(Clone, Copy)]
struct VerticalSpan {
    right: f32,
    top: f32,
    bottom: f32,
}

fn vert_break_ok(a: char, b: char) -> bool {
    if b == ' ' {
        return false;
    }
    if a == ' ' || a == '\u{3000}' {
        return true;
    }
    if layout::NO_START.contains(b) || layout::NO_END.contains(a) {
        return false;
    }
    if a == b && matches!(a, '—' | '…' | '‥' | '─' | '―') {
        return false;
    }
    if layout::cjk(a) || layout::cjk(b) {
        return true;
    }
    a == '-' && b.is_alphanumeric()
}

fn is_closing_or_pause_punct(c: char) -> bool {
    matches!(
        c,
        '、' | '。'
            | '，'
            | '．'
            | ','
            | '.'
            | '：'
            | ':'
            | '；'
            | ';'
            | '？'
            | '?'
            | '！'
            | '!'
            | '）'
            | ')'
            | '］'
            | ']'
            | '｝'
            | '}'
            | '〕'
            | '〉'
            | '》'
            | '」'
            | '』'
            | '】'
            | '’'
            | '”'
            | '…'
            | '‥'
    )
}

fn is_content_char(c: char) -> bool {
    !(c.is_whitespace()
        || c == '\u{3000}'
        || layout::NO_START.contains(c)
        || layout::NO_END.contains(c))
}

fn measure_single_char_width(
    painter: &egui::Painter,
    font_id: &FontId,
    ch: char,
    cache: &mut HashMap<char, f32>,
) -> f32 {
    *cache.entry(ch).or_insert_with(|| {
        let layout_str = |s: &str| {
            let mut job = LayoutJob::default();
            job.append(
                s,
                0.0,
                egui::TextFormat {
                    font_id: font_id.clone(),
                    color: Color32::WHITE,
                    line_height: None,
                    ..Default::default()
                },
            );
            job.wrap.max_width = f32::INFINITY;
            painter.layout_job(job).size().x
        };
        if ch == ' ' {
            (layout_str("a a") - layout_str("aa")).max(font_id.size * 0.25)
        } else {
            let mut buf = [0u8; 4];
            layout_str(ch.encode_utf8(&mut buf)).max(font_id.size * 0.25)
        }
    })
}

#[derive(Clone, Copy)]
struct WrapState {
    /// Display width/height of the line starting at this index.
    first_w: f32,
    /// Minimum total cost from this index to the end with `first_w`.
    cost: f64,
    /// Trimmed end index of this line.
    end: usize,
    /// Start index of the next line (`>= n` if this is the last line).
    next_i: usize,
    /// Index into `states[next_i]` chosen for the next line (unused when `next_i >= n`).
    next_state: usize,
}

/// Optimal dynamic-programming line/column breaker shared by horizontal and
/// vertical quote rendering. Uses a Pareto frontier of `(first_line_w, cost)`
/// per start index so non-increasing line lengths (`L_k >= L_{k+1}`) are solved
/// globally without suboptimal middle-line dips, while honouring CJK kinsoku,
/// avoiding single-character orphans (`界。`, `路。`), and avoiding punctuation
/// right at the start/top of a wrapped line (`的，`, `——`).
fn wrap_balanced_spans(chars: &[char], advs: &[f32], max_len: f32) -> Vec<(usize, usize)> {
    let n = chars.len();
    if n == 0 {
        return Vec::new();
    }
    let max_len = max_len.max(1.0);
    let total_real = chars.iter().filter(|&&c| is_content_char(c)).count();

    // `states[i]` holds Pareto-optimal `(first_w, cost)` entries sorted by ascending
    // `first_w` and strictly decreasing `cost`, so the best suffix with
    // `next_w <= trimmed_w + 0.5` is always the last entry with `first_w <= trimmed_w + 0.5`.
    let mut states: Vec<Vec<WrapState>> = vec![Vec::new(); n + 1];
    states[n].push(WrapState {
        first_w: 0.0,
        cost: 0.0,
        end: n,
        next_i: n,
        next_state: 0,
    });

    let eval_line = |i: usize,
                     trimmed_end: usize,
                     next_i: usize,
                     trimmed_w: f32,
                     real_chars: usize,
                     next_line_w: f32|
     -> f64 {
        let is_last = next_i >= n;
        let mut cost = 220.0f64;

        // Avoid single-character orphans on any row/column (e.g. `界。`, `路。`).
        if real_chars <= 1 && total_real >= 3 {
            cost += 1_000_000.0;
        } else if is_last
            && i > 0
            && real_chars == 2
            && total_real >= 6
            && trimmed_w < max_len * 0.45
        {
            cost += 25_000.0;
        }

        // Avoid punctuation right at the start/top of a wrapped line/column (e.g. `的，`, `——`).
        if i > 0 {
            if layout::NO_START.contains(chars[i]) {
                cost += 2_000_000.0;
            } else if matches!(chars[i], '—' | '─' | '―') {
                cost += 100_000.0;
            } else if trimmed_end >= i + 2 && is_closing_or_pause_punct(chars[i + 1]) {
                cost += 500_000.0;
            } else if trimmed_end >= i + 3 && is_closing_or_pause_punct(chars[i + 2]) {
                cost += 1_200.0;
            }
        }

        // Keep non-final lines full and strictly non-increasing (`L_k >= L_{k+1}`):
        // no line may be longer than the line before it.
        let fill = (trimmed_w / max_len).clamp(0.0, 1.0) as f64;
        let slack = 1.0 - fill;
        if !is_last {
            cost += slack * slack * 4000.0;
            if trimmed_w + 0.5 < next_line_w {
                let shortfall = ((next_line_w - trimmed_w) / max_len) as f64;
                cost += 250_000.0 + 250_000.0 * shortfall;
            }
        } else {
            cost += slack * slack * 250.0;
        }
        cost
    };

    let pick_best_continuation = |next_states: &[WrapState],
                                  i: usize,
                                  trimmed_end: usize,
                                  next_i: usize,
                                  trimmed_w: f32,
                                  real_chars: usize|
     -> (usize, f64) {
        // Because `next_states` is sorted by ascending `first_w` and strictly
        // decreasing `cost`, the optimal continuation is either:
        // 1. the last entry with `first_w <= trimmed_w + 0.5` (no monotonic penalty), or
        // 2. the global minimum-cost entry (`last()`) if avoiding a much costlier orphan.
        let count = next_states.partition_point(|s| s.first_w <= trimmed_w + 0.5);
        let mut best_idx = 0usize;
        let mut best_total = f64::INFINITY;
        for idx in [count.saturating_sub(1), next_states.len() - 1] {
            let st = next_states[idx];
            let total =
                eval_line(i, trimmed_end, next_i, trimmed_w, real_chars, st.first_w) + st.cost;
            if total < best_total {
                best_total = total;
                best_idx = idx;
            }
        }
        (best_idx, best_total)
    };

    for i in (0..n).rev() {
        // Leading ASCII spaces on wrapped lines are skipped automatically.
        if chars[i] == ' ' && i + 1 < n {
            states[i] = states[i + 1].clone();
            continue;
        }
        let mut w = 0.0f32;
        let mut trimmed_end = i;
        let mut trimmed_w = 0.0f32;
        let mut real_chars = 0usize;
        let mut cands: Vec<WrapState> = Vec::new();
        let mut fallback: Option<(usize, usize, f32, usize)> = None;

        for j in (i + 1)..=n {
            let ch = chars[j - 1];
            w += advs[j - 1];
            if ch != ' ' {
                if trimmed_end > i && w > max_len {
                    break;
                }
                trimmed_end = j;
                trimmed_w = w;
                if is_content_char(ch) {
                    real_chars += 1;
                }
            }
            if trimmed_end == i {
                continue;
            }
            let mut next_i = j;
            while next_i < n && chars[next_i] == ' ' {
                next_i += 1;
            }
            fallback = Some((trimmed_end, next_i, trimmed_w, real_chars));
            let can_break = j == n || vert_break_ok(chars[j - 1], chars[j]);
            if !can_break {
                continue;
            }
            let (next_state, cost) = pick_best_continuation(
                &states[next_i],
                i,
                trimmed_end,
                next_i,
                trimmed_w,
                real_chars,
            );
            cands.push(WrapState {
                first_w: trimmed_w,
                cost,
                end: trimmed_end,
                next_i,
                next_state,
            });
        }

        if cands.is_empty() {
            if let Some((te, ni, tw, rc)) = fallback {
                let (next_state, cost) = pick_best_continuation(&states[ni], i, te, ni, tw, rc);
                cands.push(WrapState {
                    first_w: tw,
                    cost,
                    end: te,
                    next_i: ni,
                    next_state,
                });
            } else {
                let st = states[i + 1][0];
                cands.push(WrapState {
                    first_w: st.first_w,
                    cost: st.cost,
                    end: i + 1,
                    next_i: i + 1,
                    next_state: 0,
                });
            }
        }

        // Build Pareto frontier: ascending `first_w`, strictly decreasing `cost`.
        cands.sort_by(|a, b| {
            a.first_w
                .total_cmp(&b.first_w)
                .then_with(|| a.cost.total_cmp(&b.cost))
        });
        let mut pareto: Vec<WrapState> = Vec::with_capacity(cands.len());
        let mut min_cost = f64::INFINITY;
        for c in cands {
            if c.cost < min_cost {
                // If two entries have essentially identical `first_w`, replace the worse one.
                if let Some(last) = pareto.last_mut()
                    && (c.first_w - last.first_w).abs() <= 0.25
                {
                    *last = c;
                } else {
                    pareto.push(c);
                }
                min_cost = c.cost;
            }
        }
        states[i] = pareto;
    }

    let mut spans = Vec::new();
    let mut i = 0;
    while i < n && chars[i] == ' ' {
        i += 1;
    }
    if i >= n || states[i].is_empty() {
        return spans;
    }
    // The last entry in the Pareto frontier has the global minimum cost.
    let mut state_idx = states[i].len() - 1;
    while i < n {
        let st = states[i][state_idx];
        if st.end > i {
            spans.push((i, st.end));
        }
        if st.next_i <= i {
            break;
        }
        i = st.next_i;
        state_idx = st.next_state;
    }
    spans
}

fn wrap_horizontal_paragraph(
    painter: &egui::Painter,
    para: &str,
    font_id: &FontId,
    max_w: f32,
    width_cache: &mut HashMap<char, f32>,
) -> String {
    let chars: Vec<char> = para.chars().collect();
    if chars.is_empty() {
        return String::new();
    }
    let advs: Vec<f32> = chars
        .iter()
        .map(|&c| measure_single_char_width(painter, font_id, c, width_cache))
        .collect();
    let spans = wrap_balanced_spans(&chars, &advs, max_w);
    let mut out = String::with_capacity(para.len() + spans.len());
    for (idx, (a, b)) in spans.into_iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        for &c in &chars[a..b] {
            out.push(c);
        }
    }
    out
}

fn build_vert_atoms(
    painter: &egui::Painter,
    text: &str,
    font_size: f32,
    char_step: f32,
    family: &FontFamily,
    vert_forms: fonts::VertForms,
    width_cache: &mut HashMap<char, f32>,
) -> Vec<VertAtom> {
    let font_id = FontId::new(font_size, family.clone());
    text.chars()
        .map(|c| {
            let vert = layout::vert_glyph(c, |vc| vert_forms.has(vc));
            let adv = match vert {
                VertGlyph::Upright { ch: ' ', .. } => font_size * 0.5,
                VertGlyph::Upright { .. } => font_size * char_step,
                VertGlyph::Sideways(gc) => {
                    measure_single_char_width(painter, &font_id, gc, width_cache)
                }
            };
            VertAtom { ch: c, vert, adv }
        })
        .collect()
}

fn wrap_vert_paragraph(atoms: &[VertAtom], max_h: f32, first_para_break: bool) -> Vec<VertColumn> {
    if atoms.is_empty() {
        return Vec::new();
    }
    let chars: Vec<char> = atoms.iter().map(|a| a.ch).collect();
    let advs: Vec<f32> = atoms.iter().map(|a| a.adv).collect();
    let spans = wrap_balanced_spans(&chars, &advs, max_h);
    spans
        .into_iter()
        .enumerate()
        .map(|(idx, (a, b))| {
            let col_atoms = atoms[a..b].to_vec();
            let col_h: f32 = col_atoms.iter().map(|x| x.adv).sum();
            VertColumn {
                atoms: col_atoms,
                height: col_h,
                para_break_before: idx == 0 && first_para_break,
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn append_vertical_ellipsis(
    painter: &egui::Painter,
    col: &mut VertColumn,
    max_h: f32,
    font_size: f32,
    char_step: f32,
    family: &FontFamily,
    vert_forms: fonts::VertForms,
    width_cache: &mut HashMap<char, f32>,
) {
    let font_id = FontId::new(font_size, family.clone());
    let vert = layout::vert_glyph('…', |vc| vert_forms.has(vc));
    let adv = match vert {
        VertGlyph::Upright { .. } => font_size * char_step,
        VertGlyph::Sideways(gc) => measure_single_char_width(painter, &font_id, gc, width_cache),
    };
    let ell = VertAtom {
        ch: '…', vert, adv
    };
    while !col.atoms.is_empty() && col.height + ell.adv > max_h {
        if let Some(popped) = col.atoms.pop() {
            col.height -= popped.adv;
        }
    }
    col.atoms.push(ell);
    col.height += ell.adv;
}

fn columns_total_width(cols: &[VertColumn], font_size: f32, col_pitch: f32, para_gap: f32) -> f32 {
    if cols.is_empty() {
        return 0.0;
    }
    let para_breaks = cols.iter().filter(|c| c.para_break_before).count();
    font_size + (cols.len() - 1) as f32 * col_pitch + para_breaks as f32 * para_gap
}

fn paint_vertical_column(
    painter: &egui::Painter,
    cx: f32,
    start_y: f32,
    atoms: &[VertAtom],
    font_size: f32,
    family: &FontFamily,
    color: Color32,
) {
    let font_id = FontId::new(font_size, family.clone());
    let mut y = start_y;
    let mut i = 0;
    while i < atoms.len() {
        match atoms[i].vert {
            VertGlyph::Upright { ch, dx, dy } => {
                if ch != ' ' && ch != '\u{3000}' {
                    let mut buf = [0u8; 4];
                    let mut job = LayoutJob::default();
                    job.append(
                        ch.encode_utf8(&mut buf),
                        0.0,
                        egui::TextFormat {
                            font_id: font_id.clone(),
                            color,
                            line_height: None,
                            ..Default::default()
                        },
                    );
                    job.wrap.max_width = f32::INFINITY;
                    let g = painter.layout_job(job);
                    let gx = cx - g.size().x * 0.5 + dx * font_size;
                    let gy = y + (atoms[i].adv - g.size().y) * 0.5 + dy * font_size;
                    painter.galley(pos2(gx, gy), g, color);
                }
                y += atoms[i].adv;
                i += 1;
            }
            VertGlyph::Sideways(_) => {
                let mut j = i + 1;
                while j < atoms.len() && matches!(atoms[j].vert, VertGlyph::Sideways(_)) {
                    j += 1;
                }
                let run: String = atoms[i..j]
                    .iter()
                    .map(|a| match a.vert {
                        VertGlyph::Sideways(gc) => gc,
                        VertGlyph::Upright { ch, .. } => ch,
                    })
                    .collect();
                let mut job = LayoutJob::default();
                job.append(
                    &run,
                    0.0,
                    egui::TextFormat {
                        font_id: font_id.clone(),
                        color,
                        line_height: None,
                        ..Default::default()
                    },
                );
                job.wrap.max_width = f32::INFINITY;
                let g = painter.layout_job(job);
                // Rotating a horizontal box by +90° CW around `pos` maps
                // galley y to `pos.x - y`; align cap-height center to `cx`.
                let visual_mid = layout::baseline(&g) - font_size * 0.35;
                painter.add(
                    TextShape::new(pos2(cx + visual_mid, y), g, color)
                        .with_angle(std::f32::consts::FRAC_PI_2),
                );
                y += atoms[i..j].iter().map(|a| a.adv).sum::<f32>();
                i = j;
            }
        }
    }
}

/// Fitted traditional `vertical-rl` paragraphs laid out and painted inside `zone`.
#[allow(clippy::too_many_arguments)]
fn paint_fitted_vertical_quote(
    painter: &egui::Painter,
    zone: Rect,
    quote: &str,
    family: &FontFamily,
    vert_forms: fonts::VertForms,
    color: Color32,
    min_size: f32,
    max_size: f32,
    col_factor: f32,
    para_factor: f32,
    char_step: f32,
) -> Option<VerticalSpan> {
    let paras: Vec<&str> = quote.split('\n').filter(|l| !l.trim().is_empty()).collect();
    if paras.is_empty() {
        return None;
    }
    let max_h = zone.height().max(20.0);

    let layout_at = |font_size: f32, cap_cols: Option<usize>| -> (Vec<VertColumn>, f32) {
        let mut width_cache = HashMap::new();
        let mut cols = Vec::new();
        for (pi, &para) in paras.iter().enumerate() {
            let atoms = build_vert_atoms(
                painter,
                para,
                font_size,
                char_step,
                family,
                vert_forms,
                &mut width_cache,
            );
            cols.extend(wrap_vert_paragraph(&atoms, max_h, pi > 0));
        }
        if let Some(max_c) = cap_cols
            && cols.len() > max_c
        {
            cols.truncate(max_c.max(1));
            if let Some(last) = cols.last_mut() {
                append_vertical_ellipsis(
                    painter,
                    last,
                    max_h,
                    font_size,
                    char_step,
                    family,
                    vert_forms,
                    &mut width_cache,
                );
            }
        }
        let col_pitch = font_size * col_factor;
        let para_gap = font_size * para_factor;
        let total_w = columns_total_width(&cols, font_size, col_pitch, para_gap);
        (cols, total_w)
    };

    // Binary search for the largest comfortable font size whose columns fit `zone.width()`.
    let mut lo = min_size;
    let mut hi = max_size;
    for _ in 0..10 {
        let mid = (lo + hi) * 0.5;
        let (_, w) = layout_at(mid, None);
        if w <= zone.width() {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let mut chosen = ((lo * 2.0).floor() / 2.0).clamp(min_size, max_size);
    let (mut cols, mut total_w) = layout_at(chosen, None);

    // If an extraordinarily long selection still overflows at `min_size`, cap cleanly
    // at columns with a vertical ellipsis so it never collides with the colophon.
    if total_w > zone.width() {
        chosen = min_size;
        let col_pitch = min_size * col_factor;
        let mut cap = (((zone.width() - min_size) / col_pitch).floor() as usize + 1).max(1);
        loop {
            let (c, w) = layout_at(min_size, Some(cap));
            cols = c;
            total_w = w;
            if total_w <= zone.width() || cap <= 1 {
                break;
            }
            cap -= 1;
        }
    }

    if cols.is_empty() {
        return None;
    }

    let col_pitch = chosen * col_factor;
    let para_gap = chosen * para_factor;
    let max_col_h = cols.iter().map(|c| c.height).fold(0.0f32, f32::max);

    // Optical horizontal center biased subtly toward the right reading-start edge (46% from right).
    let right_edge = zone.right() - ((zone.width() - total_w).max(0.0) * 0.46);
    // Optical vertical center when all columns are shorter than `zone.height()`.
    let start_y = zone.top() + ((zone.height() - max_col_h).max(0.0) * 0.46);

    let mut cx = right_edge - chosen * 0.5;
    for (ci, col) in cols.iter().enumerate() {
        if ci > 0 {
            cx -= col_pitch;
            if col.para_break_before {
                cx -= para_gap;
            }
        }
        paint_vertical_column(painter, cx, start_y, &col.atoms, chosen, family, color);
    }

    Some(VerticalSpan {
        right: right_edge,
        top: start_y,
        bottom: start_y + max_col_h,
    })
}

struct SideFooter {
    title: Option<Arc<egui::Galley>>,
    title_hang_x: f32,
    author: Option<Arc<egui::Galley>>,
    date: Option<Arc<egui::Galley>>,
    title_author_gap: f32,
    height: f32,
    title_color: Color32,
    author_color: Color32,
    date_color: Color32,
}

#[allow(clippy::too_many_arguments)]
fn measure_side_footer(
    painter: &egui::Painter,
    width: f32,
    card: &QuoteCard,
    pal: &CardPalette,
    title_family: &FontFamily,
    meta_family: &FontFamily,
    title_size: f32,
    author_size: f32,
    date_size: f32,
    s: f32,
) -> SideFooter {
    let date_text = card.date.trim();
    let date = (!date_text.is_empty()).then(|| {
        painter.layout_no_wrap(
            date_text.to_owned(),
            FontId::new(date_size, meta_family.clone()),
            pal.date_ink,
        )
    });
    let date_w = date.as_ref().map_or(0.0, |g| g.size().x);
    let left_w = if date_w > 0.0 {
        (width - date_w - 16.0 * s).max(width * 0.45)
    } else {
        width
    };

    let title_str = format_book_title(&card.title);
    // Optical hanging punctuation for leading full-width book-title brackets (`《`, `「`, `『`).
    let title_hang_x = if title_str.starts_with(['《', '「', '『']) {
        title_size * 0.42
    } else {
        0.0
    };
    let title = (!title_str.is_empty()).then(|| {
        let mut job = LayoutJob::default();
        job.wrap.max_width = left_w + title_hang_x;
        job.wrap.max_rows = 2;
        job.wrap.overflow_character = Some('…');
        job.append(
            &title_str,
            0.0,
            egui::TextFormat {
                font_id: FontId::new(title_size, title_family.clone()),
                color: pal.title_ink,
                line_height: Some(title_size * 1.32),
                ..Default::default()
            },
        );
        painter.layout_job(job)
    });

    let author_str = card.author.trim();
    let author = (!author_str.is_empty()).then(|| {
        let mut job = LayoutJob::default();
        job.wrap.max_width = left_w;
        job.wrap.max_rows = 1;
        job.wrap.overflow_character = Some('…');
        job.append(
            author_str,
            0.0,
            egui::TextFormat {
                font_id: FontId::new(author_size, meta_family.clone()),
                color: pal.author_ink,
                ..Default::default()
            },
        );
        painter.layout_job(job)
    });

    let title_author_gap = 3.0 * s;
    let left_h = match (&title, &author) {
        (Some(t), Some(a)) => t.size().y + title_author_gap + a.size().y,
        (Some(t), None) => t.size().y,
        (None, Some(a)) => a.size().y,
        (None, None) => 0.0,
    };
    let right_h = date.as_ref().map_or(0.0, |g| g.size().y);
    SideFooter {
        title,
        title_hang_x,
        author,
        date,
        title_author_gap,
        height: left_h.max(right_h),
        title_color: pal.title_ink,
        author_color: pal.author_ink,
        date_color: pal.date_ink,
    }
}

fn paint_side_footer(
    painter: &egui::Painter,
    left: f32,
    right: f32,
    top: f32,
    footer: &SideFooter,
) {
    let mut y = top;
    if let Some(t) = &footer.title {
        painter.galley(
            pos2(left - footer.title_hang_x, y),
            t.clone(),
            footer.title_color,
        );
        y += t.size().y + footer.title_author_gap;
    }
    if let Some(a) = &footer.author {
        painter.galley(pos2(left, y), a.clone(), footer.author_color);
    }
    if let Some(d) = &footer.date {
        // Align date cleanly with the bottom of the attribution block.
        let dy = top + (footer.height - d.size().y).max(0.0);
        painter.galley(pos2(right - d.size().x, dy), d.clone(), footer.date_color);
    }
}

struct CenteredFooter {
    title: Option<Arc<egui::Galley>>,
    author: Option<Arc<egui::Galley>>,
    date: Option<Arc<egui::Galley>>,
    author_gap: f32,
    date_gap: f32,
    height: f32,
    title_color: Color32,
    author_color: Color32,
    date_color: Color32,
}

#[allow(clippy::too_many_arguments)]
fn measure_centered_footer(
    painter: &egui::Painter,
    width: f32,
    card: &QuoteCard,
    pal: &CardPalette,
    title_family: &FontFamily,
    meta_family: &FontFamily,
    title_size: f32,
    author_size: f32,
    date_size: f32,
    s: f32,
) -> CenteredFooter {
    let title_str = format_book_title(&card.title);
    let title = (!title_str.is_empty()).then(|| {
        let mut job = LayoutJob::default();
        job.wrap.max_width = width;
        job.wrap.max_rows = 2;
        job.wrap.overflow_character = Some('…');
        job.halign = Align::Center;
        job.append(
            &title_str,
            0.0,
            egui::TextFormat {
                font_id: FontId::new(title_size, title_family.clone()),
                color: pal.title_ink,
                line_height: Some(title_size * 1.32),
                ..Default::default()
            },
        );
        painter.layout_job(job)
    });

    let author_str = card.author.trim();
    let author = (!author_str.is_empty()).then(|| {
        let mut job = LayoutJob::default();
        job.wrap.max_width = width;
        job.wrap.max_rows = 1;
        job.wrap.overflow_character = Some('…');
        job.halign = Align::Center;
        job.append(
            author_str,
            0.0,
            egui::TextFormat {
                font_id: FontId::new(author_size, meta_family.clone()),
                color: pal.author_ink,
                ..Default::default()
            },
        );
        painter.layout_job(job)
    });

    let date_str = card.date.trim();
    let date = (!date_str.is_empty()).then(|| {
        painter.layout_no_wrap(
            date_str.to_owned(),
            FontId::new(date_size, meta_family.clone()),
            pal.date_ink,
        )
    });

    let author_gap = 4.0 * s;
    let date_gap = 8.0 * s;
    let mut h = 0.0;
    if let Some(t) = &title {
        h += t.size().y;
    }
    if let Some(a) = &author {
        if h > 0.0 {
            h += author_gap;
        }
        h += a.size().y;
    }
    if let Some(d) = &date {
        if h > 0.0 {
            h += date_gap;
        }
        h += d.size().y;
    }

    CenteredFooter {
        title,
        author,
        date,
        author_gap,
        date_gap,
        height: h,
        title_color: pal.title_ink,
        author_color: pal.author_ink,
        date_color: pal.date_ink,
    }
}

fn paint_centered_footer(painter: &egui::Painter, cx: f32, top: f32, footer: &CenteredFooter) {
    let mut y = top;
    if let Some(t) = &footer.title {
        painter.galley(pos2(cx, y), t.clone(), footer.title_color);
        y += t.size().y;
    }
    if let Some(a) = &footer.author {
        if footer.title.is_some() {
            y += footer.author_gap;
        }
        painter.galley(pos2(cx, y), a.clone(), footer.author_color);
        y += a.size().y;
    }
    if let Some(d) = &footer.date {
        if footer.title.is_some() || footer.author.is_some() {
            y += footer.date_gap;
        }
        painter.galley(pos2(cx - d.size().x * 0.5, y), d.clone(), footer.date_color);
    }
}

enum ColophonStrip {
    VerticalCols {
        cols: Vec<VertColumn>,
        font_size: f32,
        col_pitch: f32,
        family: FontFamily,
        color: Color32,
        hang_top: f32,
    },
    Sideways {
        galley: Arc<egui::Galley>,
        font_size: f32,
        color: Color32,
    },
}

impl ColophonStrip {
    fn width(&self) -> f32 {
        match self {
            ColophonStrip::VerticalCols {
                cols,
                font_size,
                col_pitch,
                ..
            } => {
                if cols.is_empty() {
                    0.0
                } else {
                    font_size + (cols.len() - 1) as f32 * col_pitch
                }
            }
            ColophonStrip::Sideways { font_size, .. } => font_size * 1.15,
        }
    }

    fn height(&self) -> f32 {
        match self {
            ColophonStrip::VerticalCols { cols, .. } => {
                cols.iter().map(|c| c.height).fold(0.0f32, f32::max)
            }
            ColophonStrip::Sideways { galley, .. } => galley.size().x,
        }
    }

    fn col_count(&self) -> usize {
        match self {
            ColophonStrip::VerticalCols { cols, .. } => cols.len(),
            ColophonStrip::Sideways { .. } => 1,
        }
    }

    fn paint_right_aligned(&self, painter: &egui::Painter, right_edge: f32, top_y: f32) {
        match self {
            ColophonStrip::VerticalCols {
                cols,
                font_size,
                col_pitch,
                family,
                color,
                hang_top,
            } => {
                let mut cx = right_edge - font_size * 0.5;
                for (i, col) in cols.iter().enumerate() {
                    if i > 0 {
                        cx -= col_pitch;
                    }
                    let y0 = if i == 0 { top_y - hang_top } else { top_y };
                    paint_vertical_column(painter, cx, y0, &col.atoms, *font_size, family, *color);
                }
            }
            ColophonStrip::Sideways {
                galley,
                font_size,
                color,
            } => {
                let cx = right_edge - self.width() * 0.5;
                self.paint_centered_on_axis(painter, cx, top_y, galley, *font_size, *color);
            }
        }
    }

    fn paint_on_centerline(&self, painter: &egui::Painter, cx: f32, top_y: f32) {
        match self {
            ColophonStrip::VerticalCols {
                cols,
                font_size,
                family,
                color,
                ..
            } => {
                if let Some(col) = cols.first() {
                    paint_vertical_column(
                        painter, cx, top_y, &col.atoms, *font_size, family, *color,
                    );
                }
            }
            ColophonStrip::Sideways {
                galley,
                font_size,
                color,
            } => {
                self.paint_centered_on_axis(painter, cx, top_y, galley, *font_size, *color);
            }
        }
    }

    fn paint_centered_on_axis(
        &self,
        painter: &egui::Painter,
        cx: f32,
        top_y: f32,
        galley: &Arc<egui::Galley>,
        font_size: f32,
        color: Color32,
    ) {
        let visual_mid = layout::baseline(galley) - font_size * 0.35;
        painter.add(
            TextShape::new(pos2(cx + visual_mid, top_y), galley.clone(), color)
                .with_angle(std::f32::consts::FRAC_PI_2),
        );
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DatePlacement {
    None,
    ShareAuthor,
    ShareTitle,
    OwnColumn,
}

struct VerticalColophon {
    title: Option<ColophonStrip>,
    author: Option<ColophonStrip>,
    date: Option<ColophonStrip>,
    date_mode: DatePlacement,
    title_author_gap: f32,
    date_gap: f32,
    title_slot_w: f32,
    author_slot_w: f32,
    width: f32,
}

#[allow(clippy::too_many_arguments)]
fn build_colophon_strip(
    painter: &egui::Painter,
    text: &str,
    avail_h: f32,
    max_cols: usize,
    font_size: f32,
    family: &FontFamily,
    vert_forms: fonts::VertForms,
    color: Color32,
    hang_bracket: bool,
) -> Option<ColophonStrip> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if has_cjk(trimmed) {
        let char_step = 1.03;
        let hang_top = if hang_bracket && trimmed.starts_with(['《', '「', '『']) {
            font_size * 0.24
        } else {
            0.0
        };
        let mut width_cache = HashMap::new();
        let atoms = build_vert_atoms(
            painter,
            trimmed,
            font_size,
            char_step,
            family,
            vert_forms,
            &mut width_cache,
        );
        let max_h = (avail_h + hang_top).max(font_size * 1.5);
        let mut cols = wrap_vert_paragraph(&atoms, max_h, false);
        if cols.len() > max_cols {
            cols.truncate(max_cols.max(1));
            if let Some(last) = cols.last_mut() {
                append_vertical_ellipsis(
                    painter,
                    last,
                    max_h,
                    font_size,
                    char_step,
                    family,
                    vert_forms,
                    &mut width_cache,
                );
            }
        }
        Some(ColophonStrip::VerticalCols {
            cols,
            font_size,
            col_pitch: font_size * 1.32,
            family: family.clone(),
            color,
            hang_top,
        })
    } else {
        let mut job = LayoutJob::default();
        job.wrap.max_width = avail_h.max(20.0);
        job.wrap.max_rows = 1;
        job.wrap.overflow_character = Some('…');
        job.append(
            trimmed,
            0.0,
            egui::TextFormat {
                font_id: FontId::new(font_size, family.clone()),
                color,
                line_height: None,
                ..Default::default()
            },
        );
        let galley = painter.layout_job(job);
        Some(ColophonStrip::Sideways {
            galley,
            font_size,
            color,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn measure_vertical_colophon(
    painter: &egui::Painter,
    avail_h: f32,
    card: &QuoteCard,
    pal: &CardPalette,
    fonts: &CardFonts<'_>,
    title_size: f32,
    author_size: f32,
    date_size: f32,
    s: f32,
) -> VerticalColophon {
    let title_str = format_book_title(&card.title);
    let title = build_colophon_strip(
        painter,
        &title_str,
        avail_h,
        2,
        title_size,
        fonts.title,
        fonts.title_vert,
        pal.title_ink,
        true,
    );
    let author = build_colophon_strip(
        painter,
        &card.author,
        avail_h,
        1,
        author_size,
        fonts.meta,
        fonts.meta_vert,
        pal.author_ink,
        false,
    );
    let date = build_colophon_strip(
        painter,
        &card.date,
        avail_h,
        1,
        date_size,
        fonts.meta,
        fonts.meta_vert,
        pal.date_ink,
        false,
    );

    let min_vgap = 16.0 * s;
    let date_mode = match &date {
        None => DatePlacement::None,
        Some(d) => {
            if let Some(a) = &author
                && a.height() + min_vgap + d.height() <= avail_h
            {
                DatePlacement::ShareAuthor
            } else if author.is_none()
                && let Some(t) = &title
                && t.col_count() == 1
                && t.height() + min_vgap + d.height() <= avail_h
            {
                DatePlacement::ShareTitle
            } else {
                DatePlacement::OwnColumn
            }
        }
    };

    let title_author_gap = 6.0 * s;
    let date_gap = 7.0 * s;
    let mut title_slot_w = title.as_ref().map_or(0.0, ColophonStrip::width);
    let mut author_slot_w = author.as_ref().map_or(0.0, ColophonStrip::width);
    let date_w = date.as_ref().map_or(0.0, ColophonStrip::width);
    match date_mode {
        DatePlacement::ShareAuthor => author_slot_w = author_slot_w.max(date_w),
        DatePlacement::ShareTitle => title_slot_w = title_slot_w.max(date_w),
        DatePlacement::OwnColumn | DatePlacement::None => {}
    }

    let mut width = 0.0;
    if title.is_some() {
        width += title_slot_w;
    }
    if author.is_some() {
        if width > 0.0 {
            width += title_author_gap;
        }
        width += author_slot_w;
    }
    if date_mode == DatePlacement::OwnColumn {
        if width > 0.0 {
            width += date_gap;
        }
        width += date_w;
    }

    VerticalColophon {
        title,
        author,
        date,
        date_mode,
        title_author_gap,
        date_gap,
        title_slot_w,
        author_slot_w,
        width,
    }
}

fn paint_vertical_colophon(
    painter: &egui::Painter,
    left: f32,
    top: f32,
    bottom: f32,
    colophon: &VerticalColophon,
) {
    let mut cursor_right = left + colophon.width;
    let mut title_cx = None;
    let mut author_cx = None;

    if let Some(t) = &colophon.title {
        let slot_center = cursor_right - colophon.title_slot_w * 0.5;
        let strip_right = slot_center + t.width() * 0.5;
        t.paint_right_aligned(painter, strip_right, top);
        title_cx = Some(slot_center);
        cursor_right -= colophon.title_slot_w;
        if colophon.author.is_some() {
            cursor_right -= colophon.title_author_gap;
        }
    }
    if let Some(a) = &colophon.author {
        let slot_center = cursor_right - colophon.author_slot_w * 0.5;
        let strip_right = slot_center + a.width() * 0.5;
        a.paint_right_aligned(painter, strip_right, top);
        author_cx = Some(slot_center);
        cursor_right -= colophon.author_slot_w;
    }
    if let Some(d) = &colophon.date {
        let dy = (bottom - d.height()).max(top);
        match colophon.date_mode {
            DatePlacement::ShareAuthor => {
                if let Some(cx) = author_cx {
                    d.paint_on_centerline(painter, cx, dy);
                }
            }
            DatePlacement::ShareTitle => {
                if let Some(cx) = title_cx {
                    d.paint_on_centerline(painter, cx, dy);
                }
            }
            DatePlacement::OwnColumn => {
                let right_edge = if colophon.title.is_some() || colophon.author.is_some() {
                    cursor_right - colophon.date_gap
                } else {
                    cursor_right
                };
                d.paint_right_aligned(painter, right_edge, dy);
            }
            DatePlacement::None => {}
        }
    }
}

/// Rounded rectangle filled with a smooth vertical gradient (vertex-coloured
/// triangle fan), keeping crisp rounded corners with zero square-clip bleed.
fn paint_rounded_vertical_gradient(
    painter: &egui::Painter,
    rect: Rect,
    radius: f32,
    top: Color32,
    bottom: Color32,
) {
    let r = radius
        .min(rect.width() * 0.5)
        .min(rect.height() * 0.5)
        .max(0.0);
    let h = rect.height().max(1.0);
    let color_at = |y: f32| {
        let t = ((y - rect.top()) / h).clamp(0.0, 1.0);
        theme::mix(bottom, top, t)
    };
    const STEPS: usize = 8;
    let mut perimeter = Vec::with_capacity(4 * (STEPS + 1));
    let corners = [
        (
            pos2(rect.right() - r, rect.top() + r),
            -std::f32::consts::FRAC_PI_2,
            0.0,
        ),
        (
            pos2(rect.right() - r, rect.bottom() - r),
            0.0,
            std::f32::consts::FRAC_PI_2,
        ),
        (
            pos2(rect.left() + r, rect.bottom() - r),
            std::f32::consts::FRAC_PI_2,
            std::f32::consts::PI,
        ),
        (
            pos2(rect.left() + r, rect.top() + r),
            std::f32::consts::PI,
            std::f32::consts::PI * 1.5,
        ),
    ];
    for (center, a0, a1) in corners {
        for i in 0..=STEPS {
            let a = a0 + (a1 - a0) * (i as f32 / STEPS as f32);
            perimeter.push(pos2(center.x + r * a.cos(), center.y + r * a.sin()));
        }
    }
    let mut mesh = Mesh::default();
    let uv = egui::epaint::WHITE_UV;
    let c = rect.center();
    mesh.vertices.push(Vertex {
        pos: c,
        uv,
        color: color_at(c.y),
    });
    for &p in &perimeter {
        mesh.vertices.push(Vertex {
            pos: p,
            uv,
            color: color_at(p.y),
        });
    }
    let n = perimeter.len() as u32;
    for i in 0..n {
        let next = if i + 1 == n { 1 } else { i + 2 };
        mesh.indices.extend_from_slice(&[0, i + 1, next]);
    }
    painter.add(Shape::mesh(mesh));
}

/// Smooth 4-corner bilinear gradient quad via a vertex-coloured [`Mesh`].
fn paint_quad_gradient(
    painter: &egui::Painter,
    rect: Rect,
    tl: Color32,
    tr: Color32,
    bl: Color32,
    br: Color32,
) {
    let mut mesh = Mesh::default();
    let uv = egui::epaint::WHITE_UV;
    mesh.vertices.push(Vertex {
        pos: rect.left_top(),
        uv,
        color: tl,
    });
    mesh.vertices.push(Vertex {
        pos: rect.right_top(),
        uv,
        color: tr,
    });
    mesh.vertices.push(Vertex {
        pos: rect.right_bottom(),
        uv,
        color: br,
    });
    mesh.vertices.push(Vertex {
        pos: rect.left_bottom(),
        uv,
        color: bl,
    });
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    painter.add(Shape::mesh(mesh));
}

/// Soft atmospheric radial glow (triangle fan fading smoothly to transparent).
fn paint_radial_glow(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    if color.a() == 0 || radius <= 0.0 {
        return;
    }
    const SEGMENTS: usize = 32;
    let mut mesh = Mesh::default();
    let uv = egui::epaint::WHITE_UV;
    let edge = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 0);
    mesh.vertices.push(Vertex {
        pos: center,
        uv,
        color,
    });
    for i in 0..=SEGMENTS {
        let a = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        mesh.vertices.push(Vertex {
            pos: pos2(center.x + radius * a.cos(), center.y + radius * a.sin()),
            uv,
            color: edge,
        });
    }
    for i in 1..=SEGMENTS {
        mesh.indices
            .extend_from_slice(&[0, i as u32, (i + 1) as u32]);
    }
    painter.add(Shape::mesh(mesh));
}

/// Multi-layered soft downward drop shadow behind the floating paper card.
fn paint_card_shadow(
    painter: &egui::Painter,
    card: Rect,
    radius: f32,
    color: Color32,
    max_alpha: f32,
    s: f32,
) {
    let layers = 6;
    for i in (1..=layers).rev() {
        let t = i as f32 / layers as f32;
        let spread = t * 14.0 * s;
        let offset_y = (2.0 + t * 8.0) * s;
        let alpha = max_alpha * (1.0 - t * 0.85).powi(2) / layers as f32 * 2.4;
        let a = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
        if a == 0 {
            continue;
        }
        let r = card.expand(spread).translate(vec2(0.0, offset_y));
        painter.rect_filled(
            r,
            radius + spread * 0.5,
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a),
        );
    }
}

/// Subtle ex-libris L-shaped corner accents at the 4 corners of the inner frame.
fn paint_corner_ticks(painter: &egui::Painter, frame: Rect, arm: f32, width: f32, color: Color32) {
    let stroke = Stroke::new(width, color);
    let r = frame;
    let corners = [
        (r.left_top(), vec2(arm, 0.0), vec2(0.0, arm)),
        (r.right_top(), vec2(-arm, 0.0), vec2(0.0, arm)),
        (r.left_bottom(), vec2(arm, 0.0), vec2(0.0, -arm)),
        (r.right_bottom(), vec2(-arm, 0.0), vec2(0.0, -arm)),
    ];
    for (c, dx, dy) in corners {
        painter.line_segment([c + dx, c], stroke);
        painter.line_segment([c, c + dy], stroke);
    }
}

/// Hairline divider with a crisp accent dash on the left end.
fn paint_left_accent_divider(
    painter: &egui::Painter,
    left: f32,
    right: f32,
    y: f32,
    accent_len: f32,
    pal: &CardPalette,
    s: f32,
) {
    painter.line_segment(
        [pos2(left, y), pos2(right, y)],
        Stroke::new((0.75 * s).max(0.5), pal.divider),
    );
    painter.line_segment(
        [pos2(left, y), pos2((left + accent_len).min(right), y)],
        Stroke::new((1.75 * s).max(0.9), pal.accent_bar),
    );
}

/// Vertical hairline divider with a crisp accent dash on the top end.
fn paint_top_accent_vertical_divider(
    painter: &egui::Painter,
    x: f32,
    top: f32,
    bottom: f32,
    accent_len: f32,
    pal: &CardPalette,
    s: f32,
) {
    painter.line_segment(
        [pos2(x, top), pos2(x, bottom)],
        Stroke::new((0.75 * s).max(0.5), pal.divider),
    );
    painter.line_segment(
        [pos2(x, top), pos2(x, (top + accent_len).min(bottom))],
        Stroke::new((1.75 * s).max(0.9), pal.accent_bar),
    );
}

/// Crisp sculpted vector vertical opening corner bracket (`﹁`) drawn directly
/// with geometric bars so it renders identically across every font and OS.
fn paint_vertical_quote_mark(painter: &egui::Painter, top_right: Pos2, size: f32, color: Color32) {
    let t = (size * 0.17).max(1.2);
    let h_arm = size * 0.86;
    let v_arm = size * 0.94;
    let top_bar = Rect::from_min_max(
        pos2(top_right.x - h_arm, top_right.y),
        pos2(top_right.x, top_right.y + t),
    );
    let right_bar = Rect::from_min_max(
        pos2(top_right.x - t, top_right.y),
        pos2(top_right.x, top_right.y + v_arm),
    );
    painter.rect_filled(top_bar, CornerRadius::ZERO, color);
    painter.rect_filled(right_bar, CornerRadius::ZERO, color);
}

/// Centered horizontal hairlines flanking a crisp 4-point diamond ornament.
fn paint_diamond_divider(
    painter: &egui::Painter,
    cx: f32,
    y: f32,
    wing: f32,
    diamond: f32,
    pal: &CardPalette,
    s: f32,
) {
    let gap = diamond + 6.0 * s;
    let stroke = Stroke::new((0.75 * s).max(0.5), pal.divider);
    painter.line_segment([pos2(cx - gap - wing, y), pos2(cx - gap, y)], stroke);
    painter.line_segment([pos2(cx + gap, y), pos2(cx + gap + wing, y)], stroke);
    let pts = vec![
        pos2(cx, y - diamond),
        pos2(cx + diamond, y),
        pos2(cx, y + diamond),
        pos2(cx - diamond, y),
    ];
    painter.add(Shape::Path(PathShape::convex_polygon(
        pts,
        pal.accent_bar,
        Stroke::NONE,
    )));
}

/// Crisp sculpted vector opening double-quote mark (`“`) drawn directly with
/// smooth geometric forms so it renders identically across every font and OS.
fn paint_quote_mark(painter: &egui::Painter, top_left: Pos2, height: f32, color: Color32) {
    let r = height * 0.23;
    let spacing = height * 0.62;
    for k in 0..2 {
        let ox = top_left.x + r + k as f32 * spacing;
        let cy = top_left.y + height - r;
        // Round bowl at the bottom of the opening quote mark.
        painter.circle_filled(pos2(ox, cy), r, color);
        // Sweeping tapered upper stem curving gracefully up and to the right.
        let mut pts = Vec::with_capacity(14);
        let p0 = pos2(ox - r * 0.96, cy - r * 0.08);
        let c1 = pos2(ox - r * 0.92, top_left.y + height * 0.24);
        let c2 = pos2(ox - r * 0.10, top_left.y + height * 0.04);
        let tip = pos2(ox + r * 0.72, top_left.y + height * 0.03);
        let tip_in = pos2(ox + r * 0.84, top_left.y + height * 0.17);
        let ic1 = pos2(ox + r * 0.12, top_left.y + height * 0.24);
        let ic2 = pos2(ox - r * 0.15, cy - r * 0.48);
        let p_end = pos2(ox + r * 0.20, cy - r * 0.62);
        for i in 0..=6 {
            let t = i as f32 / 6.0;
            pts.push(cubic_bezier(p0, c1, c2, tip, t));
        }
        for i in 0..=6 {
            let t = i as f32 / 6.0;
            pts.push(cubic_bezier(tip_in, ic1, ic2, p_end, t));
        }
        painter.add(Shape::Path(PathShape::convex_polygon(
            pts,
            color,
            Stroke::NONE,
        )));
    }
}

fn cubic_bezier(p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2, t: f32) -> Pos2 {
    let u = 1.0 - t;
    let tt = t * t;
    let uu = u * u;
    let uuu = uu * u;
    let ttt = tt * t;
    pos2(
        uuu * p0.x + 3.0 * uu * t * p1.x + 3.0 * u * tt * p2.x + ttt * p3.x,
        uuu * p0.y + 3.0 * uu * t * p1.y + 3.0 * u * tt * p2.y + ttt * p3.y,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(Default::default(), |_| {});
        out.textures_delta.clear();
        ctx
    }

    #[test]
    fn clean_quote_trims_indentation_and_blank_lines() {
        assert_eq!(
            clean_quote("\u{3000}\u{3000}第一段。\n\n  \u{3000}第二段。  \n"),
            "第一段。\n第二段。"
        );
    }

    #[test]
    fn book_title_adds_guillemets_only_for_cjk_without_them() {
        assert_eq!(format_book_title("活着"), "《活着》");
        assert_eq!(format_book_title("《活着》"), "《活着》");
        assert_eq!(format_book_title("The Great Gatsby"), "The Great Gatsby");
        assert_eq!(format_book_title("   "), "");
    }

    #[test]
    fn date_and_filename_formatting() {
        assert_eq!(format_date(2026, 11, 27), "Nov 27, 2026");
        assert_eq!(format_date(2026, 1, 5), "Jan 5, 2026");
        assert_eq!(suggested_filename("活着"), "活着 - quote.png");
        assert_eq!(suggested_filename("A/B:C"), "A_B_C - quote.png");
        assert_eq!(suggested_filename("  "), "quote.png");
    }

    #[test]
    fn all_layouts_export_at_exact_1024px_longest_edge() {
        let ctx = test_ctx();
        let th = &theme::THEMES[0];
        let mut card = QuoteCard {
            quote: "人是为活着本身而活着的，而不是为了活着之外的任何事物所活着。".to_owned(),
            title: "活着".to_owned(),
            author: "余华".to_owned(),
            date: "Nov 27, 2026".to_owned(),
            layout: QuoteLayout::Square,
            direction: TextDirection::Horizontal,
        };
        let dir = tempfile::tempdir().unwrap();
        for direction in TextDirection::ALL {
            card.direction = direction;
            for (layout, expected) in [
                (QuoteLayout::Square, [1024, 1024]),
                (QuoteLayout::Horizontal, [1024, 640]),
                (QuoteLayout::Vertical, [640, 1024]),
            ] {
                card.layout = layout;
                let scene = render_scene(&ctx, &card, th, None, false, None, None, None);
                assert_eq!(scene.size(), expected, "{layout:?} {direction:?}");
                assert_eq!(scene.size()[0].max(scene.size()[1]), LONG_EDGE_PX);
                let out = dir.path().join(format!(
                    "{}-{}.png",
                    layout.label().to_lowercase(),
                    direction.label().to_lowercase()
                ));
                scene.write_png(&out).unwrap();
                let img = image::open(&out).unwrap();
                assert_eq!(
                    (img.width() as usize, img.height() as usize),
                    (expected[0], expected[1])
                );
            }
        }
    }

    #[test]
    fn vertical_text_direction_renders_distinct_card_and_handles_edge_cases() {
        let ctx = test_ctx();
        let th = &theme::THEMES[0];
        let base = QuoteCard {
            quote: "世上本没有路，走的人多了，也便成了路。\n希望是附丽于存在的，有存在，便有希望。"
                .to_owned(),
            title: "呐喊".to_owned(),
            author: "鲁迅".to_owned(),
            date: "Nov 28, 2026".to_owned(),
            layout: QuoteLayout::Square,
            direction: TextDirection::Horizontal,
        };
        for layout in QuoteLayout::ALL {
            let mut horiz = base.clone();
            horiz.layout = layout;
            let mut vert = horiz.clone();
            vert.direction = TextDirection::Vertical;
            let h_img =
                render_scene(&ctx, &horiz, th, None, false, None, None, None).to_color_image();
            let v_img =
                render_scene(&ctx, &vert, th, None, false, None, None, None).to_color_image();
            assert_eq!(h_img.size, v_img.size);
            assert_ne!(
                h_img.pixels, v_img.pixels,
                "vertical text must render distinctly from horizontal text for {layout:?}"
            );
        }

        // Edge cases: empty attribution, pure Latin, and extraordinarily long text (overflow cap).
        let edge_cards = [
            QuoteCard {
                quote: "凡是过去，皆为序章。".to_owned(),
                title: String::new(),
                author: String::new(),
                date: String::new(),
                layout: QuoteLayout::Square,
                direction: TextDirection::Vertical,
            },
            QuoteCard {
                quote: "We read to know we are not alone.".to_owned(),
                title: "Shadowlands".to_owned(),
                author: "C. S. Lewis".to_owned(),
                date: "Nov 28, 2026".to_owned(),
                layout: QuoteLayout::Horizontal,
                direction: TextDirection::Vertical,
            },
            QuoteCard {
                quote: "人是为活着本身而活着的，而不是为了活着之外的任何事物所活着。".repeat(30),
                title: "一个非常非常长的书名测试竖排两栏截断显示效果".to_owned(),
                author: "很长很长的作者名称测试".to_owned(),
                date: "Nov 28, 2026".to_owned(),
                layout: QuoteLayout::Horizontal,
                direction: TextDirection::Vertical,
            },
        ];
        for card in &edge_cards {
            let scene = render_scene(&ctx, card, th, None, false, None, None, None);
            assert_eq!(scene.size(), card.layout.pixel_size());
        }
    }

    #[test]
    fn balanced_wrapping_avoids_orphans_and_leading_punctuation() {
        let wrap = |s: &str, max_chars: usize| -> Vec<String> {
            let chars: Vec<char> = s.chars().collect();
            let advs = vec![10.0; chars.len()];
            wrap_balanced_spans(&chars, &advs, max_chars as f32 * 10.0)
                .into_iter()
                .map(|(a, b)| chars[a..b].iter().collect())
                .collect()
        };

        // Never start a line with NO_START punctuation, never strand NO_END at the end,
        // and never leave a 1-character orphan on the final line.
        let lines = wrap("天地玄。黄宇", 4);
        assert_eq!(lines, vec!["天地玄。", "黄宇"]);

        // Opening bracket '「' (NO_END) must not be stranded at the end of a line.
        let lines2 = wrap("天地「玄黄", 3);
        assert_eq!(lines2, vec!["天地", "「玄黄"]);

        // Real literary quotes: no `的，` / `——` at start, no `界。`/`路。` orphan,
        // and no middle/earlier line shorter than the line after it.
        for (text, max_chars) in [
            ("一个人只拥有此生此世是不够的，他还应该拥有诗意的世界。", 13),
            ("世上本没有路，走的人多了，也便成了路。", 8),
            (
                "「我只是一朵云，飘过你的天空，偶尔投影在你的波心——你不必讶异，更无须欢喜，在转瞬间消灭了踪影。」",
                9,
            ),
            (
                "我知道黄昏正在转瞬即逝，黑夜从天而降了。我看到广阔的土地袒露着结实的胸膛，那是召唤的姿态，就像女人召唤着她们的儿女，土地召唤着黑夜来临。",
                16,
            ),
        ] {
            let wrapped = wrap(text, max_chars);
            for (idx, line) in wrapped.iter().enumerate() {
                let cs: Vec<char> = line.chars().collect();
                let real = cs.iter().filter(|&&c| is_content_char(c)).count();
                assert!(real >= 2, "orphan in {wrapped:?}");
                assert!(!layout::NO_START.contains(cs[0]), "{wrapped:?}");
                if idx > 0 {
                    assert_ne!(cs[0], '—', "em-dash right at start/top in {wrapped:?}");
                    if cs.len() >= 2 {
                        assert!(
                            !is_closing_or_pause_punct(cs[1]),
                            "leading punctuation in {wrapped:?}"
                        );
                    }
                }
                if idx + 1 < wrapped.len() {
                    let next_len = wrapped[idx + 1].chars().count();
                    assert!(
                        cs.len() >= next_len,
                        "line {} longer than line {idx} in {wrapped:?}",
                        idx + 1
                    );
                }
            }
        }
    }

    #[test]
    fn independent_theme_and_font_selection_in_quote_dialog() {
        let reader_th = &theme::THEMES[theme::index("ecru")];
        let mocha_th = &theme::THEMES[theme::index("catppuccin-mocha")];

        // Default follows the reader theme.
        let mut dlg = QuoteDialog::new(
            "世上本没有路。".to_owned(),
            "呐喊".to_owned(),
            "鲁迅".to_owned(),
            QuoteLayout::Square,
            TextDirection::Horizontal,
            None,
            None,
            None,
        );
        assert_eq!(dlg.resolved_theme(reader_th).id, reader_th.id);

        // Overriding theme renders in the chosen theme independently of the reader theme.
        dlg.theme_id = Some(mocha_th.id.to_owned());
        assert_eq!(dlg.resolved_theme(reader_th).id, mocha_th.id);

        // Unknown theme id falls back safely to the reader theme.
        dlg.theme_id = Some("nonexistent-theme".to_owned());
        assert_eq!(dlg.resolved_theme(reader_th).id, reader_th.id);

        // Rendering the same card in Ecru vs Catppuccin Mocha yields distinct pixels.
        let ctx = test_ctx();
        let ecru_scene = render_scene(&ctx, &dlg.card, reader_th, None, false, None, None, None);
        let mocha_scene = render_scene(
            &ctx,
            &dlg.card,
            mocha_th,
            None,
            false,
            Some(fonts::BUNDLED),
            Some(TITLE_SAME_AS_QUOTE),
            None,
        );
        assert_ne!(
            ecru_scene.pixel(pos2(10.0, 10.0)),
            mocha_scene.pixel(pos2(10.0, 10.0)),
            "choosing a different card theme must change the exported PNG colours"
        );
    }

    #[test]
    fn opening_quote_dialog_preserves_cached_page_glyph_uvs() {
        let ctx = egui::Context::default();
        let (defs, installed) = fonts::definitions(&[], "", "", 1);
        ctx.set_fonts(defs);

        // First frame activates the font atlas and lays out a cached reader galley.
        let mut cached_latin: Option<Arc<egui::Galley>> = None;
        let sample_latin = "The tunnel to summer, the exit of goodbyes.";
        let sample_cjk = "怎么回事？追问得这么紧，跟她在学校时的个";
        let serif_id = FontId::new(18.0, installed.serif.clone());
        let mut out = ctx.run_ui(Default::default(), |ui| {
            cached_latin = Some(ui.painter().layout_no_wrap(
                sample_latin.to_owned(),
                serif_id.clone(),
                Color32::BLACK,
            ));
        });
        out.textures_delta.clear();
        let uvs = |g: &egui::Galley| {
            g.rows
                .iter()
                .flat_map(|r| r.glyphs.iter().map(|gl| gl.uv_rect))
                .collect::<Vec<_>>()
        };
        let before_latin = uvs(cached_latin.as_ref().expect("latin galley"));
        assert!(before_latin.len() > 20);
        assert!(before_latin.iter().any(|u| *u != before_latin[0]));

        // Open the Export quote modal, switch themes, fonts, and text direction across frames,
        // and confirm the live context's font atlas is never wiped/invalidated.
        let mut dlg = QuoteDialog::new(
            sample_cjk.to_owned(),
            "通往夏天的隧道，再见的出口".to_owned(),
            "八目迷".to_owned(),
            QuoteLayout::Square,
            TextDirection::Horizontal,
            None,
            None,
            None,
        );
        let th = &theme::THEMES[0];
        let mut fresh_latin: Option<Arc<egui::Galley>> = None;
        type DialogConfig<'a> = (
            Option<&'a str>,
            Option<&'a str>,
            Option<&'a str>,
            TextDirection,
        );
        let configs: [DialogConfig<'_>; 3] = [
            (None, None, None, TextDirection::Horizontal),
            (
                Some("catppuccin-mocha"),
                Some(fonts::BUNDLED),
                Some(TITLE_SAME_AS_QUOTE),
                TextDirection::Vertical,
            ),
            (
                Some("gruvbox-dark"),
                None,
                Some(fonts::BUNDLED),
                TextDirection::Horizontal,
            ),
        ];
        for (theme_id, quote_font, title_font, direction) in configs {
            dlg.theme_id = theme_id.map(str::to_owned);
            dlg.quote_font = quote_font.map(str::to_owned);
            dlg.title_font = title_font.map(str::to_owned);
            dlg.card.direction = direction;
            let mut out = ctx.run_ui(Default::default(), |ui| {
                dlg.ui(ui.ctx(), th, Some(&installed), false);
                fresh_latin = Some(ui.painter().layout_no_wrap(
                    sample_latin.to_owned(),
                    serif_id.clone(),
                    Color32::BLACK,
                ));
            });
            out.textures_delta.clear();
        }
        assert!(dlg.preview.is_some(), "offscreen preview texture generated");
        assert_eq!(
            before_latin,
            uvs(fresh_latin.as_ref().expect("fresh latin")),
            "cached reader glyph UVs must remain valid after opening QuoteDialog and changing theme/fonts"
        );
    }

    #[test]
    fn every_theme_renders_distinct_card_and_backdrop() {
        let ctx = test_ctx();
        let card = QuoteCard {
            quote: "世上本没有路，走的人多了，也便成了路。".to_owned(),
            title: "呐喊".to_owned(),
            author: "鲁迅".to_owned(),
            date: "Nov 27, 2026".to_owned(),
            layout: QuoteLayout::Square,
            direction: TextDirection::Horizontal,
        };
        for th in &theme::THEMES {
            let pal = CardPalette::from_theme(th);
            assert_ne!(
                pal.mat_tl, th.page,
                "theme {} must distinguish mat from card",
                th.id
            );
            let scene = render_scene(&ctx, &card, th, None, false, None, None, None);
            assert_eq!(scene.size(), [1024, 1024], "theme {}", th.id);
        }
    }

    /// Optional visual dump when `QUOTE_PREVIEW_OUT` is set: writes sample PNGs
    /// across layouts, text directions and themes so they can be inspected directly.
    #[test]
    fn dump_sample_quote_cards_when_requested() {
        let Some(dir) = std::env::var_os("QUOTE_PREVIEW_OUT") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = test_ctx();
        let samples: &[(&str, &str, QuoteLayout, TextDirection, &str, &str, &str)] = &[
            (
                "square-solarized",
                "solarized-light",
                QuoteLayout::Square,
                TextDirection::Horizontal,
                "人是为活着本身而活着的，而不是为了活着之外的任何事物所活着。\n我知道黄昏正在转瞬即逝，黑夜从天而降了。我看到广阔的土地袒露着结实的胸膛，那是召唤的姿态，就像女人召唤着她们的儿女，土地召唤着黑夜来临。",
                "活着",
                "余华",
            ),
            (
                "horizontal-mocha",
                "catppuccin-mocha",
                QuoteLayout::Horizontal,
                TextDirection::Horizontal,
                "世上本没有路，走的人多了，也便成了路。",
                "呐喊",
                "鲁迅",
            ),
            (
                "vertical-rose-pine-dawn",
                "rose-pine-dawn",
                QuoteLayout::Vertical,
                TextDirection::Horizontal,
                "我只是一朵云，飘过你的天空，偶尔投影在你的波心——你不必讶异，更无须欢喜，在转瞬间消灭了踪影。",
                "偶然",
                "徐志摩",
            ),
            (
                "vtext-square-solarized",
                "solarized-light",
                QuoteLayout::Square,
                TextDirection::Vertical,
                "人是为活着本身而活着的，而不是为了活着之外的任何事物所活着。\n我知道黄昏正在转瞬即逝，黑夜从天而降了。我看到广阔的土地袒露着结实的胸膛，那是召唤的姿态，就像女人召唤着她们的儿女，土地召唤着黑夜来临。",
                "活着",
                "余华",
            ),
            (
                "vtext-square-short-ecru",
                "ecru",
                QuoteLayout::Square,
                TextDirection::Vertical,
                "凡是过去，皆为序章。",
                "暴风雨",
                "威廉·莎士比亚",
            ),
            (
                "vtext-horizontal-mocha",
                "catppuccin-mocha",
                QuoteLayout::Horizontal,
                TextDirection::Vertical,
                "世上本没有路，走的人多了，也便成了路。",
                "呐喊",
                "鲁迅",
            ),
            (
                "vtext-horizontal-long-rose-pine",
                "rose-pine-dawn",
                QuoteLayout::Horizontal,
                TextDirection::Vertical,
                "「我只是一朵云，飘过你的天空，偶尔投影在你的波心——你不必讶异，更无须欢喜，在转瞬间消灭了踪影。」",
                "通往夏天的隧道，再见的出口",
                "徐志摩",
            ),
            (
                "vtext-vertical-gruvbox-dark",
                "gruvbox-dark",
                QuoteLayout::Vertical,
                TextDirection::Vertical,
                "一个人只拥有此生此世是不够的，他还应该拥有诗意的世界。",
                "万寿寺",
                "王小波",
            ),
            (
                "vtext-horizontal-latin-ecru",
                "ecru",
                QuoteLayout::Horizontal,
                TextDirection::Vertical,
                "We read to know we are not alone.",
                "Shadowlands",
                "C. S. Lewis",
            ),
        ];
        for &(name, theme_id, layout, direction, quote, title, author) in samples {
            let th = &theme::THEMES[theme::index(theme_id)];
            let card = QuoteCard {
                quote: quote.to_owned(),
                title: title.to_owned(),
                author: author.to_owned(),
                date: "Nov 28, 2026".to_owned(),
                layout,
                direction,
            };
            let scene = render_scene(&ctx, &card, th, None, false, None, None, None);
            scene.write_png(&dir.join(format!("{name}.png"))).unwrap();
        }
    }
}
