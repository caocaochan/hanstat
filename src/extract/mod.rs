//! Turning files into a list of sections made of paragraphs.

pub mod epub;
pub mod html;
pub mod txt;

use serde::Serialize;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum SectionKind {
    /// Normal reading-order content.
    Body,
    /// EPUB3 navigation document (table of contents) that is also in the spine.
    Nav,
    /// Spine item marked linear="no" (usually notes, pop-ups).
    NonLinear,
    /// TXT text before the first detected chapter heading.
    FrontMatter,
}

#[derive(Clone, Debug, Serialize)]
pub struct Section {
    pub label: String,
    /// Path inside the EPUB, if any.
    pub href: Option<String>,
    pub kind: SectionKind,
    /// Paragraphs; a paragraph boundary is always a word/sentence boundary.
    pub paragraphs: Vec<String>,
}

impl Section {
    pub fn default_included(&self) -> bool {
        !matches!(self.kind, SectionKind::Nav)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Book {
    pub title: Option<String>,
    pub format: &'static str,
    pub sections: Vec<Section>,
    /// Human-readable notes about how the file was read (encoding, skipped items, ...).
    pub notes: Vec<String>,
    /// Decoding errors (bytes that were not valid in the detected encoding).
    pub decode_errors: u64,
    /// EPUB: the cover image as stored in the book (JPEG, PNG, GIF or WebP).
    #[serde(skip)]
    pub cover: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EncodingChoice {
    #[default]
    Auto,
    Utf8,
    Gb18030,
    Big5,
    /// EUC-KR as browsers read it (the CP949 / Unified Hangul Code superset).
    EucKr,
    Utf16Le,
    Utf16Be,
}

impl EncodingChoice {
    pub const ALL: [EncodingChoice; 7] = [
        Self::Auto,
        Self::Utf8,
        Self::Gb18030,
        Self::Big5,
        Self::EucKr,
        Self::Utf16Le,
        Self::Utf16Be,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto-detect",
            Self::Utf8 => "UTF-8",
            Self::Gb18030 => "GB18030 / GBK",
            Self::Big5 => "Big5",
            Self::EucKr => "EUC-KR / CP949",
            Self::Utf16Le => "UTF-16 LE",
            Self::Utf16Be => "UTF-16 BE",
        }
    }
    pub fn encoding(self) -> Option<&'static encoding_rs::Encoding> {
        Some(match self {
            Self::Auto => return None,
            Self::Utf8 => encoding_rs::UTF_8,
            Self::Gb18030 => encoding_rs::GB18030,
            Self::Big5 => encoding_rs::BIG5,
            Self::EucKr => encoding_rs::EUC_KR,
            Self::Utf16Le => encoding_rs::UTF_16LE,
            Self::Utf16Be => encoding_rs::UTF_16BE,
        })
    }
}

pub fn load(path: &Path, enc: EncodingChoice) -> Result<Book, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let is_zip = bytes.starts_with(b"PK\x03\x04");
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if is_zip || ext == "epub" {
        if !is_zip {
            return Err("File has .epub extension but is not a ZIP archive.".into());
        }
        epub::load(bytes)
    } else {
        Ok(txt::load(&bytes, enc))
    }
}
