use base64::Engine;
use serde_json::Value;

use crate::paths::resolve_workspace_path;
use crate::{ToolContext, ToolOutcome};

const DEFAULT_LIMIT: usize = 2000;
const MAX_LIMIT: usize = 10000;
const MAX_LINE_CHARS: usize = 2000;

pub async fn read_file(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    if let Some(paths) = args.get("file_paths").and_then(Value::as_array) {
        let mut parts = Vec::new();
        let mut is_error = false;
        for path in paths {
            if let Some(path) = path.as_str().filter(|path| !path.trim().is_empty()) {
                let outcome = read_one(ctx, path, args).await;
                is_error |= outcome.is_error;
                parts.push(outcome.content);
            }
        }
        if parts.is_empty() {
            return ToolOutcome::err("Error: file_paths empty or invalid");
        }
        return ToolOutcome::with_code(parts.join("\n\n"), is_error, None);
    }

    if let Some(path) = args.get("file_paths").and_then(Value::as_str) {
        return read_one(ctx, path, args).await;
    }

    match args.get("file_path").and_then(Value::as_str) {
        Some(path) if !path.trim().is_empty() => read_one(ctx, path, args).await,
        _ => ToolOutcome::err("Error: read_file requires file_path or file_paths"),
    }
}

async fn read_one(ctx: &ToolContext, raw_path: &str, args: &Value) -> ToolOutcome {
    let path = match resolve_workspace_path(&ctx.workspace, raw_path) {
        Ok(path) => path,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };

    if let Some(ext) = path.extension().and_then(|ext| ext.to_str())
        && let Some(mime) = crate::images::mime_for_ext(ext)
    {
        return read_image(ctx, &path, raw_path, mime).await;
    }

    let content = match tokio::fs::read_to_string(&path).await {
        Ok(content) => content,
        Err(error) => return ToolOutcome::err(format!("Error: {raw_path}: {error}")),
    };

    let lines: Vec<String> = content.lines().map(trim_line).collect();
    let total = lines.len();
    let offset = args
        .get("offset")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as usize;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .filter(|limit| *limit > 0)
        .map(|limit| limit as usize)
        .unwrap_or(DEFAULT_LIMIT)
        .min(MAX_LIMIT);

    if total > 0 && offset > total {
        return ToolOutcome::err(format!(
            "Error: offset {offset} is beyond the end of {raw_path} ({total} lines)"
        ));
    }

    let start = offset.saturating_sub(1).min(total);
    let end = (start + limit).min(total);
    let start_line = if total == 0 { 0 } else { start + 1 };
    let mut output = format!("[FILE] {raw_path} (lines {start_line}-{end} of {total})\n");
    if end < total {
        output.push_str(&format!(
            "[PARTIAL view: returned lines {start_line}-{end} of {total}. \
             Use read_file(offset={}) to continue, or search_code to locate specific lines.]\n",
            end + 1
        ));
    }
    if start < end {
        output.push_str(&lines[start..end].join("\n"));
        output.push('\n');
    }
    ToolOutcome::ok(output)
}

fn trim_line(line: &str) -> String {
    let line = line.trim_end_matches('\r');
    if line.chars().count() <= MAX_LINE_CHARS {
        return line.to_string();
    }
    let mut out: String = line.chars().take(MAX_LINE_CHARS).collect();
    out.push_str("... (line truncated to 2000 chars)");
    out
}

/// Image reads queue the file for the model's next step (tool results are
/// text-only) and return a short note. The agent loop drains the queue into
/// a real `image_url` part, so no pixels travel through chat text.
async fn read_image(
    ctx: &ToolContext,
    path: &std::path::Path,
    raw_path: &str,
    mime: &str,
) -> ToolOutcome {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) => return ToolOutcome::err(format!("Error: {raw_path}: {error}")),
    };
    if bytes.is_empty() {
        return ToolOutcome::err(format!("Error: {raw_path} is empty"));
    }
    if bytes.len() > crate::images::MAX_INLINE_IMAGE_BYTES {
        return ToolOutcome::err(format!(
            "Error: {raw_path} is too large for inline vision ({} bytes, max {}); downscale it first",
            bytes.len(),
            crate::images::MAX_INLINE_IMAGE_BYTES
        ));
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(raw_path)
        .to_string();
    crate::screen::queue_image(
        &ctx.chat_id,
        format!(
            "data:{};base64,{}",
            mime,
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        ),
        name.clone(),
    );
    ToolOutcome::ok(format!(
        "[IMAGE] {raw_path} ({mime}, {} bytes) queued for the model's next step",
        bytes.len()
    ))
}
