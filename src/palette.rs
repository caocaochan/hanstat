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
pub(crate) struct Flavour {
    pub flamingo: Color32,
    pub pink: Color32,
    pub mauve: Color32,
    pub red: Color32,
    pub maroon: Color32,
    pub peach: Color32,
    pub yellow: Color32,
    pub green: Color32,
    pub teal: Color32,
    pub sapphire: Color32,
    pub blue: Color32,
    pub lavender: Color32,
    pub text: Color32,
    pub subtext1: Color32,
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
    flamingo: hex(0xf2cdcd),
    pink: hex(0xf5c2e7),
    mauve: hex(0xcba6f7),
    red: hex(0xf38ba8),
    maroon: hex(0xeba0ac),
    peach: hex(0xfab387),
    yellow: hex(0xf9e2af),
    green: hex(0xa6e3a1),
    teal: hex(0x94e2d5),
    sapphire: hex(0x74c7ec),
    blue: hex(0x89b4fa),
    lavender: hex(0xb4befe),
    text: hex(0xcdd6f4),
    subtext1: hex(0xbac2de),
    overlay0: hex(0x6c7086),
    surface2: hex(0x585b70),
    surface1: hex(0x45475a),
    surface0: hex(0x313244),
    base: hex(0x1e1e2e),
    mantle: hex(0x181825),
    crust: hex(0x11111b),
};

pub(crate) const LATTE: Flavour = Flavour {
    flamingo: hex(0xdd7878),
    pink: hex(0xea76cb),
    mauve: hex(0x8839ef),
    red: hex(0xd20f39),
    maroon: hex(0xe64553),
    peach: hex(0xfe640b),
    yellow: hex(0xdf8e1d),
    green: hex(0x40a02b),
    teal: hex(0x179299),
    sapphire: hex(0x209fb5),
    blue: hex(0x1e66f5),
    lavender: hex(0x7287fd),
    text: hex(0x4c4f69),
    subtext1: hex(0x5c5f77),
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
    RichText::new(s)
        .family(FontFamily::Name(BOLD.into()))
        .size(size)
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
        .stroke(Stroke::new(
            1.0,
            pick(f.surface0, mix(f.surface0, f.base, 0.7)),
        ))
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
    egui::Frame::NONE.fill(f.crust).inner_margin(if top {
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

/// A spinner like `ui.spinner()`, capped at ~30 FPS so `PresentMode::AutoNoVsync`
/// does not spin the GPU at hundreds/thousands of FPS while background work runs.
pub(crate) fn spinner(ui: &mut Ui) -> Response {
    let size = ui
        .style()
        .spacing
        .interact_size
        .y
        .min(ui.available_size_before_wrap().x.max(12.0));
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
        let color = ui.visuals().strong_text_color();
        let radius = (rect.height() / 2.0) - 2.0;
        let n_points = 20;
        let time = ui.input(|i| i.time);
        let start_angle = time * std::f64::consts::TAU;
        let end_angle = start_angle + 240f64.to_radians() * time.sin();
        let points: Vec<egui::Pos2> = (0..n_points)
            .map(|i| {
                let angle = egui::lerp(start_angle..=end_angle, i as f64 / n_points as f64);
                let (sin, cos) = angle.sin_cos();
                rect.center() + radius * vec2(cos as f32, sin as f32)
            })
            .collect();
        ui.painter()
            .add(egui::Shape::line(points, Stroke::new(3.0, color)));
    }
    response
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
    let wide = if selected {
        galley.size().x
    } else {
        ui.painter()
            .layout_no_wrap(name.to_owned(), bold(size), Color32::PLACEHOLDER)
            .size()
            .x
    };
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
        let pos = egui::pos2(rect.center().x - galley.size().x / 2.0, rect.top() + pad.y);
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

/// Safe viewport rect for floating popups and dialogs so they stay clear of
/// the window edges (and therefore the OS taskbar when maximised/full-height).
pub(crate) fn popup_bounds(ctx: &egui::Context) -> egui::Rect {
    let s = ctx.content_rect();
    let pad_x = 8.0_f32.min((s.width() - 120.0).max(0.0) * 0.5);
    let pad_y = 8.0_f32.min((s.height() - 120.0).max(0.0) * 0.5);
    egui::Rect::from_min_max(
        egui::pos2(s.left() + pad_x, s.top() + pad_y),
        egui::pos2(s.right() - pad_x, s.bottom() - pad_y),
    )
}

/// Maximum inner content size for an [`egui::Window`] whose outer frame (title
/// bar, margins and border) must stay inside `bounds`.
pub(crate) fn popup_max_inner(ctx: &egui::Context, bounds: egui::Rect) -> egui::Vec2 {
    let sp = &ctx.style_of(ctx.theme()).spacing;
    let margin = sp.window_margin.sum();
    // Title bar height + separator + frame stroke + small safety buffer.
    let chrome_w = margin.x + 12.0;
    let chrome_h = margin.y + sp.interact_size.y + 2.0 * sp.button_padding.y + 24.0;
    vec2(
        (bounds.width() - chrome_w).max(120.0),
        (bounds.height() - chrome_h).max(100.0),
    )
}

/// A floating popup window that is always resizable, scrollable in both
/// directions when its contents don't fit, and constrained inside the app
/// viewport so it never runs off-screen or behind the OS taskbar.
pub(crate) fn popup_window<'open>(
    ctx: &egui::Context,
    title: impl Into<egui::WidgetText>,
) -> egui::Window<'open> {
    let bounds = popup_bounds(ctx);
    let max_inner = popup_max_inner(ctx, bounds);
    let min_inner = vec2(220.0_f32.min(max_inner.x), 90.0_f32.min(max_inner.y));
    egui::Window::new(title)
        .resizable(true)
        .scroll(true)
        .constrain_to(bounds)
        .min_size(min_inner)
        .max_size(max_inner)
}

/// Maximum inner size for a dropdown [`egui::Popup`] anchored inside `ctx`.
/// Reserves space for the bar/trigger button and popup margins so it stays
/// clear of the bottom of the window (and the OS taskbar).
pub(crate) fn dropdown_max_size(ctx: &egui::Context, top_reserve: f32) -> egui::Vec2 {
    let bounds = popup_bounds(ctx);
    let sp = &ctx.style_of(ctx.theme()).spacing;
    let margin = sp.menu_margin.sum();
    vec2(
        (bounds.width() - margin.x - 16.0).max(160.0),
        (bounds.height() - margin.y - top_reserve).max(120.0),
    )
}

/// Make a dropdown or modal popup body resizable and scrollable in both
/// directions while staying within `max_size`.
pub(crate) fn resizable_scroll_popup<R>(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    default_size: egui::Vec2,
    min_size: egui::Vec2,
    max_size: egui::Vec2,
    body: impl FnOnce(&mut Ui) -> R,
) -> R {
    let max_s = vec2(max_size.x.max(120.0), max_size.y.max(100.0));
    let min_s = vec2(min_size.x.min(max_s.x), min_size.y.min(max_s.y));
    let def_s = vec2(
        default_size.x.clamp(min_s.x, max_s.x),
        default_size.y.clamp(min_s.y, max_s.y),
    );
    let scroll_id = ui.make_persistent_id((&id, "scroll"));
    let mut inner = None;
    egui::Resize::default()
        .id_salt((id, "resize"))
        .default_size(def_s)
        .min_size(min_s)
        .max_size(max_s)
        .show(ui, |ui| {
            egui::ScrollArea::both()
                .id_salt(scroll_id)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    inner = Some(body(ui));
                });
        });
    inner.expect("Resize body always runs")
}

/// A row of tabs over a hairline; sets `current` to the one clicked.
/// Scrolls horizontally when the window is too narrow for every tab to fit.
pub(crate) fn tabs<T: Copy + PartialEq>(
    ui: &mut Ui,
    current: &mut T,
    items: impl IntoIterator<Item = (T, &'static str)>,
    size: f32,
    mut hover: impl FnMut(T, Response),
) {
    let x = ui.max_rect().x_range();
    let r = egui::ScrollArea::horizontal()
        .id_salt("palette-tabs-scroll")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (t, name) in items {
                    let resp = tab(ui, *current == t, name, size);
                    if resp.clicked() {
                        *current = t;
                    }
                    hover(t, resp);
                }
            })
        });
    let y = r.inner_rect.bottom();
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
            m.red,
            m.peach,
            m.yellow,
            m.green,
            m.teal,
            m.sapphire,
            m.blue,
            m.lavender,
            m.mauve,
            ink::WEAK_DARK,
            m.subtext1,
            m.text,
        ];
        for c in dark {
            for bg in [m.mantle, m.base, m.crust] {
                assert!(contrast(c, bg) >= 4.5, "{c:?} on {bg:?}");
            }
        }
    }

    #[test]
    fn popup_window_stays_inside_viewport_with_oversized_content() {
        let ctx = egui::Context::default();
        install(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(640.0, 420.0));
        let id = egui::Id::unique("test-oversized-popup");
        for _ in 0..3 {
            let mut out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..egui::RawInput::default()
                },
                |ui| {
                    popup_window(ui.ctx(), "Oversized")
                        .id(id)
                        .collapsible(false)
                        .default_width(600.0)
                        .show(ui.ctx(), |ui| {
                            for i in 0..60 {
                                ui.label(format!("Row {i}: {}", "wide content ".repeat(12)));
                            }
                        });
                },
            );
            out.textures_delta.clear();
        }
        let rect = egui::AreaState::load(&ctx, id).unwrap().rect();
        assert!(
            rect.left() >= screen.left() && rect.right() <= screen.right(),
            "popup horizontally escaped viewport: {rect:?} vs {screen:?}"
        );
        assert!(
            rect.top() >= screen.top() && rect.bottom() <= screen.bottom(),
            "popup vertically escaped viewport: {rect:?} vs {screen:?}"
        );
    }

    #[test]
    fn resizable_scroll_popup_stays_inside_viewport() {
        let ctx = egui::Context::default();
        install(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(500.0, 360.0));
        let mut drawn_rect = egui::Rect::NOTHING;
        for _ in 0..3 {
            let mut out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..egui::RawInput::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let max = dropdown_max_size(ui.ctx(), 72.0);
                        drawn_rect = ui
                            .scope(|ui| {
                                resizable_scroll_popup(
                                    ui,
                                    "test-dropdown",
                                    vec2(420.0, 460.0),
                                    vec2(220.0, 120.0),
                                    max,
                                    |ui| {
                                        ui.allocate_space(vec2(900.0, 1200.0));
                                    },
                                );
                            })
                            .response
                            .rect;
                    });
                },
            );
            out.textures_delta.clear();
        }
        assert!(
            drawn_rect.width() <= screen.width() && drawn_rect.height() <= screen.height(),
            "dropdown popup overflowed viewport: {drawn_rect:?} vs {screen:?}"
        );
    }

    #[test]
    fn tabs_scroll_horizontally_when_narrow() {
        let ctx = egui::Context::default();
        install(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(320.0, 200.0));
        let items = [
            (0u8, "Overview"),
            (1, "Characters"),
            (2, "Words"),
            (3, "Sections"),
            (4, "Suggested words"),
            (5, "Notes & method"),
        ];
        let mut cur = 0u8;
        let mut last_tab_before = egui::Rect::NOTHING;
        let mut last_tab_after = egui::Rect::NOTHING;
        for step in 0..5 {
            let mut events = Vec::new();
            if step == 2 {
                events.push(egui::Event::PointerMoved(egui::pos2(80.0, 20.0)));
                events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(-180.0, 0.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                    source: egui::MouseWheelSource::Unknown,
                });
            }
            let mut out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..egui::RawInput::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        tabs(ui, &mut cur, items, 15.0, |t, r| {
                            if t == 5 {
                                if step == 1 {
                                    last_tab_before = r.rect;
                                } else if step == 4 {
                                    last_tab_after = r.rect;
                                }
                            }
                        });
                    });
                },
            );
            out.textures_delta.clear();
        }
        assert!(
            last_tab_before.right() > screen.right(),
            "expected 6 tabs to exceed narrow 320px viewport: {last_tab_before:?}"
        );
        assert!(
            last_tab_after.left() < last_tab_before.left() - 10.0,
            "expected horizontal scroll to move rightmost tab into view: before={last_tab_before:?}, after={last_tab_after:?}"
        );
    }
}
