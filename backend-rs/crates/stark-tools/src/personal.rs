//! Personal OS: one local-first record for money, todos, food and training.
//!
//! Data lives in `<harness>/.deep_agents/personal_os.json` as a single JSON
//! document with atomic writes. Agents call the `personal_os` tool; they never
//! edit the file directly, so amounts stay integer cents and dates stay ISO.
//!
//! Reads are safe for read-only scopes (House). Writes are gated at dispatch
//! by `read_only_denial` and by plan mode.

use std::path::Path;
use std::sync::OnceLock;

use chrono::{Local, NaiveDate};
use serde_json::{Map, Value, json};
use tokio::sync::Mutex;

use crate::{ToolContext, ToolOutcome};

const STORE_FILE: &str = "personal_os.json";
const DEFAULT_LIST: usize = 20;
const MAX_LIST: usize = 200;

static PERSONAL_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn lock() -> &'static Mutex<()> {
    PERSONAL_LOCK.get_or_init(|| Mutex::new(()))
}

pub async fn personal_os(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if action.is_empty() {
        return ToolOutcome::err(
            "Error: personal_os requires action (overview, money_add, money_list, money_summary, money_budget_set, todo_add, todo_list, todo_done, todo_remove, food_log, food_summary, training_log, training_summary).",
        );
    }
    let _guard = lock().lock().await;
    let mut state = match load_state(&ctx.harness_root).await {
        Ok(state) => state,
        Err(error) => return ToolOutcome::err(error),
    };
    let result = match action.as_str() {
        "overview" => Ok(overview(&state)),
        "money_add" => money_add(&mut state, args),
        "money_list" => money_list(&state, args),
        "money_summary" => money_summary(&state, args),
        "money_budget_set" => money_budget_set(&mut state, args),
        "todo_add" => todo_add(&mut state, args),
        "todo_list" => todo_list(&state, args),
        "todo_done" => todo_done(&mut state, args),
        "todo_remove" => todo_remove(&mut state, args),
        "food_log" => food_log(&mut state, args),
        "food_summary" => food_summary(&state, args),
        "training_log" => training_log(&mut state, args),
        "training_summary" => training_summary(&state, args),
        other => Err(format!(
            "Error: unknown personal_os action '{other}'. Valid: overview, money_add, money_list, money_summary, money_budget_set, todo_add, todo_list, todo_done, todo_remove, food_log, food_summary, training_log, training_summary."
        )),
    };
    match result {
        Ok(output) => {
            if action_is_write(&action)
                && let Err(error) = save_state(&ctx.harness_root, &state).await
            {
                return ToolOutcome::err(error);
            }
            ToolOutcome::ok(output)
        }
        Err(error) => ToolOutcome::err(error),
    }
}

fn action_is_write(action: &str) -> bool {
    !matches!(
        action,
        "overview"
            | "money_list"
            | "money_summary"
            | "todo_list"
            | "food_summary"
            | "training_summary"
    )
}

fn overview(state: &Value) -> String {
    let today = today();
    let month = current_month();
    let (spent, income, tx_count) = money_totals(state, &month);
    let budgets = month_budgets(state, &month);
    let budget_total: i64 = budgets.values().sum();
    let open_todos = state["todos"]
        .as_array()
        .map(|todos| {
            todos
                .iter()
                .filter(|todo| !todo["done"].as_bool().unwrap_or(false))
                .count()
        })
        .unwrap_or(0);
    let overdue = state["todos"]
        .as_array()
        .map(|todos| {
            todos
                .iter()
                .filter(|todo| {
                    !todo["done"].as_bool().unwrap_or(false)
                        && todo["due"]
                            .as_str()
                            .is_some_and(|due| !due.is_empty() && due < today.as_str())
                })
                .count()
        })
        .unwrap_or(0);
    let calories: i64 = state["food"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter(|entry| entry["date"].as_str() == Some(today.as_str()))
                .filter_map(|entry| entry["calories"].as_i64())
                .sum()
        })
        .unwrap_or(0);
    let training_minutes: i64 = state["training"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter(|entry| {
                    entry["date"]
                        .as_str()
                        .is_some_and(|date| date >= days_ago(7).as_str())
                })
                .filter_map(|entry| entry["minutes"].as_i64())
                .sum()
        })
        .unwrap_or(0);
    let budget_line = if budget_total > 0 {
        format!(
            " | budget {:.2} | remaining {:.2}",
            cents(budget_total),
            cents(budget_total.saturating_sub(spent))
        )
    } else {
        String::new()
    };
    format!(
        "Personal OS | {today}\n\
         Money {month}: spent {:.2} | income {:.2} ({tx_count} transactions){budget_line}\n\
         Todos: {open_todos} open, {overdue} overdue\n\
         Food today: {calories} cal\n\
         Training 7d: {training_minutes} min",
        cents(spent),
        cents(income)
    )
}

fn money_add(state: &mut Value, args: &Value) -> Result<String, String> {
    let amount = parse_amount(args)?;
    let kind = optional_str(args, "kind")
        .unwrap_or("expense")
        .to_ascii_lowercase();
    if !matches!(kind.as_str(), "expense" | "income") {
        return Err("Error: kind must be 'expense' or 'income'".to_string());
    }
    let date = parse_date(args)?;
    let category = label(args, "category", "uncategorized", 40)?;
    let note = optional_str(args, "note").unwrap_or("").trim().to_string();
    if note.chars().count() > 200 {
        return Err("Error: note is limited to 200 characters".to_string());
    }
    let id = next_id(state, "tx");
    let note_suffix = if note.is_empty() {
        String::new()
    } else {
        format!(" | {note}")
    };
    state["money"]["transactions"]
        .as_array_mut()
        .expect("transactions array")
        .push(json!({
            "id": id,
            "date": date,
            "amount_cents": amount,
            "kind": kind,
            "category": category,
            "note": note,
        }));
    Ok(format!(
        "[OK] Recorded {kind} {:.2} {category} ({date}) {id}{note_suffix}",
        cents(amount)
    ))
}

fn money_list(state: &Value, args: &Value) -> Result<String, String> {
    let month = optional_month(args)?.unwrap_or_else(current_month);
    let category = optional_str(args, "category")
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    let limit = list_limit(args);
    let mut rows = Vec::new();
    for tx in state["money"]["transactions"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let date = tx["date"].as_str().unwrap_or("");
        if !date.starts_with(&month) {
            continue;
        }
        if let Some(category) = &category
            && tx["category"].as_str() != Some(category.as_str())
        {
            continue;
        }
        let amount = tx["amount_cents"].as_i64().unwrap_or(0);
        let kind = tx["kind"].as_str().unwrap_or("expense");
        let sign = if kind == "income" { "+" } else { "-" };
        let mut line = format!(
            "{date}  {sign}{:.2}  {}",
            cents(amount.abs()),
            tx["category"].as_str().unwrap_or("uncategorized")
        );
        if let Some(note) = tx["note"].as_str().filter(|note| !note.is_empty()) {
            line.push_str(&format!("  ({note})"));
        }
        if let Some(id) = tx["id"].as_str() {
            line.push_str(&format!("  [{id}]"));
        }
        rows.push(line);
    }
    if rows.is_empty() {
        return Ok(format!("[OK] No transactions for {month}"));
    }
    rows.reverse();
    rows.truncate(limit);
    Ok(format!(
        "Money {month} — {} transaction(s) (newest first)\n{}",
        rows.len(),
        rows.join("\n")
    ))
}

fn money_summary(state: &Value, args: &Value) -> Result<String, String> {
    let month = optional_month(args)?.unwrap_or_else(current_month);
    let (spent, income, tx_count) = money_totals(state, &month);
    let mut by_category: Map<String, Value> = Map::new();
    for tx in state["money"]["transactions"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let date = tx["date"].as_str().unwrap_or("");
        if !date.starts_with(&month) || tx["kind"].as_str() == Some("income") {
            continue;
        }
        let category = tx["category"].as_str().unwrap_or("uncategorized");
        let current = by_category
            .get(category)
            .and_then(Value::as_i64)
            .unwrap_or(0);
        by_category.insert(
            category.to_string(),
            json!(current + tx["amount_cents"].as_i64().unwrap_or(0)),
        );
    }
    let budgets = month_budgets(state, &month);
    let mut lines = vec![format!(
        "Money {month}\nexpenses {:.2} ({tx_count} tx) | income {:.2} | net {:.2}",
        cents(spent),
        cents(income),
        cents(income - spent)
    )];
    if budgets.is_empty() {
        lines.push("no budgets set".to_string());
    } else {
        let total: i64 = budgets.values().sum();
        lines.push(format!(
            "budget {:.2} | remaining {:.2}",
            cents(total),
            cents(total.saturating_sub(spent))
        ));
    }
    let mut categories: Vec<(String, i64)> = by_category
        .iter()
        .filter_map(|(name, value)| Some((name.clone(), value.as_i64()?)))
        .collect();
    categories.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    for (name, amount) in categories {
        match budgets.get(&name) {
            Some(budget) => lines.push(format!(
                "  {name:<14} {:>10.2} / {:.2}  (remaining {:.2})",
                cents(amount),
                cents(*budget),
                cents(budget.saturating_sub(amount))
            )),
            None => lines.push(format!("  {name:<14} {:>10.2}", cents(amount))),
        }
    }
    for (name, budget) in &budgets {
        if !by_category.contains_key(name) {
            lines.push(format!(
                "  {name:<14} {:>10.2} / {:.2}  (unspent)",
                cents(0),
                cents(*budget)
            ));
        }
    }
    Ok(lines.join("\n"))
}

fn money_budget_set(state: &mut Value, args: &Value) -> Result<String, String> {
    let amount = parse_amount(args)?;
    let month = optional_month(args)?.unwrap_or_else(current_month);
    let category = label(args, "category", "", 40)?;
    if category.is_empty() {
        return Err("Error: money_budget_set needs category".to_string());
    }
    if !state["money"]["budgets"].is_object() {
        state["money"]["budgets"] = json!({});
    }
    if !state["money"]["budgets"][&month].is_object() {
        state["money"]["budgets"][&month] = json!({});
    }
    state["money"]["budgets"][&month][&category] = json!(amount);
    Ok(format!(
        "[OK] Budget {month} {category} = {:.2}",
        cents(amount)
    ))
}

fn todo_add(state: &mut Value, args: &Value) -> Result<String, String> {
    let text = optional_str(args, "text").unwrap_or("").trim().to_string();
    if text.is_empty() {
        return Err("Error: todo_add needs text".to_string());
    }
    if text.chars().count() > 500 {
        return Err("Error: todo text is limited to 500 characters".to_string());
    }
    let due = optional_date(args)?.unwrap_or_default();
    let priority = optional_str(args, "priority")
        .unwrap_or("normal")
        .to_ascii_lowercase();
    if !matches!(priority.as_str(), "low" | "normal" | "high") {
        return Err("Error: priority must be low, normal, or high".to_string());
    }
    let id = next_id(state, "todo");
    state["todos"]
        .as_array_mut()
        .expect("todos array")
        .push(json!({
            "id": id,
            "text": text,
            "due": due,
            "priority": priority,
            "done": false,
            "created": now_rfc3339(),
            "done_at": "",
        }));
    let due_suffix = if due.is_empty() {
        String::new()
    } else {
        format!(" (due {due})")
    };
    Ok(format!("[OK] Added {id} [{priority}] {text}{due_suffix}"))
}

fn todo_list(state: &Value, args: &Value) -> Result<String, String> {
    let status = optional_str(args, "status")
        .unwrap_or("open")
        .to_ascii_lowercase();
    if !matches!(status.as_str(), "open" | "done" | "all") {
        return Err("Error: status must be open, done, or all".to_string());
    }
    let limit = list_limit(args);
    let mut rows = Vec::new();
    for todo in state["todos"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let done = todo["done"].as_bool().unwrap_or(false);
        let keep = match status.as_str() {
            "open" => !done,
            "done" => done,
            _ => true,
        };
        if !keep {
            continue;
        }
        let mut line = format!(
            "{} [{}] {}",
            todo["id"].as_str().unwrap_or("?"),
            todo["priority"].as_str().unwrap_or("normal"),
            todo["text"].as_str().unwrap_or("")
        );
        if let Some(due) = todo["due"].as_str().filter(|due| !due.is_empty()) {
            line.push_str(&format!(" (due {due})"));
        }
        if done && let Some(done_at) = todo["done_at"].as_str().filter(|value| !value.is_empty()) {
            line.push_str(&format!(" done {done_at}"));
        }
        rows.push(line);
    }
    if rows.is_empty() {
        return Ok(format!("[OK] No {status} todos"));
    }
    rows.truncate(limit);
    Ok(format!(
        "Todos ({status}) — {} shown\n{}",
        rows.len(),
        rows.join("\n")
    ))
}

fn todo_done(state: &mut Value, args: &Value) -> Result<String, String> {
    let id = todo_id(args)?;
    let todos = state["todos"]
        .as_array_mut()
        .expect("todos array")
        .iter_mut()
        .find(|todo| todo["id"].as_str() == Some(id.as_str()))
        .ok_or_else(|| format!("Error: unknown todo '{id}'"))?;
    if todos["done"].as_bool().unwrap_or(false) {
        return Ok(format!("[OK] {id} was already done"));
    }
    todos["done"] = json!(true);
    todos["done_at"] = json!(now_rfc3339());
    Ok(format!(
        "[OK] Completed {id} {}",
        todos["text"].as_str().unwrap_or("")
    ))
}

fn todo_remove(state: &mut Value, args: &Value) -> Result<String, String> {
    let id = todo_id(args)?;
    let todos = state["todos"].as_array_mut().expect("todos array");
    let index = todos
        .iter()
        .position(|todo| todo["id"].as_str() == Some(id.as_str()))
        .ok_or_else(|| format!("Error: unknown todo '{id}'"))?;
    let removed = todos.remove(index);
    Ok(format!(
        "[OK] Removed {id} {}",
        removed["text"].as_str().unwrap_or("")
    ))
}

fn food_log(state: &mut Value, args: &Value) -> Result<String, String> {
    let calories = args
        .get("calories")
        .and_then(Value::as_i64)
        .ok_or_else(|| "Error: food_log needs calories (integer)".to_string())?;
    if !(0..=20_000).contains(&calories) {
        return Err("Error: calories must be between 0 and 20000".to_string());
    }
    let date = parse_date(args)?;
    let name = label(args, "name", "meal", 80)?;
    let id = next_id(state, "food");
    state["food"]
        .as_array_mut()
        .expect("food array")
        .push(json!({"id": id, "date": date, "name": name, "calories": calories}));
    Ok(format!("[OK] Logged {calories} cal {name} ({date}) {id}"))
}

fn food_summary(state: &Value, args: &Value) -> Result<String, String> {
    let date = optional_date(args)?.unwrap_or_else(today);
    let entries: Vec<&Value> = state["food"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|entry| entry["date"].as_str() == Some(date.as_str()))
        .collect();
    let total: i64 = entries
        .iter()
        .filter_map(|entry| entry["calories"].as_i64())
        .sum();
    if entries.is_empty() {
        return Ok(format!("[OK] No food logged for {date}"));
    }
    let mut lines = vec![format!(
        "Food {date}: {total} cal across {} entr{}",
        entries.len(),
        if entries.len() == 1 { "y" } else { "ies" }
    )];
    for entry in entries {
        lines.push(format!(
            "  {} cal  {}  [{}]",
            entry["calories"].as_i64().unwrap_or(0),
            entry["name"].as_str().unwrap_or("meal"),
            entry["id"].as_str().unwrap_or("?")
        ));
    }
    Ok(lines.join("\n"))
}

fn training_log(state: &mut Value, args: &Value) -> Result<String, String> {
    let activity = label(args, "activity", "", 80)?;
    if activity.is_empty() {
        return Err("Error: training_log needs activity".to_string());
    }
    let minutes = args.get("minutes").and_then(Value::as_i64).unwrap_or(0);
    if !(0..=1_440).contains(&minutes) {
        return Err("Error: minutes must be between 0 and 1440".to_string());
    }
    let date = parse_date(args)?;
    let note = optional_str(args, "note").unwrap_or("").trim().to_string();
    if note.chars().count() > 200 {
        return Err("Error: note is limited to 200 characters".to_string());
    }
    let id = next_id(state, "training");
    state["training"]
        .as_array_mut()
        .expect("training array")
        .push(json!({
            "id": id,
            "date": date,
            "activity": activity,
            "minutes": minutes,
            "note": note,
        }));
    Ok(format!(
        "[OK] Logged {minutes} min {activity} ({date}) {id}"
    ))
}

fn training_summary(state: &Value, args: &Value) -> Result<String, String> {
    let days = args
        .get("days")
        .and_then(Value::as_i64)
        .unwrap_or(7)
        .clamp(1, 365);
    let cutoff = days_ago(days);
    let entries: Vec<&Value> = state["training"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|entry| {
            entry["date"]
                .as_str()
                .is_some_and(|date| date >= cutoff.as_str())
        })
        .collect();
    let total: i64 = entries
        .iter()
        .filter_map(|entry| entry["minutes"].as_i64())
        .sum();
    if entries.is_empty() {
        return Ok(format!("[OK] No training logged in the last {days} days"));
    }
    let mut lines = vec![format!(
        "Training last {days} days: {total} min across {} session(s)",
        entries.len()
    )];
    for entry in entries {
        let mut line = format!(
            "  {}  {:>4} min  {}",
            entry["date"].as_str().unwrap_or(""),
            entry["minutes"].as_i64().unwrap_or(0),
            entry["activity"].as_str().unwrap_or("")
        );
        if let Some(note) = entry["note"].as_str().filter(|note| !note.is_empty()) {
            line.push_str(&format!("  ({note})"));
        }
        lines.push(line);
    }
    Ok(lines.join("\n"))
}

fn money_totals(state: &Value, month: &str) -> (i64, i64, usize) {
    let mut spent = 0;
    let mut income = 0;
    let mut count = 0;
    for tx in state["money"]["transactions"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        if !tx["date"]
            .as_str()
            .is_some_and(|date| date.starts_with(month))
        {
            continue;
        }
        count += 1;
        let amount = tx["amount_cents"].as_i64().unwrap_or(0);
        if tx["kind"].as_str() == Some("income") {
            income += amount;
        } else {
            spent += amount;
        }
    }
    (spent, income, count)
}

fn month_budgets(state: &Value, month: &str) -> std::collections::BTreeMap<String, i64> {
    let mut budgets = std::collections::BTreeMap::new();
    if let Some(values) = state["money"]["budgets"][month].as_object() {
        for (category, value) in values {
            if let Some(amount) = value.as_i64() {
                budgets.insert(category.clone(), amount);
            }
        }
    }
    budgets
}

fn parse_amount(args: &Value) -> Result<i64, String> {
    let amount = args
        .get("amount")
        .and_then(Value::as_f64)
        .ok_or_else(|| "Error: amount is required (a number, e.g. 12.50)".to_string())?;
    if !amount.is_finite() || amount <= 0.0 || amount > 1_000_000_000.0 {
        return Err("Error: amount must be greater than 0 and at most 1000000000".to_string());
    }
    let cents = (amount * 100.0).round() as i64;
    if cents <= 0 {
        return Err("Error: amount is too small".to_string());
    }
    Ok(cents)
}

fn parse_date(args: &Value) -> Result<String, String> {
    optional_date(args).map(|date| date.unwrap_or_else(today))
}

fn optional_date(args: &Value) -> Result<Option<String>, String> {
    let Some(raw) = optional_str(args, "date") else {
        return Ok(None);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .map_err(|_| format!("Error: date '{raw}' must be YYYY-MM-DD"))?;
    Ok(Some(raw.to_string()))
}

fn optional_month(args: &Value) -> Result<Option<String>, String> {
    let Some(raw) = optional_str(args, "month") else {
        return Ok(None);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    NaiveDate::parse_from_str(&format!("{raw}-01"), "%Y-%m-%d")
        .map_err(|_| format!("Error: month '{raw}' must be YYYY-MM"))?;
    Ok(Some(raw.to_string()))
}

fn todo_id(args: &Value) -> Result<String, String> {
    let raw = optional_str(args, "id").unwrap_or("").trim();
    if raw.is_empty() {
        return Err("Error: id is required (for example todo-1)".to_string());
    }
    if raw.chars().all(|ch| ch.is_ascii_digit()) {
        return Ok(format!("todo-{raw}"));
    }
    Ok(raw.to_string())
}

fn label(args: &Value, key: &str, fallback: &str, max: usize) -> Result<String, String> {
    let value = optional_str(args, key)
        .unwrap_or(fallback)
        .trim()
        .to_string();
    if value.chars().count() > max {
        return Err(format!("Error: {key} is limited to {max} characters"));
    }
    Ok(value)
}

fn optional_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn list_limit(args: &Value) -> usize {
    args.get("limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_LIST)
        .clamp(1, MAX_LIST)
}

fn next_id(state: &mut Value, kind: &str) -> String {
    let counter = state["next_id"][kind].as_u64().unwrap_or(1).max(1);
    state["next_id"][kind] = json!(counter + 1);
    format!("{kind}-{counter}")
}

fn default_state() -> Value {
    json!({
        "version": 1,
        "updated_at": "",
        "next_id": {"tx": 1, "todo": 1, "food": 1, "training": 1},
        "money": {"transactions": [], "budgets": {}},
        "todos": [],
        "food": [],
        "training": [],
    })
}

fn normalize(mut state: Value) -> Value {
    if !state.is_object() {
        return default_state();
    }
    if !state["money"].is_object() {
        state["money"] = json!({"transactions": [], "budgets": {}});
    }
    if !state["money"]["transactions"].is_array() {
        state["money"]["transactions"] = json!([]);
    }
    if !state["money"]["budgets"].is_object() {
        state["money"]["budgets"] = json!({});
    }
    for key in ["todos", "food", "training"] {
        if !state[key].is_array() {
            state[key] = json!([]);
        }
    }
    if !state["next_id"].is_object() {
        state["next_id"] = json!({});
    }
    for key in ["tx", "todo", "food", "training"] {
        if !state["next_id"][key].is_u64() {
            let existing = match key {
                "tx" => max_suffix(&state["money"]["transactions"], "tx"),
                other => max_suffix(&state[other], other),
            };
            state["next_id"][key] = json!(existing + 1);
        }
    }
    if !state["version"].is_u64() {
        state["version"] = json!(1);
    }
    state
}

fn max_suffix(items: &Value, prefix: &str) -> u64 {
    items
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item["id"].as_str())
        .filter_map(|id| id.strip_prefix(prefix)?.strip_prefix('-'))
        .filter_map(|number| number.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
}

fn store_path(root: &Path) -> std::path::PathBuf {
    root.join(".deep_agents").join(STORE_FILE)
}

async fn load_state(root: &Path) -> Result<Value, String> {
    let path = store_path(root);
    match tokio::fs::read_to_string(&path).await {
        Ok(raw) => serde_json::from_str(&raw).map(normalize).map_err(|error| {
            format!(
                "Error: personal_os store {} is not valid JSON ({error}). Fix or restore the file before writing; it was not reset.",
                path.display()
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(default_state()),
        Err(error) => Err(format!(
            "Error: cannot read personal_os store {}: {error}",
            path.display()
        )),
    }
}

async fn save_state(root: &Path, state: &Value) -> Result<(), String> {
    let path = store_path(root);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("Error: cannot create {}: {error}", parent.display()))?;
    }
    let mut state = state.clone();
    state["updated_at"] = json!(now_rfc3339());
    let raw = serde_json::to_string_pretty(&state)
        .map_err(|error| format!("Error: cannot serialize personal_os store: {error}"))?;
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, raw)
        .await
        .map_err(|error| format!("Error: cannot write {}: {error}", tmp.display()))?;
    tokio::fs::rename(&tmp, &path)
        .await
        .map_err(|error| format!("Error: cannot replace {}: {error}", path.display()))
}

fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

fn current_month() -> String {
    Local::now().format("%Y-%m").to_string()
}

fn days_ago(days: i64) -> String {
    (Local::now() - chrono::Duration::days(days))
        .format("%Y-%m-%d")
        .to_string()
}

fn now_rfc3339() -> String {
    Local::now().to_rfc3339()
}

fn cents(value: i64) -> f64 {
    value as f64 / 100.0
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::QuestionBroker;

    fn context(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir, "personal-test", Arc::new(QuestionBroker::new()))
    }

    #[tokio::test]
    async fn money_and_budget_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let added = personal_os(
            &ctx,
            &json!({"action": "money_add", "amount": 12.5, "category": "food", "note": "lunch", "date": "2026-11-08"}),
        )
        .await;
        assert!(!added.is_error, "{}", added.content);
        assert!(added.content.contains("12.50"), "{}", added.content);
        let income = personal_os(
            &ctx,
            &json!({"action": "money_add", "amount": 100, "kind": "income", "category": "salary", "date": "2026-11-08"}),
        )
        .await;
        assert!(!income.is_error, "{}", income.content);
        let budget = personal_os(
            &ctx,
            &json!({"action": "money_budget_set", "amount": 200, "category": "food", "month": "2026-11"}),
        )
        .await;
        assert!(!budget.is_error, "{}", budget.content);
        let summary = personal_os(
            &ctx,
            &json!({"action": "money_summary", "month": "2026-11"}),
        )
        .await;
        assert!(!summary.is_error, "{}", summary.content);
        assert!(
            summary.content.contains("expenses 12.50"),
            "{}",
            summary.content
        );
        assert!(
            summary.content.contains("income 100.00"),
            "{}",
            summary.content
        );
        assert!(
            summary.content.contains("remaining 187.50"),
            "{}",
            summary.content
        );
        let listed = personal_os(
            &ctx,
            &json!({"action": "money_list", "month": "2026-11", "category": "food"}),
        )
        .await;
        assert!(!listed.is_error, "{}", listed.content);
        assert!(listed.content.contains("lunch"), "{}", listed.content);
        let other = personal_os(
            &ctx,
            &json!({"action": "money_list", "month": "2026-11", "category": "salary"}),
        )
        .await;
        assert!(!other.is_error, "{}", other.content);
        assert!(!other.content.contains("lunch"), "{}", other.content);
    }

    #[tokio::test]
    async fn todo_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let added = personal_os(
            &ctx,
            &json!({"action": "todo_add", "text": "Buy milk", "due": "2026-11-09", "priority": "high"}),
        )
        .await;
        assert!(!added.is_error, "{}", added.content);
        assert!(added.content.contains("todo-1"), "{}", added.content);
        let listed = personal_os(&ctx, &json!({"action": "todo_list"})).await;
        assert!(listed.content.contains("Buy milk"), "{}", listed.content);
        let done = personal_os(&ctx, &json!({"action": "todo_done", "id": "1"})).await;
        assert!(!done.is_error, "{}", done.content);
        let open = personal_os(&ctx, &json!({"action": "todo_list"})).await;
        assert!(!open.content.contains("Buy milk"), "{}", open.content);
        let all = personal_os(&ctx, &json!({"action": "todo_list", "status": "all"})).await;
        assert!(all.content.contains("Buy milk"), "{}", all.content);
        let removed = personal_os(&ctx, &json!({"action": "todo_remove", "id": "todo-1"})).await;
        assert!(!removed.is_error, "{}", removed.content);
        let empty = personal_os(&ctx, &json!({"action": "todo_list", "status": "all"})).await;
        assert!(empty.content.contains("No all todos"), "{}", empty.content);
    }

    #[tokio::test]
    async fn food_and_training_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let food = personal_os(
            &ctx,
            &json!({"action": "food_log", "name": "Oatmeal", "calories": 450, "date": "2026-11-08"}),
        )
        .await;
        assert!(!food.is_error, "{}", food.content);
        let summary = personal_os(
            &ctx,
            &json!({"action": "food_summary", "date": "2026-11-08"}),
        )
        .await;
        assert!(summary.content.contains("450"), "{}", summary.content);
        assert!(summary.content.contains("Oatmeal"), "{}", summary.content);
        let training = personal_os(
            &ctx,
            &json!({"action": "training_log", "activity": "Run", "minutes": 30, "date": "2026-11-08"}),
        )
        .await;
        assert!(!training.is_error, "{}", training.content);
        let summary = personal_os(&ctx, &json!({"action": "training_summary", "days": 30})).await;
        assert!(summary.content.contains("30 min"), "{}", summary.content);
        assert!(summary.content.contains("Run"), "{}", summary.content);
    }

    #[tokio::test]
    async fn overview_reads_without_creating_a_store() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let out = personal_os(&ctx, &json!({"action": "overview"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("Personal OS"), "{}", out.content);
        assert!(!store_path(dir.path()).exists());
    }

    #[tokio::test]
    async fn rejects_bad_input_and_keeps_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let bad_amount = personal_os(&ctx, &json!({"action": "money_add", "amount": 0})).await;
        assert!(bad_amount.is_error, "{}", bad_amount.content);
        let bad_date = personal_os(
            &ctx,
            &json!({"action": "money_add", "amount": 1, "date": "11/08/2026"}),
        )
        .await;
        assert!(bad_date.is_error, "{}", bad_date.content);
        assert!(
            bad_date.content.contains("YYYY-MM-DD"),
            "{}",
            bad_date.content
        );
        let unknown = personal_os(&ctx, &json!({"action": "dance"})).await;
        assert!(unknown.is_error, "{}", unknown.content);
        assert!(
            unknown.content.contains("unknown personal_os action"),
            "{}",
            unknown.content
        );
        assert!(!store_path(dir.path()).exists());
    }

    #[tokio::test]
    async fn corrupt_store_is_reported_and_not_reset() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let path = store_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{not json").unwrap();
        let out = personal_os(&ctx, &json!({"action": "overview"})).await;
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("was not reset"), "{}", out.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");
    }
}
