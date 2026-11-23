//! An imported story as an EPUB 3 book (with an EPUB 2 table of contents
//! for older readers): one chapter file per DuChinese lesson, named by the
//! lesson, in reading order.
//!
//! The bytes depend only on the content: no clock, fixed ZIP times. A
//! story rebuilt with nothing changed is the same file, so its Library id
//! (the file's SHA-256) stays.

use super::data::Text;
use std::io::Write;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

pub struct Book<'a> {
    pub course: u64,
    pub title: &'a str,
    pub description: &'a str,
    /// Where it comes from (the course's page).
    pub source: &'a str,
    pub traditional: bool,
    /// Each paragraph followed by its English.
    pub translations: bool,
    /// The newest chapter's date (`2026-11-20…`), for `dcterms:modified`.
    pub modified: &'a str,
    /// JPEG, PNG or WebP.
    pub cover: Option<&'a [u8]>,
    pub chapters: Vec<Chapter<'a>>,
}

pub struct Chapter<'a> {
    pub lesson: &'a str,
    /// Its table of contents entry.
    pub label: String,
    pub text: &'a Text,
}

/// The chapter file of `lesson`.
pub fn chapter_file(lesson: &str) -> String {
    let safe: String = lesson
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    format!("l{safe}.xhtml")
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            // Not allowed in XML 1.0.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => o.push(c),
        }
    }
    o
}

fn image_type(b: &[u8]) -> Option<(&'static str, &'static str)> {
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(("image/jpeg", "jpg"))
    } else if b.starts_with(b"\x89PNG") {
        Some(("image/png", "png"))
    } else if b.len() > 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some(("image/webp", "webp"))
    } else {
        None
    }
}

/// `2026-11-20T06:00:00.000+01:00` → `2026-11-20T00:00:00Z` (the day is enough).
fn modified(date: &str) -> String {
    let day: String = date.chars().take(10).collect();
    let ok = day.len() == 10
        && day.chars().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }
        });
    format!("{}T00:00:00Z", if ok { day.as_str() } else { "2000-01-01" })
}

const CSS: &str = "h2 { text-align: center; margin: 1em 0 1.2em; }\n\
p { text-indent: 2em; margin: 0 0 0.6em; }\n\
p.tr { text-indent: 0; font-size: 0.8em; color: #777777; margin: -0.3em 0 0.9em; }\n";

fn chapter_xhtml(b: &Book, c: &Chapter) -> String {
    let lang = if b.traditional { "zh-Hant" } else { "zh-Hans" };
    let mut body = String::new();
    let heading = c.text.heading.as_deref().unwrap_or(&c.label);
    body.push_str(&format!("<h2>{}</h2>\n", esc(heading)));
    for p in &c.text.paragraphs {
        body.push_str(&format!("<p>{}</p>\n", esc(&p.text)));
        if b.translations && !p.translation.is_empty() {
            body.push_str(&format!(
                "<p class=\"tr\" lang=\"en\">{}</p>\n",
                esc(&p.translation)
            ));
        }
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n\
<html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"{lang}\" lang=\"{lang}\">\n\
<head><meta charset=\"utf-8\"/><title>{}</title><link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\"/></head>\n\
<body>\n{body}</body>\n</html>\n",
        esc(heading)
    )
}

/// The book as EPUB bytes.
pub fn build(b: &Book) -> Result<Vec<u8>, String> {
    let lang = if b.traditional { "zh-Hant" } else { "zh-Hans" };
    let ident = format!(
        "urn:hanstat:duchinese:{}:{}",
        b.course,
        if b.traditional { "tc" } else { "sc" }
    );
    let cover = b.cover.and_then(|c| image_type(c).map(|t| (c, t)));
    let mut manifest = String::new();
    let mut spine = String::new();
    let mut nav = String::new();
    let mut ncx = String::new();
    for (i, c) in b.chapters.iter().enumerate() {
        let file = chapter_file(c.lesson);
        let id = format!("c{}", i + 1);
        manifest.push_str(&format!(
            "<item id=\"{id}\" href=\"{file}\" media-type=\"application/xhtml+xml\"/>\n"
        ));
        spine.push_str(&format!("<itemref idref=\"{id}\"/>\n"));
        nav.push_str(&format!(
            "<li><a href=\"{file}\">{}</a></li>\n",
            esc(&c.label)
        ));
        ncx.push_str(&format!(
            "<navPoint id=\"n{n}\" playOrder=\"{n}\"><navLabel><text>{}</text></navLabel><content src=\"{file}\"/></navPoint>\n",
            esc(&c.label),
            n = i + 1
        ));
    }
    if let Some((_, (mime, ext))) = cover {
        manifest.push_str(&format!(
            "<item id=\"cover\" href=\"cover.{ext}\" media-type=\"{mime}\" properties=\"cover-image\"/>\n"
        ));
    }
    let opf = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"id\" xml:lang=\"{lang}\">\n\
<metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n\
<dc:identifier id=\"id\">{ident}</dc:identifier>\n\
<dc:title>{title}</dc:title>\n\
<dc:language>{lang}</dc:language>\n\
<dc:publisher>Du Chinese</dc:publisher>\n\
<dc:source>{source}</dc:source>\n\
<dc:description>{desc}</dc:description>\n\
<meta property=\"dcterms:modified\">{modified}</meta>\n\
{cover_meta}</metadata>\n\
<manifest>\n\
<item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>\n\
<item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>\n\
<item id=\"css\" href=\"style.css\" media-type=\"text/css\"/>\n\
{manifest}</manifest>\n\
<spine toc=\"ncx\">\n{spine}</spine>\n\
</package>\n",
        title = esc(b.title),
        source = esc(b.source),
        desc = esc(b.description.trim()),
        modified = modified(b.modified),
        cover_meta = if cover.is_some() {
            "<meta name=\"cover\" content=\"cover\"/>\n"
        } else {
            ""
        },
    );
    let nav_doc = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n\
<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" xml:lang=\"{lang}\" lang=\"{lang}\">\n\
<head><meta charset=\"utf-8\"/><title>{title}</title></head>\n\
<body><nav epub:type=\"toc\" id=\"toc\"><h1>{title}</h1><ol>\n{nav}</ol></nav></body>\n</html>\n",
        title = esc(b.title)
    );
    let ncx_doc = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\">\n\
<head><meta name=\"dtb:uid\" content=\"{ident}\"/></head>\n\
<docTitle><text>{title}</text></docTitle>\n<navMap>\n{ncx}</navMap>\n</ncx>\n",
        title = esc(b.title)
    );
    let container = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\n\
<rootfiles><rootfile full-path=\"OEBPS/content.opf\" media-type=\"application/oebps-package+xml\"/></rootfiles>\n\
</container>\n";

    let e = |e: zip::result::ZipError| format!("Can't write the book: {e}");
    let io = |e: std::io::Error| format!("Can't write the book: {e}");
    let mut z = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let time = DateTime::default();
    let stored = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(time);
    let deflated = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(time);
    // The mimetype first and uncompressed, as EPUB requires.
    z.start_file("mimetype", stored).map_err(e)?;
    z.write_all(b"application/epub+zip").map_err(io)?;
    let mut put = |name: &str, body: &[u8], opts: SimpleFileOptions| -> Result<(), String> {
        z.start_file(name, opts).map_err(e)?;
        z.write_all(body).map_err(io)
    };
    put("META-INF/container.xml", container.as_bytes(), deflated)?;
    put("OEBPS/content.opf", opf.as_bytes(), deflated)?;
    put("OEBPS/nav.xhtml", nav_doc.as_bytes(), deflated)?;
    put("OEBPS/toc.ncx", ncx_doc.as_bytes(), deflated)?;
    put("OEBPS/style.css", CSS.as_bytes(), deflated)?;
    if let Some((bytes, (_, ext))) = cover {
        put(&format!("OEBPS/cover.{ext}"), bytes, stored)?;
    }
    for c in &b.chapters {
        put(
            &format!("OEBPS/{}", chapter_file(c.lesson)),
            chapter_xhtml(b, c).as_bytes(),
            deflated,
        )?;
    }
    Ok(z.finish().map_err(e)?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::duchinese::data::Paragraph;

    fn sample(traditional: bool, translations: bool) -> Vec<u8> {
        let one = Text {
            heading: Some("小猫".into()),
            paragraphs: vec![
                Paragraph {
                    text: "这是一只猫。".into(),
                    translation: "This is a cat.".into(),
                },
                Paragraph {
                    text: "它说：<喵> & 走了。".into(),
                    translation: String::new(),
                },
            ],
        };
        let two = Text {
            heading: None,
            paragraphs: vec![Paragraph {
                text: "第二章的文字。".into(),
                translation: String::new(),
            }],
        };
        build(&Book {
            course: 7,
            title: "A Cat's Day",
            description: "About a cat.",
            source: "https://duchinese.net/lessons/courses/7-a-cat",
            traditional,
            translations,
            modified: "2026-11-20T06:00:00.000+01:00",
            cover: Some(b"\xFF\xD8\xFF\xE0 not really a jpeg"),
            chapters: vec![
                Chapter {
                    lesson: "101",
                    label: "1. 小猫".into(),
                    text: &one,
                },
                Chapter {
                    lesson: "102",
                    label: "2. Chapter 2".into(),
                    text: &two,
                },
            ],
        })
        .unwrap()
    }

    #[test]
    fn same_content_same_bytes() {
        assert_eq!(sample(false, false), sample(false, false));
        assert_ne!(sample(false, false), sample(false, true));
    }

    #[test]
    fn the_analysis_and_the_reader_both_read_it() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("cat.epub");
        std::fs::write(&p, sample(false, true)).unwrap();

        let book = hanstat::extract::load(&p, hanstat::extract::EncodingChoice::Auto).unwrap();
        assert_eq!(book.title.as_deref(), Some("A Cat's Day"));
        assert!(book.cover.is_some(), "the cover is found");
        let text: Vec<String> = book
            .sections
            .iter()
            .flat_map(|s| s.paragraphs.clone())
            .collect();
        assert!(text.iter().any(|p| p == "这是一只猫。"), "{text:?}");
        assert!(
            text.iter().any(|p| p == "它说：<喵> & 走了。"),
            "escaped and read back"
        );
        assert!(
            text.iter().any(|p| p == "This is a cat."),
            "translations included"
        );
        assert_eq!(
            book.sections
                .iter()
                .filter(|s| s.default_included())
                .count(),
            2,
            "one section per chapter, the nav left out"
        );

        let epub = crate::reader::epub::Epub::open(&p).unwrap();
        assert_eq!(epub.spine, ["OEBPS/l101.xhtml", "OEBPS/l102.xhtml"]);
        let labels: Vec<&str> = epub.toc.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(labels, ["1. 小猫", "2. Chapter 2"]);
    }

    #[test]
    fn modified_dates() {
        assert_eq!(
            modified("2026-11-20T06:00:00.000+01:00"),
            "2026-11-20T00:00:00Z"
        );
        assert_eq!(modified("junk"), "2000-01-01T00:00:00Z");
    }
}
