//! The dictionary popup: the word under the pointer (with Shift held, by
//! default) looked up in the imported Yomitan dictionaries on the lookup
//! thread, highlighted on the page, and its entries shown in a popup beside
//! it.
//!
//! A lookup is asked when the pointer reaches a new character; the frame
//! that asks waits a few milliseconds for the answer (lookups take about
//! 0.1 ms), so the popup shows in the same frame.
//!
//! Entries are grouped by headword and reading; under each, every
//! dictionary's glossaries in a bar that opens and closes (the first three
//! open). Words in the popup can be looked up the same way, or by clicking a
//! cross-reference: a child popup opens beside them, up to [`MAX_LEVELS`].

use super::anki::{Cards, Sentence, Source, Status};
use super::layout::Params;
use super::render::Renderer;
use super::theme::{self, Theme};
use super::tree::LinkTarget;
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Vec2, pos2, vec2};
use hanstat::dict::lookup::{DictInfo, Lookup};
use hanstat::dict::service::{Reply, Service};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The key held to look up the word under the pointer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LookupKey {
    #[default]
    Shift,
    Ctrl,
    Alt,
    /// None: the pointer rests on a word a moment.
    None,
}

impl LookupKey {
    pub const ALL: [LookupKey; 4] = [
        LookupKey::Shift,
        LookupKey::Ctrl,
        LookupKey::Alt,
        LookupKey::None,
    ];

    pub fn held(self, m: &egui::Modifiers) -> bool {
        match self {
            LookupKey::Shift => m.shift,
            LookupKey::Ctrl => m.ctrl || m.mac_cmd,
            LookupKey::Alt => m.alt,
            LookupKey::None => true,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LookupKey::Shift => "Shift",
            LookupKey::Ctrl => "Ctrl",
            LookupKey::Alt => "Alt",
            LookupKey::None => "No key (rest on the word)",
        }
    }
}

/// How long the pointer rests on a word before a lookup with no key.
const REST: f64 = 0.15;
/// How long a frame that asked a lookup waits for its answer.
const WAIT: Duration = Duration::from_millis(3);
/// Popups open at once: the page's and two inside it.
pub const MAX_LEVELS: usize = 3;
/// The popup's height by default, in lines of its text.
const DEFAULT_LINES: f32 = 16.0;
/// The popup's width by default, in characters of its text.
const DEFAULT_CHARS: f32 = 30.0;
/// The corner grip that resizes the popup.
const GRIP: f32 = 14.0;

/// A popup's frame around its scrolled entries: margins and border.
const FRAME_V: f32 = 22.0;

/// A character of the open book: its chapter and index in [`super::tree::Chapter::chars`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spot {
    pub spine: usize,
    pub src: u32,
}

/// Where a lookup came from inside a popup: (level, group, part, character).
type InPopup = (usize, usize, usize, u32);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Origin {
    Page(Spot),
    Popup(InPopup),
    Link(usize),
}

struct Asked {
    id: u64,
    origin: Origin,
    /// Where the word is on screen (the new popup goes beside it).
    anchor: Option<Rect>,
    /// On the page: the sentence it is in (for Anki cards).
    sentence: Option<Arc<Sentence>>,
}

/// One popup.
pub struct Level {
    origin: Origin,
    pub result: Arc<Lookup>,
    /// Where it sits beside (screen).
    anchor: Option<Rect>,
    /// On the page: the sentence its word is in.
    sentence: Option<Arc<Sentence>>,
    /// `<details>` toggled, by (group, part).
    details: HashMap<(usize, usize), HashSet<u32>>,
    /// Dictionary bars toggled from their default, by (group, part).
    bars: HashSet<(usize, usize)>,
    /// Heights of the parts laid out so far (the rest are estimated until
    /// they scroll into view).
    heights: HashMap<(usize, usize), f32>,
    /// Where it was drawn.
    pub rect: Option<Rect>,
    /// Put above the other popups (once, when it first shows).
    raised: bool,
    /// Not drawn yet: it starts scrolled to the top (egui keeps the scroll
    /// of the popup it replaces otherwise).
    fresh: bool,
    /// How far its entries are scrolled.
    pub scroll_y: f32,
    /// Where its Anki buttons and its entries' scroll area (the bar at its
    /// right) were drawn.
    #[cfg(test)]
    pub drawn: (Vec<Rect>, Rect),
}

impl Level {
    /// The sentence its word is in (tests).
    #[cfg(test)]
    pub fn sentence_text(&self) -> Option<String> {
        self.sentence.as_ref().map(|s| s.text.iter().collect())
    }
}

#[derive(Default)]
pub(crate) struct DictionarySession(Option<Arc<Mutex<SessionLookup>>>);

struct SessionLookup {
    worker: Service,
    freq_sort: Option<String>,
    loaded: Option<Reply>,
    root: std::path::PathBuf,
    dictionaries: Vec<DictionaryFile>,
}

#[derive(PartialEq, Eq)]
struct DictionaryFile {
    id: String,
    enabled: bool,
    collapsed: bool,
    modified: Option<std::time::SystemTime>,
}

fn dictionary_files(root: &std::path::Path) -> Vec<DictionaryFile> {
    use hanstat::dict::format::{META, Manifest};
    Manifest::load(root)
        .dicts
        .into_iter()
        .map(|d| {
            let modified = std::fs::metadata(root.join(&d.id).join(META))
                .and_then(|m| m.modified())
                .ok();
            DictionaryFile {
                id: d.id,
                enabled: d.enabled,
                collapsed: d.collapsed,
                modified,
            }
        })
        .collect()
}

impl DictionarySession {
    pub(super) fn prefetch(
        &mut self,
        ctx: &egui::Context,
        root: std::path::PathBuf,
        freq_sort: Option<String>,
    ) {
        self.0.get_or_insert_with(|| {
            let ctx = ctx.clone();
            let worker = Service::start(
                root.clone(),
                freq_sort.clone(),
                Arc::new(move || ctx.request_repaint()),
            );
            Arc::new(Mutex::new(SessionLookup {
                worker,
                freq_sort,
                loaded: None,
                dictionaries: dictionary_files(&root),
                root,
            }))
        });
    }
}

pub struct Popup {
    service: Arc<Mutex<SessionLookup>>,
    renderer: Renderer,
    pub dicts: Vec<DictInfo>,
    pub scan_len: usize,
    pub errors: Vec<String>,
    asked: Option<Asked>,
    pub levels: Vec<Level>,
    /// The page word's box on screen this frame (where the first popup goes).
    pub anchor: Option<Rect>,
    /// The last lookup's time on the lookup thread.
    pub last_micros: u64,
    /// With no key: the character the pointer rests on, since when.
    rest: Option<(Origin, f64)>,
    /// Time to build and lay out the entries drawn last frame (ms).
    pub last_layout_ms: f32,
    /// Lines to scroll the top popup by (↑ ↓), taken when it is drawn.
    pub scroll_lines: i32,
    /// The popup's size (its entries' area: width, height), as dragged by
    /// its corner; `None`: the default. Kept in the reader's settings.
    pub size: Option<Vec2>,
    /// The size was dragged this frame (to save).
    pub size_changed: bool,
    /// The popups' areas exist in egui (see [`Popup::warm`]).
    warmed: bool,
    /// Anki cards: the entries' **+** buttons.
    pub cards: Cards,
    /// The popup's theme as saved: [`SAME_AS_PAGES`] or a theme's id.
    pub theme: String,
    /// Known words: Anki and the word lists merged (the app's), and the
    /// words marked known one by one, as the book wrote them.
    pub known: Option<Arc<hanstat::anki::Knowledge>>,
    pub marked: Arc<HashSet<String>>,
    /// Marked or unmarked here since `marked` was last set (shown at once,
    /// before the app has saved them).
    marked_here: HashMap<String, bool>,
    /// Words marked (true) or unmarked here, for the app to save.
    pub known_toggles: Vec<(String, bool)>,
}

/// The text a lookup reads from `src`: up to `max` characters, stopping at a
/// block's end, a space or punctuation (a name's middle dot `·` goes on).
/// Empty if the character itself is nothing to look up.
pub fn scan_text(chars: &[char], src: usize, max: usize) -> String {
    let ok = |c: char| {
        if c == '·' || c == '・' {
            return true;
        }
        if c == '\0' || c.is_whitespace() {
            return false;
        }
        use unicode_general_category::{GeneralCategory as G, get_general_category};
        !matches!(
            get_general_category(c),
            G::ConnectorPunctuation
                | G::DashPunctuation
                | G::OpenPunctuation
                | G::ClosePunctuation
                | G::InitialPunctuation
                | G::FinalPunctuation
                | G::OtherPunctuation
                | G::MathSymbol
                | G::Control
        )
    };
    match chars.get(src) {
        Some(&c) if ok(c) && c != '·' && c != '・' => {}
        _ => return String::new(),
    }
    chars[src..]
        .iter()
        .take(max.max(1))
        .take_while(|c| ok(**c))
        .collect()
}

/// `Settings::popup_theme` for a popup in the reader's own theme.
pub const SAME_AS_PAGES: &str = "pages";

/// The theme the popup is drawn in: the one saved as `choice`, or the
/// pages' theme (for [`SAME_AS_PAGES`], or a theme removed since).
pub fn popup_theme(ctx: &egui::Context, choice: &str) -> &'static Theme {
    theme::THEMES
        .iter()
        .find(|t| t.id == choice)
        .unwrap_or_else(|| theme::get(ctx))
}

/// The popup's colours, from a reader theme. Its fills are the theme's page,
/// desk and bar, and three surfaces of its text over its page at 11, 22 and
/// 33 % (Catppuccin's surface0–2 are its text over its base at about those,
/// so the Catppuccin themes get their own surfaces). Its text and accents are
/// the theme's, unchanged; the themes have no green, so an added card's ✔ is
/// their cyan (the closest hue they have).
#[derive(Clone, Copy)]
pub struct Look {
    pub theme: &'static Theme,
    /// The popup.
    pub base: Color32,
    /// Behind scroll bars.
    pub crust: Color32,
    /// Behind a greyed-out button.
    pub mantle: Color32,
    /// Badges and buttons; hovered; rules, a grip at rest.
    pub surface0: Color32,
    pub surface1: Color32,
    pub surface2: Color32,
    /// The dictionaries' bars, separators.
    pub faint: Color32,
    /// Readings, tags, the Anki button.
    pub subtle: Color32,
    /// The headword.
    pub text: Color32,
    /// The glossaries.
    pub ink: Color32,
    pub link: Color32,
    /// A word looked up in the popup (see-through).
    pub yellow: Color32,
    /// Frequency ranks, a card that can be added.
    pub teal: Color32,
    /// A card added.
    pub green: Color32,
    pub red: Color32,
}

impl Look {
    pub fn of(t: &'static Theme) -> Look {
        let surface = |a| theme::mix(t.text, t.page, a);
        Look {
            theme: t,
            base: t.page,
            crust: t.desk,
            mantle: t.bar,
            surface0: surface(0.11),
            surface1: surface(0.22),
            surface2: surface(0.33),
            faint: t.muted,
            subtle: t.subtle,
            text: t.text,
            ink: t.ink,
            link: t.accent,
            yellow: t.yellow,
            teal: t.cyan,
            green: t.cyan,
            red: t.red,
        }
    }

    /// egui's widgets in the popup (scroll bars, the bars that open and close).
    fn visuals(&self) -> egui::Visuals {
        let mut v = if self.theme.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        v.override_text_color = Some(self.text);
        v.panel_fill = self.base;
        v.window_fill = self.base;
        v.extreme_bg_color = self.crust;
        v.faint_bg_color = self.mantle;
        v.hyperlink_color = self.link;
        v.selection.bg_fill = self.link.gamma_multiply(0.35);
        for (w, c) in [
            (&mut v.widgets.inactive, self.surface0),
            (&mut v.widgets.hovered, self.surface1),
            (&mut v.widgets.active, self.surface2),
            (&mut v.widgets.open, self.surface1),
        ] {
            w.bg_fill = c;
            w.weak_bg_fill = c;
            w.fg_stroke = egui::Stroke::new(1.0, self.subtle);
        }
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, self.surface0);
        v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, self.text);
        v
    }

    /// The word a child popup was opened for.
    fn child_mark(&self) -> Color32 {
        // As the page marks the word looked up: a little stronger on light.
        self.yellow
            .gamma_multiply(if self.theme.dark { 0.3 } else { 0.38 })
    }
}

/// An entry's Anki button, as drawn.
enum CardButton<'a> {
    /// No deck or note type yet: it opens the setup.
    Setup,
    Status(Option<&'a Status>),
}

/// An entry's ✓ (known) button, as drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
enum KnownButton {
    Unknown,
    /// Marked known by the user (a click unmarks it).
    Marked,
    /// Known from Anki or a word list: shown, not changed here.
    From(KnownStatus),
}

type KnownStatus = hanstat::anki::Status;

/// Where `word` stands: marked or unmarked here (not saved yet) first, then
/// the user's own marks, then Anki and the word lists.
fn known_of(
    here: &HashMap<String, bool>,
    marked: &HashSet<String>,
    known: Option<&hanstat::anki::Knowledge>,
    word: &str,
) -> KnownButton {
    if let Some(&on) = here.get(word) {
        return if on {
            KnownButton::Marked
        } else {
            KnownButton::Unknown
        };
    }
    if marked.contains(word) {
        return KnownButton::Marked;
    }
    match known.map(|k| k.word(word)) {
        Some(s) if s.is_known() => KnownButton::From(s),
        _ => KnownButton::Unknown,
    }
}

/// The text of the book (or popup) an entry matched: what is marked known,
/// as the book wrote it (not the dictionary's headword).
fn book_word(l: &Lookup, g: &hanstat::dict::lookup::Group) -> String {
    l.text.chars().take(g.matched).collect()
}

/// A click on an entry's Anki button.
enum CardClick {
    Add(usize),
    Browse(i64),
    Setup,
}

impl Popup {
    pub fn new(
        ctx: &egui::Context,
        freq_sort: Option<String>,
        session: &mut DictionarySession,
    ) -> Popup {
        session.prefetch(ctx, hanstat::dict::root(), freq_sort.clone());
        let mut popup = Self::with_service(ctx, session.0.as_ref().unwrap().clone());
        let loaded = {
            let mut service = popup.service.lock().unwrap();
            let dictionaries = dictionary_files(&service.root);
            if freq_sort != service.freq_sort || dictionaries != service.dictionaries {
                service.worker.reload(freq_sort.clone());
                service.freq_sort = freq_sort;
                service.dictionaries = dictionaries;
            }
            service.loaded.clone()
        };
        if let Some(reply) = loaded {
            popup.take(reply);
        }
        popup.poll();
        popup
    }

    #[cfg(test)]
    pub(super) fn with_root(
        ctx: &egui::Context,
        freq_sort: Option<String>,
        root: std::path::PathBuf,
    ) -> Popup {
        let mut session = DictionarySession::default();
        session.prefetch(ctx, root, freq_sort);
        Self::with_service(ctx, session.0.unwrap())
    }

    fn with_service(ctx: &egui::Context, service: Arc<Mutex<SessionLookup>>) -> Popup {
        let c2 = ctx.clone();
        Popup {
            cards: Cards::new(Arc::new(move || c2.request_repaint())),
            service,
            renderer: Renderer::new(),
            dicts: Vec::new(),
            scan_len: 0,
            errors: Vec::new(),
            asked: None,
            levels: Vec::new(),
            anchor: None,
            last_micros: 0,
            rest: None,
            last_layout_ms: 0.0,
            scroll_lines: 0,
            size: None,
            size_changed: false,
            warmed: false,
            theme: SAME_AS_PAGES.to_owned(),
            known: None,
            marked: Arc::new(HashSet::new()),
            marked_here: HashMap::new(),
            known_toggles: Vec::new(),
        }
    }

    /// The app's known words, each frame: what the ✓ buttons show.
    pub fn set_known(
        &mut self,
        known: Option<Arc<hanstat::anki::Knowledge>>,
        marked: Arc<HashSet<String>>,
    ) {
        if !Arc::ptr_eq(&marked, &self.marked) {
            self.marked_here.clear();
        }
        self.known = known;
        self.marked = marked;
    }

    /// Where `word` (as the book wrote it) stands, for its ✓ button.
    fn known_state(&self, word: &str) -> KnownButton {
        known_of(&self.marked_here, &self.marked, self.known.as_deref(), word)
    }

    /// Mark `word` known (or unmark it): shown at once, saved by the app.
    fn toggle_known(&mut self, word: String) {
        let on = match self.known_state(&word) {
            KnownButton::Unknown => true,
            KnownButton::Marked => false,
            KnownButton::From(_) => return,
        };
        self.marked_here.insert(word.clone(), on);
        self.known_toggles.push((word, on));
    }

    /// A: the top popup's first entry's ✓ button.
    pub fn toggle_first_known(&mut self) {
        let Some(l) = self.levels.last() else { return };
        let Some(g) = l.result.groups.first() else {
            return;
        };
        let w = book_word(&l.result, g);
        if !w.is_empty() {
            self.toggle_known(w);
        }
    }

    /// The words marked or unmarked since last asked.
    pub fn take_known_toggles(&mut self) -> Vec<(String, bool)> {
        std::mem::take(&mut self.known_toggles)
    }

    pub fn has_terms(&self) -> bool {
        self.scan_len > 0
    }

    pub fn is_open(&self) -> bool {
        !self.levels.is_empty()
    }

    pub fn close(&mut self) {
        self.levels.clear();
        self.asked = None;
        self.rest = None;
        self.scroll_lines = 0;
    }

    /// Close the topmost popup (Esc).
    pub fn close_top(&mut self) {
        self.levels.pop();
        self.asked = None;
    }

    pub fn reload(&self, freq_sort: Option<String>) {
        let mut service = self.service.lock().unwrap();
        service.worker.reload(freq_sort.clone());
        service.freq_sort = freq_sort;
        service.dictionaries = dictionary_files(&service.root);
    }

    pub fn unloader(&self) -> hanstat::dict::service::Unloader {
        self.service.lock().unwrap().worker.unloader()
    }

    fn take(&mut self, r: Reply) {
        if matches!(r, Reply::Loaded { .. }) {
            self.service.lock().unwrap().loaded = Some(r.clone());
        }
        match r {
            Reply::Loaded {
                dicts,
                scan_len,
                errors,
            } => {
                self.renderer.set_dicts(&dicts);
                self.cards.set_dicts(&dicts);
                self.dicts = dicts;
                self.scan_len = scan_len;
                self.errors = errors;
                // The popups' dictionary numbers are stale. A lookup asked
                // meanwhile is answered from the new list: it stays.
                self.levels.clear();
            }
            Reply::Lookup { id, result, micros } => {
                self.last_micros = micros;
                let Some(a) = &self.asked else { return };
                if a.id != id {
                    return;
                }
                // Nothing there: the popups stay as they were.
                if result.matched() == 0 {
                    return;
                }
                let level = match a.origin {
                    Origin::Page(_) => 0,
                    Origin::Popup((l, ..)) | Origin::Link(l) => l + 1,
                };
                if level >= MAX_LEVELS || level > self.levels.len() {
                    return;
                }
                self.levels.truncate(level);
                // A channel send: Anki is asked on the cards' thread.
                self.cards.check(level, &result, a.sentence.as_ref());
                self.levels.push(Level {
                    origin: a.origin,
                    result,
                    anchor: a.anchor,
                    sentence: a.sentence.clone(),
                    details: HashMap::new(),
                    bars: HashSet::new(),
                    heights: HashMap::new(),
                    rect: None,
                    raised: false,
                    fresh: true,
                    scroll_y: 0.0,
                    #[cfg(test)]
                    drawn: (Vec::new(), Rect::NOTHING),
                });
            }
        }
    }

    /// Take in the lookup thread's answers (and the Anki thread's).
    pub fn poll(&mut self) {
        loop {
            let reply = self.service.lock().unwrap().worker.try_recv();
            let Some(r) = reply else { break };
            self.take(r);
        }
        self.cards.poll();
    }

    /// The Anki card settings as now; if they changed, the open popups'
    /// words are checked again.
    pub fn sync_cards(&mut self, s: &super::anki::CardSettings) {
        if self.cards.set_settings(s) {
            for (i, l) in self.levels.iter().enumerate() {
                self.cards.check(i, &l.result, l.sentence.as_ref());
            }
        }
    }

    /// Ask a lookup and wait briefly for it (usually answered at once, so
    /// the popup shows this frame). Answers to earlier lookups may come first.
    fn ask(
        &mut self,
        text: String,
        origin: Origin,
        anchor: Option<Rect>,
        sentence: Option<Arc<Sentence>>,
    ) {
        let id = self.service.lock().unwrap().worker.lookup(text);
        self.asked = Some(Asked {
            id,
            origin,
            anchor,
            sentence,
        });
        let deadline = std::time::Instant::now() + WAIT;
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            let reply = self.service.lock().unwrap().worker.recv_timeout(left);
            let Some(r) = reply else {
                break;
            };
            let answered = matches!(&r, Reply::Lookup { id: got, .. } if *got == id);
            self.take(r);
            if answered {
                break;
            }
        }
    }

    /// With no key, a lookup waits until the pointer rests: `true` once it has.
    fn rested(&mut self, origin: Origin, key: LookupKey, now: f64, ctx: &egui::Context) -> bool {
        if key != LookupKey::None {
            return true;
        }
        match self.rest {
            Some((o, since)) if o == origin => {
                if now - since < REST {
                    ctx.request_repaint_after(Duration::from_secs_f64(
                        REST - (now - since) + 0.005,
                    ));
                    return false;
                }
                true
            }
            _ => {
                self.rest = Some((origin, now));
                ctx.request_repaint_after(Duration::from_secs_f64(REST + 0.005));
                false
            }
        }
    }

    fn asked_for(&self, origin: Origin) -> bool {
        self.asked.as_ref().is_some_and(|a| a.origin == origin)
    }

    /// The pointer is on `spot` of the page with the lookup key held (`chars`
    /// its chapter's text). Asks a lookup when the text there is new.
    pub fn point(
        &mut self,
        spot: Spot,
        chars: &[char],
        key: LookupKey,
        now: f64,
        ctx: &egui::Context,
    ) {
        let origin = Origin::Page(spot);
        if !self.has_terms() || self.asked_for(origin) || !self.rested(origin, key, now, ctx) {
            return;
        }
        let text = scan_text(chars, spot.src as usize, self.scan_len);
        if !text.is_empty() {
            // The sentence only if cards are made (a scan of a few hundred
            // characters at most).
            let sentence = (self.cards.shown() && self.cards.cfg.ready())
                .then(|| super::anki::sentence_around(chars, spot.src as usize))
                .flatten()
                .map(Arc::new);
            self.ask(text, origin, None, sentence);
        }
    }

    /// Show `word`'s entries in the open popup (tests: words picked by hand).
    #[cfg(test)]
    pub fn show_word(&mut self, word: &str) {
        if let Some(origin) = self.levels.first().map(|l| l.origin) {
            self.levels.truncate(1);
            self.ask(word.to_owned(), origin, None, None);
        }
    }

    /// Open every dictionary's bar in the top popup (tests).
    #[cfg(test)]
    pub fn open_all(&mut self) {
        if let Some(l) = self.levels.last_mut() {
            for (gi, g) in l.result.groups.iter().enumerate() {
                for (pi, p) in g.parts.iter().enumerate() {
                    if self.dicts.get(p.dict).is_some_and(|d| d.collapsed) {
                        l.bars.insert((gi, pi));
                    }
                }
            }
        }
    }

    /// The highlighted characters in chapter `spine`: (first, count).
    pub fn highlight(&self, spine: usize) -> Option<(u32, usize)> {
        let l = self.levels.first()?;
        match l.origin {
            Origin::Page(s) if s.spine == spine => Some((s.src, l.result.matched())),
            _ => None,
        }
    }

    /// Whether `p` is on a popup.
    pub fn contains(&self, p: Pos2) -> bool {
        self.levels
            .iter()
            .any(|l| l.rect.is_some_and(|r| r.contains(p)))
    }

    /// egui draws an area it has not seen before invisibly for a frame, to
    /// measure it: done here, off screen, for every popup level, so a real
    /// popup shows in the frame it is asked.
    pub fn warm(&mut self, ctx: &egui::Context) {
        if self.warmed {
            return;
        }
        self.warmed = true;
        for level in 0..MAX_LEVELS {
            egui::Area::new(popup_id(level))
                .order(egui::Order::Foreground)
                .fade_in(false)
                .fixed_pos(pos2(-20_000.0, -20_000.0))
                .sizing_pass(true)
                .show(ctx, |ui| {
                    ui.allocate_space(vec2(300.0, 300.0));
                });
        }
    }

    /// Draw the popups. `params` are the reader's (fonts and colours);
    /// `key_held`: the lookup key is down (for looking up inside a popup).
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        params: &Params,
        show_freq: bool,
        key_held: bool,
        key: LookupKey,
        now: f64,
    ) {
        let look = Look::of(popup_theme(ctx, &self.theme));
        self.renderer.set_look(&look);
        self.renderer.images.next_frame();
        self.renderer.images.poll(ctx);
        let screen = ctx.content_rect();
        let size = (params.base * 0.8).max(13.0);
        // The entries' area: as dragged, or the default; within the window.
        let want = self.size.unwrap_or(vec2(
            size * DEFAULT_CHARS - 24.0,
            size * 1.35 * DEFAULT_LINES,
        ));
        let width = (want.x + 24.0).clamp(240.0, (screen.width() - 24.0).max(200.0));
        let content_h = want.y.clamp(100.0, (screen.height() * 0.9).max(100.0));
        self.size_changed = false;
        // A click on a popup closes the popups opened from it (where the
        // popups were last frame, as egui finds what the pointer is on).
        if ctx.input(|i| i.pointer.primary_clicked())
            && let Some(p) = ctx.input(|i| i.pointer.interact_pos())
            && let Some(top) = self
                .levels
                .iter()
                .rposition(|l| l.rect.is_some_and(|r| r.contains(p)))
            && top + 1 < self.levels.len()
        {
            self.levels.truncate(top + 1);
            self.asked = None;
        }
        let t = std::time::Instant::now();
        let mut level = 0;
        while level < self.levels.len() {
            let anchor = if level == 0 {
                self.anchor
            } else {
                self.levels[level].anchor
            };
            let Some(anchor) = anchor else {
                self.levels.truncate(level);
                break;
            };
            let w = width - (level as f32 * 24.0).min(width * 0.3);
            // A word looked up in this popup: its child popup replaces any
            // above it, and is drawn in this frame (the loop goes on to it).
            if let Some((text, origin, anchor)) = self.draw_level(
                ctx, level, anchor, w, content_h, size, params, show_freq, key_held, &look,
            ) {
                let link = matches!(origin, Origin::Link(_));
                if !self.asked_for(origin) && (link || self.rested(origin, key, now, ctx)) {
                    self.ask(text, origin, Some(anchor), None);
                }
            }
            level += 1;
        }
        // A popup shows above every other, once, when it first shows: a
        // child above the parent it came from, even if the parent was raised
        // by a click (egui raises an area clicked; raising every frame would
        // keep that order). A click on a parent closes its children, so it
        // can't come back over them.
        for (level, l) in self.levels.iter_mut().enumerate() {
            if !l.raised {
                ctx.move_to_top(egui::LayerId::new(egui::Order::Foreground, popup_id(level)));
                l.raised = true;
            }
        }
        self.last_layout_ms = t.elapsed().as_secs_f32() * 1e3;
    }

    /// Draw popup `level` beside `anchor`; returns a lookup asked from it.
    #[allow(clippy::too_many_arguments)]
    fn draw_level(
        &mut self,
        ctx: &egui::Context,
        level: usize,
        anchor: Rect,
        width: f32,
        content_h: f32,
        size: f32,
        params: &Params,
        show_freq: bool,
        key_held: bool,
        look: &Look,
    ) -> Option<(String, Origin, Rect)> {
        let screen = ctx.content_rect();
        // A fixed height: its entries scroll inside.
        let max_h = content_h;
        let below = screen.bottom() - anchor.bottom();
        let above = anchor.top() - screen.top();
        let x = anchor.left().clamp(
            screen.left() + 8.0,
            (screen.right() - width - 8.0).max(screen.left() + 8.0),
        );
        let down = below >= max_h + FRAME_V + 14.0 || below >= above;
        let room = if down { below } else { above };
        let max_h = max_h.min(room - 14.0 - FRAME_V).max(100.0);
        // Placed by its top-left corner always: its height is fixed, so its
        // top is known now above the word too; egui would place a
        // bottom-anchored area from last frame's size, a frame off.
        let pos = if down {
            pos2(x, anchor.bottom() + 6.0)
        } else {
            pos2(
                x,
                (anchor.top() - 6.0 - max_h - FRAME_V).max(screen.top() + 2.0),
            )
        };
        let pivot = egui::Align2::LEFT_TOP;
        let inner_w = width - 24.0;
        // The entries leave the scroll bar room at their right: it floats
        // over them (as wide as `bar_width` when hovered).
        let bar = ctx.style_of(ctx.theme()).spacing.scroll;
        let content_w = inner_w - bar.bar_width - bar.bar_inner_margin;
        let mut entry_params = params.clone();
        entry_params.base = size;
        entry_params.content_w = content_w;
        entry_params.page_h = 1.0e6;
        entry_params.line_spacing = 1.2;
        // Sans-serif throughout, whatever family a dictionary names.
        entry_params.serif_default = false;
        entry_params.fonts.serif = entry_params.fonts.sans.clone();
        entry_params.fonts.serif_bold = entry_params.fonts.sans_bold.clone();
        entry_params.fonts.serif_faux_bold = entry_params.fonts.sans_faux_bold;
        entry_params.text = look.ink;
        entry_params.link = look.link;
        entry_params.rule = look.surface2;
        // Dictionaries' colours as they are (as before themes).
        entry_params.page = look.base;
        entry_params.dark = false;
        let result = self.levels[level].result.clone();
        // The word a child popup was opened for: (group, part, src, len).
        let child_hl = self.levels.get(level + 1).and_then(|c| match c.origin {
            Origin::Popup((l, g, p, src)) if l == level => Some((g, p, src, c.result.matched())),
            _ => None,
        });
        let dicts = self.dicts.clone();
        // ↑ ↓ scroll the topmost popup.
        let scroll = if level + 1 == self.levels.len() {
            std::mem::take(&mut self.scroll_lines)
        } else {
            0
        };
        let mut want = None;
        let mut card_click = None;
        let mut known_click: Option<String> = None;
        let known_states: Vec<KnownButton> = result
            .groups
            .iter()
            .map(|g| self.known_state(&book_word(&result, g)))
            .collect();
        let cards = &self.cards;
        let card_ready = cards.cfg.ready();
        let mut toggle_bar = None;
        let mut toggle_details = None;
        let mut heights: Vec<((usize, usize), f32)> = Vec::new();
        // The pointer, if this popup is what it is on (not the page under an
        // entry scrolled out of view, nor a child popup over this one).
        let layer = egui::LayerId::new(egui::Order::Foreground, popup_id(level));
        let pointer = ctx
            .pointer_hover_pos()
            .filter(|p| ctx.layer_id_at(*p) == Some(layer));
        let mut resize: Option<Vec2> = None;
        let clicked = ctx.input(|i| i.pointer.primary_clicked());
        let ppp = ctx.pixels_per_point();
        let fresh = self.levels[level].fresh;
        let mut scroll_y = 0.0;
        #[cfg(test)]
        let mut drawn = (Vec::new(), Rect::NOTHING);
        let renderer = &mut self.renderer;
        let lvl = &self.levels[level];
        let area = egui::Area::new(popup_id(level))
            .order(egui::Order::Foreground)
            .fade_in(false)
            .fixed_pos(pos)
            .pivot(pivot)
            .constrain(true)
            .show(ctx, |ui| {
                *ui.visuals_mut() = look.visuals();
                let frame = egui::Frame::new()
                    .fill(look.base)
                    .stroke(egui::Stroke::new(1.0, look.surface1))
                    .corner_radius(8)
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 4],
                        blur: 18,
                        spread: 0,
                        color: Color32::from_black_alpha(if look.theme.dark { 90 } else { 40 }),
                    })
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(inner_w);
                        // Above its word, a popup takes all the room there: its
                        // bottom stays by the word whatever its entries' height
                        // (egui places it from last frame's size).
                        let mut scroll_area = egui::ScrollArea::vertical()
                            .id_salt(("dict-scroll", level))
                            .max_height(max_h)
                            .min_scrolled_height(max_h)
                            .auto_shrink([false, false]);
                        if fresh {
                            scroll_area = scroll_area.vertical_scroll_offset(0.0);
                        }
                        let _shown = scroll_area.show(ui, |ui| {
                            scroll_y = (ui.clip_rect().top() - ui.cursor().top()).max(0.0);
                            ui.spacing_mut().item_spacing.y = 4.0;
                            // Headers (their Anki buttons) and rules too.
                            ui.set_max_width(content_w);
                            if scroll != 0 {
                                ui.scroll_with_delta(vec2(0.0, -(scroll as f32) * size * 3.0));
                            }
                            for (gi, g) in result.groups.iter().enumerate() {
                                if gi > 0 {
                                    ui.add_space(4.0);
                                    ui.separator();
                                }
                                let card = cards.shown().then(|| {
                                    if card_ready {
                                        CardButton::Status(cards.status(g))
                                    } else {
                                        CardButton::Setup
                                    }
                                });
                                let (click, known_clicked, _button) = header(
                                    ui,
                                    gi,
                                    g,
                                    &dicts,
                                    size,
                                    show_freq,
                                    card,
                                    known_states[gi],
                                    look,
                                );
                                if click.is_some() {
                                    card_click = click;
                                }
                                if known_clicked {
                                    known_click = Some(book_word(&result, g));
                                }
                                #[cfg(test)]
                                drawn.0.extend(_button);
                                for (pi, p) in g.parts.iter().enumerate() {
                                    let Some(dict) = dicts.get(p.dict) else {
                                        continue;
                                    };
                                    // Open unless the dictionary is set to start
                                    // collapsed; a click on its bar toggles it.
                                    let open = !dict.collapsed != lvl.bars.contains(&(gi, pi));
                                    let label = format!(
                                        "{} {}{}",
                                        if open { "▾" } else { "▸" },
                                        dict.title,
                                        if p.glossaries.len() > 1 {
                                            format!("  ×{}", p.glossaries.len())
                                        } else {
                                            String::new()
                                        }
                                    );
                                    let bar = ui.add(
                                        egui::Label::new(
                                            RichText::new(label)
                                                .size(size * 0.72)
                                                .color(look.faint),
                                        )
                                        .sense(Sense::click()),
                                    );
                                    if bar.clicked() {
                                        toggle_bar = Some((gi, pi));
                                    }
                                    if bar.hovered() {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    if !open {
                                        continue;
                                    }
                                    // Below the view (and a screen more): its height
                                    // as last laid out, or estimated; laid out once
                                    // it scrolls near.
                                    let clip = ui.clip_rect();
                                    if ui.cursor().top() > clip.bottom() + max_h {
                                        let h = lvl.heights.get(&(gi, pi)).copied().unwrap_or_else(
                                            || estimate(&p.glossaries, content_w, size),
                                        );
                                        ui.allocate_exact_size(vec2(content_w, h), Sense::hover());
                                        continue;
                                    }
                                    let empty = HashSet::new();
                                    let opened = lvl.details.get(&(gi, pi)).unwrap_or(&empty);
                                    let e = renderer.part(
                                        ui.ctx(),
                                        dict,
                                        &p.glossaries,
                                        opened,
                                        &entry_params,
                                    );
                                    heights.push(((gi, pi), e.height));
                                    let (rect, _) = ui.allocate_exact_size(
                                        vec2(content_w, e.height.max(1.0)),
                                        Sense::hover(),
                                    );
                                    if !ui.is_rect_visible(rect) {
                                        continue;
                                    }
                                    let painter = ui.painter_at(rect.expand(2.0));
                                    let off = rect.min.to_vec2();
                                    if let Some((hg, hp, src, n)) = child_hl
                                        && (hg, hp) == (gi, pi)
                                    {
                                        for r in e.layout.char_rects(0, src..src + n as u32) {
                                            painter.rect_filled(
                                                r.translate(off),
                                                3.0,
                                                look.child_mark(),
                                            );
                                        }
                                    }
                                    for item in &e.layout.items {
                                        super::paint_item(
                                            &mut renderer.images,
                                            look.theme,
                                            &painter,
                                            item,
                                            off,
                                            ppp,
                                        );
                                    }
                                    // Links: toggles and cross-references.
                                    if let Some(p) = pointer.filter(|p| rect.contains(*p)) {
                                        let flow = p - off;
                                        let link =
                                            e.layout.links.iter().find(|l| l.rect.contains(flow));
                                        if let Some(l) = link {
                                            ui.ctx()
                                                .set_cursor_icon(egui::CursorIcon::PointingHand);
                                            if clicked {
                                                match e.chapter.links.get(l.target as usize) {
                                                    Some(LinkTarget::Toggle(n)) => {
                                                        toggle_details = Some((gi, pi, *n))
                                                    }
                                                    Some(LinkTarget::Lookup(word)) => {
                                                        want = Some((
                                                            word.clone(),
                                                            Origin::Link(level),
                                                            l.rect.translate(off),
                                                        ))
                                                    }
                                                    _ => {}
                                                }
                                            }
                                        } else if key_held
                                            && level + 1 < MAX_LEVELS
                                            && let Some(src) = e.layout.char_at(0, flow)
                                        {
                                            let text =
                                                scan_text(&e.chapter.chars, src as usize, 16);
                                            if !text.is_empty() {
                                                let r = e
                                                    .layout
                                                    .char_rects(0, src..src + 1)
                                                    .first()
                                                    .map_or(rect, |r| r.translate(off));
                                                want = Some((
                                                    text,
                                                    Origin::Popup((level, gi, pi, src)),
                                                    r,
                                                ));
                                            }
                                        }
                                    }
                                }
                            }
                        });
                        #[cfg(test)]
                        {
                            drawn.1 = _shown.inner_rect;
                        }
                    });
                // The grip in the corner away from the word: dragging it
                // resizes every popup (down and right, or up and right).
                let r = frame.response.rect;
                let grip = if down {
                    Rect::from_min_size(r.right_bottom() - vec2(GRIP, GRIP), vec2(GRIP, GRIP))
                } else {
                    Rect::from_min_size(r.right_top() + vec2(-GRIP, 0.0), vec2(GRIP, GRIP))
                };
                let g = ui.interact(grip, egui::Id::unique(("dict-grip", level)), Sense::drag());
                if g.hovered() || g.dragged() {
                    ui.ctx().set_cursor_icon(if down {
                        egui::CursorIcon::ResizeNwSe
                    } else {
                        egui::CursorIcon::ResizeNeSw
                    });
                }
                if g.dragged() {
                    let d = g.drag_delta();
                    resize = Some(vec2(d.x, if down { d.y } else { -d.y }));
                }
                let c = if g.hovered() || g.dragged() {
                    look.subtle
                } else {
                    look.surface2
                };
                let p = ui.painter();
                for k in 1..=3 {
                    let o = k as f32 * 3.5;
                    let (a, b) = if down {
                        (
                            pos2(grip.right() - 2.0, grip.bottom() - 2.0 - o),
                            pos2(grip.right() - 2.0 - o, grip.bottom() - 2.0),
                        )
                    } else {
                        (
                            pos2(grip.right() - 2.0, grip.top() + 2.0 + o),
                            pos2(grip.right() - 2.0 - o, grip.top() + 2.0),
                        )
                    };
                    p.line_segment([a, b], egui::Stroke::new(1.2, c));
                }
            });
        if let Some(d) = resize {
            let cur = vec2(inner_w + (level as f32 * 24.0).min(width * 0.3), max_h);
            let screen = ctx.content_rect();
            self.size = Some(vec2(
                (cur.x + d.x).clamp(216.0, (screen.width() - 48.0).max(216.0)),
                (cur.y + d.y).clamp(100.0, (screen.height() * 0.9).max(100.0)),
            ));
            self.size_changed = true;
        }
        match card_click {
            Some(CardClick::Add(group)) => self.cards.add(Source {
                lookup: result.clone(),
                group,
                sentence: self.levels[level].sentence.clone(),
            }),
            Some(CardClick::Browse(id)) => self.cards.browse(id),
            Some(CardClick::Setup) => self.cards.open_setup(),
            None => {}
        }
        if let Some(w) = known_click.filter(|w| !w.is_empty()) {
            self.toggle_known(w);
        }
        let l = &mut self.levels[level];
        l.fresh = false;
        l.scroll_y = scroll_y;
        l.rect = Some(area.response.rect);
        #[cfg(test)]
        {
            l.drawn = drawn;
        }
        l.heights.extend(heights);
        if let Some(b) = toggle_bar
            && !l.bars.remove(&b)
        {
            l.bars.insert(b);
        }
        if let Some((g, p, n)) = toggle_details {
            let s = l.details.entry((g, p)).or_default();
            if !s.remove(&n) {
                s.insert(n);
            }
        }
        want
    }
}

/// The egui id of popup `level`'s area.
fn popup_id(level: usize) -> egui::Id {
    egui::Id::unique(("reader-dict-popup", level))
}

/// A height for glossaries not laid out yet: their text at about a third of
/// the JSON's bytes, in lines of the popup's width.
fn estimate(gl: &[hanstat::dict::lookup::Gloss], width: f32, size: f32) -> f32 {
    let bytes: usize = gl
        .iter()
        .map(|g| g.json.as_ref().map_or(20, |j| j.len()))
        .sum();
    let chars = bytes as f32 / 3.0;
    let per_line = (width / size).max(4.0);
    (chars / per_line).ceil().max(1.0) * size * 1.4 + gl.len() as f32 * size * 0.5
}

/// A group's headword, reading and frequency ranks, and its Anki button at
/// the right (a click on it, and where it is).
#[allow(clippy::too_many_arguments)]
fn header(
    ui: &mut egui::Ui,
    gi: usize,
    g: &hanstat::dict::lookup::Group,
    dicts: &[DictInfo],
    size: f32,
    show_freq: bool,
    card: Option<CardButton>,
    known: KnownButton,
    look: &Look,
) -> (Option<CardClick>, bool, Option<Rect>) {
    let top = ui.cursor().min;
    let full = ui.available_width();
    let side = (size * 1.45).round();
    let gap = 6.0;
    let buttons = 1.0 + card.is_some() as u8 as f32;
    let reserve = buttons * side + (buttons - 1.0) * gap + 8.0;
    let row = ui.allocate_ui_with_layout(
        vec2(full - reserve, ui.spacing().interact_size.y),
        egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
        |ui| header_text(ui, g, dicts, size, show_freq, look),
    );
    // Beside the headword, on the first line: ✓, then +.
    let line = row.response.rect.height().min(size * 1.55 * 1.4);
    let y = top.y + ((line - side) / 2.0).max(0.0);
    let card_rect = Rect::from_min_size(pos2(top.x + full - side, y), vec2(side, side));
    let known_x = if card.is_some() {
        card_rect.left() - gap - side
    } else {
        card_rect.left()
    };
    let known_rect = Rect::from_min_size(pos2(known_x, y), vec2(side, side));
    let known_clicked = known_button(ui, known_rect, gi, known, look);
    let Some(card) = card else {
        return (None, known_clicked, None);
    };
    (
        card_button(ui, card_rect, gi, card, look),
        known_clicked,
        Some(card_rect),
    )
}

/// An entry's ✓ button: outlined while the word isn't known (a click marks
/// it), filled green once marked (a click unmarks it), filled in the link
/// colour when Anki or a word list knows it (not clickable).
fn known_button(ui: &mut egui::Ui, rect: Rect, gi: usize, state: KnownButton, look: &Look) -> bool {
    let (fg, filled, enabled, tip, name) = match state {
        KnownButton::Unknown => (
            look.subtle,
            false,
            true,
            "Mark as known (A)".to_owned(),
            "Mark known",
        ),
        KnownButton::Marked => (
            look.green,
            true,
            true,
            "Marked known by you. Click to unmark it (A)".to_owned(),
            "Known",
        ),
        KnownButton::From(s) => (
            look.link,
            true,
            false,
            format!("Known: {}", from_label(s)),
            "Known",
        ),
    };
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let resp = ui.interact(rect, ui.scope_id().with(("known-word", gi)), sense);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, name));
    let bg = if resp.hovered() && enabled {
        look.surface1
    } else {
        look.surface0
    };
    let p = ui.painter();
    p.rect_filled(rect, 5.0, bg);
    // Phosphor's check circle: outlined, or filled once known.
    super::icons::paint(
        p,
        rect.center(),
        super::icons::CHECK_CIRCLE,
        rect.height() * 0.78,
        filled,
        fg,
    );
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let clicked = resp.clicked();
    resp.on_hover_text(tip);
    clicked
}

fn from_label(s: KnownStatus) -> &'static str {
    match s {
        KnownStatus::Mature => "mature in Anki",
        KnownStatus::Young => "young in Anki",
        _ => "in a word list (HSK, TOCFL, a list or a book marked read)",
    }
}

/// An entry's Anki button: + to add (greyed out when the word is already in
/// Anki), ✔ once added (opens it in Anki's browser).
fn card_button(
    ui: &mut egui::Ui,
    rect: Rect,
    gi: usize,
    card: CardButton,
    look: &Look,
) -> Option<CardClick> {
    let (label, fg, enabled, tip, click): (&str, Color32, bool, String, Option<CardClick>) =
        match card {
            CardButton::Setup => (
                "+",
                look.subtle,
                true,
                "Add to Anki: choose a deck, note type and fields first".into(),
                Some(CardClick::Setup),
            ),
            CardButton::Status(None | Some(Status::Checking)) => (
                "+",
                look.subtle,
                true,
                "Add to Anki".into(),
                Some(CardClick::Add(gi)),
            ),
            CardButton::Status(Some(Status::CanAdd)) => (
                "+",
                look.teal,
                true,
                "Add to Anki".into(),
                Some(CardClick::Add(gi)),
            ),
            CardButton::Status(Some(Status::Duplicate)) => {
                ("+", look.surface2, false, "Already in Anki".into(), None)
            }
            CardButton::Status(Some(Status::Adding)) => {
                ("…", look.subtle, false, "Adding…".into(), None)
            }
            CardButton::Status(Some(Status::Added(id))) => (
                "✔",
                look.green,
                true,
                "Added. Click to see it in Anki's browser".into(),
                Some(CardClick::Browse(*id)),
            ),
            CardButton::Status(Some(Status::AddedWithout(id, why))) => (
                "✔",
                look.yellow,
                true,
                format!("Added, without audio: {why}\nClick to see it in Anki's browser"),
                Some(CardClick::Browse(*id)),
            ),
            CardButton::Status(Some(Status::Failed(e))) => (
                "+",
                look.red,
                true,
                format!("{e}\nClick to try adding it anyway"),
                Some(CardClick::Add(gi)),
            ),
        };
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let resp = ui.interact(rect, ui.scope_id().with(("anki-card", gi)), sense);
    let bg = if !enabled {
        look.mantle
    } else if resp.hovered() {
        look.surface1
    } else {
        look.surface0
    };
    let p = ui.painter();
    p.rect_filled(rect, 5.0, bg);
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(rect.height() * 0.72),
        fg,
    );
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let clicked = resp.clicked();
    resp.on_hover_text(tip);
    if clicked { click } else { None }
}

fn header_text(
    ui: &mut egui::Ui,
    g: &hanstat::dict::lookup::Group,
    dicts: &[DictInfo],
    size: f32,
    show_freq: bool,
    look: &Look,
) {
    ui.spacing_mut().item_spacing.x = 8.0;
    ui.label(
        RichText::new(&g.headword)
            .size(size * 1.55)
            .color(look.text),
    );
    if g.reading != g.headword && !g.reading.is_empty() {
        ui.label(
            RichText::new(&g.reading)
                .size(size * 1.05)
                .color(look.subtle),
        );
    }
    if show_freq {
        for b in &g.freqs {
            let v = b
                .values
                .iter()
                .map(|v| v.display.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let name = dicts.get(b.dict).map_or("", |d| d.title.as_str());
            let text = if b.folded {
                format!("{name} {v} (简)")
            } else {
                format!("{name} {v}")
            };
            freq_badge(ui, text, size, look).on_hover_text(if b.folded {
                "The rank of the Simplified form: this list has none for the Traditional one"
            } else {
                "Frequency rank"
            });
        }
    }
}

/// One frequency rank, laid out whole before it is placed. A badge that does
/// not fit in what is left of the line goes to the next line; it only wraps
/// inside when it is wider than a whole line. (An `egui::Frame` here took the
/// rest of the line, however narrow, and its label wrapped a letter a line.)
fn freq_badge(ui: &mut egui::Ui, text: String, size: f32, look: &Look) -> egui::Response {
    let pad = vec2(5.0, 1.0);
    let line = (ui.max_rect().width() - 2.0 * pad.x).max(size);
    let font = egui::FontId::proportional(size * 0.68);
    let galley = ui.fonts_mut(|f| f.layout(text, font, look.teal, line));
    let (rect, resp) = ui.allocate_exact_size(galley.size() + 2.0 * pad, Sense::hover());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect_filled(rect, 4.0, look.surface0);
        p.galley(rect.min + pad, galley, look.teal);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_buttons_are_drawn() {
        // The three ✓ states on the popup's colours; pictures in
        // TTS_LAYOUT_OUT, if set.
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let look = Look::of(&theme::THEMES[0]);
        let mut screen = crate::raster::Screen::new();
        let size = vec2(200.0, 60.0);
        let states = [
            KnownButton::Unknown,
            KnownButton::Marked,
            KnownButton::From(hanstat::anki::Status::Mature),
        ];
        let mut glyphs = Vec::new();
        for i in 0..3 {
            let mut frame = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ui| {
                    ui.painter().rect_filled(ui.max_rect(), 0.0, look.base);
                    for (gi, st) in states.iter().enumerate() {
                        let r = Rect::from_min_size(
                            pos2(20.0 + gi as f32 * 40.0, 16.0),
                            vec2(26.0, 26.0),
                        );
                        known_button(ui, r, gi, *st, &look);
                    }
                },
            );
            screen.absorb(&mut frame);
            if i == 2 {
                fn walk(s: &egui::Shape, out: &mut Vec<char>) {
                    match s {
                        egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                        egui::Shape::Text(t) => out.extend(t.galley.text().chars()),
                        _ => {}
                    }
                }
                frame
                    .shapes
                    .iter()
                    .for_each(|s| walk(&s.shape, &mut glyphs));
                if let Some(dir) = std::env::var_os("TTS_LAYOUT_OUT") {
                    let dir = std::path::PathBuf::from(dir);
                    std::fs::create_dir_all(&dir).unwrap();
                    screen
                        .picture(&ctx, &mut frame, size)
                        .write_png(&dir.join("known-buttons.png"))
                        .unwrap();
                }
            }
        }
        assert_eq!(
            glyphs
                .iter()
                .filter(|c| **c == super::super::icons::CHECK_CIRCLE)
                .count(),
            3
        );
    }

    #[test]
    fn known_buttons_put_your_marks_before_anki_and_lists() {
        let mut k = hanstat::anki::Knowledge::empty(false);
        k.add_listed(["学校"], "HSK".into());
        let marked: HashSet<String> = ["老师".to_owned()].into();
        let mut here = HashMap::new();
        let st = |here: &HashMap<String, bool>, w| known_of(here, &marked, Some(&k), w);
        assert_eq!(st(&here, "书包"), KnownButton::Unknown);
        assert_eq!(st(&here, "老师"), KnownButton::Marked);
        assert_eq!(
            st(&here, "学校"),
            KnownButton::From(hanstat::anki::Status::Listed)
        );
        // Changed here, before the app has saved it: shown at once.
        here.insert("书包".to_owned(), true);
        here.insert("老师".to_owned(), false);
        assert_eq!(st(&here, "书包"), KnownButton::Marked);
        assert_eq!(st(&here, "老师"), KnownButton::Unknown);
        assert_eq!(known_of(&here, &marked, None, "学校"), KnownButton::Unknown);
    }

    #[test]
    fn scans_stop_at_punctuation_and_blocks() {
        let c: Vec<char> = "他说：「约翰·史密斯来了。」\0下一段".chars().collect();
        assert_eq!(scan_text(&c, 0, 16), "他说");
        assert_eq!(scan_text(&c, 2, 16), "", "on punctuation: nothing");
        assert_eq!(scan_text(&c, 4, 16), "约翰·史密斯来了");
        assert_eq!(scan_text(&c, 4, 3), "约翰·");
        assert_eq!(scan_text(&c, 6, 16), "", "not from a middle dot");
        let end = c.iter().position(|x| *x == '\0').unwrap();
        assert_eq!(scan_text(&c, end - 1, 16), "");
        assert_eq!(scan_text(&c, end + 1, 16), "下一段");
        assert_eq!(scan_text(&c, 99, 16), "");
        let c: Vec<char> = "yyds 666".chars().collect();
        assert_eq!(scan_text(&c, 0, 16), "yyds");
    }

    /// Frequency badges after a long headword: a badge that does not fit in
    /// what is left of the line starts the next line whole, one line high.
    /// (They used to wrap a letter a line in the leftover width, and the
    /// header grew taller than the popup.) Checked against the font's own
    /// row height, as CI's fonts differ from Windows'.
    #[test]
    fn freq_badges_wrap_whole() {
        let names = ["Danmei", "BLCUcoll", "BLCUlit", "SUBTLEX-CH", "Wordfreq"];
        let size = 18.0;
        let look = Look::of(&theme::THEMES[0]);
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        for width in [520.0_f32, 300.0, 200.0] {
            // What a headword and reading leave: 40 px of the first line.
            for lead in [width - 40.0, 0.0] {
                let mut rects = vec![];
                let mut left = 0.0;
                let mut row_h = 0.0;
                for _ in 0..2 {
                    rects.clear();
                    let input = egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 1000.0))),
                        ..Default::default()
                    };
                    let mut out = ctx.run_ui(input, |ui| {
                        ui.allocate_ui_with_layout(
                            vec2(width, 20.0),
                            egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
                            |ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                left = ui.max_rect().left();
                                row_h = ui.fonts_mut(|f| {
                                    f.row_height(&egui::FontId::proportional(size * 0.68))
                                });
                                if lead > 0.0 {
                                    ui.allocate_exact_size(vec2(lead, size), Sense::hover());
                                }
                                for n in names {
                                    rects.push(
                                        freq_badge(ui, format!("{n} 26819"), size, &look).rect,
                                    );
                                }
                            },
                        );
                    });
                    out.textures_delta.clear();
                }
                for (n, r) in names.iter().zip(&rects) {
                    assert!(
                        r.height() <= row_h + 2.0 + 0.5,
                        "width {width}, lead {lead}: {n} is {} tall, one line is {row_h}",
                        r.height()
                    );
                    assert!(
                        r.right() <= left + width + 0.5,
                        "width {width}: {n} sticks out"
                    );
                }
                if lead > 0.0 {
                    assert!(
                        (rects[0].left() - left).abs() < 0.5,
                        "width {width}: the first badge starts the next line: {:?}",
                        rects[0]
                    );
                }
            }
        }
    }
}
