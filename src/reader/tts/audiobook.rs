//! The Overview's audiobook dialog. Synthesis fills the normal reader cache.

mod encoder;
mod output;
#[cfg(test)]
mod tests;

use super::engine::{Batch, Pool, Repaint, Sentence, chapter_sentences};
use super::{
    MP3, OPUS, Source, Speech, TtsSettings, azure, book_dir, cache_root, key, local, saved_clip,
};
use crate::reader::{Reader, Settings, chapter_labels, epub::Epub, toc_labels, tree};
use eframe::egui;
use encoder::{Ffmpeg, Setup};
use output::{
    Cover, Destination, Files, Format, OPUS_CHAPTER_LIMIT, Render, check_cancel, check_ffmpeg,
};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const BODY_SCROLL: &str = "audiobook-body";
const CHAPTER_SCROLL: &str = "audiobook-chapters";

struct SourceBook {
    path: PathBuf,
    id: String,
    traditional: bool,
}

struct Chapter {
    spine: usize,
    title: String,
    sentences: Vec<Sentence>,
}

struct Book {
    title: String,
    cover: Option<Cover>,
    dir: PathBuf,
    chapters: Vec<Chapter>,
    known_profiles: Vec<Speech>,
    reader_profile: u64,
}

struct Coverage {
    speech: Speech,
    sentences: Vec<usize>,
}

impl Book {
    fn load(
        source: &SourceBook,
        cover: Option<Vec<u8>>,
        thumbnail: Option<PathBuf>,
    ) -> Result<Form, String> {
        let epub = Epub::open(&source.path)?;
        let dir = book_dir(&epub.title, &source.id);
        let labels = chapter_labels(&epub, &toc_labels(&epub.toc));
        let mut sheets = tree::SheetCache::default();
        let chapters: Vec<_> = epub
            .spine
            .iter()
            .enumerate()
            .filter_map(|(i, path)| {
                let chapter = tree::build(&epub, path, &mut sheets);
                let sentences = chapter_sentences(&chapter.chars, &chapter.spoiler_ranges);
                (!sentences.is_empty()).then(|| Chapter {
                    spine: i,
                    title: if labels[i].is_empty() {
                        format!("Part {}", i + 1)
                    } else {
                        labels[i].clone()
                    },
                    sentences,
                })
            })
            .collect();
        let settings = Settings::load().tts;
        let defaults = TtsSettings::default();
        let local_voices = local::VoiceStore::default()
            .list_voices()
            .unwrap_or_default();
        let azure_voices = cached_azure_voices(&settings.region);
        let known_profiles = known_profiles(
            &settings,
            &defaults,
            source.traditional,
            &azure_voices,
            &local_voices,
        );
        let cover = cover.as_deref().and_then(Cover::from_bytes).or_else(|| {
            let bytes = fs::read(thumbnail?).ok()?;
            Cover::from_bytes(&bytes)
        });
        let book = Book {
            title: if epub.title.is_empty() {
                file_stem(&source.path)
            } else {
                epub.title
            },
            cover,
            dir,
            chapters,
            known_profiles,
            reader_profile: settings.speech(settings.source, source.traditional).key(""),
        };
        let coverage = book.coverage(None);
        let selected = vec![true; book.chapters.len()];
        Ok(Form {
            book: Arc::new(book),
            coverage,
            selected,
            settings,
            azure_voices,
            local_voices,
            local_ready: [local::downloaded(), local::design_downloaded()],
            format: Format::default(),
            subtitles: true,
            typed_key: String::new(),
            fetching: None,
            download: None,
            provider_error: None,
            reset_scroll: true,
            encoder: Setup::new(Ffmpeg::find()),
        })
    }

    fn coverage(&self, extra: Option<Speech>) -> Vec<Coverage> {
        // Recorded settings are indexed separately because clip names cannot
        // be reversed. Prefer current settings for an identical cache key.
        let mut profiles = BTreeMap::new();
        for entry in fs::read_dir(self.dir.join("profiles"))
            .into_iter()
            .flatten()
            .flatten()
        {
            if entry.path().extension().is_some_and(|ext| ext == "json")
                && let Ok(bytes) = fs::read(entry.path())
                && let Ok(profile) = serde_json::from_slice::<Speech>(&bytes)
            {
                profiles.insert(profile.key(""), profile);
            }
        }
        for profile in self.known_profiles.iter().rev() {
            profiles.insert(profile.key(""), profile.clone());
        }
        if let Some(profile) = extra {
            profiles.insert(profile.key(""), profile);
        }
        let mut mp3 = HashSet::new();
        let mut opus = HashSet::new();
        read_cache(&self.dir, &mut mp3, Some(&mut opus));
        // Old Azure caches were shared by all books, with one directory per
        // leading hash byte. Include them without moving files during the scan.
        for entry in fs::read_dir(cache_root()).into_iter().flatten().flatten() {
            if entry.file_name().to_str().is_some_and(super::is_legacy) {
                read_cache(&entry.path(), &mut mp3, None);
            }
        }
        let mut key_buf = String::new();
        profiles
            .into_values()
            .filter_map(|speech| {
                let existing = if speech.local.is_some() { &opus } else { &mp3 };
                if existing.is_empty() {
                    return None;
                }
                let prefix = speech.key_prefix();
                key_buf.clear();
                key_buf.push_str(&prefix);
                let prefix_len = key_buf.len();
                let mut any = false;
                let sentences: Vec<_> = self
                    .chapters
                    .iter()
                    .map(|chapter| {
                        let count = chapter
                            .sentences
                            .iter()
                            .filter(|s| {
                                key_buf.truncate(prefix_len);
                                key_buf.push_str(&s.text);
                                existing
                                    .contains(&hanstat::dict::xxh64::xxh64(key_buf.as_bytes(), 0))
                            })
                            .count();
                        any |= count > 0;
                        count
                    })
                    .collect();
                any.then_some(Coverage { speech, sentences })
            })
            .collect()
    }

    fn preferred<'a>(
        &self,
        selected: &[bool],
        profiles: &'a [Coverage],
    ) -> Option<(&'a Speech, usize, usize)> {
        profiles
            .iter()
            .filter_map(|profile| {
                let mut full = 0;
                let mut clips = 0;
                for (i, &selected) in selected.iter().enumerate() {
                    if selected {
                        let cached = profile.sentences[i];
                        full += usize::from(cached == self.chapters[i].sentences.len());
                        clips += cached;
                    }
                }
                (clips > 0).then_some((&profile.speech, full, clips))
            })
            .max_by_key(|(profile, full, clips)| {
                (
                    *full,
                    *clips,
                    profile.key("") == self.reader_profile,
                    profile.key(""),
                )
            })
    }
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .filter(|name| !name.is_empty())
        .map_or_else(
            || "Audiobook".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
}

fn cached_azure_voices(region: &str) -> Vec<azure::Voice> {
    if !azure::valid_region(region) {
        return Vec::new();
    }
    let path = cache_root().join(format!("voices-{region}.json"));
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .map(azure::chinese_voices)
        .unwrap_or_default()
}

fn known_profiles(
    current: &TtsSettings,
    default: &TtsSettings,
    traditional: bool,
    azure: &[azure::Voice],
    local: &[local::SavedVoice],
) -> Vec<Speech> {
    let mut profiles = vec![
        current.speech(Source::Azure, traditional),
        current.speech(Source::Local, traditional),
        default.speech(Source::Azure, traditional),
        default.speech(Source::Local, traditional),
    ];
    for voice in azure
        .iter()
        .map(|v| v.short_name.as_str())
        .chain([azure::default_voice(false), azure::default_voice(true)])
    {
        for rate in [current.rate, 0] {
            profiles.push(Speech {
                voice: voice.to_owned(),
                rate,
                local: None,
            });
        }
    }
    for settings in [current, default] {
        let base = settings.speech(Source::Local, traditional);
        let local_settings = base.local.as_ref().unwrap();
        for speaker in local::SPEAKERS {
            profiles.push(Speech {
                local: Some(local::LocalSettings {
                    speaker: speaker.id.to_owned(),
                    designed: None,
                    ..local_settings.clone()
                }),
                ..base.clone()
            });
        }
        for saved in local {
            profiles.push(Speech {
                local: Some(local::LocalSettings {
                    designed: Some(saved.id.clone()),
                    ..local_settings.clone()
                }),
                ..base.clone()
            });
        }
    }
    profiles
}

fn read_cache(dir: &Path, mp3: &mut HashSet<u64>, mut opus: Option<&mut HashSet<u64>>) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let Some((stem, ext)) = name.to_str().and_then(|s| s.rsplit_once('.')) else {
            continue;
        };
        if stem.len() == 16
            && let Ok(key) = u64::from_str_radix(stem, 16)
            && entry.file_type().is_ok_and(|ty| ty.is_file())
        {
            match ext {
                MP3 => {
                    mp3.insert(key);
                }
                OPUS => {
                    if let Some(set) = opus.as_deref_mut() {
                        set.insert(key);
                    }
                }
                _ => {}
            }
        }
    }
}

pub(crate) struct Audiobook {
    source: Arc<SourceBook>,
    loading: Receiver<Result<Form, String>>,
    form: Option<Form>,
    job: Option<Job>,
    confirm: Option<Files>,
    complete: Option<Complete>,
    error: Option<String>,
}

pub(crate) enum Action {
    Keep,
    Close,
    Read(Reading),
}

pub(crate) struct Reading {
    source: Arc<SourceBook>,
    speech: Speech,
    spine: usize,
}

impl Reading {
    pub fn open(
        self,
        ctx: &egui::Context,
        dictionary: &mut crate::reader::DictionarySession,
    ) -> Result<Reader, String> {
        Reader::open_with_voice(
            &self.source.path,
            &self.source.id,
            self.source.traditional,
            self.spine,
            &self.speech,
            ctx,
            dictionary,
        )
    }
}

impl Audiobook {
    pub fn new(
        path: PathBuf,
        id: Option<&str>,
        traditional: bool,
        cover: Option<Vec<u8>>,
        ctx: &egui::Context,
    ) -> Self {
        let thumbnail = id.map(hanstat::history::cover_path);
        let id = id.map_or_else(|| path.display().to_string(), str::to_owned);
        let source = Arc::new(SourceBook {
            path,
            id,
            traditional,
        });
        let book_source = source.clone();
        let loading = local::spawn("audiobook-chapters", repaint(ctx), move || {
            Book::load(&book_source, cover, thumbnail)
        });
        Self {
            source,
            loading,
            form: None,
            job: None,
            confirm: None,
            complete: None,
            error: None,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Action {
        self.poll();
        let response = egui::Modal::new(egui::Id::unique("export-audiobook")).show(ctx, |ui| {
            let available = ctx.content_rect().size();
            ui.set_width((available.x - 30.0).clamp(240.0, 490.0));
            ui.heading("Export audiobook");
            let body_height = (available.y - 70.0).max(1.0);
            if self.form.as_ref().is_some_and(|form| form.reset_scroll) {
                reset_scroll(ui, BODY_SCROLL);
            }
            // An Area starts layout with its previous size. Give the scroll
            // area room to grow past the loading message's height.
            egui::ScrollArea::vertical()
                .id_salt(BODY_SCROLL)
                .max_height(body_height)
                .min_scrolled_height(body_height)
                .auto_shrink([false, true])
                .show(ui, |ui| self.content(ui))
                .inner
        });
        if matches!(response.inner, Action::Keep) && self.job.is_none() && response.should_close() {
            Action::Close
        } else {
            response.inner
        }
    }

    fn poll(&mut self) {
        if self.form.is_none()
            && self.error.is_none()
            && let Some(result) = received(&self.loading)
        {
            match result.flatten() {
                Ok(form) => self.form = Some(form),
                Err(error) => self.error = Some(error),
            }
        }
        if let Some(job) = &self.job
            && let Some(result) = received(&job.receiver)
        {
            self.job = None;
            match result {
                Ok(Outcome::Complete(complete)) => self.complete = Some(complete),
                Ok(Outcome::Retry { error, coverage }) => {
                    self.error = Some(error);
                    if let Some(form) = &mut self.form {
                        form.coverage = coverage;
                    }
                }
                Err(error) => self.error = Some(error),
            }
        }
        if let Some(form) = &mut self.form {
            form.poll();
        }
    }

    fn content(&mut self, ui: &mut egui::Ui) -> Action {
        if let Some(job) = &self.job {
            job.ui(ui);
            return Action::Keep;
        }
        if let Some(complete) = &self.complete {
            ui.label("Audiobook saved.");
            ui.label(complete.audio.display().to_string());
            if let Some(path) = &complete.subtitles {
                ui.label(path.display().to_string());
            }
            let mut action = Action::Keep;
            ui.horizontal(|ui| {
                if ui.button("Open in Reader").clicked() {
                    action = Action::Read(Reading {
                        source: self.source.clone(),
                        speech: complete.speech.clone(),
                        spine: complete.first,
                    });
                }
                if ui.button("Close").clicked() {
                    action = Action::Close;
                }
            });
            return action;
        }
        if let Some(files) = &self.confirm {
            ui.label("Replace the existing export files?");
            for dest in std::iter::once(&files.audio)
                .chain(&files.subtitles)
                .filter(|d| d.replace)
            {
                ui.label(dest.path.display().to_string());
            }
            let mut replace = false;
            let mut cancel = false;
            ui.horizontal(|ui| {
                replace = ui.button("Replace").clicked();
                cancel = ui.button("Back").clicked();
            });
            if replace && let Some(files) = self.confirm.take() {
                self.start(files, ui.ctx());
            } else if cancel {
                self.confirm = None;
            }
            return Action::Keep;
        }
        if let Some(error) = &self.error {
            ui.colored_label(crate::palette::error(), error);
        }
        let Some(form) = &mut self.form else {
            if self.error.is_none() {
                ui.horizontal(|ui| {
                    crate::palette::spinner(ui);
                    ui.label("Reading chapters and looking for saved audio…");
                });
            }
            return if ui.button("Close").clicked() {
                Action::Close
            } else {
                Action::Keep
            };
        };
        let provider_ready = form.show(ui, self.source.traditional);
        form.encoder.show(ui);
        let can_export = provider_ready && form.encoder.executable().is_some();
        let mut close = false;
        let mut save = false;
        ui.horizontal(|ui| {
            save = ui
                .add_enabled(can_export, egui::Button::new("Save audiobook…"))
                .clicked();
            close = ui.button("Close").clicked();
        });
        if save {
            match form.destinations(&self.source.path) {
                Ok(Some(files))
                    if files.audio.replace
                        || files.subtitles.as_ref().is_some_and(|d| d.replace) =>
                {
                    self.confirm = Some(files);
                }
                Ok(Some(files)) => self.start(files, ui.ctx()),
                Ok(None) => {}
                Err(error) => self.error = Some(error),
            }
        }
        if close { Action::Close } else { Action::Keep }
    }

    fn start(&mut self, files: Files, ctx: &egui::Context) {
        if let Some(form) = &self.form
            && let Some(ffmpeg) = form.encoder.executable().cloned()
        {
            self.error = None;
            self.job = Some(Job::start(
                form,
                self.source.traditional,
                files,
                ffmpeg,
                ctx,
            ));
        }
    }
}

struct Form {
    book: Arc<Book>,
    coverage: Vec<Coverage>,
    selected: Vec<bool>,
    settings: TtsSettings,
    format: Format,
    subtitles: bool,
    azure_voices: Vec<azure::Voice>,
    typed_key: String,
    fetching: Option<Receiver<Result<Vec<azure::Voice>, String>>>,
    local_voices: Vec<local::SavedVoice>,
    local_ready: [bool; 2],
    download: Option<Download>,
    provider_error: Option<String>,
    reset_scroll: bool,
    encoder: Setup,
}

impl Form {
    fn speech(&self, traditional: bool) -> Speech {
        self.book
            .preferred(&self.selected, &self.coverage)
            .map_or_else(
                || self.settings.speech(self.settings.source, traditional),
                |(speech, _, _)| speech.clone(),
            )
    }

    fn show(&mut self, ui: &mut egui::Ui, traditional: bool) -> bool {
        ui.label(&self.book.title);
        ui.separator();
        ui.horizontal(|ui| {
            ui.strong("Chapters");
            if ui.small_button("Select all").clicked() {
                self.selected.fill(true);
            }
            if ui.small_button("Clear").clicked() {
                self.selected.fill(false);
            }
        });
        let chapter_height = (ui.ctx().content_rect().height() * 0.30).clamp(55.0, 245.0);
        if std::mem::take(&mut self.reset_scroll) {
            reset_scroll(ui, CHAPTER_SCROLL);
        }
        let row_h = ui.spacing().interact_size.y;
        egui::ScrollArea::vertical()
            .id_salt(CHAPTER_SCROLL)
            .max_height(chapter_height)
            .min_scrolled_height(chapter_height)
            .auto_shrink([false, true])
            .show_rows(ui, row_h, self.book.chapters.len(), |ui, range| {
                for i in range {
                    ui.checkbox(&mut self.selected[i], &self.book.chapters[i].title);
                }
            });
        if self.selected.is_empty() {
            ui.label("This book has no chapters with readable text.");
            return false;
        }
        ui.separator();
        ui.horizontal(|ui| {
            for format in [Format::M4b, Format::Opus] {
                ui.radio_value(&mut self.format, format, format.label());
            }
        });
        ui.checkbox(&mut self.subtitles, "Export sentence subtitles (.srt)");
        if self.format == Format::Opus
            && self.selected.iter().filter(|&&selected| selected).count() > OPUS_CHAPTER_LIMIT
        {
            ui.label(
                "Opus supports up to 1,000 chapter markers. Select fewer chapters or choose M4B.",
            );
            return false;
        }
        let total: usize = self
            .book
            .chapters
            .iter()
            .zip(&self.selected)
            .filter(|(_, selected)| **selected)
            .map(|(chapter, _)| chapter.sentences.len())
            .sum();
        if total == 0 {
            ui.label("Select at least one chapter.");
            return false;
        }
        ui.separator();
        let mut missing = total;
        let speech = if let Some((speech, chapters, cached)) =
            self.book.preferred(&self.selected, &self.coverage)
        {
            missing -= cached;
            ui.strong(self.voice_label(speech));
            ui.label(format!(
                "{chapters} complete chapters · {cached} of {total} sentence clips cached"
            ));
            speech.clone()
        } else {
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.settings.source, Source::Azure, "Azure");
                ui.radio_value(&mut self.settings.source, Source::Local, "Local voice");
            });
            self.voice_picker(ui, traditional);
            self.settings.speech(self.settings.source, traditional)
        };
        if missing == 0 {
            ui.label("All selected audio is ready.");
            return true;
        }
        ui.label(format!("{missing} sentence clips to generate"));
        let ready = if let Some(local) = &speech.local {
            self.local_setup(ui, local.designed.as_deref())
        } else {
            self.azure_setup(ui)
        };
        if let Some(error) = &self.provider_error {
            ui.colored_label(crate::palette::error(), error);
        }
        ready
    }

    fn voice_label(&self, speech: &Speech) -> String {
        if let Some(local) = &speech.local {
            format!("Local · {}", self.local_label(local))
        } else if speech.rate == 0 {
            format!("Azure · {}", speech.voice)
        } else {
            format!("Azure · {} · {:+}%", speech.voice, speech.rate)
        }
    }

    fn local_label(&self, local: &local::LocalSettings) -> &str {
        if let Some(id) = &local.designed {
            self.local_voices
                .iter()
                .find(|v| &v.id == id)
                .map_or("Unavailable saved voice", |v| v.info.name.as_str())
        } else {
            local::speaker(&local.speaker).name
        }
    }

    fn voice_picker(&mut self, ui: &mut egui::Ui, traditional: bool) {
        if self.settings.source == Source::Azure {
            let current = self.settings.voice(traditional);
            let slot = if traditional {
                &mut self.settings.voice_trad
            } else {
                &mut self.settings.voice
            };
            let mut fetch = false;
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("audiobook-azure-voice")
                    .selected_text(&current)
                    .show_ui(ui, |ui| {
                        let default = azure::default_voice(traditional);
                        if ui.selectable_label(current == default, default).clicked() {
                            slot.clear();
                        }
                        for voice in self.azure_voices.iter().filter(|v| v.short_name != default) {
                            if ui
                                .selectable_label(current == voice.short_name, voice.label())
                                .clicked()
                            {
                                slot.clone_from(&voice.short_name);
                            }
                        }
                    });
                fetch = ui
                    .add_enabled(
                        self.fetching.is_none() && azure::valid_region(self.settings.region.trim()),
                        egui::Button::new("Get Azure voices"),
                    )
                    .clicked();
                if self.fetching.is_some() {
                    crate::palette::spinner(ui);
                }
            });
            if fetch {
                self.fetch_azure(ui.ctx());
            }
        } else {
            let name = self.local_label(&self.settings.local).to_owned();
            let local = &mut self.settings.local;
            egui::ComboBox::from_id_salt("audiobook-local-voice")
                .selected_text(name)
                .show_ui(ui, |ui| {
                    for speaker in local::SPEAKERS {
                        if ui
                            .selectable_label(
                                local.designed.is_none() && local.speaker == speaker.id,
                                format!("{} · {}", speaker.name, speaker.native),
                            )
                            .clicked()
                        {
                            local.speaker = speaker.id.into();
                            local.designed = None;
                        }
                    }
                    for saved in &self.local_voices {
                        if ui
                            .selectable_label(
                                local.designed.as_ref() == Some(&saved.id),
                                &saved.info.name,
                            )
                            .clicked()
                        {
                            local.designed = Some(saved.id.clone());
                        }
                    }
                });
        }
    }

    fn azure_setup(&mut self, ui: &mut egui::Ui) -> bool {
        let region = &mut self.settings.region;
        ui.horizontal(|ui| {
            ui.label("Azure region");
            ui.add(
                egui::TextEdit::singleline(region)
                    .desired_width(150.0)
                    .hint_text("e.g. westeurope"),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Azure key");
            ui.add(
                egui::TextEdit::singleline(&mut self.typed_key)
                    .password(true)
                    .desired_width(195.0)
                    .hint_text("Use saved key"),
            );
        });
        azure::valid_region(region.trim())
    }

    fn fetch_azure(&mut self, ctx: &egui::Context) {
        let region = self.settings.region.trim().to_owned();
        let entered = self.typed_key.trim().to_owned();
        self.provider_error = None;
        self.fetching = Some(local::spawn("audiobook-voices", repaint(ctx), move || {
            let client = azure_client(&region, entered)?;
            let voices = client.voices().map_err(|e| e.to_string())?;
            if let Ok(bytes) = serde_json::to_vec(&voices) {
                let _ = hanstat::saver::write(
                    &cache_root().join(format!("voices-{region}.json")),
                    bytes,
                );
            }
            Ok(azure::chinese_voices(voices))
        }));
    }

    fn local_setup(&mut self, ui: &mut egui::Ui, designed: Option<&str>) -> bool {
        if let Some(id) = designed
            && !self.local_voices.iter().any(|v| v.id == id)
        {
            ui.label("Restore the saved voice to generate its missing audio.");
            return false;
        }
        if self.local_ready[usize::from(designed.is_some())] {
            return true;
        }
        if let Some(download) = &self.download {
            let (done, total) = *download.bytes.lock().unwrap();
            ui.add(
                egui::ProgressBar::new(done as f32 / total.max(1) as f32).text(format!(
                    "Downloading local voice · {:.2} / {:.2} GB",
                    done as f64 / 1e9,
                    total as f64 / 1e9
                )),
            );
        } else if ui.button("Download local voice…").clicked() {
            self.download = Some(Download::start(designed.is_some(), ui.ctx()));
            self.provider_error = None;
        }
        false
    }

    fn poll(&mut self) {
        #[cfg(windows)]
        self.encoder.poll();
        if let Some(fetch) = &self.fetching
            && let Some(result) = received(fetch)
        {
            self.fetching = None;
            match result.flatten() {
                Ok(voices) => self.azure_voices = voices,
                Err(error) => self.provider_error = Some(error),
            }
        }
        if let Some(download) = &self.download
            && let Some(result) = received(&download.receiver)
        {
            let result = result.flatten();
            self.local_ready[usize::from(download.designed)] = result.is_ok();
            self.download = None;
            self.provider_error = result.err();
        }
    }

    fn destinations(&self, source: &Path) -> Result<Option<Files>, String> {
        let ext = self.format.ext();
        let Some(path) = rfd::FileDialog::new()
            .set_title("Save audiobook")
            .set_file_name(format!("{}.{ext}", file_stem(source)))
            .add_filter(self.format.label(), &[ext])
            .save_file()
        else {
            return Ok(None);
        };
        let audio_path = path.with_extension(ext);
        let audio = destination(audio_path.clone())?;
        let subtitles = self
            .subtitles
            .then(|| destination(audio_path.with_extension("srt")))
            .transpose()?;
        Ok(Some(Files { audio, subtitles }))
    }
}

fn reset_scroll(ui: &egui::Ui, salt: &str) {
    let id = ui.make_persistent_id(egui::IdSalt::new(salt));
    egui::scroll_area::State::default().store(ui.ctx(), id);
}

fn destination(path: PathBuf) -> Result<Destination, String> {
    let mut dest = Destination {
        path,
        replace: false,
    };
    dest.replace = dest.exists()?;
    Ok(dest)
}

fn azure_client(region: &str, entered: String) -> Result<Arc<azure::Azure>, String> {
    if !azure::valid_region(region) {
        return Err("Enter your Azure Speech region.".into());
    }
    let key = if entered.is_empty() {
        key::load()?
            .ok_or("Enter your Azure Speech key or save it in the reader’s Read aloud settings.")?
    } else {
        entered
    };
    Ok(Arc::new(azure::Azure::new(region, &key)))
}

struct Download {
    bytes: Arc<Mutex<(u64, u64)>>,
    receiver: Receiver<Result<(), String>>,
    designed: bool,
}

impl Download {
    fn start(designed: bool, ctx: &egui::Context) -> Self {
        let (size, download) = if designed {
            (
                local::design_download_size(),
                local::download_design as fn(&dyn Fn(u64, u64)) -> _,
            )
        } else {
            (
                local::download_size(),
                local::download as fn(&dyn Fn(u64, u64)) -> _,
            )
        };
        let bytes = Arc::new(Mutex::new((0, size)));
        let moved = bytes.clone();
        let repaint = repaint(ctx);
        let receiver = local::spawn("audiobook-download", repaint.clone(), move || {
            download(&|done, total| {
                *moved.lock().unwrap() = (done, total);
                repaint();
            })
        });
        Self {
            bytes,
            receiver,
            designed,
        }
    }
}

#[derive(Clone, Copy)]
enum Stage {
    Check,
    Generate,
    Encode,
    Finish,
}

struct Progress {
    stage: Mutex<Stage>,
    batch: Arc<Batch>,
    enc_done: AtomicUsize,
    total: usize,
}

impl Progress {
    fn finishing(&self) {
        *self.stage.lock().unwrap() = Stage::Finish;
    }
}

struct Job {
    progress: Arc<Progress>,
    receiver: Receiver<Outcome>,
}

struct Complete {
    audio: PathBuf,
    subtitles: Option<PathBuf>,
    speech: Speech,
    first: usize,
}

enum Outcome {
    Complete(Complete),
    Retry {
        error: String,
        coverage: Vec<Coverage>,
    },
}

struct Request {
    book: Arc<Book>,
    chosen: Vec<usize>,
    speech: Speech,
    format: Format,
    files: Files,
    azure_region: String,
    azure_key: String,
    ffmpeg: Ffmpeg,
}

impl Job {
    fn start(
        form: &Form,
        traditional: bool,
        files: Files,
        ffmpeg: Ffmpeg,
        ctx: &egui::Context,
    ) -> Self {
        let chosen: Vec<_> = form
            .selected
            .iter()
            .enumerate()
            .filter_map(|(i, &yes)| yes.then_some(i))
            .collect();
        let progress = Arc::new(Progress {
            stage: Mutex::new(Stage::Check),
            batch: Arc::new(Batch::default()),
            enc_done: AtomicUsize::new(0),
            total: chosen
                .iter()
                .map(|&i| form.book.chapters[i].sentences.len())
                .sum(),
        });
        let request = Request {
            book: form.book.clone(),
            chosen,
            speech: form.speech(traditional),
            format: form.format,
            files,
            azure_region: form.settings.region.trim().to_owned(),
            azure_key: form.typed_key.trim().to_owned(),
            ffmpeg,
        };
        let work = progress.clone();
        let repaint = repaint(ctx);
        let receiver = local::spawn("audiobook-export", repaint.clone(), move || {
            let Request { book, speech, .. } = &request;
            match request.run(&work, repaint) {
                Ok(complete) => Outcome::Complete(complete),
                Err(error) => Outcome::Retry {
                    error,
                    coverage: book.coverage(Some(speech.clone())),
                },
            }
        });
        Self { progress, receiver }
    }

    fn ui(&self, ui: &mut egui::Ui) {
        let p = &self.progress;
        let cancel = &p.batch.cancel;
        let (label, done) = match *p.stage.lock().unwrap() {
            Stage::Check => ("Checking FFmpeg and saved audio…", None),
            Stage::Generate => (
                "Generating audio",
                Some(p.batch.cached.load(Relaxed) + p.batch.done.load(Relaxed)),
            ),
            Stage::Encode => ("Writing the audiobook", Some(p.enc_done.load(Relaxed))),
            Stage::Finish => ("Saving the export…", None),
        };
        ui.label(if cancel.load(Relaxed) {
            "Cancelling…"
        } else {
            label
        });
        if let Some(done) = done {
            ui.add(
                egui::ProgressBar::new(done as f32 / p.total.max(1) as f32)
                    .text(format!("{done} / {} sentences", p.total)),
            );
        } else {
            crate::palette::spinner(ui);
        }
        if ui
            .add_enabled(!cancel.load(Relaxed), egui::Button::new("Cancel export"))
            .clicked()
        {
            cancel.store(true, Relaxed);
        }
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.progress.batch.cancel.store(true, Relaxed);
    }
}

impl Request {
    fn run(&self, p: &Progress, repaint: Repaint) -> Result<Complete, String> {
        check_cancel(&p.batch.cancel)?;
        self.files.check()?;
        check_ffmpeg(&self.ffmpeg, self.format)?;
        let parent = self.files.audio.path.parent().unwrap_or(Path::new("."));
        tempfile::tempfile_in(parent)
            .map_err(|e| format!("Cannot write the export in {}: {e}", parent.display()))?;
        let book = &self.book;
        let speech = &self.speech;
        let chapters: Vec<_> = self.chosen.iter().map(|&i| &book.chapters[i]).collect();
        let first = chapters
            .first()
            .ok_or("Select at least one chapter.")?
            .spine;
        let missing = chapters.iter().any(|chapter| {
            chapter
                .sentences
                .iter()
                .any(|s| saved_clip(&book.dir, speech.key(&s.text), speech.ext()).is_none())
        });
        check_cancel(&p.batch.cancel)?;
        if missing {
            let client = if let Some(local) = &speech.local {
                let ready = if local.designed.is_some() {
                    local::design_downloaded()
                } else {
                    local::downloaded()
                };
                if !ready {
                    return Err("Download the local voice to generate the missing audio.".into());
                }
                None
            } else {
                Some(azure_client(&self.azure_region, self.azure_key.clone())?)
            };
            *p.stage.lock().unwrap() = Stage::Generate;
            repaint();
            Pool::generate(
                chapters.iter().map(|chapter| &chapter.sentences),
                &book.dir,
                speech,
                client,
                &p.batch,
                repaint.clone(),
            )?;
        }
        check_cancel(&p.batch.cancel)?;
        *p.stage.lock().unwrap() = Stage::Encode;
        repaint();
        Render {
            ffmpeg: &self.ffmpeg,
            title: &book.title,
            cover: book.cover.as_ref(),
            chapters: &chapters,
            dir: &book.dir,
            speech,
            format: self.format,
            files: &self.files,
            progress: p,
        }
        .write()?;
        Ok(Complete {
            audio: self.files.audio.path.clone(),
            subtitles: self.files.subtitles.as_ref().map(|dest| dest.path.clone()),
            speech: speech.clone(),
            first,
        })
    }
}

fn repaint(ctx: &egui::Context) -> Repaint {
    let ctx = ctx.clone();
    Arc::new(move || ctx.request_repaint())
}

fn received<T>(rx: &Receiver<T>) -> Option<Result<T, String>> {
    match rx.try_recv() {
        Ok(result) => Some(Ok(result)),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => {
            Some(Err("A background task stopped unexpectedly.".into()))
        }
    }
}
