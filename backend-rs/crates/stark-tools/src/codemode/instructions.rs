//! OpenCode `CodeModeInstructions` updates, plus durable per-chat instruction
//! state. Port of `packages/core/src/codemode/instructions.ts` (MIT).
//!
//! Stark rebuilds its system prompt every turn, so it keeps the rendered
//! instruction sequence per chat and appends OpenCode's initial/update/
//! removed blocks instead of re-sending a full catalog that OpenCode would
//! have already replaced with a delta.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::catalog::{self, CatalogItem, Listing, NamespaceSummary, Summary};

const UPDATE_BUDGET: usize = 1_000;

const SEARCH_NOTE: &str = "You can also look up tools that aren't listed with the global `search(...)` function inside `execute`. Before calling a tool that is not listed, find it this way. Do not guess tool names. Each result's `path` is the complete call expression: write it out as code, for example `await tools.ns.tool_name({ ... })`. Do not use it as a key such as `tools[path]`.

- ";

const REMOVED: &str = "The `execute` tool is no longer available. Permitted tools are now available as direct tool calls.";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Stored {
    /// Frozen epoch baseline: text rendered from the first observed catalog.
    /// Empty when Code Mode was already unavailable at the baseline.
    #[serde(default)]
    initial: String,
    /// Current catalog values; `None` is OpenCode's `removed` value.
    #[serde(default)]
    summary: Option<Summary>,
    /// Legacy concatenated text, kept only to migrate old state files.
    #[serde(default, skip_serializing)]
    text: String,
    /// Legacy blocks, kept only to migrate old state files.
    #[serde(default, skip_serializing)]
    blocks: Vec<String>,
    /// Legacy active flag: `false` meant Code Mode had been removed.
    #[serde(default = "legacy_active", skip_serializing)]
    active: bool,
}

fn legacy_active() -> bool {
    true
}

/// One instruction observation: the epoch baseline for the system prompt and,
/// when the observed catalog changed, the chronological update OpenCode stores
/// as a system message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rendered {
    pub initial: Option<String>,
    pub update: Option<String>,
}

fn state_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn state_path(harness_root: &Path, chat_id: &str) -> PathBuf {
    let digest = Sha256::digest(chat_id.as_bytes());
    let mut name = String::with_capacity(32);
    for byte in &digest[..16] {
        name.push_str(&format!("{byte:02x}"));
    }
    harness_root
        .join(".deep_agents")
        .join("codemode")
        .join("instructions")
        .join(format!("{name}.json"))
}

fn load(path: &Path) -> Option<Stored> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut stored: Stored = serde_json::from_str(&text).ok()?;
    if !stored.active {
        stored.summary = None;
    }
    if stored.initial.is_empty() {
        stored.initial = stored
            .blocks
            .first()
            .cloned()
            .unwrap_or_else(|| stored.text.clone());
    }
    Some(stored)
}

fn save(path: &Path, stored: &Stored) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(stored) {
        let _ = std::fs::write(path, text);
    }
}

fn complete(summary: &Summary) -> bool {
    summary
        .namespaces
        .iter()
        .all(|namespace| namespace.entries.len() == namespace.tools.len())
}

fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn fit(listings: &[Listing], budget: usize) -> Vec<Listing> {
    let mut total = 0usize;
    for (index, entry) in listings.iter().enumerate() {
        total += catalog::cost(&entry.line);
        if total > budget {
            return listings[..index].to_vec();
        }
    }
    listings.to_vec()
}

/// OpenCode `CodeModeInstructions.update`: a delta, a full replacement when
/// the delta is not smaller, or `None` when nothing changed.
pub fn update(
    previous: &Summary,
    current: &Summary,
    lookup: &HashMap<String, Listing>,
) -> Option<String> {
    let replacement = format!(
        "The Code Mode tool catalog has changed. This catalog supersedes the previous Code Mode tool catalog.\n\n{}",
        catalog::render(current)
    );
    if current.total == 0 || previous.total == 0 {
        return Some(replacement);
    }

    // `Instructions.diffByKey` over namespace name + description.
    let current_names: HashSet<&str> = current
        .namespaces
        .iter()
        .map(|namespace| namespace.name.as_str())
        .collect();
    let previous_by_name: HashMap<&str, &NamespaceSummary> = previous
        .namespaces
        .iter()
        .map(|namespace| (namespace.name.as_str(), namespace))
        .collect();
    let added_namespaces: Vec<&NamespaceSummary> = current
        .namespaces
        .iter()
        .filter(|namespace| !previous_by_name.contains_key(namespace.name.as_str()))
        .collect();
    let changed_namespaces: Vec<&NamespaceSummary> = current
        .namespaces
        .iter()
        .filter(|namespace| {
            previous_by_name
                .get(namespace.name.as_str())
                .is_some_and(|before| before.description != namespace.description)
        })
        .collect();
    let removed_namespaces: Vec<&NamespaceSummary> = previous
        .namespaces
        .iter()
        .filter(|namespace| !current_names.contains(namespace.name.as_str()))
        .collect();
    let restated: Vec<&NamespaceSummary> = added_namespaces
        .iter()
        .chain(changed_namespaces.iter())
        .copied()
        .collect();
    let unchanged: HashSet<&str> = current
        .namespaces
        .iter()
        .map(|namespace| namespace.name.as_str())
        .filter(|name| !restated.iter().any(|item| item.name == *name))
        .collect();
    let kept = |path: &str| unchanged.contains(path.split('.').next().unwrap_or(path));
    let before: HashSet<&str> = previous
        .namespaces
        .iter()
        .flat_map(|namespace| namespace.tools.iter().map(String::as_str))
        .collect();
    let after: HashSet<&str> = current
        .namespaces
        .iter()
        .flat_map(|namespace| namespace.tools.iter().map(String::as_str))
        .collect();
    let added: Vec<&str> = current
        .namespaces
        .iter()
        .flat_map(|namespace| namespace.tools.iter().map(String::as_str))
        .filter(|path| kept(path) && !before.contains(path))
        .collect();
    let removed: Vec<&str> = previous
        .namespaces
        .iter()
        .flat_map(|namespace| namespace.tools.iter().map(String::as_str))
        .filter(|path| kept(path) && !after.contains(path))
        .collect();
    let changed: Vec<Listing> = previous
        .namespaces
        .iter()
        .flat_map(|namespace| namespace.entries.iter())
        .filter(|entry| kept(&entry.path) && after.contains(entry.path.as_str()))
        .filter_map(|entry| {
            let listing = lookup.get(&entry.path)?;
            (listing.line != entry.line).then(|| listing.clone())
        })
        .collect();
    let budget = if complete(current) {
        usize::MAX
    } else {
        UPDATE_BUDGET / (restated.len() + usize::from(!added.is_empty())).max(1)
    };

    let mut parts = vec!["The Code Mode tool catalog has changed.".to_string()];
    if !restated.is_empty() {
        let mut lines = vec!["New tools are available:".to_string()];
        for namespace in &restated {
            let listings: Vec<Listing> = namespace
                .tools
                .iter()
                .filter_map(|path| lookup.get(path).cloned())
                .collect();
            let shown = fit(&listings, budget);
            lines.push(catalog::namespace_line(
                &namespace.name,
                namespace.description.as_deref(),
                &namespace.tools,
                shown.len(),
            ));
            lines.extend(shown.into_iter().map(|entry| entry.line));
        }
        parts.push(lines.join("\n"));
    }
    if !added.is_empty() {
        let listings: Vec<Listing> = added
            .iter()
            .filter_map(|path| lookup.get(*path).cloned())
            .collect();
        let shown = fit(&listings, budget);
        let rest = added.len() - shown.len();
        let mut lines =
            vec!["New tools are available in addition to those previously listed:".to_string()];
        lines.extend(shown.into_iter().map(|entry| entry.line));
        if rest > 0 {
            lines.push(format!("  - {rest} more; search for them"));
        }
        parts.push(lines.join("\n"));
    }
    if complete(previous) && !complete(current) {
        parts.push(format!("{SEARCH_NOTE}{}", catalog::SEARCH_SIGNATURE));
    }
    if !changed.is_empty() {
        let mut lines =
            vec!["Changed tool listings supersede the previously listed ones:".to_string()];
        lines.extend(changed.into_iter().map(|entry| entry.line));
        parts.push(lines.join("\n"));
    }
    if !removed.is_empty() {
        parts.push(format!(
            "The following tools are no longer available and must not be called: {}.",
            removed
                .iter()
                .map(|path| super::schema::tool_expression(path))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !removed_namespaces.is_empty() {
        parts.push(format!(
            "The following tool namespaces are no longer available and must not be used: {}.",
            removed_namespaces
                .iter()
                .map(|namespace| format!("`{}`", namespace.name))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if parts.len() == 1 {
        return None;
    }
    let delta = parts.join("\n\n");
    if js_len(&delta) < js_len(&replacement) {
        Some(delta)
    } else {
        Some(replacement)
    }
}

/// Observes the current Code Mode inventory against this chat's durable
/// instruction state. `inventory == None` is OpenCode's `removed` observation:
/// Code Mode is unavailable for the scope or agent.
///
/// The returned `initial` is the frozen epoch baseline that belongs in the
/// system prompt. The returned `update` must be stored once as a chronological
/// system message, exactly like OpenCode's `session.instructions.updated`.
pub fn render_for_chat(
    harness_root: &Path,
    chat_id: &str,
    inventory: Option<&[CatalogItem]>,
) -> Rendered {
    let _guard = state_lock()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let path = state_path(harness_root, chat_id);
    let current =
        inventory.map(|inventory| catalog::summarize(inventory, catalog::INLINE_BUDGET_TOKENS));
    let lookup: HashMap<String, Listing> = inventory
        .map(|inventory| {
            inventory
                .iter()
                .map(|item| (item.path.clone(), catalog::listing_for_item(item)))
                .collect()
        })
        .unwrap_or_default();
    let Some(stored) = load(&path) else {
        let initial = current.as_ref().map(catalog::render);
        save(
            &path,
            &Stored {
                initial: initial.clone().unwrap_or_default(),
                summary: current,
                ..Stored::default()
            },
        );
        return Rendered {
            initial,
            update: None,
        };
    };
    let initial = (!stored.initial.is_empty()).then(|| stored.initial.clone());
    if stored.summary == current {
        return Rendered {
            initial,
            update: None,
        };
    }
    let update = match (&stored.summary, &current) {
        (Some(previous), Some(current)) => update(previous, current, &lookup),
        (Some(_), None) => Some(REMOVED.to_string()),
        (None, Some(current)) => Some(catalog::render(current)),
        (None, None) => None,
    };
    save(
        &path,
        &Stored {
            initial: stored.initial,
            summary: current,
            ..Stored::default()
        },
    );
    Rendered { initial, update }
}

/// The current epoch baseline, for rebuilding a system prompt after a
/// compaction advanced the epoch.
pub fn baseline_for_chat(harness_root: &Path, chat_id: &str) -> Option<String> {
    let _guard = state_lock()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let stored = load(&state_path(harness_root, chat_id))?;
    (!stored.initial.is_empty()).then_some(stored.initial)
}

/// OpenCode `InstructionState.advanceEpoch` after a completed compaction:
/// the current catalog values become the new epoch baseline. History before
/// the compaction is dropped, so updates folded into that history must be
/// restated in the baseline instead of disappearing.
pub fn rebase_for_chat(harness_root: &Path, chat_id: &str) {
    let _guard = state_lock()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let path = state_path(harness_root, chat_id);
    let Some(mut stored) = load(&path) else {
        return;
    };
    stored.initial = stored
        .summary
        .as_ref()
        .map(catalog::render)
        .unwrap_or_default();
    save(&path, &stored);
}

impl Default for Stored {
    fn default() -> Self {
        Self {
            initial: String::new(),
            summary: None,
            text: String::new(),
            blocks: Vec::new(),
            active: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ns(name: &str, tools: &[&str]) -> NamespaceSummary {
        NamespaceSummary {
            name: name.to_string(),
            description: None,
            tools: tools.iter().map(|tool| tool.to_string()).collect(),
            entries: tools
                .iter()
                .map(|tool| Listing {
                    path: tool.to_string(),
                    line: format!("  - tools.{tool}(): Promise<string | null>"),
                })
                .collect(),
        }
    }

    fn summary(namespaces: Vec<NamespaceSummary>) -> Summary {
        Summary {
            total: namespaces
                .iter()
                .map(|namespace| namespace.tools.len())
                .sum(),
            namespaces,
        }
    }

    #[test]
    fn added_changed_and_removed_follow_opencode() {
        let previous = summary(vec![ns("github", &["github.get", "github.old"])]);
        let current = summary(vec![ns("github", &["github.get", "github.new"])]);
        let lookup = HashMap::from([
            (
                "github.get".to_string(),
                Listing {
                    path: "github.get".to_string(),
                    line: "  - tools.github.get({ id: number }): Promise<string | null>"
                        .to_string(),
                },
            ),
            (
                "github.new".to_string(),
                Listing {
                    path: "github.new".to_string(),
                    line: "  - tools.github.new(): Promise<string | null>".to_string(),
                },
            ),
        ]);
        let text = update(&previous, &current, &lookup).expect("update");
        assert!(
            text.contains("New tools are available in addition to"),
            "{text}"
        );
        assert!(text.contains("tools.github.new"), "{text}");
        assert!(text.contains("Changed tool listings supersede"), "{text}");
        assert!(text.contains("no longer available"), "{text}");
        assert!(text.contains("tools.github.old"), "{text}");
    }

    #[test]
    fn update_matches_opencode_source() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("testdata/opencode_instructions.json")).unwrap();
        let line = |path: &str| format!("  - tools.{path}(): Promise<string | null>");
        let github = |entries: &[&str]| NamespaceSummary {
            name: "github".to_string(),
            description: None,
            tools: vec!["github.get".to_string(), "github.new".to_string()],
            entries: entries
                .iter()
                .map(|path| Listing {
                    path: path.to_string(),
                    line: line(path),
                })
                .collect(),
        };
        let read = || NamespaceSummary {
            name: "read".to_string(),
            description: None,
            tools: vec!["read".to_string()],
            entries: vec![Listing {
                path: "read".to_string(),
                line: line("read"),
            }],
        };
        let previous = summary(vec![
            NamespaceSummary {
                name: "github".to_string(),
                description: None,
                tools: vec!["github.get".to_string(), "github.old".to_string()],
                entries: vec![
                    Listing {
                        path: "github.get".to_string(),
                        line: line("github.get"),
                    },
                    Listing {
                        path: "github.old".to_string(),
                        line: line("github.old"),
                    },
                ],
            },
            read(),
        ]);
        let current = summary(vec![github(&["github.get", "github.new"]), read()]);
        let partial = summary(vec![github(&["github.get"]), read()]);
        let namespace_removed = summary(vec![read()]);
        let lookup = HashMap::from([
            (
                "github.get".to_string(),
                Listing {
                    path: "github.get".to_string(),
                    line: "  - tools.github.get({ id: number }): Promise<string | null>"
                        .to_string(),
                },
            ),
            (
                "github.new".to_string(),
                Listing {
                    path: "github.new".to_string(),
                    line: line("github.new"),
                },
            ),
            (
                "read".to_string(),
                Listing {
                    path: "read".to_string(),
                    line: line("read"),
                },
            ),
        ]);
        assert_eq!(
            update(&previous, &current, &lookup).as_deref(),
            fixture["added_removed_changed"].as_str()
        );
        assert_eq!(
            update(&current, &partial, &lookup).as_deref(),
            fixture["partial"].as_str()
        );
        assert_eq!(
            update(&previous, &namespace_removed, &lookup).as_deref(),
            fixture["namespace_removed"].as_str()
        );
        assert_eq!(
            update(&previous, &previous, &lookup).as_deref(),
            fixture["unchanged"].as_str()
        );
    }

    #[test]
    fn empty_catalog_state_does_not_grow() {
        let dir = tempfile::tempdir().unwrap();
        let first = render_for_chat(dir.path(), "empty", Some(&[]));
        assert!(
            first
                .initial
                .as_deref()
                .unwrap()
                .contains("No Code Mode tools")
        );
        assert_eq!(first.update, None);
        assert_eq!(render_for_chat(dir.path(), "empty", Some(&[])), first);
    }

    #[test]
    fn unchanged_catalog_has_no_delta() {
        let summary = summary(vec![ns("read", &["read"])]);
        let lookup = HashMap::new();
        assert_eq!(update(&summary, &summary, &lookup), None);
    }

    #[test]
    fn instruction_state_is_per_chat_and_removed_once() {
        let dir = tempfile::tempdir().unwrap();
        let items = vec![CatalogItem {
            path: "read".to_string(),
            description: "Read".to_string(),
            signature: "tools.read(): Promise<string | null>".to_string(),
            pinned: false,
        }];
        let first = render_for_chat(dir.path(), "chat-a", Some(&items));
        let baseline = first.initial.clone().expect("baseline");
        assert!(baseline.contains("# Code Mode"));
        assert_eq!(first.update, None);
        assert_eq!(render_for_chat(dir.path(), "chat-a", Some(&items)), first);
        let other = render_for_chat(dir.path(), "chat-b", Some(&items));
        assert_eq!(other.initial.as_deref(), Some(baseline.as_str()));

        let mut grown = items.clone();
        grown.push(CatalogItem {
            path: "write".to_string(),
            description: "Write".to_string(),
            signature: "tools.write(): Promise<string | null>".to_string(),
            pinned: false,
        });
        let updated = render_for_chat(dir.path(), "chat-a", Some(&grown));
        assert_eq!(updated.initial.as_deref(), Some(baseline.as_str()));
        let delta = updated.update.expect("update");
        assert!(delta.contains("New tools are available"), "{delta}");
        assert!(delta.contains("tools.write"), "{delta}");
        // The rendered update is delivered once; later observations are quiet.
        assert_eq!(
            render_for_chat(dir.path(), "chat-a", Some(&grown)).update,
            None
        );

        let removed = render_for_chat(dir.path(), "chat-a", None);
        assert_eq!(removed.initial.as_deref(), Some(baseline.as_str()));
        assert!(
            removed
                .update
                .as_deref()
                .unwrap()
                .contains("no longer available")
        );
        assert_eq!(render_for_chat(dir.path(), "chat-a", None).update, None);

        // Re-enabling after removal restates the full catalog as an update,
        // never as the frozen baseline (OpenCode `initial(next)`).
        let reenabled = render_for_chat(dir.path(), "chat-a", Some(&items));
        assert_eq!(reenabled.initial.as_deref(), Some(baseline.as_str()));
        let restated = reenabled.update.expect("restated catalog");
        assert!(!restated.contains("catalog has changed"));
        assert_eq!(restated, baseline);
    }

    #[test]
    fn instruction_sequence_matches_opencode_source() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("testdata/opencode_instruction_sequence.json"))
                .unwrap();
        for scenario in fixture["scenarios"].as_array().unwrap() {
            let name = scenario["name"].as_str().unwrap();
            let dir = tempfile::tempdir().unwrap();
            for (index, step) in scenario["steps"].as_array().unwrap().iter().enumerate() {
                let inventory = step["inventory"].as_array().map(|items| {
                    items
                        .iter()
                        .map(|item| CatalogItem {
                            path: item["path"].as_str().unwrap().to_string(),
                            description: item["description"].as_str().unwrap().to_string(),
                            signature: item["signature"].as_str().unwrap().to_string(),
                            pinned: item["pinned"].as_bool().unwrap(),
                        })
                        .collect::<Vec<_>>()
                });
                let rendered = render_for_chat(dir.path(), name, inventory.as_deref());
                assert_eq!(
                    rendered.initial.as_deref(),
                    step["initial"].as_str(),
                    "{name} step {index} initial"
                );
                assert_eq!(
                    rendered.update.as_deref(),
                    step["update"].as_str(),
                    "{name} step {index} update"
                );
            }
        }
    }

    #[test]
    fn baseline_started_without_code_mode_stays_empty() {
        let dir = tempfile::tempdir().unwrap();
        let items = vec![CatalogItem {
            path: "read".to_string(),
            description: "Read".to_string(),
            signature: "tools.read(): Promise<string | null>".to_string(),
            pinned: false,
        }];
        let off = render_for_chat(dir.path(), "chat", None);
        assert_eq!(off.initial, None);
        assert_eq!(off.update, None);
        let on = render_for_chat(dir.path(), "chat", Some(&items));
        assert_eq!(on.initial, None);
        assert!(on.update.as_deref().unwrap().contains("# Code Mode"));
        assert_eq!(
            render_for_chat(dir.path(), "chat", Some(&items)).update,
            None
        );
    }

    #[test]
    fn compaction_rebases_the_instruction_epoch() {
        let dir = tempfile::tempdir().unwrap();
        let items = vec![CatalogItem {
            path: "read".to_string(),
            description: "Read".to_string(),
            signature: "tools.read(): Promise<string | null>".to_string(),
            pinned: false,
        }];
        let _first = render_for_chat(dir.path(), "compact", Some(&items));
        let mut grown = items.clone();
        grown.push(CatalogItem {
            path: "edit".to_string(),
            description: "Edit".to_string(),
            signature: "tools.edit(): Promise<string | null>".to_string(),
            pinned: false,
        });
        let delta = render_for_chat(dir.path(), "compact", Some(&grown)).update;
        assert!(delta.unwrap().contains("tools.edit"));

        rebase_for_chat(dir.path(), "compact");
        let rebased = render_for_chat(dir.path(), "compact", Some(&grown));
        assert_eq!(rebased.update, None);
        let baseline = rebased.initial.expect("baseline");
        assert!(baseline.contains("tools.edit"), "{baseline}");
        assert!(!baseline.contains("catalog has changed"), "{baseline}");

        // The original tool list is a delta from the new baseline, not a
        // restated baseline.
        let shrunk = render_for_chat(dir.path(), "compact", Some(&items));
        assert_eq!(shrunk.initial.as_deref(), Some(baseline.as_str()));
        let removal = shrunk.update.unwrap();
        assert!(removal.contains("no longer available"), "{removal}");
        assert!(removal.contains("edit"), "{removal}");
    }

    #[test]
    fn legacy_state_migrates_to_a_frozen_baseline() {
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(dir.path(), "legacy");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let items = vec![CatalogItem {
            path: "read".to_string(),
            description: "Read".to_string(),
            signature: "tools.read(): Promise<string | null>".to_string(),
            pinned: false,
        }];
        let cached = catalog::summarize(&items, catalog::INLINE_BUDGET_TOKENS);
        let legacy = serde_json::json!({
            "summary": cached,
            "text": "# Code Mode\n\nold baseline",
            "blocks": ["# Code Mode\n\nold baseline", "The Code Mode tool catalog has changed."],
            "active": true,
        });
        std::fs::write(&path, serde_json::to_string(&legacy).unwrap()).unwrap();
        let rendered = render_for_chat(dir.path(), "legacy", Some(&items));
        assert_eq!(
            rendered.initial.as_deref(),
            Some("# Code Mode\n\nold baseline")
        );
        assert_eq!(rendered.update, None);
    }
}
