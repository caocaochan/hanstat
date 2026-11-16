//! Japanese analysis with Sudachi (sudachi.rs, Apache-2.0) and SudachiDict
//! core (Apache-2.0), split mode C.
//!
//! The dictionary is downloaded on first use ([`crate::models::SUDACHI`]), so
//! the Chinese and Korean modes never load it.
//!
//! A word is keyed by Sudachi's normalized form, which merges spellings and
//! inflections (わかった, 分かる → 分かる; かわいく → 可愛い; ウソ → うそ),
//! so Anki cards, word lists and the JLPT list match however the book writes
//! a word. Each word also keeps the book's own spelling (its dictionary form,
//! as written) for display, and, for a long unit such as 冒険者 or 生徒会, the
//! keys of its short-unit (mode A) parts (冒険 + 者): a long unit that is not
//! known itself counts as known when all its parts are.
//!
//! Before analysis, [`filter`] removes dialogue noise Sudachi misreads:
//! emphatic small っ, stretched vowels and stutters (see there). Particles,
//! auxiliaries, punctuation, numbers in digits and Latin words are not words.

use crate::chars;
use crate::models;
use rayon::prelude::*;
use std::path::Path;
use std::sync::{Arc, Mutex};
use sudachi::analysis::Mode;
use sudachi::analysis::mlist::MorphemeList;
use sudachi::analysis::stateful_tokenizer::StatefulTokenizer;
use sudachi::config::Config;
use sudachi::dic::build::DictBuilder;
use sudachi::dic::dictionary::JapaneseDictionary;
use sudachi::dic::storage::{Storage, SudachiDicData};

/// One word of the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Word {
    /// Sudachi's normalized form (numerals: as written).
    pub key: String,
    /// The dictionary form as the text spells it (わかる for わかった).
    pub spelling: String,
    /// Keys of the short-unit words a long unit is made of; empty if it is one.
    pub parts: Vec<String>,
}

/// Sudachi rejects input over 49,149 bytes; paragraphs are cut well below that.
const MAX_CHUNK: usize = 16 * 1024;

/// Connection ids and cost of SudachiDict core's 名詞,固有名詞,一般 entries
/// (as ディズニーランド), used for user-dictionary words.
const PROPER_NOUN_ID: i16 = 4785;
const USER_WORD_COST: i16 = 4000;

/// A loaded dictionary. Read-only; analysed from several threads at once.
pub struct Analyzer {
    dict: JapaneseDictionary,
}

impl Analyzer {
    /// Load `system_core.dic` from `dir`, with `user_words` added as proper
    /// nouns so they are kept whole.
    pub fn build(dir: &Path, user_words: &[String]) -> Result<Analyzer, String> {
        let dic = dir.join("system_core.dic");
        let cfg = Config::new(None, None, Some(dic.clone()))
            .map_err(|e| format!("Sudachi configuration: {e}"))?;
        let system = JapaneseDictionary::from_cfg(&cfg)
            .map_err(|e| format!("Cannot load the Sudachi dictionary {}: {e}", dic.display()))?;
        let csv = user_csv(user_words);
        if csv.is_empty() {
            return Ok(Analyzer { dict: system });
        }
        let mut b = DictBuilder::new_user(&system);
        let user = (|| {
            b.read_lexicon(csv.as_bytes())?;
            b.resolve()?;
            let mut out = Vec::new();
            b.compile(&mut out)?;
            Ok::<_, sudachi::error::SudachiError>(out)
        })()
        .map_err(|e| format!("Sudachi user dictionary: {e}"))?;
        drop(b);
        drop(system);
        let file = std::fs::File::open(&dic).map_err(|e| format!("{}: {e}", dic.display()))?;
        // SAFETY: the dictionary file is only read, and hanstat never writes it
        // while it is mapped (it is replaced only by a new download folder).
        let map =
            unsafe { memmap2::Mmap::map(&file) }.map_err(|e| format!("{}: {e}", dic.display()))?;
        let mut data = SudachiDicData::new(Storage::File(map));
        data.add_user(Storage::Owned(user));
        let dict = JapaneseDictionary::from_cfg_storage(&cfg, data)
            .map_err(|e| format!("Sudachi user dictionary: {e}"))?;
        Ok(Analyzer { dict })
    }

    /// The words of each text, analysed in parallel.
    pub fn words_many(&self, texts: &[&str]) -> Vec<Vec<Word>> {
        texts
            .par_iter()
            .map_init(
                || {
                    (
                        StatefulTokenizer::new(&self.dict, Mode::C),
                        MorphemeList::empty(&self.dict),
                        MorphemeList::empty(&self.dict),
                    )
                },
                |(tok, ml, sub), t| {
                    let mut out = Vec::new();
                    for chunk in chunks(&filter(t)) {
                        tok.reset().push_str(chunk);
                        if tok.do_tokenize().is_err() || ml.collect_results(tok).is_err() {
                            continue;
                        }
                        for i in 0..ml.len() {
                            let m = ml.get(i);
                            let surface = m.surface();
                            let Some(key) =
                                key_of(&surface, m.part_of_speech(), m.normalized_form())
                            else {
                                continue;
                            };
                            let mut parts = Vec::new();
                            // `split_into` appends: a reused list must be emptied first.
                            sub.clear();
                            if ml.split_into(Mode::A, i, sub).unwrap_or(false) && sub.len() > 1 {
                                for p in sub.iter() {
                                    if let Some(k) = key_of(
                                        &p.surface(),
                                        p.part_of_speech(),
                                        p.normalized_form(),
                                    ) {
                                        parts.push(k);
                                    }
                                }
                                if parts.len() < 2 {
                                    parts.clear();
                                }
                            }
                            out.push(Word {
                                key,
                                spelling: m.dictionary_form().to_owned(),
                                parts,
                            });
                        }
                    }
                    out
                },
            )
            .collect()
    }

    /// The keys of the words in `text` (a word list entry, a field).
    pub fn keys(&self, text: &str) -> Vec<String> {
        self.words_many(&[text])
            .pop()
            .unwrap_or_default()
            .into_iter()
            .map(|w| w.key)
            .collect()
    }
}

/// A Japanese character: kana or a kanji.
fn japanese(c: char) -> bool {
    chars::is_han(c) || chars::is_kana(c)
}

/// The key of a morpheme, or `None` if it is not a word: punctuation,
/// particles and auxiliaries are grammar; digits and Latin are not Japanese.
/// Numerals keep the text's spelling (Sudachi writes 二十四 as 24).
fn key_of(surface: &str, pos: &[String], normalized: &str) -> Option<String> {
    if matches!(
        pos[0].as_str(),
        "補助記号" | "空白" | "助詞" | "助動詞" | "記号"
    ) || !surface.chars().any(japanese)
    {
        return None;
    }
    if pos.get(1).is_some_and(|p| p == "数詞") {
        Some(surface.to_owned())
    } else {
        Some(normalized.to_owned())
    }
}

/// Pieces of `text` under [`MAX_CHUNK`] bytes, cut after sentence ends where possible.
fn chunks(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while rest.len() > MAX_CHUNK {
        let mut cut = rest.floor_char_boundary(MAX_CHUNK);
        if let Some((i, c)) = rest[..cut]
            .char_indices()
            .rev()
            .find(|(_, c)| chars::is_sentence_end(*c) || *c == '\n')
        {
            cut = i + c.len_utf8();
        }
        out.push(&rest[..cut]);
        rest = &rest[cut..];
    }
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

fn hira(c: char) -> bool {
    matches!(c, '\u{3041}'..='\u{3096}')
}

fn kana_letter(c: char) -> bool {
    hira(c) || matches!(c, '\u{30A1}'..='\u{30FA}')
}

/// Removes dialogue noise that Sudachi misreads (tested on five novels: of
/// 81 changed places judged by hand, 49 better, 6 worse, the rest equal):
/// * an emphatic small っ ending a word of two or more characters (ですっ」,
///   寒いっ！) — read as part of a made-up word otherwise;
/// * hiragana small vowels after kana (ますぅ, もっとぉ), and a long mark or a
///   wave dash inside hiragana (まーす, かんぱーい, ま〜す), when the kana run
///   before them is two or more characters long, so that short interjections
///   (はぁ, えー, へぇ, えーと) stay as they are;
/// * a stutter: a lone kana at the start of a clause, then 、, then the same
///   kana or a kanji (い、いいんですか; こ、これ; し、知る), except あ え お ん
///   before a kanji (interjections) — read as 行く, 来る, する otherwise.
pub fn filter(p: &str) -> String {
    let c: Vec<char> = p.chars().collect();
    let n = c.len();
    let mut del = vec![false; n];
    let prev_kept = |del: &[bool], i: usize| (0..i).rev().find(|&j| !del[j]).map(|j| c[j]);
    let run_before = |del: &[bool], i: usize| {
        (0..i)
            .rev()
            .filter(|&j| !del[j])
            .take_while(|&j| kana_letter(c[j]))
            .count()
    };
    for i in 1..n {
        if run_before(&del, i) < 2 {
            continue;
        }
        let pk = prev_kept(&del, i);
        let small_vowel =
            matches!(c[i], 'ぁ' | 'ぃ' | 'ぅ' | 'ぇ' | 'ぉ') && pk.is_some_and(kana_letter);
        // A wave dash only inside a word (ま〜す), not as trailing punctuation.
        let long_mark = matches!(c[i], 'ー' | '〜' | '～')
            && pk.is_some_and(hira)
            && (c[i] == 'ー' || c.get(i + 1).copied().is_some_and(hira));
        if small_vowel || long_mark {
            del[i] = true;
        }
    }
    for i in 2..n {
        if !matches!(c[i], 'っ' | 'ッ') {
            continue;
        }
        let before: Vec<usize> = (0..i).rev().filter(|&j| !del[j]).take(2).collect();
        let next = (i + 1..n).find(|&j| !del[j]).map(|j| c[j]);
        let word_before = before.len() == 2
            && kana_letter(c[before[0]])
            && (kana_letter(c[before[1]]) || chars::is_han(c[before[1]]));
        if word_before && next.is_none_or(|x| !(kana_letter(x) || chars::is_han(x))) {
            del[i] = true;
        }
    }
    for i in 0..n.saturating_sub(2) {
        let start = i == 0
            || matches!(
                c[i - 1],
                '「' | '『'
                    | '（'
                    | '('
                    | '…'
                    | '―'
                    | '─'
                    | '、'
                    | '！'
                    | '？'
                    | '!'
                    | '?'
                    | '　'
                    | ' '
                    | '‥'
            );
        if start && kana_letter(c[i]) && c[i + 1] == '、' && !del[i] {
            let nx = c[i + 2];
            if nx == c[i]
                || (chars::is_han(nx)
                    && !matches!(c[i], 'あ' | 'え' | 'お' | 'ん' | 'ア' | 'エ' | 'オ' | 'ン'))
            {
                del[i] = true;
            }
        }
    }
    c.iter()
        .zip(&del)
        .filter(|(_, d)| !**d)
        .map(|(c, _)| *c)
        .collect()
}

/// v0 lexicon rows for the user words Sudachi can take: Japanese, no spaces,
/// commas or quotes. Each is a proper noun with the dictionary's own ids.
fn user_csv(user_words: &[String]) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut s = String::new();
    for w in user_words {
        let w = w.trim();
        if w.is_empty()
            || !w.chars().any(japanese)
            || w.chars()
                .any(|c| c.is_whitespace() || matches!(c, ',' | '"'))
            || !seen.insert(w)
        {
            continue;
        }
        s.push_str(&format!(
            "{w},{PROPER_NOUN_ID},{PROPER_NOUN_ID},{USER_WORD_COST},{w},名詞,固有名詞,一般,*,*,*,*,{w},*,A,*,*,*,*\n"
        ));
    }
    s
}

/// The analyser for `user_words`. Loading is quick (the dictionary is mapped),
/// but a user dictionary takes a couple of seconds to build, so only the latest
/// is kept: the book, Anki fields and word lists share it.
pub fn shared(user_words: &[String]) -> Result<Arc<Analyzer>, String> {
    let mut g = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((w, a)) = &*g
        && w.as_slice() == user_words
    {
        return Ok(a.clone());
    }
    *g = None;
    let dir = models::SUDACHI.ensure(&|_, _| {})?;
    let a = Arc::new(Analyzer::build(&dir, user_words)?);
    *g = Some((user_words.to_vec(), a.clone()));
    Ok(a)
}

/// The keys of the words in each of `texts` (word-list entries), using the
/// latest analyser; each text as it is if Sudachi can't be loaded.
pub fn keys_each(texts: &[&str]) -> Vec<Vec<String>> {
    if texts.is_empty() {
        return Vec::new();
    }
    match shared_any() {
        Some(a) => a
            .words_many(texts)
            .into_iter()
            .map(|v| v.into_iter().map(|w| w.key).collect())
            .collect(),
        None => texts.iter().map(|t| vec![t.to_string()]).collect(),
    }
}

static CURRENT: Mutex<Option<(Vec<String>, Arc<Analyzer>)>> = Mutex::new(None);

/// The current analyser whatever its user words (they only add names), or
/// one without any.
fn shared_any() -> Option<Arc<Analyzer>> {
    if let Some((_, a)) = &*CURRENT.lock().unwrap_or_else(|e| e.into_inner()) {
        return Some(a.clone());
    }
    shared(&[]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_rules() {
        // Emphatic small tsu after a word; not in えっ or inside a word.
        assert_eq!(
            filter("「ありがとうございますっ」"),
            "「ありがとうございます」"
        );
        assert_eq!(filter("寒いっ！"), "寒い！");
        assert_eq!(filter("「えっ」"), "「えっ」");
        assert_eq!(filter("ちっさい"), "ちっさい");
        // Stretched vowels and long marks after two or more kana only.
        assert_eq!(filter("行ってきまーす"), "行ってきます");
        assert_eq!(filter("かんぱーい"), "かんぱい");
        assert_eq!(filter("ますぅ……"), "ます……");
        assert_eq!(filter("「はぁ」"), "「はぁ」");
        assert_eq!(filter("えーと、"), "えーと、");
        // Katakana long marks are spelling.
        assert_eq!(filter("マネージャー"), "マネージャー");
        // Stutters.
        assert_eq!(filter("「い、いいんですか」"), "「、いいんですか」");
        assert_eq!(filter("「こ、これって」"), "「、これって」");
        assert_eq!(filter("「ち、違う」"), "「、違う」");
        assert_eq!(filter("「あ、先生」"), "「あ、先生」");
        assert_eq!(filter("「う、うん」"), "「、うん」");
    }

    #[test]
    fn keys_and_grammar() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(
            key_of("食べ", &s(&["動詞", "一般"]), "食べる"),
            Some("食べる".into())
        );
        assert_eq!(key_of("が", &s(&["助詞", "格助詞"]), "が"), None);
        assert_eq!(key_of("た", &s(&["助動詞", "*"]), "た"), None);
        assert_eq!(key_of("。", &s(&["補助記号", "句点"]), "。"), None);
        assert_eq!(
            key_of("二十四", &s(&["名詞", "数詞"]), "24"),
            Some("二十四".into())
        );
        assert_eq!(key_of("24", &s(&["名詞", "数詞"]), "24"), None);
        assert_eq!(key_of("PPV", &s(&["名詞", "普通名詞"]), "ppv"), None);
    }

    #[test]
    fn long_text_is_cut_at_sentences() {
        let s = "あいうえお。".repeat(4000);
        let c = chunks(&s);
        assert!(c.len() > 1);
        assert!(c.iter().all(|x| x.len() <= MAX_CHUNK && x.ends_with('。')));
        assert_eq!(c.concat(), s);
    }

    #[test]
    fn user_words_csv() {
        let csv = user_csv(&[
            "ぬるま湯ぬい".into(),
            "a b".into(),
            "Hello".into(),
            "ぬるま湯ぬい".into(),
        ]);
        assert_eq!(csv.lines().count(), 1);
        assert!(csv.starts_with("ぬるま湯ぬい,4785,4785,"));
    }
}
