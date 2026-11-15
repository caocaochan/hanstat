//! Counting. Each section is analysed independently (paragraphs in parallel),
//! then included sections are merged into a [`Report`]. Toggling a section
//! only re-merges; it does not re-segment.

use crate::chars::{self, Class, NormStats};
use crate::extract::{Book, SectionKind};
use crate::script::{self, Variant};
use crate::segment::{Segmenter, SegmenterKind};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashMap;
use unicode_segmentation::UnicodeSegmentation;

pub const COVERAGE_TARGETS: [f64; 8] = [50.0, 80.0, 90.0, 95.0, 98.0, 99.0, 99.5, 100.0];

#[derive(Default, Clone, Debug)]
pub struct SectionStats {
    /// Indexed by `class_index`.
    pub class_counts: [u64; 9],
    pub han: HashMap<char, u32>,
    pub words: HashMap<String, u32>,
    pub foreign_words: HashMap<String, u32>,
    pub numbers: u64,
    pub norm: NormStats,
    pub paragraphs: u64,
    pub sentences: u64,
    pub trad_marks: u64,
    pub simp_marks: u64,
}

pub fn class_index(c: Class) -> usize {
    match c {
        Class::Han => 0,
        Class::Letter => 1,
        Class::Digit => 2,
        Class::Punct => 3,
        Class::Symbol => 4,
        Class::Space => 5,
        Class::PrivateUse => 6,
        Class::Replacement => 7,
        Class::Other => 8,
    }
}

impl SectionStats {
    fn add_ref(&mut self, o: &SectionStats) {
        for i in 0..9 {
            self.class_counts[i] += o.class_counts[i];
        }
        for (k, v) in &o.han {
            *self.han.entry(*k).or_default() += v;
        }
        for (k, v) in &o.words {
            if let Some(e) = self.words.get_mut(k) {
                *e += v;
            } else {
                self.words.insert(k.clone(), *v);
            }
        }
        for (k, v) in &o.foreign_words {
            if let Some(e) = self.foreign_words.get_mut(k) {
                *e += v;
            } else {
                self.foreign_words.insert(k.clone(), *v);
            }
        }
        self.numbers += o.numbers;
        self.norm.add(&o.norm);
        self.paragraphs += o.paragraphs;
        self.sentences += o.sentences;
        self.trad_marks += o.trad_marks;
        self.simp_marks += o.simp_marks;
    }

    fn merge(mut self, o: SectionStats) -> SectionStats {
        for i in 0..9 {
            self.class_counts[i] += o.class_counts[i];
        }
        for (k, v) in o.han {
            *self.han.entry(k).or_default() += v;
        }
        for (k, v) in o.words {
            *self.words.entry(k).or_default() += v;
        }
        for (k, v) in o.foreign_words {
            *self.foreign_words.entry(k).or_default() += v;
        }
        self.numbers += o.numbers;
        self.norm.add(&o.norm);
        self.paragraphs += o.paragraphs;
        self.sentences += o.sentences;
        self.trad_marks += o.trad_marks;
        self.simp_marks += o.simp_marks;
        self
    }
}

/// Book-wide Traditional/Simplified evidence (needed before segmentation).
pub fn detect_variant(book: &Book) -> (u64, u64) {
    let t = &*script::TABLES;
    book.sections
        .par_iter()
        .flat_map_iter(|s| s.paragraphs.iter())
        .map(|p| {
            p.chars().fold((0u64, 0u64), |(a, b), c| {
                (
                    a + t.trad_only.contains(&c) as u64,
                    b + t.simp_only.contains(&c) as u64,
                )
            })
        })
        .reduce(|| (0, 0), |x, y| (x.0 + y.0, x.1 + y.1))
}

pub struct Prepared {
    pub segmenter_kind: SegmenterKind,
    pub variant: Variant,
    pub folding_note: Option<&'static str>,
    pub stats: Vec<SectionStats>,
    pub user_words: usize,
}

/// Paragraphs per work unit: CKIP batches the chunks of one group together.
const GROUP: usize = 128;

/// Full analysis of every section of the book (included or not).
/// `progress(done, total)` is called in paragraphs.
pub fn analyze_book(
    book: &Book,
    kind: SegmenterKind,
    user_words: &[String],
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Prepared, String> {
    let (tr, si) = detect_variant(book);
    let variant = script::classify(tr, si);
    let mut norm_dummy = NormStats::default();
    let words: Vec<String> = user_words
        .iter()
        .map(|w| chars::normalize(w.trim(), &mut norm_dummy))
        .filter(|w| !w.is_empty())
        .collect();
    let seg = Segmenter::new(kind, &words, variant)?;
    let total: usize = book.sections.iter().map(|s| s.paragraphs.len()).sum();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let stats = book
        .sections
        .par_iter()
        .map(|s| {
            s.paragraphs
                .par_chunks(GROUP)
                .fold(SectionStats::default, |mut acc, group| {
                    analyze_group(&mut acc, group, &seg);
                    let d = done.fetch_add(group.len(), std::sync::atomic::Ordering::Relaxed)
                        + group.len();
                    progress(d, total);
                    acc
                })
                .reduce(SectionStats::default, SectionStats::merge)
        })
        .collect();
    Ok(Prepared {
        segmenter_kind: seg.kind,
        variant,
        folding_note: seg.folding_note(),
        stats,
        user_words: words.len(),
    })
}

fn analyze_group(acc: &mut SectionStats, raw: &[String], seg: &Segmenter) {
    let paras: Vec<String> = raw
        .iter()
        .map(|p| chars::normalize(p, &mut acc.norm))
        .collect();
    for p in &paras {
        count_paragraph(acc, p);
    }
    // Chinese words: segment whitespace-free chunks that contain Han, all at once.
    let chunks: Vec<&str> = paras
        .iter()
        .flat_map(|p| p.split(char::is_whitespace))
        .filter(|c| c.chars().any(chars::is_han))
        .collect();
    for toks in seg.cut_many(&chunks) {
        for t in toks {
            if !t.chars().any(chars::is_han) {
                continue;
            }
            let clean = t.chars().all(|c| {
                matches!(
                    chars::classify(c),
                    Class::Han | Class::Letter | Class::Digit
                )
            });
            if clean {
                *acc.words.entry(t.to_owned()).or_default() += 1;
            } else {
                // Rare: a token glued to punctuation. Keep only the Han-bearing runs.
                for run in t.split(|c: char| {
                    !matches!(
                        chars::classify(c),
                        Class::Han | Class::Letter | Class::Digit
                    )
                }) {
                    if run.chars().any(chars::is_han) {
                        *acc.words.entry(run.to_owned()).or_default() += 1;
                    }
                }
            }
        }
    }
}

/// Character classes, sentences and non-Chinese words of one normalised paragraph.
fn count_paragraph(acc: &mut SectionStats, p: &str) {
    let tables = &*script::TABLES;

    // Characters and sentences.
    let mut has_content = false;
    let mut para_has_content = false;
    let cs: Vec<char> = p.chars().collect();
    for (i, &c) in cs.iter().enumerate() {
        let cl = chars::classify(c);
        acc.class_counts[class_index(cl)] += 1;
        match cl {
            Class::Han => {
                *acc.han.entry(c).or_default() += 1;
                if tables.trad_only.contains(&c) {
                    acc.trad_marks += 1;
                } else if tables.simp_only.contains(&c) {
                    acc.simp_marks += 1;
                }
                has_content = true;
            }
            Class::Letter | Class::Digit | Class::PrivateUse => has_content = true,
            _ => {}
        }
        let ascii_stop = c == '.'
            && i > 0
            && !cs[i - 1].is_ascii_digit()
            && cs.get(i + 1).is_none_or(|n| {
                n.is_whitespace() || matches!(n, '"' | '\'' | '”' | '’' | '」' | '』')
            });
        if (chars::is_sentence_end(c) || ascii_stop) && has_content {
            acc.sentences += 1;
            has_content = false;
            para_has_content = true;
        }
    }
    if has_content {
        acc.sentences += 1;
        para_has_content = true;
    }
    if para_has_content {
        acc.paragraphs += 1;
    }

    // Non-Chinese words (UAX #29), excluding anything containing Han.
    for w in p.unicode_words() {
        if w.chars().any(chars::is_han) {
            continue;
        }
        if w.chars().any(char::is_alphabetic) {
            *acc.foreign_words.entry(w.to_lowercase()).or_default() += 1;
        } else {
            acc.numbers += 1;
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SectionRow {
    pub label: String,
    pub kind: SectionKind,
    pub included: bool,
    pub han: u64,
    pub unique_han: usize,
    /// Distinct characters first seen in this section (among included sections, in order).
    pub new_han: Option<usize>,
    pub words: u64,
    pub unique_words: usize,
    pub new_words: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Coverage {
    pub percent: f64,
    /// Number of most-frequent distinct items needed to reach `percent` of all tokens.
    pub items: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub title: Option<String>,
    pub format: &'static str,
    pub segmenter: SegmenterKind,
    pub variant: Variant,
    pub folded_for_segmentation: bool,
    pub user_words: usize,

    pub han_total: u64,
    pub han_unique: usize,
    pub han_hapax: usize,
    pub nonspace_total: u64,
    pub letters: u64,
    pub digits: u64,
    pub punctuation: u64,
    pub symbols: u64,
    pub whitespace: u64,
    pub private_use: u64,
    pub replacement: u64,
    pub other: u64,

    pub words_total: u64,
    pub words_unique: usize,
    pub words_hapax: usize,
    /// Token counts for word lengths 1, 2, 3, 4, 5+ (in characters).
    pub word_length_tokens: [u64; 5],
    pub avg_word_len: f64,
    pub foreign_words_total: u64,
    pub foreign_words_unique: usize,
    pub numbers: u64,

    pub paragraphs: u64,
    pub sentences: u64,
    pub avg_sentence_han: f64,
    pub avg_paragraph_han: f64,

    pub trad_marks: u64,
    pub simp_marks: u64,

    pub char_coverage: Vec<Coverage>,
    pub word_coverage: Vec<Coverage>,

    pub norm: NormStats,
    pub sections: Vec<SectionRow>,
    pub notes: Vec<String>,

    /// Filled in by the caller when Anki data is in use.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anki: Option<crate::anki::BookCoverage>,

    #[serde(skip)]
    pub char_freq: Vec<(char, u32)>,
    #[serde(skip)]
    pub word_freq: Vec<(String, u32)>,
    #[serde(skip)]
    pub foreign_freq: Vec<(String, u32)>,
}

fn coverage<T>(sorted: &[(T, u32)], total: u64) -> Vec<Coverage> {
    let mut out = Vec::new();
    let mut acc = 0u64;
    let mut ti = 0;
    for (i, (_, n)) in sorted.iter().enumerate() {
        acc += *n as u64;
        while ti < COVERAGE_TARGETS.len()
            && acc as f64 * 100.0 >= COVERAGE_TARGETS[ti] * total as f64 - 1e-9
        {
            out.push(Coverage {
                percent: COVERAGE_TARGETS[ti],
                items: i + 1,
            });
            ti += 1;
        }
    }
    out
}

fn sorted_freq<K: Ord + Clone>(m: &HashMap<K, u32>) -> Vec<(K, u32)> {
    let mut v: Vec<(K, u32)> = m.iter().map(|(k, v)| (k.clone(), *v)).collect();
    v.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

pub fn build_report(book: &Book, prep: &Prepared, included: &[bool]) -> Report {
    let mut total = SectionStats::default();
    let mut seen_c: std::collections::HashSet<char> = Default::default();
    let mut seen_w: std::collections::HashSet<&str> = Default::default();
    let mut rows = Vec::new();
    for (i, (sec, st)) in book.sections.iter().zip(&prep.stats).enumerate() {
        let inc = included[i];
        let (mut new_han, mut new_words) = (None, None);
        if inc {
            new_han = Some(st.han.keys().filter(|c| seen_c.insert(**c)).count());
            new_words = Some(
                st.words
                    .keys()
                    .filter(|w| seen_w.insert(w.as_str()))
                    .count(),
            );
        }
        rows.push(SectionRow {
            label: sec.label.clone(),
            kind: sec.kind,
            included: inc,
            han: st.class_counts[0],
            unique_han: st.han.len(),
            new_han,
            words: st.words.values().map(|&v| v as u64).sum(),
            unique_words: st.words.len(),
            new_words,
        });
    }
    for (st, _) in prep.stats.iter().zip(included).filter(|(_, inc)| **inc) {
        total.add_ref(st);
    }

    let char_freq = sorted_freq(&total.han);
    let word_freq = sorted_freq(&total.words);
    let foreign_freq = sorted_freq(&total.foreign_words);
    let c = &total.class_counts;
    let han_total = c[0];
    let words_total: u64 = word_freq.iter().map(|(_, n)| *n as u64).sum();
    let mut wl = [0u64; 5];
    let mut han_in_words = 0u64;
    for (w, n) in &word_freq {
        let l = w.chars().count();
        wl[l.clamp(1, 5) - 1] += *n as u64;
        han_in_words += (w.chars().filter(|c| chars::is_han(*c)).count() as u64) * *n as u64;
    }

    let mut notes = book.notes.clone();
    if total.class_counts[6] > 0 {
        notes.push(format!(
            "{} Private Use Area characters found. Some Chinese e-books encode rare or anti-copy characters this way and render them with an embedded font; they are NOT counted as Chinese characters.",
            total.class_counts[6]
        ));
    }
    if let Some(n) = prep.folding_note {
        notes.push(n.to_owned());
    }
    let n = &total.norm;
    if n.compat_ideographs
        + n.radicals_folded
        + n.variation_selectors
        + n.fullwidth_folded
        + n.invisible_removed
        > 0
    {
        notes.push(format!(
            "Normalisation: {} compatibility ideographs unified, {} Kangxi/CJK radicals folded to ideographs, {} variation selectors dropped, {} fullwidth letters/digits folded, {} invisible characters removed.",
            n.compat_ideographs, n.radicals_folded, n.variation_selectors, n.fullwidth_folded, n.invisible_removed
        ));
    }

    Report {
        title: book.title.clone(),
        format: book.format,
        segmenter: prep.segmenter_kind,
        variant: prep.variant,
        folded_for_segmentation: prep.folding_note.is_some(),
        user_words: prep.user_words,
        han_total,
        han_unique: char_freq.len(),
        han_hapax: char_freq.iter().filter(|(_, n)| *n == 1).count(),
        nonspace_total: c.iter().sum::<u64>() - c[5],
        letters: c[1],
        digits: c[2],
        punctuation: c[3],
        symbols: c[4],
        whitespace: c[5],
        private_use: c[6],
        replacement: c[7],
        other: c[8],
        words_total,
        words_unique: word_freq.len(),
        words_hapax: word_freq.iter().filter(|(_, n)| *n == 1).count(),
        word_length_tokens: wl,
        avg_word_len: if words_total > 0 {
            han_in_words as f64 / words_total as f64
        } else {
            0.0
        },
        foreign_words_total: foreign_freq.iter().map(|(_, n)| *n as u64).sum(),
        foreign_words_unique: foreign_freq.len(),
        numbers: total.numbers,
        paragraphs: total.paragraphs,
        sentences: total.sentences,
        avg_sentence_han: if total.sentences > 0 {
            han_total as f64 / total.sentences as f64
        } else {
            0.0
        },
        avg_paragraph_han: if total.paragraphs > 0 {
            han_total as f64 / total.paragraphs as f64
        } else {
            0.0
        },
        trad_marks: total.trad_marks,
        simp_marks: total.simp_marks,
        char_coverage: coverage(&char_freq, han_total),
        word_coverage: coverage(&word_freq, words_total),
        norm: total.norm,
        sections: rows,
        notes,
        anki: None,
        char_freq,
        word_freq,
        foreign_freq,
    }
}

/// CSV (UTF-8 with BOM so Excel detects it) of a frequency list.
pub fn freq_csv<K: std::fmt::Display>(items: &[(K, u32)], header: &str) -> String {
    let total: u64 = items.iter().map(|(_, n)| *n as u64).sum();
    let mut s = String::from("\u{FEFF}");
    s.push_str(&format!("rank,{header},count,percent,cumulative_percent\n"));
    let mut acc = 0u64;
    for (i, (k, n)) in items.iter().enumerate() {
        acc += *n as u64;
        let k = k.to_string();
        let k = if k.contains([',', '"']) {
            format!("\"{}\"", k.replace('"', "\"\""))
        } else {
            k
        };
        s.push_str(&format!(
            "{},{},{},{:.4},{:.4}\n",
            i + 1,
            k,
            n,
            *n as f64 * 100.0 / total.max(1) as f64,
            acc as f64 * 100.0 / total.max(1) as f64
        ));
    }
    s
}
