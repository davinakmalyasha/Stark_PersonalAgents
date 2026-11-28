//! Hex's capture level gate, independent of the STT engine.
//!
//! It rejects digital silence, a DC offset, clicks and steady room noise
//! before Silero or Parakeet sees them. The fixed cutoff is met outright, or
//! the clip carries a contiguous run of sustained energy above its own noise
//! floor; the speech detector still confirms the quiet clips.

pub const SAMPLE_RATE: usize = 16_000;

/// A capture whose loudest 30 ms window reaches this level reaches Silero,
/// because models hallucinate closing phrases such as "thank you" on
/// silence. Below it the clip's own noise floor decides.
const SILENCE_THRESHOLD_DBFS: f32 = -50.0;
/// How far a window must rise above the clip's own noise floor to count as
/// speech there. The speech detector still decides afterwards.
const SILENCE_RELATIVE_MARGIN_DB: f32 = 12.0;
/// No window quieter than this counts as speech over the floor, however quiet
/// the floor is.
const SILENCE_RELATIVE_FLOOR_DBFS: f32 = -66.0;
/// How much of the clip must carry energy above that relative threshold in
/// one contiguous run: at least 180 ms, six 30 ms windows.
const SILENCE_SUSTAINED_MS: u64 = 180;
const SILENCE_WINDOW_SAMPLES: usize = 480;
/// The quietest level a window reports; digital silence and a constant offset
/// sit here rather than at negative infinity.
const SILENCE_LEVEL_FLOOR_DBFS: f32 = -120.0;

/// Hex's level gate over a whole capture.
#[derive(Clone, Copy, Debug)]
pub struct LevelGate {
    loudest_dbfs: f32,
    noise_floor_dbfs: Option<f32>,
    relative_threshold_dbfs: Option<f32>,
    sustained_run_windows: usize,
}

impl LevelGate {
    pub fn measure(samples: &[f32]) -> Self {
        let levels: Vec<f32> = window_levels_dbfs(samples).collect();
        let loudest_dbfs = levels
            .iter()
            .copied()
            .fold(SILENCE_LEVEL_FLOOR_DBFS, f32::max);
        let noise_floor_dbfs = noise_floor_dbfs(&levels);
        let relative_threshold_dbfs = noise_floor_dbfs
            .map(|floor| (floor + SILENCE_RELATIVE_MARGIN_DB).max(SILENCE_RELATIVE_FLOOR_DBFS));
        let sustained_run_windows =
            relative_threshold_dbfs.map_or(0, |threshold| longest_run_windows(&levels, threshold));
        Self {
            loudest_dbfs,
            noise_floor_dbfs,
            relative_threshold_dbfs,
            sustained_run_windows,
        }
    }

    /// Whether the clip reaches the fixed cutoff or a sustained run above
    /// its own floor, so it is worth the detector's attention.
    pub fn passes(&self) -> bool {
        self.loudest_dbfs >= SILENCE_THRESHOLD_DBFS
            || self.sustained_run_windows >= sustained_windows_needed()
    }

    pub fn loudest_dbfs(&self) -> f32 {
        self.loudest_dbfs
    }

    pub fn noise_floor_dbfs(&self) -> Option<f32> {
        self.noise_floor_dbfs
    }

    pub fn relative_threshold_dbfs(&self) -> Option<f32> {
        self.relative_threshold_dbfs
    }

    pub fn sustained_ms(&self) -> u64 {
        self.sustained_run_windows as u64 * silence_window_ms()
    }
}

/// Whether a whole capture is near-silent by Hex's level rule alone.
pub fn level_gate(samples: &[f32]) -> LevelGate {
    LevelGate::measure(samples)
}

fn silence_window_ms() -> u64 {
    SILENCE_WINDOW_SAMPLES as u64 * 1_000 / SAMPLE_RATE as u64
}

fn sustained_windows_needed() -> usize {
    SILENCE_SUSTAINED_MS.div_ceil(silence_window_ms()) as usize
}

/// The most above-threshold windows in one contiguous run. A single quiet
/// window between loud ones is bridged but not counted; two in a row end the
/// run, and leading quiet windows don't consume the bridge.
fn longest_run_windows(levels: &[f32], threshold: f32) -> usize {
    let (mut run, mut longest, mut bridged) = (0, 0, false);
    for &level in levels {
        if level >= threshold {
            run += 1;
            longest = longest.max(run);
            bridged = false;
        } else if run == 0 {
            bridged = false;
        } else if bridged {
            run = 0;
            bridged = false;
        } else {
            bridged = true;
        }
    }
    longest
}

/// The 10th-percentile 30 ms window level, the clip's own noise floor.
fn noise_floor_dbfs(levels: &[f32]) -> Option<f32> {
    let mut audible: Vec<f32> = levels
        .iter()
        .copied()
        .filter(|&level| level > SILENCE_LEVEL_FLOOR_DBFS)
        .collect();
    if audible.is_empty() {
        return None;
    }
    audible.sort_by(f32::total_cmp);
    Some(audible[(audible.len() - 1) / 10])
}

/// The DC-removed RMS level of each 30 ms window; a short final window is
/// measured on the samples it has.
fn window_levels_dbfs(samples: &[f32]) -> impl Iterator<Item = f32> + '_ {
    samples.chunks(SILENCE_WINDOW_SAMPLES).map(|window| {
        let count = window.len() as f64;
        let mean = window.iter().map(|&sample| f64::from(sample)).sum::<f64>() / count;
        let variance = window
            .iter()
            .map(|&sample| (f64::from(sample) - mean).powi(2))
            .sum::<f64>()
            / count;
        ((20.0 * variance.sqrt().max(1e-6).log10()) as f32).max(SILENCE_LEVEL_FLOOR_DBFS)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sine whose 30 ms RMS level is `dbfs`.
    fn tone(dbfs: f32, samples: usize) -> Vec<f32> {
        let amplitude = 10f32.powf(dbfs / 20.0) * 2f32.sqrt();
        (0..samples)
            .map(|index| (index as f32 * 0.1).sin() * amplitude)
            .collect()
    }

    /// Deterministic room noise at a 30 ms RMS level of `dbfs`.
    fn noise(dbfs: f32, samples: usize) -> Vec<f32> {
        let amplitude = 10f32.powf(dbfs / 20.0) * 3f32.sqrt();
        let mut seed = 0x2545_f491_u32;
        (0..samples)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed as f32 / u32::MAX as f32 * 2.0 - 1.0) * amplitude
            })
            .collect()
    }

    #[test]
    fn level_gate_rejects_digital_silence_and_dc_offsets() {
        assert!(!level_gate(&[]).passes());
        assert!(!level_gate(&vec![0.0; 16_000]).passes());
        assert!(
            !level_gate(&vec![0.2; 16_000]).passes(),
            "a DC offset is not speech"
        );
        assert!(
            !level_gate(&noise(-60.0, 32_000)).passes(),
            "steady noise is not speech"
        );
    }

    #[test]
    fn level_gate_keeps_quiet_speech_over_its_own_floor() {
        let mut samples = noise(-78.0, 4_000);
        samples.extend(tone(-54.0, 6_000));
        samples.extend(noise(-78.0, 4_000));
        let gate = level_gate(&samples);
        assert!(gate.loudest_dbfs() < -50.0, "the fixed cutoff is not met");
        assert!(gate.passes(), "the sustained run above the floor keeps it");
        assert!(gate.sustained_ms() >= 180);
    }

    #[test]
    fn level_gate_drops_a_single_click_in_a_quiet_room() {
        let mut samples = noise(-78.0, 16_000);
        samples[8_000..8_480].copy_from_slice(&tone(-54.0, 480));
        assert!(!level_gate(&samples).passes());
    }

    #[test]
    fn level_gate_keeps_normal_speech_at_the_fixed_cutoff() {
        // 250 ms at -43 dBFS inside a quiet second.
        let mut samples = noise(-82.0, 4_000);
        samples.extend(tone(-43.0, 4_000));
        samples.extend(noise(-82.0, 4_000));
        assert!(level_gate(&samples).passes());
    }
}
