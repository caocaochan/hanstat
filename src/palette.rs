//! The window's look: Catppuccin colours (Mocha on the dark theme, Latte on
//! the light one), the egui style built from them, and the colours of text
//! and marks that read on both themes. On the light theme a colour used for
//! text is a darker counterpart of the Latte accent with a contrast of at
//! least 4.5:1 (WCAG AA for text) against the Latte panels; Latte's own
//! accents are kept for fills. `App::ui` sets the theme once per frame with
//! [`set_dark`]; [`install`] sets the style for both themes at startup.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Response, RichText, Sense, Shadow,
    Stroke, TextStyle, Theme, Ui, vec2,
};
use std::sync::atomic::{AtomicBool, Ordering};

static DARK: AtomicBool = AtomicBool::new(true);

/// Whether the frame being drawn uses the dark theme.
pub(crate) fn set_dark(dark: bool) {
    DARK.store(dark, Ordering::Relaxed);
}

/// Whether the frame being drawn uses the dark theme.
pub(crate) fn dark() -> bool {
    DARK.load(Ordering::Relaxed)
}

/// `dark` on the dark theme, `light` on the light one.
pub(crate) fn pick(dark: Color32, light: Color32) -> Color32 {
    if self::dark() { dark } else { light }
}

/// One Catppuccin flavour (palette v1.8.0).
#[allow(dead_code)]
pub(crate) struct Flavour {
    pub rosewater: Color32,
    pub flamingo: Color32,
    pub pink: Color32,
    pub mauve: Color32,
    pub red: Color32,
    pub maroon: Color32,
    pub peach: Color32,
    pub yellow: Color32,
    pub green: Color32,
    pub teal: Color32,
    pub sky: Color32,
    pub sapphire: Color32,
    pub blue: Color32,
    pub lavender: Color32,
    pub text: Color32,
    pub subtext1: Color32,
    pub subtext0: Color32,
    pub overlay2: Color32,
    pub overlay1: Color32,
    pub overlay0: Color32,
    pub surface2: Color32,
    pub surface1: Color32,
    pub surface0: Color32,
    pub base: Color32,
    pub mantle: Color32,
    pub crust: Color32,
}

const fn hex(x: u32) -> Color32 {
    Color32::from_rgb((x >> 16) as u8, (x >> 8) as u8, x as u8)
}

pub(crate) const MOCHA: Flavour = Flavour {
    rosewater: hex(0xf5e0dc),
    flamingo: hex(0xf2cdcd),
    pink: hex(0xf5c2e7),
    mauve: hex(0xcba6f7),
    red: hex(0xf38ba8),
    maroon: hex(0xeba0ac),
    peach: hex(0xfab387),
    yellow: hex(0xf9e2af),
    green: hex(0xa6e3a1),
    teal: hex(0x94e2d5),
    sky: hex(0x89dceb),
    sapphire: hex(0x74c7ec),
    blue: hex(0x89b4fa),
    lavender: hex(0xb4befe),
    text: hex(0xcdd6f4),
    subtext1: hex(0xbac2de),
    subtext0: hex(0xa6adc8),
    overlay2: hex(0x9399b2),
    overlay1: hex(0x7f849c),
    overlay0: hex(0x6c7086),
    surface2: hex(0x585b70),
    surface1: hex(0x45475a),
    surface0: hex(0x313244),
    base: hex(0x1e1e2e),
    mantle: hex(0x181825),
    crust: hex(0x11111b),
};

pub(crate) const LATTE: Flavour = Flavour {
    rosewater: hex(0xdc8a78),
    flamingo: hex(0xdd7878),
    pink: hex(0xea76cb),
    mauve: hex(0x8839ef),
    red: hex(0xd20f39),
    maroon: hex(0xe64553),
    peach: hex(0xfe640b),
    yellow: hex(0xdf8e1d),
    green: hex(0x40a02b),
    teal: hex(0x179299),
    sky: hex(0x04a5e5),
    sapphire: hex(0x209fb5),
    blue: hex(0x1e66f5),
    lavender: hex(0x7287fd),
    text: hex(0x4c4f69),
    subtext1: hex(0x5c5f77),
    subtext0: hex(0x6c6f85),
    overlay2: hex(0x7c7f93),
    overlay1: hex(0x8c8fa1),
    overlay0: hex(0x9ca0b0),
    surface2: hex(0xacb0be),
    surface1: hex(0xbcc0cc),
    surface0: hex(0xccd0da),
    base: hex(0xeff1f5),
    mantle: hex(0xe6e9ef),
    crust: hex(0xdce0e8),
};

/// Latte accents darkened towards Mocha's crust until they reach 4.5:1
/// against Latte's mantle (the darker of the two panel colours), for text
/// and thin marks on the light theme. The Mocha accents all pass as they are.
pub(crate) mod ink {
    use super::{Color32, hex};
    pub(crate) const RED: Color32 = hex(0xce0f38);
    pub(crate) const MAROON: Color32 = hex(0xb73a47);
    pub(crate) const PEACH: Color32 = hex(0xad4810);
    pub(crate) const YELLOW: Color32 = hex(0x8f5d1c);
    pub(crate) const GREEN: Color32 = hex(0x327526);
    pub(crate) const TEAL: Color32 = hex(0x16727a);
    pub(crate) const BLUE: Color32 = hex(0x1d5ee1);
    pub(crate) const LAVENDER: Color32 = hex(0x5361b5);
    pub(crate) const MAUVE: Color32 = hex(0x8438e9);
    pub(crate) const FLAMINGO: Color32 = hex(0x965457);
    /// Latte subtext0, darkened: weak text.
    pub(crate) const WEAK: Color32 = hex(0x63667a);
    /// Weak text on the dark theme: between Mocha's overlay1 (under 4.5:1
    /// on its base) and overlay2.
    pub(crate) const WEAK_DARK: Color32 = hex(0x898ea7);
}

/// The flavour of the frame being drawn.
pub(crate) fn cat() -> &'static Flavour {
    if dark() { &MOCHA } else { &LATTE }
}

/// `fg` laid over `bg` at `a`, as an opaque colour.
pub(crate) fn mix(fg: Color32, bg: Color32, a: f32) -> Color32 {
    let m = |f: u8, b: u8| (f as f32 * a + b as f32 * (1.0 - a)).round() as u8;
    Color32::from_rgb(m(fg.r(), bg.r()), m(fg.g(), bg.g()), m(fg.b(), bg.b()))
}

/// Warnings and hints that need attention (yellow).
pub(crate) fn warn() -> Color32 {
    pick(MOCHA.yellow, ink::YELLOW)
}

/// Errors (red).
pub(crate) fn error() -> Color32 {
    pick(MOCHA.red, ink::RED)
}

/// Success: a done update, a recommended word, a book read (green).
pub(crate) fn good() -> Color32 {
    pick(MOCHA.green, ink::GREEN)
}

/// Neutral marks in the Library's charts, and the window's accent (blue).
pub(crate) fn accent() -> Color32 {
    pick(MOCHA.blue, ink::BLUE)
}

/// Selection, the current tab and focus (mauve).
pub(crate) fn highlight() -> Color32 {
    pick(MOCHA.mauve, ink::MAUVE)
}

/// Text that matters most: values, titles.
pub(crate) fn strong() -> Color32 {
    cat().text
}

/// Text that explains: tile titles, notes.
pub(crate) fn weak() -> Color32 {
    pick(ink::WEAK_DARK, ink::WEAK)
}

/// Font family of the semibold face (Windows: Segoe UI Semibold and
/// Microsoft YaHei Bold); elsewhere the regular faces.
pub(crate) const BOLD: &str = "ui-bold";

pub(crate) fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(BOLD.into()))
}

/// `s` in the semibold face.
pub(crate) fn bold_text(s: impl Into<String>, size: f32) -> RichText {
    RichText::new(s).family(FontFamily::Name(BOLD.into())).size(size)
}

/// The window's style on both themes.
pub(crate) fn install(ctx: &egui::Context) {
    for (theme, f) in [(Theme::Dark, &MOCHA), (Theme::Light, &LATTE)] {
        ctx.style_mut_of(theme, |s| style(s, f, theme == Theme::Dark));
    }
}

fn style(s: &mut egui::Style, f: &Flavour, dark: bool) {
    use egui::style::{Selection, WidgetVisuals};

    s.text_styles = [
        (TextStyle::Small, FontId::proportional(10.5)),
        (TextStyle::Body, FontId::proportional(13.0)),
        (TextStyle::Button, FontId::proportional(13.0)),
        (TextStyle::Monospace, FontId::monospace(12.5)),
        (TextStyle::Heading, bold(19.0)),
    ]
    .into();

    let sp = &mut s.spacing;
    sp.item_spacing = vec2(8.0, 5.0);
    sp.button_padding = vec2(8.0, 3.0);
    sp.interact_size.y = 20.0;
    sp.window_margin = Margin::same(14);
    sp.menu_margin = Margin::same(6);
    sp.indent = 20.0;

    let (normal, weak, accent) = if dark {
        (f.subtext1, ink::WEAK_DARK, f.mauve)
    } else {
        (f.subtext1, ink::WEAK, ink::MAUVE)
    };
    let radius = CornerRadius::same(6);
    let widget = |bg: Color32, stroke: Stroke, fg: Color32, fg_width: f32| WidgetVisuals {
        weak_bg_fill: bg,
        bg_fill: bg,
        bg_stroke: stroke,
        corner_radius: radius,
        fg_stroke: Stroke::new(fg_width, fg),
        expansion: 0.0,
    };
    let v = &mut s.visuals;
    v.override_text_color = None;
    v.weak_text_color = Some(weak);
    v.widgets.noninteractive = WidgetVisuals {
        corner_radius: CornerRadius::same(4),
        ..widget(f.mantle, Stroke::new(1.0, f.surface0), normal, 1.0)
    };
    v.widgets.inactive = widget(f.surface0, Stroke::NONE, f.text, 1.0);
    v.widgets.inactive.bg_fill = f.surface1;
    v.widgets.hovered = widget(f.surface1, Stroke::new(1.0, f.surface2), f.text, 1.5);
    v.widgets.active = widget(f.surface2, Stroke::new(1.0, accent), f.text, 2.0);
    v.widgets.open = widget(f.surface0, Stroke::new(1.0, f.surface1), f.text, 1.0);
    v.selection = Selection {
        bg_fill: mix(f.mauve, f.base, if dark { 0.35 } else { 0.22 }),
        stroke: Stroke::new(1.0, if dark { f.lavender } else { ink::MAUVE }),
    };
    v.hyperlink_color = if dark { f.blue } else { ink::BLUE };
    // Striped rows, on panels and cards alike.
    v.faint_bg_color = if dark {
        Color32::from_white_alpha(7)
    } else {
        Color32::from_black_alpha(9)
    };
    v.extreme_bg_color = f.crust;
    v.text_edit_bg_color = Some(f.crust);
    v.code_bg_color = f.surface0;
    v.warn_fg_color = if dark { f.yellow } else { ink::YELLOW };
    v.error_fg_color = if dark { f.red } else { ink::RED };
    v.panel_fill = f.mantle;
    v.window_fill = f.base;
    v.window_stroke = Stroke::new(1.0, f.surface0);
    v.window_corner_radius = CornerRadius::same(12);
    v.window_shadow = Shadow {
        offset: [0, 8],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 110 } else { 30 }),
    };
    v.menu_corner_radius = CornerRadius::same(8);
    v.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 14,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 90 } else { 24 }),
    };
    v.indent_has_left_vline = false;
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
}

/// A raised card: Overview tiles and boxes.
pub(crate) fn card() -> egui::Frame {
    let f = cat();
    egui::Frame::NONE
        .fill(f.base)
        .stroke(Stroke::new(1.0, pick(f.surface0, mix(f.surface0, f.base, 0.7))))
        .corner_radius(12)
        .inner_margin(Margin::symmetric(16, 14))
        .shadow(Shadow {
            offset: [0, 2],
            blur: 10,
            spread: 0,
            color: Color32::from_black_alpha(if dark() { 40 } else { 12 }),
        })
}

/// The bars above and below the window's content.
pub(crate) fn bar(top: bool) -> egui::Frame {
    let f = cat();
    egui::Frame::NONE
        .fill(f.crust)
        .inner_margin(if top {
            Margin::symmetric(10, 7)
        } else {
            Margin::symmetric(10, 4)
        })
}

/// Smaller text and tighter spacing, for the toolbar.
pub(crate) fn compact(ui: &mut Ui) {
    let s = ui.style_mut();
    s.spacing.item_spacing.x = 5.0;
    s.spacing.button_padding.x = 7.0;
    for t in [TextStyle::Body, TextStyle::Button] {
        s.text_styles.insert(t, FontId::proportional(12.5));
    }
}

/// A tab: its name, underlined in the accent while it is the current one.
pub(crate) fn tab(ui: &mut Ui, selected: bool, name: &str, size: f32) -> Response {
    let font = if selected {
        bold(size)
    } else {
        FontId::proportional(size)
    };
    let galley = ui
        .painter()
        .layout_no_wrap(name.to_owned(), font, Color32::PLACEHOLDER);
    let pad = vec2(10.0, 6.0);
    // Room for the semibold face either way, so tabs don't move.
    let wide = ui
        .painter()
        .layout_no_wrap(name.to_owned(), bold(size), Color32::PLACEHOLDER)
        .size()
        .x;
    let size_v = vec2(wide.max(galley.size().x), galley.size().y) + 2.0 * pad + vec2(0.0, 3.0);
    let (rect, resp) = ui.allocate_exact_size(size_v, Sense::click());
    if ui.is_rect_visible(rect) {
        let f = cat();
        let p = ui.painter();
        let hovered = resp.hovered();
        if hovered && !selected {
            p.rect_filled(rect.shrink2(vec2(0.0, 2.0)), 6, f.surface0);
        }
        let color = if selected || hovered { f.text } else { weak() };
        let pos = egui::pos2(
            rect.center().x - galley.size().x / 2.0,
            rect.top() + pad.y,
        );
        p.galley(pos, galley, color);
        if selected {
            let line = egui::Rect::from_min_max(
                egui::pos2(rect.left() + 6.0, rect.bottom() - 3.0),
                egui::pos2(rect.right() - 6.0, rect.bottom()),
            );
            p.rect_filled(line, 2, highlight());
        }
    }
    resp
}

/// A row of tabs over a hairline; sets `current` to the one clicked.
pub(crate) fn tabs<T: Copy + PartialEq>(
    ui: &mut Ui,
    current: &mut T,
    items: impl IntoIterator<Item = (T, &'static str)>,
    size: f32,
    mut hover: impl FnMut(T, Response),
) {
    let r = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (t, name) in items {
            let resp = tab(ui, *current == t, name, size);
            if resp.clicked() {
                *current = t;
            }
            hover(t, resp);
        }
    });
    let y = r.response.rect.bottom();
    let x = ui.max_rect().x_range();
    ui.painter()
        .hline(x, y - 0.5, Stroke::new(1.0, cat().surface0));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: Color32) -> f64 {
        let lin = |v: u8| {
            let v = v as f64 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn text_colours_reach_aa() {
        let light = [
            ink::RED,
            ink::MAROON,
            ink::PEACH,
            ink::YELLOW,
            ink::GREEN,
            ink::TEAL,
            ink::BLUE,
            ink::LAVENDER,
            ink::MAUVE,
            ink::FLAMINGO,
            ink::WEAK,
            LATTE.subtext1,
            LATTE.text,
        ];
        for c in light {
            for bg in [LATTE.mantle, LATTE.base] {
                assert!(contrast(c, bg) >= 4.5, "{c:?} on {bg:?}");
            }
        }
        let m = &MOCHA;
        let dark = [
            m.red, m.peach, m.yellow, m.green, m.teal, m.sapphire, m.blue, m.lavender, m.mauve,
            ink::WEAK_DARK, m.subtext1, m.text,
        ];
        for c in dark {
            for bg in [m.mantle, m.base, m.crust] {
                assert!(contrast(c, bg) >= 4.5, "{c:?} on {bg:?}");
            }
        }
    }
}
