//! Live-turn smoke tests against the real Parakeet + Silero models. They are
//! ignored because they need the shared model bundle; set STARK_MODELS_DIR
//! and STARK_LIVE_SPEECH_WAV (16 kHz mono) to run them.

#![cfg(feature = "stt")]

use std::path::Path;
use std::sync::Arc;

use stark_voice::{
    LiveEvent, LiveSession, SttService, Vocabulary, VocabularyBoost, VocabularyEntry,
};

fn wav_16k(path: &str) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).expect("open speech WAV");
    let spec = reader.spec();
    assert_eq!(spec.sample_rate, 16_000, "the live path expects 16 kHz");
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|sample| sample.expect("read sample") as f32 / 32_768.0)
        .collect();
    if spec.channels == 1 {
        return samples;
    }
    samples
        .chunks(spec.channels as usize)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect()
}

fn pcm16(samples: &[f32]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|&sample| {
            let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
            value.to_le_bytes()
        })
        .collect()
}

/// Feed 20 ms blocks, like the app's capture thread and the server's
/// adaptive frame classifier expect.
fn feed(session: &LiveSession, samples: &[f32]) {
    for block in samples.chunks(320) {
        session.feed_pcm16(&pcm16(block));
    }
}

fn model_dir() -> String {
    std::env::var("STARK_MODELS_DIR").expect("set STARK_MODELS_DIR")
}

fn drain(mut rx: tokio::sync::mpsc::UnboundedReceiver<LiveEvent>) -> Vec<LiveEvent> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}

#[test]
#[ignore = "needs STARK_MODELS_DIR and STARK_LIVE_SPEECH_WAV"]
fn live_turn_transcribes_with_the_saved_vocabulary() {
    let stt = Arc::new(SttService::load(Path::new(&model_dir())).unwrap());
    assert!(stt.vad_installed());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let vocabulary = Vocabulary {
        entries: vec![VocabularyEntry {
            term: "OpenCode".into(),
            sounds_like: vec!["open code".into()],
        }],
        boost: VocabularyBoost::Normal,
    };
    let mut session = LiveSession::start(stt, tx, vocabulary).unwrap();
    let audio = std::env::var("STARK_LIVE_SPEECH_WAV").expect("set STARK_LIVE_SPEECH_WAV");
    feed(&session, &wav_16k(&audio));
    session.stop();
    let events = drain(rx);
    let text = events
        .iter()
        .find_map(|event| match event {
            LiveEvent::TurnFinal { text, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no final in {events:?}"));
    assert!(!text.trim().is_empty(), "final text: {text:?}");
    println!("{text}");
}

#[test]
#[ignore = "needs STARK_MODELS_DIR"]
fn a_loud_non_speech_turn_is_discarded_before_parakeet() {
    let stt = Arc::new(SttService::load(Path::new(&model_dir())).unwrap());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut session =
        LiveSession::start(stt, tx, Vocabulary::default()).expect("start live session");
    // Quiet room, a loud 500 ms tone (passes the level gate, is not speech),
    // then enough silence for the adaptive endpoint.
    let mut samples = vec![0.0; 8_000];
    let tone: Vec<f32> = (0..8_000)
        .map(|index| (index as f32 * 0.25).sin() * 0.15)
        .collect();
    samples.extend(tone);
    samples.extend(std::iter::repeat_n(0.0, 24_000));
    feed(&session, &samples);
    session.stop();
    let events = drain(rx);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, LiveEvent::TurnDiscarded { .. })),
        "expected a discarded turn, got {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, LiveEvent::TurnFinal { .. })),
        "a tone must not become a transcript: {events:?}"
    );
}
