//! Computed styles: the cascade of the book's CSS (and a small user-agent
//! sheet) over each element, for the properties the reader lays out.
//!
//! Lengths are kept in "base units", multiples of the reader's text size, so
//! a book's styled tree does not depend on the size the reader picks: the
//! book's `px` are read against a 16 px default (`16px` is one unit), `em`
//! against the element's font, `%` stays relative to the containing block.
//!
//! Books made for vertical layout (Readmoo, Japanese publishers) set their
//! margins, padding and borders physically for vertical lines; shown
//! horizontally, those are turned a quarter (the right margin, before a
//! vertical-rl block, becomes the top margin), so headings keep their space.

use super::css::Decl;
use eframe::egui::Color32;

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub(super) enum Len {
    #[default]
    Auto,
    /// Multiples of the reader's text size.
    Base(f32),
    /// Of the containing block's width (or the page height, for heights of
    /// replaced elements).
    Pct(f32),
}

impl Len {
    pub(super) fn resolve(self, base_px: f32, of: f32) -> Option<f32> {
        match self {
            Len::Auto => None,
            Len::Base(v) => Some(v * base_px),
            Len::Pct(p) => Some(p / 100.0 * of),
        }
    }
    pub(super) fn px(self, base_px: f32, of: f32) -> f32 {
        self.resolve(base_px, of).unwrap_or(0.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum Family {
    #[default]
    Default,
    Serif,
    Sans,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum Display {
    #[default]
    Inline,
    Block,
    ListItem,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum Align {
    #[default]
    Start,
    Center,
    End,
    Justify,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum WhiteSpace {
    #[default]
    Normal,
    NoWrap,
    Pre,
    PreWrap,
    PreLine,
}

impl WhiteSpace {
    pub(super) fn collapses(self) -> bool {
        matches!(
            self,
            WhiteSpace::Normal | WhiteSpace::NoWrap | WhiteSpace::PreLine
        )
    }
    pub(super) fn wraps(self) -> bool {
        !matches!(self, WhiteSpace::NoWrap | WhiteSpace::Pre)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum VAlign {
    #[default]
    Baseline,
    Super,
    Sub,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum Writing {
    #[default]
    Horizontal,
    VerticalRl,
    VerticalLr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum LineStyle {
    #[default]
    None,
    Solid,
    Dashed,
    Dotted,
    Double,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub(super) struct Border {
    pub(super) width: f32,
    pub(super) style: LineStyle,
    pub(super) color: Option<Color32>,
}

impl Border {
    pub(super) fn visible(&self) -> bool {
        self.style != LineStyle::None && self.width > 0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum Float {
    #[default]
    None,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum ListStyle {
    #[default]
    Disc,
    Decimal,
    None,
}

/// Sides in CSS order: top, right, bottom, left.
pub(super) type Sides<T> = [T; 4];

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Style {
    // Inherited.
    /// Font size in base units.
    pub(super) font: f32,
    pub(super) family: Family,
    pub(super) bold: bool,
    pub(super) italic: bool,
    /// `None`: the theme's text colour.
    pub(super) color: Option<Color32>,
    /// `None`: normal; else a factor of the font size.
    pub(super) line_height: Option<f32>,
    /// A line height given as a length (base units), instead of a factor.
    pub(super) line_height_abs: Option<f32>,
    pub(super) align: Align,
    pub(super) indent: Len,
    /// Base units.
    pub(super) letter_spacing: f32,
    pub(super) white_space: WhiteSpace,
    pub(super) hidden: bool,
    pub(super) writing: Writing,
    pub(super) list_style: ListStyle,
    // Not inherited (text-decoration propagates to inline descendants).
    pub(super) underline: bool,
    pub(super) strike: bool,
    pub(super) display: Display,
    pub(super) margin: Sides<Len>,
    pub(super) padding: Sides<Len>,
    pub(super) border: Sides<Border>,
    pub(super) width: Len,
    pub(super) height: Len,
    pub(super) max_width: Len,
    pub(super) max_height: Len,
    pub(super) background: Option<Color32>,
    pub(super) valign: VAlign,
    pub(super) float: Float,
    pub(super) break_before: bool,
    pub(super) break_after: bool,
    /// `display: inline-block` (or `inline-flex`, `inline-table`): an inline
    /// box with its own background and border. Laid out as such only in a
    /// dictionary popup; in a book it is plain inline text, as before.
    pub(super) inline_block: bool,
    /// `border-radius`, base units.
    pub(super) radius: f32,
    /// `opacity`, multiplied down through the element's descendants (CSS
    /// composites the whole subtree; this fades each part the same).
    pub(super) opacity: f32,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            font: 1.0,
            family: Family::Default,
            bold: false,
            italic: false,
            color: None,
            line_height: None,
            line_height_abs: None,
            align: Align::Start,
            indent: Len::Base(0.0),
            letter_spacing: 0.0,
            white_space: WhiteSpace::Normal,
            hidden: false,
            writing: Writing::Horizontal,
            list_style: ListStyle::Disc,
            underline: false,
            strike: false,
            display: Display::Inline,
            margin: [Len::Base(0.0); 4],
            padding: [Len::Base(0.0); 4],
            border: [Border::default(); 4],
            width: Len::Auto,
            height: Len::Auto,
            max_width: Len::Auto,
            max_height: Len::Auto,
            background: None,
            valign: VAlign::Baseline,
            float: Float::None,
            break_before: false,
            break_after: false,
            inline_block: false,
            radius: 0.0,
            opacity: 1.0,
        }
    }
}

impl Style {
    /// The style a child starts from: inherited properties kept, the rest reset.
    pub(super) fn inherit(&self) -> Style {
        Style {
            font: self.font,
            family: self.family,
            bold: self.bold,
            italic: self.italic,
            color: self.color,
            line_height: self.line_height,
            line_height_abs: self.line_height_abs,
            align: self.align,
            indent: self.indent,
            letter_spacing: self.letter_spacing,
            white_space: self.white_space,
            hidden: self.hidden,
            writing: self.writing,
            list_style: self.list_style,
            underline: self.underline,
            strike: self.strike,
            opacity: self.opacity,
            ..Style::default()
        }
    }
}

/// The user-agent sheet: HTML's default presentation, without the body
/// margin (the reader has its own page margins).
pub(super) const UA_SHEET: &str = r#"
address, article, aside, blockquote, body, center, dd, details, dialog, dir, div, dl, dt,
fieldset, figcaption, figure, footer, form, h1, h2, h3, h4, h5, h6, header, hgroup, hr,
html, legend, main, menu, nav, ol, p, pre, section, summary, ul, table, caption, thead,
tbody, tfoot, tr, td, th, svg { display: block }
li { display: list-item }
head, script, style, title, meta, link, template, rp, noscript { display: none }
p, dl, figure { margin: 1em 0 }
blockquote { margin: 1em 2.5em }
h1 { font-size: 2em; margin: 0.67em 0; font-weight: bold }
h2 { font-size: 1.5em; margin: 0.83em 0; font-weight: bold }
h3 { font-size: 1.17em; margin: 1em 0; font-weight: bold }
h4 { margin: 1.33em 0; font-weight: bold }
h5 { font-size: 0.83em; margin: 1.67em 0; font-weight: bold }
h6 { font-size: 0.67em; margin: 2.33em 0; font-weight: bold }
b, strong, th { font-weight: bold }
i, em, cite, var, dfn, address { font-style: italic }
u, ins { text-decoration: underline }
s, strike, del { text-decoration: line-through }
sup { vertical-align: super; font-size: smaller }
sub { vertical-align: sub; font-size: smaller }
small { font-size: smaller }
big { font-size: larger }
pre { white-space: pre; margin: 1em 0 }
center { text-align: center }
hr { border-top: 1px solid; margin: 0.5em 0 }
ul, ol, menu, dir { margin: 1em 0; padding-left: 2.5em }
ol { list-style-type: decimal }
dd { margin-left: 2.5em }
a[href] { color: #268bd2 }
td, th { padding: 0.1em 0.3em }
"#;

/// Apply the declarations matched for an element, in cascade order, to
/// `style` (which starts as the parent's [`Style::inherit`]). `parent` is the
/// parent's computed style, `root_font` the root element's font size (rem).
/// `replaced`: an image, whose width and height are not turned for vertical text.
pub(super) fn apply(
    style: &mut Style,
    parent: &Style,
    root_font: f32,
    decls: &[&Decl],
    replaced: bool,
) {
    // Writing mode first: it decides how physical sides are turned.
    for d in decls {
        if matches!(
            d.name.as_str(),
            "writing-mode" | "-epub-writing-mode" | "-webkit-writing-mode"
        ) {
            let v = d.value.to_ascii_lowercase();
            style.writing = if v.contains("vertical-rl") || v == "tb-rl" || v == "tb" {
                Writing::VerticalRl
            } else if v.contains("vertical-lr") || v == "tb-lr" {
                Writing::VerticalLr
            } else if v.contains("horizontal") || v == "lr-tb" || v == "lr" {
                Writing::Horizontal
            } else if v == "inherit" {
                parent.writing
            } else {
                style.writing
            };
        }
    }
    // Font size next: em lengths in the other declarations depend on it.
    for d in decls {
        if d.name == "font-size" {
            if let Some(f) = font_size(&d.value, parent.font, root_font) {
                style.font = f;
            }
        } else if d.name == "font"
            && let Some(f) = d
                .value
                .split_whitespace()
                .find_map(|t| font_size(t.split('/').next().unwrap_or(t), parent.font, root_font))
        {
            style.font = f;
        }
    }
    let em = style.font;
    for d in decls {
        let name = turn(&d.name, style.writing, replaced);
        set(style, parent, &name, &d.value, em, root_font);
    }
}

/// The property a physical side or axis maps to when a vertical book is
/// shown horizontally.
fn turn(name: &str, w: Writing, replaced: bool) -> std::borrow::Cow<'_, str> {
    if w == Writing::Horizontal {
        return name.into();
    }
    let side = |s: &str| -> &'static str {
        match (w, s) {
            // vertical-rl: right is the block start (top), left the end
            // (bottom), top the inline start (left), bottom the inline end.
            (Writing::VerticalRl, "right") => "top",
            (Writing::VerticalRl, "left") => "bottom",
            (Writing::VerticalLr, "left") => "top",
            (Writing::VerticalLr, "right") => "bottom",
            (_, "top") => "left",
            (_, "bottom") => "right",
            _ => "top",
        }
    };
    for prefix in ["margin-", "padding-", "border-"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            let (s, tail) = rest.split_once('-').unwrap_or((rest, ""));
            if matches!(s, "top" | "right" | "bottom" | "left") {
                let t = side(s);
                return if tail.is_empty() {
                    format!("{prefix}{t}").into()
                } else {
                    format!("{prefix}{t}-{tail}").into()
                };
            }
        }
    }
    if !replaced {
        let swapped = match name {
            "width" => "height",
            "height" => "width",
            "max-width" => "max-height",
            "max-height" => "max-width",
            _ => return name.into(),
        };
        return swapped.into();
    }
    name.into()
}

/// Four-value shorthands (`margin: a b c d`) turned for vertical books.
fn turn_sides<T: Copy>(v: Sides<T>, w: Writing) -> Sides<T> {
    let [t, r, b, l] = v;
    match w {
        Writing::Horizontal => v,
        // new top = right, right = bottom, bottom = left, left = top
        Writing::VerticalRl => [r, b, l, t],
        // new top = left, bottom = right, left = top, right = bottom
        Writing::VerticalLr => [l, b, r, t],
    }
}

fn side_index(s: &str) -> Option<usize> {
    Some(match s {
        "top" => 0,
        "right" => 1,
        "bottom" => 2,
        "left" => 3,
        _ => return None,
    })
}

fn set(style: &mut Style, parent: &Style, name: &str, value: &str, em: f32, rem: f32) {
    let v = value.trim();
    let lower = v.to_ascii_lowercase();
    if lower == "inherit" {
        inherit_one(style, parent, name);
        return;
    }
    let len = |s: &str| length(s, em, rem);
    match name {
        "font-family" => {
            if let Some(f) = family(v) {
                style.family = f;
            }
        }
        "font-weight" => style.bold = weight_bold(&lower, parent.bold),
        "font-style" => style.italic = matches!(lower.as_str(), "italic" | "oblique"),
        "font" => font_shorthand(style, v),
        "color" => {
            if let Some(c) =
                color_mix(&lower, style.color.or(parent.color)).or_else(|| color(&lower))
            {
                style.color = c;
            }
        }
        "line-height" => line_height(style, &lower, em, rem),
        "text-align" => {
            style.align = match lower.as_str() {
                "center" | "-webkit-center" => Align::Center,
                "right" | "end" => Align::End,
                "justify" => Align::Justify,
                "left" | "start" => Align::Start,
                _ => style.align,
            }
        }
        "text-indent" => {
            if let Some(l) = len(&lower) {
                style.indent = l;
            }
        }
        "letter-spacing" => {
            style.letter_spacing = match len(&lower) {
                Some(Len::Base(b)) => b,
                _ => 0.0,
            }
        }
        "white-space" => {
            style.white_space = match lower.as_str() {
                "nowrap" => WhiteSpace::NoWrap,
                "pre" => WhiteSpace::Pre,
                "pre-wrap" | "break-spaces" => WhiteSpace::PreWrap,
                "pre-line" => WhiteSpace::PreLine,
                _ => WhiteSpace::Normal,
            }
        }
        "visibility" => style.hidden = matches!(lower.as_str(), "hidden" | "collapse"),
        "list-style-type" | "list-style" => {
            if lower.contains("none") {
                style.list_style = ListStyle::None;
            } else if lower.contains("decimal")
                || lower.contains("roman")
                || lower.contains("alpha")
                || lower.contains("cjk")
            {
                style.list_style = ListStyle::Decimal;
            } else if lower.contains("disc") || lower.contains("circle") || lower.contains("square")
            {
                style.list_style = ListStyle::Disc;
            }
        }
        "text-decoration" | "text-decoration-line" => {
            if lower.contains("none") {
                style.underline = false;
                style.strike = false;
            }
            if lower.contains("underline") {
                style.underline = true;
            }
            if lower.contains("line-through") {
                style.strike = true;
            }
        }
        "display" => {
            style.display = match lower.as_str() {
                "none" => Display::None,
                "inline" | "inline-block" | "inline-flex" | "inline-table" | "contents"
                | "ruby" | "ruby-base" | "ruby-text" => Display::Inline,
                "list-item" => Display::ListItem,
                _ => Display::Block,
            };
            style.inline_block = matches!(
                lower.as_str(),
                "inline-block" | "inline-flex" | "inline-table"
            );
        }
        "border-radius" => {
            if let Some(Len::Base(b)) = lower.split_whitespace().next().and_then(len) {
                style.radius = b.max(0.0);
            } else if lower.trim_end().ends_with('%') {
                // 50% and the like: a pill.
                style.radius = 99.0;
            }
        }
        "opacity" => {
            if let Some(o) = lower
                .strip_suffix('%')
                .and_then(|p| p.trim().parse::<f32>().ok().map(|p| p / 100.0))
                .or_else(|| lower.parse::<f32>().ok())
            {
                style.opacity = parent.opacity * o.clamp(0.0, 1.0);
            }
        }
        "margin" | "padding" => {
            let parts: Vec<Len> = lower.split_whitespace().filter_map(len).collect();
            if let Some(s) = four(&parts) {
                let s = turn_sides(s, style.writing);
                if name == "margin" {
                    style.margin = s;
                } else {
                    style.padding = s;
                }
            }
        }
        "width" => style.width = len(&lower).unwrap_or(Len::Auto),
        "height" => style.height = len(&lower).unwrap_or(Len::Auto),
        "max-width" => style.max_width = len(&lower).unwrap_or(Len::Auto),
        "max-height" => style.max_height = len(&lower).unwrap_or(Len::Auto),
        "background-color" | "background" => {
            if let Some(c) = color_mix(&lower, style.color) {
                style.background = c;
            } else if let Some(c) = lower.split_whitespace().find_map(color) {
                style.background = c;
            } else if lower.contains("none") {
                style.background = None;
            }
        }
        "vertical-align" => {
            style.valign = match lower.as_str() {
                "super" | "top" | "text-top" => VAlign::Super,
                "sub" => VAlign::Sub,
                _ => VAlign::Baseline,
            }
        }
        "float" => {
            style.float = match lower.as_str() {
                "left" => Float::Left,
                "right" => Float::Right,
                _ => Float::None,
            }
        }
        "page-break-before" | "break-before" => {
            style.break_before = matches!(
                lower.as_str(),
                "always" | "page" | "left" | "right" | "recto" | "verso"
            )
        }
        "page-break-after" | "break-after" => {
            style.break_after = matches!(
                lower.as_str(),
                "always" | "page" | "left" | "right" | "recto" | "verso"
            )
        }
        "border" => {
            let b = border(&lower, em, rem);
            style.border = [b; 4];
        }
        "border-width" | "border-style" | "border-color" => {
            let parts: Vec<&str> = lower.split_whitespace().collect();
            let sides = match parts.len() {
                1 => [parts[0]; 4],
                2 => [parts[0], parts[1], parts[0], parts[1]],
                3 => [parts[0], parts[1], parts[2], parts[1]],
                4 => [parts[0], parts[1], parts[2], parts[3]],
                _ => return,
            };
            let sides = turn_sides(sides, style.writing);
            for (i, p) in sides.iter().enumerate() {
                border_part(
                    &mut style.border[i],
                    name.trim_start_matches("border-"),
                    p,
                    em,
                    rem,
                );
            }
        }
        _ => {
            let parts: Vec<&str> = name.split('-').collect();
            match parts.as_slice() {
                ["margin", s] | ["padding", s] => {
                    if let (Some(i), Some(l)) = (side_index(s), len(&lower)) {
                        if parts[0] == "margin" {
                            style.margin[i] = l;
                        } else {
                            style.padding[i] = l;
                        }
                    }
                }
                ["border", s] => {
                    if let Some(i) = side_index(s) {
                        style.border[i] = border(&lower, em, rem);
                    }
                }
                ["border", s, what] => {
                    if let Some(i) = side_index(s) {
                        border_part(&mut style.border[i], what, &lower, em, rem);
                    }
                }
                _ => {}
            }
        }
    }
}

fn inherit_one(style: &mut Style, parent: &Style, name: &str) {
    match name {
        "color" => style.color = parent.color,
        "font-weight" => style.bold = parent.bold,
        "font-style" => style.italic = parent.italic,
        "font-family" => style.family = parent.family,
        "text-align" => style.align = parent.align,
        "line-height" => {
            style.line_height = parent.line_height;
            style.line_height_abs = parent.line_height_abs;
        }
        "margin" => style.margin = parent.margin,
        "padding" => style.padding = parent.padding,
        _ => {}
    }
}

fn four<T: Copy>(p: &[T]) -> Option<Sides<T>> {
    Some(match p.len() {
        1 => [p[0]; 4],
        2 => [p[0], p[1], p[0], p[1]],
        3 => [p[0], p[1], p[2], p[1]],
        4 => [p[0], p[1], p[2], p[3]],
        _ => return None,
    })
}

fn border(v: &str, em: f32, rem: f32) -> Border {
    let mut b = Border {
        width: 1.0 / 16.0 * 3.0,
        style: LineStyle::None,
        color: None,
    };
    for t in v.split_whitespace() {
        border_part(&mut b, "style", t, em, rem);
        border_part(&mut b, "width", t, em, rem);
        border_part(&mut b, "color", t, em, rem);
    }
    b
}

fn border_part(b: &mut Border, what: &str, t: &str, em: f32, rem: f32) {
    match what {
        "style" => {
            let s = match t {
                "solid" | "groove" | "ridge" | "inset" | "outset" => Some(LineStyle::Solid),
                "dashed" => Some(LineStyle::Dashed),
                "dotted" => Some(LineStyle::Dotted),
                "double" => Some(LineStyle::Double),
                "none" | "hidden" => Some(LineStyle::None),
                _ => None,
            };
            if let Some(s) = s {
                b.style = s;
            }
        }
        "width" => {
            let w = match t {
                "thin" => Some(1.0 / 16.0),
                "medium" => Some(3.0 / 16.0),
                "thick" => Some(5.0 / 16.0),
                _ => match length(t, em, rem) {
                    Some(Len::Base(v)) => Some(v),
                    _ => None,
                },
            };
            if let Some(w) = w {
                b.width = w;
            }
        }
        "color" => {
            if let Some(c) = color(t) {
                b.color = c;
            }
        }
        _ => {}
    }
}

fn weight_bold(v: &str, parent: bool) -> bool {
    match v {
        "bold" | "bolder" => true,
        "normal" | "lighter" => false,
        _ => v.parse::<f32>().map(|n| n >= 600.0).unwrap_or(parent),
    }
}

fn font_shorthand(style: &mut Style, v: &str) {
    let lower = v.to_ascii_lowercase();
    let mut tokens = lower.split_whitespace().peekable();
    style.bold = false;
    style.italic = false;
    while let Some(t) = tokens.peek() {
        match *t {
            "italic" | "oblique" => style.italic = true,
            "bold" | "bolder" | "600" | "700" | "800" | "900" => style.bold = true,
            "normal" | "small-caps" | "lighter" | "100" | "200" | "300" | "400" | "500" => {}
            _ => break,
        }
        tokens.next();
    }
    // Size[/line-height], then the families (font size itself is set earlier).
    if let Some(size) = tokens.next()
        && let Some((_, lh)) = size.split_once('/')
    {
        line_height(style, lh, style.font, style.font);
    }
    let rest: Vec<&str> = tokens.collect();
    if let Some(f) = family(&rest.join(" ")) {
        style.family = f;
    }
}

fn line_height(style: &mut Style, v: &str, em: f32, rem: f32) {
    if v == "normal" {
        style.line_height = None;
        style.line_height_abs = None;
    } else if let Ok(n) = v.parse::<f32>() {
        style.line_height = Some(n);
        style.line_height_abs = None;
    } else if let Some(p) = v
        .strip_suffix('%')
        .and_then(|n| n.trim().parse::<f32>().ok())
    {
        style.line_height = Some(p / 100.0);
        style.line_height_abs = None;
    } else if let Some(n) = v
        .strip_suffix("em")
        .filter(|_| !v.ends_with("rem"))
        .and_then(|n| n.trim().parse::<f32>().ok())
    {
        style.line_height = Some(n);
        style.line_height_abs = None;
    } else if let Some(Len::Base(b)) = length(v, em, rem) {
        style.line_height_abs = Some(b);
        style.line_height = None;
    }
}

/// The generic family a `font-family` list asks for, from its first
/// recognisable name.
fn family(v: &str) -> Option<Family> {
    for name in v.split(',') {
        let n = name.trim().trim_matches(['"', '\'']).to_lowercase();
        if n.is_empty() {
            continue;
        }
        if n == "monospace" || n.starts_with("sans-serif") {
            return Some(Family::Sans);
        }
        const SANS: &[&str] = &[
            "sans",
            "gothic",
            "hei",
            "黑",
            "ゴシック",
            "yuan",
            "圆",
            "圓",
            "pingfang",
            "yahei",
            "jhenghei",
            "arial",
            "helvetica",
            "verdana",
            "dengxian",
            "等线",
        ];
        const SERIF: &[&str] = &[
            "serif", "ming", "明", "song", "宋", "mincho", "kai", "楷", "fangsong", "仿宋",
            "times", "georgia", "cursive", "fantasy",
        ];
        if SANS.iter().any(|k| n.contains(k)) {
            return Some(Family::Sans);
        }
        if SERIF.iter().any(|k| n.contains(k)) {
            return Some(Family::Serif);
        }
    }
    None
}

/// A font size in base units.
fn font_size(v: &str, parent: f32, root: f32) -> Option<f32> {
    let v = v.trim().to_ascii_lowercase();
    let k = match v.as_str() {
        "xx-small" => 9.0 / 16.0,
        "x-small" => 10.0 / 16.0,
        "small" => 13.0 / 16.0,
        "medium" => 1.0,
        "large" => 18.0 / 16.0,
        "x-large" => 24.0 / 16.0,
        "xx-large" => 2.0,
        "xxx-large" => 3.0,
        "smaller" => return Some(parent / 1.2),
        "larger" => return Some(parent * 1.2),
        _ => {
            return match length(&v, parent, root)? {
                Len::Base(b) => Some(b.clamp(0.3, 6.0)),
                Len::Pct(p) => Some((parent * p / 100.0).clamp(0.3, 6.0)),
                Len::Auto => None,
            };
        }
    };
    Some(k)
}

/// The links' colour in [`UA_SHEET`]; drawn in the theme's link colour.
pub(super) const UA_LINK: Color32 = Color32::from_rgb(0x26, 0x8b, 0xd2);

/// A CSS length; `em` and `rem` are the element's and the root's font size.
/// The ink `currentColor` means where a style leaves the colour to the theme.
const THEME_INK: Color32 = Color32::from_rgb(0x1f, 0x3c, 0x46);

/// `color-mix(in srgb, <a> N%, <b>)` with `transparent` as one side (what
/// dictionaries use to tint with the text colour): `<a>` at N% opacity.
/// `current` is the element's colour (`None`: the theme's).
fn color_mix(v: &str, current: Option<Color32>) -> Option<Option<Color32>> {
    let inner = v.trim().strip_prefix("color-mix(")?.strip_suffix(')')?;
    let parts = super::css::split_top(inner, ',');
    if parts.len() != 3 || !parts[0].contains("srgb") {
        return None;
    }
    let side = |s: &str| -> Option<(Option<Color32>, Option<f32>)> {
        let mut c = None;
        let mut pct = None;
        for t in s.split_whitespace() {
            if let Some(p) = t.strip_suffix('%') {
                pct = p.parse::<f32>().ok().map(|p| p / 100.0);
            } else if t == "currentcolor" {
                c = Some(Some(current.unwrap_or(THEME_INK)));
            } else if t == "transparent" {
                c = Some(None);
            } else {
                c = color(t);
            }
        }
        Some((c?, pct))
    };
    let (a, pa) = side(parts[1])?;
    let (b, pb) = side(parts[2])?;
    let (col, share) = match (a, b) {
        (Some(c), None) => (c, pa.or(pb.map(|p| 1.0 - p)).unwrap_or(0.5)),
        (None, Some(c)) => (c, pb.or(pa.map(|p| 1.0 - p)).unwrap_or(0.5)),
        _ => return None,
    };
    let [r, g, b, a] = col.to_srgba_unmultiplied();
    Some(Some(Color32::from_rgba_unmultiplied(
        r,
        g,
        b,
        (a as f32 * share.clamp(0.0, 1.0)).round() as u8,
    )))
}

fn length(v: &str, em: f32, rem: f32) -> Option<Len> {
    let v = v.trim();
    if v == "auto" {
        return Some(Len::Auto);
    }
    if v == "0" {
        return Some(Len::Base(0.0));
    }
    let num_end = v
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+'))
        .map_or(v.len(), |(i, _)| i);
    let n: f32 = v[..num_end].parse().ok()?;
    let unit = &v[num_end..];
    Some(match unit {
        "px" => Len::Base(n / 16.0),
        "pt" => Len::Base(n * 4.0 / 3.0 / 16.0),
        "pc" => Len::Base(n),
        "in" => Len::Base(n * 6.0),
        "cm" => Len::Base(n * 96.0 / 2.54 / 16.0),
        "mm" => Len::Base(n * 96.0 / 25.4 / 16.0),
        "q" => Len::Base(n * 96.0 / 101.6 / 16.0),
        "em" => Len::Base(n * em),
        "rem" => Len::Base(n * rem),
        "ex" | "ch" => Len::Base(n * em * 0.5),
        "%" => Len::Pct(n),
        "vw" | "vh" | "vmin" | "vmax" => Len::Pct(n),
        "" => Len::Base(n / 16.0),
        _ => return None,
    })
}

/// A colour; `Some(None)` is "the default text colour" (currentColor, black).
fn color(v: &str) -> Option<Option<Color32>> {
    let v = v.trim();
    if let Some(hex) = v.strip_prefix('#') {
        let h = |s: &str| u8::from_str_radix(s, 16).ok();
        let (r, g, b, a) = match hex.len() {
            3 | 4 => {
                let d: Vec<u8> = hex
                    .chars()
                    .filter_map(|c| h(&c.to_string()))
                    .map(|x| x * 17)
                    .collect();
                (
                    d.first()?.to_owned(),
                    *d.get(1)?,
                    *d.get(2)?,
                    d.get(3).copied().unwrap_or(255),
                )
            }
            6 | 8 => (
                h(&hex[0..2])?,
                h(&hex[2..4])?,
                h(&hex[4..6])?,
                hex.get(6..8).and_then(h).unwrap_or(255),
            ),
            _ => return None,
        };
        return Some(Some(Color32::from_rgba_unmultiplied(r, g, b, a)));
    }
    if let Some(args) = v.strip_prefix("rgba(").or_else(|| v.strip_prefix("rgb(")) {
        let args = args.trim_end_matches(')');
        let parts: Vec<f32> = args
            .split([',', ' ', '/'])
            .filter(|s| !s.is_empty())
            .filter_map(|s| {
                let s = s.trim();
                if let Some(p) = s.strip_suffix('%') {
                    p.parse::<f32>().ok().map(|x| x * 2.55)
                } else {
                    s.parse::<f32>().ok()
                }
            })
            .collect();
        if parts.len() < 3 {
            return None;
        }
        let a = parts
            .get(3)
            .map_or(255.0, |a| if *a <= 1.0 { a * 255.0 } else { *a });
        return Some(Some(Color32::from_rgba_unmultiplied(
            parts[0] as u8,
            parts[1] as u8,
            parts[2] as u8,
            a as u8,
        )));
    }
    let named = match v {
        "currentcolor" | "initial" | "unset" => return Some(None),
        "transparent" => Color32::TRANSPARENT,
        "black" => Color32::BLACK,
        "white" => Color32::WHITE,
        "gray" | "grey" => Color32::from_rgb(128, 128, 128),
        "silver" => Color32::from_rgb(192, 192, 192),
        "dimgray" | "dimgrey" => Color32::from_rgb(105, 105, 105),
        "darkgray" | "darkgrey" => Color32::from_rgb(169, 169, 169),
        "lightgray" | "lightgrey" => Color32::from_rgb(211, 211, 211),
        "red" => Color32::from_rgb(255, 0, 0),
        "darkred" | "maroon" => Color32::from_rgb(128, 0, 0),
        "green" => Color32::from_rgb(0, 128, 0),
        "blue" => Color32::from_rgb(0, 0, 255),
        "navy" => Color32::from_rgb(0, 0, 128),
        "purple" => Color32::from_rgb(128, 0, 128),
        "teal" => Color32::from_rgb(0, 128, 128),
        "olive" => Color32::from_rgb(128, 128, 0),
        "orange" => Color32::from_rgb(255, 165, 0),
        "brown" => Color32::from_rgb(165, 42, 42),
        _ => return None,
    };
    Some(Some(named))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(n: &str, v: &str) -> Decl {
        Decl {
            name: n.into(),
            value: v.into(),
            important: false,
        }
    }

    #[test]
    fn lengths_and_sizes() {
        assert_eq!(length("16px", 1.0, 1.0), Some(Len::Base(1.0)));
        assert_eq!(length("2em", 1.5, 1.0), Some(Len::Base(3.0)));
        assert_eq!(length("50%", 1.0, 1.0), Some(Len::Pct(50.0)));
        assert_eq!(font_size("x-large", 1.0, 1.0), Some(1.5));
        assert_eq!(font_size("120%", 1.0, 1.0), Some(1.2));
    }

    #[test]
    fn vertical_book_turns_sides() {
        let parent = Style::default();
        let mut s = parent.inherit();
        let d = [
            decl("-epub-writing-mode", "vertical-rl"),
            decl("margin-right", "2em"),
            decl("margin-top", "1em"),
            decl("padding", "1px 2px 3px 4px"),
        ];
        let refs: Vec<&Decl> = d.iter().collect();
        apply(&mut s, &parent, 1.0, &refs, false);
        assert_eq!(s.writing, Writing::VerticalRl);
        assert_eq!(s.margin[0], Len::Base(2.0)); // right → top
        assert_eq!(s.margin[3], Len::Base(1.0)); // top → left
        assert_eq!(
            s.padding,
            [
                Len::Base(2.0 / 16.0),
                Len::Base(3.0 / 16.0),
                Len::Base(4.0 / 16.0),
                Len::Base(1.0 / 16.0)
            ]
        );
    }

    #[test]
    fn families() {
        assert_eq!(family("\"MS Mincho\", serif"), Some(Family::Serif));
        assert_eq!(family("微软雅黑, sans-serif"), Some(Family::Sans));
        assert_eq!(family("serif-ja-v, serif-ja, serif"), Some(Family::Serif));
        assert_eq!(family("MOEJ"), None);
    }

    #[test]
    fn colours() {
        assert_eq!(color("#fff"), Some(Some(Color32::WHITE)));
        assert_eq!(color("rgb(0, 0, 0)"), Some(Some(Color32::BLACK)));
        assert_eq!(color("nonsense"), None);
    }
}
