//! Importing a Yomitan dictionary zip into the files of [`super::format`].
//!
//! The term and frequency banks are read in parallel, one bank per worker at
//! a time (a bank is a few MB; a whole dictionary unzipped can be gigabytes,
//! never held at once). Each glossary is kept as its raw JSON text, hashed,
//! and stored once: the first bank to claim a glossary compresses it (zstd,
//! with a dictionary trained on a sample of the first bank) and the claim
//! gives it a slot number, which the term records point to. One writer
//! appends to the files; the indexes are sorted at the end.

use super::bloom::Bloom;
use super::format::{self, Meta, SourceZip, TermRecord, UpdateUrls, put_str};
use super::xxh64;
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use serde::Deserialize;
use serde_json::value::RawValue;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering::Relaxed};

/// A bank larger than this (unzipped) is refused, as is a zip whose files
/// add up to more than [`MAX_TOTAL`]: a guard against zip bombs. The largest
/// bank seen in real dictionaries is 27 MB.
const MAX_BANK: u64 = 256 << 20;
const MAX_TOTAL: u64 = 16 << 30;

/// Banks done and to do, for a progress bar.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicUsize,
    pub total: AtomicUsize,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        let t = self.total.load(Relaxed);
        if t == 0 {
            0.0
        } else {
            self.done.load(Relaxed) as f32 / t as f32
        }
    }
}

/// What `index.json` says, before anything is imported.
#[derive(Clone, Debug, PartialEq)]
pub struct Index {
    pub title: String,
    pub revision: String,
    pub format: u32,
    pub update: Option<UpdateUrls>,
    pub author: Option<String>,
    pub attribution: Option<String>,
    pub description: Option<String>,
    pub frequency_mode: Option<String>,
}

/// An http(s) URL, as Yomitan requires of `indexUrl` and `downloadUrl`.
pub fn valid_url(s: &str) -> bool {
    (s.starts_with("https://") || s.starts_with("http://"))
        && s.len() > 8
        && !s.contains(char::is_whitespace)
}

impl Index {
    pub fn parse(text: &str) -> Result<Index, String> {
        let v: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("index.json: {e}"))?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_owned);
        let title = s("title")
            .filter(|t| !t.trim().is_empty())
            .ok_or("index.json has no title")?;
        // Revisions are strings; some dictionaries write a number.
        let revision = match v.get("revision") {
            Some(serde_json::Value::String(r)) => r.clone(),
            Some(serde_json::Value::Number(n)) => n.to_string(),
            _ => return Err("index.json has no revision".into()),
        };
        let format = v
            .get("format")
            .or_else(|| v.get("version"))
            .and_then(|x| x.as_u64())
            .unwrap_or(3) as u32;
        // Yomitan keeps the URLs only of a dictionary marked updatable; it
        // refuses one whose URLs are not valid, where hanstat imports it as
        // not updatable.
        let update = match (
            v.get("isUpdatable").and_then(|x| x.as_bool()),
            s("indexUrl"),
            s("downloadUrl"),
        ) {
            (Some(true), Some(i), Some(d)) if valid_url(&i) && valid_url(&d) => Some(UpdateUrls {
                index_url: i,
                download_url: d,
            }),
            _ => None,
        };
        Ok(Index {
            title,
            revision,
            format,
            update,
            author: s("author"),
            attribution: s("attribution"),
            description: s("description"),
            frequency_mode: s("frequencyMode"),
        })
    }
}

/// Read only `index.json` of a zip.
pub fn read_index(zip_path: &Path) -> Result<Index, String> {
    let f = std::fs::File::open(zip_path).map_err(|e| format!("{}: {e}", zip_path.display()))?;
    let mut z = zip::ZipArchive::new(std::io::BufReader::new(f))
        .map_err(|e| format!("{}: not a zip ({e})", zip_path.display()))?;
    let mut text = String::new();
    z.by_name("index.json")
        .map_err(|_| "the zip has no index.json: not a Yomitan dictionary".to_string())?
        .read_to_string(&mut text)
        .map_err(|e| format!("index.json: {e}"))?;
    Index::parse(&text)
}

/// A term bank row, borrowing from the bank's bytes: `[expression, reading,
/// definitionTags, rules, score, glossary, sequence, termTags]`. The glossary
/// stays raw JSON.
#[derive(Deserialize)]
struct TermRow<'a> {
    #[serde(borrow)]
    expr: Cow<'a, str>,
    #[serde(borrow)]
    reading: Cow<'a, str>,
    #[serde(borrow)]
    def_tags: Option<Cow<'a, str>>,
    _rules: serde::de::IgnoredAny,
    score: f64,
    #[serde(borrow)]
    gloss: &'a RawValue,
    #[serde(default)]
    _seq: Option<i64>,
    #[serde(default, borrow)]
    term_tags: Option<Cow<'a, str>>,
}

/// A `term_meta` row: `[expression, mode, data]`.
#[derive(Deserialize)]
struct MetaRow<'a> {
    #[serde(borrow)]
    expr: Cow<'a, str>,
    #[serde(borrow)]
    mode: Cow<'a, str>,
    #[serde(borrow)]
    data: &'a RawValue,
}

/// Yomitan's frequency value shapes: `5`, `"5"`, `{value, displayValue}`.
#[derive(Deserialize)]
#[serde(untagged)]
enum FreqValue<'a> {
    Num(f64),
    #[serde(borrow)]
    Str(Cow<'a, str>),
    Obj {
        value: f64,
        #[serde(rename = "displayValue", default, borrow)]
        display: Option<Cow<'a, str>>,
    },
}

#[derive(Deserialize)]
struct WithReading<'a> {
    #[serde(borrow)]
    reading: Cow<'a, str>,
    #[serde(borrow)]
    frequency: FreqValue<'a>,
}

/// A frequency entry's (reading, value, display): any of the value shapes,
/// alone or as `{reading, frequency}` (the reading then limits it).
pub fn parse_freq(data: &str) -> Option<(String, i32, String)> {
    fn flat(v: FreqValue) -> Option<(i32, String)> {
        match v {
            FreqValue::Num(n) => Some((n as i32, (n as i64).to_string())),
            FreqValue::Str(s) => {
                let n: f64 = s.trim().parse().ok()?;
                Some((n as i32, s.into_owned()))
            }
            FreqValue::Obj { value, display } => Some((
                value as i32,
                display.map_or_else(|| (value as i64).to_string(), Cow::into_owned),
            )),
        }
    }
    let s = data.trim();
    if s.starts_with('{') && s.contains("\"frequency\"") {
        let w: WithReading = serde_json::from_str(s).ok()?;
        let (v, d) = flat(w.frequency)?;
        return Some((w.reading.into_owned(), v, d));
    }
    let (v, d) = flat(serde_json::from_str(s).ok()?)?;
    Some((String::new(), v, d))
}

/// Glossary hashes already claimed: the first claim gives the slot.
struct Claims {
    shards: Vec<Mutex<FxHashMap<u128, u32>>>,
    next: AtomicU32,
}

impl Claims {
    fn new() -> Claims {
        Claims {
            shards: (0..256).map(|_| Mutex::new(FxHashMap::default())).collect(),
            next: AtomicU32::new(0),
        }
    }
    /// The slot of this glossary, and whether this call claimed it.
    fn claim(&self, h: u128) -> (u32, bool) {
        let mut s = self.shards[(h >> 120) as usize].lock().unwrap();
        if let Some(slot) = s.get(&h) {
            return (*slot, false);
        }
        let slot = self.next.fetch_add(1, Relaxed);
        s.insert(h, slot);
        (slot, true)
    }
}

#[derive(Default)]
struct BankOut {
    terms: Vec<u8>,
    keys: Vec<(u64, u64)>,
    gloss: Vec<u8>,
    /// (slot, offset in `gloss`, length)
    slots: Vec<(u32, u32, u32)>,
    n_terms: u64,
    max_len: u32,
    freq: Vec<u8>,
    freq_keys: Vec<(u64, u64)>,
    n_freq: u64,
}

enum Kind {
    Term,
    Meta,
}

fn read_entry(
    z: &mut zip::ZipArchive<Cursor<&[u8]>>,
    i: usize,
) -> Result<(String, Vec<u8>), String> {
    let mut f = z.by_index(i).map_err(|e| e.to_string())?;
    let name = f.name().unwrap_or_default().to_string();
    if f.size() > MAX_BANK && name.ends_with(".json") {
        return Err(format!(
            "{name}: {} MB unzipped, more than a bank can be",
            f.size() >> 20
        ));
    }
    let mut b = Vec::with_capacity(f.size().min(MAX_BANK) as usize);
    f.read_to_end(&mut b).map_err(|e| format!("{name}: {e}"))?;
    Ok((name, b))
}

/// A bank's number from its name (`term_bank_12.json` → 12).
fn bank_number(name: &str) -> u64 {
    let base = name.rsplit('/').next().unwrap_or(name);
    base.trim_end_matches(".json")
        .rsplit('_')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

fn term_bank(
    name: &str,
    b: &[u8],
    c: &mut zstd::bulk::Compressor,
    claims: &Claims,
) -> Result<BankOut, String> {
    let rows: Vec<TermRow> = serde_json::from_slice(b).map_err(|e| format!("{name}: {e}"))?;
    let mut o = BankOut {
        terms: Vec::with_capacity(rows.len() * 48),
        keys: Vec::with_capacity(rows.len()),
        n_terms: rows.len() as u64,
        ..Default::default()
    };
    let mut buf = Vec::new();
    let bank = bank_number(name) << 32;
    for (row, r) in rows.iter().enumerate() {
        let g = r.gloss.get().as_bytes();
        let (slot, mine) = claims.claim(xxh64::wide(g));
        if mine {
            buf.clear();
            buf.reserve(zstd::zstd_safe::compress_bound(g.len()));
            c.compress_to_buffer(g, &mut buf)
                .map_err(|e| format!("{name}: {e}"))?;
            o.slots.push((slot, o.gloss.len() as u32, buf.len() as u32));
            o.gloss.extend_from_slice(&buf);
        }
        let reading: &str = if r.reading.is_empty() {
            &r.expr
        } else {
            &r.reading
        };
        let at = o.terms.len() as u64;
        TermRecord {
            expr: &r.expr,
            reading,
            def_tags: r.def_tags.as_deref().unwrap_or(""),
            term_tags: r.term_tags.as_deref().unwrap_or(""),
            score: r.score as i32,
            slot,
            order: bank | row as u64,
        }
        .write(&mut o.terms);
        o.keys.push((xxh64::key(&r.expr), at));
        o.max_len = o.max_len.max(r.expr.chars().count() as u32);
    }
    Ok(o)
}

fn meta_bank(name: &str, b: &[u8]) -> Result<BankOut, String> {
    let rows: Vec<MetaRow> = serde_json::from_slice(b).map_err(|e| format!("{name}: {e}"))?;
    let mut o = BankOut::default();
    for r in &rows {
        if r.mode != "freq" {
            continue;
        }
        let Some((reading, value, display)) = parse_freq(r.data.get()) else {
            continue;
        };
        o.freq_keys.push((xxh64::key(&r.expr), o.freq.len() as u64));
        put_str(&mut o.freq, &r.expr);
        put_str(&mut o.freq, &reading);
        o.freq.extend_from_slice(&value.to_le_bytes());
        put_str(&mut o.freq, &display);
        o.n_freq += 1;
    }
    Ok(o)
}

/// The zstd dictionary: trained on up to 2 MB of glossaries spread through
/// the first term bank, as hoshidicts does. Empty when there is too little to
/// learn from (the glossaries are then compressed without one).
fn train(name: &str, b: &[u8]) -> Result<Vec<u8>, String> {
    const MAX: usize = 2 << 20;
    let rows: Vec<TermRow> = serde_json::from_slice(b).map_err(|e| format!("{name}: {e}"))?;
    let total: usize = rows.iter().map(|r| r.gloss.get().len()).sum();
    let step = (total / MAX).max(1);
    let mut samples: Vec<&[u8]> = Vec::new();
    let mut size = 0;
    for r in rows.iter().step_by(step) {
        if size >= MAX {
            break;
        }
        samples.push(r.gloss.get().as_bytes());
        size += r.gloss.get().len();
    }
    if samples.len() < 8 {
        return Ok(Vec::new());
    }
    Ok(train_fast_cover(&samples, threads()).unwrap_or_default())
}

fn threads() -> usize {
    std::thread::available_parallelism()
        .map_or(2, |n| n.get())
        .saturating_sub(1)
        .max(1)
}

/// zstd's fastCover training with hoshidicts' parameters (d = 8, 4 steps),
/// on `threads` threads: about 4 times faster than `zstd::dict::from_samples`
/// (single-threaded) on these glossaries.
fn train_fast_cover(samples: &[&[u8]], threads: usize) -> Option<Vec<u8>> {
    let mut flat = Vec::with_capacity(samples.iter().map(|s| s.len()).sum());
    let sizes: Vec<usize> = samples
        .iter()
        .map(|s| {
            flat.extend_from_slice(s);
            s.len()
        })
        .collect();
    let mut dict = vec![0u8; 110 * 1024];
    // Safety: zeroed is a valid "use the defaults" parameter block.
    let mut p: zstd_sys::ZDICT_fastCover_params_t = unsafe { std::mem::zeroed() };
    p.d = 8;
    p.steps = 4;
    p.splitPoint = 1.0;
    p.nbThreads = threads as u32;
    // Safety: the buffers and their sizes are as given; zstd writes at most
    // `dict.len()` bytes and returns the size or an error code.
    let n = unsafe {
        zstd_sys::ZDICT_optimizeTrainFromBuffer_fastCover(
            dict.as_mut_ptr().cast(),
            dict.len(),
            flat.as_ptr().cast(),
            sizes.as_ptr(),
            sizes.len() as u32,
            &mut p,
        )
    };
    if unsafe { zstd_sys::ZDICT_isError(n) } != 0 {
        return None;
    }
    dict.truncate(n);
    Some(dict)
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Import `zip_path` into the folder `out` (made empty first). On success the
/// folder holds every file and [`Meta`] is returned; on failure or `cancel`
/// what was written is left for the caller to delete.
pub fn import(
    zip_path: &Path,
    out: &Path,
    progress: &Progress,
    cancel: &AtomicBool,
) -> Result<Meta, String> {
    let _ = std::fs::remove_dir_all(out);
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let file = std::fs::File::open(zip_path).map_err(|e| format!("{}: {e}", zip_path.display()))?;
    let file_meta = file.metadata().map_err(|e| e.to_string())?;
    // Safety: the zip is only read; a zip changed by another program while it
    // is imported gives a failed import, not undefined behaviour in practice
    // (the same risk as reading it).
    let map =
        unsafe { memmap2::Mmap::map(&file) }.map_err(|e| format!("{}: {e}", zip_path.display()))?;
    let mut z = zip::ZipArchive::new(Cursor::new(&map[..]))
        .map_err(|e| format!("{}: not a zip ({e})", zip_path.display()))?;

    let mut index_text = String::new();
    z.by_name("index.json")
        .map_err(|_| "the zip has no index.json: not a Yomitan dictionary".to_string())?
        .read_to_string(&mut index_text)
        .map_err(|e| format!("index.json: {e}"))?;
    let index = Index::parse(&index_text)?;
    if index.format < 2 {
        return Err(format!(
            "{}: Yomitan dictionary format {} is not supported (only 2 and 3)",
            index.title, index.format
        ));
    }
    let mut styles = String::new();
    if let Ok(mut f) = z.by_name("styles.css") {
        let _ = f.read_to_string(&mut styles);
    }

    let mut banks: Vec<(usize, Kind)> = Vec::new();
    let mut media = Vec::new();
    let mut total_size = 0u64;
    for i in 0..z.len() {
        let f = z.by_index_raw(i).map_err(|e| e.to_string())?;
        let name = f.name().unwrap_or_default().to_string();
        total_size += f.size();
        if name.ends_with('/') {
            continue;
        }
        let base = name.rsplit('/').next().unwrap_or(&name);
        if base.starts_with("term_bank_") && base.ends_with(".json") {
            banks.push((i, Kind::Term));
        } else if base.starts_with("term_meta_bank_") && base.ends_with(".json") {
            banks.push((i, Kind::Meta));
        } else if (base.starts_with("tag_bank_")
            || base.starts_with("kanji_bank_")
            || base.starts_with("kanji_meta_bank_"))
            && base.ends_with(".json")
        {
            // Unused Yomitan banks (not media).
        } else if name != "index.json" && name != "styles.css" {
            media.push(i);
        }
    }
    if total_size > MAX_TOTAL {
        return Err(format!(
            "{}: {} GB unzipped, more than a dictionary can be",
            index.title,
            total_size >> 30
        ));
    }
    if banks.is_empty() {
        return Err(format!("{}: no term or frequency banks", index.title));
    }
    progress.total.store(banks.len(), Relaxed);
    progress.done.store(0, Relaxed);

    let timing = std::env::var_os("HANSTAT_DICT_TIMING").is_some();
    let t0 = std::time::Instant::now();
    let zdict = match banks.iter().find(|b| matches!(b.1, Kind::Term)) {
        Some(&(i, _)) => {
            let (name, b) = read_entry(&mut z, i)?;
            train(&name, &b)?
        }
        None => Vec::new(),
    };
    write(&out.join(format::ZDICT), &zdict)?;
    let t_train = t0.elapsed();

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads())
        .thread_name(|i| format!("dict-import-{i}"))
        .build()
        .map_err(|e| e.to_string())?;
    let claims = Claims::new();
    let open = |p: &str| std::fs::File::create(out.join(p)).map_err(|e| format!("{p}: {e}"));
    let mut gloss_file = std::io::BufWriter::with_capacity(1 << 20, open(format::GLOSS)?);
    let mut terms_file = std::io::BufWriter::with_capacity(1 << 20, open(format::TERMS)?);
    let mut freq_file = std::io::BufWriter::with_capacity(1 << 20, open(format::FREQ)?);
    let mut keys: Vec<(u64, u64)> = Vec::new();
    let mut freq_keys: Vec<(u64, u64)> = Vec::new();
    let mut slots: Vec<(u64, u32)> = Vec::new();
    let (mut g_off, mut t_off, mut f_off) = (0u64, 0u64, 0u64);
    let mut meta = Meta {
        version: format::VERSION,
        title: index.title.clone(),
        revision: index.revision.clone(),
        author: index.author.clone(),
        attribution: index.attribution.clone(),
        description: index.description.clone(),
        frequency_mode: index.frequency_mode.clone(),
        format: index.format,
        styles,
        update: index.update.clone(),
        ..Default::default()
    };
    let stop = AtomicBool::new(false);
    let (send, recv) = std::sync::mpsc::sync_channel::<Result<BankOut, String>>(32);
    let mut failed: Option<String> = None;
    std::thread::scope(|s| {
        let (z, zdict, claims, stop) = (z.clone(), &zdict, &claims, &stop);
        let pool = &pool;
        s.spawn(move || {
            pool.install(|| {
                banks.par_iter().for_each_init(
                    || {
                        (
                            z.clone(),
                            zstd::bulk::Compressor::with_dictionary(3, zdict).expect("zstd"),
                        )
                    },
                    |(z, c), (i, kind)| {
                        if stop.load(Relaxed) || cancel.load(Relaxed) {
                            return;
                        }
                        let r = read_entry(z, *i).and_then(|(name, b)| match kind {
                            Kind::Term => term_bank(&name, &b, c, claims),
                            Kind::Meta => meta_bank(&name, &b),
                        });
                        let _ = send.send(r);
                    },
                );
            });
        });
        let io = |r: std::io::Result<()>| r.map_err(|e| e.to_string());
        for r in recv {
            if cancel.load(Relaxed) {
                failed.get_or_insert_with(|| "cancelled".into());
                stop.store(true, Relaxed);
                continue;
            }
            let b = match r {
                Ok(b) => b,
                Err(e) => {
                    failed.get_or_insert(e);
                    stop.store(true, Relaxed);
                    continue;
                }
            };
            if failed.is_some() {
                continue;
            }
            let res = (|| {
                io(gloss_file.write_all(&b.gloss))?;
                io(terms_file.write_all(&b.terms))?;
                io(freq_file.write_all(&b.freq))
            })();
            if let Err(e) = res {
                failed.get_or_insert(e);
                stop.store(true, Relaxed);
                continue;
            }
            for &(slot, at, len) in &b.slots {
                let slot = slot as usize;
                if slots.len() <= slot {
                    slots.resize(slot + 1, (0, 0));
                }
                slots[slot] = (g_off + at as u64, len);
            }
            keys.extend(b.keys.iter().map(|&(h, o)| (h, o + t_off)));
            freq_keys.extend(b.freq_keys.iter().map(|&(h, o)| (h, o + f_off)));
            g_off += b.gloss.len() as u64;
            t_off += b.terms.len() as u64;
            f_off += b.freq.len() as u64;
            meta.terms += b.n_terms;
            meta.freqs += b.n_freq;
            meta.max_len = meta.max_len.max(b.max_len);
            progress.done.fetch_add(1, Relaxed);
        }
    });
    if let Some(e) = failed {
        return Err(e);
    }
    for f in [&mut gloss_file, &mut terms_file, &mut freq_file] {
        f.flush().map_err(|e| e.to_string())?;
    }
    drop((gloss_file, terms_file, freq_file));
    meta.glossaries = slots.len() as u64;
    let t_banks = t0.elapsed();

    // Indexes.
    pool.install(|| {
        rayon::join(
            || keys.par_sort_unstable(),
            || freq_keys.par_sort_unstable(),
        )
    });
    for (name, k) in [(format::KEYS, &keys), (format::FREQ_IDX, &freq_keys)] {
        let mut w = std::io::BufWriter::with_capacity(1 << 20, open(name)?);
        for (h, o) in k {
            let mut rec = [0u8; 16];
            rec[..8].copy_from_slice(&h.to_le_bytes());
            rec[8..].copy_from_slice(&o.to_le_bytes());
            w.write_all(&rec).map_err(|e| format!("{name}: {e}"))?;
        }
        w.flush().map_err(|e| format!("{name}: {e}"))?;
    }
    let mut bloom = Bloom::with_keys(keys.len());
    let mut last = None;
    for (h, _) in &keys {
        if last != Some(*h) {
            bloom.insert(*h);
            last = Some(*h);
        }
    }
    write(&out.join(format::BLOOM), &bloom.to_bytes())?;
    if !freq_keys.is_empty() {
        let mut freq_bloom = Bloom::with_keys(freq_keys.len());
        let mut last = None;
        for (h, _) in &freq_keys {
            if last != Some(*h) {
                freq_bloom.insert(*h);
                last = Some(*h);
            }
        }
        write(&out.join(format::FREQ_BLOOM), &freq_bloom.to_bytes())?;
    }
    {
        let mut w = std::io::BufWriter::with_capacity(1 << 20, open(format::GLOSS_IDX)?);
        for (off, len) in &slots {
            let mut rec = [0u8; format::SLOT_BYTES];
            rec[..8].copy_from_slice(&off.to_le_bytes());
            rec[8..].copy_from_slice(&len.to_le_bytes());
            w.write_all(&rec)
                .map_err(|e| format!("{}: {e}", format::GLOSS_IDX))?;
        }
        w.flush()
            .map_err(|e| format!("{}: {e}", format::GLOSS_IDX))?;
    }
    drop((keys, freq_keys, slots));

    let t_index = t0.elapsed();
    // Media.
    if !media.is_empty() {
        let mut bin = std::io::BufWriter::new(open(format::MEDIA)?);
        let mut idx: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        let mut off = 0u64;
        let mut b = Vec::new();
        for i in media {
            if cancel.load(Relaxed) {
                return Err("cancelled".into());
            }
            let mut f = z.by_index(i).map_err(|e| e.to_string())?;
            let name = f.name().unwrap_or_default().to_string();
            if f.size() > MAX_BANK {
                continue;
            }
            b.clear();
            b.reserve(f.size() as usize);
            f.read_to_end(&mut b).map_err(|e| format!("{name}: {e}"))?;
            bin.write_all(&b).map_err(|e| e.to_string())?;
            idx.insert(name, (off, b.len() as u64));
            off += b.len() as u64;
        }
        bin.flush().map_err(|e| e.to_string())?;
        meta.media = idx.len() as u64;
        write(
            &out.join(format::MEDIA_IDX),
            &serde_json::to_vec(&idx).unwrap(),
        )?;
    }

    if timing {
        eprintln!(
            "{}: train {:?}, banks {:?}, indexes {:?}, media and meta {:?}",
            meta.title,
            t_train,
            t_banks - t_train,
            t_index - t_banks,
            t0.elapsed() - t_index
        );
    }
    meta.source = Some(SourceZip {
        size: file_meta.len(),
    });
    meta.bytes = std::fs::read_dir(out)
        .map(|d| {
            d.filter_map(|e| e.ok()?.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0);
    format::write_atomic(
        &out.join(format::META),
        &serde_json::to_vec_pretty(&meta).unwrap(),
    )?;
    Ok(meta)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn frequency_shapes() {
        assert_eq!(parse_freq("5"), Some((String::new(), 5, "5".into())));
        assert_eq!(parse_freq("\"12\""), Some((String::new(), 12, "12".into())));
        assert_eq!(
            parse_freq(r#"{"value":194,"displayValue":"194"}"#),
            Some((String::new(), 194, "194".into()))
        );
        assert_eq!(
            parse_freq(r#"{"value":7}"#),
            Some((String::new(), 7, "7".into()))
        );
        assert_eq!(
            parse_freq(r#"{"reading":"háng","frequency":300}"#),
            Some(("háng".into(), 300, "300".into()))
        );
        assert_eq!(
            parse_freq(r#"{"reading":"xíng","frequency":{"value":12,"displayValue":"12★"}}"#),
            Some(("xíng".into(), 12, "12★".into()))
        );
        assert_eq!(parse_freq(r#"{"oops":1}"#), None);
        assert_eq!(parse_freq("\"many\""), None);
    }

    #[test]
    fn index_fields_and_update_urls() {
        let i = Index::parse(
            r#"{"title":"T","revision":"2026.11.16.3","format":3,"isUpdatable":true,
                "indexUrl":"https://example.org/i.json","downloadUrl":"https://example.org/d.zip"}"#,
        )
        .unwrap();
        assert_eq!(i.title, "T");
        assert_eq!(i.revision, "2026.11.16.3");
        assert!(i.update.is_some());
        // Not updatable, or URLs not valid: imported, without updates.
        for j in [
            r#"{"title":"T","revision":"1","isUpdatable":false}"#,
            r#"{"title":"T","revision":"1","isUpdatable":true,"indexUrl":"ftp://x","downloadUrl":"https://x.org/d"}"#,
            r#"{"title":"T","revision":1}"#,
        ] {
            assert_eq!(Index::parse(j).unwrap().update, None, "{j}");
        }
        assert!(Index::parse(r#"{"revision":"1"}"#).is_err());
        assert!(Index::parse(r#"{"title":"T"}"#).is_err());
    }

    /// A small dictionary zip for tests: entries are (name, bytes).
    pub(crate) fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let f = std::fs::File::create(path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        for (name, data) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
    }

    pub(crate) fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("hanstat-dict-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn refuses_what_is_not_a_dictionary() {
        let d = temp_dir("refuse");
        let p = Progress::default();
        let no = AtomicBool::new(false);
        let z = d.join("x.zip");
        make_zip(&z, &[("readme.txt", b"hi")]);
        assert!(
            import(&z, &d.join("out"), &p, &no)
                .unwrap_err()
                .contains("index.json")
        );
        make_zip(
            &z,
            &[("index.json", br#"{"title":"T","revision":"1","format":3}"#)],
        );
        assert!(
            import(&z, &d.join("out"), &p, &no)
                .unwrap_err()
                .contains("no term")
        );
        make_zip(
            &z,
            &[
                ("index.json", br#"{"title":"T","revision":"1","format":3}"#),
                ("term_bank_1.json", b"[[1,2"),
            ],
        );
        assert!(
            import(&z, &d.join("out"), &p, &no)
                .unwrap_err()
                .contains("term_bank_1.json")
        );
        std::fs::remove_dir_all(&d).ok();
    }
}
