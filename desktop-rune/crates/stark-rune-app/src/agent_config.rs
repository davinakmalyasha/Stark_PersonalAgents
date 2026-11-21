//! Agents & providers config — port of `AgentConfigModal.tsx` +
//! `services/api/stream.ts` agent-config contracts.
//!
//! Contract:
//! - `GET /api/config/agents` → `{agents:[{key,display,tier,override,
//!   effective:{provider,provider_label,base_url,model,thinking,has_key}}],
//!   issues[], active_provider, providers:[{id,label,has_key,
//!   models:[{key,label}]}], legacy_env[], health?}`.
//! - `PUT /api/config/agents/{key} {provider?,model?,thinking?,tools?,
//!   skills?,mcp?,system_prompt?}` (nulls inherit).
//! - `DELETE /api/config/agents/{key}` (reset to built-in).
//! - `POST /api/provider {provider}` (switch active; fires the provider
//!   refresh on the next 30s poll).
//!
//! React uses `<select>` dropdowns; Rune has no select element, so provider,
//! model and thinking pickers are tap-row option lists (the kind-dropdown
//! pattern). Everything else maps 1:1. All writes are explicit button
//! presses. Opened from the provider pill in either dock.

use crate::keys::EnterPress;
use rune::{col, row, text, text_input, El, TextEdit};

fn ink(hex: &'static str) -> rune::Color {
    rune::color::hex(hex).unwrap_or(rune::Color::BLACK)
}

/// Boxed pick handler (option rows carry their value).
type CfgPick<S> = std::rc::Rc<dyn Fn(&mut S, String)>;

/// One agent entry with its resolved effective config.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentConfigEntry {
    pub key: String,
    pub display: String,
    pub tier: String,
    pub eff_provider: String,
    pub eff_provider_label: String,
    pub eff_base_url: String,
    pub eff_model: String,
    pub eff_thinking: String,
    pub eff_has_key: bool,
}

impl AgentConfigEntry {
    pub fn from_json(v: &serde_json::Value) -> Self {
        let str_of = |v: &serde_json::Value, k: &str| {
            v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
        };
        let eff = v.get("effective").cloned().unwrap_or_default();
        Self {
            key: str_of(v, "key"),
            display: str_of(v, "display"),
            tier: str_of(v, "tier"),
            eff_provider: str_of(&eff, "provider"),
            eff_provider_label: str_of(&eff, "provider_label"),
            eff_base_url: str_of(&eff, "base_url"),
            eff_model: str_of(&eff, "model"),
            eff_thinking: str_of(&eff, "thinking"),
            eff_has_key: eff
                .get("has_key")
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
        }
    }
}

/// One provider with its model menu.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConfigProvider {
    pub id: String,
    pub label: String,
    pub has_key: bool,
    pub models: Vec<(String, String)>,
}

impl ConfigProvider {
    pub fn from_json(v: &serde_json::Value) -> Self {
        let str_of = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        Self {
            id: str_of("id"),
            label: str_of("label"),
            has_key: v.get("has_key").and_then(|x| x.as_bool()).unwrap_or(false),
            models: v
                .get("models")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .map(|m| {
                            (
                                m.get("key")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                m.get("label")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// Override form state. Empty provider/model = inherit (React's blank
/// selects); tools/skills/mcp are comma/newline lists (`valueToLines`).
#[derive(Debug, Clone, Default)]
pub struct ConfigForm {
    pub provider: String,
    pub model: String,
    pub thinking: String,
    pub tools: TextEdit,
    pub skills: TextEdit,
    pub mcp: TextEdit,
    pub system: TextEdit,
}

pub struct AgentConfigState {
    pub host: String,
    pub port: u16,
    pub open: bool,
    pub agents: Vec<AgentConfigEntry>,
    pub providers: Vec<ConfigProvider>,
    pub issues: Vec<String>,
    pub legacy_env: Vec<String>,
    pub health_error: String,
    pub active_provider: String,
    pub selected: String,
    pub form: ConfigForm,
    /// Which picker list is expanded (provider/model/thinking/none).
    pub picker: String,
    pub status: String,
    pub error: Option<String>,
    pub busy: bool,
    pub loading: bool,
    pub inbox: Option<std::sync::mpsc::Receiver<(Result<ConfigPayload, String>, String)>>,
    pub op_inbox: Option<std::sync::mpsc::Receiver<ConfigOpResult>>,
}

/// Full payload snapshot (form drafts live outside it).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConfigPayload {
    pub agents: Vec<AgentConfigEntry>,
    pub providers: Vec<ConfigProvider>,
    pub issues: Vec<String>,
    pub legacy_env: Vec<String>,
    pub health_error: String,
    pub active_provider: String,
}

impl ConfigPayload {
    pub fn from_json(v: &serde_json::Value) -> Self {
        let arr = |k: &str| {
            v.get(k)
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default()
        };
        let active = v
            .get("active_provider")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let health_error = v
            .get("health")
            .and_then(|x| x.as_object())
            .and_then(|o| o.get(&active))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        Self {
            agents: arr("agents")
                .iter()
                .map(AgentConfigEntry::from_json)
                .collect(),
            providers: arr("providers")
                .iter()
                .map(ConfigProvider::from_json)
                .collect(),
            issues: arr("issues")
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect(),
            legacy_env: arr("legacy_env")
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect(),
            health_error,
            active_provider: active,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfigOpResult {
    pub status: Option<String>,
    pub error: Option<String>,
    pub keep_selected: String,
}

impl std::fmt::Debug for AgentConfigState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentConfigState")
            .field("open", &self.open)
            .field("agents", &self.agents.len())
            .field("selected", &self.selected)
            .field("busy", &self.busy)
            .finish()
    }
}

impl Clone for AgentConfigState {
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
            port: self.port,
            open: self.open,
            agents: self.agents.clone(),
            providers: self.providers.clone(),
            issues: self.issues.clone(),
            legacy_env: self.legacy_env.clone(),
            health_error: self.health_error.clone(),
            active_provider: self.active_provider.clone(),
            selected: self.selected.clone(),
            form: self.form.clone(),
            picker: self.picker.clone(),
            status: self.status.clone(),
            error: self.error.clone(),
            busy: self.busy,
            loading: self.loading,
            inbox: None,
            op_inbox: None,
        }
    }
}

impl Default for AgentConfigState {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8124,
            open: false,
            agents: Vec::new(),
            providers: Vec::new(),
            issues: Vec::new(),
            legacy_env: Vec::new(),
            health_error: String::new(),
            active_provider: String::new(),
            selected: "developer".to_string(),
            form: ConfigForm::default(),
            picker: String::new(),
            status: String::new(),
            error: None,
            busy: false,
            loading: false,
            inbox: None,
            op_inbox: None,
        }
    }
}

/// Open the modal and load (React's `useEffect` on `[isOpen]`).
pub fn open_agent_config(s: &mut AgentConfigState) {
    s.open = true;
    s.status.clear();
    s.error = None;
    refresh_agent_config(s);
}

/// Load the payload; keeps the selection when it still exists.
pub fn refresh_agent_config(s: &mut AgentConfigState) {
    if s.loading {
        return;
    }
    s.loading = true;
    s.error = None;
    let (tx, rx) = std::sync::mpsc::channel();
    s.inbox = Some(rx);
    let (host, port) = (s.host.clone(), s.port);
    let keep = s.selected.clone();
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let result = match crate::transport::fetch_json("/api/config/agents").await {
            Ok(v) => Ok(ConfigPayload::from_json(&v)),
            Err(e) => Err(e),
        };
        let _ = tx.send((result, keep));
        let _ = (host, port);
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let result = match crate::transport::get_json(&host, port, "/api/config/agents") {
            Ok(v) => Ok(ConfigPayload::from_json(&v)),
            Err(e) => Err(e),
        };
        let _ = tx.send((result, keep));
    });
}

/// Drain both inboxes. Returns true on change.
pub fn poll_agent_config(s: &mut AgentConfigState) -> bool {
    let mut changed = false;
    // Wider inbox (payload + keep key) drains through the same helper shape.
    while let Some((result, keep)) = s.inbox.as_ref().and_then(|rx| rx.try_recv().ok()) {
        match result {
            Ok(p) => {
                let keep = if p.agents.iter().any(|a| a.key == keep) {
                    keep
                } else if p.agents.iter().any(|a| a.key == s.selected) {
                    s.selected.clone()
                } else {
                    p.agents.first().map(|a| a.key.clone()).unwrap_or_default()
                };
                s.selected = keep;
                s.agents = p.agents;
                s.providers = p.providers;
                s.issues = p.issues;
                s.legacy_env = p.legacy_env;
                s.health_error = p.health_error;
                s.active_provider = p.active_provider;
                s.error = None;
            }
            Err(message) => {
                s.error = Some(message);
            }
        }
        s.loading = false;
        changed = true;
    }
    while let Some(res) = s.op_inbox.as_ref().and_then(|rx| rx.try_recv().ok()) {
        s.busy = false;
        s.error = res.error;
        s.status = res.status.unwrap_or_default();
        if !res.keep_selected.is_empty() && s.agents.iter().any(|a| a.key == res.keep_selected) {
            s.selected = res.keep_selected;
        }
        if s.error.is_none() {
            refresh_agent_config(s);
        }
        changed = true;
    }
    let dead = s.inbox.as_ref().is_some_and(|rx| {
        matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        )
    });
    if dead {
        s.inbox = None;
    }
    let op_dead = s.op_inbox.as_ref().is_some_and(|rx| {
        matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        )
    });
    if op_dead {
        s.op_inbox = None;
    }
    changed
}

fn op_sender(s: &mut AgentConfigState) -> std::sync::mpsc::Sender<ConfigOpResult> {
    if s.op_inbox.is_none() {
        let (tx, rx) = std::sync::mpsc::channel();
        s.op_inbox = Some(rx);
        return tx;
    }
    // Reuse: swap-and-forward would strand workers; instead keep one channel
    // per state by draining first (ops serialize on `busy` anyway).
    let (tx, rx) = std::sync::mpsc::channel();
    if let Some(old) = s.op_inbox.replace(rx) {
        while let Ok(msg) = old.try_recv() {
            let _ = tx.send(msg);
        }
    }
    // The just-created sender's own messages were moved above; but workers
    // spawned earlier hold older senders whose channel was already drained.
    // Ops serialize on `busy`, so at most one worker is ever in flight.
    let (tx2, rx2) = std::sync::mpsc::channel();
    s.op_inbox = Some(rx2);
    let _ = tx;
    tx2
}

/// Save the form (`PUT /api/config/agents/{key}`; blanks inherit).
pub fn save_agent_config(s: &mut AgentConfigState) {
    if s.busy {
        return;
    }
    let Some(entry) = s.agents.iter().find(|a| a.key == s.selected).cloned() else {
        return;
    };
    s.busy = true;
    s.error = None;
    s.status.clear();
    let tx = op_sender(s);
    let (host, port) = (s.host.clone(), s.port);
    let body = serde_json::json!({
        "provider": non_empty(s.form.provider.trim()),
        "model": non_empty(s.form.model.trim()),
        "thinking": non_empty(s.form.thinking.trim()),
        "tools": split_list(&s.form.tools.text),
        "skills": split_list(&s.form.skills.text),
        "mcp": split_list(&s.form.mcp.text),
        "system_prompt": non_empty(s.form.system.text.trim()),
    });
    let key = entry.key.clone();
    let display = entry.display.clone();
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let res = match crate::transport::fetch_send(
            "PUT",
            &format!("/api/config/agents/{key}"),
            Some(&body),
        )
        .await
        {
            Ok(_) => ConfigOpResult {
                status: Some(format!("Saved {display}. The next run uses this config.")),
                error: None,
                keep_selected: key,
            },
            Err(e) => ConfigOpResult {
                status: None,
                error: Some(e),
                keep_selected: key,
            },
        };
        let _ = tx.send(res);
        let _ = (host, port);
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let res = match crate::transport::send_json(
            &host,
            port,
            "PUT",
            &format!("/api/config/agents/{key}"),
            &body,
        ) {
            Ok(_) => ConfigOpResult {
                status: Some(format!("Saved {display}. The next run uses this config.")),
                error: None,
                keep_selected: key,
            },
            Err(e) => ConfigOpResult {
                status: None,
                error: Some(e),
                keep_selected: key,
            },
        };
        let _ = tx.send(res);
    });
}

fn non_empty(s: &str) -> serde_json::Value {
    if s.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::String(s.to_string())
    }
}

fn split_list(text: &str) -> serde_json::Value {
    let t = text.trim();
    if t.is_empty() {
        return serde_json::Value::Null;
    }
    if t.eq_ignore_ascii_case("none") || t == "-" {
        return serde_json::Value::Array(vec![]);
    }
    serde_json::Value::Array(
        t.split([',', '\n'])
            .map(|x| x.trim())
            .filter(|x| !x.is_empty())
            .map(|x| serde_json::Value::String(x.to_string()))
            .collect(),
    )
}

/// Reset to built-in (`DELETE /api/config/agents/{key}`).
pub fn reset_agent_config(s: &mut AgentConfigState) {
    if s.busy {
        return;
    }
    let Some(entry) = s.agents.iter().find(|a| a.key == s.selected).cloned() else {
        return;
    };
    s.busy = true;
    s.error = None;
    s.status.clear();
    let tx = op_sender(s);
    let (host, port) = (s.host.clone(), s.port);
    let key = entry.key.clone();
    let display = entry.display.clone();
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let res = match crate::transport::fetch_send(
            "DELETE",
            &format!("/api/config/agents/{key}"),
            None,
        )
        .await
        {
            Ok(_) => ConfigOpResult {
                status: Some(format!("Reset {display} to the built-in config.")),
                error: None,
                keep_selected: key,
            },
            Err(e) => ConfigOpResult {
                status: None,
                error: Some(e),
                keep_selected: key,
            },
        };
        let _ = tx.send(res);
        let _ = (host, port);
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let res = match crate::transport::send_json(
            &host,
            port,
            "DELETE",
            &format!("/api/config/agents/{key}"),
            &serde_json::Value::Null,
        ) {
            Ok(_) => ConfigOpResult {
                status: Some(format!("Reset {display} to the built-in config.")),
                error: None,
                keep_selected: key,
            },
            Err(e) => ConfigOpResult {
                status: None,
                error: Some(e),
                keep_selected: key,
            },
        };
        let _ = tx.send(res);
    });
}

fn label_style() -> rune::tok::TextStyle {
    rune::tok::TextStyle::new(10.0, 600.0)
}

fn field_label(label: &str) -> El {
    text(label.to_string())
        .font(label_style())
        .color(ink("#71717a"))
}

/// Tap-row option list (Rune has no `<select>`): the kind-dropdown pattern.
/// `current` marks the selected row with a check.
fn option_list<S: 'static>(
    key: &str,
    options: &[(String, String)],
    current: &str,
    on_pick: CfgPick<S>,
) -> El {
    let mut list = col()
        .key(key.to_string())
        .w_full()
        .gap(2.0)
        .role("listbox")
        .label("Options")
        .focus_group();
    for (value, label) in options {
        let target = value.clone();
        let selected = value == current;
        let pick = on_pick.clone();
        list = list.child(
            row()
                .key(format!("{key}-{value}"))
                .w_full()
                .items_center()
                .gap(8.0)
                .px(10.0)
                .py(6.0)
                .radius(6.0)
                .fill(ink(if selected { "#ffffff14" } else { "#00000000" }))
                .role("option")
                .focusable(true)
                .focus_selected(selected)
                .focus_highlight(ink("#ffffff0f"))
                .label(format!("Pick {label}"))
                .on_press(move |s: &mut S| pick(s, target.clone()))
                .child(
                    text(label.clone())
                        .grow(1.0)
                        .font(rune::tok::TextStyle::new(12.0, 400.0))
                        .color(ink("#d4d4d8")),
                )
                .child(
                    text(if selected { "✓" } else { "" }.to_string())
                        .font(rune::tok::TextStyle::new(11.0, 600.0))
                        .color(ink("#e4e4e7")),
                ),
        );
    }
    list
}

/// Config modal (`AgentConfigModal`): head, provider+agent sidebar, per-agent
/// form. Mounted wherever its thread renders (standalone transcript or the
/// transcript sheet); `which` resolves handlers to that thread.
pub fn agent_config_sheet<S: crate::rune_transcript::TranscriptHost + 'static>(
    s: &AgentConfigState,
    which: crate::rune_transcript::Thread,
) -> El {
    let entry = s.agents.iter().find(|a| a.key == s.selected).cloned();
    let providers = s.providers.clone();
    let active = s.active_provider.clone();
    let mut side = col()
        .key("agentcfg-side")
        .w(210.0)
        .gap(10.0)
        .px(12.0)
        .pt(12.0)
        .child(field_label("Active provider"))
        .child(
            text(
                providers
                    .iter()
                    .find(|p| p.id == active)
                    .map(|p| p.label.clone())
                    .unwrap_or_else(|| active.clone()),
            )
            .key("agentcfg-active")
            .font(rune::tok::TextStyle::new(12.5, 400.0))
            .px(12.0)
            .py(8.0)
            .radius(8.0)
            .fill(ink("#ffffff08"))
            .border(1.0, ink("#ffffff1a"))
            .color(ink("#e4e4e7"))
            .role("button")
            .focusable(true)
            .label("Switch active provider")
            .on_press_enter(move |s: &mut S| {
                let st = &mut s.thread_mut(which).agent_config;
                st.picker = if st.picker == "active" {
                    String::new()
                } else {
                    "active".to_string()
                };
            }),
        );
    if s.picker == "active" {
        let opts: Vec<(String, String)> = providers
            .iter()
            .map(|p| {
                (
                    p.id.clone(),
                    if p.has_key {
                        p.label.clone()
                    } else {
                        format!("{} (no key)", p.label)
                    },
                )
            })
            .collect();
        side = side.child(option_list::<S>(
            "agentcfg-active-list",
            &opts,
            &active,
            std::rc::Rc::new(move |s: &mut S, id: String| {
                let st = &mut s.thread_mut(which).agent_config;
                st.picker.clear();
                switch_provider(st, &id);
            }),
        ));
    }
    for agent in &s.agents {
        let key = agent.key.clone();
        let selected = agent.key == s.selected;
        side = side.child(
            col()
                .key(format!("agentcfg-agent-{key}"))
                .w_full()
                .gap(2.0)
                .radius(8.0)
                .px(10.0)
                .py(8.0)
                .fill(ink(if selected { "#ffffff14" } else { "#00000000" }))
                .role("button")
                .focusable(true)
                .label(format!("Configure {}", agent.display))
                .on_press_enter(move |s: &mut S| {
                    let st = &mut s.thread_mut(which).agent_config;
                    st.selected = key.clone();
                    st.status.clear();
                    st.error = None;
                    st.picker.clear();
                    st.form = ConfigForm::default();
                })
                .child(
                    text(agent.display.clone())
                        .font(rune::tok::TextStyle::new(12.0, 500.0))
                        .color(ink(if selected { "#f4f4f5" } else { "#a1a1aa" })),
                )
                .child(
                    text(format!("{} · {}", agent.eff_provider, agent.eff_model))
                        .font(rune::tok::TextStyle::new(10.0, 400.0))
                        .color(ink("#71717a")),
                ),
        );
    }
    let mut main = col()
        .key("agentcfg-main")
        .grow(1.0)
        .min_h(0.0)
        .gap(12.0)
        .px(20.0)
        .pt(16.0);
    if let Some(entry) = entry {
        let form_provider = if s.form.provider.is_empty() {
            active.clone()
        } else {
            s.form.provider.clone()
        };
        let provider = providers.iter().find(|p| p.id == form_provider).cloned();
        let models: Vec<(String, String)> = provider
            .as_ref()
            .map(|p| {
                p.models
                    .iter()
                    .map(|(k, l)| (k.clone(), l.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        main = main
            .child(
                row()
                    .key("agentcfg-title")
                    .w_full()
                    .items_center()
                    .gap(10.0)
                    .child(
                        col()
                            .grow(1.0)
                            .min_h(0.0)
                            .child(
                                text(entry.display.clone())
                                    .font(rune::tok::TextStyle::new(13.5, 600.0))
                                    .color(ink("#f4f4f5")),
                            )
                            .child(
                                text(format!(
                                    "tier: {} · {} · {}",
                                    entry.tier, entry.eff_provider_label, entry.eff_base_url
                                ))
                                .font(rune::tok::TextStyle::new(10.5, 400.0))
                                .color(ink("#71717a")),
                            ),
                    )
                    .child(
                        text(if entry.eff_has_key {
                            "key ready"
                        } else {
                            "key missing"
                        })
                        .key("agentcfg-key")
                        .font(rune::tok::TextStyle::new(10.0, 600.0))
                        .px(8.0)
                        .py(2.0)
                        .radius(10.0)
                        .fill(ink(if entry.eff_has_key {
                            "#6ee7b712"
                        } else {
                            "#fbbf2412"
                        }))
                        .border(
                            1.0,
                            ink(if entry.eff_has_key {
                                "#6ee7b740"
                            } else {
                                "#fbbf244d"
                            }),
                        )
                        .color(ink(if entry.eff_has_key {
                            "#6ee7b7"
                        } else {
                            "#fbbf24"
                        })),
                    ),
            )
            .child(
                text(format!(
                    "resolved: {} / {} / {}",
                    entry.eff_provider,
                    entry.eff_model,
                    if entry.eff_thinking.is_empty() {
                        "provider default"
                    } else {
                        entry.eff_thinking.as_str()
                    }
                ))
                .key("agentcfg-resolved")
                .font(rune::tok::TextStyle::new(10.5, 400.0).mono())
                .px(12.0)
                .py(8.0)
                .radius(8.0)
                .fill(rune::Color {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.02,
                })
                .border(1.0, ink("#ffffff12"))
                .color(ink("#a1a1aa")),
            );
        // Provider + model pickers.
        main = main.child(picker_row::<S>(
            "agentcfg-provider",
            "Provider",
            if s.form.provider.is_empty() {
                format!("Inherit active ({active})")
            } else {
                s.form.provider.clone()
            },
            "provider",
            s.picker == "provider",
            which,
        ));
        if s.picker == "provider" {
            let mut opts = vec![(String::new(), format!("Inherit active ({active})"))];
            for p in &providers {
                opts.push((p.id.clone(), p.label.clone()));
            }
            main = main.child(option_list::<S>(
                "agentcfg-provider-list",
                &opts,
                &s.form.provider,
                std::rc::Rc::new(move |s: &mut S, id: String| {
                    let st = &mut s.thread_mut(which).agent_config;
                    st.form.provider = id;
                    st.form.model.clear();
                    st.picker.clear();
                }),
            ));
        }
        main = main.child(picker_row::<S>(
            "agentcfg-model",
            "Model",
            if s.form.model.is_empty() {
                "Inherit tier default".to_string()
            } else {
                s.form.model.clone()
            },
            "model",
            s.picker == "model",
            which,
        ));
        if s.picker == "model" {
            let mut opts = vec![(String::new(), "Inherit tier default".to_string())];
            for (k, l) in &models {
                opts.push((k.clone(), l.clone()));
            }
            main = main.child(option_list::<S>(
                "agentcfg-model-list",
                &opts,
                &s.form.model,
                std::rc::Rc::new(move |s: &mut S, id: String| {
                    let st = &mut s.thread_mut(which).agent_config;
                    st.form.model = id;
                    st.picker.clear();
                }),
            ));
        }
        // Reasoning + MCP.
        main = main.child(
            row()
                .key("agentcfg-row2")
                .w_full()
                .gap(10.0)
                .child(col().grow(1.0).min_h(0.0).gap(6.0).child(picker_row::<S>(
                    "agentcfg-thinking",
                    "Reasoning effort",
                    if s.form.thinking.is_empty() {
                        "Inherit tier default".to_string()
                    } else {
                        s.form.thinking.clone()
                    },
                    "thinking",
                    s.picker == "thinking",
                    which,
                )))
                .child(
                    col()
                        .grow(1.0)
                        .min_h(0.0)
                        .gap(6.0)
                        .child(field_label("MCP servers"))
                        .child(
                            text_input(&s.form.mcp, rune::tok::TextStyle::new(12.5, 400.0))
                                .key("agentcfg-mcp")
                                .w_full()
                                .placeholder("blank = all, none = none", ink("#52525b"))
                                .caret(ink("#f2f2f4"), crate::motion::TEXT_SELECTION)
                                .on_edit(move |s: &mut S| {
                                    &mut s.thread_mut(which).agent_config.form.mcp
                                }),
                        ),
                ),
        );
        if s.picker == "thinking" {
            let opts: Vec<(String, String)> = ["", "none", "minimal", "low", "medium", "high"]
                .iter()
                .map(|v| {
                    (
                        v.to_string(),
                        if v.is_empty() {
                            "Inherit tier default".to_string()
                        } else {
                            v.to_string()
                        },
                    )
                })
                .collect();
            main = main.child(option_list::<S>(
                "agentcfg-thinking-list",
                &opts,
                &s.form.thinking,
                std::rc::Rc::new(move |s: &mut S, id: String| {
                    let st = &mut s.thread_mut(which).agent_config;
                    st.form.thinking = id;
                    st.picker.clear();
                }),
            ));
        }
        // Tools, skills, system prompt.
        for (key, label, placeholder) in [
            (
                "tools",
                "Tools (comma-separated)",
                "blank = scope default, none = no tools",
            ),
            ("skills", "Skills", "blank = all skills, none = none"),
        ] {
            let label = label.to_string();
            let placeholder = placeholder.to_string();
            let key = key.to_string();
            main = main.child(
                col()
                    .key(format!("agentcfg-{key}"))
                    .w_full()
                    .gap(6.0)
                    .child(field_label(&label))
                    .child({
                        let edit = if key == "tools" {
                            &s.form.tools
                        } else {
                            &s.form.skills
                        };
                        text_input(edit, rune::tok::TextStyle::new(12.5, 400.0))
                            .w_full()
                            .placeholder(&placeholder, ink("#52525b"))
                            .caret(ink("#f2f2f4"), crate::motion::TEXT_SELECTION)
                            .on_edit(move |s: &mut S| {
                                if key == "tools" {
                                    &mut s.thread_mut(which).agent_config.form.tools
                                } else {
                                    &mut s.thread_mut(which).agent_config.form.skills
                                }
                            })
                    }),
            );
        }
        main = main.child(
            col()
                .key("agentcfg-system")
                .w_full()
                .gap(6.0)
                .child(field_label("System prompt (replaces the agent preamble)"))
                .child(
                    text_input(&s.form.system, rune::tok::TextStyle::new(12.5, 400.0))
                        .w_full()
                        .placeholder("blank = built-in prompt", ink("#52525b"))
                        .caret(ink("#f2f2f4"), crate::motion::TEXT_SELECTION)
                        .on_edit(move |s: &mut S| {
                            &mut s.thread_mut(which).agent_config.form.system
                        }),
                ),
        );
        if !s.issues.is_empty() {
            let mut notes = col()
                .key("agentcfg-issues")
                .w_full()
                .gap(4.0)
                .radius(8.0)
                .fill(ink("#fbbf240a"))
                .border(1.0, ink("#fbbf2433"))
                .px(12.0)
                .py(8.0);
            for (i, issue) in s.issues.iter().enumerate() {
                notes = notes.child(
                    text(issue.clone())
                        .key(format!("agentcfg-issue-{i}"))
                        .font(rune::tok::TextStyle::new(10.5, 400.0))
                        .color(ink("#fcd34d")),
                );
            }
            main = main.child(notes);
        }
        if !s.health_error.is_empty() {
            main = main.child(
                text(format!("Provider check failed: {}", s.health_error))
                    .key("agentcfg-health")
                    .font(rune::tok::TextStyle::new(10.5, 400.0))
                    .px(12.0)
                    .py(8.0)
                    .radius(8.0)
                    .fill(ink("#f8717112"))
                    .border(1.0, ink("#f8717133"))
                    .color(ink("#fca5a5")),
            );
        }
        if !s.legacy_env.is_empty() {
            main = main.child(
                text(format!(
                    "Ignored legacy env: {}. Provider routing lives in .deep_agents/config/providers.json; these variables are not read.",
                    s.legacy_env.join(", ")
                ))
                .key("agentcfg-legacy")
                .font(rune::tok::TextStyle::new(10.5, 400.0))
                .wrap()
                .px(12.0)
                .py(8.0)
                .radius(8.0)
                .fill(rune::Color {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.02,
                })
                .border(1.0, ink("#ffffff1a"))
                .color(ink("#a1a1aa")),
            );
        }
        if let Some(err) = &s.error {
            main = main.child(
                text(err.clone())
                    .key("agentcfg-error")
                    .role("alert")
                    .live_region(rune::LiveRegion::Assertive)
                    .font(rune::tok::TextStyle::new(11.0, 400.0))
                    .color(ink("#fca5a5")),
            );
        }
        if !s.status.is_empty() {
            main = main.child(
                text(s.status.clone())
                    .key("agentcfg-status")
                    .role("status")
                    .live_region(rune::LiveRegion::Polite)
                    .font(rune::tok::TextStyle::new(11.0, 400.0))
                    .color(ink("#6ee7b7")),
            );
        }
        main = main.child(
            row()
                .key("agentcfg-actions")
                .w_full()
                .justify_end()
                .gap(8.0)
                .child(
                    text(
                        if s.busy {
                            "Working…"
                        } else {
                            "Reset to built-in"
                        }
                        .to_string(),
                    )
                    .key("agentcfg-reset")
                    .role("button")
                    .focusable(true)
                    .label("Reset agent config")
                    .font(rune::tok::TextStyle::new(11.5, 400.0))
                    .px(12.0)
                    .py(6.0)
                    .radius(8.0)
                    .fill(ink("#00000000"))
                    .border(1.0, ink("#ffffff1a"))
                    .color(ink("#a1a1aa"))
                    .on_press_enter(move |s: &mut S| {
                        reset_agent_config(&mut s.thread_mut(which).agent_config);
                    }),
                )
                .child(
                    text(if s.busy { "Saving…" } else { "Save agent" }.to_string())
                        .key("agentcfg-save")
                        .role("button")
                        .focusable(true)
                        .label("Save agent config")
                        .font(rune::tok::TextStyle::new(11.5, 600.0))
                        .px(14.0)
                        .py(6.0)
                        .radius(8.0)
                        .fill(ink("#6366f1"))
                        .color(ink("#ffffff"))
                        .on_press_enter(move |s: &mut S| {
                            save_agent_config(&mut s.thread_mut(which).agent_config);
                        }),
                ),
        );
    } else {
        main = main.child(
            text("No agent selected.".to_string())
                .font(rune::tok::TextStyle::new(12.0, 400.0))
                .color(ink("#71717a")),
        );
    }
    rune::el()
        .key("agentcfg-overlay")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .center()
        .modal(true, move |s: &mut S| {
            s.thread_mut(which).agent_config.open = false;
        })
        // React mounts a separate `modal-backdrop-enter` layer; keeping it a
        // sibling lets the card fade/rise on its own clock.
        .child(
            rune::el()
                .key("agentcfg-backdrop")
                .absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .fill(rune::Color {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.7,
                })
                .backdrop_blur(12.0)
                .transition(crate::motion::backdrop_enter()),
        )
        .child(
            col()
                .key("agentcfg-card")
                .transition(crate::motion::modal_enter())
                .w(900.0)
                .radius(16.0)
                .fill(ink("#09090b"))
                .border(1.0, ink("#ffffff14"))
                .px(22.0)
                .pt(18.0)
                .pb(18.0)
                .gap(12.0)
                .child(
                    row()
                        .key("agentcfg-head")
                        .w_full()
                        .items_center()
                        .gap(10.0)
                        .child(
                            col()
                                .grow(1.0)
                                .min_h(0.0)
                                .child(
                                    text("Agents & providers".to_string())
                                        .key("agentcfg-title")
                                        .font(rune::tok::TextStyle::new(14.0, 600.0))
                                        .color(ink("#f4f4f5")),
                                )
                                .child(
                                    text(
                                        "Per-agent provider, model, reasoning effort, tools, skills, and prompt"
                                            .to_string(),
                                    )
                                    .font(rune::tok::TextStyle::new(11.0, 400.0))
                                    .color(ink("#71717a")),
                                ),
                        )
                        .child(
                            rune::el()
                                .key("agentcfg-close")
                                .size(16.0, 16.0)
                                .role("button")
                .focusable(true)
                                .label("Close agents and providers")
                                .shape(crate::lucide_x(16.0, 2.0, ink("#8a8a93")))
                                .on_press_enter(move |s: &mut S| {
                                    s.thread_mut(which).agent_config.open = false;
                                }),
                        ),
                )
                .child(
                    row()
                        .key("agentcfg-body")
                        .w_full()
                        .gap(12.0)
                        .child(side)
                        .child(main),
                ),
        )
}

/// Picker trigger row (label + current value + chevron). `name` is the
/// row's own picker id; `open` whether its list shows; the press toggles it.
fn picker_row<S: crate::rune_transcript::TranscriptHost + 'static>(
    key: &str,
    label: &str,
    current: String,
    name: &str,
    open: bool,
    which: crate::rune_transcript::Thread,
) -> El {
    let name = name.to_string();
    col()
        .key(key.to_string())
        .w_full()
        .gap(6.0)
        .child(field_label(label))
        .child(
            row()
                .key(format!("{key}-trigger"))
                .w_full()
                .items_center()
                .gap(8.0)
                .px(12.0)
                .py(8.0)
                .radius(8.0)
                .fill(rune::Color {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.03,
                })
                .border(1.0, ink("#ffffff1a"))
                .role("button")
                .focusable(true)
                .label(format!("Pick {label}"))
                .on_press_enter(move |s: &mut S| {
                    let st = &mut s.thread_mut(which).agent_config;
                    st.picker = if st.picker == name {
                        String::new()
                    } else {
                        name.clone()
                    };
                })
                .child(
                    text(current)
                        .grow(1.0)
                        .font(rune::tok::TextStyle::new(12.5, 400.0))
                        .color(ink("#e4e4e7")),
                )
                .child(
                    text(if open { "▴" } else { "▾" }.to_string())
                        .font(rune::tok::TextStyle::new(10.0, 400.0))
                        .color(ink("#71717a")),
                ),
        )
}

/// Switch the active provider (`POST /api/provider {provider}`).
pub fn switch_provider(s: &mut AgentConfigState, id: &str) {
    if s.busy {
        return;
    }
    s.busy = true;
    s.error = None;
    s.status.clear();
    let tx = op_sender(s);
    let (host, port) = (s.host.clone(), s.port);
    let id = id.to_string();
    let keep = s.selected.clone();
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let res = match crate::transport::fetch_send(
            "POST",
            "/api/provider",
            Some(&serde_json::json!({ "provider": id })),
        )
        .await
        {
            Ok(v) => ConfigOpResult {
                status: Some(format!(
                    "Active provider is now {}.",
                    v.get("provider").and_then(|x| x.as_str()).unwrap_or("")
                )),
                error: None,
                keep_selected: keep,
            },
            Err(e) => ConfigOpResult {
                status: None,
                error: Some(e),
                keep_selected: keep,
            },
        };
        let _ = tx.send(res);
        let _ = (host, port);
    });
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let res = match crate::transport::send_json(
            &host,
            port,
            "POST",
            "/api/provider",
            &serde_json::json!({ "provider": id }),
        ) {
            Ok(v) => ConfigOpResult {
                status: Some(format!(
                    "Active provider is now {}.",
                    v.get("provider").and_then(|x| x.as_str()).unwrap_or("")
                )),
                error: None,
                keep_selected: keep,
            },
            Err(e) => ConfigOpResult {
                status: None,
                error: Some(e),
                keep_selected: keep,
            },
        };
        let _ = tx.send(res);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_parses_live_shape() {
        let v = serde_json::json!({
            "agents": [{"key": "developer", "display": "Developer", "tier": "workspace",
                        "effective": {"provider": "opencode", "provider_label": "OpenCode Go",
                                      "base_url": "https://x", "model": "m", "thinking": "high",
                                      "has_key": true}}],
            "issues": ["i1"], "active_provider": "opencode",
            "providers": [{"id": "opencode", "label": "OpenCode Go", "has_key": true,
                           "models": [{"key": "k", "label": "L"}]}],
            "legacy_env": ["OLD"], "health": {},
        });
        let p = ConfigPayload::from_json(&v);
        assert_eq!(p.agents.len(), 1);
        assert_eq!(p.providers[0].models.len(), 1);
        assert_eq!(p.active_provider, "opencode");
        assert_eq!(p.legacy_env, vec!["OLD".to_string()]);
    }

    #[test]
    fn list_splitting_matches_reference() {
        assert_eq!(split_list("a, b\nc"), serde_json::json!(["a", "b", "c"]));
        assert_eq!(split_list("none"), serde_json::json!([]));
        assert_eq!(split_list(""), serde_json::Value::Null);
    }

    #[test]
    fn modal_mounts_with_form_and_pickers() {
        use rune::{Env, Runtime};
        let mut init = crate::rune_transcript::RuneTranscriptState::screenshot();
        init.live = false;
        init.agent_config.open = true;
        init.agent_config.selected = "developer".to_string();
        init.agent_config.active_provider = "opencode".to_string();
        init.agent_config.agents = vec![AgentConfigEntry {
            key: "developer".to_string(),
            display: "Developer".to_string(),
            tier: "workspace".to_string(),
            eff_provider: "opencode".to_string(),
            eff_provider_label: "OpenCode Go".to_string(),
            eff_base_url: "https://x".to_string(),
            eff_model: "m".to_string(),
            eff_thinking: "high".to_string(),
            eff_has_key: true,
        }];
        init.agent_config.providers = vec![ConfigProvider {
            id: "opencode".to_string(),
            label: "OpenCode Go".to_string(),
            has_key: true,
            models: vec![("k".to_string(), "L".to_string())],
        }];
        let mut rt = Runtime::new(
            init,
            crate::rune_transcript::transcript_view,
            (900.0, 700.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        for key in [
            "agentcfg-overlay",
            "agentcfg-backdrop",
            "agentcfg-card",
            "agentcfg-title",
            "agentcfg-active",
            "agentcfg-agent-developer",
            "agentcfg-key",
            "agentcfg-resolved",
            "agentcfg-mcp",
            "agentcfg-save",
            "agentcfg-close",
        ] {
            assert!(rt.find(key).is_some(), "{key} mounted");
        }
        // Provider picker expands with the inherit row.
        let (x, y) = rt.find("agentcfg-provider-trigger").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.find("agentcfg-provider-list").is_some());
    }
}
