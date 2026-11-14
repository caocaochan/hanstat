//! Traditional/Simplified detection and character-level T→S folding,
//! from OpenCC's TSCharacters / STCharacters tables (Apache-2.0).

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

static TS_TXT: &str = include_str!("../data/TSCharacters.txt");
static ST_TXT: &str = include_str!("../data/STCharacters.txt");

pub struct Tables {
    /// Traditional char -> first simplified candidate (always a single char).
    pub t2s: HashMap<char, char>,
    /// Characters only used in Traditional text.
    pub trad_only: HashSet<char>,
    /// Characters only used in Simplified text.
    pub simp_only: HashSet<char>,
}

fn parse(txt: &str) -> impl Iterator<Item = (char, Vec<char>)> + '_ {
    txt.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let (k, v) = l.split_once('\t')?;
            let mut kc = k.chars();
            let key = kc.next()?;
            if kc.next().is_some() {
                return None;
            }
            let vals: Vec<char> = v
                .split(' ')
                .filter_map(|s| {
                    let mut c = s.chars();
                    let f = c.next()?;
                    c.next().is_none().then_some(f)
                })
                .collect();
            Some((key, vals))
        })
}

pub static TABLES: LazyLock<Tables> = LazyLock::new(|| {
    let mut t2s = HashMap::new();
    let mut trad_only = HashSet::new();
    for (k, v) in parse(TS_TXT) {
        if let Some(&first) = v.first()
            && first != k
        {
            t2s.insert(k, first);
            // A char is trad-only if it never maps to itself (e.g. 乾 -> 乾/干 is ambiguous: skip).
            if !v.contains(&k) {
                trad_only.insert(k);
            }
        }
    }
    let mut simp_only = HashSet::new();
    for (k, v) in parse(ST_TXT) {
        if !v.contains(&k) && !t2s.contains_key(&k) {
            simp_only.insert(k);
        }
    }
    Tables {
        t2s,
        trad_only,
        simp_only,
    }
});

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum Variant {
    Simplified,
    Traditional,
    Mixed,
    Unknown,
}

pub fn classify(trad: u64, simp: u64) -> Variant {
    let n = trad + simp;
    if n < 20 {
        return Variant::Unknown;
    }
    let t = trad as f64 / n as f64;
    if t >= 0.8 {
        Variant::Traditional
    } else if t <= 0.2 {
        Variant::Simplified
    } else {
        Variant::Mixed
    }
}

static S2T: LazyLock<HashMap<char, char>> = LazyLock::new(|| {
    parse(ST_TXT)
        .filter_map(|(k, v)| v.first().copied().filter(|&f| f != k).map(|f| (k, f)))
        .collect()
});

static ST_PHRASES_TXT: &str = include_str!("../data/STPhrases.txt");
/// Same-length OpenCC S→T phrase table, and the longest key in chars.
static ST_PHRASES: LazyLock<(HashMap<&'static str, &'static str>, usize)> = LazyLock::new(|| {
    let mut m = HashMap::new();
    let mut max = 0;
    for l in ST_PHRASES_TXT.lines().filter(|l| !l.starts_with('#')) {
        let Some((k, v)) = l.split_once('\t') else {
            continue;
        };
        let v = v.split(' ').next().unwrap_or("");
        let n = k.chars().count();
        if n >= 2 && n == v.chars().count() {
            m.insert(k, v);
            max = max.max(n);
        }
    }
    (m, max)
});

/// Simplified → Traditional with OpenCC phrases (forward maximum matching),
/// falling back to characters. Always length-preserving in chars.
pub fn fold_s2t(s: &str) -> String {
    let (phrases, max) = &*ST_PHRASES;
    let offs: Vec<usize> = s.char_indices().map(|(i, _)| i).chain([s.len()]).collect();
    let n = offs.len() - 1;
    let mut out = String::with_capacity(s.len() + 8);
    let mut i = 0;
    'outer: while i < n {
        for len in (2..=(*max).min(n - i)).rev() {
            if let Some(v) = phrases.get(&s[offs[i]..offs[i + len]]) {
                out.push_str(v);
                i += len;
                continue 'outer;
            }
        }
        let c = s[offs[i]..].chars().next().unwrap();
        out.push(s2t_char(c));
        i += 1;
    }
    out
}

/// Simplified → Traditional, first OpenCC candidate (length-preserving; used
/// only as model input for the Traditional-trained CKIP segmenter).
#[inline]
pub fn s2t_char(c: char) -> char {
    S2T.get(&c).copied().unwrap_or(c)
}

/// Character-wise T→S fold (length-preserving in chars). Used only to feed
/// the segmenters, whose models are trained on Simplified text.
pub fn fold_t2s(s: &str) -> String {
    let t = &*TABLES;
    s.chars().map(|c| *t.t2s.get(&c).unwrap_or(&c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tables() {
        let t = &*TABLES;
        assert!(t.trad_only.contains(&'們'));
        assert!(t.simp_only.contains(&'们'));
        assert!(!t.trad_only.contains(&'的'));
        assert!(!t.simp_only.contains(&'的'));
        assert_eq!(fold_t2s("我們說話"), "我们说话");
        assert_eq!(fold_s2t("以后他的头发很干净"), "以後他的頭髮很乾淨");
        let s = "后来𠮷野家的皇后";
        assert_eq!(fold_s2t(s).chars().count(), s.chars().count());
    }
}
