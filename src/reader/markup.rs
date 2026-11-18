//! XHTML as the HTML parser needs it.

/// HTML void elements: `<x/>` and `<x>` mean the same thing for these.
const VOID: &[&str] = &[
    "area", "base", "basefont", "bgsound", "br", "col", "embed", "frame", "hr", "img", "input",
    "keygen", "link", "meta", "param", "source", "track", "wbr",
];

/// Rewrite XML self-closing tags of non-void elements (`<p class="x"/>`,
/// `<a id="x"/>`) as an explicit start/end pair: the HTML parser ignores the
/// `/` on them, so `<p/>` would swallow the following paragraphs and
/// `<title/>` the whole document. (The same rule as
/// `hanstat::extract::html`, which is private there.)
pub fn expand_self_closing(src: &str) -> std::borrow::Cow<'_, str> {
    if !src.contains("/>") {
        return src.into();
    }
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 64);
    let mut copied = 0;
    let mut i = 0;
    while let Some(off) = src[i..].find('<') {
        let lt = i + off;
        let rest = &src[lt..];
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

#[cfg(test)]
mod tests {
    #[test]
    fn self_closing() {
        assert_eq!(
            super::expand_self_closing(r#"<p class="a"/><br/><img src="x"/><a id="q/>"/>"#),
            r#"<p class="a"></p><br/><img src="x"/><a id="q/>"></a>"#
        );
    }
}
