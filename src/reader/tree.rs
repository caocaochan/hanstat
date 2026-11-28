//! A chapter's XHTML as styled blocks and inline runs, ready for layout.
//! Built off the UI thread: nothing here depends on fonts or page size.

use super::css::{Decl, Parser, RuleSet};
use super::epub::{self, Epub};
use super::style::{self, Display, Len, ListStyle, Style, WhiteSpace};
use std::collections::HashMap;
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
        /// Index of the first character in [`Chapter::chars`].
        src: u32,
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
    /// An inline box (dictionary popups: `inline-block` with a background,
    /// border or padding): its content on one line inside its own box.
    Box {
        items: Vec<Inline>,
        style: StyleId,
        link: Option<u32>,
    },
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
    /// A DuChinese sentence translation block, blurred until clicked.
    pub spoiler: Option<u32>,
}

/// Where a link goes.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkTarget {
    /// A file of the book (ZIP path) and an element id in it; external links
    /// are not kept.
    Chapter {
        path: String,
        fragment: Option<String>,
    },
    /// A dictionary popup's cross-reference: look this up.
    Lookup(String),
    /// A dictionary popup's `<details>` (its number in the document): open
    /// or close it.
    Toggle(u32),
}

pub struct Chapter {
    pub styles: Vec<Style>,
    pub root: Block,
    pub links: Vec<LinkTarget>,
    /// Characters of text, for the book's progress.
    pub text_len: usize,
    /// Chinese characters ([`hanstat::chars::is_han`]) in [`Chapter::chars`]:
    /// what reading stats count.
    pub han_len: u32,
    /// Element ids and the characters of text before them.
    pub anchors: Vec<(String, usize)>,
    /// The text in layout order, `'\0'` between blocks and at line breaks:
    /// what a dictionary lookup reads from the character under the pointer.
    pub chars: Vec<char>,
    /// Character ranges in [`Chapter::chars`] belonging to DuChinese sentence
    /// translations (excluded from read-aloud / TTS whether blurred or revealed).
    pub spoiler_ranges: Vec<std::ops::Range<u32>>,
}

/// Style sheets parsed once per book, by the list of sheets a chapter links
/// and the hash of its inline `<style>` blocks.
#[derive(Default)]
pub struct SheetCache {
    cache: HashMap<(Vec<String>, u64), Arc<RuleSet>>,
    ua: Option<Arc<RuleSet>>,
}

/// Rules from a style sheet's text, with no files to import.
pub fn rules(css: &str) -> RuleSet {
    let mut set = RuleSet::default();
    let none = |_: &str| None;
    let mut p = Parser {
        set: &mut set,
        order: 0,
        load: &none,
        seen: Vec::new(),
    };
    p.sheet(css, "");
    set
}

/// The user-agent sheet's rules, parsed once.
pub fn ua_rules() -> Arc<RuleSet> {
    static UA: std::sync::OnceLock<Arc<RuleSet>> = std::sync::OnceLock::new();
    UA.get_or_init(|| Arc::new(rules(style::UA_SHEET))).clone()
}

impl SheetCache {
    fn ua(&mut self) -> Arc<RuleSet> {
        self.ua.get_or_insert_with(ua_rules).clone()
    }

    fn get(&mut self, epub: &Epub, key: Vec<String>, inline: &[(String, String)]) -> Arc<RuleSet> {
        let mut inline_hash = 0u64;
        for (text, base) in inline {
            inline_hash = hanstat::dict::xxh64::xxh64(text.as_bytes(), inline_hash ^ 0x1);
            inline_hash = hanstat::dict::xxh64::xxh64(base.as_bytes(), inline_hash ^ 0x2);
        }
        let cache_key = (key, inline_hash);
        if let Some(s) = self.cache.get(&cache_key) {
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
        for path in &cache_key.0 {
            p.sheet_file(path);
        }
        for (text, base) in inline {
            p.sheet(text, base);
        }
        let set = Arc::new(set);
        if self.cache.len() < 64 {
            self.cache.insert(cache_key, set.clone());
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
    build_doc(
        &doc,
        &ua,
        &author,
        Source::Epub {
            base: base.to_owned(),
            path: path.to_owned(),
            duchinese: epub.duchinese,
        },
    )
}

/// What a document is, for its links and images.
pub enum Source {
    /// A file of the book: links and image paths resolve against it.
    Epub {
        base: String,
        path: String,
        duchinese: bool,
    },
    /// A dictionary popup's entry: `?query=` links look up, `<details>` not in
    /// `open` (numbered in document order) show only their summary, and image
    /// paths are the dictionary's media paths.
    Popup {
        open: std::collections::HashSet<u32>,
        /// Put before image paths (which dictionary's media they are).
        media: String,
        /// The popup's text colour: the document's own, so `currentColor`
        /// (and `color-mix` with it) means it.
        ink: eframe::egui::Color32,
    },
}

/// The styled tree of a parsed document, with `ua` and `author` sheets.
pub fn build_doc(doc: &scraper::Html, ua: &RuleSet, author: &RuleSet, source: Source) -> Chapter {
    let mut b = Builder {
        ua,
        author,
        styles: Vec::new(),
        links: Vec::new(),
        source,
        details: 0,
        spoilers: 0,
        summary: None,
        text_len: 0,
        anchors: Vec::new(),
        cands: Vec::new(),
        root_font: 1.0,
    };
    let html = doc.root_element();
    let top = match &b.source {
        Source::Popup { ink, .. } => Style {
            color: Some(*ink),
            ..Style::default()
        },
        Source::Epub { .. } => Style::default(),
    };
    let root_style = b.compute(&html, &top);
    b.root_font = root_style.font;
    let root_id = b.push(root_style.clone());
    let mut root = Block {
        style: root_id,
        kind: BlockKind::Flow,
        children: Vec::new(),
        id: None,
        spoiler: None,
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
    let mut chars = Vec::new();
    let mut spoiler_ranges = Vec::new();
    index_text(&mut root, &mut chars, false, &mut spoiler_ranges);
    let han_len = chars.iter().filter(|&&c| hanstat::chars::is_han(c)).count() as u32;
    Chapter {
        styles: b.styles,
        root,
        links: b.links,
        text_len: b.text_len,
        han_len,
        anchors: b.anchors,
        chars,
        spoiler_ranges,
    }
}

/// Number the text runs in layout order and collect their characters, with
/// `'\0'` between blocks and inline groups and at forced breaks.
fn index_text(
    block: &mut Block,
    chars: &mut Vec<char>,
    in_spoiler: bool,
    spoiler_ranges: &mut Vec<std::ops::Range<u32>>,
) {
    let is_spoiler = in_spoiler || block.spoiler.is_some();
    let start = chars.len() as u32;
    for c in &mut block.children {
        match c {
            Child::Block(b) => index_text(b, chars, is_spoiler, spoiler_ranges),
            Child::Inline(items) => {
                for it in items.iter_mut() {
                    match it {
                        Inline::Text { text, src, .. } => {
                            *src = chars.len() as u32;
                            chars.extend(text.chars());
                        }
                        Inline::Break => chars.push('\0'),
                        _ => {}
                    }
                }
            }
        }
        if chars.last().is_some_and(|c| *c != '\0') {
            chars.push('\0');
        }
    }
    if !in_spoiler && block.spoiler.is_some() {
        let end = chars.len() as u32;
        if end > start {
            spoiler_ranges.push(start..end);
        }
    }
}

struct Builder<'a> {
    ua: &'a RuleSet,
    author: &'a RuleSet,
    styles: Vec<Style>,
    links: Vec<LinkTarget>,
    source: Source,
    /// `<details>` elements seen so far (popup documents).
    details: u32,
    /// Spoiler blocks seen so far (DuChinese sentence translations).
    spoilers: u32,
    /// A `<summary>` about to be built: its marker and toggle link.
    summary: Option<(String, u32)>,
    text_len: usize,
    anchors: Vec<(String, usize)>,
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
            Inline::Image { .. } | Inline::Break | Inline::Box { .. } => true,
            _ => false,
        })
    }

    fn push_char(&mut self, c: char, style: StyleId, link: Option<u32>) {
        if let Some(Inline::Text {
            text,
            style: s,
            link: l,
            ..
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
                src: 0,
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
                        n += counts(c);
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
            n += counts(c);
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

/// The word a dictionary cross-reference looks up: Yomitan's structured
/// content links to `?query=<word>&wildcards=off`.
fn lookup_query(href: &str) -> Option<String> {
    let q = href.strip_prefix('?')?;
    let v = q.split('&').find_map(|kv| kv.strip_prefix("query="))?;
    let v = v.replace('+', " ");
    let s = percent_encoding::percent_decode_str(&v)
        .decode_utf8()
        .ok()?
        .into_owned();
    (!s.is_empty()).then_some(s)
}

/// Whether `c` counts toward a chapter's length: letters (Chinese
/// characters among them) and digits, not punctuation or symbols.
fn counts(c: char) -> usize {
    c.is_alphanumeric() as usize
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
        let target = match &self.source {
            Source::Epub { base, path, .. } => {
                if href.contains("://") || href.starts_with("mailto:") {
                    return None;
                }
                let path = if href.starts_with('#') {
                    path.clone()
                } else {
                    epub::resolve(base, href)
                };
                LinkTarget::Chapter {
                    path,
                    fragment: epub::fragment(href),
                }
            }
            Source::Popup { .. } => LinkTarget::Lookup(lookup_query(href)?),
        };
        self.links.push(target);
        Some((self.links.len() - 1) as u32)
    }

    /// The block for `el`, whose style is computed.
    fn block(&mut self, el: &scraper::ElementRef, style: Style) -> Block {
        let id = el.value().id().map(str::to_owned);
        if let Some(id) = &id {
            self.anchors.push((id.clone(), self.text_len));
        }
        let list = style.display == Display::ListItem;
        let marker_style = style.list_style;
        let sid = self.push(style.clone());
        let spoiler = if matches!(
            self.source,
            Source::Epub {
                duchinese: true,
                ..
            }
        ) && el.value().name() == "p"
            && el.value().classes().any(|c| c == "tr")
        {
            let n = self.spoilers;
            self.spoilers += 1;
            Some(n)
        } else {
            None
        };
        let mut block = Block {
            style: sid,
            kind: BlockKind::Flow,
            children: Vec::new(),
            id,
            spoiler,
        };
        let mut run = Run::default();
        let mut link = None;
        if let Some((marker, l)) = self.summary.take() {
            run.items.push(Inline::Text {
                text: marker,
                style: sid,
                link: Some(l),
                src: 0,
            });
            run.last = Some(' ');
            link = Some(l);
        }
        if list && marker_style != ListStyle::None {
            let marker = match marker_style {
                ListStyle::Decimal => {
                    let n = el
                        .prev_siblings()
                        .filter_map(scraper::ElementRef::wrap)
                        .filter(|e| e.value().name() == "li")
                        .count()
                        + 1;
                    format!("{n}. ")
                }
                _ => "• ".to_owned(),
            };
            run.items.push(Inline::Text {
                text: marker,
                style: sid,
                link: None,
                src: 0,
            });
            run.last = Some(' ');
        }
        self.children(el, &style, sid, &mut block, &mut run, link);
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
                            spoiler: None,
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
                        spoiler: None,
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
                    spoiler: None,
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
            "details" if matches!(self.source, Source::Popup { .. }) => {
                self.details(el, style, block, run);
                return;
            }
            _ => {}
        }
        if matches!(self.source, Source::Popup { .. })
            && style.display == Display::Inline
            && style.inline_block
            && (style.background.is_some()
                || style.border.iter().any(|b| b.visible())
                || style
                    .padding
                    .iter()
                    .any(|p| matches!(p, Len::Base(v) if *v > 0.0)))
        {
            let link = self.link(el).or(link);
            let sid = self.push(style.clone());
            let mut inner = Run::default();
            let mut scratch = Block {
                style: sid,
                kind: BlockKind::Flow,
                children: Vec::new(),
                id: None,
                spoiler: None,
            };
            self.children(el, &style, sid, &mut scratch, &mut inner, link);
            // Content only; any blocks inside are flattened into the line.
            let mut items = Vec::new();
            for c in scratch.children {
                if let Child::Inline(i) = c {
                    items.extend(i);
                }
            }
            items.extend(inner.items);
            items.retain(|i| !matches!(i, Inline::Break));
            run.atomic(Inline::Box {
                items,
                style: sid,
                link,
            });
            return;
        }
        let is_block = matches!(style.display, Display::Block | Display::ListItem);
        if is_block {
            self.flush(block, run);
            let child = self.block(el, style);
            block.children.push(Child::Block(child));
        } else {
            if let Some(id) = el.value().id() {
                self.anchors.push((id.to_owned(), self.text_len));
                run.items.push(Inline::Anchor(id.to_owned()));
            }
            let link = self.link(el).or(link);
            let sid = self.push(style.clone());
            self.children(el, &style, sid, block, run, link);
        }
    }

    /// A popup's `<details>`: its summary, with ▸ (closed) or ▾ (open) as
    /// a link that toggles it; the rest only when open. Open: its `open`
    /// attribute, unless the popup's toggles say otherwise.
    fn details(
        &mut self,
        el: &scraper::ElementRef,
        style: Style,
        block: &mut Block,
        run: &mut Run,
    ) {
        let n = self.details;
        self.details += 1;
        let toggled = match &self.source {
            Source::Popup { open, .. } => open.contains(&n),
            Source::Epub { .. } => false,
        };
        let is_open = (el.value().attr("open").is_some()) != toggled;
        self.links.push(LinkTarget::Toggle(n));
        let toggle = (self.links.len() - 1) as u32;
        self.flush(block, run);
        let sid = self.push(style.clone());
        let mut d = Block {
            style: sid,
            kind: BlockKind::Flow,
            children: Vec::new(),
            id: None,
            spoiler: None,
        };
        let mut inner = Run::default();
        let mut has_summary = false;
        for node in el.children() {
            match node.value() {
                scraper::Node::Element(e) if e.name() == "summary" => {
                    let Some(c) = scraper::ElementRef::wrap(node) else {
                        continue;
                    };
                    has_summary = true;
                    self.flush(&mut d, &mut inner);
                    let st = self.compute(&c, &style);
                    self.summary = Some((if is_open { "▾ " } else { "▸ " }.to_owned(), toggle));
                    let b = self.block(&c, st);
                    d.children.push(Child::Block(b));
                }
                _ if !is_open => {}
                scraper::Node::Text(t) => {
                    self.text_len += inner.text(t, sid, style.white_space, None);
                }
                scraper::Node::Element(_) => {
                    if let Some(c) = scraper::ElementRef::wrap(node) {
                        self.element(&c, &style, &mut d, &mut inner, None);
                    }
                }
                _ => {}
            }
        }
        if !has_summary {
            // A details without a summary is "Details", as in browsers.
            inner.items.insert(
                0,
                Inline::Text {
                    text: if is_open {
                        "▾ Details"
                    } else {
                        "▸ Details"
                    }
                    .to_owned(),
                    style: sid,
                    link: Some(toggle),
                    src: 0,
                },
            );
        }
        self.flush(&mut d, &mut inner);
        block.children.push(Child::Block(d));
    }

    fn img(&mut self, el: &scraper::ElementRef, fit_page: bool) -> Option<Img> {
        let v = el.value();
        let href = v
            .attr("src")
            .or_else(|| v.attr("xlink:href"))
            .or_else(|| v.attrs().find(|(k, _)| k.ends_with("href")).map(|(_, x)| x))?;
        let path = match &self.source {
            Source::Epub { base, .. } => epub::resolve(base, href),
            Source::Popup { media, .. } => format!("{media}{href}"),
        };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn popup_text(html: &str, open: &[u32]) -> (String, Vec<LinkTarget>) {
        let doc = scraper::Html::parse_document(html);
        let ch = build_doc(
            &doc,
            &ua_rules(),
            &RuleSet::default(),
            Source::Popup {
                open: open.iter().copied().collect(),
                media: "d/".into(),
                ink: eframe::egui::Color32::WHITE,
            },
        );
        (ch.chars.iter().filter(|c| **c != '\0').collect(), ch.links)
    }

    /// `<details>`: the summary with ▸ (a toggle link) while closed, the
    /// rest only when open; `open` toggled by the popup's set.
    #[test]
    fn details_open_and_close() {
        let html = "<html><body><details><summary>例</summary><p>正文</p></details><details open><summary>源</summary>出处</details></body></html>";
        let (t, links) = popup_text(html, &[]);
        assert_eq!(t, "▸ 例▾ 源出处");
        assert_eq!(links, [LinkTarget::Toggle(0), LinkTarget::Toggle(1)]);
        let (t, _) = popup_text(html, &[0, 1]);
        assert_eq!(t, "▾ 例正文▸ 源");
    }

    #[test]
    fn cross_references_look_up() {
        let (_, links) = popup_text(
            r#"<html><body><a href="?query=%E8%AF%B4%E8%AF%9D&wildcards=off">说话</a><a href="https://x.org">x</a></body></html>"#,
            &[],
        );
        assert_eq!(links, [LinkTarget::Lookup("说话".into())]);
        assert_eq!(lookup_query("?wildcards=off&query=a+b"), Some("a b".into()));
        assert_eq!(lookup_query("#x"), None);
    }

    /// Popups only: inline boxes with a background become boxes; plain
    /// inline-block stays text, and so does any in a book.
    #[test]
    fn inline_boxes() {
        let doc = scraper::Html::parse_document(
            r#"<html><body><p><span style="display:inline-block;background:#111">简</span><span style="display:inline-block">x</span></p></body></html>"#,
        );
        let boxes = |ch: &Chapter| -> usize {
            fn count(b: &Block) -> usize {
                b.children
                    .iter()
                    .map(|c| match c {
                        Child::Block(b) => count(b),
                        Child::Inline(i) => {
                            i.iter().filter(|x| matches!(x, Inline::Box { .. })).count()
                        }
                    })
                    .sum()
            }
            count(&ch.root)
        };
        let popup = build_doc(
            &doc,
            &ua_rules(),
            &RuleSet::default(),
            Source::Popup {
                open: Default::default(),
                media: String::new(),
                ink: eframe::egui::Color32::WHITE,
            },
        );
        assert_eq!(boxes(&popup), 1);
        let book = build_doc(
            &doc,
            &ua_rules(),
            &RuleSet::default(),
            Source::Epub {
                base: String::new(),
                path: String::new(),
                duchinese: false,
            },
        );
        assert_eq!(boxes(&book), 0);
    }

    #[test]
    fn duchinese_translations_are_spoilers_only_in_duchinese_books() {
        let html = r#"<html><body><p>中文</p><p class="tr" lang="en">English one.</p><p>第二段</p><p class="tr" lang="en">English two.</p></body></html>"#;
        let doc = scraper::Html::parse_document(html);
        let collect = |ch: &Chapter| -> Vec<Option<u32>> {
            fn walk(b: &Block, out: &mut Vec<Option<u32>>) {
                if b.spoiler.is_some() {
                    out.push(b.spoiler);
                }
                for c in &b.children {
                    if let Child::Block(child) = c {
                        walk(child, out);
                    }
                }
            }
            let mut out = Vec::new();
            walk(&ch.root, &mut out);
            out
        };
        let du = build_doc(
            &doc,
            &ua_rules(),
            &RuleSet::default(),
            Source::Epub {
                base: String::new(),
                path: String::new(),
                duchinese: true,
            },
        );
        assert_eq!(collect(&du), [Some(0), Some(1)]);
        let other = build_doc(
            &doc,
            &ua_rules(),
            &RuleSet::default(),
            Source::Epub {
                base: String::new(),
                path: String::new(),
                duchinese: false,
            },
        );
        assert_eq!(collect(&other), []);
    }
}
