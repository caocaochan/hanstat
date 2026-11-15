//! Character normalisation and classification.
//!
//! All counting is done on Unicode scalar values *after* the normalisation in
//! [`normalize`], so that visually identical text produces identical counts no
//! matter how the producer of the file encoded it.

use crate::lang::Language;
use serde::Serialize;
use unicode_general_category::{GeneralCategory as Gc, get_general_category};
use unicode_normalization::UnicodeNormalization;
use unicode_script::{Script, UnicodeScript};

/// What was changed by [`normalize`]; reported so the user can see it.
#[derive(Default, Clone, Debug, Serialize)]
pub struct NormStats {
    /// CJK Compatibility Ideographs (U+F900.., U+2F800..) mapped to their unified form by NFC.
    pub compat_ideographs: u64,
    /// Kangxi / CJK radical code points that were really meant as ideographs (PDF/OCR artefacts).
    pub radicals_folded: u64,
    /// Variation selectors (incl. ideographic variation sequences) dropped.
    pub variation_selectors: u64,
    /// Fullwidth ASCII letters/digits folded to ASCII.
    pub fullwidth_folded: u64,
    /// Invisible format/control characters removed (ZWSP, ZWJ, soft hyphen, BOM, ...).
    pub invisible_removed: u64,
}

impl NormStats {
    pub fn add(&mut self, o: &NormStats) {
        self.compat_ideographs += o.compat_ideographs;
        self.radicals_folded += o.radicals_folded;
        self.variation_selectors += o.variation_selectors;
        self.fullwidth_folded += o.fullwidth_folded;
        self.invisible_removed += o.invisible_removed;
    }
}

fn is_variation_selector(c: char) -> bool {
    matches!(c, '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}' | '\u{180B}'..='\u{180D}' | '\u{180F}')
}

fn is_compat_ideograph_block(c: char) -> bool {
    matches!(c, '\u{F900}'..='\u{FAFF}' | '\u{2F800}'..='\u{2FA1F}')
}

/// Normalise one paragraph. Newlines are not expected inside a paragraph but
/// are preserved if present.
pub fn normalize(input: &str, stats: &mut NormStats) -> String {
    let mut pre = String::with_capacity(input.len());
    for c in input.chars() {
        if is_variation_selector(c) {
            stats.variation_selectors += 1;
            continue;
        }
        match get_general_category(c) {
            Gc::Format => {
                stats.invisible_removed += 1;
                continue;
            }
            Gc::Control if c != '\n' && c != '\t' => {
                stats.invisible_removed += 1;
                continue;
            }
            _ => {}
        }
        match c {
            // Kangxi Radicals and CJK Radicals Supplement: NFKC maps those that
            // have an ideograph equivalent (e.g. U+2F08 ⼈ -> 人).
            '\u{2E80}'..='\u{2EFF}' | '\u{2F00}'..='\u{2FDF}' => {
                let mut it = std::iter::once(c).nfkc();
                if let (Some(m), None) = (it.next(), it.next())
                    && m != c
                    && is_han(m)
                {
                    stats.radicals_folded += 1;
                    pre.push(m);
                    continue;
                }
                pre.push(c);
            }
            // Fullwidth digits and Latin letters.
            '\u{FF10}'..='\u{FF19}' | '\u{FF21}'..='\u{FF3A}' | '\u{FF41}'..='\u{FF5A}' => {
                stats.fullwidth_folded += 1;
                pre.push(char::from_u32(c as u32 - 0xFF00 + 0x20).unwrap());
            }
            _ => {
                if is_compat_ideograph_block(c) {
                    let mut it = std::iter::once(c).nfc();
                    if it.next() != Some(c) {
                        stats.compat_ideographs += 1;
                    }
                }
                pre.push(c);
            }
        }
    }
    // Canonical composition: merges compatibility ideographs into unified ones
    // and composes Latin combining sequences. Never changes Han identity otherwise.
    if unicode_normalization::is_nfc_quick(pre.chars()) == unicode_normalization::IsNormalized::Yes
    {
        pre
    } else {
        pre.nfc().collect()
    }
}

/// A Chinese character ("hanzi"): Script=Han and a letter/letter-number
/// (covers all CJK Unified Ideograph blocks incl. Ext A–J, compatibility
/// ideographs, 〇 and 々; excludes radicals and strokes, which are symbols).
#[inline]
pub fn is_han(c: char) -> bool {
    if (c as u32) < 0x2E80 {
        return false;
    }
    c.script() == Script::Han
        && matches!(
            get_general_category(c),
            Gc::OtherLetter | Gc::ModifierLetter | Gc::LetterNumber
        )
}

/// A precomposed Hangul syllable (가–힣). Conjoining jamo are composed into
/// these by the NFC step of [`normalize`]; lone jamo (ㅋㅋ) are letters.
#[inline]
pub fn is_hangul(c: char) -> bool {
    ('\u{AC00}'..='\u{D7A3}').contains(&c)
}

/// A character of the book's main script: Han for Chinese, Hangul syllables for Korean.
#[inline]
pub fn is_main(c: char, lang: Language) -> bool {
    match lang {
        Language::Chinese => is_han(c),
        Language::Korean => is_hangul(c),
    }
}

/// [`classify`] for a book in `lang`. A Korean book has no character
/// statistics (only words), so Hanja there are letters like Hangul.
#[inline]
pub fn classify_in(c: char, lang: Language) -> Class {
    match lang {
        Language::Chinese => classify(c),
        Language::Korean if is_han(c) => Class::Letter,
        Language::Korean => classify(c),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    /// A Chinese character in a Chinese book (never used for Korean, see [`classify_in`]).
    Han,
    /// Letters of any other script (Latin, kana, hangul, ...), plus combining marks.
    Letter,
    Digit,
    Punct,
    Symbol,
    Space,
    PrivateUse,
    Replacement,
    Other,
}

pub fn classify(c: char) -> Class {
    if is_han(c) {
        return Class::Han;
    }
    if c == '\u{FFFD}' {
        return Class::Replacement;
    }
    if c.is_whitespace() {
        return Class::Space;
    }
    match get_general_category(c) {
        Gc::UppercaseLetter
        | Gc::LowercaseLetter
        | Gc::TitlecaseLetter
        | Gc::ModifierLetter
        | Gc::OtherLetter
        | Gc::NonspacingMark
        | Gc::SpacingMark
        | Gc::EnclosingMark => Class::Letter,
        Gc::DecimalNumber | Gc::LetterNumber | Gc::OtherNumber => Class::Digit,
        Gc::ConnectorPunctuation
        | Gc::DashPunctuation
        | Gc::OpenPunctuation
        | Gc::ClosePunctuation
        | Gc::InitialPunctuation
        | Gc::FinalPunctuation
        | Gc::OtherPunctuation => Class::Punct,
        Gc::MathSymbol | Gc::CurrencySymbol | Gc::ModifierSymbol | Gc::OtherSymbol => Class::Symbol,
        Gc::PrivateUse => Class::PrivateUse,
        _ => Class::Other,
    }
}

/// East-Asian "wide" for the CSS segment-break rule: a line break in HTML
/// source between two such characters is removed rather than rendered as a space.
pub fn is_wide(c: char) -> bool {
    is_han(c)
        || matches!(c,
            '\u{3000}'..='\u{303F}'   // CJK symbols & punctuation
            | '\u{3040}'..='\u{30FF}' // kana
            | '\u{FF00}'..='\u{FFEF}' // half/fullwidth forms
            | '\u{2E80}'..='\u{2FDF}'
            | '\u{FE30}'..='\u{FE4F}'
            | '\u{2018}'..='\u{201F}' // curly quotes, used as CJK quotes
            | '\u{2026}' | '\u{2014}')
}

/// Sentence-final punctuation.
pub fn is_sentence_end(c: char) -> bool {
    matches!(
        c,
        '。' | '！'
            | '？'
            | '!'
            | '?'
            | '｡'
            | '…'
            | '‼'
            | '⁇'
            | '⁈'
            | '⁉'
            | '︒'
            | '﹒'
            | '﹖'
            | '﹗'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn han_ranges() {
        for c in ['一', '龥', '㐀', '𠀀', '𪜀', '𰀀', '〇', '々', '﨑'] {
            assert!(is_han(c), "{c:?} U+{:04X}", c as u32);
        }
        for c in ['，', '。', 'A', '1', '⼀', '⺀', 'あ', '　', '・', '㇀'] {
            assert!(!is_han(c), "{c:?} U+{:04X}", c as u32);
        }
    }

    #[test]
    fn korean_classes() {
        let k = Language::Korean;
        assert_eq!(classify_in('한', k), Class::Letter);
        assert_eq!(classify_in('漢', k), Class::Letter);
        assert_eq!(classify_in('ㅋ', k), Class::Letter);
        assert_eq!(classify_in('한', Language::Chinese), Class::Letter);
        assert_eq!(classify_in('漢', Language::Chinese), Class::Han);
        // Conjoining jamo compose into a syllable.
        let mut s = NormStats::default();
        assert_eq!(normalize("\u{1112}\u{1161}\u{11AB}", &mut s), "한");
    }

    #[test]
    fn normalization() {
        let mut s = NormStats::default();
        // ZWSP between chars, compat ideograph 﨟 -> 隷? use U+F900 豈 -> 豈, kangxi ⼈ -> 人,
        // IVS on 葛, fullwidth Ａ1.
        let out = normalize("中\u{200B}文\u{F900}\u{2F08}葛\u{E0100}Ａ１", &mut s);
        assert_eq!(out, "中文豈人葛A1");
        assert_eq!(s.invisible_removed, 1);
        assert_eq!(s.compat_ideographs, 1);
        assert_eq!(s.radicals_folded, 1);
        assert_eq!(s.variation_selectors, 1);
        assert_eq!(s.fullwidth_folded, 2);
    }
}
