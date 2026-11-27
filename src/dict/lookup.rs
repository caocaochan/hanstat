//! Looking up the text at the pointer in every enabled dictionary.
//!
//! The scan tries the text's prefixes longest first (up to the longest
//! headword of any dictionary, at most [`MAX_SCAN`] characters), as written
//! and, when it differs, with Traditional characters folded to Simplified
//! (most dictionaries list both forms; some only one). Each prefix is checked
//! against a dictionary's Bloom filter before its files are touched, so the
//! many prefixes that are no word cost almost nothing.
//!
//! Results are grouped by (headword, reading), as Yomitan does: longest match
//! first, then by the chosen frequency list's rank, then dictionary order.

use super::format::Manifest;
use super::store::{Freq, Store, TermRef};
use super::xxh64;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

const MAX_SCAN: usize = 16;
/// Groups kept for one lookup.
const MAX_GROUPS: usize = 32;
const CACHE: usize = 64;

/// One dictionary's glossaries for a group.
#[derive(Clone, Debug)]
pub struct Part {
    /// Index into [`Engine::dicts`].
    pub dict: usize,
    pub glossaries: Vec<Gloss>,
}

#[derive(Clone, Debug)]
pub struct Gloss {
    /// The definitions, as the dictionary's JSON array (or an error message).
    pub json: Result<String, String>,
    pub def_tags: String,
    pub term_tags: String,
}

/// A frequency list's ranks for a group's headword.
#[derive(Clone, Debug)]
pub struct FreqBadge {
    pub dict: usize,
    pub values: Vec<Freq>,
    /// Found under the Simplified form of a Traditional headword.
    pub folded: bool,
}

#[derive(Clone, Debug)]
pub struct Group {
    pub headword: String,
    pub reading: String,
    /// Characters of the looked-up text it covers.
    pub matched: usize,
    /// Matched only once Traditional characters were folded to Simplified.
    pub folded: bool,
    pub freqs: Vec<FreqBadge>,
    pub parts: Vec<Part>,
}

#[derive(Clone, Debug, Default)]
pub struct Lookup {
    pub text: String,
    pub groups: Vec<Group>,
}

impl Lookup {
    /// Characters of the text the best match covers (0: nothing found).
    pub fn matched(&self) -> usize {
        self.groups.first().map_or(0, |g| g.matched)
    }
}

/// A reading as grouping compares it: no spaces or apostrophes, lower case.
fn reading_key(r: &str) -> String {
    r.chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '\'' | '’' | '·' | '-'))
        .flat_map(char::to_lowercase)
        .collect()
}

/// A loaded dictionary, as the popup shows it.
#[derive(Clone, Debug)]
pub struct DictInfo {
    pub id: String,
    pub title: String,
    pub styles: String,
    pub has_freq: bool,
    pub dir: std::path::PathBuf,
    /// Its entries start collapsed in the popup (the user's choice).
    pub collapsed: bool,
}

pub struct Engine {
    stores: Vec<Store>,
    pub dicts: Vec<DictInfo>,
    /// Sort readings and headwords of equal length by this frequency list
    /// (index into `stores`).
    freq_sort: Option<usize>,
    longest: usize,
    cache: Vec<(String, Arc<Lookup>)>,
}

impl Engine {
    pub fn empty() -> Engine {
        Engine {
            stores: Vec::new(),
            dicts: Vec::new(),
            freq_sort: None,
            longest: 0,
            cache: Vec::new(),
        }
    }

    /// The enabled dictionaries of `root`, in the manifest's order. Ones that
    /// can't be opened are skipped and reported.
    pub fn load(root: &Path, freq_sort: Option<&str>) -> (Engine, Vec<String>) {
        let manifest = Manifest::load(root);
        let mut e = Engine::empty();
        let mut errors = Vec::new();
        for entry in manifest.dicts.iter().filter(|d| d.enabled) {
            match Store::open(&root.join(&entry.id)) {
                Ok(s) => {
                    e.dicts.push(DictInfo {
                        id: entry.id.clone(),
                        title: s.meta.title.clone(),
                        styles: s.meta.styles.clone(),
                        has_freq: s.meta.has_freq(),
                        dir: s.dir.clone(),
                        collapsed: entry.collapsed,
                    });
                    e.stores.push(s);
                }
                Err(err) => errors.push(err),
            }
        }
        e.longest = e
            .stores
            .iter()
            .filter(|s| s.meta.has_terms())
            .map(|s| s.meta.max_len as usize)
            .max()
            .unwrap_or(0)
            .min(MAX_SCAN);
        e.freq_sort =
            freq_sort.and_then(|id| e.dicts.iter().position(|d| d.id == id && d.has_freq));
        (e, errors)
    }

    /// Drop the dictionaries `ids` (their files are then free to delete).
    pub fn unload(&mut self, ids: &[String]) {
        let keep: Vec<bool> = self.dicts.iter().map(|d| !ids.contains(&d.id)).collect();
        let mut k = keep.iter();
        self.stores.retain(|_| *k.next().unwrap());
        let mut k = keep.iter();
        self.dicts.retain(|_| *k.next().unwrap());
        self.freq_sort = None;
        self.cache.clear();
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.stores.is_empty()
    }

    /// How many characters of text a lookup reads.
    pub fn scan_len(&self) -> usize {
        self.longest
    }

    pub fn lookup(&mut self, text: &str) -> Arc<Lookup> {
        let chars: Vec<char> = text.chars().take(self.longest.max(1)).collect();
        let text: String = chars.iter().collect();
        if let Some(i) = self.cache.iter().position(|(t, _)| *t == text) {
            let hit = self.cache.remove(i);
            let r = hit.1.clone();
            self.cache.push(hit);
            return r;
        }
        let r = Arc::new(self.lookup_uncached(&chars));
        if self.cache.len() >= CACHE {
            self.cache.remove(0);
        }
        self.cache.push((text, r.clone()));
        r
    }

    fn lookup_uncached(&mut self, chars: &[char]) -> Lookup {
        let text: String = chars.iter().collect();
        let folded: Vec<char> = crate::script::fold_t2s(&text).chars().collect();
        let mut variants: Vec<(&[char], bool)> = vec![(chars, false)];
        if folded.as_slice() != chars {
            variants.push((&folded, true));
        }
        // (store, term) → (matched length, via folding), first found kept:
        // longest prefixes come first, and the text as written before folded.
        let mut found: Vec<(usize, TermRef, usize, bool)> = Vec::new();
        let mut seen: HashMap<(usize, u64), ()> = HashMap::new();
        let mut buf = Vec::new();
        let mut q = String::new();
        for len in (1..=chars.len()).rev() {
            for &(v, is_fold) in &variants {
                q.clear();
                q.extend(&v[..len]);
                if is_fold && v[..len] == chars[..len] {
                    continue;
                }
                let key = xxh64::key(&q);
                for (si, s) in self.stores.iter().enumerate() {
                    if !s.meta.has_terms() || len > s.meta.max_len as usize {
                        continue;
                    }
                    buf.clear();
                    s.terms(&q, key, &mut buf);
                    for t in &buf {
                        if seen.insert((si, t.offset), ()).is_none() {
                            found.push((si, *t, len, is_fold));
                        }
                    }
                }
            }
        }
        // Group by (headword, reading); readings differing only in spacing
        // (pinyin: "huàshé tiānzú", "huàshétiānzú") are one.
        let mut groups: Vec<Group> = Vec::new();
        let mut by_key: HashMap<(String, String), usize> = HashMap::new();
        // (dictionary, term, its place in the dictionary)
        let mut members: Vec<Vec<(usize, TermRef, u64)>> = Vec::new();
        for (si, t, len, is_fold) in found {
            let Some(r) = self.stores[si].record(t) else {
                continue;
            };
            let k = (r.expr.to_owned(), reading_key(r.reading));
            let gi = *by_key.entry(k).or_insert_with(|| {
                groups.push(Group {
                    headword: r.expr.to_owned(),
                    reading: r.reading.to_owned(),
                    matched: len,
                    folded: is_fold,
                    freqs: Vec::new(),
                    parts: Vec::new(),
                });
                members.push(Vec::new());
                groups.len() - 1
            });
            members[gi].push((si, t, r.order));
        }
        // Only query the sorting frequency dictionary before truncating to
        // `MAX_GROUPS`; remaining frequency dictionaries are queried below
        // only for the surviving groups.
        let mut sort_badges: Vec<Option<FreqBadge>> = if let Some(fs) = self.freq_sort {
            groups
                .iter()
                .map(|g| {
                    let hk = xxh64::key(&g.headword);
                    let folded = crate::script::fold_t2s(&g.headword);
                    let folded_ref =
                        (folded != g.headword).then(|| (folded.as_str(), xxh64::key(&folded)));
                    self.freq_in(fs, &g.headword, hk, folded_ref, &g.reading)
                })
                .collect()
        } else {
            Vec::new()
        };
        // Order: longest match, then the chosen list's rank, then the first
        // dictionary that has it, then its best score.
        let rank = |i: usize| -> i64 {
            sort_badges
                .get(i)
                .and_then(|b| b.as_ref())
                .and_then(|b| b.values.iter().map(|v| v.value as i64).min())
                .unwrap_or(i64::MAX)
        };
        let mut order: Vec<usize> = (0..groups.len()).collect();
        order.sort_by_key(|&i| {
            let first = members[i]
                .iter()
                .map(|m| (m.0, m.2))
                .min()
                .unwrap_or((usize::MAX, u64::MAX));
            let score = members[i]
                .iter()
                .filter_map(|&(si, t, _)| self.stores[si].record(t).map(|r| r.score))
                .max()
                .unwrap_or(0);
            (
                std::cmp::Reverse(groups[i].matched),
                groups[i].folded,
                rank(i),
                first.0,
                std::cmp::Reverse(score),
                first.1,
            )
        });
        order.truncate(MAX_GROUPS);
        let has_other_freq = self
            .stores
            .iter()
            .enumerate()
            .any(|(si, s)| s.meta.has_freq() && self.freq_sort != Some(si));
        let mut out = Vec::with_capacity(order.len());
        let mut taken: Vec<Option<Group>> = groups.into_iter().map(Some).collect();
        for i in order {
            let mut g = taken[i].take().unwrap();
            let mut sort_badge = sort_badges.get_mut(i).and_then(Option::take);
            if has_other_freq {
                let hk = xxh64::key(&g.headword);
                let folded = crate::script::fold_t2s(&g.headword);
                let folded_ref =
                    (folded != g.headword).then(|| (folded.as_str(), xxh64::key(&folded)));
                for si in 0..self.stores.len() {
                    let badge = if self.freq_sort == Some(si) {
                        sort_badge.take()
                    } else {
                        self.freq_in(si, &g.headword, hk, folded_ref, &g.reading)
                    };
                    if let Some(b) = badge {
                        g.freqs.push(b);
                    }
                }
            } else if let Some(b) = sort_badge {
                g.freqs.push(b);
            }
            let mut ms = std::mem::take(&mut members[i]);
            ms.sort_by_key(|m| (m.0, m.2));
            for (si, t, _) in ms {
                let Some(r) = self.stores[si].record(t) else {
                    continue;
                };
                let (def_tags, term_tags, slot) =
                    (r.def_tags.to_owned(), r.term_tags.to_owned(), r.slot);
                let json = self.stores[si].glossary(slot);
                let gloss = Gloss {
                    json,
                    def_tags,
                    term_tags,
                };
                match g.parts.last_mut() {
                    Some(p) if p.dict == si => p.glossaries.push(gloss),
                    _ => g.parts.push(Part {
                        dict: si,
                        glossaries: vec![gloss],
                    }),
                }
            }
            out.push(g);
        }
        Lookup { text, groups: out }
    }

    fn freq_in(
        &self,
        si: usize,
        headword: &str,
        headword_key: u64,
        folded: Option<(&str, u64)>,
        reading: &str,
    ) -> Option<FreqBadge> {
        let s = &self.stores[si];
        if !s.meta.has_freq() {
            return None;
        }
        let keep = |v: Vec<Freq>| -> Vec<Freq> {
            v.into_iter()
                .filter(|f| f.reading.is_empty() || f.reading == reading)
                .collect()
        };
        // Rank lists: lower is more frequent; occurrence counts: higher.
        let occurrences = s.meta.frequency_mode.as_deref() == Some("occurrence-based");
        let best = |v: &[Freq]| -> Option<i64> {
            let it = v.iter().map(|f| f.value as i64);
            if occurrences {
                it.max().map(|x| -x)
            } else {
                it.min()
            }
        };
        let mut values = keep(s.freq(headword, headword_key));
        let mut is_fold = false;
        if let Some((simp_str, simp_key)) = folded {
            let simp = keep(s.freq(simp_str, simp_key));
            let better = match (best(&values), best(&simp)) {
                (None, Some(_)) => true,
                (Some(a), Some(b)) => b < a,
                _ => false,
            };
            if better {
                values = simp;
                is_fold = true;
            }
        }
        (!values.is_empty()).then_some(FreqBadge {
            dict: si,
            values,
            folded: is_fold,
        })
    }

    /// A headword's frequencies in every frequency list. A Traditional
    /// headword also gets its Simplified form's, and the more frequent of the
    /// two is shown: lists made from Simplified text rank the rare
    /// Traditional spelling low (時 136,904th in Danmei, 时 near the top).
    pub fn freqs(&self, headword: &str, reading: &str) -> Vec<FreqBadge> {
        let headword_key = xxh64::key(headword);
        let folded = crate::script::fold_t2s(headword);
        let folded_ref = (folded != headword).then(|| (folded.as_str(), xxh64::key(&folded)));
        let mut out = Vec::new();
        for si in 0..self.stores.len() {
            if let Some(badge) = self.freq_in(si, headword, headword_key, folded_ref, reading) {
                out.push(badge);
            }
        }
        out
    }
}
