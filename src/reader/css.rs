//! A CSS parser for the subset EPUBs use: rules, `@import`, `@media` (as a
//! reflowable e-reader on a screen sees them) and declarations. Selectors are
//! matched with `scraper` (the `selectors` crate); unsupported ones (pseudo-
//! elements, `:hover`) are dropped.

use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Decl {
    pub name: String,
    pub value: String,
    pub important: bool,
}

pub struct Rule {
    pub selector: scraper::Selector,
    /// (ids, classes/attributes/pseudo-classes, types), packed for ordering.
    pub specificity: u32,
    /// Source order across every sheet of the document.
    pub order: u32,
    pub decls: Arc<[Decl]>,
}

/// The rules of a document, indexed by the rightmost compound selector's
/// id, first class or type, so an element is only matched against rules that
/// can apply to it.
#[derive(Default)]
pub struct RuleSet {
    pub rules: Vec<Rule>,
    by_id: HashMap<String, Vec<u32>>,
    by_class: HashMap<String, Vec<u32>>,
    by_tag: HashMap<String, Vec<u32>>,
    any: Vec<u32>,
}

enum Key {
    Id(String),
    Class(String),
    Tag(String),
    Any,
}

impl RuleSet {
    pub fn add(&mut self, selector_text: &str, decls: Arc<[Decl]>, order: u32) {
        for sel in split_top(selector_text, ',') {
            let sel = sel.trim();
            if sel.is_empty() {
                continue;
            }
            let Ok(selector) = scraper::Selector::parse(sel) else {
                continue;
            };
            let i = self.rules.len() as u32;
            self.rules.push(Rule {
                selector,
                specificity: specificity(sel),
                order,
                decls: decls.clone(),
            });
            match key(sel) {
                Key::Id(k) => self.by_id.entry(k).or_default().push(i),
                Key::Class(k) => self.by_class.entry(k).or_default().push(i),
                Key::Tag(k) => self.by_tag.entry(k).or_default().push(i),
                Key::Any => self.any.push(i),
            }
        }
    }

    /// The rules whose key matches `el` (they still have to be matched).
    pub fn candidates(&self, el: &scraper::node::Element, out: &mut Vec<u32>) {
        out.clear();
        if let Some(id) = el.id()
            && let Some(v) = self.by_id.get(id)
        {
            out.extend(v);
        }
        for c in el.classes() {
            if let Some(v) = self.by_class.get(c) {
                out.extend(v);
            }
        }
        if let Some(v) = self.by_tag.get(el.name()) {
            out.extend(v);
        }
        out.extend(&self.any);
        out.sort_unstable();
        out.dedup();
    }
}

/// Split at `sep` outside brackets, parentheses and strings.
pub fn split_top(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(' | '[') => depth += 1,
            (None, ')' | ']') => depth -= 1,
            (None, c) if c == sep && depth <= 0 => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '-' || c == '_' || c == '\\' || !c.is_ascii()
}

/// Specificity of one complex selector, as a sortable number.
fn specificity(sel: &str) -> u32 {
    let (mut a, mut b, mut c) = (0u32, 0u32, 0u32);
    let cs: Vec<char> = sel.chars().collect();
    let mut i = 0;
    let skip_ident = |i: &mut usize| {
        while *i < cs.len() && is_ident(cs[*i]) {
            *i += 1;
        }
    };
    while i < cs.len() {
        match cs[i] {
            '#' => {
                a += 1;
                i += 1;
                skip_ident(&mut i);
            }
            '.' => {
                b += 1;
                i += 1;
                skip_ident(&mut i);
            }
            '[' => {
                b += 1;
                while i < cs.len() && cs[i] != ']' {
                    i += 1;
                }
                i += 1;
            }
            ':' => {
                i += 1;
                if cs.get(i) == Some(&':') {
                    c += 1;
                    i += 1;
                    skip_ident(&mut i);
                } else {
                    let start = i;
                    skip_ident(&mut i);
                    let name: String = cs[start..i].iter().collect();
                    if cs.get(i) == Some(&'(') {
                        // :not(x) counts as x; others as one pseudo-class.
                        let open = i;
                        let mut depth = 0;
                        while i < cs.len() {
                            match cs[i] {
                                '(' => depth += 1,
                                ')' => {
                                    depth -= 1;
                                    if depth == 0 {
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            i += 1;
                        }
                        if name == "not" {
                            let inner: String = cs[open + 1..i.min(cs.len())].iter().collect();
                            let s = specificity(&inner);
                            a += s >> 16;
                            b += (s >> 8) & 0xFF;
                            c += s & 0xFF;
                        } else {
                            b += 1;
                        }
                        i += 1;
                    } else {
                        b += 1;
                    }
                }
            }
            ch if is_ident(ch) => {
                c += 1;
                skip_ident(&mut i);
            }
            _ => i += 1,
        }
    }
    (a.min(255) << 16) | (b.min(255) << 8) | c.min(255)
}

/// The index key of a complex selector: its last compound's id, else first
/// class, else type.
fn key(sel: &str) -> Key {
    // The last compound: after the last combinator outside brackets.
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in sel.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ' ' | '>' | '+' | '~' if depth == 0 => start = i + 1,
            _ => {}
        }
    }
    let last = &sel[start..];
    let cs: Vec<(usize, char)> = last.char_indices().collect();
    let ident_at = |from: usize| -> String {
        last[from..]
            .chars()
            .take_while(|c| is_ident(*c) && *c != '\\')
            .collect()
    };
    // Inside :not(...) or [...] nothing is a key.
    let mut depth = 0i32;
    let mut class = None;
    for &(i, c) in &cs {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '#' if depth == 0 => {
                let id = ident_at(i + 1);
                if !id.is_empty() {
                    return Key::Id(id);
                }
            }
            '.' if depth == 0 && class.is_none() => {
                let c = ident_at(i + 1);
                if !c.is_empty() {
                    class = Some(c);
                }
            }
            _ => {}
        }
    }
    if let Some(c) = class {
        return Key::Class(c);
    }
    let tag = ident_at(0);
    if !tag.is_empty() && !tag.starts_with('-') {
        return Key::Tag(tag.to_ascii_lowercase());
    }
    Key::Any
}

/// Remove comments.
fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        match rest[i + 2..].find("*/") {
            Some(j) => rest = &rest[i + 2 + j + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Parse declarations (`a: b; c: d !important`).
pub fn parse_declarations(s: &str) -> Vec<Decl> {
    let mut out = Vec::new();
    for d in split_top(s, ';') {
        let Some((name, value)) = d.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let mut value = value.trim();
        let mut important = false;
        if let Some(i) = value.to_ascii_lowercase().rfind("!important") {
            important = true;
            value = value[..i].trim();
        }
        if name.is_empty() || value.is_empty() {
            continue;
        }
        out.push(Decl {
            name,
            value: value.to_owned(),
            important,
        });
    }
    out
}

/// Whether a media query list applies to a reflowable e-reader screen.
fn media_ok(q: &str) -> bool {
    let q = q.trim().to_ascii_lowercase();
    if q.is_empty() {
        return true;
    }
    split_top(&q, ',').iter().any(|one| {
        let one = one.trim();
        let (neg, one) = match one.strip_prefix("not ") {
            Some(r) => (true, r.trim()),
            None => (false, one.strip_prefix("only ").unwrap_or(one).trim()),
        };
        let ty = one.split_whitespace().next().unwrap_or("all");
        let type_ok = if ty.starts_with('(') {
            true
        } else {
            matches!(ty, "all" | "screen")
        };
        // Kindle-only and fixed-layout queries are not for us.
        let features_ok =
            !one.contains("amzn-") && !one.contains("-epub-") || one.contains("writing-mode");
        (type_ok && features_ok) != neg
    })
}

/// Parsing state: the sheets are loaded through `load` (ZIP path → text) and
/// their rules added in document order.
pub struct Parser<'a> {
    pub set: &'a mut RuleSet,
    pub order: u32,
    pub load: &'a dyn Fn(&str) -> Option<String>,
    pub seen: Vec<String>,
}

impl Parser<'_> {
    /// A sheet at ZIP path `path`, with `@import`s resolved relative to it.
    pub fn sheet_file(&mut self, path: &str) {
        if self.seen.iter().any(|p| p == path) || self.seen.len() > 64 {
            return;
        }
        self.seen.push(path.to_owned());
        if let Some(text) = (self.load)(path) {
            self.sheet(&text, super::epub::dir_of(path));
        }
    }

    /// A sheet's text; `base` is the directory relative URLs resolve against.
    pub fn sheet(&mut self, text: &str, base: &str) {
        let text = strip_comments(text);
        self.block_list(&text, base);
    }

    fn block_list(&mut self, s: &str, base: &str) {
        let mut rest = s;
        loop {
            rest = rest.trim_start();
            if rest.is_empty() {
                return;
            }
            if let Some(at) = rest.strip_prefix('@') {
                let name: String = at
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                    .collect();
                let name = name.to_ascii_lowercase();
                let semi = find_top(rest, ';');
                let brace = find_top(rest, '{');
                match (semi, brace) {
                    (Some(sc), b) if b.is_none_or(|b| sc < b) => {
                        if name == "import" {
                            self.import(&rest[1 + name.len()..sc], base);
                        }
                        rest = &rest[sc + 1..];
                    }
                    (_, Some(b)) => {
                        let end = matching_brace(rest, b);
                        let inner = &rest[b + 1..end.min(rest.len())];
                        let prelude = &rest[1 + name.len()..b];
                        if (name == "media" && media_ok(prelude)) || name == "supports" {
                            self.block_list(inner, base);
                        }
                        rest = &rest[(end + 1).min(rest.len())..];
                    }
                    _ => return,
                }
                continue;
            }
            let Some(b) = find_top(rest, '{') else {
                return;
            };
            let end = matching_brace(rest, b);
            let selector = rest[..b].trim();
            let body = &rest[b + 1..end.min(rest.len())];
            let decls: Arc<[Decl]> = parse_declarations(body).into();
            if !decls.is_empty() {
                self.set.add(selector, decls, self.order);
            }
            self.order += 1;
            rest = &rest[(end + 1).min(rest.len())..];
        }
    }

    fn import(&mut self, prelude: &str, base: &str) {
        let p = prelude.trim();
        let (url, media) = if let Some(r) = p.strip_prefix("url(") {
            let end = r.find(')').unwrap_or(r.len());
            (
                r[..end].trim().trim_matches(['"', '\'']),
                r.get(end + 1..).unwrap_or(""),
            )
        } else if let Some(q) = p.chars().next().filter(|c| *c == '"' || *c == '\'') {
            let r = &p[1..];
            let end = r.find(q).unwrap_or(r.len());
            (&r[..end], r.get(end + 1..).unwrap_or(""))
        } else {
            return;
        };
        if media_ok(media) {
            let path = super::epub::resolve(base, url);
            self.sheet_file(&path);
        }
    }
}

/// The first `c` outside strings and parentheses.
fn find_top(s: &str, c: char) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut depth = 0i32;
    for (i, ch) in s.char_indices() {
        match (quote, ch) {
            (Some(q), ch) if ch == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(ch),
            (None, '(') => depth += 1,
            (None, ')') => depth -= 1,
            (None, ch) if ch == c && depth <= 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// The index of the `}` closing the `{` at `open` (or the end).
fn matching_brace(s: &str, open: usize) -> usize {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for (i, ch) in s[open..].char_indices() {
        match (quote, ch) {
            (Some(q), ch) if ch == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(ch),
            (None, '{') => depth += 1,
            (None, '}') => {
                depth -= 1;
                if depth == 0 {
                    return open + i;
                }
            }
            _ => {}
        }
    }
    s.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specificity_and_keys() {
        assert_eq!(specificity("p"), 1);
        assert_eq!(specificity(".a .b p"), (2 << 8) | 1);
        assert_eq!(specificity("#x .y"), (1 << 16) | (1 << 8));
        assert_eq!(specificity("a:not(.b)"), (1 << 8) | 1);
        assert!(matches!(key(".vrtl .m-start-0"), Key::Class(c) if c == "m-start-0"));
        assert!(matches!(key("div > p.x"), Key::Class(c) if c == "x"));
        assert!(matches!(key("h1"), Key::Tag(t) if t == "h1"));
        assert!(matches!(key("p:not(.a)"), Key::Tag(t) if t == "p"));
        assert!(matches!(key("*"), Key::Any));
    }

    #[test]
    fn sheets() {
        let mut set = RuleSet::default();
        let files = |p: &str| match p {
            "css/a.css" => Some("p { margin: 0 } @media amzn-kf8 { p { color: red } }".to_owned()),
            _ => None,
        };
        let mut parser = Parser {
            set: &mut set,
            order: 0,
            load: &files,
            seen: Vec::new(),
        };
        parser.sheet(
            "@charset \"UTF-8\"; @import url(\"a.css\"); /* x { } */ h1, h2::before { font-weight: bold !important } @font-face { src: url(x) } @media screen { .c { text-indent: 2em } }",
            "css/",
        );
        let names: Vec<String> = set
            .rules
            .iter()
            .map(|r| {
                r.decls
                    .iter()
                    .map(|d| d.name.clone())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect();
        assert_eq!(names, ["margin", "font-weight", "text-indent"]);
        assert!(set.rules[1].decls[0].important);
    }

    #[test]
    fn media() {
        assert!(media_ok("screen"));
        assert!(media_ok("all and (min-width: 300px)"));
        assert!(!media_ok("amzn-kf8"));
        assert!(media_ok("not amzn-mobi"));
        assert!(!media_ok("print"));
    }
}
