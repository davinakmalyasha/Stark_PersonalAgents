#![allow(dead_code, unused_imports)]

//! Server-side microphone capture and Moonshine streaming.
//!
//! The browser keeps its mic only for VAD/barge-in; the actual audio is captured here with
//! `cpal` and fed to Moonshine's native streaming API. This mirrors the Python `MicTranscriber`
//! lifecycle used by `voice.html`: start, mute, peek, flush, stop.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
#[cfg(all(target_env = "msvc", feature = "stt"))]
use cpal::{FromSample, Sample, SizedSample};

use crate::stt::SttService;

const AUDIO_QUEUE: usize = 128;
const POLL_INTERVAL: Duration = Duration::from_millis(60);
const READY_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Default, Clone)]
struct LiveState {
    active: bool,
    muted: bool,
    text: String,
    level: f32,
    error: String,
}

enum Command {
    Flush(Sender<Result<String>>),
    Stop {
        discard: bool,
        reply: Sender<Result<String>>,
    },
    Shutdown,
}

pub struct CaptureManager {
    stt: Arc<SttService>,
    state: Arc<Mutex<LiveState>>,
    commands: Mutex<Option<Sender<Command>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    start_gate: Mutex<()>,
}

impl CaptureManager {
    pub fn new(stt: Arc<SttService>) -> Self {
        Self {
            stt,
            state: Arc::new(Mutex::new(LiveState::default())),
            commands: Mutex::new(None),
            worker: Mutex::new(None),
            start_gate: Mutex::new(()),
        }
    }

    pub fn active(&self) -> bool {
        self.state.lock().map(|s| s.active).unwrap_or(false)
    }

    pub fn live_text(&self) -> String {
        self.state
            .lock()
            .map(|s| s.text.clone())
            .unwrap_or_default()
    }

    pub fn live_peak(&self) -> f32 {
        self.state.lock().map(|s| s.level).unwrap_or(0.0)
    }

    pub fn error(&self) -> String {
        self.state
            .lock()
            .map(|s| s.error.clone())
            .unwrap_or_default()
    }

    pub fn set_muted(&self, muted: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.muted = muted;
        }
    }

    pub fn start(&self) -> Result<bool> {
        #[cfg(not(all(target_env = "msvc", feature = "stt")))]
        {
            bail!("Moonshine microphone capture is only available in the MSVC build")
        }
        #[cfg(all(target_env = "msvc", feature = "stt"))]
        {
            let _gate = self.start_gate.lock().unwrap();
            if self.active() {
                return Ok(true);
            }
            if self.worker.lock().unwrap().is_some() {
                self.join_finished();
            }

            let (command_tx, command_rx) = mpsc::channel();
            let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
            let (audio_tx, audio_rx) = mpsc::sync_channel::<Vec<f32>>(AUDIO_QUEUE);
            let state = self.state.clone();
            let stt = self.stt.clone();
            {
                let mut state = state.lock().unwrap();
                *state = LiveState {
                    active: false,
                    ..LiveState::default()
                };
            }

            let handle = thread::Builder::new()
                .name("stark-mic".to_string())
                .spawn(move || capture_worker(stt, command_rx, audio_rx, audio_tx, state, ready_tx))
                .context("failed to spawn microphone thread")?;
            *self.worker.lock().unwrap() = Some(handle);

            match ready_rx.recv_timeout(READY_TIMEOUT) {
                Ok(Ok(())) => {
                    *self.commands.lock().unwrap() = Some(command_tx);
                    Ok(true)
                }
                Ok(Err(error)) => {
                    *self.commands.lock().unwrap() = None;
                    self.join_finished();
                    Err(error)
                }
                Err(_) => {
                    *self.commands.lock().unwrap() = None;
                    let _ = command_tx.send(Command::Shutdown);
                    self.join_finished();
                    bail!(
                        "microphone capture did not start within {}s",
                        READY_TIMEOUT.as_secs()
                    )
                }
            }
        }
    }

    pub fn flush_round(&self) -> Result<String> {
        let tx = self
            .commands
            .lock()
            .unwrap()
            .clone()
            .context("microphone capture is not running")?;
        let (reply_tx, reply_rx) = mpsc::channel();
        tx.send(Command::Flush(reply_tx))
            .map_err(|_| anyhow::anyhow!("microphone capture is not running"))?;
        reply_rx
            .recv_timeout(Duration::from_secs(30))
            .context("Moonshine flush timed out")?
    }

    pub fn stop(&self, discard: bool) -> Result<String> {
        let tx = self.commands.lock().unwrap().take();
        let Some(tx) = tx else {
            return Ok(String::new());
        };
        let (reply_tx, reply_rx) = mpsc::channel();
        if tx
            .send(Command::Stop {
                discard,
                reply: reply_tx,
            })
            .is_err()
        {
            self.join_finished();
            return Ok(String::new());
        }
        let result = reply_rx
            .recv_timeout(Duration::from_secs(30))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("Moonshine stop timed out")));
        self.join_finished();
        if let Ok(mut state) = self.state.lock() {
            state.active = false;
            state.text.clear();
            state.level = 0.0;
        }
        result
    }

    pub fn release(&self) {
        let _ = self.stop(true);
    }

    fn join_finished(&self) {
        let handle = self.worker.lock().unwrap().take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

impl Drop for CaptureManager {
    fn drop(&mut self) {
        let _ = self.stop(true);
    }
}

#[cfg(all(target_env = "msvc", feature = "stt"))]
fn capture_worker(
    stt: Arc<SttService>,
    command_rx: Receiver<Command>,
    audio_rx: Receiver<Vec<f32>>,
    audio_tx: SyncSender<Vec<f32>>,
    state: Arc<Mutex<LiveState>>,
    ready_tx: Sender<Result<()>>,
) {
    let result = run_capture(
        stt,
        command_rx,
        audio_rx,
        audio_tx,
        state.clone(),
        ready_tx.clone(),
    );
    if let Err(error) = result {
        if let Ok(mut state) = state.lock() {
            state.active = false;
            state.error = format!("{error:#}");
        }
        let _ = ready_tx.send(Err(error));
    }
}

#[cfg(all(target_env = "msvc", feature = "stt"))]
fn run_capture(
    stt: Arc<SttService>,
    command_rx: Receiver<Command>,
    audio_rx: Receiver<Vec<f32>>,
    audio_tx: SyncSender<Vec<f32>>,
    state: Arc<Mutex<LiveState>>,
    ready_tx: Sender<Result<()>>,
) -> Result<()> {
    let mut stream = stt
        .create_stream()
        .context("cannot start Moonshine stream")?;
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .context("no default microphone found")?;
    let supported = device
        .default_input_config()
        .context("cannot read default microphone config")?;
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let sample_rate = config.sample_rate.0;
    let channels = config.channels as usize;
    let error_state = state.clone();

    let input = match sample_format {
        cpal::SampleFormat::F32 => {
            build_stream::<f32>(&device, &config, channels, audio_tx, error_state)?
        }
        cpal::SampleFormat::I16 => {
            build_stream::<i16>(&device, &config, channels, audio_tx, error_state)?
        }
        cpal::SampleFormat::U16 => {
            build_stream::<u16>(&device, &config, channels, audio_tx, error_state)?
        }
        other => bail!("unsupported microphone sample format {other:?}"),
    };
    input.play().context("cannot start microphone stream")?;
    {
        let mut state = state.lock().unwrap();
        state.active = true;
        state.error.clear();
    }
    let _ = ready_tx.send(Ok(()));

    let speech_threshold = std::env::var("STARK_MIC_THRESHOLD")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| *value >= 0.0)
        .unwrap_or(0.008);
    let mut pre_roll: VecDeque<Vec<f32>> = VecDeque::new();
    let mut speech_active = false;
    let mut last_speech_at = Instant::now();
    let mut last_poll = Instant::now();
    let mut last_peak = Instant::now();
    loop {
        match command_rx.try_recv() {
            Ok(Command::Shutdown) => break,
            Ok(Command::Stop { discard, reply }) => {
                let text = if discard {
                    Ok(String::new())
                } else {
                    stream
                        .finalize()
                        .map(|t| t.text().trim().to_string())
                        .map_err(|e| anyhow::anyhow!("Moonshine finalize failed: {e}"))
                };
                let _ = reply.send(text);
                break;
            }
            Ok(Command::Flush(reply)) => {
                let text = stream
                    .finalize()
                    .map(|t| t.text().trim().to_string())
                    .map_err(|e| anyhow::anyhow!("Moonshine finalize failed: {e}"));
                if let Ok(mut state) = state.lock() {
                    state.text.clear();
                }
                match stt.create_stream() {
                    Ok(next) => {
                        stream = next;
                        let _ = reply.send(text);
                    }
                    Err(error) => {
                        let _ = reply.send(Err(error));
                        break;
                    }
                }
                continue;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => break,
        }

        let muted = state.lock().map(|s| s.muted).unwrap_or(true);
        let mut peak = 0.0f32;
        while let Ok(chunk) = audio_rx.try_recv() {
            if chunk.is_empty() {
                continue;
            }
            let chunk_peak = chunk.iter().map(|v| v.abs()).fold(0.0, f32::max);
            peak = peak.max(chunk_peak);
            if muted {
                continue;
            }

            // Moonshine will hallucinate words on open-air silence. Gate the
            // streaming decoder on actual speech, keeping a short pre-roll so
            // the first syllable is not clipped.
            let voiced = chunk_peak >= speech_threshold;
            if voiced {
                if !speech_active {
                    for buffered in pre_roll.drain(..) {
                        if let Err(error) = stream.add_audio(&buffered, sample_rate) {
                            log::warn!("Moonshine add_audio failed: {error}");
                        }
                    }
                    speech_active = true;
                }
                if let Err(error) = stream.add_audio(&chunk, sample_rate) {
                    log::warn!("Moonshine add_audio failed: {error}");
                }
                last_speech_at = Instant::now();
            } else if speech_active && last_speech_at.elapsed() < Duration::from_millis(350) {
                // Preserve the tail of the utterance for a natural EOS.
                if let Err(error) = stream.add_audio(&chunk, sample_rate) {
                    log::warn!("Moonshine add_audio failed: {error}");
                }
            } else {
                speech_active = false;
                pre_roll.push_back(chunk);
                while pre_roll.len() > 24 {
                    pre_roll.pop_front();
                }
                // Drop a stale hypothesis after a long quiet period; the next
                // voice round starts from a clean decoder state.
                if last_speech_at.elapsed() > Duration::from_millis(1600) {
                    if let Err(error) = stream.restart() {
                        log::warn!("Moonshine restart failed: {error}");
                    }
                    if let Ok(mut state) = state.lock() {
                        state.text.clear();
                    }
                    last_speech_at = Instant::now();
                }
            }
        }

        if last_poll.elapsed() >= POLL_INTERVAL {
            last_poll = Instant::now();
            match stream.poll(false) {
                Ok(transcript) => {
                    let text = transcript.text().trim().replace('\n', " ");
                    if let Ok(mut state) = state.lock() {
                        state.text = text;
                    }
                }
                Err(error) => log::warn!("Moonshine poll failed: {error}"),
            }
        }
        if last_peak.elapsed() >= Duration::from_millis(120) {
            last_peak = Instant::now();
            if let Ok(mut state) = state.lock() {
                state.level = peak;
            }
        }
        thread::sleep(Duration::from_millis(25));
    }

    if let Ok(mut state) = state.lock() {
        state.active = false;
        state.level = 0.0;
    }
    Ok(())
}

#[cfg(all(target_env = "msvc", feature = "stt"))]
fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    audio_tx: SyncSender<Vec<f32>>,
    state: Arc<Mutex<LiveState>>,
) -> Result<cpal::Stream>
where
    T: cpal::Sample + cpal::SizedSample + Send + 'static,
    f32: cpal::FromSample<T>,
{
    let error_cb = move |error| {
        log::error!("microphone stream error: {error}");
        if let Ok(mut state) = state.lock() {
            state.error = format!("microphone error: {error}");
        }
    };
    let data_cb = move |data: &[T], _: &cpal::InputCallbackInfo| {
        if data.is_empty() {
            return;
        }
        let mut mono = Vec::with_capacity(data.len() / channels.max(1));
        for frame in data.chunks(channels.max(1)) {
            let sum: f32 = frame.iter().map(|sample| f32::from_sample(*sample)).sum();
            mono.push(sum / channels.max(1) as f32);
        }
        match audio_tx.try_send(mono) {
            Ok(()) | Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {}
        }
    };
    device
        .build_input_stream(config, data_cb, error_cb, None)
        .context("cannot open microphone input stream")
}
