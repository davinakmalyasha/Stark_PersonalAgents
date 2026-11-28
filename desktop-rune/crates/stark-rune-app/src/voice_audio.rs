//! Microphone PCM helpers and Hex's anti-aliased resampler.
//!
//! The app owns only capture. Speech recognition runs in `stark-server`
//! (Parakeet Unified English), so this module has no model code and no ONNX
//! runtime. WASAPI shared mode usually exposes 48 kHz; the FFT resampler keeps
//! 48k → 16k clean instead of the old linear decimation.

pub const SAMPLE_RATE: u32 = 16_000;
/// 30 s of 10 ms blocks. The server keeps the complete turn for its offline
/// fallback; the app must not silently truncate the front of a long thought.
pub const MAX_AUDIO_QUEUE: usize = 3_000;

/// Downmix interleaved stereo/multichannel samples to mono.
pub fn mono_mix(input: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 || input.is_empty() {
        return input.to_vec();
    }
    input
        .chunks_exact(channels as usize)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use rubato::audioadapter_buffers::direct::InterleavedSlice;
    use rubato::{Fft, FixedSync, Indexing, Resampler, WindowFunction};

    const CHUNK_SIZE: usize = 1_024;

    /// Streaming FFT resampler with a Blackman-Harris window, the same
    /// resampler Hex uses for microphone capture.
    pub struct FftResampler {
        inner: Fft<f32>,
        input: Vec<f32>,
        delay: usize,
    }

    impl FftResampler {
        pub fn new(from: u32, to: u32) -> Option<Self> {
            if from == to || from == 0 || to == 0 {
                return None;
            }
            let inner = Fft::new_custom(
                from as usize,
                to as usize,
                CHUNK_SIZE,
                1,
                1,
                WindowFunction::BlackmanHarris2,
                FixedSync::Input,
            )
            .ok()?;
            let delay = inner.output_delay();
            Some(Self {
                inner,
                input: Vec::with_capacity(CHUNK_SIZE),
                delay,
            })
        }

        pub fn process(&mut self, samples: &[f32], out: &mut Vec<f32>) {
            let mut rest = samples;
            while !rest.is_empty() {
                let take = rest.len().min(CHUNK_SIZE - self.input.len());
                self.input.extend_from_slice(&rest[..take]);
                rest = &rest[take..];
                if self.input.len() == CHUNK_SIZE {
                    let chunk = std::mem::take(&mut self.input);
                    self.resample(&chunk, out, false);
                    self.input = chunk;
                    self.input.clear();
                }
            }
        }

        pub fn flush(&mut self, out: &mut Vec<f32>) {
            if self.input.is_empty() {
                return;
            }
            let chunk = std::mem::take(&mut self.input);
            self.resample(&chunk, out, true);
            self.input = chunk;
            self.input.clear();
        }

        fn resample(&mut self, samples: &[f32], out: &mut Vec<f32>, partial: bool) {
            let input = InterleavedSlice::new(samples, 1, samples.len())
                .expect("mono chunk fits its slice");
            let indexing = partial.then(|| Indexing {
                partial_len: Some(samples.len()),
                ..Indexing::default()
            });
            let Ok(result) = self.inner.process(&input, indexing.as_ref()) else {
                return;
            };
            let mut chunk = result.take_data();
            if self.delay > 0 {
                let drop = self.delay.min(chunk.len());
                self.delay -= drop;
                chunk.drain(..drop);
            }
            out.extend_from_slice(&chunk);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::FftResampler;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_mix_averages_channels() {
        assert_eq!(mono_mix(&[1.0, 0.0, 0.0, 1.0], 2), vec![0.5, 0.5]);
        assert_eq!(mono_mix(&[0.25], 1), vec![0.25]);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn fft_resampler_outputs_the_expected_rate() {
        let mut resampler = FftResampler::new(48_000, 16_000).unwrap();
        let input: Vec<f32> = (0..48_000)
            .map(|index| (index as f32 * 0.01).sin())
            .collect();
        let mut out = Vec::new();
        resampler.process(&input, &mut out);
        resampler.flush(&mut out);
        // The FFT resampler drops its output delay at the start; the stream
        // is continuous, so a one-second buffer lands just under 16k.
        assert!(out.len() >= 15_000, "got {}", out.len());
        assert!(out.len() <= 16_010, "got {}", out.len());
    }
}
