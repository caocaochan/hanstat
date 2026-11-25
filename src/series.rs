//! Series and volume numbers read from book titles (or, for an EPUB that
//! names its series in its metadata as calibre writes it, from there: [`of`]),
//! so that the reading path keeps a series in order.
//!
//! A title is split into a **key** (the series: the title with its volume
//! marker, bracketed tags, spaces and punctuation taken out, lowercased and
//! folded to Simplified) and a **volume**, a list of numbers compared in
//! order (`第一部（下）` is `[1, 2]`). Two books are in one series when their
//! keys are equal; a book of the series with no marker (often the first,
//! published before the others were numbered) counts as volume 1.
//!
//! Markers are recognised at the end of the title, before a subtitle
//! (`三体2：黑暗森林`, `Title 2 - Subtitle`) or at the start (`[01] Title`,
//! `第一卷 Title`), with or without brackets of any kind (`（第二部）`,
//! `[Vol. 3]`, `【上】`):
//!
//! - numbers: `2`, `2.5`, `II`, `Ⅱ`, `②`, `二`, `十二`, `贰`, `two`, `second`;
//! - Chinese units: `第二部`, `第2卷`, `二册`, `卷三`, `第4集`, `第2季`,
//!   `上`, `中`, `下`, `上册`, `前篇`, `後編`, `续`;
//! - English: `Vol. 2`, `Volume II`, `Book 3`, `Part Two`, `No. 4`, `#5`,
//!   `Season 2`, `Tome 2`;
//! - a prologue (`序章`, `前传`, `Prologue`), before volume 1, and a final
//!   part (`终章`, `Epilogue`), after every volume (`Title 3 Epilogue`:
//!   after 3);
//! - extras (`番外`, `外传`, `特别篇`), which come after every volume.
//!
//! To keep ordinary titles whole, a marker joined to the title without a
//! space or bracket must be unambiguous: digits after letters of another
//! kind (`三体2`, not `1984`), `第` and a number, or a number with a unit.
//! A bare Chinese numeral or `上`/`下` counts only after a space, a dash or
//! inside brackets (`天下` stays one title). Tags that are not markers are
//! dropped from the key (`猫咪的玫瑰[星际]`, `（完结）`, `(全三册)`).

/// The series and volume a title names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    /// The series, for matching titles; empty if nothing is left.
    pub key: String,
    /// Volume numbers, outermost first, in tenths (`2.5` is 25); `None` if
    /// the title has no marker.
    pub number: Option<Vec<u32>>,
}

/// Extras (番外, 外传): after every numbered volume.
pub const EXTRA: u32 = u32::MAX;
/// The last part (完结篇, 终章): after every numbered volume, before extras.
pub const FINAL: u32 = u32::MAX - 1;

/// The volume number an unnumbered book of a series has.
pub const UNNUMBERED: u32 = 10;

pub fn parse(title: &str) -> Volume {
    let t = normalise(title);
    let t = t.trim_matches(is_sep);
    let (base, vols) = parse_norm(t);
    Volume {
        key: key_of(base),
        number: (!vols.is_empty()).then_some(vols),
    }
}

/// A book's series as its EPUB metadata gives it, the way calibre writes
/// and reads it (`calibre/ebooks/metadata/opf3.py`, `read_series`):
/// an EPUB 3 `belongs-to-collection` whose `collection-type` is `series`
/// (its `group-position` the index), else the EPUB 2
/// `calibre:series` / `calibre:series_index` metas.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Meta {
    pub name: String,
    /// 1 when missing or not a number, as calibre reads it.
    pub index: f64,
}

/// The series in a package document's metadata, read as calibre reads it.
pub fn from_opf(doc: &roxmltree::Document) -> Option<Meta> {
    let metas: Vec<roxmltree::Node> = doc
        .descendants()
        .filter(|n| {
            n.tag_name().name() == "meta"
                && n.ancestors().any(|a| a.tag_name().name() == "metadata")
        })
        .collect();
    let text = |n: &roxmltree::Node| n.text().unwrap_or("").trim().to_owned();
    let refines = |id: &str, prop: &str| {
        let want = format!("#{id}");
        metas
            .iter()
            .find(|m| {
                m.attribute("refines") == Some(want.as_str())
                    && m.attribute("property") == Some(prop)
            })
            .map(&text)
    };
    // EPUB 3: the first collection of type `series` with a name.
    for m in &metas {
        if m.attribute("property") != Some("belongs-to-collection") {
            continue;
        }
        let Some(id) = m.attribute("id") else {
            continue;
        };
        let name = text(m);
        if name.is_empty() || refines(id, "collection-type").as_deref() != Some("series") {
            continue;
        }
        let index = refines(id, "group-position")
            .and_then(|p| p.parse::<f64>().ok())
            .filter(|x| x.is_finite())
            .unwrap_or(1.0);
        return Some(Meta { name, index });
    }
    // EPUB 2 (and calibre's older EPUB 3 output).
    let named = |key: &'static str| {
        metas
            .iter()
            .filter(move |m| m.attribute("name") == Some(key))
            .filter_map(|m| m.attribute("content"))
            .map(str::trim)
            .collect::<Vec<_>>()
    };
    let name = named("calibre:series")
        .into_iter()
        .find(|c| !c.is_empty())?
        .to_owned();
    let index = named("calibre:series_index")
        .into_iter()
        .find_map(|c| c.parse::<f64>().ok().filter(|x| x.is_finite()))
        .unwrap_or(1.0);
    Some(Meta { name, index })
}

/// The series in the EPUB file `path`'s metadata (`None`: it names none).
/// Reads only the ZIP directory, `container.xml` and the package document.
pub fn read_epub(path: &std::path::Path) -> Result<Option<Meta>, String> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut z = zip::ZipArchive::new(std::io::BufReader::new(f))
        .map_err(|e| format!("{}: not an EPUB ({e})", path.display()))?;
    let names: Vec<String> = z
        .file_names()
        .filter_map(|n| n.ok().map(|n| n.into_owned()))
        .collect();
    let mut read = |want: &str| -> Option<String> {
        let real = names.iter().find(|n| n.eq_ignore_ascii_case(want))?.clone();
        let mut entry = z.by_name(&real).ok()?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).ok()?;
        Some(crate::extract::epub::decode_markup(&bytes).0)
    };
    let opts = || roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let container = read("META-INF/container.xml").ok_or("no META-INF/container.xml")?;
    let doc = roxmltree::Document::parse_with_options(&container, opts())
        .map_err(|e| format!("container.xml: {e}"))?;
    let opf_path = doc
        .descendants()
        .find(|n| n.tag_name().name() == "rootfile")
        .and_then(|n| n.attribute("full-path"))
        .ok_or("no rootfile in container.xml")?
        .to_owned();
    let opf = read(&opf_path).ok_or_else(|| format!("{opf_path} missing"))?;
    let doc = roxmltree::Document::parse_with_options(&opf, opts())
        .map_err(|e| format!("{opf_path}: {e}"))?;
    Ok(from_opf(&doc))
}

/// The series and volume of a book: from its metadata when it names one
/// (any volume marker in the title is then ignored), else from its title.
/// With metadata a book always counts as numbered.
pub fn of(title: &str, meta: Option<&Meta>) -> Volume {
    match meta {
        Some(m) => Volume {
            key: key_of(&normalise(&m.name)),
            number: Some(vec![meta_number(m.index)]),
        },
        None => parse(title),
    }
}

/// A metadata index in tenths, within the numbered volumes.
fn meta_number(index: f64) -> u32 {
    (index * 10.0).round().clamp(0.0, (FINAL - 1) as f64) as u32
}

/// [`of`], with the series' name to show: the metadata's, or the title
/// without its volume marker.
pub fn named(title: &str, meta: Option<&Meta>) -> (Volume, String) {
    if let Some(m) = meta {
        return (of(title, meta), m.name.clone());
    }
    let n = normalise(title);
    let t = n.trim_matches(is_sep);
    let (base, vols) = parse_norm(t);
    let v = Volume {
        key: key_of(base),
        number: (!vols.is_empty()).then_some(vols),
    };
    // `normalise` maps character to character unless lowercasing changes a
    // length: then the original title's characters can be shown.
    let start = n[..base.as_ptr() as usize - n.as_ptr() as usize]
        .chars()
        .count();
    let len = base.chars().count();
    let shown = if n.chars().count() == title.chars().count() {
        title.chars().skip(start).take(len).collect::<String>()
    } else {
        base.to_owned()
    };
    let shown = shown
        .trim_matches(|c: char| {
            is_sep(c) || c == ':' || opening(c).is_some() || closing(c).is_some()
        })
        .to_owned();
    (v, shown)
}

/// The series of each book: (series number, volume) for the books that
/// share their series with another one, at least one of them numbered.
/// Series are numbered in the order of their first book.
pub fn group(vols: &[Volume]) -> Vec<Option<(u32, Vec<u32>)>> {
    let mut groups: std::collections::HashMap<&str, Vec<usize>> = Default::default();
    for (i, v) in vols.iter().enumerate() {
        if !v.key.is_empty() {
            groups.entry(v.key.as_str()).or_default().push(i);
        }
    }
    let mut groups: Vec<Vec<usize>> = groups
        .into_values()
        .filter(|m| m.len() > 1 && m.iter().any(|&i| vols[i].number.is_some()))
        .collect();
    groups.sort_unstable();
    let mut out = vec![None; vols.len()];
    for (id, members) in groups.into_iter().enumerate() {
        for i in members {
            let n = vols[i].number.clone().unwrap_or_else(|| vec![UNNUMBERED]);
            out[i] = Some((id as u32, n));
        }
    }
    out
}

/// A volume as shown: `2`, `2.5`, `1·3` (part 3 of volume 1), `final`, `extra`.
pub fn volume_label(number: &[u32]) -> String {
    number
        .iter()
        .map(|&n| match n {
            EXTRA => "extra".to_owned(),
            FINAL => "final".to_owned(),
            n if n % 10 == 0 => (n / 10).to_string(),
            n => format!("{}.{}", n / 10, n % 10),
        })
        .collect::<Vec<_>>()
        .join("·")
}

fn parse_norm(t: &str) -> (&str, Vec<u32>) {
    // A tag in front (`[作者]`, `【新译】`) is not part of the series.
    if let Some(rest) = leading_tag(t) {
        let (b, v) = parse_norm(rest);
        if !key_of(b).is_empty() {
            return (b, v);
        }
    }
    // At the end; a volume before a subtitle as well comes first
    // (`Title 3: Subtitle (Part 1)` is 3·1).
    let (base, vols) = peel(t);
    if !vols.is_empty() {
        if let Some((b, mut outer)) = before_subtitle(base) {
            outer.extend(vols);
            return (b, outer);
        }
        return (base, vols);
    }
    if let Some(found) = before_subtitle(t) {
        return found;
    }
    // At the start.
    if let Some((v, rest)) = leading(t) {
        let (b, more) = peel(rest);
        let mut vols = v;
        vols.extend(more);
        return (b, vols);
    }
    (base, vols)
}

/// A marker before a subtitle: at the first separator, or the first place a
/// number is followed by a subtitle without one (`破云2吞海`), after which
/// the title so far ends in a marker. Markers after it up to the next
/// separator other than a space belong to it (`Title 3 Epilogue: Subtitle`
/// is 3·final; `Title 4: Three Cakes` is 4).
fn before_subtitle(t: &str) -> Option<(&str, Vec<u32>)> {
    let mut prev = None;
    let mut found: Option<(&str, Vec<u32>)> = None;
    for (i, c) in t.char_indices() {
        let after_number = prev.is_some_and(numeral) && class(c) == Class::Other && !numeral(c);
        if is_sep(c) || c == ':' || after_number {
            let (b, v) = peel(&t[..i]);
            match &found {
                None if !v.is_empty() && !key_of(b).is_empty() => found = Some((b, v)),
                Some((fb, fv)) if v.len() > fv.len() && key_of(b) == key_of(fb) => {
                    found = Some((b, v))
                }
                _ => {}
            }
            if found.is_some() && !c.is_whitespace() {
                break;
            }
        }
        prev = Some(c);
    }
    found
}

/// A hyphen joining two words (`thirty-six`, `spider-man`): not a separator
/// a volume can follow.
fn in_word_hyphen(before: &str, next: char) -> bool {
    let mut cs = before.chars().rev();
    matches!(cs.next(), Some('-' | '‐' | '–'))
        && cs.next().is_some_and(|c| c.is_ascii_alphabetic())
        && next.is_ascii_alphabetic()
}

/// Full-width ASCII to ASCII, ideographic space to space, Traditional to
/// Simplified, lowercase.
fn normalise(s: &str) -> String {
    let s: String = s
        .chars()
        .map(|c| match c {
            '\u{ff01}'..='\u{ff5e}' => char::from_u32(c as u32 - 0xfee0).unwrap_or(c),
            '\u{3000}' => ' ',
            _ => c,
        })
        .collect();
    crate::anki::fold(&s, true).to_lowercase()
}

/// Digits and the numerals that are one symbol (`2`, `ⅱ`, `②`).
fn numeral(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, 'ⅰ'..='ⅻ' | '①'..='⑳')
}

/// The rest of `t` after a bracketed tag at its start that is not a volume.
fn leading_tag(t: &str) -> Option<&str> {
    let first = t.chars().next()?;
    let close = closing(first)?;
    let end = t.find(close)?;
    let inner = &t[first.len_utf8()..end];
    if marker(inner.trim_matches(is_sep), Ctx::Apart).is_some() {
        return None;
    }
    let rest = t[end + close.len_utf8()..].trim_start_matches(|c| is_sep(c) || c == ':');
    // `【X】(3)`: what follows is the volume, not the title.
    let bare = rest.trim_matches(|c| is_sep(c) || opening(c).is_some() || closing(c).is_some());
    (!bare.is_empty() && marker(bare, Ctx::Apart).is_none()).then_some(rest)
}

/// The series key: letters and digits only.
fn key_of(base: &str) -> String {
    base.chars().filter(|c| c.is_alphanumeric()).collect()
}

fn is_sep(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '-' | '_'
                | '.'
                | ','
                | ';'
                | '·'
                | '・'
                | '—'
                | '–'
                | '―'
                | '~'
                | '〜'
                | '、'
                | '。'
                | '/'
                | '|'
                | '!'
                | '?'
                | '…'
                | '︰'
        )
}

const BRACKETS: &[(char, char)] = &[
    ('(', ')'),
    ('[', ']'),
    ('{', '}'),
    ('<', '>'),
    ('【', '】'),
    ('〔', '〕'),
    ('〈', '〉'),
    ('《', '》'),
    ('「', '」'),
    ('『', '』'),
    ('〖', '〗'),
    ('｟', '｠'),
    ('«', '»'),
];

fn closing(open: char) -> Option<char> {
    BRACKETS.iter().find(|b| b.0 == open).map(|b| b.1)
}

fn opening(close: char) -> Option<char> {
    BRACKETS.iter().find(|b| b.1 == close).map(|b| b.0)
}

/// Where a marker stands.
#[derive(Clone, Copy, PartialEq)]
enum Ctx {
    /// Joined to the title: `三体2`, `集第二部`.
    Glued,
    /// After a space or dash, or in brackets.
    Apart,
}

/// Markers and tags taken off the end of `s`: what is left, and the
/// volume numbers, outermost first.
fn peel(mut s: &str) -> (&str, Vec<u32>) {
    let mut found: Vec<Vec<u32>> = Vec::new();
    for _ in 0..8 {
        let t = s.trim_end_matches(|c| is_sep(c) || c == ':');
        if t.is_empty() {
            break;
        }
        let last = t.chars().next_back().unwrap();
        if let Some(open) = opening(last) {
            // The matching opening bracket.
            let mut depth = 0;
            let mut start = None;
            for (i, c) in t.char_indices().rev() {
                if c == last {
                    depth += 1;
                } else if c == open {
                    depth -= 1;
                    if depth == 0 {
                        start = Some(i);
                        break;
                    }
                }
            }
            let Some(start) = start else { break };
            let inner = &t[start + open.len_utf8()..t.len() - last.len_utf8()];
            let before = &t[..start];
            if key_of(before).is_empty() {
                // The whole title is in brackets: 《三体》.
                if found.is_empty() {
                    s = inner;
                    continue;
                }
                break;
            }
            if let Some(v) = marker(inner.trim_matches(|c| is_sep(c) || c == ':'), Ctx::Apart) {
                found.push(v);
            }
            // Not a marker: a tag ([星际], (完结)), left out.
            s = before;
            continue;
        }
        if found.len() < 3
            && let Some((v, rest)) = trailing(t)
        {
            found.push(v);
            s = rest;
            continue;
        }
        s = t;
        break;
    }
    found.reverse();
    (s, found.into_iter().flatten().collect())
}

#[derive(Clone, Copy, PartialEq)]
enum Class {
    Digit,
    Latin,
    Sep,
    Other,
}

fn class(c: char) -> Class {
    if c.is_ascii_digit() {
        Class::Digit
    } else if c.is_ascii_alphabetic() {
        Class::Latin
    } else if is_sep(c) || c == ':' {
        Class::Sep
    } else {
        Class::Other
    }
}

/// The longest marker at the end of `t` (no brackets) that leaves a title.
fn trailing(t: &str) -> Option<(Vec<u32>, &str)> {
    let starts: Vec<usize> = t.char_indices().map(|(i, _)| i).collect();
    // Markers are short: `volume twelve` is the longest kind.
    let from = starts.len().saturating_sub(16);
    for &i in &starts[from.max(1)..] {
        let (before, suffix) = t.split_at(i);
        let a = before.chars().next_back().unwrap();
        let b = suffix.chars().next().unwrap();
        if in_word_hyphen(before, b) {
            continue;
        }
        let ctx = match (class(a), class(b)) {
            (Class::Sep, _) | (_, Class::Sep) => Ctx::Apart,
            (Class::Digit, Class::Digit) | (Class::Latin, Class::Latin) => continue,
            _ => Ctx::Glued,
        };
        if key_of(before).is_empty() {
            continue;
        }
        if let Some(v) = marker(suffix, ctx) {
            return Some((v, before));
        }
    }
    None
}

/// A marker at the start: in brackets, or one word followed by a separator.
fn leading(t: &str) -> Option<(Vec<u32>, &str)> {
    let first = t.chars().next()?;
    if let Some(close) = closing(first) {
        let end = t.find(close)?;
        let inner = &t[first.len_utf8()..end];
        let rest = &t[end + close.len_utf8()..];
        let v = marker(inner.trim_matches(is_sep), Ctx::Apart)?;
        let rest = rest.trim_start_matches(|c| is_sep(c) || c == ':');
        return (!key_of(rest).is_empty()).then_some((v, rest));
    }
    let end = t.find(|c: char| is_sep(c) || c == ':')?;
    // `01 Title`, `第一卷 Title`, but not `三 Title` or `X Title`.
    let word = &t[..end];
    if !(word.starts_with(|c: char| c.is_ascii_digit()) || word.starts_with('第')) {
        return None;
    }
    let v = marker(word, Ctx::Glued)?;
    let rest = t[end..].trim_start_matches(|c| is_sep(c) || c == ':');
    (!key_of(rest).is_empty()).then_some((v, rest))
}

/// Units after a number (`二部`, `3卷`) or after 第.
const UNITS: &[&str] = &[
    "部", "卷", "册", "冊", "集", "季", "篇", "编", "編", "章", "辑", "本", "回", "弹", "话", "期",
    "幕",
];
/// Units before a number: `卷二`, `vol. 2`.
const PREFIXES: &[&str] = &[
    "卷", "册", "volume", "vol", "v", "book", "bk", "part", "pt", "no", "number", "#", "tome",
    "band", "season", "episode", "ep", "chapter", "issue", "série", "serie", "libro",
];

/// The volume a whole marker names.
fn marker(s: &str, ctx: Ctx) -> Option<Vec<u32>> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // A number alone.
    if let Some(n) = number(s, ctx) {
        return Some(vec![n]);
    }
    // 第N, 第N部.
    if let Some(rest) = s.strip_prefix('第') {
        let rest = rest.trim();
        let rest = UNITS
            .iter()
            .find_map(|u| rest.strip_suffix(u))
            .unwrap_or(rest)
            .trim();
        if let Some(n) = number(rest, Ctx::Apart) {
            return Some(vec![n]);
        }
    }
    // N部, 3卷.
    for u in UNITS {
        if let Some(rest) = s.strip_suffix(u)
            && let Some(n) = number(rest.trim(), Ctx::Apart)
        {
            return Some(vec![n]);
        }
    }
    // 卷二, vol. 2, #5.
    for p in PREFIXES {
        if let Some(rest) = s.strip_prefix(p) {
            let latin = p.chars().all(|c| c.is_ascii_alphabetic());
            // `vol2` and `vol 2` but not `volcano`.
            if latin && rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
                continue;
            }
            let rest = rest.trim_start_matches(['.', ' ', ':', '#']).trim();
            if let Some(n) = number(rest, Ctx::Apart) {
                return Some(vec![n]);
            }
        }
    }
    // 上, 下册, 前篇.
    let (head, unit) = match UNITS
        .iter()
        .find_map(|u| s.strip_suffix(u).map(|h| (h, true)))
    {
        Some((h, true)) if !h.is_empty() => (h.trim(), true),
        _ => (s, false),
    };
    if unit || ctx == Ctx::Apart {
        let n = match head {
            "上" | "前" | "前半" => Some(10),
            "中" => Some(20),
            "下" | "后" | "後" | "后半" | "後半" => Some(30),
            _ => None,
        };
        if let Some(n) = n {
            return Some(vec![n]);
        }
    }
    if ctx == Ctx::Apart {
        match s {
            "续" | "续集" | "续篇" | "sequel" => {
                return Some(vec![20]);
            }
            "序章" | "序幕" | "楔子" | "前传" | "prologue" | "prequel" => {
                return Some(vec![0]);
            }
            "完结篇" | "终章" | "最终章" | "最终卷" | "最终回" | "尾声" | "finale" | "final"
            | "epilogue" => {
                return Some(vec![FINAL]);
            }
            "番外" | "番外篇" | "番外集" | "外传" | "特别篇" | "extra" | "extras"
            | "side story" | "side stories" => return Some(vec![EXTRA]),
            _ => {}
        }
    }
    None
}

/// A number in tenths. Glued to a title, only digits and symbols count
/// (`三体2`, `三体Ⅲ`, `三体III`), not Chinese numerals or words.
fn number(s: &str, ctx: Ctx) -> Option<u32> {
    if s.is_empty() {
        return None;
    }
    // 2, 02, 2.5
    if s.chars().all(|c| c.is_ascii_digit() || c == '.') {
        let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
        if whole.is_empty()
            || whole.len() > 4
            || frac.len() > 1
            || (s.contains('.') && frac.is_empty())
        {
            return None;
        }
        let w: u32 = whole.parse().ok()?;
        let f: u32 = if frac.is_empty() {
            0
        } else {
            frac.parse().ok()?
        };
        return Some(w * 10 + f);
    }
    let mut cs = s.chars();
    if let (Some(c), None) = (cs.next(), cs.next()) {
        let n = match c {
            'Ⅰ'..='Ⅻ' => Some(c as u32 - 'Ⅰ' as u32 + 1),
            'ⅰ'..='ⅻ' => Some(c as u32 - 'ⅰ' as u32 + 1),
            '①'..='⑳' => Some(c as u32 - '①' as u32 + 1),
            '⑴'..='⒇' => Some(c as u32 - '⑴' as u32 + 1),
            '❶'..='❿' => Some(c as u32 - '❶' as u32 + 1),
            '➀'..='➉' => Some(c as u32 - '➀' as u32 + 1),
            _ => None,
        };
        if let Some(n) = n {
            return Some(n * 10);
        }
    }
    if let Some(n) = roman(s) {
        return Some(n * 10);
    }
    if ctx == Ctx::Glued {
        return None;
    }
    if let Some(n) = chinese(s) {
        return Some(n * 10);
    }
    const WORDS: &[&str] = &[
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
    ];
    const ORDINALS: &[&str] = &[
        "first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth", "ninth",
        "tenth", "eleventh", "twelfth",
    ];
    WORDS.iter().chain(ORDINALS).position(|w| *w == s).map(|i| {
        let i = if i >= WORDS.len() { i - WORDS.len() } else { i };
        (i as u32 + 1) * 10
    })
}

/// i to xxxix (I, V and X only: `mix` and `civil` are words).
fn roman(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 7 || !s.chars().all(|c| matches!(c, 'i' | 'v' | 'x')) {
        return None;
    }
    let val = |c| match c {
        'i' => 1,
        'v' => 5,
        _ => 10,
    };
    let cs: Vec<u32> = s.chars().map(val).collect();
    let mut n = 0i32;
    for (k, &v) in cs.iter().enumerate() {
        if cs.get(k + 1).is_some_and(|&next| next > v) {
            n -= v as i32;
        } else {
            n += v as i32;
        }
    }
    let n = u32::try_from(n).ok().filter(|&n| n > 0)?;
    // Only the canonical spelling (`iv`, not `iiii` or `vx`).
    (to_roman(n) == s).then_some(n)
}

fn to_roman(mut n: u32) -> String {
    let mut s = String::new();
    for (v, r) in [(10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")] {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

/// Chinese numerals up to 9999: 二, 十二, 二十, 一百零五, 两, 贰拾.
fn chinese(s: &str) -> Option<u32> {
    let digit = |c| match c {
        '〇' | '零' => Some(0),
        '一' | '壹' => Some(1),
        '二' | '两' | '贰' | '弐' => Some(2),
        '三' | '叁' | '参' => Some(3),
        '四' | '肆' => Some(4),
        '五' | '伍' => Some(5),
        '六' | '陆' => Some(6),
        '七' | '柒' => Some(7),
        '八' | '捌' => Some(8),
        '九' | '玖' => Some(9),
        _ => None,
    };
    let unit = |c| match c {
        '十' | '拾' => Some(10),
        '百' | '佰' => Some(100),
        '千' | '仟' => Some(1000),
        _ => None,
    };
    let mut total = 0u32;
    let mut cur: Option<u32> = None;
    let mut any = false;
    for c in s.chars() {
        if let Some(d) = digit(c) {
            // 二〇二四: digit by digit.
            cur = Some(cur.map_or(d, |x| {
                if total == 0 && x < 1000 {
                    x * 10 + d
                } else {
                    d
                }
            }));
        } else {
            total += cur.unwrap_or(1) * unit(c)?;
            cur = None;
        }
        any = true;
    }
    if !any {
        return None;
    }
    let n = total + cur.unwrap_or(0);
    (n > 0).then_some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vol(t: &str) -> (String, Option<Vec<u32>>) {
        let v = parse(t);
        (
            v.key,
            v.number.map(|n| n.into_iter().map(|x| x / 10).collect()),
        )
    }

    fn same_series(titles: &[&str], expect: &[u32]) {
        let key = parse(titles[0]).key;
        for (t, e) in titles.iter().zip(expect) {
            let v = parse(t);
            assert_eq!(v.key, key, "{t}: key");
            assert_eq!(
                v.number.unwrap_or(vec![UNNUMBERED])[0] / 10,
                *e,
                "{t}: volume"
            );
        }
    }

    #[test]
    fn chinese_titles() {
        same_series(
            &[
                "SCI谜案集（第一部）",
                "SCI谜案集（第二部）",
                "SCI谜案集（第三部）",
                "SCI谜案集 （第四部）",
                "SCI谜案集(第5部)",
                "SCI谜案集 第六部",
                "SCI谜案集第七部",
                "SCI谜案集【第八部】",
                "SCI謎案集（第九部）",
                "SCI谜案集（第十部）[完结]",
            ],
            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        );
        same_series(
            &["三体", "三体2：黑暗森林", "三体III：死神永生"],
            &[1, 2, 3],
        );
        same_series(&["三体Ⅰ", "三体Ⅱ 黑暗森林"], &[1, 2]);
        same_series(&["鬼吹灯", "鬼吹灯 二", "鬼吹灯 卷三"], &[1, 2, 3]);
        same_series(&["X（上）", "X（中）", "X（下）"], &[1, 2, 3]);
        same_series(&["X上册", "X 下"], &[1, 3]);
        same_series(
            &["X 第十二卷", "X 第二十一卷", "X 第一百零五卷"],
            &[12, 21, 105],
        );
        same_series(&["X 第2季", "X 第三集", "X 贰"], &[2, 3, 2]);
        same_series(&["X", "X 番外"], &[1, EXTRA / 10]);
        same_series(
            &["《X》第二部", "【X】(3)", "[作者]X 第四部", "【新译】 X 5"],
            &[2, 3, 4, 5],
        );
        same_series(&["破云", "破云2吞海", "破云3：标题"], &[1, 2, 3]);
        same_series(
            &["Re︰從零開始的異世界生活01", "Re:从零开始的异世界生活 02"],
            &[1, 2],
        );
    }

    /// DuChinese course titles (catalogue read 2026-11-25).
    #[test]
    fn duchinese_titles() {
        let in_order = |titles: &[&str]| {
            let vs: Vec<Volume> = titles.iter().map(|t| parse(t)).collect();
            for (t, v) in titles.iter().zip(&vs) {
                assert_eq!(v.key, vs[0].key, "{t}: key");
                assert!(v.number.is_some(), "{t}: no volume");
            }
            for (w, t) in vs.windows(2).zip(&titles[1..]) {
                assert!(w[0].number < w[1].number, "{t}: out of order");
            }
        };
        in_order(&[
            "Thirty-Six Stratagems 1: The Disappearing Casino",
            "Thirty-Six Stratagems 2: He Is My Person",
            "Thirty-Six Stratagems 3: Heard Through the Grapevine",
            "Thirty-Six Stratagems 4: I Am Brother Hai",
            "Thirty-Six Stratagems 5: Murder Case",
            "Thirty-Six Stratagems 6: Don’t Mess With My People",
        ]);
        in_order(&[
            "Journey to the West 1: Birth of the Stone Monkey",
            "Journey to the West 2: Great Sage Equal to Heaven",
            "Journey to the West 3: Havoc in Heaven (Part 1)",
        ]);
        in_order(&[
            "Three Kingdoms Prologue: Oath of the Peach Garden",
            "Three Kingdoms 1: The Beauty Trap",
            "Three Kingdoms 2: Battle of Xuzhou",
            "Three Kingdoms 3: Heroes",
            "Three Kingdoms 3 Epilogue: Friends",
            "Three Kingdoms 4: Master of Jiangdong",
            "Three Kingdoms 5: Thatched Cottage",
            "Three Kingdoms 6: Sun–Liu Alliance",
            "Three Kingdoms 7: The Battle of Red Cliffs  (Part One)",
            "Three Kingdoms 7: The Battle of Red Cliffs (Part Two)",
            "Three Kingdoms 8: Reunification of the Three Kingdoms Under Jin",
        ]);
        in_order(&["I’m a Cat 1: What is Home?", "I’m a Cat 6: We Can Try"]);
        // A number word in the subtitle is not a part.
        for (t, n) in [
            ("I’m a Cat 4: Three Cakes", 4),
            ("Newbie Course 4: Three People Walking Together", 4),
            ("Thirty-Six Stratagems 4: I Am Brother Hai", 4),
            ("Dune 7 - Two Towers Again", 7),
        ] {
            assert_eq!(vol(t).1, Some(vec![n]), "{t}");
        }
        // A hyphen inside a word does not start a volume.
        assert_eq!(vol("The House at Number Thirty-Three").1, None);
        assert_eq!(vol("Spider-Man Two").0, "spiderman");
    }

    #[test]
    fn nested_volumes() {
        assert_eq!(vol("X 第一部（下）").1, Some(vec![1, 3]));
        assert_eq!(vol("X 第二部 上").1, Some(vec![2, 1]));
    }

    #[test]
    fn english() {
        same_series(
            &[
                "Dune, Book 1",
                "Dune Book Two",
                "Dune - Vol. 3",
                "Dune Volume IV",
                "Dune (Part 5)",
                "Dune #6",
                "Dune 7: Subtitle",
                "[08] Dune",
                "09 - Dune",
            ],
            &[1, 2, 3, 4, 5, 6, 7, 8, 9],
        );
        assert_eq!(vol("Dune 2.5").1, Some(vec![2]));
        assert_eq!(parse("Dune 2.5").number, Some(vec![25]));
    }

    fn opf(meta: &str) -> Option<Meta> {
        let xml = format!(
            r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>X</dc:title>{meta}</metadata></package>"#
        );
        from_opf(&roxmltree::Document::parse(&xml).unwrap())
    }

    fn m(name: &str, index: f64) -> Option<Meta> {
        Some(Meta {
            name: name.into(),
            index,
        })
    }

    #[test]
    fn metadata_epub3_collection() {
        let e3 = r##"<meta property="belongs-to-collection" id="c01">三体</meta>
            <meta refines="#c01" property="collection-type">series</meta>
            <meta refines="#c01" property="group-position">2</meta>"##;
        assert_eq!(opf(e3), m("三体", 2.0));
    }

    #[test]
    fn metadata_epub2_calibre() {
        let e2 = r#"<meta name="calibre:series" content="Dune"/><meta name="calibre:series_index" content="2.5"/>"#;
        assert_eq!(opf(e2), m("Dune", 2.5));
        // The index after the series (PocketBook order) and missing.
        assert_eq!(
            opf(
                r#"<meta name="calibre:series_index" content="3"/><meta name="calibre:series" content="D"/>"#
            ),
            m("D", 3.0)
        );
        assert_eq!(
            opf(r#"<meta name="calibre:series" content="D"/>"#),
            m("D", 1.0)
        );
        assert_eq!(
            opf(
                r#"<meta name="calibre:series" content="D"/><meta name="calibre:series_index" content="x"/>"#
            ),
            m("D", 1.0)
        );
        assert_eq!(opf(r#"<meta name="calibre:series" content=" "/>"#), None);
        assert_eq!(opf(""), None);
    }

    #[test]
    fn metadata_epub3_wins_and_other_collections_are_not_series() {
        let both = r##"<meta name="calibre:series" content="Old"/>
            <meta property="belongs-to-collection" id="s">New</meta>
            <meta refines="#s" property="collection-type">series</meta>"##;
        assert_eq!(opf(both), m("New", 1.0));
        let set = r##"<meta property="belongs-to-collection" id="s">Box</meta>
            <meta refines="#s" property="collection-type">set</meta>
            <meta name="calibre:series" content="Real"/>"##;
        assert_eq!(opf(set), m("Real", 1.0));
        let untyped = r##"<meta property="belongs-to-collection" id="s">Box</meta>"##;
        assert_eq!(opf(untyped), None);
    }

    #[test]
    fn metadata_wins_over_the_title_and_joins_title_series() {
        let meta = Meta {
            name: "三體".into(),
            index: 2.5,
        };
        let v = of("黑暗森林 第九部", Some(&meta));
        assert_eq!(v.number, Some(vec![25]));
        assert_eq!(v.key, parse("三体2：黑暗森林").key);
        assert_eq!(
            of(
                "x",
                Some(&Meta {
                    name: "S".into(),
                    index: 0.0
                })
            )
            .number,
            Some(vec![0])
        );
        assert_eq!(
            of(
                "x",
                Some(&Meta {
                    name: "S".into(),
                    index: -4.0
                })
            )
            .number,
            Some(vec![0])
        );
        // Two books in a metadata series group even with plain titles.
        let s = Meta {
            name: "Saga".into(),
            index: 1.0,
        };
        let t = Meta {
            name: "Saga".into(),
            index: 2.0,
        };
        let g = group(&[
            of("Alpha", Some(&s)),
            of("Beta", Some(&t)),
            of("Gamma", None),
        ]);
        assert_eq!(g[0], Some((0, vec![10])));
        assert_eq!(g[1], Some((0, vec![20])));
        assert_eq!(g[2], None);
    }

    #[test]
    fn names_shown() {
        assert_eq!(named("三体2：黑暗森林", None).1, "三体");
        assert_eq!(named("Dune, Book 1", None).1, "Dune");
        assert_eq!(named("SCI謎案集（第九部）", None).1, "SCI謎案集");
        let meta = Meta {
            name: "My Saga".into(),
            index: 1.0,
        };
        assert_eq!(named("whatever", Some(&meta)).1, "My Saga");
        assert_eq!(volume_label(&[20]), "2");
        assert_eq!(volume_label(&[25]), "2.5");
        assert_eq!(volume_label(&[10, 30]), "1·3");
        assert_eq!(volume_label(&[EXTRA]), "extra");
    }

    /// Every EPUB under `HANSTAT_TEST_EPUBS` (a folder, searched
    /// recursively) whose package document has series tags reads a series;
    /// skipped without the folder.
    #[test]
    fn real_epubs_with_series_tags() {
        let Some(dir) = std::env::var_os("HANSTAT_TEST_EPUBS") else {
            return;
        };
        fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("epub"))
                {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(std::path::Path::new(&dir), &mut files);
        let mut tagged = 0;
        for p in files {
            let Ok(Some(m)) = read_epub(&p) else { continue };
            tagged += 1;
            assert!(!m.name.is_empty() && m.index >= 0.0, "{}", p.display());
            eprintln!("{} → {} #{}", p.display(), m.name, m.index);
        }
        eprintln!("{tagged} with a series");
    }

    #[test]
    fn ordinary_titles_stay_whole() {
        for t in [
            "天下",
            "实用主义者的爱情",
            "美人剑",
            "1984",
            "一二三",
            "空船",
            "死亡万花筒",
            "全球高考",
            "volcano",
            "The Civil War",
            "Mix",
        ] {
            assert_eq!(vol(t).1, None, "{t}");
        }
        // Tags are not volumes and are left out of the key.
        assert_eq!(vol("猫咪的玫瑰[星际]"), ("猫咪的玫瑰".into(), None));
        assert_eq!(vol("X（全三册）"), ("x".into(), None));
        assert_eq!(vol("X (完结+番外)").0, "x");
    }
}
