//! One browser-owned audio session: PCM in, VAD, streaming STT, turn commit.
//!
//! The browser is the only microphone owner. Audio arrives as 16 kHz mono
//! PCM16 frames over the voice WebSocket; this worker owns the Moonshine
//! stream and emits exactly one final transcript per detected turn.
#![cfg_attr(
    not(all(target_env = "msvc", feature = "stt")),
    allow(dead_code, unused_imports)
)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

#[cfg(all(target_env = "msvc", feature = "stt"))]
use crate::SttService;

pub const SAMPLE_RATE: u32 = 16_000;
const POLL_INTERVAL: Duration = Duration::from_millis(60);
const MAX_AUDIO_QUEUE: usize = 96;
const PRE_ROLL_FRAMES: usize = 24;

#[derive(Debug, Clone)]
pub enum LiveEvent {
    SpeechStarted {
        turn_id: String,
        level: f32,
    },
    Partial {
        turn_id: String,
        text: String,
    },
    TurnFinal {
        turn_id: String,
        text: String,
        duration_ms: u64,
    },
    Error {
        message: String,
    },
}

#[cfg(not(all(target_env = "msvc", feature = "stt")))]
pub struct LiveSession {
    model: String,
}

#[cfg(not(all(target_env = "msvc", feature = "stt")))]
impl LiveSession {
    pub fn start(
        _stt: std::sync::Arc<crate::SttService>,
        _events: UnboundedSender<LiveEvent>,
    ) -> Result<Self> {
        anyhow::bail!("Moonshine live capture is only available in the MSVC build")
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn feed_pcm16(&self, _bytes: &[u8]) {}

    pub fn stop(&mut self) {}
}

#[cfg(all(target_env = "msvc", feature = "stt"))]
enum Command {
    Audio(Vec<f32>),
    Stop,
}

#[cfg(all(target_env = "msvc", feature = "stt"))]
struct Turn {
    id: String,
    started: Instant,
    stream: moonshine_rs::OwnedTranscriberStream,
    speech_ms: u64,
    silence_ms: u64,
    total_ms: u64,
    last_partial: String,
}

#[cfg(all(target_env = "msvc", feature = "stt"))]
pub struct LiveSession {
    audio_tx: SyncSender<Command>,
    worker: Option<JoinHandle<()>>,
    model: String,
}

#[cfg(all(target_env = "msvc", feature = "stt"))]
impl LiveSession {
    pub fn start(stt: Arc<SttService>, events: UnboundedSender<LiveEvent>) -> Result<Self> {
        let (audio_tx, audio_rx) = mpsc::sync_channel::<Command>(MAX_AUDIO_QUEUE);
        let model = stt.model_name().to_string();
        let worker = thread::Builder::new()
            .name("stark-live-stt".to_string())
            .spawn(move || run_session(stt, audio_rx, events))
            .context("failed to spawn live STT worker")?;
        Ok(Self {
            audio_tx,
            worker: Some(worker),
            model,
        })
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn feed_pcm16(&self, bytes: &[u8]) {
        if bytes.len() < 2 {
            return;
        }
        let mut pcm = Vec::with_capacity(bytes.len() / 2);
        for pair in bytes.as_chunks::<2>().0 {
            pcm.push(i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32768.0);
        }
        match self.audio_tx.try_send(Command::Audio(pcm)) {
            Ok(()) | Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    pub fn stop(&mut self) {
        let _ = self.audio_tx.try_send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(all(target_env = "msvc", feature = "stt"))]
impl Drop for LiveSession {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(all(target_env = "msvc", feature = "stt"))]
fn run_session(
    stt: Arc<SttService>,
    audio_rx: mpsc::Receiver<Command>,
    events: UnboundedSender<LiveEvent>,
) {
    let endpoint_ms = env_u64("VOICE_ENDPOINT_MS", 900).clamp(400, 3_000);
    let min_speech_ms = env_u64("VOICE_MIN_SPEECH_MS", 300).clamp(80, 1_500);
    let max_turn_ms = env_u64("VOICE_MAX_TURN_MS", 30_000).clamp(2_000, 120_000);
    let extended_endpoint_ms =
        env_u64("VOICE_ENDPOINT_EXTENDED_MS", 1_800).clamp(endpoint_ms, 4_000);
    let min_level = env_f32("VOICE_MIN_LEVEL", 0.008).clamp(0.001, 0.2);
    let mut noise_floor = 0.003f32;
    let mut pre_roll: VecDeque<Vec<f32>> = VecDeque::new();
    let mut turn: Option<Turn> = None;
    let mut last_poll = Instant::now();

    loop {
        match audio_rx.recv_timeout(POLL_INTERVAL) {
            Ok(Command::Stop) => break,
            Ok(Command::Audio(frame)) => {
                if frame.is_empty() {
                    continue;
                }
                let frame_ms = (frame.len() as u64 * 1000 / SAMPLE_RATE as u64).max(1);
                let rms = (frame.iter().map(|sample| sample * sample).sum::<f32>()
                    / frame.len() as f32)
                    .sqrt();
                let threshold = (min_level).max(noise_floor * 2.5);
                let voiced = rms >= threshold;

                if let Some(current) = turn.as_mut() {
                    current.total_ms += frame_ms;
                    if voiced {
                        current.speech_ms += frame_ms;
                        current.silence_ms = 0;
                    } else {
                        current.silence_ms += frame_ms;
                    }
                    if let Err(error) = current.stream.add_audio(&frame, SAMPLE_RATE) {
                        let _ = events.send(LiveEvent::Error {
                            message: format!("Moonshine add_audio failed: {error}"),
                        });
                        turn = None;
                        continue;
                    }

                    // Do not cut a thought after a short pause. If the decoder
                    // currently ends on a continuation word, allow a longer gap.
                    let endpoint = if ends_mid_thought(&current.last_partial) {
                        extended_endpoint_ms
                    } else {
                        endpoint_ms
                    };
                    let should_finish = (current.speech_ms >= min_speech_ms
                        && current.silence_ms >= endpoint)
                        || current.total_ms >= max_turn_ms;
                    if should_finish {
                        finish_turn(turn.take(), &events);
                    }
                } else if voiced {
                    let mut stream = match stt.create_stream() {
                        Ok(stream) => stream,
                        Err(error) => {
                            let _ = events.send(LiveEvent::Error {
                                message: format!("cannot start Moonshine stream: {error}"),
                            });
                            continue;
                        }
                    };
                    let turn_id = Uuid::new_v4().to_string();
                    for buffered in pre_roll.drain(..) {
                        if let Err(error) = stream.add_audio(&buffered, SAMPLE_RATE) {
                            let _ = events.send(LiveEvent::Error {
                                message: format!("Moonshine pre-roll failed: {error}"),
                            });
                        }
                    }
                    if let Err(error) = stream.add_audio(&frame, SAMPLE_RATE) {
                        let _ = events.send(LiveEvent::Error {
                            message: format!("Moonshine add_audio failed: {error}"),
                        });
                        continue;
                    }
                    let _ = events.send(LiveEvent::SpeechStarted {
                        turn_id: turn_id.clone(),
                        level: rms,
                    });
                    turn = Some(Turn {
                        id: turn_id,
                        started: Instant::now(),
                        stream,
                        speech_ms: frame_ms,
                        silence_ms: 0,
                        total_ms: frame_ms,
                        last_partial: String::new(),
                    });
                } else {
                    // Learn the room while idle, then keep a short pre-roll so
                    // the first syllable is never clipped.
                    noise_floor = (noise_floor * 0.98 + rms * 0.02).clamp(0.0005, 0.03);
                    pre_roll.push_back(frame);
                    while pre_roll.len() > PRE_ROLL_FRAMES {
                        pre_roll.pop_front();
                    }
                }

                if last_poll.elapsed() >= POLL_INTERVAL
                    && let Some(current) = turn.as_mut()
                {
                    last_poll = Instant::now();
                    match current.stream.poll(false) {
                        Ok(transcript) => {
                            let text = transcript.text().trim().replace('\n', " ");
                            if !text.is_empty() && text != current.last_partial {
                                current.last_partial = text.clone();
                                let _ = events.send(LiveEvent::Partial {
                                    turn_id: current.id.clone(),
                                    text,
                                });
                            }
                        }
                        Err(error) => {
                            let _ = events.send(LiveEvent::Error {
                                message: format!("Moonshine poll failed: {error}"),
                            });
                        }
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if last_poll.elapsed() >= POLL_INTERVAL
            && let Some(current) = turn.as_mut()
        {
            last_poll = Instant::now();
            match current.stream.poll(false) {
                Ok(transcript) => {
                    let text = transcript.text().trim().replace('\n', " ");
                    if !text.is_empty() && text != current.last_partial {
                        current.last_partial = text.clone();
                        let _ = events.send(LiveEvent::Partial {
                            turn_id: current.id.clone(),
                            text,
                        });
                    }
                }
                Err(error) => {
                    let _ = events.send(LiveEvent::Error {
                        message: format!("Moonshine poll failed: {error}"),
                    });
                }
            }
        }
    }
}

#[cfg(all(target_env = "msvc", feature = "stt"))]
fn finish_turn(turn: Option<Turn>, events: &UnboundedSender<LiveEvent>) {
    let Some(mut turn) = turn else {
        return;
    };
    // A short silence tail gives the decoder a clean end-of-utterance state.
    let silence = vec![0.0f32; (SAMPLE_RATE / 5) as usize];
    let _ = turn.stream.add_audio(&silence, SAMPLE_RATE);
    let result = turn.stream.finalize();
    match result {
        Ok(transcript) => {
            let text = transcript.text().trim().replace('\n', " ");
            if !text.is_empty() {
                let _ = events.send(LiveEvent::TurnFinal {
                    turn_id: turn.id,
                    text,
                    duration_ms: turn.started.elapsed().as_millis() as u64,
                });
            }
        }
        Err(error) => {
            let _ = events.send(LiveEvent::Error {
                message: format!("Moonshine finalize failed: {error}"),
            });
        }
    }
}

#[cfg(all(target_env = "msvc", feature = "stt"))]
#[cfg(all(target_env = "msvc", feature = "stt"))]
fn ends_mid_thought(text: &str) -> bool {
    let last = text
        .trim()
        .trim_end_matches(|character: char| character.is_ascii_punctuation())
        .split_whitespace()
        .last()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        last.as_str(),
        "a" | "an"
            | "and"
            | "are"
            | "as"
            | "at"
            | "be"
            | "because"
            | "but"
            | "by"
            | "can"
            | "could"
            | "do"
            | "does"
            | "for"
            | "from"
            | "how"
            | "i"
            | "if"
            | "in"
            | "is"
            | "it"
            | "me"
            | "my"
            | "of"
            | "on"
            | "or"
            | "so"
            | "that"
            | "the"
            | "their"
            | "then"
            | "there"
            | "these"
            | "they"
            | "this"
            | "to"
            | "was"
            | "we"
            | "were"
            | "what"
            | "when"
            | "where"
            | "which"
            | "who"
            | "why"
            | "will"
            | "with"
            | "would"
            | "you"
            | "your"
    )
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

#[cfg(all(target_env = "msvc", feature = "stt"))]
fn env_f32(name: &str, default: f32) -> f32 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}
