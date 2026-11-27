//! Ogg Opus (RFC 7845) for the local voice's clips: 24 kHz mono speech at
//! about 32 kbit/s, a twelfth of the PCM the model returns and playable in
//! any player. Only what hanstat writes needs reading back: one mono stream,
//! no chaining.

use ogg::writing::PacketWriteEndInfo;
use opus::{Application, Bitrate, Channels, Decoder, Encoder};
use std::io::Cursor;

/// The clips' sample rate (the model's).
pub const RATE: u32 = 24_000;
/// What the clips are, in the cache key: other settings make other files.
pub const FORMAT: &str = "ogg-opus-24khz-mono-32kbps-vbr";
/// 20 ms frames.
const FRAME: usize = RATE as usize / 50;
/// Opus counts granule positions at 48 kHz whatever the input rate.
const GRANULE_PER_SAMPLE: u64 = 48_000 / RATE as u64;

/// `pcm` (mono, [`RATE`]) as an Ogg Opus file.
pub fn encode(pcm: &[i16]) -> Result<Vec<u8>, String> {
    let err = |e: opus::Error| format!("Opus encoder: {e}");
    let mut enc = Encoder::new(RATE, Channels::Mono, Application::Audio).map_err(err)?;
    enc.set_bitrate(Bitrate::Bits(32_000)).map_err(err)?;
    enc.set_vbr(true).map_err(err)?;
    enc.set_complexity(10).map_err(err)?;
    let lookahead = enc.get_lookahead().map_err(err)? as usize;
    let pre_skip = (lookahead as u64 * GRANULE_PER_SAMPLE) as u16;

    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1); // version
    head.push(1); // channels
    head.extend_from_slice(&pre_skip.to_le_bytes());
    head.extend_from_slice(&RATE.to_le_bytes()); // the input's rate, informative
    head.extend_from_slice(&0i16.to_le_bytes()); // output gain
    head.push(0); // mapping family: mono/stereo
    let vendor = format!("hanstat (libopus {})", opus::version());
    let mut tags = Vec::new();
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor.as_bytes());
    tags.extend_from_slice(&0u32.to_le_bytes()); // no comments

    let serial = 0x6873_7461; // "hsta"
    let mut w = ogg::PacketWriter::new(Cursor::new(Vec::new()));
    let io = |e: std::io::Error| format!("Ogg: {e}");
    w.write_packet(head, serial, PacketWriteEndInfo::EndPage, 0)
        .map_err(io)?;
    w.write_packet(tags, serial, PacketWriteEndInfo::EndPage, 0)
        .map_err(io)?;
    // The encoder's lookahead delays the sound: feed that much silence after
    // the end, so the last samples come out, and cut it on decoding.
    let total = pcm.len() + lookahead;
    let frames = total.div_ceil(FRAME).max(1);
    let end_granule = pre_skip as u64 + pcm.len() as u64 * GRANULE_PER_SAMPLE;
    let mut buf = vec![0i16; FRAME];
    let mut out = vec![0u8; 4000];
    for f in 0..frames {
        buf.fill(0);
        let a = f * FRAME;
        if a < pcm.len() {
            let b = (a + FRAME).min(pcm.len());
            buf[..b - a].copy_from_slice(&pcm[a..b]);
        }
        let n = enc.encode(&buf, &mut out).map_err(err)?;
        let last = f + 1 == frames;
        let granule = if last {
            end_granule
        } else {
            ((f + 1) * FRAME) as u64 * GRANULE_PER_SAMPLE
        };
        let info = if last {
            PacketWriteEndInfo::EndStream
        } else {
            PacketWriteEndInfo::NormalPacket
        };
        w.write_packet(out[..n].to_vec(), serial, info, granule)
            .map_err(io)?;
    }
    Ok(w.into_inner().into_inner())
}

/// The samples (mono, [`RATE`], -1..1) of a file made by [`encode`].
pub fn decode(bytes: Vec<u8>) -> Result<(Vec<f32>, u32), String> {
    let mut out = Vec::new();
    let rate = decode_into(&bytes, &mut out)?;
    Ok((out, rate))
}

/// Decode an Ogg Opus clip into `out` (reusing its allocation), returning the
/// sample rate.
pub(super) fn decode_into(bytes: &[u8], out: &mut Vec<f32>) -> Result<u32, String> {
    out.clear();
    let bad = |e: &dyn std::fmt::Display| format!("The audio can't be read: {e}");
    let mut r = ogg::PacketReader::new(Cursor::new(bytes));
    let head = r
        .read_packet()
        .map_err(|e| bad(&e))?
        .ok_or_else(|| bad(&"empty file"))?;
    if head.data.len() < 19 || &head.data[..8] != b"OpusHead" || head.data[9] != 1 {
        return Err(bad(&"not a mono Ogg Opus file"));
    }
    let pre_skip = u16::from_le_bytes([head.data[10], head.data[11]]) as u64;
    let mut dec = Decoder::new(RATE, Channels::Mono).map_err(|e| bad(&e))?;
    // 120 ms, the longest Opus packet.
    let mut buf = vec![0f32; RATE as usize * 120 / 1000];
    let mut end = None;
    let mut seen_tags = false;
    while let Some(p) = r.read_packet().map_err(|e| bad(&e))? {
        if !seen_tags {
            seen_tags = true;
            if p.data.starts_with(b"OpusTags") {
                continue;
            }
        }
        let n = dec
            .decode_float(&p.data, &mut buf, false)
            .map_err(|e| bad(&e))?;
        out.extend_from_slice(&buf[..n]);
        if p.last_in_stream() {
            end = Some(p.absgp_page());
        }
    }
    let skip = (pre_skip / GRANULE_PER_SAMPLE) as usize;
    let len = end.map_or(out.len(), |g| {
        (g.saturating_sub(pre_skip) / GRANULE_PER_SAMPLE) as usize + skip
    });
    out.truncate(len.min(out.len()));
    let start = skip.min(out.len());
    out.drain(..start);
    Ok(RATE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_keeps_length_and_sound() {
        // 1.37 s of a 220 Hz tone with a gap: not a whole number of frames.
        let n = (RATE as f32 * 1.37) as usize;
        let pcm: Vec<i16> = (0..n)
            .map(|i| {
                if (i / 4000) % 3 == 2 {
                    0
                } else {
                    let t = i as f32 / RATE as f32;
                    ((t * 220.0 * std::f32::consts::TAU).sin() * 12_000.0) as i16
                }
            })
            .collect();
        let file = encode(&pcm).unwrap();
        assert!(&file[..4] == b"OggS");
        // About 32 kbit/s.
        let kbps = file.len() as f32 * 8.0 / 1.37 / 1000.0;
        assert!(kbps < 48.0, "{kbps} kbit/s");
        let (out, rate) = decode(file).unwrap();
        assert_eq!(rate, RATE);
        assert_eq!(out.len(), pcm.len());
        // Same signal, aligned (pre-skip removed): high correlation.
        let (mut xy, mut xx, mut yy) = (0f64, 0f64, 0f64);
        for (a, b) in pcm.iter().zip(&out) {
            let a = *a as f64 / 32768.0;
            let b = *b as f64;
            xy += a * b;
            xx += a * a;
            yy += b * b;
        }
        let r = xy / (xx.sqrt() * yy.sqrt());
        assert!(r > 0.95, "correlation {r}");
    }

    #[test]
    fn empty_and_short_clips() {
        for n in [0usize, 1, 479, 480, 481] {
            let pcm = vec![1000i16; n];
            let (out, _) = decode(encode(&pcm).unwrap()).unwrap();
            assert_eq!(out.len(), n, "{n} samples");
        }
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(decode(b"not ogg at all".to_vec()).is_err());
        assert!(decode(Vec::new()).is_err());
    }
}
