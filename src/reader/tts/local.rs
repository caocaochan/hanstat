//! The local voice: Qwen3-TTS 1.7B (Alibaba Qwen, Apache-2.0),
//! Q8_0 GGUF, run on the GPU by qwentts.cpp's `tts-server`
//! ([`hanstat::models::QWEN_TTS`], [`hanstat::models::QWEN_TTS_MODEL`]),
//! nothing downloaded until the user asks.
//!
//! One server for the whole app, kept until the model changes, hanstat
//! closes or the user chooses Unload. It batches up
//! to [`LocalSettings::parallel`] requests on the GPU: on an RTX 5070 (Vulkan,
//! 2026-11-19) one request ran at 6.5× real time, four at once at 17.9×.
//!
//! Text goes in as chunks of whole paragraphs ([`chunks`]), not sentence by
//! sentence: sentences read alone came out with uneven liveliness. Each
//! chunk's audio is then cut back into one clip per sentence at its pauses
//! ([`cuts`]), so the reader still highlights and starts per sentence.

use super::azure::Fail;
use super::engine::Repaint;
use super::opus;
use hanstat::models::{
    Model, QWEN_TTS, QWEN_TTS_CODEC, QWEN_TTS_DESIGN, QWEN_TTS_MODEL, QWEN_TTS_SERVER,
};
use serde::{Deserialize, Serialize};
use std::ops::Range;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Condvar, Mutex, RwLock};
use std::time::{Duration, Instant};

mod voices;
pub use voices::{SavedVoice, VoiceStore};

/// A named voice of the CustomVoice model: its id, what it sounds like (the
/// model card's descriptions) and its own language.
pub struct Speaker {
    pub id: &'static str,
    pub name: &'static str,
    pub sound: &'static str,
    pub native: &'static str,
}

pub const SPEAKERS: &[Speaker] = &[
    Speaker {
        id: "vivian",
        name: "Vivian",
        sound: "bright young woman, a little edgy",
        native: "Mandarin",
    },
    Speaker {
        id: "serena",
        name: "Serena",
        sound: "warm, gentle young woman",
        native: "Mandarin",
    },
    Speaker {
        id: "uncle_fu",
        name: "Uncle Fu",
        sound: "seasoned man, low and mellow",
        native: "Mandarin",
    },
    Speaker {
        id: "dylan",
        name: "Dylan",
        sound: "youthful man, clear and natural",
        native: "Beijing Mandarin",
    },
    Speaker {
        id: "eric",
        name: "Eric",
        sound: "lively man, slightly husky",
        native: "Sichuanese (speaks in dialect)",
    },
    Speaker {
        id: "ryan",
        name: "Ryan",
        sound: "dynamic man, strong rhythm",
        native: "English",
    },
    Speaker {
        id: "aiden",
        name: "Aiden",
        sound: "sunny American man, clear midrange",
        native: "English",
    },
    Speaker {
        id: "ono_anna",
        name: "Ono Anna",
        sound: "playful, light young woman",
        native: "Japanese",
    },
    Speaker {
        id: "sohee",
        name: "Sohee",
        sound: "warm woman, full of feeling",
        native: "Korean",
    },
];

pub fn speaker(id: &str) -> &'static Speaker {
    SPEAKERS.iter().find(|s| s.id == id).unwrap_or(&SPEAKERS[0])
}

/// How the local voice reads. Everything but `parallel` is part of a
/// clip's cache key: changing it makes new audio, old audio stays.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalSettings {
    pub speaker: String,
    /// A kept sample's WAV hash. Base reads it in place of a named speaker.
    pub designed: Option<String>,
    /// Free-text style instruction (any language), e.g. "用温柔平稳的语气朗读".
    pub instruct: String,
    /// Sampling of the main model (the first codebook: what is said and how).
    pub temperature: f32,
    pub top_k: u32,
    pub top_p: f32,
    pub repetition_penalty: f32,
    /// Sampling of the code predictor (the other 15 codebooks: timbre detail).
    pub sub_temperature: f32,
    pub sub_top_k: u32,
    pub sub_top_p: f32,
    /// Mixed into every chunk's seed: another value, another take.
    pub seed: u32,
    /// Characters per request (whole paragraphs up to this).
    pub chunk_chars: usize,
    /// Requests the GPU works on at once (memory: about 0.9 GB each).
    pub parallel: usize,
}

impl Default for LocalSettings {
    fn default() -> Self {
        // The server's own defaults (qwentts.cpp, Qwen's generation config).
        LocalSettings {
            speaker: "vivian".into(),
            designed: None,
            instruct: String::new(),
            temperature: 0.9,
            top_k: 50,
            top_p: 1.0,
            repetition_penalty: 1.05,
            sub_temperature: 0.9,
            sub_top_k: 50,
            sub_top_p: 1.0,
            seed: 0,
            chunk_chars: 200,
            parallel: 4,
        }
    }
}

impl LocalSettings {
    pub fn mode(&self) -> Mode {
        if self.designed.is_some() {
            Mode::Base
        } else {
            Mode::Custom
        }
    }

    /// The cache-key part: what the audio depends on.
    pub fn key_text(&self) -> String {
        let voice = match &self.designed {
            Some(id) => format!("qwen3-tts-1.7b-base-q8_0|{}|cuts-{CUTS}|{id}", opus::FORMAT),
            None => format!(
                "{}|{}|cuts-{CUTS}|{}|{}",
                QWEN_TTS_MODEL.id,
                opus::FORMAT,
                self.speaker,
                self.instruct.trim()
            ),
        };
        format!(
            "{voice}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.temperature,
            self.top_k,
            self.top_p,
            self.repetition_penalty,
            self.sub_temperature,
            self.sub_top_k,
            self.sub_top_p,
            self.seed,
            self.chunk_chars,
        )
    }
}

/// How chunks are cut into sentences, in the cache key: clips cut another
/// way are made again. 1: by pause length (misplaced 13% of clips on
/// 死亡万花筒, 2026-11-19); 2: pauses matched to the punctuation ([`cuts`]).
const CUTS: u32 = 2;

/// Both downloads are present.
pub fn downloaded() -> bool {
    QWEN_TTS_SERVER.is_some() && QWEN_TTS.is_cached() && QWEN_TTS_MODEL.is_cached()
}

/// What a download takes, in bytes.
pub fn download_size() -> u64 {
    QWEN_TTS.total_size() + QWEN_TTS_MODEL.total_size()
}

/// Download both (the server, then the model), reporting bytes done of all.
pub fn download(progress: &dyn Fn(u64, u64)) -> Result<(), String> {
    if QWEN_TTS_SERVER.is_none() {
        return Err("The local voice has no build for this platform".into());
    }
    let total = download_size();
    let first = QWEN_TTS.total_size();
    QWEN_TTS.ensure(&|d, _| progress(d, total))?;
    QWEN_TTS_MODEL.ensure(&|d, _| progress(first + d, total))?;
    Ok(())
}

/// The designer and the clone share the codec and server with CustomVoice.
const DESIGN_DOWNLOADS: [Model; 3] = [QWEN_TTS, QWEN_TTS_CODEC, QWEN_TTS_DESIGN];

pub fn design_downloaded() -> bool {
    QWEN_TTS_SERVER.is_some() && DESIGN_DOWNLOADS.iter().all(Model::is_cached)
}

pub fn design_download_size() -> u64 {
    DESIGN_DOWNLOADS
        .iter()
        .filter(|m| !m.is_cached())
        .map(Model::total_size)
        .sum()
}

pub fn download_design(progress: &dyn Fn(u64, u64)) -> Result<(), String> {
    if QWEN_TTS_SERVER.is_none() {
        return Err("The local voice has no build for this platform".into());
    }
    let missing: Vec<_> = DESIGN_DOWNLOADS
        .into_iter()
        .filter(|m| !m.is_cached())
        .collect();
    let total = missing.iter().map(Model::total_size).sum();
    let mut done = 0;
    for m in missing {
        m.ensure(&|d, _| progress(done + d, total))?;
        done += m.total_size();
        progress(done, total);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Chunks and cuts.

/// Paragraph of each sentence: sentences `i` and `i + 1` share one unless a
/// block end or line break (`'\0'`, `'\n'`) lies between them.
pub fn paragraphs(chars: &[char], sents: &[Range<u32>]) -> Vec<usize> {
    let mut out = Vec::with_capacity(sents.len());
    let mut p = 0;
    for (i, r) in sents.iter().enumerate() {
        if i > 0 {
            let gap = &chars[sents[i - 1].end as usize..r.start as usize];
            if gap.iter().any(|&c| c == '\0' || c == '\n') {
                p += 1;
            }
        }
        out.push(p);
    }
    out
}

/// Requests from sentence `from` on: each takes whole paragraphs (the
/// first, the rest of `from`'s) while they fit its limit in characters; a
/// paragraph alone over the limit is cut between sentences (at least one
/// sentence a chunk). `limits` gives each chunk's limit in turn; at most
/// `max` chunks.
pub fn chunks(
    lens: &[usize],
    para: &[usize],
    from: usize,
    mut limits: impl FnMut(usize) -> usize,
    max: usize,
) -> Vec<Range<usize>> {
    let n = lens.len();
    let mut out = Vec::new();
    let mut i = from;
    while i < n && out.len() < max {
        let limit = limits(out.len()).max(1);
        let start = i;
        let mut len = 0;
        while i < n {
            let mut end = i + 1;
            while end < n && para[end] == para[i] {
                end += 1;
            }
            let plen: usize = lens[i..end].iter().sum();
            if len + plen <= limit {
                len += plen;
                i = end;
                continue;
            }
            if len == 0 {
                while i < end && (len == 0 || len + lens[i] <= limit) {
                    len += lens[i];
                    i += 1;
                }
            }
            break;
        }
        out.push(start..i);
    }
    out
}

/// 10 ms frames.
const HOP: usize = opus::RATE as usize / 100;
/// A frame quieter than this (RMS, full scale 1) is silent: -40 dBFS.
const QUIET: f64 = 0.01;
/// The shortest pause a sentence may end at.
const MIN_PAUSE: usize = 6;

/// A chunk's audio, frame by frame: loudness, the pauses inside the speech
/// (not leading or trailing silence) and the voiced time before each frame.
struct Acoustic {
    energy: Vec<f64>,
    /// Silent runs: first frame and the frame after.
    pauses: Vec<(usize, usize)>,
    /// Voiced frames before frame `f` (pauses not counted).
    voiced: Vec<usize>,
}

impl Acoustic {
    fn new(pcm: &[i16]) -> Acoustic {
        let frames = pcm.len() / HOP;
        let energy: Vec<f64> = (0..frames)
            .map(|f| {
                let s = &pcm[f * HOP..(f + 1) * HOP];
                (s.iter().map(|&x| (x as f64 / 32768.0).powi(2)).sum::<f64>() / HOP as f64).sqrt()
            })
            .collect();
        let mut pauses = Vec::new();
        let mut f = 0;
        while f < frames {
            if energy[f] < QUIET {
                let a = f;
                while f < frames && energy[f] < QUIET {
                    f += 1;
                }
                if a > 0 && f < frames && f - a >= MIN_PAUSE {
                    pauses.push((a, f));
                }
            } else {
                f += 1;
            }
        }
        let mut in_pause = vec![false; frames];
        for &(a, b) in &pauses {
            in_pause[a..b].iter_mut().for_each(|x| *x = true);
        }
        let mut voiced = vec![0usize; frames + 1];
        for f in 0..frames {
            voiced[f + 1] = voiced[f] + usize::from(!in_pause[f]);
        }
        Acoustic {
            energy,
            pauses,
            voiced,
        }
    }

    fn frames(&self) -> usize {
        self.energy.len()
    }

    /// The sample where `v` voiced seconds have been spoken, moved to the
    /// quietest frame within 0.3 s.
    fn at_voiced(&self, v: f64) -> usize {
        let frames = self.frames();
        let target = (v * 100.0).round().max(0.0) as usize;
        let f = self
            .voiced
            .partition_point(|&x| x < target)
            .min(frames.saturating_sub(1));
        let (lo, hi) = (f.saturating_sub(30), (f + 30).min(frames));
        let q = (lo..hi)
            .min_by(|&a, &b| self.energy[a].total_cmp(&self.energy[b]))
            .unwrap_or(f);
        q * HOP + HOP / 2
    }
}

/// What the text says where the voice may pause.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mark {
    /// A comma, colon, dash… inside a sentence.
    Minor,
    /// The end of one of the reader's sentences.
    Sentence,
    /// The end of a paragraph.
    Paragraph,
}

impl Mark {
    /// The cost of reading past it without a pause.
    fn skip(self) -> f64 {
        match self {
            Mark::Minor => 0.3,
            Mark::Sentence => 2.0,
            Mark::Paragraph => 3.0,
        }
    }
}

fn pause_punct(c: char) -> bool {
    matches!(
        c,
        '，' | '、'
            | '；'
            | '：'
            | '。'
            | '！'
            | '？'
            | '…'
            | '—'
            | '～'
            | ','
            | ';'
            | ':'
            | '.'
            | '!'
            | '?'
            | '~'
    )
}

/// The places the voice may pause, with the spoken characters (letters,
/// digits, Chinese characters) before each and, for sentence ends, which
/// boundary it is. And the spoken characters in all.
fn marks(texts: &[String], paras: &[usize]) -> (Vec<(f64, Mark, Option<usize>)>, f64) {
    let mut out = Vec::new();
    let mut spoken = 0usize;
    for (i, t) in texts.iter().enumerate() {
        let chars: Vec<char> = t.chars().collect();
        for (k, &c) in chars.iter().enumerate() {
            if c.is_alphanumeric() {
                spoken += 1;
            } else if pause_punct(c)
                // The end of a run of punctuation with more to say after it.
                && chars[k + 1..].iter().any(|c| c.is_alphanumeric())
                && !chars.get(k + 1).is_some_and(|&n| pause_punct(n))
            {
                out.push((spoken as f64, Mark::Minor, None));
            }
        }
        if i + 1 < texts.len() {
            let kind = if paras[i + 1] != paras[i] {
                Mark::Paragraph
            } else {
                Mark::Sentence
            };
            out.push((spoken as f64, kind, Some(i)));
        }
    }
    (out, spoken as f64)
}

/// Where to cut a chunk's audio (`pcm`) into its sentences (`texts`, in
/// paragraphs `paras`): one sample index per boundary, increasing.
///
/// The pauses in the audio are matched, in order, to the places in the text
/// where the voice may pause: commas and the like, sentence ends, paragraph
/// ends. A pause may be left unmatched (a breath) and so may a place (read
/// on without a pause), each at a cost, and between two matches the voiced
/// time must fit the characters spoken at the chunk's own pace. Of all such
/// matchings the cheapest wins (dynamic programming). A long pause at a comma
/// thus stays at that comma instead of being taken for the sentence's end,
/// which a choice by pause length alone got wrong. A sentence end with its
/// pause is cut in the middle of it; one without falls where the characters
/// put it between the matches around it, at the quietest frame near there.
///
/// Each cut comes with its pause (first and last sample) when it is at one.
fn cut_points(
    pcm: &[i16],
    texts: &[String],
    paras: &[usize],
) -> Vec<(usize, Option<(usize, usize)>)> {
    if texts.len() < 2 {
        return Vec::new();
    }
    let ac = Acoustic::new(pcm);
    let (marks, total_c) = marks(texts, paras);
    let total_v = *ac.voiced.last().unwrap() as f64 / 100.0;
    let pauses = &ac.pauses;
    // Seconds of voice a character.
    let rate = if total_c > 0.0 {
        total_v / total_c
    } else {
        0.25
    };
    let (k_n, r_n) = (marks.len(), pauses.len());
    let v_at = |r: usize| ac.voiced[pauses[r].0] as f64 / 100.0;
    let len_at = |r: usize| (pauses[r].1 - pauses[r].0) as f64 / 100.0;
    // Leaving a pause unexplained: the longer, the less likely.
    let skip_pause = |r: usize| 1.0 + 3.0 * len_at(r).min(1.0);
    // Prefix sums of skip costs: marks before k, pauses before r.
    let mut mark_skip = vec![0.0; k_n + 1];
    for k in 0..k_n {
        mark_skip[k + 1] = mark_skip[k] + marks[k].1.skip();
    }
    let mut pause_skip = vec![0.0; r_n + 1];
    for r in 0..r_n {
        pause_skip[r + 1] = pause_skip[r] + skip_pause(r);
    }
    // The fit of `dv` voiced seconds to `dc` characters.
    let fit = |dc: f64, dv: f64| {
        let want = dc * rate;
        let e = (dv - want) / (0.2 * want + 0.4);
        -e * e
    };
    // Nodes: (mark k, pause r) matched. best[k][r] and where it came from
    // (None: the chunk's start).
    let mut best = vec![vec![f64::NEG_INFINITY; r_n]; k_n];
    let mut from: Vec<Vec<Option<(usize, usize)>>> = vec![vec![None; r_n]; k_n];
    for k in 0..k_n {
        for r in 0..r_n {
            let (c, v) = (marks[k].0, v_at(r));
            // From the start.
            let mut b = fit(c, v) - mark_skip[k] - pause_skip[r];
            let mut f = None;
            for k2 in 0..k {
                for r2 in 0..r {
                    let prev = best[k2][r2];
                    if prev == f64::NEG_INFINITY {
                        continue;
                    }
                    let s = prev + fit(c - marks[k2].0, v - v_at(r2))
                        - (mark_skip[k] - mark_skip[k2 + 1])
                        - (pause_skip[r] - pause_skip[r2 + 1]);
                    if s > b {
                        b = s;
                        f = Some((k2, r2));
                    }
                }
            }
            best[k][r] = b;
            from[k][r] = f;
        }
    }
    // To the end: the best last match (or none at all).
    let mut end = fit(total_c, total_v) - mark_skip[k_n] - pause_skip[r_n];
    let mut last = None;
    for k in 0..k_n {
        for r in 0..r_n {
            let s = best[k][r] + fit(total_c - marks[k].0, total_v - v_at(r))
                - (mark_skip[k_n] - mark_skip[k + 1])
                - (pause_skip[r_n] - pause_skip[r + 1]);
            if s > end {
                end = s;
                last = Some((k, r));
            }
        }
    }
    // The matches, in order.
    let mut matched: Vec<(usize, usize)> = Vec::new();
    let mut at = last;
    while let Some((k, r)) = at {
        matched.push((k, r));
        at = from[k][r];
    }
    matched.reverse();
    // Anchors (characters, voiced seconds) for sentence ends without a pause.
    let mut anchors = vec![(0.0, 0.0)];
    anchors.extend(matched.iter().map(|&(k, r)| (marks[k].0, v_at(r))));
    anchors.push((total_c, total_v));
    let mut out = Vec::with_capacity(texts.len() - 1);
    for (k, &(c, _, boundary)) in marks.iter().enumerate() {
        if boundary.is_none() {
            continue;
        }
        let (cut, pause) = match matched.iter().find(|m| m.0 == k) {
            Some(&(_, r)) => (
                (pauses[r].0 + pauses[r].1) / 2 * HOP,
                Some((pauses[r].0 * HOP, pauses[r].1 * HOP)),
            ),
            None => {
                let i = anchors
                    .partition_point(|a| a.0 <= c)
                    .clamp(1, anchors.len() - 1);
                let ((c0, v0), (c1, v1)) = (anchors[i - 1], anchors[i]);
                let v = if c1 > c0 {
                    v0 + (c - c0) / (c1 - c0) * (v1 - v0)
                } else {
                    v0
                };
                (ac.at_voiced(v), None)
            }
        };
        let floor = out.last().map_or(0, |c: &(usize, _)| c.0);
        out.push((cut.clamp(floor, pcm.len()), pause));
    }
    out
}

/// Where to cut a chunk's audio into its sentences ([`cut_points`]).
#[cfg(test)]
pub fn cuts(pcm: &[i16], texts: &[String], paras: &[usize]) -> Vec<usize> {
    cut_points(pcm, texts, paras)
        .into_iter()
        .map(|c| c.0)
        .collect()
}

/// Longest a lead-in may be, in characters.
const LEAD_MAX: usize = 60;
/// Most of the pause after a lead-in kept at the start of the chunk, in
/// samples: 0.8 s. After a paragraph the model paused up to 1.5 s; inside
/// chunks a sentence or paragraph pause is 0.5–1.0 s (middle half).
const LEAD_PAUSE: usize = opus::RATE as usize * 8 / 10;

/// What to read before a chunk so that it goes on from the text before it
/// instead of starting afresh: the sentence before (`prev`), or only its
/// last clauses when it is long (cut after a comma or the like).
pub fn lead_in(prev: &str) -> String {
    let chars: Vec<char> = prev.chars().collect();
    if chars.len() <= LEAD_MAX {
        return prev.to_owned();
    }
    // The longest tail within the limit that starts after a clause.
    (chars.len() - LEAD_MAX..chars.len())
        .find(|&i| i > 0 && matches!(chars[i - 1], '，' | '、' | '；' | '：' | ',' | ';' | ':'))
        .map(|i| chars[i..].iter().collect::<String>())
        .filter(|t| t.chars().any(char::is_alphanumeric))
        .unwrap_or_else(|| chars[chars.len() - LEAD_MAX..].iter().collect())
}

/// The first build's cuts (2026.11.19.03, never released): every boundary at a pause, chosen by
/// length and closeness to the boundary's share of the characters. Kept for
/// the evaluation that replaced it.
#[cfg(test)]
pub fn cuts_by_pauses(pcm: &[i16], lens: &[usize]) -> Vec<usize> {
    let m = lens.len().saturating_sub(1);
    if m == 0 {
        return Vec::new();
    }
    let ac = Acoustic::new(pcm);
    let (pauses, voiced) = (&ac.pauses, &ac.voiced);
    let frames = ac.frames();
    let total_voiced = voiced[frames].max(1) as f64;
    let total_chars: usize = lens.iter().sum::<usize>().max(1);
    let mut acc = 0;
    let expected: Vec<f64> = lens[..m]
        .iter()
        .map(|l| {
            acc += l;
            acc as f64 / total_chars as f64 * total_voiced / 100.0
        })
        .collect();
    let r = pauses.len();
    if r < m {
        return expected.iter().map(|&e| ac.at_voiced(e)).collect();
    }
    let score = |j: usize, k: usize| {
        let (a, b) = pauses[k];
        let len = ((b - a) as f64 / 100.0).min(1.0);
        let at = voiced[a] as f64 / 100.0;
        1.5 * len - (at - expected[j]).abs()
    };
    let mut best = vec![vec![f64::NEG_INFINITY; r]; m];
    let mut from = vec![vec![usize::MAX; r]; m];
    for (k, b) in best[0].iter_mut().enumerate() {
        *b = score(0, k);
    }
    for j in 1..m {
        let mut run_best = f64::NEG_INFINITY;
        let mut run_k = usize::MAX;
        for k in j..r {
            if best[j - 1][k - 1] > run_best {
                run_best = best[j - 1][k - 1];
                run_k = k - 1;
            }
            if run_best > f64::NEG_INFINITY {
                best[j][k] = run_best + score(j, k);
                from[j][k] = run_k;
            }
        }
    }
    let mut k = (0..r)
        .max_by(|&a, &b| best[m - 1][a].total_cmp(&best[m - 1][b]))
        .unwrap();
    let mut chosen = vec![0; m];
    for j in (0..m).rev() {
        chosen[j] = k;
        if j > 0 {
            k = from[j][k];
        }
    }
    chosen
        .into_iter()
        .map(|k| {
            let (a, b) = pauses[k];
            (a + b) / 2 * HOP
        })
        .collect()
}

/// Seconds of speech a text of `chars` characters about takes (measured on
/// a novel: 3.8 characters a second).
pub fn seconds(chars: usize) -> f64 {
    chars as f64 / 3.8
}

// ---------------------------------------------------------------------------
// The server.

/// Each server process can load just one talker. A kept sample is read by
/// Base, since VoiceDesign can change the speaker with every request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Custom,
    Design,
    Base,
}

struct Reference {
    id: String,
    text: String,
    wav: Vec<u8>,
    frames: usize,
}

#[derive(Clone, Copy)]
enum Voice<'a> {
    Custom,
    Design(&'a str),
    Base(&'a Reference),
}

/// qwentts.cpp's server with the model loaded, on a free local port.
pub struct Server {
    child: Mutex<Child>,
    url: String,
    agent: ureq::Agent,
    mode: Mode,
    parallel: usize,
    /// Check and register together so batch workers don't all upload the WAV.
    registry: Mutex<()>,
    /// Requests in flight, at most `parallel` (more would wait in the
    /// server's queue behind what was asked first).
    busy: Mutex<usize>,
    freed: Condvar,
    log: PathBuf,
}

/// Workers share a read guard while synthesising. Switching models takes a
/// write guard, so a running request gets to finish before the server stops.
static SERVER: RwLock<Option<Server>> = RwLock::new(None);

/// What the server is doing, for the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Unloaded,
    Loading,
    Loaded,
}

pub fn status() -> Status {
    let Ok(g) = SERVER.try_read() else {
        return Status::Loading;
    };
    match g.as_ref() {
        Some(s) if s.alive() => Status::Loaded,
        _ => Status::Unloaded,
    }
}

/// A different talker on the GPU still needs swapping out before this one
/// can speak.
pub fn loaded(mode: Mode, parallel: usize) -> bool {
    SERVER.try_read().is_ok_and(|g| {
        g.as_ref()
            .is_some_and(|s| s.mode == mode && s.parallel == parallel.max(1) && s.alive())
    })
}

/// Stop the server, freeing the GPU memory.
pub fn unload() {
    if let Ok(mut g) = SERVER.try_write()
        && let Some(s) = g.take()
    {
        s.kill();
    }
}

/// Use the server in the requested mode, starting it if necessary. Keep it
/// alive through the request and any retries, even if another window needs
/// a different model or batch size.
pub fn server<T>(mode: Mode, parallel: usize, f: impl FnOnce(&Server) -> T) -> Result<T, String> {
    let parallel = if mode == Mode::Design {
        1
    } else {
        parallel.max(1)
    };
    let matches = |s: &&Server| s.mode == mode && s.parallel == parallel && s.alive();
    loop {
        let g = SERVER.read().unwrap();
        if let Some(s) = g.as_ref().filter(matches) {
            return Ok(f(s));
        }
        drop(g);
        let mut g = SERVER.write().unwrap();
        if g.as_ref().filter(matches).is_none() {
            if let Some(s) = g.take() {
                s.kill();
            }
            *g = Some(Server::start(mode, parallel)?);
        }
    }
}

impl Server {
    fn start(mode: Mode, parallel: usize) -> Result<Server, String> {
        let talker = match mode {
            Mode::Custom if downloaded() => QWEN_TTS_MODEL.dir().join(QWEN_TTS_MODEL.files[0].name),
            Mode::Custom => return Err(NOT_DOWNLOADED.into()),
            Mode::Design if design_downloaded() => {
                QWEN_TTS_DESIGN.dir().join(QWEN_TTS_DESIGN.files[0].name)
            }
            Mode::Base if design_downloaded() => {
                QWEN_TTS_DESIGN.dir().join(QWEN_TTS_DESIGN.files[1].name)
            }
            _ => return Err(DESIGN_NOT_DOWNLOADED.into()),
        };
        let exe = QWEN_TTS.dir().join(QWEN_TTS_SERVER.unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| format!("Cannot make {} executable: {e}", exe.display()))?;
        }
        let port = std::net::TcpListener::bind(("127.0.0.1", 0))
            .and_then(|l| l.local_addr())
            .map_err(|e| format!("No free local port: {e}"))?
            .port();
        let dir = hanstat::models::app_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
        let log = dir.join("tts-server.log");
        let out = std::fs::File::create(&log)
            .map_err(|e| format!("Cannot create {}: {e}", log.display()))?;
        let err = out.try_clone().map_err(|e| e.to_string())?;
        let mut cmd = Command::new(&exe);
        cmd.current_dir(QWEN_TTS.dir())
            .arg("--model")
            .arg(talker)
            .arg("--codec")
            .arg(QWEN_TTS_CODEC.dir().join(QWEN_TTS_CODEC.files[0].name))
            .args(["--host", "127.0.0.1", "--port", &port.to_string()])
            .args(["--lang", "chinese", "--max-batch", &parallel.to_string()])
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut guard_error = None;
        let mut child = hanstat::child_guard::spawn(&mut cmd, |error| guard_error = Some(error))
            .map_err(|e| format!("Cannot start {}: {e}", exe.display()))?;
        if let Some(error) = guard_error {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        let s = Server {
            child: Mutex::new(child),
            url: format!("http://127.0.0.1:{port}"),
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(900)))
                .http_status_as_error(false)
                .build()
                .into(),
            mode,
            parallel,
            registry: Mutex::new(()),
            busy: Mutex::new(0),
            freed: Condvar::new(),
            log,
        };
        let t = Instant::now();
        loop {
            if let Ok(Some(st)) = s.child.lock().unwrap().try_wait() {
                return Err(format!(
                    "The local voice stopped while loading ({st}):\n{}",
                    s.log_tail()
                ));
            }
            if s.agent
                .get(format!("{}/health", s.url))
                .call()
                .is_ok_and(|r| r.status().is_success())
            {
                return Ok(s);
            }
            if t.elapsed() > Duration::from_secs(300) {
                s.kill();
                return Err(format!(
                    "The local voice did not load in time:\n{}",
                    s.log_tail()
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn alive(&self) -> bool {
        matches!(self.child.lock().unwrap().try_wait(), Ok(None))
    }

    fn kill(&self) {
        let mut c = self.child.lock().unwrap();
        let _ = c.kill();
        let _ = c.wait();
    }

    fn log_tail(&self) -> String {
        let text = std::fs::read(&self.log)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        lines[lines.len().saturating_sub(6)..].join("\n")
    }

    #[cfg(test)]
    pub fn synth(
        &self,
        text: &str,
        s: &LocalSettings,
        seed: u64,
    ) -> Result<(Vec<i16>, bool), String> {
        self.synth_voice(text, s, seed, Voice::Custom)
    }

    /// `text` read with `s`: samples and whether the take ran out of
    /// frames ([`frame_budget`]) before reaching the end of the text.
    fn synth_voice(
        &self,
        text: &str,
        s: &LocalSettings,
        seed: u64,
        voice: Voice<'_>,
    ) -> Result<(Vec<i16>, bool), String> {
        let budget = frame_budget(text, s, voice)?;
        {
            let mut b = self.busy.lock().unwrap();
            while *b >= self.parallel {
                b = self.freed.wait(b).unwrap();
            }
            *b += 1;
        }
        let r = self.speech(text, s, seed, budget, voice).and_then(|wav| {
            wav_samples(&wav).ok_or_else(|| "The local voice sent no audio".into())
        });
        *self.busy.lock().unwrap() -= 1;
        self.freed.notify_one();
        r.map(|pcm| {
            let stopped = at_budget(pcm.len(), budget);
            (pcm, stopped)
        })
    }

    fn speech(
        &self,
        text: &str,
        s: &LocalSettings,
        seed: u64,
        budget: usize,
        voice: Voice<'_>,
    ) -> Result<Vec<u8>, String> {
        if let Voice::Base(reference) = voice {
            self.register(reference)?;
        }
        self.post(
            "/v1/audio/speech",
            request_body(text, s, seed, budget, voice),
        )
    }

    /// The registry is in the Base process's memory. It starts empty and
    /// can lose a voice, so check the list before using the reference.
    fn register(&self, reference: &Reference) -> Result<(), String> {
        let _guard = self.registry.lock().unwrap();
        let mut resp = self
            .agent
            .get(format!("{}/v1/audio/voices", self.url))
            .call()
            .map_err(|e| format!("Cannot check the local voice list: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("Cannot check the local voice list ({status})"));
        }
        #[derive(Deserialize)]
        struct Entry {
            name: String,
            kind: String,
        }
        #[derive(Deserialize)]
        struct Registry {
            voices: Vec<Entry>,
        }
        let bytes = resp.body_mut().read_to_vec().map_err(|e| e.to_string())?;
        let list: Registry = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Cannot read the local voice list: {e}"))?;
        if !list
            .voices
            .iter()
            .any(|v| v.name == reference.id && v.kind == "registered")
        {
            self.post(
                "/v1/audio/voices",
                serde_json::json!({
                    "name": reference.id,
                    "ref_text": reference.text,
                    "wav_b64": base64(&reference.wav),
                }),
            )?;
        }
        Ok(())
    }

    fn post(&self, path: &str, body: serde_json::Value) -> Result<Vec<u8>, String> {
        let body = serde_json::to_vec(&body).unwrap();
        let resp = self
            .agent
            .post(format!("{}{path}", self.url))
            .header("Content-Type", "application/json")
            .send(&body[..]);
        let mut resp = match resp {
            Ok(r) => r,
            Err(e) => {
                return Err(if self.alive() {
                    format!("The local voice didn't answer: {e}")
                } else {
                    format!("The local voice stopped:\n{}", self.log_tail())
                });
            }
        };
        let status = resp.status();
        let bytes = resp
            .body_mut()
            .with_config()
            .limit(1 << 30)
            .read_to_vec()
            .map_err(|e| format!("The local voice's answer was cut off: {e}"))?;
        if !status.is_success() {
            return Err(format!(
                "The local voice refused ({status}): {}",
                String::from_utf8_lossy(&bytes).trim()
            ));
        }
        Ok(bytes)
    }
}

/// The three talkers accept different voice and instruction fields.
fn request_body(
    text: &str,
    s: &LocalSettings,
    seed: u64,
    budget: usize,
    voice: Voice<'_>,
) -> serde_json::Value {
    let mut body = serde_json::json!({
            "max_new_tokens": budget,
            "input": text,
            "language": "chinese",
            "response_format": "wav",
            // The server takes a signed 64-bit seed.
            "seed": (seed >> 1) as i64,
            "temperature": s.temperature,
            "top_k": s.top_k,
            "top_p": s.top_p.clamp(0.01, 1.0),
            "repetition_penalty": s.repetition_penalty.max(0.01),
            "subtalker_temperature": s.sub_temperature,
            "subtalker_top_k": s.sub_top_k,
            "subtalker_top_p": s.sub_top_p.clamp(0.01, 1.0),
    });
    match voice {
        Voice::Custom => {
            body["voice"] = s.speaker.as_str().into();
            if !s.instruct.trim().is_empty() {
                body["instructions"] = s.instruct.trim().into();
            }
        }
        Voice::Base(r) => body["voice"] = r.id.as_str().into(),
        Voice::Design(description) => body["instructions"] = description.into(),
    }
    body
}

/// The server takes standard padded base64 for the reference WAV.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let digit = |n| char::from(ALPHABET[n as usize]);
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        out.push(digit(a >> 2));
        out.push(digit((a & 3) << 4 | b >> 4));
        out.push(if chunk.len() >= 2 {
            digit((b & 15) << 2 | c >> 6)
        } else {
            '='
        });
        out.push(if chunk.len() == 3 { digit(c & 63) } else { '=' });
    }
    out
}

/// Samples an audio frame makes (the 12.5 Hz codec at 24 kHz).
const FRAME_SAMPLES: usize = 1920;
/// Positions a request may fill in the server's context: the prompt and
/// every frame made (qwentts.cpp 2edb35f sizes its KV cache to 4096 and
/// does not check for running past it).
const CONTEXT: usize = 4096;

/// Most frames to let the model make for `text` read with `s`: three times
/// its expected length plus 4 s (longer and it has run on; the same rule
/// as [`make_chunk`]'s retry), within what the context holds after the
/// prompt, counted generously (a position for every character of text and
/// instruction, the template and, for Base, the reference transcript and
/// codec frames). Without this the server stops at 2048
/// frames (163.8 s), which a slow voice could reach before the end of a
/// long chunk, dropping its last sentences without a word.
fn frame_budget(text: &str, s: &LocalSettings, voice: Voice<'_>) -> Result<usize, String> {
    let chars = text.chars().count();
    let frames = ((seconds(chars) * 3.0 + 4.0) * 12.5).ceil() as usize;
    let extra = match voice {
        Voice::Custom => s.instruct.chars().count(),
        Voice::Design(description) => description.chars().count(),
        Voice::Base(r) => r.text.chars().count() + r.frames,
    };
    let room = CONTEXT.saturating_sub(chars + extra + 128);
    if room < 25 {
        return Err("The text and voice settings are too long. Try shorter text or a shorter description or sample.".into());
    }
    Ok(frames.min(room))
}

/// A take of `samples` stopped at its `budget` of frames (within two, for
/// what the codec trims), not at the end of the text.
fn at_budget(samples: usize, budget: usize) -> bool {
    samples + 2 * FRAME_SAMPLES >= budget * FRAME_SAMPLES
}

/// The 16-bit mono samples of a RIFF/WAVE file at [`opus::RATE`].
fn wav_samples(b: &[u8]) -> Option<Vec<i16>> {
    if b.len() < 12 || &b[..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return None;
    }
    let mut i = 12;
    let mut fmt_ok = false;
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let n = u32::from_le_bytes(b[i + 4..i + 8].try_into().ok()?) as usize;
        let body = &b[i + 8..(i + 8 + n).min(b.len())];
        if id == b"fmt " && body.len() >= 16 {
            let format = u16::from_le_bytes([body[0], body[1]]);
            let channels = u16::from_le_bytes([body[2], body[3]]);
            let rate = u32::from_le_bytes(body[4..8].try_into().ok()?);
            let bits = u16::from_le_bytes([body[14], body[15]]);
            fmt_ok = format == 1 && channels == 1 && rate == opus::RATE && bits == 16;
        } else if id == b"data" {
            if !fmt_ok {
                return None;
            }
            return Some(
                body.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| i16::from_le_bytes(*c))
                    .collect(),
            );
        }
        i += 8 + n + (n & 1);
    }
    None
}

pub const NOT_DOWNLOADED: &str =
    "Download the local voice first (Read aloud → On this computer → Download).";
pub const DESIGN_NOT_DOWNLOADED: &str = "Download the models in Design a voice first.";

/// Text read before a chunk and thrown away ([`lead_in`]): the end of the
/// sentence before it, and whether that is in the chunk's first paragraph.
#[derive(Clone, Debug, PartialEq)]
pub struct Lead {
    pub text: String,
    pub same_para: bool,
}

/// What is sent for a chunk: the lead-in (if any) then the chunk's
/// sentences, paragraphs on lines of their own, sentences run together;
/// and the sentences and paragraphs as read (the lead-in first).
fn assemble(
    texts: &[String],
    paras: &[usize],
    lead: Option<&Lead>,
) -> (String, Vec<String>, Vec<usize>) {
    let mut all = Vec::with_capacity(texts.len() + 1);
    let mut all_paras = Vec::with_capacity(texts.len() + 1);
    if let Some(l) = lead {
        all.push(l.text.clone());
        all_paras.push(if l.same_para {
            paras[0]
        } else {
            paras[0].wrapping_sub(1)
        });
    }
    all.extend(texts.iter().cloned());
    all_paras.extend_from_slice(paras);
    let mut text = String::new();
    for (i, t) in all.iter().enumerate() {
        if i > 0 && all_paras[i] != all_paras[i - 1] {
            text.push('\n');
        }
        text.push_str(t);
    }
    (text, all, all_paras)
}

/// One chunk read and cut into its sentences' clips. With a lead-in, the
/// model reads the end of the sentence before first, so the chunk's first
/// sentence sounds like what comes next rather than a new start; that audio
/// is cut away in the pause after it, so the first clip opens with the
/// model's own pause, up to [`LEAD_PAUSE`] (where chunks met, the pause was
/// otherwise 0.09–0.18 s against a median 0.7 s inside a chunk). On
/// 死亡万花筒 a speech recogniser heard no trace of the lead-in in any of
/// seven chunks' first clips. A take far longer or shorter than
/// the text should be (the model ran on or skipped text) is tried again
/// once with another seed.
pub fn make_chunk(
    texts: &[String],
    paras: &[usize],
    lead: Option<&Lead>,
    s: &LocalSettings,
) -> Result<Vec<Vec<i16>>, Fail> {
    // A bare chapter heading can make Base run on instead of continuing
    // the narration. Carry over a preceding paragraph only when it has
    // sentence punctuation. CustomVoice keeps its existing lead-ins.
    let lead = lead.filter(|l| {
        s.designed.is_none()
            || l.same_para
            || l.text.contains(['。', '！', '？', '.', '!', '?', '…'])
    });
    let reference = s
        .designed
        .as_deref()
        .map(|id| VoiceStore::default().reference(id))
        .transpose()
        .map_err(Fail::Refused)?;
    let voice = reference.as_ref().map_or(Voice::Custom, Voice::Base);
    let (text, all, all_paras) = assemble(texts, paras, lead);
    let chars: usize = all.iter().map(|t| t.chars().count()).sum();
    // The seed follows the chunk's own text, lead-in or not.
    let (own, _, _) = assemble(texts, paras, None);
    let base = hanstat::dict::xxh64::xxh64(own.as_bytes(), s.seed as u64);
    // If the server won't start, there is no use trying the next chunk.
    let (pcm, stopped) = server(s.mode(), s.parallel, |server| {
        let mut take = (Vec::new(), false);
        for attempt in 0..2u64 {
            take = server.synth_voice(&text, s, base.wrapping_add(attempt), voice)?;
            let secs = take.0.len() as f64 / opus::RATE as f64;
            let want = seconds(chars);
            if !take.1 && secs < want * 3.0 + 4.0 && secs > want * 0.25 {
                break;
            }
        }
        Ok(take)
    })
    .map_err(Fail::Refused)?
    .map_err(Fail::Other)?;
    if stopped {
        // Twice past its budget: cut into clips, the text it never reached
        // would put every highlight in the wrong place.
        return Err(Fail::Other(format!(
            "The local voice ran past {} characters of text twice without finishing; \
             a shorter chunk length (Advanced) may help",
            chars
        )));
    }
    let points = cut_points(&pcm, &all, &all_paras);
    let mut cut: Vec<usize> = points.iter().map(|c| c.0).collect();
    if lead.is_some() && !points.is_empty() {
        // The lead-in goes; the pause after it stays, up to LEAD_PAUSE.
        cut[0] = points[0]
            .1
            .map_or(points[0].0, |(a, b)| a.max(b.saturating_sub(LEAD_PAUSE)));
    }
    let mut out = Vec::with_capacity(all.len());
    let mut a = 0;
    for &c in cut.iter().chain(std::iter::once(&pcm.len())) {
        let c = c.clamp(a, pcm.len());
        out.push(pcm[a..c].to_vec());
        a = c;
    }
    if lead.is_some() {
        out.remove(0);
    }
    Ok(out)
}

/// A short passage read with `s`, for hearing the settings before making a
/// chapter: its samples at [`opus::RATE`]. Not saved.
pub fn preview(text: &str, s: &LocalSettings) -> Result<Vec<i16>, String> {
    let reference = s
        .designed
        .as_deref()
        .map(|id| VoiceStore::default().reference(id))
        .transpose()?;
    let voice = reference.as_ref().map_or(Voice::Custom, Voice::Base);
    let seed = hanstat::dict::xxh64::xxh64(text.as_bytes(), s.seed as u64);
    // A preview cut short is still worth hearing.
    server(s.mode(), s.parallel, |server| {
        server.synth_voice(text, s, seed, voice).map(|t| t.0)
    })?
}

/// The original WAV is kept for cloning and hashing. Play the decoded
/// samples so the user hears exactly what will be kept.
pub struct Sample {
    pub wav: Vec<u8>,
    pub pcm: Vec<i16>,
}

/// Make a take to hear before keeping it. A cut-off take can't be used for
/// cloning because its transcript would include words it never said.
pub fn design_sample(description: &str, text: &str, seed: u64) -> Result<Sample, String> {
    if description.trim().is_empty() || text.trim().is_empty() {
        return Err("Enter a description and sample text first.".into());
    }
    let settings = LocalSettings::default();
    let voice = Voice::Design(description);
    let budget = frame_budget(text, &settings, voice)?;
    let wav = server(Mode::Design, 1, |server| {
        server.speech(text, &settings, seed, budget, voice)
    })??;
    let pcm = wav_samples(&wav)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "The voice designer sent no audio".to_owned())?;
    if at_budget(pcm.len(), budget) {
        return Err(
            "The voice designer didn't finish the sample. Try shorter text or make another take."
                .into(),
        );
    }
    Ok(Sample { wav, pcm })
}

/// Run `f` on a thread of its own and tell `repaint` when it is done.
pub fn spawn<T: Send + 'static>(
    name: &str,
    repaint: Repaint,
    f: impl FnOnce() -> T + Send + 'static,
) -> std::sync::mpsc::Receiver<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let _ = tx.send(f());
            repaint();
        });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paragraphs_follow_blocks_and_lines() {
        let s = "一。二。\0三。\n四。";
        let c: Vec<char> = s.chars().collect();
        let sents = super::super::sentences(&c);
        assert_eq!(paragraphs(&c, &sents), [0, 0, 1, 2]);
    }

    #[test]
    fn chunks_take_whole_paragraphs() {
        // Paragraphs: [0,1] (10+10), [2] (30), [3,4,5] (40 each), [6] (5).
        let lens = [10, 10, 30, 40, 40, 40, 5];
        let para = [0, 0, 1, 2, 2, 2, 3];
        assert_eq!(
            chunks(&lens, &para, 0, |_| 60, 10),
            [0..3, 3..4, 4..5, 5..7]
        );
        // From inside a paragraph: its rest first.
        assert_eq!(chunks(&lens, &para, 1, |_| 60, 10)[0], 1..3);
        // Growing limits, and the cap on chunks.
        assert_eq!(
            chunks(&lens, &para, 0, |k| [20, 200][k.min(1)], 2),
            [0..2, 2..7]
        );
        // A sentence over the limit still makes a chunk.
        assert_eq!(chunks(&[500, 3], &[0, 1], 0, |_| 100, 10), [0..1, 1..2]);
        assert!(chunks(&lens, &para, 7, |_| 60, 10).is_empty());
    }

    /// Speech-like noise bursts with silences between them.
    fn speech(parts: &[(f64, f64)]) -> Vec<i16> {
        let mut out = Vec::new();
        let mut x = 12345u32;
        for &(talk, pause) in parts {
            for _ in 0..(talk * 24_000.0) as usize {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
                out.push(((x >> 16) as i16) / 4);
            }
            out.extend(std::iter::repeat_n(0i16, (pause * 24_000.0) as usize));
        }
        out
    }

    /// `n` Chinese characters then `end`.
    fn said(n: usize, end: &str) -> String {
        "字".repeat(n) + end
    }

    fn at(s: f64) -> usize {
        (s * 24_000.0) as usize
    }

    #[test]
    fn a_long_pause_at_a_comma_is_not_the_sentence_end() {
        // What went wrong in 死亡万花筒: the voice paused 0.9 s at a comma
        // inside a sentence, longer than at the paragraph end after it.
        // 0.2 s a character: 4 s, then 2.2 s + comma pause + 2.8 s, then 4 s.
        let texts = [
            said(20, "。"),
            said(11, "，") + &said(14, "。"),
            said(20, "。"),
        ];
        let pcm = speech(&[(4.0, 0.6), (2.2, 0.9), (2.8, 0.5), (4.0, 0.0)]);
        let c = cuts(&pcm, &texts, &[0, 1, 2]);
        assert_eq!(c.len(), 2);
        assert!(c[0] > at(4.0) && c[0] < at(4.6), "{c:?}");
        // In the 0.5 s pause after 10.5 s, not the 0.9 s one after 6.8 s.
        assert!(c[1] > at(10.5) && c[1] < at(11.0), "{c:?}");
    }

    #[test]
    fn short_lines_each_get_their_own_pause() {
        // "……" lines between sentences: several short sentences in a row.
        let texts = [said(12, "。"), "“……”".into(), said(2, "？"), said(12, "。")];
        let pcm = speech(&[(2.4, 1.0), (0.3, 1.0), (0.4, 1.0), (2.4, 0.0)]);
        let c = cuts(&pcm, &texts, &[0, 1, 2, 3]);
        assert_eq!(c.len(), 3);
        assert!(c[0] < c[1] && c[1] < c[2], "distinct and in order: {c:?}");
        assert!(c[0] > at(2.4) && c[0] < at(3.4), "{c:?}");
        assert!(c[1] > at(3.7) && c[1] < at(4.7), "{c:?}");
        assert!(c[2] > at(5.1) && c[2] < at(6.1), "{c:?}");
    }

    #[test]
    fn sentences_of_one_paragraph_and_commas() {
        // One paragraph: a sentence end inside it pauses less than a
        // paragraph would, but commas around it pause too.
        let texts = [
            said(8, "，") + &said(8, "。"),
            said(10, "，") + &said(6, "！"),
            said(16, "。"),
        ];
        let pcm = speech(&[(1.6, 0.25), (1.6, 0.45), (2.0, 0.3), (1.2, 0.5), (3.2, 0.0)]);
        let c = cuts(&pcm, &texts, &[0, 0, 0]);
        assert!(c[0] > at(3.45) && c[0] < at(3.9), "{c:?}");
        // The 0.5 s pause from 7.4 s, not the comma's at 5.9 s.
        assert!(c[1] > at(7.4) && c[1] < at(7.9), "{c:?}");
    }

    #[test]
    fn without_pauses_cuts_fall_by_characters() {
        let pcm = speech(&[(6.0, 0.0)]);
        let c = cuts(&pcm, &[said(10, "。"), said(10, "。")], &[0, 0]);
        assert_eq!(c.len(), 1);
        assert!(c[0].abs_diff(at(3.0)) < at(0.4), "{c:?}");
        assert!(cuts(&pcm, &[said(5, "。")], &[0]).is_empty());
    }

    #[test]
    fn frame_budgets() {
        let s = LocalSettings::default();
        // A short text: three times its length plus 4 s.
        let short = frame_budget("你好。", &s, Voice::Custom).unwrap();
        assert!((70..=90).contains(&short), "{short}");
        // The longest chunk the settings allow (400) and a lead-in (60):
        // room for a voice far slower than measured (2.5 characters a
        // second, against 3.8), and never past the context.
        let longest = said(459, "。");
        let b = frame_budget(&longest, &s, Voice::Custom).unwrap();
        assert!(b as f64 >= 460.0 / 2.5 * 12.5, "{b}");
        // Past the server's default of 2048 frames, which would cut it off.
        assert!(b > 2048, "{b}");
        // Whatever the length and instruction, prompt and frames fit.
        let mut s2 = s.clone();
        s2.instruct = "史诗感十足的低沉有声书旁白声线".into();
        for n in [1, 50, 200, 460, 1000, 2000, 3500] {
            let t = said(n, "");
            let b = frame_budget(&t, &s2, Voice::Custom).unwrap();
            assert!(
                n + s2.instruct.chars().count() + 128 + b <= CONTEXT,
                "{n}: {b}"
            );
        }
    }

    fn reference() -> Reference {
        Reference {
            id: "sample-hash".into(),
            text: said(59, "。"),
            wav: Vec::new(),
            frames: 188, // About 15 seconds.
        }
    }

    #[test]
    fn icl_reference_takes_context_space() {
        let s = LocalSettings {
            instruct: said(4096, ""),
            ..Default::default()
        };
        let text = said(859, "。");
        let reference = reference();
        let budget = frame_budget(&text, &s, Voice::Base(&reference)).unwrap();
        assert_eq!(860 + 60 + 188 + 128 + budget, CONTEXT);
        assert!(frame_budget(&said(3800, ""), &s, Voice::Base(&reference)).is_err());
    }

    #[test]
    fn request_fields_follow_the_model() {
        let s = LocalSettings {
            instruct: " 慢一点 ".into(),
            ..Default::default()
        };
        let body = |voice| request_body("你好。", &s, 8, 20, voice);
        let custom = body(Voice::Custom);
        assert_eq!(custom["voice"], "vivian");
        assert_eq!(custom["instructions"], "慢一点");
        assert_eq!(custom["seed"], 4);
        assert_eq!(custom["max_new_tokens"], 20);
        let reference = reference();
        let base = body(Voice::Base(&reference));
        assert_eq!(base["voice"], reference.id);
        assert!(base.get("instructions").is_none());
        let design = body(Voice::Design("低沉温和"));
        assert_eq!(design["instructions"], "低沉温和");
        assert!(design.get("voice").is_none());
    }

    #[test]
    fn base64_uses_standard_padding() {
        for (bytes, encoded) in [
            (b"".as_slice(), ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"\xfb\xff\xff", "+///"),
        ] {
            assert_eq!(base64(bytes), encoded);
        }
    }

    #[test]
    fn designed_keys_use_the_sample_hash_and_leave_custom_keys_stable() {
        let mut s: LocalSettings =
            serde_json::from_str(r#"{"speaker":"vivian","instruct":"  慢一点  "}"#).unwrap();
        assert_eq!(s.designed, None);
        let custom = s.key_text();
        assert_eq!(
            custom,
            "qwen3-tts-1.7b-customvoice-q8_0|ogg-opus-24khz-mono-32kbps-vbr|cuts-2|vivian|慢一点|0.9|50|1|1.05|0.9|50|1|0|200"
        );
        s.designed = Some("f13e".into());
        let base = s.key_text();
        assert_ne!(base, custom);
        s.speaker = "serena".into();
        s.instruct = "有精神一点".into();
        s.parallel = 1;
        assert_eq!(s.key_text(), base);
        s.designed = Some("ffd2".into());
        assert_ne!(s.key_text(), base);
        s.designed = Some("f13e".into());
        s.seed = 4;
        assert_ne!(s.key_text(), base);
        s.seed = 0;
        s.chunk_chars = 120;
        assert_ne!(s.key_text(), base);
    }

    #[test]
    fn takes_stopped_at_their_budget() {
        assert!(at_budget(100 * FRAME_SAMPLES, 100));
        // The codec may trim a frame or two at the end.
        assert!(at_budget(98 * FRAME_SAMPLES, 100));
        assert!(!at_budget(97 * FRAME_SAMPLES, 100));
        assert!(!at_budget(10 * FRAME_SAMPLES, 100));
    }

    #[test]
    fn lead_ins() {
        // Short: the whole sentence.
        assert_eq!(lead_in("他点了点头。"), "他点了点头。");
        // Long: its last clauses within the limit, from after a comma.
        let long = format!("{}，{}，{}。", said(30, ""), said(30, ""), said(20, ""));
        let l = lead_in(&long);
        assert_eq!(l, format!("{}，{}。", said(30, ""), said(20, "")));
        assert!(l.chars().count() <= LEAD_MAX);
        // No comma near the end: the last characters.
        let flat = said(100, "。");
        assert_eq!(lead_in(&flat).chars().count(), LEAD_MAX);
    }

    #[test]
    fn lead_ins_are_read_first_and_on_their_line() {
        let texts = [said(3, "。"), said(2, "。")];
        let lead = Lead {
            text: "前面。".into(),
            same_para: false,
        };
        let (text, all, paras) = assemble(&texts, &[5, 6], Some(&lead));
        assert_eq!(text, "前面。\n字字字。\n字字。");
        assert_eq!(all.len(), 3);
        assert_eq!(paras, [4, 5, 6]);
        let same = Lead {
            same_para: true,
            ..lead
        };
        assert_eq!(
            assemble(&texts, &[5, 6], Some(&same)).0,
            "前面。字字字。\n字字。"
        );
        assert_eq!(assemble(&texts, &[5, 6], None).0, "字字字。\n字字。");
    }

    #[test]
    fn cut_points_give_their_pause() {
        let texts = [said(10, "。"), said(10, "。")];
        let pcm = speech(&[(2.0, 0.6), (2.0, 0.0)]);
        let p = cut_points(&pcm, &texts, &[0, 1]);
        let (cut, pause) = (p[0].0, p[0].1.unwrap());
        // The pause runs 2.0–2.6 s; the cut is in its middle.
        assert!(
            pause.0.abs_diff(at(2.0)) <= 240 && pause.1.abs_diff(at(2.6)) <= 240,
            "{pause:?}"
        );
        assert!(cut > pause.0 && cut < pause.1);
    }

    #[test]
    fn marks_in_text() {
        let texts = [
            "他说：“你好，再见。”".to_owned(),
            "好！".to_owned(),
            "是的……".to_owned(),
        ];
        let (m, total) = marks(&texts, &[0, 0, 1]);
        let kinds: Vec<(f64, Mark)> = m.iter().map(|x| (x.0, x.1)).collect();
        assert_eq!(
            kinds,
            [
                (2.0, Mark::Minor),
                (4.0, Mark::Minor),
                (6.0, Mark::Sentence),
                (7.0, Mark::Paragraph)
            ]
        );
        assert_eq!(m[2].2, Some(0));
        assert_eq!(total, 9.0);
    }

    pub(super) fn test_wav() -> Vec<u8> {
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&24_000u32.to_le_bytes());
        w.extend_from_slice(&48_000u32.to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&4u32.to_le_bytes());
        w.extend_from_slice(&[1, 0, 0xff, 0xff]);
        w
    }

    #[test]
    fn wav_parsing() {
        assert_eq!(wav_samples(&test_wav()), Some(vec![1, -1]));
        assert_eq!(wav_samples(b"nope"), None);
    }
}
