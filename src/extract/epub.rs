//! EPUB 2/3 loading: container → OPF → spine order, with TOC labels.

use super::html;
use super::{Book, Section, SectionKind};
use encoding_rs::Encoding;
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};

struct Zip {
    ar: zip::ZipArchive<Cursor<Vec<u8>>>,
    names: Vec<String>,
}

impl Zip {
    fn read(&mut self, name: &str) -> Option<Vec<u8>> {
        let real = if self.names.iter().any(|n| n == name) {
            name.to_owned()
        } else {
            // Some EPUBs reference files with the wrong case.
            self.names
                .iter()
                .find(|n| n.eq_ignore_ascii_case(name))?
                .clone()
        };
        let mut f = self.ar.by_name(&real).ok()?;
        let mut v = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut v).ok()?;
        Some(v)
    }
}

/// Decode an XML/HTML document: BOM, then XML declaration / meta charset, else UTF-8.
pub fn decode_markup(bytes: &[u8]) -> (String, bool) {
    if let Some((enc, n)) = Encoding::for_bom(bytes) {
        let (s, e) = enc.decode_without_bom_handling(&bytes[n..]);
        return (s.into_owned(), e);
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]).to_ascii_lowercase();
    let label = ["encoding=", "charset="].iter().find_map(|k| {
        let i = head.find(k)? + k.len();
        let rest = head[i..].trim_start_matches(['"', '\'', ' ']);
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .unwrap_or(rest.len());
        Some(rest[..end].to_owned())
    });
    let enc = label
        .and_then(|l| Encoding::for_label(l.as_bytes()))
        // A UTF-16 label on a BOM-less ASCII-compatible stream is a lie; HTML spec says use UTF-8.
        .filter(|e| *e != encoding_rs::UTF_16LE && *e != encoding_rs::UTF_16BE)
        .unwrap_or(encoding_rs::UTF_8);
    let (s, e) = enc.decode_without_bom_handling(bytes);
    (s.into_owned(), e)
}

fn dir_of(path: &str) -> &str {
    path.rfind('/').map(|i| &path[..=i]).unwrap_or("")
}

/// Resolve an href relative to a base directory inside the ZIP.
fn resolve(base_dir: &str, href: &str) -> String {
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

fn xml_opts<'a>() -> roxmltree::ParsingOptions<'a> {
    roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    }
}

struct Item {
    href: String,
    media_type: String,
    properties: String,
}

pub fn load(bytes: Vec<u8>) -> Result<Book, String> {
    let ar = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| format!("Not a valid EPUB/ZIP: {e}"))?;
    let names: Vec<String> = ar
        .file_names()
        .filter_map(|n| n.ok().map(|n| n.into_owned()))
        .collect();
    let mut z = Zip { ar, names };
    let mut notes = Vec::new();
    let mut decode_errors = 0u64;

    check_drm(&mut z)?;

    // container.xml → OPF path.
    let opf_path = z
        .read("META-INF/container.xml")
        .and_then(|b| {
            let (s, _) = decode_markup(&b);
            let doc = roxmltree::Document::parse_with_options(&s, xml_opts()).ok()?;
            let p = doc
                .descendants()
                .filter(|n| n.tag_name().name() == "rootfile")
                .find(|n| {
                    n.attribute("media-type")
                        .is_none_or(|m| m == "application/oebps-package+xml")
                })?
                .attribute("full-path")?
                .to_owned();
            Some(p)
        })
        .or_else(|| {
            z.names
                .iter()
                .find(|n| n.to_ascii_lowercase().ends_with(".opf"))
                .cloned()
        })
        .ok_or("No OPF package document found (META-INF/container.xml missing or broken).")?;

    let opf_bytes = z
        .read(&opf_path)
        .ok_or_else(|| format!("Package document {opf_path} missing from archive."))?;
    let (opf_src, _) = decode_markup(&opf_bytes);
    let opf = roxmltree::Document::parse_with_options(&opf_src, xml_opts())
        .map_err(|e| format!("Cannot parse {opf_path}: {e}"))?;
    let opf_dir = dir_of(&opf_path).to_owned();

    let title = opf
        .descendants()
        .find(|n| {
            n.tag_name().name() == "title"
                && n.ancestors().any(|a| a.tag_name().name() == "metadata")
        })
        .and_then(|n| n.text())
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty());

    let mut manifest: HashMap<String, Item> = HashMap::new();
    for n in opf.descendants().filter(|n| n.tag_name().name() == "item") {
        if let (Some(id), Some(href)) = (n.attribute("id"), n.attribute("href")) {
            manifest.insert(
                id.to_owned(),
                Item {
                    href: resolve(&opf_dir, href),
                    media_type: n.attribute("media-type").unwrap_or("").to_owned(),
                    properties: n.attribute("properties").unwrap_or("").to_owned(),
                },
            );
        }
    }
    let spine_node = opf.descendants().find(|n| n.tag_name().name() == "spine");
    let spine: Vec<(String, bool)> = spine_node
        .map(|s| {
            s.children()
                .filter(|n| n.tag_name().name() == "itemref")
                .filter_map(|n| {
                    Some((
                        n.attribute("idref")?.to_owned(),
                        n.attribute("linear") != Some("no"),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    if spine.is_empty() {
        return Err("EPUB spine is empty; nothing to read.".into());
    }

    // TOC labels: NCX (EPUB2) first, then EPUB3 nav.
    let mut labels: HashMap<String, String> = HashMap::new();
    let mut toc_texts: HashSet<String> = HashSet::new();
    if let Some(ncx) = spine_node
        .and_then(|s| s.attribute("toc"))
        .and_then(|id| manifest.get(id))
    {
        let ncx_href = ncx.href.clone();
        if let Some(b) = z.read(&ncx_href) {
            let (s, _) = decode_markup(&b);
            if let Ok(doc) = roxmltree::Document::parse_with_options(&s, xml_opts()) {
                let base = dir_of(&ncx_href).to_owned();
                for np in doc
                    .descendants()
                    .filter(|n| n.tag_name().name() == "navPoint")
                {
                    let label = np
                        .children()
                        .find(|c| c.tag_name().name() == "navLabel")
                        .and_then(|l| l.descendants().find(|c| c.tag_name().name() == "text"))
                        .and_then(|t| t.text())
                        .map(html::collapse);
                    let src = np
                        .children()
                        .find(|c| c.tag_name().name() == "content")
                        .and_then(|c| c.attribute("src"));
                    if let (Some(l), Some(src)) = (label, src)
                        && !l.is_empty()
                    {
                        toc_texts.insert(l.clone());
                        labels.entry(resolve(&base, src)).or_insert(l);
                    }
                }
            }
        }
    }
    let nav_href = manifest
        .values()
        .find(|i| i.properties.split_whitespace().any(|p| p == "nav"))
        .map(|i| i.href.clone());
    if let Some(nav_href) = &nav_href
        && let Some(b) = z.read(nav_href)
    {
        let (s, _) = decode_markup(&b);
        let doc = scraper::Html::parse_document(&s);
        let sel = scraper::Selector::parse("nav a[href]").unwrap();
        let base = dir_of(nav_href).to_owned();
        for a in doc.select(&sel) {
            let l = html::collapse(&a.text().collect::<String>());
            if !l.is_empty() {
                toc_texts.insert(l.clone());
                labels
                    .entry(resolve(&base, a.value().attr("href").unwrap()))
                    .or_insert(l);
            }
        }
    }

    let mut sections = Vec::new();
    let mut seen = HashSet::new();
    let (mut dupes, mut nonlinear, mut non_html, mut missing, mut toc_pages) = (0, 0, 0, 0, 0);
    for (idref, linear) in &spine {
        let Some(item) = manifest.get(idref) else {
            missing += 1;
            continue;
        };
        if !seen.insert(item.href.clone()) {
            dupes += 1;
            continue;
        }
        let is_html = matches!(
            item.media_type.as_str(),
            "application/xhtml+xml" | "text/html"
        ) || item.href.to_ascii_lowercase().ends_with("html")
            || item.href.to_ascii_lowercase().ends_with(".htm");
        if !is_html {
            non_html += 1;
            continue;
        }
        let Some(b) = z.read(&item.href) else {
            missing += 1;
            continue;
        };
        let (src, err) = decode_markup(&b);
        if err {
            decode_errors += src.chars().filter(|&c| c == '\u{FFFD}').count() as u64;
        }
        let ex = html::extract(&src);
        let is_nav = nav_href.as_deref() == Some(item.href.as_str());
        // A hand-made "Contents" page in the spine: most of its paragraphs are TOC labels.
        let is_toc_page = !is_nav && is_toc_like(&ex.paragraphs, &toc_texts);
        if is_toc_page {
            toc_pages += 1;
        }
        let kind = if is_nav || is_toc_page {
            SectionKind::Nav
        } else if !linear {
            nonlinear += 1;
            SectionKind::NonLinear
        } else {
            SectionKind::Body
        };
        let file_name = item
            .href
            .rsplit('/')
            .next()
            .unwrap_or(&item.href)
            .to_owned();
        let label = labels
            .get(&item.href)
            .cloned()
            .or(ex.first_heading)
            .or(ex.title)
            .unwrap_or_else(|| file_name.clone());
        let label: String = if label.chars().count() > 60 {
            label.chars().take(59).chain(['…']).collect()
        } else {
            label
        };
        sections.push(Section {
            label,
            href: Some(item.href.clone()),
            kind,
            paragraphs: ex.paragraphs,
        });
    }

    let spine_set: HashSet<&str> = sections.iter().filter_map(|s| s.href.as_deref()).collect();
    let orphan = manifest
        .values()
        .filter(|i| i.media_type == "application/xhtml+xml" && !spine_set.contains(i.href.as_str()))
        .filter(|i| nav_href.as_deref() != Some(i.href.as_str()))
        .count();

    notes.push(format!(
        "EPUB package {opf_path}: {} spine documents read in reading order.",
        sections.len()
    ));
    if sections
        .iter()
        .any(|s| s.kind == SectionKind::Nav && Some(s.href.as_deref()) == Some(nav_href.as_deref()))
    {
        notes.push(
            "The table-of-contents (nav) document is in the spine; excluded by default.".into(),
        );
    }
    if toc_pages > 0 {
        notes.push(format!(
            "{toc_pages} table-of-contents page(s) detected (most lines are chapter titles); excluded by default so titles are not counted twice."
        ));
    }
    if nonlinear > 0 {
        notes.push(format!("{nonlinear} spine documents are marked non-linear (notes/pop-ups); included by default."));
    }
    if dupes > 0 {
        notes.push(format!(
            "{dupes} duplicate spine references skipped (each document is counted once)."
        ));
    }
    if non_html > 0 {
        notes.push(format!(
            "{non_html} non-HTML spine items (e.g. SVG/images) contain no countable text."
        ));
    }
    if missing > 0 {
        notes.push(format!(
            "{missing} spine items reference files missing from the archive."
        ));
    }
    if orphan > 0 {
        notes.push(format!("{orphan} XHTML documents are in the manifest but not in the spine; not counted (not part of reading order)."));
    }

    Ok(Book {
        title,
        format: "EPUB",
        sections,
        notes,
        decode_errors,
    })
}

fn is_toc_like(paras: &[String], toc: &HashSet<String>) -> bool {
    if paras.len() < 5 || toc.len() < 5 || paras.len() > toc.len() * 2 + 10 {
        return false;
    }
    let hits = paras
        .iter()
        .filter(|p| {
            toc.contains(p.as_str())
                || toc.iter().any(|t| {
                    p.chars().count() <= t.chars().count() + 12
                        && (p.contains(t.as_str()) || t.contains(p.as_str()))
                })
        })
        .count();
    hits * 10 >= paras.len() * 6
}

/// Refuse DRM-encrypted content (font obfuscation is fine and common).
fn check_drm(z: &mut Zip) -> Result<(), String> {
    if z.names.iter().any(|n| n == "META-INF/rights.xml")
        && z.names.iter().any(|n| n == "META-INF/encryption.xml")
    {
        return Err(
            "This EPUB is DRM-protected (Adobe ADEPT rights.xml present); its text cannot be read."
                .into(),
        );
    }
    let Some(b) = z.read("META-INF/encryption.xml") else {
        return Ok(());
    };
    let (s, _) = decode_markup(&b);
    let Ok(doc) = roxmltree::Document::parse_with_options(&s, xml_opts()) else {
        return Ok(());
    };
    for ed in doc
        .descendants()
        .filter(|n| n.tag_name().name() == "EncryptedData")
    {
        let alg = ed
            .descendants()
            .find(|n| n.tag_name().name() == "EncryptionMethod")
            .and_then(|n| n.attribute("Algorithm"))
            .unwrap_or("");
        let uri = ed
            .descendants()
            .find(|n| n.tag_name().name() == "CipherReference")
            .and_then(|n| n.attribute("URI"))
            .unwrap_or("")
            .to_ascii_lowercase();
        let font_obfuscation =
            alg == "http://www.idpf.org/2008/embedding" || alg == "http://ns.adobe.com/pdf/enc#RC";
        if !font_obfuscation && (uri.ends_with("html") || uri.ends_with(".htm")) {
            return Err(
                "This EPUB's text documents are encrypted (DRM); its text cannot be read.".into(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolve_paths() {
        assert_eq!(
            resolve("OEBPS/", "Text/ch%201.xhtml#p1"),
            "OEBPS/Text/ch 1.xhtml"
        );
        assert_eq!(
            resolve("OEBPS/Text/", "../Styles/a.css"),
            "OEBPS/Styles/a.css"
        );
        assert_eq!(resolve("", "/abs.xhtml"), "abs.xhtml");
    }
}
