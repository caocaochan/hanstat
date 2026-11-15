//! Word segmentation back-ends.
//!
//! * CKIP BERT (bert-tiny-chinese-ws, ASBC standard) — most accurate, slowest.
//! * LTP legacy (HIT-SCIR averaged perceptron, PKU-style standard) — fast, default.
//! * jieba (dictionary DAG + max-probability path, HMM off).
//! * Kiwi, for Korean books only: returns dictionary forms instead of slices
//!   (see [`Segmenter::lemmas_many`] and [`crate::kiwi`]).
//!
//! Every back-end gets whitespace-free chunks of the *original* text and returns
//! slices of it. Script folding needed by a model (T→S for LTP/jieba, S→T for
//! CKIP) happens inside, character-for-character, so counts never see it.
//! A user dictionary is applied to every back-end the same way: each match
//! (leftmost-longest) on the original text becomes exactly one token.
//! Known words (word lists, marked words) are applied more gently: a match
//! only joins tokens the model split, and never cuts through a model token
//! (see [`Segmenter::with_known`]).

use crate::ckip::Ckip;
use crate::kiwi;
use crate::lang::Language;
use crate::models;
use crate::script::{self, Variant};
use rayon::prelude::*;
use serde::Serialize;
use std::io::BufReader;
use std::sync::{Arc, OnceLock};

// Only successful loads are memoised, so a failed download can be retried.
static LTP_MODEL: OnceLock<ltp::CWSModel> = OnceLock::new();
static CKIP_MODEL: OnceLock<Ckip> = OnceLock::new();

/// Load (once) the LTP model, downloading it first if needed. Call from a background thread.
pub fn ltp_model() -> Result<&'static ltp::CWSModel, String> {
    if let Some(m) = LTP_MODEL.get() {
        return Ok(m);
    }
    let dir = models::LTP.ensure(&|_, _| {})?;
    let f =
        std::fs::File::open(dir.join("cws_model.bin")).map_err(|e| format!("LTP model: {e}"))?;
    let m: ltp::CWSModel =
        ltp::ModelSerde::load(BufReader::new(f), ltp::Format::AVRO(ltp::Codec::Deflate))
            .map_err(|e| format!("Failed to load LTP model: {e}"))?;
    Ok(LTP_MODEL.get_or_init(|| m))
}

/// Load (once) the CKIP model, downloading it first if needed.
pub fn ckip_model() -> Result<&'static Ckip, String> {
    if let Some(m) = CKIP_MODEL.get() {
        return Ok(m);
    }
    let dir = models::CKIP.ensure(&|_, _| {})?;
    let m = Ckip::from_pth_dir(&dir)?;
    Ok(CKIP_MODEL.get_or_init(|| m))
}

/// The downloadable model a segmenter needs, if any.
pub fn model_for(kind: SegmenterKind) -> Option<&'static models::Model> {
    match kind {
        SegmenterKind::Ltp | SegmenterKind::Auto => Some(&models::LTP),
        SegmenterKind::Ckip => Some(&models::CKIP),
        SegmenterKind::Jieba => None,
        SegmenterKind::Kiwi => Some(&models::KIWI),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize)]
pub enum SegmenterKind {
    /// Best-benchmarked model for the detected script: CKIP for Traditional, LTP otherwise.
    #[default]
    Auto,
    Ckip,
    Ltp,
    Jieba,
    /// Korean morphological analysis; used for every Korean book.
    Kiwi,
}

impl SegmenterKind {
    /// The Chinese segmenters the user can choose from.
    pub const ALL: [SegmenterKind; 4] = [Self::Auto, Self::Ckip, Self::Ltp, Self::Jieba];
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto (by script)",
            Self::Ckip => "CKIP BERT",
            Self::Ltp => "LTP perceptron",
            Self::Jieba => "jieba",
            Self::Kiwi => "Kiwi",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Auto => {
                "LTP for Simplified text, CKIP for Traditional text: the most accurate choice for each script on the SIGHAN 2005 benchmarks."
            }
            Self::Ckip => {
                "Academia Sinica CKIP bert-tiny, a context-aware neural model (Taiwan AS standard). Best on Traditional text (SIGHAN F1 90–98) and the most consistent with character names; ~1–3 min per novel on CPU."
            }
            Self::Ltp => {
                "HIT-SCIR LTP averaged perceptron, PKU-style granularity (三/个, 吃/掉). Best on Simplified text (SIGHAN F1 88–95); a few seconds per novel."
            }
            Self::Jieba => {
                "jieba dictionary segmentation (HMM off). Fastest, least accurate (SIGHAN F1 81–84); merges many compounds."
            }
            Self::Kiwi => {
                "Kiwi Korean morphological analyser (CoNg model). Counts words as dictionary forms (먹었어요 → 먹다); used for every Korean book."
            }
        }
    }
    /// The concrete model `Auto` stands for, given the detected script.
    pub fn resolve(self, variant: Variant) -> SegmenterKind {
        match self {
            Self::Auto if variant == Variant::Traditional => Self::Ckip,
            Self::Auto => Self::Ltp,
            k => k,
        }
    }

    /// The segmenter for a book in `lang`: Kiwi for Korean, otherwise the
    /// chosen Chinese one with `Auto` resolved by script.
    pub fn resolve_in(self, lang: Language, variant: Variant) -> SegmenterKind {
        match lang {
            Language::Korean => Self::Kiwi,
            Language::Chinese => self.resolve(variant),
        }
    }
}

enum Backend {
    Ltp(&'static ltp::CWSModel),
    Jieba(Box<jieba_rs::Jieba>),
    Ckip(&'static Ckip),
    Kiwi(Arc<kiwi::Kiwi>),
}

pub struct Segmenter {
    pub kind: SegmenterKind,
    backend: Backend,
    /// User dictionary, matched leftmost-longest on the original text.
    forced: Option<aho_corasick::AhoCorasick>,
    /// Known words, matched overlapping; see [`Segmenter::with_known`].
    known: Option<aho_corasick::AhoCorasick>,
    /// LTP/jieba: fold Traditional → Simplified for the model.
    fold_t2s: bool,
    /// CKIP: fold Simplified → Traditional for the model.
    fold_s2t: bool,
}

impl Segmenter {
    /// `user_words` must be NFC-normalised (same form as the analysed text).
    pub fn new(
        kind: SegmenterKind,
        user_words: &[String],
        variant: Variant,
    ) -> Result<Self, String> {
        let kind = kind.resolve(variant);
        let trad_like = matches!(variant, Variant::Traditional | Variant::Mixed);
        let (fold_t2s, fold_s2t) = match kind {
            SegmenterKind::Ckip => (false, variant != Variant::Traditional),
            _ => (trad_like, false),
        };
        let backend = match kind {
            SegmenterKind::Ltp | SegmenterKind::Auto => Backend::Ltp(ltp_model()?),
            SegmenterKind::Ckip => Backend::Ckip(ckip_model()?),
            SegmenterKind::Jieba => Backend::Jieba(Box::new(jieba_rs::Jieba::new())),
            // Kiwi takes the user dictionary itself (as proper nouns).
            SegmenterKind::Kiwi => {
                return Ok(Segmenter {
                    kind,
                    backend: Backend::Kiwi(kiwi::shared(user_words)?),
                    forced: None,
                    known: None,
                    fold_t2s: false,
                    fold_s2t: false,
                });
            }
        };
        let words: Vec<&str> = user_words
            .iter()
            .map(String::as_str)
            .filter(|w| !w.is_empty())
            .collect();
        let forced = if words.is_empty() {
            None
        } else {
            Some(
                aho_corasick::AhoCorasick::builder()
                    .match_kind(aho_corasick::MatchKind::LeftmostLongest)
                    .build(&words)
                    .map_err(|e| format!("User dictionary: {e}"))?,
            )
        };
        Ok(Segmenter {
            kind,
            backend,
            forced,
            known: None,
            fold_t2s,
            fold_s2t,
        })
    }

    /// Words the reader knows (NFC-normalised). Where the model split one into
    /// several tokens, they are joined back into one, but only if the word
    /// starts and ends on the model's own boundaries: 想/到 becomes 想到, while
    /// 研究/生命 stays as it is although 研究生 is known. Each word is also
    /// matched in its other script (T↔S), as known words are elsewhere.
    /// Kiwi (Korean) returns dictionary forms, not slices, so it ignores them.
    pub fn with_known(mut self, words: &[String]) -> Result<Self, String> {
        if matches!(self.backend, Backend::Kiwi(_)) {
            return Ok(self);
        }
        let mut pats: Vec<String> = words
            .iter()
            .filter(|w| w.chars().count() > 1)
            .flat_map(|w| [w.clone(), script::fold_t2s(w), script::fold_s2t(w)])
            .collect();
        pats.sort_unstable();
        pats.dedup();
        self.known = if pats.is_empty() {
            None
        } else {
            Some(aho_corasick::AhoCorasick::new(&pats).map_err(|e| format!("Known words: {e}"))?)
        };
        Ok(self)
    }

    /// Join model tokens that together spell a known word (see [`Self::with_known`]).
    /// A word inside a longer one gives way to it (一口气 over 口气). Two that
    /// partly overlap are both left alone, as the split there is ambiguous:
    /// 地/点/头 with 地点 and 点头 known stays as the model cut it.
    fn merge_known<'a>(&self, s: &'a str, toks: Vec<&'a str>) -> Vec<&'a str> {
        let Some(ac) = &self.known else { return toks };
        if toks.len() < 2 {
            return toks;
        }
        let base = s.as_ptr() as usize;
        let mut cut = vec![false; s.len() + 1];
        for t in &toks {
            cut[t.as_ptr() as usize - base] = true;
        }
        cut[s.len()] = true;
        // Known words on model boundaries that span more than one token.
        let mut cands: Vec<(usize, usize)> = ac
            .find_overlapping_iter(s)
            .map(|m| (m.start(), m.end()))
            .filter(|&(a, b)| cut[a] && cut[b] && cut[a + 1..b].iter().any(|&c| c))
            .collect();
        if cands.is_empty() {
            return toks;
        }
        // Keep the maximal ones (not inside another): sorted by start, then
        // longest first, a candidate is maximal iff it ends past all before it.
        cands.sort_unstable_by(|x, y| x.0.cmp(&y.0).then(y.1.cmp(&x.1)));
        let mut maximal: Vec<(usize, usize)> = Vec::new();
        for c in cands {
            if maximal.last().is_none_or(|m| c.1 > m.1) {
                maximal.push(c);
            }
        }
        // Maximal ones have increasing starts and ends: only neighbours can overlap.
        let mut best = vec![0usize; s.len() + 1];
        let mut any = false;
        for (i, &(a, b)) in maximal.iter().enumerate() {
            let clash_prev = i > 0 && maximal[i - 1].1 > a;
            let clash_next = maximal.get(i + 1).is_some_and(|n| n.0 < b);
            if !clash_prev && !clash_next {
                best[a] = b;
                any = true;
            }
        }
        if !any {
            return toks;
        }
        let mut out = Vec::with_capacity(toks.len());
        let mut start = 0;
        for t in toks {
            let a = t.as_ptr() as usize - base;
            if a < start {
                continue; // inside a merged word
            }
            let b = if best[a] > 0 { best[a] } else { a + t.len() };
            out.push(&s[a..b]);
            start = b;
        }
        out
    }

    /// Enforce user-dictionary words: each match becomes exactly one token
    /// (boundaries forced at its edges, removed inside it).
    fn force<'a>(&self, s: &'a str, toks: Vec<&'a str>) -> Vec<&'a str> {
        let Some(ac) = &self.forced else { return toks };
        let mut it = ac.find_iter(s).peekable();
        if it.peek().is_none() {
            return toks;
        }
        let base = s.as_ptr() as usize;
        let mut cut = vec![false; s.len() + 1];
        for t in &toks {
            cut[t.as_ptr() as usize - base] = true;
        }
        cut[s.len()] = true;
        for m in it {
            cut[m.start() + 1..m.end()]
                .iter_mut()
                .for_each(|b| *b = false);
            cut[m.start()] = true;
            cut[m.end()] = true;
        }
        let mut out = Vec::with_capacity(toks.len());
        let mut start = 0;
        for i in 1..=s.len() {
            if cut[i] {
                out.push(&s[start..i]);
                start = i;
            }
        }
        out
    }

    /// Human-readable description of the script folding in effect, if any.
    pub fn folding_note(&self) -> Option<&'static str> {
        if self.fold_t2s {
            Some(
                "Text is Traditional/Mixed: it was folded character-by-character to Simplified for the segmenter only (LTP/jieba are trained on Simplified). Counts and word lists keep the original characters.",
            )
        } else if self.fold_s2t {
            Some(
                "Text was converted to Traditional (OpenCC phrases, length-preserving) for the CKIP segmenter only (CKIP is trained on Traditional). Counts and word lists keep the original characters.",
            )
        } else {
            None
        }
    }

    /// Korean (Kiwi): the dictionary-form words of each text, analysed in
    /// parallel. Other segmenters return their tokens.
    pub fn lemmas_many(&self, texts: &[&str]) -> Result<Vec<Vec<String>>, String> {
        match &self.backend {
            Backend::Kiwi(k) => texts.par_iter().map(|t| k.lemmas(t)).collect(),
            _ => Ok(self
                .cut_many(texts)
                .into_iter()
                .map(|v| v.into_iter().map(str::to_owned).collect())
                .collect()),
        }
    }

    /// Segment many whitespace-free chunks; result `i` tiles `chunks[i]`.
    pub fn cut_many<'a>(&self, chunks: &[&'a str]) -> Vec<Vec<&'a str>> {
        match &self.backend {
            Backend::Ckip(m) => m
                .segment_many(chunks, self.fold_s2t)
                .into_iter()
                .zip(chunks)
                .map(|(toks, c)| {
                    let toks = if covers(c, &toks) {
                        toks
                    } else {
                        c.split_inclusive(|_| true).collect()
                    };
                    self.force(c, self.merge_known(c, toks))
                })
                .collect(),
            _ => chunks
                .iter()
                .map(|c| self.force(c, self.merge_known(c, self.cut_one(c))))
                .collect(),
        }
    }

    fn cut_one<'a>(&self, orig: &'a str) -> Vec<&'a str> {
        if !self.fold_t2s {
            return self.model_cut(orig);
        }
        let folded = script::fold_t2s(orig);
        let toks = self.model_cut(&folded);
        // byte offset in folded -> char index -> byte offset in orig (char counts are equal)
        let offs: Vec<usize> = orig
            .char_indices()
            .map(|(i, _)| i)
            .chain([orig.len()])
            .collect();
        let mut out = Vec::with_capacity(toks.len());
        let mut ci = 0usize;
        for t in toks {
            let n = t.chars().count();
            let end = (ci + n).min(offs.len() - 1);
            out.push(&orig[offs[ci]..offs[end]]);
            ci = end;
        }
        out
    }

    /// Model output on text already in model script. Result tiles `s`.
    fn model_cut<'a>(&self, s: &'a str) -> Vec<&'a str> {
        let toks: Vec<&'a str> = match &self.backend {
            Backend::Ltp(m) => m
                .predict(s)
                .unwrap_or_else(|_| s.split_inclusive(|_| true).collect()),
            Backend::Jieba(j) => j.cut(s, false).into_iter().map(|t| t.word).collect(),
            Backend::Ckip(m) => m
                .segment_many(&[s], self.fold_s2t)
                .pop()
                .unwrap_or_default(),
            // Kiwi output is lemmas, not slices (see `lemmas_many`).
            Backend::Kiwi(_) => s.split_inclusive(|_| true).collect(),
        };
        if covers(s, &toks) {
            toks
        } else {
            s.split_inclusive(|_| true).collect()
        }
    }
}

/// True if `toks` are consecutive slices that exactly tile `s`.
fn covers(s: &str, toks: &[&str]) -> bool {
    let base = s.as_ptr() as usize;
    let mut pos = 0usize;
    for t in toks {
        if t.as_ptr() as usize != base + pos {
            return false;
        }
        pos += t.len();
    }
    pos == s.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Byte offsets where `toks` (slices of `s`) start, plus the end.
    fn cuts(s: &str, toks: &[&str]) -> Vec<usize> {
        let base = s.as_ptr() as usize;
        let mut v: Vec<usize> = toks.iter().map(|t| t.as_ptr() as usize - base).collect();
        v.push(s.len());
        v
    }

    #[test]
    fn known_words_only_join_model_tokens() {
        let s = "我们今天下午一起去图书馆看书，然后研究生命的起源问题。";
        let plain = Segmenter::new(SegmenterKind::Jieba, &[], Variant::Simplified).unwrap();
        let base = plain.cut_many(&[s]).pop().unwrap();
        let i = base.iter().position(|t| t.chars().count() > 1).unwrap();
        // Two adjacent model tokens spell a known word: joined.
        let joined = format!("{}{}", base[i], base[i + 1]);
        // A word that crosses a model boundary in the middle of a token: ignored.
        let crossing = format!(
            "{}{}",
            base[i].chars().last().unwrap(),
            base[i + 1].chars().next().unwrap()
        );
        let seg = Segmenter::new(SegmenterKind::Jieba, &[], Variant::Simplified)
            .unwrap()
            .with_known(&[joined.clone(), crossing.clone()])
            .unwrap();
        let out = seg.cut_many(&[s]).pop().unwrap();
        assert!(covers(s, &out));
        assert!(out.contains(&joined.as_str()), "{out:?}");
        let (b, o) = (cuts(s, &base), cuts(s, &out));
        assert!(
            o.iter().all(|c| b.contains(c)),
            "new boundary: {base:?} -> {out:?}"
        );
        assert_eq!(out.len(), base.len() - 1);
    }

    #[test]
    fn overlapping_known_words_keep_model_split() {
        let s = "我们今天下午一起去图书馆看书";
        let plain = Segmenter::new(SegmenterKind::Jieba, &[], Variant::Simplified).unwrap();
        let base = plain.cut_many(&[s]).pop().unwrap();
        assert!(base.len() >= 4, "{base:?}");
        let (x, y, z) = (base[1], base[2], base[3]);
        let seg = Segmenter::new(SegmenterKind::Jieba, &[], Variant::Simplified)
            .unwrap()
            .with_known(&[
                format!("{x}{y}"),
                format!("{y}{z}"),
                format!("{}{}", base[0], x),
            ])
            .unwrap();
        // 0+1 and 1+2 clash, 1+2 and 2+3 clash: nothing joins.
        assert_eq!(seg.cut_many(&[s]).pop().unwrap(), base);
        // A word containing both wins over them.
        let seg = Segmenter::new(SegmenterKind::Jieba, &[], Variant::Simplified)
            .unwrap()
            .with_known(&[format!("{x}{y}"), format!("{y}{z}"), format!("{x}{y}{z}")])
            .unwrap();
        let out = seg.cut_many(&[s]).pop().unwrap();
        assert!(out.contains(&format!("{x}{y}{z}").as_str()), "{out:?}");
    }

    #[test]
    fn known_words_match_other_script() {
        let s = "她只看了一瞬间就走了";
        let plain = Segmenter::new(SegmenterKind::Jieba, &[], Variant::Simplified).unwrap();
        let base = plain.cut_many(&[s]).pop().unwrap();
        let seg = Segmenter::new(SegmenterKind::Jieba, &[], Variant::Simplified)
            .unwrap()
            .with_known(&["一瞬間".to_owned()])
            .unwrap();
        let out = seg.cut_many(&[s]).pop().unwrap();
        // Joined when jieba split it along its own boundaries.
        let starts = cuts(s, &base);
        let a = s.find("一瞬间").unwrap();
        if starts.contains(&a) && starts.contains(&(a + "一瞬间".len())) {
            assert!(out.contains(&"一瞬间"), "{base:?} -> {out:?}");
        }
    }
}
