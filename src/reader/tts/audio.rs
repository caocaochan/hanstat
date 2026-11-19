//! Sound: MP3 decoded with symphonia, played through cpal on the default
//! output device (WASAPI, CoreAudio, ALSA/PulseAudio/PipeWire).
//!
//! Clips (one per sentence) are queued back to back, so sentences follow
//! each other without a gap; the device's callback says which clip it is
//! playing, which is what the page highlights.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Mono samples from MP3 bytes, and their rate.
pub fn decode_mp3(bytes: Vec<u8>) -> Result<(Vec<f32>, u32), String> {
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::errors::Error;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

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
    let mut out = Vec::new();
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
    Ok((out, rate))
}

/// `input` at `from` Hz as `to` Hz (linear: speech at 24 kHz to a device's
/// 44.1 or 48 kHz).
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let n = (input.len() as u64 * to as u64 / from as u64) as usize;
    let step = from as f64 / to as f64;
    (0..n)
        .map(|i| {
            let t = i as f64 * step;
            let k = t as usize;
            let f = (t - k as f64) as f32;
            let a = input[k.min(input.len() - 1)];
            let b = input[(k + 1).min(input.len() - 1)];
            a + (b - a) * f
        })
        .collect()
}

/// Tests run the real device silently.
#[cfg(test)]
pub static MUTE: AtomicBool = AtomicBool::new(false);

/// Seconds over which sound fades out (pause, stop, a jump to another
/// sentence) and back in (resume): no click where the wave is cut.
const FADE_OUT: f32 = 0.06;
const FADE_IN: f32 = 0.04;

struct Clip {
    id: u64,
    samples: Vec<f32>,
    pos: usize,
}

struct Queue {
    clips: VecDeque<Clip>,
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
            clips: VecDeque::new(),
            paused: false,
            cut_below: 0,
            gain: 1.0,
            current: None,
            finished: Vec::new(),
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
            // Faded out: what was cut goes, the next clip fades in.
            let cut = q.cut_below;
            q.clips.retain(|c| c.id >= cut);
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
                q.clips.pop_front();
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
        self.queue.lock().unwrap().clips.push_back(Clip {
            id,
            samples,
            pos: 0,
        });
    }

    /// Stop and forget everything queued: what is heard fades out first.
    pub fn clear(&self) {
        self.queue.lock().unwrap().cut();
    }

    pub fn set_paused(&self, paused: bool) {
        self.queue.lock().unwrap().paused = paused;
    }

    /// Clips queued, the one playing included (not those being cut).
    pub fn queued(&self) -> Vec<u64> {
        let q = self.queue.lock().unwrap();
        q.clips
            .iter()
            .filter(|c| c.id >= q.cut_below)
            .map(|c| c.id)
            .collect()
    }

    /// The clip being heard.
    pub fn current(&self) -> Option<u64> {
        self.queue.lock().unwrap().current
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
        std::mem::take(&mut self.queue.lock().unwrap().finished)
    }

    /// The device went away (unplugged, or changed): open it again.
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
