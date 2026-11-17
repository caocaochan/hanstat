//! Writing-style difficulty: how hard a Chinese book's prose is to read for
//! a learner who knows its words — sentence structure, written or classical
//! register, rhetoric and description, how much a reader must infer. The
//! share of words known ([`crate::anki`]) misses this: two books with the
//! same coverage can read very differently.
//!
//! **How.** A local language model, Qwen3.5-4B ([`models::STYLE_LLM`]), run by
//! llama.cpp's server ([`models::LLAMA_CPP`]) on the GPU (Vulkan or Metal)
//! or the CPU, rates four samples of [`SAMPLE_HAN`] Chinese characters taken
//! at 30, 45, 60 and 75% of the book's body text ([`samples`]) on a scale
//! of 1 to 9, with five short example passages for levels 1, 3, 5, 7 and 9
//! and the instruction to assume every word is known. Each sample is asked
//! five questions (overall, syntax, register, rhetoric, narrative); an
//! answer counts as the expected value of the digit from the model's
//! probabilities, not the digit itself. A sample's rating is the mean of its
//! answers, a book's the mean of its samples. Chapters are not used: a
//! section is 600 or 40,000 characters depending on how the book was made.
//!
//! **Measured** on 20 novels (2026-11-17), against the first prompt tried
//! (one question, which asked to "ignore vocabulary" and described only the
//! odd levels): the agreement between samples of one book (ICC) went from
//! 0.41 to 0.57 for one sample, 0.84 for the mean of four; the rating's
//! correlation with the share known is −0.54, and what it adds beyond that
//! share repeats between the two halves of a book's samples (r = 0.62; 0.13
//! with the first prompt). Books' ratings ran from 5.4 to 6.6. It agreed
//! with a reader on the three pairs checked, one of which the share known
//! puts the other way round. Ratings are kept in `style.json` by History id.
//!
//! What the rater did (downloads, starting llama.cpp, each book's samples
//! and answers, and the reason for every failure) goes to `style.log` next
//! to it, written anew each time a [`Rater`] starts ([`Log`]).

use crate::extract::{Book, EncodingChoice, SectionKind};
use crate::models::{self, LLAMA_CPP, LLAMA_SERVER, STYLE_LLM};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Bumped whenever the samples, prompt or model change, so that older
/// ratings are made again.
pub const VERSION: u32 = 1;
/// Where the samples are taken, as shares of the body text.
pub const POSITIONS: [f64; 4] = [0.30, 0.45, 0.60, 0.75];
/// Chinese characters in each sample (from whole paragraphs, so a little more).
pub const SAMPLE_HAN: usize = 3000;
/// A book with fewer Chinese characters in its body is not rated.
pub const MIN_HAN: usize = 2000;
/// The rating that counts as typical: the median of the 20 novels measured.
/// The reading path asks more of a book above it and less below.
pub const TYPICAL: f32 = 5.95;

/// What a rating was made with; one made otherwise is out of date.
pub fn method() -> String {
    format!("{}/v{VERSION}", STYLE_LLM.id)
}

/// Whether this platform has a build of llama.cpp's server.
pub fn supported() -> bool {
    LLAMA_SERVER.is_some()
}

/// Bytes to download before the first rating (0 once everything is there).
pub fn download_size() -> u64 {
    [&LLAMA_CPP, &STYLE_LLM]
        .iter()
        .filter(|m| !m.is_cached())
        .map(|m| m.total_size())
        .sum()
}

fn is_han(c: char) -> bool {
    matches!(c, '\u{4e00}'..='\u{9fff}' | '\u{3400}'..='\u{4dbf}')
}

fn han(s: &str) -> usize {
    s.chars().filter(|&c| is_han(c)).count()
}

/// A sample ends with the paragraph that reaches [`SAMPLE_HAN`], unless that
/// paragraph goes this many Chinese characters beyond it: then it is cut.
const OVERSHOOT: usize = 1000;

/// The start of `s` up to its `n`th Chinese character.
fn han_prefix(s: &str, n: usize) -> &str {
    let mut seen = 0;
    for (i, c) in s.char_indices() {
        if is_han(c) {
            if seen == n {
                return &s[..i];
            }
            seen += 1;
        }
    }
    s
}

/// The samples rated for a book: whole paragraphs from about [`SAMPLE_HAN`]
/// Chinese characters before to after each of [`POSITIONS`] of its body
/// text (the sections in reading order, after any short ones at the start:
/// cover, contents, blurb). `None` if the body is shorter than [`MIN_HAN`].
pub fn samples(book: &Book) -> Option<Vec<String>> {
    let body: Vec<&Vec<String>> = book
        .sections
        .iter()
        .filter(|s| s.kind == SectionKind::Body)
        .map(|s| &s.paragraphs)
        .collect();
    let start = body
        .iter()
        .position(|ps| ps.iter().map(|p| han(p)).sum::<usize>() >= 1000)
        .unwrap_or(0);
    let paras: Vec<&str> = body[start..]
        .iter()
        .flat_map(|ps| ps.iter().map(|p| p.trim()))
        .filter(|p| !p.is_empty())
        .collect();
    let counts: Vec<usize> = paras.iter().map(|p| han(p)).collect();
    let total: usize = counts.iter().sum();
    if total < MIN_HAN {
        return None;
    }
    let mut cum = Vec::with_capacity(counts.len());
    let mut acc = 0;
    for &c in &counts {
        cum.push(acc);
        acc += c;
    }
    Some(
        POSITIONS
            .iter()
            .map(|&pos| {
                let target = (total as f64 * pos - SAMPLE_HAN as f64 / 2.0).max(0.0) as usize;
                let mut i = cum.iter().position(|&c| c >= target).unwrap_or(0);
                let (mut out, mut n) = (Vec::new(), 0);
                while i < paras.len() && n < SAMPLE_HAN {
                    let left = SAMPLE_HAN - n;
                    if counts[i] > left + OVERSHOOT {
                        // A whole chapter as one paragraph: cut it, so that
                        // the sample fits the model's context.
                        out.push(han_prefix(paras[i], left));
                        break;
                    }
                    out.push(paras[i]);
                    n += counts[i];
                    i += 1;
                }
                out.join("\n")
            })
            .collect(),
    )
}

/// One book's rating.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rating {
    /// Mean of `samples`, 1 (plain, spoken) to 9 (near classical Chinese).
    pub score: f32,
    /// Each sample's rating, in [`POSITIONS`] order.
    pub samples: Vec<f32>,
    /// [`method`] when it was made.
    pub method: String,
    /// Seconds since the Unix epoch.
    pub rated: u64,
}

impl Rating {
    /// A word for where the rating falls among the 20 novels measured (5.4
    /// to 6.6): 5 of them plain, 7 typical, 4 literary, 4 very literary.
    pub fn label(&self) -> &'static str {
        label(self.score)
    }
}

/// See [`Rating::label`].
pub fn label(score: f32) -> &'static str {
    match score {
        s if s < 5.6 => "Plain",
        s if s < 6.0 => "Typical",
        s if s < 6.3 => "Literary",
        _ => "Very literary",
    }
}

/// The ratings made so far, by History id, in `style.json` next to the History.
#[derive(Default, Serialize, Deserialize)]
pub struct Store {
    books: HashMap<String, Rating>,
}

impl Store {
    pub fn path() -> PathBuf {
        models::app_dir().join("style.json")
    }

    /// Missing file → empty; an unreadable one is reported (and replaced on the next save).
    pub fn load() -> (Store, Option<String>) {
        let p = Self::path();
        match std::fs::read(&p) {
            Err(_) => (Store::default(), None),
            Ok(b) => match serde_json::from_slice(&b) {
                Ok(s) => (s, None),
                Err(e) => (
                    Store::default(),
                    Some(format!("Ignoring unreadable {}: {e}", p.display())),
                ),
            },
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let p = Self::path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap())
            .and_then(|()| std::fs::rename(&tmp, &p))
            .map_err(|e| format!("Cannot save {}: {e}", p.display()))
    }

    /// The book's rating, if made with the current [`method`].
    pub fn get(&self, id: &str) -> Option<&Rating> {
        let m = method();
        self.books.get(id).filter(|r| r.method == m)
    }

    pub fn insert(&mut self, id: String, r: Rating) {
        self.books.insert(id, r);
    }

    /// Ratings of the current method, by id.
    pub fn current(&self) -> HashMap<&str, f32> {
        let m = method();
        self.books
            .iter()
            .filter(|(_, r)| r.method == m)
            .map(|(id, r)| (id.as_str(), r.score))
            .collect()
    }
}

/// What the rater did, to find out why a book was not rated: `style.log`
/// next to `style.json`, replaced each time a [`Rater`] starts. Each line
/// starts with the seconds since then. A log that cannot be written is
/// left out; rating goes on without it.
#[derive(Clone, Default)]
pub struct Log(Option<Arc<Mutex<(std::fs::File, Instant)>>>);

impl Log {
    pub fn path() -> PathBuf {
        models::app_dir().join("style.log")
    }

    /// Start the log afresh, with what is used and when.
    pub fn create() -> Log {
        let p = Self::path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let log = match std::fs::File::create(&p) {
            Ok(f) => Log(Some(Arc::new(Mutex::new((f, Instant::now()))))),
            Err(_) => Log(None),
        };
        let state = |m: &models::Model| {
            if m.is_cached() {
                "downloaded"
            } else {
                "not downloaded"
            }
        };
        log.line(format!(
            "hanstat {} on {}-{}, {} UTC; rating method {}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            utc(crate::history::now()),
            method()
        ));
        log.line(format!(
            "llama.cpp {} in {} ({}); model in {} ({})",
            LLAMA_CPP.id,
            LLAMA_CPP.dir().display(),
            state(&LLAMA_CPP),
            STYLE_LLM.dir().display(),
            state(&STYLE_LLM),
        ));
        log
    }

    /// Add `msg`; its later lines are indented under the first.
    pub fn line(&self, msg: impl AsRef<str>) {
        let Some(f) = &self.0 else { return };
        let mut g = f.lock().unwrap_or_else(|e| e.into_inner());
        let t = g.1.elapsed().as_secs_f64();
        let text = msg.as_ref().trim_end().replace('\n', "\n           ");
        let _ = writeln!(g.0, "[{t:8.1}] {text}");
    }
}

/// `secs` since the Unix epoch as `YYYY-MM-DD hh:mm:ss`.
fn utc(secs: u64) -> String {
    // Howard Hinnant's civil_from_days.
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// At most the last `n` characters of `s`, for the log.
fn tail_chars(s: &str, n: usize) -> &str {
    let k = s.chars().count();
    match s.char_indices().nth(k.saturating_sub(n)) {
        Some((i, _)) if k > n => &s[i..],
        _ => s,
    }
}

const SYSTEM: &str = "你是对外汉语阅读分级专家。你只评估文风难度，并且假设读者认识文中的每一个词语、人名和术语；生词多少完全不计入评分。评分范围1到9，2、4、6、8为相邻等级之间的中间值，请充分使用整个量表。

【示例与等级】（示例只为说明文风，读者认识其中所有词语）
等级1：
“你吃饭了吗？”“还没。你呢？”“我也没有。走，一起去吧。”他笑了笑，拿起包就走。外面下雨了。两个人站在门口，看了看天。“没带伞。”“那就跑吧！”
等级3：
早上七点，林小雨像平时一样走进公司。办公室里还没有什么人，她打开电脑，给自己倒了一杯热水。窗外的天有点阴，好像要下雨。她想起昨天经理说的话，心里有些不安，但还是决定先把手上的工作做完。
等级5：
他站在走廊尽头，看着窗外那片被夕阳染成橘红色的天空，心里那点说不清道不明的烦躁慢慢沉了下去。这些年他习惯了独来独往，习惯了把所有情绪都压在一张若无其事的脸后面，可偏偏是这个人，三言两语就让他乱了阵脚。
等级7：
是夜，长街寂寂，唯有更夫的梆子声自巷口遥遥传来。他倚窗独坐，案上残灯如豆，映得那封未拆的书信忽明忽暗。昔年纵马长安的少年，如今鬓边竟已染了霜色；而那些说好要同饮一壶酒的人，或葬于边关黄沙，或隐于江湖烟雨，再无一人可问归期。
等级9：
时值深秋，朔风渐起，庭前梧桐萧萧而下。公负手立于阶前，良久乃叹曰：“吾少时读书，自谓可致君尧舜，今观之，不过一腐儒耳。”左右皆默然，莫敢对。是岁冬，公卒于任，年五十有三。";

/// Asked of each sample; its rating is the mean of the answers. The overall
/// question alone rates about as consistently, but the mean is what was
/// checked against a reader.
const QUESTIONS: [&str; 5] = [
    "对照上面的示例，这段原文的整体文风难度是几级？",
    "只看句法：句子长度、长定语、从句与插入成分的嵌套程度。1为短小单句，9为大量长句与多层嵌套。这段原文是几级？",
    "只看语体：书面语、文言词汇与文言句式、四字格和成语的密度。1为纯口语，9为接近文言。这段原文是几级？",
    "只看修辞与描写：比喻、排比、意象、景物与心理描写的密度和含蓄程度。1为几乎没有，9为通篇密集且含蓄。这段原文是几级？",
    "只看叙事跟读难度：视角与时间的跳跃、省略和暗示、需要推断言外之意的对话。1为线性直白，9为频繁跳跃且大量留白。这段原文是几级？",
];

/// The request for one question about a sample. The sample comes before the
/// question, so the server reuses its processed prompt for the next question.
fn request(sample: &str, question: &str) -> serde_json::Value {
    serde_json::json!({
        "messages": [
            {"role": "system", "content": SYSTEM},
            {"role": "user", "content": format!("原文：\n{sample}\n\n{question}只输出一个数字。")},
        ],
        "max_tokens": 1,
        "temperature": 0,
        "logprobs": true,
        "top_logprobs": 20,
        "chat_template_kwargs": {"enable_thinking": false},
        "cache_prompt": true,
    })
}

/// The expected digit 1–9 from the answer's top probabilities, if the
/// digits have at least half of the probability.
fn expected_digit(resp: &serde_json::Value) -> Result<f32, String> {
    let top = resp["choices"][0]["logprobs"]["content"][0]["top_logprobs"]
        .as_array()
        .ok_or("the answer has no probabilities")?;
    let (mut mass, mut sum) = (0f64, 0f64);
    for t in top {
        let (Some(tok), Some(lp)) = (t["token"].as_str(), t["logprob"].as_f64()) else {
            continue;
        };
        if let Ok(d @ 1..=9) = tok.trim().parse::<u32>() {
            let p = lp.exp();
            mass += p;
            sum += p * d as f64;
        }
    }
    if mass < 0.5 {
        let tops: Vec<String> = top
            .iter()
            .take(5)
            .map(|t| {
                format!(
                    "{:?} {:.0}%",
                    t["token"].as_str().unwrap_or("?"),
                    t["logprob"].as_f64().unwrap_or(f64::NEG_INFINITY).exp() * 100.0
                )
            })
            .collect();
        return Err(format!(
            "the model did not answer with a digit ({:?}; most likely: {})",
            resp["choices"][0]["message"]["content"],
            tops.join(", ")
        ));
    }
    Ok((sum / mass) as f32)
}

/// llama.cpp's server with [`STYLE_LLM`] loaded, on a free local port. It is
/// stopped when dropped.
pub struct Server {
    child: Arc<Mutex<Child>>,
    url: String,
    agent: ureq::Agent,
    /// llama.cpp's own output.
    server_log: PathBuf,
    log: Log,
}

/// How llama.cpp is run, fastest first. Each is tried after llama.cpp
/// crashed while rating with the one before: on an RX 7600 XT (Windows,
/// Adrenalin 32.0.32015.7029, 2026-11-17) the Vulkan build crashed with an
/// access violation on the first prompt unless cooperative matrices or
/// flash attention were off, while every setting worked on an RTX 5070.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Gpu,
    /// `GGML_VK_DISABLE_COOPMAT=1`.
    GpuNoCoopmat,
    /// Both off: `GGML_VK_DISABLE_COOPMAT=1` and `-fa off`.
    GpuNoFlashAttn,
    Cpu,
}

impl Mode {
    /// The one to try after a crash with this one.
    pub fn next(self) -> Option<Mode> {
        match self {
            Mode::Gpu => Some(Mode::GpuNoCoopmat),
            Mode::GpuNoCoopmat => Some(Mode::GpuNoFlashAttn),
            Mode::GpuNoFlashAttn => Some(Mode::Cpu),
            Mode::Cpu => None,
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Mode::Gpu => "on the GPU",
            Mode::GpuNoCoopmat => "on the GPU without cooperative matrices",
            Mode::GpuNoFlashAttn => "on the GPU without cooperative matrices or flash attention",
            Mode::Cpu => "on the CPU only",
        }
    }

    /// `style-llama.json` next to `style.json`: the mode that last worked
    /// with this build of llama.cpp (a new build starts again from the GPU).
    fn path() -> PathBuf {
        models::app_dir().join("style-llama.json")
    }

    pub fn load() -> Mode {
        #[derive(Deserialize)]
        struct Saved {
            llama: String,
            mode: Mode,
        }
        std::fs::read(Self::path())
            .ok()
            .and_then(|b| serde_json::from_slice::<Saved>(&b).ok())
            .filter(|s| s.llama == LLAMA_CPP.id)
            .map_or(Mode::Gpu, |s| s.mode)
    }

    fn save(self) -> Result<(), String> {
        let json = serde_json::json!({"llama": LLAMA_CPP.id, "mode": self});
        std::fs::write(Self::path(), serde_json::to_vec_pretty(&json).unwrap())
            .map_err(|e| format!("Cannot save {}: {e}", Self::path().display()))
    }
}

/// llama.cpp's own list of the devices it can use, for the log.
pub fn list_devices() -> String {
    let Some(name) = LLAMA_SERVER else {
        return String::new();
    };
    let mut cmd = Command::new(LLAMA_CPP.dir().join(name));
    cmd.current_dir(LLAMA_CPP.dir())
        .arg("--list-devices")
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    match cmd.output() {
        Ok(o) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            lines.join("\n")
        }
        Err(e) => format!("llama.cpp --list-devices: {e}"),
    }
}

/// How a process ended, with Windows' NTSTATUS in hex (0xC0000005 is an
/// access violation).
fn describe_exit(st: std::process::ExitStatus) -> String {
    match st.code() {
        #[cfg(windows)]
        Some(c) => format!("exit code {:#010X}", c as u32),
        #[cfg(not(windows))]
        Some(c) => format!("exit code {c}"),
        None => st.to_string(),
    }
}

impl Server {
    /// Start the server and wait until the model is loaded, run as `mode`
    /// (on a GPU mode without a GPU, llama.cpp uses the CPU). Both downloads
    /// must be in the cache. What happens goes to `log`.
    pub fn start(mode: Mode, log: &Log) -> Result<Server, String> {
        let name = LLAMA_SERVER.ok_or("llama.cpp has no build for this platform")?;
        let exe = LLAMA_CPP.dir().join(name);
        let model = STYLE_LLM.dir().join(STYLE_LLM.files[0].name);
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
        let log_path = models::cache_root().join("llama-server.log");
        let out = std::fs::File::create(&log_path)
            .map_err(|e| format!("Cannot create {}: {e}", log_path.display()))?;
        let err = out.try_clone().map_err(|e| e.to_string())?;
        let mut cmd = Command::new(&exe);
        cmd.current_dir(LLAMA_CPP.dir())
            .arg("--model")
            .arg(&model)
            .args(["--host", "127.0.0.1", "--port", &port.to_string()])
            // A sample is about 2,600 tokens and the instructions 600.
            .args(["--ctx-size", "8192", "--parallel", "1", "--no-ui"])
            .args(match mode {
                Mode::Gpu | Mode::GpuNoCoopmat => &["--n-gpu-layers", "99"][..],
                Mode::GpuNoFlashAttn => &["--n-gpu-layers", "99", "--flash-attn", "off"][..],
                Mode::Cpu => &["--n-gpu-layers", "0", "--device", "none"][..],
            })
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        if matches!(mode, Mode::GpuNoCoopmat | Mode::GpuNoFlashAttn) {
            cmd.env("GGML_VK_DISABLE_COOPMAT", "1");
        }
        log.line(format!("Starting llama.cpp {}: {cmd:?}", mode.describe()));
        let child = cmd
            .spawn()
            .map_err(|e| format!("Cannot start {}: {e}", exe.display()))?;
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(600)))
            // Read the body of an error answer: it says what went wrong.
            .http_status_as_error(false)
            .build()
            .into();
        let mut s = Server {
            child: Arc::new(Mutex::new(child)),
            url: format!("http://127.0.0.1:{port}"),
            agent,
            server_log: log_path,
            log: log.clone(),
        };
        let t = Instant::now();
        match s.wait_ready(Duration::from_secs(600)) {
            Ok(()) => {
                log.line(format!(
                    "llama.cpp loaded the model in {:.1} s",
                    t.elapsed().as_secs_f64()
                ));
                Ok(s)
            }
            Err(e) => {
                log.line(format!(
                    "llama.cpp failed: {}\nIts output ({}) ends:\n{}",
                    e.lines().next().unwrap_or(""),
                    s.server_log.display(),
                    s.log_lines(80)
                ));
                Err(e)
            }
        }
    }

    /// The end of the server's log, for error messages.
    fn log_tail(&self) -> String {
        self.log_lines(6)
    }

    /// The last `n` lines of the server's log that are not empty.
    fn log_lines(&self, n: usize) -> String {
        let text = std::fs::read(&self.server_log)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        lines[lines.len().saturating_sub(n)..].join("\n")
    }

    /// How the server's process ended, if it does within `limit`. A crash
    /// closes the connection before the process is gone, so a failed
    /// request waits a moment before deciding the server is still alive.
    fn exited_within(&self, limit: Duration) -> Option<String> {
        let t = Instant::now();
        loop {
            match self.child.lock().unwrap().try_wait() {
                Ok(Some(st)) => return Some(describe_exit(st)),
                Err(e) => return Some(e.to_string()),
                Ok(None) => {}
            }
            if t.elapsed() >= limit {
                return None;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn wait_ready(&mut self, limit: Duration) -> Result<(), String> {
        let t = Instant::now();
        loop {
            if let Ok(Some(st)) = self.child.lock().unwrap().try_wait() {
                return Err(format!("llama.cpp stopped ({st}):\n{}", self.log_tail()));
            }
            // 503 while the model loads.
            if self
                .agent
                .get(format!("{}/health", self.url))
                .call()
                .is_ok_and(|r| r.status().is_success())
            {
                return Ok(());
            }
            if t.elapsed() > limit {
                return Err(format!(
                    "llama.cpp did not load the model in time:\n{}",
                    self.log_tail()
                ));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// One sample's rating, 1 to 9: the mean answer to [`QUESTIONS`].
    pub fn rate(&self, sample: &str) -> Result<f32, String> {
        let mut answers = Vec::with_capacity(QUESTIONS.len());
        for (k, q) in QUESTIONS.iter().enumerate() {
            let body = serde_json::to_vec(&request(sample, q)).unwrap();
            let (status, text) = self
                .agent
                .post(format!("{}/v1/chat/completions", self.url))
                .header("Content-Type", "application/json")
                .send(&body[..])
                .and_then(|mut r| Ok((r.status(), r.body_mut().read_to_string()?)))
                .map_err(|e| format!("llama.cpp, question {}: {e}", k + 1))?;
            if !status.is_success() {
                return Err(format!(
                    "llama.cpp answered question {} with HTTP {status}: {}",
                    k + 1,
                    tail_chars(text.trim(), 2000)
                ));
            }
            let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
                format!(
                    "llama.cpp's answer to question {} is not JSON ({e}): {}",
                    k + 1,
                    tail_chars(&text, 2000)
                )
            })?;
            answers.push(expected_digit(&v).map_err(|e| format!("Question {}: {e}", k + 1))?);
        }
        let mean = answers.iter().sum::<f32>() / answers.len() as f32;
        self.log.line(format!(
            "    answers {} → {mean:.2}",
            answers
                .iter()
                .map(|a| format!("{a:.2}"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
        Ok(mean)
    }

    /// A book's rating; `Ok(None)` if it is too short to rate.
    pub fn rate_book(&self, book: &Book) -> Result<Option<Rating>, String> {
        let body: Vec<_> = book
            .sections
            .iter()
            .filter(|s| s.kind == SectionKind::Body)
            .collect();
        self.log.line(format!(
            "  {}: {} sections, {} of them body text with {} paragraphs and {} Chinese characters",
            book.format,
            book.sections.len(),
            body.len(),
            body.iter().map(|s| s.paragraphs.len()).sum::<usize>(),
            body.iter()
                .flat_map(|s| &s.paragraphs)
                .map(|p| han(p))
                .sum::<usize>()
        ));
        let Some(samples) = samples(book) else {
            return Ok(None);
        };
        let mut scores = Vec::with_capacity(samples.len());
        for (k, s) in samples.iter().enumerate() {
            self.log.line(format!(
                "  sample {} of {} at {:.0}%: {} Chinese characters, {} in all, starting {:?}",
                k + 1,
                samples.len(),
                POSITIONS[k] * 100.0,
                han(s),
                s.chars().count(),
                s.chars().take(20).collect::<String>()
            ));
            scores.push(self.rate(s)?);
        }
        Ok(Some(Rating {
            score: scores.iter().sum::<f32>() / scores.len() as f32,
            samples: scores,
            method: method(),
            rated: crate::history::now(),
        }))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let mut c = self.child.lock().unwrap();
        let _ = c.kill();
        let _ = c.wait();
    }
}

/// A book to rate.
#[derive(Clone, Debug)]
pub struct Job {
    /// History id.
    pub id: String,
    pub path: PathBuf,
    pub title: String,
}

/// What became of a job.
pub enum Outcome {
    Rated(String, Rating),
    /// Too short, unreadable or gone: (id, why).
    Skipped(String, String),
}

/// What the rater is doing, for the status bar.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Phase {
    #[default]
    Idle,
    /// Downloading llama.cpp and the model (bytes).
    Downloading { done: u64, total: u64 },
    /// Loading the model.
    Starting,
    /// Rating `title`; `left` books are waiting after it.
    Rating { title: String, left: usize },
    /// The server could not be started, or crashed with every [`Mode`];
    /// nothing is rated until the rater is started again.
    Failed(String),
}

#[derive(Default)]
struct State {
    queue: VecDeque<Job>,
    stop: bool,
    phase: Phase,
    out: Vec<Outcome>,
    /// The running server's process, so that dropping the rater stops it
    /// even in the middle of a request.
    child: Option<Arc<Mutex<Child>>>,
    /// How llama.cpp is run ([`Mode::load`] at the start).
    mode: Mode,
}

type Shared = Arc<(Mutex<State>, Condvar)>;

/// Rates books one after another on a background thread: downloads what is
/// missing, starts the server on the first book and stops it after a minute
/// with nothing to do (freeing the GPU's memory). Dropping it stops all.
pub struct Rater {
    shared: Shared,
    thread: Option<std::thread::JoinHandle<()>>,
    log: Log,
}

/// How long the server is kept after the last book.
const IDLE: Duration = Duration::from_secs(60);

impl Rater {
    /// `notify` is called whenever there is something new to show. Starts
    /// a new [`Log`].
    pub fn new(notify: impl Fn() + Send + 'static) -> Rater {
        let shared: Shared = Arc::default();
        shared.0.lock().unwrap().mode = Mode::load();
        let s = shared.clone();
        let log = Log::create();
        let l = log.clone();
        let thread = std::thread::Builder::new()
            .name("style-rater".into())
            .spawn(move || run(s, notify, l))
            .expect("cannot start a thread");
        Rater {
            shared,
            thread: Some(thread),
            log,
        }
    }

    /// Queue `job`; `first`: before the others (the open book).
    pub fn push(&self, job: Job, first: bool) {
        let (m, cv) = &*self.shared;
        let mut st = m.lock().unwrap();
        if let Some(i) = st.queue.iter().position(|j| j.id == job.id) {
            if !first {
                return;
            }
            st.queue.remove(i);
        }
        self.log.line(format!(
            "Queued {}{} ({})",
            job.title,
            if first { ", first: it is open" } else { "" },
            job.path.display()
        ));
        if first {
            st.queue.push_front(job);
        } else {
            st.queue.push_back(job);
        }
        cv.notify_all();
    }

    pub fn phase(&self) -> Phase {
        self.shared.0.lock().unwrap().phase.clone()
    }

    /// How llama.cpp is run now.
    pub fn mode(&self) -> Mode {
        self.shared.0.lock().unwrap().mode
    }

    /// Books waiting, including the one being rated.
    pub fn pending(&self) -> usize {
        let st = self.shared.0.lock().unwrap();
        st.queue.len() + matches!(st.phase, Phase::Rating { .. }) as usize
    }

    /// What was done since the last call.
    pub fn take(&self) -> Vec<Outcome> {
        std::mem::take(&mut self.shared.0.lock().unwrap().out)
    }
}

impl Drop for Rater {
    fn drop(&mut self) {
        let (m, cv) = &*self.shared;
        {
            let mut st = m.lock().unwrap();
            st.stop = true;
            if let Some(c) = &st.child {
                let _ = c.lock().unwrap().kill();
            }
        }
        cv.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.log
            .line("Stopped: the app closed, the setting was turned off or rating started again");
    }
}

fn run(shared: Shared, notify: impl Fn(), log: Log) {
    let (m, cv) = &*shared;
    let set = |p: Phase| {
        m.lock().unwrap().phase = p;
        notify();
    };
    let fail = |e: String| {
        log.line(format!(
            "Rating stopped: {e}\nNothing more is rated until it is started again from the ⚙ menu."
        ));
        set(Phase::Failed(e));
    };
    let mut server: Option<Server> = None;
    let mut listed = false;
    loop {
        // The next job, or stop the server after `IDLE` without one.
        let job = {
            let mut st = m.lock().unwrap();
            loop {
                if st.stop {
                    return;
                }
                if matches!(st.phase, Phase::Failed(_)) {
                    st = cv.wait(st).unwrap();
                    continue;
                }
                if let Some(j) = st.queue.pop_front() {
                    break j;
                }
                let (g, t) = cv.wait_timeout(st, IDLE).unwrap();
                st = g;
                if t.timed_out() && st.queue.is_empty() && server.is_some() {
                    st.child = None;
                    drop(st);
                    log.line(format!(
                        "Nothing to rate for {} s: stopping llama.cpp",
                        IDLE.as_secs()
                    ));
                    server = None;
                    st = m.lock().unwrap();
                }
            }
        };
        if server.is_none() {
            let total = download_size();
            if total > 0 {
                set(Phase::Downloading { done: 0, total });
                let mut base = 0;
                for model in [&LLAMA_CPP, &STYLE_LLM] {
                    if model.is_cached() {
                        continue;
                    }
                    log.line(format!(
                        "Downloading {} ({:.0} MB) to {}",
                        model.id,
                        model.total_size() as f64 / 1e6,
                        model.dir().display()
                    ));
                    let t = Instant::now();
                    let res = model.ensure(&|d, _| {
                        let mut st = m.lock().unwrap();
                        st.phase = Phase::Downloading {
                            done: base + d,
                            total,
                        };
                        drop(st);
                        notify();
                    });
                    if let Err(e) = res {
                        m.lock().unwrap().queue.push_front(job.clone());
                        fail(format!("Cannot download {}: {e}", model.id));
                        break;
                    }
                    log.line(format!("Downloaded in {:.0} s", t.elapsed().as_secs_f64()));
                    base += model.total_size();
                }
                if matches!(m.lock().unwrap().phase, Phase::Failed(_)) {
                    continue;
                }
            }
            if !listed {
                listed = true;
                log.line(format!("llama.cpp --list-devices:\n{}", list_devices()));
            }
            set(Phase::Starting);
            let mode = m.lock().unwrap().mode;
            // A GPU that cannot even load the model: the CPU then (for this run only).
            let started = Server::start(mode, &log).or_else(|gpu| {
                if mode == Mode::Cpu {
                    return Err(gpu);
                }
                log.line("Trying again on the CPU only");
                let s = Server::start(Mode::Cpu, &log)
                    .map_err(|cpu| format!("{gpu}\n\nOn the CPU only: {cpu}"))?;
                m.lock().unwrap().mode = Mode::Cpu;
                Ok(s)
            });
            match started {
                Ok(s) => {
                    m.lock().unwrap().child = Some(s.child.clone());
                    server = Some(s);
                }
                Err(e) => {
                    m.lock().unwrap().queue.push_front(job);
                    fail(e);
                    continue;
                }
            }
        }
        let left = m.lock().unwrap().queue.len();
        set(Phase::Rating {
            title: job.title.clone(),
            left,
        });
        log.line(format!("Rating {} ({})", job.title, job.path.display()));
        let t = Instant::now();
        let srv = server.as_ref().unwrap();
        // A panic would leave the rater stuck on this book: report it as its failure.
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::extract::load(&job.path, EncodingChoice::Auto)
                .map_err(|e| format!("Cannot read the book: {e}"))
                .and_then(|book| srv.rate_book(&book))
        }))
        .unwrap_or_else(|p| {
            let msg = p
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            Err(format!("Internal error (panic): {msg}"))
        });
        let secs = t.elapsed().as_secs_f64();
        // A failed request may mean llama.cpp crashed: not the book's fault.
        let exit = if res.is_err() {
            srv.exited_within(Duration::from_secs(3))
        } else {
            None
        };
        if let Some(how) = &exit {
            log.line(format!(
                "llama.cpp stopped ({how}) while rating; its output ({}) ends:\n{}",
                srv.server_log.display(),
                srv.log_lines(80)
            ));
        }
        let mut st = m.lock().unwrap();
        if st.stop {
            return;
        }
        if let Some(how) = exit {
            st.child = None;
            drop(st);
            server = None;
            st = m.lock().unwrap();
            // The book again, with llama.cpp run more safely, unless it
            // crashed every way.
            st.queue.push_front(job);
            let was = st.mode;
            match was.next() {
                Some(next) => {
                    st.mode = next;
                    st.phase = Phase::Idle;
                    drop(st);
                    log.line(format!(
                        "  llama.cpp crashed {}: trying the book again {}",
                        was.describe(),
                        next.describe()
                    ));
                    if let Err(e) = next.save() {
                        log.line(e);
                    }
                    notify();
                }
                None => {
                    drop(st);
                    fail(format!(
                        "llama.cpp crashed ({how}) with every setting tried, on the GPU and on the CPU. \
                         Its output is in the log."
                    ));
                }
            }
            continue;
        }
        match res {
            Ok(Some(r)) => {
                log.line(format!(
                    "  rated {:.2} ({}) in {secs:.1} s",
                    r.score,
                    r.samples
                        .iter()
                        .map(|x| format!("{x:.2}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                st.out.push(Outcome::Rated(job.id, r));
            }
            Ok(None) => {
                let why = format!("Fewer than {MIN_HAN} Chinese characters in the body text");
                log.line(format!("  not rated: {why}"));
                st.out.push(Outcome::Skipped(job.id, why));
            }
            Err(e) => {
                log.line(format!("  FAILED after {secs:.1} s: {e}"));
                st.out.push(Outcome::Skipped(job.id, e));
            }
        }
        st.phase = Phase::Idle;
        drop(st);
        notify();
    }
}

/// Percentage points of running words known that one point of rating counts
/// for, by default. Not measured: a judgment that a book rated a point above
/// typical (plain → literary) reads about like one with 2% fewer words known.
pub const DEFAULT_WEIGHT: f32 = 2.0;

/// How much harder a book's writing makes it, in percentage points of running
/// words known: `weight` for each point its rating is above [`TYPICAL`]
/// (negative below). Its difficulty counts it as that much less known, and
/// the reading path raises its target by as much.
pub fn target_offset(score: f32, weight: f32) -> f64 {
    ((score - TYPICAL) * weight) as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::Section;

    fn book(paras: Vec<String>) -> Book {
        Book {
            title: None,
            format: "TXT",
            sections: vec![
                Section {
                    label: "cover".into(),
                    href: None,
                    kind: SectionKind::Body,
                    paragraphs: vec!["封面".into()],
                },
                Section {
                    label: "1".into(),
                    href: None,
                    kind: SectionKind::Body,
                    paragraphs: paras,
                },
            ],
            notes: vec![],
            decode_errors: 0,
            cover: None,
        }
    }

    #[test]
    fn samples_at_positions() {
        // 100 paragraphs of 100 characters, each a different character.
        let paras: Vec<String> = (0..100)
            .map(|i| char::from_u32(0x4e00 + i).unwrap().to_string().repeat(100))
            .collect();
        let s = samples(&book(paras)).unwrap();
        assert_eq!(s.len(), 4);
        for (sample, pos) in s.iter().zip(POSITIONS) {
            assert_eq!(han(sample), SAMPLE_HAN);
            // Starts SAMPLE_HAN / 2 before the position (cover skipped).
            let first = sample.chars().next().unwrap() as u32 - 0x4e00;
            assert_eq!(first, ((10_000.0 * pos - 1500.0) / 100.0).ceil() as u32);
        }
    }

    #[test]
    fn huge_paragraphs_are_cut() {
        // Chapters as single paragraphs of 20,000 characters.
        let paras: Vec<String> = (0..5).map(|_| "长".repeat(20_000)).collect();
        for s in samples(&book(paras)).unwrap() {
            assert_eq!(han(&s), SAMPLE_HAN);
        }
        assert_eq!(han_prefix("一a二b三", 2), "一a二b");
        assert_eq!(han_prefix("一二", 5), "一二");
    }

    #[test]
    fn short_books_are_not_rated() {
        assert!(samples(&book(vec!["短".repeat(1500)])).is_none());
    }

    #[test]
    fn log_helpers() {
        assert_eq!(utc(0), "1970-01-01 00:00:00");
        assert_eq!(utc(951_868_800), "2000-03-01 00:00:00");
        assert_eq!(utc(1_700_000_000), "2023-11-14 22:13:20");
        assert_eq!(tail_chars("一二三四", 2), "三四");
        assert_eq!(tail_chars("ab", 5), "ab");
    }

    #[test]
    fn digit_expectation() {
        let v = serde_json::json!({"choices": [{"logprobs": {"content": [{"top_logprobs": [
            {"token": "5", "logprob": (0.6f64).ln()},
            {"token": "6", "logprob": (0.3f64).ln()},
            {"token": "x", "logprob": (0.1f64).ln()},
        ]}]}}]});
        let e = expected_digit(&v).unwrap();
        assert!((e - (5.0 * 0.6 + 6.0 * 0.3) / 0.9).abs() < 1e-5);
        let none = serde_json::json!({"choices": [{"logprobs": {"content": [{"top_logprobs": [
            {"token": "A", "logprob": 0.0}]}]}, "message": {"content": "A"}}]});
        assert!(expected_digit(&none).is_err());
    }
}
