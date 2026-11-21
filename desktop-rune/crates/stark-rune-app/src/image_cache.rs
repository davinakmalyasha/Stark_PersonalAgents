//! Shared Rune image cache for attachment thumbnails and previews.
//!
//! The cache needs a waker from `AppSpec::setup`, so the app entry installs it
//! once; headless unit tests never do and simply fall back to icon chips.
//! Native decodes `attachment.path` (or a `file:` URL); the web shell fetches
//! the stored URL or the same-origin attachment endpoint.

use std::sync::{Mutex, OnceLock};

use rune::{Image, ImageCache};

use crate::chats::ChatAttachment;

static CACHE: OnceLock<Mutex<ImageCache>> = OnceLock::new();

/// Install the process-wide cache. Called from every `AppSpec::setup`; later
/// installs are no-ops (the first waker keeps the runtime alive).
pub fn install(waker: rune::Waker) {
    let _ = CACHE.set(Mutex::new(ImageCache::new(waker)));
}

/// `AppSpec::setup` adapter: each spec has its own state type, but the cache
/// is process-wide.
pub fn setup<S>(_: &mut S, waker: rune::Waker) {
    install(waker);
}

/// The attachment's picture: the local upload path natively, or the API URL
/// on the web (where a server path is meaningless to the browser).
pub fn attachment(
    attachment: &ChatAttachment,
    size_pt: f32,
    workspace_id: &str,
    chat_id: &str,
) -> Option<Image> {
    // The web build builds the API URL from these; native reads the path.
    let _ = (workspace_id, chat_id);
    #[cfg(not(target_arch = "wasm32"))]
    {
        if !attachment.path.trim().is_empty() && std::path::Path::new(&attachment.path).exists() {
            return file(&attachment.path, size_pt);
        }
        // Native only loads local paths/file: URLs — a remote API URL needs
        // bytes fetched by the caller, so leave the icon chip in its place.
        if attachment
            .url
            .as_deref()
            .is_some_and(|url| url.starts_with("file:"))
        {
            return attachment
                .url
                .as_deref()
                .and_then(|url| url_image(url, size_pt));
        }
        None
    }
    #[cfg(target_arch = "wasm32")]
    {
        let absolute = |url: &str| {
            if url.starts_with('/') {
                crate::transport::api_url(url)
            } else {
                url.to_string()
            }
        };
        if let Some(url) = attachment.url.as_deref() {
            if let Some(image) = url_image(&absolute(url), size_pt) {
                return Some(image);
            }
        }
        if !workspace_id.trim().is_empty()
            && !chat_id.trim().is_empty()
            && !attachment.id.trim().is_empty()
        {
            let url = absolute(&format!(
                "/api/workspaces/{workspace_id}/chats/{chat_id}/attachments/{}",
                attachment.id
            ));
            return url_image(&url, size_pt);
        }
        None
    }
}

fn url_image(url: &str, size_pt: f32) -> Option<Image> {
    if url.trim().is_empty() {
        return None;
    }
    let cache = CACHE.get()?;
    let cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    Some(cache.url(url, size_pt, 2.0))
}

/// Decode a local image path at `size_pt` points for the current display.
/// `None` before the app entry installed the cache (tests, touch-free scenes)
/// or for an empty path. The image decodes off-thread; callers pass
/// `image.pixels()` to `.pixels_fit(...)` and Rune redraws when it lands.
pub fn file(path: &str, size_pt: f32) -> Option<Image> {
    if path.trim().is_empty() {
        return None;
    }
    let cache = CACHE.get()?;
    let cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    Some(cache.file(path, size_pt, 2.0))
}
