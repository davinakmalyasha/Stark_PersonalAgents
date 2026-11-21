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
/// Native only: browsers have no TCP sockets — the web build uses
/// [`post_run_stream`] over `fetch`.
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

/// Browser pairing state. The phone stores its token in localStorage and
/// sends `X-API-Token` on every API request; a 401 raises this flag so the
/// shell can show the pairing gate instead of a dead chat list.
#[cfg(target_arch = "wasm32")]
pub const PAIRING_TOKEN_KEY: &str = "stark.pairingToken";
#[cfg(target_arch = "wasm32")]
const LEGACY_PAIRING_TOKEN_KEY: &str = "vincode.pairingToken";

#[cfg(target_arch = "wasm32")]
static AUTH_REJECTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Web API origin: an injected origin wins (static hosts), otherwise the
/// page's own origin (the normal same-origin deployment), with the desktop
/// default only for `file:`/opaque contexts.
#[cfg(target_arch = "wasm32")]
pub fn api_origin() -> String {
    let Some(window) = web_sys::window() else {
        return "http://127.0.0.1:8124".to_string();
    };
    if let Ok(injected) = js_sys::Reflect::get(&window, &"__STARK_API_ORIGIN__".into()) {
        if let Some(origin) = injected.as_string() {
            let origin = origin.trim().trim_end_matches('/');
            if !origin.is_empty() {
                return origin.to_string();
            }
        }
    }
    if let Ok(location) = window.location().origin() {
        if location != "null" && !location.is_empty() {
            return location;
        }
    }
    "http://127.0.0.1:8124".to_string()
}

#[cfg(target_arch = "wasm32")]
pub fn api_url(path: &str) -> String {
    format!("{}{}", api_origin(), path)
}

#[cfg(target_arch = "wasm32")]
fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

#[cfg(target_arch = "wasm32")]
pub fn pairing_token() -> String {
    let Some(storage) = storage() else {
        return String::new();
    };
    if let Ok(Some(token)) = storage.get_item(PAIRING_TOKEN_KEY) {
        return token;
    }
    let Ok(Some(legacy)) = storage.get_item(LEGACY_PAIRING_TOKEN_KEY) else {
        return String::new();
    };
    let _ = storage.set_item(PAIRING_TOKEN_KEY, &legacy);
    let _ = storage.remove_item(LEGACY_PAIRING_TOKEN_KEY);
    legacy
}

#[cfg(target_arch = "wasm32")]
pub fn set_pairing_token(token: &str) {
    if let Some(storage) = storage() {
        let token = token.trim();
        if token.is_empty() {
            let _ = storage.remove_item(PAIRING_TOKEN_KEY);
        } else {
            let _ = storage.set_item(PAIRING_TOKEN_KEY, token);
        }
        let _ = storage.remove_item(LEGACY_PAIRING_TOKEN_KEY);
    }
}

#[cfg(target_arch = "wasm32")]
pub fn auth_rejected() -> bool {
    AUTH_REJECTED.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(target_arch = "wasm32")]
pub fn clear_auth_rejected() {
    AUTH_REJECTED.store(false, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(target_arch = "wasm32")]
fn mark_auth_rejected() {
    AUTH_REJECTED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Pull `?token=` (or `#…?token=`) out of the URL, persist it, and remove it
/// from the address bar so it is not left in history or shares.
#[cfg(target_arch = "wasm32")]
fn query_param(query: &str, key: &str) -> Option<String> {
    query
        .trim_start_matches('?')
        .split('&')
        .find_map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (name == key).then(|| value.to_string())
        })
        .filter(|value| !value.is_empty())
}

#[cfg(target_arch = "wasm32")]
fn without_query_param(query: &str, key: &str) -> String {
    query
        .trim_start_matches('?')
        .split('&')
        .filter(|pair| {
            let name = pair.split_once('=').map_or(*pair, |(name, _)| name);
            !pair.is_empty() && name != key
        })
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(target_arch = "wasm32")]
pub fn capture_url_token() -> String {
    let Some(window) = web_sys::window() else {
        return String::new();
    };
    let location = window.location();
    let search = location.search().unwrap_or_default();
    let hash = location.hash().unwrap_or_default();
    let token = query_param(&search, "token")
        .or_else(|| {
            hash.split_once('?')
                .and_then(|(_, query)| query_param(query, "token"))
        })
        .unwrap_or_default();
    if !token.is_empty() {
        set_pairing_token(&token);
        let clean_search = without_query_param(&search, "token");
        let clean_hash = hash.split('?').next().unwrap_or_default();
        let clean = format!(
            "{}{}{}",
            location.pathname().unwrap_or_else(|_| "/".to_string()),
            if clean_search.is_empty() {
                String::new()
            } else {
                format!("?{clean_search}")
            },
            clean_hash
        );
        let _ = window.history().and_then(|history| {
            history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&clean))
        });
    }
    token
}

/// Install browser auth once: capture a `?token=` link before the first poll.
#[cfg(target_arch = "wasm32")]
pub fn init_web_auth() {
    let _ = capture_url_token();
}

#[cfg(target_arch = "wasm32")]
fn auth_headers(content_type: bool) -> Result<web_sys::Headers, String> {
    let headers = web_sys::Headers::new().map_err(|e| format!("headers: {e:?}"))?;
    if content_type {
        headers
            .set("Content-Type", "application/json")
            .map_err(|e| format!("headers: {e:?}"))?;
    }
    let token = pairing_token();
    if !token.is_empty() {
        headers
            .set("X-API-Token", &token)
            .map_err(|e| format!("headers: {e:?}"))?;
    }
    Ok(headers)
}

/// Web `fetch` GET returning JSON — the browser twin of [`get_json`].
/// Same contract; the page talks to the LAN backend over plain HTTP
/// (default `127.0.0.1:8124`, mirroring the desktop default). CORS is open
/// server-side (`access-control-allow-origin: *`, verified live).
#[cfg(target_arch = "wasm32")]
pub async fn fetch_json(path: &str) -> Result<serde_json::Value, String> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    let url = api_url(path);
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_method("GET");
    init.set_mode(web_sys::RequestMode::Cors);
    init.set_headers(&auth_headers(false)?.into());
    let request = web_sys::Request::new_with_str_and_init(&url, &init)
        .map_err(|e| format!("request: {e:?}"))?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("fetch {path}: {e:?}"))?;
    let resp: web_sys::Response = resp_value
        .dyn_into()
        .map_err(|e| format!("response: {e:?}"))?;
    if resp.status() == 401 {
        mark_auth_rejected();
        return Err("HTTP 401 Unauthorized".to_string());
    }
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
    let url = api_url(path);
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    init.set_mode(web_sys::RequestMode::Cors);
    if let Some(b) = body {
        init.set_body(&wasm_bindgen::JsValue::from_str(&b.to_string()));
    }
    init.set_headers(&auth_headers(true)?.into());
    let request = web_sys::Request::new_with_str_and_init(&url, &init)
        .map_err(|e| format!("request: {e:?}"))?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("fetch {path}: {e:?}"))?;
    let resp: web_sys::Response = resp_value
        .dyn_into()
        .map_err(|e| format!("response: {e:?}"))?;
    if resp.status() == 401 {
        mark_auth_rejected();
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
    let url = api_url("/api/abort");
    let body = serde_json::json!({"chat_id": chat_id}).to_string();
    let window = match web_sys::window() {
        Some(w) => w,
        None => return false,
    };
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_mode(web_sys::RequestMode::Cors);
    init.set_body(&wasm_bindgen::JsValue::from_str(&body));
    let headers = match auth_headers(true) {
        Ok(headers) => headers,
        Err(_) => return false,
    };
    init.set_headers(&headers.into());
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
    if resp.status() == 401 {
        mark_auth_rejected();
    }
    resp.ok()
}

/// Browser download with the pairing token. `window.open` cannot attach the
/// header, so fetch the bytes, hand them to a temporary object URL, and click
/// an anchor with the server filename.
#[cfg(target_arch = "wasm32")]
pub async fn download_web(path: &str, filename: &str) -> Result<String, String> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_method("GET");
    init.set_mode(web_sys::RequestMode::Cors);
    init.set_headers(&auth_headers(false)?.into());
    let request = web_sys::Request::new_with_str_and_init(&api_url(path), &init)
        .map_err(|e| format!("request: {e:?}"))?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("fetch: {e:?}"))?;
    let resp: web_sys::Response = resp_value
        .dyn_into()
        .map_err(|e| format!("response: {e:?}"))?;
    if resp.status() == 401 {
        mark_auth_rejected();
        return Err("HTTP 401 Unauthorized".to_string());
    }
    if !resp.ok() {
        return Err(format!("download: HTTP {}", resp.status()));
    }
    let blob = JsFuture::from(resp.blob().map_err(|e| format!("blob: {e:?}"))?)
        .await
        .map_err(|e| format!("blob: {e:?}"))?;
    let blob: web_sys::Blob = blob.dyn_into().map_err(|e| format!("blob: {e:?}"))?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)
        .map_err(|e| format!("object url: {e:?}"))?;
    let document = window.document().ok_or("no document")?;
    let anchor: web_sys::HtmlAnchorElement = document
        .create_element("a")
        .map_err(|e| format!("anchor: {e:?}"))?
        .dyn_into()
        .map_err(|e| format!("anchor: {e:?}"))?;
    anchor.set_href(&url);
    anchor.set_download(filename);
    if let Some(body) = document.body() {
        let _ = body.append_child(&anchor);
    }
    anchor.click();
    let _ = anchor.remove();
    web_sys::Url::revoke_object_url(&url).ok();
    Ok(filename.to_string())
}

/// Stable upload target. Cards carry their id (not a map order index) because
/// the upload outlives the frame that opened the picker.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, PartialEq, Eq)]
pub enum WebTarget {
    Main,
    Open,
    Card(String),
}

/// A browser picker result waiting for the shell poll. Files stay as browser
/// `File` handles until the async upload runs.
#[cfg(target_arch = "wasm32")]
pub struct WebPick {
    pub target: WebTarget,
    pub files: Vec<web_sys::File>,
}

/// Completed browser upload. `created` means the picker had to create the
/// chat first (React creates it at send time; uploading first needs an id).
#[cfg(target_arch = "wasm32")]
pub struct WebUpload {
    pub target: WebTarget,
    pub workspace_id: String,
    pub chat_id: String,
    pub created: bool,
    pub result: Result<Vec<crate::chats::ChatAttachment>, String>,
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    static WEB_PICKS: std::cell::RefCell<Vec<WebPick>> =
        const { std::cell::RefCell::new(Vec::new()) };
    static WEB_UPLOADS: std::cell::RefCell<Vec<WebUpload>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// Picker opened at (millis); cleared on a selection or after two minutes.
    static WEB_PICK_AT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static WEB_UPLOADS_IN_FLIGHT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static WEB_BACKUP: std::cell::RefCell<Option<web_sys::File>> =
        const { std::cell::RefCell::new(None) };
    static WEB_BACKUP_NAME: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// True while a browser picker may still answer or an upload is in flight;
/// the shell schedules a wake so the async result is applied promptly.
#[cfg(target_arch = "wasm32")]
pub fn web_busy() -> bool {
    let now = crate::clock::now_millis();
    let picking = WEB_PICK_AT.with(|at| {
        let at = at.get();
        at != 0 && now.saturating_sub(at) < 120_000
    });
    picking
        || WEB_UPLOADS_IN_FLIGHT.with(std::cell::Cell::get) > 0
        || WEB_UPLOADS.with(|queue| !queue.borrow().is_empty())
}

#[cfg(target_arch = "wasm32")]
pub fn take_web_picks() -> Vec<WebPick> {
    WEB_PICKS.with(|queue| std::mem::take(&mut *queue.borrow_mut()))
}

#[cfg(target_arch = "wasm32")]
pub fn take_web_uploads() -> Vec<WebUpload> {
    WEB_UPLOADS.with(|queue| std::mem::take(&mut *queue.borrow_mut()))
}

#[cfg(target_arch = "wasm32")]
fn push_web_upload(upload: WebUpload) {
    WEB_UPLOADS.with(|queue| queue.borrow_mut().push(upload));
}

/// Open the browser's native file picker for one surface. The change handler
/// queues the selected `File`s; the shell uploads them on the next poll.
#[cfg(target_arch = "wasm32")]
pub fn pick_files(target: WebTarget) {
    use wasm_bindgen::{closure::Closure, JsCast};
    WEB_PICK_AT.with(|at| at.set(crate::clock::now_millis()));
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Ok(element) = document.create_element("input") else {
        return;
    };
    let Ok(input) = element.dyn_into::<web_sys::HtmlInputElement>() else {
        return;
    };
    input.set_type("file");
    input.set_multiple(true);
    input.set_accept(
        "image/*,audio/*,.pdf,.txt,.md,.json,.js,.ts,.tsx,.py,.html,.css,.csv,.doc,.docx,.zip",
    );
    let _ = input.set_attribute("style", "display:none");
    if let Some(body) = document.body() {
        let _ = body.append_child(&input);
    }
    let change_input = input.clone();
    let closure = Closure::wrap(Box::new(move || {
        let files: Vec<web_sys::File> = change_input
            .files()
            .map(|list| {
                (0..list.length())
                    .filter_map(|index| list.get(index))
                    .collect()
            })
            .unwrap_or_default();
        if !files.is_empty() {
            WEB_PICK_AT.with(|at| at.set(0));
            WEB_PICKS.with(|queue| {
                queue.borrow_mut().push(WebPick {
                    target: target.clone(),
                    files,
                });
            });
        }
        let _ = change_input.remove();
    }) as Box<dyn FnMut()>);
    input.set_onchange(Some(closure.as_ref().unchecked_ref()));
    closure.forget();
    input.click();
}

/// Browser zip picker for Settings' backup import. The selected `File`
/// stays here until `run_backup_import` uploads it.
#[cfg(target_arch = "wasm32")]
pub fn pick_backup_file() {
    use wasm_bindgen::{closure::Closure, JsCast};
    WEB_PICK_AT.with(|at| at.set(crate::clock::now_millis()));
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Ok(element) = document.create_element("input") else {
        return;
    };
    let Ok(input) = element.dyn_into::<web_sys::HtmlInputElement>() else {
        return;
    };
    input.set_type("file");
    input.set_accept(".zip,application/zip");
    let _ = input.set_attribute("style", "display:none");
    if let Some(body) = document.body() {
        let _ = body.append_child(&input);
    }
    let change_input = input.clone();
    let closure = Closure::wrap(Box::new(move || {
        if let Some(file) = change_input.files().and_then(|list| list.get(0)) {
            WEB_PICK_AT.with(|at| at.set(0));
            WEB_BACKUP_NAME.with(|name| *name.borrow_mut() = Some(file.name()));
            WEB_BACKUP.with(|slot| *slot.borrow_mut() = Some(file));
        }
        let _ = change_input.remove();
    }) as Box<dyn FnMut()>);
    input.set_onchange(Some(closure.as_ref().unchecked_ref()));
    closure.forget();
    input.click();
}

#[cfg(target_arch = "wasm32")]
pub fn backup_file_name() -> Option<String> {
    WEB_BACKUP_NAME.with(|name| name.borrow().clone())
}

#[cfg(target_arch = "wasm32")]
pub fn clear_backup_file() {
    WEB_BACKUP.with(|slot| *slot.borrow_mut() = None);
    WEB_BACKUP_NAME.with(|name| *name.borrow_mut() = None);
}

#[cfg(target_arch = "wasm32")]
pub async fn upload_backup_web(projects_root: &str) -> Result<serde_json::Value, String> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    let file = WEB_BACKUP.with(|slot| slot.borrow().clone());
    let file = file.ok_or("Choose a backup zip first.")?;
    let form = web_sys::FormData::new().map_err(|e| format!("form: {e:?}"))?;
    form.append_with_blob_and_filename("file", file.as_ref(), &file.name())
        .map_err(|e| format!("form file: {e:?}"))?;
    let root = projects_root.trim();
    if !root.is_empty() {
        form.append_with_str("projects_root", root)
            .map_err(|e| format!("form root: {e:?}"))?;
    }
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_mode(web_sys::RequestMode::Cors);
    init.set_body(form.as_ref());
    init.set_headers(&auth_headers(false)?.into());
    let request = web_sys::Request::new_with_str_and_init(&api_url("/api/data/import"), &init)
        .map_err(|e| format!("request: {e:?}"))?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("fetch: {e:?}"))?;
    let resp: web_sys::Response = resp_value
        .dyn_into()
        .map_err(|e| format!("response: {e:?}"))?;
    if resp.status() == 401 {
        mark_auth_rejected();
        return Err("HTTP 401 Unauthorized".to_string());
    }
    if !resp.ok() {
        return Err(format!("import: HTTP {}", resp.status()));
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

/// Upload picked browser files with the same multipart contract React uses.
#[cfg(target_arch = "wasm32")]
async fn upload_web_files(
    workspace_id: &str,
    chat_id: &str,
    files: &[web_sys::File],
) -> Result<Vec<crate::chats::ChatAttachment>, String> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    let form = web_sys::FormData::new().map_err(|e| format!("form: {e:?}"))?;
    for file in files {
        form.append_with_blob_and_filename("files", file.as_ref(), &file.name())
            .map_err(|e| format!("form file: {e:?}"))?;
    }
    let url = api_url(&format!(
        "/api/workspaces/{workspace_id}/chats/{chat_id}/attachments"
    ));
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_mode(web_sys::RequestMode::Cors);
    init.set_body(form.as_ref());
    init.set_headers(&auth_headers(false)?.into());
    let request = web_sys::Request::new_with_str_and_init(&url, &init)
        .map_err(|e| format!("request: {e:?}"))?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("fetch: {e:?}"))?;
    let resp: web_sys::Response = resp_value
        .dyn_into()
        .map_err(|e| format!("response: {e:?}"))?;
    if resp.status() == 401 {
        mark_auth_rejected();
        return Err("HTTP 401 Unauthorized".to_string());
    }
    if !resp.ok() {
        return Err(format!("upload: HTTP {}", resp.status()));
    }
    let json = JsFuture::from(resp.json().map_err(|e| format!("json: {e:?}"))?)
        .await
        .map_err(|e| format!("body: {e:?}"))?;
    let text = js_sys::JSON::stringify(&json)
        .map_err(|e| format!("stringify: {e:?}"))?
        .as_string()
        .ok_or("non-string json")?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("parse: {e}"))?;
    Ok(value
        .get("attachments")
        .and_then(|v| v.as_array())
        .map(|files| {
            files
                .iter()
                .map(crate::chats::ChatAttachment::from_json)
                .collect()
        })
        .unwrap_or_default())
}

/// Create the chat (when this is a fresh card) then upload. Results land in
/// `take_web_uploads()` for the shell poll to attach to the right surface.
#[cfg(target_arch = "wasm32")]
pub fn spawn_web_upload(pick: WebPick, workspace_id: String, chat_id: String, title: String) {
    WEB_UPLOADS_IN_FLIGHT.with(|count| count.set(count.get() + 1));
    wasm_bindgen_futures::spawn_local(async move {
        let (workspace_id, chat_id, created) = if chat_id.trim().is_empty() {
            match create_chat_wasm(&workspace_id, &title).await {
                Ok(id) => (workspace_id, id, true),
                Err(error) => {
                    WEB_UPLOADS_IN_FLIGHT.with(|count| count.set(count.get().saturating_sub(1)));
                    push_web_upload(WebUpload {
                        target: pick.target,
                        workspace_id,
                        chat_id: String::new(),
                        created: false,
                        result: Err(error),
                    });
                    return;
                }
            }
        } else {
            (workspace_id, chat_id, false)
        };
        let result = upload_web_files(&workspace_id, &chat_id, &pick.files).await;
        WEB_UPLOADS_IN_FLIGHT.with(|count| count.set(count.get().saturating_sub(1)));
        push_web_upload(WebUpload {
            target: pick.target,
            workspace_id,
            chat_id,
            created,
            result,
        });
    });
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
    let url = api_url("/api/run");
    let body = req.to_json().to_string();
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_mode(web_sys::RequestMode::Cors);
    init.set_body(&wasm_bindgen::JsValue::from_str(&body));
    init.set_headers(&auth_headers(true)?.into());
    let request = web_sys::Request::new_with_str_and_init(&url, &init)
        .map_err(|e| format!("request: {e:?}"))?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("fetch: {e:?}"))?;
    let resp: web_sys::Response = resp_value
        .dyn_into()
        .map_err(|e| format!("response: {e:?}"))?;
    if resp.status() == 401 {
        mark_auth_rejected();
        return Err("HTTP 401 Unauthorized".to_string());
    }
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

/// A downloaded file: body bytes plus the server's `Content-Disposition`
/// filename when one is present.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    pub bytes: Vec<u8>,
    pub filename: String,
}

/// Blocking `GET` returning raw bytes and the attachment filename. Used by
/// Settings' backup export; native only (see `post_run`).
#[cfg(not(target_arch = "wasm32"))]
pub fn download_file(host: &str, port: u16, path: &str) -> Result<Download, String> {
    let (mut reader, chunked, headers) =
        request_with_headers(host, port, "GET", path, "Accept: application/zip\r\n", &[])?;
    let mut bytes = Vec::new();
    for piece in read_body(&mut reader, chunked)? {
        bytes.extend_from_slice(&piece);
    }
    let filename = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-disposition"))
        .and_then(|(_, value)| attachment_filename(value))
        .unwrap_or_else(|| "stark-backup.zip".to_string());
    Ok(Download { bytes, filename })
}

#[cfg(not(target_arch = "wasm32"))]
fn attachment_filename(value: &str) -> Option<String> {
    let marker = "filename=\"";
    let start = value.find(marker)? + marker.len();
    let rest = &value[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Upload one Stark backup zip with the optional projects root
/// (`POST /api/data/import`), returning the staged-import JSON. Native only:
/// the picker and local file paths exist only there.
#[cfg(not(target_arch = "wasm32"))]
pub fn upload_backup(
    host: &str,
    port: u16,
    file: &std::path::Path,
    projects_root: &str,
) -> Result<serde_json::Value, String> {
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("invalid file name: {}", file.display()))?
        .replace(['"', '\r', '\n'], "_");
    let bytes = std::fs::read(file).map_err(|e| format!("read {name}: {e}"))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let boundary = format!("----stark-rune-backup-{stamp:x}");
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/zip\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&bytes);
    body.extend_from_slice(b"\r\n");
    let root = projects_root.trim();
    if !root.is_empty() {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"projects_root\"\r\n\r\n{root}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let head = format!(
        "Content-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {}\r\nAccept: application/json\r\n",
        body.len()
    );
    let (mut reader, chunked) = request(host, port, "POST", "/api/data/import", &head, &body)?;
    let mut raw = Vec::new();
    for piece in read_body(&mut reader, chunked)? {
        raw.extend_from_slice(&piece);
    }
    serde_json::from_slice(&raw).map_err(|e| format!("json: {e}"))
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
    let (reader, chunked, _) = request_with_headers(host, port, method, path, headers, body)?;
    Ok((reader, chunked))
}

/// Response headers, lowercased names in wire order.
#[cfg(not(target_arch = "wasm32"))]
type ResponseHeaders = Vec<(String, String)>;

/// [`request`] plus the response headers.
#[cfg(not(target_arch = "wasm32"))]
fn request_with_headers(
    host: &str,
    port: u16,
    method: &str,
    path: &str,
    headers: &str,
    body: &[u8],
) -> Result<(BufReader<TcpStream>, bool, ResponseHeaders), String> {
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
    let mut headers_out = Vec::new();
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("headers: {e}"))?;
        let t = line.trim();
        if t.is_empty() {
            break;
        }
        if let Some((name, value)) = t.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if name == "transfer-encoding" && value.to_ascii_lowercase().contains("chunked") {
                chunked = true;
            }
            headers_out.push((name, value));
        }
    }
    Ok((reader, chunked, headers_out))
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

    #[test]
    fn download_reads_the_backup_filename_and_bytes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(sock.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            assert!(
                request_line.starts_with("GET /api/data/export "),
                "{request_line}"
            );
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
            }
            let body = b"PK\x03\x04zip";
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Disposition: attachment; filename=\"stark-backup-20260101-120000.zip\"\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            sock.write_all(head.as_bytes()).unwrap();
            sock.write_all(body).unwrap();
        });
        let download = download_file("127.0.0.1", port, "/api/data/export").unwrap();
        server.join().unwrap();
        assert_eq!(download.filename, "stark-backup-20260101-120000.zip");
        assert_eq!(download.bytes, b"PK\x03\x04zip");
    }

    #[test]
    fn backup_upload_sends_zip_and_projects_root() {
        let dir = std::env::temp_dir().join(format!(
            "stark-backup-upload-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("stark-backup.zip");
        std::fs::write(&path, b"PK\x03\x04zip").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
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
            assert!(
                request_line.starts_with("POST /api/data/import "),
                "{request_line}"
            );
            let body = String::from_utf8_lossy(&body);
            assert!(
                body.contains("name=\"file\"; filename=\"stark-backup.zip\""),
                "{body}"
            );
            assert!(
                body.contains("name=\"projects_root\"\r\n\r\nC:\\Projects"),
                "{body}"
            );
            let json =
                r#"{"ok":true,"restart_required":true,"from_root":"C:\\old","to_root":"C:\\new"}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                json.len(),
                json
            );
            sock.write_all(reply.as_bytes()).unwrap();
        });
        let value = upload_backup("127.0.0.1", port, &path, r"C:\Projects").unwrap();
        server.join().unwrap();
        assert_eq!(value["ok"], true);
        assert_eq!(value["restart_required"], true);
        let _ = std::fs::remove_dir_all(&dir);
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
