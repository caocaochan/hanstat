//! Shareable quote-card PNG export for selected text in the reader.
//!
//! Renders a bookplate-inspired literary card in the current reader theme's
//! colours across Square (`1024×1024`), Horizontal (`1024×640`) and Vertical
//! (`640×1024`) layouts. The same painter draws both the live preview in the
//! export window and the offscreen PNG via [`crate::raster`].

use super::fonts;
use super::theme::{self, Theme};
use eframe::egui::{
    self, Align, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Pos2, Rect,
    RichText, Sense, Stroke, StrokeKind, Vec2,
    epaint::{Mesh, PathShape, Shape, Vertex},
    pos2,
    text::LayoutJob,
    vec2,
};
use serde::{Deserialize, Serialize};
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

/// Content and layout of a quote card.
#[derive(Clone, Debug)]
pub(super) struct QuoteCard {
    pub(super) quote: String,
    pub(super) title: String,
    pub(super) author: String,
    pub(super) date: String,
    pub(super) layout: QuoteLayout,
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
    pub(super) fn new(
        quote: String,
        title: String,
        author: String,
        layout: QuoteLayout,
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
        egui::Window::new("Export quote")
            .id(egui::Id::unique("reader-quote-export"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(440.0);
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
                            let col_w = super::THEME_COLUMN;
                            let w = 2.0 * col_w + ui.spacing().item_spacing.x;
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
                                            RichText::new(if dark { "Dark" } else { "Light" })
                                                .size(12.0)
                                                .color(reader_th.muted),
                                        );
                                        for (i, t) in theme::THEMES
                                            .iter()
                                            .enumerate()
                                            .filter(|(_, t)| t.dark == dark)
                                        {
                                            let r =
                                                theme::swatch(ui, t, Some(i) == current_override);
                                            if r.clicked() {
                                                self.theme_id = Some(t.id.to_owned());
                                                ui.close();
                                            }
                                        }
                                    });
                                }
                            });
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
    let quote_fam = quote_font
        .and_then(|name| register_custom_family(&mut defs, "quote-body-custom", name, fams))
        .unwrap_or_else(|| quote_font_family(reader_fonts, sans_default));
    let title_fam = match title_font {
        Some(TITLE_SAME_AS_QUOTE) => quote_fam.clone(),
        Some(name) => register_custom_family(&mut defs, "quote-title-custom", name, fams)
            .unwrap_or_else(title_font_family),
        None => title_font_family(),
    };
    let meta_fam = FontFamily::Proportional;
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
            paint_quote_card(&painter, rect, &card, th, &quote_fam, &title_fam, &meta_fam);
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

fn quote_font_family(reader_fonts: Option<&fonts::Installed>, sans_default: bool) -> FontFamily {
    match reader_fonts {
        Some(f) if sans_default => f.sans.clone(),
        Some(f) => f.serif.clone(),
        None => title_font_family(),
    }
}

fn register_custom_family(
    defs: &mut FontDefinitions,
    slot_key: &str,
    family_name: &str,
    fams: Option<&[fonts::FamilyInfo]>,
) -> Option<FontFamily> {
    if family_name.eq_ignore_ascii_case(fonts::BUNDLED) {
        return Some(title_font_family());
    }
    let (blob, index) = fonts::load_family_face(fams?, family_name)?;
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
    Some(fam)
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

/// Paint the entire quote card into `rect`, scaling proportionally from its
/// logical size so preview and 1024px PNG are identical.
fn paint_quote_card(
    painter: &egui::Painter,
    rect: Rect,
    card: &QuoteCard,
    th: &Theme,
    quote_family: &FontFamily,
    title_family: &FontFamily,
    meta_family: &FontFamily,
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

    // 4. Layout-specific editorial composition inside `frame_rect`.
    match card.layout {
        QuoteLayout::Square => {
            paint_square_content(
                painter,
                frame_rect,
                card,
                &pal,
                quote_family,
                title_family,
                meta_family,
                s,
            );
        }
        QuoteLayout::Horizontal => {
            paint_horizontal_content(
                painter,
                frame_rect,
                card,
                &pal,
                quote_family,
                title_family,
                meta_family,
                s,
            );
        }
        QuoteLayout::Vertical => {
            paint_vertical_content(
                painter,
                frame_rect,
                card,
                &pal,
                quote_family,
                title_family,
                meta_family,
                s,
            );
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
        let mut galleys = Vec::with_capacity(paras.len());
        let mut total_h = 0.0f32;
        let mut rows_left = cap_rows;
        for (i, &para) in paras.iter().enumerate() {
            if let Some(0) = rows_left {
                break;
            }
            let mut job = LayoutJob::default();
            job.wrap.max_width = zone.width().max(20.0);
            job.halign = align;
            if let Some(r) = rows_left {
                job.wrap.max_rows = r;
                job.wrap.overflow_character = Some('…');
            }
            job.append(
                para,
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
        };
        let dir = tempfile::tempdir().unwrap();
        for (layout, expected) in [
            (QuoteLayout::Square, [1024, 1024]),
            (QuoteLayout::Horizontal, [1024, 640]),
            (QuoteLayout::Vertical, [640, 1024]),
        ] {
            card.layout = layout;
            let scene = render_scene(&ctx, &card, th, None, false, None, None, None);
            assert_eq!(scene.size(), expected, "{layout:?}");
            assert_eq!(scene.size()[0].max(scene.size()[1]), LONG_EDGE_PX);
            let out = dir
                .path()
                .join(format!("{}.png", layout.label().to_lowercase()));
            scene.write_png(&out).unwrap();
            let img = image::open(&out).unwrap();
            assert_eq!(
                (img.width() as usize, img.height() as usize),
                (expected[0], expected[1])
            );
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

        // Open the Export quote modal, switch themes and fonts across frames,
        // and confirm the live context's font atlas is never wiped/invalidated.
        let mut dlg = QuoteDialog::new(
            sample_cjk.to_owned(),
            "通往夏天的隧道，再见的出口".to_owned(),
            "八目迷".to_owned(),
            QuoteLayout::Square,
            None,
            None,
            None,
        );
        let th = &theme::THEMES[0];
        let mut fresh_latin: Option<Arc<egui::Galley>> = None;
        let configs: [(Option<&str>, Option<&str>, Option<&str>); 3] = [
            (None, None, None),
            (
                Some("catppuccin-mocha"),
                Some(fonts::BUNDLED),
                Some(TITLE_SAME_AS_QUOTE),
            ),
            (Some("gruvbox-dark"), None, Some(fonts::BUNDLED)),
        ];
        for (theme_id, quote_font, title_font) in configs {
            dlg.theme_id = theme_id.map(str::to_owned);
            dlg.quote_font = quote_font.map(str::to_owned);
            dlg.title_font = title_font.map(str::to_owned);
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
    /// across layouts and themes so they can be inspected directly.
    #[test]
    fn dump_sample_quote_cards_when_requested() {
        let Some(dir) = std::env::var_os("QUOTE_PREVIEW_OUT") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = test_ctx();
        let samples: &[(&str, &str, QuoteLayout, &str, &str, &str)] = &[
            (
                "square-solarized",
                "solarized-light",
                QuoteLayout::Square,
                "人是为活着本身而活着的，而不是为了活着之外的任何事物所活着。\n我知道黄昏正在转瞬即逝，黑夜从天而降了。我看到广阔的土地袒露着结实的胸膛，那是召唤的姿态，就像女人召唤着她们的儿女，土地召唤着黑夜来临。",
                "活着",
                "余华",
            ),
            (
                "horizontal-mocha",
                "catppuccin-mocha",
                QuoteLayout::Horizontal,
                "世上本没有路，走的人多了，也便成了路。",
                "呐喊",
                "鲁迅",
            ),
            (
                "vertical-rose-pine-dawn",
                "rose-pine-dawn",
                QuoteLayout::Vertical,
                "我只是一朵云，飘过你的天空，偶尔投影在你的波心——你不必讶异，更无须欢喜，在转瞬间消灭了踪影。",
                "偶然",
                "徐志摩",
            ),
            (
                "vertical-gruvbox-dark",
                "gruvbox-dark",
                QuoteLayout::Vertical,
                "一个人只拥有此生此世是不够的，他还应该拥有诗意的世界。",
                "万寿寺",
                "王小波",
            ),
            (
                "square-black",
                "black",
                QuoteLayout::Square,
                "凡是过去，皆为序章。",
                "暴风雨",
                "威廉·莎士比亚",
            ),
            (
                "horizontal-ecru",
                "ecru",
                QuoteLayout::Horizontal,
                "We read to know we are not alone. A room without books is like a body without a soul.",
                "Shadowlands",
                "C. S. Lewis",
            ),
        ];
        for &(name, theme_id, layout, quote, title, author) in samples {
            let th = &theme::THEMES[theme::index(theme_id)];
            let card = QuoteCard {
                quote: quote.to_owned(),
                title: title.to_owned(),
                author: author.to_owned(),
                date: "Nov 27, 2026".to_owned(),
                layout,
            };
            let scene = render_scene(&ctx, &card, th, None, false, None, None, None);
            scene.write_png(&dir.join(format!("{name}.png"))).unwrap();
        }
    }
}
