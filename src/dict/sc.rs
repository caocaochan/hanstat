//! A glossary (Yomitan's JSON definitions) as HTML for the popup, the way
//! Yomitan builds its DOM from structured content: elements from a fixed set
//! of tags, `data` entries as `data-sc-<key>` attributes (what dictionaries'
//! `styles.css` select on), `style` objects as inline CSS, `?query=` links.
//! Written from the format's JSON schema (`dictionary-term-bank-v3-schema`).

use serde_json::Value;
use std::fmt::Write;

const CONTAINER: &[&str] = &[
    "ruby", "rt", "rp", "table", "thead", "tbody", "tfoot", "tr", "td", "th", "span", "div", "ol",
    "ul", "li", "details", "summary",
];

pub fn escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

/// `fooBar` as `foo-bar` (what the DOM's `dataset.scFooBar` is called as an attribute).
fn kebab(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        if c.is_ascii_uppercase() {
            o.push('-');
            o.push(c.to_ascii_lowercase());
        } else {
            o.push(c);
        }
    }
    o
}

fn safe_name(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A structured-content `style` object as CSS declarations.
fn style(v: &Value, out: &mut String) {
    let Some(o) = v.as_object() else { return };
    for (k, val) in o {
        let value = match val {
            Value::String(s) => s.clone(),
            // Numbers are ems (Yomitan: `marginTop: 1` is `1em`).
            Value::Number(n) => format!("{n}em"),
            Value::Array(a) => a
                .iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            _ => continue,
        };
        if !safe_name(k) || value.contains([';', '{', '}', '"', '<']) {
            continue;
        }
        let _ = write!(out, "{}:{};", kebab(k), value);
    }
}

fn attrs(o: &serde_json::Map<String, Value>, out: &mut String) {
    if let Some(d) = o.get("data").and_then(|d| d.as_object()) {
        for (k, v) in d {
            if let Some(v) = v.as_str()
                && safe_name(k)
            {
                let _ = write!(out, " data-sc-{}=\"", kebab(k));
                escape(v, out);
                out.push('"');
            }
        }
    }
    for k in ["lang", "title"] {
        if let Some(v) = o.get(k).and_then(|v| v.as_str()) {
            let _ = write!(out, " {k}=\"");
            escape(v, out);
            out.push('"');
        }
    }
    if let Some(s) = o.get("style") {
        let mut css = String::new();
        style(s, &mut css);
        if !css.is_empty() {
            out.push_str(" style=\"");
            escape(&css, out);
            out.push('"');
        }
    }
}

fn img(o: &serde_json::Map<String, Value>, out: &mut String) {
    let Some(path) = o.get("path").and_then(|p| p.as_str()) else {
        return;
    };
    out.push_str("<img src=\"");
    escape(path, out);
    out.push('"');
    let em = o.get("sizeUnits").and_then(|u| u.as_str()) == Some("em");
    let mut css = String::new();
    for k in ["width", "height"] {
        if let Some(n) = o.get(k).and_then(|v| v.as_f64()) {
            if em {
                let _ = write!(css, "{k}:{n}em;");
            } else {
                let _ = write!(out, " {k}=\"{n}\"");
            }
        }
    }
    if let Some(v) = o.get("verticalAlign").and_then(|v| v.as_str())
        && safe_name(v)
    {
        let _ = write!(css, "vertical-align:{v};");
    }
    if !css.is_empty() {
        let _ = write!(out, " style=\"{css}\"");
    }
    for k in ["title", "alt"] {
        if let Some(v) = o.get(k).and_then(|v| v.as_str()) {
            let _ = write!(out, " {k}=\"");
            escape(v, out);
            out.push('"');
        }
    }
    out.push('>');
}

/// Structured content (a string, an array, or an element object).
pub fn content(v: &Value, out: &mut String) {
    match v {
        Value::String(s) => escape(s, out),
        Value::Array(a) => a.iter().for_each(|x| content(x, out)),
        Value::Object(o) => {
            let Some(tag) = o.get("tag").and_then(|t| t.as_str()) else {
                return;
            };
            match tag {
                "br" => out.push_str("<br>"),
                "img" => img(o, out),
                "a" => {
                    let href = o.get("href").and_then(|h| h.as_str()).unwrap_or("");
                    // Internal links only: a lookup of another word.
                    if href.starts_with('?') {
                        out.push_str("<a href=\"");
                        escape(href, out);
                        out.push('"');
                        attrs(o, out);
                        out.push('>');
                        if let Some(c) = o.get("content") {
                            content(c, out);
                        }
                        out.push_str("</a>");
                    } else if let Some(c) = o.get("content") {
                        content(c, out);
                    }
                }
                "ul" | "ol" if compact_items(o).is_some() => {
                    // Short plain items (CC-CEDICT senses) on one line, as
                    // Yomitan's compact glossaries: "to speak; to say; talk".
                    out.push_str("<span class=\"gloss-sc-list\"");
                    attrs(o, out);
                    out.push('>');
                    for (i, li) in compact_items(o).unwrap().into_iter().enumerate() {
                        if i > 0 {
                            out.push_str("<span class=\"sep\">; </span>");
                        }
                        if let Some(c) = li.get("content") {
                            content(c, out);
                        }
                    }
                    out.push_str("</span>");
                }
                t if CONTAINER.contains(&t) => {
                    let _ = write!(out, "<{t} class=\"gloss-sc-{t}\"");
                    attrs(o, out);
                    if t == "details" && o.get("open").and_then(|v| v.as_bool()) == Some(true) {
                        out.push_str(" open");
                    }
                    for k in ["colSpan", "rowSpan"] {
                        if let Some(n) = o.get(k).and_then(|v| v.as_u64()) {
                            let _ = write!(out, " {}=\"{n}\"", k.to_ascii_lowercase());
                        }
                    }
                    out.push('>');
                    if let Some(c) = o.get("content") {
                        content(c, out);
                    }
                    let _ = write!(out, "</{t}>");
                }
                _ => {
                    if let Some(c) = o.get("content") {
                        content(c, out);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Plain text within an item: inline elements only.
fn inline_only(v: &Value) -> bool {
    match v {
        Value::String(_) => true,
        Value::Array(a) => a.iter().all(inline_only),
        Value::Object(o) => {
            matches!(
                o.get("tag").and_then(|t| t.as_str()),
                Some("span" | "a" | "ruby" | "rt" | "rp" | "img")
            ) && o.get("content").is_none_or(inline_only)
        }
        _ => false,
    }
}

fn text_len(v: &Value) -> usize {
    match v {
        Value::String(s) => s.chars().count(),
        Value::Array(a) => a.iter().map(text_len).sum(),
        Value::Object(o) if o.get("tag").and_then(|t| t.as_str()) == Some("rt") => 0,
        Value::Object(o) => o.get("content").map_or(0, text_len),
        _ => 0,
    }
}

/// A list's items, if it can be one line: `li` of plain inline content, at
/// most 60 characters each.
fn compact_items(
    o: &serde_json::Map<String, Value>,
) -> Option<Vec<&serde_json::Map<String, Value>>> {
    let items: Vec<&Value> = match o.get("content")? {
        Value::Array(a) => a.iter().collect(),
        v => vec![v],
    };
    if items.is_empty() {
        return None;
    }
    items
        .into_iter()
        .map(|it| {
            let li = it.as_object()?;
            if li.get("tag").and_then(|t| t.as_str()) != Some("li") || li.get("style").is_some() {
                return None;
            }
            let c = li.get("content")?;
            (inline_only(c) && text_len(c) <= 60).then_some(li)
        })
        .collect()
}

/// One definition of a glossary array: a string, `{type: text}`,
/// `{type: structured-content}` or `{type: image}`. Deinflection entries
/// (arrays) are not shown.
fn definition(v: &Value, out: &mut String) {
    match v {
        Value::String(s) => escape(s, out),
        Value::Object(o) => match o.get("type").and_then(|t| t.as_str()) {
            Some("text") => escape(o.get("text").and_then(|t| t.as_str()).unwrap_or(""), out),
            Some("structured-content") => {
                out.push_str("<div class=\"structured-content\">");
                if let Some(c) = o.get("content") {
                    content(c, out);
                }
                out.push_str("</div>");
            }
            Some("image") => img(o, out),
            _ => {}
        },
        _ => {}
    }
}

/// A glossary's definitions as HTML: one as it is, several as a list.
pub fn glossary(json: &str, out: &mut String) -> Result<(), String> {
    let v: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let items: Vec<&Value> = match &v {
        Value::Array(a) => a.iter().filter(|x| !x.is_array()).collect(),
        other => vec![other],
    };
    let short = |v: &Value| match v {
        Value::String(s) => s.chars().count() <= 60,
        Value::Object(o) if o.get("type").and_then(|t| t.as_str()) == Some("text") => o
            .get("text")
            .and_then(|t| t.as_str())
            .is_some_and(|s| s.chars().count() <= 60),
        _ => false,
    };
    if items.len() == 1 {
        out.push_str("<div class=\"glossary\">");
        definition(items[0], out);
        out.push_str("</div>");
    } else if items.iter().all(|v| short(v)) {
        // Short plain definitions on one line.
        out.push_str("<div class=\"glossary\">");
        for (i, it) in items.iter().enumerate() {
            if i > 0 {
                out.push_str("<span class=\"sep\">; </span>");
            }
            definition(it, out);
        }
        out.push_str("</div>");
    } else {
        out.push_str("<ul class=\"glossary\">");
        for it in items {
            out.push_str("<li>");
            definition(it, out);
            out.push_str("</li>");
        }
        out.push_str("</ul>");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn html(json: &str) -> String {
        let mut s = String::new();
        glossary(json, &mut s).unwrap();
        s
    }

    #[test]
    fn strings_and_lists() {
        assert_eq!(
            html(r#"["to speak"]"#),
            r#"<div class="glossary">to speak</div>"#
        );
        assert_eq!(
            html(r#"["a<b", {"type":"text","text":"c&d"}]"#),
            r#"<div class="glossary">a&lt;b<span class="sep">; </span>c&amp;d</div>"#
        );
        let long = "x".repeat(61);
        assert_eq!(
            html(&format!(r#"["a", "{long}"]"#)),
            format!(r#"<ul class="glossary"><li>a</li><li>{long}</li></ul>"#)
        );
        // Deinflection entries are not definitions.
        assert_eq!(
            html(r#"["x", ["y", ["v5"]]]"#),
            r#"<div class="glossary">x</div>"#
        );
    }

    #[test]
    fn structured_content() {
        let j = r#"[{"type":"structured-content","content":[
            {"tag":"span","data":{"hanyu7":"simp","simplifiedTerm":"x"},"style":{"fontWeight":"bold","marginTop":0.5,"textDecorationLine":["underline","line-through"]},"content":"说"},
            {"tag":"br"},
            {"tag":"ruby","content":["話",{"tag":"rt","content":"huà"}]},
            {"tag":"a","href":"?query=%E8%AF%B4&wildcards=off","content":"see"},
            {"tag":"a","href":"https://example.org","content":"site"},
            {"tag":"details","open":true,"content":[{"tag":"summary","content":"例"},"…"]},
            {"tag":"td","colSpan":2,"content":"c"},
            {"tag":"script","content":"<b>"}
        ]}]"#;
        assert_eq!(
            html(j),
            concat!(
                r#"<div class="glossary"><div class="structured-content">"#,
                r#"<span class="gloss-sc-span" data-sc-hanyu7="simp" data-sc-simplified-term="x" style="font-weight:bold;margin-top:0.5em;text-decoration-line:underline line-through;">说</span>"#,
                "<br>",
                r#"<ruby class="gloss-sc-ruby">話<rt class="gloss-sc-rt">huà</rt></ruby>"#,
                r#"<a href="?query=%E8%AF%B4&amp;wildcards=off">see</a>"#,
                "site",
                r#"<details class="gloss-sc-details" open><summary class="gloss-sc-summary">例</summary>…</details>"#,
                r#"<td class="gloss-sc-td" colspan="2">c</td>"#,
                "&lt;b&gt;",
                "</div></div>"
            )
        );
    }

    #[test]
    fn hostile_values_stay_inside_their_attributes() {
        let j = r#"[{"type":"structured-content","content":{"tag":"span",
            "data":{"x\" onclick=\"y":"1","ok":"a\"><script>"},"style":{"color":"red;}body{x:1"},"content":"t"}}]"#;
        let h = html(j);
        assert!(
            !h.contains("onclick") && !h.contains("<script") && !h.contains("body{"),
            "{h}"
        );
        assert!(
            h.contains(r#"data-sc-ok="a&quot;&gt;&lt;script&gt;""#),
            "{h}"
        );
    }

    /// CC-CEDICT's senses: one line; a list with a block inside stays a list.
    #[test]
    fn short_lists_are_one_line() {
        let j = r#"[{"type":"structured-content","content":{"tag":"ul","data":{"cccedict":"definition"},"content":[
            {"tag":"li","content":"to speak"},{"tag":"li","content":["to ",{"tag":"a","href":"?query=x","content":"say"}]}]}}]"#;
        assert_eq!(
            html(j),
            concat!(
                r#"<div class="glossary"><div class="structured-content">"#,
                r#"<span class="gloss-sc-list" data-sc-cccedict="definition">to speak<span class="sep">; </span>to <a href="?query=x">say</a></span>"#,
                "</div></div>"
            )
        );
        let j = r#"[{"type":"structured-content","content":{"tag":"ul","content":[
            {"tag":"li","content":"a"},{"tag":"li","content":{"tag":"div","content":"b"}}]}}]"#;
        assert!(html(j).contains("<ul class=\"gloss-sc-ul\">"));
    }

    #[test]
    fn images() {
        assert_eq!(
            html(
                r#"[{"type":"image","path":"img/a.png","width":2,"height":1,"sizeUnits":"em","title":"T"}]"#
            ),
            r#"<div class="glossary"><img src="img/a.png" style="width:2em;height:1em;" title="T"></div>"#
        );
    }
}
