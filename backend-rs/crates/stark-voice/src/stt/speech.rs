//! Speech framing, pause capping and final-text checks, ported from Hex's
//! transcription path. These are the parts that keep a buffered Parakeet
//! stream honest: cap long pauses, append the model's trailing silence, and
//! reject a stream whose text does not cover the speech it heard.

use std::collections::VecDeque;

use transcribe_cpp::Token;

pub const SAMPLE_RATE: usize = 16_000;
pub const FRAME_SAMPLES: usize = SAMPLE_RATE / 50; // 20 ms

/// Parakeet Unified English's closing-token context, in samples (400 ms).
pub const UNIFIED_TRAILING_SILENCE: usize = 6_400;
/// The longest pause head and tail a buffered Parakeet stream hears.
pub const PAUSE_HEAD_SAMPLES: usize = 40_000;
pub const PAUSE_TAIL_SAMPLES: usize = 4_800;
/// A buffered Parakeet stream's first word timestamp lags its audio.
pub const PARAKEET_OPENING_SLACK: usize = 19_200;
const UNTRANSCRIBED_SLACK: usize = 6_400;
const UNTRANSCRIBED_SPEECH: usize = 4_800;

const SPEECH_MARGIN_DB: f32 = 12.0;
const QUIET_MARGIN_DB: f32 = 4.0;
const MINIMUM_SPEECH_DB: f32 = -55.0;
const QUIET_MINIMUM_DB: f32 = -70.0;
const LOWEST_FLOOR_DB: f32 = -90.0;
const FLOOR_RISE_DB_PER_FRAME: f32 = 0.06;

/// One 20 ms frame against an adaptive noise floor. `quiet` is stricter than
/// "not speech": a frame at the floor is a pause, while speech a few dB over
/// steady room noise is neither quiet nor confidently speech.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameLevel {
    pub quiet: bool,
    pub speech: bool,
}

/// Adaptive 16 kHz frame classifier, ported from Hex's `pause_segmenter`.
#[derive(Default)]
pub struct SpeechFrames {
    floor_db: Option<f32>,
}

impl SpeechFrames {
    pub fn is_speech(&mut self, frame: &[f32]) -> bool {
        self.classify(frame).speech
    }

    pub fn classify(&mut self, frame: &[f32]) -> FrameLevel {
        let (level_db, floor_db) = self.measure(frame);
        FrameLevel {
            quiet: level_db <= (floor_db + QUIET_MARGIN_DB).max(QUIET_MINIMUM_DB),
            speech: level_db > (floor_db + SPEECH_MARGIN_DB).max(MINIMUM_SPEECH_DB),
        }
    }

    fn measure(&mut self, frame: &[f32]) -> (f32, f32) {
        let count = frame.len().max(1) as f32;
        let mean = frame.iter().sum::<f32>() / count;
        let energy = frame
            .iter()
            .map(|sample| (sample - mean) * (sample - mean))
            .sum::<f32>()
            / count;
        let level_db = if energy.is_finite() {
            10.0 * energy.max(1e-18).log10()
        } else {
            0.0
        };
        let floor_db = match self.floor_db {
            Some(floor_db) if level_db >= floor_db => floor_db + FLOOR_RISE_DB_PER_FRAME,
            _ => level_db.max(LOWEST_FLOOR_DB),
        };
        self.floor_db = Some(floor_db);
        (level_db, floor_db)
    }
}

/// Caps each long pause to its head and tail, exactly as Hex's `PauseCap`.
///
/// A buffered Parakeet stream re-encodes a 5.6 s left window for every chunk;
/// a long pause is wasted work and can drop the word after it. The head lets
/// the decoder finish the words before the pause; the tail gives it the next
/// onset. Everything else is dropped from the stream (the whole clip remains
/// available for the offline fallback).
pub struct PauseCap {
    speech: SpeechFrames,
    frame: Vec<f32>,
    sent: usize,
    speech_ends: Vec<usize>,
    pause: usize,
    tail: VecDeque<f32>,
    dropped: usize,
    heard: bool,
    trailing_silence: usize,
    head_ignored: usize,
    opening_slack: usize,
}

impl PauseCap {
    pub fn new(head_ignored: usize) -> Self {
        Self {
            speech: SpeechFrames::default(),
            frame: Vec::with_capacity(FRAME_SAMPLES),
            sent: 0,
            speech_ends: Vec::new(),
            pause: 0,
            tail: VecDeque::with_capacity(PAUSE_TAIL_SAMPLES),
            dropped: 0,
            heard: false,
            trailing_silence: UNIFIED_TRAILING_SILENCE,
            head_ignored,
            opening_slack: PARAKEET_OPENING_SLACK,
        }
    }

    /// Feed arbitrary sample blocks; `out` receives the audio the stream
    /// should hear.
    pub fn push(&mut self, mut samples: &[f32], out: &mut Vec<f32>) {
        while !samples.is_empty() {
            let take = samples.len().min(FRAME_SAMPLES - self.frame.len());
            self.frame.extend_from_slice(&samples[..take]);
            samples = &samples[take..];
            if self.frame.len() == FRAME_SAMPLES {
                self.complete_frame(out);
            }
        }
    }

    fn complete_frame(&mut self, out: &mut Vec<f32>) {
        let level = self.speech.classify(&self.frame);
        if !level.quiet {
            self.heard = true;
            out.extend(self.tail.drain(..));
            self.pause = 0;
            out.append(&mut self.frame);
            if level.speech {
                self.speech_ends.push(self.sent + out.len());
            }
            return;
        }
        self.pause += self.frame.len();
        if self.pause <= PAUSE_HEAD_SAMPLES {
            out.append(&mut self.frame);
            return;
        }
        self.tail.extend(self.frame.drain(..));
        let overflow = self.tail.len().saturating_sub(PAUSE_TAIL_SAMPLES);
        self.tail.drain(..overflow);
        self.dropped += overflow;
    }

    /// Flush the last frame, append trailing silence, and return whether the
    /// cap cut everything it heard as a pause.
    pub fn finish(&mut self, out: &mut Vec<f32>) -> Result<(), String> {
        if self.pause <= PAUSE_HEAD_SAMPLES && !self.frame.is_empty() {
            out.extend(std::mem::take(&mut self.frame));
        } else {
            self.dropped += self.frame.len();
            self.frame.clear();
        }
        self.dropped += self.tail.len();
        self.tail.clear();
        if !self.heard && self.dropped > 0 {
            return Err(format!(
                "the stream heard only quiet audio and {} ms was cut as a pause",
                self.dropped / 16
            ));
        }
        if self.trailing_silence > 0 {
            out.extend(std::iter::repeat_n(0.0, self.trailing_silence));
        }
        Ok(())
    }

    /// Reject a final stream text that starts after speech it did not
    /// transcribe, or ends before speech that followed it.
    pub fn check_coverage(&self, tokens: &[Token]) -> Result<(), String> {
        let (Some(start_ms), Some(end_ms)) = (text_start_ms(tokens), text_end_ms(tokens)) else {
            return Ok(());
        };
        let covered = (start_ms as usize * 16).saturating_sub(self.opening_slack);
        let untranscribed = self
            .speech_ends
            .iter()
            .copied()
            .filter(|&end| end > self.head_ignored && end < covered)
            .count()
            * FRAME_SAMPLES;
        if untranscribed >= UNTRANSCRIBED_SPEECH {
            return Err(format!(
                "the stream's text starts at {start_ms} ms but {} ms of speech precedes it",
                untranscribed / 16
            ));
        }
        let covered = end_ms as usize * 16 + UNTRANSCRIBED_SLACK;
        let after = self
            .speech_ends
            .iter()
            .filter(|&&end| end > covered)
            .count()
            * FRAME_SAMPLES;
        if after >= UNTRANSCRIBED_SPEECH {
            return Err(format!(
                "the stream's text ends at {end_ms} ms but {} ms of speech follows",
                after / 16
            ));
        }
        Ok(())
    }

    /// Track an emitted block. Call after the stream accepts `out`.
    pub fn note_sent(&mut self, samples: usize) {
        self.sent += samples;
    }
}

fn text_start_ms(tokens: &[Token]) -> Option<u64> {
    if tokens.is_empty() {
        return None;
    }
    tokens
        .iter()
        .filter(|token| token.text.chars().any(char::is_alphanumeric))
        .map(|token| token.t0_ms.max(0) as u64)
        .min()
}

fn text_end_ms(tokens: &[Token]) -> Option<u64> {
    if tokens.is_empty() {
        return None;
    }
    tokens
        .iter()
        .filter(|token| token.text.chars().any(char::is_alphanumeric))
        .map(|token| token.t1_ms.max(0) as u64)
        .max()
        .filter(|&end| end > 0)
}

/// Unified English's chunked decode often drops its closing sentence mark.
/// Hex trusts the whole clip when that happens; so do we.
pub fn check_closing_mark(text: &str) -> Result<String, String> {
    if has_closing_mark(text) {
        Ok(text.to_string())
    } else {
        Err("the stream ended without closing sentence punctuation".into())
    }
}

fn has_closing_mark(text: &str) -> bool {
    const CLOSERS: [char; 8] = ['"', '\'', '”', '’', ')', ']', '}', '»'];
    text.trim_end()
        .trim_end_matches(CLOSERS)
        .chars()
        .last()
        .is_some_and(|last| matches!(last, '.' | '?' | '!' | '…'))
}

/// Pad a whole clip for an offline Parakeet run: the model's 1.5 s minimum
/// plus the 400 ms trailing-silence context.
pub fn pad_clip(mut clip: Vec<f32>) -> Vec<f32> {
    let minimum = SAMPLE_RATE * 3 / 2;
    if clip.len() < minimum {
        clip.resize(minimum, 0.0);
    }
    clip.extend(std::iter::repeat_n(0.0, UNIFIED_TRAILING_SILENCE));
    clip
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voiced(ms: usize) -> Vec<f32> {
        (0..ms * SAMPLE_RATE / 1000)
            .map(|i| if i % 2 == 0 { 0.1 } else { -0.1 })
            .collect()
    }

    #[test]
    fn speech_frames_adapt_to_a_quiet_room() {
        let mut frames = SpeechFrames::default();
        let room = vec![0.0005; FRAME_SAMPLES];
        assert!(frames.classify(&room).quiet);
        assert!(frames.is_speech(&voiced(20)));
    }

    #[test]
    fn pause_cap_keeps_a_long_pause_head_and_tail() {
        let mut cap = PauseCap::new(0);
        let mut out = Vec::new();
        cap.push(&voiced(1_000), &mut out);
        cap.note_sent(out.len());
        let speech = out.len();
        cap.push(&vec![0.0; 20_000], &mut out);
        // Head is kept only after the pause threshold; the middle is dropped.
        assert!(out.len() > speech);
        assert!(out.len() < speech + 20_000);
        let mut tail = Vec::new();
        cap.finish(&mut tail).unwrap();
        assert!(tail.len() >= UNIFIED_TRAILING_SILENCE);
    }

    #[test]
    fn closing_mark_accepts_sentence_endings() {
        assert!(check_closing_mark("Hello there.").is_ok());
        assert!(check_closing_mark("Are you there?").is_ok());
        assert!(check_closing_mark("Hello there").is_err());
    }

    #[test]
    fn pad_clip_has_the_minimum_and_trailing_context() {
        let padded = pad_clip(vec![0.0; 100]);
        assert_eq!(padded.len(), SAMPLE_RATE * 3 / 2 + UNIFIED_TRAILING_SILENCE);
    }
}
