use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use serde_json::{Map, Value, json};
use tokio::sync::Mutex;

use crate::{ToolContext, ToolOutcome};

const DEFAULT_HOURS: i64 = 24;
const MAX_HOURS: i64 = 168;

static HOUSE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn lock() -> &'static Mutex<()> {
    HOUSE_LOCK.get_or_init(|| Mutex::new(()))
}

pub async fn house_get_state(ctx: &ToolContext, _args: &Value) -> ToolOutcome {
    let _guard = lock().lock().await;
    match load_state(&ctx.harness_root).await {
        Ok(state) => ToolOutcome::ok(format_state(&state)),
        Err(error) => ToolOutcome::err(error),
    }
}

pub async fn house_get_device(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let device_id = args
        .get("device_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if device_id.is_empty() {
        return ToolOutcome::err("Error: device_id is required");
    }
    let _guard = lock().lock().await;
    let state = match load_state(&ctx.harness_root).await {
        Ok(state) => state,
        Err(error) => return ToolOutcome::err(error),
    };
    let Some(device) = state["devices"].get(device_id) else {
        return ToolOutcome::err(format!("Error: Unknown device '{device_id}'"));
    };
    let capabilities = capabilities(device["type"].as_str().unwrap_or(""));
    ToolOutcome::ok(format!(
        "{} | online={} | writable={}",
        format_device(device_id, device),
        device
            .get("isOnline")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        if capabilities.is_empty() {
            "read-only".to_string()
        } else {
            capabilities.join(", ")
        }
    ))
}

pub async fn house_set_device(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let device_id = args
        .get("device_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if device_id.is_empty() {
        return ToolOutcome::err("Error: device_id is required");
    }
    let Some(patch) = args.get("state").and_then(Value::as_object) else {
        return ToolOutcome::err(
            "Error: state must be a non-empty object like {\"brightness\": 50}",
        );
    };
    if patch.is_empty() {
        return ToolOutcome::err(
            "Error: state must be a non-empty object like {\"brightness\": 50}",
        );
    }
    let _guard = lock().lock().await;
    let mut state = match load_state(&ctx.harness_root).await {
        Ok(state) => state,
        Err(error) => return ToolOutcome::err(error),
    };
    {
        let Some(device) = state["devices"].get_mut(device_id) else {
            return ToolOutcome::err(format!("Error: Unknown device '{device_id}'"));
        };
        let (valid, error) = validate_patch(device["type"].as_str().unwrap_or(""), patch);
        if valid.is_empty() {
            return ToolOutcome::err(format!("Error: {error}"));
        }
        let state_object = device
            .get_mut("state")
            .and_then(Value::as_object_mut)
            .expect("device state is an object");
        for (key, value) in valid {
            state_object.insert(key, value);
        }
    }
    state["updatedAt"] = json!(Utc::now().to_rfc3339());
    if let Err(error) = save_state(&ctx.harness_root, &state).await {
        return ToolOutcome::err(error);
    }
    let device = &state["devices"][device_id];
    ToolOutcome::ok(format!("[OK] {}", format_device(device_id, device)))
}

pub async fn house_run_scene(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let scene_id = args
        .get("scene_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if scene_id.is_empty() {
        return ToolOutcome::err("Error: scene_id is required");
    }
    let _guard = lock().lock().await;
    let mut state = match load_state(&ctx.harness_root).await {
        Ok(state) => state,
        Err(error) => return ToolOutcome::err(error),
    };
    let Some(scene) = state["scenes"].get(scene_id).cloned() else {
        return ToolOutcome::err(format!("Error: Scene {scene_id} not found"));
    };
    let actions = scene_actions(&scene);
    let mut applied = Vec::new();
    let mut skipped = Vec::new();
    for (device_id, patch) in actions {
        let Some(device) = state["devices"].get_mut(&device_id) else {
            skipped.push(format!("{device_id} (unknown device)"));
            continue;
        };
        let (valid, error) = validate_patch(device["type"].as_str().unwrap_or(""), &patch);
        if valid.is_empty() {
            skipped.push(format!("{device_id} ({error})"));
            continue;
        }
        let state_object = device
            .get_mut("state")
            .and_then(Value::as_object_mut)
            .expect("device state is an object");
        for (key, value) in valid {
            state_object.insert(key, value);
        }
        applied.push(device_id);
    }
    state["updatedAt"] = json!(Utc::now().to_rfc3339());
    if let Err(error) = save_state(&ctx.harness_root, &state).await {
        return ToolOutcome::err(error);
    }
    let name = scene["name"].as_str().unwrap_or(scene_id);
    let mut output = format!(
        "[OK] Scene '{name}' ({scene_id}) applied to {} device(s)",
        applied.len()
    );
    if !skipped.is_empty() {
        output.push_str(&format!(" | skipped: {}", skipped.join(", ")));
    }
    ToolOutcome::ok(output)
}

pub async fn house_get_history(ctx: &ToolContext, args: &Value) -> ToolOutcome {
    let device_id = args
        .get("device_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if device_id.is_empty() {
        return ToolOutcome::err("Error: device_id is required");
    }
    let hours = args
        .get("hours")
        .and_then(Value::as_i64)
        .unwrap_or(DEFAULT_HOURS)
        .clamp(1, MAX_HOURS);
    let _guard = lock().lock().await;
    let state = match load_state(&ctx.harness_root).await {
        Ok(state) => state,
        Err(error) => return ToolOutcome::err(error),
    };
    if !state["devices"].get(device_id).is_some() {
        return ToolOutcome::err(format!("Error: Unknown device '{device_id}'"));
    }
    let cutoff = unix_now() - hours * 3_600;
    let mut series = Vec::new();
    if let Some(points) = state["history"].get(device_id).and_then(Value::as_array) {
        for point in points {
            if point.get("ts").and_then(Value::as_i64).unwrap_or(0) >= cutoff {
                series.push(point.get("kwh").and_then(Value::as_f64).unwrap_or(0.0));
            }
        }
    }
    if series.is_empty() {
        for index in 0..hours {
            series.push(0.12 * ((index % 7) as f64));
        }
    }
    let total: f64 = series.iter().sum();
    ToolOutcome::ok(format!(
        "[OK] {device_id} last {hours}h: total {total:.3} kWh\nseries: {}",
        serde_json::to_string(&series).unwrap_or_else(|_| "[]".to_string())
    ))
}

#[derive(Clone, Debug)]
pub struct HouseStore {
    root: PathBuf,
}

impl HouseStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub async fn state(&self) -> Result<Value, String> {
        let _guard = lock().lock().await;
        load_state(&self.root).await
    }

    pub async fn device(&self, device_id: &str) -> Result<Option<Value>, String> {
        let _guard = lock().lock().await;
        let state = load_state(&self.root).await?;
        let Some(device) = state["devices"].get(device_id) else {
            return Ok(None);
        };
        Ok(Some(with_id(device_id, device)))
    }

    pub async fn set_device(
        &self,
        device_id: &str,
        patch: &Map<String, Value>,
    ) -> Result<Value, String> {
        let _guard = lock().lock().await;
        let mut state = load_state(&self.root).await?;
        {
            let Some(device) = state["devices"].get_mut(device_id) else {
                return Err(format!("Unknown device {device_id}"));
            };
            let (valid, error) = validate_patch(device["type"].as_str().unwrap_or(""), patch);
            if valid.is_empty() {
                return Err(error);
            }
            let state_object = device
                .get_mut("state")
                .and_then(Value::as_object_mut)
                .expect("device state is an object");
            for (key, value) in valid {
                state_object.insert(key, value);
            }
        }
        state["updatedAt"] = json!(Utc::now().to_rfc3339());
        save_state(&self.root, &state).await?;
        Ok(with_id(device_id, &state["devices"][device_id]))
    }

    pub async fn set_room(
        &self,
        room_id: &str,
        patch: &Map<String, Value>,
    ) -> Result<Value, String> {
        let _guard = lock().lock().await;
        let mut state = load_state(&self.root).await?;
        let mut applied = Vec::new();
        let mut skipped = Vec::new();
        let ids: Vec<String> = state["devices"]
            .as_object()
            .map(|devices| devices.keys().cloned().collect())
            .unwrap_or_default();
        for device_id in ids {
            let Some(device) = state["devices"].get_mut(&device_id) else {
                continue;
            };
            if device_room(device) != room_id {
                continue;
            }
            let (valid, error) = validate_patch(device["type"].as_str().unwrap_or(""), patch);
            if valid.is_empty() {
                skipped.push(json!([device_id, error]));
                continue;
            }
            let state_object = device
                .get_mut("state")
                .and_then(Value::as_object_mut)
                .expect("device state is an object");
            for (key, value) in valid {
                state_object.insert(key, value);
            }
            applied.push(device_id);
        }
        if applied.is_empty() && skipped.is_empty() {
            return Err(format!("Room {room_id} not found"));
        }
        state["updatedAt"] = json!(Utc::now().to_rfc3339());
        save_state(&self.root, &state).await?;
        Ok(json!({"applied": applied, "skipped": skipped}))
    }

    pub async fn run_scene(&self, scene_id: &str) -> Result<Value, String> {
        let _guard = lock().lock().await;
        let mut state = load_state(&self.root).await?;
        let Some(scene) = state["scenes"].get(scene_id).cloned() else {
            return Err(format!("Scene {scene_id} not found"));
        };
        let mut applied = Vec::new();
        let mut skipped = Vec::new();
        for (device_id, patch) in scene_actions(&scene) {
            let Some(device) = state["devices"].get_mut(&device_id) else {
                skipped.push(json!([device_id, "unknown device"]));
                continue;
            };
            let (valid, error) = validate_patch(device["type"].as_str().unwrap_or(""), &patch);
            if valid.is_empty() {
                skipped.push(json!([device_id, error]));
                continue;
            }
            let state_object = device
                .get_mut("state")
                .and_then(Value::as_object_mut)
                .expect("device state is an object");
            for (key, value) in valid {
                state_object.insert(key, value);
            }
            applied.push(device_id);
        }
        state["updatedAt"] = json!(Utc::now().to_rfc3339());
        save_state(&self.root, &state).await?;
        Ok(json!({
            "ok": true,
            "scene": with_id(scene_id, &scene),
            "applied": applied,
            "skipped": skipped,
        }))
    }

    pub async fn history(&self, device_id: &str, hours: i64) -> Result<Value, String> {
        let _guard = lock().lock().await;
        let state = load_state(&self.root).await?;
        if state["devices"].get(device_id).is_none() {
            return Err(format!("Unknown device {device_id}"));
        }
        let hours = hours.clamp(1, MAX_HOURS);
        let cutoff = unix_now() - hours * 3_600;
        let mut points = Vec::new();
        if let Some(items) = state["history"].get(device_id).and_then(Value::as_array) {
            for point in items {
                if point.get("ts").and_then(Value::as_i64).unwrap_or(0) >= cutoff {
                    points.push(point.clone());
                }
            }
        }
        if points.is_empty() {
            for index in 0..hours {
                points.push(json!({
                    "ts": unix_now() - index * 3_600,
                    "kwh": 0.12 * ((index % 7) as f64),
                }));
            }
        }
        let series: Vec<f64> = points
            .iter()
            .map(|point| point.get("kwh").and_then(Value::as_f64).unwrap_or(0.0))
            .collect();
        Ok(json!({
            "device_id": device_id,
            "hours": hours,
            "points": points,
            "series": series,
        }))
    }

    pub async fn reset(&self) -> Result<Value, String> {
        let _guard = lock().lock().await;
        let state = normalize_state(default_state());
        save_state(&self.root, &state).await?;
        Ok(json!({"ok": true, "state": state}))
    }
}

pub fn public_state(state: &Value) -> Value {
    let rooms: Vec<Value> = state["rooms"]
        .as_array()
        .map(|rooms| rooms.iter().map(public_room).collect())
        .unwrap_or_default();
    let devices: Vec<Value> = state["devices"]
        .as_object()
        .map(|devices| {
            devices
                .iter()
                .map(|(id, device)| public_device(id, device))
                .collect()
        })
        .unwrap_or_default();
    let scenes: Vec<Value> = state["scenes"]
        .as_object()
        .map(|scenes| {
            scenes
                .iter()
                .map(|(id, scene)| public_scene(id, scene))
                .collect()
        })
        .unwrap_or_default();
    json!({
        "version": 1,
        "updatedAt": state["updatedAt"].as_str().unwrap_or(""),
        "rooms": rooms,
        "devices": devices,
        "scenes": scenes,
        "energy": {
            "todayKwh": state["energy"]["today_kwh"].as_f64().unwrap_or(0.0),
            "cost": state["energy"]["cost_today"].as_f64().unwrap_or(0.0),
            "series": [],
        },
        "security": {
            "armed": state["security"]["armed"].as_bool().unwrap_or(false),
            "mode": if state["security"]["armed"].as_bool().unwrap_or(false) { "armed" } else { "disarmed" },
            "lastEvent": "",
        },
    })
}

pub fn public_device(id: &str, device: &Value) -> Value {
    let mut result = device.clone();
    result["id"] = json!(id);
    result["roomId"] = json!(device_room(device));
    result["capabilities"] = json!(capabilities(device["type"].as_str().unwrap_or("")));
    result["isOnline"] = json!(
        device
            .get("isOnline")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    );
    if !result["state"].is_object() {
        result["state"] = json!({});
    }
    result
}

fn with_id(id: &str, device: &Value) -> Value {
    let mut result = device.clone();
    result["id"] = json!(id);
    result
}

fn public_room(room: &Value) -> Value {
    if let Some(id) = room.as_str() {
        let mut chars = id.chars();
        let name = chars
            .next()
            .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
            .unwrap_or_default();
        return json!({"id": id, "name": name, "icon": "home"});
    }
    let mut room = room.clone();
    room["icon"] = room.get("icon").cloned().unwrap_or_else(|| json!("home"));
    room
}

fn public_scene(id: &str, scene: &Value) -> Value {
    let actions: Vec<Value> = scene_actions(scene)
        .into_iter()
        .map(|(device_id, state)| json!({"deviceId": device_id, "state": state}))
        .collect();
    json!({
        "id": id,
        "name": scene["name"].as_str().unwrap_or(""),
        "icon": scene.get("icon").cloned().unwrap_or_else(|| json!("sparkles")),
        "actions": actions,
    })
}

fn device_room(device: &Value) -> String {
    device["roomId"]
        .as_str()
        .or_else(|| device["room"].as_str())
        .unwrap_or("")
        .to_string()
}

async fn load_state(root: &Path) -> Result<Value, String> {
    let path = root.join(".deep_agents").join("house_state.json");
    let raw = match tokio::fs::read_to_string(&path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let state = normalize_state(default_state());
            save_state(root, &state).await?;
            return Ok(state);
        }
        Err(error) => return Err(format!("Error: {error}")),
    };
    let state = serde_json::from_str::<Value>(&raw).unwrap_or_else(|_| default_state());
    Ok(normalize_state(state))
}

async fn save_state(root: &Path, state: &Value) -> Result<(), String> {
    let path = root.join(".deep_agents").join("house_state.json");
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("Error: {error}"))?;
    }
    let raw = serde_json::to_string_pretty(state).map_err(|error| format!("Error: {error}"))?;
    tokio::fs::write(path, raw)
        .await
        .map_err(|error| format!("Error: {error}"))
}

fn normalize_state(mut state: Value) -> Value {
    if !state.is_object() {
        state = default_state();
    }
    if !state["devices"].is_object() {
        let mut devices = Map::new();
        if let Some(items) = state["devices"].as_array() {
            for item in items {
                if let Some(id) = item.get("id").and_then(Value::as_str) {
                    devices.insert(id.to_string(), item.clone());
                }
            }
        }
        state["devices"] = Value::Object(devices);
    }
    if !state["scenes"].is_object() {
        let mut scenes = Map::new();
        if let Some(items) = state["scenes"].as_array() {
            for item in items {
                if let Some(id) = item.get("id").and_then(Value::as_str) {
                    scenes.insert(id.to_string(), item.clone());
                }
            }
        }
        state["scenes"] = Value::Object(scenes);
    }
    for key in ["rooms", "history"] {
        if !state[key].is_object() && !state[key].is_array() {
            state[key] = if key == "rooms" {
                json!(["living", "kitchen", "bedroom", "bathroom", "garage"])
            } else {
                json!({})
            };
        }
    }
    if !state["energy"].is_object() {
        state["energy"] =
            json!({"today_kwh": 0.0, "month_kwh": 0.0, "cost_today": 0.0, "cost_month": 0.0});
    }
    if !state["security"].is_object() {
        state["security"] = json!({"armed": false, "alerts": [], "cameras": []});
    }
    state
}

fn default_state() -> Value {
    json!({
        "rooms": ["living", "kitchen", "bedroom", "bathroom", "garage"],
        "devices": {
            "living_light_1": {"name": "Living Room Light", "type": "light", "room": "living", "isOnline": true, "state": {"power": true, "brightness": 70}},
            "living_thermostat": {"name": "Living Room Thermostat", "type": "thermostat", "room": "living", "isOnline": true, "state": {"mode": "auto", "targetTemp": 72}},
            "living_tv_plug": {"name": "TV Plug", "type": "plug", "room": "living", "isOnline": true, "state": {"power": true}},
            "living_blinds": {"name": "Living Room Blinds", "type": "blinds", "room": "living", "isOnline": true, "state": {"position": 40}},
            "kitchen_light_1": {"name": "Kitchen Light", "type": "light", "room": "kitchen", "isOnline": true, "state": {"power": false, "brightness": 80}},
            "kitchen_light_2": {"name": "Kitchen Counter Light", "type": "light", "room": "kitchen", "isOnline": true, "state": {"power": false, "brightness": 60}},
            "kitchen_plug_1": {"name": "Coffee Maker Plug", "type": "plug", "room": "kitchen", "isOnline": true, "state": {"power": false}},
            "kitchen_sensor": {"name": "Kitchen Motion Sensor", "type": "sensor", "room": "kitchen", "isOnline": true, "state": {"alert": false}},
            "bedroom_light_1": {"name": "Bedroom Light", "type": "light", "room": "bedroom", "isOnline": true, "state": {"power": false, "brightness": 50}},
            "bedroom_thermostat": {"name": "Bedroom Thermostat", "type": "thermostat", "room": "bedroom", "isOnline": true, "state": {"mode": "cool", "targetTemp": 68}},
            "bedroom_lock": {"name": "Bedroom Lock", "type": "lock", "room": "bedroom", "isOnline": true, "state": {"locked": true}},
            "bathroom_light_1": {"name": "Bathroom Light", "type": "light", "room": "bathroom", "isOnline": true, "state": {"power": false, "brightness": 40}},
            "bathroom_plug": {"name": "Bathroom Fan Plug", "type": "plug", "room": "bathroom", "isOnline": true, "state": {"power": false}},
            "garage_light_1": {"name": "Garage Light", "type": "light", "room": "garage", "isOnline": true, "state": {"power": false, "brightness": 100}},
            "garage_lock": {"name": "Garage Door Lock", "type": "lock", "room": "garage", "isOnline": true, "state": {"locked": true}},
            "garage_camera": {"name": "Garage Camera", "type": "camera", "room": "garage", "isOnline": true, "state": {"recording": true}},
            "front_door_lock": {"name": "Front Door Lock", "type": "lock", "room": "living", "isOnline": true, "state": {"locked": true}},
            "front_camera": {"name": "Front Camera", "type": "camera", "room": "living", "isOnline": true, "state": {"recording": true}},
            "garage_sensor": {"name": "Garage Motion Sensor", "type": "sensor", "room": "garage", "isOnline": true, "state": {"alert": false}}
        },
        "scenes": {
            "movie_night": {"name": "Movie Night", "actions": {"living_light_1": {"power": false}, "living_blinds": {"position": 0}}},
            "good_morning": {"name": "Good Morning", "actions": {"living_light_1": {"power": true, "brightness": 60}, "kitchen_light_1": {"power": true, "brightness": 80}}},
            "goodnight": {"name": "Goodnight", "actions": {"living_light_1": {"power": false}, "bedroom_light_1": {"power": false}, "garage_lock": {"locked": true}, "front_door_lock": {"locked": true}}},
            "vacation": {"name": "Vacation Mode", "actions": {"living_light_1": {"power": false}, "garage_lock": {"locked": true}, "front_door_lock": {"locked": true}, "garage_camera": {"recording": true}}},
            "home": {"name": "I'm Home", "actions": {"living_light_1": {"power": true, "brightness": 70}, "front_door_lock": {"locked": false}}}
        },
        "energy": {"today_kwh": 3.2, "month_kwh": 82.5, "cost_today": 0.58, "cost_month": 14.9},
        "security": {"armed": false, "alerts": [], "cameras": ["garage_camera", "front_camera"]},
        "history": {},
        "updatedAt": Utc::now().to_rfc3339()
    })
}

fn format_state(state: &Value) -> String {
    let devices: BTreeMap<String, &Value> = state["devices"]
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(id, device)| (id.clone(), device))
                .collect()
        })
        .unwrap_or_default();
    let rooms = state["rooms"].as_array().cloned().unwrap_or_default();
    let scenes = state["scenes"].as_object().cloned().unwrap_or_default();
    let mut lines = vec![format!(
        "## House ({} rooms, {} devices)",
        rooms.len(),
        devices.len()
    )];
    lines.push(summary(state, &devices));
    for room in &rooms {
        let (id, name) = if let Some(id) = room.as_str() {
            (id.to_string(), id.to_string())
        } else {
            (
                room["id"].as_str().unwrap_or("").to_string(),
                room["name"].as_str().unwrap_or("").to_string(),
            )
        };
        lines.push(format!("\n### {name} ({id})"));
        for (device_id, device) in &devices {
            let room_id = device["roomId"]
                .as_str()
                .or_else(|| device["room"].as_str())
                .unwrap_or("");
            if room_id == id {
                lines.push(format!("- {}", format_device(device_id, device)));
            }
        }
    }
    lines.push("\n### Scenes".to_string());
    for (id, scene) in &scenes {
        let count = scene_actions(scene).len();
        lines.push(format!(
            "- {id} \"{}\" ({count} actions)",
            scene["name"].as_str().unwrap_or("")
        ));
    }
    lines.join("\n")
}

fn summary(state: &Value, devices: &BTreeMap<String, &Value>) -> String {
    let lights_on = devices
        .values()
        .filter(|device| device["type"].as_str() == Some("light") && power_on(device))
        .count();
    let unlocked = devices
        .values()
        .filter(|device| {
            device["type"].as_str() == Some("lock")
                && !device["state"]["locked"].as_bool().unwrap_or(false)
        })
        .count();
    let thermostat = devices
        .values()
        .find(|device| device["type"].as_str() == Some("thermostat"));
    let (mode, temp) = thermostat
        .map(|device| {
            (
                device["state"]["mode"].as_str().unwrap_or("unknown"),
                device["state"]["targetTemp"]
                    .as_f64()
                    .or_else(|| device["state"]["target_temperature"].as_f64())
                    .unwrap_or(0.0),
            )
        })
        .unwrap_or(("unknown", 0.0));
    let energy = state["energy"]["today_kwh"].as_f64().unwrap_or(0.0);
    format!(
        "House summary: {lights_on} lights on, {unlocked} locks unlocked, thermostat {mode} at {temp}°, today {energy} kWh."
    )
}

fn format_device(id: &str, device: &Value) -> String {
    let state = device["state"]
        .as_object()
        .map(|state| {
            state
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let room = device["roomId"]
        .as_str()
        .or_else(|| device["room"].as_str())
        .unwrap_or("?");
    format!(
        "{id} [{}] {} ({room}): {state}",
        device["type"].as_str().unwrap_or(""),
        device["name"].as_str().unwrap_or("")
    )
}

fn capabilities(device_type: &str) -> Vec<String> {
    match device_type {
        "light" => vec![
            "power".into(),
            "on".into(),
            "brightness".into(),
            "colorTemp".into(),
        ],
        "thermostat" => vec![
            "mode".into(),
            "targetTemp".into(),
            "target_temperature".into(),
        ],
        "lock" => vec!["locked".into()],
        "plug" => vec!["power".into(), "on".into()],
        "blinds" => vec!["position".into()],
        "camera" => vec!["recording".into()],
        "sensor" => vec!["alert".into(), "motion".into()],
        _ => Vec::new(),
    }
}

fn validate_patch(
    device_type: &str,
    patch: &Map<String, Value>,
) -> (BTreeMap<String, Value>, String) {
    let caps = capabilities(device_type);
    let bool_keys = ["on", "power", "locked", "recording", "alert", "motion"];
    let limits = [
        ("brightness", 0.0, 100.0),
        ("targetTemp", 10.0, 35.0),
        ("target_temperature", 50.0, 90.0),
        ("position", 0.0, 100.0),
        ("colorTemp", 1500.0, 9000.0),
    ];
    let mut valid = BTreeMap::new();
    for (key, value) in patch {
        if !caps.iter().any(|cap| cap == key) {
            continue;
        }
        if bool_keys.contains(&key.as_str()) {
            let canonical = if key == "on" { "power" } else { key };
            valid.insert(
                canonical.to_string(),
                json!(value.as_bool().unwrap_or(false)),
            );
            continue;
        }
        if key == "mode" && device_type == "thermostat" {
            if let Some(mode) = value.as_str()
                && ["heat", "cool", "auto", "off"].contains(&mode)
            {
                valid.insert("mode".to_string(), json!(mode));
            }
            continue;
        }
        if let Some((_, low, high)) = limits.iter().find(|(name, _, _)| name == key)
            && let Some(number) = as_f64(value)
        {
            let canonical = if key == "target_temperature" {
                "targetTemp"
            } else {
                key
            };
            valid.insert(
                canonical.to_string(),
                json!(number.clamp(*low, *high) as i64),
            );
        }
    }
    let error = if valid.is_empty() {
        "no valid state keys for this device type".to_string()
    } else {
        String::new()
    };
    (valid, error)
}

fn as_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|value| value as f64))
        .or_else(|| value.as_str()?.parse::<f64>().ok())
}

fn power_on(device: &Value) -> bool {
    device["state"]["power"]
        .as_bool()
        .or_else(|| device["state"]["on"].as_bool())
        .unwrap_or(false)
}

fn scene_actions(scene: &Value) -> Vec<(String, Map<String, Value>)> {
    let mut actions = Vec::new();
    if let Some(object) = scene["actions"].as_object() {
        for (device_id, patch) in object {
            if let Some(patch) = patch.as_object() {
                actions.push((device_id.clone(), patch.clone()));
            }
        }
    } else if let Some(items) = scene["actions"].as_array() {
        for item in items {
            let device_id = item["deviceId"]
                .as_str()
                .or_else(|| item["device_id"].as_str())
                .or_else(|| item["dev"].as_str())
                .or_else(|| item["id"].as_str())
                .unwrap_or("");
            let patch = item["state"]
                .as_object()
                .or_else(|| item["patch"].as_object())
                .cloned();
            if !device_id.is_empty()
                && let Some(patch) = patch
            {
                actions.push((device_id.to_string(), patch));
            }
        }
    }
    actions
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::QuestionBroker;

    use super::*;

    #[tokio::test]
    async fn reads_sets_and_runs_the_house() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path(), "house", Arc::new(QuestionBroker::new()));
        let state = house_get_state(&ctx, &json!({})).await;
        assert!(!state.is_error);
        assert!(state.content.contains("Living Room Light"));

        let set = house_set_device(
            &ctx,
            &json!({"device_id": "living_light_1", "state": {"brightness": 30, "on": true}}),
        )
        .await;
        assert!(!set.is_error, "{}", set.content);
        assert!(set.content.contains("brightness=30"));
        assert!(set.content.contains("power=true"));

        let scene = house_run_scene(&ctx, &json!({"scene_id": "movie_night"})).await;
        assert!(!scene.is_error, "{}", scene.content);
        assert!(scene.content.contains("Movie Night"));
        assert!(scene.content.contains("2 device"));

        let history =
            house_get_history(&ctx, &json!({"device_id": "living_light_1", "hours": 3})).await;
        assert!(!history.is_error);
        assert!(history.content.contains("last 3h"));

        let store = HouseStore::new(dir.path());
        let room = store
            .set_room("kitchen", json!({"power": true}).as_object().unwrap())
            .await
            .unwrap();
        assert!(
            room["applied"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == "kitchen_light_1")
        );
        let public = public_state(&store.state().await.unwrap());
        assert!(public["rooms"].as_array().unwrap()[0].get("id").is_some());
        assert!(
            public["devices"]
                .as_array()
                .unwrap()
                .iter()
                .any(|device| device["id"] == "kitchen_light_1")
        );
        assert!(
            public["scenes"].as_array().unwrap()[0]
                .get("actions")
                .is_some()
        );
    }
}
