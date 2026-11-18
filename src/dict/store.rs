//! An imported dictionary opened for lookups: its indexes and records mapped,
//! its Bloom filter in memory.

use super::bloom::Bloom;
use super::format::{self, Bytes, Meta, TermRecord, get_str, lower_bound, u32_at, u64_at};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct Store {
    pub dir: PathBuf,
    pub meta: Meta,
    bloom: Option<Bloom>,
    keys: Bytes,
    terms: Bytes,
    gloss_idx: Bytes,
    gloss: Bytes,
    freq_idx: Bytes,
    freq: Bytes,
    dec: zstd::bulk::Decompressor<'static>,
}

/// A term found by headword: where its record is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TermRef {
    pub offset: u64,
    pub slot: u32,
}

/// A frequency: the reading it is limited to ("" for any), its value (a
/// rank, for rank-based lists) and how to show it.
#[derive(Clone, Debug, PartialEq)]
pub struct Freq {
    pub reading: String,
    pub value: i32,
    pub display: String,
}

impl Store {
    pub fn open(dir: &Path) -> Result<Store, String> {
        let meta: Meta = std::fs::read(dir.join(format::META))
            .map_err(|e| format!("{}: {e}", dir.display()))
            .and_then(|b| {
                serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", dir.display()))
            })?;
        if meta.version != format::VERSION {
            return Err(format!(
                "{}: made by another version of hanstat (format {}, this one reads {})",
                meta.title,
                meta.version,
                format::VERSION
            ));
        }
        let zdict = std::fs::read(dir.join(format::ZDICT)).unwrap_or_default();
        let bloom = std::fs::read(dir.join(format::BLOOM))
            .ok()
            .and_then(|b| Bloom::from_bytes(&b));
        let dec = zstd::bulk::Decompressor::with_dictionary(&zdict).map_err(|e| e.to_string())?;
        Ok(Store {
            dir: dir.to_path_buf(),
            bloom,
            keys: Bytes::map(&dir.join(format::KEYS))?,
            terms: Bytes::map(&dir.join(format::TERMS))?,
            gloss_idx: Bytes::map(&dir.join(format::GLOSS_IDX))?,
            gloss: Bytes::map(&dir.join(format::GLOSS))?,
            freq_idx: Bytes::map(&dir.join(format::FREQ_IDX))?,
            freq: Bytes::map(&dir.join(format::FREQ))?,
            dec,
            meta,
        })
    }

    /// Whether the dictionary may have this headword (by its key): `false`
    /// is certain and costs no file access.
    pub fn may_have(&self, key: u64) -> bool {
        self.bloom.as_ref().is_none_or(|b| b.may_contain(key))
    }

    /// Terms whose headword is `expr` (`key` its hash), in file order.
    pub fn terms(&self, expr: &str, key: u64, out: &mut Vec<TermRef>) {
        if !self.may_have(key) {
            return;
        }
        let n = self.keys.len() / 16;
        let mut i = lower_bound(&self.keys, key);
        while i < n && u64_at(&self.keys, i * 16) == Some(key) {
            let at = u64_at(&self.keys, i * 16 + 8).unwrap_or(u64::MAX);
            i += 1;
            let Some(r) = TermRecord::read(&self.terms, at as usize) else {
                continue;
            };
            if r.expr == expr {
                out.push(TermRef {
                    offset: at,
                    slot: r.slot,
                });
            }
        }
    }

    pub fn record(&self, t: TermRef) -> Option<TermRecord<'_>> {
        TermRecord::read(&self.terms, t.offset as usize)
    }

    /// A glossary's JSON (an array of definitions).
    pub fn glossary(&mut self, slot: u32) -> Result<String, String> {
        let i = slot as usize * format::SLOT_BYTES;
        let (off, len) = u64_at(&self.gloss_idx, i)
            .zip(u32_at(&self.gloss_idx, i + 8))
            .ok_or_else(|| format!("{}: no glossary {slot}", self.meta.title))?;
        let data = self
            .gloss
            .get(off as usize..off as usize + len as usize)
            .ok_or_else(|| format!("{}: glossary {slot} is cut short", self.meta.title))?;
        let b = self
            .dec
            .decompress(data, 64 << 20)
            .map_err(|e| format!("{}: glossary {slot}: {e}", self.meta.title))?;
        String::from_utf8(b).map_err(|e| e.to_string())
    }

    /// The frequencies of `expr` (`key` its hash).
    pub fn freq(&self, expr: &str, key: u64) -> Vec<Freq> {
        let n = self.freq_idx.len() / 16;
        let mut i = lower_bound(&self.freq_idx, key);
        let mut out = Vec::new();
        while i < n && u64_at(&self.freq_idx, i * 16) == Some(key) {
            let mut p = u64_at(&self.freq_idx, i * 16 + 8).unwrap_or(u64::MAX) as usize;
            i += 1;
            let f = &self.freq;
            let (Some(e), Some(reading)) = (get_str(f, &mut p), get_str(f, &mut p)) else {
                continue;
            };
            if e != expr {
                continue;
            }
            let Some(value) = u32_at(f, p) else { continue };
            p += 4;
            let display = get_str(f, &mut p).unwrap_or_default();
            out.push(Freq {
                reading: reading.to_owned(),
                value: value as i32,
                display: display.to_owned(),
            });
        }
        out
    }
}

/// A dictionary's image by its path in the zip, read from `media.bin` with
/// no file kept open (so a dictionary can always be deleted or replaced).
pub fn read_media(dir: &Path, path: &str) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let idx: BTreeMap<String, (u64, u64)> =
        serde_json::from_slice(&std::fs::read(dir.join(format::MEDIA_IDX)).ok()?).ok()?;
    let path = path.trim_start_matches("./").trim_start_matches('/');
    let &(off, len) = idx.get(path)?;
    let mut f = std::fs::File::open(dir.join(format::MEDIA)).ok()?;
    f.seek(SeekFrom::Start(off)).ok()?;
    let mut b = vec![0; len as usize];
    f.read_exact(&mut b).ok()?;
    Some(b)
}
