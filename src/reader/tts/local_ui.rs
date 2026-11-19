//! The local voice in the Read aloud panel (download, voice, instructions,
//! preview, advanced settings) and the "Generate locally" window (the same
//! settings plus the chapters to make).

use super::audio::{self, Output};
use super::local::{self, LocalSettings, SPEAKERS};
use super::{Source, Tts, TtsSettings, group, sentence_at, sentences};
use eframe::egui::{self, Color32, RichText};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;

const RED: Color32 = Color32::from_rgb(0xdc, 0x32, 0x2f);

/// The preview clip: being made, playing, or what went wrong.
#[derive(Default)]
enum Preview {
    #[default]
    None,
    Making(Receiver<Result<Vec<i16>, String>>),
    Playing(Output),
    Failed(String),
}

/// A download under way: bytes done and of all, and its end.
type Download = (Arc<AtomicU64>, Arc<AtomicU64>, Receiver<Result<(), String>>);

#[derive(Default)]
pub struct State {
    /// Both downloads present (checked when the panel or window opens and
    /// after a download, not every frame).
    present: Option<bool>,
    /// A download under way.
    downloading: Option<Download>,
    download_error: Option<String>,
    preview: Preview,
    /// The "Generate locally" window.
    pub window_open: bool,
    /// Chapters ticked in the window, by spine index.
    chosen: Vec<bool>,
    /// Each chapter's sentences and characters, once built.
    sizes: Vec<Option<(usize, usize)>>,
    /// Generating switched reading aloud to the local voice.
    switched: bool,
}

impl State {
    pub fn downloaded(&self) -> bool {
        self.present.unwrap_or(false)
    }
}

impl Tts {
    /// Once a frame: the download's and the preview's answers.
    pub(super) fn local_poll(&mut self) {
        if self.local.present.is_none() {
            self.local.present = Some(local::downloaded());
        }
        if let Some((_, _, rx)) = &self.local.downloading
            && let Ok(r) = rx.try_recv()
        {
            self.local.downloading = None;
            self.local.present = Some(local::downloaded());
            if let Err(e) = r {
                self.local.download_error = Some(e);
            } else {
                self.key_note = None;
            }
        }
        match &self.local.preview {
            Preview::Making(rx) => {
                if let Ok(r) = rx.try_recv() {
                    self.local.preview = match r.and_then(|pcm| play(&pcm, &self.repaint)) {
                        Ok(o) => Preview::Playing(o),
                        Err(e) => Preview::Failed(e),
                    };
                }
            }
            Preview::Playing(o) if o.queued().is_empty() || o.failed() => {
                self.local.preview = Preview::None;
            }
            _ => {}
        }
    }

    pub(super) fn stop_preview(&mut self) {
        if let Preview::Playing(o) = &self.local.preview {
            o.clear();
        }
        self.local.preview = Preview::None;
    }

    fn start_download(&mut self) {
        let done = Arc::new(AtomicU64::new(0));
        let total = Arc::new(AtomicU64::new(local::download_size()));
        let (d, t) = (done.clone(), total.clone());
        let repaint = self.repaint.clone();
        let rx = local::spawn("tts-download", self.repaint.clone(), move || {
            local::download(&|a, b| {
                d.store(a, Ordering::Relaxed);
                t.store(b, Ordering::Relaxed);
                repaint();
            })
        });
        self.local.download_error = None;
        self.local.downloading = Some((done, total, rx));
    }

    /// A short passage from the page shown, read with the current settings:
    /// whole sentences up to about 120 characters, paragraphs on their own
    /// lines, as chapters are read.
    fn start_preview(&mut self, s: &LocalSettings, spine: usize) {
        if self.active() {
            self.stop();
        }
        self.stop_preview();
        let ch = self.chapters.lock().unwrap().get(spine).cloned().flatten();
        let Some(ch) = ch else {
            self.local.preview = Preview::Failed("This chapter is still loading.".into());
            return;
        };
        let sents = sentences(&ch.chars);
        let paras = local::paragraphs(&ch.chars, &sents);
        let first = sentence_at(&sents, self.page_src).min(sents.len().saturating_sub(1));
        let mut text = String::new();
        let mut n = 0;
        for i in first..sents.len() {
            let r = &sents[i];
            if n > 0 && n + r.len() > 120 {
                break;
            }
            if n > 0 && paras[i] != paras[i - 1] {
                text.push('\n');
            }
            text.extend(&ch.chars[r.start as usize..r.end as usize]);
            n += r.len();
        }
        if text.is_empty() {
            text = "这是一段试听，用来听一听现在的设置读起来是什么样子。".into();
        }
        let s = s.clone();
        self.local.preview = Preview::Making(local::spawn(
            "tts-preview",
            self.repaint.clone(),
            move || local::preview(&text, &s),
        ));
    }

    /// The download row: a button that greys out once pressed, its
    /// progress, and whether the model is loaded on the GPU.
    fn download_row(&mut self, ui: &mut egui::Ui) {
        if hanstat::models::QWEN_TTS_SERVER.is_none() {
            ui.colored_label(RED, "The local voice isn't available on this platform.");
            return;
        }
        let size = local::download_size() as f64 / 1e9;
        ui.horizontal(|ui| {
            let downloading = self.local.downloading.is_some();
            let present = self.local.downloaded();
            let label = if present {
                "Downloaded".to_owned()
            } else if downloading {
                "Downloading…".to_owned()
            } else {
                format!("Download ({size:.1} GB)")
            };
            if ui
                .add_enabled(!present && !downloading, egui::Button::new(label))
                .on_hover_text(
                    "Qwen3-TTS 1.7B CustomVoice (Alibaba Qwen, Apache-2.0) and \
                     qwentts.cpp's server (MIT) that runs it on the GPU",
                )
                .clicked()
            {
                self.start_download();
            }
            if let Some((done, total, _)) = &self.local.downloading {
                let (d, t) = (
                    done.load(Ordering::Relaxed),
                    total.load(Ordering::Relaxed).max(1),
                );
                ui.add(
                    egui::ProgressBar::new(d as f32 / t as f32)
                        .desired_width(180.0)
                        .text(format!("{:.2} of {:.1} GB", d as f64 / 1e9, t as f64 / 1e9)),
                );
            } else if present {
                let loaded = local::status() == local::Status::Loaded;
                ui.label(
                    RichText::new(if loaded {
                        "loaded on the GPU"
                    } else {
                        "loads on first use"
                    })
                    .weak(),
                );
                let busy = self.batch.as_ref().is_some_and(|b| !b.finished())
                    || matches!(self.local.preview, Preview::Making(_))
                    || self.active();
                if loaded
                    && ui
                        .add_enabled(!busy, egui::Button::new("Unload").small())
                        .on_hover_text("Free the GPU memory it holds (about 4–5 GB)")
                        .clicked()
                {
                    local::unload();
                }
            }
        });
        if let Some(e) = &self.local.download_error {
            ui.colored_label(RED, e);
        }
    }

    /// Voice, instructions, preview and the advanced settings: in the
    /// panel and in the window.
    fn local_controls(&mut self, ui: &mut egui::Ui, s: &mut TtsSettings, spine: usize, wide: bool) {
        let l = &mut s.local;
        if wide {
            // Nine voices, three a row, each with how it sounds below it.
            ui.label(RichText::new("Voice").strong());
            egui::Grid::new("reader-tts-local-voices")
                .num_columns(3)
                .spacing([18.0, 8.0])
                .show(ui, |ui| {
                    for (i, sp) in SPEAKERS.iter().enumerate() {
                        ui.vertical(|ui| {
                            ui.set_width(170.0);
                            ui.radio_value(&mut l.speaker, sp.id.to_owned(), sp.name);
                            ui.label(RichText::new(sp.sound).size(12.0).weak());
                            ui.label(RichText::new(sp.native).size(12.0).weak());
                        });
                        if i % 3 == 2 {
                            ui.end_row();
                        }
                    }
                });
            ui.add_space(4.0);
        }
        egui::Grid::new(("reader-tts-local", wide))
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                if !wide {
                    ui.label("Voice");
                    let cur = local::speaker(&l.speaker);
                    egui::ComboBox::from_id_salt("reader-tts-local-voice")
                        .selected_text(format!("{} — {}", cur.name, cur.sound))
                        .width(260.0)
                        .show_ui(ui, |ui| {
                            for sp in SPEAKERS {
                                ui.selectable_value(
                                    &mut l.speaker,
                                    sp.id.to_owned(),
                                    format!("{} — {} · {}", sp.name, sp.sound, sp.native),
                                );
                            }
                        });
                    ui.end_row();
                }
                ui.label("Instructions");
                ui.add(
                    egui::TextEdit::multiline(&mut l.instruct)
                        .hint_text("Optional, e.g. 用温柔平稳的语气，语速稍慢")
                        .desired_rows(2)
                        .desired_width(if wide { 320.0 } else { 260.0 }),
                )
                .on_hover_text(
                    "How to read: tone, pace, emotion, in any language. Each wording \
                     is its own audio: changing it makes sentences again.",
                );
                ui.end_row();
            });
        self.preview_row(ui, s, spine);
        egui::CollapsingHeader::new("Advanced")
            .id_salt(("reader-tts-local-adv", wide))
            .show(ui, |ui| advanced(ui, &mut s.local, self.batch_running()));
    }

    fn batch_running(&self) -> bool {
        self.batch
            .as_ref()
            .is_some_and(|b| !b.finished() && !b.cancel.load(Ordering::Relaxed))
    }

    fn preview_row(&mut self, ui: &mut egui::Ui, s: &TtsSettings, spine: usize) {
        ui.horizontal(|ui| {
            let ready = self.local.downloaded();
            match &self.local.preview {
                Preview::Making(_) => {
                    ui.add_enabled(false, egui::Button::new("Preview"));
                    ui.spinner();
                    ui.label(
                        RichText::new(if local::status() == local::Status::Loaded {
                            "Making the preview…"
                        } else {
                            "Loading the voice, then the preview…"
                        })
                        .weak(),
                    );
                }
                Preview::Playing(_) => {
                    if ui.button("Stop preview").clicked() {
                        self.stop_preview();
                    }
                    ui.label(RichText::new("Playing").weak());
                }
                _ => {
                    if ui
                        .add_enabled(ready, egui::Button::new("Preview"))
                        .on_hover_text(
                            "Hear a few sentences from this page read with these \
                             settings (not saved)",
                        )
                        .clicked()
                    {
                        self.start_preview(&s.local, spine);
                    }
                    if let Preview::Failed(e) = &self.local.preview {
                        ui.colored_label(RED, e);
                    }
                }
            }
        });
    }

    /// The local voice in the Read aloud panel.
    pub(super) fn local_settings(&mut self, ui: &mut egui::Ui, s: &mut TtsSettings) {
        if self.local.present.is_none() {
            self.local.present = Some(local::downloaded());
        }
        self.download_row(ui);
        if let Some((ok, note)) = &self.key_note
            && note == local::NOT_DOWNLOADED
        {
            let c = if *ok {
                ui.visuals().weak_text_color()
            } else {
                RED
            };
            ui.colored_label(c, note);
        }
        let spine = self.spine_shown();
        self.local_controls(ui, s, spine, false);
        ui.label(
            RichText::new(
                "Runs on your GPU. Changing the voice, instructions or an advanced \
                 setting makes new audio; what was made before stays.",
            )
            .size(12.0)
            .weak(),
        );
    }

    /// The chapter the reader shows (set with the page).
    fn spine_shown(&self) -> usize {
        self.local_spine
    }

    pub fn open_local_window(&mut self, spine: usize) {
        let n = self.chapters.lock().unwrap().len();
        if self.local.chosen.len() != n {
            self.local.chosen = vec![false; n];
            self.local.sizes = vec![None; n];
        }
        if !self.local.chosen.iter().any(|&c| c) && spine < n {
            self.local.chosen[spine] = true;
        }
        self.local.present = Some(local::downloaded());
        self.local.window_open = true;
    }

    /// Sentences and characters of each built chapter (counted once).
    fn count_chapters(&mut self) {
        let chapters = self.chapters.lock().unwrap().clone();
        for (i, c) in chapters.iter().enumerate() {
            if self.local.sizes[i].is_none()
                && let Some(c) = c
            {
                let s = sentences(&c.chars);
                let chars = s.iter().map(|r| r.len()).sum();
                self.local.sizes[i] = Some((s.len(), chars));
            }
        }
    }

    /// The "Generate locally" window.
    pub(super) fn local_window(&mut self, ctx: &egui::Context, s: &mut TtsSettings, spine: usize) {
        self.local_spine = spine;
        if !self.local.window_open {
            return;
        }
        self.count_chapters();
        let mut open = true;
        egui::Window::new("Generate locally")
            .id(egui::Id::unique("reader-tts-local-window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(560.0)
            .default_height(ctx.content_rect().height() * 0.8)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.label(
                        RichText::new(
                            "Make chapters' audio with Qwen3-TTS on this computer's GPU. \
                             Reading aloud then plays it without waiting.",
                        )
                        .weak(),
                    );
                    self.download_row(ui);
                    ui.separator();
                    self.local_controls(ui, s, spine, true);
                    ui.separator();
                    self.chapter_list(ui, spine);
                    ui.separator();
                    self.generate_row(ui, s);
                });
            });
        if !open {
            self.local.window_open = false;
        }
    }

    fn chapter_list(&mut self, ui: &mut egui::Ui, spine: usize) {
        let n = self.local.chosen.len();
        let readable: Vec<usize> = (0..n)
            .filter(|&i| self.local.sizes[i].is_none_or(|(k, _)| k > 0))
            .collect();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Chapters").strong());
            let set = |st: &mut State, f: &dyn Fn(usize) -> bool| {
                for (i, c) in st.chosen.iter_mut().enumerate() {
                    *c = f(i);
                }
            };
            if ui.small_button("All").clicked() {
                set(&mut self.local, &|_| true);
            }
            if ui.small_button("None").clicked() {
                set(&mut self.local, &|_| false);
            }
            if ui.small_button("This one").clicked() {
                set(&mut self.local, &|i| i == spine);
            }
            if ui.small_button("From this one on").clicked() {
                set(&mut self.local, &|i| i >= spine);
            }
        });
        egui::ScrollArea::vertical()
            .id_salt("reader-tts-local-chapters")
            .max_height(260.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for &i in &readable {
                    let label = match self.labels.get(i).filter(|l| !l.is_empty()) {
                        Some(l) => l.clone(),
                        None => format!("Part {}", i + 1),
                    };
                    ui.horizontal(|ui| {
                        let text = if i == spine {
                            RichText::new(label).strong()
                        } else {
                            RichText::new(label)
                        };
                        ui.checkbox(&mut self.local.chosen[i], text);
                        let size = match self.local.sizes[i] {
                            Some((k, chars)) => format!(
                                "{} sentences, ~{}",
                                group(k),
                                duration(local::seconds(chars))
                            ),
                            None => "counting…".to_owned(),
                        };
                        ui.label(RichText::new(size).size(12.0).weak());
                    });
                }
            });
    }

    fn generate_row(&mut self, ui: &mut egui::Ui, s: &mut TtsSettings) {
        let chosen: Vec<usize> = (0..self.local.chosen.len())
            .filter(|&i| self.local.chosen[i] && self.local.sizes[i].is_none_or(|(k, _)| k > 0))
            .collect();
        let known: Vec<(usize, usize)> =
            chosen.iter().filter_map(|&i| self.local.sizes[i]).collect();
        let all_known = known.len() == chosen.len();
        let (sents, chars) = known.iter().fold((0, 0), |(a, b), (k, c)| (a + k, b + c));
        let audio = local::seconds(chars);
        // Measured on an RTX 5070 (Vulkan): 6.5× real time for one request,
        // 17.9× for four at once.
        let speed = 6.5 + (17.9 - 6.5) * (s.local.parallel.clamp(1, 4) - 1) as f64 / 3.0;
        ui.label(if chosen.is_empty() {
            "No chapter chosen.".to_owned()
        } else {
            format!(
                "{} chapter{}, {}{} sentences: about {} of audio, made in about {} \
                 on an RTX 5070 (sentences already made are skipped).",
                chosen.len(),
                if chosen.len() == 1 { "" } else { "s" },
                group(sents),
                if all_known { "" } else { "+" },
                duration(audio),
                duration(audio / speed),
            )
        });
        let running = self.batch_running();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.local.downloaded() && !chosen.is_empty() && !running,
                    egui::Button::new(RichText::new("Generate").strong()),
                )
                .clicked()
            {
                if s.source != Source::Local {
                    s.source = Source::Local;
                    self.local.switched = true;
                }
                let what = if chosen.len() == 1 {
                    "1 chapter".to_owned()
                } else {
                    format!("{} chapters", chosen.len())
                };
                // The engine must have the local settings before it lists.
                self.engine(s);
                self.poll(s);
                self.pregenerate(s, chosen.clone(), &what);
            }
            if !self.local.downloaded() {
                ui.label(RichText::new("Download the voice first.").weak());
            }
        });
        self.batch_progress(ui);
        if self.local.switched {
            ui.label(
                RichText::new(
                    "Reading aloud now uses this voice (switch back under Read aloud → Read by).",
                )
                .size(12.0)
                .weak(),
            );
        }
    }

    pub fn local_window_open(&self) -> bool {
        self.local.window_open
    }

    pub fn close_local_window(&mut self) {
        self.local.window_open = false;
    }
}

/// The settings few will change: sampling, seed, chunk size, parallel requests.
fn advanced(ui: &mut egui::Ui, l: &mut LocalSettings, running: bool) {
    egui::Grid::new("reader-tts-local-advanced")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            let f = |ui: &mut egui::Ui, v: &mut f32, r: std::ops::RangeInclusive<f32>, step: f64, tip: &str| {
                ui.add(egui::Slider::new(v, r).step_by(step)).on_hover_text(tip);
            };
            ui.label("Temperature");
            f(ui, &mut l.temperature, 0.0..=1.5, 0.05,
              "Randomness of what is said and how: lower is steadier and flatter, higher livelier \
               and less predictable; 0 always takes the likeliest (0.9 default)");
            ui.end_row();
            ui.label("Top-p");
            f(ui, &mut l.top_p, 0.05..=1.0, 0.05,
              "Sample only from the likeliest choices making up this share (1 = all)");
            ui.end_row();
            ui.label("Top-k");
            ui.add(egui::Slider::new(&mut l.top_k, 0..=200))
                .on_hover_text("Sample only from this many likeliest choices (0 = no limit; 50 default)");
            ui.end_row();
            ui.label("Repetition penalty");
            f(ui, &mut l.repetition_penalty, 1.0..=1.5, 0.01,
              "Discourages repeating sounds: too high can garble (1.05 default)");
            ui.end_row();
            ui.label("Detail temperature");
            f(ui, &mut l.sub_temperature, 0.0..=1.5, 0.05,
              "Randomness of the fine acoustic detail (timbre, breath) (0.9 default)");
            ui.end_row();
            ui.label("Detail top-p");
            f(ui, &mut l.sub_top_p, 0.05..=1.0, 0.05, "Top-p for the fine detail (1 default)");
            ui.end_row();
            ui.label("Detail top-k");
            ui.add(egui::Slider::new(&mut l.sub_top_k, 0..=200))
                .on_hover_text("Top-k for the fine detail (50 default)");
            ui.end_row();
            ui.label("Seed");
            ui.add(egui::DragValue::new(&mut l.seed))
                .on_hover_text("Another number gives another take of the same text and settings");
            ui.end_row();
            ui.label("Chunk length");
            ui.add(egui::Slider::new(&mut l.chunk_chars, 40..=400).step_by(10.0).suffix(" chars"))
                .on_hover_text(
                    "Whole paragraphs are read together up to this many characters: longer \
                     keeps the voice steadier across sentences (200 default)",
                );
            ui.end_row();
            ui.label("At once");
            ui.add_enabled(!running, egui::Slider::new(&mut l.parallel, 1..=4).suffix(" requests"))
                .on_hover_text(
                    "Requests the GPU works on together: 4 is fastest but holds about 5 GB \
                     of GPU memory, 1 about 3.7 GB. Doesn't change the audio.",
                );
            ui.end_row();
        });
    if ui.small_button("Reset to defaults").clicked() {
        let keep = (l.speaker.clone(), l.instruct.clone());
        *l = LocalSettings::default();
        (l.speaker, l.instruct) = keep;
    }
}

/// `pcm` (the clip rate) played on the default device.
fn play(pcm: &[i16], repaint: &super::engine::Repaint) -> Result<Output, String> {
    let r = repaint.clone();
    let out = Output::open(Arc::new(move || r()))?;
    let f: Vec<f32> = pcm.iter().map(|&x| x as f32 / 32768.0).collect();
    out.push(1, audio::resample(&f, super::opus::RATE, out.rate));
    out.set_paused(false);
    Ok(out)
}

/// "2 h 05 min", "12 min", "40 s".
fn duration(secs: f64) -> String {
    let s = secs.round() as u64;
    if s >= 3600 {
        format!("{} h {:02} min", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{} min", (s + 30) / 60)
    } else {
        format!("{s} s")
    }
}
