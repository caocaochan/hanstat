//! The reader's view of an EPUB: its files, reading order and table of
//! contents. (`hanstat::extract::epub` reads the text for counting; it is part
//! of the Library's fingerprint, so the reader keeps its own loader.)
//!
//! A plain-text book is read with the analysis's own TXT loader (encoding,
//! paragraphs, chapter headings) and turned into one XHTML document per
//! chapter, so the rest of the reader sees an EPUB like any other.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::sync::Mutex;

/// One entry of the table of contents.
#[derive(Clone, Debug)]
pub struct TocEntry {
    pub label: String,
    pub depth: usize,
    /// Index into [`Epub::spine`].
    pub spine: usize,
    /// The element id the entry points at, if any.
    pub fragment: Option<String>,
}

struct TextBook {
    title: String,
    /// Paragraphs per chapter, and whether the first paragraph is a heading.
    chapters: Vec<(Vec<String>, bool)>,
}

enum Files {
    Zip(Mutex<zip::ZipArchive<Cursor<Vec<u8>>>>),
    /// A plain-text book's chapters, turned into XHTML on demand.
    Text(TextBook),
}

fn text_chapter_index(name: &str) -> Option<usize> {
    name.strip_prefix("text/")?
        .strip_suffix(".xhtml")?
        .parse::<usize>()
        .ok()?
        .checked_sub(1)
}

pub struct Epub {
    files: Files,
    /// Lower-cased name → name in the archive (some books get the case wrong).
    names: HashMap<String, String>,
    pub title: String,
    pub author: String,
    /// Content documents in reading order (ZIP paths), linear ones only.
    pub spine: Vec<String>,
    pub toc: Vec<TocEntry>,
}

impl Epub {
    pub fn open(path: &std::path::Path) -> Result<Epub, String> {
        let bytes =
            std::fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
        // As `hanstat::extract::load` decides.
        let is_zip = bytes.starts_with(b"PK\x03\x04");
        let epub_ext = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("epub"));
        if !is_zip && !epub_ext {
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            return Ok(Self::from_text(&bytes, &stem));
        }
        let zip =
            zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("Not an EPUB: {e}"))?;
        let names = zip
            .file_names()
            .filter_map(|n| n.ok())
            .map(|n| (n.to_lowercase(), n.into_owned()))
            .collect();
        let mut epub = Epub {
            files: Files::Zip(Mutex::new(zip)),
            names,
            title: String::new(),
            author: String::new(),
            spine: Vec::new(),
            toc: Vec::new(),
        };
        epub.read_package()?;
        if epub.title.is_empty()
            && let Some(stem) = path.file_stem()
        {
            epub.title = stem.to_string_lossy().trim().to_owned();
        }
        Ok(epub)
    }

    /// A file of the archive by its full path.
    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        let real = self.names.get(&name.to_lowercase())?;
        match &self.files {
            Files::Zip(zip) => {
                let mut zip = zip.lock().unwrap_or_else(|e| e.into_inner());
                let mut f = zip.by_name(real).ok()?;
                let mut v = Vec::with_capacity(f.size() as usize);
                f.read_to_end(&mut v).ok()?;
                Some(v)
            }
            Files::Text(tb) => {
                let (paras, heading) = tb.chapters.get(text_chapter_index(real)?)?;
                Some(chapter_xhtml(&tb.title, paras, *heading).into_bytes())
            }
        }
    }

    /// A plain-text book (the file's bytes; `stem` its file name without the
    /// extension, the title) as chapter documents. Each detected chapter is
    /// one document, its heading drawn as one; a chapter longer than
    /// [`PART_CHARS`] is cut at paragraphs into several, so that no page
    /// layout has to take in a whole book that has no headings.
    pub fn from_text(bytes: &[u8], stem: &str) -> Epub {
        const PART_CHARS: usize = 20_000;
        let book = hanstat::extract::txt::load(bytes, hanstat::extract::EncodingChoice::Auto);
        let title = stem.trim().to_owned();
        let mut chapters: Vec<(Vec<String>, bool)> = Vec::new();
        let mut spine: Vec<String> = Vec::new();
        let mut toc = Vec::new();
        let mut doc = |spine: &mut Vec<String>, paragraphs: Vec<String>, heading: bool| {
            let name = format!("text/{:05}.xhtml", spine.len() + 1);
            chapters.push((paragraphs, heading));
            spine.push(name);
        };
        for section in book.sections {
            // The TXT loader keeps a chapter's heading line as its first paragraph.
            let heading = section.kind == hanstat::extract::SectionKind::Body
                && section.paragraphs.first() == Some(&section.label);
            toc.push(TocEntry {
                label: section.label,
                depth: 0,
                spine: spine.len(),
                fragment: None,
            });
            let total_paras = section.paragraphs.len();
            let mut chunk = Vec::new();
            let mut chars = 0usize;
            let mut first_chunk = true;
            for (i, p) in section.paragraphs.into_iter().enumerate() {
                chars += p.chars().count();
                chunk.push(p);
                if chars >= PART_CHARS && i + 1 < total_paras {
                    doc(
                        &mut spine,
                        std::mem::take(&mut chunk),
                        heading && first_chunk,
                    );
                    first_chunk = false;
                    chars = 0;
                }
            }
            if !chunk.is_empty() {
                doc(&mut spine, chunk, heading && first_chunk);
            }
        }
        if spine.is_empty() {
            doc(&mut spine, Vec::new(), false);
        }
        // The only entry of a book without headings says nothing.
        if toc.len() == 1 {
            toc.clear();
        }
        let names = spine
            .iter()
            .map(|n| (n.to_lowercase(), n.clone()))
            .collect();
        Epub {
            files: Files::Text(TextBook {
                title: title.clone(),
                chapters,
            }),
            names,
            title,
            author: String::new(),
            spine,
            toc,
        }
    }

    pub fn read_text(&self, name: &str) -> Option<String> {
        if let Files::Text(tb) = &self.files {
            let real = self.names.get(&name.to_lowercase())?;
            let (paras, heading) = tb.chapters.get(text_chapter_index(real)?)?;
            return Some(chapter_xhtml(&tb.title, paras, *heading));
        }
        self.read(name)
            .map(|b| hanstat::extract::epub::decode_markup(&b).0)
    }

    fn read_package(&mut self) -> Result<(), String> {
        let container = self
            .read_text("META-INF/container.xml")
            .ok_or("No META-INF/container.xml")?;
        let doc = roxmltree::Document::parse_with_options(&container, xml_opts())
            .map_err(|e| format!("container.xml: {e}"))?;
        let opf_path = doc
            .descendants()
            .find(|n| n.tag_name().name() == "rootfile")
            .and_then(|n| n.attribute("full-path"))
            .ok_or("No rootfile in container.xml")?
            .to_owned();
        let opf = self
            .read_text(&opf_path)
            .ok_or("Cannot read the package document")?;
        let doc = roxmltree::Document::parse_with_options(&opf, xml_opts())
            .map_err(|e| format!("{opf_path}: {e}"))?;
        let base = dir_of(&opf_path);
        self.title = doc
            .descendants()
            .find(|n| {
                n.tag_name().name() == "title"
                    && n.ancestors().any(|a| a.tag_name().name() == "metadata")
            })
            .or_else(|| doc.descendants().find(|n| n.tag_name().name() == "title"))
            .and_then(|n| n.text())
            .unwrap_or("")
            .trim()
            .to_owned();
        self.author = extract_author(&doc);
        // id → (path, media type, properties)
        let mut manifest: HashMap<&str, (String, &str, &str)> = HashMap::new();
        for n in doc.descendants().filter(|n| n.tag_name().name() == "item") {
            if let (Some(id), Some(href)) = (n.attribute("id"), n.attribute("href")) {
                manifest.insert(
                    id,
                    (
                        resolve(base, href),
                        n.attribute("media-type").unwrap_or(""),
                        n.attribute("properties").unwrap_or(""),
                    ),
                );
            }
        }
        let spine_node = doc.descendants().find(|n| n.tag_name().name() == "spine");
        for n in doc
            .descendants()
            .filter(|n| n.tag_name().name() == "itemref")
        {
            if n.attribute("linear") == Some("no") {
                continue;
            }
            if let Some((path, mt, _)) = n.attribute("idref").and_then(|id| manifest.get(id))
                && (mt.contains("html") || mt.is_empty())
            {
                self.spine.push(path.clone());
            }
        }
        if self.spine.is_empty() {
            return Err("The book has no pages".into());
        }
        let index: HashMap<&str, usize> = self
            .spine
            .iter()
            .enumerate()
            .map(|(i, p)| (p.as_str(), i))
            .collect();
        let nav = manifest
            .values()
            .find(|(_, _, props)| props.split_whitespace().any(|p| p == "nav"))
            .map(|(p, _, _)| p.clone());
        let mut toc = Vec::new();
        if let Some(nav) = nav
            && let Some(text) = self.read_text(&nav)
        {
            toc = nav_toc(&text, dir_of(&nav), &index);
        }
        if toc.is_empty() {
            let ncx = spine_node
                .and_then(|s| s.attribute("toc"))
                .and_then(|id| manifest.get(id))
                .map(|(p, _, _)| p.clone())
                .or_else(|| {
                    manifest
                        .values()
                        .find(|(_, mt, _)| *mt == "application/x-dtbncx+xml")
                        .map(|(p, _, _)| p.clone())
                });
            if let Some(ncx) = ncx
                && let Some(text) = self.read_text(&ncx)
            {
                toc = ncx_toc(&text, dir_of(&ncx), &index);
            }
        }
        self.toc = toc;
        Ok(())
    }
}

/// Extract the book's author(s) from OPF `<metadata>` `<dc:creator>` elements.
/// Prefers primary authors (`role="aut"` or unspecified role) over translators
/// or editors, falling back to any non-empty `<creator>` if none match.
fn extract_author(doc: &roxmltree::Document<'_>) -> String {
    let mut refined_roles: HashMap<&str, String> = HashMap::new();
    for meta in doc.descendants().filter(|n| {
        n.tag_name().name() == "meta"
            && n.attribute("property") == Some("role")
            && n.ancestors().any(|a| a.tag_name().name() == "metadata")
    }) {
        if let (Some(refines), Some(role)) = (meta.attribute("refines"), meta.text()) {
            let id = refines.trim().trim_start_matches('#');
            let role = role.trim().to_ascii_lowercase();
            if !id.is_empty() && !role.is_empty() {
                refined_roles.insert(id, role);
            }
        }
    }

    let mut primary = Vec::new();
    let mut all = Vec::new();
    for n in doc.descendants().filter(|n| {
        n.tag_name().name() == "creator" && n.ancestors().any(|a| a.tag_name().name() == "metadata")
    }) {
        let Some(name) = n.text().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        if !all.iter().any(|s: &String| s == name) {
            all.push(name.to_owned());
        }
        let opf_role = n
            .attributes()
            .find(|a| a.name() == "role")
            .map(|a| a.value().trim().to_ascii_lowercase());
        let role = opf_role.or_else(|| {
            n.attribute("id")
                .and_then(|id| refined_roles.get(id))
                .cloned()
        });
        if role.as_deref().is_none_or(|r| r == "aut") && !primary.iter().any(|s: &String| s == name)
        {
            primary.push(name.to_owned());
        }
    }
    let chosen = if primary.is_empty() { all } else { primary };
    if chosen.is_empty() {
        return String::new();
    }
    let cjk = chosen
        .iter()
        .any(|s| s.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)));
    chosen.join(if cjk { "、" } else { ", " })
}

/// One chapter of a plain-text book as XHTML: paragraphs indented by two
/// characters, the first drawn as a heading when `heading`.
fn chapter_xhtml(title: &str, paragraphs: &[String], heading: bool) -> String {
    fn escape(s: &str, out: &mut String) {
        for c in s.chars() {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                c => out.push(c),
            }
        }
    }
    let mut out =
        String::with_capacity(paragraphs.iter().map(|p| p.len() + 8).sum::<usize>() + 400);
    out.push_str(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>",
    );
    escape(title, &mut out);
    out.push_str(
        "</title><style>p { margin: 0 0 0.4em; text-indent: 2em } \
         h2 { font-size: 1.3em; margin: 0.5em 0 1.5em; text-align: center }</style></head><body>\n",
    );
    for (i, p) in paragraphs.iter().enumerate() {
        let tag = if heading && i == 0 { "h2" } else { "p" };
        out.push('<');
        out.push_str(tag);
        out.push('>');
        escape(p, &mut out);
        out.push_str("</");
        out.push_str(tag);
        out.push_str(">\n");
    }
    out.push_str("</body></html>\n");
    out
}

fn xml_opts<'a>() -> roxmltree::ParsingOptions<'a> {
    roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    }
}

pub fn dir_of(path: &str) -> &str {
    path.rfind('/').map(|i| &path[..=i]).unwrap_or("")
}

/// `href` (relative to the directory `base_dir`, fragment dropped) as a ZIP path.
pub fn resolve(base_dir: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or("");
    let href = percent_encoding::percent_decode_str(href).decode_utf8_lossy();
    let joined = if let Some(abs) = href.strip_prefix('/') {
        abs.to_owned()
    } else {
        format!("{base_dir}{href}")
    };
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// The fragment of an href, percent-decoded.
pub fn fragment(href: &str) -> Option<String> {
    let f = href.split_once('#')?.1;
    (!f.is_empty()).then(|| {
        percent_encoding::percent_decode_str(f)
            .decode_utf8_lossy()
            .into_owned()
    })
}

/// EPUB 3 navigation document: the `toc` nav's nested lists.
fn nav_toc(text: &str, base: &str, index: &HashMap<&str, usize>) -> Vec<TocEntry> {
    let doc = scraper::Html::parse_document(&super::markup::expand_self_closing(text));
    let navs: Vec<_> = doc
        .tree
        .root()
        .descendants()
        .filter_map(scraper::ElementRef::wrap)
        .filter(|e| e.value().name() == "nav")
        .collect();
    let Some(nav) = navs
        .iter()
        .find(|n| {
            n.value()
                .attrs()
                .any(|(k, v)| k.contains("type") && v.split_whitespace().any(|t| t == "toc"))
        })
        .or(navs.first())
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    fn walk(
        e: scraper::ElementRef,
        depth: usize,
        base: &str,
        index: &HashMap<&str, usize>,
        out: &mut Vec<TocEntry>,
    ) {
        for c in e.children().filter_map(scraper::ElementRef::wrap) {
            match c.value().name() {
                "li" => {
                    if let Some(a) = c
                        .children()
                        .filter_map(scraper::ElementRef::wrap)
                        .find(|x| matches!(x.value().name(), "a" | "span"))
                    {
                        let label: String = a
                            .text()
                            .collect::<String>()
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ");
                        if let Some(href) = a.value().attr("href")
                            && let Some(&spine) = index.get(resolve(base, href).as_str())
                            && !label.is_empty()
                        {
                            out.push(TocEntry {
                                label,
                                depth,
                                spine,
                                fragment: fragment(href),
                            });
                        }
                    }
                    walk(c, depth + 1, base, index, out);
                }
                "ol" | "ul" => walk(c, depth, base, index, out),
                _ => {}
            }
        }
    }
    for list in nav
        .children()
        .filter_map(scraper::ElementRef::wrap)
        .filter(|c| matches!(c.value().name(), "ol" | "ul"))
    {
        walk(list, 0, base, index, &mut out);
    }
    out
}

/// EPUB 2 NCX: navPoints, nested.
fn ncx_toc(text: &str, base: &str, index: &HashMap<&str, usize>) -> Vec<TocEntry> {
    let Ok(doc) = roxmltree::Document::parse_with_options(text, xml_opts()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    fn walk(
        n: roxmltree::Node,
        depth: usize,
        base: &str,
        index: &HashMap<&str, usize>,
        out: &mut Vec<TocEntry>,
    ) {
        for c in n.children().filter(|c| c.tag_name().name() == "navPoint") {
            let label = c
                .descendants()
                .find(|x| x.tag_name().name() == "text")
                .and_then(|x| x.text())
                .unwrap_or("")
                .trim()
                .to_owned();
            if let Some(src) = c
                .children()
                .find(|x| x.tag_name().name() == "content")
                .and_then(|x| x.attribute("src"))
                && let Some(&spine) = index.get(resolve(base, src).as_str())
                && !label.is_empty()
            {
                out.push(TocEntry {
                    label,
                    depth,
                    spine,
                    fragment: fragment(src),
                });
            }
            walk(c, depth + 1, base, index, out);
        }
    }
    if let Some(map) = doc.descendants().find(|n| n.tag_name().name() == "navMap") {
        walk(map, 0, base, index, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_book_chapters() {
        let text = "书名\n作者\n\n第一章 起\n　　他说：“<你好> & 再见。”\n　　第二段。\n第2章 承\n　　正文二。\n";
        let e = Epub::from_text(text.as_bytes(), "书名");
        assert_eq!(e.title, "书名");
        assert_eq!(e.spine.len(), 3);
        let labels: Vec<_> = e.toc.iter().map(|t| (t.label.as_str(), t.spine)).collect();
        assert_eq!(
            labels,
            [("Front matter", 0), ("第一章 起", 1), ("第2章 承", 2)]
        );
        let front = e.read_text(&e.spine[0]).unwrap();
        assert!(front.contains("<p>书名</p>") && !front.contains("<h2>"));
        let one = e.read_text(&e.spine[1]).unwrap();
        assert!(one.contains("<h2>第一章 起</h2>"), "{one}");
        assert!(
            one.contains("<p>他说：“&lt;你好&gt; &amp; 再见。”</p>"),
            "{one}"
        );
        assert!(one.contains("<p>第二段。</p>"));
        // Laid out like any EPUB chapter (which counts letters and digits).
        let ch = super::super::tree::build(&e, &e.spine[1], &mut Default::default());
        assert_eq!(ch.text_len, "第一章起他说你好再见第二段".chars().count());
    }

    #[test]
    fn long_text_chapter_is_cut_at_paragraphs() {
        let para = "这是一个很长的段落。".repeat(500); // 5,000 characters
        let text = format!("第一章 长\n{}", format!("{para}\n").repeat(9));
        let e = Epub::from_text(text.as_bytes(), "长");
        // 9 × 5,000 characters in parts of at least 20,000: 4 + 4 + 1.
        assert_eq!(e.spine.len(), 3);
        assert!(e.toc.is_empty(), "one chapter needs no contents");
        assert!(e.read_text(&e.spine[0]).unwrap().contains("<h2>"));
        assert!(!e.read_text(&e.spine[1]).unwrap().contains("<h2>"));
        let paras: usize = e
            .spine
            .iter()
            .map(|p| e.read_text(p).unwrap().matches("<p>").count())
            .sum();
        assert_eq!(paras, 9);
    }

    #[test]
    fn empty_text_has_a_page() {
        let e = Epub::from_text(b"", "x");
        assert_eq!(e.spine.len(), 1);
        assert!(e.read_text(&e.spine[0]).is_some());
    }

    #[test]
    fn extracts_author_from_opf_metadata() {
        let opf = r##"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="3.0">
  <metadata>
    <dc:title>活着</dc:title>
    <dc:creator id="c1">余华</dc:creator>
    <meta refines="#c1" property="role">aut</meta>
    <dc:creator id="c2">某译者</dc:creator>
    <meta refines="#c2" property="role">trl</meta>
  </metadata>
</package>"##;
        let doc = roxmltree::Document::parse_with_options(opf, xml_opts()).unwrap();
        assert_eq!(extract_author(&doc), "余华");

        let multi = r#"<package xmlns:dc="http://purl.org/dc/elements/1.1/">
  <metadata>
    <dc:creator>鲁迅</dc:creator>
    <dc:creator>周作人</dc:creator>
  </metadata>
</package>"#;
        let doc = roxmltree::Document::parse_with_options(multi, xml_opts()).unwrap();
        assert_eq!(extract_author(&doc), "鲁迅、周作人");

        let latin = r#"<package xmlns:dc="http://purl.org/dc/elements/1.1/">
  <metadata>
    <dc:creator>Jane Austen</dc:creator>
    <dc:creator>Someone Else</dc:creator>
  </metadata>
</package>"#;
        let doc = roxmltree::Document::parse_with_options(latin, xml_opts()).unwrap();
        assert_eq!(extract_author(&doc), "Jane Austen, Someone Else");
    }

    /// A real TXT from `HANSTAT_TEST_TXT`: chapters and their text agree
    /// with the analysis's loader, and how long opening takes (it is done
    /// on the UI thread). Skipped without one.
    #[test]
    fn real_text_book() {
        let Some(p) = std::env::var_os("HANSTAT_TEST_TXT") else {
            return;
        };
        let p = std::path::Path::new(&p);
        let t = std::time::Instant::now();
        let e = Epub::open(p).unwrap();
        let open = t.elapsed();
        let book = hanstat::extract::load(p, hanstat::extract::EncodingChoice::Auto).unwrap();
        assert_eq!(e.toc.len(), book.sections.len());
        let t = std::time::Instant::now();
        let mut sheets = Default::default();
        let reader: usize = e
            .spine
            .iter()
            .map(|s| super::super::tree::build(&e, s, &mut sheets).text_len)
            .sum();
        let build = t.elapsed();
        let analysis: usize = book
            .sections
            .iter()
            .flat_map(|s| &s.paragraphs)
            .map(|p| p.chars().filter(|c| c.is_alphanumeric()).count())
            .sum();
        eprintln!(
            "{}: open {open:?}, {} documents, {} toc entries, {reader} characters laid out in {build:?} (analysis {analysis})",
            p.display(),
            e.spine.len(),
            e.toc.len(),
        );
        assert_eq!(reader, analysis);
    }
}
