//! Visible-text extraction from (X)HTML content documents.
//!
//! Rules:
//! * Only `<body>` content; `<head>`, `<script>`, `<style>`, `<template>` skipped.
//! * Ruby annotations (`<rt>`, `<rp>`, `<rtc>`) skipped — they are pinyin/zhuyin glosses, not text.
//! * Elements with the `hidden` attribute or inline `display:none` are skipped.
//! * Block-level elements and `<br>` end a paragraph, so words never join across them.
//!   Inline elements (`<span>`, `<a>`, `<em>`, ...) never split text.
//! * Whitespace follows CSS: runs collapse to one space, and a source line break
//!   between two East-Asian wide characters is removed entirely.
//! * Images' alt text is not counted.

use crate::chars::is_wide;
use scraper::{Html, Node};

const BLOCK: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "body",
    "br",
    "caption",
    "center",
    "dd",
    "details",
    "dialog",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "li",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "ul",
    "img",
];
const SKIP: &[&str] = &[
    "head", "script", "style", "template", "rt", "rp", "rtc", "title", "desc", "metadata",
];

pub struct Extracted {
    pub paragraphs: Vec<String>,
    /// First h1–h6 text, used as a fallback section label.
    pub first_heading: Option<String>,
    /// `<title>`, last-resort label.
    pub title: Option<String>,
}

pub fn extract(src: &str) -> Extracted {
    let doc = Html::parse_document(&expand_self_closing(src));
    let mut out = Extracted {
        paragraphs: Vec::new(),
        first_heading: None,
        title: None,
    };

    for node in doc.tree.root().descendants() {
        if let Node::Element(e) = node.value()
            && e.name() == "title"
            && out.title.is_none()
        {
            let t: String = node
                .descendants()
                .filter_map(|n| n.value().as_text().map(|t| t.to_string()))
                .collect();
            let t = collapse(&t);
            if !t.is_empty() {
                out.title = Some(t);
            }
        }
    }

    let mut buf = String::new();
    let mut heading: Option<String> = None;
    walk(
        doc.tree.root(),
        &mut buf,
        &mut out.paragraphs,
        &mut heading,
        false,
    );
    flush(&mut buf, &mut out.paragraphs);
    out.first_heading = heading;
    out
}

/// HTML void elements: `<x/>` and `<x>` mean the same thing for these.
const VOID: &[&str] = &[
    "area", "base", "basefont", "bgsound", "br", "col", "embed", "frame", "hr", "img", "input",
    "keygen", "link", "meta", "param", "source", "track", "wbr",
];

/// Rewrite XML self-closing tags of non-void elements (`<title/>`, `<a id="x"/>`)
/// as an explicit start/end pair. EPUB content documents are XHTML, but the HTML
/// parser ignores the `/` on non-void elements, so `<title/>` would open an RCDATA
/// element that swallows the whole rest of the document.
fn expand_self_closing(src: &str) -> std::borrow::Cow<'_, str> {
    if !src.contains("/>") {
        return src.into();
    }
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 64);
    let mut copied = 0; // src[..copied] is already in `out`
    let mut i = 0;
    while let Some(off) = src[i..].find('<') {
        let lt = i + off;
        let rest = &src[lt..];
        // Comments and CDATA may contain anything; skip them whole.
        let skip_to = if rest.starts_with("<!--") {
            Some(rest.find("-->").map_or(src.len(), |e| lt + e + 3))
        } else if rest.starts_with("<![CDATA[") {
            Some(rest.find("]]>").map_or(src.len(), |e| lt + e + 3))
        } else if !b.get(lt + 1).is_some_and(u8::is_ascii_alphabetic) {
            Some(lt + 1)
        } else {
            None
        };
        if let Some(j) = skip_to {
            i = j;
            continue;
        }
        let name_end = rest[1..]
            .find(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
            .map_or(src.len(), |e| lt + 1 + e);
        let name = &src[lt + 1..name_end];
        // Find the closing '>' outside quoted attribute values.
        let mut j = name_end;
        let mut quote = None;
        while j < b.len() {
            match (quote, b[j]) {
                (Some(q), c) if c == q => quote = None,
                (None, b'"' | b'\'') => quote = Some(b[j]),
                (None, b'>') => break,
                _ => {}
            }
            j += 1;
        }
        if j >= b.len() {
            break;
        }
        if b[j - 1] == b'/' && j > name_end && !VOID.contains(&name.to_ascii_lowercase().as_str()) {
            out.push_str(&src[copied..j - 1]);
            out.push_str("></");
            out.push_str(name);
            out.push('>');
            copied = j + 1;
        }
        i = j + 1;
    }
    if copied == 0 {
        return src.into();
    }
    out.push_str(&src[copied..]);
    out.into()
}

fn walk(
    node: ego_tree::NodeRef<'_, Node>,
    buf: &mut String,
    paras: &mut Vec<String>,
    heading: &mut Option<String>,
    in_svg: bool,
) {
    for child in node.children() {
        match child.value() {
            Node::Text(t) => {
                if !in_svg {
                    buf.push_str(t);
                }
            }
            Node::Element(e) => {
                let name = e.name();
                if SKIP.contains(&name)
                    || e.attr("hidden").is_some()
                    || is_display_none(e.attr("style"))
                {
                    continue;
                }
                let block = BLOCK.contains(&name);
                if block {
                    flush(buf, paras);
                }
                // In SVG only <text>/<tspan> content is visible.
                let svg_here = in_svg || name == "svg";
                let svg_text = svg_here && matches!(name, "text" | "tspan" | "textPath");
                if svg_text {
                    let t: String = child
                        .descendants()
                        .filter_map(|n| n.value().as_text().map(|t| t.to_string()))
                        .collect();
                    buf.push_str(&t);
                } else {
                    let before = paras.len();
                    walk(child, buf, paras, heading, svg_here);
                    if heading.is_none() && matches!(name, "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
                    {
                        flush(buf, paras);
                        let h: String = paras[before..].join(" ");
                        if !h.is_empty() {
                            *heading = Some(h);
                        }
                    }
                }
                if block {
                    flush(buf, paras);
                }
            }
            _ => {}
        }
    }
}

fn is_display_none(style: Option<&str>) -> bool {
    style.is_some_and(|s| {
        let s: String = s
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        s.contains("display:none") || s.contains("visibility:hidden")
    })
}

fn flush(buf: &mut String, paras: &mut Vec<String>) {
    let p = collapse(buf);
    if !p.is_empty() {
        paras.push(p);
    }
    buf.clear();
}

/// CSS-style whitespace collapsing (white-space: normal), with the CSS Text 3
/// segment-break transformation for East Asian wide characters.
/// U+00A0 and U+3000 are not collapsible and are kept as-is.
pub fn collapse(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let is_ws = |c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0C');
    while i < chars.len() {
        let c = chars[i];
        if is_ws(c) {
            let mut has_break = false;
            while i < chars.len() && is_ws(chars[i]) {
                has_break |= matches!(chars[i], '\n' | '\r');
                i += 1;
            }
            let prev = out.chars().last();
            let next = chars.get(i).copied();
            match (prev, next) {
                (None, _) | (_, None) => {}
                (Some(p), Some(n)) if has_break && is_wide(p) && is_wide(n) => {}
                _ => out.push(' '),
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruby_blocks_and_breaks() {
        let html = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>T</title><style>p{}</style></head>
<body><h1>第一章</h1><p>他<span class="x">们</span>是<ruby>漢<rp>(</rp><rt>hàn</rt><rp>)</rp></ruby>字
测试</p><div>甲<br/>乙</div><p hidden="">隐藏</p><p style="display: none">藏</p><img alt="图片文字"/>
<p>&#x4E2D;&amp;&nbsp;文</p></body></html>"#;
        let e = extract(html);
        assert_eq!(
            e.paragraphs,
            ["第一章", "他们是漢字测试", "甲", "乙", "中&\u{a0}文"]
        );
        assert_eq!(e.first_heading.as_deref(), Some("第一章"));
        assert_eq!(e.title.as_deref(), Some("T"));
    }

    #[test]
    fn xhtml_self_closing() {
        // Readmoo/Pubu (Sigil) chapters have <title/>; HTML parsing would make the
        // whole body title text.
        let html = r#"<?xml version='1.0' encoding='utf-8'?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title/><link href="a.css" rel="stylesheet"/></head>
<body><h1>壹、黑與白</h1><a id="x"/><p class="a">濕熱的氣息</p><div/><p title="a/>b">第二段<br/>行</p><!-- <script/> --></body></html>"#;
        let e = extract(html);
        assert_eq!(e.paragraphs, ["壹、黑與白", "濕熱的氣息", "第二段", "行"]);
        assert_eq!(e.first_heading.as_deref(), Some("壹、黑與白"));
        assert_eq!(e.title, None);
        assert_eq!(
            expand_self_closing(r#"<title/><a  id="q/>"/><br/><svg:rect x="1"/><!--<b/>-->"#),
            r#"<title></title><a  id="q/>"></a><br/><svg:rect x="1"></svg:rect><!--<b/>-->"#
        );
    }

    #[test]
    fn whitespace() {
        assert_eq!(collapse("  中文\n  文字 "), "中文文字");
        assert_eq!(collapse("hello\n world"), "hello world");
        assert_eq!(collapse("中 文"), "中 文");
    }
}
