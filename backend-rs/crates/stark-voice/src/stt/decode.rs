//! Decode any file the server accepts (WAV/FLAC/MP3/OGG/M4A/…) into the
//! 16 kHz mono f32 stream Parakeet expects. The resampler is Hex's: rubato's
//! FFT, Blackman-Harris windowed, not the linear decimation the old bridge
//! used.

use std::path::Path;

use anyhow::{Context, Result, bail};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler, WindowFunction};
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use super::speech::SAMPLE_RATE;

const CHUNK_SIZE: usize = 1024;

pub fn load_audio_16k(path: &Path) -> Result<Vec<f32>> {
    let file =
        std::fs::File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
        hint.with_extension(extension);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .with_context(|| format!("unsupported audio {}", path.display()))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        .context("audio has no decodable track")?;
    let track_id = track.id;
    let source_rate = track.codec_params.sample_rate.unwrap_or(16_000);
    let channels = track
        .codec_params
        .channels
        .map(|value| value.count())
        .unwrap_or(1);
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .context("unsupported audio codec")?;

    let mut mono = Vec::new();
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track_id {
            continue;
        }
        let Ok(decoded) = decoder.decode(&packet) else {
            continue;
        };
        append_mono(&decoded, channels, &mut mono);
    }
    if mono.is_empty() {
        bail!("audio decoded to no samples");
    }
    if source_rate as usize == SAMPLE_RATE {
        return Ok(mono);
    }
    resample_fft(&mono, source_rate)
}

fn append_mono(buffer: &AudioBufferRef, channels: usize, out: &mut Vec<f32>) {
    let frames = buffer.frames();
    let channels = channels.max(1);
    for index in 0..frames {
        let mut sum = 0.0f32;
        for channel in 0..channels {
            let sample = match buffer {
                AudioBufferRef::F32(buffer) => buffer.chan(channel)[index],
                AudioBufferRef::S16(buffer) => buffer.chan(channel)[index] as f32 / 32_768.0,
                AudioBufferRef::S32(buffer) => buffer.chan(channel)[index] as f32 / 2_147_483_648.0,
                AudioBufferRef::U8(buffer) => (buffer.chan(channel)[index] as f32 - 128.0) / 128.0,
                AudioBufferRef::F64(buffer) => buffer.chan(channel)[index] as f32,
                _ => 0.0,
            };
            sum += sample;
        }
        out.push(sum / channels as f32);
    }
}

fn resample_fft(samples: &[f32], source_rate: u32) -> Result<Vec<f32>> {
    let mut resampler = Fft::<f32>::new_custom(
        source_rate as usize,
        SAMPLE_RATE,
        CHUNK_SIZE,
        1,
        1,
        WindowFunction::BlackmanHarris2,
        FixedSync::Input,
    )
    .map_err(|error| anyhow::anyhow!("resampler: {error}"))?;
    let delay = resampler.output_delay();
    let expected = samples.len() * SAMPLE_RATE / source_rate as usize;
    let mut output = Vec::with_capacity(expected + delay);
    let mut input = Vec::with_capacity(CHUNK_SIZE);
    for sample in samples {
        input.push(*sample);
        if input.len() == CHUNK_SIZE {
            output.extend(resample_chunk(&mut resampler, &input, false));
            input.clear();
        }
    }
    if !input.is_empty() {
        output.extend(resample_chunk(&mut resampler, &input, true));
    }
    for _ in 0..8 {
        if output.len() >= expected + delay {
            break;
        }
        output.extend(resample_chunk(&mut resampler, &[], true));
    }
    if output.len() > delay {
        output.drain(..delay);
    }
    output.truncate(expected);
    Ok(output)
}

fn resample_chunk(resampler: &mut Fft<f32>, samples: &[f32], partial: bool) -> Vec<f32> {
    let input =
        InterleavedSlice::new(samples, 1, samples.len()).expect("mono chunk fits its slice");
    let indexing = partial.then(|| Indexing {
        partial_len: Some(samples.len()),
        ..Indexing::default()
    });
    resampler
        .process(&input, indexing.as_ref())
        .expect("fixed-rate resampling must succeed")
        .take_data()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resamples_48k_to_16k_with_a_whole_number_of_samples() {
        let samples: Vec<f32> = (0..48_000)
            .map(|index| (index as f32 * 0.01).sin())
            .collect();
        let out = resample_fft(&samples, 48_000).unwrap();
        assert_eq!(out.len(), 16_000);
    }
}
