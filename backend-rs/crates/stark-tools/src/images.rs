//! Inline image collection for vision-capable runs.
//!
//! Chat attachments (uploaded files, voice screen frames) arrive at `/api/run`
//! as JSON descriptors with a server-side path. This module resolves the image
//! ones to bytes so the run can send real `image_url` parts instead of path
//! notes. Reads are containment-checked against caller-supplied roots and
//! size-capped so a hostile descriptor cannot exfiltrate files or blow up the
//! context window.

use std::path::{Path, PathBuf};

use base64::Engine;
use serde_json::Value;

pub const MAX_INLINE_IMAGE_BYTES: usize = 2_500_000;
pub const MAX_INLINE_IMAGES: usize = 4;

#[derive(Debug, Clone)]
pub struct InlineImage {
    pub mime: String,
    pub name: String,
    pub bytes: Vec<u8>,
}

impl InlineImage {
    pub fn data_url(&self) -> String {
        format!(
            "data:{};base64,{}",
            self.mime,
            base64::engine::general_purpose::STANDARD.encode(&self.bytes)
        )
    }
}

pub fn mime_for_ext(ext: &str) -> Option<&'static str> {
    match ext.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

pub fn is_image_mime(mime: &str) -> bool {
    mime.trim().to_ascii_lowercase().starts_with("image/")
}

fn canonical_under(path: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let canonical = std::fs::canonicalize(path).ok()?;
    roots.iter().find_map(|root| {
        let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        (canonical == canonical_root || canonical.starts_with(&canonical_root))
            .then(|| canonical.clone())
    })
}

fn attachment_path(attachment: &Value) -> Option<String> {
    attachment
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Collect image bytes for `attachments`, restricted to `allowed_roots`.
/// Non-image entries, missing files, escapes and oversized files are skipped.
pub fn collect_inline_images(allowed_roots: &[PathBuf], attachments: &[Value]) -> Vec<InlineImage> {
    let mut out = Vec::new();
    for attachment in attachments {
        if out.len() >= MAX_INLINE_IMAGES {
            break;
        }
        let Some(path) = attachment_path(attachment) else {
            continue;
        };
        let mime = attachment
            .get("type")
            .or_else(|| attachment.get("mime"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let flagged_image = attachment
            .get("isImage")
            .or_else(|| attachment.get("screenFrame"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let ext_mime = Path::new(&path)
            .extension()
            .and_then(|ext| ext.to_str())
            .and_then(mime_for_ext)
            .unwrap_or("");
        if !is_image_mime(&mime) && !(flagged_image && !ext_mime.is_empty()) {
            continue;
        }
        let resolved = match canonical_under(Path::new(&path), allowed_roots) {
            Some(resolved) => resolved,
            None => continue,
        };
        let bytes = match std::fs::read(&resolved) {
            Ok(bytes) if !bytes.is_empty() && bytes.len() <= MAX_INLINE_IMAGE_BYTES => bytes,
            _ => continue,
        };
        let name = attachment
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("image")
            .to_string();
        out.push(InlineImage {
            mime: if is_image_mime(&mime) {
                mime
            } else {
                ext_mime.to_string()
            },
            name,
            bytes,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn project() -> tempfile::TempDir {
        tempfile::TempDir::new().unwrap()
    }

    fn attachment(path: &std::path::Path, mime: &str, name: &str) -> Value {
        json!({"name": name, "type": mime, "size": 4, "path": path.to_string_lossy()})
    }

    #[test]
    fn collects_images_only_inside_allowed_roots() {
        let dir = project();
        let uploads = dir.path().join("uploads");
        std::fs::create_dir_all(&uploads).unwrap();
        let image = uploads.join("shot.png");
        let mut file = std::fs::File::create(&image).unwrap();
        file.write_all(b"\x89PNG\r\n\x1a\nfake").unwrap();
        let text = uploads.join("note.txt");
        std::fs::write(&text, "hello").unwrap();

        let found = collect_inline_images(
            std::slice::from_ref(&uploads),
            &[
                attachment(&image, "image/png", "shot.png"),
                attachment(&text, "text/plain", "note.txt"),
            ],
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].mime, "image/png");
        assert!(found[0].data_url().starts_with("data:image/png;base64,"));
    }

    #[test]
    fn rejects_escapes_and_oversized_files() {
        let dir = project();
        let uploads = dir.path().join("uploads");
        std::fs::create_dir_all(&uploads).unwrap();
        let outside = dir.path().join("secret.png");
        std::fs::write(&outside, b"fake").unwrap();
        let big = uploads.join("big.png");
        std::fs::write(&big, vec![0u8; MAX_INLINE_IMAGE_BYTES + 1]).unwrap();

        let found = collect_inline_images(
            std::slice::from_ref(&uploads),
            &[
                attachment(&outside, "image/png", "secret.png"),
                attachment(&big, "image/png", "big.png"),
            ],
        );
        assert!(found.is_empty());
    }

    #[test]
    fn screen_frame_flag_with_image_extension_counts() {
        let dir = project();
        let frames = dir.path().join("live_screen");
        std::fs::create_dir_all(&frames).unwrap();
        let frame = frames.join("turn.jpg");
        std::fs::write(&frame, b"\xff\xd8fake").unwrap();
        let descriptor = json!({
            "name": "turn.jpg",
            "type": "application/octet-stream",
            "screenFrame": true,
            "path": frame.to_string_lossy(),
        });
        let found = collect_inline_images(&[frames], &[descriptor]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].mime, "image/jpeg");
    }
}
