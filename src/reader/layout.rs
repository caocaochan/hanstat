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
pub(super) struct Params {
    /// The reader's text size, in points.
    pub(super) base: f32,
    pub(super) content_w: f32,
    pub(super) page_h: f32,
    /// Lines are at least this many times their font size apart.
    pub(super) line_spacing: f32,
    pub(super) fonts: Installed,
    pub(super) serif_default: bool,
    pub(super) text: Color32,
    pub(super) link: Color32,
    pub(super) rule: Color32,
    /// The page the text is drawn on; on a `dark` one the book's colours
    /// that would be hard to read there are turned light.
    pub(super) page: Color32,
    pub(super) dark: bool,
    /// Traditional East Asian vertical layout (`vertical-rl`): columns run
    /// top to bottom and progress right to left.
    pub(super) vertical: bool,
}

pub(super) enum Item {
    Text {
        pos: Pos2,
        galley: Arc<Galley>,
        /// Clockwise rotation in radians (0.0 horizontal/upright, π/2 sideways).
        angle: f32,
        /// Synthetic bold: the bold runs again, shifted by this offset.
        faux: Option<(Arc<Galley>, Vec2)>,
    },
    Image {
        rect: Rect,
        path: String,
    },
    Fill {
        rect: Rect,
        color: Color32,
        /// Rounded corners.
        radius: f32,
    },
    /// An outline with rounded corners (an inline box's border).
    Frame {
        rect: Rect,
        radius: f32,
        width: f32,
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
    /// Chinese characters of the chapter before it (reading stats).
    han: u32,
    /// DuChinese translation block id, when this line is blurred until clicked.
    spoiler: Option<u32>,
}

pub(super) struct Page {
    pub(super) y0: f32,
    units: Range<usize>,
    /// A lone image: centred on the page.
    pub(super) center: bool,
}

pub(super) struct Link {
    pub(super) rect: Rect,
    pub(super) target: u32,
    unit: usize,
}

/// One word-sized blur blob inside a [`Spoiler`] line.
#[derive(Clone, Copy)]
pub(super) struct SpoilerBlob {
    pub(super) rect: Rect,
    /// Relative opacity weight (`0.7..=1.0`) so consecutive words undulate like blurred text.
    pub(super) weight: f32,
}

/// One visual line of a blurred DuChinese translation paragraph.
pub(super) struct Spoiler {
    /// Click/hover target (flow coordinates).
    pub(super) rect: Rect,
    /// Font size of this line, used to scale the Gaussian feather width.
    pub(super) font_px: f32,
    /// Per-word boxes painted as smooth Gaussian-feathered clouds while blurred.
    pub(super) blobs: Vec<SpoilerBlob>,
    pub(super) id: u32,
    unit: usize,
}

/// Where a line's characters are, for finding the one under the pointer.
/// Its characters are `start..start + len` of [`Hits`]' arrays.
#[derive(Clone, Copy, Debug)]
pub(super) struct HitLine {
    /// The line box.
    pub(super) top: f32,
    pub(super) bottom: f32,
    /// The text's own height in it (a highlight's top and bottom).
    pub(super) glyph_top: f32,
    pub(super) glyph_bottom: f32,
    unit: usize,
    start: u32,
    len: u32,
}

/// The characters of a laid-out chapter: lines in layout order, and for each
/// character its left edge, advance and index in [`Chapter::chars`]
/// (increasing through the chapter).
#[derive(Default)]
pub(super) struct Hits {
    pub(super) lines: Vec<HitLine>,
    x: Vec<f32>,
    w: Vec<f32>,
    src: Vec<u32>,
}

pub(super) struct ChapterLayout {
    pub(super) items: Vec<Item>,
    units: Vec<Unit>,
    /// Backgrounds and borders, which may cross pages: (block_start, block_end, item).
    decos: Vec<(f32, f32, usize)>,
    pub(super) pages: Vec<Page>,
    pub(super) anchors: HashMap<String, usize>,
    pub(super) links: Vec<Link>,
    pub(super) spoilers: Vec<Spoiler>,
    pub(super) hits: Hits,
    /// Characters of text laid out in the chapter (the unit of [`Unit::text`]).
    pub(super) text_total: usize,
    /// Chinese characters laid out in the chapter.
    pub(super) han_total: u32,
    /// Laid out in `vertical-rl` mode.
    pub(super) vertical: bool,
    /// Content width used when laying out (for right-edge alignment in vertical mode).
    content_w: f32,
}

impl ChapterLayout {
    /// Chinese characters of the chapter before `page` (the whole chapter
    /// past its last page).
    pub(super) fn page_han(&self, page: usize) -> u32 {
        self.pages
            .get(page)
            .and_then(|p| self.units.get(p.units.start))
            .map_or(self.han_total, |u| u.han)
    }

    /// Characters of text laid out on `page`.
    pub(super) fn page_text_len(&self, page: usize) -> usize {
        let start = self.page_offset(page);
        let end = self
            .pages
            .get(page + 1)
            .and_then(|p| self.units.get(p.units.start))
            .map_or(self.text_total, |u| u.text);
        end.saturating_sub(start)
    }

    /// The page holding the text at `offset` characters.
    pub(super) fn page_of_offset(&self, offset: usize) -> usize {
        if offset == 0 {
            return 0;
        }
        let Some(mut p) = self.pages.iter().rposition(|p| {
            self.units
                .get(p.units.start)
                .is_some_and(|u| u.text <= offset)
        }) else {
            return 0;
        };
        // If `offset` lands on trailing zero-text pages (e.g. end-of-chapter
        // illustrations), stay on the first of them rather than jumping to the last.
        if self.page_offset(p) == offset && self.page_text_len(p) == 0 {
            while p > 0 && self.page_offset(p - 1) == offset {
                p -= 1;
            }
        }
        p
    }

    pub(super) fn page_offset(&self, page: usize) -> usize {
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

    pub(super) fn page_of_anchor(&self, id: &str) -> Option<usize> {
        self.anchors.get(id).map(|u| self.page_of_unit(*u))
    }

    /// The flow → page offset for `page`, without collecting its items.
    pub(super) fn page_shift(&self, page: usize) -> Vec2 {
        let Some(p) = self.pages.get(page) else {
            return Vec2::ZERO;
        };
        if self.vertical {
            vec2(self.content_w + p.y0, 0.0)
        } else {
            vec2(0.0, -p.y0)
        }
    }

    /// The height of the whole flow (a popup entry: one long page).
    pub(super) fn flow_height(&self) -> f32 {
        self.units.last().map_or(0.0, |u| u.bottom)
    }

    /// The page's used height (for centring a lone image).
    pub(super) fn page_height(&self, page: usize) -> f32 {
        self.pages.get(page).map_or(0.0, |p| {
            self.units[p.units.end - 1].bottom - self.units[p.units.start].top
        })
    }

    /// Items of a page with each item's spoiler id (if any).
    pub(super) fn page_render_items(&self, page: usize) -> Vec<(usize, Option<u32>)> {
        let Some(p) = self.pages.get(page) else {
            return Vec::new();
        };
        let first = &self.units[p.units.start];
        let last = &self.units[p.units.end - 1];
        let mut out: Vec<(usize, Option<u32>)> = self
            .decos
            .iter()
            .filter(|(t, b, _)| *b > first.top && *t < last.bottom)
            .map(|d| (d.2, None))
            .collect();
        for u in &self.units[p.units.clone()] {
            for idx in u.items.clone() {
                out.push((idx, u.spoiler));
            }
        }
        out
    }

    pub(super) fn page_spoilers(&self, page: usize) -> std::slice::Iter<'_, Spoiler> {
        let Some(p) = self.pages.get(page) else {
            return [].iter();
        };
        let a = self.spoilers.partition_point(|s| s.unit < p.units.start);
        let z = self.spoilers.partition_point(|s| s.unit < p.units.end);
        self.spoilers[a..z].iter()
    }

    pub(super) fn page_links(&self, page: usize) -> std::slice::Iter<'_, Link> {
        let Some(p) = self.pages.get(page) else {
            return [].iter();
        };
        let a = self.links.partition_point(|l| l.unit < p.units.start);
        let z = self.links.partition_point(|l| l.unit < p.units.end);
        self.links[a..z].iter()
    }

    /// The lines of `page`, as a range of [`Hits::lines`].
    fn page_lines(&self, page: usize) -> Range<usize> {
        let Some(p) = self.pages.get(page) else {
            return 0..0;
        };
        let lines = &self.hits.lines;
        let a = lines.partition_point(|l| l.unit < p.units.start);
        let z = lines.partition_point(|l| l.unit < p.units.end);
        a..z
    }

    /// The character at `p` (flow coordinates) on `page`: its index in
    /// [`Chapter::chars`]. Only a point on a character's own box counts.
    pub(super) fn char_at(&self, page: usize, p: Pos2) -> Option<u32> {
        let (b, inline) = if self.vertical {
            (-p.x, p.y)
        } else {
            (p.y, p.x)
        };
        let h = &self.hits;
        let lines = &h.lines[self.page_lines(page)];
        let i = lines.partition_point(|l| l.bottom <= b);
        let l = lines.get(i).filter(|l| l.top <= b)?;
        let (a, z) = (l.start as usize, (l.start + l.len) as usize);
        let k = a + h.x[a..z].partition_point(|x| *x <= inline);
        let k = k.checked_sub(1).filter(|k| *k >= a)?;
        (inline < h.x[k] + h.w[k]).then_some(h.src[k])
    }

    /// The chapter-text boundary nearest the pointer, including in the margins.
    pub(super) fn caret_near(&self, page: usize, p: Pos2) -> Option<u32> {
        let (b, inline) = if self.vertical {
            (-p.x, p.y)
        } else {
            (p.y, p.x)
        };
        let h = &self.hits;
        let lines = &h.lines[self.page_lines(page)];
        let first = lines.first()?;
        let last = lines.last()?;
        if b < first.top {
            return Some(h.src[first.start as usize]);
        }
        if b >= last.bottom {
            return Some(h.src[(last.start + last.len - 1) as usize] + 1);
        }
        let i = lines.partition_point(|l| l.bottom <= b);
        let mut line = &lines[i];
        if i > 0 && b - lines[i - 1].bottom < line.top - b {
            line = &lines[i - 1];
        }
        let a = line.start as usize;
        let z = (line.start + line.len) as usize;
        let k = a + h.x[a..z].partition_point(|x| *x <= inline);
        if k == a {
            return Some(h.src[a]);
        }
        let prev = k - 1;
        let right = h.x[prev] + h.w[prev];
        if inline < (h.x[prev] + right) * 0.5 {
            return Some(h.src[prev]);
        }
        if k < z && h.x[k] - inline < inline - right {
            return Some(h.src[k]);
        }
        Some(h.src[prev] + 1)
    }

    /// Whether the range includes any laid-out characters on this page.
    pub(super) fn shows_text(&self, page: usize, src: &Range<u32>) -> bool {
        let h = &self.hits;
        let lines = &h.lines[self.page_lines(page)];
        let (Some(first), Some(last)) = (lines.first(), lines.last()) else {
            return false;
        };
        let chars = &h.src[first.start as usize..(last.start + last.len) as usize];
        chars
            .get(chars.partition_point(|x| *x < src.start))
            .is_some_and(|x| *x < src.end)
    }

    /// The page holding character `src` of [`Chapter::chars`], or the first
    /// character laid out after it; `None` past the last one.
    pub(super) fn page_of_src(&self, src: u32) -> Option<usize> {
        let h = &self.hits;
        let k = h.src.partition_point(|x| *x < src);
        if k >= h.src.len() {
            return None;
        }
        let i = h.lines.partition_point(|l| (l.start + l.len) as usize <= k);
        h.lines.get(i).map(|l| self.page_of_unit(l.unit))
    }

    /// The first character on `page` (its index in [`Chapter::chars`]).
    pub(super) fn page_first_src(&self, page: usize) -> Option<u32> {
        let h = &self.hits;
        h.lines[self.page_lines(page)]
            .iter()
            .find(|l| l.len > 0)
            .map(|l| h.src[l.start as usize])
    }

    /// The boxes (flow coordinates) of the characters `src` of
    /// [`Chapter::chars`] that are on `page`, one per line.
    pub(super) fn char_rects(&self, page: usize, src: Range<u32>) -> Vec<Rect> {
        let h = &self.hits;
        let mut out = Vec::new();
        for l in &h.lines[self.page_lines(page)] {
            let (a, z) = (l.start as usize, (l.start + l.len) as usize);
            let s = &h.src[a..z];
            let i = a + s.partition_point(|x| *x < src.start);
            let j = a + s.partition_point(|x| *x < src.end);
            if i < j {
                let r = if self.vertical {
                    Rect::from_min_max(
                        pos2(-l.glyph_bottom, h.x[i]),
                        pos2(-l.glyph_top, h.x[j - 1] + h.w[j - 1]),
                    )
                } else {
                    Rect::from_min_max(
                        pos2(h.x[i], l.glyph_top),
                        pos2(h.x[j - 1] + h.w[j - 1], l.glyph_bottom),
                    )
                };
                out.push(r);
            }
        }
        out
    }
}

#[derive(Clone, Copy, PartialEq)]
enum AKind {
    Char,
    Image(usize),
    Break,
    /// An inline box: index into `Flow::boxes`.
    Box(usize),
}

#[derive(Clone)]
enum BoxPiece {
    Text {
        galley: Arc<Galley>,
        x: f32,
        y: f32,
    },
    Image {
        path: String,
        x: f32,
        y: f32,
        size: Vec2,
    },
}

/// An inline box laid out: its content on one line, its padding and border.
struct InBox {
    /// Its content, on one baseline: text runs and images, by x from the
    /// content's left and y from its top.
    pieces: Vec<BoxPiece>,
    style: StyleId,
    /// Outer size (border box).
    size: Vec2,
    /// From the border box's top to the content's baseline.
    base: f32,
    /// Border and padding before the content: left, top.
    inset: Vec2,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum VertGlyph {
    /// Upright character (`angle = 0.0`) with optical shift in ems.
    Upright { ch: char, dx: f32, dy: f32 },
    /// Rotated 90° clockwise (`angle = FRAC_PI_2`) in a sideways run.
    Sideways(char),
}

#[derive(Clone, Copy)]
struct Atom {
    ch: char,
    kind: AKind,
    style: StyleId,
    link: Option<u32>,
    ruby: Option<usize>,
    /// Advance along the inline axis (letter spacing included).
    w: f32,
    /// Space before it (ruby longer than its base).
    lead: f32,
    /// Index in [`Chapter::chars`]; [`NO_SRC`] for images and breaks.
    src: u32,
    /// How the glyph is oriented in vertical mode.
    vert: VertGlyph,
}

const NO_SRC: u32 = u32::MAX;

struct Flow<'a, 'f> {
    p: &'a Params,
    ch: &'a Chapter,
    fonts: &'a mut FontsView<'f>,
    images: &'a mut Images,
    state: &'a mut FlowState,
}

type VertCacheEntry = (Arc<Galley>, Option<(Arc<Galley>, Vec2)>);

#[derive(Default)]
struct FlowState {
    items: Vec<Item>,
    units: Vec<Unit>,
    decos: Vec<(f32, f32, usize)>,
    links: Vec<Link>,
    spoilers: Vec<Spoiler>,
    y: f32,
    margin_pos: f32,
    margin_neg: f32,
    break_next: bool,
    text: usize,
    han: u32,
    pending_anchors: Vec<String>,
    anchors: HashMap<String, usize>,
    struts: HashMap<StyleId, (f32, f32)>,
    /// Glyph advances cached across paragraphs of a chapter.
    widths: HashMap<(char, StyleId), f32>,
    /// Single upright character galleys cached per chapter in vertical mode.
    vert_cache: HashMap<(char, StyleId), VertCacheEntry>,
    hits: Hits,
    /// The inline boxes of the paragraph being laid out.
    boxes: Vec<InBox>,
}

struct BlockFrame {
    next_child: usize,
    first_inline: bool,
    x: f32,
    width: f32,
    left: f32,
    total: f32,
    top: f32,
    bottom_pad: f32,
    bottom_margin: f32,
    border: [f32; 4],
    deco_slot: Option<usize>,
    spoiler: Option<u32>,
}

/// A chapter laid out across frames using the same fonts and geometry.
#[derive(Default)]
pub(super) struct LayoutWork {
    state: FlowState,
    stack: Vec<BlockFrame>,
}

impl LayoutWork {
    pub(super) fn advance(
        &mut self,
        ch: &Chapter,
        p: &Params,
        fonts: &mut FontsView<'_>,
        images: &mut Images,
        deadline: Option<std::time::Instant>,
    ) -> Option<ChapterLayout> {
        let mut flow = Flow {
            p,
            ch,
            fonts,
            images,
            state: &mut self.state,
        };
        if self.stack.is_empty() {
            let inline_len = if p.vertical { p.page_h } else { p.content_w };
            self.stack
                .push(flow.start_block(&ch.root, 0.0, inline_len, None));
        }
        while let Some((frame, parents)) = self.stack.split_last_mut() {
            let mut block = &ch.root;
            for parent in parents {
                let Child::Block(child) = &block.children[parent.next_child - 1] else {
                    unreachable!("only a block puts a frame on the stack");
                };
                block = child;
            }
            let child = if matches!(block.kind, BlockKind::Flow) {
                block.children.get(frame.next_child)
            } else {
                None
            };
            let (x, width, spoiler) = (frame.x, frame.width, frame.spoiler);
            match child {
                Some(Child::Block(child)) => {
                    frame.next_child += 1;
                    self.stack.push(flow.start_block(child, x, width, spoiler));
                }
                Some(Child::Inline(items)) => {
                    flow.inline(items, block.style, x, width, frame.first_inline, spoiler);
                    frame.first_inline = false;
                    frame.next_child += 1;
                }
                None => flow.end_block(block, self.stack.pop().unwrap()),
            }
            if !self.stack.is_empty() && deadline.is_some_and(|t| std::time::Instant::now() >= t) {
                return None;
            }
        }
        let page_block = if p.vertical { p.content_w } else { p.page_h };
        Some(finish(
            std::mem::take(&mut self.state),
            page_block,
            p.vertical,
            p.content_w,
        ))
    }
}

pub(super) fn layout(
    ch: &Chapter,
    p: &Params,
    fonts: &mut FontsView<'_>,
    images: &mut Images,
) -> ChapterLayout {
    LayoutWork::default()
        .advance(ch, p, fonts, images, None)
        .unwrap()
}

fn finish(mut f: FlowState, page_block: f32, vertical: bool, content_w: f32) -> ChapterLayout {
    let last = f.units.len().saturating_sub(1);
    for a in std::mem::take(&mut f.pending_anchors) {
        f.anchors.entry(a).or_insert(last);
    }
    // Pages.
    let mut pages = Vec::new();
    let mut cur: Option<(f32, usize)> = None;
    for (i, u) in f.units.iter().enumerate() {
        if let Some((y0, first)) = cur
            && (u.break_before || u.bottom - y0 > page_block + 0.5)
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
        spoilers: f.spoilers,
        hits: f.hits,
        text_total: f.text,
        han_total: f.han,
        vertical,
        content_w,
    }
}

/// Characters that may not start a line.
pub(super) const NO_START: &str = "、。，．,.：:；;？?！!）)］]｝}〕〉》」』】〙〗〟’”｠»ヽヾーァィゥェォッャュョヮヵヶぁぃぅぇぉっゃゅょゎゕゖ々〻‐゠–〜～・…‥％%℃°′″";
/// Characters that may not end a line.
pub(super) const NO_END: &str = "（(［[｛{〔〈《「『【〘〖〝‘“｟«";

fn is_upright_vertical(c: char) -> bool {
    hanstat::chars::is_wide(c)
        || matches!(
            c,
            '\u{3000}'..='\u{303F}'
                | '\u{3040}'..='\u{30FF}'
                | '\u{3100}'..='\u{312F}'
                | '\u{31A0}'..='\u{31BF}'
                | '\u{FE10}'..='\u{FE6F}'
                | '\u{FF01}'..='\u{FF60}'
                | '\u{FFE0}'..='\u{FFE6}'
                | '·'
        )
}

/// `(vertical presentation form, rotatable_fallback)`.
fn vert_form(c: char) -> Option<(char, bool)> {
    Some(match c {
        '「' | '“' => ('﹁', true),
        '」' | '”' => ('﹂', true),
        '『' | '‘' => ('﹃', true),
        '』' | '’' => ('﹄', true),
        '《' => ('︽', true),
        '》' => ('︾', true),
        '〈' | '<' => ('︿', true),
        '〉' | '>' => ('﹀', true),
        '（' | '(' => ('︵', true),
        '）' | ')' => ('︶', true),
        '【' => ('︻', true),
        '】' => ('︼', true),
        '〔' => ('︹', true),
        '〕' => ('︺', true),
        '｛' | '{' => ('︷', true),
        '｝' | '}' => ('︸', true),
        '［' | '[' => ('﹇', true),
        '］' | ']' => ('﹈', true),
        '—' => ('︱', true),
        '–' => ('︲', true),
        '…' => ('︙', true),
        '‥' => ('︰', true),
        '，' => ('︐', false),
        '、' => ('︑', false),
        '。' => ('︒', false),
        '：' => ('︓', false),
        '；' => ('︔', false),
        '！' => ('︕', false),
        '？' => ('︖', false),
        _ => return None,
    })
}

pub(super) fn vert_glyph(c: char, has_vert: impl Fn(char) -> bool) -> VertGlyph {
    if let Some((v, rotatable)) = vert_form(c) {
        if has_vert(v) {
            return VertGlyph::Upright {
                ch: v,
                dx: 0.0,
                dy: 0.0,
            };
        }
        if rotatable {
            return VertGlyph::Sideways(c);
        }
        if matches!(c, '。' | '，' | '、') {
            return VertGlyph::Upright {
                ch: c,
                dx: 0.45,
                dy: -0.45,
            };
        }
        return VertGlyph::Upright {
            ch: c,
            dx: 0.0,
            dy: 0.0,
        };
    }
    if matches!(c, '～' | '〜' | '─' | '―' | '━' | 'ー' | '-') {
        return VertGlyph::Sideways(c);
    }
    // Everything else — Hanzi, digits, Latin letters and symbols — stacks
    // upright (`text-orientation: upright`).
    VertGlyph::Upright {
        ch: c,
        dx: 0.0,
        dy: 0.0,
    }
}

pub(super) fn cjk(c: char) -> bool {
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
            self.state.margin_pos = self.state.margin_pos.max(m);
        } else {
            self.state.margin_neg = self.state.margin_neg.min(m);
        }
    }

    fn flush_margin(&mut self) {
        self.state.y += self.state.margin_pos + self.state.margin_neg;
        self.state.margin_pos = 0.0;
        self.state.margin_neg = 0.0;
    }

    fn force_break(&mut self) {
        self.state.break_next = true;
        self.state.margin_pos = 0.0;
        self.state.margin_neg = 0.0;
    }

    #[expect(clippy::too_many_arguments)]
    fn push_unit(
        &mut self,
        top: f32,
        bottom: f32,
        items: Range<usize>,
        image_only: bool,
        chars: usize,
        han: u32,
        spoiler: Option<u32>,
    ) {
        let i = self.state.units.len();
        for a in std::mem::take(&mut self.state.pending_anchors) {
            self.state.anchors.entry(a).or_insert(i);
        }
        self.state.units.push(Unit {
            top,
            bottom,
            items,
            break_before: std::mem::take(&mut self.state.break_next),
            image_only,
            text: self.state.text,
            han: self.state.han,
            spoiler,
        });
        self.state.text += chars;
        self.state.han += han;
    }

    fn is_serif(&self, s: &Style) -> bool {
        match s.family {
            Family::Serif => true,
            Family::Sans => false,
            Family::Default => self.p.serif_default,
        }
    }

    fn family(&self, s: &Style) -> (FontFamily, bool) {
        let f = &self.p.fonts;
        match (self.is_serif(s), s.bold) {
            (true, false) => (f.serif.clone(), false),
            (true, true) => (f.serif_bold.clone(), f.serif_faux_bold),
            (false, false) => (f.sans.clone(), false),
            (false, true) => (f.sans_bold.clone(), f.sans_faux_bold),
        }
    }

    fn has_vert(&self, s: &Style, ch: char) -> bool {
        if self.is_serif(s) {
            self.p.fonts.serif_vert
        } else {
            self.p.fonts.sans_vert
        }
        .has(ch)
    }

    /// Format for vertical glyphs and sideways runs: tight natural height
    /// (no horizontal line-height leading) and no horizontal underlines.
    fn vert_format(&self, s: &Style, extra: f32) -> TextFormat {
        let fp = self.font_px(s);
        let (family, _) = self.family(s);
        let color = self.color(s);
        TextFormat {
            font_id: FontId::new(fp, family),
            extra_letter_spacing: s.letter_spacing * self.p.base + extra,
            line_height: None,
            color,
            italics: s.italic,
            underline: Stroke::NONE,
            strikethrough: Stroke::NONE,
            valign: match s.valign {
                VAlign::Super => eframe::egui::Align::TOP,
                VAlign::Baseline | VAlign::Sub => eframe::egui::Align::BOTTOM,
            },
            ..Default::default()
        }
    }

    fn vert_galley(&mut self, ch: char, st: StyleId) -> VertCacheEntry {
        if let Some(cached) = self.state.vert_cache.get(&(ch, st)) {
            return cached.clone();
        }
        let s = self.ch.styles[st as usize].clone();
        let mut job = LayoutJob::default();
        let mut fmt = self.vert_format(&s, 0.0);
        fmt.extra_letter_spacing = 0.0;
        let mut buf = [0u8; 4];
        job.append(ch.encode_utf8(&mut buf), 0.0, fmt);
        job.wrap.max_width = f32::INFINITY;
        let g = self.fonts.layout_job(job);
        let faux = (s.bold && self.family(&s).1)
            .then(|| (g.clone(), vec2((self.font_px(&s) * 0.035).max(0.5), 0.0)));
        let entry = (g, faux);
        self.state.vert_cache.insert((ch, st), entry.clone());
        entry
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
        let c = self.color_opaque(s);
        if s.opacity < 1.0 {
            c.gamma_multiply(s.opacity)
        } else {
            c
        }
    }

    fn color_opaque(&self, s: &Style) -> Color32 {
        match s.color {
            None => self.p.text,
            Some(c) => {
                // The book's black (and near-black) is the theme's ink.
                let [r, g, b, a] = c.to_srgba_unmultiplied();
                if a == 0 {
                    Color32::TRANSPARENT
                } else if c == super::style::UA_LINK {
                    self.p.link
                } else if (r as u32 + g as u32 + b as u32) < 3 * 70
                    && r.abs_diff(g) < 30
                    && g.abs_diff(b) < 30
                {
                    let [r, g, b, _] = self.p.text.to_srgba_unmultiplied();
                    Color32::from_rgba_unmultiplied(r, g, b, a)
                } else {
                    super::theme::book_text(c, self.p.page, self.p.dark)
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
                VAlign::Baseline | VAlign::Sub => eframe::egui::Align::BOTTOM,
            },
            ..Default::default()
        }
    }

    /// (height, baseline) of an empty line in this style.
    fn strut(&mut self, id: StyleId) -> (f32, f32) {
        if let Some(s) = self.state.struts.get(&id) {
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
        self.state.struts.insert(id, v);
        v
    }

    fn start_block(
        &mut self,
        b: &Block,
        x: f32,
        avail: f32,
        parent_spoiler: Option<u32>,
    ) -> BlockFrame {
        let s = self.style(b.style).clone();
        if let Some(id) = &b.id {
            self.state.pending_anchors.push(id.clone());
        }
        if s.break_before {
            self.force_break();
        }
        let base = self.p.base;
        let m = s.margin.map(|l| l.resolve(base, avail));
        let pad = s.padding.map(|l| l.px(base, avail).max(0.0));
        let b_kind_rule = matches!(b.kind, BlockKind::Rule);
        let bw = s
            .border
            // A rule's border is the rule itself (see `rule`).
            .map(|b| {
                if b.visible() && !b_kind_rule {
                    (b.width * base).max(0.75)
                } else {
                    0.0
                }
            });
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
        let top = self.state.y;
        self.state.y += bw[0] + pad[0];
        let deco_slot = if decorated {
            self.state.items.push(Item::Fill {
                rect: Rect::NOTHING,
                color: Color32::TRANSPARENT,
                radius: 0.0,
            });
            Some(self.state.items.len() - 1)
        } else {
            None
        };
        let x0 = left + bw[3] + pad[3];
        match &b.kind {
            BlockKind::Flow => {}
            BlockKind::Image(img) => self.block_image(img, &s, x0, w),
            BlockKind::Rule => self.rule(&s, x0, w),
        }
        BlockFrame {
            next_child: 0,
            first_inline: true,
            x: x0,
            width: w,
            left,
            total,
            top,
            bottom_pad: pad[2],
            bottom_margin: m[2].unwrap_or(0.0),
            border: bw,
            deco_slot,
            spoiler: b.spoiler.or(parent_spoiler),
        }
    }

    fn end_block(&mut self, b: &Block, frame: BlockFrame) {
        let s = self.style(b.style).clone();
        let BlockFrame {
            left,
            total,
            top,
            bottom_pad,
            bottom_margin,
            border: bw,
            deco_slot,
            ..
        } = frame;
        if bottom_pad > 0.0 || bw[2] > 0.0 || deco_slot.is_some() {
            self.flush_margin();
        }
        self.state.y += bottom_pad + bw[2];
        let bottom = self.state.y;
        if let Some(slot) = deco_slot {
            let rect = if self.p.vertical {
                Rect::from_min_max(pos2(-bottom, left), pos2(-top, left + total))
            } else {
                Rect::from_min_max(pos2(left, top), pos2(left + total, bottom))
            };
            if let Some(bg) = s.background.filter(|c| !near_white(*c)) {
                let bg = super::theme::book_fill(bg, self.p.dark);
                let bg = if s.opacity < 1.0 {
                    bg.gamma_multiply(s.opacity)
                } else {
                    bg
                };
                self.state.items[slot] = Item::Fill {
                    rect,
                    color: bg,
                    radius: s.radius * self.p.base,
                };
                self.state.decos.push((top, bottom, slot));
            }
            for (i, b) in s.border.iter().enumerate() {
                if bw[i] == 0.0 {
                    continue;
                }
                let h = bw[i] / 2.0;
                // Logical sides [block-start, inline-end, block-end, inline-start]
                // map to [top, right, bottom, left] horizontally and
                // [right, bottom, left, top] in vertical-rl.
                let phys = if self.p.vertical { (i + 1) % 4 } else { i };
                let (a, z) = match phys {
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
                self.state.items.push(Item::Line {
                    a,
                    b: z,
                    width: bw[i],
                    color,
                    style: b.style,
                });
                self.state
                    .decos
                    .push((top, bottom, self.state.items.len() - 1));
            }
        }
        self.add_margin(bottom_margin);
        if s.break_after {
            self.force_break();
        }
    }

    /// The size to show an image at, within the available physical bounds.
    fn image_size(&mut self, img: &Img, s: &Style, avail: f32) -> Vec2 {
        let base = self.p.base;
        let (max_w, max_h) = if self.p.vertical {
            (self.p.content_w, avail)
        } else {
            (avail, self.p.page_h)
        };
        let natural = self
            .images
            .dims(&img.path)
            .map(|[w, h]| vec2(w as f32, h as f32));
        let aspect = natural
            .or_else(|| Some(vec2(img.attr_w?, img.attr_h?)))
            .map_or(1.0, |n| n.x / n.y.max(1.0));
        if img.fit_page {
            let w = max_w;
            let h = (w / aspect).min(max_h);
            return vec2(h * aspect, h);
        }
        let css_w = s.width.resolve(base, max_w);
        let css_h = match s.height {
            Len::Pct(p) => Some(p / 100.0 * max_h),
            l => l.resolve(base, max_h),
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
        if let Some(mw) = s.max_width.resolve(base, max_w)
            && size.x > mw
        {
            size *= mw / size.x;
        }
        if let Some(mh) = match s.max_height {
            Len::Pct(p) => Some(p / 100.0 * max_h),
            l => l.resolve(base, max_h),
        } && size.y > mh
        {
            size *= mh / size.y;
        }
        if size.x > max_w {
            size *= max_w / size.x;
        }
        if size.y > max_h {
            size *= max_h / size.y;
        }
        size.max(vec2(1.0, 1.0))
    }

    fn block_image(&mut self, img: &Img, s: &Style, x0: f32, w: f32) {
        let size = self.image_size(img, s, w);
        self.flush_margin();
        let inline_span = if self.p.vertical { size.y } else { size.x };
        let inline_pos = match s.align {
            Align::Start
                if !img.fit_page
                    && inline_span < w * 0.9
                    && s.float != super::style::Float::None =>
            {
                if s.float == super::style::Float::Right {
                    x0 + w - inline_span
                } else {
                    x0
                }
            }
            Align::End => x0 + w - inline_span,
            // Block images are centred unless they say otherwise.
            _ => x0 + (w - inline_span) / 2.0,
        };
        let top = self.state.y;
        let rect = if self.p.vertical {
            self.state.y += size.x;
            Rect::from_min_size(pos2(-self.state.y, inline_pos), size)
        } else {
            self.state.y += size.y;
            Rect::from_min_size(pos2(inline_pos, top), size)
        };
        self.state.items.push(Item::Image {
            rect,
            path: img.path.clone(),
        });
        let n = self.state.items.len();
        self.push_unit(top, self.state.y, n - 1..n, true, 0, 0, None);
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
        let top = self.state.y;
        let (a, z) = if self.p.vertical {
            let x = -(top + width / 2.0);
            (pos2(x, x0), pos2(x, x0 + w))
        } else {
            (pos2(x0, top + width / 2.0), pos2(x0 + w, top + width / 2.0))
        };
        self.state.items.push(Item::Line {
            a,
            b: z,
            width,
            color,
            style: if b.visible() {
                b.style
            } else {
                LineStyle::Solid
            },
        });
        let n = self.state.items.len();
        self.state.y += width;
        self.push_unit(top, self.state.y, n - 1..n, false, 0, 0, None);
    }

    /// Lay out an inline box's content on one line, within `avail`:
    /// (the box, the margin before it, the margin after it).
    fn inline_box(&mut self, items: &[Inline], style: StyleId, avail: f32) -> (InBox, f32, f32) {
        let s = self.ch.styles[style as usize].clone();
        let base = self.p.base;
        let pad: Vec<f32> = s
            .padding
            .iter()
            .map(|l| l.px(base, avail).max(0.0))
            .collect();
        let bw: Vec<f32> = s
            .border
            .iter()
            .map(|b| {
                if b.visible() {
                    (b.width * base).max(0.75)
                } else {
                    0.0
                }
            })
            .collect();
        let ml = s.margin[3].px(base, avail);
        let mr = s.margin[1].px(base, avail);
        // Text runs and images (a dictionary's glyph for a rare character),
        // nested boxes flattened into them.
        enum Run<'a> {
            Text(&'a str, StyleId),
            Image(&'a Img, StyleId),
        }
        fn flatten<'a>(items: &'a [Inline], out: &mut Vec<Run<'a>>) {
            for it in items {
                match it {
                    Inline::Text { text, style, .. } => out.push(Run::Text(text, *style)),
                    Inline::Image { img, style, .. } => out.push(Run::Image(img, *style)),
                    Inline::Box { items, .. } => flatten(items, out),
                    _ => {}
                }
            }
        }
        let mut runs = Vec::new();
        flatten(items, &mut runs);
        if runs.is_empty() {
            runs.push(Run::Text(" ", style));
        }
        // Pieces with their height above and below the baseline.
        let mut pieces: Vec<(BoxPiece, f32, f32)> = Vec::new();
        let mut x = 0.0;
        let mut job = LayoutJob::default();
        let flush = |job: &mut LayoutJob,
                     x: &mut f32,
                     pieces: &mut Vec<(BoxPiece, f32, f32)>,
                     fonts: &mut FontsView<'_>| {
            if job.sections.is_empty() {
                return;
            }
            let mut j = std::mem::take(job);
            j.wrap.max_width = f32::INFINITY;
            let g = fonts.layout_job(j);
            let b = baseline(&g);
            let (w, h) = (g.size().x, g.size().y);
            pieces.push((
                BoxPiece::Text {
                    galley: g,
                    x: *x,
                    y: 0.0,
                },
                b,
                h - b,
            ));
            *x += w;
        };
        for r in &runs {
            match r {
                Run::Text(t, st) => {
                    let f = self.format(&self.ch.styles[*st as usize].clone(), 0.0);
                    job.append(t, 0.0, f);
                }
                Run::Image(img, st) => {
                    flush(&mut job, &mut x, &mut pieces, self.fonts);
                    let s = self.ch.styles[*st as usize].clone();
                    let size = self.image_size(img, &s, avail);
                    pieces.push((
                        BoxPiece::Image {
                            path: img.path.clone(),
                            x,
                            y: 0.0,
                            size,
                        },
                        size.y,
                        0.0,
                    ));
                    x += size.x;
                }
            }
        }
        flush(&mut job, &mut x, &mut pieces, self.fonts);
        let above = pieces.iter().map(|p| p.1).fold(0.0, f32::max);
        let below = pieces.iter().map(|p| p.2).fold(0.0, f32::max);
        let pieces: Vec<BoxPiece> = pieces
            .into_iter()
            .map(|(p, a, _)| match p {
                BoxPiece::Text { galley, x, .. } => BoxPiece::Text {
                    galley,
                    x,
                    y: above - a,
                },
                BoxPiece::Image { path, x, size, .. } => BoxPiece::Image {
                    path,
                    x,
                    y: above - size.y,
                    size,
                },
            })
            .collect();
        let inset = vec2(bw[3] + pad[3], bw[0] + pad[0]);
        let size = vec2(
            (x + inset.x + pad[1] + bw[1]).min(avail.max(1.0)),
            above + below + inset.y + pad[2] + bw[2],
        );
        let b = InBox {
            base: inset.y + above,
            pieces,
            style,
            size,
            inset,
        };
        (b, ml, mr)
    }

    /// Draw inline box `k` with its content on the baseline `at.y`, from `at.x`.
    fn emit_box(&mut self, k: usize, at: Pos2) {
        let (pieces, style, size, base, inset) = {
            let b = &self.state.boxes[k];
            (b.pieces.clone(), b.style, b.size, b.base, b.inset)
        };
        let s = self.ch.styles[style as usize].clone();
        let rect = Rect::from_min_size(pos2(at.x, at.y - base), size);
        let radius = (s.radius * self.p.base).min(size.y / 2.0);
        if let Some(bg) = s.background {
            let bg = super::theme::book_fill(bg, self.p.dark);
            let bg = if s.opacity < 1.0 {
                bg.gamma_multiply(s.opacity)
            } else {
                bg
            };
            self.state.items.push(Item::Fill {
                rect,
                color: bg,
                radius,
            });
        }
        if let Some(b) = s.border.iter().find(|b| b.visible()) {
            let color = b.color.map_or(self.p.rule, |c| {
                let c = self.color_opaque(&Style {
                    color: Some(c),
                    ..Style::default()
                });
                if s.opacity < 1.0 {
                    c.gamma_multiply(s.opacity)
                } else {
                    c
                }
            });
            let width = (b.width * self.p.base).max(0.75);
            self.state.items.push(Item::Frame {
                rect: rect.shrink(width / 2.0),
                radius,
                width,
                color,
            });
        }
        let origin = rect.min + inset;
        for p in pieces {
            match p {
                BoxPiece::Text { galley, x, y } => self.state.items.push(Item::Text {
                    pos: origin + vec2(x, y),
                    galley,
                    angle: 0.0,
                    faux: None,
                }),
                BoxPiece::Image { path, x, y, size } => self.state.items.push(Item::Image {
                    rect: Rect::from_min_size(origin + vec2(x, y), size),
                    path,
                }),
            }
        }
    }
}

struct Ruby {
    galley: Option<Arc<Galley>>,
    upright: Vec<Arc<Galley>>,
    fp: f32,
    inline_len: f32,
    thickness: f32,
    first: usize,
    end: usize,
}

impl Flow<'_, '_> {
    fn inline(
        &mut self,
        items: &[Inline],
        block_style: StyleId,
        x0: f32,
        w: f32,
        first: bool,
        spoiler: Option<u32>,
    ) {
        let ch = self.ch;
        let bs = ch.styles[block_style as usize].clone();
        let vertical = self.p.vertical;
        // Atoms.
        let mut atoms: Vec<Atom> = Vec::new();
        let mut imgs: Vec<(Img, Vec2)> = Vec::new();
        let mut rubies: Vec<Ruby> = Vec::new();
        let mut anchors: Vec<(usize, String)> = Vec::new();
        let mut ruby_open: Option<usize> = None;
        self.state.boxes.clear();
        for it in items {
            match it {
                Inline::Text {
                    text,
                    style,
                    link,
                    src,
                } => {
                    let s = &ch.styles[*style as usize];
                    let fp = self.font_px(s);
                    let fid = FontId::new(fp, self.family(s).0);
                    let ls = s.letter_spacing * self.p.base;
                    let vert_forms = if self.is_serif(s) {
                        self.p.fonts.serif_vert
                    } else {
                        self.p.fonts.sans_vert
                    };
                    let widths = &mut self.state.widths;
                    let fonts = &mut *self.fonts;
                    for (k, c) in text.chars().enumerate() {
                        let (w, vert) = if vertical {
                            let vg = vert_glyph(c, |vc| vert_forms.has(vc));
                            let adv = match vg {
                                VertGlyph::Upright { ch: ' ', .. } => fp * 0.5 + ls,
                                VertGlyph::Upright { .. } => fp + ls,
                                VertGlyph::Sideways(gc) => *widths
                                    .entry((gc, *style))
                                    .or_insert_with(|| fonts.glyph_width(&fid, gc) + ls),
                            };
                            (adv, vg)
                        } else {
                            let adv = *widths
                                .entry((c, *style))
                                .or_insert_with(|| fonts.glyph_width(&fid, c) + ls);
                            (
                                adv,
                                VertGlyph::Upright {
                                    ch: c,
                                    dx: 0.0,
                                    dy: 0.0,
                                },
                            )
                        };
                        atoms.push(Atom {
                            ch: c,
                            kind: AKind::Char,
                            style: *style,
                            link: *link,
                            ruby: ruby_open,
                            w,
                            lead: 0.0,
                            src: src + k as u32,
                            vert,
                        });
                    }
                }
                Inline::Image { img, style, link } => {
                    let s = self.ch.styles[*style as usize].clone();
                    let size = self.image_size(img, &s, w);
                    imgs.push((img.clone(), size));
                    atoms.push(Atom {
                        ch: '\u{FFFC}',
                        kind: AKind::Image(imgs.len() - 1),
                        style: *style,
                        link: *link,
                        ruby: ruby_open,
                        w: if vertical { size.y } else { size.x },
                        lead: 0.0,
                        src: NO_SRC,
                        vert: VertGlyph::Upright {
                            ch: '\u{FFFC}',
                            dx: 0.0,
                            dy: 0.0,
                        },
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
                    src: NO_SRC,
                    vert: VertGlyph::Upright {
                        ch: '\n',
                        dx: 0.0,
                        dy: 0.0,
                    },
                }),
                Inline::Anchor(id) => anchors.push((atoms.len(), id.clone())),
                Inline::RubyStart { annotation, style } => {
                    let s = self.ch.styles[*style as usize].clone();
                    let rt_fp = self.font_px(&s);
                    let ruby = if vertical && annotation.chars().any(is_upright_vertical) {
                        let mut upright = Vec::new();
                        for rc in annotation.chars() {
                            let rc = match vert_glyph(rc, |vc| self.has_vert(&s, vc)) {
                                VertGlyph::Upright { ch, .. } => ch,
                                VertGlyph::Sideways(ch) => ch,
                            };
                            let mut job = LayoutJob::default();
                            let mut fmt = self.vert_format(&s, 0.0);
                            fmt.extra_letter_spacing = 0.0;
                            let mut buf = [0u8; 4];
                            job.append(rc.encode_utf8(&mut buf), 0.0, fmt);
                            job.wrap.max_width = f32::INFINITY;
                            upright.push(self.fonts.layout_job(job));
                        }
                        let inline_len = upright.len() as f32 * rt_fp;
                        Ruby {
                            galley: None,
                            upright,
                            fp: rt_fp,
                            inline_len,
                            thickness: rt_fp,
                            first: atoms.len(),
                            end: atoms.len(),
                        }
                    } else {
                        let mut job = LayoutJob::default();
                        let fmt = if vertical {
                            self.vert_format(&s, 0.0)
                        } else {
                            self.format(&s, 0.0)
                        };
                        job.append(annotation, 0.0, fmt);
                        job.wrap.max_width = f32::INFINITY;
                        let g = self.fonts.layout_job(job);
                        let sz = g.size();
                        Ruby {
                            galley: Some(g),
                            upright: Vec::new(),
                            fp: rt_fp,
                            inline_len: sz.x,
                            thickness: sz.y,
                            first: atoms.len(),
                            end: atoms.len(),
                        }
                    };
                    rubies.push(ruby);
                    ruby_open = Some(rubies.len() - 1);
                }
                Inline::RubyEnd => {
                    if let Some(r) = ruby_open.take() {
                        rubies[r].end = atoms.len();
                    }
                }
                Inline::Box { items, style, link } => {
                    let (b, lead, trail) = self.inline_box(items, *style, w);
                    let outer = b.size.x;
                    self.state.boxes.push(b);
                    atoms.push(Atom {
                        ch: '\u{FFFC}',
                        kind: AKind::Box(self.state.boxes.len() - 1),
                        style: *style,
                        link: *link,
                        ruby: ruby_open,
                        w: outer + trail,
                        lead,
                        src: NO_SRC,
                        vert: VertGlyph::Upright {
                            ch: '\u{FFFC}',
                            dx: 0.0,
                            dy: 0.0,
                        },
                    });
                }
            }
        }
        if let Some(r) = ruby_open {
            rubies[r].end = atoms.len();
        }
        // A ruby longer than its base spreads the base.
        for r in &rubies {
            if r.end <= r.first {
                continue;
            }
            let base_w: f32 = atoms[r.first..r.end].iter().map(|x| x.w).sum();
            let pad = (r.inline_len - base_w) / 2.0;
            if pad > 0.0 {
                atoms[r.first].lead += pad;
                atoms[r.end - 1].w += pad;
            }
        }
        if atoms.is_empty() {
            for (_, id) in anchors {
                self.state.pending_anchors.push(id);
            }
            return;
        }
        let wraps = bs.white_space.wraps();
        let indent = if first {
            bs.indent.px(self.p.base, w)
        } else {
            0.0
        };
        // Lines (or vertical columns).
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
                self.state.pending_anchors.push(id);
            }
            let line_indent = if line_no == 0 { indent } else { 0.0 };
            let justify_ok = !(last_line || forced);
            if vertical {
                self.vertical_line(
                    &atoms[start..end],
                    &imgs,
                    &rubies,
                    start,
                    &bs,
                    x0,
                    w,
                    line_indent,
                    justify_ok,
                    spoiler,
                );
            } else {
                self.line(
                    &atoms[start..end],
                    &imgs,
                    &rubies,
                    start,
                    &bs,
                    block_style,
                    x0,
                    w,
                    line_indent,
                    justify_ok,
                    spoiler,
                );
            }
            start = next;
            line_no += 1;
        }
        for (_, id) in anchor_iter {
            self.state.pending_anchors.push(id);
        }
    }

    #[expect(clippy::too_many_arguments)]
    fn line(
        &mut self,
        atoms: &[Atom],
        imgs: &[(Img, Vec2)],
        rubies: &[Ruby],
        offset: usize,
        bs: &Style,
        block_style: StyleId,
        x0: f32,
        w: f32,
        indent: f32,
        justify_ok: bool,
        spoiler: Option<u32>,
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
        // Segments: runs of text between images, ruby bases, subscripts and
        // (when the spaces take the justification) spaces.
        let is_sub = |st: StyleId| self.ch.styles[st as usize].valign == VAlign::Sub;
        let mut segs: Vec<(usize, usize)> = Vec::new();
        let mut i = 0;
        while i < atoms.len() {
            let a = &atoms[i];
            if matches!(a.kind, AKind::Image(_) | AKind::Box(_)) {
                segs.push((i, i + 1));
                i += 1;
                continue;
            }
            let a_sub = is_sub(a.style);
            let mut j = i + 1;
            while j < atoms.len()
                && atoms[j].kind == AKind::Char
                && atoms[j].ruby == a.ruby
                && atoms[j].lead == 0.0
                && is_sub(atoms[j].style) == a_sub
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
            faux: Option<(Arc<Galley>, Vec2)>,
            base: f32,
            sub_shift: f32,
            img: Option<(usize, Vec2)>,
            bx: Option<usize>,
            ruby: Option<usize>,
            ascent: f32,
        }
        let mut out: Vec<Seg> = Vec::new();
        for &(a, z) in &segs {
            let first = &atoms[a];
            if let AKind::Image(k) = first.kind {
                let size = imgs[k].1;
                above = above.max(size.y);
                out.push(Seg {
                    x: xs[a],
                    galley: None,
                    faux: None,
                    base: 0.0,
                    sub_shift: 0.0,
                    img: Some((k, size)),
                    bx: None,
                    ruby: None,
                    ascent: 0.0,
                });
                continue;
            }
            if let AKind::Box(k) = first.kind {
                let b = &self.state.boxes[k];
                above = above.max(b.base);
                below = below.max(b.size.y - b.base);
                out.push(Seg {
                    x: xs[a],
                    galley: None,
                    faux: None,
                    base: 0.0,
                    sub_shift: 0.0,
                    img: None,
                    bx: Some(k),
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
                (self.fonts.layout_job(fj), vec2((fp * 0.035).max(0.5), 0.0))
            });
            let base = baseline(&g);
            let sub_shift = if is_sub(first.style) { fp * 0.25 } else { 0.0 };
            let ascent = g
                .rows
                .first()
                .and_then(|r| r.glyphs.first())
                .map_or(base, |gl| gl.font_ascent);
            above = above.max(base);
            below = below.max(g.size().y - base + sub_shift);
            out.push(Seg {
                x: xs[a],
                galley: Some(g),
                faux,
                base,
                sub_shift,
                img: None,
                bx: None,
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
                above = above.max(s.ascent + rubies[r].thickness + 1.0);
            }
        }
        // Emit.
        self.flush_margin();
        let top = self.state.y;
        let baseline_y = top + above;
        let first_item = self.state.items.len();
        let mut ruby_drawn = std::collections::HashSet::new();
        for s in out {
            if let Some((k, size)) = s.img {
                self.state.items.push(Item::Image {
                    rect: Rect::from_min_size(pos2(s.x, baseline_y - size.y), size),
                    path: imgs[k].0.path.clone(),
                });
                continue;
            }
            if let Some(k) = s.bx {
                self.emit_box(k, pos2(s.x, baseline_y));
                continue;
            }
            let Some(g) = s.galley else { continue };
            let pos = pos2(s.x, baseline_y - s.base + s.sub_shift);
            if let Some(r) = s.ruby
                && ruby_drawn.insert(r)
                && let Some(rg) = &rubies[r].galley
            {
                let (ra, rz) = (rubies[r].first, rubies[r].end);
                // Centre over the whole base (which may span segments of this line).
                let a_local = ra.saturating_sub(offset);
                let z_local = rz.saturating_sub(offset).min(xs.len());
                if a_local < z_local {
                    let x_start = xs[a_local];
                    let x_end = xs[z_local - 1] + atoms[z_local - 1].w;
                    let rx = (x_start + x_end - rg.size().x) / 2.0;
                    let ry = baseline_y - s.ascent - rg.size().y;
                    self.state.items.push(Item::Text {
                        pos: pos2(rx, ry),
                        galley: rg.clone(),
                        angle: 0.0,
                        faux: None,
                    });
                }
            }
            self.state.items.push(Item::Text {
                pos,
                galley: g,
                angle: 0.0,
                faux: s.faux,
            });
        }
        let bottom = baseline_y + below;
        // Links.
        let unit = self.state.units.len();
        let mut i = 0;
        while i < atoms.len() {
            if let Some(l) = atoms[i].link {
                let mut j = i + 1;
                while j < atoms.len() && atoms[j].link == Some(l) {
                    j += 1;
                }
                let rect =
                    Rect::from_min_max(pos2(xs[i], top), pos2(xs[j - 1] + atoms[j - 1].w, bottom));
                self.state.links.push(Link {
                    rect,
                    target: l,
                    unit,
                });
                i = j;
            } else {
                i += 1;
            }
        }
        // Spoilers (blurred DuChinese sentence translations).
        if let Some(id) = spoiler
            && !atoms.is_empty()
        {
            let x_min = xs[0];
            let x_max = xs[atoms.len() - 1] + atoms[atoms.len() - 1].w;
            let fp = self.font_px(bs);
            let blobs = spoiler_blobs_horizontal(atoms, &xs, baseline_y, fp);
            self.state.spoilers.push(Spoiler {
                rect: Rect::from_min_max(pos2(x_min, top), pos2(x_max, bottom)),
                font_px: fp,
                blobs,
                id,
                unit,
            });
        }
        // Characters, for the dictionary.
        let start = self.state.hits.x.len() as u32;
        for (i, a) in atoms.iter().enumerate() {
            if a.kind == AKind::Char && a.src != NO_SRC {
                self.state.hits.x.push(xs[i]);
                self.state.hits.w.push(a.w);
                self.state.hits.src.push(a.src);
            }
        }
        let len = self.state.hits.x.len() as u32 - start;
        if len > 0 {
            self.state.hits.lines.push(HitLine {
                top,
                bottom,
                glyph_top: baseline_y - strut_base,
                glyph_bottom: baseline_y - strut_base + strut_h,
                unit,
                start,
                len,
            });
        }
        let chars = atoms
            .iter()
            .filter(|a| a.kind == AKind::Char && a.ch != ' ')
            .count();
        let han = atoms
            .iter()
            .filter(|a| a.kind == AKind::Char && a.src != NO_SRC && hanstat::chars::is_han(a.ch))
            .count() as u32;
        let n = self.state.items.len();
        self.state.y = bottom;
        self.push_unit(top, bottom, first_item..n, false, chars, han, spoiler);
    }

    /// Emit one vertical column (`vertical-rl`): characters flow top to bottom
    /// along `y0..y0 + h`, and columns advance right to left (`x = -b`).
    #[expect(clippy::too_many_arguments)]
    fn vertical_line(
        &mut self,
        atoms: &[Atom],
        imgs: &[(Img, Vec2)],
        rubies: &[Ruby],
        offset: usize,
        bs: &Style,
        y0: f32,
        h: f32,
        indent: f32,
        justify_ok: bool,
        spoiler: Option<u32>,
    ) {
        // Trailing spaces hang.
        let mut len = atoms.len();
        while len > 0 && atoms[len - 1].ch == ' ' {
            len -= 1;
        }
        let atoms = &atoms[..len];
        let natural: f32 = atoms.iter().map(|a| a.lead + a.w).sum();
        let avail = h - indent;
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
        // Inline (top-to-bottom) positions.
        let mut ys = Vec::with_capacity(atoms.len());
        let mut y = y0 + indent + shift;
        for (i, a) in atoms.iter().enumerate() {
            y += a.lead;
            ys.push(y);
            y += a.w;
            if i + 1 < atoms.len() {
                y += extra_all;
                if a.ch == ' ' {
                    y += extra_space;
                }
            }
        }
        // Column width around its vertical centerline: `right_half` is toward
        // block-start (+x), `left_half` toward block-end (-x).
        let body_fp = self.font_px(bs);
        let col_pitch = self.line_px(bs);
        let mut right_half = col_pitch * 0.5;
        let mut left_half = col_pitch * 0.5;
        let mut max_fp = body_fp;
        for a in atoms {
            match a.kind {
                AKind::Char => {
                    let s = self.style(a.style);
                    let fp = self.font_px(s);
                    let pitch = self.line_px(s);
                    max_fp = max_fp.max(fp);
                    right_half = right_half.max(pitch * 0.5);
                    left_half = left_half.max(pitch * 0.5);
                }
                AKind::Image(k) => {
                    let size = imgs[k].1;
                    right_half = right_half.max(size.x * 0.5);
                    left_half = left_half.max(size.x * 0.5);
                }
                _ => {}
            }
        }
        // Room for ruby on the right side of the column.
        let mut ruby_seen = std::collections::HashSet::new();
        for a in atoms {
            if let Some(r) = a.ruby
                && ruby_seen.insert(r)
            {
                let base_fp = self.font_px(self.style(a.style));
                right_half = right_half.max(base_fp * 0.5 + rubies[r].thickness + 1.5);
            }
        }
        self.flush_margin();
        let b_start = self.state.y;
        let b_center = b_start + right_half;
        let b_end = b_center + left_half;
        let cx = -b_center;
        let first_item = self.state.items.len();

        // Emit upright chars, rotated sideways runs, and inline images.
        let mut i = 0;
        let mut ruby_drawn = std::collections::HashSet::new();
        while i < atoms.len() {
            let a = &atoms[i];
            if let AKind::Image(k) = a.kind {
                let size = imgs[k].1;
                self.state.items.push(Item::Image {
                    rect: Rect::from_min_size(pos2(cx - size.x * 0.5, ys[i]), size),
                    path: imgs[k].0.path.clone(),
                });
                i += 1;
                continue;
            }
            if a.kind != AKind::Char {
                i += 1;
                continue;
            }
            if let Some(r) = a.ruby
                && ruby_drawn.insert(r)
            {
                let rb = &rubies[r];
                let a_local = rb.first.saturating_sub(offset);
                let z_local = rb.end.saturating_sub(offset).min(ys.len());
                if a_local < z_local {
                    let base_fp = self.font_px(self.style(a.style));
                    let y_start = ys[a_local];
                    let y_end = ys[z_local - 1] + atoms[z_local - 1].w;
                    let rx_center = cx + base_fp * 0.5 + rb.thickness * 0.5 + 1.0;
                    if let Some(rg) = &rb.galley {
                        let ry = (y_start + y_end - rg.size().x) * 0.5;
                        let mid = baseline(rg) - rb.fp * 0.35;
                        self.state.items.push(Item::Text {
                            pos: pos2(rx_center + mid, ry),
                            galley: rg.clone(),
                            angle: std::f32::consts::FRAC_PI_2,
                            faux: None,
                        });
                    } else if !rb.upright.is_empty() {
                        let start_y = (y_start + y_end - rb.inline_len) * 0.5;
                        for (k, g) in rb.upright.iter().enumerate() {
                            let gx = rx_center - g.size().x * 0.5;
                            let gy = start_y + k as f32 * rb.fp + (rb.fp - g.size().y) * 0.5;
                            self.state.items.push(Item::Text {
                                pos: pos2(gx, gy),
                                galley: g.clone(),
                                angle: 0.0,
                                faux: None,
                            });
                        }
                    }
                }
            }
            match a.vert {
                VertGlyph::Upright { ch, dx, dy } => {
                    if ch != ' ' && ch != '\u{3000}' {
                        let fp = self.font_px(self.style(a.style));
                        let (g, faux) = self.vert_galley(ch, a.style);
                        let gx = cx - g.size().x * 0.5 + dx * fp;
                        let gy = ys[i] + (fp - g.size().y) * 0.5 + dy * fp;
                        self.state.items.push(Item::Text {
                            pos: pos2(gx, gy),
                            galley: g,
                            angle: 0.0,
                            faux,
                        });
                    }
                    i += 1;
                }
                VertGlyph::Sideways(_) => {
                    let mut j = i + 1;
                    while j < atoms.len()
                        && atoms[j].kind == AKind::Char
                        && matches!(atoms[j].vert, VertGlyph::Sideways(_))
                        && atoms[j].ruby == a.ruby
                        && atoms[j].lead == 0.0
                        && !(extra_space > 0.0 && atoms[j - 1].ch == ' ')
                    {
                        j += 1;
                    }
                    let mut job = LayoutJob::default();
                    let mut faux_job: Option<LayoutJob> = None;
                    let mut k = i;
                    while k < j {
                        let st = atoms[k].style;
                        let mut e = k + 1;
                        while e < j && atoms[e].style == st {
                            e += 1;
                        }
                        let text: String = atoms[k..e]
                            .iter()
                            .map(|x| match x.vert {
                                VertGlyph::Sideways(gc) => gc,
                                VertGlyph::Upright { ch, .. } => ch,
                            })
                            .collect();
                        let s = self.ch.styles[st as usize].clone();
                        let fmt = self.vert_format(&s, extra_all);
                        let faux = s.bold && self.family(&s).1;
                        if faux && faux_job.is_none() {
                            let mut fj = job.clone();
                            for sec in &mut fj.sections {
                                sec.format.color = Color32::TRANSPARENT;
                            }
                            faux_job = Some(fj);
                        }
                        if let Some(fj) = &mut faux_job {
                            let mut f2 = fmt.clone();
                            if !faux {
                                f2.color = Color32::TRANSPARENT;
                            }
                            fj.append(&text, 0.0, f2);
                        }
                        job.append(&text, 0.0, fmt);
                        k = e;
                    }
                    job.wrap.max_width = f32::INFINITY;
                    let g = self.fonts.layout_job(job);
                    let fp = self.font_px(&self.ch.styles[a.style as usize]);
                    let faux = faux_job.map(|mut fj| {
                        fj.wrap.max_width = f32::INFINITY;
                        (self.fonts.layout_job(fj), vec2(0.0, (fp * 0.035).max(0.5)))
                    });
                    // Rotating a horizontal box by +90° CW around `pos` maps
                    // galley y to `pos.x - y`; align cap-height center to `cx`.
                    let visual_mid = baseline(&g) - fp * 0.35;
                    let pos = pos2(cx + visual_mid, ys[i]);
                    self.state.items.push(Item::Text {
                        pos,
                        galley: g,
                        angle: std::f32::consts::FRAC_PI_2,
                        faux,
                    });
                    i = j;
                }
            }
        }

        // Vertical underline (left of column) and strikethrough (centerline).
        for strike in [false, true] {
            let mut k = 0;
            while k < atoms.len() {
                let s = self.style(atoms[k].style);
                let active =
                    atoms[k].kind == AKind::Char && if strike { s.strike } else { s.underline };
                if !active {
                    k += 1;
                    continue;
                }
                let st = atoms[k].style;
                let mut e = k + 1;
                while e < atoms.len() && atoms[e].kind == AKind::Char && atoms[e].style == st {
                    e += 1;
                }
                let s = self.ch.styles[st as usize].clone();
                let fp = self.font_px(&s);
                let color = self.color(&s);
                let width = (fp / 16.0).max(0.8);
                let x = if strike { cx } else { cx - fp * 0.56 };
                self.state.items.push(Item::Line {
                    a: pos2(x, ys[k]),
                    b: pos2(x, ys[e - 1] + atoms[e - 1].w),
                    width,
                    color,
                    style: LineStyle::Solid,
                });
                k = e;
            }
        }

        // Links.
        let unit = self.state.units.len();
        let mut i = 0;
        while i < atoms.len() {
            if let Some(l) = atoms[i].link {
                let mut j = i + 1;
                while j < atoms.len() && atoms[j].link == Some(l) {
                    j += 1;
                }
                let rect = Rect::from_min_max(
                    pos2(-b_end, ys[i]),
                    pos2(-b_start, ys[j - 1] + atoms[j - 1].w),
                );
                self.state.links.push(Link {
                    rect,
                    target: l,
                    unit,
                });
                i = j;
            } else {
                i += 1;
            }
        }

        // Characters, for selection, dictionary lookup, search and read-aloud.
        let glyph_half = (max_fp * 0.58).min(left_half.min(right_half));
        if let Some(id) = spoiler
            && !atoms.is_empty()
        {
            let y_min = ys[0];
            let y_max = ys[atoms.len() - 1] + atoms[atoms.len() - 1].w;
            let fp = self.font_px(bs);
            let blobs = spoiler_blobs_vertical(atoms, &ys, -b_center, fp);
            self.state.spoilers.push(Spoiler {
                rect: Rect::from_min_max(pos2(-b_end, y_min), pos2(-b_start, y_max)),
                font_px: fp,
                blobs,
                id,
                unit,
            });
        }
        let start = self.state.hits.x.len() as u32;
        for (i, a) in atoms.iter().enumerate() {
            if a.kind == AKind::Char && a.src != NO_SRC {
                self.state.hits.x.push(ys[i]);
                self.state.hits.w.push(a.w);
                self.state.hits.src.push(a.src);
            }
        }
        let len = self.state.hits.x.len() as u32 - start;
        if len > 0 {
            self.state.hits.lines.push(HitLine {
                top: b_start,
                bottom: b_end,
                glyph_top: b_center - glyph_half,
                glyph_bottom: b_center + glyph_half,
                unit,
                start,
                len,
            });
        }
        let chars = atoms
            .iter()
            .filter(|a| a.kind == AKind::Char && a.ch != ' ')
            .count();
        let han = atoms
            .iter()
            .filter(|a| a.kind == AKind::Char && a.src != NO_SRC && hanstat::chars::is_han(a.ch))
            .count() as u32;
        let n = self.state.items.len();
        self.state.y = b_end;
        self.push_unit(b_start, b_end, first_item..n, false, chars, han, spoiler);
    }
}

pub(super) fn baseline(g: &Galley) -> f32 {
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

fn word_weight(slice: &[Atom]) -> f32 {
    let mut h = 0u32;
    let mut dense = 0usize;
    for a in slice {
        h = h.wrapping_mul(31).wrapping_add(a.ch as u32);
        if a.ch.is_alphanumeric() {
            dense += 1;
        }
    }
    let len_bias = (dense.min(7) as f32 / 7.0) * 0.14;
    let jitter = ((h % 11) as f32 / 10.0) * 0.18;
    (0.68 + len_bias + jitter).min(1.0)
}

fn push_word_blobs_horizontal(
    blobs: &mut Vec<SpoilerBlob>,
    atoms: &[Atom],
    xs: &[f32],
    start: usize,
    end: usize,
    baseline_y: f32,
    fp: f32,
) {
    // Split long words into ~2-em chunks so every blob has a consistent
    // aspect ratio and gentle organic undulation along the line.
    let mut s = start;
    while s < end {
        let mut e = s + 1;
        while e < end && (xs[e] - xs[s]) < fp * 1.9 {
            e += 1;
        }
        // Avoid leaving a tiny 1-char orphan chunk at the end of a word.
        if end - e == 1 {
            e = end;
        }
        let slice = &atoms[s..e];
        let x0 = xs[s];
        let x1 = xs[e - 1] + atoms[e - 1].w;
        let has_tall = slice.iter().any(|a| {
            a.ch.is_uppercase() || matches!(a.ch, 'b' | 'd' | 'f' | 'h' | 'k' | 'l' | 't')
        });
        let has_desc = slice
            .iter()
            .any(|a| matches!(a.ch, 'g' | 'j' | 'p' | 'q' | 'y'));
        let y0 = baseline_y - fp * if has_tall { 0.68 } else { 0.54 };
        let y1 = baseline_y + fp * if has_desc { 0.14 } else { 0.02 };
        blobs.push(SpoilerBlob {
            rect: Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)),
            weight: word_weight(slice),
        });
        s = e;
    }
}

fn spoiler_blobs_horizontal(
    atoms: &[Atom],
    xs: &[f32],
    baseline_y: f32,
    fp: f32,
) -> Vec<SpoilerBlob> {
    let mut blobs = Vec::new();
    let mut i = 0;
    while i < atoms.len() {
        while i < atoms.len() && (atoms[i].ch == ' ' || atoms[i].ch == '\u{3000}') {
            i += 1;
        }
        if i >= atoms.len() {
            break;
        }
        let start = i;
        while i < atoms.len() && atoms[i].ch != ' ' && atoms[i].ch != '\u{3000}' {
            i += 1;
        }
        push_word_blobs_horizontal(&mut blobs, atoms, xs, start, i, baseline_y, fp);
    }
    blobs
}

fn spoiler_blobs_vertical(atoms: &[Atom], ys: &[f32], cx: f32, fp: f32) -> Vec<SpoilerBlob> {
    let mut blobs = Vec::new();
    let half = fp * 0.30;
    let mut i = 0;
    while i < atoms.len() {
        while i < atoms.len() && (atoms[i].ch == ' ' || atoms[i].ch == '\u{3000}') {
            i += 1;
        }
        if i >= atoms.len() {
            break;
        }
        let start = i;
        while i < atoms.len() && atoms[i].ch != ' ' && atoms[i].ch != '\u{3000}' {
            i += 1;
        }
        let slice = &atoms[start..i];
        let y0 = ys[start];
        let y1 = ys[i - 1] + atoms[i - 1].w;
        blobs.push(SpoilerBlob {
            rect: Rect::from_min_max(pos2(cx - half, y0), pos2(cx + half, y1)),
            weight: word_weight(slice),
        });
    }
    blobs
}

fn near_white(c: Color32) -> bool {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    a < 10 || (r > 235 && g > 235 && b > 235)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document laid out with the app's fonts, `cols` characters of 20 px wide.
    fn lay_out_html(html: &str, cols: f32) -> (Chapter, ChapterLayout) {
        lay_out_html_in_steps(html, cols, false)
    }

    fn lay_out_html_in_steps(html: &str, cols: f32, sliced: bool) -> (Chapter, ChapterLayout) {
        use super::super::tree::{Source, build_doc, ua_rules};
        let ctx = eframe::egui::Context::default();
        // The bundled LXGW WenKai first: the same glyphs on every machine
        // (CI machines may have no Chinese font, which makes them 0 wide).
        let mut defs = eframe::egui::FontDefinitions::default();
        defs.font_data.insert(
            "wenkai".into(),
            Arc::new(eframe::egui::FontData::from_static(
                crate::png_report::WENKAI,
            )),
        );
        defs.families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "wenkai".into());
        ctx.set_fonts(defs);
        ctx.run_ui(Default::default(), |_| {})
            .textures_delta
            .clear();
        let doc = scraper::Html::parse_document(html);
        let ch = build_doc(
            &doc,
            &ua_rules(),
            &Default::default(),
            Source::Epub {
                base: String::new(),
                path: String::new(),
                duchinese: false,
            },
        );
        let p = Params {
            base: 20.0,
            content_w: 20.0 * cols,
            page_h: 2000.0,
            line_spacing: 1.75,
            fonts: super::super::fonts::Installed {
                serif: FontFamily::Proportional,
                serif_bold: FontFamily::Proportional,
                sans: FontFamily::Proportional,
                sans_bold: FontFamily::Proportional,
                serif_faux_bold: true,
                sans_faux_bold: true,
                serif_vert: super::super::fonts::VertForms::from_font(crate::png_report::WENKAI, 0),
                sans_vert: super::super::fonts::VertForms::from_font(crate::png_report::WENKAI, 0),
                key: Default::default(),
            },
            serif_default: true,
            text: Color32::BLACK,
            link: Color32::BLUE,
            rule: Color32::GRAY,
            page: Color32::WHITE,
            dark: false,
            vertical: false,
        };
        let mut images = Images::with_source("test", Arc::new(|_: &str| None));
        let l = if sliced {
            let mut work = LayoutWork::default();
            let mut result = None;
            let mut frames = 0;
            loop {
                ctx.run_ui(Default::default(), |ui| {
                    if result.is_none() {
                        result = ui.ctx().fonts_mut(|f| {
                            work.advance(&ch, &p, f, &mut images, Some(std::time::Instant::now()))
                        });
                    }
                })
                .textures_delta
                .clear();
                frames += 1;
                if let Some(layout) = result {
                    assert!(frames > 1);
                    break layout;
                }
                assert!(frames < 100, "the chapter never finished");
            }
        } else {
            ctx.fonts_mut(|f| layout(&ch, &p, f, &mut images))
        };
        (ch, l)
    }

    #[test]
    fn nested_blocks_keep_the_same_layout_across_frames() {
        let html = r#"<html><body><div style="margin: 1em; padding: 1em; background: #afa">
            <section><p>第一段。</p><p id="next">第二段，<strong>有点长有点长有点长</strong>。</p></section>
            <p>最后一段。</p></div></body></html>"#;
        let (chapter, full) = lay_out_html(html, 12.0);
        let (_, sliced) = lay_out_html_in_steps(html, 12.0, true);
        assert_eq!(full.items.len(), sliced.items.len());
        assert_eq!(full.decos, sliced.decos);
        assert_eq!(full.page_of_anchor("next"), sliced.page_of_anchor("next"));
        assert_eq!(full.pages.len(), sliced.pages.len());
        for page in 0..full.pages.len() {
            let range = 0..chapter.chars.len() as u32;
            assert_eq!(
                full.char_rects(page, range.clone()),
                sliced.char_rects(page, range)
            );
        }
    }

    /// The text is indexed in layout order with block boundaries, and the
    /// character under a point and the boxes of a range come back from the
    /// layout, across a line break.
    #[test]
    fn characters_under_the_pointer() {
        let (ch, l) = lay_out_html(
            "<html><body><p>你好世界</p><p>第二段<ruby>文<rt>wén</rt></ruby>字很长很长很长很长很长</p></body></html>",
            8.0,
        );
        let text: String = ch.chars.iter().collect();
        assert_eq!(text, "你好世界\0第二段文字很长很长很长很长很长\0");
        // Every character's box finds the character back.
        for src in 0..ch.chars.len() as u32 {
            if ch.chars[src as usize] == '\0' {
                assert!(l.char_rects(0, src..src + 1).is_empty());
                continue;
            }
            let r = l.char_rects(0, src..src + 1);
            assert_eq!(r.len(), 1, "one box for {src}");
            assert_eq!(l.char_at(0, r[0].center()), Some(src), "at {src}");
        }
        // The second paragraph wraps: one box per line, top to bottom. (How
        // many lines depends on the fonts installed: CI machines may have no
        // Chinese font.)
        let lines = l.char_rects(0, 5..21);
        assert!(lines.len() >= 2, "{lines:?}");
        assert!(lines.windows(2).all(|w| w[0].bottom() <= w[1].top() + 0.5));
        // Off the text: nothing.
        let first = l.char_rects(0, 0..4)[0];
        assert_eq!(l.char_at(0, first.right_center() + vec2(5.0, 0.0)), None);
        assert_eq!(
            l.char_at(0, pos2(first.left() - 5.0, first.center().y)),
            None
        );
        assert_eq!(l.char_at(0, pos2(10.0, -50.0)), None);
        assert_eq!(l.char_at(5, pos2(10.0, 10.0)), None, "no such page");
    }

    fn atom(ch: char) -> Atom {
        Atom {
            ch,
            kind: AKind::Char,
            style: 0,
            link: None,
            ruby: None,
            w: 1.0,
            lead: 0.0,
            src: 0,
            vert: VertGlyph::Upright {
                ch,
                dx: 0.0,
                dy: 0.0,
            },
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

    fn lay_out_vertical(
        html: &str,
        rows: f32,
        page_w: f32,
        sliced: bool,
    ) -> (Chapter, ChapterLayout) {
        use super::super::tree::{Source, build_doc, ua_rules};
        let ctx = eframe::egui::Context::default();
        let mut defs = eframe::egui::FontDefinitions::default();
        defs.font_data.insert(
            "wenkai".into(),
            Arc::new(eframe::egui::FontData::from_static(
                crate::png_report::WENKAI,
            )),
        );
        defs.families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "wenkai".into());
        ctx.set_fonts(defs);
        ctx.run_ui(Default::default(), |_| {})
            .textures_delta
            .clear();
        let doc = scraper::Html::parse_document(html);
        let ch = build_doc(
            &doc,
            &ua_rules(),
            &Default::default(),
            Source::Epub {
                base: String::new(),
                path: String::new(),
                duchinese: false,
            },
        );
        let vert = super::super::fonts::VertForms::from_font(crate::png_report::WENKAI, 0);
        let p = Params {
            base: 20.0,
            content_w: page_w,
            page_h: 20.0 * rows,
            line_spacing: 1.75,
            fonts: super::super::fonts::Installed {
                serif: FontFamily::Proportional,
                serif_bold: FontFamily::Proportional,
                sans: FontFamily::Proportional,
                sans_bold: FontFamily::Proportional,
                serif_faux_bold: true,
                sans_faux_bold: true,
                serif_vert: vert,
                sans_vert: vert,
                key: Default::default(),
            },
            serif_default: true,
            text: Color32::BLACK,
            link: Color32::BLUE,
            rule: Color32::GRAY,
            page: Color32::WHITE,
            dark: false,
            vertical: true,
        };
        let mut images = Images::with_source("test", Arc::new(|_: &str| None));
        let l = if sliced {
            let mut work = LayoutWork::default();
            let mut result = None;
            let mut frames = 0;
            loop {
                ctx.run_ui(Default::default(), |ui| {
                    if result.is_none() {
                        result = ui.ctx().fonts_mut(|f| {
                            work.advance(&ch, &p, f, &mut images, Some(std::time::Instant::now()))
                        });
                    }
                })
                .textures_delta
                .clear();
                frames += 1;
                if let Some(layout) = result {
                    break layout;
                }
                assert!(frames < 100, "vertical chapter never finished");
            }
        } else {
            ctx.fonts_mut(|f| layout(&ch, &p, f, &mut images))
        };
        (ch, l)
    }

    #[test]
    fn vertical_columns_hit_testing_and_slicing() {
        let html = r#"<html><body>
            <p>你好世界</p>
            <p>第二段<ruby>文<rt>wén</rt></ruby>字很长很长很长很长很长，还有ABC123。</p>
        </body></html>"#;
        let (ch, l) = lay_out_vertical(html, 8.0, 800.0, false);
        assert!(l.vertical);
        assert_eq!(l.pages.len(), 1);
        // Every non-sentinel character round-trips through char_rects and char_at.
        for src in 0..ch.chars.len() as u32 {
            if ch.chars[src as usize] == '\0' {
                assert!(l.char_rects(0, src..src + 1).is_empty());
                continue;
            }
            let r = l.char_rects(0, src..src + 1);
            assert_eq!(r.len(), 1, "one box for {src}");
            assert_eq!(l.char_at(0, r[0].center()), Some(src), "at {src}");
        }
        // Within the first paragraph (one column), characters run top to bottom.
        let c0 = l.char_rects(0, 0..1)[0];
        let c1 = l.char_rects(0, 1..2)[0];
        assert!(c0.bottom() <= c1.top() + 0.5, "{c0:?} vs {c1:?}");
        // The second paragraph wraps across columns moving right to left.
        let cols = l.char_rects(0, 5..ch.chars.len() as u32);
        assert!(cols.len() >= 2, "{cols:?}");
        assert!(
            cols.windows(2).all(|w| w[0].left() >= w[1].right() - 0.5),
            "columns must advance right to left: {cols:?}"
        );
        // Right of the first column is caret 0; left of the last column is the end.
        let first_col = l.char_rects(0, 0..4)[0];
        assert_eq!(
            l.caret_near(0, pos2(first_col.right() + 50.0, first_col.center().y)),
            Some(0)
        );
        let last_col = *cols.last().unwrap();
        let last_src = ch.chars.iter().rposition(|&c| c != '\0').expect("has text") as u32;
        assert_eq!(
            l.caret_near(0, pos2(last_col.left() - 50.0, last_col.center().y)),
            Some(last_src + 1)
        );
        // Page offset places the first column inside [0, content_w].
        let shift = l.page_shift(0);
        let placed = first_col.translate(shift);
        assert!(
            placed.left() >= 0.0 && placed.right() <= 800.5,
            "{placed:?}"
        );
        // Sliced layout matches full layout.
        let (_, sliced) = lay_out_vertical(html, 8.0, 800.0, true);
        assert_eq!(l.pages.len(), sliced.pages.len());
        let all = 0..ch.chars.len() as u32;
        assert_eq!(l.char_rects(0, all.clone()), sliced.char_rects(0, all));
    }

    #[test]
    fn vertical_punctuation_and_fallbacks() {
        // With vertical forms available.
        let all = |_: char| true;
        assert_eq!(
            vert_glyph('「', all),
            VertGlyph::Upright {
                ch: '﹁',
                dx: 0.0,
                dy: 0.0
            }
        );
        assert_eq!(
            vert_glyph('“', all),
            VertGlyph::Upright {
                ch: '﹁',
                dx: 0.0,
                dy: 0.0
            }
        );
        assert_eq!(
            vert_glyph('。', all),
            VertGlyph::Upright {
                ch: '︒',
                dx: 0.0,
                dy: 0.0
            }
        );
        assert_eq!(
            vert_glyph('—', all),
            VertGlyph::Upright {
                ch: '︱',
                dx: 0.0,
                dy: 0.0
            }
        );
        assert_eq!(
            vert_glyph('A', all),
            VertGlyph::Upright {
                ch: 'A',
                dx: 0.0,
                dy: 0.0
            }
        );
        assert_eq!(
            vert_glyph('9', all),
            VertGlyph::Upright {
                ch: '9',
                dx: 0.0,
                dy: 0.0
            }
        );
        assert_eq!(vert_glyph('～', all), VertGlyph::Sideways('～'));
        assert_eq!(vert_glyph('-', all), VertGlyph::Sideways('-'));
        assert_eq!(
            vert_glyph('(', all),
            VertGlyph::Upright {
                ch: '︵',
                dx: 0.0,
                dy: 0.0
            }
        );
        assert_eq!(
            vert_glyph('字', all),
            VertGlyph::Upright {
                ch: '字',
                dx: 0.0,
                dy: 0.0
            }
        );

        // Without vertical forms in the primary font: brackets/dashes rotate,
        // stops nudge optically upright.
        let none = |_: char| false;
        assert_eq!(vert_glyph('「', none), VertGlyph::Sideways('「'));
        assert_eq!(vert_glyph('—', none), VertGlyph::Sideways('—'));
        assert_eq!(vert_glyph('…', none), VertGlyph::Sideways('…'));
        assert_eq!(
            vert_glyph('。', none),
            VertGlyph::Upright {
                ch: '。',
                dx: 0.45,
                dy: -0.45
            }
        );
    }

    #[test]
    fn page_of_offset_handles_zero_text_and_image_pages() {
        let unit = |text: usize, image_only: bool| Unit {
            top: 0.0,
            bottom: 10.0,
            items: 0..0,
            break_before: false,
            image_only,
            text,
            han: 0,
            spoiler: None,
        };
        let make = |pages: &[(usize, bool)], text_total: usize| ChapterLayout {
            items: Vec::new(),
            units: pages.iter().map(|&(t, img)| unit(t, img)).collect(),
            decos: Vec::new(),
            pages: (0..pages.len())
                .map(|i| Page {
                    y0: 0.0,
                    units: i..i + 1,
                    center: pages[i].1,
                })
                .collect(),
            anchors: HashMap::new(),
            links: Vec::new(),
            spoilers: Vec::new(),
            hits: Hits::default(),
            text_total,
            han_total: 0,
            vertical: false,
            content_w: 400.0,
        };

        // Leading illustrations before text: offset 0 stays on page 0; offset > 0 lands on text.
        let leading = make(&[(0, true), (0, true), (0, false), (300, false)], 600);
        assert_eq!(leading.page_of_offset(0), 0);
        assert_eq!(leading.page_of_offset(1), 2);
        assert_eq!(leading.page_of_offset(299), 2);
        assert_eq!(leading.page_of_offset(300), 3);
        assert_eq!(leading.page_of_offset(999), 3);

        // All-image chapter: offset 0 stays on page 0 (never jumps to the last page).
        let images_only = make(&[(0, true), (0, true), (0, true)], 0);
        assert_eq!(images_only.page_of_offset(0), 0);

        // Mid-chapter image: text page right after an image stays on the text page.
        let mid = make(&[(0, false), (300, true), (300, false), (600, false)], 900);
        assert_eq!(mid.page_of_offset(0), 0);
        assert_eq!(mid.page_of_offset(300), 2);
        assert_eq!(mid.page_of_offset(450), 2);
        assert_eq!(mid.page_of_offset(600), 3);

        // Trailing illustrations: landing at text_total hits the first trailing image, not the last.
        let trailing = make(&[(0, false), (300, true), (300, true)], 300);
        assert_eq!(trailing.page_of_offset(0), 0);
        assert_eq!(trailing.page_of_offset(300), 1);
    }
}
