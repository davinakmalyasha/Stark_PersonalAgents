//! Backend-assisted dictation — "the bridge": capture stays local, speech
//! recognition runs in `stark-server`.
//!
//! The app streams 16 kHz mono PCM (`i16` little-endian) over the backend's
//! existing `/api/voice/ws` endpoint (`screen_routes::voice_ws_handler`) and
//! maps the server's events onto [`VoiceMsg::Transcript`], so the overlay,
//! live-bubble and draft-staging path is unchanged from the older producer.
//! The server owns VAD, pre-roll, endpointing and decoding, so no speech
//! model — and therefore no MSVC prebuilt — is linked into the app. That is
//! the same split the pre-rewrite shell used (`7b5810b`: browser PCM, server
//! VAD/STT, turn commit).
//!
//! Wire protocol (server side is authoritative):
//! * app → server: `{"type":"session.start"}`, binary PCM16 frames,
//!   `{"type":"session.stop"}`;
//! * server → app: `session.ready`, `transcript` (interim), `turn_final`,
//!   `session.error`, `session.stopped` (`live_event_json`).

#[cfg(not(target_arch = "wasm32"))]
use std::io::ErrorKind;
#[cfg(not(target_arch = "wasm32"))]
use std::net::{TcpStream, ToSocketAddrs};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use tungstenite::error::Error as WsError;
#[cfg(not(target_arch = "wasm32"))]
use tungstenite::Message;

#[cfg(not(target_arch = "wasm32"))]
use crate::voice::VoiceMsg;

/// Endpoint mounted by `stark-server` (`main.rs` route table).
pub const VOICE_WS_PATH: &str = "/api/voice/ws";

/// Localhost connect budget: a refused connection fails instantly; this only
/// guards against a wedged listener so the mic toggle never hangs the UI.
#[cfg(not(target_arch = "wasm32"))]
const CONNECT_TIMEOUT: Duration = Duration::from_millis(400);
#[cfg(not(target_arch = "wasm32"))]
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(1_500);
/// Session read poll. Tungstenite buffers partial frames, so a timed-out
/// read is the normal idle case; the short gap keeps audio frames flowing
/// from the capture channel while we wait for transcript events.
#[cfg(not(target_arch = "wasm32"))]
const READ_POLL: Duration = Duration::from_millis(120);

#[cfg(not(target_arch = "wasm32"))]
type Ws = tungstenite::WebSocket<TcpStream>;

/// The server events we act on (`live_event_json`); everything else
/// (`speech_started`, `pong`, unknown types) is ignored.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerEvent {
    Ready,
    Partial(String),
    Final(String),
    Error(String),
    Stopped,
    Ignore,
}

/// Parse one server text frame. Malformed JSON and unknown types are
/// ignored rather than fatal: the link stays usable.
#[cfg(not(target_arch = "wasm32"))]
pub fn parse_event(json: &str) -> ServerEvent {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return ServerEvent::Ignore;
    };
    let text_of = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    match value.get("type").and_then(serde_json::Value::as_str) {
        Some("session.ready") => ServerEvent::Ready,
        Some("transcript") => ServerEvent::Partial(text_of("text")),
        Some("turn_final") => ServerEvent::Final(text_of("text")),
        Some("session.error") => ServerEvent::Error(text_of("error")),
        Some("session.stopped") => ServerEvent::Stopped,
        _ => ServerEvent::Ignore,
    }
}

/// Convert captured `f32` samples (`-1.0..=1.0`) to interleaved-free mono
/// `i16` little-endian bytes, which `LiveSession::feed_pcm16` reads back as
/// `i16` pairs. NaN maps to silence and out-of-range values clamp.
#[cfg(not(target_arch = "wasm32"))]
pub fn pcm16le(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for &sample in samples {
        let value = if sample.is_nan() {
            0.0
        } else {
            sample.clamp(-1.0, 1.0)
        };
        let pcm = (value * i16::MAX as f32).round() as i16;
        bytes.extend_from_slice(&pcm.to_le_bytes());
    }
    bytes
}

/// Connect + handshake + `session.start`. Returns the open socket on
/// success; every failure is a one-line reason the caller can fall back on.
#[cfg(not(target_arch = "wasm32"))]
pub fn try_open(host: &str, port: u16) -> Result<Ws, String> {
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("voice link {host}:{port}: {e}"))?
        .next()
        .ok_or_else(|| format!("voice link {host}:{port}: no address"))?;
    let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .map_err(|e| format!("voice link {host}:{port}: {e}"))?;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    let _ = stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT));
    let url = format!("ws://{host}:{port}{VOICE_WS_PATH}");
    let (mut socket, _response) = tungstenite::client::client(url.as_str(), stream)
        .map_err(|e| format!("voice link {host}:{port}: {e}"))?;
    // Dialogue mode: short polls instead of one blocking read.
    let _ = socket.get_ref().set_read_timeout(Some(READ_POLL));
    socket
        .send(Message::text(r#"{"type":"session.start"}"#))
        .map_err(|e| format!("voice link {host}:{port}: {e}"))?;
    Ok(socket)
}

/// Run the session on its own worker: forward captured PCM, surface server
/// transcripts, honour the stop flag. The socket is already open, so this
/// only fails when the OS refuses to start the thread.
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_with(
    socket: Ws,
    pcm_rx: Receiver<Vec<f32>>,
    tx: Sender<VoiceMsg>,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name("stark-voice-ws".to_string())
        .spawn(move || run_session(socket, pcm_rx, tx, stop))
        .map(|_| ())
        .map_err(|e| format!("voice worker: {e}"))
}

#[cfg(not(target_arch = "wasm32"))]
fn run_session(
    mut socket: Ws,
    pcm_rx: Receiver<Vec<f32>>,
    tx: Sender<VoiceMsg>,
    stop: Arc<AtomicBool>,
) {
    let mut ready = false;
    // The server may still be loading Parakeet when it accepts the socket.
    // Keep a few seconds of the first utterance instead of dropping it.
    let mut pending: Vec<Vec<f32>> = Vec::new();
    'session: loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match socket.read() {
            Ok(Message::Text(text)) => match parse_event(text.as_str()) {
                ServerEvent::Ready => {
                    ready = true;
                    for frame in pending.drain(..) {
                        if let Err(e) = socket.send(Message::binary(pcm16le(&frame))) {
                            let _ = tx.send(VoiceMsg::Note(format!("voice link lost: {e}")));
                            break 'session;
                        }
                    }
                }
                ServerEvent::Partial(text) => {
                    let _ = tx.send(VoiceMsg::Transcript {
                        text,
                        is_final: false,
                    });
                }
                // Final utterance: the existing bus ends capture and stages
                // the draft, exactly like the old local final.
                ServerEvent::Final(text) => {
                    let _ = tx.send(VoiceMsg::Transcript {
                        text,
                        is_final: true,
                    });
                    break;
                }
                ServerEvent::Error(message) => {
                    // Honest note (e.g. a server built without Parakeet); the
                    // meter keeps running, matching the capture failure mode.
                    let _ = tx.send(VoiceMsg::Note(format!("dictation: {message}")));
                    break;
                }
                ServerEvent::Stopped => break,
                ServerEvent::Ignore => {}
            },
            Ok(Message::Close(_)) => break,
            // Ping/Pong are answered by tungstenite; other frames are noise.
            Ok(_) => {}
            Err(WsError::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => {
                let _ = tx.send(VoiceMsg::Note(format!("voice link closed: {e}")));
                break;
            }
        }
        // Drain the bounded capture queue. Before `session.ready`, keep a
        // short pre-roll instead of discarding the first words of a cold
        // `session.start` (the model load can take seconds).
        loop {
            match pcm_rx.try_recv() {
                Ok(frame) => {
                    if !ready {
                        if pending.len() < 3_000 {
                            pending.push(frame);
                        }
                        continue;
                    }
                    if let Err(e) = socket.send(Message::binary(pcm16le(&frame))) {
                        let _ = tx.send(VoiceMsg::Note(format!("voice link lost: {e}")));
                        break 'session;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break 'session,
            }
        }
    }
    let _ = socket.send(Message::text(r#"{"type":"session.stop"}"#));
    let _ = socket.close(None);
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn pcm16le_clamps_nan_and_roundtrips() {
        let bytes = pcm16le(&[0.0, 1.0, -1.0, 0.5, f32::NAN, 2.0]);
        assert_eq!(bytes.len(), 12, "two bytes per sample");
        assert_eq!(&bytes[0..2], &[0, 0], "silence");
        assert_eq!(&bytes[2..4], &[0xff, 0x7f], "+1.0 is i16::MAX");
        assert_eq!(&bytes[4..6], &[0x01, 0x80], "-1.0 is -i16::MAX");
        assert_eq!(&bytes[6..8], &[0x00, 0x40], "0.5 is half scale");
        assert_eq!(&bytes[8..10], &[0, 0], "NaN is silence");
        assert_eq!(&bytes[10..12], &[0xff, 0x7f], "out of range clamps");
    }

    #[test]
    fn server_events_map_to_the_voice_protocol() {
        assert_eq!(
            parse_event(r#"{"type":"session.ready","model":"parakeet-unified-en-0.6b"}"#),
            ServerEvent::Ready
        );
        assert_eq!(
            parse_event(r#"{"type":"transcript","turn_id":"t1","text":"hel","final":false}"#),
            ServerEvent::Partial("hel".to_string())
        );
        assert_eq!(
            parse_event(r#"{"type":"turn_final","turn_id":"t1","text":"hello","duration_ms":900}"#),
            ServerEvent::Final("hello".to_string())
        );
        assert_eq!(
            parse_event(r#"{"type":"session.error","error":"Parakeet STT is not available"}"#),
            ServerEvent::Error("Parakeet STT is not available".to_string())
        );
        assert_eq!(
            parse_event(r#"{"type":"session.stopped"}"#),
            ServerEvent::Stopped
        );
        assert_eq!(
            parse_event(r#"{"type":"speech_started","level":0.4}"#),
            ServerEvent::Ignore
        );
        assert_eq!(parse_event("not json"), ServerEvent::Ignore);
    }

    /// End-to-end against a real tungstenite server: start handshake, PCM
    /// frames as binary `i16`, partial then final mapped onto `VoiceMsg`.
    #[test]
    fn streams_pcm_and_maps_server_transcripts() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let port = listener.local_addr().expect("addr").port();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("timeout");
            let mut socket = tungstenite::accept(stream).expect("ws handshake");
            let start = socket.read().expect("session.start");
            assert!(
                matches!(&start, Message::Text(t) if t.as_str().contains("session.start")),
                "expected session.start, got {start:?}"
            );
            socket
                .send(Message::text(r#"{"type":"session.ready","model":"test"}"#))
                .expect("ready");
            let audio_bytes = loop {
                match socket.read().expect("audio") {
                    Message::Binary(bytes) => break bytes.len(),
                    Message::Ping(_) | Message::Pong(_) => {}
                    other => panic!("expected binary PCM, got {other:?}"),
                }
            };
            socket
                .send(Message::text(
                    r#"{"type":"transcript","turn_id":"t1","text":"hello","final":false}"#,
                ))
                .expect("partial");
            socket
                .send(Message::text(
                    r#"{"type":"turn_final","turn_id":"t1","text":"hello world","duration_ms":900}"#,
                ))
                .expect("final");
            // Keep the link up until the client acknowledges the stop, so
            // the final event is never lost to a close race.
            loop {
                match socket.read() {
                    Ok(Message::Text(text)) if text.as_str().contains("session.stop") => break,
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            audio_bytes
        });

        let (pcm_tx, pcm_rx) = std::sync::mpsc::channel();
        let (tx, rx) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let socket = try_open("127.0.0.1", port).expect("open voice link");
        spawn_with(socket, pcm_rx, tx, stop).expect("spawn worker");
        // The server answers a single frame, so later sends may race the
        // worker's exit (final receipt stops the session): only the first
        // frame must reach it.
        for _ in 0..20 {
            if pcm_tx.send(vec![0.25f32; 160]).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        let mut partial = None;
        let mut final_text = None;
        for _ in 0..100 {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(VoiceMsg::Transcript {
                    text,
                    is_final: false,
                }) => partial = Some(text),
                Ok(VoiceMsg::Transcript {
                    text,
                    is_final: true,
                }) => {
                    final_text = Some(text);
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        assert_eq!(partial.as_deref(), Some("hello"));
        assert_eq!(final_text.as_deref(), Some("hello world"));
        let audio_bytes = server.join().expect("server thread");
        assert_eq!(audio_bytes, 160 * 2, "one captured frame as i16 LE");
    }

    #[test]
    fn unreachable_server_returns_a_reason() {
        // Port 1 is never a Stark server on this box; the failure must be a
        // string (the caller falls back), not a panic.
        let err = try_open("127.0.0.1", 1).expect_err("must fail");
        assert!(err.starts_with("voice link 127.0.0.1:1"), "got: {err}");
    }
}
