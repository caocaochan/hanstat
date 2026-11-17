//! Keyboard shortcuts: what each one does, its default keys, and the keys
//! chosen instead in the Settings window (⚙), saved in `settings.json` as
//! [`hanstat::settings::Settings::shortcuts`]: only those that differ from
//! the default, so a new default reaches everyone who kept the old one.
//!
//! Keys are matched exactly (Ctrl+K does not fire on Ctrl+Shift+K). "Ctrl" is
//! egui's `command`: Ctrl, or ⌘ on a Mac.

use eframe::egui::{self, Event, Key, KeyboardShortcut, Modifiers};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Open,
    Library,
    Filter,
    Overview,
    Characters,
    Words,
    Sections,
    Suggestions,
    Notes,
    NextTab,
    PrevTab,
    KnownWords,
    Anki,
    ForcedWords,
    Settings,
    CloseWindow,
    MarkKnown,
}

const N: usize = Action::ALL.len();

impl Action {
    pub const ALL: [Action; 17] = [
        Action::Open,
        Action::Library,
        Action::Filter,
        Action::Overview,
        Action::Characters,
        Action::Words,
        Action::Sections,
        Action::Suggestions,
        Action::Notes,
        Action::NextTab,
        Action::PrevTab,
        Action::KnownWords,
        Action::Anki,
        Action::ForcedWords,
        Action::Settings,
        Action::CloseWindow,
        Action::MarkKnown,
    ];

    /// Its key in `settings.json`.
    fn id(self) -> &'static str {
        match self {
            Action::Open => "open",
            Action::Library => "library",
            Action::Filter => "filter",
            Action::Overview => "tab-overview",
            Action::Characters => "tab-characters",
            Action::Words => "tab-words",
            Action::Sections => "tab-sections",
            Action::Suggestions => "tab-suggestions",
            Action::Notes => "tab-notes",
            Action::NextTab => "tab-next",
            Action::PrevTab => "tab-previous",
            Action::KnownWords => "known-words",
            Action::Anki => "anki",
            Action::ForcedWords => "forced-words",
            Action::Settings => "settings",
            Action::CloseWindow => "close-window",
            Action::MarkKnown => "mark-known",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Action::Open => "Open books…",
            Action::Library => "Show or hide the Library",
            Action::Filter => "Search box",
            Action::Overview => "Overview tab",
            Action::Characters => "Characters tab",
            Action::Words => "Words tab",
            Action::Sections => "Sections tab",
            Action::Suggestions => "Suggested words tab",
            Action::Notes => "Notes & method tab",
            Action::NextTab => "Next tab",
            Action::PrevTab => "Previous tab",
            Action::KnownWords => "Known words window",
            Action::Anki => "Anki window",
            Action::ForcedWords => "Forced words window",
            Action::Settings => "Settings window",
            Action::CloseWindow => "Close the front window",
            Action::MarkKnown => "Mark the selected word known",
        }
    }

    /// More on what it does, for its row's hover text.
    pub fn hover(self) -> Option<&'static str> {
        Some(match self {
            Action::Filter => {
                "In the Characters and Words tabs, their filter; in the other tabs, the Words tab's. \
                 In the Library (or with no book open), its search."
            }
            Action::NextTab | Action::PrevTab => {
                "The book's tabs, or the Library's while it is shown."
            }
            Action::CloseWindow => {
                "The window in front of the others (Anki, Known words, Forced words, Settings, Mark read). \
                 In a text box, the first press leaves the box."
            }
            Action::MarkKnown => "In the Words tab, or unmark it.",
            Action::Overview
            | Action::Characters
            | Action::Words
            | Action::Sections
            | Action::Suggestions
            | Action::Notes => "Also leaves the Library for the open book.",
            _ => return None,
        })
    }

    fn default_keys(self) -> Option<KeyboardShortcut> {
        let ctrl = Modifiers::COMMAND;
        let (m, k) = match self {
            Action::Open => (ctrl, Key::O),
            Action::Library => (ctrl, Key::L),
            Action::Filter => (ctrl, Key::F),
            Action::Overview => (ctrl, Key::Num1),
            Action::Characters => (ctrl, Key::Num2),
            Action::Words => (ctrl, Key::Num3),
            Action::Sections => (ctrl, Key::Num4),
            Action::Suggestions => (ctrl, Key::Num5),
            Action::Notes => (ctrl, Key::Num6),
            Action::NextTab => (ctrl, Key::PageDown),
            Action::PrevTab => (ctrl, Key::PageUp),
            Action::KnownWords => (ctrl, Key::K),
            Action::Anki => (ctrl | Modifiers::SHIFT, Key::K),
            Action::ForcedWords => (ctrl, Key::D),
            Action::Settings => (ctrl, Key::Comma),
            Action::CloseWindow => (Modifiers::NONE, Key::Escape),
            Action::MarkKnown => (Modifiers::NONE, Key::A),
        };
        Some(KeyboardShortcut::new(m, k))
    }

    /// Handled where it applies (the Words table), not by [`Shortcuts::pressed`].
    fn local(self) -> bool {
        self == Action::MarkKnown
    }

    fn index(self) -> usize {
        Action::ALL.iter().position(|&a| a == self).unwrap()
    }
}

pub struct Shortcuts {
    keys: [Option<KeyboardShortcut>; N],
}

impl Default for Shortcuts {
    fn default() -> Self {
        Shortcuts {
            keys: Action::ALL.map(Action::default_keys),
        }
    }
}

impl Shortcuts {
    /// From the changes saved in `settings.json` (an unknown action or keys
    /// that don't parse keep the default; an empty string means none).
    pub fn from_saved(saved: &BTreeMap<String, String>) -> Self {
        let mut s = Shortcuts::default();
        for a in Action::ALL {
            if let Some(v) = saved.get(a.id()) {
                if v.is_empty() {
                    s.keys[a.index()] = None;
                } else if let Some(k) = parse(v) {
                    s.keys[a.index()] = Some(k);
                }
            }
        }
        // Two actions on the same keys (a new default the user had already
        // chosen for something else, or a hand edit): the keys chosen win
        // over a default, else the first action keeps them.
        let chosen = |a: Action| saved.contains_key(a.id());
        for i in 0..N {
            for j in i + 1..N {
                if s.keys[i].is_some() && s.keys[j] == s.keys[i] {
                    let (a, b) = (Action::ALL[i], Action::ALL[j]);
                    let lose = if chosen(b) && !chosen(a) { a } else { b };
                    s.keys[lose.index()] = None;
                }
            }
        }
        s
    }

    /// What to save: the actions whose keys differ from the default.
    pub fn to_saved(&self) -> BTreeMap<String, String> {
        Action::ALL
            .into_iter()
            .filter(|&a| !self.is_default(a))
            .map(|a| {
                (
                    a.id().to_owned(),
                    self.get(a).map(format).unwrap_or_default(),
                )
            })
            .collect()
    }

    pub fn get(&self, a: Action) -> Option<KeyboardShortcut> {
        self.keys[a.index()]
    }

    pub fn is_default(&self, a: Action) -> bool {
        self.get(a) == a.default_keys()
    }

    /// Give `a` these keys, taking them from the action that had them, which is returned.
    pub fn set(&mut self, a: Action, keys: Option<KeyboardShortcut>) -> Option<Action> {
        let taken = keys.and_then(|k| {
            Action::ALL
                .into_iter()
                .find(|&b| b != a && self.get(b) == Some(k))
        });
        if let Some(b) = taken {
            self.keys[b.index()] = None;
        }
        self.keys[a.index()] = keys;
        taken
    }

    /// The keys as shown (Ctrl+K; ⌘K on a Mac).
    pub fn text(&self, ctx: &egui::Context, a: Action) -> Option<String> {
        self.get(a).map(|k| display(ctx, k))
    }

    /// `text` with " (keys)" after it, for a button's hover text.
    pub fn hint(&self, ctx: &egui::Context, a: Action, text: &str) -> String {
        match self.text(ctx, a) {
            Some(k) => format!("{text} ({k})"),
            None => text.to_owned(),
        }
    }

    /// The actions whose keys were pressed this frame (taking those key
    /// presses). With `plain_blocked` (a text box is being typed in, or a
    /// menu is open) keys without Ctrl or Alt are left alone: they type, or
    /// close the menu.
    pub fn pressed(&self, ctx: &egui::Context, plain_blocked: bool) -> Vec<Action> {
        ctx.input_mut(|i| {
            Action::ALL
                .into_iter()
                .filter(|a| !a.local())
                .filter(|&a| {
                    self.get(a).is_some_and(|k| {
                        let plain = !(k.modifiers.command || k.modifiers.ctrl || k.modifiers.alt);
                        !(plain && plain_blocked) && consume(i, k)
                    })
                })
                .collect()
        })
    }

    /// Take a press of `a`'s keys this frame, if there was one.
    pub fn consume(&self, ctx: &egui::Context, a: Action) -> bool {
        self.get(a)
            .is_some_and(|k| ctx.input_mut(|i| consume(i, k)))
    }
}

/// Keys as shown: egui's names, but `Ctrl+,` and `Page Down` rather than
/// `Ctrl+Comma` and `PageDown` (a Mac has its symbols: ⌘,).
pub fn display(ctx: &egui::Context, k: KeyboardShortcut) -> String {
    if ctx.os().is_mac() {
        return ctx.format_shortcut(&k);
    }
    let key = match k.logical_key {
        Key::Escape => "Esc",
        Key::PageUp => "Page Up",
        Key::PageDown => "Page Down",
        Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight => {
            k.logical_key.symbol_or_name()
        }
        Key::Comma
        | Key::Period
        | Key::Semicolon
        | Key::Colon
        | Key::Slash
        | Key::Backslash
        | Key::Minus
        | Key::Plus
        | Key::Equals
        | Key::OpenBracket
        | Key::CloseBracket
        | Key::Backtick
        | Key::Quote => k.logical_key.symbol_or_name(),
        other => other.name(),
    };
    let mods = egui::ModifierNames::NAMES.format(&k.modifiers, false);
    if mods.is_empty() {
        key.to_owned()
    } else {
        format!("{mods}+{key}")
    }
}

/// Take the presses of exactly these keys (repeats included); whether there were any.
fn consume(i: &mut egui::InputState, k: KeyboardShortcut) -> bool {
    let mut hit = false;
    i.events.retain(|e| {
        let m = matches!(e, Event::Key { key, pressed: true, modifiers, .. }
            if *key == k.logical_key && modifiers.matches_exact(k.modifiers));
        hit |= m;
        !m
    });
    hit
}

/// Pressed modifiers as a pattern: Ctrl (⌘ on a Mac) as `command`, the
/// Mac's Control key as `ctrl`.
fn pattern(m: Modifiers) -> Modifiers {
    Modifiers {
        alt: m.alt,
        shift: m.shift,
        command: m.command,
        ctrl: m.ctrl && !m.command,
        mac_cmd: false,
    }
}

/// As saved: `Ctrl+Shift+K` (`MacCtrl` for a Mac's Control key).
fn format(k: KeyboardShortcut) -> String {
    let m = k.modifiers;
    let mut s = String::new();
    for (on, name) in [
        (m.command, "Ctrl"),
        (m.ctrl, "MacCtrl"),
        (m.alt, "Alt"),
        (m.shift, "Shift"),
    ] {
        if on {
            s.push_str(name);
            s.push('+');
        }
    }
    s + k.logical_key.name()
}

fn parse(s: &str) -> Option<KeyboardShortcut> {
    let (mods, key) = match s.rsplit_once('+') {
        // "Ctrl++": the key is +.
        Some((m, "")) => (m.strip_suffix('+')?, "+"),
        Some((m, k)) => (m, k),
        None => ("", s),
    };
    let mut m = Modifiers::NONE;
    for part in mods.split('+').filter(|p| !p.is_empty()) {
        match part {
            "Ctrl" | "Cmd" => m.command = true,
            "MacCtrl" => m.ctrl = true,
            "Alt" => m.alt = true,
            "Shift" => m.shift = true,
            _ => return None,
        }
    }
    Some(KeyboardShortcut::new(m, Key::from_name(key)?))
}

/// What a press means while new keys are being chosen for an action.
pub enum Captured {
    /// Nothing pressed yet.
    Waiting,
    /// Esc: keep the old keys.
    Cancel,
    Keys(KeyboardShortcut),
    /// Keys that can't be used, and why.
    Refused(String),
}

/// The keys pressed while choosing them; takes every key and text event this
/// frame so that they don't act on anything else.
pub fn capture(ctx: &egui::Context) -> Captured {
    let first = ctx.input_mut(|i| {
        let first = i.events.iter().find_map(|e| match e {
            Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } => Some(KeyboardShortcut::new(pattern(*modifiers), *key)),
            _ => None,
        });
        i.events.retain(|e| {
            !matches!(
                e,
                Event::Key { .. } | Event::Text(_) | Event::Copy | Event::Cut | Event::Paste(_)
            )
        });
        first
    });
    let Some(k) = first else {
        return Captured::Waiting;
    };
    if k.logical_key == Key::Escape && k.modifiers.is_none() {
        return Captured::Cancel;
    }
    match reserved(k) {
        Some(why) => Captured::Refused(format!("{} {why}", display(ctx, k))),
        None => Captured::Keys(k),
    }
}

/// Why these keys can't be a shortcut, if they can't.
fn reserved(k: KeyboardShortcut) -> Option<&'static str> {
    let m = k.modifiers;
    let ctrl_only = m.command && !m.ctrl && !m.alt;
    let plain = m.is_none();
    match k.logical_key {
        Key::Tab if plain || m == Modifiers::SHIFT => Some("moves between buttons and boxes."),
        Key::A | Key::Z | Key::Y if ctrl_only => Some("edits text in a text box."),
        Key::ArrowUp
        | Key::ArrowDown
        | Key::ArrowLeft
        | Key::ArrowRight
        | Key::Home
        | Key::End
        | Key::PageUp
        | Key::PageDown
            if plain =>
        {
            Some("moves through the Words tab and text boxes.")
        }
        Key::Enter | Key::Space | Key::Backspace | Key::Delete if plain => {
            Some("presses buttons or edits text.")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_round_trip() {
        let mut s = Shortcuts::default();
        let ck = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::K);
        // Taken from Anki, which had it.
        assert_eq!(s.set(Action::Library, Some(ck)), Some(Action::Anki));
        s.set(Action::Open, None);
        let saved = s.to_saved();
        assert_eq!(
            saved.get("library").map(String::as_str),
            Some("Ctrl+Shift+K")
        );
        assert_eq!(saved.get("anki").map(String::as_str), Some(""));
        assert_eq!(saved.get("open").map(String::as_str), Some(""));
        assert_eq!(saved.len(), 3);
        let back = Shortcuts::from_saved(&saved);
        for a in Action::ALL {
            assert_eq!(back.get(a), s.get(a), "{a:?}");
        }
    }

    #[test]
    fn parse_keys() {
        for k in Action::ALL.map(|a| a.default_keys().unwrap()) {
            assert_eq!(parse(&format(k)), Some(k));
        }
        let plus = KeyboardShortcut::new(Modifiers::COMMAND, Key::Plus);
        assert_eq!(parse(&format(plus)), Some(plus));
        assert_eq!(
            parse("Ctrl+Comma"),
            Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::Comma))
        );
        assert_eq!(parse("Hyper+K"), None);
        assert_eq!(parse("Ctrl+NoSuchKey"), None);
    }

    #[test]
    fn chosen_keys_win_over_a_default() {
        let saved = BTreeMap::from([("anki".to_owned(), "Ctrl+O".to_owned())]);
        let s = Shortcuts::from_saved(&saved);
        assert_eq!(s.get(Action::Anki), Action::Open.default_keys());
        assert_eq!(s.get(Action::Open), None);
        // Both chosen: the first keeps them.
        let saved = BTreeMap::from([
            ("anki".to_owned(), "Ctrl+J".to_owned()),
            ("settings".to_owned(), "Ctrl+J".to_owned()),
        ]);
        let s = Shortcuts::from_saved(&saved);
        assert_eq!(s.get(Action::Anki), parse("Ctrl+J"));
        assert_eq!(s.get(Action::Settings), None);
    }

    #[test]
    fn exact_modifiers() {
        let k = KeyboardShortcut::new(Modifiers::COMMAND, Key::K);
        let press = |m| Event::Key {
            key: Key::K,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: m,
        };
        let windows_ctrl = Modifiers {
            ctrl: true,
            command: true,
            ..Modifiers::NONE
        };
        let mut i = egui::InputState::default();
        i.events = vec![press(windows_ctrl | Modifiers::SHIFT)];
        assert!(!consume(&mut i, k));
        i.events = vec![press(windows_ctrl)];
        assert!(consume(&mut i, k));
        assert!(i.events.is_empty());
        assert_eq!(pattern(windows_ctrl), Modifiers::COMMAND);
    }
}
