use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::{ToolContext, ToolOutcome};

const VALID_VERTICALS: &[&str] = &[
    "fnb", "salon", "laundry", "klinik", "gym", "bimbel", "bengkel", "beauty", "travel",
    "property", "katering", "photo",
];

const ENDPOINTS: &[(&str, &str, &str)] = &[
    ("lead_list", "GET", "/api/agent/leads"),
    ("lead_get", "GET", "/api/agent/leads/{id}"),
    ("lead_update", "PATCH", "/api/agent/leads/{id}"),
    ("order_list", "GET", "/api/agent/orders"),
    ("order_get", "GET", "/api/agent/orders/{id}"),
    (
        "order_milestones",
        "GET",
        "/api/agent/orders/{id}/milestones",
    ),
    (
        "milestone_create",
        "POST",
        "/api/agent/orders/{orderId}/milestones",
    ),
    ("milestone_update", "PATCH", "/api/agent/milestones/{id}"),
    ("note_create", "POST", "/api/agent/orders/{orderId}/notes"),
    ("note_update", "PATCH", "/api/agent/notes/{id}"),
    (
        "activity_create",
        "POST",
        "/api/agent/orders/{orderId}/activities",
    ),
    ("request_list", "GET", "/api/agent/requests"),
    ("request_get", "GET", "/api/agent/requests/{id}"),
    ("request_update", "PATCH", "/api/agent/requests/{id}"),
    ("project_list", "GET", "/api/agent/projects"),
    ("project_get", "GET", "/api/agent/projects/{id}"),
    ("project_update", "PATCH", "/api/agent/projects/{id}"),
    ("article_list", "GET", "/api/agent/articles"),
    ("article_create", "POST", "/api/agent/articles"),
    ("article_update", "PATCH", "/api/agent/articles/{id}"),
    ("whatsapp_send", "POST", "/api/agent/whatsapp/send"),
];

static CLIENT_LOCK: tokio::sync::OnceCell<Mutex<()>> = tokio::sync::OnceCell::const_new();

async fn client_lock() -> &'static Mutex<()> {
    CLIENT_LOCK.get_or_init(|| async { Mutex::new(()) }).await
}

pub async fn dfd_bridge(_ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let base = std::env::var("DFD_API_URL")
        .unwrap_or_default()
        .trim()
        .trim_end_matches('/')
        .to_string();
    if base.is_empty() {
        return ToolOutcome::err(
            "Error: DFD_API_URL is not set. Configure it in the environment or .env.",
        );
    }
    let key = std::env::var("DFD_API_KEY").unwrap_or_default();
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let raw_url = args.get("url").and_then(Value::as_str).unwrap_or("").trim();
    let (method, mut path) = if raw_url.is_empty() {
        let Some((_, method, path)) = ENDPOINTS.iter().find(|(name, _, _)| *name == action) else {
            let known = ENDPOINTS
                .iter()
                .map(|(name, _, _)| *name)
                .collect::<Vec<_>>()
                .join(", ");
            return ToolOutcome::err(format!(
                "Error: unknown dfd_bridge action '{action}'. Known: {known}"
            ));
        };
        (method.to_string(), path.to_string())
    } else {
        (
            args.get("method")
                .and_then(Value::as_str)
                .unwrap_or("GET")
                .to_uppercase(),
            raw_url.to_string(),
        )
    };

    let id = args.get("id").and_then(Value::as_str).unwrap_or("");
    let order_id = args.get("orderId").and_then(Value::as_str).unwrap_or("");
    path = path.replace("{id}", id).replace("{orderId}", order_id);
    if let Some(params) = args.get("path").and_then(Value::as_object) {
        for (name, value) in params {
            let replacement = value
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| value.to_string());
            path = path.replace(&format!("{{{name}}}"), &replacement);
        }
    }
    let full_url = if path.starts_with('/') {
        format!("{base}{path}")
    } else {
        format!("{base}/{path}")
    };
    if key.trim().is_empty() {
        return ToolOutcome::err(format!(
            "Error: DFD_API_KEY is not set; cannot call {full_url}."
        ));
    }
    let timeout = args
        .get("timeout")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| {
            std::env::var("DFD_API_TIMEOUT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(30.0)
        })
        .max(1.0);
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs_f64(timeout))
        .build()
    {
        Ok(client) => client,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    let method = match reqwest::Method::from_bytes(method.as_bytes()) {
        Ok(method) => method,
        Err(error) => return ToolOutcome::err(format!("Error: invalid method: {error}")),
    };
    let mut request = client
        .request(method, &full_url)
        .bearer_auth(key)
        .header("Accept", "application/json");
    if let Some(payload) = args.get("payload").filter(|value| !value.is_null()) {
        request = request.json(payload);
    }
    match request.send().await {
        Ok(response) => {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            if status.is_success() {
                ToolOutcome::ok(text)
            } else {
                ToolOutcome::err(format!(
                    "Error: DFD API {} {}: {}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or(""),
                    text.chars().take(500).collect::<String>()
                ))
            }
        }
        Err(error) => ToolOutcome::err(format!("Error: DFD API request failed: {error}")),
    }
}

pub async fn create_client(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    create_client_in(ctx, args, None).await
}

async fn create_client_in(
    ctx: &ToolContext,
    args: &Value,
    root_override: Option<PathBuf>,
) -> ToolOutcome {
    let approved_by = args
        .get("approved_by")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if approved_by.is_empty()
        || matches!(
            approved_by.to_lowercase().as_str(),
            "owner" | "self" | "auto" | "yes" | "approve" | "approved"
        )
    {
        return ToolOutcome::err(
            "Error: create_client requires explicit owner approval (approved_by must be a real approval marker, e.g. 'telegram:<request_id>'). The DFD agent must obtain approval via the approval channel first.",
        );
    }
    let name = args
        .get("business_name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if name.is_empty() {
        return ToolOutcome::err("Error: business_name is required.");
    }
    let vertical = args
        .get("vertical")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if !VALID_VERTICALS.contains(&vertical.as_str()) {
        return ToolOutcome::err(format!(
            "Error: vertical must be one of: {}.",
            VALID_VERTICALS.join(", ")
        ));
    }

    let root = root_override.unwrap_or_else(|| {
        std::env::var("DFD_CLIENTS_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(r"D:\MyProject\DFDClientProjects"))
    });
    let slug = slugify(name);
    let _guard = client_lock().await.lock().await;
    if let Some(mut existing) = find_client(&ctx.harness_root, &slug).await {
        existing["path"] = json!(root.join(&slug).to_string_lossy());
        return ToolOutcome::ok(format_client(&existing, true));
    }

    let client_dir = root.join(&slug);
    if client_dir.exists() && !client_dir.is_dir() {
        return ToolOutcome::err(format!(
            "Error: Cannot create client: '{}' exists and is not a directory.",
            client_dir.display()
        ));
    }
    if let Err(error) = tokio::fs::create_dir_all(client_dir.join("mem")).await {
        return ToolOutcome::err(format!("Error: {error}"));
    }

    let now = Utc::now().to_rfc3339();
    let features = args
        .get("features")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let client = json!({
        "slug": slug,
        "name": name,
        "vertical": vertical,
        "features": features,
        "whatsapp": args.get("whatsapp").and_then(Value::as_str).unwrap_or("").trim(),
        "domain": args.get("domain").and_then(Value::as_str).unwrap_or("").trim(),
        "status": "active",
        "orderId": args.get("order_id").and_then(Value::as_str).unwrap_or(""),
        "createdAt": now,
        "updatedAt": now,
        "projects": [],
    });
    if let Err(error) = write_json(&client_dir.join("client.json"), &client).await {
        return ToolOutcome::err(format!("Error: {error}"));
    }
    let progress_path = client_dir.join("progress.md");
    if !progress_path.is_file() {
        let _ = tokio::fs::write(
            &progress_path,
            format!(
                "# {name} — Progress\n\n- {now}: Client scaffold created (approved by {approved_by}).\n"
            ),
        )
        .await;
    }
    let mem_readme = client_dir.join("mem").join("README.md");
    if !mem_readme.is_file() {
        let _ = tokio::fs::write(
            mem_readme,
            "# Client Memory\n\nShared memory for all chats of this client.\n",
        )
        .await;
    }
    if let Err(error) = update_index(&ctx.harness_root, &client).await {
        return ToolOutcome::err(format!("Error: {error}"));
    }
    let mut client = client;
    client["path"] = json!(client_dir.to_string_lossy());
    ToolOutcome::ok(format_client(&client, false))
}

fn format_client(client: &Value, already_existed: bool) -> String {
    let prefix = if already_existed {
        "[OK] Client scaffold already exists."
    } else {
        "[OK] Client scaffold created."
    };
    format!(
        "{prefix}\nFolder: {}\nSlug: {}\nVertical: {}",
        client["path"].as_str().unwrap_or(""),
        client["slug"].as_str().unwrap_or(""),
        client["vertical"].as_str().unwrap_or("")
    )
}

fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut pending_dash = false;
    for character in name.trim().to_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.push(character);
        } else {
            pending_dash = true;
        }
    }
    if slug.is_empty() {
        "client".to_string()
    } else {
        slug
    }
}

async fn find_client(harness_root: &Path, slug: &str) -> Option<Value> {
    let index = load_index(harness_root).await;
    index["clients"]
        .as_array()?
        .iter()
        .find(|client| client["slug"].as_str() == Some(slug))
        .cloned()
}

async fn load_index(harness_root: &Path) -> Value {
    let path = index_path(harness_root);
    let raw = match tokio::fs::read_to_string(path).await {
        Ok(raw) => raw,
        Err(_) => return json!({"clients": []}),
    };
    serde_json::from_str(&raw).unwrap_or_else(|_| json!({"clients": []}))
}

async fn update_index(harness_root: &Path, client: &Value) -> Result<(), String> {
    let path = index_path(harness_root);
    let mut index = load_index(harness_root).await;
    if !index.is_object() {
        index = json!({"clients": []});
    }
    let clients = index["clients"].as_array().cloned().unwrap_or_default();
    let slug = client["slug"].as_str().unwrap_or("");
    let mut clients: Vec<Value> = clients
        .into_iter()
        .filter(|entry| entry["slug"].as_str() != Some(slug))
        .collect();
    clients.push(client.clone());
    index["clients"] = Value::Array(clients);
    write_json(&path, &index).await
}

fn index_path(harness_root: &Path) -> PathBuf {
    harness_root.join(".deep_agents").join("dfd_clients.json")
}

async fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| error.to_string())?;
    }
    let raw = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    tokio::fs::write(path, raw)
        .await
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::QuestionBroker;

    use super::*;

    #[tokio::test]
    async fn client_creation_requires_real_approval_and_writes_scaffold() {
        let dir = tempfile::tempdir().unwrap();
        let clients = dir.path().join("clients");
        let ctx = ToolContext::new(dir.path(), "dfd", Arc::new(QuestionBroker::new()));
        let denied = create_client_in(
            &ctx,
            &json!({"business_name": "Kopi Senja", "vertical": "fnb"}),
            Some(clients.clone()),
        )
        .await;
        assert!(denied.is_error);

        let created = create_client_in(
            &ctx,
            &json!({
                "business_name": "Kopi Senja",
                "vertical": "fnb",
                "features": ["online_order", "online_order", "review_qr"],
                "approved_by": "telegram:req-1"
            }),
            Some(clients.clone()),
        )
        .await;
        assert!(!created.is_error, "{}", created.content);
        assert!(created.content.contains("kopi-senja"));
        let client_dir = clients.join("kopi-senja");
        assert!(client_dir.join("client.json").is_file());
        assert!(client_dir.join("progress.md").is_file());
        assert!(client_dir.join("mem").join("README.md").is_file());
        let index: Value = serde_json::from_str(
            &tokio::fs::read_to_string(dir.path().join(".deep_agents").join("dfd_clients.json"))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(index["clients"][0]["slug"], "kopi-senja");
    }

    #[test]
    fn known_endpoint_lookup_and_slugify() {
        assert!(ENDPOINTS.iter().any(|(name, _, _)| *name == "lead_list"));
        assert_eq!(slugify("  Kopi Senja!! "), "kopi-senja");
    }
}
