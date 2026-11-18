//! A chapter's XHTML as styled blocks and inline runs, ready for layout.
//! Built off the UI thread: nothing here depends on fonts or page size.

use super::css::{Decl, Parser, RuleSet};
use super::epub::{self, Epub};
use super::style::{self, Display, Len, ListStyle, Style, WhiteSpace};
use std::sync::Arc;

pub type StyleId = u32;

#[derive(Clone, Debug)]
pub struct Img {
    /// ZIP path.
    pub path: String,
    /// `width`/`height` attributes, in CSS px (SVG viewBox units for SVG pages).
    pub attr_w: Option<f32>,
    pub attr_h: Option<f32>,
    /// An image page wrapped in SVG (covers, illustrations): fit to the page.
    pub fit_page: bool,
}

#[derive(Clone, Debug)]
pub enum Inline {
    Text {
        text: String,
        style: StyleId,
        link: Option<u32>,
    },
    Image {
        img: Img,
        style: StyleId,
        link: Option<u32>,
    },
    Break,
    /// An element id at this point of the text.
    Anchor(String),
    /// Ruby: the base runs follow until [`Inline::RubyEnd`].
    RubyStart {
        annotation: String,
        style: StyleId,
    },
    RubyEnd,
}

#[derive(Clone, Debug)]
pub enum Child {
    Block(Block),
    Inline(Vec<Inline>),
}

#[derive(Clone, Debug)]
pub enum BlockKind {
    Flow,
    Image(Img),
    Rule,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub style: StyleId,
    pub kind: BlockKind,
    pub children: Vec<Child>,
    pub id: Option<String>,
}

pub struct Chapter {
    pub styles: Vec<Style>,
    pub root: Block,
    /// Link targets: (ZIP path, fragment); external links are not kept.
    pub links: Vec<(String, Option<String>)>,
    /// Characters of text, for the book's progress.
    pub text_len: usize,
}

/// Style sheets parsed once per book, by the list of sheets a chapter links.
#[derive(Default)]
pub struct SheetCache {
    cache: Vec<(Vec<String>, Arc<RuleSet>)>,
    ua: Option<Arc<RuleSet>>,
}

impl SheetCache {
    fn ua(&mut self) -> Arc<RuleSet> {
        self.ua
            .get_or_insert_with(|| {
                let mut set = RuleSet::default();
                let none = |_: &str| None;
                let mut p = Parser {
                    set: &mut set,
                    order: 0,
                    load: &none,
                    seen: Vec::new(),
                };
                p.sheet(style::UA_SHEET, "");
                Arc::new(set)
            })
            .clone()
    }

    fn get(&mut self, epub: &Epub, key: Vec<String>, inline: &[(String, String)]) -> Arc<RuleSet> {
        if inline.is_empty()
            && let Some((_, s)) = self.cache.iter().find(|(k, _)| *k == key)
        {
            return s.clone();
        }
        let mut set = RuleSet::default();
        let load = |p: &str| epub.read_text(p);
        let mut p = Parser {
            set: &mut set,
            order: 0,
            load: &load,
            seen: Vec::new(),
        };
        for path in &key {
            p.sheet_file(path);
        }
        for (text, base) in inline {
            p.sheet(text, base);
        }
        let set = Arc::new(set);
        if inline.is_empty() {
            self.cache.push((key, set.clone()));
        }
        set
    }
}

pub fn build(epub: &Epub, path: &str, sheets: &mut SheetCache) -> Chapter {
    let text = epub.read_text(path).unwrap_or_default();
    let doc = scraper::Html::parse_document(&super::markup::expand_self_closing(&text));
    let base = epub::dir_of(path);
    // Linked and embedded sheets, in document order.
    let mut linked = Vec::new();
    let mut inline = Vec::new();
    for e in doc
        .tree
        .root()
        .descendants()
        .filter_map(scraper::ElementRef::wrap)
    {
        match e.value().name() {
            "link" => {
                let rel = e.value().attr("rel").unwrap_or("").to_ascii_lowercase();
                if rel.split_whitespace().any(|r| r == "stylesheet")
                    && !rel.contains("alternate")
                    && let Some(href) = e.value().attr("href")
                {
                    linked.push(epub::resolve(base, href));
                }
            }
            "style" => inline.push((e.text().collect::<String>(), base.to_owned())),
            _ => {}
        }
    }
    let author = sheets.get(epub, linked, &inline);
    let ua = sheets.ua();
    let mut b = Builder {
        ua: &ua,
        author: &author,
        styles: Vec::new(),
        links: Vec::new(),
        base: base.to_owned(),
        path: path.to_owned(),
        text_len: 0,
        cands: Vec::new(),
        root_font: 1.0,
    };
    let html = doc.root_element();
    let root_style = b.compute(&html, &Style::default());
    b.root_font = root_style.font;
    let root_id = b.push(root_style.clone());
    let mut root = Block {
        style: root_id,
        kind: BlockKind::Flow,
        children: Vec::new(),
        id: None,
    };
    if let Some(body) = html
        .children()
        .filter_map(scraper::ElementRef::wrap)
        .find(|e| e.value().name() == "body")
    {
        let body_style = b.compute(&body, &root_style);
        let mut block = b.block(&body, body_style);
        // The body's margins are the reader's.
        let st = &mut b.styles[block.style as usize];
        st.margin = [Len::Base(0.0); 4];
        st.padding = [Len::Base(0.0); 4];
        block.id = None;
        root.children.push(Child::Block(block));
    }
    Chapter {
        styles: b.styles,
        root,
        links: b.links,
        text_len: b.text_len,
    }
}

struct Builder<'a> {
    ua: &'a RuleSet,
    author: &'a RuleSet,
    styles: Vec<Style>,
    links: Vec<(String, Option<String>)>,
    base: String,
    path: String,
    text_len: usize,
    cands: Vec<u32>,
    root_font: f32,
}

/// Inline content being collected, with CSS white-space processing across
/// element boundaries.
#[derive(Default)]
struct Run {
    items: Vec<Inline>,
    /// A collapsible space is pending (emitted before the next character
    /// unless the line starts or the space is a removable line break).
    pending: Option<(bool, StyleId)>,
    last: Option<char>,
}

impl Run {
    fn has_content(&self) -> bool {
        self.items.iter().any(|i| match i {
            Inline::Text { text, .. } => !text.is_empty(),
            Inline::Image { .. } | Inline::Break => true,
            _ => false,
        })
    }

    fn push_char(&mut self, c: char, style: StyleId, link: Option<u32>) {
        if let Some(Inline::Text {
            text,
            style: s,
            link: l,
        }) = self.items.last_mut()
            && *s == style
            && *l == link
        {
            text.push(c);
        } else {
            self.items.push(Inline::Text {
                text: c.to_string(),
                style,
                link,
            });
        }
        self.last = Some(c);
    }

    fn text(&mut self, t: &str, style: StyleId, ws: WhiteSpace, link: Option<u32>) -> usize {
        let mut n = 0;
        if !ws.collapses() {
            for c in t.chars() {
                match c {
                    '\n' => {
                        self.items.push(Inline::Break);
                        self.last = None;
                    }
                    '\r' => {}
                    '\t' => {
                        for _ in 0..4 {
                            self.push_char(' ', style, link);
                        }
                    }
                    c => {
                        self.push_char(c, style, link);
                        n += 1;
                    }
                }
            }
            return n;
        }
        for c in t.chars() {
            if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0C') {
                if ws == WhiteSpace::PreLine && c == '\n' {
                    self.pending = None;
                    self.items.push(Inline::Break);
                    self.last = None;
                    continue;
                }
                let brk = matches!(c, '\n' | '\r');
                self.pending = Some(match self.pending {
                    Some((b, s)) => (b || brk, s),
                    None => (brk, style),
                });
                continue;
            }
            if let Some((brk, s)) = self.pending.take() {
                // A source line break between two wide characters is removed
                // (CSS Text 3 segment break transformation); a space at the
                // start of a line is dropped.
                let wide_join = brk && self.last.is_some_and(wide) && wide(c);
                if self.last.is_some() && !wide_join {
                    self.push_char(' ', s, link);
                }
            }
            self.push_char(c, style, link);
            n += 1;
        }
        n
    }

    fn hard_break(&mut self) {
        self.pending = None;
        self.items.push(Inline::Break);
        self.last = None;
    }

    fn atomic(&mut self, item: Inline) {
        if let Some((_, s)) = self.pending.take()
            && self.last.is_some()
        {
            self.push_char(' ', s, None);
        }
        self.items.push(item);
        self.last = Some('\u{FFFC}');
    }
}

fn wide(c: char) -> bool {
    hanstat::chars::is_wide(c)
}

impl Builder<'_> {
    fn push(&mut self, s: Style) -> StyleId {
        // Consecutive equal styles share an entry (siblings mostly repeat).
        if let Some(i) = self.styles.iter().rev().take(8).position(|x| *x == s) {
            return (self.styles.len() - 1 - i) as StyleId;
        }
        self.styles.push(s);
        (self.styles.len() - 1) as StyleId
    }

    fn compute(&mut self, el: &scraper::ElementRef, parent: &Style) -> Style {
        let mut matched: Vec<(bool, u8, u32, u32, &Decl)> = Vec::new();
        let value = el.value();
        let mut cands = std::mem::take(&mut self.cands);
        for (origin, set) in [(0u8, self.ua), (1u8, self.author)] {
            set.candidates(value, &mut cands);
            for &i in &cands {
                let r = &set.rules[i as usize];
                if r.selector.matches(el) {
                    for d in r.decls.iter() {
                        matched.push((d.important, origin, r.specificity, r.order, d));
                    }
                }
            }
        }
        self.cands = cands;
        let inline_decls = value
            .attr("style")
            .map(super::css::parse_declarations)
            .unwrap_or_default();
        for d in &inline_decls {
            matched.push((d.important, 2, u32::MAX, u32::MAX, d));
        }
        // Presentational hints and attributes the reader honours.
        let mut hints = Vec::new();
        if value.attr("hidden").is_some() {
            hints.push(Decl {
                name: "display".into(),
                value: "none".into(),
                important: true,
            });
        }
        if let Some(a) = value.attr("align") {
            hints.push(Decl {
                name: "text-align".into(),
                value: a.to_owned(),
                important: false,
            });
        }
        for d in &hints {
            matched.push((d.important, if d.important { 3 } else { 0 }, 0, 0, d));
        }
        // Normal declarations by origin, specificity and order; then !important.
        matched.sort_by_key(|(imp, origin, spec, order, _)| (*imp, *origin, *spec, *order));
        let decls: Vec<&Decl> = matched.iter().map(|m| m.4).collect();
        let mut s = parent.inherit();
        let replaced = matches!(value.name(), "img" | "svg" | "image");
        style::apply(&mut s, parent, self.root_font, &decls, replaced);
        s
    }

    fn link(&mut self, el: &scraper::ElementRef) -> Option<u32> {
        if el.value().name() != "a" {
            return None;
        }
        let href = el.value().attr("href")?;
        if href.contains("://") || href.starts_with("mailto:") {
            return None;
        }
        let path = if href.starts_with('#') {
            self.path.clone()
        } else {
            epub::resolve(&self.base, href)
        };
        self.links.push((path, epub::fragment(href)));
        Some((self.links.len() - 1) as u32)
    }

    /// The block for `el`, whose style is computed.
    fn block(&mut self, el: &scraper::ElementRef, style: Style) -> Block {
        let id = el.value().id().map(str::to_owned);
        let list = style.display == Display::ListItem;
        let marker_style = style.list_style;
        let sid = self.push(style.clone());
        let mut block = Block {
            style: sid,
            kind: BlockKind::Flow,
            children: Vec::new(),
            id,
        };
        let mut run = Run::default();
        if list && marker_style != ListStyle::None {
            let n = el
                .prev_siblings()
                .filter_map(scraper::ElementRef::wrap)
                .filter(|e| e.value().name() == "li")
                .count()
                + 1;
            let marker = match marker_style {
                ListStyle::Decimal => format!("{n}. "),
                _ => "• ".to_owned(),
            };
            run.items.push(Inline::Text {
                text: marker,
                style: sid,
                link: None,
            });
            run.last = Some(' ');
        }
        self.children(el, &style, sid, &mut block, &mut run, None);
        self.flush(&mut block, &mut run);
        block
    }

    fn flush(&mut self, block: &mut Block, run: &mut Run) {
        if run.has_content() || run.items.iter().any(|i| matches!(i, Inline::Anchor(_))) {
            // Trailing breaks add no empty line.
            while matches!(run.items.last(), Some(Inline::Break)) && run.items.len() > 1 {
                run.items.pop();
            }
            block
                .children
                .push(Child::Inline(std::mem::take(&mut run.items)));
        }
        *run = Run::default();
    }

    fn children(
        &mut self,
        el: &scraper::ElementRef,
        style: &Style,
        sid: StyleId,
        block: &mut Block,
        run: &mut Run,
        link: Option<u32>,
    ) {
        for node in el.children() {
            match node.value() {
                scraper::Node::Text(t) => {
                    let n = run.text(t, sid, style.white_space, link);
                    self.text_len += n;
                }
                scraper::Node::Element(_) => {
                    let Some(c) = scraper::ElementRef::wrap(node) else {
                        continue;
                    };
                    self.element(&c, style, block, run, link);
                }
                _ => {}
            }
        }
    }

    fn element(
        &mut self,
        el: &scraper::ElementRef,
        parent: &Style,
        block: &mut Block,
        run: &mut Run,
        link: Option<u32>,
    ) {
        let name = el.value().name();
        let style = self.compute(el, parent);
        if style.display == Display::None {
            return;
        }
        match name {
            "br" => {
                run.hard_break();
                return;
            }
            "img" | "image" => {
                if let Some(img) = self.img(el, false) {
                    let block_level =
                        style.display != Display::Inline || style.float != style::Float::None;
                    let sid = self.push(style);
                    if block_level {
                        self.flush(block, run);
                        block.children.push(Child::Block(Block {
                            style: sid,
                            kind: BlockKind::Image(img),
                            children: Vec::new(),
                            id: el.value().id().map(str::to_owned),
                        }));
                    } else {
                        run.atomic(Inline::Image {
                            img,
                            style: sid,
                            link,
                        });
                    }
                }
                return;
            }
            "svg" => {
                // An image page: <svg viewBox><image href/></svg>.
                if let Some(img) = el
                    .descendants()
                    .filter_map(scraper::ElementRef::wrap)
                    .find(|e| e.value().name() == "image")
                    .and_then(|i| self.img(&i, true))
                {
                    let sid = self.push(style);
                    self.flush(block, run);
                    block.children.push(Child::Block(Block {
                        style: sid,
                        kind: BlockKind::Image(img),
                        children: Vec::new(),
                        id: None,
                    }));
                }
                return;
            }
            "hr" => {
                self.flush(block, run);
                let sid = self.push(style);
                block.children.push(Child::Block(Block {
                    style: sid,
                    kind: BlockKind::Rule,
                    children: Vec::new(),
                    id: None,
                }));
                return;
            }
            "ruby" => {
                let annotation: String = el
                    .descendants()
                    .filter_map(scraper::ElementRef::wrap)
                    .filter(|e| e.value().name() == "rt")
                    .flat_map(|e| e.text())
                    .collect::<String>()
                    .trim()
                    .to_owned();
                let sid = self.push(style.clone());
                if !annotation.is_empty() {
                    let rt_style = {
                        let mut s = style.inherit();
                        s.font = style.font * 0.5;
                        s.bold = false;
                        self.push(s)
                    };
                    run.atomic(Inline::RubyStart {
                        annotation,
                        style: rt_style,
                    });
                }
                // The base: the ruby's content without rt/rp.
                for node in el.children() {
                    match node.value() {
                        scraper::Node::Text(t) => {
                            self.text_len += run.text(t, sid, style.white_space, link);
                        }
                        scraper::Node::Element(e) if !matches!(e.name(), "rt" | "rp" | "rtc") => {
                            if let Some(c) = scraper::ElementRef::wrap(node) {
                                self.element(&c, &style, block, run, link);
                            }
                        }
                        _ => {}
                    }
                }
                if !matches!(run.items.last(), Some(Inline::RubyStart { .. })) {
                    run.items.push(Inline::RubyEnd);
                } else {
                    run.items.pop();
                }
                return;
            }
            "rt" | "rp" | "rtc" => return,
            _ => {}
        }
        let is_block = matches!(style.display, Display::Block | Display::ListItem)
            || style.float != style::Float::None && style.display != Display::Inline;
        if is_block {
            self.flush(block, run);
            let child = self.block(el, style);
            block.children.push(Child::Block(child));
        } else {
            if let Some(id) = el.value().id() {
                run.items.push(Inline::Anchor(id.to_owned()));
            }
            let link = self.link(el).or(link);
            let sid = self.push(style.clone());
            self.children(el, &style, sid, block, run, link);
        }
    }

    fn img(&mut self, el: &scraper::ElementRef, fit_page: bool) -> Option<Img> {
        let v = el.value();
        let href = v
            .attr("src")
            .or_else(|| v.attr("xlink:href"))
            .or_else(|| v.attrs().find(|(k, _)| k.ends_with("href")).map(|(_, x)| x))?;
        let path = epub::resolve(&self.base, href);
        let num = |a: &str| {
            v.attr(a)
                .and_then(|x| x.trim_end_matches("px").parse::<f32>().ok())
        };
        Some(Img {
            path,
            attr_w: num("width"),
            attr_h: num("height"),
            fit_page,
        })
    }
}
