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

mod designer;

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
type Transfer = (Arc<AtomicU64>, Arc<AtomicU64>, Receiver<Result<(), String>>);

#[derive(Default)]
struct Download {
    /// Files present (checked when the panel or window opens and
    /// after a download, not every frame).
    present: Option<bool>,
    transfer: Option<Transfer>,
    error: Option<String>,
}

impl Download {
    fn poll(&mut self, present: fn() -> bool) -> bool {
        if self.present.is_none() {
            self.present = Some(present());
        }
        if let Some((_, _, rx)) = &self.transfer
            && let Ok(result) = rx.try_recv()
        {
            self.transfer = None;
            self.present = Some(present());
            self.error = result.err();
            return true;
        }
        false
    }

    fn button(&mut self, ui: &mut egui::Ui, size: u64, tip: &str) -> bool {
        let present = self.present == Some(true);
        let label = if present {
            "Downloaded".to_owned()
        } else if self.transfer.is_some() {
            "Downloading…".to_owned()
        } else {
            format!("Download ({:.1} GB)", size as f64 / 1e9)
        };
        let clicked = ui
            .add_enabled(
                !present && self.transfer.is_none(),
                egui::Button::new(label),
            )
            .on_hover_text(tip)
            .clicked();
        if let Some((done, total, _)) = &self.transfer {
            let (d, t) = (
                done.load(Ordering::Relaxed),
                total.load(Ordering::Relaxed).max(1),
            );
            ui.add(
                egui::ProgressBar::new(d as f32 / t as f32)
                    .desired_width(180.0)
                    .text(format!("{:.2} of {:.1} GB", d as f64 / 1e9, t as f64 / 1e9)),
            );
        }
        clicked
    }

    fn start(
        &mut self,
        size: u64,
        repaint: super::engine::Repaint,
        download: impl FnOnce(&dyn Fn(u64, u64)) -> Result<(), String> + Send + 'static,
    ) {
        let done = Arc::new(AtomicU64::new(0));
        let total = Arc::new(AtomicU64::new(size));
        let (d, t) = (done.clone(), total.clone());
        let rx = local::spawn("tts-download", repaint.clone(), move || {
            download(&|a, b| {
                d.store(a, Ordering::Relaxed);
                t.store(b, Ordering::Relaxed);
                repaint();
            })
        });
        self.error = None;
        self.transfer = Some((done, total, rx));
    }
}

#[derive(Default)]
pub struct State {
    custom: Download,
    design: Download,
    designer: designer::State,
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
    pub fn downloaded(&self, s: &LocalSettings) -> bool {
        let download = if s.designed.is_some() {
            &self.design
        } else {
            &self.custom
        };
        download.present.unwrap_or(false)
    }
}

impl Tts {
    /// Once a frame: the download's and the preview's answers.
    pub(super) fn local_poll(&mut self) {
        let custom = self.local.custom.poll(local::downloaded);
        let design = self.local.design.poll(local::design_downloaded);
        if custom || design {
            self.local.custom.present = Some(local::downloaded());
            self.local.design.present = Some(local::design_downloaded());
            if [local::NOT_DOWNLOADED, local::DESIGN_NOT_DOWNLOADED]
                .iter()
                .any(|note| self.key_note.as_ref().is_some_and(|(_, text)| text == note))
            {
                self.key_note = None;
            }
        }
        self.local.designer.poll();
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
        self.local.designer.stop_sample();
    }

    /// Whole sentences from the page shown, with paragraphs on their own
    /// lines, as chapters are read. A long first sentence is kept whole.
    fn page_passage(&self, spine: usize, limit: usize, end: u32) -> Option<String> {
        let ch = self
            .chapters
            .lock()
            .unwrap()
            .get(spine)
            .cloned()
            .flatten()?;
        let sents = sentences(&ch.chars);
        let paras = local::paragraphs(&ch.chars, &sents);
        let first = sentence_at(&sents, self.page_src).min(sents.len().saturating_sub(1));
        let mut text = String::new();
        let mut n = 0;
        for i in first..sents.len() {
            let r = &sents[i];
            if r.start >= end || (n > 0 && n + r.len() > limit) {
                break;
            }
            if n > 0 && paras[i] != paras[i - 1] {
                text.push('\n');
            }
            text.extend(&ch.chars[r.start as usize..r.end as usize]);
            n += r.len();
        }
        Some(text)
    }

    fn start_preview(&mut self, s: &LocalSettings, spine: usize) {
        if self.active() {
            self.stop();
        }
        self.stop_preview();
        let Some(mut text) = self.page_passage(spine, 120, u32::MAX) else {
            self.local.preview = Preview::Failed("This chapter is still loading.".into());
            return;
        };
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
    fn download_row(&mut self, ui: &mut egui::Ui, s: &LocalSettings) {
        if hanstat::models::QWEN_TTS_SERVER.is_none() {
            ui.colored_label(RED, "The local voice isn't available on this platform.");
            return;
        }
        ui.horizontal(|ui| {
            let present = self.local.downloaded(s);
            if s.designed.is_some() {
                if !present {
                    ui.label(local::DESIGN_NOT_DOWNLOADED);
                }
            } else if self.local.custom.button(
                ui,
                local::download_size(),
                "Qwen3-TTS 1.7B CustomVoice (Alibaba Qwen, Apache-2.0) and \
                 qwentts.cpp's server (MIT) that runs it on the GPU",
            ) {
                self.local.custom.start(
                    local::download_size(),
                    self.repaint.clone(),
                    local::download,
                );
            }
            if present {
                ui.label(
                    RichText::new(if local::loaded(s.mode(), s.parallel) {
                        "loaded on the GPU"
                    } else {
                        "loads when needed"
                    })
                    .weak(),
                );
                if local::status() == local::Status::Loaded
                    && ui
                        .add_enabled(
                            !self.local_voice_busy() && !self.active(),
                            egui::Button::new("Unload").small(),
                        )
                        .on_hover_text("Free the GPU memory it holds (about 4–5 GB)")
                        .clicked()
                {
                    local::unload();
                }
            }
        });
        if s.designed.is_none()
            && let Some(e) = &self.local.custom.error
        {
            ui.colored_label(RED, e);
        }
    }

    fn design_download_row(&mut self, ui: &mut egui::Ui) {
        if hanstat::models::QWEN_TTS_SERVER.is_none() {
            return;
        }
        let size = local::design_download_size();
        ui.horizontal(|ui| {
            ui.label("Voice models");
            let design = &mut self.local.design;
            if design.button(
                ui,
                size,
                "Qwen3-TTS VoiceDesign makes a sample. Qwen3-TTS Base uses it to read books.",
            ) {
                design.start(size, self.repaint.clone(), local::download_design);
            }
        });
        if let Some(e) = &self.local.design.error {
            ui.colored_label(RED, e);
        }
    }

    /// Voice, instructions, preview and the advanced settings: in the
    /// panel and in the window.
    fn local_controls(&mut self, ui: &mut egui::Ui, s: &mut TtsSettings, spine: usize, wide: bool) {
        self.local.designer.load_voices();
        let busy = self.local_voice_busy();
        let mut design = false;
        let l = &mut s.local;
        let voices = &self.local.designer.voices;
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
                            if ui
                                .add_enabled(
                                    !busy,
                                    egui::RadioButton::new(
                                        l.designed.is_none() && l.speaker == sp.id,
                                        sp.name,
                                    ),
                                )
                                .clicked()
                            {
                                l.speaker = sp.id.into();
                                l.designed = None;
                            }
                            ui.label(RichText::new(sp.sound).size(12.0).weak());
                            ui.label(RichText::new(sp.native).size(12.0).weak());
                        });
                        if i % 3 == 2 {
                            ui.end_row();
                        }
                    }
                });
            ui.add_enabled_ui(!busy, |ui| {
                if !voices.is_empty() {
                    ui.label(RichText::new("Designed voices").strong());
                    ui.horizontal_wrapped(|ui| {
                        for v in voices {
                            if ui
                                .radio(l.designed.as_ref() == Some(&v.id), &v.info.name)
                                .on_hover_text(&v.info.description)
                                .clicked()
                            {
                                l.designed = Some(v.id.clone());
                            }
                        }
                    });
                }
                design = ui.button("Design a voice…").clicked();
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
                    let selected = match &l.designed {
                        Some(id) => voices.iter().find(|v| &v.id == id)
                            .map_or("Missing designed voice", |v| v.info.name.as_str()).to_owned(),
                        None => format!("{} — {}", cur.name, cur.sound),
                    };
                    ui.add_enabled_ui(!busy, |ui| egui::ComboBox::from_id_salt("reader-tts-local-voice")
                        .selected_text(selected)
                        .width(260.0)
                        .show_ui(ui, |ui| {
                            for sp in SPEAKERS {
                                if ui.selectable_label(l.designed.is_none() && l.speaker == sp.id,
                                    format!("{} — {} · {}", sp.name, sp.sound, sp.native)).clicked() {
                                    l.speaker = sp.id.into();
                                    l.designed = None;
                                }
                            }
                            ui.separator();
                            if !voices.is_empty() {
                                ui.label(RichText::new("Designed voices").strong());
                                for v in voices {
                                    if ui.selectable_label(l.designed.as_ref() == Some(&v.id), &v.info.name)
                                        .on_hover_text(&v.info.description).clicked() {
                                        l.designed = Some(v.id.clone());
                                    }
                                }
                            }
                        }));
                    ui.end_row();
                }
                let instructions = l.designed.is_none();
                let tip = if instructions {
                    "How to read, including tone, pace and emotion, in any language. Changing the wording makes new audio."
                } else {
                    designer::SAMPLE_STYLE
                };
                ui.label("Instructions").on_hover_text(tip);
                ui.add_enabled(
                    instructions,
                    egui::TextEdit::multiline(&mut l.instruct)
                        .hint_text("Optional, e.g. 用温柔平稳的语气，语速稍慢")
                        .desired_rows(2)
                        .desired_width(if wide { 320.0 } else { 260.0 }),
                )
                .on_disabled_hover_text(tip)
                .on_hover_text(tip);
                ui.end_row();
            });
        if design {
            self.open_design_window();
        }
        self.preview_row(ui, s, spine);
        egui::CollapsingHeader::new("Advanced")
            .id_salt(("reader-tts-local-adv", wide))
            .show(ui, |ui| advanced(ui, &mut s.local, busy));
    }

    fn batch_running(&self) -> bool {
        self.batch
            .as_ref()
            .is_some_and(|b| !b.finished() && !b.cancel.load(Ordering::Relaxed))
    }

    /// Keep the running batch's talker and parallel count until it finishes.
    pub(super) fn local_voice_busy(&self) -> bool {
        self.batch_running()
            || self.local.designer.making()
            || matches!(self.local.preview, Preview::Making(_))
    }

    fn preview_row(&mut self, ui: &mut egui::Ui, s: &TtsSettings, spine: usize) {
        ui.horizontal(|ui| {
            let ready = self.local.downloaded(&s.local) && !self.local_voice_busy();
            match &self.local.preview {
                Preview::Making(_) => {
                    ui.add_enabled(false, egui::Button::new("Preview"));
                    ui.spinner();
                    ui.label(
                        RichText::new(if local::loaded(s.local.mode(), s.local.parallel) {
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
        self.local
            .custom
            .present
            .get_or_insert_with(local::downloaded);
        self.local
            .design
            .present
            .get_or_insert_with(local::design_downloaded);
        self.download_row(ui, &s.local);
        if let Some((ok, note)) = &self.key_note
            && (note == local::NOT_DOWNLOADED || note == local::DESIGN_NOT_DOWNLOADED)
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
        self.local.custom.present = Some(local::downloaded());
        self.local.design.present = Some(local::design_downloaded());
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
                    self.download_row(ui, &s.local);
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
        let ready = self.local.downloaded(&s.local);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    ready && !chosen.is_empty() && !self.local_voice_busy(),
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
            if !ready {
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
    ui.label(
        RichText::new(
            "Hover a setting to see what it changes. Use Preview to hear it before \
             making a chapter.",
        )
        .size(12.0)
        .weak(),
    );
    egui::Grid::new("reader-tts-local-advanced")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            // The explanation shows on the name and on the control.
            let row = |ui: &mut egui::Ui,
                       name: &str,
                       tip: &str,
                       add: &mut dyn FnMut(&mut egui::Ui) -> egui::Response| {
                ui.label(name).on_hover_text(tip);
                add(ui).on_hover_text(tip);
                ui.end_row();
            };
            row(ui, "Temperature", TIP_TEMPERATURE, &mut |ui| {
                ui.add(egui::Slider::new(&mut l.temperature, 0.0..=1.5).step_by(0.05))
            });
            row(ui, "Top-p", TIP_TOP_P, &mut |ui| {
                ui.add(egui::Slider::new(&mut l.top_p, 0.05..=1.0).step_by(0.05))
            });
            row(ui, "Top-k", TIP_TOP_K, &mut |ui| {
                ui.add(egui::Slider::new(&mut l.top_k, 0..=200))
            });
            row(ui, "Repetition penalty", TIP_REPETITION, &mut |ui| {
                ui.add(egui::Slider::new(&mut l.repetition_penalty, 1.0..=1.5).step_by(0.01))
            });
            row(ui, "Detail temperature", TIP_SUB_TEMPERATURE, &mut |ui| {
                ui.add(egui::Slider::new(&mut l.sub_temperature, 0.0..=1.5).step_by(0.05))
            });
            row(ui, "Detail top-p", TIP_SUB_TOP_P, &mut |ui| {
                ui.add(egui::Slider::new(&mut l.sub_top_p, 0.05..=1.0).step_by(0.05))
            });
            row(ui, "Detail top-k", TIP_SUB_TOP_K, &mut |ui| {
                ui.add(egui::Slider::new(&mut l.sub_top_k, 0..=200))
            });
            row(ui, "Seed", TIP_SEED, &mut |ui| {
                ui.add(egui::DragValue::new(&mut l.seed))
            });
            row(ui, "Chunk length", TIP_CHUNK, &mut |ui| {
                ui.add(
                    egui::Slider::new(&mut l.chunk_chars, 40..=400)
                        .step_by(10.0)
                        .suffix(" chars"),
                )
            });
            row(ui, "At once", TIP_PARALLEL, &mut |ui| {
                ui.add_enabled(
                    !running,
                    egui::Slider::new(&mut l.parallel, 1..=4).suffix(" requests"),
                )
            });
        });
    if ui
        .small_button("Reset to defaults")
        .on_hover_text("Every setting here back to its default; the voice and instructions stay")
        .clicked()
    {
        let keep = (l.speaker.clone(), l.instruct.clone(), l.designed.clone());
        let parallel = l.parallel;
        *l = LocalSettings::default();
        (l.speaker, l.instruct, l.designed) = keep;
        if running {
            l.parallel = parallel;
        }
    }
}

// What each advanced setting does, for someone listening to learn rather
// than tuning a model. Every one but "At once" makes new audio when changed.

const TIP_TEMPERATURE: &str = "How lively and varied the reading is.\n\n\
Lower (0.5–0.7): a calm, even narrator who reads every sentence much the same way. \
Easier to follow, and fewer slips such as a wrong tone or an odd stress.\n\
Higher (1.0+): more expressive and dramatic, but less predictable: sudden changes \
of mood, rushed or mumbled words, now and then a wrong reading.\n\n\
Default 0.9. If the voice sounds too excitable or stumbles, try 0.7.";

const TIP_TOP_P: &str = "Keeps the voice to its most natural-sounding choices.\n\n\
Lower (0.7–0.9) cuts unlikely choices, the source of most strange intonation and \
mispronounced words, at the cost of a slightly flatter delivery.\n\
1.0 sets no limit.\n\n\
Default 1.0. Try 0.8 if you hear odd readings that would teach you the wrong sound.";

const TIP_TOP_K: &str = "Another way of keeping the voice to safe choices: only the \
most likely few are ever considered: this many of them.\n\n\
Lower is safer and more uniform; 0 sets no limit. It rarely needs changing; \
temperature and top-p make a bigger difference.\n\nDefault 50.";

const TIP_REPETITION: &str = "Stops the voice getting stuck on a sound.\n\n\
Raise it a little (1.1) if you hear a syllable or word repeated, a stutter, or a \
long hum. Too high makes it skip or slur syllables, which is worse for learning.\n\n\
Default 1.05.";

const TIP_SUB_TEMPERATURE: &str = "How much the sound of the voice varies: its \
texture, breathiness and crispness, not the words or the intonation.\n\n\
Lower sounds cleaner and more uniform, a little synthetic.\n\
Higher sounds more natural and alive, but can add hiss, crackle or a rough edge.\n\n\
Default 0.9. Lower it if the voice sounds noisy or unclear.";

const TIP_SUB_TOP_P: &str = "Top-p for the sound of the voice: lower trims the rare \
choices that cause crackles and odd timbre.\n\nDefault 1.0 (no limit).";

const TIP_SUB_TOP_K: &str = "Top-k for the sound of the voice: lower is cleaner and \
more uniform; 0 sets no limit.\n\nDefault 50.";

const TIP_SEED: &str = "Which take you get. The voice is a little different every \
time it reads, and the seed picks one reading: the same text, settings and seed always \
sound the same.\n\n\
Change it to hear a different take of everything, for example if the reading came \
out with a tone you don't like.";

const TIP_CHUNK: &str = "How much text the voice reads in one go. Whole paragraphs \
are read together up to this many characters, then cut into sentences.\n\n\
Longer keeps the voice consistent from sentence to sentence, like a narrator \
reading on.\nShorter means each sentence is read more on its own, and changes \
of tone between them are more likely.\n\n\
Default 200. Reading from a new place starts within a few seconds either way.";

const TIP_PARALLEL: &str = "How many pieces of text the graphics card reads at once \
when making audio.\n\n\
4 is the fastest (about 18 times faster than listening on an RTX 5070) and holds \
about 5 GB of its memory; 1 is about a third as fast and holds about 3.7 GB. Lower it \
if other programs need the graphics card.\n\n\
It never changes how the audio sounds.";

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
