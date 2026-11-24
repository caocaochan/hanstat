//! A book's vocabulary, kept for the reading path ([`crate::plan`]): its
//! word counts over the included sections, the names left out of its
//! difficulty. Unlike
//! the analysis cache, which is trimmed to a size and keyed by everything the
//! analysis depends on, a profile lasts as long as the book's History entry.
//!
//! Kept in `profiles/<id>.json.gz` next to `history.json`, where `<id>` is
//! the History entry's id (the SHA-256 of the book's file).

use crate::analyze::Report;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Layout of a profile; one of another layout is treated as missing.
pub const FORMAT: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub format: u32,
    /// The History entry's id.
    pub id: String,
    /// Simplified, Traditional, Mixed or Unknown (Chinese only).
    pub variant: String,
    /// The segmenter's label, e.g. "LTP perceptron".
    pub segmenter: String,
    /// Word counts over the included sections, most frequent first (as
    /// `Report::word_freq`).
    pub words: Vec<(String, u32)>,
    /// Names and terms: the user dictionary and the recommended Suggested
    /// words when the profile was written. A word containing one (of two or
    /// more characters) is left out of the running words unless known.
    #[serde(default)]
    pub names: Vec<String>,
    /// Character counts over the included sections, as
    /// `Report::char_freq`: with them a Library row's coverage is computed
    /// again from the profile alone when only the known words change.
    /// `None` in a profile written before they were kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chars: Option<Vec<(char, u32)>>,
}

impl Profile {
    /// From the report the Overview shows. `names` as given to
    /// [`crate::anki::BookCoverage::compute`].
    pub fn from_report<'a>(
        id: &str,
        r: &Report,
        names: impl IntoIterator<Item = &'a String>,
    ) -> Profile {
        let mut names: Vec<String> = names.into_iter().cloned().collect();
        names.sort_unstable();
        names.dedup();
        Profile {
            format: FORMAT,
            id: id.to_owned(),
            variant: format!("{:?}", r.variant),
            segmenter: r.segmenter.label().to_owned(),
            words: r.word_freq.clone(),
            names,
            chars: Some(r.char_freq.clone()),
        }
    }

    /// A hash of the contents, to tell whether a profile needs writing again.
    pub fn digest(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::hash::DefaultHasher::new();
        self.id.hash(&mut h);
        self.variant.hash(&mut h);
        self.segmenter.hash(&mut h);
        self.words.hash(&mut h);
        self.names.hash(&mut h);
        self.chars.hash(&mut h);
        h.finish()
    }

    /// Gzip-compressed JSON, through [`crate::saver`] (in the window,
    /// written in the background).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
        serde_json::to_writer(&mut gz, self)
            .map_err(std::io::Error::other)
            .and_then(|_| gz.finish())
            .map_err(|e| format!("Cannot write {}: {e}", path.display()))
            .and_then(|bytes| crate::saver::write(path, bytes))
    }

    /// `None` if missing, unreadable or of another layout.
    pub fn load(path: &Path) -> Option<Profile> {
        let bytes = crate::saver::read(path).ok()?;
        let p: Profile = serde_json::from_reader(flate2::read::GzDecoder::new(&bytes[..])).ok()?;
        (p.format == FORMAT).then_some(p)
    }
}

/// `profiles/<id>.json.gz` next to the history file `history`.
pub fn path_in(history: &Path, id: &str) -> PathBuf {
    history
        .with_file_name("profiles")
        .join(format!("{id}.json.gz"))
}

/// `profiles/<id>.pos.gz` next to the history file `history`: the book's
/// [`Positions`].
pub fn positions_path_in(history: &Path, id: &str) -> PathBuf {
    history
        .with_file_name("profiles")
        .join(format!("{id}.pos.gz"))
}

/// `read/<id>.json.gz` next to the history file `history`: the profile of a
/// book marked read, as it was then ([`crate::known::across_books`] counts
/// its words), so analysing it again or removing it from the Library
/// doesn't change what it taught.
pub fn read_copy_path_in(history: &Path, id: &str) -> PathBuf {
    history.with_file_name("read").join(format!("{id}.json.gz"))
}

/// Occurrences of a word whose place is kept; the rest are taken as spread
/// evenly over the book after them. Covers learning after up to this many
/// meetings exactly.
pub const POSITION_CAP: u32 = 32;

/// One included section of a book, in reading order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PosSection {
    /// Path inside the EPUB, if any: how the reader's places find it.
    pub href: Option<String>,
    /// Running words before it, over the included sections.
    pub start: u64,
    /// Its running words.
    pub words: u64,
}

/// Where a book's words occur, section by section: the reading path's
/// "at the start" and the book being read are computed from it. Kept beside
/// the profile (`profiles/<id>.pos.gz`, binary: the Library loads profiles
/// all the time and has no use for these), and read only with the profile
/// it was made with ([`Profile::digest`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Positions {
    pub sections: Vec<PosSection>,
    /// Parallel to [`Profile::words`]: (index into `sections`, count there)
    /// for the sections the word occurs in, in order, until
    /// [`POSITION_CAP`] occurrences are covered.
    pub words: Vec<Vec<(u32, u32)>>,
}

const POS_MAGIC: &[u8; 6] = b"HSPOS\x01";

impl Positions {
    /// From the analysis the report was built from: `words` is the report's
    /// `word_freq` (the profile's words), `included` its sections.
    pub fn from_analysis(
        book: &crate::extract::Book,
        prep: &crate::analyze::Prepared,
        included: &[bool],
        words: &[(String, u32)],
    ) -> Positions {
        let index: std::collections::HashMap<&str, usize> = words
            .iter()
            .enumerate()
            .map(|(i, (w, _))| (w.as_str(), i))
            .collect();
        let mut out = Positions {
            sections: Vec::new(),
            words: vec![Vec::new(); words.len()],
        };
        let mut covered = vec![0u32; words.len()];
        let mut start = 0u64;
        for ((sec, st), _) in book
            .sections
            .iter()
            .zip(&prep.stats)
            .zip(included)
            .filter(|(_, inc)| **inc)
        {
            let si = out.sections.len() as u32;
            let n: u64 = st.words.values().map(|&v| v as u64).sum();
            out.sections.push(PosSection {
                href: sec.href.clone(),
                start,
                words: n,
            });
            start += n;
            // In the profile's order, so the file is the same each time.
            let mut here: Vec<(usize, u32)> = st
                .words
                .iter()
                .filter_map(|(w, &c)| index.get(w.as_str()).map(|&i| (i, c)))
                .collect();
            here.sort_unstable();
            for (i, c) in here {
                if covered[i] < POSITION_CAP {
                    out.words[i].push((si, c));
                    covered[i] = covered[i].saturating_add(c);
                }
            }
        }
        out
    }

    /// Running words in all.
    pub fn total(&self) -> u64 {
        self.sections.last().map_or(0, |s| s.start + s.words)
    }

    /// The running word `fraction` of the way through the section with
    /// `href` (compared without case), if the book has it. The reader gives
    /// the fraction: its offset over its own chapter text's length, which
    /// counts the text differently from the analysis (about 13% shorter on
    /// JJWXC books), so the analysis's characters can't be used.
    pub fn running_word_at(&self, href: &str, fraction: f64) -> Option<u64> {
        running_word_in(&self.sections, href, fraction)
    }

    /// Gzip-compressed, through [`crate::saver`]. `digest`: the profile's
    /// ([`Profile::digest`]), checked on loading.
    pub fn save(&self, path: &Path, digest: u64) -> Result<(), String> {
        use std::io::Write;
        let mut b = Vec::new();
        b.extend_from_slice(POS_MAGIC);
        b.extend_from_slice(&digest.to_le_bytes());
        put(&mut b, self.sections.len() as u64);
        for s in &self.sections {
            match &s.href {
                Some(h) => {
                    put(&mut b, h.len() as u64 + 1);
                    b.extend_from_slice(h.as_bytes());
                }
                None => put(&mut b, 0),
            }
            put(&mut b, s.start);
            put(&mut b, s.words);
        }
        put(&mut b, self.words.len() as u64);
        for w in &self.words {
            put(&mut b, w.len() as u64);
            let mut prev = 0u32;
            for &(s, c) in w {
                put(&mut b, (s - prev) as u64);
                put(&mut b, c as u64);
                prev = s;
            }
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
        gz.write_all(&b)
            .and_then(|_| gz.finish())
            .map_err(|e| format!("Cannot write {}: {e}", path.display()))
            .and_then(|bytes| crate::saver::write(path, bytes))
    }

    /// `None` if missing, unreadable, of another layout, or made with
    /// another profile than the one of `digest` with `words` words.
    pub fn load(path: &Path, digest: u64, words: usize) -> Option<Positions> {
        use std::io::Read;
        let gz = crate::saver::read(path).ok()?;
        let mut b = Vec::new();
        flate2::read::GzDecoder::new(&gz[..])
            .read_to_end(&mut b)
            .ok()?;
        let mut r = Cursor { b: &b, i: 0 };
        if r.take(POS_MAGIC.len())? != POS_MAGIC
            || u64::from_le_bytes(r.take(8)?.try_into().ok()?) != digest
        {
            return None;
        }
        let ns = r.get()? as usize;
        let mut sections = Vec::with_capacity(ns.min(1 << 16));
        for _ in 0..ns {
            let h = r.get()? as usize;
            let href = if h == 0 {
                None
            } else {
                Some(String::from_utf8(r.take(h - 1)?.to_vec()).ok()?)
            };
            sections.push(PosSection {
                href,
                start: r.get()?,
                words: r.get()?,
            });
        }
        if r.get()? as usize != words {
            return None;
        }
        let mut out = Vec::with_capacity(words);
        for _ in 0..words {
            let k = r.get()? as usize;
            let mut v = Vec::with_capacity(k.min(POSITION_CAP as usize));
            let mut s = 0u32;
            for _ in 0..k {
                s = s.checked_add(u32::try_from(r.get()?).ok()?)?;
                if s as usize >= ns {
                    return None;
                }
                v.push((s, u32::try_from(r.get()?).ok()?));
            }
            out.push(v);
        }
        (r.i == b.len()).then_some(Positions {
            sections,
            words: out,
        })
    }
}

/// [`Positions::running_word_at`] over `sections`.
pub fn running_word_in(sections: &[PosSection], href: &str, fraction: f64) -> Option<u64> {
    let s = sections.iter().find(|s| {
        s.href
            .as_deref()
            .is_some_and(|h| h.eq_ignore_ascii_case(href))
    })?;
    Some(s.start + (fraction.clamp(0.0, 1.0) * s.words as f64).round() as u64)
}

/// LEB128.
fn put(b: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            b.push(byte);
            return;
        }
        b.push(byte | 0x80);
    }
}

struct Cursor<'a> {
    b: &'a [u8],
    i: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.i..self.i.checked_add(n)?)?;
        self.i += n;
        Some(s)
    }
    fn get(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self.b.get(self.i)?;
            self.i += 1;
            v |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }
}

/// Write `p`'s [`Positions`] beside it, made from the analysis.
pub fn save_positions(
    history: &Path,
    p: &Profile,
    book: &crate::extract::Book,
    prep: &crate::analyze::Prepared,
    included: &[bool],
) -> Result<(), String> {
    Positions::from_analysis(book, prep, included, &p.words)
        .save(&positions_path_in(history, &p.id), p.digest())
}

/// Whether profile `id` has positions beside it (not checked against it).
pub fn has_positions(history: &Path, id: &str) -> bool {
    positions_path_in(history, id).is_file()
}

/// Analyse a book file the way the window does, for a Library book analysed
/// in the background. `included`: the sections to include (the book's
/// defaults if `None` or not of its length). `words` and
/// `known` give the user dictionary and the known words passed to the
/// segmenter. `fold_chars`: see [`analyze::build_report`].
/// Downloads the segmenter's model if missing.
pub struct Analysed {
    pub book: crate::extract::Book,
    pub report: Report,
    /// The user dictionary plus the recommended discovered words.
    pub names: Vec<String>,
    pub file_id: Option<String>,
    /// Where the report's words occur.
    pub positions: Positions,
}

pub fn analyse_file(
    path: &Path,
    kind: crate::segment::SegmenterKind,
    included: Option<&[bool]>,
    words: &[String],
    known: &[String],
    cache: Option<&crate::cache::Cache>,
    fold_chars: bool,
) -> Result<Analysed, String> {
    use crate::{analyze, discover, extract};
    let book = extract::load(path, extract::EncodingChoice::Auto)?;
    let (tr, si) = analyze::detect_variant(&book);
    if let Some(m) = crate::segment::model_for(kind.resolve(crate::script::classify(tr, si)))
        && !m.is_cached()
    {
        m.ensure(&|_, _| {})?;
    }
    let user = words.to_vec();
    let prep = analyze::analyze_book_cached(&book, kind, &user, known, cache, &|_, _| {})?;
    let included: Vec<bool> = match included {
        Some(inc) if inc.len() == book.sections.len() => inc.to_vec(),
        _ => book.sections.iter().map(|s| s.default_included()).collect(),
    };
    let report = analyze::build_report(&book, &prep, &included, fold_chars);
    let positions = Positions::from_analysis(&book, &prep, &included, &report.word_freq);
    let have: std::collections::HashSet<String> = user.iter().cloned().collect();
    let mut names = user;
    names.extend(
        discover::discover(
            &book,
            &included,
            &report.word_freq,
            &have,
            &discover::Options::for_size(report.han_total),
        )
        .into_iter()
        .filter(|c| c.recommended)
        .map(|c| c.word),
    );
    Ok(Analysed {
        file_id: crate::history::file_id(path).ok(),
        book,
        report,
        names,
        positions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_digest() {
        let dir = std::env::temp_dir().join(format!("hanstat-profile-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = Profile {
            format: FORMAT,
            id: "abc".into(),
            variant: "Simplified".into(),
            segmenter: "LTP perceptron".into(),
            words: vec![("我们".into(), 9), ("冒险者".into(), 3)],
            names: vec!["爱丽丝".into()],
            chars: Some(vec![('冒', 3), ('险', 3)]),
        };
        let path = path_in(&dir.join("history.json"), &p.id);
        p.save(&path).unwrap();
        let q = Profile::load(&path).unwrap();
        assert_eq!(q.words, p.words);
        assert_eq!(q.digest(), p.digest());
        let mut r = q.clone();
        r.names.push("鲍勃".into());
        assert_ne!(r.digest(), p.digest());
        std::fs::write(&path, b"junk").unwrap();
        assert!(Profile::load(&path).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn section(
        href: &str,
        text: &str,
        words: &[(&str, u32)],
    ) -> (crate::extract::Section, crate::analyze::SectionStats) {
        let sec = crate::extract::Section {
            label: href.into(),
            href: Some(href.into()),
            kind: crate::extract::SectionKind::Body,
            paragraphs: vec![text.into()],
        };
        let mut st = crate::analyze::SectionStats::default();
        for (w, n) in words {
            st.words.insert((*w).into(), *n);
        }
        (sec, st)
    }

    fn analysed(
        secs: Vec<(crate::extract::Section, crate::analyze::SectionStats)>,
    ) -> (crate::extract::Book, crate::analyze::Prepared) {
        let (sections, stats): (Vec<_>, Vec<_>) = secs.into_iter().unzip();
        (
            crate::extract::Book {
                title: None,
                format: "epub",
                sections,
                notes: Vec::new(),
                decode_errors: 0,
                cover: None,
            },
            crate::analyze::Prepared {
                segmenter_kind: crate::segment::SegmenterKind::Ltp,
                variant: crate::script::Variant::Simplified,
                folding_note: None,
                stats,
                user_words: 0,
                known_words: Vec::new(),
                cached: false,
            },
        )
    }

    #[test]
    fn positions_by_section() {
        let (book, prep) = analysed(vec![
            section("toc.xhtml", "目录", &[("目录", 1)]),
            section(
                "c1.xhtml",
                "一二三四五六七八九十",
                &[("我们", 30), ("冒险", 2)],
            ),
            section(
                "c2.xhtml",
                "一二三四五",
                &[("我们", 10), ("冒险", 8), ("龙", 4)],
            ),
        ]);
        // The table of contents left out.
        let included = [false, true, true];
        let words: Vec<(String, u32)> =
            vec![("我们".into(), 40), ("冒险".into(), 10), ("龙".into(), 4)];
        let p = Positions::from_analysis(&book, &prep, &included, &words);
        assert_eq!(p.sections.len(), 2);
        assert_eq!((p.sections[0].start, p.sections[0].words), (0, 32));
        assert_eq!((p.sections[1].start, p.sections[1].words), (32, 22));
        assert_eq!(p.total(), 54);
        // 我们: 30 in c1, then 10 in c2 (30 < 32); 冒险: both; 龙: c2 only.
        assert_eq!(
            p.words,
            vec![vec![(0, 30), (1, 10)], vec![(0, 2), (1, 8)], vec![(1, 4)]]
        );
        // Past the cap, no more sections are kept.
        let words2: Vec<(String, u32)> = vec![("我们".into(), 40)];
        let (book2, prep2) = analysed(vec![
            section("a", "x", &[("我们", 33)]),
            section("b", "x", &[("我们", 7)]),
        ]);
        let q = Positions::from_analysis(&book2, &prep2, &[true, true], &words2);
        assert_eq!(q.words, vec![vec![(0, 33)]]);
        // Reader places: a share of c2 is that share of its words.
        assert_eq!(p.running_word_at("C2.xhtml", 0.0), Some(32));
        assert_eq!(p.running_word_at("c2.xhtml", 0.4), Some(32 + 9));
        assert_eq!(p.running_word_at("c2.xhtml", 1.5), Some(54));
        assert_eq!(p.running_word_at("toc.xhtml", 0.0), None);

        let dir = std::env::temp_dir().join(format!("hanstat-pos-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = positions_path_in(&dir.join("history.json"), "abc");
        p.save(&path, 42).unwrap();
        assert_eq!(Positions::load(&path, 42, 3).as_ref(), Some(&p));
        // Made with another profile, or another number of words: not read.
        assert!(Positions::load(&path, 43, 3).is_none());
        assert!(Positions::load(&path, 42, 4).is_none());
        std::fs::write(&path, b"junk").unwrap();
        assert!(Positions::load(&path, 42, 3).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
