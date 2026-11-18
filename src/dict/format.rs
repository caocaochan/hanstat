//! The files of an imported dictionary (format version [`VERSION`]) and the
//! list of dictionaries.
//!
//! A dictionary lives in `dictionaries/<id>/`, `<id>` a random number given
//! at its first import and kept through updates:
//!
//! | file | contents |
//! |---|---|
//! | `meta.json` | [`Meta`] |
//! | `zdict` | the zstd dictionary the glossaries are compressed with |
//! | `gloss.bin` | the compressed glossaries, each distinct one once |
//! | `gloss.idx` | per glossary slot: offset u64, length u32 |
//! | `terms.bin` | term records (see [`TermRecord`]) |
//! | `keys.idx` | (xxh64 of the headword u64, term offset u64), sorted |
//! | `keys.bloom` | a Bloom filter of the headword hashes |
//! | `freq.bin` | frequency records: headword, reading ("" = any), value i32, display |
//! | `freq.idx` | (xxh64 of the headword u64, record offset u64), sorted |
//! | `media.bin`, `media.json` | images, by their path in the zip |
//!
//! All numbers little-endian; strings are a u16 length and UTF-8.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Bumped when the files change: dictionaries of another version are
/// imported again.
pub const VERSION: u32 = 1;

pub const META: &str = "meta.json";
pub const ZDICT: &str = "zdict";
pub const GLOSS: &str = "gloss.bin";
pub const GLOSS_IDX: &str = "gloss.idx";
pub const TERMS: &str = "terms.bin";
pub const KEYS: &str = "keys.idx";
pub const BLOOM: &str = "keys.bloom";
pub const FREQ: &str = "freq.bin";
pub const FREQ_IDX: &str = "freq.idx";
pub const MEDIA: &str = "media.bin";
pub const MEDIA_IDX: &str = "media.json";

/// Bytes of a `gloss.idx` entry.
pub const SLOT_BYTES: usize = 12;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Meta {
    pub version: u32,
    pub title: String,
    pub revision: String,
    pub author: Option<String>,
    pub attribution: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub source_language: Option<String>,
    pub target_language: Option<String>,
    pub frequency_mode: Option<String>,
    /// The Yomitan format (1–3).
    pub format: u32,
    pub terms: u64,
    pub glossaries: u64,
    pub freqs: u64,
    pub media: u64,
    /// Rows not kept, by reason (`pitch`, `ipa`, `kanji`, `kanji_meta`).
    pub skipped: BTreeMap<String, u64>,
    /// The longest headword, in characters.
    pub max_len: u32,
    /// The dictionary's `styles.css`.
    pub styles: String,
    pub tags: Vec<Tag>,
    pub update: Option<UpdateUrls>,
    pub source: Option<SourceZip>,
    /// Unix seconds.
    pub imported: u64,
    /// Bytes on disk.
    pub bytes: u64,
}

impl Meta {
    pub fn has_terms(&self) -> bool {
        self.terms > 0
    }
    pub fn has_freq(&self) -> bool {
        self.freqs > 0
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Tag {
    pub name: String,
    pub category: String,
    pub order: i64,
    pub notes: String,
    pub score: i64,
}

/// Where a newer revision is announced (`indexUrl`) and downloaded.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct UpdateUrls {
    pub index_url: String,
    pub download_url: String,
}

/// The zip a dictionary was imported from.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SourceZip {
    pub path: PathBuf,
    pub size: u64,
    /// Unix seconds.
    pub mtime: u64,
}

/// The dictionaries, in the user's order.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Manifest {
    pub dicts: Vec<Entry>,
    /// Unix seconds of the last automatic update check.
    pub last_auto_check: u64,
    /// The default dictionary (CC-CEDICT) was installed once: never again,
    /// so deleting it sticks.
    pub default_installed: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    pub enabled: bool,
    #[serde(default)]
    pub check: Option<Check>,
    /// Its entries start collapsed in the popup.
    #[serde(default)]
    pub collapsed: bool,
}

/// The last update check of a dictionary.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Check {
    /// Unix seconds.
    pub at: u64,
    pub result: CheckResult,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum CheckResult {
    UpToDate,
    Available {
        revision: String,
        download_url: String,
    },
    Failed(String),
}

impl Manifest {
    pub fn path(root: &Path) -> PathBuf {
        root.join("dictionaries.json")
    }

    pub fn load(root: &Path) -> Manifest {
        std::fs::read(Self::path(root))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Written to a temporary file and renamed, so a crash never leaves half a list.
    pub fn save(&self, root: &Path) -> Result<(), String> {
        std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
        write_atomic(&Self::path(root), &serde_json::to_vec_pretty(self).unwrap())
    }
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A new dictionary id.
pub fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    h.write_u32(std::process::id());
    format!("{:016x}", h.finish())
}

/// A read-only mapped file (an empty one is not mapped: Windows can't map it).
pub enum Bytes {
    Map(memmap2::Mmap),
    Empty,
}

impl std::ops::Deref for Bytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Bytes::Map(m) => m,
            Bytes::Empty => &[],
        }
    }
}

impl Bytes {
    pub fn map(path: &Path) -> Result<Bytes, String> {
        let f = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Bytes::Empty),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let len = f.metadata().map_err(|e| e.to_string())?.len();
        if len == 0 {
            return Ok(Bytes::Empty);
        }
        // Safety: the dictionary's files are written once, before they are
        // mapped, and replaced only by renaming a new folder in after the
        // maps are dropped.
        unsafe { memmap2::Mmap::map(&f) }
            .map(Bytes::Map)
            .map_err(|e| format!("{}: {e}", path.display()))
    }
}

pub fn put_str(out: &mut Vec<u8>, s: &str) {
    let mut n = s.len().min(u16::MAX as usize);
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    out.extend_from_slice(&(n as u16).to_le_bytes());
    out.extend_from_slice(&s.as_bytes()[..n]);
}

/// Read a string at `*p`, moving past it. `None` if the record is cut short.
pub fn get_str<'a>(b: &'a [u8], p: &mut usize) -> Option<&'a str> {
    let n = u16::from_le_bytes(b.get(*p..*p + 2)?.try_into().ok()?) as usize;
    let s = std::str::from_utf8(b.get(*p + 2..*p + 2 + n)?).ok()?;
    *p += 2 + n;
    Some(s)
}

pub fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

pub fn u64_at(b: &[u8], i: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(i..i + 8)?.try_into().ok()?))
}

/// A term record of `terms.bin`: headword, reading, definition tags, rules,
/// term tags (strings), then score i32, glossary slot u32 and its place in
/// the dictionary u64 (bank number << 32 | row), which orders equal results
/// as the dictionary does.
#[derive(Clone, Debug, PartialEq)]
pub struct TermRecord<'a> {
    pub expr: &'a str,
    pub reading: &'a str,
    pub def_tags: &'a str,
    pub rules: &'a str,
    pub term_tags: &'a str,
    pub score: i32,
    pub slot: u32,
    pub order: u64,
}

impl<'a> TermRecord<'a> {
    pub fn write(&self, out: &mut Vec<u8>) {
        put_str(out, self.expr);
        put_str(out, self.reading);
        put_str(out, self.def_tags);
        put_str(out, self.rules);
        put_str(out, self.term_tags);
        out.extend_from_slice(&self.score.to_le_bytes());
        out.extend_from_slice(&self.slot.to_le_bytes());
        out.extend_from_slice(&self.order.to_le_bytes());
    }

    pub fn read(b: &'a [u8], at: usize) -> Option<TermRecord<'a>> {
        let mut p = at;
        let expr = get_str(b, &mut p)?;
        let reading = get_str(b, &mut p)?;
        let def_tags = get_str(b, &mut p)?;
        let rules = get_str(b, &mut p)?;
        let term_tags = get_str(b, &mut p)?;
        let score = u32_at(b, p)? as i32;
        let slot = u32_at(b, p + 4)?;
        let order = u64_at(b, p + 8)?;
        Some(TermRecord {
            expr,
            reading,
            def_tags,
            rules,
            term_tags,
            score,
            slot,
            order,
        })
    }

    /// Just the headword (cheap: for checking a hash match).
    pub fn expr_at(b: &'a [u8], at: usize) -> Option<&'a str> {
        let mut p = at;
        get_str(b, &mut p)
    }
}

/// The first entry of a sorted (hash, offset) index with this hash.
pub fn lower_bound(keys: &[u8], h: u64) -> usize {
    let (mut lo, mut hi) = (0, keys.len() / 16);
    while lo < hi {
        let m = (lo + hi) / 2;
        if u64_at(keys, m * 16).unwrap_or(u64::MAX) < h {
            lo = m + 1;
        } else {
            hi = m;
        }
    }
    lo
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip() {
        let r = TermRecord {
            expr: "说话",
            reading: "shuō huà",
            def_tags: "v",
            rules: "",
            term_tags: "",
            score: -3,
            slot: 77,
            order: (2 << 32) | 5,
        };
        let mut b = vec![9u8];
        r.write(&mut b);
        assert_eq!(TermRecord::read(&b, 1), Some(r));
        assert_eq!(TermRecord::expr_at(&b, 1), Some("说话"));
        assert_eq!(TermRecord::read(&b[..b.len() - 1], 1), None, "cut short");
    }

    #[test]
    fn long_strings_are_cut_at_a_character() {
        let s = "字".repeat(30_000);
        let mut b = Vec::new();
        put_str(&mut b, &s);
        let mut p = 0;
        let back = get_str(&b, &mut p).unwrap();
        assert!(back.len() <= u16::MAX as usize && back.chars().all(|c| c == '字'));
    }

    #[test]
    fn empty_and_missing_files_map_to_nothing() {
        let dir = std::env::temp_dir().join(format!("hanstat-bytes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let empty = dir.join("empty");
        std::fs::write(&empty, b"").unwrap();
        assert!(Bytes::map(&empty).unwrap().is_empty());
        assert!(Bytes::map(&dir.join("missing")).unwrap().is_empty());
        std::fs::write(dir.join("one"), b"x").unwrap();
        assert_eq!(&*Bytes::map(&dir.join("one")).unwrap(), b"x");
        std::fs::remove_dir_all(&dir).ok();
    }
}
