//! Series and volume numbers read from book titles, so that the reading path
//! keeps a series in order.
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
//! - Chinese and Japanese units: `第二部`, `第2卷`, `二册`, `卷三`, `3巻`,
//!   `第4集`, `第2季`, `上`, `中`, `下`, `上册`, `下巻`, `前篇`, `後編`, `续`;
//! - Korean: `2권`, `제3부`, `상`, `하`, `상권`;
//! - English: `Vol. 2`, `Volume II`, `Book 3`, `Part Two`, `No. 4`, `#5`,
//!   `Season 2`, `Tome 2`;
//! - extras (`番外`, `外传`, `外伝`, `特别篇`), which come after every volume.
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

fn parse_norm(t: &str) -> (&str, Vec<u32>) {
    // A tag in front (`[作者]`, `【新译】`) is not part of the series.
    if let Some(rest) = leading_tag(t) {
        let (b, v) = parse_norm(rest);
        if !key_of(b).is_empty() {
            return (b, v);
        }
    }
    // At the end.
    let (base, vols) = peel(t);
    if !vols.is_empty() {
        return (base, vols);
    }
    // Before a subtitle: the first separator, or the first place a number
    // is followed by a subtitle without one (`破云2吞海`), after which the
    // title so far ends in a marker.
    let mut prev = None;
    for (i, c) in t.char_indices() {
        let after_number = prev.is_some_and(numeral) && class(c) == Class::Other && !numeral(c);
        if is_sep(c) || c == ':' || after_number {
            let (b, v) = peel(&t[..i]);
            if !v.is_empty() && !key_of(b).is_empty() {
                return (b, v);
            }
        }
        prev = Some(c);
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
    if !(word.starts_with(|c: char| c.is_ascii_digit()) || word.starts_with(['第', '제'])) {
        return None;
    }
    let v = marker(word, Ctx::Glued)?;
    let rest = t[end..].trim_start_matches(|c| is_sep(c) || c == ':');
    (!key_of(rest).is_empty()).then_some((v, rest))
}

/// Units after a number (`二部`, `3巻`, `2권`) or after 第/제.
const UNITS: &[&str] = &[
    "部", "卷", "巻", "册", "冊", "集", "季", "篇", "编", "編", "章", "辑", "本", "回", "弹", "弾",
    "话", "期", "幕", "권", "부", "편", "화", "장", "기", "시즌",
];
/// Units before a number: `卷二`, `vol. 2`.
const PREFIXES: &[&str] = &[
    "卷", "巻", "册", "volume", "vol", "v", "book", "bk", "part", "pt", "no", "number", "#",
    "tome", "band", "season", "episode", "ep", "chapter", "issue", "série", "serie", "libro",
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
    // 第N, 第N部, 제N권.
    for lead in ["第", "제"] {
        if let Some(rest) = s.strip_prefix(lead) {
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
    }
    // N部, 3巻, 2권.
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
    // 上, 下册, 上巻, 상권, 前篇.
    let (head, unit) = match UNITS
        .iter()
        .find_map(|u| s.strip_suffix(u).map(|h| (h, true)))
    {
        Some((h, true)) if !h.is_empty() => (h.trim(), true),
        _ => (s, false),
    };
    if unit || ctx == Ctx::Apart {
        let n = match head {
            "上" | "상" | "前" | "前半" => Some(10),
            "中" | "중" => Some(20),
            "下" | "하" | "后" | "後" | "后半" | "後半" => Some(30),
            _ => None,
        };
        if let Some(n) = n {
            return Some(vec![n]);
        }
    }
    if ctx == Ctx::Apart {
        match s {
            "续" | "続" | "续集" | "続編" | "续篇" | "후속" | "sequel" => {
                return Some(vec![20]);
            }
            "完结篇" | "终章" | "最终章" | "最终卷" | "最终巻" | "最终回" | "finale" | "final" =>
            {
                return Some(vec![FINAL]);
            }
            "番外" | "番外篇" | "番外集" | "外传" | "外伝" | "特别篇" | "특별편" | "외전"
            | "extra" | "extras" | "side story" | "side stories" => return Some(vec![EXTRA]),
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

    #[test]
    fn nested_volumes() {
        assert_eq!(vol("X 第一部（下）").1, Some(vec![1, 3]));
        assert_eq!(vol("X 第二部 上").1, Some(vec![2, 1]));
    }

    #[test]
    fn japanese_and_korean() {
        same_series(
            &[
                "とある魔術の禁書目録 22",
                "とある魔術の禁書目録(3)",
                "とある魔術の禁書目録 第4巻",
            ],
            &[22, 3, 4],
        );
        same_series(&["本好きの下剋上 第二部「神殿の巫女見習いI」"], &[2]);
        same_series(&["X 上巻", "X 下巻", "X ②"], &[1, 3, 2]);
        same_series(
            &[
                "전지적 독자 시점 1권",
                "전지적 독자 시점 제2권",
                "전지적 독자 시점 3",
            ],
            &[1, 2, 3],
        );
        same_series(&["X 상", "X 하권"], &[1, 3]);
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
