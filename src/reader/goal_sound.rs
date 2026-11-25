//! The sound when the daily goal is met: the built-in one
//! (`assets/goal-met.opus`), or a message of the reader's own said by one of
//! their local voices, made once and kept as `goal-sound.opus` in the app's
//! folder. The choice is in [`rs::TrackerSettings`]; the Stats panel's
//! settings show it ([`State::rows`]).

use super::tts::{local, opus};
use crate::reading_stats as rs;
use eframe::egui::{self, Color32, RichText};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

/// What the built-in sound says: `assets/goal-met.opus`, read by the
/// designed local voice Citrus (made by the ignored test
/// `goal_cheer_takes`). Also the message a chosen voice starts with.
pub(super) const CHEER_TEXT: &str = "宝贝！你好厉害哦！你完成今天的目标啦！";

/// The built-in sound: Ogg Opus, 24 kHz mono, about 4 s.
const CHEER: &[u8] = include_bytes!("../../assets/goal-met.opus");

/// The longest message. The built-in one (19 characters) took 3.4–4.6 s
/// in six takes of Citrus, so this one probably stays under 5 s.
pub(super) const MAX_CHARS: usize = 20;

/// The label of the built-in sound in the voice list.
const BUILT_IN: &str = "Built in (Citrus)";

/// Where the made sound is kept.
pub(super) fn path() -> PathBuf {
    hanstat::models::app_dir().join("goal-sound.opus")
}

/// The message the chosen voice says.
fn text(set: &rs::TrackerSettings) -> &str {
    let t = set.goal_sound_text.trim();
    if t.is_empty() { CHEER_TEXT } else { t }
}

/// What a made sound is for: the chosen voice and its message; `None` for
/// the built-in sound.
pub(super) fn key(set: &rs::TrackerSettings) -> Option<String> {
    let voice = set.goal_sound_voice.as_ref()?;
    Some(format!("{voice}\n{}", text(set)))
}

/// The sound made for the chosen voice and message is in `path`.
fn is_made(set: &rs::TrackerSettings) -> bool {
    key(set).is_some_and(|k| k == set.goal_sound_made)
}

/// The sound to play: the one made for the chosen voice and message, while
/// they are what it was made for and it can be read; else the built-in one.
pub(super) fn clip(set: &rs::TrackerSettings, path: &Path) -> Vec<u8> {
    if is_made(set)
        && let Ok(b) = hanstat::saver::read(path)
    {
        return b;
    }
    CHEER.to_vec()
}

/// The voice settings that read a message with `voice` (`designed:<id>` or
/// `speaker:<id>`), sampling as read aloud does (`base`), without its style
/// instruction.
pub(super) fn voice_settings(
    voice: &str,
    base: &local::LocalSettings,
) -> Option<local::LocalSettings> {
    let mut s = base.clone();
    s.instruct.clear();
    if let Some(id) = voice.strip_prefix("designed:") {
        s.designed = Some(id.to_owned());
    } else {
        s.speaker = voice.strip_prefix("speaker:")?.to_owned();
        s.designed = None;
    }
    Some(s)
}

/// The reader's local voices, as (id, label): designed voices, then the
/// built-in speakers, each once its model is downloaded.
fn voices() -> Vec<(String, String)> {
    let mut out = Vec::new();
    if local::design_downloaded()
        && let Ok(list) = local::VoiceStore::default().list_voices()
    {
        for v in list {
            out.push((format!("designed:{}", v.id), v.info.name));
        }
    }
    if local::downloaded() {
        for s in local::SPEAKERS {
            out.push((
                format!("speaker:{}", s.id),
                format!("{} — {}", s.name, s.sound),
            ));
        }
    }
    out
}

/// Seconds of a sound's samples.
fn seconds(pcm: &[f32], rate: u32) -> f64 {
    pcm.len() as f64 / rate.max(1) as f64
}

/// Goal-met sounds played (tests play none).
#[cfg(test)]
pub(super) static PLAYED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Play the sound for `set` and return how long it lasts.
pub(super) fn play(set: &rs::TrackerSettings) -> f64 {
    let (pcm, rate) = opus::decode(clip(set, &path()))
        .or_else(|_| opus::decode(CHEER.to_vec()))
        .unwrap_or_default();
    let secs = seconds(&pcm, rate);
    output(pcm, rate);
    secs
}

#[cfg(test)]
fn output(_: Vec<f32>, _: u32) {
    PLAYED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Play `pcm` on an output of its own, beside read aloud, off the UI
/// thread. Without a sound device it stays silent.
#[cfg(not(test))]
fn output(pcm: Vec<f32>, rate: u32) {
    if pcm.is_empty() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("goal-met-sound".into())
        .spawn(move || {
            let Ok(out) = super::tts::audio::Output::open(Arc::new(|| {})) else {
                return;
            };
            out.push(0, super::tts::audio::resample(&pcm, rate, out.rate));
            // The stream plays while `out` lives: until the sound is heard.
            let start = std::time::Instant::now();
            while !out.queued().is_empty() && !out.failed() && start.elapsed().as_secs() < 15 {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        });
}

/// Say `text` with `s` (the model loads first if it isn't): the sound as
/// Ogg Opus, and its seconds.
fn make(text: &str, s: &local::LocalSettings) -> Result<(Vec<u8>, f64), String> {
    let pcm = local::preview(text, s)?;
    let secs = pcm.len() as f64 / opus::RATE as f64;
    Ok((opus::encode(&pcm)?, secs))
}

/// A sound being made: what it is for, and the answer to come (the Opus
/// file and its seconds).
type Job = (String, Receiver<Result<(Vec<u8>, f64), String>>);

/// Making a sound, and what the panel says about it.
#[derive(Default)]
pub(super) struct State {
    /// The sound being made: its key, and the answer to come.
    job: Option<Job>,
    /// The last making failed: why.
    error: Option<String>,
    /// The made sound's length, by its key.
    made_secs: Option<(String, f64)>,
    /// The voices to choose from, listed when the panel opens.
    voices: Option<Vec<(String, String)>>,
}

impl State {
    /// The panel opened or closed: list the voices again next time.
    pub(super) fn forget_voices(&mut self) {
        self.voices = None;
    }

    #[cfg(test)]
    pub(super) fn set_voices(&mut self, v: Vec<(String, String)>) {
        self.voices = Some(v);
    }

    pub(super) fn making(&self) -> bool {
        self.job.is_some()
    }

    /// Once a frame: a sound made is saved, and its key returned for the
    /// settings (it plays from then on).
    pub(super) fn poll(&mut self) -> Option<String> {
        let (key, rx) = self.job.as_ref()?;
        let answer = match rx.try_recv() {
            Ok(a) => a,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err("Making the sound stopped.".to_owned()),
        };
        let key = key.clone();
        self.job = None;
        match answer.and_then(|(bytes, secs)| {
            hanstat::saver::write(&path(), bytes)
                .map(|()| secs)
                .map_err(|e| format!("The sound can't be saved: {e}"))
        }) {
            Ok(secs) => {
                self.error = None;
                self.made_secs = Some((key.clone(), secs));
                Some(key)
            }
            Err(e) => {
                self.error = Some(e);
                None
            }
        }
    }

    /// The settings rows under "Play a sound when it's met": the voice, its
    /// message, Make and Play, and how it stands. `base`: read aloud's
    /// local voice settings, whose sampling a made sound uses.
    pub(super) fn rows(
        &mut self,
        ui: &mut egui::Ui,
        set: &mut rs::TrackerSettings,
        base: &local::LocalSettings,
        muted: Color32,
        red: Color32,
    ) {
        let voices = self.voices.get_or_insert_with(voices).clone();
        ui.label("Voice");
        let shown = match &set.goal_sound_voice {
            None => BUILT_IN.to_owned(),
            Some(v) => voices
                .iter()
                .find(|(id, _)| id == v)
                .map_or_else(|| "A voice no longer here".to_owned(), |(_, l)| l.clone()),
        };
        egui::ComboBox::from_id_salt("goal-sound-voice")
            .selected_text(shown)
            .width(230.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut set.goal_sound_voice, None, BUILT_IN);
                for (id, label) in &voices {
                    ui.selectable_value(&mut set.goal_sound_voice, Some(id.clone()), label);
                }
            })
            .response
            .on_hover_text(if voices.is_empty() {
                "To use a voice of your own saying your own message, download the local voice under Read aloud (and design voices there)."
            } else {
                "The built-in sound, or one of your local voices saying a message of your own"
            });
        ui.end_row();

        if set.goal_sound_voice.is_some() {
            if set.goal_sound_text.trim().is_empty() {
                set.goal_sound_text = CHEER_TEXT.to_owned();
            }
            ui.label("Says");
            ui.horizontal(|ui| {
                // As wide as there is room for, beside the count: the
                // longest message shows whole.
                let width = (ui.available_width() - 44.0).max(200.0);
                ui.add(
                    egui::TextEdit::singleline(&mut set.goal_sound_text)
                        .char_limit(MAX_CHARS)
                        .desired_width(width),
                )
                .on_hover_text(format!(
                    "Up to {MAX_CHARS} characters, so it stays under about 5 seconds"
                ));
                ui.label(
                    RichText::new(format!(
                        "{}/{MAX_CHARS}",
                        set.goal_sound_text.chars().count()
                    ))
                    .color(muted)
                    .small(),
                );
            });
            ui.end_row();
        }

        ui.label("");
        let made = is_made(set);
        ui.horizontal(|ui| {
            if let Some(voice) = set.goal_sound_voice.clone() {
                let can = !self.making() && !made;
                if ui
                    .add_enabled(can, egui::Button::new("Make"))
                    .on_hover_text("Have the voice say it, once; it plays from then on")
                    .on_disabled_hover_text(if made {
                        "Made: change the voice or the message to make another"
                    } else {
                        "Making…"
                    })
                    .clicked()
                    && let (Some(key), Some(s)) = (key(set), voice_settings(&voice, base))
                {
                    let text = text(set).to_owned();
                    let ctx = ui.ctx().clone();
                    self.error = None;
                    self.job = Some((
                        key,
                        local::spawn(
                            "goal-sound",
                            Arc::new(move || ctx.request_repaint()),
                            move || make(&text, &s),
                        ),
                    ));
                }
            }
            if ui
                .button("▶ Play")
                .on_hover_text("Hear the sound that plays when the goal is met")
                .clicked()
            {
                play(set);
            }
        });
        ui.end_row();

        let note = if self.making() {
            Some(("Making… the voice model loads first.".to_owned(), muted))
        } else if let Some(e) = &self.error {
            Some((e.clone(), red))
        } else if set.goal_sound_voice.is_none() {
            None
        } else if made {
            let secs = self
                .made_secs
                .as_ref()
                .filter(|(k, _)| Some(k) == key(set).as_ref());
            let secs = secs.map(|(_, s)| *s).or_else(|| {
                let (pcm, rate) = opus::decode(clip(set, &path())).ok()?;
                let s = seconds(&pcm, rate);
                self.made_secs = key(set).map(|k| (k, s));
                Some(s)
            });
            Some(match secs {
                Some(s) if s > 5.0 => (
                    format!("Made: {s:.1} s. A shorter message keeps it under 5 s."),
                    muted,
                ),
                Some(s) => (format!("Made: {s:.1} s."), muted),
                None => ("Made.".to_owned(), muted),
            })
        } else {
            Some((
                "Not made yet: the built-in sound plays until it is.".to_owned(),
                muted,
            ))
        };
        if let Some((text, color)) = note {
            ui.label("");
            ui.label(RichText::new(text).color(color).small());
            ui.end_row();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_sound_decodes() {
        let (pcm, rate) = opus::decode(CHEER.to_vec()).unwrap();
        assert_eq!(rate, opus::RATE);
        let secs = seconds(&pcm, rate);
        assert!((3.5..4.5).contains(&secs), "{secs} s");
        let peak = pcm.iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!((0.3..1.0).contains(&peak), "peak {peak}");
        assert!(CHEER_TEXT.chars().count() <= MAX_CHARS);
    }

    /// The made sound plays only while the voice and message are what it
    /// was made for, and it can be read.
    #[test]
    fn the_made_sound_plays_only_for_what_it_was_made_for() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("goal-sound.opus");
        std::fs::write(&file, b"made").unwrap();
        let mut set = rs::TrackerSettings::default();
        assert_eq!(key(&set), None);
        assert_eq!(clip(&set, &file), CHEER, "built in");
        set.goal_sound_voice = Some("speaker:vivian".into());
        assert_eq!(
            key(&set).as_deref(),
            Some(format!("speaker:vivian\n{CHEER_TEXT}").as_str())
        );
        assert_eq!(clip(&set, &file), CHEER, "not made yet");
        set.goal_sound_text = " 加油！ ".into();
        set.goal_sound_made = key(&set).unwrap();
        assert_eq!(set.goal_sound_made, "speaker:vivian\n加油！");
        assert_eq!(clip(&set, &file), b"made");
        set.goal_sound_text = "加油！！".into();
        assert_eq!(clip(&set, &file), CHEER, "message changed");
        set.goal_sound_text = "加油！".into();
        set.goal_sound_voice = Some("designed:abc".into());
        assert_eq!(clip(&set, &file), CHEER, "voice changed");
        set.goal_sound_voice = Some("speaker:vivian".into());
        std::fs::remove_file(&file).unwrap();
        assert_eq!(clip(&set, &file), CHEER, "file gone");
    }

    #[test]
    fn voices_map_to_local_settings() {
        let base = local::LocalSettings {
            speaker: "vivian".into(),
            designed: Some("old".into()),
            instruct: "飞快".into(),
            temperature: 0.7,
            ..Default::default()
        };
        let d = voice_settings("designed:abc", &base).unwrap();
        assert_eq!(d.designed.as_deref(), Some("abc"));
        assert_eq!((d.instruct.as_str(), d.temperature), ("", 0.7));
        let s = voice_settings("speaker:dylan", &base).unwrap();
        assert_eq!((s.speaker.as_str(), s.designed.as_deref()), ("dylan", None));
        assert!(voice_settings("azure:x", &base).is_none());
    }

    /// A message made with the real model, as Make does: under 5 s, and
    /// Opus that decodes to as long. `HANSTAT_MODEL_DIR` with the local
    /// voice; `GOAL_SOUND_VOICE` (default `speaker:vivian`; a designed
    /// voice needs `LOCALAPPDATA` with its folder in hanstat/voices) and
    /// `GOAL_SOUND_OUT` (a folder for the .opus, to listen to).
    #[test]
    #[ignore]
    fn goal_sound_made_with_a_local_voice() {
        let voice = std::env::var("GOAL_SOUND_VOICE").unwrap_or_else(|_| "speaker:vivian".into());
        let text = "加油！你今天真棒！";
        let s = voice_settings(&voice, &local::LocalSettings::default()).unwrap();
        let (bytes, secs) = make(text, &s).unwrap();
        eprintln!("{voice}: {secs:.2} s, {} KB", bytes.len() / 1024);
        assert!((1.0..5.0).contains(&secs), "{secs} s");
        let (pcm, rate) = opus::decode(bytes.clone()).unwrap();
        assert!((seconds(&pcm, rate) - secs).abs() < 0.1);
        if let Ok(out) = std::env::var("GOAL_SOUND_OUT") {
            std::fs::create_dir_all(&out).unwrap();
            let name = voice.replace(':', "-");
            std::fs::write(Path::new(&out).join(format!("{name}.opus")), &bytes).unwrap();
        }
    }
}
