//! The reader's colour themes: the page, the desk around it, the bar and
//! menus, and what the book's own colours become on a dark page.
//!
//! Palettes: Solarized (Ethan Schoonover); Catppuccin 1.8.0 and Rosé Pine as
//! published in their `palette.json`; Ecru from ttsu reader's
//! `theme-option.ts`; Gruvbox, Nord and Tokyo Night from their authors'
//! palettes. Paper and Black are hanstat's own.
//!
//! The theme in use is kept in the egui context, so each window (and each
//! test's context) has its own.

use eframe::egui::{self, Color32, Stroke};

pub(super) struct Theme {
    /// Saved in the reader's settings.
    pub(super) id: &'static str,
    pub(super) name: &'static str,
    pub(super) dark: bool,
    /// The sheet of paper.
    pub(super) page: Color32,
    /// Around the pages.
    pub(super) desk: Color32,
    /// The top bar and panels.
    pub(super) bar: Color32,
    /// Body text.
    pub(super) ink: Color32,
    /// Interface text: the title, menus, labels.
    pub(super) text: Color32,
    /// Secondary labels.
    pub(super) subtle: Color32,
    /// Running heads, folios, hints.
    pub(super) muted: Color32,
    /// Links and selection.
    pub(super) accent: Color32,
    /// The word looked up (see-through).
    pub(super) yellow: Color32,
    /// The sentence read aloud (see-through).
    pub(super) cyan: Color32,
    pub(super) red: Color32,
    pub(super) orange: Color32,
}

const fn hex(x: u32) -> Color32 {
    Color32::from_rgb((x >> 16) as u8, (x >> 8) as u8, x as u8)
}

/// `page, desk, bar, ink, text, subtle, muted, accent, yellow, cyan, red, orange`.
const fn theme(id: &'static str, name: &'static str, dark: bool, c: [u32; 12]) -> Theme {
    Theme {
        id,
        name,
        dark,
        page: hex(c[0]),
        desk: hex(c[1]),
        bar: hex(c[2]),
        ink: hex(c[3]),
        text: hex(c[4]),
        subtle: hex(c[5]),
        muted: hex(c[6]),
        accent: hex(c[7]),
        yellow: hex(c[8]),
        cyan: hex(c[9]),
        red: hex(c[10]),
        orange: hex(c[11]),
    }
}

/// Light themes first, then dark; the chooser lists them in this order.
///
/// Every colour is the palette's own, unchanged (some are low in contrast by
/// design); only which palette colour plays which part is chosen here.
#[rustfmt::skip]
pub(super) static THEMES: [Theme; 16] = [
    // Solarized: base3 page, base2 desk and bar; base00 body text, base01
    // emphasised (the title, menus), base1 secondary.
    theme("solarized-light", "Solarized Light", false, [
        0xfdf6e3, 0xeee8d5, 0xeee8d5, 0x657b83, 0x586e75, 0x657b83,
        0x93a1a1, 0x268bd2, 0xb58900, 0x2aa198, 0xdc322f, 0xcb4b16,
    ]),
    // ttsu reader's Ecru: its page everywhere, its 87% black text, 60%
    // black tooltips and 33% black hints (laid over the page), its grey
    // selection for links. It has no other colours: the marks and warnings
    // are Paper's.
    theme("ecru", "Ecru", false, [
        0xf7f6eb, 0xf7f6eb, 0xf7f6eb, 0x20201f, 0x20201f, 0x63625e,
        0xa5a59d, 0x979797, 0xf0b400, 0x2a9d9d, 0xcf222e, 0xbc4c00,
    ]),
    // hanstat's own: near-white paper, near-black ink.
    theme("paper", "Paper", false, [
        0xfdfdfc, 0xe6e6e3, 0xf1f1ef, 0x1b1b1b, 0x3a3a3a, 0x5c5c5c,
        0x8c8c8c, 0x1f6feb, 0xf0b400, 0x2a9d9d, 0xcf222e, 0xbc4c00,
    ]),
    // Catppuccin: base, crust, mantle, text, text, subtext1, overlay1;
    // blue, yellow, teal, red, peach.
    theme("catppuccin-latte", "Catppuccin Latte", false, [
        0xeff1f5, 0xdce0e8, 0xe6e9ef, 0x4c4f69, 0x4c4f69, 0x5c5f77,
        0x8c8fa1, 0x1e66f5, 0xdf8e1d, 0x179299, 0xd20f39, 0xfe640b,
    ]),
    // Rosé Pine Dawn: base page, overlay desk and bar; text, text, subtle,
    // muted; pine, gold, foam, love, rose.
    theme("rose-pine-dawn", "Rosé Pine Dawn", false, [
        0xfaf4ed, 0xf2e9e1, 0xf2e9e1, 0x464261, 0x464261, 0x797593,
        0x9893a5, 0x286983, 0xea9d34, 0x56949f, 0xb4637a, 0xd7827e,
    ]),
    // Gruvbox light: bg0, bg1, bg0_s, fg1, fg1, fg3, gray; faded blue,
    // neutral yellow and aqua, faded red and orange.
    theme("gruvbox-light", "Gruvbox Light", false, [
        0xfbf1c7, 0xebdbb2, 0xf2e5bc, 0x3c3836, 0x3c3836, 0x665c54,
        0x928374, 0x076678, 0xd79921, 0x689d6a, 0x9d0006, 0xaf3a03,
    ]),
    // Solarized: base03 page, base02 desk and bar; base0 body text, base1
    // emphasised, base01 secondary.
    theme("solarized-dark", "Solarized Dark", true, [
        0x002b36, 0x073642, 0x073642, 0x839496, 0x93a1a1, 0x839496,
        0x586e75, 0x268bd2, 0xb58900, 0x2aa198, 0xdc322f, 0xcb4b16,
    ]),
    theme("catppuccin-frappe", "Catppuccin Frappé", true, [
        0x303446, 0x232634, 0x292c3c, 0xc6d0f5, 0xc6d0f5, 0xb5bfe2,
        0x838ba7, 0x8caaee, 0xe5c890, 0x81c8be, 0xe78284, 0xef9f76,
    ]),
    theme("catppuccin-macchiato", "Catppuccin Macchiato", true, [
        0x24273a, 0x181926, 0x1e2030, 0xcad3f5, 0xcad3f5, 0xb8c0e0,
        0x8087a2, 0x8aadf4, 0xeed49f, 0x8bd5ca, 0xed8796, 0xf5a97f,
    ]),
    theme("catppuccin-mocha", "Catppuccin Mocha", true, [
        0x1e1e2e, 0x11111b, 0x181825, 0xcdd6f4, 0xcdd6f4, 0xbac2de,
        0x7f849c, 0x89b4fa, 0xf9e2af, 0x94e2d5, 0xf38ba8, 0xfab387,
    ]),
    // Rosé Pine: base page, surface desk and bar; text, text, subtle,
    // muted; iris, gold, foam, love, rose.
    theme("rose-pine", "Rosé Pine", true, [
        0x191724, 0x1f1d2e, 0x1f1d2e, 0xe0def4, 0xe0def4, 0x908caa,
        0x6e6a86, 0xc4a7e7, 0xf6c177, 0x9ccfd8, 0xeb6f92, 0xebbcba,
    ]),
    theme("rose-pine-moon", "Rosé Pine Moon", true, [
        0x232136, 0x2a273f, 0x2a273f, 0xe0def4, 0xe0def4, 0x908caa,
        0x6e6a86, 0xc4a7e7, 0xf6c177, 0x9ccfd8, 0xeb6f92, 0xea9a97,
    ]),
    // Gruvbox dark: bg0, bg0_h, bg0_s, fg1, fg1, fg2, gray; bright blue,
    // yellow, aqua, red, orange.
    theme("gruvbox-dark", "Gruvbox Dark", true, [
        0x282828, 0x1d2021, 0x32302f, 0xebdbb2, 0xebdbb2, 0xd5c4a1,
        0x928374, 0x83a598, 0xfabd2f, 0x8ec07c, 0xfb4934, 0xfe8019,
    ]),
    // Nord: nord0 page, nord1 desk and bar; nord4 text, nord5 emphasised,
    // nord3 secondary; nord8, nord13, nord7, nord11, nord12.
    theme("nord", "Nord", true, [
        0x2e3440, 0x3b4252, 0x3b4252, 0xd8dee9, 0xe5e9f0, 0xd8dee9,
        0x4c566a, 0x88c0d0, 0xebcb8b, 0x8fbcbb, 0xbf616a, 0xd08770,
    ]),
    // Tokyo Night (night): bg page, bg_dark desk and bar; fg, fg, fg_dark,
    // comment; blue, yellow, cyan, red, orange.
    theme("tokyo-night", "Tokyo Night", true, [
        0x1a1b26, 0x16161e, 0x16161e, 0xc0caf5, 0xc0caf5, 0xa9b1d6,
        0x565f89, 0x7aa2f7, 0xe0af68, 0x7dcfff, 0xf7768e, 0xff9e64,
    ]),
    // hanstat's own, for OLED screens and dark rooms: black, with grey ink
    // that doesn't glare.
    theme("black", "Black", true, [
        0x000000, 0x000000, 0x111111, 0xbdbdbd, 0xc8c8c8, 0x9e9e9e,
        0x6e6e6e, 0x6ea8fe, 0xe0b000, 0x4fb3b3, 0xff6b6b, 0xff9f43,
    ]),
];

/// The index of the theme saved as `id` (the first, Solarized Light, if
/// none is: a theme removed in a later release, or a hand-edited file).
pub(super) fn index(id: &str) -> usize {
    THEMES.iter().position(|t| t.id == id).unwrap_or(0)
}

fn key() -> egui::Id {
    egui::Id::unique("reader-theme")
}

/// The theme in use in `ctx`.
pub(super) fn get(ctx: &egui::Context) -> &'static Theme {
    &THEMES[ctx.data(|d| d.get_temp::<usize>(key())).unwrap_or(0) % THEMES.len()]
}

/// Use theme `i` in `ctx`.
pub(super) fn set(ctx: &egui::Context, i: usize) {
    ctx.data_mut(|d| d.insert_temp(key(), i % THEMES.len()));
}

/// `fg` laid over `bg` at `a`, as an opaque colour.
pub(super) fn mix(fg: Color32, bg: Color32, a: f32) -> Color32 {
    let m = |f: u8, b: u8| (f as f32 * a + b as f32 * (1.0 - a)).round() as u8;
    Color32::from_rgb(m(fg.r(), bg.r()), m(fg.g(), bg.g()), m(fg.b(), bg.b()))
}

fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG contrast ratio of two opaque colours.
fn contrast(a: Color32, b: Color32) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// The colour with its HSL lightness turned over (hue, saturation and alpha
/// kept): a book's dark blue becomes a light blue, a pale grey box a dark one.
fn flip(c: Color32) -> Color32 {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    let [r, g, b] = [r, g, b].map(|v| v as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    let (h, s) = if d == 0.0 {
        (0.0, 0.0)
    } else {
        let s = d / (1.0 - (2.0 * l - 1.0).abs());
        let h = if max == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        (h * 60.0, s)
    };
    let l = 1.0 - l;
    let c2 = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c2 * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c2 / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c2, x, 0.0),
        1 => (x, c2, 0.0),
        2 => (0.0, c2, x),
        3 => (0.0, x, c2),
        4 => (x, 0.0, c2),
        _ => (c2, 0.0, x),
    };
    let to = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(to(r), to(g), to(b), a)
}

/// A book's text colour, readable on `page`: on a dark page, colours that
/// would be hard to read there (the book's dark grey or navy) turn light.
pub(super) fn book_text(c: Color32, page: Color32, dark: bool) -> Color32 {
    if !dark || c.a() == 0 {
        return c;
    }
    let opaque = Color32::from_rgb(c.r(), c.g(), c.b());
    let before = contrast(opaque, page);
    if before >= 3.0 || contrast(flip(opaque), page) <= before {
        c
    } else {
        flip(c)
    }
}

/// A book's background: on a dark page, light boxes turn dark.
pub(super) fn book_fill(c: Color32, dark: bool) -> Color32 {
    if dark { flip(c) } else { c }
}

impl Theme {
    /// The word looked up.
    /// A search match on the page (see-through).
    pub(super) fn search_hit(&self) -> Color32 {
        self.orange
            .gamma_multiply(if self.dark { 0.32 } else { 0.38 })
    }

    pub(super) fn highlight(&self) -> Color32 {
        let [r, g, b, _] = self.yellow.to_array();
        Color32::from_rgba_unmultiplied(r, g, b, if self.dark { 80 } else { 96 })
    }

    /// The sentence read aloud.
    pub(super) fn reading(&self) -> Color32 {
        let [r, g, b, _] = self.cyan.to_array();
        Color32::from_rgba_unmultiplied(r, g, b, if self.dark { 60 } else { 56 })
    }

    /// The shade toward a spread's fold, at full strength.
    pub(super) fn fold(&self) -> (Color32, f32) {
        if self.dark {
            (Color32::BLACK, 40.0)
        } else {
            (mix(self.ink, self.desk, 0.35), 16.0)
        }
    }

    /// The bar, menus and windows while the reader is open.
    pub(super) fn visuals(&self) -> egui::Visuals {
        let mut v = if self.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        let toward = |a| mix(self.text, self.bar, a);
        let (fill, hover, active) = if self.dark {
            (toward(0.09), toward(0.15), toward(0.22))
        } else {
            (toward(0.06), toward(0.11), toward(0.17))
        };
        v.panel_fill = self.bar;
        v.window_fill = self.page;
        v.extreme_bg_color = if self.dark { self.desk } else { self.page };
        v.faint_bg_color = mix(self.bar, self.page, 0.5);
        v.code_bg_color = self.bar;
        v.hyperlink_color = self.accent;
        v.override_text_color = Some(self.text);
        v.window_stroke = Stroke::new(1.0, toward(0.14));
        let [r, g, b, _] = self.accent.to_array();
        v.selection.bg_fill =
            Color32::from_rgba_unmultiplied(r, g, b, if self.dark { 110 } else { 90 });
        v.selection.stroke = Stroke::new(1.0, self.accent);
        v.widgets.noninteractive.bg_fill = self.page;
        v.widgets.noninteractive.weak_bg_fill = self.page;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, mix(self.text, self.page, 0.12));
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, self.text);
        for (w, c) in [
            (&mut v.widgets.inactive, fill),
            (&mut v.widgets.hovered, hover),
            (&mut v.widgets.active, active),
            (&mut v.widgets.open, hover),
        ] {
            w.bg_fill = c;
            w.weak_bg_fill = c;
            w.bg_stroke = Stroke::NONE;
            w.fg_stroke = Stroke::new(1.0, self.ink);
        }
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, self.muted);
        v
    }
}

/// One theme in the chooser: a strip of its page with its name in its ink,
/// and dots of its link, highlight and read-aloud colours.
pub(super) fn swatch(ui: &mut egui::Ui, t: &Theme, selected: bool) -> egui::Response {
    swatch_as(ui, t, t.name, selected)
}

/// [`swatch`] with another name on it.
pub(super) fn swatch_as(
    ui: &mut egui::Ui,
    t: &Theme,
    name: &str,
    selected: bool,
) -> egui::Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 30.0), egui::Sense::click());
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), selected, name)
    });
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let r = rect.shrink2(egui::vec2(1.0, 2.0));
        p.rect_filled(r, 5.0, t.page);
        let outline = if selected {
            Stroke::new(2.0, t.accent)
        } else if resp.hovered() {
            Stroke::new(1.5, t.muted)
        } else {
            Stroke::new(1.0, mix(t.ink, t.page, 0.15))
        };
        p.rect_stroke(r, 5.0, outline, egui::StrokeKind::Inside);
        p.text(
            r.left_center() + egui::vec2(10.0, 0.0),
            egui::Align2::LEFT_CENTER,
            name,
            egui::FontId::proportional(14.0),
            t.ink,
        );
        for (k, c) in [t.accent, t.yellow, t.cyan].into_iter().enumerate() {
            p.circle_filled(
                r.right_center() - egui::vec2(38.0 - 13.0 * k as f32, 0.0),
                4.5,
                c,
            );
        }
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_known() {
        for (i, t) in THEMES.iter().enumerate() {
            assert_eq!(index(t.id), i, "{} listed twice", t.id);
        }
        assert_eq!(index("no-such-theme"), 0);
        assert_eq!(THEMES[0].id, "solarized-light", "the default stays first");
        let darks: Vec<bool> = THEMES.iter().map(|t| t.dark).collect();
        assert!(darks.is_sorted(), "light themes before dark ones");
    }

    #[test]
    fn dark_flag_matches_the_page() {
        for t in &THEMES {
            assert_eq!(t.dark, luminance(t.page) < 0.2, "{}", t.id);
        }
    }

    #[test]
    fn flip_turns_lightness_over() {
        assert_eq!(flip(Color32::BLACK), Color32::WHITE);
        assert_eq!(flip(Color32::WHITE), Color32::BLACK);
        assert_eq!(flip(hex(0xeeeeee)), hex(0x111111));
        // Navy becomes a light blue of the same hue.
        let navy = flip(hex(0x000080));
        assert!(
            navy.b() > 200 && navy.r() == navy.g() && navy.r() > 100,
            "{navy:?}"
        );
        // Alpha is kept.
        assert_eq!(flip(Color32::from_rgba_unmultiplied(0, 0, 0, 128)).a(), 128);
    }

    #[test]
    fn book_colours_on_dark_pages() {
        let mocha = &THEMES[index("catppuccin-mocha")];
        let latte = &THEMES[index("catppuccin-latte")];
        let navy = hex(0x000080);
        assert_eq!(
            book_text(navy, latte.page, latte.dark),
            navy,
            "light pages keep the book's colours"
        );
        assert!(contrast(book_text(navy, mocha.page, mocha.dark), mocha.page) >= 4.5);
        let red = hex(0xff4040);
        assert_eq!(
            book_text(red, mocha.page, mocha.dark),
            red,
            "readable colours stay"
        );
        assert!(luminance(book_fill(hex(0xeeeeee), mocha.dark)) < 0.02);
    }
}
