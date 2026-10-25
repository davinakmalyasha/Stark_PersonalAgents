use serde_json::Value;
use stark_store::{ScheduleRecord, ScheduleStore, parse_schedule_time};
use stark_tools::ToolOutcome;

use crate::RunContext;

pub(crate) async fn schedule(ctx: &RunContext, args: &Value) -> ToolOutcome {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("create")
        .trim()
        .to_lowercase();
    let store = match ScheduleStore::open(ctx.store.harness_root()).await {
        Ok(store) => store,
        Err(error) => return ToolOutcome::err(format!("Error: {error}")),
    };
    match action.as_str() {
        "create" => {
            let name = args
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Scheduled wake")
                .trim();
            let at = args.get("at").and_then(Value::as_str).unwrap_or("");
            let Some(at) = parse_schedule_time(at) else {
                return ToolOutcome::err("Error: at must be an ISO timestamp or HH:MM");
            };
            let prompt = args
                .get("prompt")
                .and_then(Value::as_str)
                .unwrap_or(name)
                .to_string();
            let recurrence = args
                .get("recurrence")
                .and_then(Value::as_str)
                .unwrap_or("none")
                .trim()
                .to_lowercase();
            let record = ScheduleRecord {
                id: String::new(),
                chat_id: ctx.chat_id.clone(),
                workspace_id: ctx.workspace_id.clone(),
                workspace_path: ctx.workspace.to_string_lossy().to_string(),
                scope: ctx.scope.clone(),
                role: ctx.role.clone(),
                model: ctx.model.clone(),
                temperature: ctx.temperature,
                name: name.to_string(),
                prompt,
                next_run_at: at.to_rfc3339(),
                recurrence,
                status: "scheduled".to_string(),
            };
            match store.create(record).await {
                Ok(record) => ToolOutcome::ok(format!(
                    "[OK] Scheduled '{}' at {} ({}) id={}",
                    record.name, record.next_run_at, record.recurrence, record.id
                )),
                Err(error) => ToolOutcome::err(format!("Error: {error}")),
            }
        }
        "list" => match store.list(&ctx.chat_id).await {
            Ok(records) if records.is_empty() => ToolOutcome::ok("No schedules."),
            Ok(records) => {
                let lines = records
                    .iter()
                    .map(|record| {
                        format!(
                            "- {} · {} · {} · {}",
                            record.id, record.name, record.next_run_at, record.recurrence
                        )
                    })
                    .collect::<Vec<_>>();
                ToolOutcome::ok(format!("[SCHEDULES]\n{}", lines.join("\n")))
            }
            Err(error) => ToolOutcome::err(format!("Error: {error}")),
        },
        "cancel" => {
            let Some(schedule_id) = args.get("schedule_id").and_then(Value::as_str) else {
                return ToolOutcome::err("Error: schedule_id is required to cancel.");
            };
            match store.cancel(schedule_id).await {
                Ok(true) => ToolOutcome::ok(format!("[OK] Cancelled {schedule_id}.")),
                Ok(false) => ToolOutcome::err(format!("Error: unknown schedule '{schedule_id}'.")),
                Err(error) => ToolOutcome::err(format!("Error: {error}")),
            }
        }
        other => ToolOutcome::err(format!(
            "Error: unknown action '{other}' (create|list|cancel)."
        )),
    }
}
