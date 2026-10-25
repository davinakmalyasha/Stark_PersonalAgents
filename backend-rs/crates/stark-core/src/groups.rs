use serde_json::{Value, json};
use stark_store::GroupStore;
use stark_tools::ToolOutcome;

use crate::RunContext;

fn store() -> GroupStore {
    GroupStore::default_root()
}

pub(crate) async fn create_group(ctx: &RunContext, args: &Value) -> ToolOutcome {
    match store().create(args, &ctx.workspace_id).await {
        Ok(detail) => ToolOutcome::ok(format!(
            "[OK] Group project '{}' created at {}.\nUse group_board(project_id=\"{}\") to follow progress.",
            detail["id"].as_str().unwrap_or(""),
            detail["path"].as_str().unwrap_or(""),
            detail["id"].as_str().unwrap_or("")
        )),
        Err(error) => ToolOutcome::err(format!("[ERR] create_group failed: {error}")),
    }
}

pub(crate) async fn group_board(_ctx: &RunContext, args: &Value) -> ToolOutcome {
    let Some(project_id) = args.get("project_id").and_then(Value::as_str) else {
        return ToolOutcome::err("[ERR] group_board requires project_id");
    };
    match store().detail(project_id, 60).await {
        Ok(detail) => ToolOutcome::ok(format_board(&detail)),
        Err(error) => ToolOutcome::err(format!("[ERR] group_board failed: {error}")),
    }
}

pub(crate) async fn group_item_detail(_ctx: &RunContext, args: &Value) -> ToolOutcome {
    let Some(project_id) = args.get("project_id").and_then(Value::as_str) else {
        return ToolOutcome::err("[ERR] group_item_detail requires project_id");
    };
    let Some(item_id) = args.get("item_id").and_then(Value::as_str) else {
        return ToolOutcome::err("[ERR] group_item_detail requires item_id");
    };
    match store().item(project_id, item_id).await {
        Ok(item) => ToolOutcome::ok(
            serde_json::to_string_pretty(&item).unwrap_or_else(|_| "{}".to_string()),
        ),
        Err(error) => ToolOutcome::err(format!("[ERR] group_item_detail failed: {error}")),
    }
}

pub(crate) async fn group_events(_ctx: &RunContext, args: &Value) -> ToolOutcome {
    let Some(project_id) = args.get("project_id").and_then(Value::as_str) else {
        return ToolOutcome::err("[ERR] group_events requires project_id");
    };
    let since = args.get("since").and_then(Value::as_i64).unwrap_or(0);
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(50)
        .clamp(1, 200) as usize;
    match store().events(project_id, since, limit).await {
        Ok(events) => {
            if events.is_empty() {
                return ToolOutcome::ok("[OK] No events.");
            }
            let lines = events
                .iter()
                .map(|event| {
                    format!(
                        "#{} {} {} {}",
                        event["seq"].as_i64().unwrap_or(0),
                        event["kind"].as_str().unwrap_or(""),
                        event["actor"].as_str().unwrap_or(""),
                        event["item_id"].as_str().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>();
            ToolOutcome::ok(format!("[GROUP EVENTS]\n{}", lines.join("\n")))
        }
        Err(error) => ToolOutcome::err(format!("[ERR] group_events failed: {error}")),
    }
}

pub(crate) async fn group_update(_ctx: &RunContext, args: &Value) -> ToolOutcome {
    let Some(project_id) = args.get("project_id").and_then(Value::as_str) else {
        return ToolOutcome::err("[ERR] group_update requires project_id");
    };
    let Some(op) = args.get("op").and_then(Value::as_str) else {
        return ToolOutcome::err("[ERR] group_update requires op");
    };
    let params = args.get("params").cloned().unwrap_or_else(|| json!({}));
    match store().update(project_id, op, &params).await {
        Ok(result) => ToolOutcome::ok(format!(
            "[OK] {}",
            result["message"].as_str().unwrap_or("updated")
        )),
        Err(error) => ToolOutcome::err(format!("[ERR] group_update failed: {error}")),
    }
}

fn format_board(detail: &Value) -> String {
    let project = &detail["project"];
    let mut lines = vec![format!(
        "[GROUP] {} — {}",
        detail["id"].as_str().unwrap_or(""),
        project["status"].as_str().unwrap_or("unknown")
    )];
    lines.push(format!("goal: {}", project["goal"].as_str().unwrap_or("")));
    lines.push("milestones:".to_string());
    for milestone in detail["milestones"].as_array().into_iter().flatten() {
        lines.push(format!(
            "- [{}] {} ({})",
            milestone["status"].as_str().unwrap_or(""),
            milestone["title"].as_str().unwrap_or(""),
            milestone["id"].as_str().unwrap_or("")
        ));
    }
    lines.push("items:".to_string());
    for item in detail["items"].as_array().into_iter().flatten() {
        lines.push(format!(
            "- [{}] {} ({})",
            item["status"].as_str().unwrap_or(""),
            item["title"].as_str().unwrap_or(""),
            item["id"].as_str().unwrap_or("")
        ));
    }
    lines.join("\n")
}
