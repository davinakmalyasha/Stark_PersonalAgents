//! Blocking HTTP transport for `POST /api/run` with incremental SSE decode.
//!
//! `std` only (no async runtime): the Rune UI thread never blocks — the send
//! handler spawns one thread running [`post_run`], which streams frames into
//! an `mpsc` channel drained by the `poll` hook before every frame.
//!
//! Framing mirrors `frontend/src/services/api/stream.ts`: `event:` + `data:`
//! lines, blank-line dispatch, trailing-buffer flush. Bodies may be
//! `Transfer-Encoding: chunked` (axum SSE) or close-delimited.

#[cfg(not(target_arch = "wasm32"))]
use std::io::{BufRead, BufReader, Read, Write};

#[cfg(not(target_arch = "wasm32"))]
use std::net::TcpStream;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use crate::stream::{infer_event, RunRequest, SseFrame};

/// Incremental SSE decoder over raw bytes. Handles UTF-8 split across chunks
/// (only complete lines up to the last valid `\n` are processed) and keeps
/// `event:`/`data:` state across pushes.
#[derive(Debug, Default)]
pub struct SseDecoder {
    raw: Vec<u8>,
    event: String,
    data: Vec<String>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push bytes, appending decoded frames to `out`.
    pub fn push(&mut self, bytes: &[u8], out: &mut Vec<SseFrame>) {
        self.raw.extend_from_slice(bytes);
        let valid = match std::str::from_utf8(&self.raw) {
            Ok(_) => self.raw.len(),
            Err(e) => e.valid_up_to(),
        };
        let end = self.raw[..valid]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |i| i + 1);
        if end == 0 {
            return;
        }
        // SAFETY: `end` is within the valid UTF-8 prefix at a `\n` boundary.
        let text = String::from_utf8(self.raw[..end].to_vec()).expect("valid prefix");
        self.raw.drain(..end);
        for line in text.split('\n') {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if let Some(name) = line.strip_prefix("event:") {
                self.event = name.trim().to_string();
            } else if let Some(payload) = line.strip_prefix("data:") {
                self.data.push(payload.trim().to_string());
            } else if line.trim().is_empty() {
                self.dispatch(out);
            }
        }
    }

    /// End of stream: flush the trailing buffer like `streamPipeline` does.
    pub fn finish(&mut self, out: &mut Vec<SseFrame>) {
        if !self.raw.is_empty() {
            let tail = String::from_utf8_lossy(&self.raw).into_owned();
            self.raw.clear();
            for line in tail.split('\n') {
                let line = line.strip_suffix('\r').unwrap_or(line);
                if let Some(name) = line.strip_prefix("event:") {
                    self.event = name.trim().to_string();
                } else if let Some(payload) = line.strip_prefix("data:") {
                    self.data.push(payload.trim().to_string());
                }
            }
        }
        self.dispatch(out);
    }

    fn dispatch(&mut self, out: &mut Vec<SseFrame>) {
        if self.data.is_empty() {
            self.event.clear();
            return;
        }
        let joined = self.data.join("\n");
        if let Ok(data) = serde_json::from_str(&joined) {
            let event = if self.event.is_empty() {
                infer_event(&data)
            } else {
                std::mem::take(&mut self.event)
            };
            out.push(SseFrame { event, data });
        }
        self.event.clear();
        self.data.clear();
    }
}

/// Outcome of [`post_run`]: the terminal `done` status, if the server sent one.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, PartialEq)]
pub struct RunOutcome {
    pub status: String,
}

/// Blocking `POST /api/run`, streaming frames to `on_frame` as they arrive.
/// Chunked and close-delimited bodies both supported. Errors are strings so
/// the UI thread can surface them as an `error` frame.
///
/// Native only: browsers have no TCP sockets — the web build uses `fetch`
/// (not yet wired; sends stage locally, see `stage_send`).
#[cfg(not(target_arch = "wasm32"))]
pub fn post_run(
    host: &str,
    port: u16,
    req: &RunRequest,
    on_frame: &mut dyn FnMut(SseFrame),
) -> Result<RunOutcome, String> {
    let body = req.to_json().to_string();
    let head = format!(
        "Content-Type: application/json\r\nContent-Length: {}\r\nAccept: text/event-stream\r\n",
        body.len()
    );
    let (mut reader, chunked) = request(host, port, "POST", "/api/run", &head, body.as_bytes())?;
    let mut decoder = SseDecoder::new();
    let mut frames: Vec<SseFrame> = Vec::new();
    let mut outcome = RunOutcome {
        status: "unknown".to_string(),
    };
    for bytes in read_body(&mut reader, chunked)? {
        decoder.push(&bytes, &mut frames);
        drain(&mut frames, &mut outcome, on_frame);
    }
    decoder.finish(&mut frames);
    drain(&mut frames, &mut outcome, on_frame);
    Ok(outcome)
}

/// Blocking `GET` returning a JSON body. Used for status endpoints
/// (`/api/runs/active`, `/api/cloud/status`). Native only (see `post_run`).
#[cfg(not(target_arch = "wasm32"))]
pub fn get_json(host: &str, port: u16, path: &str) -> Result<serde_json::Value, String> {
    let (mut reader, chunked) =
        request(host, port, "GET", path, "Accept: application/json\r\n", &[])?;
    let mut raw = Vec::new();
    for bytes in read_body(&mut reader, chunked)? {
        raw.extend_from_slice(&bytes);
    }
    serde_json::from_slice(&raw).map_err(|e| format!("json: {e}"))
}

/// Blocking JSON write (`POST`/`PATCH`/`PUT`/`DELETE`) returning the parsed
/// body. Native only. Powers the notes/agent panels, whose writes are all
/// explicit button presses (user go-ahead at press time, like composer send).
#[cfg(not(target_arch = "wasm32"))]
pub fn send_json(
    host: &str,
    port: u16,
    method: &str,
    path: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let raw = body.to_string();
    let head = format!(
        "Content-Type: application/json\r\nContent-Length: {}\r\nAccept: application/json\r\n",
        raw.len()
    );
    let (mut reader, chunked) = request(host, port, method, path, &head, raw.as_bytes())?;
    let mut buf = Vec::new();
    for bytes in read_body(&mut reader, chunked)? {
        buf.extend_from_slice(&bytes);
    }
    if buf.is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_slice(&buf).map_err(|e| format!("json: {e}"))
}
/// Hand-built `multipart/form-data` upload (the backend's `/attachments`
/// handler), returning the parsed `{attachments:[…]}` body. Native only:
/// the picker and local paths exist only there. File reads stay synchronous
/// because the composer's send is synchronous for the same reason.
#[cfg(not(target_arch = "wasm32"))]
pub fn upload_attachments(
    host: &str,
    port: u16,
    workspace_id: &str,
    chat_id: &str,
    files: &[std::path::PathBuf],
) -> Result<serde_json::Value, String> {
    if files.is_empty() {
        return Err("no files to upload".to_string());
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let boundary = format!("----stark-rune-{stamp:x}");
    let mut body = Vec::new();
    for path in files {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("invalid file name: {}", path.display()))?
            .replace(['"', '\r', '\n'], "_");
        let mime = crate::chats::mime_for_path(path);
        let bytes = std::fs::read(path).map_err(|e| format!("read {name}: {e}"))?;
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"{name}\"\r\nContent-Type: {mime}\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(&bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let head = format!(
        "Content-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {}\r\nAccept: application/json\r\n",
        body.len()
    );
    let path = format!("/api/workspaces/{workspace_id}/chats/{chat_id}/attachments");
    let (mut reader, chunked) = request(host, port, "POST", &path, &head, &body)?;
    let mut raw = Vec::new();
    for bytes in read_body(&mut reader, chunked)? {
        raw.extend_from_slice(&bytes);
    }
    serde_json::from_slice(&raw).map_err(|e| format!("json: {e}"))
}

/// Create a chat in a workspace (`createChat`): POST title/model, return id.
#[cfg(not(target_arch = "wasm32"))]
pub fn create_chat(
    host: &str,
    port: u16,
    workspace_id: &str,
    title: &str,
) -> Result<String, String> {
    let path = format!("/api/workspaces/{workspace_id}/chats");
    let body = serde_json::json!({"title": title, "model": "Automatic Fallback"});
    let value = send_json(host, port, "POST", &path, &body)?;
    value
        .get("id")
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "create chat: missing id".to_string())
}

/// Browser twin of [`create_chat`].
#[cfg(target_arch = "wasm32")]
pub async fn create_chat_wasm(workspace_id: &str, title: &str) -> Result<String, String> {
    let path = format!("/api/workspaces/{workspace_id}/chats");
    let body = serde_json::json!({"title": title, "model": "Automatic Fallback"});
    let value = fetch_send("POST", &path, Some(&body)).await?;
    value
        .get("id")
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "create chat: missing id".to_string())
}

/// Blocking `POST /api/abort {chat_id}` — mirrors `abortRun`
/// (`stream.ts`): tells the backend to actually cancel the run (inbox abort +
/// pydantic-ai token), not just detach this client. Fire-and-forget from Stop
/// handlers; the response body is a bare `{ok}`. Empty `chat_id` is a no-op
/// (`false`), like the React guard. Native only.
#[cfg(not(target_arch = "wasm32"))]
pub fn post_abort(host: &str, port: u16, chat_id: &str) -> bool {
    if chat_id.trim().is_empty() {
        return false;
    }
    let body = serde_json::json!({"chat_id": chat_id}).to_string();
    let head = format!(
        "Content-Type: application/json\r\nContent-Length: {}\r\nAccept: application/json\r\n",
        body.len()
    );
    let Ok((mut reader, chunked)) =
        request(host, port, "POST", "/api/abort", &head, body.as_bytes())
    else {
        return false;
    };
    let mut raw = Vec::new();
    for bytes in read_body(&mut reader, chunked).unwrap_or_default() {
        raw.extend_from_slice(&bytes);
    }
    serde_json::from_slice::<serde_json::Value>(&raw)
        .ok()
        .and_then(|v| v.get("ok").and_then(|x| x.as_bool()))
        .unwrap_or(true)
}

/// `true` when a transport error means the server is locked and wants
/// pairing, not a dead backend: HTTP 401 (see `PairingGate.tsx` — the
/// phone-side gate over the same status). The native desktop is the pairing
/// host, so callers show the "locked" notice instead of the gate itself.
pub fn is_locked_error(message: &str) -> bool {
    message.contains("401")
}

/// Web `fetch` GET returning JSON — the browser twin of [`get_json`].
/// Same contract; the page talks to the LAN backend over plain HTTP
/// (default `127.0.0.1:8124`, mirroring the desktop default). CORS is open
/// server-side (`access-control-allow-origin: *`, verified live).
#[cfg(target_arch = "wasm32")]
pub async fn fetch_json(path: &str) -> Result<serde_json::Value, String> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    let url = format!("http://127.0.0.1:8124{path}");
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_method("GET");
    init.set_mode(web_sys::RequestMode::Cors);
    let request = web_sys::Request::new_with_str_and_init(&url, &init)
        .map_err(|e| format!("request: {e:?}"))?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("fetch {path}: {e:?}"))?;
    let resp: web_sys::Response = resp_value
        .dyn_into()
        .map_err(|e| format!("response: {e:?}"))?;
    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let json = JsFuture::from(resp.json().map_err(|e| format!("json: {e:?}"))?)
        .await
        .map_err(|e| format!("body: {e:?}"))?;
    let text = js_sys::JSON::stringify(&json)
        .map_err(|e| format!("stringify: {e:?}"))?
        .as_string()
        .ok_or("non-string json")?;
    serde_json::from_str(&text).map_err(|e| format!("parse: {e}"))
}

/// Web `fetch` JSON write — the browser twin of [`send_json`]. `body=None`
/// sends a bodiless request (cancel/merge/discard style).
#[cfg(target_arch = "wasm32")]
pub async fn fetch_send(
    method: &str,
    path: &str,
    body: Option<&serde_json::Value>,
) -> Result<serde_json::Value, String> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    let url = format!("http://127.0.0.1:8124{path}");
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    init.set_mode(web_sys::RequestMode::Cors);
    if let Some(b) = body {
        init.set_body(&wasm_bindgen::JsValue::from_str(&b.to_string()));
    }
    init.set_headers(&{
        let h = web_sys::Headers::new().map_err(|e| format!("headers: {e:?}"))?;
        h.set("Content-Type", "application/json")
            .map_err(|e| format!("headers: {e:?}"))?;
        h.into()
    });
    let request = web_sys::Request::new_with_str_and_init(&url, &init)
        .map_err(|e| format!("request: {e:?}"))?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("fetch {path}: {e:?}"))?;
    let resp: web_sys::Response = resp_value
        .dyn_into()
        .map_err(|e| format!("response: {e:?}"))?;
    if resp.status() == 401 {
        return Err("HTTP 401 Unauthorized".to_string());
    }
    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let json = JsFuture::from(resp.json().map_err(|e| format!("json: {e:?}"))?)
        .await
        .map_err(|e| format!("body: {e:?}"))?;
    if json.is_null() || json.is_undefined() {
        return Ok(serde_json::Value::Null);
    }
    let text = js_sys::JSON::stringify(&json)
        .map_err(|e| format!("stringify: {e:?}"))?
        .as_string()
        .ok_or("non-string json")?;
    serde_json::from_str(&text).map_err(|e| format!("parse: {e}"))
}

/// Web `fetch` POST `/api/abort` — the browser twin of [`post_abort`].
/// Same contract (`{chat_id}` → `{ok}`); empty id is a no-op.
#[cfg(target_arch = "wasm32")]
pub async fn fetch_abort(chat_id: &str) -> bool {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    if chat_id.trim().is_empty() {
        return false;
    }
    let url = "http://127.0.0.1:8124/api/abort".to_string();
    let body = serde_json::json!({"chat_id": chat_id}).to_string();
    let window = match web_sys::window() {
        Some(w) => w,
        None => return false,
    };
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_mode(web_sys::RequestMode::Cors);
    init.set_body(&wasm_bindgen::JsValue::from_str(&body));
    init.set_headers(&{
        let h = match web_sys::Headers::new() {
            Ok(h) => h,
            Err(_) => return false,
        };
        if h.set("Content-Type", "application/json").is_err() {
            return false;
        }
        h.into()
    });
    let request = match web_sys::Request::new_with_str_and_init(&url, &init) {
        Ok(r) => r,
        Err(_) => return false,
    };
    let resp_value = match JsFuture::from(window.fetch_with_request(&request)).await {
        Ok(v) => v,
        Err(_) => return false,
    };
    let resp: web_sys::Response = match resp_value.dyn_into() {
        Ok(r) => r,
        Err(_) => return false,
    };
    resp.ok()
}

/// Web `POST /api/run` with ReadableStream SSE decode — the browser twin of
/// native [`post_run`]. Frames stream into `emit` as they arrive; the shared
/// `SseDecoder` keeps framing rules identical on both targets.
#[cfg(target_arch = "wasm32")]
pub async fn post_run_stream(req: &RunRequest, emit: &mut dyn FnMut(SseFrame)) {
    let outcome = post_run_stream_inner(req, emit).await;
    if let Err(message) = outcome {
        emit(SseFrame {
            event: "error".to_string(),
            data: serde_json::json!({"message": message}),
        });
    }
}

#[cfg(target_arch = "wasm32")]
async fn post_run_stream_inner(
    req: &RunRequest,
    emit: &mut dyn FnMut(SseFrame),
) -> Result<String, String> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    let url = "http://127.0.0.1:8124/api/run".to_string();
    let body = req.to_json().to_string();
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_mode(web_sys::RequestMode::Cors);
    init.set_body(&wasm_bindgen::JsValue::from_str(&body));
    init.set_headers(&{
        let h = web_sys::Headers::new().map_err(|e| format!("headers: {e:?}"))?;
        h.set("Content-Type", "application/json")
            .map_err(|e| format!("headers: {e:?}"))?;
        h.into()
    });
    let request = web_sys::Request::new_with_str_and_init(&url, &init)
        .map_err(|e| format!("request: {e:?}"))?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("fetch: {e:?}"))?;
    let resp: web_sys::Response = resp_value
        .dyn_into()
        .map_err(|e| format!("response: {e:?}"))?;
    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let stream = resp.body().ok_or("no body")?;
    let reader = stream
        .get_reader()
        .dyn_into::<web_sys::ReadableStreamDefaultReader>()
        .map_err(|e| format!("reader: {e:?}"))?;
    let mut decoder = SseDecoder::new();
    let mut frames = Vec::new();
    let mut status = "unknown".to_string();
    loop {
        let chunk = JsFuture::from(reader.read())
            .await
            .map_err(|e| format!("read: {e:?}"))?;
        let done = js_sys::Reflect::get(&chunk, &"done".into())
            .map_err(|e| format!("done: {e:?}"))?
            .as_bool()
            .unwrap_or(true);
        if done {
            break;
        }
        let value =
            js_sys::Reflect::get(&chunk, &"value".into()).map_err(|e| format!("value: {e:?}"))?;
        let bytes = js_sys::Uint8Array::from(value).to_vec();
        decoder.push(&bytes, &mut frames);
        for frame in frames.drain(..) {
            if frame.event == "done" {
                if let Some(s) = frame.data.get("status").and_then(|v| v.as_str()) {
                    status = s.to_string();
                }
            }
            emit(frame);
        }
    }
    decoder.finish(&mut frames);
    for frame in frames.drain(..) {
        emit(frame);
    }
    Ok(status)
}

/// Open a connection, send the request, and return a reader past the
/// response headers plus whether the body is chunked.
#[cfg(not(target_arch = "wasm32"))]
fn request(
    host: &str,
    port: u16,
    method: &str,
    path: &str,
    headers: &str,
    body: &[u8],
) -> Result<(BufReader<TcpStream>, bool), String> {
    let addr = format!("{host}:{port}");
    let mut sock = TcpStream::connect(&addr).map_err(|e| format!("connect {addr}: {e}"))?;
    sock.set_read_timeout(Some(Duration::from_secs(120)))
        .map_err(|e| e.to_string())?;
    sock.set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    let http =
        format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\n{headers}Connection: close\r\n\r\n");
    sock.write_all(http.as_bytes())
        .map_err(|e| format!("write: {e}"))?;
    sock.write_all(body).map_err(|e| format!("write: {e}"))?;
    let mut reader = BufReader::new(sock);
    let mut status = String::new();
    reader
        .read_line(&mut status)
        .map_err(|e| format!("status: {e}"))?;
    if !status.contains("200") {
        return Err(format!("HTTP {}", status.trim()));
    }
    let mut chunked = false;
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("headers: {e}"))?;
        let t = line.trim();
        if t.is_empty() {
            break;
        }
        if t.to_ascii_lowercase().starts_with("transfer-encoding:") && t.contains("chunked") {
            chunked = true;
        }
    }
    Ok((reader, chunked))
}

/// Read one body as pieces (streaming-friendly): chunk frames when chunked,
/// 8KB reads until close otherwise.
#[cfg(not(target_arch = "wasm32"))]
fn read_body(reader: &mut BufReader<TcpStream>, chunked: bool) -> Result<Vec<Vec<u8>>, String> {
    let mut pieces = Vec::new();
    if chunked {
        loop {
            let mut size_line = String::new();
            reader
                .read_line(&mut size_line)
                .map_err(|e| format!("chunk size: {e}"))?;
            let size = u64::from_str_radix(size_line.trim(), 16)
                .map_err(|_| format!("bad chunk size {size_line:?}"))?;
            if size == 0 {
                break;
            }
            let mut chunk = vec![0u8; size as usize];
            reader
                .read_exact(&mut chunk)
                .map_err(|e| format!("chunk: {e}"))?;
            let mut crlf = [0u8; 2];
            reader
                .read_exact(&mut crlf)
                .map_err(|e| format!("chunk end: {e}"))?;
            pieces.push(chunk);
        }
    } else {
        loop {
            let mut buf = [0u8; 8192];
            let n = reader.read(&mut buf).map_err(|e| format!("body: {e}"))?;
            if n == 0 {
                break;
            }
            pieces.push(buf[..n].to_vec());
        }
    }
    Ok(pieces)
}

#[cfg(not(target_arch = "wasm32"))]
fn drain(frames: &mut Vec<SseFrame>, outcome: &mut RunOutcome, on_frame: &mut dyn FnMut(SseFrame)) {
    for frame in frames.drain(..) {
        if frame.event == "done" {
            if let Some(status) = frame.data.get("status").and_then(|v| v.as_str()) {
                outcome.status = status.to_string();
            }
        }
        on_frame(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn decoder_handles_split_lines_and_multibyte() {
        let mut dec = SseDecoder::new();
        let mut out = Vec::new();
        // "…" (3 bytes) split across chunks; lines split mid-way too.
        dec.push(
            "event: delta\ndata: {\"kind\":\"text\",\"text\":\"42 …".as_bytes(),
            &mut out,
        );
        assert!(out.is_empty());
        dec.push("90.4\"}\n\n".as_bytes(), &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].data["text"], serde_json::json!("42 …90.4"));
    }

    /// Multipart upload over a real socket: path, boundary, file part, and
    /// the parsed attachment ids the composer puts back in its draft deck.
    #[test]
    fn upload_attachments_posts_multipart_and_reads_ids() {
        let dir = std::env::temp_dir().join(format!(
            "stark-upload-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("note.txt");
        std::fs::write(&path, b"hello").unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(sock.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut content_len = 0usize;
            let mut content_type = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
                if let Some(v) = line.strip_prefix("Content-Length:") {
                    content_len = v.trim().parse().unwrap();
                }
                if let Some(v) = line.strip_prefix("Content-Type:") {
                    content_type = v.trim().to_string();
                }
            }
            let mut body = vec![0u8; content_len];
            reader.read_exact(&mut body).unwrap();
            assert!(
                request_line.starts_with("POST /api/workspaces/team/chats/c1/attachments "),
                "{request_line}"
            );
            assert!(
                content_type.starts_with("multipart/form-data; boundary="),
                "{content_type}"
            );
            let body = String::from_utf8_lossy(&body);
            assert!(
                body.contains("name=\"files\"; filename=\"note.txt\""),
                "{body}"
            );
            assert!(body.contains("Content-Type: text/plain"), "{body}");
            assert!(body.contains("hello"), "{body}");
            assert!(
                body.ends_with(&format!(
                    "--{}--\r\n",
                    content_type.split('=').nth(1).unwrap()
                )),
                "closing boundary"
            );
            let json = r#"{"attachments":[{"id":"a1","name":"note.txt","type":"text/plain","size":5,"path":"/tmp/note.txt","isImage":false}]}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                json.len(),
                json
            );
            sock.write_all(reply.as_bytes()).unwrap();
        });

        let value =
            upload_attachments("127.0.0.1", port, "team", "c1", std::slice::from_ref(&path))
                .unwrap();
        server.join().unwrap();
        let files = value["attachments"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["id"], "a1");
        assert_eq!(files[0]["name"], "note.txt");
        assert_eq!(files[0]["size"], 5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Full client↔server round trip over a real socket with chunked SSE.
    #[test]
    fn post_run_streams_chunked_sse() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            // Read headers + exact body.
            let mut reader = BufReader::new(sock.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut content_len = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
                if let Some(v) = line.strip_prefix("Content-Length:") {
                    content_len = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0u8; content_len];
            reader.read_exact(&mut body).unwrap();
            let sent = String::from_utf8(body).unwrap();
            assert!(request_line.starts_with("POST /api/run "), "{request_line}");
            assert!(sent.contains("\"workspace_kind\":\"general\""), "{sent}");
            // Chunked SSE, deliberately fragmented: one logical line split
            // across two chunks to prove incremental decode.
            let parts: &[&[u8]] = &[
                b"event: delta\r\ndata: {\"kind\":\"text\",\"text\":\"hel",
                b"lo\"}\r\n\r\nevent: done\r\ndata: {\"status\":\"ok\"}\r\n\r\n",
            ];
            let mut head =
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n"
                    .to_vec();
            sock.write_all(&head).unwrap();
            head.clear();
            for part in parts {
                sock.write_all(format!("{:x}\r\n", part.len()).as_bytes())
                    .unwrap();
                // Split the write itself to fragment TCP segments.
                let mid = part.len() / 2;
                sock.write_all(&part[..mid]).unwrap();
                sock.write_all(&part[mid..]).unwrap();
                sock.write_all(b"\r\n").unwrap();
            }
            sock.write_all(b"0\r\n\r\n").unwrap();
        });

        let req = RunRequest {
            prompt: "hi".to_string(),
            model: "m".to_string(),
            temperature: 0.7,
            workspace_path: String::new(),
            workspace_id: "_general".to_string(),
            chat_id: "c1".to_string(),
            workspace_kind: "general".to_string(),
            attachments: Vec::new(),
        };
        let mut got: Vec<SseFrame> = Vec::new();
        let outcome = post_run("127.0.0.1", port, &req, &mut |f| got.push(f)).unwrap();
        server.join().unwrap();
        assert_eq!(outcome.status, "ok");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].data["text"], serde_json::json!("hello"));
        assert_eq!(got[1].event, "done");
    }
}
