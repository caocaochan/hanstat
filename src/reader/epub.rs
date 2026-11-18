//! The reader's view of an EPUB: its files, reading order and table of
//! contents. (`hanstat::extract::epub` reads the text for counting; it is part
//! of the Library's fingerprint, so the reader keeps its own loader.)

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

pub struct Epub {
    zip: Mutex<zip::ZipArchive<Cursor<Vec<u8>>>>,
    /// Lower-cased name → name in the archive (some books get the case wrong).
    names: HashMap<String, String>,
    pub title: String,
    /// Content documents in reading order (ZIP paths), linear ones only.
    pub spine: Vec<String>,
    pub toc: Vec<TocEntry>,
}

impl Epub {
    pub fn open(path: &std::path::Path) -> Result<Epub, String> {
        let bytes =
            std::fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
        let zip =
            zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("Not an EPUB: {e}"))?;
        let names = zip
            .file_names()
            .filter_map(|n| n.ok())
            .map(|n| (n.to_lowercase(), n.into_owned()))
            .collect();
        let mut epub = Epub {
            zip: Mutex::new(zip),
            names,
            title: String::new(),
            spine: Vec::new(),
            toc: Vec::new(),
        };
        epub.read_package()?;
        Ok(epub)
    }

    /// A file of the archive by its full path.
    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        let real = self.names.get(&name.to_lowercase())?;
        let mut zip = self.zip.lock().unwrap_or_else(|e| e.into_inner());
        let mut f = zip.by_name(real).ok()?;
        let mut v = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut v).ok()?;
        Some(v)
    }

    pub fn read_text(&self, name: &str) -> Option<String> {
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
            .find(|n| n.tag_name().name() == "title")
            .and_then(|n| n.text())
            .unwrap_or("")
            .trim()
            .to_owned();
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
