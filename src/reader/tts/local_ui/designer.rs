//! Make a reference to keep, and manage the references available to both
//! local voice pickers.

use super::super::engine::Repaint;
use super::super::{Tts, TtsSettings};
use super::{Playback, RED, local};
use eframe::egui::{self, RichText};
use local::{LocalSettings, SavedVoice, VoiceStore};
use std::fs;
use std::hash::{BuildHasher, RandomState};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

const EXAMPLE: &str = "三十岁左右的男声，低沉温和，像深夜电台主持人";
pub const SAMPLE_TEXT: &str = "天渐渐亮了，河面上还飘着一层薄雾。她沿着石板路慢慢走到桥边，停下脚步，听见远处传来几声清脆的鸟鸣。";
const DESCRIPTION_TIP: &str = "Describe the voice you want to hear. You can give its gender, age, \
    pitch, pace and mood, or suggest a setting such as a late-night radio show. \
    Use any language.";
pub(super) const SAMPLE_STYLE: &str = "A kept voice reads every book in the same voice. \
    Its tone and pace come from the sample. To change them, make a new sample.";

/// Keep the words that were actually sent even if the text box is edited
/// before the sample is named.
struct Take {
    description: String,
    text: String,
    seed: u64,
    sample: Arc<local::Sample>,
}

enum Edit {
    Rename { id: String, name: String },
    Delete { id: String, name: String },
    ReplaceExport { id: String, path: PathBuf },
}

enum Transfer {
    Imported(Option<String>),
    Exported(PathBuf),
}

enum VoiceOp {
    Saved { id: String, voices: Vec<SavedVoice> },
    Renamed(Vec<SavedVoice>),
    Deleted { id: String, voices: Vec<SavedVoice> },
}

enum PendingDesigned {
    Select(String),
    ClearIf(String),
}

fn voice_dialog(title: &str) -> rfd::FileDialog {
    rfd::FileDialog::new()
        .set_title(title)
        .add_filter("Hanstat voice", &["voice"])
}

fn export_filename(name: &str) -> String {
    let stem: String = name
        .chars()
        .take(60)
        .map(|ch| {
            if ch.is_control() || "<>:\"/\\|?*".contains(ch) {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let stem = stem.trim_matches([' ', '.']);
    if stem.is_empty() {
        "voice.voice".into()
    } else {
        // The prefix also rules out Windows device names such as CON and NUL.
        format!("voice-{stem}.voice")
    }
}

pub(super) struct State {
    open: bool,
    description: String,
    text: String,
    pending: Option<Receiver<Result<Take, String>>>,
    take: Option<Take>,
    keep_name: Option<String>,
    /// A playing take has no id, since it hasn't been kept yet.
    playing: Option<(Option<String>, Playback)>,
    listed: bool,
    listing: Option<Receiver<Result<Vec<SavedVoice>, String>>>,
    op: Option<Receiver<Result<VoiceOp, String>>>,
    pending_designed: Option<PendingDesigned>,
    pub voices: Vec<SavedVoice>,
    edit: Option<Edit>,
    error: Option<String>,
    transfer: Option<Receiver<Result<Transfer, String>>>,
    transfer_note: Option<Result<String, String>>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            description: String::new(),
            text: SAMPLE_TEXT.into(),
            pending: None,
            take: None,
            keep_name: None,
            playing: None,
            listed: false,
            listing: None,
            op: None,
            pending_designed: None,
            voices: Vec::new(),
            edit: None,
            error: None,
            transfer: None,
            transfer_note: None,
        }
    }
}

impl State {
    pub fn making(&self) -> bool {
        self.pending.is_some()
    }

    pub fn apply_pending_designed(&mut self, s: &mut LocalSettings) {
        match self.pending_designed.take() {
            Some(PendingDesigned::Select(id)) => s.designed = Some(id),
            Some(PendingDesigned::ClearIf(id)) if s.designed.as_ref() == Some(&id) => {
                s.designed = None;
            }
            Some(PendingDesigned::ClearIf(_)) | None => {}
        }
    }

    pub fn poll(&mut self, repaint: &Repaint) {
        if let Some(rx) = &self.pending {
            let answer = match rx.try_recv() {
                Ok(answer) => Some(answer),
                Err(TryRecvError::Disconnected) => Some(Err("The voice designer stopped.".into())),
                Err(TryRecvError::Empty) => None,
            };
            if let Some(answer) = answer {
                self.pending = None;
                match answer {
                    Ok(take) => self.take = Some(take),
                    Err(e) => self.error = Some(e),
                }
            }
        }
        if let Some(rx) = &self.listing {
            let answer = match rx.try_recv() {
                Ok(answer) => Some(answer),
                Err(TryRecvError::Disconnected) => Some(Err("The voice list stopped.".into())),
                Err(TryRecvError::Empty) => None,
            };
            if let Some(answer) = answer {
                self.listing = None;
                match answer {
                    Ok(voices) => self.voices = voices,
                    Err(e) => self.error = Some(e),
                }
            }
        }
        if let Some(rx) = &self.op {
            let answer = match rx.try_recv() {
                Ok(answer) => Some(answer),
                Err(TryRecvError::Disconnected) => Some(Err("The voice update stopped.".into())),
                Err(TryRecvError::Empty) => None,
            };
            if let Some(answer) = answer {
                self.op = None;
                match answer {
                    Ok(VoiceOp::Saved { id, voices }) => {
                        self.voices = voices;
                        self.keep_name = None;
                        self.error = None;
                        self.pending_designed = Some(PendingDesigned::Select(id));
                    }
                    Ok(VoiceOp::Renamed(voices)) => {
                        self.voices = voices;
                        self.error = None;
                    }
                    Ok(VoiceOp::Deleted { id, voices }) => {
                        self.voices = voices;
                        self.error = None;
                        self.pending_designed = Some(PendingDesigned::ClearIf(id));
                    }
                    Err(e) => self.error = Some(e),
                }
            }
        }
        if let Some(rx) = &self.transfer {
            let answer = match rx.try_recv() {
                Ok(answer) => Some(answer),
                Err(TryRecvError::Disconnected) => Some(Err("The voice transfer stopped.".into())),
                Err(TryRecvError::Empty) => None,
            };
            if let Some(answer) = answer {
                self.transfer = None;
                self.transfer_note = Some(answer.map(|result| match result {
                    Transfer::Imported(name) => {
                        self.refresh(repaint);
                        match name {
                            Some(name) => format!("Imported “{name}”."),
                            None => "Already imported.".into(),
                        }
                    }
                    Transfer::Exported(path) => {
                        format!("Exported “{}”.", crate::file_name(&path))
                    }
                }));
            }
        }
        if let Some((_, p)) = &self.playing
            && let Some(result) = p.poll()
        {
            self.playing = None;
            if let Err(e) = result {
                self.error = Some(e);
            }
        }
    }

    pub fn stop_sample(&mut self) {
        self.playing = None;
    }

    fn playing(&self, id: Option<&str>) -> bool {
        self.playing
            .as_ref()
            .is_some_and(|(from, _)| from.as_deref() == id)
    }

    pub fn load_voices(&mut self, repaint: &Repaint) {
        if !self.listed {
            self.refresh(repaint);
        }
    }

    fn refresh(&mut self, repaint: &Repaint) {
        self.listed = true;
        self.listing = Some(local::spawn("tts-voices-list", repaint.clone(), || {
            VoiceStore::default().list_voices()
        }));
    }
}

impl Tts {
    pub fn open_design_window(&mut self) {
        self.local.design.present = Some(local::design_downloaded());
        self.local.design_size = None;
        self.local.designer.refresh(&self.repaint);
        self.local.designer.open = true;
    }

    pub fn design_window_open(&self) -> bool {
        self.local.designer.open
    }

    pub fn close_design_window(&mut self) {
        self.local.designer.open = false;
        self.local.designer.edit = None;
        self.local.designer.stop_sample();
    }

    pub(in crate::reader::tts) fn design_making(&self) -> bool {
        self.local.designer.making()
    }

    fn start_sample(&mut self) {
        if self.active() {
            self.stop();
        }
        self.stop_preview();
        let d = &mut self.local.designer;
        let description = d.description.trim().to_owned();
        let text = d.text.trim().to_owned();
        // Fresh randomness for each press, without a new dependency.
        let seed = RandomState::new().hash_one(());
        d.error = None;
        d.keep_name = None;
        d.take = None;
        d.pending = Some(local::spawn(
            "tts-design",
            self.repaint.clone(),
            move || {
                let sample = Arc::new(local::design_sample(&description, &text, seed)?);
                Ok(Take {
                    description,
                    text,
                    seed,
                    sample,
                })
            },
        ));
    }

    fn toggle_sample(&mut self, id: Option<&str>) {
        if self.local.designer.playing(id) {
            self.local.designer.stop_sample();
            return;
        }
        if self.active() {
            self.stop();
        }
        self.stop_preview();
        let repaint = self.repaint.clone();
        let d = &mut self.local.designer;
        let playback = match id {
            Some(id) => {
                let id = id.to_owned();
                Playback::start("tts-sample", repaint, move || {
                    VoiceStore::default().samples(&id)
                })
            }
            None => {
                let Some(take) = &d.take else { return };
                let sample = Arc::clone(&take.sample);
                Playback::start("tts-sample", repaint, move || Ok(sample.pcm.clone()))
            }
        };
        d.error = None;
        d.playing = Some((id.map(str::to_owned), playback));
    }

    pub(in crate::reader::tts) fn design_window(
        &mut self,
        ctx: &egui::Context,
        s: &mut TtsSettings,
        spine: usize,
    ) {
        self.local.designer.apply_pending_designed(&mut s.local);
        if !self.design_window_open() {
            return;
        }
        let mut open = true;
        egui::Window::new("Design a voice")
            .id(egui::Id::unique("reader-tts-designer"))
            .open(&mut open)
            .collapsible(false)
            .default_width(460.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.design_download_row(ui);
                    ui.separator();
                    self.make_sample_form(ui, spine);
                    ui.separator();
                    self.kept_voices(ui);
                    if let Some(e) = &self.local.designer.error {
                        ui.colored_label(RED, e);
                    }
                });
            });
        if !open {
            self.close_design_window();
        }
    }

    fn make_sample_form(&mut self, ui: &mut egui::Ui, spine: usize) {
        let busy = self.local_voice_busy();
        let d = &mut self.local.designer;
        let can_keep = !busy && d.transfer.is_none() && d.op.is_none();
        let mut from_page = false;
        ui.add_enabled_ui(!d.making(), |ui| {
            ui.label("Description").on_hover_text(DESCRIPTION_TIP);
            ui.add(egui::TextEdit::multiline(&mut d.description)
                .hint_text(EXAMPLE).desired_rows(2).desired_width(f32::INFINITY))
                .on_hover_text(DESCRIPTION_TIP);
            ui.horizontal(|ui| {
                ui.label("Sample text");
                from_page = ui.small_button("Use this page")
                    .on_hover_text("Take a few sentences from the page you are reading.").clicked();
            });
            ui.add(egui::TextEdit::multiline(&mut d.text).desired_rows(3).desired_width(f32::INFINITY))
                .on_hover_text("A short passage lets you hear the voice's pace and tone. The kept voice reads other passages in the same style.");
        });
        if from_page {
            let passage = self
                .page_passage(spine, 60, self.page_end)
                .filter(|text| !text.is_empty());
            let d = &mut self.local.designer;
            if let Some(text) = passage {
                d.text = text;
                d.error = None;
            } else {
                d.error = Some("There is no text on this page yet.".into());
            }
        }
        ui.horizontal(|ui| {
            let d = &self.local.designer;
            let ready = self.local.design.present == Some(true)
                && !busy && !d.description.trim().is_empty() && !d.text.trim().is_empty();
            if ui.add_enabled(ready, egui::Button::new("Make sample"))
                .on_hover_text("Hear a new take from this description. You can make another if you prefer a different reading.")
                .clicked() {
                self.start_sample();
            }
            let d = &self.local.designer;
            if d.making() {
                crate::palette::spinner(ui);
                ui.label(RichText::new(if local::loaded(local::Mode::Design, 1) {
                    "Making the sample…"
                } else {
                    "Loading the voice designer…"
                }).weak());
            } else if d.take.is_some() {
                if ui.button(if d.playing(None) { "Stop sample" } else { "Play sample" }).clicked() {
                    self.toggle_sample(None);
                }
                if ui.add_enabled(can_keep, egui::Button::new("Keep…")).on_hover_text(SAMPLE_STYLE).clicked() {
                    self.local.designer.keep_name = Some(String::new());
                }
            }
        });
        let repaint = self.repaint.clone();
        let d = &mut self.local.designer;
        if let (Some(name), Some(take)) = (&mut d.keep_name, &d.take) {
            let mut keep = false;
            let mut cancel = false;
            ui.add_enabled_ui(can_keep, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(name);
                    keep = ui
                        .add_enabled(!name.trim().is_empty(), egui::Button::new("Keep"))
                        .clicked();
                    cancel = ui.button("Cancel").clicked();
                })
            });
            if cancel {
                d.keep_name = None;
            } else if keep {
                let name = name.clone();
                let description = take.description.clone();
                let text = take.text.clone();
                let seed = take.seed;
                let sample = Arc::clone(&take.sample);
                d.error = None;
                d.op = Some(local::spawn("tts-voice-save", repaint, move || {
                    let store = VoiceStore::default();
                    let id = store.save_voice(&name, &description, &text, seed, &sample.wav)?;
                    let voices = store.list_voices()?;
                    Ok(VoiceOp::Saved { id, voices })
                }));
            }
        }
    }

    fn kept_voices(&mut self, ui: &mut egui::Ui) {
        let busy = self.local_voice_busy();
        let d = &mut self.local.designer;
        let transferring = d.transfer.is_some();
        let working = transferring || d.op.is_some();
        let mut import = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Kept voices").strong())
                .on_hover_text(SAMPLE_STYLE);
            import = ui
                .add_enabled(!working, egui::Button::new("Import voice…").small())
                .clicked();
            if working {
                crate::palette::spinner(ui);
            }
        });
        if d.voices.is_empty() {
            ui.label(RichText::new("No voices kept yet.").weak());
        }
        let mut play_id = None;
        let mut export = None;
        for voice in &d.voices {
            let playing = d.playing(Some(&voice.id));
            ui.push_id(&voice.id, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(&voice.info.name)
                        .on_hover_text(&voice.info.description);
                    if ui
                        .small_button(if playing {
                            "Stop sample"
                        } else {
                            "Play sample"
                        })
                        .clicked()
                    {
                        play_id = Some(voice.id.clone());
                    }
                    ui.add_enabled_ui(!busy && !working, |ui| {
                        if ui.small_button("Rename…").clicked() {
                            d.edit = Some(Edit::Rename {
                                id: voice.id.clone(),
                                name: voice.info.name.clone(),
                            });
                        }
                        if ui.small_button("Delete…").clicked() {
                            d.edit = Some(Edit::Delete {
                                id: voice.id.clone(),
                                name: voice.info.name.clone(),
                            });
                        }
                    });
                    if ui
                        .add_enabled(!working, egui::Button::new("Export…").small())
                        .clicked()
                    {
                        export = Some((voice.id.clone(), voice.info.name.clone()));
                    }
                })
            });
        }
        if let Some(id) = play_id {
            self.toggle_sample(Some(&id));
        }
        if import {
            self.choose_voice_import();
        } else if let Some((id, name)) = export {
            self.choose_voice_export(id, &name);
        }
        self.edit_voice(ui, busy);
        match &self.local.designer.transfer_note {
            Some(Ok(note)) => {
                ui.label(note);
            }
            Some(Err(error)) => {
                ui.colored_label(RED, error);
            }
            None => {}
        }
    }

    fn choose_voice_import(&mut self) {
        let Some(path) = voice_dialog("Import voice").pick_file() else {
            return;
        };
        let d = &mut self.local.designer;
        d.edit = None;
        d.transfer_note = None;
        d.transfer = Some(local::spawn(
            "tts-voice-import",
            self.repaint.clone(),
            move || {
                VoiceStore::default()
                    .import_voice(&path)
                    .map(Transfer::Imported)
            },
        ));
    }

    fn choose_voice_export(&mut self, id: String, name: &str) {
        let Some(mut path) = voice_dialog("Export voice")
            .set_file_name(export_filename(name))
            .save_file()
        else {
            return;
        };
        if !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("voice"))
        {
            path.as_mut_os_string().push(".voice");
        }
        let d = &mut self.local.designer;
        d.edit = None;
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => {
                d.edit = Some(Edit::ReplaceExport { id, path });
            }
            Ok(_) => {
                d.transfer_note = Some(Err("Choose a regular file to replace.".into()));
            }
            Err(e) if e.kind() == ErrorKind::NotFound => self.start_voice_export(id, path, false),
            Err(e) => {
                d.transfer_note = Some(Err(format!("Cannot check the export destination: {e}")));
            }
        }
    }

    fn start_voice_export(&mut self, id: String, path: PathBuf, replace: bool) {
        let d = &mut self.local.designer;
        d.edit = None;
        d.transfer_note = None;
        d.transfer = Some(local::spawn(
            "tts-voice-export",
            self.repaint.clone(),
            move || {
                VoiceStore::default().export_voice(&id, &path, replace)?;
                Ok(Transfer::Exported(path))
            },
        ));
    }

    fn edit_voice(&mut self, ui: &mut egui::Ui, busy: bool) {
        let repaint = self.repaint.clone();
        let d = &mut self.local.designer;
        let Some(edit) = &mut d.edit else { return };
        let mut confirm = false;
        let mut cancel = false;
        let ready = d.transfer.is_none()
            && d.op.is_none()
            && (!busy || matches!(edit, Edit::ReplaceExport { .. }));
        ui.add_enabled_ui(ready, |ui| match edit {
            Edit::Rename { name, .. } => {
                ui.horizontal(|ui| {
                    ui.label("New name");
                    ui.text_edit_singleline(name);
                    confirm = ui
                        .add_enabled(!name.trim().is_empty(), egui::Button::new("Rename"))
                        .clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            }
            Edit::Delete { name, .. } => {
                ui.label(format!(
                    "Delete “{name}”? Its book audio stays under Saved audio."
                ));
                ui.horizontal(|ui| {
                    confirm = ui.button("Delete voice").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            }
            Edit::ReplaceExport { path, .. } => {
                ui.label(format!("Replace the existing file at {}?", path.display()));
                ui.horizontal(|ui| {
                    confirm = ui.button("Replace file").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            }
        });
        if cancel {
            d.edit = None;
        } else if confirm {
            match d.edit.take().unwrap() {
                Edit::Rename { id, name } => {
                    d.error = None;
                    d.op = Some(local::spawn("tts-voice-rename", repaint, move || {
                        let store = VoiceStore::default();
                        store.rename_voice(&id, &name)?;
                        let voices = store.list_voices()?;
                        Ok(VoiceOp::Renamed(voices))
                    }));
                }
                Edit::Delete { id, .. } => {
                    d.error = None;
                    d.op = Some(local::spawn("tts-voice-delete", repaint, move || {
                        let store = VoiceStore::default();
                        store.delete_voice(&id)?;
                        let voices = store.list_voices()?;
                        Ok(VoiceOp::Deleted { id, voices })
                    }));
                }
                Edit::ReplaceExport { id, path } => {
                    self.start_voice_export(id, path, true);
                }
            }
        }
    }
}
