//! Laying out a chapter's styled tree into pages.
//!
//! egui shapes and draws the text; line breaking, justification and the
//! block flow are done here. egui's own wrapping breaks before 「 and after
//! 《 and drops leading ideographic spaces when justifying, so each line is
//! measured, broken by East Asian rules (no 、。，」… at the start of a line,
//! no 「《“ at the end, —— and …… never split) and laid out as one galley
//! per run, with the justification as letter spacing.
//!
//! Pages are cut between lines (and images), after the whole chapter has
//! been laid out as one column of the page's width.

use super::fonts::Installed;
use super::images::Images;
use super::style::{Align, Family, Len, LineStyle, Style, VAlign};
use super::tree::{Block, BlockKind, Chapter, Child, Img, Inline, StyleId};
use eframe::egui::epaint::text::{FontsView, LayoutJob, TextFormat};
use eframe::egui::{Color32, FontFamily, FontId, Galley, Pos2, Rect, Stroke, Vec2, pos2, vec2};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    /// The reader's text size, in points.
    pub base: f32,
    pub content_w: f32,
    pub page_h: f32,
    /// Lines are at least this many times their font size apart.
    pub line_spacing: f32,
    pub fonts: Installed,
    pub serif_default: bool,
    pub text: Color32,
    pub link: Color32,
    pub rule: Color32,
}

pub enum Item {
    Text {
        pos: Pos2,
        galley: Arc<Galley>,
        /// Synthetic bold: the bold runs again, this far to the right.
        faux: Option<(Arc<Galley>, f32)>,
    },
    Image {
        rect: Rect,
        path: String,
    },
    Fill {
        rect: Rect,
        color: Color32,
    },
    Line {
        a: Pos2,
        b: Pos2,
        width: f32,
        color: Color32,
        style: LineStyle,
    },
}

/// Something that is never split between pages: a line, an image, a rule.
struct Unit {
    top: f32,
    bottom: f32,
    items: Range<usize>,
    break_before: bool,
    image_only: bool,
    /// Characters of the chapter before it.
    text: usize,
}

pub struct Page {
    pub y0: f32,
    units: Range<usize>,
    /// A lone image: centred on the page.
    pub center: bool,
}

pub struct Link {
    pub rect: Rect,
    pub target: u32,
    unit: usize,
}

pub struct ChapterLayout {
    pub items: Vec<Item>,
    units: Vec<Unit>,
    /// Backgrounds and borders, which may cross pages: (top, bottom, item).
    decos: Vec<(f32, f32, usize)>,
    pub pages: Vec<Page>,
    pub anchors: HashMap<String, usize>,
    pub links: Vec<Link>,
}

impl ChapterLayout {
    /// The page holding the text at `offset` characters.
    pub fn page_of_offset(&self, offset: usize) -> usize {
        self.pages
            .iter()
            .rposition(|p| {
                self.units
                    .get(p.units.start)
                    .is_some_and(|u| u.text <= offset)
            })
            .unwrap_or(0)
    }

    pub fn page_offset(&self, page: usize) -> usize {
        self.pages
            .get(page)
            .and_then(|p| self.units.get(p.units.start))
            .map_or(0, |u| u.text)
    }

    fn page_of_unit(&self, unit: usize) -> usize {
        self.pages
            .iter()
            .position(|p| p.units.contains(&unit) || p.units.start > unit)
            .unwrap_or(self.pages.len().saturating_sub(1))
    }

    pub fn page_of_anchor(&self, id: &str) -> Option<usize> {
        self.anchors.get(id).map(|u| self.page_of_unit(*u))
    }

    /// Items of a page and the offset that places them (flow → page).
    pub fn page_items(&self, page: usize) -> (Vec<usize>, Vec2) {
        let Some(p) = self.pages.get(page) else {
            return (Vec::new(), Vec2::ZERO);
        };
        let first = &self.units[p.units.start];
        let last = &self.units[p.units.end - 1];
        let mut out: Vec<usize> = self
            .decos
            .iter()
            .filter(|(t, b, _)| *b > first.top && *t < last.bottom)
            .map(|d| d.2)
            .collect();
        for u in &self.units[p.units.clone()] {
            out.extend(u.items.clone());
        }
        (out, vec2(0.0, -p.y0))
    }

    /// The page's used height (for centring a lone image).
    pub fn page_height(&self, page: usize) -> f32 {
        self.pages.get(page).map_or(0.0, |p| {
            self.units[p.units.end - 1].bottom - self.units[p.units.start].top
        })
    }

    pub fn page_links(&self, page: usize) -> impl Iterator<Item = &Link> {
        let range = self
            .pages
            .get(page)
            .map(|p| p.units.clone())
            .unwrap_or(0..0);
        self.links.iter().filter(move |l| range.contains(&l.unit))
    }
}

#[derive(Clone, Copy, PartialEq)]
enum AKind {
    Char,
    Image(usize),
    Break,
}

#[derive(Clone, Copy)]
struct Atom {
    ch: char,
    kind: AKind,
    style: StyleId,
    link: Option<u32>,
    ruby: Option<usize>,
    /// Advance (letter spacing included).
    w: f32,
    /// Space before it (ruby wider than its base).
    lead: f32,
}

struct Flow<'a, 'f> {
    p: &'a Params,
    ch: &'a Chapter,
    fonts: &'a mut FontsView<'f>,
    images: &'a mut Images,
    items: Vec<Item>,
    units: Vec<Unit>,
    decos: Vec<(f32, f32, usize)>,
    links: Vec<Link>,
    y: f32,
    margin_pos: f32,
    margin_neg: f32,
    break_next: bool,
    text: usize,
    pending_anchors: Vec<String>,
    anchors: HashMap<String, usize>,
    struts: HashMap<StyleId, (f32, f32)>,
}

pub fn layout(
    ch: &Chapter,
    p: &Params,
    fonts: &mut FontsView<'_>,
    images: &mut Images,
) -> ChapterLayout {
    let mut f = Flow {
        p,
        ch,
        fonts,
        images,
        items: Vec::new(),
        units: Vec::new(),
        decos: Vec::new(),
        links: Vec::new(),
        y: 0.0,
        margin_pos: 0.0,
        margin_neg: 0.0,
        break_next: false,
        text: 0,
        pending_anchors: Vec::new(),
        anchors: HashMap::new(),
        struts: HashMap::new(),
    };
    f.block(&ch.root, 0.0, p.content_w);
    let last = f.units.len().saturating_sub(1);
    for a in std::mem::take(&mut f.pending_anchors) {
        f.anchors.entry(a).or_insert(last);
    }
    // Pages.
    let mut pages = Vec::new();
    let mut cur: Option<(f32, usize)> = None;
    for (i, u) in f.units.iter().enumerate() {
        if let Some((y0, first)) = cur
            && (u.break_before || u.bottom - y0 > p.page_h + 0.5)
        {
            pages.push(Page {
                y0,
                units: first..i,
                center: false,
            });
            cur = None;
        }
        if cur.is_none() {
            cur = Some((u.top, i));
        }
    }
    if let Some((y0, first)) = cur {
        pages.push(Page {
            y0,
            units: first..f.units.len(),
            center: false,
        });
    }
    for pg in &mut pages {
        pg.center = pg.units.len() == 1 && f.units[pg.units.start].image_only;
    }
    ChapterLayout {
        items: f.items,
        units: f.units,
        decos: f.decos,
        pages,
        anchors: f.anchors,
        links: f.links,
    }
}

/// Characters that may not start a line.
const NO_START: &str = "、。，．,.：:；;？?！!）)］]｝}〕〉》」』】〙〗〟’”｠»ヽヾーァィゥェォッャュョヮヵヶぁぃぅぇぉっゃゅょゎゕゖ々〻‐゠–〜～・…‥％%℃°′″";
/// Characters that may not end a line.
const NO_END: &str = "（(［[｛{〔〈《「『【〘〖〝‘“｟«";

fn cjk(c: char) -> bool {
    hanstat::chars::is_wide(c)
        || matches!(c, '\u{3000}'..='\u{303F}' | '\u{FF00}'..='\u{FFEF}' | '\u{2014}' | '\u{2026}' | '“' | '”' | '‘' | '’')
}

fn break_ok(a: &Atom, b: &Atom, ws_wraps: bool) -> bool {
    if !ws_wraps {
        return false;
    }
    if a.ruby.is_some() && a.ruby == b.ruby {
        return false;
    }
    if b.ch == ' ' {
        return false;
    }
    if a.ch == ' ' || a.ch == '\u{3000}' {
        return true;
    }
    if NO_START.contains(b.ch) || NO_END.contains(a.ch) {
        return false;
    }
    if a.ch == b.ch && matches!(a.ch, '—' | '…' | '‥' | '─' | '―') {
        return false;
    }
    if matches!(a.kind, AKind::Image(_)) || matches!(b.kind, AKind::Image(_)) {
        return true;
    }
    if cjk(a.ch) || cjk(b.ch) {
        return true;
    }
    a.ch == '-' && b.ch.is_alphanumeric()
}

impl Flow<'_, '_> {
    fn style(&self, id: StyleId) -> &Style {
        &self.ch.styles[id as usize]
    }

    fn add_margin(&mut self, m: f32) {
        if m >= 0.0 {
            self.margin_pos = self.margin_pos.max(m);
        } else {
            self.margin_neg = self.margin_neg.min(m);
        }
    }

    fn flush_margin(&mut self) {
        self.y += self.margin_pos + self.margin_neg;
        self.margin_pos = 0.0;
        self.margin_neg = 0.0;
    }

    fn force_break(&mut self) {
        self.break_next = true;
        self.margin_pos = 0.0;
        self.margin_neg = 0.0;
    }

    fn push_unit(
        &mut self,
        top: f32,
        bottom: f32,
        items: Range<usize>,
        image_only: bool,
        chars: usize,
    ) {
        let i = self.units.len();
        for a in std::mem::take(&mut self.pending_anchors) {
            self.anchors.entry(a).or_insert(i);
        }
        self.units.push(Unit {
            top,
            bottom,
            items,
            break_before: std::mem::take(&mut self.break_next),
            image_only,
            text: self.text,
        });
        self.text += chars;
    }

    fn family(&self, s: &Style) -> (FontFamily, bool) {
        let f = &self.p.fonts;
        let serif = match s.family {
            Family::Serif => true,
            Family::Sans => false,
            Family::Default => self.p.serif_default,
        };
        match (serif, s.bold) {
            (true, false) => (f.serif.clone(), false),
            (true, true) => (f.serif_bold.clone(), f.serif_faux_bold),
            (false, false) => (f.sans.clone(), false),
            (false, true) => (f.sans_bold.clone(), f.sans_faux_bold),
        }
    }

    fn font_px(&self, s: &Style) -> f32 {
        (s.font * self.p.base).max(6.0)
    }

    fn line_px(&self, s: &Style) -> f32 {
        let fp = self.font_px(s);
        let css = match (s.line_height_abs, s.line_height) {
            (Some(a), _) => a * self.p.base,
            (None, Some(f)) => f * fp,
            (None, None) => 1.3 * fp,
        };
        css.max(fp * self.p.line_spacing)
    }

    fn color(&self, s: &Style) -> Color32 {
        if s.hidden {
            return Color32::TRANSPARENT;
        }
        match s.color {
            None => self.p.text,
            Some(c) => {
                // The book's black (and near-black) is the theme's ink.
                let [r, g, b, a] = c.to_srgba_unmultiplied();
                if a == 0 {
                    Color32::TRANSPARENT
                } else if (r as u32 + g as u32 + b as u32) < 3 * 70
                    && r.abs_diff(g) < 30
                    && g.abs_diff(b) < 30
                {
                    self.p.text
                } else {
                    c
                }
            }
        }
    }

    fn format(&self, s: &Style, extra: f32) -> TextFormat {
        let fp = self.font_px(s);
        let (family, _) = self.family(s);
        let color = self.color(s);
        let deco = Stroke::new((fp / 16.0).max(0.8), color);
        TextFormat {
            font_id: FontId::new(fp, family),
            extra_letter_spacing: s.letter_spacing * self.p.base + extra,
            line_height: Some(self.line_px(s)),
            color,
            italics: s.italic,
            underline: if s.underline { deco } else { Stroke::NONE },
            strikethrough: if s.strike { deco } else { Stroke::NONE },
            valign: match s.valign {
                VAlign::Super => eframe::egui::Align::TOP,
                _ => eframe::egui::Align::BOTTOM,
            },
            ..Default::default()
        }
    }

    /// (height, baseline) of an empty line in this style.
    fn strut(&mut self, id: StyleId) -> (f32, f32) {
        if let Some(s) = self.struts.get(&id) {
            return *s;
        }
        let mut job = LayoutJob::default();
        job.append(
            "国",
            0.0,
            self.format(&self.ch.styles[id as usize].clone(), 0.0),
        );
        let g = self.fonts.layout_job(job);
        let v = (g.size().y, baseline(&g));
        self.struts.insert(id, v);
        v
    }

    fn block(&mut self, b: &Block, x: f32, avail: f32) {
        let s = self.style(b.style).clone();
        if let Some(id) = &b.id {
            self.pending_anchors.push(id.clone());
        }
        if s.break_before {
            self.force_break();
        }
        let base = self.p.base;
        let m: Vec<Option<f32>> = s.margin.iter().map(|l| l.resolve(base, avail)).collect();
        let pad: Vec<f32> = s
            .padding
            .iter()
            .map(|l| l.px(base, avail).max(0.0))
            .collect();
        let b_kind_rule = matches!(b.kind, BlockKind::Rule);
        let bw: Vec<f32> = s
            .border
            .iter()
            // A rule's border is the rule itself (see `rule`).
            .map(|b| {
                if b.visible() && !b_kind_rule {
                    (b.width * base).max(0.75)
                } else {
                    0.0
                }
            })
            .collect();
        let horizontal_extra = pad[1] + pad[3] + bw[1] + bw[3];
        let ml = m[3].unwrap_or(0.0);
        let mr = m[1].unwrap_or(0.0);
        let mut w = match s.width.resolve(base, avail) {
            Some(w) if !matches!(b.kind, BlockKind::Image(_)) => w.min(avail - horizontal_extra),
            _ => avail - ml - mr - horizontal_extra,
        };
        if let Some(mw) = s.max_width.resolve(base, avail)
            && !matches!(b.kind, BlockKind::Image(_))
        {
            w = w.min(mw);
        }
        w = w.clamp(base.min(avail), avail.max(1.0));
        let total = w + horizontal_extra;
        let left = if s.width != Len::Auto && m[3].is_none() && m[1].is_none() {
            x + ((avail - total) / 2.0).max(0.0)
        } else if m[3].is_none() && m[1].is_some() && s.width != Len::Auto {
            x + (avail - total - mr).max(0.0)
        } else {
            x + ml.clamp(-avail / 2.0, avail)
        };
        self.add_margin(m[0].unwrap_or(0.0));
        let decorated = s.background.is_some() || bw.iter().any(|w| *w > 0.0);
        if decorated || pad[0] > 0.0 || bw[0] > 0.0 {
            self.flush_margin();
        }
        let top = self.y;
        self.y += bw[0] + pad[0];
        let deco_slot = if decorated {
            self.items.push(Item::Fill {
                rect: Rect::NOTHING,
                color: Color32::TRANSPARENT,
            });
            Some(self.items.len() - 1)
        } else {
            None
        };
        let x0 = left + bw[3] + pad[3];
        match &b.kind {
            BlockKind::Flow => {
                let mut first_inline = true;
                for c in &b.children {
                    match c {
                        Child::Block(cb) => self.block(cb, x0, w),
                        Child::Inline(items) => {
                            self.inline(items, b.style, x0, w, first_inline);
                            first_inline = false;
                        }
                    }
                }
            }
            BlockKind::Image(img) => self.block_image(img, &s, x0, w),
            BlockKind::Rule => self.rule(&s, x0, w),
        }
        if pad[2] > 0.0 || bw[2] > 0.0 || decorated {
            self.flush_margin();
        }
        self.y += pad[2] + bw[2];
        let bottom = self.y;
        if let Some(slot) = deco_slot {
            let rect = Rect::from_min_max(pos2(left, top), pos2(left + total, bottom));
            if let Some(bg) = s.background.filter(|c| !near_white(*c)) {
                self.items[slot] = Item::Fill { rect, color: bg };
                self.decos.push((top, bottom, slot));
            }
            for (i, b) in s.border.iter().enumerate() {
                if bw[i] == 0.0 {
                    continue;
                }
                let h = bw[i] / 2.0;
                let (a, z) = match i {
                    0 => (
                        pos2(rect.left(), rect.top() + h),
                        pos2(rect.right(), rect.top() + h),
                    ),
                    1 => (
                        pos2(rect.right() - h, rect.top()),
                        pos2(rect.right() - h, rect.bottom()),
                    ),
                    2 => (
                        pos2(rect.left(), rect.bottom() - h),
                        pos2(rect.right(), rect.bottom() - h),
                    ),
                    _ => (
                        pos2(rect.left() + h, rect.top()),
                        pos2(rect.left() + h, rect.bottom()),
                    ),
                };
                let color = b
                    .color
                    .map(|c| {
                        self.color(&Style {
                            color: Some(c),
                            ..Style::default()
                        })
                    })
                    .unwrap_or(self.p.rule);
                self.items.push(Item::Line {
                    a,
                    b: z,
                    width: bw[i],
                    color,
                    style: b.style,
                });
                self.decos.push((top, bottom, self.items.len() - 1));
            }
        }
        self.add_margin(m[2].unwrap_or(0.0));
        if s.break_after {
            self.force_break();
        }
    }

    /// The size to show an image at, within `avail` × the page.
    fn image_size(&mut self, img: &Img, s: &Style, avail: f32) -> Vec2 {
        let base = self.p.base;
        let page_h = self.p.page_h;
        let natural = self
            .images
            .dims(&img.path)
            .map(|[w, h]| vec2(w as f32, h as f32));
        let aspect = natural
            .or_else(|| Some(vec2(img.attr_w?, img.attr_h?)))
            .map_or(1.0, |n| n.x / n.y.max(1.0));
        if img.fit_page {
            let w = avail;
            let h = (w / aspect).min(page_h);
            return vec2(h * aspect, h);
        }
        let css_w = s.width.resolve(base, avail);
        let css_h = match s.height {
            Len::Pct(p) => Some(p / 100.0 * page_h),
            l => l.resolve(base, avail),
        };
        let mut size = match (css_w, css_h) {
            (Some(w), Some(h)) => vec2(w, h),
            (Some(w), None) => vec2(w, w / aspect),
            (None, Some(h)) => vec2(h * aspect, h),
            (None, None) => {
                let px = img
                    .attr_w
                    .zip(img.attr_h)
                    .map(|(w, h)| vec2(w, h))
                    .or(natural)
                    .unwrap_or(vec2(base * 16.0, base * 16.0 / aspect));
                // Image pixels as CSS pixels against the 16 px default.
                px * (base / 16.0)
            }
        };
        if let Some(mw) = s.max_width.resolve(base, avail)
            && size.x > mw
        {
            size *= mw / size.x;
        }
        if let Some(mh) = match s.max_height {
            Len::Pct(p) => Some(p / 100.0 * page_h),
            l => l.resolve(base, avail),
        } && size.y > mh
        {
            size *= mh / size.y;
        }
        if size.x > avail {
            size *= avail / size.x;
        }
        if size.y > page_h {
            size *= page_h / size.y;
        }
        size.max(vec2(1.0, 1.0))
    }

    fn block_image(&mut self, img: &Img, s: &Style, x0: f32, w: f32) {
        let size = self.image_size(img, s, w);
        self.flush_margin();
        let x = match s.align {
            Align::Start
                if !img.fit_page && size.x < w * 0.9 && s.float != super::style::Float::None =>
            {
                if s.float == super::style::Float::Right {
                    x0 + w - size.x
                } else {
                    x0
                }
            }
            Align::End => x0 + w - size.x,
            Align::Start if !img.fit_page && size.x < w * 0.9 && s.margin[3] != Len::Auto => {
                // Block images are centred unless they say otherwise.
                x0 + (w - size.x) / 2.0
            }
            _ => x0 + (w - size.x) / 2.0,
        };
        let rect = Rect::from_min_size(pos2(x, self.y), size);
        self.items.push(Item::Image {
            rect,
            path: img.path.clone(),
        });
        let n = self.items.len();
        let top = self.y;
        self.y += size.y;
        self.push_unit(top, self.y, n - 1..n, true, 0);
    }

    fn rule(&mut self, s: &Style, x0: f32, w: f32) {
        self.flush_margin();
        let b = s
            .border
            .iter()
            .find(|b| b.visible())
            .copied()
            .unwrap_or_default();
        let width = if b.visible() {
            (b.width * self.p.base).max(0.75)
        } else {
            0.75
        };
        let color = b
            .color
            .map(|c| {
                self.color(&Style {
                    color: Some(c),
                    ..Style::default()
                })
            })
            .unwrap_or(self.p.rule);
        let top = self.y;
        self.items.push(Item::Line {
            a: pos2(x0, top + width / 2.0),
            b: pos2(x0 + w, top + width / 2.0),
            width,
            color,
            style: if b.visible() {
                b.style
            } else {
                LineStyle::Solid
            },
        });
        let n = self.items.len();
        self.y += width;
        self.push_unit(top, self.y, n - 1..n, false, 0);
    }

    fn inline(&mut self, items: &[Inline], block_style: StyleId, x0: f32, w: f32, first: bool) {
        let ch = self.ch;
        let bs = ch.styles[block_style as usize].clone();
        // Atoms.
        let mut atoms: Vec<Atom> = Vec::new();
        let mut imgs: Vec<(Img, StyleId, Vec2)> = Vec::new();
        let mut rubies: Vec<(Arc<Galley>, usize, usize)> = Vec::new(); // (annotation, first atom, end atom)
        let mut anchors: Vec<(usize, String)> = Vec::new();
        let mut ruby_open: Option<usize> = None;
        let mut widths: HashMap<(char, StyleId), f32> = HashMap::new();
        for it in items {
            match it {
                Inline::Text { text, style, link } => {
                    let s = &ch.styles[*style as usize];
                    let fid = FontId::new(self.font_px(s), self.family(s).0);
                    let ls = s.letter_spacing * self.p.base;
                    for c in text.chars() {
                        let w = *widths
                            .entry((c, *style))
                            .or_insert_with(|| self.fonts.glyph_width(&fid, c) + ls);
                        atoms.push(Atom {
                            ch: c,
                            kind: AKind::Char,
                            style: *style,
                            link: *link,
                            ruby: ruby_open,
                            w,
                            lead: 0.0,
                        });
                    }
                }
                Inline::Image { img, style, link } => {
                    let s = self.ch.styles[*style as usize].clone();
                    let size = self.image_size(img, &s, w);
                    imgs.push((img.clone(), *style, size));
                    atoms.push(Atom {
                        ch: '\u{FFFC}',
                        kind: AKind::Image(imgs.len() - 1),
                        style: *style,
                        link: *link,
                        ruby: ruby_open,
                        w: size.x,
                        lead: 0.0,
                    });
                }
                Inline::Break => atoms.push(Atom {
                    ch: '\n',
                    kind: AKind::Break,
                    style: block_style,
                    link: None,
                    ruby: None,
                    w: 0.0,
                    lead: 0.0,
                }),
                Inline::Anchor(id) => anchors.push((atoms.len(), id.clone())),
                Inline::RubyStart { annotation, style } => {
                    let s = self.ch.styles[*style as usize].clone();
                    let mut job = LayoutJob::default();
                    job.append(annotation, 0.0, self.format(&s, 0.0));
                    let g = self.fonts.layout_job(job);
                    rubies.push((g, atoms.len(), atoms.len()));
                    ruby_open = Some(rubies.len() - 1);
                }
                Inline::RubyEnd => {
                    if let Some(r) = ruby_open.take() {
                        rubies[r].2 = atoms.len();
                    }
                }
            }
        }
        if let Some(r) = ruby_open {
            rubies[r].2 = atoms.len();
        }
        // A ruby wider than its base spreads the base.
        for (g, a, z) in &rubies {
            if z <= a {
                continue;
            }
            let base_w: f32 = atoms[*a..*z].iter().map(|x| x.w).sum();
            let pad = (g.size().x - base_w) / 2.0;
            if pad > 0.0 {
                atoms[*a].lead += pad;
                atoms[*z - 1].w += pad;
            }
        }
        if atoms.is_empty() {
            for (_, id) in anchors {
                self.pending_anchors.push(id);
            }
            return;
        }
        let wraps = bs.white_space.wraps();
        let indent = if first {
            bs.indent.px(self.p.base, w)
        } else {
            0.0
        };
        // Lines.
        let n = atoms.len();
        let mut start = 0;
        let mut line_no = 0;
        let mut anchor_iter = anchors.into_iter().peekable();
        while start < n {
            let avail = w - if line_no == 0 { indent } else { 0.0 };
            let mut x = 0.0;
            let mut last_ok: Option<usize> = None;
            let mut end = n;
            let mut next = n;
            let mut forced = false;
            let mut i = start;
            while i < n {
                let a = &atoms[i];
                if a.kind == AKind::Break {
                    end = i;
                    next = i + 1;
                    forced = true;
                    break;
                }
                if i > start
                    && break_ok(
                        &atoms[i - 1],
                        a,
                        wraps && self.style(a.style).white_space.wraps(),
                    )
                {
                    last_ok = Some(i);
                }
                if a.ch == ' ' {
                    x += a.w;
                    i += 1;
                    continue;
                }
                if x + a.lead + a.w > avail + 0.01 && i > start {
                    let b = last_ok.unwrap_or(i);
                    end = b;
                    next = b;
                    break;
                }
                x += a.lead + a.w;
                i += 1;
            }
            if i >= n && !forced {
                end = n;
                next = n;
            }
            let last_line = next >= n;
            // Anchors of this line.
            while anchor_iter
                .peek()
                .is_some_and(|(at, _)| *at < next.max(start + 1))
            {
                let (_, id) = anchor_iter.next().unwrap();
                self.pending_anchors.push(id);
            }
            self.line(
                &atoms[start..end],
                &imgs,
                &rubies,
                start,
                &bs,
                block_style,
                x0,
                w,
                if line_no == 0 { indent } else { 0.0 },
                !(last_line || forced),
            );
            start = next;
            line_no += 1;
        }
        for (_, id) in anchor_iter {
            self.pending_anchors.push(id);
        }
    }

    #[expect(clippy::too_many_arguments)]
    fn line(
        &mut self,
        atoms: &[Atom],
        imgs: &[(Img, StyleId, Vec2)],
        rubies: &[(Arc<Galley>, usize, usize)],
        offset: usize,
        bs: &Style,
        block_style: StyleId,
        x0: f32,
        w: f32,
        indent: f32,
        justify_ok: bool,
    ) {
        // Trailing spaces hang.
        let mut len = atoms.len();
        while len > 0 && atoms[len - 1].ch == ' ' {
            len -= 1;
        }
        let atoms = &atoms[..len];
        let natural: f32 = atoms.iter().map(|a| a.lead + a.w).sum();
        let avail = w - indent;
        let mut extra_all = 0.0;
        let mut extra_space = 0.0;
        let mut shift = 0.0;
        match bs.align {
            Align::Justify if justify_ok && atoms.len() > 1 && natural < avail => {
                let spaces = atoms.iter().filter(|a| a.ch == ' ').count();
                let has_cjk = atoms.iter().any(|a| cjk(a.ch));
                let gap = avail - natural;
                if !has_cjk && spaces > 0 {
                    extra_space = gap / spaces as f32;
                } else if gap / ((atoms.len() - 1) as f32) < self.p.base * 1.5 {
                    extra_all = gap / (atoms.len() - 1) as f32;
                }
            }
            Align::Center => shift = ((avail - natural) / 2.0).max(0.0),
            Align::End => shift = (avail - natural).max(0.0),
            _ => {}
        }
        // Positions.
        let mut xs = Vec::with_capacity(atoms.len());
        let mut x = x0 + indent + shift;
        for (i, a) in atoms.iter().enumerate() {
            x += a.lead;
            xs.push(x);
            x += a.w;
            if i + 1 < atoms.len() {
                x += extra_all;
                if a.ch == ' ' {
                    x += extra_space;
                }
            }
        }
        // Segments: runs of text between images, ruby bases and (when the
        // spaces take the justification) spaces.
        let mut segs: Vec<(usize, usize)> = Vec::new();
        let mut i = 0;
        while i < atoms.len() {
            let a = &atoms[i];
            if matches!(a.kind, AKind::Image(_)) {
                segs.push((i, i + 1));
                i += 1;
                continue;
            }
            let mut j = i + 1;
            while j < atoms.len()
                && atoms[j].kind == AKind::Char
                && atoms[j].ruby == a.ruby
                && atoms[j].lead == 0.0
                && !(extra_space > 0.0 && atoms[j - 1].ch == ' ')
            {
                j += 1;
            }
            segs.push((i, j));
            i = j;
        }
        // Galleys and vertical metrics.
        let (strut_h, strut_base) = self.strut(block_style);
        let mut above = strut_base;
        let mut below = strut_h - strut_base;
        struct Seg {
            x: f32,
            galley: Option<Arc<Galley>>,
            faux: Option<(Arc<Galley>, f32)>,
            base: f32,
            img: Option<(usize, Vec2)>,
            ruby: Option<usize>,
            ascent: f32,
        }
        let mut out: Vec<Seg> = Vec::new();
        for &(a, z) in &segs {
            let first = &atoms[a];
            if let AKind::Image(k) = first.kind {
                let size = imgs[k].2;
                above = above.max(size.y);
                out.push(Seg {
                    x: xs[a],
                    galley: None,
                    faux: None,
                    base: 0.0,
                    img: Some((k, size)),
                    ruby: None,
                    ascent: 0.0,
                });
                continue;
            }
            let extra = extra_all;
            let mut job = LayoutJob::default();
            let mut faux_job: Option<LayoutJob> = None;
            let mut k = a;
            while k < z {
                let st = atoms[k].style;
                let mut e = k + 1;
                while e < z && atoms[e].style == st {
                    e += 1;
                }
                let text: String = atoms[k..e].iter().map(|x| x.ch).collect();
                let s = self.ch.styles[st as usize].clone();
                let fmt = self.format(&s, extra);
                let faux = s.bold && self.family(&s).1;
                if faux && faux_job.is_none() {
                    // The runs so far, invisible.
                    let mut fj = job.clone();
                    for sec in &mut fj.sections {
                        sec.format.color = Color32::TRANSPARENT;
                        sec.format.underline = Stroke::NONE;
                        sec.format.strikethrough = Stroke::NONE;
                    }
                    faux_job = Some(fj);
                }
                if let Some(fj) = &mut faux_job {
                    let mut f2 = fmt.clone();
                    if !faux {
                        f2.color = Color32::TRANSPARENT;
                    }
                    f2.underline = Stroke::NONE;
                    f2.strikethrough = Stroke::NONE;
                    fj.append(&text, 0.0, f2);
                }
                job.append(&text, 0.0, fmt);
                k = e;
            }
            job.wrap.max_width = f32::INFINITY;
            let g = self.fonts.layout_job(job);
            let fp = self.font_px(&self.ch.styles[first.style as usize]);
            let faux = faux_job.map(|mut fj| {
                fj.wrap.max_width = f32::INFINITY;
                (self.fonts.layout_job(fj), (fp * 0.035).max(0.5))
            });
            let base = baseline(&g);
            let ascent = g
                .rows
                .first()
                .and_then(|r| r.glyphs.first())
                .map_or(base, |gl| gl.font_ascent);
            above = above.max(base);
            below = below.max(g.size().y - base);
            out.push(Seg {
                x: xs[a],
                galley: Some(g),
                faux,
                base,
                img: None,
                ruby: first.ruby,
                ascent,
            });
        }
        // Room for ruby above the base.
        let mut ruby_done = std::collections::HashSet::new();
        for s in &out {
            if let Some(r) = s.ruby
                && ruby_done.insert(r)
            {
                above = above.max(s.ascent + rubies[r].0.size().y + 1.0);
            }
        }
        // Emit.
        self.flush_margin();
        let top = self.y;
        let baseline_y = top + above;
        let first_item = self.items.len();
        let mut ruby_drawn = std::collections::HashSet::new();
        for s in out {
            if let Some((k, size)) = s.img {
                self.items.push(Item::Image {
                    rect: Rect::from_min_size(pos2(s.x, baseline_y - size.y), size),
                    path: imgs[k].0.path.clone(),
                });
                continue;
            }
            let Some(g) = s.galley else { continue };
            let pos = pos2(s.x, baseline_y - s.base);
            if let Some(r) = s.ruby
                && ruby_drawn.insert(r)
            {
                let (rg, ra, rz) = &rubies[r];
                // Centre over the whole base (which may span segments of this line).
                let a_local = ra.saturating_sub(offset);
                let z_local = rz.saturating_sub(offset).min(xs.len());
                if a_local < z_local {
                    let x_start = xs[a_local];
                    let x_end = xs[z_local - 1] + atoms[z_local - 1].w;
                    let rx = (x_start + x_end - rg.size().x) / 2.0;
                    let ry = baseline_y - s.ascent - rg.size().y;
                    self.items.push(Item::Text {
                        pos: pos2(rx, ry),
                        galley: rg.clone(),
                        faux: None,
                    });
                }
            }
            self.items.push(Item::Text {
                pos,
                galley: g,
                faux: s.faux,
            });
        }
        let bottom = baseline_y + below;
        // Links.
        let unit = self.units.len();
        let mut i = 0;
        while i < atoms.len() {
            if let Some(l) = atoms[i].link {
                let mut j = i + 1;
                while j < atoms.len() && atoms[j].link == Some(l) {
                    j += 1;
                }
                let rect =
                    Rect::from_min_max(pos2(xs[i], top), pos2(xs[j - 1] + atoms[j - 1].w, bottom));
                self.links.push(Link {
                    rect,
                    target: l,
                    unit,
                });
                i = j;
            } else {
                i += 1;
            }
        }
        let chars = atoms
            .iter()
            .filter(|a| a.kind == AKind::Char && a.ch != ' ')
            .count();
        let n = self.items.len();
        self.y = bottom;
        self.push_unit(top, bottom, first_item..n, false, chars);
    }
}

fn baseline(g: &Galley) -> f32 {
    g.rows
        .first()
        .and_then(|r| {
            r.glyphs
                .iter()
                .max_by(|a, b| a.font_height.total_cmp(&b.font_height))
                .map(|gl| gl.pos.y + r.pos.y)
        })
        .unwrap_or(g.size().y * 0.8)
}

fn near_white(c: Color32) -> bool {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    a < 10 || (r > 235 && g > 235 && b > 235)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atom(ch: char) -> Atom {
        Atom {
            ch,
            kind: AKind::Char,
            style: 0,
            link: None,
            ruby: None,
            w: 1.0,
            lead: 0.0,
        }
    }

    fn ok(a: char, b: char) -> bool {
        break_ok(&atom(a), &atom(b), true)
    }

    #[test]
    fn east_asian_line_breaking() {
        assert!(ok('的', '人'));
        // Closing punctuation never starts a line; opening never ends one.
        for p in [
            '，', '。', '」', '』', '）', '！', '？', '、', '…', '”', '》',
        ] {
            assert!(!ok('字', p), "{p} at line start");
        }
        for p in ['「', '（', '《', '“'] {
            assert!(!ok(p, '字'), "{p} at line end");
        }
        // Dashes and ellipses stay whole.
        assert!(!ok('—', '—'));
        assert!(!ok('…', '…'));
        assert!(ok('。', '「'));
        // Latin words break at spaces only.
        assert!(!ok('a', 'b'));
        assert!(ok(' ', 'b'));
        assert!(!ok('a', ' '));
        assert!(ok('a', '字'));
        // nowrap
        assert!(!break_ok(&atom('的'), &atom('人'), false));
    }
}
