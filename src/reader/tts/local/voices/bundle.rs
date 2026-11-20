//! The portable .voice format. The WAV is stored without any audio conversion.

use super::{Publication, VoiceInfo, VoiceStore, read_info, voice_id};
use crate::reader::tts::local::wav_data;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
use std::path::Path;
use tempfile::NamedTempFile;
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

const FORMAT: &str = "hanstat-voice";
const VERSION: u32 = 1;
const INFO: &str = "voice.json";
const WAV: &str = "sample.wav";
const MAX_ARCHIVE: usize = 20 * 1024 * 1024;
const MAX_INFO: usize = 1024 * 1024;
const MAX_WAV: usize = 16 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    #[serde(flatten)]
    info: VoiceInfo,
}

fn validate_reference(info: &VoiceInfo, wav: &[u8]) -> Result<(), String> {
    if info.name.trim().is_empty() || info.sample_text.trim().is_empty() {
        return Err("The voice file needs a name and a sample transcript.".into());
    }
    let riff_size = wav
        .get(4..8)
        .map(|size| u32::from_le_bytes(size.try_into().unwrap()) as usize);
    if riff_size != wav.len().checked_sub(8)
        || wav_data(wav)
            .and_then(|data| wav.get(data))
            .is_none_or(|data| data.is_empty() || !data.len().is_multiple_of(2))
    {
        return Err("The voice file needs a complete 24 kHz mono 16-bit PCM WAV sample.".into());
    }
    Ok(())
}

fn read_bounded(reader: impl Read, limit: usize, label: &str) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut data)
        .map_err(|e| format!("Cannot read {label}: {e}"))?;
    if data.len() > limit {
        return Err(format!(
            "{label} exceeds the {} MiB limit.",
            limit / (1024 * 1024)
        ));
    }
    Ok(data)
}

fn read_entry<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
    limit: usize,
) -> Result<Vec<u8>, String> {
    let entry = archive
        .by_name(name)
        .map_err(|e| format!("Cannot open {name} in the voice file: {e}"))?;
    if !entry.is_file()
        || !matches!(
            entry.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        )
    {
        return Err(format!("Unsupported {name} entry in the voice file."));
    }
    let size = entry.size();
    if size > limit as u64 {
        return Err(format!(
            "{name} exceeds the {} MiB limit.",
            limit / (1024 * 1024)
        ));
    }
    let data = read_bounded(entry, limit, name)?;
    if data.len() as u64 != size {
        return Err(format!("Unexpected size for {name} in the voice file."));
    }
    Ok(data)
}

impl VoiceStore {
    /// Returns the imported display name, or None when the reference is already kept.
    pub fn import_voice(&self, path: &Path) -> Result<Option<String>, String> {
        let metadata =
            fs::metadata(path).map_err(|e| format!("Cannot open the voice file: {e}"))?;
        if !metadata.is_file() {
            return Err("Choose a .voice file to import.".into());
        }
        if metadata.len() > MAX_ARCHIVE as u64 {
            return Err("The voice file exceeds the 20 MiB limit.".into());
        }
        let file = File::open(path).map_err(|e| format!("Cannot open the voice file: {e}"))?;
        let bytes = read_bounded(file, MAX_ARCHIVE, "The voice file")?;
        let mut archive = ZipArchive::new(Cursor::new(bytes))
            .map_err(|e| format!("Cannot read the voice archive: {e}"))?;
        if archive.len() != 2 || archive.has_duplicate_names() {
            return Err("The voice file must contain exactly voice.json and sample.wav.".into());
        }
        archive
            .validate_header_consistency()
            .map_err(|e| format!("Invalid voice archive: {e}"))?;
        let json = read_entry(&mut archive, INFO, MAX_INFO)?;
        let manifest: Manifest =
            serde_json::from_slice(&json).map_err(|e| format!("Invalid voice metadata: {e}"))?;
        if manifest.format != FORMAT {
            return Err("This file is not a Hanstat voice.".into());
        }
        if manifest.version != VERSION {
            return Err(format!(
                "Unsupported voice file version {}.",
                manifest.version
            ));
        }
        let wav = read_entry(&mut archive, WAV, MAX_WAV)?;
        validate_reference(&manifest.info, &wav)?;
        match self.publish(&voice_id(&wav), &manifest.info, &wav)? {
            Publication::Added => Ok(Some(manifest.info.name)),
            Publication::AlreadyKept => Ok(None),
        }
    }

    pub fn export_voice(&self, id: &str, path: &Path, replace: bool) -> Result<(), String> {
        let dir = self.dir(id)?;
        let info = read_info(&dir)?;
        let wav = self.wav(&dir, id)?;
        validate_reference(&info, &wav)?;
        let manifest = Manifest {
            format: FORMAT.into(),
            version: VERSION,
            info,
        };
        let json = serde_json::to_vec_pretty(&manifest)
            .map_err(|e| format!("Cannot write voice metadata: {e}"))?;
        if json.len() > MAX_INFO || wav.len() > MAX_WAV {
            return Err(
                "This voice exceeds the export size limits (1 MiB metadata, 16 MiB WAV).".into(),
            );
        }

        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut temp = NamedTempFile::new_in(parent)
            .map_err(|e| format!("Cannot create the voice export: {e}"))?;
        let mut archive = ZipWriter::new(temp.as_file_mut());
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, data) in [(INFO, json.as_slice()), (WAV, wav.as_slice())] {
            archive
                .start_file(name, options)
                .map_err(|e| format!("Cannot write the voice archive: {e}"))?;
            archive
                .write_all(data)
                .map_err(|e| format!("Cannot write the voice archive: {e}"))?;
        }
        archive
            .finish()
            .map_err(|e| format!("Cannot finish the voice archive: {e}"))?;
        temp.as_file()
            .sync_all()
            .map_err(|e| format!("Cannot save the voice archive: {e}"))?;
        // Deflate overhead for the two capped entries fits within MAX_ARCHIVE.
        let result = if replace {
            temp.persist(path)
        } else {
            temp.persist_noclobber(path)
        };
        result.map(|_| ()).map_err(|e| {
            if !replace && e.error.kind() == std::io::ErrorKind::AlreadyExists {
                "A file already exists at that path. Choose Export again to replace it.".into()
            } else {
                format!("Cannot save the voice file: {}", e.error)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::tts::local::tests::test_wav;

    const NAME: &str = "深夜 / Radio: narrator 🎙";

    fn manifest() -> Manifest {
        Manifest {
            format: FORMAT.into(),
            version: VERSION,
            info: VoiceInfo {
                name: NAME.into(),
                description: "低沉温和".into(),
                sample_text: "河岸。\n雨停了。".into(),
                seed: u64::MAX,
                created: 1_774_500_000,
            },
        }
    }

    fn archive(entries: &[(&str, &[u8])], compression: CompressionMethod) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(compression);
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn bundle(manifest: &Manifest, wav: &[u8]) -> Vec<u8> {
        let json = serde_json::to_vec(manifest).unwrap();
        archive(&[(INFO, &json), (WAV, wav)], CompressionMethod::Stored)
    }

    #[test]
    fn bundle_round_trip_preserves_saved_name_metadata_and_reference() {
        let work = tempfile::tempdir().unwrap();
        let source = VoiceStore(work.path().join("source"));
        let dest = VoiceStore(work.path().join("dest"));
        let path = work.path().join("different filename.voice");
        let wav = test_wav();
        let id = source
            .save_voice("draft", "低沉温和", "河岸。", u64::MAX, &wav)
            .unwrap();
        source.rename_voice(&id, NAME).unwrap();
        source.export_voice(&id, &path, false).unwrap();

        let mut zip = ZipArchive::new(File::open(&path).unwrap()).unwrap();
        assert_eq!(zip.len(), 2);
        assert_eq!(
            zip.by_name(WAV).unwrap().compression(),
            CompressionMethod::Deflated
        );
        drop(zip);

        assert_eq!(dest.import_voice(&path).unwrap().as_deref(), Some(NAME));
        let expected = source.list_voices().unwrap().pop().unwrap();
        let imported = dest.list_voices().unwrap().pop().unwrap();
        assert_eq!(imported.id, id);
        assert_eq!(
            serde_json::to_value(&imported.info).unwrap(),
            serde_json::to_value(&expected.info).unwrap()
        );
        let reference = dest.reference(&id).unwrap();
        assert_eq!(reference.wav, wav);
        assert_eq!(reference.text, expected.info.sample_text);
        assert_eq!(dest.samples(&id).unwrap(), [1, -1]);

        dest.rename_voice(&id, "My local name").unwrap();
        assert_eq!(dest.import_voice(&path).unwrap(), None);
        let mut expected_info = expected.info;
        expected_info.name = "My local name".into();
        let mut duplicate = manifest();
        duplicate.info.sample_text = expected_info.sample_text.clone();
        fs::write(&path, bundle(&duplicate, &wav)).unwrap();
        assert_eq!(dest.import_voice(&path).unwrap(), None);
        duplicate.info.sample_text = "山顶。".into();
        fs::write(&path, bundle(&duplicate, &wav)).unwrap();
        assert!(
            dest.import_voice(&path)
                .unwrap_err()
                .contains("different transcript")
        );
        let kept = dest.list_voices().unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(
            serde_json::to_value(&kept[0].info).unwrap(),
            serde_json::to_value(&expected_info).unwrap()
        );

        let mut other_wav = wav.clone();
        other_wav[44] = 2;
        duplicate.info.name = expected_info.name.clone();
        fs::write(&path, bundle(&duplicate, &other_wav)).unwrap();
        assert_eq!(
            dest.import_voice(&path).unwrap().as_deref(),
            Some("My local name")
        );
        let voices = dest.list_voices().unwrap();
        assert_eq!(voices.len(), 2);
        assert_ne!(voices[0].id, voices[1].id);
        assert!(voices.iter().all(|v| v.info.name == "My local name"));
    }

    #[test]
    fn export_requires_permission_to_replace_and_preserves_old_file_on_error() {
        let work = tempfile::tempdir().unwrap();
        let store = VoiceStore(work.path().join("voices"));
        let wav = test_wav();
        let id = store.save_voice(NAME, "", "Sample", 0, &wav).unwrap();
        let path = work.path().join("export.voice");
        fs::write(&path, b"existing file").unwrap();
        assert!(store.export_voice(&id, &path, false).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"existing file");
        store.export_voice(&id, &path, true).unwrap();
        let exported = fs::read(&path).unwrap();
        assert!(exported.starts_with(b"PK"));
        assert!(store.export_voice("missing", &path, true).is_err());
        assert_eq!(fs::read(&path).unwrap(), exported);
        assert_eq!(fs::read_dir(work.path()).unwrap().count(), 2);
    }

    #[test]
    fn invalid_bundles_never_publish_a_voice() {
        let work = tempfile::tempdir().unwrap();
        let dest = VoiceStore(work.path().join("voices"));
        let path = work.path().join("bad.voice");
        let wav = test_wav();
        let json = serde_json::to_vec(&manifest()).unwrap();
        let stored = CompressionMethod::Stored;
        let rejects = |bytes: &[u8], error: &str| {
            fs::write(&path, bytes).unwrap();
            let message = dest.import_voice(&path).unwrap_err();
            assert!(message.contains(error), "{message}");
            assert!(!dest.0.exists());
        };

        rejects(b"not a zip", "archive");
        let mut unsupported = manifest();
        unsupported.version = 2;
        rejects(&bundle(&unsupported, &wav), "version 2");
        unsupported.version = VERSION;
        unsupported.info.name = "  ".into();
        rejects(&bundle(&unsupported, &wav), "name and a sample transcript");
        unsupported.info.name = NAME.into();
        unsupported.info.sample_text = "\n ".into();
        rejects(&bundle(&unsupported, &wav), "name and a sample transcript");
        rejects(&archive(&[(INFO, b"{"), (WAV, &wav)], stored), "metadata");
        rejects(&archive(&[(INFO, &json)], stored), "exactly");
        rejects(
            &archive(&[(INFO, &json), ("../sample.wav", &wav)], stored),
            "sample.wav",
        );
        rejects(
            &archive(&[(INFO, &json), (WAV, &wav), ("extra", b"")], stored),
            "exactly",
        );

        let mut duplicate = archive(&[(INFO, &json), ("copy0.json", &json), (WAV, &wav)], stored);
        while let Some(pos) = duplicate
            .windows(INFO.len())
            .position(|b| b == b"copy0.json")
        {
            duplicate[pos..pos + INFO.len()].copy_from_slice(INFO.as_bytes());
        }
        rejects(&duplicate, "exactly");

        let mut truncated = wav.clone();
        truncated[40..44].copy_from_slice(&6u32.to_le_bytes());
        rejects(&bundle(&manifest(), &truncated), "complete 24 kHz");
        let mut wrong_rate = wav.clone();
        wrong_rate[24..28].copy_from_slice(&48_000u32.to_le_bytes());
        rejects(&bundle(&manifest(), &wrong_rate), "complete 24 kHz");
        let mut bad_crc = bundle(&manifest(), &wav);
        let pos = bad_crc.windows(wav.len()).position(|b| b == wav).unwrap();
        bad_crc[pos + 44] ^= 1;
        rejects(&bad_crc, "checksum");

        let deflated = CompressionMethod::Deflated;
        let too_much_info = vec![0; MAX_INFO + 1];
        rejects(
            &archive(&[(INFO, &too_much_info), (WAV, &wav)], deflated),
            "1 MiB",
        );
        let too_much_wav = vec![0; MAX_WAV + 1];
        rejects(
            &archive(&[(INFO, &json), (WAV, &too_much_wav)], deflated),
            "16 MiB",
        );
        File::create(&path)
            .unwrap()
            .set_len(MAX_ARCHIVE as u64 + 1)
            .unwrap();
        assert!(dest.import_voice(&path).unwrap_err().contains("20 MiB"));
        assert!(!dest.0.exists());
    }
}
