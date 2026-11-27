//! A dictionary's glossaries for one word, laid out for the popup with the
//! reader's own engine: their structured content as HTML
//! ([`hanstat::dict::sc`]), styled by the dictionary's `styles.css` over a
//! small popup sheet, laid out at the popup's width as one long page.
//! Results are kept (by content, width, text size and which `<details>` are
//! open), so redrawing or scrolling the popup lays nothing out again.

use super::css::RuleSet;
use super::images::Images;
use super::layout::{self, ChapterLayout, Params};
use super::popup::Look;
use super::tree::{self, Chapter, Source};
use eframe::egui;
use hanstat::dict::lookup::{DictInfo, Gloss};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// The popup's own defaults, after the user-agent sheet and before the
/// dictionary's sheet, in `look`'s colours.
/// Compact: dictionaries without a sheet of their own (CC-CEDICT) would get
/// the browser spacing, 1em around every list.
fn popup_sheet(look: &Look) -> String {
    let hex = |c: egui::Color32| format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b());
    let (subtle, faint, link) = (hex(look.subtle), hex(look.faint), hex(look.link));
    format!(
        r#"
body {{ margin: 0; padding: 0 }}
.entry {{ margin: 0 0 0.3em 0 }}
.tags {{ font-size: 0.72em; color: {subtle}; margin-right: 0.4em }}
ul, ol, menu, dir {{ margin: 0.05em 0; padding-left: 1.1em }}
ul.glossary {{ list-style-type: disc }}
li {{ margin: 0 }}
p, dl, figure {{ margin: 0.15em 0 }}
blockquote {{ margin: 0.15em 0 0.15em 1em }}
h1, h2, h3, h4, h5, h6 {{ margin: 0.2em 0 }}
details {{ margin: 0.1em 0 }}
summary, a[href] {{ color: {link} }}
table {{ margin: 0.1em 0 }}
[data-sc-cccedict="headword"] {{ display: inline; font-size: 0.82em; color: {subtle}; margin: 0 0.3em 0 0 }}
.sep {{ color: {faint} }}
"#
    )
}

const CACHE: usize = 160;

/// Time spent building entries (µs): HTML, parsing, styling, layout; and
/// how many (for measuring).
#[cfg(test)]
pub static STAGES: [std::sync::atomic::AtomicU64; 5] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 5];

#[cfg(test)]
fn stage(i: usize, since: std::time::Instant) -> std::time::Instant {
    let now = std::time::Instant::now();
    STAGES[i].fetch_add(
        (now - since).as_micros() as u64,
        std::sync::atomic::Ordering::Relaxed,
    );
    now
}

pub struct Entry {
    pub chapter: Chapter,
    pub layout: ChapterLayout,
    pub height: f32,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    content: u64,
    width: u32,
    size: u32,
    fonts: (String, String),
    open: Vec<u32>,
}

fn content_hash(dict_id: &str, glossaries: &[Gloss]) -> u64 {
    let mut h = hanstat::dict::xxh64::key(dict_id);
    h = hanstat::dict::xxh64::xxh64(&(glossaries.len() as u64).to_le_bytes(), h);
    for g in glossaries {
        h = hanstat::dict::xxh64::xxh64(g.def_tags.as_bytes(), h ^ 0x1);
        h = hanstat::dict::xxh64::xxh64(g.term_tags.as_bytes(), h ^ 0x2);
        match &g.json {
            Ok(j) => h = hanstat::dict::xxh64::xxh64(j.as_bytes(), h ^ 0x3),
            Err(e) => h = hanstat::dict::xxh64::xxh64(e.as_bytes(), h ^ 0x4),
        }
    }
    h
}

pub struct Renderer {
    ua: Arc<RuleSet>,
    sheets: HashMap<String, Arc<RuleSet>>,
    cache: HashMap<Key, (u64, Arc<Entry>)>,
    tick: u64,
    pub images: Images,
    /// Each dictionary's folder, for its images.
    dirs: Arc<Mutex<HashMap<String, PathBuf>>>,
    /// The theme `ua` and the entries kept are in.
    theme: &'static str,
}

impl Renderer {
    pub fn new() -> Renderer {
        let dirs: Arc<Mutex<HashMap<String, PathBuf>>> = Arc::default();
        let d = dirs.clone();
        let images = Images::with_source(
            "dict",
            Arc::new(move |p: &str| {
                let (id, rest) = p.split_once('/')?;
                let dir = d.lock().unwrap().get(id)?.clone();
                hanstat::dict::store::read_media(&dir, rest)
            }),
        );
        let mut r = Renderer {
            ua: Arc::new(RuleSet::default()),
            sheets: HashMap::new(),
            cache: HashMap::new(),
            tick: 0,
            images,
            dirs,
            theme: "",
        };
        r.set_look(&Look::of(&super::theme::THEMES[0]));
        r
    }

    /// Draw in `look`: the popup's sheet in its colours, the entries laid
    /// out again (their colours are in their layout), greyscale images
    /// light on a dark popup.
    pub fn set_look(&mut self, look: &Look) {
        if self.theme == look.theme.id {
            return;
        }
        self.theme = look.theme.id;
        self.ua = Arc::new(tree::rules(&format!(
            "{}\n{}",
            super::style::UA_SHEET,
            popup_sheet(look)
        )));
        self.cache.clear();
        self.images.set_invert_grey(look.theme.dark);
    }

    /// The dictionaries changed: their sheets, images and folders are read again.
    pub fn set_dicts(&mut self, dicts: &[DictInfo]) {
        self.sheets.clear();
        self.cache.clear();
        self.images.clear();
        let mut d = self.dirs.lock().unwrap();
        d.clear();
        for x in dicts {
            d.insert(x.id.clone(), x.dir.clone());
        }
    }

    /// `glossaries` of `dict` laid out at `params.content_w`, with the
    /// `<details>` numbered in `open` toggled.
    pub fn part(
        &mut self,
        ctx: &egui::Context,
        dict: &DictInfo,
        glossaries: &[Gloss],
        open: &HashSet<u32>,
        params: &Params,
    ) -> Arc<Entry> {
        #[cfg(test)]
        let t = std::time::Instant::now();
        let mut open_v: Vec<u32> = open.iter().copied().collect();
        open_v.sort_unstable();
        let key = Key {
            content: content_hash(&dict.id, glossaries),
            width: params.content_w.to_bits(),
            size: params.base.to_bits(),
            fonts: params.fonts.key.clone(),
            open: open_v,
        };
        if let Some((used, entry)) = self.cache.get_mut(&key) {
            self.tick += 1;
            *used = self.tick;
            return entry.clone();
        }
        #[cfg(test)]
        STAGES[4].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut html = String::from("<html><body><div class=\"entries\">");
        for g in glossaries {
            html.push_str("<div class=\"entry\">");
            let tags: Vec<&str> = g
                .def_tags
                .split_whitespace()
                .chain(g.term_tags.split_whitespace())
                .collect();
            if !tags.is_empty() {
                html.push_str("<span class=\"tags\">");
                hanstat::dict::sc::escape(&tags.join(" · "), &mut html);
                html.push_str("</span>");
            }
            match &g.json {
                Ok(j) => {
                    if hanstat::dict::sc::glossary(j, &mut html).is_err() {
                        html.push_str("(unreadable entry)");
                    }
                }
                Err(e) => hanstat::dict::sc::escape(e, &mut html),
            }
            html.push_str("</div>");
        }
        html.push_str("</div></body></html>");
        #[cfg(test)]
        let t = stage(0, t);
        let sheet = self
            .sheets
            .entry(dict.id.clone())
            .or_insert_with(|| Arc::new(tree::rules(&dict.styles)))
            .clone();
        let doc = scraper::Html::parse_document(&html);
        #[cfg(test)]
        let t = stage(1, t);
        let chapter = tree::build_doc(
            &doc,
            &self.ua,
            &sheet,
            Source::Popup {
                open: open.clone(),
                media: format!("{}/", dict.id),
                ink: params.text,
            },
        );
        #[cfg(test)]
        let t = stage(2, t);
        let images = &mut self.images;
        let layout = ctx.fonts_mut(|f| layout::layout(&chapter, params, f, images));
        #[cfg(test)]
        stage(3, t);
        let height = layout.flow_height();
        let e = Arc::new(Entry {
            chapter,
            layout,
            height,
        });
        if self.cache.len() >= CACHE
            && let Some(oldest) = self
                .cache
                .iter()
                .min_by_key(|(_, (u, _))| *u)
                .map(|(k, _)| k.clone())
        {
            self.cache.remove(&oldest);
        }
        self.tick += 1;
        self.cache.insert(key, (self.tick, e.clone()));
        e
    }
}
