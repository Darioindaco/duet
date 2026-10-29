//! Reading audio files of any common kind: WAV through the fast reader in
//! daw.rs, everything else (MP3, M4A/AAC/ALAC as an iPhone records them,
//! FLAC, AIFF, Ogg Vorbis) decoded with symphonia.

use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::daw::{self, AudioData, PEAK_BUCKET};


fn is_wav(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(), Some("wav" | "wave"))
}

/// The whole file: mono stays mono, two channels or more come in as stereo.
pub fn read_audio(path: &Path) -> Result<AudioData, String> {
    if is_wav(path) {
        match daw::read_wav(path) {
            Ok(a) => return Ok(a),
            Err(e) => return decoded(path).map_err(|_| e), // an odd WAV symphonia may still know
        }
    }
    decoded(path)
}

fn decoded(path: &Path) -> Result<AudioData, String> {
    let (mut l, mut r) = (Vec::new(), Vec::new());
    let (sr, ch, _) = decode(path, |block, ch| {
        if ch == 1 {
            l.extend_from_slice(block);
        } else {
            for f in block.chunks_exact(ch) {
                l.push(f[0]);
                r.push(f[1]);
            }
        }
    })?;
    Ok(AudioData { sr, samples: l, right: (ch > 1).then_some(r) })
}

/// (frames, sample rate, channels): from the header where it says, else by
/// decoding the file (an MP3 without a length in it).
pub fn audio_header(path: &Path) -> Result<(usize, u32, usize), String> {
    if is_wav(path) {
        if let Ok(h) = daw::wav_header(path) {
            return Ok(h);
        }
    }
    let (_, format) = open(path)?;
    let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).ok_or("no audio in this file")?;
    let p = &track.codec_params;
    if let (Some(n), Some(sr), Some(ch)) = (p.n_frames, p.sample_rate, p.channels) {
        return Ok((n as usize, sr, ch.count()));
    }
    let mut frames = 0;
    let (sr, ch, _) = decode(path, |block, ch| frames += block.len() / ch)?;
    Ok((frames, sr, ch))
}

/// (frames, sample rate, channels, waveform peaks of both sides mixed),
/// without keeping the audio.
pub fn audio_peaks(path: &Path) -> Result<(usize, u32, usize, Vec<u8>), String> {
    if is_wav(path) {
        if let Ok(x) = daw::wav_peaks(path) {
            return Ok(x);
        }
    }
    let mut out = Vec::new();
    let (mut lo, mut hi, mut n, mut frames) = (0f32, 0f32, 0usize, 0usize);
    let (sr, ch, _) = decode(path, |block, ch| {
        for f in block.chunks_exact(ch) {
            let v = if ch > 1 { 0.5 * (f[0] + f[1]) } else { f[0] };
            lo = lo.min(v);
            hi = hi.max(v);
            n += 1;
            frames += 1;
            if n == PEAK_BUCKET {
                out.push((lo.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8);
                out.push((hi.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8);
                (lo, hi, n) = (0.0, 0.0, 0);
            }
        }
    })?;
    if n > 0 {
        out.push((lo.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8);
        out.push((hi.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8);
    }
    Ok((frames, sr, ch, out))
}

fn open(path: &Path) -> Result<(Hint, Box<dyn FormatReader>), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    // gapless: an AAC file's encoder padding at the start and end is taken off
    let opts = FormatOptions { enable_gapless: true, ..Default::default() };
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &opts, &MetadataOptions::default())
        .map_err(|e| format!("{}: can't read this kind of file ({e})", path.display()))?;
    Ok((hint, probed.format))
}

/// Decode the file's first audio track, handing over interleaved floats a
/// packet at a time. Returns (sample rate, channels, frames the header gave).
fn decode(path: &Path, mut block: impl FnMut(&[f32], usize)) -> Result<(u32, usize, Option<u64>), String> {
    let (_, mut format) = open(path)?;
    let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).ok_or("no audio in this file")?;
    let (id, params) = (track.id, track.codec_params.clone());
    let mut dec = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .map_err(|e| format!("{}: can't decode this ({e})", path.display()))?;
    let mut sr = params.sample_rate.unwrap_or(0);
    let mut ch = params.channels.map_or(0, |c| c.count());
    let mut buf: Option<(SampleBuffer<f32>, usize)> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(Error::ResetRequired) => break,
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        if packet.track_id() != id {
            continue;
        }
        match dec.decode(&packet) {
            Ok(audio) => {
                let spec = *audio.spec();
                (sr, ch) = (spec.rate, spec.channels.count().max(1));
                if buf.as_ref().map_or(true, |(b, c)| b.capacity() < audio.capacity() || *c != ch) {
                    buf = Some((SampleBuffer::new(audio.capacity() as u64, spec), ch));
                }
                let (b, _) = buf.as_mut().expect("made above");
                b.copy_interleaved_ref(audio);
                block(b.samples(), ch);
            }
            Err(Error::DecodeError(_)) => continue, // one damaged packet: skip it
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    if sr == 0 || ch == 0 {
        return Err(format!("{}: no audio in this file", path.display()));
    }
    Ok((sr, ch, params.n_frames))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_wav_through_both_paths() {
        let dir = std::env::temp_dir().join(format!("duet-decode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.wav");
        let spec = hound::WavSpec { channels: 2, sample_rate: 44100, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&p, spec).unwrap();
        for k in 0..20_000 {
            w.write_sample(((k as f32 * 0.01).sin() * 20000.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        let fast = read_audio(&p).unwrap();
        let slow = decoded(&p).unwrap(); // symphonia's WAV reader
        assert_eq!((fast.sr, fast.samples.len()), (slow.sr, slow.samples.len()));
        assert!(fast.samples.iter().zip(&slow.samples).all(|(a, b)| (a - b).abs() < 1e-4));
        assert_eq!(audio_header(&p).unwrap(), (10_000, 44100, 2));
        // the same file under another name: decoded by content, not by name
        let q = dir.join("t.flac");
        std::fs::copy(&p, &q).unwrap();
        assert!(read_audio(&q).is_ok_and(|a| a.samples.len() == 10_000));
        std::fs::remove_dir_all(&dir).ok();
    }
}
