//! One live dictation session: PCM in, adaptive VAD, turn commit, Parakeet.
//!
//! The browser/app owns the microphone and sends 16 kHz mono PCM16 over the
//! voice WebSocket. This worker detects a turn with the same adaptive frame
//! classifier Hex uses, keeps a 450 ms pre-roll, and hands the complete turn
//! to a Parakeet turn worker. The turn worker streams when the model runs on a
//! GPU and otherwise decodes the complete clip offline.

#![cfg_attr(not(feature = "stt"), allow(dead_code, unused_imports))]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

use crate::stt::SttService;
#[cfg(feature = "stt")]
use crate::stt::{SpeechFrames, TurnHandle};

pub const LIVE_SAMPLE_RATE: u32 = 16_000;
const POLL_INTERVAL: Duration = Duration::from_millis(60);
/// A 30 s turn is 3,000 ten-millisecond frames. Keeping the whole turn means a
/// slow CPU or a busy websocket can fall back to the complete clip instead of
/// silently losing words.
const MAX_AUDIO_QUEUE: usize = 3_000;
/// 450 ms, Hex's hotkey pre-roll.
const PRE_ROLL_FRAMES: usize = 45;
const MIN_SPEECH_MS: u64 = 160;
const ENDPOINT_MS: u64 = 1_200;
const MAX_TURN_MS: u64 = 30_000;

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

#[cfg(not(feature = "stt"))]
pub struct LiveSession {
    model: String,
}

#[cfg(not(feature = "stt"))]
impl LiveSession {
    pub fn start(_stt: Arc<SttService>, _events: UnboundedSender<LiveEvent>) -> Result<Self> {
        anyhow::bail!("Parakeet STT is not available in this build")
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn feed_pcm16(&self, _bytes: &[u8]) {}

    pub fn stop(&mut self) {}
}

#[cfg(feature = "stt")]
enum Command {
    Audio(Vec<f32>),
    Stop,
}

#[cfg(feature = "stt")]
pub struct LiveSession {
    audio_tx: SyncSender<Command>,
    worker: Option<JoinHandle<()>>,
    model: String,
}

#[cfg(feature = "stt")]
impl LiveSession {
    pub fn start(stt: Arc<SttService>, events: UnboundedSender<LiveEvent>) -> Result<Self> {
        let (audio_tx, audio_rx) = mpsc::sync_channel::<Command>(MAX_AUDIO_QUEUE);
        let model = stt.model_name().to_string();
        let worker = thread::Builder::new()
            .name("stark-live-parakeet".to_string())
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
            pcm.push(i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32_768.0);
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

#[cfg(feature = "stt")]
impl Drop for LiveSession {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(feature = "stt")]
struct ActiveTurn {
    handle: TurnHandle,
    pcm: Vec<f32>,
    speech_ms: u64,
    silence_ms: u64,
    total_ms: u64,
    queue_full_warned: bool,
}

#[cfg(feature = "stt")]
fn run_session(
    stt: Arc<SttService>,
    audio_rx: mpsc::Receiver<Command>,
    events: UnboundedSender<LiveEvent>,
) {
    let mut speech = SpeechFrames::default();
    let mut pre_roll: VecDeque<Vec<f32>> = VecDeque::new();
    let mut turn: Option<ActiveTurn> = None;

    loop {
        match audio_rx.recv_timeout(POLL_INTERVAL) {
            Ok(Command::Stop) => {
                if let Some(active) = turn.take() {
                    finish_turn(active, &events);
                }
                break;
            }
            Ok(Command::Audio(frame)) => {
                if frame.is_empty() {
                    continue;
                }
                let frame_ms = (frame.len() as u64 * 1_000 / LIVE_SAMPLE_RATE as u64).max(1);
                let level = speech.classify(&frame);
                let voiced = level.speech;
                if let Some(active) = turn.as_mut() {
                    active.pcm.extend_from_slice(&frame);
                    active.total_ms += frame_ms;
                    if voiced {
                        active.speech_ms += frame_ms;
                        active.silence_ms = 0;
                    } else {
                        active.silence_ms += frame_ms;
                    }
                    if !active.handle.feed(frame.clone()) && !active.queue_full_warned {
                        active.queue_full_warned = true;
                        let _ = events.send(LiveEvent::Error {
                            message:
                                "Parakeet turn queue is full; the final will use the full clip"
                                    .to_string(),
                        });
                    }
                    let endpoint =
                        active.silence_ms >= ENDPOINT_MS && active.speech_ms >= MIN_SPEECH_MS;
                    if endpoint || active.total_ms >= MAX_TURN_MS {
                        let active = turn.take().unwrap();
                        finish_turn(active, &events);
                        // The pre-roll belongs to the next utterance, not the
                        // tail of the one that just ended. Queued audio after
                        // the endpoint refills it with silence.
                        pre_roll.clear();
                    }
                } else if voiced {
                    let turn_id = Uuid::new_v4().to_string();
                    let handle = match stt.start_turn(events.clone(), turn_id.clone()) {
                        Ok(handle) => handle,
                        Err(error) => {
                            let _ = events.send(LiveEvent::Error {
                                message: format!("cannot start Parakeet turn: {error:#}"),
                            });
                            continue;
                        }
                    };
                    let mut pcm = Vec::new();
                    let mut speech_ms = 0;
                    let mut total_ms = 0;
                    for buffered in pre_roll.drain(..) {
                        let buffered_ms =
                            (buffered.len() as u64 * 1_000 / LIVE_SAMPLE_RATE as u64).max(1);
                        pcm.extend_from_slice(&buffered);
                        let _ = handle.feed(buffered);
                        total_ms += buffered_ms;
                    }
                    pcm.extend_from_slice(&frame);
                    if !handle.feed(frame.clone()) {
                        let _ = events.send(LiveEvent::Error {
                            message: "Parakeet turn queue is full".to_string(),
                        });
                    }
                    speech_ms += frame_ms;
                    total_ms += frame_ms;
                    let _ = events.send(LiveEvent::SpeechStarted {
                        turn_id,
                        level: rms(&frame),
                    });
                    turn = Some(ActiveTurn {
                        handle,
                        pcm,
                        speech_ms,
                        silence_ms: 0,
                        total_ms,
                        queue_full_warned: false,
                    });
                } else {
                    pre_roll.push_back(frame);
                    while pre_roll.len() > PRE_ROLL_FRAMES {
                        pre_roll.pop_front();
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[cfg(feature = "stt")]
fn finish_turn(active: ActiveTurn, events: &UnboundedSender<LiveEvent>) {
    let _ = events;
    let clip = active.pcm.clone();
    active.handle.finish(clip);
}

#[cfg(feature = "stt")]
fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32)
        .sqrt()
        .clamp(0.0, 1.0)
}
