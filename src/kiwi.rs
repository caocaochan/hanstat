//! Korean analysis with Kiwi (bab2min/Kiwi, Apache-2.0) through its C API.
//!
//! `kiwi.dll` and the CoNg base model are downloaded on first use
//! ([`crate::models::KIWI`]) and the DLL is loaded at run time, so building
//! hanstat never needs Kiwi and the Chinese modes never load it.
//!
//! Words are counted as dictionary forms (lemmas): Kiwi restores irregular
//! stems (걸어 → 걷/VV, 도와 → 돕/VV), predicates get 다 (먹었어요 → 먹다), and a
//! noun, root or adverb followed by the 하/되 derivation is one headword
//! (공부했다 → 공부하다, 깨끗해 → 깨끗하다). Particles, endings and affixes are
//! grammar, not words.

use crate::models;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

type H = *mut c_void;

/// `kiwi_analyze_option_t` (Kiwi v0.24 `include/kiwi/capi.h`).
#[repr(C)]
#[derive(Clone, Copy)]
struct AnalyzeOption {
    match_options: c_int,
    blocklist: H,
    open_ending: c_int,
    allowed_dialects: c_int,
    dialect_cost: f32,
    typo_transformer: H,
    typo_threshold: f32,
}

/// The C functions used, resolved once from the DLL.
struct Api {
    _lib: libloading::Library,
    builder_init: unsafe extern "C" fn(*const c_char, c_int, c_int, c_int) -> H,
    builder_add_word: unsafe extern "C" fn(H, *const c_char, *const c_char, f32) -> c_int,
    builder_build: unsafe extern "C" fn(H, H, f32) -> H,
    builder_close: unsafe extern "C" fn(H) -> c_int,
    analyze: unsafe extern "C" fn(H, *const c_char, c_int, AnalyzeOption, H) -> H,
    close: unsafe extern "C" fn(H) -> c_int,
    res_word_num: unsafe extern "C" fn(H, c_int) -> c_int,
    res_form: unsafe extern "C" fn(H, c_int, c_int) -> *const c_char,
    res_tag: unsafe extern "C" fn(H, c_int, c_int) -> *const c_char,
    res_word_position: unsafe extern "C" fn(H, c_int, c_int) -> c_int,
    res_close: unsafe extern "C" fn(H) -> c_int,
    error: unsafe extern "C" fn() -> *const c_char,
}

/// `KIWI_BUILD_DEFAULT`: integrate allomorphs, load the default, typo and multi-word dictionaries.
const BUILD_DEFAULT: c_int = 15;
/// `KIWI_MATCH_ALL_WITH_NORMALIZING`: URL|EMAIL|HASHTAG|MENTION|SERIAL|EMOJI|Z_CODA|NORMALIZE_CODA.
const MATCH_ALL_WITH_NORMALIZING: c_int = 0x3F | (1 << 23) | (1 << 16);

static API: OnceLock<Result<Api, String>> = OnceLock::new();

fn api(dir: &Path) -> Result<&'static Api, String> {
    API.get_or_init(|| load_api(&dir.join("kiwi.dll")))
        .as_ref()
        .map_err(Clone::clone)
}

fn load_api(dll: &Path) -> Result<Api, String> {
    let fail = |e: libloading::Error| format!("Cannot load {}: {e}", dll.display());
    // SAFETY: kiwi.dll is the SHA-256-verified Kiwi v0.24.1 release build; its
    // initialisers have no requirements, and each symbol's type matches capi.h.
    unsafe {
        let lib = libloading::Library::new(dll).map_err(fail)?;
        macro_rules! sym {
            ($name:literal) => {
                *lib.get(concat!($name, "\0").as_bytes()).map_err(fail)?
            };
        }
        Ok(Api {
            builder_init: sym!("kiwi_builder_init"),
            builder_add_word: sym!("kiwi_builder_add_word"),
            builder_build: sym!("kiwi_builder_build"),
            builder_close: sym!("kiwi_builder_close"),
            analyze: sym!("kiwi_analyze"),
            close: sym!("kiwi_close"),
            res_word_num: sym!("kiwi_res_word_num"),
            res_form: sym!("kiwi_res_form"),
            res_tag: sym!("kiwi_res_tag"),
            res_word_position: sym!("kiwi_res_word_position"),
            res_close: sym!("kiwi_res_close"),
            error: sym!("kiwi_error"),
            _lib: lib,
        })
    }
}

/// One morpheme of an analysis.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Morph {
    pub form: String,
    /// Sejong-style tag without Kiwi's regular/irregular suffix (`VV-I` → `VV`).
    pub tag: String,
    /// Index of the eojeol (space-separated unit) it belongs to.
    pub word: u32,
}

impl Morph {
    pub fn new(form: &str, tag: &str, word: u32) -> Self {
        Morph {
            form: form.to_owned(),
            tag: tag.to_owned(),
            word,
        }
    }
}

/// A built analyser. Read-only after building; Kiwi analyses concurrently.
pub struct Kiwi {
    api: &'static Api,
    handle: H,
}

// SAFETY: the handle is only used through `kiwi_analyze`, which Kiwi supports
// from several threads at once (its own thread pool shares one instance).
unsafe impl Send for Kiwi {}
unsafe impl Sync for Kiwi {}

impl Drop for Kiwi {
    fn drop(&mut self) {
        // SAFETY: the handle came from kiwi_builder_build and is closed once.
        unsafe { (self.api.close)(self.handle) };
    }
}

fn last_error(api: &Api) -> String {
    // SAFETY: kiwi_error returns null or a NUL-terminated thread-local message.
    unsafe {
        let p = (api.error)();
        if p.is_null() {
            "unknown error".into()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}

/// A user-dictionary entry Kiwi accepts: Korean, no spaces.
fn user_word_ok(w: &str) -> bool {
    !w.is_empty() && !w.chars().any(char::is_whitespace) && w.chars().any(crate::chars::is_hangul)
}

impl Kiwi {
    /// Build from the model folder (`kiwi.dll` plus the model files), with
    /// `user_words` added as proper nouns so they are kept whole.
    pub fn build(dir: &Path, user_words: &[String]) -> Result<Kiwi, String> {
        let api = api(dir)?;
        let model = CString::new(dir.to_string_lossy().as_bytes())
            .map_err(|_| format!("Invalid model folder {}", dir.display()))?;
        let nnp = CString::new("NNP").unwrap();
        // SAFETY: arguments follow capi.h; the builder is closed on every path.
        unsafe {
            let b = (api.builder_init)(model.as_ptr(), 1, BUILD_DEFAULT, 0);
            if b.is_null() {
                return Err(format!(
                    "Kiwi could not load its model: {}",
                    last_error(api)
                ));
            }
            for w in user_words.iter().filter(|w| user_word_ok(w)) {
                if let Ok(c) = CString::new(w.as_str()) {
                    // A word Kiwi rejects (e.g. a duplicate) is skipped.
                    (api.builder_add_word)(b, c.as_ptr(), nnp.as_ptr(), 0.0);
                }
            }
            let h = (api.builder_build)(b, std::ptr::null_mut(), 0.0);
            (api.builder_close)(b);
            if h.is_null() {
                return Err(format!(
                    "Kiwi could not build its analyser: {}",
                    last_error(api)
                ));
            }
            Ok(Kiwi { api, handle: h })
        }
    }

    /// The best analysis of `text` (a paragraph or a field value).
    pub fn analyze(&self, text: &str) -> Result<Vec<Morph>, String> {
        let Ok(t) = CString::new(text) else {
            return Ok(Vec::new());
        };
        let opt = AnalyzeOption {
            match_options: MATCH_ALL_WITH_NORMALIZING,
            blocklist: std::ptr::null_mut(),
            open_ending: 0,
            allowed_dialects: 0,
            dialect_cost: 3.0,
            typo_transformer: std::ptr::null_mut(),
            typo_threshold: 2.5,
        };
        let api = self.api;
        let s = |p: *const c_char| {
            if p.is_null() {
                String::new()
            } else {
                // SAFETY: Kiwi returns NUL-terminated UTF-8 owned by the result.
                unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
            }
        };
        // SAFETY: arguments follow capi.h; the result is closed before returning.
        unsafe {
            let r = (api.analyze)(self.handle, t.as_ptr(), 1, opt, std::ptr::null_mut());
            if r.is_null() {
                return Err(format!("Kiwi analysis failed: {}", last_error(api)));
            }
            let n = (api.res_word_num)(r, 0).max(0);
            let mut out = Vec::with_capacity(n as usize);
            for i in 0..n {
                let tag = s((api.res_tag)(r, 0, i));
                out.push(Morph {
                    form: s((api.res_form)(r, 0, i)),
                    tag: tag.split('-').next().unwrap_or("").to_owned(),
                    word: (api.res_word_position)(r, 0, i).max(0) as u32,
                });
            }
            (api.res_close)(r);
            Ok(out)
        }
    }

    /// Dictionary-form words of `text` (see [`lemmas`]).
    pub fn lemmas(&self, text: &str) -> Result<Vec<String>, String> {
        Ok(lemmas(&self.analyze(text)?))
    }
}

/// Content words of an analysis as dictionary forms:
/// * nouns, pronouns, numerals, determiners, adverbs, interjections and roots as written
///   (the bound noun 거 as 것, its standard form);
/// * verbs, adjectives, auxiliaries and 아니다 as stem + 다;
/// * a noun, root, bound noun or adverb directly followed (in the same eojeol) by the
///   verb/adjective-forming 하/되 suffix as one headword (공부 + 하 → 공부하다,
///   깨끗 + 하 → 깨끗하다, 듯 + 하 → 듯하다).
///
/// Particles, endings, the copula 이다, prefixes, suffixes, symbols and
/// foreign/Hanja/number tokens are not words.
pub fn lemmas(ms: &[Morph]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // Index of the morpheme behind the last lemma, if it can take a 하/되 suffix.
    let mut base: Option<usize> = None;
    for (i, m) in ms.iter().enumerate() {
        let joins = base.is_some_and(|j| j + 1 == i && ms[j].word == m.word);
        let mut next_base = None;
        match m.tag.as_str() {
            "NNB" if m.form == "거" => out.push("것".to_owned()),
            "NNG" | "NNP" | "NNB" | "XR" | "MAG" => {
                out.push(m.form.clone());
                next_base = Some(i);
            }
            "NR" | "NP" | "MM" | "MAJ" | "IC" => out.push(m.form.clone()),
            "VV" | "VA" | "VX" | "VCN" => out.push(format!("{}다", m.form)),
            "XSV" | "XSA" if joins => {
                let stem = out.pop().unwrap_or_default();
                out.push(format!("{stem}{}다", m.form));
            }
            _ => {}
        }
        base = next_base;
    }
    out
}

/// The analyser for `user_words`. Building one takes a couple of seconds and
/// several hundred MB, so only the latest is kept: the book and the Anki
/// sentences share it, and a new dictionary replaces it.
pub fn shared(user_words: &[String]) -> Result<Arc<Kiwi>, String> {
    static CURRENT: Mutex<Option<(Vec<String>, Arc<Kiwi>)>> = Mutex::new(None);
    let mut g = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((w, k)) = &*g
        && w.as_slice() == user_words
    {
        return Ok(k.clone());
    }
    // Free the old one before building (unless a caller still holds it).
    *g = None;
    let dir = models::KIWI.ensure(&|_, _| {})?;
    let k = Arc::new(Kiwi::build(&dir, user_words)?);
    *g = Some((user_words.to_vec(), k.clone()));
    Ok(k)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(spec: &str) -> Vec<Morph> {
        // "form/TAG form/TAG | form/TAG": `|` starts the next eojeol.
        let mut w = 0;
        let mut v = Vec::new();
        for t in spec.split_whitespace() {
            if t == "|" {
                w += 1;
                continue;
            }
            let (f, tag) = t.rsplit_once('/').unwrap();
            v.push(Morph::new(f, tag, w));
        }
        v
    }

    #[test]
    fn lemma_rules() {
        // 그는 학교까지 걸어서 갔다.
        assert_eq!(
            lemmas(&m(
                "그/NP 는/JX | 학교/NNG 까지/JX | 걷/VV 어서/EC | 가/VV 었/EP 다/EF ./SF"
            )),
            ["그", "학교", "걷다", "가다"]
        );
        // 열심히 공부했다. 방이 깨끗해졌다.
        assert_eq!(
            lemmas(&m("열심히/MAG | 공부/NNG 하/XSV 었/EP 다/EF ./SF")),
            ["열심히", "공부하다"]
        );
        assert_eq!(
            lemmas(&m("방/NNG 이/JKS | 깨끗/XR 하/XSA 어/EC 지/VX 었/EP 다/EF")),
            ["방", "깨끗하다", "지다"]
        );
        // 웃는 듯한 표정: bound noun + 하 is the auxiliary 듯하다.
        assert_eq!(
            lemmas(&m("웃/VV 는/ETM | 듯/NNB 하/XSA ㄴ/ETM | 표정/NNG")),
            ["웃다", "듯하다", "표정"]
        );
        // A 하 suffix in the next eojeol (mis-spaced) does not join.
        assert_eq!(lemmas(&m("공부/NNG | 하/XSV 었/EP 다/EF")), ["공부"]);
        // 걸 = 것을, written 거 by Kiwi; copula and particles are grammar.
        assert_eq!(
            lemmas(&m(
                "거/NNB 을/JKO | 알/VV 았/EP 다/EF | 학생/NNG 이/VCP 다/EF"
            )),
            ["것", "알다", "학생"]
        );
        // Foreign words, numbers and Hanja are not Korean words.
        assert_eq!(lemmas(&m("404/SN 호/NNB | OK/SL | 漢字/SH")), ["호"]);
    }

    #[test]
    fn user_words_filter() {
        assert!(user_word_ok("하남"));
        assert!(!user_word_ok("하 남"));
        assert!(!user_word_ok("温简言"));
        assert!(!user_word_ok(""));
    }
}
