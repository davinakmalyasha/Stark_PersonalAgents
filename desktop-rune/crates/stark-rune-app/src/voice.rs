//! Mic capture + level metering behind the voice toggle. Dictation prefers
//! the backend bridge ([`crate::voice_ws`]): captured 16 kHz mono PCM is
//! streamed to `stark-server`, whose VAD/STT answers with
//! [`VoiceMsg::Transcript`]. When no server is reachable the on-device
//! producer (`stt`, MSVC builds) takes over, else capture degrades to
//! meter-only levels with an honest note.
//!
//! Native: cpal default input streams RMS blocks into an `mpsc` channel the
//! poll hook drains into `voice_level`. Web: `getUserMedia` binds later —
//! the toggle owns the affordance, and capture reports honestly when there
//! is no device. Pure math (`rms_f32`, `decay_level`, `lit_bars`) is tested;
//! hardware paths degrade to notes, never panics.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[cfg(not(target_arch = "wasm32"))]
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Messages from the capture thread to the UI thread.
#[derive(Debug, Clone, PartialEq)]
pub enum VoiceMsg {
    Level(f32),
    /// Interim (`is_final: false`) or final (`true`) dictation text from the
    /// speech thread. React's overlay protocol is the reference: `transcript`
    /// events refresh the live bubble in place, `pipeline-start` swaps it for
    /// the single canonical user message. Produced by the `stt` worker on
    /// MSVC + `voice-stt`; elsewhere the variant is exercised by tests.
    #[allow(dead_code)]
    Transcript {
        text: String,
        is_final: bool,
    },
    /// Non-fatal note (STT build limits, link problems): reported
    /// without stopping a live meter, unlike [`VoiceMsg::Error`].
    Note(String),
    Error(String),
}

/// All voice runtime state in one bus so every view (transcript, canvas,
/// personal) shares toggle/drain logic. `stop`/`rx` reset on clone: a copied
/// state never inherits a live microphone.
pub struct VoiceBus {
    pub live: bool,
    pub level: f32,
    pub note: Option<String>,
    /// Latest interim dictation (`transcript` events overwrite it in place,
    /// like React's `voice-live-input` bubble content).
    pub live_text: String,
    /// A finished utterance waiting for the poll hook to stage it into the
    /// composer draft (`take_final` consumes it once).
    pending_final: Option<String>,
    pub stop: Option<Arc<AtomicBool>>,
    pub rx: Option<std::sync::mpsc::Receiver<VoiceMsg>>,
}

impl std::fmt::Debug for VoiceBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoiceBus")
            .field("live", &self.live)
            .field("level", &self.level)
            .field("note", &self.note)
            .field("live_text", &self.live_text)
            .field("pending_final", &self.pending_final.is_some())
            .field("capturing", &self.stop.is_some())
            .finish()
    }
}

impl Clone for VoiceBus {
    fn clone(&self) -> Self {
        Self {
            live: self.live,
            level: self.level,
            note: self.note.clone(),
            live_text: self.live_text.clone(),
            pending_final: self.pending_final.clone(),
            stop: None,
            rx: None,
        }
    }
}

impl Default for VoiceBus {
    fn default() -> Self {
        Self {
            live: false,
            level: 0.0,
            note: None,
            live_text: String::new(),
            pending_final: None,
            stop: None,
            rx: None,
        }
    }
}

impl VoiceBus {
    /// Toggle capture. Returns true when a capture attempt started.
    /// Stopping always succeeds. Hardware failures (no device, OS error)
    /// arrive asynchronously as [`VoiceMsg::Error`] notes via [`VoiceBus::drain`].
    pub fn set_live(&mut self, on: bool) -> bool {
        if !on {
            if let Some(stop) = self.stop.take() {
                stop.store(true, Ordering::Relaxed);
            }
            self.rx = None;
            self.live = false;
            self.level = 0.0;
            self.live_text.clear();
            return false;
        }
        self.note = None;
        self.level = 0.0;
        self.live_text.clear();
        self.pending_final = None;
        #[cfg(target_arch = "wasm32")]
        {
            // getUserMedia binds here next; the affordance is live today.
            self.note = Some("mic capture binds next on web".to_string());
            self.live = false;
            return false;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            // Fail fast without spawning: a doomed thread only adds latency.
            if probe_input().is_none() {
                self.note = Some("no microphone found".to_string());
                self.live = false;
                return false;
            }
            let (tx, rx) = std::sync::mpsc::channel();
            let stop = Arc::new(AtomicBool::new(false));
            let flag = stop.clone();
            self.stop = Some(stop);
            self.rx = Some(rx);
            self.live = true;
            // Dictation prefers the backend bridge: the app streams PCM to
            // `stark-server` (`/api/voice/ws`), which owns VAD/STT — the
            // pre-rewrite split, and the only path that needs no speech model
            // in the app. Without a reachable server it falls back to the
            // on-device model (MSVC builds only), then to meter-only levels
            // with an honest note.
            let (pcm_tx, pcm_rx) = std::sync::mpsc::sync_channel(crate::stt::MAX_AUDIO_QUEUE);
            let host = crate::rune_transcript::DEFAULT_API_HOST;
            let port = crate::rune_transcript::api_port();
            let started = match crate::voice_ws::try_open(host, port) {
                Ok(socket) => crate::voice_ws::spawn_with(socket, pcm_rx, tx.clone(), flag.clone())
                    .map_err(Some),
                Err(server) => match crate::stt::spawn_producer(pcm_rx, tx.clone(), flag.clone()) {
                    Ok(()) => Ok(()),
                    Err(local) => Err(Some(format!(
                        "dictation unavailable — server: {server}; local: {local}"
                    ))),
                },
            };
            match started {
                Ok(()) => {
                    self.note = None;
                    std::thread::spawn(move || {
                        capture_loop(flag, tx, Some(pcm_tx));
                    });
                }
                Err(note) => {
                    self.note = note;
                    std::thread::spawn(move || capture_loop(flag, tx, None));
                }
            }
            true
        }
    }
    pub fn drain(&mut self, dt: f32) -> bool {
        let mut changed = false;
        while let Some(msg) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            match msg {
                VoiceMsg::Level(peak) => {
                    self.level = decay_level(self.level, peak, dt);
                    changed = true;
                }
                VoiceMsg::Transcript { text, is_final } => {
                    if is_final {
                        // Session end (`pipeline-start`): drop the live bubble,
                        // stop capture, and hand the canonical prompt to the
                        // poll hook for draft staging.
                        self.pending_final = Some(text);
                        self.live_text.clear();
                        self.live = false;
                        self.level = 0.0;
                        if let Some(stop) = self.stop.take() {
                            stop.store(true, Ordering::Relaxed);
                        }
                        self.rx = None;
                    } else {
                        self.live_text = text;
                    }
                    changed = true;
                }
                VoiceMsg::Note(note) => {
                    // Advisory only: the meter keeps running.
                    self.note = Some(note);
                    changed = true;
                }
                VoiceMsg::Error(note) => {
                    self.note = Some(note);
                    self.live = false;
                    self.level = 0.0;
                    changed = true;
                }
            }
        }
        // Idle decay keeps the meter falling between blocks.
        if self.live && self.level > 0.0 {
            let next = decay_level(self.level, 0.0, dt);
            if (next - self.level).abs() > f32::EPSILON {
                self.level = next;
                changed = true;
            }
        }
        changed
    }

    /// Consume a finished utterance once (the poll hook stages it into the
    /// composer draft, like React's `voiceInputText` dock effect).
    pub fn take_final(&mut self) -> Option<String> {
        self.pending_final.take()
    }

    /// Meter snapshot for the composer bars: (live, level, note).
    pub fn meter(&self) -> (bool, f32, Option<&str>) {
        (self.live, self.level, self.note.as_deref())
    }
}

/// RMS loudness in 0..1 of an f32 sample block.
pub fn rms_f32(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt().clamp(0.0, 1.0)
}

/// Meter dynamics: instant attack to a new peak, linear release otherwise.
pub fn decay_level(current: f32, peak: f32, dt: f32) -> f32 {
    if peak >= current {
        peak.clamp(0.0, 1.0)
    } else {
        (current - dt * 1.5).max(peak).max(0.0)
    }
}

/// How many of the 5 meter bars a level lights.
pub fn lit_bars(level: f32) -> usize {
    if level <= 0.0 {
        return 0;
    }
    ((level.clamp(0.0, 1.0) * 5.0).ceil() as usize).min(5)
}

/// Default input device name, if the OS reports one.
#[cfg(not(target_arch = "wasm32"))]
pub fn probe_input() -> Option<String> {
    cpal::default_host()
        .default_input_device()
        .and_then(|d| d.description().ok())
        .map(|d| d.name().to_string())
        .filter(|n| !n.trim().is_empty())
}

/// 16 kHz capture mode for the STT frontend: the config plus whether the
/// mono mixdown applies (stereo fallback). Mono F32/I16/U16 first, then
/// stereo with downmix. When no exact mode exists the caller falls back to
/// the device default rate and [`LinearResampler`] converts to 16 kHz.
#[cfg(not(target_arch = "wasm32"))]
fn prefer_16k(device: &cpal::Device) -> Option<(cpal::SupportedStreamConfig, bool)> {
    let mut best: Option<(u8, cpal::SupportedStreamConfig, bool)> = None;
    let Ok(ranges) = device.supported_input_configs() else {
        return None;
    };
    for range in ranges {
        let channels = range.channels();
        if channels != 1 && channels != 2 {
            continue;
        }
        if range.min_sample_rate() > crate::stt::SAMPLE_RATE
            || range.max_sample_rate() < crate::stt::SAMPLE_RATE
        {
            continue;
        }
        let rank = match (range.sample_format(), channels) {
            (cpal::SampleFormat::F32, 1) => 0,
            (cpal::SampleFormat::I16, 1) => 1,
            (cpal::SampleFormat::U16, 1) => 2,
            (cpal::SampleFormat::F32, _) => 3,
            (cpal::SampleFormat::I16, _) => 4,
            (cpal::SampleFormat::U16, _) => 5,
            _ => continue,
        };
        let better = best
            .as_ref()
            .is_none_or(|(best_rank, _, _)| rank < *best_rank);
        if better {
            best = Some((
                rank,
                range.with_sample_rate(crate::stt::SAMPLE_RATE),
                channels == 2,
            ));
        }
    }
    best.map(|(_, config, stereo)| (config, stereo))
}

/// Streaming linear resampler: WASAPI shared mode usually exposes only
/// 48 kHz while the STT frontend expects 16 kHz. Linear interpolation keeps
/// phase continuity across capture blocks (fractional position + the last
/// sample of the previous block), so block boundaries never click.
#[cfg(not(target_arch = "wasm32"))]
struct LinearResampler {
    /// Input samples per output sample (`from / to`).
    step: f64,
    /// Next output position inside the current block, in input samples.
    pos: f64,
    /// Last input sample of the previous block (interpolation carry).
    prev: f32,
    primed: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl LinearResampler {
    fn new(from: u32, to: u32) -> Self {
        Self {
            step: from as f64 / to as f64,
            pos: 0.0,
            prev: 0.0,
            primed: false,
        }
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let n = input.len();
        if n == 0 {
            return;
        }
        let mut pos = self.pos;
        while pos < n as f64 {
            let index = pos.floor() as isize;
            let frac = (pos - index as f64) as f32;
            // An interpolating partner lives in the next block, except when
            // the position lands exactly on the last sample (nothing to
            // interpolate): keep the fractional position for the next call
            // so the join is seamless.
            if frac > 0.0 && index + 1 >= n as isize {
                break;
            }
            let a = if index < 0 {
                if self.primed {
                    self.prev
                } else {
                    input[0]
                }
            } else {
                input[index as usize]
            };
            let b = if index + 1 < n as isize {
                input[(index + 1) as usize]
            } else {
                a
            };
            out.push(a + (b - a) * frac);
            pos += self.step;
        }
        self.pos = pos - n as f64;
        self.prev = input[n - 1];
        self.primed = true;
    }
}

/// One capture block on its way to the STT worker: pass-through when the
/// device already runs at 16 kHz, resampled otherwise.
#[cfg(not(target_arch = "wasm32"))]
struct PcmForward {
    tx: std::sync::mpsc::SyncSender<Vec<f32>>,
    resampler: Option<LinearResampler>,
    out: Vec<f32>,
}

#[cfg(not(target_arch = "wasm32"))]
impl PcmForward {
    fn new(tx: std::sync::mpsc::SyncSender<Vec<f32>>, rate: u32) -> Self {
        Self {
            tx,
            resampler: (rate != crate::stt::SAMPLE_RATE)
                .then(|| LinearResampler::new(rate, crate::stt::SAMPLE_RATE)),
            out: Vec::new(),
        }
    }

    fn push(&mut self, mono: Vec<f32>) {
        match self.resampler.as_mut() {
            Some(resampler) => {
                self.out.clear();
                resampler.process(&mono, &mut self.out);
                if !self.out.is_empty() {
                    let out = std::mem::take(&mut self.out);
                    let _ = self.tx.try_send(out);
                }
            }
            None => {
                let _ = self.tx.try_send(mono);
            }
        }
    }
}

/// When `pcm` is `Some`, every block is downmixed to mono and forwarded to
/// the STT worker at 16 kHz (dropped on overflow, like the backend): an
/// exact 16 kHz device mode is used when offered, otherwise the captured
/// rate is resampled ([`LinearResampler`]) — WASAPI shared mode usually
/// exposes only 48 kHz. Owns the cpal `Stream` (dropping it stops the
/// device). Never panics: failures arrive as [`VoiceMsg::Error`].
#[cfg(not(target_arch = "wasm32"))]
pub fn capture_loop(
    stop: Arc<AtomicBool>,
    tx: std::sync::mpsc::Sender<VoiceMsg>,
    pcm: Option<std::sync::mpsc::SyncSender<Vec<f32>>>,
) {
    // Each callback owns a cloned sender: everything handed to cpal must be
    // `'static` (the stream outlives this frame).
    let emit = |tx: &std::sync::mpsc::Sender<VoiceMsg>| tx.clone();
    let fail = |tx: &std::sync::mpsc::Sender<VoiceMsg>, msg: String| {
        let _ = tx.send(VoiceMsg::Error(msg));
    };
    let host = cpal::default_host();
    let Some(device) = host.default_input_device() else {
        fail(&tx, "no microphone found".to_string());
        return;
    };
    // 16 kHz mono is the STT frontend's native rate: prefer an exact mode so
    // the common case ships no resampling. Any other rate still works — the
    // bridge resamples rather than disabling dictation (48 kHz-only mics are
    // the norm on Windows shared mode).
    let forward = pcm;
    let mut stereo = false;
    let config = if forward.is_none() {
        match device.default_input_config() {
            Ok(c) => c,
            Err(e) => {
                fail(&tx, format!("mic config: {e}"));
                return;
            }
        }
    } else {
        match prefer_16k(&device) {
            Some((picked, downmix)) => {
                stereo = downmix;
                picked
            }
            None => match device.default_input_config() {
                Ok(c) => {
                    stereo = c.channels() >= 2;
                    c
                }
                Err(e) => {
                    fail(&tx, format!("mic config: {e}"));
                    return;
                }
            },
        }
    };
    let rate = config.sample_rate();
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => {
            let (dtx, etx) = (emit(&tx), emit(&tx));
            let mut ptx = forward.map(|tx| PcmForward::new(tx, rate));
            device.build_input_stream(
                config.into(),
                move |data: &[f32], _| {
                    let mono = if stereo {
                        crate::stt::mono_mix(data, 2)
                    } else {
                        data.to_vec()
                    };
                    let _ = dtx.send(VoiceMsg::Level(rms_f32(&mono)));
                    if let Some(fwd) = ptx.as_mut() {
                        fwd.push(mono);
                    }
                },
                move |e| {
                    let _ = etx.send(VoiceMsg::Error(format!("mic: {e}")));
                },
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let (dtx, etx) = (emit(&tx), emit(&tx));
            let mut ptx = forward.map(|tx| PcmForward::new(tx, rate));
            device.build_input_stream(
                config.into(),
                move |data: &[i16], _| {
                    let mono = if stereo {
                        crate::stt::mono_mix(
                            &data
                                .iter()
                                .map(|s| *s as f32 / i16::MAX as f32)
                                .collect::<Vec<_>>(),
                            2,
                        )
                    } else {
                        data.iter()
                            .map(|s| *s as f32 / i16::MAX as f32)
                            .collect::<Vec<_>>()
                    };
                    let _ = dtx.send(VoiceMsg::Level(rms_f32(&mono)));
                    if let Some(fwd) = ptx.as_mut() {
                        fwd.push(mono);
                    }
                },
                move |e| {
                    let _ = etx.send(VoiceMsg::Error(format!("mic: {e}")));
                },
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let (dtx, etx) = (emit(&tx), emit(&tx));
            let mut ptx = forward.map(|tx| PcmForward::new(tx, rate));
            device.build_input_stream(
                config.into(),
                move |data: &[u16], _| {
                    let mono = if stereo {
                        crate::stt::mono_mix(
                            &data
                                .iter()
                                .map(|s| (*s as f32 / u16::MAX as f32) * 2.0 - 1.0)
                                .collect::<Vec<_>>(),
                            2,
                        )
                    } else {
                        data.iter()
                            .map(|s| (*s as f32 / u16::MAX as f32) * 2.0 - 1.0)
                            .collect::<Vec<_>>()
                    };
                    let _ = dtx.send(VoiceMsg::Level(rms_f32(&mono)));
                    if let Some(fwd) = ptx.as_mut() {
                        fwd.push(mono);
                    }
                },
                move |e| {
                    let _ = etx.send(VoiceMsg::Error(format!("mic: {e}")));
                },
                None,
            )
        }
        fmt => {
            fail(&tx, format!("unsupported mic format: {fmt:?}"));
            return;
        }
    };
    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            fail(&tx, format!("mic open: {e}"));
            return;
        }
    };
    if let Err(e) = stream.play() {
        fail(&tx, format!("mic play: {e}"));
        return;
    }
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_measures_loudness() {
        assert_eq!(rms_f32(&[]), 0.0);
        assert_eq!(rms_f32(&[0.0, 0.0]), 0.0);
        assert!((rms_f32(&[1.0, -1.0]) - 1.0).abs() < 1e-6);
        assert!((rms_f32(&[0.5, -0.5, 0.5, -0.5]) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn decay_attacks_instantly_releases_linearly() {
        assert_eq!(decay_level(0.2, 0.8, 0.016), 0.8);
        let next = decay_level(0.8, 0.1, 0.1);
        assert!((next - 0.65).abs() < 1e-6, "{next}");
        assert_eq!(decay_level(0.0, 0.0, 1.0), 0.0);
    }

    #[test]
    fn bars_light_by_threshold() {
        assert_eq!(lit_bars(0.0), 0);
        assert_eq!(lit_bars(0.01), 1);
        assert_eq!(lit_bars(0.5), 3);
        assert_eq!(lit_bars(1.0), 5);
        assert_eq!(lit_bars(2.0), 5);
    }

    /// WASAPI shared mode is usually 48 kHz-only; decimation must keep the
    /// 3:1 phase across capture blocks (no duplicate or skipped sample).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn resampler_downsamples_48k_to_16k_across_blocks() {
        let mut resampler = LinearResampler::new(48_000, 16_000);
        let mut out = Vec::new();
        resampler.process(&[0.0, 3.0, 6.0], &mut out);
        assert_eq!(out, vec![0.0]);
        out.clear();
        resampler.process(&[9.0, 12.0, 15.0], &mut out);
        assert_eq!(out, vec![9.0], "next decimated sample keeps the 3:1 phase");
    }

    /// Upsampling interpolates across the block boundary instead of
    /// repeating or dropping the join sample.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn resampler_upsamples_8k_to_16k_continuously() {
        let mut resampler = LinearResampler::new(8_000, 16_000);
        let mut out = Vec::new();
        resampler.process(&[0.0, 2.0], &mut out);
        assert_eq!(out, vec![0.0, 1.0, 2.0]);
        out.clear();
        resampler.process(&[4.0, 6.0], &mut out);
        assert_eq!(
            out,
            vec![3.0, 4.0, 5.0, 6.0],
            "carry interpolates 2..4 across the boundary"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn forward_passes_16k_through_and_resamples_48k() {
        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        let mut direct = PcmForward::new(tx, 16_000);
        direct.push(vec![0.5, 0.5]);
        assert_eq!(rx.try_recv().expect("passthrough"), vec![0.5, 0.5]);

        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        let mut resampled = PcmForward::new(tx, 48_000);
        resampled.push(vec![0.0, 0.3, 0.6]);
        assert_eq!(rx.try_recv().expect("resampled").len(), 1);
    }

    #[test]
    fn note_reports_without_killing_live() {
        let mut bus = VoiceBus::default();
        let (tx, rx) = std::sync::mpsc::channel();
        bus.rx = Some(rx);
        bus.live = true;
        bus.level = 0.4;
        tx.send(VoiceMsg::Note(
            "dictation: Moonshine STT is not available in this build".to_string(),
        ))
        .expect("send note");
        assert!(bus.drain(1.0 / 60.0));
        assert_eq!(
            bus.note.as_deref(),
            Some("dictation: Moonshine STT is not available in this build")
        );
        assert!(bus.live, "advisory only");
        assert!(bus.level > 0.0, "meter keeps running");
    }

    /// Hardware-dependent by nature: asserts only that probing never panics
    /// and, when a device exists, names something non-empty.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn probe_never_panics() {
        if let Some(name) = probe_input() {
            assert!(!name.trim().is_empty());
        }
    }

    #[test]
    fn interim_overwrites_and_final_stages_once() {
        let mut bus = VoiceBus::default();
        let (tx, rx) = std::sync::mpsc::channel();
        bus.rx = Some(rx);
        bus.live = true;
        tx.send(VoiceMsg::Transcript {
            text: "hello".to_string(),
            is_final: false,
        })
        .expect("send interim");
        tx.send(VoiceMsg::Transcript {
            text: "hello world".to_string(),
            is_final: false,
        })
        .expect("send interim");
        assert!(bus.drain(1.0 / 60.0));
        // Interim events overwrite in place (React's `voice-live-input`).
        assert_eq!(bus.live_text, "hello world");
        assert!(bus.live);
        tx.send(VoiceMsg::Transcript {
            text: "hello world".to_string(),
            is_final: true,
        })
        .expect("send final");
        assert!(bus.drain(1.0 / 60.0));
        // Final drops the bubble, ends the session, and hands the canonical
        // prompt over exactly once (`pipeline-start`).
        assert!(bus.live_text.is_empty());
        assert!(!bus.live);
        assert_eq!(bus.take_final().as_deref(), Some("hello world"));
        assert!(bus.take_final().is_none());
    }
}
