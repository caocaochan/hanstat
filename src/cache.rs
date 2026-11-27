//! Saved analysis results. Segmenting a book takes seconds (LTP, jieba) to
//! minutes (CKIP); the outcome, the per-section counts of
//! [`crate::analyze::analyze_book`], is kept in `cache` in
//! [`crate::models::cache_dir`] (`%LOCALAPPDATA%\hanstat\cache`; override: `HANSTAT_CACHE_DIR`; off: `HANSTAT_NO_CACHE`) and read back the
//! next time nothing it depends on has changed.
//!
//! An entry is found by the SHA-256 of everything the counts depend on:
//!
//! * the extracted text, paragraph by paragraph and section by section (not
//!   the file: a renamed, moved or re-zipped book is still the same book, and
//!   a TXT read with another encoding is not);
//! * the script variant, the concrete segmenter and its model
//!   (see [`crate::segment::model_identity`]);
//! * the forced words and the known words given to the segmenter;
//! * the analysis code and its data, as a fingerprint made by `build.rs` of
//!   the sources, the bundled tables and the dependency versions. A release
//!   that changes none of them keeps the cache; one that does starts afresh,
//!   so a result never outlives the code that produced it.
//!
//! Not in the key, because they are applied after the counts: which sections
//! are included, Anki, vocabulary levels, and section labels.
//!
//! An entry is gzip-compressed JSON (`<key>.json.gz`) written under a
//! temporary name and renamed. One that cannot be read, or does not fit the
//! book, counts as missing and is deleted. The folder is kept under
//! [`MAX_BYTES`] by deleting the entries unused the longest.

use crate::analyze::SectionStats;
use crate::chars::NormStats;
use crate::extract::Book;
use crate::script::Variant;
use crate::segment::SegmenterKind;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

/// Layout of an entry; entries of another layout are not found (it is hashed into the key).
const FORMAT: u32 = 2;

/// The analysis sources, bundled tables and dependency versions (`build.rs`).
const FINGERPRINT: &str = env!("HANSTAT_ANALYSIS_FINGERPRINT");

/// The folder is trimmed to this size after each new entry. An entry is
/// about 0.1 MB for a light novel, 1.5 MB for an 830,000-character novel in
/// 180 chapters and 6.5 MB for a 3.5-million-character one in 771: the size
/// follows the number of sections, each of which has its own word counts.
pub const MAX_BYTES: u64 = 1_000_000_000;

const SUFFIX: &str = ".json.gz";

/// What identifies an analysis: see the module documentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key(String);

impl Key {
    /// `words` and `known` are the forced and known words as the segmenter
    /// gets them (normalised); `model` is [`crate::segment::model_identity`].
    pub fn new(
        book: &Book,
        variant: Variant,
        segmenter: SegmenterKind,
        model: &str,
        words: &[String],
        known: &[String],
    ) -> Key {
        let mut h = Sha256::new();
        // Every variable-length item is preceded by its length, so two
        // different inputs never hash the same bytes.
        let text = |h: &mut Sha256, s: &str| {
            h.update((s.len() as u64).to_le_bytes());
            h.update(s.as_bytes());
        };
        text(&mut h, "hanstat analysis");
        h.update(FORMAT.to_le_bytes());
        text(&mut h, FINGERPRINT);
        text(&mut h, &format!("{variant:?}/{segmenter:?}"));
        text(&mut h, model);
        h.update((words.len() as u64).to_le_bytes());
        for w in words {
            text(&mut h, w);
        }
        // The segmenter treats known words as a set.
        let mut known: Vec<&str> = known.iter().map(String::as_str).collect();
        known.sort_unstable();
        known.dedup();
        h.update((known.len() as u64).to_le_bytes());
        for w in known {
            text(&mut h, w);
        }
        h.update((book.sections.len() as u64).to_le_bytes());
        for s in &book.sections {
            h.update((s.paragraphs.len() as u64).to_le_bytes());
            for p in &s.paragraphs {
                text(&mut h, p);
            }
        }
        Key(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }
}

/// On disk: every distinct string once, and the sections' maps as indexes into them.
#[derive(Serialize, Deserialize)]
struct Entry {
    format: u32,
    key: String,
    strings: Vec<String>,
    sections: Vec<Section>,
}

#[derive(Serialize, Deserialize)]
struct Section {
    class_counts: [u64; 9],
    numbers: u64,
    paragraphs: u64,
    sentences: u64,
    trad_marks: u64,
    simp_marks: u64,
    norm: NormStats,
    /// The distinct characters, with their counts in `han_counts`.
    han: String,
    han_counts: Vec<u32>,
    /// (string, count) pairs, flattened.
    words: Vec<u32>,
    foreign_words: Vec<u32>,
}

/// Gives each distinct string a number, in order of first use.
#[derive(Default)]
struct Interner(HashMap<String, u32>);

impl Interner {
    fn id(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.0.get(s) {
            return i;
        }
        let i = self.0.len() as u32;
        self.0.insert(s.to_owned(), i);
        i
    }

    fn pairs(&mut self, m: &HashMap<String, u32>) -> Vec<u32> {
        let mut v = Vec::with_capacity(m.len() * 2);
        for (k, n) in m {
            v.push(self.id(k));
            v.push(*n);
        }
        v
    }

    fn into_strings(self) -> Vec<String> {
        let mut v = vec![String::new(); self.0.len()];
        for (s, i) in self.0 {
            v[i as usize] = s;
        }
        v
    }
}

fn pack(key: &Key, stats: &[SectionStats]) -> Entry {
    let mut it = Interner::default();
    let sections = stats
        .iter()
        .map(|s| {
            let (han, han_counts) = s.han.iter().map(|(c, n)| (*c, *n)).unzip();
            Section {
                class_counts: s.class_counts,
                numbers: s.numbers,
                paragraphs: s.paragraphs,
                sentences: s.sentences,
                trad_marks: s.trad_marks,
                simp_marks: s.simp_marks,
                norm: s.norm.clone(),
                han,
                han_counts,
                words: it.pairs(&s.words),
                foreign_words: it.pairs(&s.foreign_words),
            }
        })
        .collect();
    Entry {
        format: FORMAT,
        key: key.0.clone(),
        strings: it.into_strings(),
        sections,
    }
}

/// `None` if anything is out of range: the entry is damaged.
fn unpack(e: Entry) -> Option<Vec<SectionStats>> {
    let st = &e.strings;
    let get = |i: u32| st.get(i as usize).cloned();
    let pairs = |v: &[u32]| -> Option<HashMap<String, u32>> {
        let (pairs, rest) = v.as_chunks::<2>();
        if !rest.is_empty() {
            return None;
        }
        pairs.iter().map(|&[s, n]| Some((get(s)?, n))).collect()
    };
    e.sections
        .iter()
        .map(|s| {
            let han: Vec<char> = s.han.chars().collect();
            if han.len() != s.han_counts.len() {
                return None;
            }
            Some(SectionStats {
                class_counts: s.class_counts,
                han: han.into_iter().zip(s.han_counts.iter().copied()).collect(),
                words: pairs(&s.words)?,
                foreign_words: pairs(&s.foreign_words)?,
                numbers: s.numbers,
                norm: s.norm.clone(),
                paragraphs: s.paragraphs,
                sentences: s.sentences,
                trad_marks: s.trad_marks,
                simp_marks: s.simp_marks,
            })
        })
        .collect()
}

/// A folder of saved analyses.
#[derive(Clone, Debug)]
pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    /// The user's cache, or `None` when `HANSTAT_NO_CACHE` is set.
    pub fn open() -> Option<Cache> {
        if std::env::var_os("HANSTAT_NO_CACHE").is_some() {
            return None;
        }
        Some(Cache::at(Cache::default_dir()))
    }

    pub fn default_dir() -> PathBuf {
        match std::env::var_os("HANSTAT_CACHE_DIR") {
            Some(d) => PathBuf::from(d),
            None => crate::models::cache_dir().join("cache"),
        }
    }

    pub fn at(dir: impl Into<PathBuf>) -> Cache {
        Cache { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, key: &Key) -> PathBuf {
        self.dir.join(format!("{}{SUFFIX}", key.0))
    }

    /// The saved counts for `key`, one per section; `None` if there are none
    /// (or they are unreadable, or not for a book of `sections` sections).
    pub fn get(&self, key: &Key, sections: usize) -> Option<Vec<SectionStats>> {
        let path = self.path(key);
        let file = std::fs::File::open(&path).ok()?;
        let entry: Option<Entry> =
            serde_json::from_reader(BufReader::new(flate2::read::GzDecoder::new(file))).ok();
        let stats = entry
            .filter(|e| e.format == FORMAT && e.key == key.0 && e.sections.len() == sections)
            .and_then(unpack);
        match stats {
            Some(s) => {
                // Mark it used, for `trim`.
                if let Ok(f) = std::fs::File::options().write(true).open(&path) {
                    let _ = f.set_modified(std::time::SystemTime::now());
                }
                Some(s)
            }
            None => {
                let _ = std::fs::remove_file(&path);
                None
            }
        }
    }

    /// Save `stats` under `key`, then trim the folder to [`MAX_BYTES`].
    /// Failing to (a full or read-only disk) only means analysing again next time.
    pub fn put(&self, key: &Key, stats: &[SectionStats]) -> Result<(), String> {
        let err = |e: &dyn std::fmt::Display| format!("Analysis cache {}: {e}", self.dir.display());
        std::fs::create_dir_all(&self.dir).map_err(|e| err(&e))?;
        let tmp = self
            .dir
            .join(format!("{}.{}.part", key.0, std::process::id()));
        let written = (|| -> std::io::Result<()> {
            let gz = flate2::write::GzEncoder::new(
                std::fs::File::create(&tmp)?,
                flate2::Compression::fast(),
            );
            let mut w = BufWriter::new(gz);
            serde_json::to_writer(&mut w, &pack(key, stats)).map_err(std::io::Error::other)?;
            w.into_inner().map_err(|e| e.into_error())?.finish()?;
            std::fs::rename(&tmp, self.path(key))
        })();
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(err(&e));
        }
        static PUTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        if PUTS
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .is_multiple_of(16)
        {
            self.trim(MAX_BYTES);
        }
        Ok(())
    }

    /// (path, size, last used) of every entry.
    fn entries(&self) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
        std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(SUFFIX))
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                Some((e.path(), m.len(), m.modified().ok()?))
            })
            .collect()
    }

    /// Delete the entries unused the longest until the rest fit in `max` bytes.
    fn trim(&self, max: u64) {
        let mut es = self.entries();
        let mut total: u64 = es.iter().map(|e| e.1).sum();
        if total <= max {
            return;
        }
        es.sort_unstable_by_key(|e| e.2);
        for (path, size, _) in es {
            if total <= max {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total -= size;
            }
        }
    }

    /// Number of saved analyses and their size in bytes.
    pub fn usage(&self) -> (usize, u64) {
        let es = self.entries();
        (es.len(), es.iter().map(|e| e.1).sum())
    }

    /// Delete every saved analysis (and anything an interrupted save left).
    pub fn clear(&self) -> Result<(), String> {
        let mut failed = None;
        for e in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if (name.ends_with(SUFFIX) || name.ends_with(".part"))
                && let Err(err) = std::fs::remove_file(e.path())
            {
                failed = Some(format!("Cannot delete {}: {err}", e.path().display()));
            }
        }
        failed.map_or(Ok(()), Err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{Section, SectionKind};

    fn temp_cache(tag: &str) -> Cache {
        let dir =
            std::env::temp_dir().join(format!("hanstat-cache-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Cache::at(dir)
    }

    fn book(sections: &[&[&str]]) -> Book {
        Book {
            title: None,
            format: "TXT",
            sections: sections
                .iter()
                .map(|ps| Section {
                    label: String::new(),
                    href: None,
                    kind: SectionKind::Body,
                    paragraphs: ps.iter().map(|p| p.to_string()).collect(),
                })
                .collect(),
            notes: Vec::new(),
            cover: None,
        }
    }

    fn key_of(b: &Book, model: &str, words: &[&str], known: &[&str]) -> Key {
        let v = |x: &[&str]| x.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        Key::new(
            b,
            Variant::Simplified,
            SegmenterKind::Ltp,
            model,
            &v(words),
            &v(known),
        )
    }

    fn sample() -> Vec<SectionStats> {
        let mut a = SectionStats {
            class_counts: [5, 4, 3, 2, 1, 0, 9, 8, 7],
            numbers: 2,
            paragraphs: 3,
            sentences: 4,
            trad_marks: 5,
            simp_marks: 6,
            ..Default::default()
        };
        a.norm.fullwidth_folded = 7;
        a.han.extend([('我', 3), ('𠮷', 1)]);
        a.words
            .extend([("我们".to_owned(), 2), ("学习".to_owned(), 1)]);
        a.foreign_words.insert("abc".to_owned(), 4);
        // An empty section, and one sharing strings with the first.
        let mut c = SectionStats::default();
        c.words.insert("我们".to_owned(), 9);
        vec![a, SectionStats::default(), c]
    }

    #[test]
    fn round_trip() {
        let cache = temp_cache("round-trip");
        let key = key_of(&book(&[&["我们学习"]]), "m", &[], &[]);
        assert!(cache.get(&key, 3).is_none());
        let stats = sample();
        cache.put(&key, &stats).unwrap();
        assert_eq!(cache.get(&key, 3).as_deref(), Some(stats.as_slice()));
        assert_eq!(cache.usage().0, 1);
        // Not for a book with another number of sections; such an entry is dropped.
        assert!(cache.get(&key, 2).is_none());
        assert_eq!(cache.usage().0, 0);
        std::fs::remove_dir_all(cache.dir()).unwrap();
    }

    #[test]
    fn key_follows_everything_the_counts_depend_on() {
        let b = book(&[&["我们", "学习"], &["汉字"]]);
        let k = key_of(&b, "m", &["甲"], &["乙", "丙"]);
        assert_eq!(k, key_of(&b, "m", &["甲"], &["乙", "丙"]));
        // Known words are a set; forced words are kept in order.
        assert_eq!(k, key_of(&b, "m", &["甲"], &["丙", "乙", "丙"]));
        assert_ne!(k, key_of(&b, "m2", &["甲"], &["乙", "丙"]));
        assert_ne!(k, key_of(&b, "m", &[], &["乙", "丙"]));
        assert_ne!(k, key_of(&b, "m", &["甲"], &["乙"]));
        // A forced word is not a known word.
        assert_ne!(key_of(&b, "m", &["甲"], &[]), key_of(&b, "m", &[], &["甲"]));
        // The same characters split differently into paragraphs or sections.
        for other in [
            book(&[&["我们学习"], &["汉字"]]),
            book(&[&["我们"], &["学习", "汉字"]]),
            book(&[&["我们", "学习"], &["汉字"], &[]]),
            book(&[&["我们", "学习"], &["汉子"]]),
        ] {
            assert_ne!(k, key_of(&other, "m", &["甲"], &["乙", "丙"]));
        }
        let other = Key::new(
            &b,
            Variant::Traditional,
            SegmenterKind::Ltp,
            "m",
            &["甲".to_owned()],
            &["乙".to_owned(), "丙".to_owned()],
        );
        assert_ne!(k, other);
    }

    #[test]
    fn damaged_entries_count_as_missing() {
        let cache = temp_cache("damaged");
        let key = key_of(&book(&[&["我们"]]), "m", &[], &[]);
        let stats = sample();
        let write = |e: &Entry| {
            std::fs::create_dir_all(cache.dir()).unwrap();
            let mut gz = flate2::write::GzEncoder::new(
                std::fs::File::create(cache.path(&key)).unwrap(),
                flate2::Compression::fast(),
            );
            serde_json::to_writer(&mut gz, e).unwrap();
            gz.finish().unwrap();
        };

        // Not gzip at all.
        std::fs::create_dir_all(cache.dir()).unwrap();
        std::fs::write(cache.path(&key), b"not a cache entry").unwrap();
        assert!(cache.get(&key, 3).is_none());
        assert!(!cache.path(&key).exists());

        // Cut short.
        cache.put(&key, &stats).unwrap();
        let bytes = std::fs::read(cache.path(&key)).unwrap();
        std::fs::write(cache.path(&key), &bytes[..bytes.len() / 2]).unwrap();
        assert!(cache.get(&key, 3).is_none());

        // A string index past the table.
        let mut e = pack(&key, &stats);
        e.sections[0].words[0] = e.strings.len() as u32;
        write(&e);
        assert!(cache.get(&key, 3).is_none());

        // A list that ends in the middle of an item.
        let mut e = pack(&key, &stats);
        e.sections[0].words.pop();
        write(&e);
        assert!(cache.get(&key, 3).is_none());

        // Saved under another key's name.
        let mut e = pack(&key, &stats);
        e.key = "0".repeat(64);
        write(&e);
        assert!(cache.get(&key, 3).is_none());

        cache.put(&key, &stats).unwrap();
        assert_eq!(cache.get(&key, 3), Some(stats));
        std::fs::remove_dir_all(cache.dir()).unwrap();
    }

    #[test]
    fn trim_deletes_the_entries_unused_the_longest() {
        let cache = temp_cache("trim");
        let stats = sample();
        let keys: Vec<Key> = ["一", "二", "三"]
            .iter()
            .map(|t| key_of(&book(&[&[t]]), "m", &[], &[]))
            .collect();
        let t0 = std::time::SystemTime::now() - std::time::Duration::from_secs(1000);
        for (i, k) in keys.iter().enumerate() {
            cache.put(k, &stats).unwrap();
            let f = std::fs::File::options()
                .write(true)
                .open(cache.path(k))
                .unwrap();
            f.set_modified(t0 + std::time::Duration::from_secs(i as u64 * 100))
                .unwrap();
        }
        // Reading the oldest makes it the newest.
        assert!(cache.get(&keys[0], 3).is_some());
        let size = cache.usage().1 / 3;
        cache.trim(size * 2);
        assert!(cache.path(&keys[0]).exists());
        assert!(!cache.path(&keys[1]).exists());
        assert!(cache.path(&keys[2]).exists());

        cache.clear().unwrap();
        assert_eq!(cache.usage(), (0, 0));
        std::fs::remove_dir_all(cache.dir()).unwrap();
    }
}
