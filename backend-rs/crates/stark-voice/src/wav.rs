use std::io::Cursor;

use anyhow::Result;

/// Convert f32 samples in [-1, 1] to little-endian mono PCM16.
pub fn pcm16(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for &sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// A complete mono PCM16 WAV file.
pub fn wav_bytes(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(&mut cursor, spec)?;
        for &sample in samples {
            let value = (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            writer.write_sample(value)?;
        }
        writer.finalize()?;
    }
    Ok(cursor.into_inner())
}
