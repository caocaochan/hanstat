//! The local voice: Qwen3-TTS 1.7B CustomVoice (Alibaba Qwen, Apache-2.0),
//! Q8_0 GGUF, run on the GPU by qwentts.cpp's `tts-server`
//! ([`hanstat::models::QWEN_TTS`], [`hanstat::models::QWEN_TTS_MODEL`]),
//! nothing downloaded until the user asks.
//!
//! One server for the whole app, started on first use and kept until
//! hanstat closes (or "Unload"), so the model is loaded once. It batches up
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
use hanstat::models::{QWEN_TTS, QWEN_TTS_MODEL, QWEN_TTS_SERVER};
use serde::{Deserialize, Serialize};
use std::ops::Range;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

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
    /// The cache-key part: what the audio depends on.
    pub fn key_text(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            QWEN_TTS_MODEL.id,
            opus::FORMAT,
            self.speaker,
            self.instruct.trim(),
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

/// Where to cut a chunk's audio (`pcm`) into its sentences, whose lengths in
/// characters are `lens`: one sample index per boundary, increasing.
///
/// Every boundary gets its own pause (a run of silent frames), in order;
/// of all such assignments the one where pauses are long and close to the
/// boundary's expected place wins (dynamic programming). The expected place
/// is the boundary's share of the characters, measured in voiced time, so
/// long paragraph pauses don't push later estimates off. A cut is the middle
/// of its pause. With fewer pauses than boundaries the cuts fall at the
/// expected places, at the quietest frame near each.
pub fn cuts(pcm: &[i16], lens: &[usize]) -> Vec<usize> {
    let m = lens.len().saturating_sub(1);
    if m == 0 {
        return Vec::new();
    }
    let frames = pcm.len() / HOP;
    let energy: Vec<f64> = (0..frames)
        .map(|f| {
            let s = &pcm[f * HOP..(f + 1) * HOP];
            (s.iter().map(|&x| (x as f64 / 32768.0).powi(2)).sum::<f64>() / HOP as f64).sqrt()
        })
        .collect();
    // Pauses inside the speech (not leading or trailing silence).
    let mut pauses: Vec<(usize, usize)> = Vec::new();
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
    // Voiced frames before each frame.
    let mut voiced = vec![0usize; frames + 1];
    {
        let mut in_pause = vec![false; frames];
        for &(a, b) in &pauses {
            in_pause[a..b].iter_mut().for_each(|x| *x = true);
        }
        for f in 0..frames {
            voiced[f + 1] = voiced[f] + usize::from(!in_pause[f]);
        }
    }
    let total_voiced = voiced[frames].max(1) as f64;
    let total_chars: usize = lens.iter().sum::<usize>().max(1);
    // Expected place of boundary j, in voiced seconds.
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
        return expected
            .iter()
            .map(|&e| {
                // Voiced seconds to a frame: the first frame with that many before it.
                let target = (e * 100.0) as usize;
                let f = voiced
                    .partition_point(|&v| v < target)
                    .min(frames.saturating_sub(1));
                let lo = f.saturating_sub(30);
                let hi = (f + 30).min(frames);
                let q = (lo..hi)
                    .min_by(|&a, &b| energy[a].total_cmp(&energy[b]))
                    .unwrap_or(f);
                q * HOP + HOP / 2
            })
            .collect();
    }
    // score(j, k): pause k as boundary j. Long pauses count up to 1 s.
    let score = |j: usize, k: usize| {
        let (a, b) = pauses[k];
        let len = ((b - a) as f64 / 100.0).min(1.0);
        let at = voiced[a] as f64 / 100.0;
        1.5 * len - (at - expected[j]).abs()
    };
    // best[j][k]: best total for boundaries 0..=j with boundary j at pause k.
    let mut best = vec![vec![f64::NEG_INFINITY; r]; m];
    let mut from = vec![vec![usize::MAX; r]; m];
    for (k, b) in best[0].iter_mut().enumerate() {
        *b = score(0, k);
    }
    for j in 1..m {
        let mut run_best = f64::NEG_INFINITY;
        let mut run_k = usize::MAX;
        for k in j..r {
            // Best of best[j-1][..k].
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

/// qwentts.cpp's server with the model loaded, on a free local port.
pub struct Server {
    child: Mutex<Child>,
    url: String,
    agent: ureq::Agent,
    parallel: usize,
    /// Requests in flight, at most `parallel` (more would wait in the
    /// server's queue behind what was asked first).
    busy: Mutex<usize>,
    freed: Condvar,
    log: PathBuf,
}

/// The one server, shared by every reader and the preview.
static SERVER: Mutex<Option<Arc<Server>>> = Mutex::new(None);

/// What the server is doing, for the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Unloaded,
    Loaded,
}

pub fn status() -> Status {
    match SERVER.lock().unwrap().as_ref() {
        Some(s) if s.alive() => Status::Loaded,
        _ => Status::Unloaded,
    }
}

/// Stop the server, freeing the GPU memory.
pub fn unload() {
    if let Some(s) = SERVER.lock().unwrap().take() {
        s.kill();
    }
}

/// The running server for `parallel` requests at once, started (and the
/// model loaded) if there is none or it ran for another number.
pub fn server(parallel: usize) -> Result<Arc<Server>, String> {
    let mut g = SERVER.lock().unwrap();
    if let Some(s) = g.as_ref() {
        if s.alive() && s.parallel == parallel {
            return Ok(s.clone());
        }
        s.kill();
        *g = None;
    }
    let s = Arc::new(Server::start(parallel)?);
    *g = Some(s.clone());
    Ok(s)
}

impl Server {
    fn start(parallel: usize) -> Result<Server, String> {
        if !downloaded() {
            return Err(NOT_DOWNLOADED.into());
        }
        let exe = QWEN_TTS.dir().join(QWEN_TTS_SERVER.unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| format!("Cannot make {} executable: {e}", exe.display()))?;
        }
        let dir = QWEN_TTS_MODEL.dir();
        let port = std::net::TcpListener::bind(("127.0.0.1", 0))
            .and_then(|l| l.local_addr())
            .map_err(|e| format!("No free local port: {e}"))?
            .port();
        let log = hanstat::models::cache_root().join("tts-server.log");
        let out = std::fs::File::create(&log)
            .map_err(|e| format!("Cannot create {}: {e}", log.display()))?;
        let err = out.try_clone().map_err(|e| e.to_string())?;
        let mut cmd = Command::new(&exe);
        cmd.current_dir(QWEN_TTS.dir())
            .arg("--model")
            .arg(dir.join(QWEN_TTS_MODEL.files[0].name))
            .arg("--codec")
            .arg(dir.join(QWEN_TTS_MODEL.files[1].name))
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
        let child = hanstat::child_guard::spawn(&mut cmd, |_| {})
            .map_err(|e| format!("Cannot start {}: {e}", exe.display()))?;
        let s = Server {
            child: Mutex::new(child),
            url: format!("http://127.0.0.1:{port}"),
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(900)))
                .http_status_as_error(false)
                .build()
                .into(),
            parallel: parallel.max(1),
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

    /// `text` read with `s`: 16-bit mono samples at [`opus::RATE`].
    pub fn synth(&self, text: &str, s: &LocalSettings, seed: u64) -> Result<Vec<i16>, String> {
        {
            let mut b = self.busy.lock().unwrap();
            while *b >= self.parallel {
                b = self.freed.wait(b).unwrap();
            }
            *b += 1;
        }
        let r = self.request(text, s, seed);
        *self.busy.lock().unwrap() -= 1;
        self.freed.notify_one();
        r
    }

    fn request(&self, text: &str, s: &LocalSettings, seed: u64) -> Result<Vec<i16>, String> {
        let mut body = serde_json::json!({
            "input": text,
            "voice": s.speaker,
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
        if !s.instruct.trim().is_empty() {
            body["instructions"] = s.instruct.trim().into();
        }
        let body = serde_json::to_vec(&body).unwrap();
        let resp = self
            .agent
            .post(format!("{}/v1/audio/speech", self.url))
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
        wav_samples(&bytes).ok_or_else(|| "The local voice sent no audio".to_owned())
    }
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

/// One chunk read and cut into its sentences' clips. A take far longer or
/// shorter than the text should be (the model ran on or skipped text) is
/// tried again once with another seed.
pub fn make_chunk(
    texts: &[String],
    paras: &[usize],
    s: &LocalSettings,
) -> Result<Vec<Vec<i16>>, Fail> {
    // Not downloaded, or it won't start: no use trying the next chunk.
    let server = server(s.parallel).map_err(Fail::Refused)?;
    // Paragraphs on lines of their own, sentences run together.
    let mut text = String::new();
    for (i, t) in texts.iter().enumerate() {
        if i > 0 && paras[i] != paras[i - 1] {
            text.push('\n');
        }
        text.push_str(t);
    }
    let lens: Vec<usize> = texts.iter().map(|t| t.chars().count()).collect();
    let chars: usize = lens.iter().sum();
    let base = hanstat::dict::xxh64::xxh64(text.as_bytes(), s.seed as u64);
    let mut pcm = Vec::new();
    for attempt in 0..2u64 {
        pcm = server
            .synth(&text, s, base.wrapping_add(attempt))
            .map_err(Fail::Other)?;
        let secs = pcm.len() as f64 / opus::RATE as f64;
        let want = seconds(chars);
        if secs < want * 3.0 + 4.0 && secs > want * 0.25 {
            break;
        }
    }
    let cut = cuts(&pcm, &lens);
    let mut out = Vec::with_capacity(texts.len());
    let mut a = 0;
    for &c in cut.iter().chain(std::iter::once(&pcm.len())) {
        let c = c.clamp(a, pcm.len());
        out.push(pcm[a..c].to_vec());
        a = c;
    }
    Ok(out)
}

/// A short passage read with `s`, for hearing the settings before making a
/// chapter: its samples at [`opus::RATE`]. Not saved.
pub fn preview(text: &str, s: &LocalSettings) -> Result<Vec<i16>, String> {
    let seed = hanstat::dict::xxh64::xxh64(text.as_bytes(), s.seed as u64);
    server(s.parallel)?.synth(text, s, seed)
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

    #[test]
    fn cuts_land_in_sentence_pauses_not_comma_pauses() {
        // Three sentences of 20, 10 and 30 characters (about 5.3, 2.6, 7.9 s),
        // with short comma pauses inside and long pauses between.
        let pcm = speech(&[(2.5, 0.15), (2.8, 0.7), (2.6, 0.9), (4.0, 0.2), (3.9, 0.0)]);
        let c = cuts(&pcm, &[20, 10, 30]);
        assert_eq!(c.len(), 2);
        let at = |s: f64| (s * 24_000.0) as usize;
        // Inside the 0.7 s pause after 5.45 s and the 0.9 s one after 8.85 s.
        assert!(c[0] > at(5.45) && c[0] < at(6.15), "{c:?}");
        assert!(c[1] > at(8.85) && c[1] < at(9.75), "{c:?}");
    }

    #[test]
    fn short_lines_each_get_their_own_pause() {
        // "……" lines between sentences: several short sentences in a row.
        let pcm = speech(&[(3.0, 1.0), (0.6, 1.0), (0.5, 1.0), (3.0, 0.0)]);
        let c = cuts(&pcm, &[12, 2, 2, 12]);
        assert_eq!(c.len(), 3);
        assert!(c[0] < c[1] && c[1] < c[2], "distinct and in order: {c:?}");
        let at = |s: f64| (s * 24_000.0) as usize;
        assert!(c[0] > at(3.0) && c[0] < at(4.0));
        assert!(c[1] > at(4.6) && c[1] < at(5.6));
        assert!(c[2] > at(6.1) && c[2] < at(7.1));
    }

    #[test]
    fn without_enough_pauses_cuts_fall_by_characters() {
        let pcm = speech(&[(6.0, 0.0)]);
        let c = cuts(&pcm, &[1, 1]);
        assert_eq!(c.len(), 1);
        let mid = 3 * 24_000;
        assert!(c[0].abs_diff(mid) < 24_000, "{c:?}");
        assert!(cuts(&pcm, &[5]).is_empty());
    }

    #[test]
    fn wav_parsing() {
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
        assert_eq!(wav_samples(&w), Some(vec![1, -1]));
        assert_eq!(wav_samples(b"nope"), None);
    }
}
