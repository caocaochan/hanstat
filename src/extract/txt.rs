//! Plain-text loading: encoding detection, paragraphs, chapter detection.

use super::{Book, EncodingChoice, Section, SectionKind};
use crate::chars::{is_han, is_hangul, is_kana, is_sentence_end};
use encoding_rs::Encoding;
use regex::Regex;
use std::sync::LazyLock;

static CHAPTER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*(?:第\s*[0-9０-９零〇一二三四五六七八九十百千万萬两兩]+\s*[章回节節卷集部篇幕话話]|(?:序章|序幕|楔子|引子|序言|前言|尾声|尾聲|终章|終章|后记|後記|番外)|(?i:chapter)\s*\d+|제\s*[0-9０-９]+\s*[장화권부편막]|[0-9]+\s*화(?:\s|$|[.:)\]])|(?:프롤로그|에필로그|외전|서장|종장|작가의 말|작가 후기))",
    )
    .unwrap()
});

/// Decode with BOM sniffing, strict UTF-8, BOM-less UTF-16 heuristics, then chardetng.
pub fn decode(
    bytes: &[u8],
    choice: EncodingChoice,
) -> (String, &'static Encoding, &'static str, bool) {
    if let Some(enc) = choice.encoding() {
        let (s, had_err) = enc.decode_without_bom_handling(strip_bom(bytes, enc));
        return (s.into_owned(), enc, "chosen manually", had_err);
    }
    if let Some((enc, bom_len)) = Encoding::for_bom(bytes) {
        let (s, had_err) = enc.decode_without_bom_handling(&bytes[bom_len..]);
        return (s.into_owned(), enc, "byte-order mark", had_err);
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return (s.to_owned(), encoding_rs::UTF_8, "valid UTF-8", false);
    }
    // BOM-less UTF-16. Chinese UTF-16 has few NUL bytes, so instead score how
    // often very common code units (newline, space, CJK punctuation, ASCII
    // letters as 00xx) occur under each byte order. Real text scores far above
    // a legacy multibyte file misread as UTF-16.
    if bytes.len() >= 4 && bytes.len().is_multiple_of(2) {
        let sample = &bytes[..bytes.len().min(131072) & !1];
        let score = |le: bool| {
            let units = sample.as_chunks::<2>().0.iter().map(|p| {
                if le {
                    u16::from_le_bytes([p[0], p[1]])
                } else {
                    u16::from_be_bytes([p[0], p[1]])
                }
            });
            let n = sample.len() / 2;
            let hits = units
                .filter(|&u| {
                    matches!(
                        u,
                        0x000A
                            | 0x000D
                            | 0x0020
                            | 0x3000
                            | 0x3001
                            | 0x3002
                            | 0xFF0C
                            | 0xFF01
                            | 0xFF1F
                            | 0xFF1A
                            | 0x201C
                            | 0x201D
                    ) || (0x0021..=0x007E).contains(&u)
                })
                .count();
            hits as f64 / n as f64
        };
        let (le, be) = (score(true), score(false));
        let (best, enc) = if le >= be {
            (le, encoding_rs::UTF_16LE)
        } else {
            (be, encoding_rs::UTF_16BE)
        };
        if best >= 0.05 && best >= 2.0 * le.min(be).max(0.001) {
            let (s, had_err) = enc.decode_without_bom_handling(bytes);
            if !had_err {
                return (
                    s.into_owned(),
                    enc,
                    "code-unit statistics (BOM-less UTF-16)",
                    had_err,
                );
            }
        }
    }
    let mut det = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    det.feed(bytes, true);
    let enc = det.guess(None, chardetng::Utf8Detection::Allow);
    // GBK is a subset of GB18030; decoding as GB18030 is strictly more permissive.
    let enc = if enc == encoding_rs::GBK {
        encoding_rs::GB18030
    } else {
        enc
    };
    let (s, had_err) = enc.decode_without_bom_handling(bytes);
    (
        s.into_owned(),
        enc,
        "statistical detection (chardetng)",
        had_err,
    )
}

fn strip_bom<'a>(bytes: &'a [u8], enc: &'static Encoding) -> &'a [u8] {
    match Encoding::for_bom(bytes) {
        Some((e, n)) if e == enc => &bytes[n..],
        _ => bytes,
    }
}

pub fn load(bytes: &[u8], choice: EncodingChoice) -> Book {
    let (text, enc, how, _) = decode(bytes, choice);
    let decode_errors = text.chars().filter(|&c| c == '\u{FFFD}').count() as u64;
    let mut notes = vec![format!("Encoding: {} ({how}).", enc.name())];
    if decode_errors > 0 {
        notes.push(format!(
            "{decode_errors} undecodable byte sequences became U+FFFD; if this is large, try another encoding."
        ));
    }

    let lines: Vec<&str> = text.lines().collect();

    // Korean text wraps between Hangul syllables, and its lines rejoin with a
    // space (Korean separates words with spaces); Chinese and Japanese rejoin
    // without one, and Japanese lines may end in kana.
    let korean = {
        let (hangul, han) = text.chars().fold((0usize, 0usize), |(a, b), c| {
            (a + is_hangul(c) as usize, b + is_han(c) as usize)
        });
        hangul > han
    };
    let is_main = |c: char| {
        if korean {
            is_hangul(c)
        } else {
            is_han(c) || is_kana(c)
        }
    };

    // Hard-wrap detection: in paragraph-per-line files almost every non-empty line ends
    // in punctuation; in hard-wrapped files many end mid-sentence on a Han character.
    let nonempty: Vec<&str> = lines
        .iter()
        .map(|l| l.trim_end())
        .filter(|l| !l.trim().is_empty())
        .collect();
    let ends_han = nonempty
        .iter()
        .filter(|l| l.chars().last().is_some_and(is_main))
        .count();
    let hard_wrapped = nonempty.len() >= 20 && ends_han * 10 > nonempty.len() * 4;
    if hard_wrapped {
        notes.push(format!(
            "Hard-wrapped text detected ({}% of lines end mid-sentence): lines are joined when one ends and the next starts with a {} without indentation.",
            ends_han * 100 / nonempty.len().max(1),
            if korean { "Hangul letter (with a space between them)" } else { "Chinese character" }
        ));
    }

    let mut sections: Vec<Section> = Vec::new();
    let mut cur = Section {
        label: "Front matter".into(),
        href: None,
        kind: SectionKind::FrontMatter,
        paragraphs: Vec::new(),
    };
    let mut para = String::new();
    let flush = |para: &mut String, sec: &mut Section| {
        let t = para.trim();
        if !t.is_empty() {
            sec.paragraphs.push(t.to_owned());
        }
        para.clear();
    };

    for line in &lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush(&mut para, &mut cur);
            continue;
        }
        if trimmed.chars().count() <= 40 && CHAPTER_RE.is_match(trimmed) {
            flush(&mut para, &mut cur);
            let done = std::mem::replace(
                &mut cur,
                Section {
                    label: trimmed.to_owned(),
                    href: None,
                    kind: SectionKind::Body,
                    paragraphs: Vec::new(),
                },
            );
            if !done.paragraphs.is_empty() {
                sections.push(done);
            }
            cur.paragraphs.push(trimmed.to_owned());
            continue;
        }
        let indented = line.starts_with(|c: char| c.is_whitespace());
        let joinable = hard_wrapped
            && !indented
            && para
                .chars()
                .last()
                .is_some_and(|c| is_main(c) && !is_sentence_end(c))
            && trimmed.chars().next().is_some_and(is_main);
        if !joinable {
            flush(&mut para, &mut cur);
        } else if korean {
            para.push(' ');
        }
        para.push_str(trimmed);
    }
    flush(&mut para, &mut cur);
    if !cur.paragraphs.is_empty() {
        sections.push(cur);
    }
    if sections.len() == 1 && sections[0].kind == SectionKind::FrontMatter {
        sections[0].label = "Full text".into();
        sections[0].kind = SectionKind::Body;
    } else if sections.len() > 1 {
        notes.push(format!(
            "{} chapters detected from headings.",
            sections
                .iter()
                .filter(|s| s.kind == SectionKind::Body)
                .count()
        ));
    }

    Book {
        title: None,
        format: "TXT",
        sections,
        notes,
        decode_errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gbk_detection() {
        let (bytes, _, _) = encoding_rs::GBK.encode(
            "第一章 开始\n　　他说：“这是一个用来测试编码检测的句子，内容需要足够长才行。”\n",
        );
        let (s, enc, _, err) = decode(&bytes, EncodingChoice::Auto);
        assert!(!err);
        assert_eq!(enc, encoding_rs::GB18030);
        assert!(s.contains("编码检测"));
    }

    #[test]
    fn utf16_bomless() {
        let s = "第一章 开始\n　　他说：“这是一个用来测试的句子。”她点了点头，没有回答。\n　　窗外下着雨，街上没有什么人。\n";
        let bytes: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let (d, enc, _, _) = decode(&bytes, EncodingChoice::Auto);
        assert_eq!(enc, encoding_rs::UTF_16LE);
        assert_eq!(d, s);
    }

    #[test]
    fn chapters() {
        let b = load(
            "书名\n作者\n\n第一章 起\n　　正文一。\n第2章 承\n　　正文二。\n".as_bytes(),
            EncodingChoice::Auto,
        );
        let labels: Vec<_> = b.sections.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["Front matter", "第一章 起", "第2章 承"]);
    }

    #[test]
    fn korean_chapters_and_euc_kr() {
        let text = "제목\n\n프롤로그\n그는 문을 열었다.\n제1장 시작\n본문입니다.\n12화. 다음\n본문입니다.\n1화의 내용을 다시 읽었다.\n";
        let (bytes, _, _) = encoding_rs::EUC_KR.encode(text);
        let (s, enc, _, err) = decode(&bytes, EncodingChoice::Auto);
        assert!(!err);
        assert_eq!(enc, encoding_rs::EUC_KR);
        let b = load(s.as_bytes(), EncodingChoice::Auto);
        let labels: Vec<_> = b.sections.iter().map(|s| s.label.as_str()).collect();
        // "1화의 …" is prose, not a heading.
        assert_eq!(
            labels,
            ["Front matter", "프롤로그", "제1장 시작", "12화. 다음"]
        );
    }

    #[test]
    fn korean_hard_wrap_joins_with_a_space() {
        let mut text = String::from("제1장\n");
        for _ in 0..12 {
            text.push_str("그는 오래된 문을 천천히\n열고 안으로 들어갔다.\n");
        }
        let b = load(text.as_bytes(), EncodingChoice::Auto);
        assert!(
            b.notes.iter().any(|n| n.contains("Hard-wrapped")),
            "{:?}",
            b.notes
        );
        assert_eq!(
            b.sections[0].paragraphs[1],
            "그는 오래된 문을 천천히 열고 안으로 들어갔다."
        );
    }
}
