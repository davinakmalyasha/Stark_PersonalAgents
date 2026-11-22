//! Inline audio playback for transcript attachments.
//!
//! Native uses rodio (cpal output + Symphonia decoding) with one mixer and one
//! `Player` per attachment. Web uses a hidden `HtmlAudioElement`, fetching
//! authenticated API URLs into a Blob URL first. The transcript keeps the
//! engine in its own state, so a virtual row can unmount and remount without
//! stopping playback. Leaving the thread stops its clips.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioStatus {
    Loading,
    Ready,
    Playing,
    Paused,
    Ended,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioSnapshot {
    pub status: AudioStatus,
    pub position: f64,
    pub duration: Option<f64>,
    pub volume: f32,
    pub muted: bool,
    pub error: Option<String>,
}

impl Default for AudioSnapshot {
    fn default() -> Self {
        Self {
            status: AudioStatus::Ready,
            position: 0.0,
            duration: None,
            volume: 1.0,
            muted: false,
            error: None,
        }
    }
}

impl AudioSnapshot {
    pub fn progress(&self) -> f32 {
        match self.duration {
            Some(duration) if duration > 0.0 => (self.position / duration).clamp(0.0, 1.0) as f32,
            _ => 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioSource {
    File(PathBuf),
    Url(String),
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::{AudioSnapshot, AudioSource, AudioStatus};
    use std::path::{Path, PathBuf};

    /// The transcript's audio service. Clones share the same mixer and clips.
    #[derive(Clone)]
    pub struct AudioEngine {
        inner: std::sync::Arc<std::sync::Mutex<Inner>>,
    }

    struct Inner {
        sink: Option<rodio::MixerDeviceSink>,
        mixer: Option<rodio::mixer::Mixer>,
        init_error: Option<String>,
        clips: std::collections::HashMap<String, Clip>,
        /// Bumped on asynchronous state changes so the shell repaints a
        /// finished/failed clip once without polling it forever.
        revision: u64,
    }

    struct Clip {
        player: Option<rodio::Player>,
        status: AudioStatus,
        duration: Option<f64>,
        volume: f32,
        muted: bool,
        error: Option<String>,
        /// A background load only installs itself when its generation is current.
        generation: u64,
    }

    impl std::fmt::Debug for AudioEngine {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let inner = self.inner.lock().unwrap();
            f.debug_struct("AudioEngine")
                .field("clips", &inner.clips.len())
                .field("error", &inner.init_error)
                .finish()
        }
    }

    impl Default for AudioEngine {
        fn default() -> Self {
            Self::new()
        }
    }

    impl AudioEngine {
        pub fn new() -> Self {
            Self {
                inner: std::sync::Arc::new(std::sync::Mutex::new(Inner {
                    sink: None,
                    mixer: None,
                    init_error: None,
                    clips: std::collections::HashMap::new(),
                    revision: 0,
                })),
            }
        }

        fn ensure_mixer(inner: &mut Inner) -> Result<rodio::mixer::Mixer, String> {
            if let Some(error) = &inner.init_error {
                return Err(error.clone());
            }
            if let Some(mixer) = &inner.mixer {
                return Ok(mixer.clone());
            }
            match rodio::DeviceSinkBuilder::open_default_sink() {
                Ok(mut sink) => {
                    sink.log_on_drop(false);
                    let mixer = sink.mixer().clone();
                    inner.sink = Some(sink);
                    inner.mixer = Some(mixer.clone());
                    Ok(mixer)
                }
                Err(error) => {
                    let message = format!("No audio output device: {error}");
                    inner.init_error = Some(message.clone());
                    Err(message)
                }
            }
        }

        /// Start a clip, or resume it if it is already loaded.
        pub fn play(&self, id: &str, source: AudioSource) {
            let generation;
            {
                let mut inner = self.inner.lock().unwrap();
                if let Some(clip) = inner.clips.get_mut(id) {
                    if let Some(player) = &clip.player {
                        if clip.status == AudioStatus::Ended {
                            let _ = player.try_seek(std::time::Duration::ZERO);
                        }
                        player.play();
                        clip.status = AudioStatus::Playing;
                        return;
                    }
                    if clip.status == AudioStatus::Loading {
                        return;
                    }
                }
                generation = inner
                    .clips
                    .get(id)
                    .map_or(1, |clip| clip.generation.wrapping_add(1));
                let volume = inner.clips.get(id).map_or(1.0, |clip| clip.volume);
                let muted = inner.clips.get(id).is_some_and(|clip| clip.muted);
                inner.clips.insert(
                    id.to_string(),
                    Clip {
                        player: None,
                        status: AudioStatus::Loading,
                        duration: None,
                        volume,
                        muted,
                        error: None,
                        generation,
                    },
                );
                inner.revision = inner.revision.wrapping_add(1);
            }
            let engine = self.clone();
            let id = id.to_string();
            std::thread::spawn(move || engine.load(id, source, generation));
        }

        fn load(&self, id: String, source: AudioSource, generation: u64) {
            let result = (|| -> Result<(rodio::Player, Option<f64>), String> {
                let path = match source {
                    AudioSource::File(path) => path,
                    AudioSource::Url(url) => resolve_url(&url)?,
                };
                let file = std::fs::File::open(&path)
                    .map_err(|error| format!("Cannot open {}: {error}", path.display()))?;
                let decoder = rodio::Decoder::try_from(file)
                    .map_err(|error| format!("Cannot decode this audio: {error}"))?;
                let duration = rodio::Source::total_duration(&decoder).map(|d| d.as_secs_f64());
                let mut inner = self.inner.lock().unwrap();
                let (mixer, volume, muted) = {
                    let mixer = Self::ensure_mixer(&mut inner)?;
                    let clip = inner.clips.get(&id).ok_or("Audio clip was removed")?;
                    (mixer, clip.volume, clip.muted)
                };
                let player = rodio::Player::connect_new(&mixer);
                player.append(decoder);
                player.set_volume(if muted { 0.0 } else { volume });
                player.play();
                let current = inner.clips.get(&id).map_or(0, |clip| clip.generation);
                if current != generation {
                    return Err("Audio clip was replaced".to_string());
                }
                Ok((player, duration))
            })();
            let mut inner = self.inner.lock().unwrap();
            let Some(clip) = inner.clips.get_mut(&id) else {
                return;
            };
            if clip.generation != generation {
                return;
            }
            match result {
                Ok((player, duration)) => {
                    clip.player = Some(player);
                    clip.duration = duration;
                    clip.status = AudioStatus::Playing;
                    clip.error = None;
                }
                Err(error) => {
                    clip.status = AudioStatus::Failed;
                    clip.error = Some(error);
                }
            }
            inner.revision = inner.revision.wrapping_add(1);
        }

        pub fn pause(&self, id: &str) {
            let mut inner = self.inner.lock().unwrap();
            if let Some(clip) = inner.clips.get_mut(id) {
                if let Some(player) = &clip.player {
                    player.pause();
                    clip.status = AudioStatus::Paused;
                }
            }
        }

        pub fn resume(&self, id: &str) {
            let mut inner = self.inner.lock().unwrap();
            if let Some(clip) = inner.clips.get_mut(id) {
                if let Some(player) = &clip.player {
                    if clip.status == AudioStatus::Ended {
                        let _ = player.try_seek(std::time::Duration::ZERO);
                    }
                    player.play();
                    clip.status = AudioStatus::Playing;
                }
            }
        }

        pub fn seek(&self, id: &str, seconds: f64) -> bool {
            let mut inner = self.inner.lock().unwrap();
            let Some(clip) = inner.clips.get_mut(id) else {
                return false;
            };
            let Some(player) = &clip.player else {
                return false;
            };
            let seconds = if seconds.is_finite() {
                seconds.max(0.0)
            } else {
                0.0
            };
            match player.try_seek(std::time::Duration::from_secs_f64(seconds)) {
                Ok(()) => {
                    clip.status = AudioStatus::Playing;
                    true
                }
                Err(error) => {
                    clip.error = Some(format!("Cannot seek this clip: {error}"));
                    false
                }
            }
        }

        pub fn set_muted(&self, id: &str, muted: bool) {
            let mut inner = self.inner.lock().unwrap();
            if let Some(clip) = inner.clips.get_mut(id) {
                clip.muted = muted;
                if let Some(player) = &clip.player {
                    player.set_volume(if muted { 0.0 } else { clip.volume });
                }
            }
        }

        pub fn stop_all(&self) {
            let mut inner = self.inner.lock().unwrap();
            for (_, mut clip) in inner.clips.drain() {
                if let Some(player) = clip.player.take() {
                    player.stop();
                }
            }
        }

        pub fn snapshot(&self, id: &str) -> Option<AudioSnapshot> {
            let mut inner = self.inner.lock().unwrap();
            let clip = inner.clips.get_mut(id)?;
            let mut status = clip.status;
            let mut position = 0.0;
            if let Some(player) = &clip.player {
                position = player.get_pos().as_secs_f64();
                if player.is_paused() {
                    status = AudioStatus::Paused;
                } else if player.empty() {
                    status = AudioStatus::Ended;
                    position = clip.duration.unwrap_or(position);
                } else {
                    status = AudioStatus::Playing;
                }
            }
            let status_changed = clip.status != status;
            clip.status = status;
            let snapshot = AudioSnapshot {
                status,
                position,
                duration: clip.duration,
                volume: clip.volume,
                muted: clip.muted,
                error: clip.error.clone(),
            };
            if status_changed {
                inner.revision = inner.revision.wrapping_add(1);
            }
            Some(snapshot)
        }

        pub fn revision(&self) -> u64 {
            self.inner.lock().unwrap().revision
        }

        pub fn has_active(&self) -> bool {
            let inner = self.inner.lock().unwrap();
            inner
                .clips
                .values()
                .any(|clip| matches!(clip.status, AudioStatus::Loading | AudioStatus::Playing))
        }
    }

    fn resolve_url(url: &str) -> Result<PathBuf, String> {
        if let Some(path) = url.strip_prefix("file://") {
            let path = path.trim_start_matches('/');
            let path = PathBuf::from(percent_decode(path));
            if path.is_file() {
                return Ok(path);
            }
        }
        if !url.starts_with("http://") && !url.starts_with("https://") {
            let path = PathBuf::from(url);
            if path.is_file() {
                return Ok(path);
            }
            return Err(format!("Audio file not found: {url}"));
        }
        let cache = audio_cache_dir();
        std::fs::create_dir_all(&cache)
            .map_err(|error| format!("Cannot create audio cache: {error}"))?;
        let digest = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(url.as_bytes());
            format!("{:x}", hasher.finalize())
        };
        let extension = url
            .split('?')
            .next()
            .and_then(|path| Path::new(path).extension())
            .and_then(|ext| ext.to_str())
            .filter(|ext| ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric()))
            .unwrap_or("audio");
        let path = cache.join(format!("{digest}.{extension}"));
        if path.is_file() {
            return Ok(path);
        }
        let part = cache.join(format!("{digest}.part"));
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(15))
            .timeout_read(std::time::Duration::from_secs(60))
            .build();
        let response = agent
            .get(url)
            .call()
            .map_err(|error| format!("Cannot download audio: {error}"))?;
        let mut reader = response.into_reader();
        let mut file =
            std::fs::File::create(&part).map_err(|error| format!("Cannot cache audio: {error}"))?;
        std::io::copy(&mut reader, &mut file)
            .map_err(|error| format!("Cannot save audio: {error}"))?;
        std::fs::rename(&part, &path)
            .map_err(|error| format!("Cannot finish audio cache: {error}"))?;
        prune_cache(&cache, 500 * 1024 * 1024);
        Ok(path)
    }

    fn prune_cache(dir: &Path, cap: u64) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
            .flatten()
            .filter_map(|entry| {
                let meta = entry.metadata().ok()?;
                meta.is_file().then(|| {
                    (
                        meta.modified().unwrap_or(std::time::UNIX_EPOCH),
                        meta.len(),
                        entry.path(),
                    )
                })
            })
            .collect();
        let mut total: u64 = files.iter().map(|(_, size, _)| *size).sum();
        if total <= cap {
            return;
        }
        files.sort_by_key(|(modified, _, _)| *modified);
        for (_, size, path) in files {
            if total <= cap {
                break;
            }
            if std::fs::remove_file(path).is_ok() {
                total = total.saturating_sub(size);
            }
        }
    }

    fn audio_cache_dir() -> PathBuf {
        #[cfg(windows)]
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            return PathBuf::from(local)
                .join("StarkRune")
                .join("cache")
                .join("audio");
        }
        std::env::temp_dir().join("stark-rune-audio")
    }

    fn percent_decode(text: &str) -> String {
        let bytes = text.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%' && index + 2 < bytes.len() {
                let hex = &text[index + 1..index + 3];
                if let Ok(value) = u8::from_str_radix(hex, 16) {
                    out.push(value);
                    index += 3;
                    continue;
                }
            }
            out.push(bytes[index]);
            index += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn wav_fixture() -> std::path::PathBuf {
            let path =
                std::env::temp_dir().join(format!("stark-audio-decode-{}.wav", std::process::id()));
            let sample_rate = 8_000u32;
            let samples: Vec<i16> = (0..800)
                .map(|i| ((i as f32 * 0.2).sin() * 4_000.0) as i16)
                .collect();
            let data_len = (samples.len() * 2) as u32;
            let mut bytes = Vec::new();
            bytes.extend_from_slice(b"RIFF");
            bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
            bytes.extend_from_slice(b"WAVEfmt ");
            bytes.extend_from_slice(&16u32.to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes());
            bytes.extend_from_slice(&sample_rate.to_le_bytes());
            bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
            bytes.extend_from_slice(&2u16.to_le_bytes());
            bytes.extend_from_slice(&16u16.to_le_bytes());
            bytes.extend_from_slice(b"data");
            bytes.extend_from_slice(&data_len.to_le_bytes());
            for sample in samples {
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
            std::fs::write(&path, bytes).unwrap();
            path
        }

        #[test]
        fn wav_decodes_with_a_duration_without_an_output_device() {
            let path = wav_fixture();
            let file = std::fs::File::open(&path).unwrap();
            let decoder = rodio::Decoder::try_from(file).unwrap();
            let duration = rodio::Source::total_duration(&decoder).unwrap();
            assert!(duration.as_secs_f64() > 0.09 && duration.as_secs_f64() < 0.12);
            let _ = std::fs::remove_file(path);
        }

        #[test]
        fn file_urls_decode_and_plain_paths_work() {
            let path = std::env::temp_dir().join(format!("stark-audio-{}.wav", std::process::id()));
            std::fs::write(&path, b"x").unwrap();
            assert_eq!(resolve_url(path.to_str().unwrap()).unwrap(), path);
            let url = format!("file:///{}", path.to_string_lossy().replace('\\', "/"));
            assert_eq!(resolve_url(&url).unwrap(), path);
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use super::{AudioSnapshot, AudioSource, AudioStatus};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;
    use wasm_bindgen::JsCast;

    #[derive(Clone)]
    pub struct AudioEngine {
        inner: Rc<RefCell<Inner>>,
    }

    struct Inner {
        clips: HashMap<String, Clip>,
        revision: u64,
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            for (_, clip) in self.clips.drain() {
                release_web(clip);
            }
        }
    }

    struct Clip {
        element: Option<web_sys::HtmlAudioElement>,
        status: AudioStatus,
        duration: Option<f64>,
        volume: f32,
        muted: bool,
        error: Option<String>,
        revoke: bool,
        generation: u64,
    }

    impl std::fmt::Debug for AudioEngine {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("AudioEngine")
                .field("clips", &self.inner.borrow().clips.len())
                .finish()
        }
    }

    impl Default for AudioEngine {
        fn default() -> Self {
            Self::new()
        }
    }

    impl AudioEngine {
        pub fn new() -> Self {
            Self {
                inner: Rc::new(RefCell::new(Inner {
                    clips: HashMap::new(),
                    revision: 0,
                })),
            }
        }

        pub fn play(&self, id: &str, source: AudioSource) {
            let generation;
            {
                let mut inner = self.inner.borrow_mut();
                if let Some(clip) = inner.clips.get_mut(id) {
                    if let Some(element) = &clip.element {
                        if clip.status == AudioStatus::Ended {
                            element.set_current_time(0.0);
                        }
                        let _ = element.play();
                        clip.status = AudioStatus::Playing;
                        return;
                    }
                    if clip.status == AudioStatus::Loading {
                        return;
                    }
                }
                generation = inner
                    .clips
                    .get(id)
                    .map_or(1, |clip| clip.generation.wrapping_add(1));
                let volume = inner.clips.get(id).map_or(1.0, |clip| clip.volume);
                let muted = inner.clips.get(id).is_some_and(|clip| clip.muted);
                inner.clips.insert(
                    id.to_string(),
                    Clip {
                        element: None,
                        status: AudioStatus::Loading,
                        duration: None,
                        volume,
                        muted,
                        error: None,
                        revoke: false,
                        generation,
                    },
                );
                inner.revision = inner.revision.wrapping_add(1);
            }
            let url = match source {
                AudioSource::Url(url) => url,
                AudioSource::File(path) => path.to_string_lossy().into_owned(),
            };
            if url.starts_with("blob:") || url.starts_with("data:") {
                self.attach(id, generation, url, false);
                return;
            }
            let inner = self.inner.clone();
            let id = id.to_string();
            wasm_bindgen_futures::spawn_local(async move {
                let result = crate::transport::fetch_blob_url(&url).await;
                let mut state = inner.borrow_mut();
                let Some(clip) = state.clips.get_mut(&id) else {
                    if let Ok((url, true)) = &result {
                        let _ = web_sys::Url::revoke_object_url(url);
                    }
                    return;
                };
                if clip.generation != generation {
                    if let Ok((url, true)) = &result {
                        let _ = web_sys::Url::revoke_object_url(url);
                    }
                    return;
                }
                match result {
                    Ok((blob_url, revoke)) => {
                        clip.revoke = revoke;
                        drop(state);
                        attach_web(&inner, &id, blob_url, generation);
                    }
                    Err(error) => {
                        clip.status = AudioStatus::Failed;
                        clip.error = Some(error);
                        state.revision = state.revision.wrapping_add(1);
                    }
                }
            });
        }

        fn attach(&self, id: &str, generation: u64, url: String, revoke: bool) {
            {
                let mut inner = self.inner.borrow_mut();
                if let Some(clip) = inner.clips.get_mut(id) {
                    if clip.generation == generation {
                        clip.revoke = revoke;
                    }
                }
            }
            attach_web(&self.inner, id, url, generation);
        }

        pub fn pause(&self, id: &str) {
            let mut inner = self.inner.borrow_mut();
            if let Some(clip) = inner.clips.get_mut(id) {
                if let Some(element) = &clip.element {
                    element.pause().ok();
                    clip.status = AudioStatus::Paused;
                }
            }
        }

        pub fn resume(&self, id: &str) {
            let mut inner = self.inner.borrow_mut();
            if let Some(clip) = inner.clips.get_mut(id) {
                if let Some(element) = &clip.element {
                    if clip.status == AudioStatus::Ended {
                        element.set_current_time(0.0);
                    }
                    let _ = element.play();
                    clip.status = AudioStatus::Playing;
                }
            }
        }

        pub fn toggle(&self, id: &str) {
            match self.snapshot(id).map(|snapshot| snapshot.status) {
                Some(AudioStatus::Playing) => self.pause(id),
                Some(AudioStatus::Paused | AudioStatus::Ready | AudioStatus::Ended) => {
                    self.resume(id)
                }
                _ => {}
            }
        }

        pub fn seek(&self, id: &str, seconds: f64) -> bool {
            let mut inner = self.inner.borrow_mut();
            let Some(clip) = inner.clips.get_mut(id) else {
                return false;
            };
            let Some(element) = &clip.element else {
                return false;
            };
            element.set_current_time(if seconds.is_finite() {
                seconds.max(0.0)
            } else {
                0.0
            });
            true
        }

        pub fn set_muted(&self, id: &str, muted: bool) {
            let mut inner = self.inner.borrow_mut();
            if let Some(clip) = inner.clips.get_mut(id) {
                clip.muted = muted;
                if let Some(element) = &clip.element {
                    element.set_muted(muted);
                }
            }
        }

        pub fn stop(&self, id: &str) {
            let mut inner = self.inner.borrow_mut();
            if let Some(clip) = inner.clips.remove(id) {
                release_web(clip);
            }
        }

        pub fn stop_all(&self) {
            let clips: Vec<Clip> = self
                .inner
                .borrow_mut()
                .clips
                .drain()
                .map(|(_, c)| c)
                .collect();
            for clip in clips {
                release_web(clip);
            }
        }

        pub fn snapshot(&self, id: &str) -> Option<AudioSnapshot> {
            let mut inner = self.inner.borrow_mut();
            let clip = inner.clips.get_mut(id)?;
            let mut status = clip.status;
            let mut position = 0.0;
            if let Some(element) = &clip.element {
                let duration = element.duration();
                if duration.is_finite() && duration > 0.0 {
                    clip.duration = Some(duration);
                }
                position = element.current_time();
                if element.ready_state() < 1 {
                    status = AudioStatus::Loading;
                } else if element.paused() {
                    status = if clip.duration.is_some_and(|d| position + 0.05 >= d) {
                        AudioStatus::Ended
                    } else {
                        AudioStatus::Paused
                    };
                } else {
                    status = AudioStatus::Playing;
                }
            }
            let status_changed = clip.status != status;
            clip.status = status;
            let snapshot = AudioSnapshot {
                status,
                position,
                duration: clip.duration,
                volume: clip.volume,
                muted: clip.muted,
                error: clip.error.clone(),
            };
            if status_changed {
                inner.revision = inner.revision.wrapping_add(1);
            }
            Some(snapshot)
        }

        pub fn revision(&self) -> u64 {
            self.inner.borrow().revision
        }

        pub fn has_active(&self) -> bool {
            self.inner
                .borrow()
                .clips
                .values()
                .any(|clip| matches!(clip.status, AudioStatus::Loading | AudioStatus::Playing))
        }
    }

    fn attach_web(inner: &Rc<RefCell<Inner>>, id: &str, url: String, generation: u64) {
        {
            let state = inner.borrow();
            match state.clips.get(id) {
                Some(clip) if clip.generation == generation => {}
                _ => return,
            }
        }
        let blob_url = url.starts_with("blob:");
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(document) = window.document() else {
            return;
        };
        let Some(element) = document
            .create_element("audio")
            .ok()
            .and_then(|element| element.dyn_into::<web_sys::HtmlAudioElement>().ok())
        else {
            return;
        };
        element.set_src(&url);
        element.set_preload("metadata");
        let _ = element.set_attribute("style", "display:none");
        if let Some(body) = document.body() {
            let _ = body.append_child(&element);
        }
        let mut state = inner.borrow_mut();
        let Some(clip) = state.clips.get_mut(id) else {
            let _ = element.remove();
            if blob_url {
                let _ = web_sys::Url::revoke_object_url(&url);
            }
            return;
        };
        if clip.generation != generation {
            let _ = element.remove();
            if blob_url {
                let _ = web_sys::Url::revoke_object_url(&url);
            }
            return;
        }
        element.set_volume(if clip.muted { 0.0 } else { clip.volume as f64 });
        clip.element = Some(element.clone());
        clip.status = AudioStatus::Loading;
        drop(state);
        let inner = inner.clone();
        let id = id.to_string();
        match element.play() {
            Ok(promise) => {
                wasm_bindgen_futures::spawn_local(async move {
                    if wasm_bindgen_futures::JsFuture::from(promise).await.is_err() {
                        let mut state = inner.borrow_mut();
                        if let Some(clip) = state.clips.get_mut(&id) {
                            clip.status = AudioStatus::Failed;
                            clip.error = Some("Tap play to start this audio.".to_string());
                        }
                    }
                });
            }
            Err(_) => {
                let mut state = inner.borrow_mut();
                if let Some(clip) = state.clips.get_mut(&id) {
                    clip.status = AudioStatus::Failed;
                    clip.error = Some("Tap play to start this audio.".to_string());
                }
            }
        }
    }

    fn release_web(clip: Clip) {
        if let Some(element) = clip.element {
            element.pause().ok();
            if let Some(parent) = element.parent_node() {
                let _ = parent.remove_child(&element);
            }
            let src = element.src();
            if clip.revoke && src.starts_with("blob:") {
                let _ = web_sys::Url::revoke_object_url(&src);
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::AudioEngine;
#[cfg(target_arch = "wasm32")]
pub use web::AudioEngine;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_and_defaults_are_stable() {
        let snapshot = AudioSnapshot {
            duration: Some(4.0),
            position: 1.0,
            ..AudioSnapshot::default()
        };
        assert!((snapshot.progress() - 0.25).abs() < 1e-6);
        assert_eq!(AudioSnapshot::default().progress(), 0.0);
    }
}
