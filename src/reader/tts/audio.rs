//! Sound: MP3 decoded with symphonia, played through cpal on the default
//! output device (WASAPI, CoreAudio, ALSA/PulseAudio/PipeWire).
//!
//! Clips (one per sentence) are queued back to back, so sentences follow
//! each other without a gap; the device's callback says which clip it is
//! playing, which is what the page highlights.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use std::collections::VecDeque;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Mono samples from MP3 bytes, and their rate.
pub fn decode_mp3(bytes: Vec<u8>) -> Result<(Vec<f32>, u32), String> {
    let mut out = Vec::new();
    let rate = decode_mp3_into(bytes, &mut out)?;
    Ok((out, rate))
}

/// Decode MP3 bytes into `out` (reusing its allocation), returning the sample rate.
pub(super) fn decode_mp3_into(bytes: Vec<u8>, out: &mut Vec<f32>) -> Result<u32, String> {
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::errors::Error;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

    out.clear();
    let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let bad = |e: Error| format!("The audio can't be read: {e}");
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(bad)?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or("The audio has no sound track")?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or("The audio has no sound track")?
        .clone();
    let id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(bad)?;
    let mut rate = params.sample_rate.unwrap_or(24_000);
    let mut buf: Vec<f32> = Vec::new();
    while let Some(packet) = format.next_packet().map_err(bad)? {
        if packet.track_id != id {
            continue;
        }
        let audio = match decoder.decode(&packet) {
            Ok(a) => a,
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(bad(e)),
        };
        rate = audio.spec().rate();
        let channels = audio.spec().channels().count().max(1);
        buf.resize(audio.samples_interleaved(), 0.0);
        audio.copy_to_slice_interleaved(&mut buf);
        if channels == 1 {
            out.extend_from_slice(&buf);
        } else {
            out.extend(
                buf.chunks_exact(channels)
                    .map(|f| f.iter().sum::<f32>() / channels as f32),
            );
        }
    }
    Ok(rate)
}

/// `input` at `from` Hz as `to` Hz (linear: speech at 24 kHz to a device's
/// 44.1 or 48 kHz).
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    let mut out = Vec::new();
    resample_into(input, from, to, &mut out);
    out
}

/// Resample `input` from `from` Hz to `to` Hz into `out` (reusing its allocation).
pub(super) fn resample_into(input: &[f32], from: u32, to: u32, out: &mut Vec<f32>) {
    out.clear();
    if from == to || input.is_empty() {
        out.extend_from_slice(input);
        return;
    }
    let n = (input.len() as u64 * to as u64 / from as u64) as usize;
    let step = from as f64 / to as f64;
    out.reserve(n);
    for i in 0..n {
        let t = i as f64 * step;
        let k = t as usize;
        let f = (t - k as f64) as f32;
        let a = input[k.min(input.len() - 1)];
        let b = input[(k + 1).min(input.len() - 1)];
        out.push(a + (b - a) * f);
    }
}

/// Owned [`resample`]: returns `input` without copying when `from == to` or empty.
pub(super) fn resample_owned(input: Vec<f32>, from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input;
    }
    resample(&input, from, to)
}

/// Tests run the real device silently.
#[cfg(test)]
pub static MUTE: AtomicBool = AtomicBool::new(false);

fn stretch_window(rate: u32) -> usize {
    (((rate as f32 * 0.025) as usize).max(16)) & !1
}

fn stretch_noop(len: usize, n: usize, speed: f32) -> bool {
    (speed - 1.0).abs() < 0.005 || !speed.is_finite() || speed <= 0.0 || len < 2 * n
}

/// `input` (mono, `rate` Hz) played `speed` times as fast, at the same
/// pitch: WSOLA (overlap-add of 25 ms Hann windows at half-window steps,
/// each taken within ±8 ms of where it would be where it best continues the
/// last one). 1.0 gives the samples unchanged.
#[cfg(test)]
pub fn stretch(input: &[f32], rate: u32, speed: f32) -> Vec<f32> {
    let n = stretch_window(rate);
    if stretch_noop(input.len(), n, speed) {
        return input.to_vec();
    }
    stretch_core(input, rate, speed, n)
}

/// Owned [`stretch`]: returns `input` without copying when `speed` is ~1.0 or
/// the clip is too short to stretch.
pub(super) fn stretch_owned(input: Vec<f32>, rate: u32, speed: f32) -> Vec<f32> {
    let n = stretch_window(rate);
    if stretch_noop(input.len(), n, speed) {
        return input;
    }
    stretch_core(&input, rate, speed, n)
}

fn stretch_core(input: &[f32], rate: u32, speed: f32, n: usize) -> Vec<f32> {
    let hs = n / 2;
    let out_len = (input.len() as f64 / speed as f64).round() as usize;
    let ha = hs as f64 * speed as f64;
    let delta = (rate as f32 * 0.008) as isize;
    let win: Vec<f32> = (0..n)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos())
        .collect();
    let get = |i: isize| -> f32 {
        if i >= 0 && (i as usize) < input.len() {
            input[i as usize]
        } else {
            0.0
        }
    };
    let mut out = vec![0f32; out_len + n];
    let mut norm = vec![0f32; out_len + n];
    let mut prev: isize = 0;
    for k in 0..=out_len / hs {
        let nominal = (k as f64 * ha).round() as isize;
        let pos = if k == 0 {
            0
        } else {
            // What the last window's second half went on to: the new
            // window's first half should look like it.
            let target = prev + hs as isize;
            let mut best = nominal;
            let mut best_c = f32::MIN;
            let mut off = -delta;
            while off <= delta {
                let cand = nominal + off;
                let (mut c, mut e) = (0.0f32, 1e-9f32);
                let mut j = 0;
                while j < hs {
                    let a = get(cand + j as isize);
                    c += a * get(target + j as isize);
                    e += a * a;
                    j += 2;
                }
                let c = c / e.sqrt();
                if c > best_c {
                    best_c = c;
                    best = cand;
                }
                off += 2;
            }
            best
        };
        let o = k * hs;
        for (j, w) in win.iter().enumerate() {
            out[o + j] += get(pos + j as isize) * w;
            norm[o + j] += w;
        }
        prev = pos;
    }
    out.truncate(out_len);
    for (s, w) in out.iter_mut().zip(&norm) {
        if *w > 1e-3 {
            *s /= w;
        }
    }
    out
}

/// Seconds over which sound fades out (pause, stop, a jump to another
/// sentence) and back in (resume): no click where the wave is cut.
const FADE_OUT: f32 = 0.06;
const FADE_IN: f32 = 0.04;

struct Clip {
    id: u64,
    samples: Vec<f32>,
    pos: usize,
    /// The samples between the silence before and after the voice.
    voiced: Range<usize>,
}

/// Quieter than this is silence (about -46 dBFS).
const SILENCE: f32 = 0.005;

/// The samples from the first to the last one louder than silence (all of
/// them if none is).
fn voiced(samples: &[f32]) -> Range<usize> {
    let loud = |s: &f32| s.abs() > SILENCE;
    match (
        samples.iter().position(loud),
        samples.iter().rposition(loud),
    ) {
        (Some(a), Some(z)) => a..z + 1,
        _ => 0..samples.len(),
    }
}

struct Queue {
    clips: VecDeque<Clip>,
    /// Finished or cut clips waiting to be dropped off the audio thread.
    retired: Vec<Clip>,
    paused: bool,
    /// Clips below this id are being cut: faded out, then dropped.
    cut_below: u64,
    /// The volume now, from 0 (silent) to 1, moving towards 0 while paused
    /// or cutting and towards 1 otherwise.
    gain: f32,
    /// The clip being heard (the last one heard once the queue runs dry).
    current: Option<u64>,
    /// Clips played to their end since asked.
    finished: Vec<u64>,
}

impl Default for Queue {
    fn default() -> Self {
        Queue {
            clips: VecDeque::with_capacity(8),
            retired: Vec::with_capacity(8),
            paused: false,
            cut_below: 0,
            gain: 1.0,
            current: None,
            finished: Vec::with_capacity(16),
        }
    }
}

impl Queue {
    /// Fade out and drop every clip queued; the id the next one needs.
    fn cut(&mut self) -> u64 {
        let next = self.clips.iter().map(|c| c.id + 1).max().unwrap_or(0);
        self.cut_below = self.cut_below.max(next);
        self.current = None;
        self.finished.clear();
        next
    }

    fn take_retired(&mut self) -> Vec<Clip> {
        if self.retired.is_empty() {
            Vec::new()
        } else {
            let mut out = Vec::with_capacity(8);
            std::mem::swap(&mut self.retired, &mut out);
            out
        }
    }
}

/// The sound device, open while reading aloud.
pub struct Output {
    queue: Arc<Mutex<Queue>>,
    failed: Arc<AtomicBool>,
    pub rate: u32,
    _stream: cpal::Stream,
}

/// A smooth step: the fade's volume for `g` from 0 to 1, flat at both ends.
fn smooth(g: f32) -> f32 {
    g * g * (3.0 - 2.0 * g)
}

fn fill<T: SizedSample + FromSample<f32>>(
    out: &mut [T],
    channels: usize,
    rate: u32,
    queue: &Mutex<Queue>,
    wake: &(dyn Fn() + Send + Sync),
) {
    let mut q = match queue.lock() {
        Ok(q) => q,
        Err(_) => {
            out.fill(T::EQUILIBRIUM);
            return;
        }
    };
    let down = 1.0 / (FADE_OUT * rate as f32);
    let up = 1.0 / (FADE_IN * rate as f32);
    let mut ended = false;
    for frame in out.chunks_mut(channels) {
        let cutting = q.clips.front().is_some_and(|c| c.id < q.cut_below);
        if q.paused || cutting {
            q.gain = (q.gain - down).max(0.0);
        } else {
            q.gain = (q.gain + up).min(1.0);
        }
        if cutting && q.gain == 0.0 {
            // Faded out: what was cut moves to `retired` (dropped off the audio thread).
            let cut = q.cut_below;
            let mut i = 0;
            while i < q.clips.len() {
                if q.clips[i].id < cut {
                    if let Some(c) = q.clips.remove(i) {
                        q.retired.push(c);
                    }
                } else {
                    i += 1;
                }
            }
        }
        // Paused and silent: the sound waits where it stopped.
        let s = if q.paused && q.gain == 0.0 {
            0.0
        } else {
            loop {
                let cut = q.cut_below;
                let Some(c) = q.clips.front_mut() else {
                    break 0.0;
                };
                if c.pos < c.samples.len() {
                    let s = c.samples[c.pos];
                    c.pos += 1;
                    let id = c.id;
                    if id >= cut {
                        q.current = Some(id);
                    }
                    break s;
                }
                let id = c.id;
                if let Some(done) = q.clips.pop_front() {
                    q.retired.push(done);
                }
                if id >= cut {
                    q.finished.push(id);
                    ended = true;
                }
            }
        };
        let s = s * smooth(q.gain);
        #[cfg(test)]
        let s = if MUTE.load(Ordering::Relaxed) { 0.0 } else { s };
        let v = T::from_sample(s);
        frame.fill(v);
    }
    drop(q);
    if ended {
        wake();
    }
}

impl Output {
    /// Open the default output device. `wake` is called (from the audio
    /// thread) when a clip ends.
    pub fn open(wake: Arc<dyn Fn() + Send + Sync>) -> Result<Output, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("No sound output device")?;
        let supported = device
            .default_output_config()
            .map_err(|e| format!("The sound device can't be used: {e}"))?;
        let format = supported.sample_format();
        let config = supported.config();
        let channels = config.channels.max(1) as usize;
        let rate = config.sample_rate;
        let queue = Arc::new(Mutex::new(Queue::default()));
        let failed = Arc::new(AtomicBool::new(false));
        let err = {
            let failed = failed.clone();
            let wake = wake.clone();
            move |_e: cpal::Error| {
                failed.store(true, Ordering::Relaxed);
                wake();
            }
        };
        macro_rules! build {
            ($t:ty) => {{
                let (queue, wake) = (queue.clone(), wake.clone());
                device.build_output_stream::<$t, _, _>(
                    config.clone(),
                    move |out: &mut [$t], _| fill(out, channels, rate, &queue, &*wake),
                    err,
                    None,
                )
            }};
        }
        use cpal::SampleFormat as F;
        let stream = match format {
            F::F32 => build!(f32),
            F::F64 => build!(f64),
            F::I16 => build!(i16),
            F::U16 => build!(u16),
            F::I32 => build!(i32),
            F::U32 => build!(u32),
            F::I8 => build!(i8),
            F::U8 => build!(u8),
            f => return Err(format!("The sound device's format ({f}) isn't supported")),
        }
        .map_err(|e| format!("The sound device can't be opened: {e}"))?;
        stream
            .play()
            .map_err(|e| format!("The sound device can't play: {e}"))?;
        Ok(Output {
            queue,
            failed,
            rate,
            _stream: stream,
        })
    }

    /// Play `samples` (mono, at [`Output::rate`]) after what is queued.
    pub fn push(&self, id: u64, samples: Vec<f32>) {
        self.push_at(id, samples, 0);
    }

    /// Play `samples` from sample `pos` on (the rest of a clip heard partly).
    pub fn push_at(&self, id: u64, samples: Vec<f32>, pos: usize) {
        let voiced = voiced(&samples);
        let pos = pos.min(samples.len());
        let retired = {
            let mut q = self.queue.lock().unwrap();
            q.clips.push_back(Clip {
                id,
                samples,
                pos,
                voiced,
            });
            q.take_retired()
        };
        drop(retired);
    }

    /// The clip being heard and how far into it it is (0 to 1).
    pub fn progress(&self) -> Option<(u64, f32)> {
        let q = self.queue.lock().unwrap();
        let id = q.current?;
        q.clips
            .iter()
            .find(|c| c.id == id && c.id >= q.cut_below && !c.samples.is_empty())
            .map(|c| (id, c.pos as f32 / c.samples.len() as f32))
    }

    /// Stop and forget everything queued: what is heard fades out first.
    pub fn clear(&self) {
        let retired = {
            let mut q = self.queue.lock().unwrap();
            q.cut();
            q.take_retired()
        };
        drop(retired);
    }

    pub fn set_paused(&self, paused: bool) {
        self.queue.lock().unwrap().paused = paused;
    }

    /// Clips queued, the one playing included (not those being cut).
    pub fn queued(&self) -> Vec<u64> {
        let (ids, retired) = {
            let mut q = self.queue.lock().unwrap();
            let ids = q
                .clips
                .iter()
                .filter(|c| c.id >= q.cut_below)
                .map(|c| c.id)
                .collect();
            let retired = q.take_retired();
            (ids, retired)
        };
        drop(retired);
        ids
    }

    /// The clip being heard.
    pub fn current(&self) -> Option<u64> {
        self.queue.lock().unwrap().current
    }

    /// The clip being heard, how many of its samples were played, and
    /// where its voice is (samples).
    pub fn position(&self) -> Option<(u64, usize, Range<usize>)> {
        let q = self.queue.lock().unwrap();
        let id = q.current?;
        q.clips
            .iter()
            .find(|c| c.id == id && c.id >= q.cut_below && c.pos < c.samples.len())
            .map(|c| (id, c.pos, c.voiced.clone()))
    }

    /// Seconds of sound queued and not yet heard.
    pub fn ahead(&self) -> f32 {
        let q = self.queue.lock().unwrap();
        let n: usize = q
            .clips
            .iter()
            .filter(|c| c.id >= q.cut_below)
            .map(|c| c.samples.len() - c.pos)
            .sum();
        n as f32 / self.rate as f32
    }

    /// Clips finished since the last call.
    pub fn take_finished(&self) -> Vec<u64> {
        let (finished, retired) = {
            let mut q = self.queue.lock().unwrap();
            let finished = if q.finished.is_empty() {
                Vec::new()
            } else {
                let mut next = Vec::with_capacity(16);
                std::mem::swap(&mut q.finished, &mut next);
                next
            };
            let retired = q.take_retired();
            (finished, retired)
        };
        drop(retired);
        finished
    }

    /// The device went away (unplugged, or changed): open it again.
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, rate: u32, secs: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize)
            .map(|i| (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    fn crossings(x: &[f32]) -> usize {
        x.windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count()
    }

    #[test]
    fn stretching_changes_length_not_pitch() {
        let rate = 24_000;
        let x = sine(440.0, rate, 2.0);
        assert_eq!(stretch(&x, rate, 1.0), x, "normal speed: unchanged");
        for speed in [0.5f32, 0.75, 1.25, 1.5, 2.0] {
            let y = stretch(&x, rate, speed);
            let want = (x.len() as f32 / speed).round() as usize;
            assert_eq!(y.len(), want, "{speed}");
            // The same pitch: crossings per second as before (within 2%),
            // away from the ends.
            let mid = &y[y.len() / 10..y.len() * 9 / 10];
            let hz = crossings(mid) as f32 / 2.0 / (mid.len() as f32 / rate as f32);
            assert!((hz - 440.0).abs() < 9.0, "{speed}: {hz} Hz");
            // No louder than the input, and not much quieter.
            let peak = mid.iter().fold(0f32, |m, s| m.max(s.abs()));
            assert!((0.4..=0.55).contains(&peak), "{speed}: peak {peak}");
        }
        // A blip too short to stretch stays as it is.
        let blip = sine(440.0, rate, 0.02);
        assert_eq!(stretch(&blip, rate, 1.5), blip);
    }

    /// A saved clip (`HANSTAT_STRETCH_IN`, .mp3 or .opus) stretched to each
    /// playback speed, as WAVs in `HANSTAT_STRETCH_OUT`: to listen to.
    #[test]
    #[ignore = "needs a clip, writes WAVs to listen to"]
    fn stretch_samples() {
        let input = std::path::PathBuf::from(std::env::var_os("HANSTAT_STRETCH_IN").unwrap());
        let out = std::path::PathBuf::from(std::env::var_os("HANSTAT_STRETCH_OUT").unwrap());
        std::fs::create_dir_all(&out).unwrap();
        let bytes = std::fs::read(&input).unwrap();
        let (x, rate) = if input.extension().is_some_and(|e| e == "opus") {
            super::super::opus::decode(bytes).unwrap()
        } else {
            decode_mp3(bytes).unwrap()
        };
        for speed in [0.5f32, 0.75, 1.0, 1.25, 1.5, 2.0] {
            let t = std::time::Instant::now();
            let y = stretch(&x, rate, speed);
            eprintln!(
                "{speed}×: {:.1} s of sound in {:?}",
                y.len() as f32 / rate as f32,
                t.elapsed()
            );
            let mut w = Vec::new();
            let data = (y.len() * 2) as u32;
            w.extend_from_slice(b"RIFF");
            w.extend_from_slice(&(36 + data).to_le_bytes());
            w.extend_from_slice(b"WAVEfmt ");
            w.extend_from_slice(&16u32.to_le_bytes());
            w.extend_from_slice(&1u16.to_le_bytes());
            w.extend_from_slice(&1u16.to_le_bytes());
            w.extend_from_slice(&rate.to_le_bytes());
            w.extend_from_slice(&(rate * 2).to_le_bytes());
            w.extend_from_slice(&2u16.to_le_bytes());
            w.extend_from_slice(&16u16.to_le_bytes());
            w.extend_from_slice(b"data");
            w.extend_from_slice(&data.to_le_bytes());
            for s in y {
                w.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
            }
            std::fs::write(out.join(format!("speed-{speed:.2}.wav")), w).unwrap();
        }
    }

    #[test]
    fn a_clip_can_start_part_way() {
        let out = Mutex::new(Queue::default());
        out.lock().unwrap().clips.push_back(Clip {
            id: 1,
            samples: vec![0.5; 100],
            pos: 40,
            voiced: 0..100,
        });
        let mut buf = vec![0f32; 70];
        fill(&mut buf, 1, 48_000, &out, &|| {});
        assert_eq!(out.lock().unwrap().finished, vec![1], "60 left, then done");
    }

    #[test]
    fn resampling_keeps_length_and_ends() {
        let x: Vec<f32> = (0..240).map(|i| i as f32).collect();
        let y = resample(&x, 24_000, 48_000);
        assert_eq!(y.len(), 480);
        assert_eq!(y[0], 0.0);
        assert!((y[1] - 0.5).abs() < 1e-6);
        assert_eq!(resample(&x, 24_000, 24_000), x);
        assert_eq!(resample(&x, 24_000, 44_100).len(), 441);
    }

    #[test]
    fn queue_plays_clips_in_order() {
        let queue = Mutex::new(Queue::default());
        {
            let mut q = queue.lock().unwrap();
            for (id, n) in [(1u64, 3usize), (2, 2)] {
                q.clips.push_back(Clip {
                    id,
                    samples: vec![id as f32; n],
                    pos: 0,
                    voiced: 0..n,
                });
            }
        }
        let woke = AtomicBool::new(false);
        let wake = || woke.store(true, Ordering::Relaxed);
        let mut out = [0f32; 8];
        fill(&mut out, 2, 48_000, &queue, &wake);
        assert_eq!(out, [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 2.0, 2.0]);
        assert!(woke.load(Ordering::Relaxed));
        let q = queue.lock().unwrap();
        assert_eq!(q.finished, [1]);
        assert_eq!(q.current, Some(2));
    }

    fn queue_of(clips: &[(u64, f32, usize)]) -> Mutex<Queue> {
        let mut q = Queue::default();
        for &(id, v, n) in clips {
            q.clips.push_back(Clip {
                id,
                samples: vec![v; n],
                pos: 0,
                voiced: 0..n,
            });
        }
        Mutex::new(q)
    }

    /// 1 kHz: a fade of 60 frames out, 40 in.
    const RATE: u32 = 1000;

    #[test]
    fn pausing_fades_out_and_resuming_fades_in() {
        let queue = queue_of(&[(1, 1.0, 1000)]);
        let wake = || {};
        let mut out = [0f32; 10];
        fill(&mut out, 1, RATE, &queue, &wake);
        assert_eq!(out, [1.0; 10]);
        queue.lock().unwrap().paused = true;
        let mut out = [0f32; 100];
        fill(&mut out, 1, RATE, &queue, &wake);
        // Down smoothly, never a jump, to silence after 60 frames.
        assert!(
            out.windows(2).all(|w| w[1] <= w[0] && w[0] - w[1] < 0.03),
            "{out:?}"
        );
        assert!(out[0] > 0.99 && out[30] > 0.3 && out[30] < 0.7);
        assert!(out[60..].iter().all(|&s| s == 0.0));
        // The sound under the fade was played; then nothing more.
        assert_eq!(queue.lock().unwrap().clips[0].pos, 10 + 60);
        queue.lock().unwrap().paused = false;
        let mut out = [0f32; 50];
        fill(&mut out, 1, RATE, &queue, &wake);
        assert!(
            out.windows(2).all(|w| w[1] >= w[0] && w[1] - w[0] < 0.05),
            "{out:?}"
        );
        assert!(out[0] < 0.01 && out[45] == 1.0);
    }

    #[test]
    fn voice_is_found_between_silences() {
        let mut x = vec![0.0f32; 100];
        x[20] = 0.3;
        x[70] = -0.2;
        x[90] = 0.001;
        assert_eq!(voiced(&x), 20..71);
        assert_eq!(voiced(&[0.0; 5]), 0..5);
        assert_eq!(voiced(&[]), 0..0);
    }

    #[test]
    fn a_cut_fades_out_before_the_next_clip() {
        let queue = queue_of(&[(1, 1.0, 1000)]);
        let wake = || {};
        let mut out = [0f32; 10];
        fill(&mut out, 1, RATE, &queue, &wake);
        let o = queue.lock().unwrap().cut();
        queue.lock().unwrap().clips.push_back(Clip {
            id: o,
            samples: vec![-1.0; 1000],
            pos: 0,
            voiced: 0..1000,
        });
        let mut out = [0f32; 120];
        fill(&mut out, 1, RATE, &queue, &wake);
        // The old clip fades out from 1, the new one fades in towards -1.
        assert!(out[0] > 0.99);
        let low = out.iter().position(|&s| s <= 0.0).unwrap();
        assert!((55..=62).contains(&low), "faded out at {low}");
        assert!(
            out.windows(2).all(|w| (w[1] - w[0]).abs() < 0.06),
            "{out:?}"
        );
        assert_eq!(out[119], -1.0);
        let q = queue.lock().unwrap();
        assert_eq!(q.clips.len(), 1);
        assert_eq!(q.current, Some(o));
        assert!(q.finished.is_empty(), "a cut clip isn't reported as heard");
    }
}
