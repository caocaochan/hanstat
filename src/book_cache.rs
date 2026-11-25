//! Exact-file EPUB extraction cache. The text-keyed analysis cache still owns
//! segmentation results and can share them across different EPUB files.

use crate::cache::Cache;
use crate::extract::{self, Book, EncodingChoice, Section, SectionKind};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::Path;

const MAX_BYTES: u64 = 128 * 1024 * 1024;
const VERSION: &str = "1";

#[derive(Deserialize)]
struct Snapshot {
    title: Option<String>,
    sections: Vec<SavedSection>,
    notes: Vec<String>,
    decode_errors: u64,
}

#[derive(Deserialize)]
struct SavedSection {
    label: String,
    href: Option<String>,
    #[serde(with = "SavedKind")]
    kind: SectionKind,
    paragraphs: Vec<String>,
}

#[derive(Deserialize)]
#[serde(remote = "SectionKind")]
enum SavedKind {
    Body,
    Nav,
    NonLinear,
    FrontMatter,
}

pub fn load(path: &Path, encoding: EncodingChoice, cache: Option<&Cache>) -> Result<Book, String> {
    let Some(cache) = cache else {
        return extract::load(path, encoding);
    };
    let bytes = fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    if !bytes.starts_with(b"PK\x03\x04") {
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("epub"))
        {
            return Err("File has .epub extension but is not a ZIP archive.".into());
        }
        return Ok(extract::txt::load(&bytes, encoding));
    }

    let dir = cache.dir().join("epub-text");
    let hash: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let path = dir.join(format!(
        "{VERSION}-{}-{hash}.zst",
        env!("HANSTAT_EXTRACTION_FINGERPRINT")
    ));
    if let Ok(book) = get(&path) {
        if let Ok(file) = File::options().write(true).open(&path) {
            let _ = file.set_modified(std::time::SystemTime::now());
        }
        return Ok(book);
    }
    let book = extract::epub::load(bytes)?;
    if save(&dir, &path, &book).is_ok() {
        trim(&dir);
    }
    Ok(book)
}

fn get(path: &Path) -> io::Result<Book> {
    let file = File::open(path)?;
    let mut input = zstd::stream::read::Decoder::new(file)?.take(MAX_BYTES);
    let mut header = [0; 4];
    input.read_exact(&mut header)?;
    let cover_len = u32::from_le_bytes(header) as usize;
    if cover_len as u64 >= MAX_BYTES {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let cover = if cover_len == 0 {
        None
    } else {
        let mut bytes = Vec::new();
        input
            .by_ref()
            .take(cover_len as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() != cover_len {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        Some(bytes)
    };
    let mut json = Vec::new();
    input.read_to_end(&mut json)?;
    let snapshot: Snapshot = serde_json::from_slice(&json)?;
    Ok(Book {
        title: snapshot.title,
        format: "EPUB",
        sections: snapshot
            .sections
            .into_iter()
            .map(|s| Section {
                label: s.label,
                href: s.href,
                kind: s.kind,
                paragraphs: s.paragraphs,
            })
            .collect(),
        notes: snapshot.notes,
        decode_errors: snapshot.decode_errors,
        cover,
    })
}

fn save(dir: &Path, path: &Path, book: &Book) -> io::Result<()> {
    let cover = book.cover.as_deref().unwrap_or_default();
    let len = u32::try_from(cover.len()).map_err(io::Error::other)?;
    fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    let mut encoder = zstd::stream::write::Encoder::new(file.as_file_mut(), 1)?;
    encoder.include_checksum(true)?;
    let mut output = BufWriter::new(encoder);
    output.write_all(&len.to_le_bytes())?;
    output.write_all(cover)?;
    serde_json::to_writer(&mut output, book)?;
    output.into_inner()?.finish()?;
    file.persist(path)?;
    Ok(())
}

fn trim(dir: &Path) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "zst"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut bytes: u64 = entries.iter().map(|(_, len, _)| len).sum();
    if bytes <= MAX_BYTES {
        return;
    }
    entries.sort_unstable();
    for (_, len, path) in entries {
        if fs::remove_file(path).is_ok() {
            bytes -= len;
        }
        if bytes <= MAX_BYTES {
            break;
        }
    }
}

pub fn clear(cache: &Cache) -> Result<(), String> {
    let dir = cache.dir().join("epub-text");
    match fs::read_dir(&dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "zst") {
                    fs::remove_file(&path)
                        .map_err(|err| format!("Cannot delete {}: {err}", path.display()))?;
                }
            }
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Cannot read {}: {e}", dir.display())),
    }
}
