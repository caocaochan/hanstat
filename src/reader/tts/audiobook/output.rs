//! Assemble a single audio stream and derive chapter and subtitle times
//! from the samples that go into it.

use super::encoder::{Ffmpeg, start_error};
use super::{Chapter, Progress, Speech, saved_clip};
use crate::reader::tts::{audio, local, opus};
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Duration;
use tempfile::{NamedTempFile, TempPath};

const RATE: u64 = opus::RATE as u64;
pub(super) const CANCELLED: &str = "Export cancelled.";
pub(super) const OPUS_CHAPTER_LIMIT: usize = 1000;
const COVER_MAX_EDGE: u32 = 2048;

pub(super) struct Cover {
    jpeg: Vec<u8>,
    pub(super) width: u32,
    pub(super) height: u32,
}

impl Cover {
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let image = image::load_from_memory(bytes).ok()?;
        let image = if image.width().max(image.height()) > COVER_MAX_EDGE {
            image.resize(
                COVER_MAX_EDGE,
                COVER_MAX_EDGE,
                image::imageops::FilterType::Lanczos3,
            )
        } else {
            image
        };
        let (width, height) = (image.width(), image.height());
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
            .encode_image(&image.into_rgb8())
            .ok()?;
        Some(Self {
            jpeg,
            width,
            height,
        })
    }

    fn opus_comment(&self) -> String {
        // FLAC PICTURE layout for a 24-bit JPEG front cover (picture type 3).
        let mime = b"image/jpeg";
        let mut picture = Vec::with_capacity(32 + mime.len() + self.jpeg.len());
        picture.extend_from_slice(&3u32.to_be_bytes());
        picture.extend_from_slice(&(mime.len() as u32).to_be_bytes());
        picture.extend_from_slice(mime);
        for value in [0, self.width, self.height, 24, 0, self.jpeg.len() as u32] {
            picture.extend_from_slice(&value.to_be_bytes());
        }
        picture.extend_from_slice(&self.jpeg);
        local::base64(&picture)
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Format {
    #[default]
    M4b,
    Opus,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Self::M4b => "M4B (AAC)",
            Self::Opus => "Opus",
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Self::M4b => "m4b",
            Self::Opus => "opus",
        }
    }

    fn codec(self) -> &'static str {
        match self {
            Self::M4b => "aac",
            Self::Opus => "libopus",
        }
    }

    fn muxer(self) -> &'static str {
        match self {
            Self::M4b => "ipod",
            Self::Opus => "opus",
        }
    }

    fn bitrate(self) -> &'static str {
        match self {
            Self::M4b => "64k",
            Self::Opus => "48k",
        }
    }
}

pub(super) fn check_ffmpeg(ffmpeg: &Ffmpeg, format: Format) -> Result<(), String> {
    check_encoder(ffmpeg.command(), format)
}

fn check_encoder(mut cmd: Command, format: Format) -> Result<(), String> {
    let output = cmd.arg("-encoders").output().map_err(start_error)?;
    if !output.status.success() {
        return Err(format!(
            "Cannot list FFmpeg encoders: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let available = String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        let mut fields = line.split_whitespace();
        fields.next().is_some_and(|flags| flags.starts_with('A'))
            && fields.next() == Some(format.codec())
    });
    if available {
        Ok(())
    } else {
        Err(format!(
            "FFmpeg on PATH needs the {} audio encoder for {} export.",
            format.codec(),
            format.label()
        ))
    }
}

pub(super) fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Relaxed) {
        Err(CANCELLED.into())
    } else {
        Ok(())
    }
}

fn wait_for(child: &mut Child, cancel: &AtomicBool) -> Result<ExitStatus, String> {
    let result = loop {
        if let Err(e) = check_cancel(cancel) {
            break Err(e);
        }
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => break Err(format!("Cannot wait for FFmpeg: {e}")),
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn start_ffmpeg(cmd: &mut Command, cancel: &AtomicBool) -> Result<(Child, File), String> {
    check_cancel(cancel)?;
    cmd.stdout(Stdio::null());
    let log = tempfile::tempfile().map_err(|e| format!("Cannot open an FFmpeg log: {e}"))?;
    cmd.stderr(log.try_clone().map_err(|e| e.to_string())?);
    let child =
        hanstat::child_guard::spawn(cmd, |warning| eprintln!("{warning}")).map_err(start_error)?;
    Ok((child, log))
}

fn ffmpeg_log(log: &mut File, status: ExitStatus) -> String {
    let mut bytes = Vec::new();
    let _ = log
        .rewind()
        .and_then(|()| log.take(8192).read_to_end(&mut bytes));
    let message = String::from_utf8_lossy(&bytes);
    if message.trim().is_empty() {
        format!("FFmpeg exited with {status}.")
    } else {
        format!("FFmpeg: {}", message.trim())
    }
}

/// Only existing files explicitly approved by the user may be replaced.
pub(super) struct Destination {
    pub path: PathBuf,
    pub replace: bool,
}

impl Destination {
    pub fn exists(&self) -> Result<bool, String> {
        match fs::symlink_metadata(&self.path) {
            Ok(meta) if meta.is_file() => Ok(true),
            Ok(_) => Err(format!("{} is not a regular file.", self.path.display())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(format!("Cannot check {}: {e}", self.path.display())),
        }
    }

    fn check(&self) -> Result<bool, String> {
        let exists = self.exists()?;
        if exists && !self.replace {
            Err(format!("{} already exists.", self.path.display()))
        } else {
            Ok(exists)
        }
    }
}

pub(super) struct Files {
    pub audio: Destination,
    pub subtitles: Option<Destination>,
}

impl Files {
    pub fn check(&self) -> Result<(), String> {
        self.audio.check()?;
        if let Some(subtitles) = &self.subtitles {
            subtitles.check()?;
        }
        Ok(())
    }
}

pub(super) struct Render<'a> {
    pub ffmpeg: &'a Ffmpeg,
    pub title: &'a str,
    pub cover: Option<&'a Cover>,
    pub chapters: &'a [&'a Chapter],
    pub dir: &'a Path,
    pub speech: &'a Speech,
    pub format: Format,
    pub files: &'a Files,
    pub progress: &'a Progress,
}

impl Render<'_> {
    pub fn write(&self) -> Result<(), String> {
        let parent = self.files.audio.path.parent().unwrap_or(Path::new("."));
        let work = tempfile::Builder::new()
            .prefix(".hanstat-audiobook-")
            .tempdir_in(parent)
            .map_err(|e| format!("Cannot stage the audiobook in {}: {e}", parent.display()))?;
        let staged = work.path().join("encoded");
        let metadata = work.path().join("chapters.ffmetadata");
        let audio = NamedTempFile::new_in(work.path())
            .map_err(|e| format!("Cannot stage the audiobook: {e}"))?
            .into_temp_path();
        let subtitles = self
            .files
            .subtitles
            .as_ref()
            .map(|_| NamedTempFile::new_in(work.path()))
            .transpose()
            .map_err(|e| format!("Cannot stage the subtitles: {e}"))?;
        self.encode(&staged, &metadata, subtitles.as_ref())?;
        self.progress.finishing();
        let cover = if let Some(cover) = self.cover.filter(|_| self.format == Format::M4b) {
            let path = work.path().join("cover.jpg");
            fs::write(&path, &cover.jpeg).map_err(|e| format!("Cannot stage the cover: {e}"))?;
            Some(path)
        } else {
            None
        };
        self.remux(&staged, &metadata, &audio, cover.as_deref())?;
        check_cancel(&self.progress.batch.cancel)?;
        let mut outputs = Vec::with_capacity(2);
        if let (Some(file), Some(dest)) = (subtitles, &self.files.subtitles) {
            outputs.push((file.into_temp_path(), dest));
        }
        outputs.push((audio, &self.files.audio));
        publish(outputs, work)
    }

    fn encode(
        &self,
        staged: &Path,
        metadata: &Path,
        subtitles: Option<&NamedTempFile>,
    ) -> Result<(), String> {
        let cancel = &self.progress.batch.cancel;
        let mut cmd = self.ffmpeg.command();
        cmd.args(["-loglevel", "error", "-y", "-f", "s16le", "-ar"])
            .arg(RATE.to_string())
            .args(["-ac", "1", "-i", "pipe:0", "-map", "0:a:0", "-c:a"])
            .arg(self.format.codec())
            .args(["-b:a", self.format.bitrate(), "-f", self.format.muxer()])
            .arg(staged)
            .stdin(Stdio::piped());
        let (mut child, mut log) = start_ffmpeg(&mut cmd, cancel)?;
        let input = child.stdin.take().expect("piped FFmpeg stdin");
        let (status, result) = std::thread::scope(|scope| {
            let writer = scope.spawn(move || self.write_pcm(input, metadata, subtitles));
            let status = wait_for(&mut child, cancel);
            (
                status,
                writer
                    .join()
                    .map_err(|_| "The audio writer stopped unexpectedly."),
            )
        });
        let status = status?;
        match result? {
            Err(RenderError::Pipe(e)) if status.success() => {
                Err(format!("Cannot send audio to FFmpeg: {e}"))
            }
            Err(RenderError::Other(e)) => Err(e),
            _ if !status.success() => Err(ffmpeg_log(&mut log, status)),
            _ => Ok(()),
        }
    }

    fn remux(
        &self,
        staged: &Path,
        metadata: &Path,
        output: &Path,
        cover: Option<&Path>,
    ) -> Result<(), String> {
        let mut cmd = self.ffmpeg.command();
        cmd.args(["-loglevel", "error", "-y", "-i"])
            .arg(staged)
            .args(["-f", "ffmetadata", "-i"])
            .arg(metadata);
        if let Some(cover) = cover {
            cmd.arg("-i").arg(cover);
        }
        cmd.args(["-map", "0:a:0", "-map_metadata", "1", "-map_chapters"])
            .arg(if self.format == Format::M4b {
                "1"
            } else {
                "-1"
            })
            .args(["-c:a", "copy"]);
        if cover.is_some() {
            cmd.args([
                "-map",
                "2:v:0",
                "-c:v",
                "copy",
                "-disposition:v:0",
                "attached_pic",
            ]);
        }
        if self.format == Format::M4b {
            cmd.args(["-movflags", "+faststart"]);
        }
        cmd.args(["-f", self.format.muxer()])
            .arg(output)
            .stdin(Stdio::null());
        let (mut child, mut log) = start_ffmpeg(&mut cmd, &self.progress.batch.cancel)?;
        let status = wait_for(&mut child, &self.progress.batch.cancel)?;
        if status.success() {
            Ok(())
        } else {
            Err(ffmpeg_log(&mut log, status))
        }
    }

    fn write_pcm(
        &self,
        mut input: impl Write,
        metadata: &Path,
        subtitles: Option<&NamedTempFile>,
    ) -> Result<(), RenderError> {
        let mut subtitles = subtitles.map(|file| BufWriter::new(file.as_file()));
        let mut start = 0;
        let mut cue = 0;
        let mut meta = format!(";FFMETADATA1\ntitle={}\n", escape_metadata(self.title));
        if let Some(cover) = self.cover.filter(|_| self.format == Format::Opus) {
            meta.push_str("METADATA_BLOCK_PICTURE=");
            meta.push_str(&cover.opus_comment());
            meta.push('\n');
        }
        let prefix = self.speech.key_prefix();
        let mut key_buf = String::with_capacity(prefix.len() + 128);
        key_buf.push_str(&prefix);
        let prefix_len = key_buf.len();
        let mut file_buf = Vec::new();
        let mut samples_buf = Vec::new();
        let mut resampled_buf = Vec::new();
        let mut pcm_bytes = Vec::new();
        for (index, chapter) in self.chapters.iter().enumerate() {
            let chapter_start = start;
            for sentence in &chapter.sentences {
                check_cancel(&self.progress.batch.cancel)?;
                key_buf.truncate(prefix_len);
                key_buf.push_str(&sentence.text);
                let key = hanstat::dict::xxh64::xxh64(key_buf.as_bytes(), 0);
                let path = saved_clip(self.dir, key, self.speech.ext()).ok_or_else(|| {
                    format!(
                        "Audio is missing in “{}”. Try exporting again.",
                        chapter.title
                    )
                })?;
                file_buf.clear();
                File::open(&path)
                    .and_then(|mut f| f.read_to_end(&mut file_buf))
                    .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
                let rate = self
                    .speech
                    .decode_into(&mut file_buf, &mut samples_buf)
                    .map_err(|e| format!("Cannot decode the clip {}: {e}", path.display()))?;
                let samples: &[f32] = if rate == RATE as u32 {
                    &samples_buf
                } else {
                    audio::resample_into(&samples_buf, rate, RATE as u32, &mut resampled_buf);
                    &resampled_buf
                };
                if samples.is_empty() {
                    return Err(format!("The saved clip {} is empty.", path.display()).into());
                }
                pcm_bytes.clear();
                pcm_bytes.reserve(samples.len() * 2);
                for &sample in samples {
                    pcm_bytes.extend_from_slice(&((sample * 32768.0).round() as i16).to_le_bytes());
                }
                input.write_all(&pcm_bytes).map_err(RenderError::Pipe)?;
                let end = start + samples.len() as u64;
                cue += 1;
                if let Some(subs) = subtitles.as_mut() {
                    write_cue(subs, cue, start, end, &sentence.text)
                        .map_err(|e| format!("Cannot write subtitles: {e}"))?;
                }
                self.progress.enc_done.store(cue, Relaxed);
                start = end;
            }
            let title = escape_metadata(&chapter.title);
            if self.format == Format::M4b {
                meta.push_str(&format!(
                    "[CHAPTER]\nTIMEBASE=1/{RATE}\nSTART={chapter_start}\nEND={start}\ntitle={title}\n",
                ));
            } else {
                // Write the Vorbis comments explicitly. Some FFmpeg builds
                // round the seconds and milliseconds separately when they
                // convert chapters, moving some timestamps a second ahead.
                let time = timestamp(milliseconds(chapter_start), '.');
                meta.push_str(&format!(
                    "CHAPTER{index:03}={time}\nCHAPTER{index:03}NAME={title}\n"
                ));
            }
        }
        if let Some(mut subs) = subtitles {
            subs.flush()
                .map_err(|e| format!("Cannot write subtitles: {e}"))?;
        }
        fs::write(metadata, meta).map_err(|e| format!("Cannot write chapter metadata: {e}"))?;
        Ok(())
    }
}

enum RenderError {
    Pipe(io::Error),
    Other(String),
}

impl From<String> for RenderError {
    fn from(error: String) -> Self {
        Self::Other(error)
    }
}

fn escape_metadata(text: &str) -> String {
    let mut result = String::new();
    for c in text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
    {
        if matches!(c, '=' | ';' | '#' | '\\') {
            result.push('\\');
        }
        if !c.is_control() {
            result.push(c);
        }
    }
    // FFmpeg reads a newline following a backslash as a continuation,
    // even when the backslash itself is escaped.
    if result.ends_with('\\') {
        result.push(' ');
    }
    result
}

fn milliseconds(samples: u64) -> u64 {
    samples / (RATE / 1000) + u64::from(samples % (RATE / 1000) >= RATE / 2000)
}

fn timestamp(ms: u64, decimal: char) -> String {
    format!(
        "{:02}:{:02}:{:02}{decimal}{:03}",
        ms / 3_600_000,
        (ms / 60_000) % 60,
        (ms / 1000) % 60,
        ms % 1000
    )
}

fn write_cue(
    out: &mut impl Write,
    index: usize,
    start: u64,
    end: u64,
    text: &str,
) -> io::Result<()> {
    let start = milliseconds(start);
    let end = milliseconds(end).max(start + 1);
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    writeln!(
        out,
        "{index}\n{} --> {}\n{text}\n",
        timestamp(start, ','),
        timestamp(end, ',')
    )
}

/// Keep originals until every staged output is in place. Roll back prior
/// publications if the next destination cannot be installed.
fn publish(outputs: Vec<(TempPath, &Destination)>, work: tempfile::TempDir) -> Result<(), String> {
    let mut published = Vec::<(&Destination, Option<PathBuf>)>::new();
    let result = (|| {
        for (i, (file, dest)) in outputs.into_iter().enumerate() {
            let backup = if dest.check()? {
                let backup = work.path().join(format!("original-{i}"));
                fs::hard_link(&dest.path, &backup)
                    .or_else(|_| fs::copy(&dest.path, &backup).map(|_| ()))
                    .map_err(|e| format!("Cannot back up {}: {e}", dest.path.display()))?;
                Some(backup)
            } else {
                None
            };
            if backup.is_some() {
                file.persist(&dest.path)
            } else {
                file.persist_noclobber(&dest.path)
            }
            .map_err(|e| format!("Cannot write {}: {}", dest.path.display(), e.error))?;
            published.push((dest, backup));
        }
        Ok(())
    })();
    if let Err(error) = result {
        let mut restore_errors = Vec::new();
        for (dest, backup) in published.into_iter().rev() {
            let restored = match backup {
                Some(backup) => fs::rename(backup, &dest.path),
                None => fs::remove_file(&dest.path),
            };
            if let Err(e) = restored {
                restore_errors.push(format!("Cannot restore {}: {e}", dest.path.display()));
            }
        }
        if !restore_errors.is_empty() {
            return Err(format!(
                "{} {} Recovery files are in {}.",
                error,
                restore_errors.join(" "),
                work.keep().display()
            ));
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtitles_use_the_same_sample_boundary_after_multiple_hours() {
        let start = 24_000 * (3 * 3600 + 2 * 60 + 3) + 240;
        let mut cues = Vec::new();
        write_cue(&mut cues, 1, start, start + 24_013, "第一句。").unwrap();
        write_cue(&mut cues, 2, start + 24_013, start + 47_654, "第二句。").unwrap();
        assert_eq!(
            String::from_utf8(cues).unwrap(),
            "1\n03:02:03,010 --> 03:02:04,011\n第一句。\n\n\
             2\n03:02:04,011 --> 03:02:04,996\n第二句。\n\n"
        );
    }

    #[test]
    fn missing_ffmpeg_has_an_actionable_error() {
        let work = tempfile::tempdir().unwrap();
        let error =
            check_encoder(Command::new(work.path().join("ffmpeg")), Format::Opus).unwrap_err();
        assert!(error.contains("FFmpeg was not found"));
    }

    #[test]
    fn failed_publication_restores_a_replaced_sidecar() {
        let parent = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir_in(parent.path()).unwrap();
        let subtitles = Destination {
            path: parent.path().join("book.srt"),
            replace: true,
        };
        let audio = Destination {
            path: parent.path().join("book.m4b"),
            replace: false,
        };
        fs::write(&subtitles.path, b"original subtitles").unwrap();
        fs::write(&audio.path, b"file that arrived after confirmation").unwrap();
        let make_stage = |text: &[u8]| {
            let mut file = NamedTempFile::new_in(work.path()).unwrap();
            file.write_all(text).unwrap();
            file.into_temp_path()
        };
        let files = vec![
            (make_stage(b"new subtitles"), &subtitles),
            (make_stage(b"new audio"), &audio),
        ];
        let error = publish(files, work).unwrap_err();
        assert!(error.contains("already exists"));
        assert_eq!(fs::read(&subtitles.path).unwrap(), b"original subtitles");
        assert_eq!(
            fs::read(&audio.path).unwrap(),
            b"file that arrived after confirmation"
        );
    }
}
