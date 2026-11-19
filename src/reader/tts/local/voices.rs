//! Kept samples live with the user's settings so clearing the audio cache
//! leaves the references available for the next book.

use super::{FRAME_SAMPLES, Reference, wav_samples};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize, Deserialize)]
pub struct VoiceInfo {
    pub name: String,
    pub description: String,
    pub sample_text: String,
    pub seed: u64,
    pub created: u64,
}

pub struct SavedVoice {
    pub id: String,
    pub info: VoiceInfo,
}

pub struct VoiceStore(PathBuf);

impl Default for VoiceStore {
    fn default() -> Self {
        Self(hanstat::models::app_dir().join("voices"))
    }
}

/// Use the first 128 bits of the WAV's SHA-256. Even renamed voices keep
/// their clips, since the reference behind the id is immutable.
fn voice_id(wav: &[u8]) -> String {
    let hash = Sha256::digest(wav);
    format!(
        "{:032x}",
        u128::from_be_bytes(hash[..16].try_into().unwrap())
    )
}

fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A pending directory or file is hidden from the voice list.
fn temporary(parent: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    parent.join(format!(
        ".{}-{}-{n}.tmp",
        std::process::id(),
        now.as_nanos()
    ))
}

fn read_info(dir: &Path) -> Result<VoiceInfo, String> {
    let path = dir.join("voice.json");
    let bytes = fs::read(&path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("Cannot read {}: {e}", path.display()))
}

fn name_for_save(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the voice a name first.".into());
    }
    Ok(name.to_owned())
}

impl VoiceStore {
    fn dir(&self, id: &str) -> Result<PathBuf, String> {
        if !valid_id(id) {
            return Err("The designed voice has an invalid id.".into());
        }
        let dir = self.0.join(id);
        if !fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
            return Err("The kept voice could not be found. Choose another voice.".into());
        }
        Ok(dir)
    }

    pub fn list_voices(&self) -> Result<Vec<SavedVoice>, String> {
        let entries = match fs::read_dir(&self.0) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("Cannot list the kept voices: {e}")),
        };
        let mut voices: Vec<_> = entries
            .flatten()
            .filter_map(|entry| {
                let id = entry.file_name().into_string().ok()?;
                let dir = self.dir(&id).ok()?;
                let info = read_info(&dir).ok()?;
                dir.join("sample.wav")
                    .is_file()
                    .then_some(SavedVoice { id, info })
            })
            .collect();
        voices.sort_by_cached_key(|v| (v.info.name.to_lowercase(), v.id.clone()));
        Ok(voices)
    }

    /// Publish the two files together. Saving an identical reference again
    /// leaves its name and exact transcript where earlier clips expect them.
    pub fn save_voice(
        &self,
        name: &str,
        description: &str,
        text: &str,
        seed: u64,
        wav: &[u8],
    ) -> Result<String, String> {
        let name = name_for_save(name)?;
        if text.trim().is_empty() || wav_samples(wav).is_none_or(|s| s.is_empty()) {
            return Err("The sample needs a recording and its text.".into());
        }
        let id = voice_id(wav);
        let dir = self.0.join(&id);
        let same_reference = || {
            read_info(&dir).is_ok_and(|info| info.sample_text == text)
                && fs::read(dir.join("sample.wav")).is_ok_and(|saved| saved == wav)
        };
        if same_reference() {
            return Ok(id);
        }
        let info = VoiceInfo {
            name,
            description: description.to_owned(),
            sample_text: text.to_owned(),
            seed,
            created: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };
        let bytes = serde_json::to_vec_pretty(&info).map_err(|e| e.to_string())?;
        let tmp = temporary(&self.0);
        fs::create_dir_all(&self.0)
            .and_then(|()| fs::create_dir(&tmp))
            .map_err(|e| format!("Cannot create a folder for this voice: {e}"))?;
        let written = fs::write(tmp.join("sample.wav"), wav)
            .and_then(|()| fs::write(tmp.join("voice.json"), bytes))
            .and_then(|()| fs::rename(&tmp, &dir));
        if let Err(e) = written {
            let _ = fs::remove_dir_all(&tmp);
            if !same_reference() {
                return Err(format!("Cannot keep this voice: {e}"));
            }
        }
        Ok(id)
    }

    pub fn rename_voice(&self, id: &str, name: &str) -> Result<(), String> {
        let dir = self.dir(id)?;
        let mut info = read_info(&dir)?;
        info.name = name_for_save(name)?;
        let bytes = serde_json::to_vec_pretty(&info).map_err(|e| e.to_string())?;
        let tmp = temporary(&dir);
        fs::write(&tmp, bytes)
            .and_then(|()| fs::rename(&tmp, dir.join("voice.json")))
            .map_err(|e| {
                let _ = fs::remove_file(&tmp);
                format!("Cannot rename this voice: {e}")
            })
    }

    /// Take the whole voice out of the list before removing its files.
    /// Its sentence clips live elsewhere and can still be cleared there.
    pub fn delete_voice(&self, id: &str) -> Result<(), String> {
        let dir = self.dir(id)?;
        let tmp = temporary(&self.0);
        fs::rename(&dir, &tmp).map_err(|e| format!("Cannot delete this voice: {e}"))?;
        if let Err(e) = fs::remove_dir_all(&tmp) {
            return Err(format!(
                "The voice was removed, but its files could not all be deleted: {e}"
            ));
        }
        Ok(())
    }

    fn wav(&self, dir: &Path, id: &str) -> Result<Vec<u8>, String> {
        let wav =
            fs::read(dir.join("sample.wav")).map_err(|e| format!("Cannot read the sample: {e}"))?;
        if voice_id(&wav) != id {
            return Err(
                "The kept sample has changed. Make and keep a new sample to use it for reading."
                    .into(),
            );
        }
        Ok(wav)
    }

    pub fn samples(&self, id: &str) -> Result<Vec<i16>, String> {
        let wav = self.wav(&self.dir(id)?, id)?;
        wav_samples(&wav)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "The kept sample has no audio.".into())
    }

    pub(super) fn reference(&self, id: &str) -> Result<Reference, String> {
        let dir = self.dir(id)?;
        let info = read_info(&dir)?;
        let wav = self.wav(&dir, id)?;
        let samples = wav_samples(&wav)
            .filter(|s| !s.is_empty())
            .ok_or("The kept sample has no audio.")?;
        if info.sample_text.trim().is_empty() {
            return Err("The kept sample has no text.".into());
        }
        Ok(Reference {
            id: id.to_owned(),
            text: info.sample_text,
            wav,
            frames: samples.len().div_ceil(FRAME_SAMPLES),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_store_round_trip() {
        let store = VoiceStore(temporary(&std::env::temp_dir()));
        let wav = super::super::tests::test_wav();
        assert!(store.list_voices().unwrap().is_empty());
        let id = store
            .save_voice(" Radio ", "低沉温和", "河岸。", 42, &wav)
            .unwrap();
        assert_eq!(store.samples(&id).unwrap(), [1, -1]);
        let voices = store.list_voices().unwrap();
        assert_eq!(voices.len(), 1);
        let voice = &voices[0];
        assert_eq!(voice.id, id);
        assert_eq!(voice.info.name, "Radio");
        assert_eq!(voice.info.description, "低沉温和");
        assert_eq!(voice.info.sample_text, "河岸。");
        assert_eq!(voice.info.seed, 42);
        assert!(voice.info.created > 0);
        assert_eq!(store.reference(&id).unwrap().wav, wav);
        assert_eq!(
            store
                .save_voice("Another name", "低沉温和", "河岸。", 42, &wav)
                .unwrap(),
            id
        );
        assert!(
            store
                .save_voice("Wrong text", "低沉温和", "山顶。", 42, &wav)
                .is_err()
        );
        assert_eq!(store.list_voices().unwrap()[0].info.name, "Radio");
        store.rename_voice(&id, "Narrator").unwrap();
        assert_eq!(store.list_voices().unwrap()[0].info.name, "Narrator");
        assert_eq!(store.reference(&id).unwrap().text, "河岸。");
        assert!(store.delete_voice("..").is_err());
        fs::write(store.dir(&id).unwrap().join("sample.wav"), b"changed").unwrap();
        assert!(store.reference(&id).is_err());
        store.delete_voice(&id).unwrap();
        assert!(store.list_voices().unwrap().is_empty());
        fs::remove_dir(&store.0).unwrap();
    }
}
