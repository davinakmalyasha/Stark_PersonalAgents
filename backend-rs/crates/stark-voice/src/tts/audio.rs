//! Audio input and output: port of `pocket_tts/data/audio.py` and
//! `pocket_tts/data/audio_utils.py`.

use anyhow::{Context, Result};
use std::path::Path;

/// Reads an audio file as mono samples, averaging the channels, and its sample rate.
///
/// TODO: upstream to xn-ptts: `ptts::audio::decode_file` keeps only the first channel and does not
/// trim the MP3/AAC encoder delay, which libsndfile (the Python package's decoder) does.
pub fn audio_read(path: &Path) -> Result<(Vec<f32>, u32)> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::errors::Error as SymphoniaError;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file =
        std::fs::File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(ext);
    }
    // Gapless: the encoder delay and padding of MP3 and AAC are trimmed, as libsndfile (the
    // Python package's decoder) does, rather than heard as a shift.
    let format_options = FormatOptions {
        enable_gapless: true,
        ..Default::default()
    };
    let probed = symphonia::default::get_probe()
        .format(&hint, stream, &format_options, &MetadataOptions::default())
        .with_context(|| format!("unsupported audio format: {}", path.display()))?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .with_context(|| format!("no audio track in {}", path.display()))?;
    let track_id = track.id;
    let mut sample_rate = track.codec_params.sample_rate;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .with_context(|| format!("unsupported audio codec in {}", path.display()))?;

    let mut samples = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // A corrupt packet is skipped, as decoders conventionally do.
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(e).with_context(|| format!("cannot decode {}", path.display())),
        };
        let spec = *decoded.spec();
        sample_rate.get_or_insert(spec.rate);
        let channels = spec.channels.count();
        let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buffer.copy_interleaved_ref(decoded);
        for frame in buffer.samples().chunks_exact(channels) {
            samples.push(frame.iter().sum::<f32>() / channels as f32);
        }
    }
    let sample_rate =
        sample_rate.with_context(|| format!("no sample rate in {}", path.display()))?;
    Ok((samples, sample_rate))
}

/// End a voice prompt on exactly `pause_sec` of silence.
///
/// Training prompts end inside the pause between two words. A prompt that stops on speech makes
/// the model continue that utterance, and one that ends on a long silence delays the onset. The
/// trailing silence (20 ms frames more than `floor_db` below the loudest one) is cut, the last
/// `fade_sec` of what remains is faded out, and `pause_sec` of zeros is appended.
pub fn end_on_pause(wav: &[f32], sample_rate: usize) -> Vec<f32> {
    let (pause_sec, fade_sec, floor_db) = (0.08, 0.02, 35.0f32);
    let frame = ((0.02 * sample_rate as f64) as usize).max(1);
    let n = wav.len() / frame;
    if n == 0 {
        return wav.to_vec();
    }
    let db: Vec<f32> = wav[..n * frame]
        .chunks_exact(frame)
        .map(|chunk| {
            let rms = (chunk.iter().map(|v| v * v).sum::<f32>() / frame as f32).sqrt();
            20.0 * (rms + 1e-12).log10()
        })
        .collect();
    let max_db = db.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let last_loud = db.iter().rposition(|&d| d > max_db - floor_db).unwrap_or(0);
    let end = (last_loud + 1) * frame;
    let mut out = wav[..end].to_vec();
    let fade = ((fade_sec * sample_rate as f64) as usize).min(end);
    for i in 0..fade {
        let gain = if fade == 1 {
            1.0
        } else {
            1.0 - i as f32 / (fade - 1) as f32
        };
        out[end - fade + i] *= gain;
    }
    out.extend(std::iter::repeat_n(
        0.0,
        (pause_sec * sample_rate as f64) as usize,
    ));
    out
}

/// The official 18M model only needs decoding for voice prompts; WAV output is handled by
/// `stark_voice::wav`, so the upstream streaming writer is intentionally not vendored.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_on_pause_trims_silence_and_appends_a_pause() {
        let sr = 1000;
        let mut wav = vec![0.5f32; 500];
        wav.extend(vec![0.0f32; 500]);
        let out = end_on_pause(&wav, sr);
        assert_eq!(out.len(), 500 + 80);
        assert_eq!(out[0], 0.5);
        assert_eq!(out[499], 0.0);
        assert!(out[500..].iter().all(|&v| v == 0.0));
    }
}
